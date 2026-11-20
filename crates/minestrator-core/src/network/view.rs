//! **Vue « Réseau » de l'app** : façade typée des réseaux proxy pour un administrateur humain
//! (méthodes `Core::network_*`, commandes Tauri du desktop). Même logique que les outils MCP
//! `network_topology`, `ping_server`, `link_proxy` et `network_preset` — lecture et plan partagés
//! ([`super::collect`], [`super::link::prepare`], [`super::preset::prepare`],
//! [`super::ping::ping_target`]) —, rendue en structures (ts-rs) plutôt qu'en texte d'outil.
//!
//! - **Secrets** : jamais dans une structure. Le secret de transfert, les jetons BungeeGuard et
//!   le secret de FabricProxy-Lite sont seulement **comparés** ([`SecretCheck`]) ; le mot de passe
//!   d'une base n'est que « lisible ou non ». Tout texte rendu (problèmes, résumés, diffs, notes)
//!   passe en plus par l'effacement des valeurs lues et le masque des secrets des agents.
//! - **Pas d'anonymisation IA** : l'humain voit ses propres adresses ; seul le mode démo du front
//!   les remplace à l'affichage.
//! - **Écritures** : seulement après une simulation montrée à l'humain. L'application recalcule
//!   le plan et n'écrit rien si son empreinte ([`NetworkLinkPlan::fingerprint`]) n'est plus celle
//!   de la simulation confirmée : le nouveau plan est rendu (`changed`). Chaque écriture passe par
//!   la garde des agents, dans un changeset d'origine « app » ([`crate::agent_guard::app_scope`]) :
//!   annulable depuis Réglages → MCP (« Modifications des agents »), comme celle d'un agent, en plus
//!   des snapshots préalables. Le mode constructeur ne concerne que les agents : il ne s'applique
//!   pas ici.

use super::link::{self, Mode, SecretPlan, Status, Step};
use super::ping::{self, Pinged};
use super::preset::{self, PortChoice, Preset};
use super::*;
use crate::db_connect::{self, Engine};
use crate::Error;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Serveurs au plus par ping groupé.
const MAX_PINGS: usize = 40;
/// Pings concurrents.
const PING_CONCURRENCY: usize = 8;
/// Libellé des écritures de la vue dans la garde des agents (client du changeset).
const APP_CLIENT: &str = "vue Réseau";

// --- Types : topologie --------------------------------------------------------------------------

/// Gravité d'un problème de réseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkSeverity {
    Critical,
    Important,
    Info,
}

impl From<Severity> for NetworkSeverity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Critical => Self::Critical,
            Severity::Important => Self::Important,
            Severity::Info => Self::Info,
        }
    }
}

/// Plateforme d'un serveur, du point de vue d'un réseau proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkPlatform {
    Velocity,
    Bungeecord,
    /// Paper et ses forks (Purpur, Folia…).
    Paper,
    Spigot,
    Fabric,
    /// Forge, NeoForge et les hybrides.
    Forge,
    Vanilla,
    Unknown,
}

impl From<ServerType> for NetworkPlatform {
    fn from(t: ServerType) -> Self {
        match t {
            ServerType::Velocity => Self::Velocity,
            ServerType::Bungee => Self::Bungeecord,
            ServerType::Paper => Self::Paper,
            ServerType::Spigot => Self::Spigot,
            ServerType::Fabric => Self::Fabric,
            ServerType::Forge => Self::Forge,
            ServerType::Vanilla => Self::Vanilla,
            ServerType::Unknown => Self::Unknown,
        }
    }
}

/// Logiciel d'un proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkProxyKind {
    Velocity,
    Bungeecord,
}

impl From<ProxyKind> for NetworkProxyKind {
    fn from(k: ProxyKind) -> Self {
        match k {
            ProxyKind::Velocity => Self::Velocity,
            ProxyKind::Bungee => Self::Bungeecord,
        }
    }
}

/// Transfert des joueurs (IP, UUID, skin) du proxy vers les serveurs de jeu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ForwardingMode {
    None,
    /// BungeeCord (`ip_forward: true`) ou Velocity `legacy` : sans signature.
    Legacy,
    /// Velocity `bungeeguard` : legacy avec jeton.
    Bungeeguard,
    /// Velocity `modern` : signé par le secret partagé.
    Modern,
    /// Valeur inconnue (voir `forwarding_raw`).
    Other,
}

impl From<&Forwarding> for ForwardingMode {
    fn from(f: &Forwarding) -> Self {
        match f {
            Forwarding::None => Self::None,
            Forwarding::Legacy => Self::Legacy,
            Forwarding::BungeeGuard => Self::Bungeeguard,
            Forwarding::Modern => Self::Modern,
            Forwarding::Other(_) => Self::Other,
        }
    }
}

/// Secret d'un serveur de jeu comparé à celui du proxy — jamais sa valeur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SecretCheck {
    Same,
    Different,
    Missing,
    /// Secret du proxy illisible : rien à comparer.
    Unknown,
}

impl From<SecretMatch> for SecretCheck {
    fn from(m: SecretMatch) -> Self {
        match m {
            SecretMatch::Same => Self::Same,
            SecretMatch::Different => Self::Different,
            SecretMatch::Missing => Self::Missing,
            SecretMatch::Unknown => Self::Unknown,
        }
    }
}

/// Comment une entrée a été rapprochée d'un serveur du compte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkMatch {
    /// IP et port principal.
    Primary,
    /// Port secondaire du serveur.
    Secondary,
    /// Nom DNS du serveur.
    Dns,
    /// Nom d'hôte résolu vers l'adresse du serveur.
    Resolved,
    /// Port seul (adresse privée) : à confirmer.
    PortOnly,
}

impl From<How> for NetworkMatch {
    fn from(h: How) -> Self {
        match h {
            How::Primary => Self::Primary,
            How::Secondary => Self::Secondary,
            How::Dns => Self::Dns,
            How::Resolved => Self::Resolved,
            How::PortOnly => Self::PortOnly,
        }
    }
}

/// Fichier de réglages de Paper trouvé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkPaperConfig {
    /// `config/paper-global.yml` (1.19 et plus).
    Global,
    /// `paper.yml` (1.13 à 1.18).
    Legacy,
    Missing,
}

/// BungeeGuard sur un proxy BungeeCord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkGuard {
    Absent,
    /// Présent, jeton défini (jamais affiché).
    Token,
    TokenUnreadable,
}

/// FabricProxy-Lite sur un serveur Fabric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkFabricProxy {
    Absent,
    /// Mod présent, configuration absente (jamais démarré).
    ModOnly,
    Configured,
}

/// Réseaux proxy d'une MyBox (ou du compte) : proxys → entrées → serveurs du compte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkTopology {
    /// Serveurs Minecraft Java de tout le compte (choix de l'assistant, noms, états).
    pub servers: Vec<NetworkServer>,
    /// Proxys de la MyBox demandée (de tout le compte sans MyBox), triés par nom.
    pub proxies: Vec<NetworkProxy>,
    pub counts: NetworkCounts,
    /// Serveurs non lus (suspendus, proxys illisibles…), et pourquoi.
    pub not_read: Vec<String>,
    pub notes: Vec<String>,
}

