//! **Préréglages de réseau** composites (outil MCP `network_preset`) : monter en une fois ce que
//! chaque serveur d'un réseau proxy doit partager. `dry_run` VRAI par défaut ; à l'application,
//! chaque sous-écriture passe par [`crate::agent_guard::guarded_write`] (`install_mod`,
//! `set_config`, `copy_between_servers`) : une étape annulable chacune, l'outil composite
//! lui-même n'en crée pas (`Plan::Skip`).
//!
//! - **`luckperms`** : LuckPerms sur le proxy et chaque serveur de jeu, installé s'il manque (build
//!   de la plateforme, voir [`super::install`]) ; même base de données pour tous, lue par
//!   `db_connect` (mot de passe jamais affiché : clé secrète masquée dans les diffs, valeur effacée
//!   de toute sortie) ; `messaging-service: sql` (synchronisation par la base) ; `server:` de
//!   chaque serveur de jeu = le nom de son entrée sur le proxy (contexte des permissions). Une
//!   config pas encore générée (plugin jamais démarré) : étape `manual` (démarrer une fois, puis
//!   relancer).
//! - **`geyser`** : Geyser et Floodgate sur le proxy, Floodgate sur chaque serveur de jeu (builds
//!   GeyserMC de la plateforme) ; la clé de Floodgate (`key.pem`) est recopiée du proxy vers les
//!   serveurs par `copy_between_servers`, sans jamais être lue par l'IA (seule son empreinte est
//!   comparée en interne) ; `bedrock.port` (port UDP du proxy), `auth-type: floodgate`,
//!   `send-floodgate-data: true` sur le proxy. Le port Bedrock est une allocation du proxy : sans
//!   port utilisable, étape `manual` avec l'appel exact de l'outil officiel `manage_ports` (le
//!   numéro est attribué par la plateforme, connu seulement après l'ajout).

use super::link::{apply_steps, config_step, manual, render_step, Action, Status, Step};
use super::*;
use crate::db_connect::{self, Creds};

/// Serveurs de jeu au plus par appel.
const MAX_BACKENDS: usize = 30;
/// Dossier de Floodgate (même nom sur Paper, Velocity et BungeeCord).
const FLOODGATE_DIR: &str = "/plugins/floodgate";
const FLOODGATE_CONFIG: &str = "/plugins/floodgate/config.yml";
/// Taille maximale d'une clé de Floodgate lue (pour son empreinte).
const MAX_KEY: u64 = 64 * 1024;
/// Stockages de LuckPerms qui gardent les données sur le serveur lui-même.
const LOCAL_STORAGE: &[&str] = &["h2", "sqlite", "yaml", "json", "hocon", "toml"];

// --- Arguments ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preset {
    LuckPerms,
    Geyser,
}

impl Preset {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Preset::LuckPerms => "luckperms",
            Preset::Geyser => "geyser",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PresetArgs {
    pub preset: Preset,
    pub proxy: i64,
    pub backends: Vec<i64>,
    pub mybox: Option<i64>,
    pub database: Option<i64>,
    pub bedrock_port: Option<u16>,
    pub dry_run: bool,
    pub snapshot: bool,
}

fn int(args: &Value, k: &str) -> Result<Option<i64>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(Some)
            .ok_or_else(|| format!("`{k}` : entier attendu")),
    }
}

