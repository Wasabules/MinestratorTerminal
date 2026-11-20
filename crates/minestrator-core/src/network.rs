//! **Réseaux proxy** Minecraft (Velocity, BungeeCord/Waterfall) → serveurs de jeu : outils MCP
//! `network_topology` (lecture, ce module) et `link_proxy` (liaison, voir [`link`]).
//!
//! - **Repérage** : un proxy se reconnaît à la plateforme nommée par l'hébergeur
//!   (`GET /server/{id}/minecraft_version`), au nom de son jar de démarrage, ou à défaut à ses
//!   fichiers (`velocity.toml`, `config.yml` de BungeeCord avec `listeners`). Seuls les serveurs
//!   Minecraft Java actifs sont lus ; les autres (suspendus, expirés) sont cités comme non lus.
//! - **Rapprochement** : chaque entrée du proxy (`[servers]` de Velocity, `servers` de
//!   BungeeCord) est associée à un serveur du compte par son adresse : IP et port principal
//!   (`GET /server/{id}` : `server.ip`, `server.port`), port secondaire (`ports[]`), nom DNS du
//!   serveur (`server.dns`), nom résolu, ou port seul pour une adresse privée. Une adresse locale
//!   (`127.0.0.1`, `localhost`) ne mène à aucun autre serveur : chaque serveur MineStrator a son
//!   conteneur, et deux serveurs d'une même MyBox peuvent tourner sur des machines différentes
//!   (`id_daemon`) ; l'adresse à écrire dans un proxy est l'`ip:port` publique du serveur.
//! - **Cohérence** de chaque serveur relié : `online-mode` (`server.properties`, par l'API),
//!   `proxies.velocity.*` de `config/paper-global.yml` (`settings.velocity-support.*` de
//!   `paper.yml` avant 1.19), `settings.bungeecord` de `spigot.yml`, BungeeGuard (jetons),
//!   FabricProxy-Lite.
//!
//! **Secrets** : le secret de transfert (`forwarding.secret`, `proxies.velocity.secret`) et les
//! jetons BungeeGuard sont lus et comparés **en interne** (identique, différent, absent) ; ils ne
//! sortent jamais. Toute valeur lue est en plus effacée des sorties ([`scrub_value`]), qui passent
//! ensuite par l'anonymisation IA (`redact.rs`).

mod install;
mod link;
mod ping;
mod preset;
#[cfg(test)]
mod tests;

pub(crate) use install::{install_mod_tool, proxy_versions};
pub(crate) use link::link_proxy_tool;
pub(crate) use ping::ping_server_tool;
pub(crate) use preset::network_preset_tool;

use crate::config_edit::Format;
use crate::findings::Severity;
use crate::jar_meta::Platform;
use crate::{sftp, yaml_lite, Core};
use futures_util::{stream, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::time::Duration;

/// Port Minecraft par défaut : celui qu'un proxy utilise pour une adresse sans port.
pub(crate) const DEFAULT_PORT: u16 = 25565;
/// Appels API concurrents (version, démarrage, adresse de chaque serveur).
const API_CONCURRENCY: usize = 6;
/// Sessions SFTP concurrentes (lecture des configs).
const SFTP_CONCURRENCY: usize = 4;
/// Résolution d'un nom d'hôte trouvé dans un proxy.
const DNS_TIMEOUT: Duration = Duration::from_secs(3);
/// Noms d'hôte résolus au plus par appel.
const MAX_DNS_LOOKUPS: usize = 16;
/// Serveurs examinés au plus comme proxys possibles.
const MAX_PROXY_PROBES: usize = 40;
/// Masque d'un secret dans une sortie (même masque que `config_edit`).
pub(crate) const MASK: &str = crate::config_edit::SECRET_MASK;
/// Longueur minimale d'une valeur lue pour être effacée des sorties : en dessous, le
/// remplacement risquerait de toucher du texte ordinaire (les secrets générés en font 32).
const SCRUB_MIN_LEN: usize = 6;

pub(crate) const VELOCITY_TOML: &str = "/velocity.toml";
pub(crate) const BUNGEE_CONFIG: &str = "/config.yml";
pub(crate) const PAPER_GLOBAL: &str = "/config/paper-global.yml";
pub(crate) const PAPER_LEGACY: &str = "/paper.yml";
pub(crate) const SPIGOT_YML: &str = "/spigot.yml";
const FABRIC_PROXY: &str = "/config/FabricProxy-Lite.toml";
const BUNGEEGUARD_CONFIG: &str = "/plugins/BungeeGuard/config.yml";
const BUNGEEGUARD_TOKEN: &str = "/plugins/BungeeGuard/token.yml";
/// Fichier du secret de Velocity quand `forwarding-secret-file` n'est pas écrit.
pub(crate) const DEFAULT_SECRET_FILE: &str = "forwarding.secret";

// --- Types de serveurs --------------------------------------------------------------------------

/// Plateforme d'un serveur, du point de vue d'un réseau proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerType {
    Velocity,
    Bungee,
    /// Paper et ses forks (Purpur, Folia, Pufferfish…) : transfert moderne de Velocity possible.
    Paper,
    /// Spigot/CraftBukkit : transfert legacy (BungeeCord) seulement.
    Spigot,
    Fabric,
    /// Forge, NeoForge, et les hybrides (Mohist, Arclight…).
    Forge,
    Vanilla,
    Unknown,
}

impl ServerType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Velocity => "velocity",
            Self::Bungee => "bungeecord",
            Self::Paper => "paper",
            Self::Spigot => "spigot",
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::Vanilla => "vanilla",
            Self::Unknown => "inconnue",
        }
    }

    pub(crate) fn is_proxy(self) -> bool {
        matches!(self, Self::Velocity | Self::Bungee)
    }

    /// Plateforme nommée par un texte : `source` de l'hébergeur (`paper`, `velocity`…) ou nom du
    /// jar (`velocity-3.4.0-SNAPSHOT-500.jar`). `None` s'il n'en nomme aucune (`explicit_jar`).
    pub(crate) fn from_hint(text: &str) -> Option<Self> {
        let acc = crate::plugin_deps::platforms_from_source(text, None)?;
        if acc.vanilla {
            return Some(Self::Vanilla);
        }
        let has = |p: Platform| acc.platforms.contains(&p);
        Some(if has(Platform::Velocity) {
            Self::Velocity
        } else if has(Platform::Bungee) {
            Self::Bungee
        } else if has(Platform::Forge) || has(Platform::NeoForge) || has(Platform::LegacyForge) {
            Self::Forge
        } else if has(Platform::Fabric) || has(Platform::Quilt) {
            Self::Fabric
        } else if has(Platform::Paper) {
            Self::Paper
        } else if has(Platform::Bukkit) {
            Self::Spigot
        } else {
            return None;
        })
    }

    /// D'après la source de l'hébergeur, sinon le nom du jar de démarrage.
    pub(crate) fn detect(source: Option<&str>, jar: Option<&str>) -> Option<Self> {
        source
            .and_then(Self::from_hint)
            .or_else(|| jar.and_then(Self::from_hint))
    }
}

/// Version de Minecraft antérieure à 1.13 (transfert moderne impossible) ?
pub(crate) fn below_1_13(version: Option<&str>) -> bool {
    version
        .and_then(crate::jar_meta::version_nums)
        .is_some_and(|n| crate::updates::compare_numbers(&n, &[1, 13]) == std::cmp::Ordering::Less)
}

// --- Configuration d'un proxy -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyKind {
    Velocity,
    Bungee,
}

impl ProxyKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Velocity => "velocity",
            Self::Bungee => "bungeecord",
        }
    }
}

/// Transfert des joueurs (IP, UUID, skin) du proxy vers les serveurs de jeu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Forwarding {
    None,
    /// BungeeCord (`ip_forward: true`) ou Velocity `legacy` : sans signature.
    Legacy,
    /// Velocity `bungeeguard` : legacy + jeton (le secret de transfert).
    BungeeGuard,
    /// Velocity `modern` : signé avec le secret partagé.
    Modern,
    Other(String),
}

impl Forwarding {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" => Self::None,
            "legacy" => Self::Legacy,
            "bungeeguard" => Self::BungeeGuard,
            "modern" => Self::Modern,
            other => Self::Other(other.to_string()),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::None => "aucun (none)".into(),
            Self::Legacy => "legacy (BungeeCord)".into(),
            Self::BungeeGuard => "bungeeguard".into(),
            Self::Modern => "modern (Velocity)".into(),
            Self::Other(s) => format!("inconnu ({s})"),
        }
    }
}

/// Une entrée du proxy : nom et adresse telle qu'écrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub address: String,
}

/// Ce qui compte d'une configuration de proxy. Le secret éventuellement écrit dans
/// `velocity.toml` (anciennes versions) reste interne : jamais rendu.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProxyConf {
    pub kind: ProxyKind,
    pub entries: Vec<Entry>,
    /// Serveurs de repli dans l'ordre (`try` de Velocity, `priorities` du premier listener).
    pub try_order: Vec<String>,
    /// Hôte forcé → serveurs.
    pub forced_hosts: Vec<(String, Vec<String>)>,
    /// Hôtes forcés du premier listener (les premiers de `forced_hosts`) : BungeeCord en a un jeu
    /// par listener, `link_proxy` n'écrit que dans le premier. Velocity : tous.
    pub first_listener_hosts: usize,
    pub forwarding: Forwarding,
    /// Valeur brute de `player-info-forwarding-mode` (Velocity) : sa casse est gardée à l'écriture.
    pub forwarding_raw: Option<String>,
    pub online_mode: Option<bool>,
    /// Velocity : `forwarding-secret-file`, tel qu'écrit.
    pub secret_file: Option<String>,
    /// Velocity d'avant 3.1.2 : `forwarding-secret` écrit dans `velocity.toml`.
    pub inline_secret: Option<String>,
    /// BungeeCord : nombre de listeners.
    pub listeners: usize,
}