/// Problèmes par gravité.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkCounts {
    pub critical: u32,
    pub important: u32,
    pub info: u32,
}

/// Serveur Minecraft Java du compte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkServer {
    pub id: i64,
    pub name: String,
    pub mybox_id: Option<i64>,
    /// État d'infrastructure (`active`, `hibernation`, `suspended`…).
    pub status: String,
    /// Fichiers accessibles (actif ou en veille).
    pub readable: bool,
    /// Plateforme repérée (hébergeur, jar de démarrage) ; `null` : inconnue.
    pub platform: Option<NetworkPlatform>,
    pub version: Option<String>,
    /// `ip:port` publique.
    pub address: Option<String>,
    /// Proxy repéré dans la MyBox lue.
    pub proxy: bool,
}

/// Un proxy et ses entrées.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkProxy {
    pub server_id: i64,
    pub name: String,
    pub kind: NetworkProxyKind,
    /// `/velocity.toml` ou `/config.yml`.
    pub config_path: String,
    pub online_mode: Option<bool>,
    pub forwarding: ForwardingMode,
    /// Valeur écrite de `player-info-forwarding-mode` (Velocity).
    pub forwarding_raw: Option<String>,
    /// Velocity : secret de transfert défini (jamais sa valeur) ; `null` pour BungeeCord.
    pub secret_defined: Option<bool>,
    /// D'où vient le secret ou pourquoi il manque (chemin, sans valeur).
    pub secret_note: String,
    /// BungeeCord seulement.
    pub bungeeguard: Option<NetworkGuard>,
    /// Liste de repli, dans l'ordre.
    pub try_order: Vec<String>,
    pub forced_hosts: Vec<NetworkForcedHost>,
    pub entries: Vec<NetworkEntry>,
    /// Triés par gravité.
    pub problems: Vec<NetworkProblem>,
}

/// Hôte forcé (routage par domaine).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkForcedHost {
    pub host: String,
    pub servers: Vec<String>,
}

/// Une entrée du proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkEntry {
    pub name: String,
    /// Adresse telle qu'écrite.
    pub address: String,
    /// Dans la liste de repli.
    pub in_try: bool,
    /// Adresse locale (`127.0.0.1`, `localhost`) : ne mène à aucun autre serveur.
    pub local: bool,
    /// Serveur du compte reconnu ; `null` : aucun.
    pub target: Option<NetworkTarget>,
    pub backend: Option<NetworkBackend>,
    /// Réglages non lus, et pourquoi.
    pub unread: Option<String>,
}

/// Serveur du compte visé par une entrée.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkTarget {
    pub server_id: i64,
    pub name: Option<String>,
    pub matched_by: NetworkMatch,
    /// Le port de l'entrée est celui du serveur.
    pub port_ok: bool,
    /// `ip:port` principale du serveur.
    pub address: Option<String>,
    /// Joignable directement depuis Internet (IP publique).
    pub reachable: bool,
    /// L'entrée vise le proxy lui-même.
    pub self_proxy: bool,
}

/// Réglages d'un serveur de jeu qui comptent derrière le proxy (secrets comparés seulement).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkBackend {
    pub platform: NetworkPlatform,
    pub version: Option<String>,
    pub online_mode: Option<bool>,
    pub paper_config: NetworkPaperConfig,
    /// `proxies.velocity.enabled`.
    pub velocity_forwarding: Option<bool>,
    /// Secret de transfert comparé (transfert moderne seulement).
    pub velocity_secret: Option<SecretCheck>,
    /// `spigot.yml` présent.
    pub spigot_file: bool,
    /// `settings.bungeecord` de `spigot.yml`.
    pub bungeecord: Option<bool>,
    pub bungeeguard: bool,
    /// Fabric seulement.
    pub fabric_proxy: Option<NetworkFabricProxy>,
    pub fabric_secret: Option<SecretCheck>,
    /// Lui-même un proxy (chaîne de proxys, non vérifiée).
    pub chained_proxy: bool,
}

/// Problème du réseau, avec sa correction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkProblem {
    pub severity: NetworkSeverity,
    /// Serveur de jeu concerné ; `null` : le proxy.
    pub server_id: Option<i64>,
    pub message: String,
    /// Conseil (vide s'il n'y en a pas).
    pub advice: String,
    /// La correction passe par l'assistant de liaison.
    pub link_fix: bool,
}

// --- Types : ping ------------------------------------------------------------------------------------

/// Édition pinguée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkPingEdition {
    Java,
    Bedrock,
}

/// Ping d'un serveur du compte à son adresse publique.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkPing {
    pub server_id: i64,
    pub name: String,
    pub edition: NetworkPingEdition,
    /// `ip:port` principale du serveur (`null` : inconnue).
    pub address: Option<String>,
    /// Port qui a répondu (UDP en Bedrock).
    pub port: Option<u16>,
    pub online: bool,
    /// Pourquoi il ne répond pas.
    pub error: Option<String>,
    pub version: Option<String>,
    pub protocol: Option<i64>,
    /// MOTD en texte brut (codes de couleur retirés).
    pub motd: String,
    pub sub_motd: Option<String>,
    pub gamemode: Option<String>,
    pub players_online: Option<i64>,
    pub players_max: Option<i64>,
    pub latency_ms: Option<u64>,
    /// Réponse au ping legacy (avant 1.7).
    pub legacy: bool,
    /// Mods annoncés (Forge).
    pub modded: Option<String>,
}

/// Base de données d'une MyBox (préréglage LuckPerms), **sans** mot de passe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkDatabase {
    pub id: i64,
    pub label: Option<String>,
    pub host: String,
    pub port: i64,
    pub database: String,
    pub username: String,
    /// MySQL ou MariaDB (ou moteur inconnu, supposé tel) : utilisable par LuckPerms.
    pub supported: bool,
}

// --- Types : plans ---------------------------------------------------------------------------------

/// État d'une étape d'un plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkStepStatus {
    /// Déjà conforme.
    Ok,
    /// À écrire.
    Change,
    /// À faire à la main (expliqué).
    Manual,
    /// Impossible (fichier invalide…) : rien n'est écrit tant qu'elle reste.
    Refused,
}

impl From<Status> for NetworkStepStatus {
    fn from(s: Status) -> Self {
        match s {
            Status::Ok => Self::Ok,
            Status::Change => Self::Change,
            Status::Manual => Self::Manual,
            Status::Refused => Self::Refused,
        }
    }
}

/// Résultat d'une étape appliquée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkOutcome {
    Done,
    Failed,
    /// Non exécutée (arrêt après un échec).
    NotRun,
}