pub(crate) fn parse_args(args: &Value) -> Result<PresetArgs, String> {
    let preset = match args
        .get("preset")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("luckperms") => Preset::LuckPerms,
        Some("geyser") => Preset::Geyser,
        Some(other) => {
            return Err(format!(
                "`preset` « {other} » inconnu : luckperms ou geyser"
            ))
        }
        None => return Err("paramètre `preset` requis : luckperms ou geyser".into()),
    };
    let proxy =
        int(args, "proxy_server_id")?.ok_or("paramètre `proxy_server_id` (entier) requis")?;
    let backends: Vec<i64> = match args.get("backend_server_ids") {
        Some(Value::Array(a)) if !a.is_empty() => a
            .iter()
            .map(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .ok_or_else(|| format!("`backend_server_ids` : identifiant illisible ({v})"))
            })
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(
                "paramètre `backend_server_ids` (liste d'entiers, au moins un) requis : les serveurs de jeu du réseau (network_topology)".into(),
            )
        }
    };
    if backends.len() > MAX_BACKENDS {
        return Err(format!(
            "{} serveurs de jeu : {MAX_BACKENDS} au plus par appel",
            backends.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for id in &backends {
        if *id == proxy {
            return Err(format!(
                "le proxy {proxy} ne peut pas être aussi un serveur de jeu"
            ));
        }
        if !seen.insert(*id) {
            return Err(format!(
                "serveur {id} cité deux fois dans `backend_server_ids`"
            ));
        }
    }
    let bedrock_port = match int(args, "bedrock_port")? {
        None => None,
        Some(p) => Some(u16::try_from(p).map_err(|_| "`bedrock_port` : port 1-65535 attendu")?),
    };
    if preset != Preset::Geyser && bedrock_port.is_some() {
        return Err("`bedrock_port` ne sert qu'au préréglage geyser".into());
    }
    if preset != Preset::LuckPerms
        && (args.get("mybox_id").is_some() || args.get("database_id").is_some())
    {
        return Err("`mybox_id` et `database_id` ne servent qu'au préréglage luckperms".into());
    }
    let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(true);
    Ok(PresetArgs {
        preset,
        proxy,
        backends,
        mybox: int(args, "mybox_id")?,
        database: int(args, "database_id")?,
        bedrock_port,
        dry_run: flag("dry_run"),
        snapshot: flag("snapshot"),
    })
}

// --- Serveurs lus ---------------------------------------------------------------------------------------

/// Fichiers d'un serveur utiles aux préréglages.
#[derive(Debug, Clone, Default)]
pub(crate) struct NodeFiles {
    /// Noms des fichiers de `/plugins`.
    pub plugins: Vec<String>,
    /// Config de LuckPerms : chemin et texte.
    pub luckperms: Option<(String, String)>,
    /// Config de Geyser : chemin et texte.
    pub geyser: Option<(String, String)>,
    /// Config de Floodgate (texte).
    pub floodgate: Option<String>,
    /// Empreinte sha256 de la clé de Floodgate — jamais son contenu.
    pub key: Option<String>,
}

/// Un serveur du réseau.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub server_id: i64,
    pub name: String,
    pub ty: Option<ServerType>,
    pub version: Option<String>,
    pub proxy: bool,
    /// Serveur de jeu : nom de son entrée sur le proxy.
    pub entry: Option<String>,
    pub files: Result<NodeFiles, String>,
}

impl Node {
    fn label(&self) -> String {
        format!("« {} » ({})", self.name, self.server_id)
    }

    fn platform(&self) -> &'static str {
        self.ty.map(ServerType::label).unwrap_or("inconnue")
    }
}

/// Chemins possibles de la config de LuckPerms (Velocity : dossier `luckperms`, l'identifiant du
/// plugin ; ailleurs `LuckPerms`).
pub(crate) fn luckperms_paths(ty: Option<ServerType>) -> [&'static str; 2] {
    if ty == Some(ServerType::Velocity) {
        [
            "/plugins/luckperms/config.yml",
            "/plugins/LuckPerms/config.yml",
        ]
    } else {
        [
            "/plugins/LuckPerms/config.yml",
            "/plugins/luckperms/config.yml",
        ]
    }
}

/// Chemins possibles de la config de Geyser selon la plateforme.
pub(crate) fn geyser_paths(ty: Option<ServerType>) -> &'static [&'static str] {
    match ty {
        Some(ServerType::Velocity) => &[
            "/plugins/geyser/config.yml",
            "/plugins/Geyser-Velocity/config.yml",
        ],
        Some(ServerType::Bungee) => &["/plugins/Geyser-BungeeCord/config.yml"],
        _ => &["/plugins/Geyser-Spigot/config.yml"],
    }
}

/// Nom du fichier de clé de Floodgate (`key-file-name`, défaut `key.pem`), sans chemin.
pub(crate) fn key_file(floodgate_config: Option<&str>) -> String {
    floodgate_config
        .map(yaml_tree)
        .and_then(|t| text_of(t.get("key-file-name")))
        .filter(|n| !n.contains(['/', '\\']) && !n.starts_with('.'))
        .unwrap_or_else(|| "key.pem".into())
}

/// Plugin présent dans `/plugins` (actif : `.jar`) ?
pub(crate) fn has_plugin(plugins: &[String], which: &str) -> bool {
    plugins.iter().any(|p| {
        let c = compact(p);
        c.ends_with("jar")
            && match which {
                "luckperms" => c.starts_with("luckperms"),
                "floodgate" => c.starts_with("floodgate"),
                "geyser" => {
                    c.starts_with("geyser")
                        && !c.contains("floodgate")
                        && (c == "geyserjar"
                            || ["velocity", "bungee", "spigot", "paper"]
                                .iter()
                                .any(|x| c.contains(x)))
                }
                _ => false,
            }
    })
}

fn plugin_platform(ty: Option<ServerType>) -> bool {
    matches!(
        ty,
        Some(ServerType::Paper | ServerType::Spigot | ServerType::Velocity | ServerType::Bungee)
    )
}

// --- LuckPerms ----------------------------------------------------------------------------------------

/// Plan d'un préréglage : étapes dans l'ordre d'application, avertissements, effets à confirmer.
#[derive(Debug, Clone, Default)]
pub(crate) struct PresetPlan {
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    /// Effets qui ne se défont pas entièrement : confirmés par l'humain avant l'application.
    pub destructive: Vec<String>,
}

/// Version de LuckPerms à installer sur un serveur : `(version_id, libellé)`.
pub(crate) type VersionPick = Result<(String, String), String>;