impl ProxyConf {
    fn new(kind: ProxyKind) -> Self {
        Self {
            kind,
            entries: Vec::new(),
            try_order: Vec::new(),
            forced_hosts: Vec::new(),
            first_listener_hosts: 0,
            forwarding: Forwarding::None,
            forwarding_raw: None,
            online_mode: None,
            secret_file: None,
            inline_secret: None,
            listeners: 0,
        }
    }

    /// Entrée d'après son nom (insensible à la casse, comme Velocity et BungeeCord).
    pub(crate) fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Fichier du secret de transfert (Velocity), chemin SFTP. `None` : secret écrit dans
    /// `velocity.toml` (ancien format), ou chemin refusé.
    pub(crate) fn secret_path(&self) -> Option<String> {
        if self.kind != ProxyKind::Velocity
            || (self.secret_file.is_none() && self.inline_secret.is_some())
        {
            return None;
        }
        proxy_file_path(self.secret_file.as_deref().unwrap_or(DEFAULT_SECRET_FILE))
    }
}

/// Chemin SFTP d'un fichier désigné dans la config du proxy (relatif à sa racine, ou absolu dans
/// le conteneur `/home/container`). `..` refusé.
pub(crate) fn proxy_file_path(raw: &str) -> Option<String> {
    let r = raw.trim().replace('\\', "/");
    let r = r.strip_prefix("/home/container").unwrap_or(&r);
    let r = r.trim_start_matches("./");
    if r.is_empty() {
        return None;
    }
    let p = if r.starts_with('/') {
        r.to_string()
    } else {
        format!("/{r}")
    };
    crate::fleet::normalize_path(&p).ok()
}

/// Arbre d'un YAML : analyseur strict, sinon tolérant (fichier écrit à la main).
pub(crate) fn yaml_tree(text: &str) -> Value {
    Format::Yaml
        .tree(text)
        .unwrap_or_else(|_| yaml_lite::parse(text))
}

/// Texte non vide d'un scalaire.
fn text_of(v: Option<&Value>) -> Option<String> {
    yaml_lite::as_text(v)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `velocity.toml`.
pub(crate) fn parse_velocity(text: &str) -> Result<ProxyConf, String> {
    let tree = Format::Toml
        .tree(text)
        .map_err(|e| format!("velocity.toml illisible : {e}"))?;
    let mut c = ProxyConf::new(ProxyKind::Velocity);
    if let Some(Value::Object(m)) = tree.get("servers") {
        for (k, v) in m {
            if k == "try" {
                c.try_order = yaml_lite::as_list(Some(v));
            } else if let Some(a) = v.as_str() {
                c.entries.push(Entry {
                    name: k.clone(),
                    address: a.trim().to_string(),
                });
            }
        }
    }
    if let Some(Value::Object(m)) = tree.get("forced-hosts") {
        c.forced_hosts = m
            .iter()
            .map(|(h, v)| (h.clone(), yaml_lite::as_list(Some(v))))
            .collect();
        c.first_listener_hosts = c.forced_hosts.len();
    }
    c.forwarding_raw = tree
        .get("player-info-forwarding-mode")
        .and_then(Value::as_str)
        .map(str::to_string);
    c.forwarding = c
        .forwarding_raw
        .as_deref()
        .map(Forwarding::parse)
        .unwrap_or(Forwarding::None);
    c.online_mode = tree.get("online-mode").and_then(Value::as_bool);
    c.secret_file = text_of(tree.get("forwarding-secret-file"));
    c.inline_secret = text_of(tree.get("forwarding-secret"));
    Ok(c)
}

/// `config.yml` de BungeeCord/Waterfall. Erreur si ce n'en est pas un (pas de `listeners`).
pub(crate) fn parse_bungee(text: &str) -> Result<ProxyConf, String> {
    let tree = yaml_tree(text);
    let Some(listeners) = tree.get("listeners").and_then(Value::as_array) else {
        return Err(
            "config.yml sans `listeners` : ce n'est pas une configuration BungeeCord".into(),
        );
    };
    let mut c = ProxyConf::new(ProxyKind::Bungee);
    c.listeners = listeners.len();
    if let Some(Value::Object(m)) = tree.get("servers") {
        for (name, v) in m {
            if let Some(a) = text_of(v.get("address")) {
                c.entries.push(Entry {
                    name: name.clone(),
                    address: a,
                });
            }
        }
    }
    if let Some(first) = listeners.first() {
        c.try_order = yaml_lite::as_list(first.get("priorities"));
    }
    for (i, l) in listeners.iter().enumerate() {
        if let Some(Value::Object(m)) = l.get("forced_hosts") {
            for (h, v) in m {
                c.forced_hosts
                    .push((h.clone(), yaml_lite::as_list(Some(v))));
            }
        }
        if i == 0 {
            c.first_listener_hosts = c.forced_hosts.len();
        }
    }
    c.forwarding = if yaml_lite::as_bool(tree.get("ip_forward")) == Some(true) {
        Forwarding::Legacy
    } else {
        Forwarding::None
    };
    c.online_mode = yaml_lite::as_bool(tree.get("online_mode"));
    Ok(c)
}

// --- Adresses ------------------------------------------------------------------------------------

/// Adresse d'une entrée de proxy : hôte (en minuscules) et port s'il est écrit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Addr {
    pub host: String,
    pub port: Option<u16>,
}

impl Addr {
    pub(crate) fn parse(raw: &str) -> Self {
        let s = raw.trim();
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            match rest.split_once(']') {
                Some((h, tail)) => (
                    h,
                    tail.strip_prefix(':').and_then(|p| p.trim().parse().ok()),
                ),
                None => (rest, None),
            }
        } else if s.matches(':').count() == 1 {
            let (h, p) = s.split_once(':').unwrap_or((s, ""));
            (h, p.trim().parse().ok())
        } else {
            (s, None)
        };
        Addr {
            host: host.trim().trim_end_matches('.').to_ascii_lowercase(),
            port,
        }
    }

    pub(crate) fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }

    fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
}

/// Nature d'un hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostClass {
    /// Boucle locale ou adresse non spécifiée : le conteneur du proxy lui-même.
    Local,
    /// Réseau privé (RFC 1918, CGNAT, lien local, ULA).
    Private,
    Public,
    /// Nom d'hôte.
    Name,
}

pub(crate) fn classify(host: &str) -> HostClass {
    if host.eq_ignore_ascii_case("localhost") {
        return HostClass::Local;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            let o = v4.octets();
            if v4.is_loopback() || v4.is_unspecified() {
                HostClass::Local
            } else if v4.is_private() || v4.is_link_local() || (o[0] == 100 && (o[1] & 0xC0) == 64)
            {
                HostClass::Private
            } else {
                HostClass::Public
            }
        }
        Ok(IpAddr::V6(v6)) => {
            let first = v6.segments()[0];
            if v6.is_loopback() || v6.is_unspecified() {
                HostClass::Local
            } else if (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 {
                HostClass::Private
            } else {
                HostClass::Public
            }
        }
        Err(_) => HostClass::Name,
    }
}

/// Adresses d'un serveur du compte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub server_id: i64,
    /// IP du port principal.
    pub ip: String,
    pub port: u16,
    /// Nom DNS du serveur (`<nom>.minesr.com`…), en minuscules.
    pub dns: Option<String>,
    /// Ports secondaires `(ip, port)`.
    pub secondary: Vec<(String, u16)>,
}

impl Endpoint {
    /// D'après `GET /server/{id}` : `server.ip`, `server.port`, `server.dns`, `ports[]`.
    pub(crate) fn from_server_value(id: i64, v: &Value) -> Option<Self> {
        let s = v.get("server")?;
        let ip = s.get("ip")?.as_str()?.trim().to_string();
        let port = u16::try_from(s.get("port")?.as_i64()?).ok()?;
        if ip.is_empty() || port == 0 {
            return None;
        }
        let dns = s
            .get("dns")
            .and_then(Value::as_str)
            .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|d| !d.is_empty());
        let secondary = v
            .get("ports")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let port = u16::try_from(p.get("port")?.as_i64()?).ok()?;
                        let pip = p
                            .get("ip")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        Some((pip.unwrap_or(&ip).to_string(), port))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            server_id: id,
            ip,
            port,
            dns,
            secondary,
        })
    }

    /// Repli : l'adresse de la liste des serveurs quand c'est `ip:port` (sinon c'est un nom DNS
    /// sans port, inutilisable).
    pub(crate) fn from_list_address(id: i64, address: &str) -> Option<Self> {
        let a = Addr::parse(address);
        let port = a.port?;
        a.ip()?;
        Some(Self {
            server_id: id,
            ip: a.host,
            port,
            dns: None,
            secondary: Vec::new(),
        })
    }

    /// `ip:port` à écrire dans un proxy.
    pub(crate) fn address(&self) -> String {
        if self.ip.contains(':') {
            format!("[{}]:{}", self.ip, self.port)
        } else {
            format!("{}:{}", self.ip, self.port)
        }
    }

    fn has_ip(&self, ip: &IpAddr) -> bool {
        self.ip.parse::<IpAddr>().is_ok_and(|x| &x == ip)
    }

    fn secondary_has(&self, ip: &IpAddr, port: u16) -> bool {
        self.secondary
            .iter()
            .any(|(i, p)| *p == port && i.parse::<IpAddr>().is_ok_and(|x| &x == ip))
    }

    /// Joignable directement depuis Internet (IP publique) ?
    pub(crate) fn public(&self) -> bool {
        classify(&self.ip) == HostClass::Public
    }
}