/// Une étape d'un plan : une écriture (un fichier, un réglage, une installation) ou une action
/// manuelle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkStep {
    /// Identifiant stable (`<serveur>.paper`, `proxy.config`…).
    pub id: String,
    pub server_id: i64,
    pub server: String,
    pub status: NetworkStepStatus,
    pub summary: String,
    /// Diff unifié, secrets masqués.
    pub diff: Option<String>,
    pub outcome: Option<NetworkOutcome>,
    /// Mention de la garde (annulation) ou erreur.
    pub detail: Option<String>,
}

/// Étapes par état.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkStepCounts {
    pub change: u32,
    pub ok: u32,
    pub manual: u32,
    pub refused: u32,
}

/// Où en est un plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkPlanStatus {
    /// Simulation : rien n'a été écrit.
    Simulated,
    /// Le plan recalculé n'est plus celui confirmé : rien n'a été écrit, à revoir.
    Changed,
    /// Des effets qui ne se défont pas (`to_confirm`) attendent une confirmation explicite.
    NeedsConfirmation,
    /// Des étapes sont refusées : rien n'a été écrit.
    Refused,
    /// Rien à écrire.
    NothingToDo,
    Done,
    /// Arrêt sur une étape en échec : les suivantes n'ont pas été exécutées.
    Failed,
    Cancelled,
}

/// Snapshot prise avant l'application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkSnapshot {
    pub server_id: i64,
    pub server: String,
    pub name: String,
    pub job_id: i64,
}

// --- Types : liaison ------------------------------------------------------------------------------

/// Cible d'une liste de repli ou d'un hôte forcé : identifiant d'un serveur de jeu de la
/// demande (nombre) ou nom d'une entrée déjà déclarée (texte).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum LinkTarget {
    Server(i64),
    Entry(String),
}

/// Nom d'entrée imposé pour un serveur de jeu.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkEntryName {
    pub server_id: i64,
    pub name: String,
}

/// Hôte forcé demandé : domaine (sans port) → entrées.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkForcedHost {
    pub host: String,
    pub targets: Vec<LinkTarget>,
}

/// Mode de transfert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LinkMode {
    /// Transfert moderne de Velocity, signé (Paper 1.13+).
    Modern,
    /// Transfert BungeeCord, sans signature (BungeeGuard conseillé).
    Legacy,
}

impl From<Mode> for LinkMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Modern => Self::Modern,
            Mode::Legacy => Self::Legacy,
        }
    }
}

/// Demande de liaison d'un proxy à des serveurs de jeu (mêmes règles que l'outil `link_proxy`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkRequest {
    pub proxy_server_id: i64,
    /// Ordre gardé : c'est la liste de repli par défaut.
    pub backend_server_ids: Vec<i64>,
    /// Noms imposés ; sinon le nom d'une entrée qui vise déjà le serveur, ou tiré de son nom.
    pub names: Vec<LinkEntryName>,
    /// Liste de repli ; vide : l'actuelle si elle reste valable, sinon le premier serveur.
    pub try_order: Vec<LinkTarget>,
    pub forced_hosts: Vec<LinkForcedHost>,
    /// Retirer les entrées qui ne mènent à aucun serveur de jeu (exemples de la config).
    pub prune: bool,
    /// `null` : modern pour Velocity, legacy pour BungeeCord.
    pub mode: Option<LinkMode>,
    /// Snapshot de chaque serveur modifié avant d'écrire (attendue).
    pub snapshot: bool,
}

/// Changement d'une entrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LinkEntryChange {
    Added,
    Updated,
    Unchanged,
}

/// Changement d'un hôte forcé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LinkHostChange {
    Added,
    Modified,
    Unchanged,
    Removed,
}

/// Provenance du secret de transfert (jamais sa valeur).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LinkSecret {
    /// Transfert legacy : pas de secret.
    NotUsed,
    /// Secret du proxy réutilisé.
    Reused,
    /// Généré (32 caractères, générateur du système), écrit seulement à l'application.
    Generated,
}

/// Entrée du proxy après le plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkPlannedEntry {
    pub name: String,
    pub address: String,
    pub server_id: i64,
    pub server: String,
    pub change: LinkEntryChange,
}

/// Hôte forcé après le plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkPlannedHost {
    pub host: String,
    pub servers: Vec<String>,
    pub change: LinkHostChange,
}

/// Entrée retirée (nettoyage).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LinkPrunedEntry {
    pub name: String,
    pub address: String,
}

/// Plan de liaison (simulation) ou résultat de son application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkLinkPlan {
    pub proxy_id: i64,
    pub proxy_name: String,
    pub proxy_kind: NetworkProxyKind,
    pub config_path: String,
    pub mode: LinkMode,
    pub secret: LinkSecret,
    /// Fichier du secret réutilisé (chemin, sans valeur).
    pub secret_source: Option<String>,
    pub entries: Vec<LinkPlannedEntry>,
    pub try_order: Vec<String>,
    pub try_changed: bool,
    pub forced_hosts: Vec<LinkPlannedHost>,
    pub pruned: Vec<LinkPrunedEntry>,
    pub steps: Vec<NetworkStep>,
    pub counts: NetworkStepCounts,
    pub warnings: Vec<String>,
    /// Empreinte du plan simulé : à rendre pour appliquer exactement ce plan.
    pub fingerprint: String,
    pub status: NetworkPlanStatus,
    /// Étapes écrites.
    pub applied: u32,
    pub snapshots: Vec<NetworkSnapshot>,
}

// --- Types : préréglages ------------------------------------------------------------------------

/// Préréglage de réseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NetworkPresetKind {
    /// LuckPerms partout, même base MySQL/MariaDB.
    Luckperms,
    /// Geyser et Floodgate (joueurs Bedrock).
    Geyser,
}

impl From<Preset> for NetworkPresetKind {
    fn from(p: Preset) -> Self {
        match p {
            Preset::LuckPerms => Self::Luckperms,
            Preset::Geyser => Self::Geyser,
        }
    }
}

/// Demande de préréglage (mêmes règles que l'outil `network_preset`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkPresetRequest {
    pub preset: NetworkPresetKind,
    pub proxy_server_id: i64,
    pub backend_server_ids: Vec<i64>,
    /// LuckPerms : MyBox de la base (défaut : celle du proxy).
    pub mybox_id: Option<i64>,
    /// LuckPerms : base (défaut : la seule de la MyBox).
    pub database_id: Option<i64>,
    /// Geyser : port UDP du proxy (défaut : port actuel de Geyser ou port décrit « Bedrock »).
    pub bedrock_port: Option<u16>,
    pub snapshot: bool,
}

/// Serveur concerné par un préréglage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkPresetServer {
    pub server_id: i64,
    pub name: String,
    pub proxy: bool,
    pub platform: NetworkPlatform,
    /// Nom de son entrée sur le proxy (contexte `server` de LuckPerms).
    pub entry: Option<String>,
}

/// Base retenue (LuckPerms), sans mot de passe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkPresetDatabase {
    pub mybox_id: i64,
    pub id: i64,
    pub label: Option<String>,
    pub host: String,
    pub port: i64,
    pub database: String,
    pub username: String,
    /// Mot de passe lisible par l'API (sinon aucune config n'est écrite).
    pub password_readable: bool,
}