/// Préréglage `luckperms` (pur). `password` : `None` s'il n'est pas lisible.
pub(crate) fn plan_luckperms(
    nodes: &[Node],
    creds: &Creds,
    password: Option<&str>,
    versions: &HashMap<i64, VersionPick>,
    redact: &dyn Fn(&str) -> String,
) -> Result<PresetPlan, String> {
    let mut plan = PresetPlan::default();
    let base = password
        .map(|pw| db_connect::preset_changes("luckperms", creds, pw).map(|(_, c)| c))
        .transpose()?;
    if base.is_none() {
        plan.warnings.push(format!(
            "mot de passe de la base {} illisible par l'API : aucune config n'est écrite ; connect_database avec rotate_password=true (sur l'un des serveurs, confirmation demandée) le réinitialise, puis relance network_preset",
            creds.id
        ));
    }
    for n in nodes {
        let (id, who) = (n.server_id, n.name.as_str());
        let files = match &n.files {
            Ok(f) => f,
            Err(e) => {
                plan.steps.push(manual(
                    id,
                    who,
                    "unread",
                    format!(
                        "{} illisible ({e}) : rien n'est fait sur ce serveur.",
                        n.label()
                    ),
                ));
                continue;
            }
        };
        if !plugin_platform(n.ty) {
            plan.steps.push(manual(id, who, "platform", format!("{} (plateforme {}) : LuckPerms en mod (Fabric, Forge) ou serveur vanilla, non pris en charge par le préréglage ; à régler à la main avec la même base.", n.label(), n.platform())));
            continue;
        }
        if !has_plugin(&files.plugins, "luckperms") {
            match versions.get(&id) {
                Some(Ok((version_id, label))) => {
                    let mut s = Step::new(
                        format!("{id}.install"),
                        id,
                        who,
                        Status::Change,
                        format!("installer LuckPerms {label} (build {})", n.platform()),
                    );
                    s.action = Some(Action::Install {
                        source: "modrinth".into(),
                        slug: "luckperms".into(),
                        version_id: version_id.clone(),
                    });
                    plan.steps.push(s);
                }
                Some(Err(e)) => plan.steps.push(manual(
                    id,
                    who,
                    "install",
                    format!(
                        "{} : LuckPerms absent, aucune version installable trouvée ({e}).",
                        n.label()
                    ),
                )),
                None => plan.steps.push(manual(
                    id,
                    who,
                    "install",
                    format!("{} : LuckPerms absent.", n.label()),
                )),
            }
            plan.steps.push(manual(id, who, "config", format!("{} : LuckPerms doit générer sa configuration avant d'être réglé — démarre le serveur une fois après l'installation (power_action start, wait_for state=running), arrête-le, puis relance network_preset avec les mêmes arguments (idempotent).", n.label())));
            continue;
        }
        let Some((path, text)) = &files.luckperms else {
            plan.steps.push(manual(id, who, "config", format!("{} : LuckPerms est installé mais sa configuration n'existe pas encore ({}) — démarre le serveur une fois, arrête-le, puis relance network_preset (idempotent).", n.label(), luckperms_paths(n.ty)[0])));
            continue;
        };
        let Some(base) = &base else {
            plan.steps.push(manual(
                id,
                who,
                "password",
                format!(
                    "{} : config non écrite (mot de passe de la base illisible, voir warnings).",
                    n.label()
                ),
            ));
            continue;
        };
        let mut changes = base.clone();
        changes.push(("messaging-service".into(), json!("sql")));
        if let (false, Some(entry)) = (n.proxy, &n.entry) {
            changes.push(("server".into(), json!(entry)));
        }
        // Bascule depuis un stockage local, ou vers une autre base : les données ne suivent pas.
        let tree = yaml_tree(text);
        let storage = text_of(tree.get("storage-method")).map(|s| s.to_ascii_lowercase());
        let address = text_of(yaml_lite::path(&tree, &["data", "address"]));
        let database = text_of(yaml_lite::path(&tree, &["data", "database"]));
        match storage.as_deref() {
            Some(s) if LOCAL_STORAGE.contains(&s) => plan.destructive.push(format!(
                "{} : stockage actuel « {s} » (local) — ses groupes et permissions ne passent pas dans la base : /lp export avant, puis /lp import sur un serveur après",
                n.label()
            )),
            Some("mysql" | "mariadb")
                if address.as_deref().is_some_and(|a| a != format!("{}:{}", creds.host, creds.port))
                    || database.as_deref().is_some_and(|d| d != creds.database) =>
            {
                plan.destructive.push(format!(
                    "{} : utilisait déjà une autre base MySQL/MariaDB — elle n'est plus lue",
                    n.label()
                ))
            }
            _ => {}
        }
        plan.steps.push(config_step(
            format!("{id}.luckperms"),
            id,
            who,
            path,
            text,
            changes,
            redact,
        ));
    }
    Ok(plan)
}

// --- Geyser ------------------------------------------------------------------------------------------------

/// Port UDP de Geyser sur le proxy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PortChoice {
    Use {
        port: u16,
        why: String,
    },
    /// Aucun port utilisable : l'appel officiel qui en ajoute un.
    Missing {
        call: Value,
    },
}