/// Comment une entrée a été rapprochée d'un serveur du compte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum How {
    Primary,
    Secondary,
    Dns,
    Resolved,
    PortOnly,
}

impl How {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Primary => "adresse principale (ip:port)",
            Self::Secondary => "port secondaire du serveur",
            Self::Dns => "nom DNS du serveur",
            Self::Resolved => "nom d'hôte résolu vers l'adresse du serveur",
            Self::PortOnly => "port seul (adresse privée) : à confirmer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Matched {
    pub server_id: i64,
    pub how: How,
    /// Le port de l'entrée est celui du serveur (faux : nom DNS sans port ou autre port).
    pub port_ok: bool,
}

/// Serveur du compte visé par une adresse de proxy. `resolved` : IP du nom d'hôte, s'il en est un.
pub(crate) fn match_entry(addr: &Addr, resolved: &[IpAddr], eps: &[Endpoint]) -> Option<Matched> {
    let port = addr.port_or_default();
    let by_ip = |ip: &IpAddr, how: How| {
        eps.iter()
            .find(|e| e.port == port && e.has_ip(ip))
            .map(|e| Matched {
                server_id: e.server_id,
                how,
                port_ok: true,
            })
            .or_else(|| {
                eps.iter()
                    .find(|e| e.secondary_has(ip, port))
                    .map(|e| Matched {
                        server_id: e.server_id,
                        how: if how == How::Primary {
                            How::Secondary
                        } else {
                            how
                        },
                        port_ok: true,
                    })
            })
    };
    match classify(&addr.host) {
        HostClass::Local => None,
        HostClass::Public => by_ip(&addr.ip()?, How::Primary),
        HostClass::Private => by_ip(&addr.ip()?, How::Primary).or_else(|| {
            let mut same_port = eps.iter().filter(|e| e.port == port);
            match (same_port.next(), same_port.next()) {
                (Some(e), None) => Some(Matched {
                    server_id: e.server_id,
                    how: How::PortOnly,
                    port_ok: true,
                }),
                _ => None,
            }
        }),
        HostClass::Name => {
            if let Some(e) = eps
                .iter()
                .find(|e| e.dns.as_deref() == Some(addr.host.as_str()))
            {
                return Some(Matched {
                    server_id: e.server_id,
                    how: How::Dns,
                    port_ok: port == e.port,
                });
            }
            resolved.iter().find_map(|ip| by_ip(ip, How::Resolved))
        }
    }
}

// --- État d'un serveur de jeu -------------------------------------------------------------------

/// Fichier de réglages de Paper trouvé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PaperFile {
    /// `config/paper-global.yml` (Paper 1.19 et plus).
    Global,
    /// `paper.yml` (Paper 1.13 à 1.18).
    Legacy,
    #[default]
    Missing,
}

/// Fichiers lus sur un serveur de jeu (`None` : absent).
#[derive(Debug, Clone, Default)]
pub(crate) struct BackendFiles {
    pub paper_global: Option<String>,
    pub paper_legacy: Option<String>,
    pub spigot: Option<String>,
    pub fabric_proxy: Option<String>,
    pub bungeeguard_config: Option<String>,
    /// Noms des fichiers de `/plugins` et `/mods`.
    pub plugins: Vec<String>,
    pub mods: Vec<String>,
}

/// Réglages d'un serveur de jeu qui comptent derrière un proxy. Secrets et jetons : internes.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct BackendState {
    pub ty: Option<ServerType>,
    pub version: Option<String>,
    pub online_mode: Option<bool>,
    pub paper: PaperFile,
    pub velocity_enabled: Option<bool>,
    pub velocity_online_mode: Option<bool>,
    pub velocity_secret: Option<String>,
    pub bungee_online_mode: Option<bool>,
    pub spigot_file: bool,
    pub bungeecord: Option<bool>,
    /// Plugin BungeeGuard présent dans `/plugins`.
    pub bungeeguard: bool,
    pub bungeeguard_tokens: Vec<String>,
    /// FabricProxy-Lite : mod présent, config présente, secret de sa config.
    pub fabric_proxy_mod: bool,
    pub fabric_proxy_config: bool,
    pub fabric_proxy_secret: Option<String>,
}

impl BackendState {
    pub(crate) fn ty(&self) -> ServerType {
        self.ty.unwrap_or(ServerType::Unknown)
    }

    /// Secrets et jetons lus : à effacer des sorties.
    fn secrets(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .velocity_secret
            .iter()
            .chain(&self.fabric_proxy_secret)
            .cloned()
            .collect();
        v.extend(self.bungeeguard_tokens.iter().cloned());
        v
    }
}

/// Nom compact (minuscules, alphanumérique) d'un fichier, pour reconnaître un plugin ou un mod.
fn compact(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// État d'un serveur de jeu (pur). `ty` : plateforme repérée, sinon déduite des fichiers.
pub(crate) fn backend_state(
    ty: Option<ServerType>,
    version: Option<String>,
    online_mode: Option<bool>,
    f: &BackendFiles,
) -> BackendState {
    use yaml_lite::{as_bool, path};
    let mut s = BackendState {
        ty,
        version,
        online_mode,
        ..Default::default()
    };
    if let Some(t) = &f.paper_global {
        let v = yaml_tree(t);
        s.paper = PaperFile::Global;
        s.velocity_enabled = as_bool(path(&v, &["proxies", "velocity", "enabled"]));
        s.velocity_online_mode = as_bool(path(&v, &["proxies", "velocity", "online-mode"]));
        s.velocity_secret = text_of(path(&v, &["proxies", "velocity", "secret"]));
        s.bungee_online_mode = as_bool(path(&v, &["proxies", "bungee-cord", "online-mode"]));
    } else if let Some(t) = &f.paper_legacy {
        let v = yaml_tree(t);
        s.paper = PaperFile::Legacy;
        s.velocity_enabled = as_bool(path(&v, &["settings", "velocity-support", "enabled"]));
        s.velocity_online_mode =
            as_bool(path(&v, &["settings", "velocity-support", "online-mode"]));
        s.velocity_secret = text_of(path(&v, &["settings", "velocity-support", "secret"]));
        s.bungee_online_mode = as_bool(path(&v, &["settings", "bungee-online-mode"]));
    }
    if let Some(t) = &f.spigot {
        s.spigot_file = true;
        s.bungeecord = as_bool(path(&yaml_tree(t), &["settings", "bungeecord"]));
    }
    s.bungeeguard = f.plugins.iter().any(|p| compact(p).contains("bungeeguard"));
    if let Some(t) = &f.bungeeguard_config {
        s.bungeeguard_tokens = yaml_lite::as_list(path(&yaml_tree(t), &["allowed-tokens"]));
    }
    s.fabric_proxy_mod = f.mods.iter().any(|m| compact(m).contains("fabricproxy"));
    if let Some(t) = &f.fabric_proxy {
        s.fabric_proxy_config = true;
        s.fabric_proxy_secret = Format::Toml
            .tree(t)
            .ok()
            .and_then(|v| text_of(v.get("secret")));
    }
    if s.ty.is_none() {
        s.ty = if s.paper != PaperFile::Missing {
            Some(ServerType::Paper)
        } else if s.spigot_file {
            Some(ServerType::Spigot)
        } else if s.fabric_proxy_config || s.fabric_proxy_mod {
            Some(ServerType::Fabric)
        } else {
            None
        };
    }
    s
}

/// Comparaison de deux secrets, sans jamais les montrer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretMatch {
    Same,
    Different,
    /// Absent (ou vide) côté serveur de jeu.
    Missing,
    /// Secret du proxy inconnu : rien à comparer.
    Unknown,
}

impl SecretMatch {
    pub(crate) fn compare(proxy: Option<&str>, backend: Option<&str>) -> Self {
        match (proxy, backend) {
            (_, None) => Self::Missing,
            (None, Some(_)) => Self::Unknown,
            (Some(p), Some(b)) if p == b => Self::Same,
            _ => Self::Different,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Same => "identique au proxy",
            Self::Different => "DIFFÉRENT de celui du proxy",
            Self::Missing => "absent",
            Self::Unknown => "non comparé (secret du proxy illisible)",
        }
    }
}

// --- Proxy lu ------------------------------------------------------------------------------------

/// Un proxy du compte et ce qui en a été lu. Secret et jeton : internes, jamais rendus.
#[derive(Debug, Clone)]
pub(crate) struct ProxyState {
    pub server_id: i64,
    pub name: String,
    pub conf: ProxyConf,
    pub config_path: &'static str,
    /// Secret de transfert (Velocity) : fichier, sinon `velocity.toml`.
    pub secret: Option<String>,
    /// D'où vient le secret, ou pourquoi il manque (sans valeur).
    pub secret_note: String,
    /// BungeeCord : plugin BungeeGuard présent, et son jeton.
    pub bungeeguard: bool,
    pub bungeeguard_token: Option<String>,
}

impl ProxyState {
    fn secrets(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .secret
            .iter()
            .chain(&self.bungeeguard_token)
            .cloned()
            .collect();
        v.extend(self.conf.inline_secret.iter().cloned());
        v
    }
}