/// Port Bedrock retenu (Geyser).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkBedrockPort {
    /// `null` : aucun port utilisable, à ajouter (`manage_ports`).
    pub port: Option<u16>,
    /// Pourquoi ce port.
    pub source: Option<String>,
    /// Appel de l'outil officiel `manage_ports` qui en ajoute un (JSON).
    pub manage_ports: Option<String>,
    /// IP MyShield : seuls quelques ports publics sont joignables.
    pub myshield: bool,
}

/// Plan d'un préréglage (simulation) ou résultat de son application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetworkPresetPlan {
    pub preset: NetworkPresetKind,
    pub proxy_name: String,
    pub servers: Vec<NetworkPresetServer>,
    pub steps: Vec<NetworkStep>,
    pub counts: NetworkStepCounts,
    pub warnings: Vec<String>,
    /// Effets qui ne se défont pas entièrement : à confirmer explicitement.
    pub to_confirm: Vec<String>,
    pub database: Option<NetworkPresetDatabase>,
    pub bedrock: Option<NetworkBedrockPort>,
    /// Empreinte du plan simulé : à rendre pour appliquer exactement ce plan.
    pub fingerprint: String,
    pub status: NetworkPlanStatus,
    pub applied: u32,
    pub snapshots: Vec<NetworkSnapshot>,
}

// --- Conversions (pures) ----------------------------------------------------------------------

/// Texte rendu : valeurs lues effacées, secrets des agents masqués.
fn clean(t: &str, secrets: &[String]) -> String {
    scrub_text(&crate::redact::mask_known_secrets(t), secrets)
}

/// Les textes du cœur s'adressent aux agents MCP (noms d'outils et d'arguments) : la vue nomme
/// plutôt ses propres assistants.
fn for_humans(t: &str) -> String {
    t.replace("link_proxy", "l'assistant « Relier des serveurs »")
        .replace("network_preset", "le préréglage")
        .replace("network_topology", "la vue Réseau")
        .replace("prune=true", "le nettoyage des entrées")
}

/// [`clean`] puis [`for_humans`] : textes en prose (problèmes, résumés, avertissements).
fn prose(t: &str, secrets: &[String]) -> String {
    for_humans(&clean(t, secrets))
}

fn backend_view(p: &ProxyState, b: &BackendState) -> NetworkBackend {
    let paper = b.paper != PaperFile::Missing;
    let fabric = b.ty() == ServerType::Fabric;
    NetworkBackend {
        platform: b.ty().into(),
        version: b.version.clone(),
        online_mode: b.online_mode,
        paper_config: match b.paper {
            PaperFile::Global => NetworkPaperConfig::Global,
            PaperFile::Legacy => NetworkPaperConfig::Legacy,
            PaperFile::Missing => NetworkPaperConfig::Missing,
        },
        velocity_forwarding: b.velocity_enabled,
        velocity_secret: (paper
            && (b.velocity_enabled == Some(true) || p.conf.forwarding == Forwarding::Modern))
            .then(|| {
                SecretMatch::compare(p.secret.as_deref(), b.velocity_secret.as_deref()).into()
            }),
        spigot_file: b.spigot_file,
        bungeecord: b.bungeecord,
        bungeeguard: b.bungeeguard,
        fabric_proxy: fabric.then_some(if b.fabric_proxy_config {
            NetworkFabricProxy::Configured
        } else if b.fabric_proxy_mod {
            NetworkFabricProxy::ModOnly
        } else {
            NetworkFabricProxy::Absent
        }),
        fabric_secret: (fabric && b.fabric_proxy_config).then(|| {
            SecretMatch::compare(p.secret.as_deref(), b.fabric_proxy_secret.as_deref()).into()
        }),
        chained_proxy: b.ty().is_proxy(),
    }
}

fn entry_view(p: &ProxyState, l: &Link, secrets: &[String]) -> NetworkEntry {
    let addr = Addr::parse(&l.entry.address);
    let (backend, unread) = match &l.backend {
        Some(Ok(b)) => (Some(backend_view(p, b)), None),
        Some(Err(e)) => (None, Some(clean(e, secrets))),
        None => (None, None),
    };
    NetworkEntry {
        name: l.entry.name.clone(),
        address: l.entry.address.clone(),
        in_try: p
            .conf
            .try_order
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&l.entry.name)),
        local: classify(&addr.host) == HostClass::Local,
        target: l.matched.map(|m| NetworkTarget {
            server_id: m.server_id,
            name: l.server_name.clone(),
            matched_by: m.how.into(),
            port_ok: m.port_ok,
            address: l.server_address.clone(),
            reachable: l.reachable,
            self_proxy: m.server_id == p.server_id,
        }),
        backend,
        unread,
    }
}

fn proxy_view(p: &ProxyState, links: &[Link], secrets: &[String]) -> NetworkProxy {
    let c = &p.conf;
    let mut problems = evaluate(p, links);
    problems.sort_by_key(|x| x.severity);
    NetworkProxy {
        server_id: p.server_id,
        name: p.name.clone(),
        kind: c.kind.into(),
        config_path: p.config_path.to_string(),
        online_mode: c.online_mode,
        forwarding: (&c.forwarding).into(),
        forwarding_raw: match &c.forwarding {
            Forwarding::Other(s) => Some(s.clone()),
            _ => c.forwarding_raw.clone(),
        },
        secret_defined: (c.kind == ProxyKind::Velocity).then_some(p.secret.is_some()),
        secret_note: clean(&p.secret_note, secrets),
        bungeeguard: (c.kind == ProxyKind::Bungee).then_some(
            match (p.bungeeguard, &p.bungeeguard_token) {
                (false, _) => NetworkGuard::Absent,
                (true, Some(_)) => NetworkGuard::Token,
                (true, None) => NetworkGuard::TokenUnreadable,
            },
        ),
        try_order: c.try_order.clone(),
        forced_hosts: c
            .forced_hosts
            .iter()
            .map(|(host, servers)| NetworkForcedHost {
                host: host.clone(),
                servers: servers.clone(),
            })
            .collect(),
        entries: links.iter().map(|l| entry_view(p, l, secrets)).collect(),
        problems: problems
            .iter()
            .map(|x| NetworkProblem {
                severity: x.severity.into(),
                server_id: x.server_id,
                message: prose(&x.message, secrets),
                advice: prose(&x.advice, secrets),
                link_fix: x.advice.contains("link_proxy"),
            })
            .collect(),
    }
}