/// Port Bedrock du proxy : `bedrock_port` (un de ses ports), sinon le port actuel de Geyser s'il
/// est l'un des siens, sinon un port secondaire décrit « geyser » ou « bedrock » ; à défaut,
/// l'appel `manage_ports` à faire (pur).
pub(crate) fn choose_port(
    proxy_id: i64,
    ep: &Endpoint,
    raw: &Value,
    asked: Option<u16>,
    geyser_config: Option<&str>,
) -> Result<PortChoice, String> {
    let own: Vec<u16> = std::iter::once(ep.port)
        .chain(ep.secondary.iter().map(|(_, p)| *p))
        .collect();
    if let Some(p) = asked {
        if !own.contains(&p) {
            return Err(format!(
                "`bedrock_port` {p} : ce n'est pas un port du proxy ({}) ; ajoute-en un avec l'outil officiel manage_ports, puis relance avec le port attribué",
                own.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
        let why = if p == ep.port {
            "bedrock_port demandé : port principal (même numéro que Java en TCP ; l'UDP doit y être ouvert — vérifie avec ping_server bedrock=true)".to_string()
        } else {
            "bedrock_port demandé".to_string()
        };
        return Ok(PortChoice::Use { port: p, why });
    }
    if let Some(cur) = geyser_config
        .map(yaml_tree)
        .and_then(|t| yaml_lite::path(&t, &["bedrock", "port"]).and_then(Value::as_i64))
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != ep.port && own.contains(p))
    {
        return Ok(PortChoice::Use {
            port: cur,
            why: "port actuel de Geyser, alloué au proxy".into(),
        });
    }
    let described = raw
        .get("ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|x| {
            let port = u16::try_from(x.get("port")?.as_i64()?).ok()?;
            let d = x.get("description")?.as_str()?.to_string();
            let l = d.to_ascii_lowercase();
            (l.contains("geyser") || l.contains("bedrock")).then_some((port, d))
        });
    if let Some((port, d)) = described {
        return Ok(PortChoice::Use {
            port,
            why: format!("port secondaire du proxy décrit « {d} »"),
        });
    }
    Ok(PortChoice::Missing {
        call: json!({ "server_id": proxy_id, "action": "add", "type": "public", "description": "Geyser (Bedrock)" }),
    })
}

/// Clé d'`auth-type` de Geyser : `java.auth-type` (configuration récente) ou `remote.auth-type`.
pub(crate) fn auth_key(geyser_tree: &Value) -> Option<&'static str> {
    if geyser_tree.get("java").is_some_and(Value::is_object) {
        Some("java.auth-type")
    } else if geyser_tree.get("remote").is_some_and(Value::is_object) {
        Some("remote.auth-type")
    } else {
        None
    }
}