/// Une entrée du proxy, rapprochée d'un serveur du compte et de ses réglages.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub entry: Entry,
    pub matched: Option<Matched>,
    pub server_name: Option<String>,
    /// Adresse principale du serveur rapproché.
    pub server_address: Option<String>,
    /// Joignable directement (IP publique).
    pub reachable: bool,
    /// Réglages du serveur rapproché (`Err` : non lus, et pourquoi).
    pub backend: Option<Result<BackendState, String>>,
}

// --- Évaluation (pure) ---------------------------------------------------------------------------

/// Un problème du réseau, rattaché au serveur de jeu concerné s'il y en a un.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Problem {
    pub severity: Severity,
    pub server_id: Option<i64>,
    pub message: String,
    pub advice: String,
}

fn problem(
    severity: Severity,
    server_id: Option<i64>,
    message: impl Into<String>,
    advice: impl Into<String>,
) -> Problem {
    Problem {
        severity,
        server_id,
        message: message.into(),
        advice: advice.into(),
    }
}

/// Problèmes d'un proxy et de ses serveurs de jeu (pur).
pub(crate) fn evaluate(p: &ProxyState, links: &[Link]) -> Vec<Problem> {
    use Severity::{Critical, Important, Info};
    let mut out = Vec::new();
    let c = &p.conf;
    let velocity = c.kind == ProxyKind::Velocity;
    let pname = &p.name;
    let linked = links
        .iter()
        .any(|l| l.matched.is_some_and(|m| m.server_id != p.server_id));

    if c.online_mode == Some(false) {
        out.push(problem(
            Critical,
            None,
            format!("Proxy « {pname} » en online-mode=false : il n'authentifie pas les comptes, n'importe qui peut se connecter sous n'importe quel pseudo sur tout le réseau."),
            "Remets online-mode à true sur le proxy, sauf réseau volontairement hors ligne protégé par un plugin d'authentification.",
        ));
    }
    match &c.forwarding {
        Forwarding::None if linked => out.push(problem(
            Important,
            None,
            if velocity {
                format!("Proxy « {pname} » sans transfert des joueurs (player-info-forwarding-mode = none) : les serveurs de jeu voient l'IP du proxy et des UUID hors ligne (inventaires et permissions perdus).")
            } else {
                format!("Proxy « {pname} » sans transfert des joueurs (ip_forward: false) : les serveurs de jeu voient l'IP du proxy et des UUID hors ligne.")
            },
            if velocity {
                "link_proxy (mode modern) règle le proxy et les serveurs de jeu."
            } else {
                "link_proxy (mode legacy) règle le proxy et les serveurs de jeu ; ajoute BungeeGuard."
            },
        )),
        Forwarding::Other(m) => out.push(problem(
            Important,
            None,
            format!("Proxy « {pname} » : mode de transfert inconnu « {m} »."),
            "Valeurs reconnues par Velocity : none, legacy, bungeeguard, modern.",
        )),
        _ => {}
    }
    let needs_secret =
        velocity && matches!(c.forwarding, Forwarding::Modern | Forwarding::BungeeGuard);
    if needs_secret && p.secret.is_none() {
        out.push(problem(
            Critical,
            None,
            format!(
                "Proxy « {pname} » en transfert {} sans secret ({}) : Velocity refuse de démarrer.",
                c.forwarding.label(),
                p.secret_note
            ),
            "link_proxy génère un secret et l'écrit sur le proxy et sur chaque serveur de jeu.",
        ));
    }

    // Serveurs de repli et hôtes forcés : un nom non déclaré empêche Velocity de démarrer.
    let undeclared = if velocity { Critical } else { Important };
    let stop = if velocity {
        " : Velocity refuse de démarrer"
    } else {
        ""
    };
    if c.try_order.is_empty() && !c.entries.is_empty() {
        out.push(problem(
            Important,
            None,
            format!("Proxy « {pname} » sans serveur de repli ({}) : un joueur qui se connecte n'a nulle part où aller.", if velocity { "servers.try" } else { "priorities" }),
            "link_proxy fixe la liste (argument `try`).",
        ));
    }
    for t in &c.try_order {
        if c.entry(t).is_none() {
            out.push(problem(
                undeclared,
                None,
                format!("Proxy « {pname} » : le serveur de repli « {t} » n'est pas déclaré{stop}."),
                "Déclare-le (link_proxy) ou retire-le de la liste de repli.",
            ));
        }
    }
    for (host, targets) in &c.forced_hosts {
        for t in targets.iter().filter(|t| c.entry(t).is_none()) {
            out.push(problem(
                undeclared,
                None,
                format!("Proxy « {pname} » : l'hôte forcé « {host} » renvoie vers « {t} », non déclaré{stop}."),
                "Corrige ou retire cet hôte forcé (forced-hosts / forced_hosts).",
            ));
        }
    }

    let mut seen: HashMap<i64, &str> = HashMap::new();
    for l in links {
        let name = &l.entry.name;
        let addr = Addr::parse(&l.entry.address);
        let in_try = c.try_order.iter().any(|t| t.eq_ignore_ascii_case(name));
        let Some(m) = l.matched else {
            if classify(&addr.host) == HostClass::Local {
                out.push(problem(
                    if in_try { Important } else { Info },
                    None,
                    format!(
                        "« {name} » pointe vers {} : chaque serveur MineStrator a son propre conteneur, cette adresse locale ne mène à aucun autre serveur (exemple de la configuration par défaut ?){}.",
                        l.entry.address,
                        if in_try { " ; c'est un serveur de repli" } else { "" }
                    ),
                    "Remplace-la par l'ip:port du serveur visé (link_proxy), ou retire l'entrée.",
                ));
            } else {
                out.push(problem(
                    Info,
                    None,
                    format!("« {name} » ({}) ne correspond à aucun serveur de ce compte (serveur externe, adresse périmée ?).", l.entry.address),
                    "Vérifie l'adresse (list_servers) ; link_proxy écrit l'ip:port exacte d'un serveur du compte.",
                ));
            }
            continue;
        };
        let who = l
            .server_name
            .clone()
            .unwrap_or_else(|| m.server_id.to_string());
        if m.server_id == p.server_id {
            out.push(problem(
                Important,
                None,
                format!("« {name} » pointe vers le proxy « {pname} » lui-même."),
                "Retire cette entrée.",
            ));
            continue;
        }
        if let Some(prev) = seen.insert(m.server_id, name) {
            out.push(problem(
                Info,
                Some(m.server_id),
                format!("« {prev} » et « {name} » pointent vers le même serveur « {who} »."),
                "",
            ));
        }
        if !m.port_ok {
            out.push(problem(
                Important,
                Some(m.server_id),
                format!(
                    "« {name} » ({}) vise « {who} » par son nom mais avec le port {} : le serveur écoute sur {}.",
                    l.entry.address,
                    addr.port_or_default(),
                    l.server_address.as_deref().unwrap_or("un autre port")
                ),
                "Écris l'ip:port exacte (link_proxy le fait).",
            ));
        }
        if m.how == How::Secondary {
            out.push(problem(
                Important,
                Some(m.server_id),
                format!(
                    "« {name} » ({}) vise un port secondaire de « {who} » : le serveur de jeu écoute d'ordinaire sur son port principal ({}).",
                    l.entry.address,
                    l.server_address.as_deref().unwrap_or("?")
                ),
                "Écris l'ip:port principale (link_proxy le fait), sauf si server-port a été changé exprès.",
            ));
        }
        if m.how == How::PortOnly {
            out.push(problem(
                Info,
                Some(m.server_id),
                format!("« {name} » ({}) n'a été rapproché de « {who} » que par son port.", l.entry.address),
                "Vérifie qu'il s'agit bien de ce serveur ; une adresse ip:port publique évite l'ambiguïté.",
            ));
        }
        match &l.backend {
            Some(Err(e)) => out.push(problem(
                Info,
                Some(m.server_id),
                format!("« {who} » non lu ({e}) : cohérence non vérifiée."),
                "",
            )),
            Some(Ok(b)) => check_backend(p, l, m.server_id, &who, b, &mut out),
            None => {}
        }
    }
    out
}