/// Topologie typée (pure). `mybox` : MyBox de chaque serveur du compte.
pub(crate) fn topology_view(c: &Collected, mybox: &HashMap<i64, i64>) -> NetworkTopology {
    let proxies: Vec<NetworkProxy> = c
        .proxies
        .iter()
        .zip(&c.links)
        .map(|(p, ls)| proxy_view(p, ls, &c.secrets))
        .collect();
    let mut counts = NetworkCounts::default();
    for x in proxies.iter().flat_map(|p| &p.problems) {
        match x.severity {
            NetworkSeverity::Critical => counts.critical += 1,
            NetworkSeverity::Important => counts.important += 1,
            NetworkSeverity::Info => counts.info += 1,
        }
    }
    let servers = c
        .servers
        .iter()
        .map(|s| NetworkServer {
            id: s.id,
            name: s.name.clone(),
            mybox_id: mybox.get(&s.id).copied().filter(|m| *m > 0),
            status: s.status.clone(),
            readable: s.readable(),
            platform: s.ty.map(Into::into),
            version: s.version.clone(),
            address: s.endpoint.as_ref().map(Endpoint::address),
            proxy: c.proxies.iter().any(|p| p.server_id == s.id),
        })
        .collect();
    NetworkTopology {
        servers,
        proxies,
        counts,
        not_read: c.not_read.iter().map(|t| prose(t, &c.secrets)).collect(),
        notes: c.notes.iter().map(|t| prose(t, &c.secrets)).collect(),
    }
}

/// Ping typé (pur).
pub(crate) fn ping_view(
    id: i64,
    name: &str,
    address: Option<String>,
    bedrock: bool,
    r: Result<Pinged, String>,
) -> NetworkPing {
    let mut v = NetworkPing {
        server_id: id,
        name: name.to_string(),
        edition: if bedrock {
            NetworkPingEdition::Bedrock
        } else {
            NetworkPingEdition::Java
        },
        address,
        port: None,
        online: false,
        error: None,
        version: None,
        protocol: None,
        motd: String::new(),
        sub_motd: None,
        gamemode: None,
        players_online: None,
        players_max: None,
        latency_ms: None,
        legacy: false,
        modded: None,
    };
    match r {
        Err(e) => v.error = Some(e),
        Ok(Pinged::Java { port, status }) => {
            v.online = true;
            v.port = Some(port);
            v.version = status.version;
            v.protocol = status.protocol;
            v.motd = status.motd;
            v.players_online = status.online;
            v.players_max = status.max;
            v.latency_ms = status.latency_ms;
            v.legacy = status.legacy;
            v.modded = status.modded;
        }
        Ok(Pinged::Bedrock { port, status }) => {
            v.online = true;
            v.port = Some(port);
            v.version = status.version;
            v.protocol = status.protocol;
            v.motd = status.motd;
            v.sub_motd = status.sub_motd;
            v.gamemode = status.gamemode;
            v.players_online = status.online;
            v.players_max = status.max;
            v.latency_ms = status.latency_ms;
        }
    }
    v
}

fn step_view(s: &Step, secrets: &[String]) -> NetworkStep {
    NetworkStep {
        id: s.id.clone(),
        server_id: s.server_id,
        server: s.server.clone(),
        status: s.status.into(),
        summary: prose(&s.summary, secrets),
        diff: s.diff.as_deref().map(|d| clean(d, secrets)),
        outcome: None,
        detail: None,
    }
}

fn counts_of(steps: &[Step]) -> NetworkStepCounts {
    let n = |st: Status| steps.iter().filter(|s| s.status == st).count() as u32;
    NetworkStepCounts {
        change: n(Status::Change),
        ok: n(Status::Ok),
        manual: n(Status::Manual),
        refused: n(Status::Refused),
    }
}

/// Empreinte d'un plan rendu (sans son empreinte ni son état) : sha256 du JSON.
fn fingerprint_of(v: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    crate::util::hex(&Sha256::digest(&bytes))
}

/// Plan de liaison typé (pur), empreinte comprise.
pub(crate) fn link_view(p: &link::Prepared) -> NetworkLinkPlan {
    let (plan, proxy, secrets) = (&p.plan, &p.proxy, &p.secrets);
    let mut v = NetworkLinkPlan {
        proxy_id: proxy.server_id,
        proxy_name: proxy.name.clone(),
        proxy_kind: proxy.conf.kind.into(),
        config_path: proxy.config_path.to_string(),
        mode: plan.mode.into(),
        secret: match plan.secret {
            SecretPlan::NotUsed => LinkSecret::NotUsed,
            SecretPlan::Reused(_) => LinkSecret::Reused,
            SecretPlan::Generated => LinkSecret::Generated,
        },
        secret_source: match &plan.secret {
            SecretPlan::Reused(from) => Some(clean(from, secrets)),
            _ => None,
        },
        entries: plan
            .entries
            .iter()
            .map(|e| LinkPlannedEntry {
                name: e.name.clone(),
                address: e.address.clone(),
                server_id: e.server_id,
                server: e.server.clone(),
                change: match e.status {
                    "ajoutée" => LinkEntryChange::Added,
                    "mise à jour" => LinkEntryChange::Updated,
                    _ => LinkEntryChange::Unchanged,
                },
            })
            .collect(),
        try_order: plan.try_order.clone(),
        try_changed: plan.try_changed,
        forced_hosts: plan
            .hosts
            .iter()
            .map(|h| LinkPlannedHost {
                host: h.host.clone(),
                servers: h.servers.clone(),
                change: match h.status {
                    "ajouté" => LinkHostChange::Added,
                    "modifié" => LinkHostChange::Modified,
                    "retiré" => LinkHostChange::Removed,
                    _ => LinkHostChange::Unchanged,
                },
            })
            .collect(),
        pruned: plan
            .pruned
            .iter()
            .map(|e| LinkPrunedEntry {
                name: e.name.clone(),
                address: e.address.clone(),
            })
            .collect(),
        steps: plan.steps.iter().map(|s| step_view(s, secrets)).collect(),
        counts: counts_of(&plan.steps),
        warnings: plan.warnings.iter().map(|w| prose(w, secrets)).collect(),
        fingerprint: String::new(),
        status: NetworkPlanStatus::Simulated,
        applied: 0,
        snapshots: Vec::new(),
    };
    v.fingerprint = fingerprint_of(&v);
    v
}