/// Préréglage `geyser` (pur). `builds` : libellés des builds GeyserMC (`geyser`, `floodgate`).
pub(crate) fn plan_geyser(
    proxy: &Node,
    forwarding: &Forwarding,
    backends: &[Node],
    port: &PortChoice,
    builds: &HashMap<&'static str, String>,
    redact: &dyn Fn(&str) -> String,
) -> PresetPlan {
    let mut plan = PresetPlan::default();
    let (pid, pname) = (proxy.server_id, proxy.name.as_str());
    let label = |p: &str| {
        builds
            .get(p)
            .cloned()
            .unwrap_or_else(|| "dernière build".into())
    };
    let install = |id: i64, who: &str, project: &'static str, platform: &str| {
        let mut s = Step::new(
            format!("{id}.{project}"),
            id,
            who,
            Status::Change,
            format!(
                "installer {} {} (build {platform}, GeyserMC)",
                if project == "geyser" {
                    "Geyser"
                } else {
                    "Floodgate"
                },
                label(project)
            ),
        );
        s.action = Some(Action::Install {
            source: "geysermc".into(),
            slug: project.into(),
            version_id: "latest".into(),
        });
        s
    };
    let files = match &proxy.files {
        Ok(f) => f,
        Err(e) => {
            plan.steps.push(manual(
                pid,
                pname,
                "unread",
                format!("{} illisible ({e}) : rien n'est fait.", proxy.label()),
            ));
            return plan;
        }
    };
    if *forwarding == Forwarding::None {
        plan.steps.push(manual(pid, pname, "forwarding", format!("{} : transfert des joueurs désactivé — Floodgate sur les serveurs de jeu en a besoin. Relie d'abord le réseau avec link_proxy (dry_run d'abord), puis relance network_preset.", proxy.label())));
    }
    let gaming: Vec<&Node> = backends
        .iter()
        .filter(|b| matches!(b.ty, Some(ServerType::Paper | ServerType::Spigot)))
        .collect();
    // Proxy : Geyser et Floodgate.
    let geyser_in = has_plugin(&files.plugins, "geyser");
    let floodgate_in = has_plugin(&files.plugins, "floodgate");
    if !geyser_in {
        plan.steps
            .push(install(pid, pname, "geyser", proxy.platform()));
    }
    if !floodgate_in {
        plan.steps
            .push(install(pid, pname, "floodgate", proxy.platform()));
    }
    if !geyser_in || !floodgate_in {
        plan.steps.push(manual(pid, pname, "start", format!("{} : Geyser et Floodgate génèrent leur configuration et la clé de Floodgate au premier démarrage — démarre le proxy une fois après l'installation, arrête-le, puis relance network_preset (idempotent).", proxy.label())));
    }
    match (&files.geyser, port) {
        (Some((path, text)), _) if geyser_in => {
            let tree = yaml_tree(text);
            let mut changes = Vec::new();
            match auth_key(&tree) {
                Some(k) => changes.push((k.to_string(), json!("floodgate"))),
                None => plan.steps.push(manual(pid, pname, "geyser-auth", format!("{path} : section `java` ou `remote` introuvable — règle auth-type: floodgate à la main."))),
            }
            if let PortChoice::Use { port, .. } = port {
                changes.push(("bedrock.port".into(), json!(port)));
                if yaml_lite::as_bool(yaml_lite::path(&tree, &["bedrock", "clone-remote-port"])) == Some(true) {
                    changes.push(("bedrock.clone-remote-port".into(), json!(false)));
                }
            }
            plan.steps.push(config_step(format!("{pid}.geyser-config"), pid, pname, path, text, changes, redact));
        }
        (None, _) if geyser_in => plan.steps.push(manual(pid, pname, "geyser-config", format!("{} : configuration de Geyser absente ({}) — démarre le proxy une fois, puis relance.", proxy.label(), geyser_paths(proxy.ty)[0]))),
        _ => {}
    }
    if let PortChoice::Missing { call } = port {
        plan.steps.push(manual(pid, pname, "port", format!(
            "{} : aucun port UDP pour Bedrock. Ajoute-en un avec l'outil officiel manage_ports (passerelle MCP officielle ou panel) : {call} — le numéro est attribué par la plateforme (plage 40000-49999 sur une IP partagée ; sur une IP MyShield, seuls 19132 et 25565-25585 sont joignables) ; lis-le dans sa réponse (ou get_server → ports), puis relance network_preset avec bedrock_port=<ce port>.",
            proxy.label()
        )));
    }
    match &files.floodgate {
        Some(text) if floodgate_in => {
            let changes = if gaming.is_empty() { Vec::new() } else { vec![("send-floodgate-data".to_string(), json!(true))] };
            plan.steps.push(config_step(format!("{pid}.floodgate-config"), pid, pname, FLOODGATE_CONFIG, text, changes, redact));
        }
        None if floodgate_in => plan.steps.push(manual(pid, pname, "floodgate-config", format!("{} : configuration de Floodgate absente ({FLOODGATE_CONFIG}) — démarre le proxy une fois, puis relance.", proxy.label()))),
        _ => {}
    }
    let key_name = key_file(files.floodgate.as_deref());
    let proxy_key = files.key.clone().filter(|_| floodgate_in);
    if floodgate_in && proxy_key.is_none() {
        plan.steps.push(manual(pid, pname, "key", format!("{} : clé de Floodgate absente ({FLOODGATE_DIR}/{key_name}) — elle est créée au premier démarrage de Floodgate : démarre le proxy une fois, puis relance.", proxy.label())));
    }
    // Serveurs de jeu : Floodgate et la clé du proxy.
    for b in backends {
        let (id, who) = (b.server_id, b.name.as_str());
        let f = match &b.files {
            Ok(f) => f,
            Err(e) => {
                plan.steps.push(manual(
                    id,
                    who,
                    "unread",
                    format!(
                        "{} illisible ({e}) : rien n'est fait sur ce serveur.",
                        b.label()
                    ),
                ));
                continue;
            }
        };
        if !matches!(b.ty, Some(ServerType::Paper | ServerType::Spigot)) {
            plan.steps.push(manual(id, who, "platform", format!("{} (plateforme {}) : Floodgate s'y installe à la main (mod Floodgate pour Fabric ou NeoForge), avec la clé du proxy.", b.label(), b.platform())));
            continue;
        }
        if !has_plugin(&f.plugins, "floodgate") {
            plan.steps.push(install(id, who, "floodgate", "spigot"));
        }
        let Some(pk) = &proxy_key else { continue };
        let target = key_file(f.floodgate.as_deref());
        if target != key_name {
            plan.warnings.push(format!("{} : Floodgate y attend sa clé sous « {target} » (key-file-name), le proxy sous « {key_name} » : la clé est copiée sous « {key_name} », aligne key-file-name à la main", b.label()));
        }
        match &f.key {
            Some(k) if k == pk => plan.steps.push(Step::new(format!("{id}.key"), id, who, Status::Ok, format!("{FLOODGATE_DIR}/{key_name} : identique à celle du proxy (comparée par empreinte, jamais lue)"))),
            other => {
                let overwrite = other.is_some();
                if overwrite {
                    plan.destructive.push(format!("{} : sa clé de Floodgate, différente de celle du proxy, est remplacée (l'ancienne n'est pas restaurable par l'annulation)", b.label()));
                }
                let mut s = Step::new(format!("{id}.key"), id, who, Status::Change, format!(
                    "{FLOODGATE_DIR}/{key_name} : {} la clé du proxy (copiée de serveur à serveur, jamais lue ni affichée)",
                    if overwrite { "remplacée par" } else { "reçoit" }
                ));
                s.action = Some(Action::CopyFile { from_server: pid, from_path: format!("{FLOODGATE_DIR}/{key_name}"), to_dir: FLOODGATE_DIR.into(), overwrite });
                plan.steps.push(s);
            }
        }
    }
    plan
}