/// Cohérence d'un serveur de jeu avec le transfert du proxy.
fn check_backend(
    p: &ProxyState,
    l: &Link,
    id: i64,
    who: &str,
    b: &BackendState,
    out: &mut Vec<Problem>,
) {
    use Severity::{Critical, Important, Info};
    let c = &p.conf;
    let velocity = c.kind == ProxyKind::Velocity;
    let modern = velocity && c.forwarding == Forwarding::Modern;
    let guard = velocity && c.forwarding == Forwarding::BungeeGuard;
    let legacy = c.forwarding == Forwarding::Legacy;
    let proxy_online = c.online_mode.unwrap_or(true);
    let direct = l
        .server_address
        .as_deref()
        .filter(|_| l.reachable)
        .map(|a| format!(" ({a})"))
        .unwrap_or_default();
    let mut push = |s: Severity, m: String, a: &str| out.push(problem(s, Some(id), m, a));

    if b.ty().is_proxy() {
        push(
            Info,
            format!("« {who} » est lui-même un proxy (chaîne de proxys) : non vérifié."),
            "",
        );
        return;
    }
    if !(modern || guard || legacy) {
        return;
    }
    if b.online_mode == Some(true) {
        push(
            Important,
            format!(
                "online-mode=true sur « {who} » derrière le proxy : les joueurs qui passent par le proxy sont refusés."
            ),
            "Mets online-mode=false sur ce serveur (l'authentification se fait sur le proxy) ; link_proxy le fait.",
        );
    }
    let offline = b.online_mode == Some(false);
    if modern {
        match b.ty() {
            ServerType::Paper => {
                if below_1_13(b.version.as_deref()) {
                    push(
                        Critical,
                        format!(
                            "« {who} » est en Minecraft {} : le transfert moderne de Velocity exige 1.13 ou plus.",
                            b.version.as_deref().unwrap_or("?")
                        ),
                        "Passe le proxy en legacy avec BungeeGuard, ou mets le serveur à jour.",
                    );
                    return;
                }
                if b.paper == PaperFile::Missing {
                    push(
                        Important,
                        format!(
                            "« {who} » : réglages de Paper absents (config/paper-global.yml) — serveur jamais démarré ? Transfert non vérifié."
                        ),
                        "Démarre le serveur une fois pour générer sa configuration, puis link_proxy.",
                    );
                } else if b.velocity_enabled != Some(true) {
                    if offline {
                        push(
                            Critical,
                            format!(
                                "« {who} » : transfert Velocity désactivé (proxies.velocity.enabled) avec online-mode=false : les joueurs du proxy sont refusés, et quiconque joint directement le serveur{direct} peut prendre n'importe quel pseudo."
                            ),
                            "link_proxy active le transfert moderne et écrit le secret.",
                        );
                    } else {
                        push(
                            Important,
                            format!(
                                "« {who} » : transfert Velocity désactivé (proxies.velocity.enabled) : les joueurs du proxy sont refusés."
                            ),
                            "link_proxy active le transfert moderne et écrit le secret.",
                        );
                    }
                } else {
                    match SecretMatch::compare(p.secret.as_deref(), b.velocity_secret.as_deref()) {
                        SecretMatch::Same => {}
                        SecretMatch::Different => push(
                            Critical,
                            format!(
                                "« {who} » : secret de transfert différent de celui du proxy : connexions refusées."
                            ),
                            "link_proxy recopie le secret du proxy (sans jamais l'afficher).",
                        ),
                        SecretMatch::Missing => push(
                            Critical,
                            format!(
                                "« {who} » : secret de transfert absent (proxies.velocity.secret) : connexions refusées."
                            ),
                            "link_proxy recopie le secret du proxy (sans jamais l'afficher).",
                        ),
                        SecretMatch::Unknown => push(
                            Info,
                            format!("« {who} » : secret non comparé (secret du proxy illisible)."),
                            "",
                        ),
                    }
                    if b.velocity_online_mode.is_some_and(|v| v != proxy_online) {
                        push(
                            Info,
                            format!(
                                "« {who} » : proxies.velocity.online-mode ({}) diffère de l'online-mode du proxy ({proxy_online}).",
                                b.velocity_online_mode.unwrap_or_default()
                            ),
                            "Aligne-le sur le proxy (link_proxy le fait).",
                        );
                    }
                }
                if b.bungeecord == Some(true) {
                    push(
                        Important,
                        format!(
                            "« {who} » : settings.bungeecord: true (spigot.yml) alors que le proxy utilise le transfert moderne."
                        ),
                        "Mets-le à false (link_proxy le fait).",
                    );
                }
            }
            ServerType::Spigot => push(
                Critical,
                format!(
                    "« {who} » est un serveur Spigot : il ne gère pas le transfert moderne de Velocity, les joueurs du proxy seront refusés."
                ),
                "Passe-le sur Paper (ou un fork), ou le proxy en legacy avec BungeeGuard.",
            ),
            ServerType::Fabric => {
                if !b.fabric_proxy_config {
                    push(
                        Important,
                        format!(
                            "« {who} » (Fabric) : FabricProxy-Lite absent ou jamais démarré (config/FabricProxy-Lite.toml) : le transfert moderne n'est pas compris."
                        ),
                        "Installe FabricProxy-Lite et règle son secret à la main (manual).",
                    );
                } else {
                    match SecretMatch::compare(
                        p.secret.as_deref(),
                        b.fabric_proxy_secret.as_deref(),
                    ) {
                        SecretMatch::Same => {}
                        SecretMatch::Unknown => push(
                            Info,
                            format!(
                                "« {who} » : secret FabricProxy-Lite non comparé (secret du proxy illisible)."
                            ),
                            "",
                        ),
                        m => push(
                            Critical,
                            format!(
                                "« {who} » : secret de FabricProxy-Lite {} : connexions refusées.",
                                m.label()
                            ),
                            "Recopie le contenu de forwarding.secret du proxy dans config/FabricProxy-Lite.toml (clé secret), à la main.",
                        ),
                    }
                }
            }
            ServerType::Forge => push(
                Info,
                format!(
                    "« {who} » (Forge/NeoForge) : un mod de transfert est nécessaire derrière Velocity (Proxy-Compatible-Forge…) ; non vérifié."
                ),
                "Vérifie à la main la configuration du mod et son secret.",
            ),
            ServerType::Vanilla => push(
                Critical,
                format!(
                    "« {who} » est un serveur vanilla : il ne comprend aucun transfert de proxy."
                ),
                "Passe-le sur Paper, ou Fabric avec FabricProxy-Lite.",
            ),
            _ => push(
                Info,
                format!("« {who} » : plateforme non reconnue, cohérence non vérifiée."),
                "",
            ),
        }
        return;
    }

    // Legacy (BungeeCord, Velocity legacy) et bungeeguard de Velocity.
    match b.ty() {
        ServerType::Paper | ServerType::Spigot => {
            if !b.spigot_file {
                push(
                    Info,
                    format!(
                        "« {who} » : spigot.yml absent (serveur jamais démarré ?), transfert non vérifié."
                    ),
                    "",
                );
            } else if b.bungeecord != Some(true) {
                push(
                    Important,
                    format!(
                        "« {who} » : settings.bungeecord n'est pas à true (spigot.yml) : IP et UUID transmis par le proxy ignorés, connexions refusées."
                    ),
                    if velocity {
                        "link_proxy (mode legacy) le règle."
                    } else {
                        "link_proxy le règle."
                    },
                );
            }
            if b.velocity_enabled == Some(true) {
                push(
                    Important,
                    format!(
                        "« {who} » : transfert Velocity moderne activé alors que le proxy envoie le transfert legacy."
                    ),
                    "Désactive proxies.velocity.enabled (link_proxy le fait), ou passe le proxy en modern.",
                );
            }
            if guard {
                if !b.bungeeguard {
                    push(
                        Critical,
                        format!(
                            "« {who} » : le proxy est en bungeeguard mais le plugin BungeeGuard est absent du serveur{}.",
                            if offline {
                                " ; quiconque joint directement le serveur peut prendre n'importe quel pseudo"
                            } else {
                                ""
                            }
                        ),
                        "Installe BungeeGuard sur ce serveur avec le secret du proxy comme jeton, ou passe le proxy en modern (Paper).",
                    );
                } else if p
                    .secret
                    .as_deref()
                    .is_some_and(|s| !b.bungeeguard_tokens.iter().any(|t| t == s))
                {
                    push(
                        Critical,
                        format!(
                            "« {who} » : le secret du proxy n'est pas dans les allowed-tokens de BungeeGuard : connexions refusées."
                        ),
                        "Ajoute-le à la main dans plugins/BungeeGuard/config.yml.",
                    );
                }
            } else if !b.bungeeguard {
                if offline && l.reachable {
                    push(
                        Critical,
                        format!(
                            "« {who} » en transfert legacy sans BungeeGuard, joignable directement{direct} : quiconque s'y connecte sans passer par le proxy peut prendre n'importe quel pseudo, opérateurs compris."
                        ),
                        if velocity {
                            "Passe le proxy en modern (link_proxy, serveurs Paper), ou installe BungeeGuard des deux côtés."
                        } else {
                            "Installe BungeeGuard sur le proxy et sur ce serveur (même jeton), ou passe à Velocity (transfert moderne avec secret)."
                        },
                    );
                }
            } else if !velocity {
                match (p.bungeeguard, &p.bungeeguard_token) {
                    (false, _) => push(
                        Important,
                        format!(
                            "« {who} » : BungeeGuard est installé sur le serveur mais pas sur le proxy : connexions refusées."
                        ),
                        "Installe BungeeGuard sur le proxy aussi.",
                    ),
                    (true, Some(t)) if !b.bungeeguard_tokens.iter().any(|x| x == t) => push(
                        Critical,
                        format!(
                            "« {who} » : le jeton BungeeGuard du proxy n'est pas dans ses allowed-tokens : connexions refusées."
                        ),
                        "Recopie le jeton du proxy (plugins/BungeeGuard/token.yml) dans plugins/BungeeGuard/config.yml du serveur, à la main.",
                    ),
                    _ => {}
                }
            }
        }
        ServerType::Fabric | ServerType::Forge => push(
            Info,
            format!(
                "« {who} » ({}) derrière un transfert legacy : un mod est nécessaire ; non vérifié.",
                b.ty().label()
            ),
            "",
        ),
        ServerType::Vanilla => push(
            Critical,
            format!("« {who} » est un serveur vanilla : il ne comprend aucun transfert de proxy."),
            "Passe-le sur Paper ou Spigot.",
        ),
        _ => push(
            Info,
            format!("« {who} » : plateforme non reconnue, cohérence non vérifiée."),
            "",
        ),
    }
}

// --- Rendu ---------------------------------------------------------------------------------------