/// Plan de préréglage typé (pur), empreinte comprise.
pub(crate) fn preset_view(kind: Preset, p: &preset::Prepared) -> NetworkPresetPlan {
    let secrets = &p.secrets;
    let mut v = NetworkPresetPlan {
        preset: kind.into(),
        proxy_name: p.proxy_name.clone(),
        servers: p
            .nodes
            .iter()
            .map(|n| NetworkPresetServer {
                server_id: n.server_id,
                name: n.name.clone(),
                proxy: n.proxy,
                platform: n.ty.map_or(NetworkPlatform::Unknown, Into::into),
                entry: n.entry.clone(),
            })
            .collect(),
        steps: p.plan.steps.iter().map(|s| step_view(s, secrets)).collect(),
        counts: counts_of(&p.plan.steps),
        warnings: p.plan.warnings.iter().map(|w| prose(w, secrets)).collect(),
        to_confirm: p
            .plan
            .destructive
            .iter()
            .map(|w| prose(w, secrets))
            .collect(),
        database: p
            .database
            .as_ref()
            .map(|(mybox, c, readable)| NetworkPresetDatabase {
                mybox_id: *mybox,
                id: c.id,
                label: c.label.clone(),
                host: c.host.clone(),
                port: c.port,
                database: c.database.clone(),
                username: c.username.clone(),
                password_readable: *readable,
            }),
        bedrock: p.bedrock.as_ref().map(|b| match b {
            PortChoice::Use { port, why } => NetworkBedrockPort {
                port: Some(*port),
                source: Some(clean(why, secrets)),
                manage_ports: None,
                myshield: p.myshield,
            },
            PortChoice::Missing { call } => NetworkBedrockPort {
                port: None,
                source: None,
                manage_ports: Some(call.to_string()),
                myshield: p.myshield,
            },
        }),
        fingerprint: String::new(),
        status: NetworkPlanStatus::Simulated,
        applied: 0,
        snapshots: Vec::new(),
    };
    v.fingerprint = fingerprint_of(&v);
    v
}

/// Reporte le résultat d'une application sur les étapes rendues.
fn apply_outcomes(
    steps: &mut [NetworkStep],
    r: &link::Applied,
    secrets: &[String],
) -> (NetworkPlanStatus, u32, Vec<NetworkSnapshot>) {
    for s in steps.iter_mut() {
        if let Some((o, detail)) = r.outcomes.get(&s.id) {
            s.outcome = Some(match o.as_str() {
                "réussie" => NetworkOutcome::Done,
                "échouée" => NetworkOutcome::Failed,
                _ => NetworkOutcome::NotRun,
            });
            s.detail = detail.as_deref().map(|d| clean(d, secrets));
        }
    }
    let status = if r.failed {
        NetworkPlanStatus::Failed
    } else if r.cancelled {
        NetworkPlanStatus::Cancelled
    } else {
        NetworkPlanStatus::Done
    };
    let snapshots = r
        .snapshots
        .iter()
        .filter_map(|v| serde_json::from_value::<NetworkSnapshot>(v.clone()).ok())
        .collect();
    (status, r.applied as u32, snapshots)
}

impl LinkRequest {
    /// Arguments de l'outil `link_proxy`, validés ensuite par [`link::parse_args`].
    fn to_args(&self) -> Result<Value, String> {
        let mut names = Map::new();
        for n in self.names.iter().filter(|n| !n.name.trim().is_empty()) {
            if names
                .insert(n.server_id.to_string(), json!(n.name.trim()))
                .is_some()
            {
                return Err(format!("deux noms donnés au serveur {}", n.server_id));
            }
        }
        let mut hosts = Map::new();
        for h in &self.forced_hosts {
            let host = h.host.trim();
            if host.is_empty() {
                return Err("hôte forcé sans domaine".into());
            }
            if h.targets.is_empty() {
                return Err(format!("hôte forcé « {host} » sans serveur"));
            }
            if hosts.insert(host.to_string(), json!(h.targets)).is_some() {
                return Err(format!("hôte forcé « {host} » donné deux fois"));
            }
        }
        Ok(json!({
            "proxy_server_id": self.proxy_server_id,
            "backend_server_ids": self.backend_server_ids,
            "names": if names.is_empty() { Value::Null } else { Value::Object(names) },
            "try": if self.try_order.is_empty() { Value::Null } else { json!(self.try_order) },
            "forced_hosts": if hosts.is_empty() { Value::Null } else { Value::Object(hosts) },
            "prune": self.prune,
            "mode": self.mode.map(|m| match m { LinkMode::Modern => "modern", LinkMode::Legacy => "legacy" }),
            "dry_run": true,
            "snapshot": self.snapshot,
        }))
    }
}

impl NetworkPresetRequest {
    /// Arguments de l'outil `network_preset`, validés ensuite par [`preset::parse_args`].
    fn to_args(&self) -> Value {
        let mut v = json!({
            "preset": match self.preset {
                NetworkPresetKind::Luckperms => "luckperms",
                NetworkPresetKind::Geyser => "geyser",
            },
            "proxy_server_id": self.proxy_server_id,
            "backend_server_ids": self.backend_server_ids,
            "dry_run": true,
            "snapshot": self.snapshot,
        });
        // Clés seulement présentes si données : leur simple présence est refusée hors de leur
        // préréglage.
        if let Some(m) = self.mybox_id {
            v["mybox_id"] = json!(m);
        }
        if let Some(d) = self.database_id {
            v["database_id"] = json!(d);
        }
        if let Some(p) = self.bedrock_port {
            v["bedrock_port"] = json!(p);
        }
        v
    }
}

// --- Façade ------------------------------------------------------------------------------------

impl Core {
    /// Vue Réseau : proxys d'une MyBox (`None` : tout le compte), leurs entrées rapprochées des
    /// serveurs du compte, réglages des serveurs de jeu et problèmes triés par gravité. Lectures
    /// seulement (API et SFTP), aucun ping.
    pub async fn network_topology(&self, mybox_id: Option<i64>) -> crate::Result<NetworkTopology> {
        let overview = self.list_servers().await?;
        let mybox: HashMap<i64, i64> = overview
            .servers
            .iter()
            .map(|s| (s.id, s.mybox_id))
            .collect();
        let only: Option<BTreeSet<i64>> = mybox_id.map(|m| {
            overview
                .servers
                .iter()
                .filter(|s| s.mybox_id == m)
                .map(|s| s.id)
                .collect()
        });
        self.refresh_secret_masks();
        let c = collect(self, None, false, only.as_ref())
            .await
            .map_err(Error::Invalid)?;
        Ok(topology_view(&c, &mybox))
    }