// --- Lecture -----------------------------------------------------------------------------------------------

/// Fichiers d'un serveur pour un préréglage (SFTP). La clé de Floodgate n'est lue que pour son
/// empreinte.
async fn read_files(
    core: &Core,
    id: i64,
    ty: Option<ServerType>,
    preset: Preset,
) -> Result<NodeFiles, String> {
    core.with_sftp(id, |conn| async move {
        let mut f = NodeFiles {
            plugins: list_names(&conn, "/plugins").await?,
            ..Default::default()
        };
        match preset {
            Preset::LuckPerms => {
                for p in luckperms_paths(ty) {
                    if let Some(t) = read_optional(&conn, p).await? {
                        f.luckperms = Some((p.to_string(), t));
                        break;
                    }
                }
            }
            Preset::Geyser => {
                for p in geyser_paths(ty) {
                    if let Some(t) = read_optional(&conn, p).await? {
                        f.geyser = Some((p.to_string(), t));
                        break;
                    }
                }
                f.floodgate = read_optional(&conn, FLOODGATE_CONFIG).await?;
                let key = format!("{FLOODGATE_DIR}/{}", key_file(f.floodgate.as_deref()));
                f.key = match sftp::read_bytes(&conn, &key, MAX_KEY).await {
                    Ok(b) if !b.is_empty() => Some(crate::util::hex(
                        &<sha2::Sha256 as sha2::Digest>::digest(&b),
                    )),
                    Ok(_) => None,
                    Err(e) if sftp::is_session_dead(&e) => return Err(e),
                    Err(_) => None,
                };
            }
        }
        Ok(f)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Version de LuckPerms pour un serveur : build de proxy sur Modrinth, sinon catalogue MineStrator.
async fn luckperms_version(core: &Core, n: &Node) -> VersionPick {
    if let Some(loaders) = super::install::proxy_loaders(n.ty) {
        let v = super::install::modrinth_versions("luckperms", loaders, "").await?;
        let pick = super::install::newest(&v)
            .ok_or_else(|| format!("aucune build {} de LuckPerms sur Modrinth", loaders[0]))?;
        return Ok((pick.id.clone(), pick.version_number.clone()));
    }
    let platform = match n.ty {
        Some(ServerType::Spigot) => "spigot",
        _ => "paper",
    };
    let gv = n.version.clone().unwrap_or_default();
    let versions = core
        .market_versions("modrinth", "luckperms", "", &gv)
        .await
        .map_err(|e| format!("versions de LuckPerms illisibles : {e}"))?;
    let pick = crate::pregen::pick_version(&versions, platform, &gv)
        .ok_or_else(|| format!("aucune version de LuckPerms pour {platform} {gv}"))?;
    Ok((pick.id.clone(), pick.version_number.clone()))
}

// --- Outil ----------------------------------------------------------------------------------------------------

fn render(
    a: &PresetArgs,
    nodes: &[Node],
    plan: &PresetPlan,
    outcomes: &HashMap<String, (String, Option<String>)>,
) -> Value {
    let count = |st: Status| plan.steps.iter().filter(|s| s.status == st).count();
    let manual: Vec<&str> = plan
        .steps
        .iter()
        .filter(|s| s.status == Status::Manual)
        .map(|s| s.summary.as_str())
        .collect();
    let mut v = json!({
        "preset": a.preset.label(),
        "dry_run": a.dry_run,
        "servers": nodes.iter().map(|n| json!({
            "server_id": n.server_id,
            "name": n.name,
            "role": if n.proxy { "proxy" } else { "serveur de jeu" },
            "type": n.platform(),
            "entry": n.entry,
        })).collect::<Vec<_>>(),
        "counts": { "change": count(Status::Change), "ok": count(Status::Ok), "manual": count(Status::Manual), "refused": count(Status::Refused) },
        "steps": plan.steps.iter().map(|s| render_step(s, outcomes.get(&s.id))).collect::<Vec<_>>(),
    });
    if !manual.is_empty() {
        v["manual"] = json!(manual);
    }
    if !plan.warnings.is_empty() {
        v["warnings"] = json!(plan.warnings);
    }
    if !plan.destructive.is_empty() {
        v["to_confirm"] = json!(plan.destructive);
    }
    v
}

/// Outil MCP `network_preset`.
pub(crate) async fn network_preset_tool(core: &Core, args: &Value) -> Result<String, String> {
    let a = parse_args(args)?;
    crate::mcp_ctx::progress(0.0, None, "lecture du réseau").await;
    let servers = mc_servers(core).await?;
    let find = |id: i64| -> Result<&McServer, String> {
        servers.iter().find(|s| s.id == id).ok_or_else(|| {
            format!("serveur {id} introuvable parmi les serveurs Minecraft Java du compte (list_servers)")
        })
    };
    let p = find(a.proxy)?;
    if !p.readable() {
        return Err(format!(
            "le proxy {} est {} : fichiers inaccessibles",
            p.label(),
            p.status_label()
        ));
    }
    if let Some(t) = p.ty.filter(|t| !t.is_proxy()) {
        return Err(format!(
            "{} n'est pas un proxy : plateforme {} (Velocity ou BungeeCord attendu)",
            p.label(),
            t.label()
        ));
    }
    let (proxy_state, _) = read_proxy(core, p.id, &p.name, p.ty)
        .await?
        .ok_or_else(|| format!("{} n'est pas un proxy Velocity ou BungeeCord (ni velocity.toml ni config.yml de BungeeCord)", p.label()))?;
    let proxy_ty = Some(match proxy_state.conf.kind {
        ProxyKind::Velocity => ServerType::Velocity,
        ProxyKind::Bungee => ServerType::Bungee,
    });
    let mut chosen = Vec::new();
    for id in &a.backends {
        let s = find(*id)?;
        if s.ty.is_some_and(ServerType::is_proxy) {
            return Err(format!("{} est un proxy, pas un serveur de jeu", s.label()));
        }
        chosen.push(s.clone());
    }
    // Nom de chaque serveur de jeu sur le proxy (contexte `server` de LuckPerms).
    let eps: Vec<Endpoint> = chosen.iter().filter_map(|s| s.endpoint.clone()).collect();
    let entry_of = |s: &McServer| {
        proxy_state
            .conf
            .entries
            .iter()
            .find(|e| {
                match_entry(&Addr::parse(&e.address), &[], &eps)
                    .is_some_and(|m| m.server_id == s.id)
            })
            .map(|e| e.name.clone())
            .unwrap_or_else(|| super::link::slug(&s.name, s.id))
    };
    let mut nodes = vec![Node {
        server_id: p.id,
        name: p.name.clone(),
        ty: proxy_ty,
        version: p.version.clone(),
        proxy: true,
        entry: None,
        files: read_files(core, p.id, proxy_ty, a.preset).await,
    }];
    let preset = a.preset;
    let tasks: Vec<_> = chosen
        .iter()
        .map(|s| {
            let entry = entry_of(s);
            async move {
                let files = if s.readable() {
                    read_files(core, s.id, s.ty, preset).await
                } else {
                    Err(format!("serveur {}", s.status_label()))
                };
                Node {
                    server_id: s.id,
                    name: s.name.clone(),
                    ty: s.ty,
                    version: s.version.clone(),
                    proxy: false,
                    entry: Some(entry),
                    files,
                }
            }
        })
        .collect();
    nodes.extend(
        stream::iter(tasks)
            .buffered(SFTP_CONCURRENCY)
            .collect::<Vec<_>>()
            .await,
    );
    if crate::mcp_ctx::cancelled() {
        return Err("annulé : rien n'a été modifié".into());
    }
    crate::mcp_ctx::progress(1.0, Some(3.0), "préparation du plan").await;
    let redact = |s: &str| core.redact_ai(s);
    let mut secrets: Vec<String> = proxy_state.secrets();
    let mut extra = json!({});
    let plan = match a.preset {
        Preset::LuckPerms => {
            let mybox = match a.mybox {
                Some(m) => m,
                None => db_connect::mybox_of(core, a.proxy).await?,
            };
            let creds = db_connect::load_creds(core, mybox, a.database).await?;
            secrets.extend(creds.password.clone());
            let mut versions: HashMap<i64, VersionPick> = HashMap::new();
            for n in &nodes {
                let missing = n
                    .files
                    .as_ref()
                    .is_ok_and(|f| !has_plugin(&f.plugins, "luckperms"));
                if missing && plugin_platform(n.ty) {
                    versions.insert(n.server_id, luckperms_version(core, n).await);
                }
            }
            extra["database"] = json!({
                "mybox_id": mybox,
                "id": creds.id,
                "label": creds.label,
                "host": creds.host,
                "port": creds.port,
                "database": creds.database,
                "username": creds.username,
                "password": if creds.password.is_some() { "lu par l'API, jamais affiché" } else { "non lisible" },
            });
            plan_luckperms(
                &nodes,
                &creds,
                creds.password.as_deref(),
                &versions,
                &redact,
            )
            .map_err(|e| scrub_text(&e, &secrets))?
        }
        Preset::Geyser => {
            let token = core.token().map_err(|e| e.to_string())?;
            let raw = core
                .api
                .server_value(&token, a.proxy)
                .await
                .map_err(|e| format!("proxy illisible : {e}"))?;
            let ep = Endpoint::from_server_value(a.proxy, &raw)
                .or_else(|| p.endpoint.clone())
                .ok_or_else(|| format!("{} : adresse introuvable", p.label()))?;
            let geyser_text = nodes[0]
                .files
                .as_ref()
                .ok()
                .and_then(|f| f.geyser.as_ref())
                .map(|(_, t)| t.as_str());
            let port = choose_port(a.proxy, &ep, &raw, a.bedrock_port, geyser_text)?;
            extra["bedrock_port"] = match &port {
                PortChoice::Use { port, why } => json!({ "port": port, "source": why }),
                PortChoice::Missing { call } => json!({ "port": null, "manage_ports": call }),
            };
            if raw
                .pointer("/server/myshield")
                .is_some_and(|m| !m.is_null())
            {
                extra["myshield"] = json!("IP MyShield : un port public n'y est joignable que dans server.myshield.open_port_ranges (19132 et 25565-25585 au 16/11/2026)");
            }
            let mut builds: HashMap<&'static str, String> = HashMap::new();
            for project in ["geyser", "floodgate"] {
                if let Ok(b) = super::install::geyser_build(project, "latest").await {
                    builds.insert(project, format!("{} (build {})", b.version, b.build));
                }
            }
            plan_geyser(
                &nodes[0],
                &proxy_state.conf.forwarding,
                &nodes[1..],
                &port,
                &builds,
                &redact,
            )
        }
    };
    let finish = |mut v: Value, status: &str, next: &str| {
        if let Value::Object(m) = &extra {
            for (k, x) in m {
                v[k.as_str()] = x.clone();
            }
        }
        v["status"] = json!(status);
        v["next"] = json!(next);
        crate::config_edit::render_for_ai(core, scrub_value(v, &secrets))
    };
    let outcomes: HashMap<String, (String, Option<String>)> = HashMap::new();
    let todo: Vec<&Step> = plan
        .steps
        .iter()
        .filter(|s| s.status == Status::Change)
        .collect();
    let refused = plan.steps.iter().any(|s| s.status == Status::Refused);
    let verify = match a.preset {
        Preset::LuckPerms => "Redémarre chaque serveur (power_action restart), puis vérifie la console : LuckPerms doit se connecter à la base (wait_for « Successfully enabled » ou error_digest) ; /lp info indique le stockage et le service de messagerie.",
        Preset::Geyser => "Redémarre les serveurs de jeu puis le proxy (power_action restart), puis vérifie : ping_server du proxy avec bedrock=true et le port Bedrock, tail_console du proxy (Geyser démarré, aucune erreur de clé Floodgate).",
    };
    if a.dry_run {
        let next = if refused {
            "Des étapes sont refusées (fichier invalide) : corrige-les d'abord, rien ne sera écrit tant qu'elles restent."
        } else if todo.is_empty() {
            "Rien à écrire pour l'instant : fais les étapes manual, puis relance network_preset."
        } else {
            "Montre ce plan à l'utilisateur (et `to_confirm` s'il y en a) ; après son accord, relance avec dry_run=false (snapshot de chaque serveur modifié d'abord, sauf snapshot=false)."
        };
        return Ok(finish(
            render(&a, &nodes, &plan, &outcomes),
            "simulation : rien n'a été écrit",
            next,
        ));
    }
    if refused {
        return Err(finish(
            render(&a, &nodes, &plan, &outcomes),
            "refusé : rien n'a été écrit",
            "Corrige les étapes refused (fichier invalide : edit_file), puis relance.",
        ));
    }
    if todo.is_empty() {
        return Ok(finish(
            render(&a, &nodes, &plan, &outcomes),
            "rien à écrire",
            "Fais les étapes manual, puis relance network_preset (idempotent).",
        ));
    }
    // Effets qui ne se défont pas entièrement : l'humain confirme AVANT toute écriture (en MCP sans
    // état, l'appel est rejoué après sa réponse : rien n'a encore été fait).
    if !plan.destructive.is_empty() {
        let message = format!(
            "Appliquer le préréglage {} sur le réseau du proxy « {} » ? À savoir : {}",
            a.preset.label(),
            p.name,
            plan.destructive.join(" ; ")
        );
        if crate::mcp_ctx::confirm(&message, None).await == crate::mcp_ctx::Confirmation::Declined {
            return Err("Refusé par l'utilisateur : rien n'a été modifié.".into());
        }
    }
    crate::mcp_ctx::progress(2.0, Some(3.0), "application").await;
    // Le mot de passe de la base voyage dans les modifications de config (set_config interne,
    // masqué par config_edit) : aucun secret de fichier à écrire ici.
    let r = apply_steps(core, &todo, None, a.snapshot, "network_preset")
        .await
        .map_err(|e| scrub_text(&e, &secrets))?;
    let mut v = render(&a, &nodes, &plan, &r.outcomes);
    v["applied"] = json!(r.applied);
    if !r.snapshots.is_empty() {
        v["snapshots"] = json!(r.snapshots);
    }
    let status = if r.failed {
        "terminé avec un échec : étapes suivantes non exécutées"
    } else if r.cancelled {
        "annulé : étapes suivantes non exécutées"
    } else {
        "terminé"
    };
    let next = if r.failed || r.cancelled {
        "Les étapes réussies sont annulables (list_changesets puis rollback_changeset) ; ou corrige la cause et relance network_preset (idempotent)."
    } else if plan.steps.iter().any(|s| s.status == Status::Manual) {
        "Fais les étapes manual (souvent : démarrer une fois les serveurs où un plugin vient d'être installé), puis relance network_preset pour terminer."
    } else {
        verify
    };
    Ok(finish(v, status, next))
}

#[cfg(test)]
mod tests;