pub(crate) fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "critique",
        Severity::Important => "important",
        Severity::Info => "info",
    }
}

fn render_problem(p: &Problem, proxy: &ProxyState) -> Value {
    let mut v = json!({
        "severity": severity_label(p.severity),
        "proxy": format!("{} ({})", proxy.name, proxy.server_id),
        "message": p.message,
    });
    if let Some(id) = p.server_id {
        v["server_id"] = json!(id);
    }
    if !p.advice.is_empty() {
        v["advice"] = json!(p.advice);
    }
    v
}

fn render_backend(p: &ProxyState, b: &BackendState) -> Value {
    let mut v = json!({
        "type": b.ty().label(),
        "online_mode": b.online_mode,
    });
    if let Some(ver) = &b.version {
        v["version"] = json!(ver);
    }
    if b.paper != PaperFile::Missing {
        v["velocity_forwarding"] = json!(b.velocity_enabled);
        if b.velocity_enabled == Some(true) || p.conf.forwarding == Forwarding::Modern {
            v["velocity_secret"] =
                json!(
                    SecretMatch::compare(p.secret.as_deref(), b.velocity_secret.as_deref()).label()
                );
        }
    } else if b.ty() == ServerType::Paper {
        v["paper_config"] = json!("absente (serveur jamais démarré ?)");
    }
    if b.spigot_file {
        v["spigot_bungeecord"] = json!(b.bungeecord);
    }
    if b.bungeeguard {
        v["bungeeguard"] = json!(true);
    }
    if b.ty() == ServerType::Fabric {
        v["fabric_proxy_lite"] = json!(if b.fabric_proxy_config {
            SecretMatch::compare(p.secret.as_deref(), b.fabric_proxy_secret.as_deref()).label()
        } else if b.fabric_proxy_mod {
            "mod présent, configuration absente"
        } else {
            "absent"
        });
    }
    v
}

fn render_proxy(p: &ProxyState, links: &[Link], problems: &[Problem]) -> Value {
    let c = &p.conf;
    let servers: Vec<Value> = links
        .iter()
        .map(|l| {
            let mut v = json!({
                "name": l.entry.name,
                "address": l.entry.address,
                "in_try": c.try_order.iter().any(|t| t.eq_ignore_ascii_case(&l.entry.name)),
            });
            v["account_server"] = match l.matched {
                Some(m) => json!({
                    "server_id": m.server_id,
                    "name": l.server_name,
                    "matched_by": m.how.label(),
                }),
                None => Value::Null,
            };
            match &l.backend {
                Some(Ok(b)) => v["settings"] = render_backend(p, b),
                Some(Err(e)) => v["settings"] = json!({ "unread": e }),
                None => {}
            }
            v
        })
        .collect();
    let mut v = json!({
        "server_id": p.server_id,
        "name": p.name,
        "type": c.kind.label(),
        "config": p.config_path,
        "online_mode": c.online_mode,
        "forwarding": c.forwarding.label(),
        "try": c.try_order,
        "servers": servers,
        "problems": problems.iter().map(|x| render_problem(x, p)).collect::<Vec<_>>(),
    });
    if c.kind == ProxyKind::Velocity {
        v["secret"] = json!(match &p.secret {
            Some(_) => format!("défini ({})", p.secret_note),
            None => format!("absent ({})", p.secret_note),
        });
    } else {
        v["bungeeguard"] = json!(match (p.bungeeguard, &p.bungeeguard_token) {
            (false, _) => "absent",
            (true, Some(_)) => "présent, jeton défini",
            (true, None) => "présent, jeton illisible",
        });
    }
    if !c.forced_hosts.is_empty() {
        v["forced_hosts"] = json!(c.forced_hosts.len());
    }
    v
}

/// Efface de chaque texte (et clé) d'une valeur les secrets lus. Filet de sécurité : les secrets
/// ne sont de toute façon jamais placés dans une sortie.
pub(crate) fn scrub_value(v: Value, secrets: &[String]) -> Value {
    let secrets: Vec<&str> = secrets
        .iter()
        .map(|s| s.trim())
        .filter(|s| s.chars().count() >= SCRUB_MIN_LEN)
        .collect();
    if secrets.is_empty() {
        return v;
    }
    fn walk(v: Value, s: &[&str]) -> Value {
        let clean = |t: String| {
            if s.iter().any(|x| t.contains(x)) {
                s.iter().fold(t, |acc, x| acc.replace(x, MASK))
            } else {
                t
            }
        };
        match v {
            Value::String(t) => Value::String(clean(t)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| walk(x, s)).collect()),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, x)| (clean(k), walk(x, s)))
                    .collect::<Map<_, _>>(),
            ),
            other => other,
        }
    }
    walk(v, &secrets)
}

/// Même filet pour un texte (messages d'erreur).
pub(crate) fn scrub_text(t: &str, secrets: &[String]) -> String {
    match scrub_value(Value::String(t.to_string()), secrets) {
        Value::String(s) => s,
        _ => t.to_string(),
    }
}

// --- Collecte (API, SFTP) ------------------------------------------------------------------------

/// Serveur Minecraft Java du compte, vu par le repérage.
#[derive(Debug, Clone)]
pub(crate) struct McServer {
    pub id: i64,
    pub name: String,
    pub status: String,
    /// Adresse de la liste des serveurs (`ip:port` ou nom DNS).
    pub address: String,
    pub ty: Option<ServerType>,
    pub version: Option<String>,
    /// Adresses (`GET /server/{id}`, repli sur `address` quand c'est `ip:port`).
    pub endpoint: Option<Endpoint>,
}

impl McServer {
    /// Fichiers accessibles : serveur actif (ou en veille).
    pub(crate) fn readable(&self) -> bool {
        matches!(self.status.as_str(), "active" | "hibernation")
    }

    pub(crate) fn status_label(&self) -> &'static str {
        match self.status.as_str() {
            "suspended" => "suspendu",
            "expired" => "expiré",
            "disabled" => "désactivé",
            "hibernation" => "en veille",
            _ => "actif",
        }
    }

    pub(crate) fn label(&self) -> String {
        format!("« {} » ({})", self.name, self.id)
    }
}

/// Serveurs Minecraft Java du compte, avec leur plateforme (source de l'hébergeur, jar de
/// démarrage) et leurs adresses pour ceux dont les fichiers sont accessibles. `GET /server/{id}`
/// donne à la fois le jar de démarrage et les adresses : un appel par serveur, plus la version.
pub(crate) async fn mc_servers(core: &Core) -> Result<Vec<McServer>, String> {
    let overview = core.list_servers().await.map_err(|e| e.to_string())?;
    let token = core.token().map_err(|e| e.to_string())?;
    let base: Vec<McServer> = overview
        .servers
        .iter()
        .filter(|s| s.capabilities.is_minecraft_java())
        .map(|s| McServer {
            id: s.id,
            name: s.name.clone(),
            status: s.status.clone(),
            address: s.address.clone(),
            ty: None,
            version: None,
            endpoint: None,
        })
        .collect();
    let tasks: Vec<_> = base
        .into_iter()
        .map(|mut s| {
            let token = token.clone();
            async move {
                if s.readable() {
                    let (mc, raw) = tokio::join!(
                        core.api.minecraft_version(&token, s.id),
                        core.api.server_value(&token, s.id)
                    );
                    let (mc, raw) = (mc.ok(), raw.ok());
                    let jar = raw
                        .as_ref()
                        .and_then(|v| v.pointer("/settings/startup_file"))
                        .and_then(Value::as_str);
                    s.ty = ServerType::detect(mc.as_ref().map(|m| m.source.as_str()), jar);
                    s.version = mc.map(|m| m.version).filter(|v| !v.trim().is_empty());
                    s.endpoint = raw
                        .as_ref()
                        .and_then(|v| Endpoint::from_server_value(s.id, v));
                }
                if s.endpoint.is_none() {
                    s.endpoint = Endpoint::from_list_address(s.id, &s.address);
                }
                s
            }
        })
        .collect();
    let mut out: Vec<McServer> = stream::iter(tasks)
        .buffer_unordered(API_CONCURRENCY)
        .collect()
        .await;
    out.sort_by_cached_key(|s| (s.name.to_lowercase(), s.id));
    Ok(out)
}

/// Texte d'un fichier, `None` s'il est absent ou illisible ; `Err` si la session est morte.
async fn read_optional(conn: &sftp::SftpConn, path: &str) -> crate::Result<Option<String>> {
    match sftp::read_text(conn, path).await {
        Ok(t) => Ok(Some(t)),
        Err(e) if sftp::is_session_dead(&e) => Err(e),
        Err(_) => Ok(None),
    }
}

/// Noms des entrées d'un dossier (vide s'il n'existe pas).
async fn list_names(conn: &sftp::SftpConn, dir: &str) -> crate::Result<Vec<String>> {
    match sftp::list(conn, dir).await {
        Ok(v) => Ok(v.into_iter().map(|e| e.name).collect()),
        Err(e) if sftp::is_session_dead(&e) => Err(e),
        Err(_) => Ok(Vec::new()),
    }
}