    /// Pingue des serveurs **du compte** à leur adresse publique (Java, ou Bedrock en UDP sur un
    /// port du serveur). Un serveur muet ou hors du compte rend `online: false` et la raison.
    pub async fn network_ping(
        &self,
        server_ids: Vec<i64>,
        bedrock: bool,
    ) -> crate::Result<Vec<NetworkPing>> {
        if server_ids.len() > MAX_PINGS {
            return Err(Error::Invalid(format!(
                "{} serveurs : {MAX_PINGS} au plus par ping groupé",
                server_ids.len()
            )));
        }
        let overview = self.list_servers().await?;
        let overview = &overview;
        let tasks = server_ids.into_iter().map(|id| async move {
            let fallback = overview
                .servers
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| id.to_string());
            match ping::endpoint_in(self, overview, id).await {
                Err(e) => ping_view(id, &fallback, None, bedrock, Err(e)),
                Ok((name, ep, raw)) => {
                    let r = ping::ping_target(id, &name, &ep, &raw, bedrock, None).await;
                    ping_view(id, &name, Some(ep.address()), bedrock, r)
                }
            }
        });
        Ok(stream::iter(tasks)
            .buffered(PING_CONCURRENCY)
            .collect()
            .await)
    }

    /// Bases de données d'une MyBox, **sans** mot de passe (choix du préréglage LuckPerms).
    pub async fn network_databases(&self, mybox_id: i64) -> crate::Result<Vec<NetworkDatabase>> {
        let token = self.token()?;
        let data = self
            .api
            .get_data(&token, &format!("/mybox/{mybox_id}"))
            .await?;
        Ok(db_connect::database_entries(&data)
            .into_iter()
            .filter_map(|e| db_connect::read_creds(e).ok())
            .map(|c| NetworkDatabase {
                id: c.id,
                label: c.label,
                host: c.host,
                port: c.port,
                database: c.database,
                username: c.username,
                supported: !matches!(c.engine, Engine::Other),
            })
            .collect())
    }

    /// Liaison d'un proxy à des serveurs de jeu (outil `link_proxy`). Sans `apply` : simulation,
    /// rien n'est écrit. Avec `apply` = l'empreinte de la simulation confirmée par l'humain : le
    /// plan est recalculé et appliqué s'il est identique (snapshots attendues si `snapshot`, puis
    /// une étape annulable par écriture), sinon rendu tel quel (`changed`), sans rien écrire.
    pub async fn network_link(
        &self,
        req: LinkRequest,
        apply: Option<String>,
    ) -> crate::Result<NetworkLinkPlan> {
        let args = req.to_args().map_err(Error::Invalid)?;
        let a = link::parse_args(&args).map_err(Error::Invalid)?;
        let p = link::prepare(self, &a, false)
            .await
            .map_err(Error::Invalid)?;
        let mut view = link_view(&p);
        let Some(expected) = apply else {
            return Ok(view);
        };
        let todo: Vec<&Step> = p
            .plan
            .steps
            .iter()
            .filter(|s| s.status == Status::Change)
            .collect();
        view.status = if expected != view.fingerprint {
            NetworkPlanStatus::Changed
        } else if view.counts.refused > 0 {
            NetworkPlanStatus::Refused
        } else if todo.is_empty() {
            NetworkPlanStatus::NothingToDo
        } else {
            // Proxy d'abord : un secret généré est posé sur le proxy avant les serveurs de jeu.
            let r = crate::agent_guard::app_scope(
                "reseau",
                APP_CLIENT,
                link::apply_steps(self, &todo, p.secret.as_deref(), req.snapshot, "link_proxy"),
            )
            .await
            .map_err(|e| Error::Invalid(clean(&e, &p.secrets)))?;
            let (status, applied, snapshots) = apply_outcomes(&mut view.steps, &r, &p.secrets);
            view.applied = applied;
            view.snapshots = snapshots;
            status
        };
        Ok(view)
    }

    /// Préréglage de réseau (outil `network_preset`) : même déroulé que [`Core::network_link`].
    /// `confirmed` : l'humain a accepté les effets qui ne se défont pas (`to_confirm`) ; sans
    /// lui, rien n'est écrit (`needs_confirmation`).
    pub async fn network_preset(
        &self,
        req: NetworkPresetRequest,
        apply: Option<String>,
        confirmed: bool,
    ) -> crate::Result<NetworkPresetPlan> {
        let a = preset::parse_args(&req.to_args()).map_err(Error::Invalid)?;
        let p = preset::prepare(self, &a, false)
            .await
            .map_err(Error::Invalid)?;
        let mut view = preset_view(a.preset, &p);
        let Some(expected) = apply else {
            return Ok(view);
        };
        let todo: Vec<&Step> = p
            .plan
            .steps
            .iter()
            .filter(|s| s.status == Status::Change)
            .collect();
        view.status = if expected != view.fingerprint {
            NetworkPlanStatus::Changed
        } else if view.counts.refused > 0 {
            NetworkPlanStatus::Refused
        } else if todo.is_empty() {
            NetworkPlanStatus::NothingToDo
        } else if !view.to_confirm.is_empty() && !confirmed {
            NetworkPlanStatus::NeedsConfirmation
        } else {
            let r = crate::agent_guard::app_scope(
                "reseau",
                APP_CLIENT,
                link::apply_steps(self, &todo, None, req.snapshot, "network_preset"),
            )
            .await
            .map_err(|e| Error::Invalid(clean(&e, &p.secrets)))?;
            let (status, applied, snapshots) = apply_outcomes(&mut view.steps, &r, &p.secrets);
            view.applied = applied;
            view.snapshots = snapshots;
            status
        };
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_request_becomes_the_tool_arguments() {
        let req = LinkRequest {
            proxy_server_id: 1,
            backend_server_ids: vec![2, 3],
            names: vec![
                LinkEntryName {
                    server_id: 2,
                    name: " lobby ".into(),
                },
                LinkEntryName {
                    server_id: 3,
                    name: String::new(),
                },
            ],
            try_order: vec![LinkTarget::Server(2), LinkTarget::Entry("survie".into())],
            forced_hosts: vec![LinkForcedHost {
                host: "Play.Exemple.fr".into(),
                targets: vec![LinkTarget::Server(3)],
            }],
            prune: true,
            mode: Some(LinkMode::Legacy),
            snapshot: false,
        };
        let a = link::parse_args(&req.to_args().unwrap()).unwrap();
        assert_eq!((a.proxy, a.backends.as_slice()), (1, &[2, 3][..]));
        assert_eq!(a.names.get(&2).map(String::as_str), Some("lobby"));
        assert!(!a.names.contains_key(&3), "nom vide : automatique");
        assert_eq!(
            a.try_order,
            Some(vec![
                link::TryItem::Server(2),
                link::TryItem::Name("survie".into())
            ])
        );
        assert_eq!(
            a.forced_hosts,
            vec![(
                "play.exemple.fr".to_string(),
                vec![link::TryItem::Server(3)]
            )]
        );
        assert!(a.prune && !a.snapshot && a.dry_run);
        assert_eq!(a.mode, Some(Mode::Legacy));

        // Défauts : pas de repli imposé, mode du proxy.
        let bare = LinkRequest {
            names: Vec::new(),
            try_order: Vec::new(),
            forced_hosts: Vec::new(),
            mode: None,
            ..req.clone()
        };
        let a = link::parse_args(&bare.to_args().unwrap()).unwrap();
        assert_eq!((a.try_order, a.mode), (None, None));
        assert!(a.forced_hosts.is_empty() && a.names.is_empty());

        // Doublons refusés avant les règles de l'outil.
        let twice = LinkRequest {
            forced_hosts: vec![
                LinkForcedHost {
                    host: "a.fr".into(),
                    targets: vec![LinkTarget::Server(2)],
                },
                LinkForcedHost {
                    host: "a.fr".into(),
                    targets: vec![LinkTarget::Server(3)],
                },
            ],
            ..req.clone()
        };
        assert!(twice.to_args().unwrap_err().contains("deux fois"));
        // Et les règles de l'outil s'appliquent (le proxy n'est pas un serveur de jeu).
        let own = LinkRequest {
            backend_server_ids: vec![1],
            names: Vec::new(),
            try_order: Vec::new(),
            forced_hosts: Vec::new(),
            ..req
        };
        assert!(link::parse_args(&own.to_args().unwrap()).is_err());
    }

    #[test]
    fn link_targets_are_numbers_or_names_in_json() {
        let v = serde_json::to_value(vec![LinkTarget::Server(5), LinkTarget::Entry("hub".into())])
            .unwrap();
        assert_eq!(v, json!([5, "hub"]));
        let back: Vec<LinkTarget> = serde_json::from_value(json!([7, "x"])).unwrap();
        assert_eq!(
            back,
            vec![LinkTarget::Server(7), LinkTarget::Entry("x".into())]
        );
    }

    #[test]
    fn a_preset_request_only_carries_the_keys_of_its_preset() {
        let req = NetworkPresetRequest {
            preset: NetworkPresetKind::Geyser,
            proxy_server_id: 1,
            backend_server_ids: vec![2],
            mybox_id: None,
            database_id: None,
            bedrock_port: Some(40001),
            snapshot: true,
        };
        let a = preset::parse_args(&req.to_args()).unwrap();
        assert_eq!((a.preset, a.bedrock_port), (Preset::Geyser, Some(40001)));
        let lp = NetworkPresetRequest {
            preset: NetworkPresetKind::Luckperms,
            mybox_id: Some(9),
            database_id: Some(4),
            bedrock_port: None,
            ..req
        };
        let a = preset::parse_args(&lp.to_args()).unwrap();
        assert_eq!((a.mybox, a.database), (Some(9), Some(4)));
    }

    fn state(secret: Option<&str>) -> ProxyState {
        let mut conf = parse_velocity(
            "player-info-forwarding-mode = \"modern\"\nonline-mode = true\n[servers]\nlobby = \"203.0.113.10:25565\"\nexemple = \"127.0.0.1:30066\"\ntry = [\"lobby\"]\n",
        )
        .unwrap();
        conf.forced_hosts = vec![("play.exemple.fr".into(), vec!["lobby".into()])];
        ProxyState {
            server_id: 1,
            name: "Proxy".into(),
            conf,
            config_path: VELOCITY_TOML,
            secret: secret.map(str::to_string),
            secret_note: "/forwarding.secret".into(),
            bungeeguard: false,
            bungeeguard_token: None,
            public_port: None,
        }
    }

    #[test]
    fn the_topology_is_typed_without_any_secret() {
        let secret = "s3cr3t-du-proxy-0123456789";
        let p = state(Some(secret));
        let entry = |n: &str| p.conf.entry(n).unwrap().clone();
        let backend = BackendState {
            ty: Some(ServerType::Paper),
            version: Some("1.21.4".into()),
            online_mode: Some(true),
            paper: PaperFile::Global,
            velocity_enabled: Some(true),
            velocity_secret: Some("autre-secret-du-serveur".into()),
            spigot_file: true,
            bungeecord: Some(false),
            ..Default::default()
        };
        let links = vec![
            Link {
                entry: entry("lobby"),
                matched: Some(Matched {
                    server_id: 2,
                    how: How::Primary,
                    port_ok: true,
                }),
                server_name: Some("Survie".into()),
                server_address: Some("203.0.113.10:25565".into()),
                reachable: true,
                backend: Some(Ok(backend)),
            },
            Link {
                entry: entry("exemple"),
                matched: None,
                server_name: None,
                server_address: None,
                reachable: false,
                backend: None,
            },
        ];
        let c = Collected {
            servers: Vec::new(),
            proxies: vec![p],
            links: vec![links],
            states: HashMap::new(),
            pings: HashMap::new(),
            none_found: false,
            readable: 2,
            not_read: vec![format!("lecture : {secret}")],
            notes: Vec::new(),
            secrets: vec![secret.into(), "autre-secret-du-serveur".into()],
            focus_alone: None,
        };
        let t = topology_view(&c, &HashMap::new());
        let px = &t.proxies[0];
        assert_eq!(px.kind, NetworkProxyKind::Velocity);
        assert_eq!(px.forwarding, ForwardingMode::Modern);
        assert_eq!(px.secret_defined, Some(true));
        assert_eq!(px.forced_hosts[0].servers, ["lobby"]);
        let lobby = &px.entries[0];
        assert!(lobby.in_try && !lobby.local);
        let target = lobby.target.as_ref().unwrap();
        assert_eq!(
            (target.server_id, target.matched_by),
            (2, NetworkMatch::Primary)
        );
        let b = lobby.backend.as_ref().unwrap();
        assert_eq!(b.velocity_secret, Some(SecretCheck::Different));
        assert_eq!(b.paper_config, NetworkPaperConfig::Global);
        assert!(px.entries[1].local && px.entries[1].target.is_none());
        // Problèmes triés par gravité, et comptés.
        assert_eq!(px.problems[0].severity, NetworkSeverity::Critical);
        assert!(px
            .problems
            .windows(2)
            .all(|w| w[0].severity as u8 <= w[1].severity as u8));
        assert!(px.problems.iter().any(|x| x.link_fix));
        // Les conseils nomment l'assistant de l'app, pas l'outil MCP.
        assert!(px
            .problems
            .iter()
            .all(|x| !x.advice.contains("link_proxy") && !x.message.contains("link_proxy")));
        assert!(px
            .problems
            .iter()
            .any(|x| x.advice.contains("l'assistant « Relier des serveurs »")));
        assert_eq!(
            t.counts.critical + t.counts.important + t.counts.info,
            px.problems.len() as u32
        );
        // Aucune valeur secrète nulle part.
        let json = serde_json::to_string(&t).unwrap();
        assert!(
            !json.contains(secret) && !json.contains("autre-secret"),
            "{json}"
        );
    }

    #[test]
    fn a_failed_ping_says_why() {
        let v = ping_view(4, "Hub", None, true, Err("pas de réponse".into()));
        assert!(!v.online);
        assert_eq!(v.edition, NetworkPingEdition::Bedrock);
        assert_eq!(v.error.as_deref(), Some("pas de réponse"));
        let ok = ping_view(
            4,
            "Hub",
            Some("203.0.113.10:25565".into()),
            false,
            Ok(Pinged::Java {
                port: 25565,
                status: ping::JavaStatus {
                    version: Some("Paper 1.21.4".into()),
                    online: Some(3),
                    max: Some(20),
                    latency_ms: Some(42),
                    ..Default::default()
                },
            }),
        );
        assert!(ok.online && ok.error.is_none());
        assert_eq!(
            (ok.port, ok.latency_ms, ok.players_online),
            (Some(25565), Some(42), Some(3))
        );
    }

    #[test]
    fn the_fingerprint_follows_the_plan() {
        let a = NetworkStepCounts::default();
        let b = NetworkStepCounts {
            change: 1,
            ..Default::default()
        };
        assert_eq!(fingerprint_of(&a), fingerprint_of(&a));
        assert_ne!(fingerprint_of(&a), fingerprint_of(&b));
        assert_eq!(fingerprint_of(&a).len(), 64);
    }
}