/// Secret de transfert tel que Velocity le lit : lignes jointes, espaces retirés.
fn secret_from_file(text: &str) -> Option<String> {
    let s: String = text.lines().collect::<String>().trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Fichiers bruts d'un proxy.
struct ProxyFiles {
    config: Option<(&'static str, String)>,
    secret: Option<String>,
    plugins: Vec<String>,
    bungeeguard_token: Option<String>,
}

/// Lecture d'un proxy : son état et le texte de sa configuration ; `Ok(None)` : pas un proxy.
pub(crate) type ProxyRead = Result<Option<(ProxyState, String)>, String>;

/// Lit un proxy : configuration, secret, BungeeGuard. `Ok(None)` : ce n'est pas un proxy (ni
/// `velocity.toml`, ni `config.yml` de BungeeCord). `expected` : plateforme repérée par
/// l'hébergeur (un proxy annoncé sans configuration est une erreur, pas un « non-proxy »).
pub(crate) async fn read_proxy(
    core: &Core,
    id: i64,
    name: &str,
    expected: Option<ServerType>,
) -> ProxyRead {
    let raw = core
        .with_sftp(id, |conn| async move {
            let velocity = read_optional(&conn, VELOCITY_TOML).await?;
            let (config, conf) = match velocity {
                Some(t) => (Some((VELOCITY_TOML, t.clone())), parse_velocity(&t).ok()),
                None => match read_optional(&conn, BUNGEE_CONFIG).await? {
                    Some(t) => {
                        let c = parse_bungee(&t).ok();
                        (c.is_some().then_some((BUNGEE_CONFIG, t)), c)
                    }
                    None => (None, None),
                },
            };
            let mut files = ProxyFiles {
                config,
                secret: None,
                plugins: Vec::new(),
                bungeeguard_token: None,
            };
            match conf {
                Some(c) if c.kind == ProxyKind::Velocity => {
                    if let Some(path) = c.secret_path() {
                        files.secret = read_optional(&conn, &path)
                            .await?
                            .as_deref()
                            .and_then(secret_from_file);
                    }
                }
                Some(_) => {
                    files.plugins = list_names(&conn, "/plugins").await?;
                    if files
                        .plugins
                        .iter()
                        .any(|p| compact(p).contains("bungeeguard"))
                    {
                        files.bungeeguard_token = read_optional(&conn, BUNGEEGUARD_TOKEN)
                            .await?
                            .and_then(|t| text_of(yaml_tree(&t).get("token")));
                    }
                }
                None => {}
            }
            Ok(files)
        })
        .await
        .map_err(|e| e.to_string())?;
    let Some((path, text)) = raw.config else {
        return match expected {
            Some(ServerType::Velocity) => {
                Err("velocity.toml introuvable (proxy jamais démarré ?)".into())
            }
            Some(ServerType::Bungee) => Err(
                "config.yml de BungeeCord introuvable ou illisible (proxy jamais démarré ?)".into(),
            ),
            _ => Ok(None),
        };
    };
    let conf = if path == VELOCITY_TOML {
        parse_velocity(&text)?
    } else {
        parse_bungee(&text)?
    };
    let (secret, secret_note) = match conf.kind {
        ProxyKind::Bungee => (None, String::new()),
        ProxyKind::Velocity => match conf.secret_path() {
            Some(p) => {
                let note = if raw.secret.is_some() {
                    p.clone()
                } else {
                    format!("{p} absent ou vide")
                };
                (raw.secret, note)
            }
            None if conf.inline_secret.is_some() => (
                conf.inline_secret.clone(),
                "velocity.toml (forwarding-secret)".into(),
            ),
            None => (None, "chemin de forwarding-secret-file refusé".into()),
        },
    };
    let bungeeguard = raw
        .plugins
        .iter()
        .any(|p| compact(p).contains("bungeeguard"));
    let state = ProxyState {
        server_id: id,
        name: name.to_string(),
        conf,
        config_path: path,
        secret,
        secret_note,
        bungeeguard,
        bungeeguard_token: raw.bungeeguard_token,
    };
    Ok(Some((state, text)))
}

/// Fichiers d'un serveur de jeu.
async fn read_backend_files(
    conn: &sftp::SftpConn,
    ty: Option<ServerType>,
) -> crate::Result<BackendFiles> {
    let mut f = BackendFiles {
        paper_global: read_optional(conn, PAPER_GLOBAL).await?,
        ..Default::default()
    };
    if f.paper_global.is_none() {
        f.paper_legacy = read_optional(conn, PAPER_LEGACY).await?;
    }
    f.spigot = read_optional(conn, SPIGOT_YML).await?;
    f.plugins = list_names(conn, "/plugins").await?;
    if f.plugins.iter().any(|p| compact(p).contains("bungeeguard")) {
        f.bungeeguard_config = read_optional(conn, BUNGEEGUARD_CONFIG).await?;
    }
    if matches!(
        ty,
        None | Some(ServerType::Fabric) | Some(ServerType::Unknown)
    ) {
        f.mods = list_names(conn, "/mods").await?;
        f.fabric_proxy = read_optional(conn, FABRIC_PROXY).await?;
    }
    Ok(f)
}

/// Lit un serveur de jeu : `online-mode` (API) et fichiers (SFTP).
pub(crate) async fn read_backend(
    core: &Core,
    id: i64,
    ty: Option<ServerType>,
    version: Option<String>,
) -> Result<(BackendState, BackendFiles), String> {
    let online = core.server_properties(id).await.ok().and_then(|p| {
        p.properties
            .get("online-mode")
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
    });
    let files = core
        .with_sftp(
            id,
            |conn| async move { read_backend_files(&conn, ty).await },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok((backend_state(ty, version, online, &files), files))
}

/// Résout les noms d'hôte des proxys (bornés, avec délai) ; un échec laisse le nom sans IP.
async fn resolve_names(hosts: Vec<(String, u16)>) -> HashMap<String, Vec<IpAddr>> {
    let tasks = hosts
        .into_iter()
        .take(MAX_DNS_LOOKUPS)
        .map(|(host, port)| async move {
            let ips = match tokio::time::timeout(
                DNS_TIMEOUT,
                tokio::net::lookup_host((host.as_str(), port)),
            )
            .await
            {
                Ok(Ok(it)) => it.map(|a| a.ip()).collect(),
                _ => Vec::new(),
            };
            (host, ips)
        });
    futures_util::future::join_all(tasks)
        .await
        .into_iter()
        .collect()
}

/// Rapproche les entrées de chaque proxy des serveurs du compte.
async fn match_links(
    proxies: &[ProxyState],
    eps: &[Endpoint],
    servers: &[McServer],
) -> Vec<Vec<Link>> {
    let mut names: BTreeSet<(String, u16)> = BTreeSet::new();
    for p in proxies {
        for e in &p.conf.entries {
            let a = Addr::parse(&e.address);
            if classify(&a.host) == HostClass::Name
                && !eps
                    .iter()
                    .any(|x| x.dns.as_deref() == Some(a.host.as_str()))
            {
                names.insert((a.host.clone(), a.port_or_default()));
            }
        }
    }
    let resolved = resolve_names(names.into_iter().collect()).await;
    proxies
        .iter()
        .map(|p| {
            p.conf
                .entries
                .iter()
                .map(|e| {
                    let a = Addr::parse(&e.address);
                    let ips = resolved.get(&a.host).map(Vec::as_slice).unwrap_or(&[]);
                    let matched = match_entry(&a, ips, eps);
                    let ep = matched.and_then(|m| eps.iter().find(|x| x.server_id == m.server_id));
                    Link {
                        entry: e.clone(),
                        matched,
                        server_name: matched
                            .and_then(|m| servers.iter().find(|s| s.id == m.server_id))
                            .map(|s| s.name.clone()),
                        server_address: ep.map(Endpoint::address),
                        reachable: ep.is_some_and(Endpoint::public),
                        backend: None,
                    }
                })
                .collect()
        })
        .collect()
}

/// Réglages d'un serveur visé que rien ne relie (sans secret).
fn backend_brief(s: &McServer, b: &BackendState) -> Value {
    json!({
        "server_id": s.id,
        "name": s.name,
        "type": b.ty().label(),
        "online_mode": b.online_mode,
        "velocity_forwarding": b.velocity_enabled,
        "velocity_secret": if b.velocity_secret.is_some() { "défini" } else { "absent" },
        "spigot_bungeecord": b.bungeecord,
        "bungeeguard": b.bungeeguard,
    })
}

/// Note sur un serveur visé qu'aucun proxy lisible du compte ne référence (pur).
pub(crate) fn orphan_note(label: &str, state: Option<&BackendState>) -> String {
    let configured =
        state.is_some_and(|b| b.velocity_enabled == Some(true) || b.bungeecord == Some(true));
    let exposed = state.is_some_and(|b| b.online_mode == Some(false));
    if configured && exposed {
        format!(
            "Aucun proxy lisible du compte ne référence {label}, pourtant réglé pour un proxy (transfert Velocity ou bungeecord activé, online-mode=false) : proxy externe ou suspendu, ou entrée manquante ; sans proxy, vérifie qu'il n'est pas usurpable (audit_server)."
        )
    } else if configured {
        format!(
            "Aucun proxy lisible du compte ne référence {label}, pourtant réglé pour un proxy (transfert Velocity ou bungeecord activé) : proxy externe ou suspendu, ou entrée manquante."
        )
    } else if exposed {
        format!(
            "Aucun proxy lisible du compte ne référence {label}, en online-mode=false sans transfert de proxy : n'importe qui peut s'y connecter sous n'importe quel pseudo (détail : audit_server ; pour le placer derrière un proxy : link_proxy)."
        )
    } else {
        format!("Aucun proxy lisible du compte ne référence {label}.")
    }
}

/// Topologie du compte (outil `network_topology`) : sortie JSON et secrets lus (à effacer).
/// `ping` : ping Java du proxy et de chaque serveur du compte reconnu.
async fn topology(
    core: &Core,
    focus: Option<i64>,
    ping: bool,
) -> Result<(Value, Vec<String>), String> {
    crate::mcp_ctx::progress(0.0, None, "repérage des proxys").await;
    let servers = mc_servers(core).await?;
    if let Some(f) = focus {
        if !servers.iter().any(|s| s.id == f) {
            return Err(format!(
                "serveur {f} introuvable parmi les serveurs Minecraft Java du compte (list_servers)"
            ));
        }
    }
    let mut not_read: Vec<String> = servers
        .iter()
        .filter(|s| !s.readable())
        .map(|s| {
            format!(
                "{} : {}, fichiers inaccessibles",
                s.label(),
                s.status_label()
            )
        })
        .collect();
    let mut notes: Vec<String> = Vec::new();
    let mut candidates: Vec<&McServer> = servers
        .iter()
        .filter(|s| s.readable() && s.ty.is_none_or(ServerType::is_proxy))
        .collect();
    if candidates.len() > MAX_PROXY_PROBES {
        notes.push(format!(
            "{} serveurs de plateforme inconnue : seuls les {MAX_PROXY_PROBES} premiers ont été examinés comme proxys possibles",
            candidates.len()
        ));
        candidates.truncate(MAX_PROXY_PROBES);
    }
    let tasks: Vec<_> = candidates
        .iter()
        .map(|s| async move { (*s, read_proxy(core, s.id, &s.name, s.ty).await) })
        .collect();
    let found: Vec<(&McServer, ProxyRead)> = stream::iter(tasks)
        .buffer_unordered(SFTP_CONCURRENCY)
        .collect()
        .await;
    let mut proxies: Vec<ProxyState> = Vec::new();
    for (s, r) in found {
        match r {
            Ok(Some((p, _))) => proxies.push(p),
            Ok(None) => {}
            Err(e) => not_read.push(format!("{} : proxy illisible ({e})", s.label())),
        }
    }
    proxies.sort_by_cached_key(|p| (p.name.to_lowercase(), p.server_id));
    let mut secrets: Vec<String> = proxies.iter().flat_map(ProxyState::secrets).collect();
    if crate::mcp_ctx::cancelled() {
        return Err("annulé".into());
    }
    let readable = servers.iter().filter(|s| s.readable()).count();
    if proxies.is_empty() {
        let mut out = json!({
            "proxies": [],
            "note": format!("Aucun proxy Velocity ou BungeeCord trouvé parmi {readable} serveur(s) Minecraft Java lisible(s)."),
            "next": "Pour monter un réseau : un serveur Velocity (proxy) et des serveurs Paper, puis link_proxy (dry_run d'abord).",
        });
        // Serveur visé : ses réglages disent s'il attend un proxy (externe, suspendu…).
        if let Some(s) = focus.and_then(|f| servers.iter().find(|s| s.id == f && s.readable())) {
            let state = read_backend(core, s.id, s.ty, s.version.clone())
                .await
                .ok()
                .map(|(b, _)| b);
            if let Some(b) = &state {
                secrets.extend(b.secrets());
                out["server"] = backend_brief(s, b);
            }
            out["note"] = json!(orphan_note(&s.label(), state.as_ref()));
        }
        if !not_read.is_empty() {
            out["not_read"] = json!(not_read);
        }
        return Ok((out, secrets));
    }

    crate::mcp_ctx::progress(1.0, Some(3.0), "rapprochement des adresses").await;
    let eps: Vec<Endpoint> = servers.iter().filter_map(|s| s.endpoint.clone()).collect();
    let mut links = match_links(&proxies, &eps, &servers).await;

    // Serveur visé : le proxy lui-même, ou les proxys qui le référencent.
    if let Some(f) = focus {
        let keep: Vec<bool> = proxies
            .iter()
            .zip(&links)
            .map(|(p, ls)| {
                p.server_id == f
                    || ls
                        .iter()
                        .any(|l| l.matched.is_some_and(|m| m.server_id == f))
            })
            .collect();
        let mut it = keep.iter();
        proxies.retain(|_| *it.next().unwrap_or(&false));
        let mut it = keep.iter();
        links.retain(|_| *it.next().unwrap_or(&false));
    }

    crate::mcp_ctx::progress(2.0, Some(3.0), "réglages des serveurs reliés").await;
    let mut ids: BTreeSet<i64> = links
        .iter()
        .flatten()
        .filter_map(|l| l.matched.map(|m| m.server_id))
        .filter(|id| !proxies.iter().any(|p| p.server_id == *id))
        .collect();
    let focus_alone = focus.filter(|f| proxies.is_empty() && !ids.contains(f));
    if let Some(f) = focus_alone {
        ids.insert(f);
    }
    let tasks: Vec<_> = ids
        .iter()
        .filter_map(|id| servers.iter().find(|s| s.id == *id))
        .map(|s| async move {
            let r = if s.readable() {
                read_backend(core, s.id, s.ty, s.version.clone())
                    .await
                    .map(|(b, _)| b)
            } else {
                Err(format!("serveur {}", s.status_label()))
            };
            (s.id, r)
        })
        .collect();
    let states: HashMap<i64, Result<BackendState, String>> = stream::iter(tasks)
        .buffer_unordered(SFTP_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect();
    for st in states.values().flatten() {
        secrets.extend(st.secrets());
    }
    for l in links.iter_mut().flatten() {
        if let Some(m) = l.matched {
            if let Some(proxy) = proxies.iter().find(|p| p.server_id == m.server_id) {
                l.backend = Some(Ok(BackendState {
                    ty: Some(match proxy.conf.kind {
                        ProxyKind::Velocity => ServerType::Velocity,
                        ProxyKind::Bungee => ServerType::Bungee,
                    }),
                    ..Default::default()
                }));
            } else {
                l.backend = states.get(&m.server_id).cloned();
            }
        }
    }

    let mut rendered = Vec::new();
    let mut all: Vec<(Severity, Value)> = Vec::new();
    let (mut entries, mut recognized) = (0usize, 0usize);
    for (p, ls) in proxies.iter().zip(&links) {
        let problems = evaluate(p, ls);
        entries += ls.len();
        recognized += ls.iter().filter(|l| l.matched.is_some()).count();
        all.extend(problems.iter().map(|x| (x.severity, render_problem(x, p))));
        rendered.push(render_proxy(p, ls, &problems));
    }
    if ping {
        crate::mcp_ctx::progress(3.0, Some(4.0), "ping des serveurs").await;
        let mut ids: BTreeSet<i64> = proxies.iter().map(|p| p.server_id).collect();
        ids.extend(
            links
                .iter()
                .flatten()
                .filter_map(|l| l.matched.map(|m| m.server_id)),
        );
        let eps: Vec<Endpoint> = ids
            .iter()
            .filter_map(|id| servers.iter().find(|s| s.id == *id))
            .filter(|s| s.readable())
            .filter_map(|s| s.endpoint.clone())
            .collect();
        let pings = ping::ping_many(eps).await;
        for (v, p) in rendered.iter_mut().zip(&proxies) {
            if let Some(x) = pings.get(&p.server_id) {
                v["ping"] = x.clone();
            }
            if let Some(Value::Array(list)) = v.get_mut("servers") {
                for s in list {
                    let id = s
                        .pointer("/account_server/server_id")
                        .and_then(Value::as_i64);
                    if let Some(x) = id.and_then(|id| pings.get(&id)) {
                        s["ping"] = x.clone();
                    }
                }
            }
        }
    }
    all.sort_by_key(|(s, _)| *s);
    let count = |s: Severity| all.iter().filter(|(x, _)| *x == s).count();
    let summary = format!(
        "{} proxy(s), {entries} serveur(s) déclaré(s) dont {recognized} reconnu(s) sur le compte ; problèmes : {} critique(s), {} important(s), {} info(s).",
        proxies.len(),
        count(Severity::Critical),
        count(Severity::Important),
        count(Severity::Info)
    );
    let mut out = json!({
        "summary": summary,
        "proxies": rendered,
        "problems": all.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "secrets": "secret de transfert et jetons comparés en interne (identique, différent, absent), jamais affichés",
        "next": "Corriger : link_proxy (dry_run d'abord) ; vérifier ensuite par network_topology. Les réglages ne prennent effet qu'au redémarrage des serveurs de jeu et du proxy.",
    });
    if let Some(f) = focus_alone {
        let state = states.get(&f).and_then(|r| r.as_ref().ok());
        if let (Some(s), Some(b)) = (servers.iter().find(|s| s.id == f), state) {
            out["server"] = backend_brief(s, b);
        }
        let label = servers
            .iter()
            .find(|s| s.id == f)
            .map(McServer::label)
            .unwrap_or_else(|| f.to_string());
        out["note"] = json!(orphan_note(&label, state));
    }
    if !not_read.is_empty() {
        out["not_read"] = json!(not_read);
    }
    if !notes.is_empty() {
        out["notes"] = json!(notes);
    }
    Ok((out, secrets))
}

/// Outil MCP `network_topology`.
pub(crate) async fn network_topology_tool(core: &Core, args: &Value) -> Result<String, String> {
    let focus = match args.get("server_id") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .ok_or("`server_id` : entier attendu")?,
        ),
    };
    let ping = args.get("ping").and_then(Value::as_bool).unwrap_or(false);
    let (out, secrets) = topology(core, focus, ping).await?;
    Ok(crate::config_edit::render_for_ai(
        core,
        scrub_value(out, &secrets),
    ))
}
