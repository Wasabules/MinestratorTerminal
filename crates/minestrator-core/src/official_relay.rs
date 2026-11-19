//! Outils du **MCP officiel** MineStrator : qui peut voir et appeler quoi.
//!
//! Le transport est dans [`crate::official_mcp`] ; ici vit la **politique**, partagée par deux
//! consommateurs :
//!
//! - **le Copilote** (`copilot.rs`). Il lit du contenu non fiable (chat des joueurs, logs), donc
//!   tout ce qu'une injection pourrait lui faire faire doit rester borné :
//!   - **deny-by-default** : les outils de **lecture** (ceux que l'officiel garde en `readonly=1`)
//!     sont autorisés ; parmi les outils **modifiants**, seule la liste blanche
//!     [`COPILOT_OFFICIAL_WRITES`] (limités à un serveur, raisonnablement réversibles) l'est, et
//!     seulement en mode autonome (ou sur « Appliquer ») ; tout le reste — MyBox, réinstallation,
//!     restaurations, suppressions, bases de données, RCON, tickets, renommage, ports, adresse,
//!     Java, changement de version, modpacks, mondes, écritures de schedules, et **tout outil
//!     inconnu** — est exclu, au listing **et** à l'exécution ;
//!   - **portée** : un appel dont `server_id` vise un autre serveur que celui de la session est
//!     refusé ;
//!   - les agents CLI ne se connectent plus à l'officiel : notre serveur `--mcp` le **relaie** avec
//!     cette même politique (variables d'env [`crate::config::MCP_COPILOT_SERVER_ENV`] et
//!     [`crate::config::MCP_OFFICIAL_ENV`]) — la clé API ne quitte plus le trousseau.
//! - **la passerelle** du serveur MCP local (`mcp.rs`, désactivée par défaut) : un client MCP
//!   (Claude Code…) n'a qu'une connexion, et la clé reste au trousseau au lieu d'être en clair dans
//!   sa config. Tous les toolsets sauf `files` (notre SFTP le remplace) ; lecture = liste
//!   `readonly=1`, tout le reste (inconnu compris) est modifiant et soumis à `allow_writes` ; un
//!   outil **destructif** (annotation `destructiveHint` absente ou vraie) n'est relayé qu'avec une
//!   confirmation `confirm_name` **vérifiée localement** (nom exact du serveur, du snapshot, du site
//!   ou de la MyBox), sinon il est masqué.
//!
//! **Table d'équivalences** ([`EQUIVALENTS`]) : un outil officiel qui fait doublon avec un des
//! nôtres est masqué (ou, pour la paire où l'officiel est meilleur, c'est le nôtre qui l'est) —
//! un seul outil par fonction, quel que soit le consommateur.
//!
//! Sorties : toujours passées par `Core::redact_ai`, comme celles de nos outils ; l'argument
//! `reveal_credentials` (mots de passe SFTP / FTP / base en clair) est retiré des schémas et des
//! appels. La clé API est lue au trousseau (compte actif) à chaque appel et n'apparaît jamais dans
//! une sortie ni un journal.

use crate::config::{MCP_COPILOT_SERVER_ENV, MCP_OFFICIAL_ENV};
use crate::mcp::McpConfig;
use crate::Core;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Toolsets délégués au Copilote : la gestion d'un serveur de jeu. Ni `files` (notre SFTP), ni
/// `webhosting` (hors du serveur de la session).
pub(crate) const COPILOT_TOOLSETS: &str =
    "core,actions,content,backups,mybox,schedules,databases,config";

/// Toolsets relayés par la passerelle du MCP local : tous sauf `files`, que notre SFTP remplace.
pub(crate) const GATEWAY_TOOLSETS: &str =
    "core,actions,config,content,backups,mybox,schedules,databases,webhosting";

/// Durée de vie du catalogue en cache (il ne change qu'aux mises à jour de l'officiel).
const CATALOG_TTL: Duration = Duration::from_secs(600);
/// Après un échec (officiel injoignable), délai avant de retenter : un client MCP qui liste ses
/// outils ne doit pas attendre le délai réseau à chaque fois.
const CATALOG_RETRY: Duration = Duration::from_secs(60);

// --- Table d'équivalences -------------------------------------------------------------------

/// Lequel garder quand un outil officiel et son équivalent local sont tous deux proposés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    Local,
    Official,
}

/// `(outil officiel, équivalent local, lequel garder)`. Source unique du dédoublonnage, pour le
/// Copilote comme pour la passerelle : un outil officiel n'est masqué que si son équivalent est
/// effectivement proposé (le Copilote, recentré sur `LOCAL_KEEP_TOOLS`, garde donc l'officiel
/// `power_action` ; la passerelle, qui a tout le catalogue local, garde le nôtre).
pub(crate) const EQUIVALENTS: &[(&str, &str, Keep)] = &[
    // Les quatre paires du Copilote (catalogue local recentré), comparées une à une :
    // - le docteur démarrage lit la commande, la fin de latest.log et le dernier crash-report et
    //   pré-classe les pannes connues ; `diagnose_server` n'agrège que fiche, état et événements
    //   (il renvoie lui-même vers la console en cas de crash) ;
    ("diagnose_server", "diagnose_startup", Keep::Local),
    // - même endpoint (`/server/{id}/disk/usage`), le nôtre ajoute les limites du serveur et de la MyBox ;
    ("get_disk_usage", "disk_usage", Keep::Local),
    // - l'officiel décode bien plus du rapport profiler (TPS/MSPT, CPU par mod, entités par chunk,
    //   arbre d'appels, profondeur réglable) ; le nôtre ne garde que les points chauds (et les heap
    //   summaries, rares) ;
    ("get_spark_report", "parse_spark_report", Keep::Official),
    // - le nôtre croise la surveillance locale (10 s) et l'historique de l'hébergeur, avec résumé
    //   et joueurs ; l'officiel ne donne que l'historique de l'hébergeur.
    ("get_server_stats_history", "server_metrics", Keep::Local),
    // Passerelle (catalogue local complet) : nos outils, validés localement, priment.
    ("list_servers", "list_servers", Keep::Local),
    ("get_server_live", "server_status", Keep::Local),
    ("get_console_logs", "read_console", Keep::Local),
    ("power_action", "power_action", Keep::Local),
    ("send_console_command", "send_command", Keep::Local),
    ("manage_player", "player_action", Keep::Local),
    ("get_mybox", "mybox_info", Keep::Local),
    ("get_server_catalog", "server_catalog", Keep::Local),
    ("create_server_on_mybox", "mybox_create_server", Keep::Local),
    (
        "update_server_resources",
        "mybox_resize_server",
        Keep::Local,
    ),
    ("enable_disable_server", "mybox_pause_server", Keep::Local),
    // Suppression d'un serveur : la nôtre exige le nom exact et fait une snapshot préalable.
    (
        "delete_server_from_mybox",
        "mybox_delete_server",
        Keep::Local,
    ),
    ("rename_resource", "rename_server", Keep::Local),
    ("list_snapshots", "list_snapshots", Keep::Local),
    ("create_snapshot", "create_snapshot", Keep::Local),
    ("list_smartbackups", "list_backups", Keep::Local),
    ("list_schedules", "list_schedules", Keep::Local),
    ("create_schedule", "create_schedule", Keep::Local),
    ("edit_schedule", "update_schedule", Keep::Local),
    ("delete_schedule", "delete_schedule", Keep::Local),
    ("get_server_parameters", "read_properties", Keep::Local),
    ("set_server_parameter", "set_property", Keep::Local),
    ("get_startup_config", "read_startup", Keep::Local),
    // Pas de paire `set_startup_config` ↔ `set_startup_params` : l'officiel change aussi le JAR
    // de démarrage (`file`), ce que le nôtre ne sait pas faire. Les deux restent proposés.
    ("install_mod", "install_mod", Keep::Local),
    ("install_plugin", "install_mod", Keep::Local),
];

/// Dédoublonnage : `(locaux à masquer, officiels à masquer → outil local qui les remplace)`. En
/// plus de la table, un officiel qui porte le NOM d'un outil local proposé est toujours masqué :
/// deux outils ne peuvent pas partager un nom, même si la table ne connaît pas encore la paire.
pub(crate) fn dedup(
    local: &HashSet<String>,
    official: &HashSet<String>,
) -> (HashSet<String>, HashMap<String, String>) {
    let mut drop_local = HashSet::new();
    let mut drop_official = HashMap::new();
    for &(off, loc, keep) in EQUIVALENTS {
        if local.contains(loc) && official.contains(off) {
            match keep {
                Keep::Local => {
                    drop_official.insert(off.to_string(), loc.to_string());
                }
                Keep::Official => {
                    drop_local.insert(loc.to_string());
                }
            }
        }
    }
    for name in official {
        if local.contains(name) && !drop_local.contains(name) {
            drop_official
                .entry(name.clone())
                .or_insert_with(|| name.clone());
        }
    }
    (drop_local, drop_official)
}

// --- Catalogue (mis en cache) -----------------------------------------------------------------

/// Catalogue de l'officiel pour un jeu de toolsets.
pub(crate) struct Catalog {
    /// Définitions brutes (`name`, `description`, `inputSchema`, `annotations`…), dans l'ordre de
    /// l'officiel.
    pub tools: Vec<Value>,
    /// Outils de LECTURE : ceux que l'officiel garde avec `readonly=1`. Tout autre outil est
    /// modifiant (inconnu compris).
    pub read: HashSet<String>,
}

impl Catalog {
    pub(crate) fn get(&self, name: &str) -> Option<&Value> {
        self.tools.iter().find(|t| tool_name(t) == Some(name))
    }

    pub(crate) fn is_read(&self, name: &str) -> bool {
        self.read.contains(name)
    }
}

fn tool_name(def: &Value) -> Option<&str> {
    def.get("name").and_then(Value::as_str)
}

type CatalogCache = HashMap<String, (Instant, Option<Arc<Catalog>>)>;
static CATALOGS: LazyLock<Mutex<CatalogCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Empreinte de la clé (jamais la clé elle-même) : un changement de compte invalide le cache.
fn fingerprint(token: &str) -> u64 {
    let mut h = DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// Catalogue des `toolsets` (outils complets + liste de lecture `readonly=1`), en cache
/// [`CATALOG_TTL`]. `None` sans clé au trousseau ou si l'officiel est injoignable : l'appelant
/// retombe alors sur nos seuls outils (avertissement au journal, au plus une fois par minute).
pub(crate) async fn catalog(toolsets: &str) -> Option<Arc<Catalog>> {
    let token = crate::secrets::read_key()
        .ok()
        .flatten()
        .filter(|t| !t.trim().is_empty())?;
    let key = format!("{toolsets}\u{1}{:x}", fingerprint(&token));
    if let Some((at, cached)) = CATALOGS.lock().unwrap().get(&key) {
        let ttl = if cached.is_some() {
            CATALOG_TTL
        } else {
            CATALOG_RETRY
        };
        if at.elapsed() < ttl {
            return cached.clone();
        }
    }
    let (full, read) = tokio::join!(
        crate::official_mcp::list_tools(&token, toolsets, false),
        crate::official_mcp::list_tools(&token, toolsets, true),
    );
    let fetched = match (full, read) {
        (Ok(tools), Ok(read)) if !tools.is_empty() => Some(Arc::new(Catalog {
            read: read
                .iter()
                .filter_map(tool_name)
                .map(str::to_string)
                .collect(),
            tools,
        })),
        (Ok(_), Ok(_)) => {
            tracing::warn!("MCP officiel ({toolsets}) : catalogue vide — outils locaux seuls");
            None
        }
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!("MCP officiel ({toolsets}) indisponible : {e} — outils locaux seuls");
            None
        }
    };
    CATALOGS
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), fetched.clone()));
    fetched
}

// --- Présentation et arguments ----------------------------------------------------------------

/// Arguments qui feraient sortir un secret en clair (mot de passe SFTP, FTP ou de base) : retirés
/// des schémas présentés et des appels relayés.
const SECRET_REVEAL_ARGS: &[&str] = &["reveal_credentials"];

/// Le schéma d'entrée déclare-t-il cette propriété ?
fn has_prop(def: &Value, key: &str) -> bool {
    def.pointer("/inputSchema/properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key(key))
}

fn requires(def: &Value, key: &str) -> bool {
    def.pointer("/inputSchema/required")
        .and_then(Value::as_array)
        .is_some_and(|r| r.iter().any(|v| v == key))
}

/// Destructif ? Default-deny : seul un `destructiveHint: false` explicite (spécification MCP : la
/// valeur par défaut est `true`) rend un outil modifiant non destructif.
fn is_destructive(def: &Value) -> bool {
    def.pointer("/annotations/destructiveHint")
        .and_then(Value::as_bool)
        != Some(false)
}

/// Ressource dont le nom exact confirme un outil destructif.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Domaine du site (`get_webhosting` → `webhosting.domain`, ce qu'attend aussi l'officiel).
    Webhosting,
    Server,
    Snapshot,
    MyBox,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::Webhosting => "du site (son domaine, cf. get_webhosting)",
            Target::Server => "du serveur visé",
            Target::Snapshot => "du snapshot",
            Target::MyBox => "de la MyBox",
        }
    }
}

/// Ressource à confirmer, d'après les identifiants que prend l'outil (la plus précise d'abord).
/// `None` : rien à vérifier localement → l'outil destructif est masqué.
fn confirm_target(def: &Value) -> Option<Target> {
    if has_prop(def, "webhosting_id") {
        Some(Target::Webhosting)
    } else if has_prop(def, "server_id") {
        Some(Target::Server)
    } else if has_prop(def, "snapshot_id") {
        Some(Target::Snapshot)
    } else if has_prop(def, "mybox_id") {
        Some(Target::MyBox)
    } else {
        None
    }
}

/// Définition présentée au client : secrets retirés du schéma et, pour un outil destructif,
/// `confirm_name` ajouté (obligatoire) et annoncé en tête de description.
fn present(def: &Value, confirm: Option<Target>) -> Value {
    let mut out = def.clone();
    if let Some(schema) = out.get_mut("inputSchema").and_then(Value::as_object_mut) {
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            for k in SECRET_REVEAL_ARGS {
                props.remove(*k);
            }
            if let Some(t) = confirm {
                props.insert(
                    "confirm_name".into(),
                    json!({ "type": "string", "description": format!(
                        "Confirmation : nom EXACT {} (casse comprise), donné par l'utilisateur. Vérifié avant envoi ; sinon refus.",
                        t.label()
                    ) }),
                );
            }
        }
        if let Some(req) = schema.get_mut("required").and_then(Value::as_array_mut) {
            req.retain(|v| !SECRET_REVEAL_ARGS.iter().any(|k| v == k));
        }
        if confirm.is_some() {
            let req = schema.entry("required").or_insert_with(|| json!([]));
            if let Some(req) = req.as_array_mut() {
                if !req.iter().any(|v| v == "confirm_name") {
                    req.push(json!("confirm_name"));
                }
            }
        }
    }
    if let Some(t) = confirm {
        let desc = out
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        out["description"] = json!(format!(
            "⚠ DESTRUCTIF : exige `confirm_name` = nom exact {}, vérifié localement avant envoi ; ne l'appelle que sur demande explicite de l'utilisateur. {desc}",
            t.label()
        ));
    }
    out
}

/// Retire des arguments ce qui révélerait un secret.
fn sanitize_args(args: &mut Value) {
    if let Some(map) = args.as_object_mut() {
        for k in SECRET_REVEAL_ARGS {
            map.remove(*k);
        }
    }
}

/// Identifiant entier d'un argument (nombre, ou texte numérique).
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

// --- Copilote -----------------------------------------------------------------------------------

/// Outils officiels MODIFIANTS que le Copilote peut appeler (autonome, ou « Appliquer ») : limités
/// à un serveur et raisonnablement réversibles. Tout autre outil modifiant — et tout outil inconnu —
/// est refusé. Exclus volontairement, entre autres : `set_startup_config` (la commande de démarrage
/// est exécutée par le shell du conteneur : une injection y planterait une commande persistante),
/// `send_rcon_command` (n'importe quelle IP), les mondes, versions, modpacks, schedules, MyBox,
/// bases, tickets, renommages, ports, adresse et Java.
pub(crate) const COPILOT_OFFICIAL_WRITES: &[&str] = &[
    "power_action",
    "send_console_command",
    "manage_player",
    "set_server_parameter",
    "install_plugin",
    "install_mod",
    "install_datapack",
    "create_snapshot",
];

/// Le Copilote peut-il appeler cet outil officiel ? Lecture : toujours. Modifiant : en autonome,
/// s'il est dans la liste blanche, exige un `server_id` et n'est pas annoncé destructif (si
/// l'officiel change un de ces outils, il sort de lui-même de la liste).
pub(crate) fn copilot_allows(cat: &Catalog, name: &str, autonomous: bool) -> bool {
    let Some(def) = cat.get(name) else {
        return false;
    };
    if cat.is_read(name) {
        return true;
    }
    autonomous
        && COPILOT_OFFICIAL_WRITES.contains(&name)
        && requires(def, "server_id")
        && !is_destructive(def)
}

/// Clés d'arguments qui désignent UN serveur : `server_id`, et le second serveur des outils locaux
/// qui en lisent deux (`diff_files`, copie).
const SERVER_KEYS: &[&str] = &["server_id", "other_server_id", "source_server_id", "target_server_id"];

/// Refus si l'appel vise un autre serveur que celui de la session (un identifiant illisible est
/// refusé aussi), par l'une des `SERVER_KEYS` ou par une liste `server_ids`. Sans identifiant, rien
/// à vérifier.
pub(crate) fn scope_violation(server_id: i64, args: &Value) -> Option<String> {
    let refuse = |v: &Value| {
        Some(format!("Refusé : cette conversation ne concerne que le serveur {server_id} (appel visant {v})."))
    };
    for key in SERVER_KEYS {
        match args.get(*key) {
            None | Some(Value::Null) => {}
            Some(_) if arg_i64(args, key) == Some(server_id) => {}
            Some(v) => return refuse(v),
        }
    }
    match args.get("server_ids") {
        None | Some(Value::Null) => None,
        Some(Value::Array(ids)) if ids.iter().all(|v| v.as_i64() == Some(server_id)) => None,
        Some(v) => refuse(v),
    }
}

/// Outils LOCAUX de lecture qui couvrent tout le compte quand on ne leur donne pas de serveur
/// (`find_player` sans liste, `list_incidents` sans `server_id`) : pour le Copilote, bornés au
/// serveur de la session.
pub(crate) fn bound_local_args(server_id: i64, name: &str, args: Value) -> Value {
    let key = match name {
        "find_player" => "server_ids",
        "list_incidents" => "server_id",
        _ => return args,
    };
    match args {
        Value::Object(mut m) => {
            if m.get(key).is_none_or(Value::is_null) {
                let v = if key == "server_ids" { json!([server_id]) } else { json!(server_id) };
                m.insert(key.into(), v);
            }
            Value::Object(m)
        }
        other => other,
    }
}

/// Arguments bornés au serveur de la session : refus s'ils en visent un autre ; `server_id` ajouté
/// s'il manque et que l'outil le prend.
fn scope_args(def: &Value, server_id: i64, args: Value) -> Result<Value, String> {
    let mut args = match args {
        Value::Object(m) => Value::Object(m),
        Value::Null => json!({}),
        _ => return Err("Arguments invalides : objet JSON attendu.".into()),
    };
    if let Some(refus) = scope_violation(server_id, &args) {
        return Err(refus);
    }
    if args.get("server_id").is_none_or(Value::is_null) && has_prop(def, "server_id") {
        args["server_id"] = json!(server_id);
    }
    Ok(args)
}

/// Contrôle complet d'un appel du Copilote (pur, testable) : outil autorisé, portée, arguments
/// nettoyés. Renvoie les arguments à relayer et « lecture ? ».
pub(crate) fn copilot_check(
    cat: &Catalog,
    server_id: i64,
    autonomous: bool,
    name: &str,
    args: Value,
) -> Result<(Value, bool), String> {
    let Some(def) = cat.get(name) else {
        return Err(format!(
            "Outil « {name} » inconnu : utilise uniquement les outils proposés."
        ));
    };
    if !copilot_allows(cat, name, autonomous) {
        return Err(if !autonomous && COPILOT_OFFICIAL_WRITES.contains(&name) {
            format!("Action non exécutée (mode suggestion) : « {name} » modifie le serveur, propose-la.")
        } else {
            format!(
                "Action refusée : « {name} » (MCP officiel) n'est pas permis au Copilote — seulement la lecture, et en autonome : {}.",
                COPILOT_OFFICIAL_WRITES.join(", ")
            )
        });
    }
    let mut args = scope_args(def, server_id, args)?;
    sanitize_args(&mut args);
    Ok((args, cat.is_read(name)))
}

/// Ce qu'un consommateur (Copilote ou passerelle) voit de l'officiel, dédoublonné contre nos
/// outils proposés.
pub(crate) struct View {
    /// Outils relayés : définition présentée, « lecture ? », confirmation éventuelle.
    pub tools: Vec<Relayed>,
    /// Outils locaux à retirer (l'officiel est meilleur).
    pub masked_local: HashSet<String>,
    /// Outils officiels non relayés → raison (outil local qui les remplace, ou confirmation
    /// impossible à vérifier).
    pub masked_official: HashMap<String, String>,
}

pub(crate) struct Relayed {
    pub name: String,
    pub read: bool,
    pub confirm: Option<Target>,
    /// L'officiel attend lui-même `confirm_name` (il est alors transmis, pas retiré).
    pub official_confirm: bool,
    pub def: Value,
}

/// Vue du Copilote : lecture + liste blanche (en autonome), dédoublonnée.
pub(crate) fn copilot_view(cat: &Catalog, local: &HashSet<String>, autonomous: bool) -> View {
    let allowed: Vec<&Value> = cat
        .tools
        .iter()
        .filter(|d| tool_name(d).is_some_and(|n| copilot_allows(cat, n, autonomous)))
        .collect();
    let names: HashSet<String> = allowed
        .iter()
        .filter_map(|d| tool_name(d))
        .map(str::to_string)
        .collect();
    let (masked_local, drop) = dedup(local, &names);
    let masked_official = drop
        .into_iter()
        .map(|(off, loc)| (off, format!("remplacé par l'outil `{loc}`")))
        .collect::<HashMap<_, _>>();
    let tools = allowed
        .into_iter()
        .filter_map(|def| {
            let name = tool_name(def)?.to_string();
            (!masked_official.contains_key(&name)).then(|| Relayed {
                read: cat.is_read(&name),
                confirm: None,
                official_confirm: false,
                def: present(def, None),
                name,
            })
        })
        .collect();
    View {
        tools,
        masked_local,
        masked_official,
    }
}

/// Catalogue des toolsets du Copilote (en cache) ; sa politique s'applique ensuite
/// (`copilot_view`, `copilot_check`).
pub(crate) async fn copilot_catalog() -> Option<Arc<Catalog>> {
    catalog(COPILOT_TOOLSETS).await
}

/// Exécute un outil officiel pour le Copilote (voie API HTTP) après [`copilot_check`]. Le texte
/// renvoyé (succès ou erreur) est anonymisé ; une lecture passe par une connexion `readonly=1`.
pub(crate) async fn copilot_call(
    core: &Core,
    server_id: i64,
    autonomous: bool,
    name: &str,
    args: Value,
) -> String {
    match copilot_run(core, server_id, autonomous, name, args).await {
        Ok(out) => out,
        Err(e) => e,
    }
}

async fn copilot_run(
    core: &Core,
    server_id: i64,
    autonomous: bool,
    name: &str,
    args: Value,
) -> Result<String, String> {
    let cat = copilot_catalog()
        .await
        .ok_or_else(|| "MCP officiel indisponible pour le moment.".to_string())?;
    let (args, read) = copilot_check(&cat, server_id, autonomous, name, args)?;
    let token = crate::secrets::read_key()
        .ok()
        .flatten()
        .ok_or_else(|| "Clé API MineStrator indisponible pour le MCP officiel.".to_string())?;
    let out = crate::official_mcp::call_tool(&token, COPILOT_TOOLSETS, read, name, args).await;
    if !read {
        core.cache().clear(); // l'état du serveur a pu changer : nos lectures en cache sont périmées
    }
    out.map(|o| core.redact_ai(&o))
        .map_err(|e| format!("Erreur outil : {}", core.redact_ai(&e)))
}

/// « Appliquer » une action du Copilote qui désigne un outil officiel : même règle qu'en
/// autonome (lecture ou liste blanche, portée du serveur du diagnostic), puis relais.
pub(crate) async fn copilot_apply(
    core: &Core,
    server_id: i64,
    tool: &str,
    args: Value,
) -> Result<String, String> {
    if !core.copilot_config_with(|c| c.use_official_mcp) {
        return Err(format!(
            "« {tool} » est un outil du MCP officiel, désactivé dans les réglages du Copilote."
        ));
    }
    copilot_run(core, server_id, true, tool, args).await
}

// --- Passerelle du serveur MCP local ------------------------------------------------------------

/// Rôle du relais dans un processus `--mcp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayMode {
    Off,
    /// Passerelle pour un client MCP externe.
    Gateway,
    /// Serveur MCP lancé par le Copilote pour un agent CLI : politique du Copilote, bornée à ce serveur.
    Copilot(i64),
}

fn env_flag(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" | "oui" => Some(true),
        "0" | "false" | "off" | "no" | "non" => Some(false),
        _ => None,
    }
}

/// Mode du relais (pur, testable) d'après la variable [`MCP_OFFICIAL_ENV`], la variable
/// [`MCP_COPILOT_SERVER_ENV`] et le réglage persistant. Un processus du Copilote n'a jamais la
/// passerelle : le relais n'y existe que demandé explicitement, et borné au serveur.
pub(crate) fn resolve_mode(
    official_env: Option<&str>,
    copilot_server: Option<&str>,
    setting: bool,
) -> RelayMode {
    let forced = official_env.and_then(env_flag);
    match copilot_server {
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(id) if forced == Some(true) => RelayMode::Copilot(id),
            _ => RelayMode::Off,
        },
        None if forced.unwrap_or(setting) => RelayMode::Gateway,
        None => RelayMode::Off,
    }
}

fn relay_mode(core: &Core) -> RelayMode {
    resolve_mode(
        std::env::var(MCP_OFFICIAL_ENV).ok().as_deref(),
        std::env::var(MCP_COPILOT_SERVER_ENV).ok().as_deref(),
        core.get_mcp_config().official_gateway,
    )
}

/// Portée imposée par le Copilote à son serveur `--mcp` (agents CLI) : refus d'un appel — local ou
/// relayé — qui vise un autre serveur. Variable illisible → refus (fail-closed).
pub(crate) fn env_scope_violation(args: &Value) -> Option<String> {
    let raw = std::env::var(MCP_COPILOT_SERVER_ENV).ok()?;
    match raw.trim().parse::<i64>() {
        Ok(id) => scope_violation(id, args),
        Err(_) => Some("Refusé : portée du Copilote illisible.".into()),
    }
}

/// Vue de la passerelle : tous les outils du catalogue, dédoublonnés contre nos outils `local` ;
/// un outil destructif reçoit une confirmation vérifiable, ou il est masqué.
pub(crate) fn gateway_view(cat: &Catalog, local: &HashSet<String>) -> View {
    let names: HashSet<String> = cat
        .tools
        .iter()
        .filter_map(tool_name)
        .map(str::to_string)
        .collect();
    let (masked_local, drop) = dedup(local, &names);
    let mut masked_official: HashMap<String, String> = drop
        .into_iter()
        .map(|(off, loc)| (off, format!("remplacé par l'outil `{loc}`")))
        .collect();
    let mut tools = Vec::new();
    for def in &cat.tools {
        let Some(name) = tool_name(def) else { continue };
        if masked_official.contains_key(name) {
            continue;
        }
        let read = cat.is_read(name);
        let confirm = if !read && is_destructive(def) {
            match confirm_target(def) {
                Some(t) => Some(t),
                None => {
                    masked_official.insert(
                        name.to_string(),
                        "destructif, et sans ressource dont le nom puisse être vérifié localement"
                            .into(),
                    );
                    continue;
                }
            }
        } else {
            None
        };
        tools.push(Relayed {
            name: name.to_string(),
            read,
            confirm,
            official_confirm: has_prop(def, "confirm_name"),
            def: present(def, confirm),
        });
    }
    View {
        tools,
        masked_local,
        masked_official,
    }
}

fn names_of(tools: &Value) -> HashSet<String> {
    tools
        .as_array()
        .map(|a| a.iter().filter_map(tool_name).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Nos outils effectivement proposés par ce processus (liste blanche d'env appliquée).
fn local_offered() -> HashSet<String> {
    names_of(crate::mcp::tool_list())
        .into_iter()
        .filter(|n| crate::mcp::env_allows(n))
        .collect()
}

async fn current_view(core: &Core) -> Option<(RelayMode, View)> {
    let mode = relay_mode(core);
    let (toolsets, autonomous) = match mode {
        RelayMode::Off => return None,
        RelayMode::Gateway => (GATEWAY_TOOLSETS, false),
        // Les écritures permises au Copilote ne passent que si sa liste blanche d'env les nomme
        // (mode autonome) : sans liste, lecture seule.
        RelayMode::Copilot(_) => (COPILOT_TOOLSETS, crate::mcp::env_has_allowlist()),
    };
    let cat = catalog(toolsets).await?;
    let local = local_offered();
    let view = match mode {
        RelayMode::Copilot(_) => copilot_view(&cat, &local, autonomous),
        _ => gateway_view(&cat, &local),
    };
    Some((mode, view))
}

/// `tools/list` : ajoute les outils relayés (liste blanche d'env appliquée) au catalogue local
/// `tools` (déjà filtré), et en retire nos outils que l'officiel remplace. Sans relais ou sans
/// officiel joignable : `tools` inchangé.
pub(crate) async fn extend_tool_list(core: &Core, tools: &mut Value) {
    let Some((_, view)) = current_view(core).await else {
        return;
    };
    let Some(arr) = tools.as_array_mut() else {
        return;
    };
    arr.retain(|t| tool_name(t).is_none_or(|n| !view.masked_local.contains(n)));
    // Client capable d'élicitation : la confirmation d'un destructif est demandée à l'humain,
    // `confirm_name` devient facultatif (voir `relay_call`).
    let human = crate::mcp_ctx::elicitation_supported();
    arr.extend(
        view.tools
            .into_iter()
            .filter(|r| crate::mcp::env_allows(&r.name))
            .map(|r| {
                let mut def = r.def;
                if human && r.confirm.is_some() {
                    crate::mcp_catalog::relax_confirmation(&mut def);
                }
                def
            }),
    );
}

/// Un outil officiel **modifiant** est-il appelable maintenant par le client de ce serveur MCP ?
/// Passerelle active, groupe `official` actif, écritures permises, outil relayé (ni masqué ni
/// absent). `Err` : pourquoi pas, avec la marche à suivre. Sert aux étapes `manual` du manifeste
/// (`manifest/official.rs`).
pub(crate) async fn gateway_availability(core: &Core, name: &str) -> Result<(), String> {
    match relay_mode(core) {
        RelayMode::Gateway => {}
        RelayMode::Copilot(_) => {
            return Err("serveur MCP du Copilote : cet outil officiel ne lui est pas relayé, le changement revient à l'utilisateur".into())
        }
        RelayMode::Off => {
            return Err(format!(
                "passerelle vers le MCP officiel désactivée : l'activer dans Réglages → MCP (« Passerelle vers le MCP officiel », ou variable {MCP_OFFICIAL_ENV}=1) puis reconnecter le client MCP, ou faire le changement dans le panel"
            ))
        }
    }
    if !crate::mcp_catalog::tool_in_toolsets(
        crate::mcp_catalog::active_toolsets(core).as_ref(),
        crate::mcp_catalog::OFFICIAL_GROUP,
    ) {
        return Err(format!(
            "groupe d'outils `{}` inactif (--toolsets, MINESTRATOR_MCP_TOOLSETS ou réglage) : l'ajouter",
            crate::mcp_catalog::OFFICIAL_GROUP
        ));
    }
    if !crate::mcp::env_allows(name) {
        return Err("outil hors de la liste blanche de ce serveur MCP".into());
    }
    if !core.get_mcp_config().allow_writes {
        return Err("actions modifiantes désactivées (mode lecture seule, Réglages → MCP)".into());
    }
    let Some((_, view)) = current_view(core).await else {
        return Err("MCP officiel injoignable pour l'instant : réessayer, ou passer par le panel".into());
    };
    if view.tools.iter().any(|t| t.name == name) {
        return Ok(());
    }
    Err(match view.masked_official.get(name) {
        Some(why) => format!("masqué par la passerelle ({why})"),
        None => "absent du catalogue du MCP officiel".into(),
    })
}

fn tool_text(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}
fn tool_err(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// `tools/call` d'un outil relayé. `None` si `name` n'est pas un outil relayé (l'appelant répond
/// alors « outil inconnu »). Gating : liste blanche d'env, `allow_writes` pour tout outil hors de
/// la liste de lecture, portée du Copilote, confirmation vérifiée des destructifs.
pub(crate) async fn relay_call(
    core: &Core,
    cfg: &McpConfig,
    name: &str,
    args: Value,
) -> Option<Value> {
    let (mode, view) = current_view(core).await?;
    let Some(tool) = view.tools.iter().find(|t| t.name == name) else {
        return view.masked_official.get(name).map(|why| {
            tool_err(&format!(
                "« {name} » (MCP officiel) n'est pas relayé : {why}."
            ))
        });
    };
    if !crate::mcp::env_allows(name) {
        return Some(tool_err(
            "Outil non autorisé par la politique de permissions.",
        ));
    }
    if !tool.read && !cfg.allow_writes {
        return Some(tool_err(
            "Actions modifiantes désactivées (mode lecture seule) dans les réglages.",
        ));
    }
    // Mode constructeur (agent_guard.rs) : une écriture relayée suit la même portée que les nôtres.
    if !tool.read {
        if let Some(refus) = crate::agent_guard::relay_write_scope(core, cfg, name, &args).await {
            return Some(tool_err(&refus));
        }
    }
    let toolsets = match mode {
        RelayMode::Copilot(_) => COPILOT_TOOLSETS,
        _ => GATEWAY_TOOLSETS,
    };
    let mut args = match mode {
        RelayMode::Copilot(server_id) => match scope_args(&tool.def, server_id, args) {
            Ok(a) => a,
            Err(e) => return Some(tool_err(&e)),
        },
        _ => args,
    };
    let Some(token) = crate::secrets::read_key().ok().flatten() else {
        return Some(tool_err(
            "Clé API MineStrator absente du trousseau : connecte-toi dans l'app.",
        ));
    };
    if let Some(target) = tool.confirm {
        // Client capable d'élicitation : l'HUMAIN retape le nom exact (refus → rien n'est relayé) ;
        // sinon `confirm_name` fourni par le modèle, vérifié comme avant.
        let human = match human_confirmation(core, &token, name, target, &args).await {
            Ok(h) => h,
            Err(e) => return Some(tool_err(&e)),
        };
        if human.is_none() {
            if let Err(e) = verify_confirmation(core, &token, target, &args).await {
                return Some(tool_err(&e));
            }
        }
        if let Some(map) = args.as_object_mut() {
            match human {
                // L'officiel attend lui-même la confirmation : le nom confirmé par l'humain.
                Some(expected) if tool.official_confirm => {
                    map.insert("confirm_name".into(), json!(expected));
                }
                _ if !tool.official_confirm => {
                    map.remove("confirm_name");
                }
                _ => {}
            }
        }
    }
    sanitize_args(&mut args);
    let out = crate::official_mcp::call_tool(&token, toolsets, tool.read, name, args).await;
    if !tool.read {
        core.cache().clear();
    }
    Some(match out {
        Ok(text) => tool_text(core.redact_ai(&text)),
        Err(e) => tool_err(&format!("Erreur : {}", core.redact_ai(&e))),
    })
}

/// La confirmation reprend-elle exactement le nom attendu (espaces de bord ignorés) ?
fn confirmation_matches(expected: &str, given: &str) -> bool {
    let expected = expected.trim();
    !expected.is_empty() && given.trim() == expected
}

/// Domaine d'un site d'après la sortie JSON de `get_webhosting`.
fn webhosting_domain(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    v.pointer("/webhosting/domain")
        .or_else(|| v.get("domain"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|d| !d.trim().is_empty())
}

/// Confirmation par l'humain (élicitation MCP) quand son client sait la demander : `Ok(Some(nom))`
/// s'il a retapé le nom exact, `Err` s'il refuse (ou si le nom est invérifiable), `Ok(None)` si le
/// client ne gère pas l'élicitation (garde `confirm_name` habituelle).
async fn human_confirmation(
    core: &Core,
    token: &str,
    name: &str,
    target: Target,
    args: &Value,
) -> Result<Option<String>, String> {
    if !crate::mcp_ctx::elicitation_supported() {
        return Ok(None);
    }
    let expected = expected_name(core, token, target, args).await?.ok_or_else(|| {
        format!("Refusé : ressource introuvable, nom {} impossible à vérifier.", target.label())
    })?;
    let ids: Vec<String> = ["server_id", "snapshot_id", "mybox_id", "webhosting_id"]
        .iter()
        .filter_map(|k| arg_i64(args, k).map(|v| format!("{k} = {v}")))
        .collect();
    let message = format!(
        "Action DESTRUCTIVE du MCP officiel MineStrator : « {name} » ({}), sur la ressource {}.",
        ids.join(", "),
        target.label()
    );
    match crate::mcp_ctx::confirm(&message, Some(&expected)).await {
        crate::mcp_ctx::Confirmation::Accepted => Ok(Some(expected)),
        crate::mcp_ctx::Confirmation::Declined => {
            Err("Refusé par l'utilisateur (confirmation non donnée) : rien n'a été relayé.".into())
        }
        crate::mcp_ctx::Confirmation::Unsupported => Ok(None),
    }
}

/// Vérifie `confirm_name` contre le nom réel de la ressource, lu à la source. Toute impossibilité
/// de vérifier est un refus. Le nom attendu n'est jamais révélé dans l'erreur : il doit venir de
/// l'utilisateur.
async fn verify_confirmation(
    core: &Core,
    token: &str,
    target: Target,
    args: &Value,
) -> Result<(), String> {
    let given = args
        .get("confirm_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if given.trim().is_empty() {
        return Err(format!(
            "Confirmation requise : `confirm_name` = nom exact {}, à demander à l'utilisateur.",
            target.label()
        ));
    }
    match expected_name(core, token, target, args).await? {
        Some(name) if confirmation_matches(&name, given) => Ok(()),
        Some(_) => Err(format!(
            "Refusé : `confirm_name` ne correspond pas au nom exact {} (casse comprise).",
            target.label()
        )),
        None => Err(format!(
            "Refusé : ressource introuvable, nom {} impossible à vérifier.",
            target.label()
        )),
    }
}

/// Nom réel de la ressource visée, lu à la source (`None` : introuvable).
async fn expected_name(
    core: &Core,
    token: &str,
    target: Target,
    args: &Value,
) -> Result<Option<String>, String> {
    let id_of =
        |key: &str| arg_i64(args, key).ok_or_else(|| format!("paramètre `{key}` (entier) requis"));
    let expected: Option<String> = match target {
        Target::Server => {
            let id = id_of("server_id")?;
            let all = core.list_servers().await.map_err(|e| e.to_string())?;
            all.servers.into_iter().find(|s| s.id == id).map(|s| s.name)
        }
        Target::MyBox => {
            let id = id_of("mybox_id")?;
            let all = core.list_servers().await.map_err(|e| e.to_string())?;
            all.myboxes.into_iter().find(|m| m.id == id).map(|m| m.name)
        }
        Target::Snapshot => {
            let id = id_of("snapshot_id")?;
            let all = core.list_snapshots().await.map_err(|e| e.to_string())?;
            all.into_iter().find(|s| s.id == Some(id)).map(|s| s.name)
        }
        Target::Webhosting => {
            let id = id_of("webhosting_id")?;
            let text = crate::official_mcp::call_tool(
                token,
                GATEWAY_TOOLSETS,
                true,
                "get_webhosting",
                json!({ "webhosting_id": id }),
            )
            .await
            .map_err(|e| format!("vérification impossible : {}", core.redact_ai(&e)))?;
            webhosting_domain(&text)
        }
    };
    Ok(expected.filter(|n| !n.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    /// `R` lecture (`readonly=1`), `W` modifiant non destructif, `D` destructif (annotation).
    #[derive(Clone, Copy, PartialEq)]
    enum K {
        R,
        W,
        D,
    }
    use K::{D, R, W};

    /// Relevé du 2026-11-18 (`tools/list` complet et `readonly=1`) : toolset, nom, genre, paramètres.
    #[rustfmt::skip]
    const RELEVE: &[(&str, &str, K, &[&str])] = &[
        ("core", "create_ticket", W, &["type", "title", "content", "server_id"]),
        ("core", "diagnose_server", R, &["server_id"]),
        ("core", "get_account", R, &[]),
        ("core", "get_account_activity", R, &["scope", "mybox_id", "include_servers", "category", "search", "since", "until", "limit", "offset"]),
        ("core", "get_console_logs", R, &["server_id"]),
        ("core", "get_disk_usage", R, &["server_id", "path", "limit"]),
        ("core", "get_server", R, &["server_id", "reveal_credentials"]),
        ("core", "get_server_activity", R, &["server_id"]),
        ("core", "get_server_live", R, &["server_id", "wait_seconds", "wait_for_state"]),
        ("core", "get_server_stats_history", R, &["server_id", "date_start", "date_end"]),
        ("core", "get_spark_report", R, &["spark_url", "depth"]),
        ("core", "get_ticket", R, &["ticket_id"]),
        ("core", "get_tickets", R, &["status", "limit"]),
        ("core", "list_servers", R, &[]),
        ("core", "list_tasks", R, &[]),
        ("actions", "manage_player", W, &["server_id", "action", "player"]),
        ("actions", "power_action", W, &["server_id", "action"]),
        ("actions", "reinstall_server", D, &["server_id"]),
        ("actions", "send_console_command", W, &["server_id", "command"]),
        ("actions", "send_rcon_command", W, &["protocol", "ip", "port", "password", "command"]),
        ("config", "get_server_parameters", R, &["server_id"]),
        ("config", "get_startup_config", R, &["server_id"]),
        ("config", "manage_java", W, &["server_id", "java_version_id", "memory_percentage", "docker_image"]),
        ("config", "manage_ports", W, &["server_id", "action", "port", "type", "description"]),
        ("config", "rename_resource", W, &["type", "id", "name"]),
        ("config", "set_server_address", W, &["server_id", "name", "domain"]),
        ("config", "set_server_parameter", W, &["server_id", "property", "value"]),
        ("config", "set_startup_config", W, &["server_id", "parameters", "file"]),
        ("content", "change_version", D, &["server_id", "version_id", "game_version", "build", "loader", "reinstall", "autostart"]),
        ("content", "get_content_files", R, &["type", "id", "slug", "loader", "game_version"]),
        ("content", "install_datapack", W, &["server_id", "source", "slug", "version_id", "game_version", "mod_id", "file_id"]),
        ("content", "install_mod", W, &["server_id", "source", "slug", "mod_id", "file_id", "version_id", "loader", "game_version"]),
        ("content", "install_modpack", D, &["server_id", "source", "modpack_id", "version_id", "reinstall"]),
        ("content", "install_plugin", W, &["server_id", "source", "plugin_id", "version_id", "slug", "modrinth_version", "game_version"]),
        ("content", "install_world", W, &["server_id", "mod_id", "file_id"]),
        ("content", "list_datapacks", R, &["server_id"]),
        ("content", "manage_bedrock_addons", W, &["server_id", "action", "file", "mod_id", "file_id", "uuid", "restart"]),
        ("content", "manage_hytale_mods", W, &["server_id", "action", "name"]),
        ("content", "manage_steamworkshop_mods", W, &["server_id", "variables", "sync_mods"]),
        ("content", "manage_worlds", W, &["server_id", "set_active_folder", "settings", "set_gamerules", "set_experiments", "restart", "fresh"]),
        ("content", "search_game_content", R, &["type", "query", "loader", "game_version"]),
        ("content", "search_steamworkshop_mods", R, &["server_id", "query", "page", "workshop_id"]),
        ("backups", "create_snapshot", W, &["server_id", "name"]),
        ("backups", "delete_snapshot", D, &["snapshot_id"]),
        ("backups", "get_backup_job_status", R, &["job_id", "wait_seconds"]),
        ("backups", "list_smartbackups", R, &["server_id", "download_backup_id"]),
        ("backups", "list_snapshots", R, &["server_id"]),
        ("backups", "restore_smartbackup", D, &["server_id", "backup_id", "restore_startup_config"]),
        ("backups", "restore_snapshot", D, &["snapshot_id", "server_id"]),
        ("mybox", "create_server_on_mybox", W, &["mybox_id", "egg_id", "location_id", "cpu", "ram", "name", "experimental", "required_fields"]),
        ("mybox", "delete_server_from_mybox", D, &["mybox_id", "server_id", "snapshot_first"]),
        ("mybox", "enable_disable_server", W, &["mybox_id", "server_id", "action"]),
        ("mybox", "get_mybox", R, &["mybox_id"]),
        ("mybox", "get_server_catalog", R, &[]),
        ("mybox", "update_server_resources", W, &["mybox_id", "server_id", "cpu", "ram"]),
        ("schedules", "create_schedule", W, &["server_id", "name", "minute", "hour", "day_of_week", "day_of_month", "month", "is_active", "only_when_online", "tasks"]),
        ("schedules", "delete_schedule", D, &["server_id", "schedule_id"]),
        ("schedules", "edit_schedule", W, &["server_id", "schedule_id", "name", "minute", "hour", "day_of_week", "day_of_month", "month", "is_active", "only_when_online", "add_tasks", "update_tasks", "remove_task_ids"]),
        ("schedules", "list_schedules", R, &["server_id", "schedule_id"]),
        ("databases", "create_database", W, &["mybox_id", "engine", "description"]),
        ("databases", "list_db_backups", R, &["mybox_id", "database_id", "download_date"]),
        ("databases", "manage_database", W, &["mybox_id", "database_id", "action", "description"]),
        ("databases", "restore_db_backup", D, &["mybox_id", "database_id", "date"]),
        ("files", "compress_files", W, &["server_id", "directory", "files", "archive_name", "overwrite", "wait_seconds"]),
        ("files", "create_folder", W, &["server_id", "path", "name"]),
        ("files", "delete_files", D, &["server_id", "files"]),
        ("files", "edit_file", W, &["server_id", "file", "old_string", "new_string", "replace_all"]),
        ("files", "extract_archive", D, &["server_id", "file", "destination"]),
        ("files", "get_compress_job_status", R, &["server_id", "job_id", "wait_seconds"]),
        ("files", "list_compress_jobs", R, &["server_id", "limit"]),
        ("files", "list_files", R, &["server_id", "path", "limit", "offset"]),
        ("files", "move_files", W, &["server_id", "moves"]),
        ("files", "read_file", R, &["server_id", "file", "offset", "limit", "tail"]),
        ("files", "search_files", R, &["server_id", "query"]),
        ("files", "search_in_file", R, &["server_id", "file", "query", "context_lines", "case_sensitive"]),
        ("files", "set_file_permissions", W, &["server_id", "files", "executable", "mode"]),
        ("files", "write_file", W, &["server_id", "file", "content", "mode"]),
        ("webhosting", "change_webhosting_offer", W, &["webhosting_id", "offer"]),
        ("webhosting", "check_webhosting_http", R, &["webhosting_id", "target", "path"]),
        ("webhosting", "create_webhosting", W, &["mybox_id", "name"]),
        ("webhosting", "create_webhosting_folder", W, &["webhosting_id", "path"]),
        ("webhosting", "delete_webhosting", D, &["webhosting_id", "confirm_name"]),
        ("webhosting", "delete_webhosting_files", D, &["webhosting_id", "files"]),
        ("webhosting", "edit_webhosting_file", W, &["webhosting_id", "path", "old_string", "new_string", "replace_all"]),
        ("webhosting", "extract_webhosting_archive", D, &["webhosting_id", "file", "destination"]),
        ("webhosting", "get_webhosting", R, &["webhosting_id", "reveal_credentials"]),
        ("webhosting", "get_webhosting_logs", R, &["webhosting_id", "type", "lines", "search", "since", "until"]),
        ("webhosting", "get_webhosting_offers", R, &["webhosting_id"]),
        ("webhosting", "get_webhosting_ssl", R, &["webhosting_id"]),
        ("webhosting", "get_webhosting_stats", R, &["webhosting_id"]),
        ("webhosting", "install_webhosting_cms", D, &["webhosting_id", "cms", "site_name", "admin_email", "admin_username", "locale", "description"]),
        ("webhosting", "list_webhosting_backups", R, &["webhosting_id"]),
        ("webhosting", "list_webhosting_files", R, &["webhosting_id", "path"]),
        ("webhosting", "manage_webhosting_mails", D, &["webhosting_id", "action", "name", "mail_id"]),
        ("webhosting", "manage_webhosting_subdomains", W, &["webhosting_id", "action", "name", "new_name"]),
        ("webhosting", "move_webhosting_files", W, &["webhosting_id", "files"]),
        ("webhosting", "publish_server_file_to_webhosting", W, &["webhosting_id", "server_id", "path", "dest", "overwrite"]),
        ("webhosting", "read_webhosting_file", R, &["webhosting_id", "path", "offset", "limit", "tail"]),
        ("webhosting", "reset_webhosting", D, &["webhosting_id", "confirm_name"]),
        ("webhosting", "reset_webhosting_password", W, &["webhosting_id", "target"]),
        ("webhosting", "restore_webhosting_backup", D, &["webhosting_id", "backup"]),
        ("webhosting", "search_webhosting_files", R, &["webhosting_id", "query", "path", "type"]),
        ("webhosting", "secure_webhosting_ssl", W, &["webhosting_id"]),
        ("webhosting", "set_webhosting_domain", D, &["webhosting_id", "name"]),
        ("webhosting", "set_webhosting_home", W, &["webhosting_id", "path"]),
        ("webhosting", "set_webhosting_php_version", W, &["webhosting_id", "version"]),
        ("webhosting", "write_webhosting_file", W, &["webhosting_id", "path", "content"]),
    ];

    fn def(name: &str, kind: K, params: &[&str]) -> Value {
        let props: Map<String, Value> = params
            .iter()
            .map(|p| ((*p).to_string(), json!({ "type": "string" })))
            .collect();
        let required: Vec<&str> = params
            .iter()
            .copied()
            .filter(|p| *p == "server_id")
            .collect();
        let annotations = match kind {
            R => json!({ "readOnlyHint": true }),
            W => json!({ "readOnlyHint": false, "destructiveHint": false }),
            D => json!({ "readOnlyHint": false, "destructiveHint": true }),
        };
        json!({ "name": name, "description": format!("outil {name}"), "annotations": annotations,
                "inputSchema": { "type": "object", "properties": props, "required": required } })
    }

    /// Catalogue tel que le renverrait l'officiel pour ces toolsets.
    fn catalog_for(toolsets: &str) -> Catalog {
        let sets: Vec<&str> = toolsets.split(',').collect();
        let rows: Vec<_> = RELEVE.iter().filter(|(ts, ..)| sets.contains(ts)).collect();
        Catalog {
            tools: rows.iter().map(|(_, n, k, p)| def(n, *k, p)).collect(),
            read: rows
                .iter()
                .filter(|(_, _, k, _)| *k == R)
                .map(|(_, n, ..)| n.to_string())
                .collect(),
        }
    }

    /// Catalogue local complet (ce que voit un client du MCP local).
    fn all_local() -> HashSet<String> {
        names_of(crate::mcp::tool_list())
    }

    /// Catalogue local du Copilote quand l'officiel répond (recentré) + `report_diagnosis`.
    fn copilot_local() -> HashSet<String> {
        let mut s: HashSet<String> = crate::mcp::LOCAL_KEEP_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect();
        s.insert("report_diagnosis".into());
        s
    }

    #[test]
    fn releve_matches_the_known_shape() {
        assert_eq!(RELEVE.len(), 107);
        assert_eq!(RELEVE.iter().filter(|r| r.2 == R).count(), 43);
        let toolsets: HashSet<&str> = RELEVE.iter().map(|r| r.0).collect();
        assert_eq!(toolsets.len(), 10);
        // Le Copilote a 63 outils dans ses toolsets, dont 27 en lecture.
        let c = catalog_for(COPILOT_TOOLSETS);
        assert_eq!((c.tools.len(), c.read.len()), (63, 27));
    }

    #[test]
    fn copilot_writes_are_deny_by_default() {
        let cat = catalog_for(COPILOT_TOOLSETS);
        for t in &cat.tools {
            let n = tool_name(t).unwrap();
            // Suggestion / diagnostic : lecture seule, rien d'autre.
            assert_eq!(
                copilot_allows(&cat, n, false),
                cat.is_read(n),
                "{n} (suggestion)"
            );
            // Autonome : lecture + liste blanche, exactement.
            let expected = cat.is_read(n) || COPILOT_OFFICIAL_WRITES.contains(&n);
            assert_eq!(copilot_allows(&cat, n, true), expected, "{n} (autonome)");
        }
        for n in [
            "delete_server_from_mybox",
            "create_server_on_mybox",
            "update_server_resources",
            "enable_disable_server",
            "reinstall_server",
            "restore_snapshot",
            "restore_smartbackup",
            "delete_snapshot",
            "manage_database",
            "restore_db_backup",
            "create_database",
            "send_rcon_command",
            "create_ticket",
            "rename_resource",
            "manage_ports",
            "set_server_address",
            "manage_java",
            "change_version",
            "install_modpack",
            "manage_worlds",
            "install_world",
            "create_schedule",
            "edit_schedule",
            "delete_schedule",
            "set_startup_config",
        ] {
            assert!(
                !copilot_allows(&cat, n, true),
                "{n} ne doit jamais être donné au Copilote"
            );
        }
        // Outil inconnu (futur ou halluciné) : refusé, même s'il porte un nom de la liste blanche
        // sans exister dans le catalogue.
        assert!(!copilot_allows(&cat, "wipe_everything", true));
        let empty = Catalog {
            tools: vec![],
            read: HashSet::new(),
        };
        assert!(!copilot_allows(&empty, "power_action", true));
    }

    #[test]
    fn copilot_whitelist_is_server_scoped_and_not_destructive() {
        let cat = catalog_for(COPILOT_TOOLSETS);
        for n in COPILOT_OFFICIAL_WRITES {
            let d = cat.get(n).unwrap_or_else(|| panic!("{n} absent du relevé"));
            assert!(!cat.is_read(n), "{n} : modifiant");
            assert!(
                requires(d, "server_id") && !is_destructive(d),
                "{n} : borné à un serveur, non destructif"
            );
        }
        // Si l'officiel annonce un jour un de ces outils comme destructif, il sort de la liste.
        let mut cat = catalog_for(COPILOT_TOOLSETS);
        let i = cat
            .tools
            .iter()
            .position(|t| t["name"] == "install_mod")
            .unwrap();
        cat.tools[i]["annotations"] = json!({ "readOnlyHint": false });
        assert!(
            !copilot_allows(&cat, "install_mod", true),
            "annotation absente = destructif"
        );
    }

    #[test]
    fn copilot_calls_are_scoped_to_the_session_server() {
        let cat = catalog_for(COPILOT_TOOLSETS);
        // Autre serveur : refusé (en écriture comme en lecture).
        assert!(copilot_check(
            &cat,
            7,
            true,
            "power_action",
            json!({ "server_id": 8, "action": "stop" })
        )
        .is_err());
        assert!(copilot_check(
            &cat,
            7,
            false,
            "get_console_logs",
            json!({ "server_id": 8 })
        )
        .is_err());
        assert!(
            copilot_check(&cat, 7, true, "power_action", json!({ "server_id": "abc" })).is_err()
        );
        // Même serveur (nombre ou texte) : accepté ; absent : ajouté.
        let (args, read) = copilot_check(
            &cat,
            7,
            true,
            "power_action",
            json!({ "server_id": "7", "action": "restart" }),
        )
        .unwrap();
        assert!(!read && args["action"] == "restart");
        let (args, read) = copilot_check(&cat, 7, false, "get_console_logs", json!({})).unwrap();
        assert!(read && args["server_id"] == 7);
        // Outil sans `server_id` : rien d'ajouté.
        let (args, _) = copilot_check(&cat, 7, false, "list_servers", Value::Null).unwrap();
        assert_eq!(args, json!({}));
        // Mode suggestion : l'écriture est refusée avec une invite à la proposer.
        let e = copilot_check(&cat, 7, false, "power_action", json!({ "action": "start" }))
            .unwrap_err();
        assert!(e.contains("suggestion"), "{e}");
        // Hors liste ou halluciné : refus clair.
        assert!(copilot_check(
            &cat,
            7,
            true,
            "delete_server_from_mybox",
            json!({ "server_id": 7 })
        )
        .unwrap_err()
        .contains("refusée"));
        assert!(copilot_check(&cat, 7, true, "format_disk", json!({}))
            .unwrap_err()
            .contains("inconnu"));
        assert!(copilot_check(&cat, 7, true, "power_action", json!("start")).is_err());
    }

    #[test]
    fn secrets_are_never_revealed() {
        let cat = catalog_for(COPILOT_TOOLSETS);
        let (args, _) = copilot_check(
            &cat,
            7,
            false,
            "get_server",
            json!({ "server_id": 7, "reveal_credentials": true }),
        )
        .unwrap();
        assert!(args.get("reveal_credentials").is_none());
        let shown = present(cat.get("get_server").unwrap(), None);
        assert!(!has_prop(&shown, "reveal_credentials"));
    }

    #[test]
    fn copilot_keeps_one_tool_per_function() {
        let cat = catalog_for(COPILOT_TOOLSETS);
        for autonomous in [false, true] {
            let local = copilot_local();
            let view = copilot_view(&cat, &local, autonomous);
            let kept_local: HashSet<&String> = local
                .iter()
                .filter(|n| !view.masked_local.contains(*n))
                .collect();
            let kept_official: HashSet<&str> = view.tools.iter().map(|t| t.name.as_str()).collect();
            // Aucun outil officiel gardé n'a d'équivalent local gardé (et aucun nom partagé).
            for &(off, loc, _) in EQUIVALENTS {
                assert!(
                    !(kept_official.contains(off) && kept_local.contains(&loc.to_string())),
                    "doublon : {off} (officiel) et {loc} (local)"
                );
            }
            for n in &kept_official {
                assert!(!kept_local.contains(&n.to_string()), "nom partagé : {n}");
            }
            // Les quatre paires : le meilleur de chacune est gardé.
            assert!(
                kept_local.contains(&"diagnose_startup".to_string())
                    && !kept_official.contains("diagnose_server")
            );
            assert!(
                kept_local.contains(&"disk_usage".to_string())
                    && !kept_official.contains("get_disk_usage")
            );
            assert!(
                kept_local.contains(&"server_metrics".to_string())
                    && !kept_official.contains("get_server_stats_history")
            );
            assert!(
                kept_official.contains("get_spark_report")
                    && !kept_local.contains(&"parse_spark_report".to_string())
            );
            // Pas d'outil modifiant hors liste blanche, aucun en suggestion.
            for t in &view.tools {
                assert!(
                    t.read || (autonomous && COPILOT_OFFICIAL_WRITES.contains(&t.name.as_str())),
                    "{}",
                    t.name
                );
            }
        }
    }

    #[test]
    fn equivalences_name_real_tools() {
        let local = all_local();
        let official: HashSet<&str> = RELEVE.iter().map(|r| r.1).collect();
        for &(off, loc, _) in EQUIVALENTS {
            assert!(
                official.contains(off),
                "{off} : outil officiel inconnu du relevé"
            );
            assert!(local.contains(loc), "{loc} : outil local inexistant");
        }
    }

    #[test]
    fn gateway_relays_everything_but_files_without_duplicates() {
        assert!(!GATEWAY_TOOLSETS.split(',').any(|t| t == "files"));
        let all: HashSet<&str> = RELEVE
            .iter()
            .map(|r| r.0)
            .filter(|t| *t != "files")
            .collect();
        assert_eq!(GATEWAY_TOOLSETS.split(',').collect::<HashSet<_>>(), all);

        let cat = catalog_for(GATEWAY_TOOLSETS);
        let local = all_local();
        let view = gateway_view(&cat, &local);
        let kept_local: HashSet<&String> = local
            .iter()
            .filter(|n| !view.masked_local.contains(*n))
            .collect();
        let mut seen = HashSet::new();
        for t in &view.tools {
            assert!(seen.insert(t.name.clone()), "{} relayé deux fois", t.name);
            assert!(
                !kept_local.contains(&t.name),
                "collision de noms : {}",
                t.name
            );
            assert_eq!(t.def["name"], t.name.as_str());
            for &(off, loc, _) in EQUIVALENTS {
                assert!(
                    !(t.name == off && kept_local.contains(&loc.to_string())),
                    "{off} doublonne {loc}"
                );
            }
        }
        // Chaque outil du catalogue est soit relayé, soit masqué avec une raison.
        for t in &cat.tools {
            let n = tool_name(t).unwrap();
            assert!(
                seen.contains(n) ^ view.masked_official.contains_key(n),
                "{n}"
            );
        }
        // Lecture = liste `readonly=1` ; le reste (inconnu compris) est modifiant.
        for t in &view.tools {
            assert_eq!(t.read, cat.is_read(&t.name), "{}", t.name);
        }
    }

    #[test]
    fn gateway_destructive_tools_need_a_verified_confirmation() {
        let cat = catalog_for(GATEWAY_TOOLSETS);
        let view = gateway_view(&cat, &all_local());
        for t in &view.tools {
            let destructive = !t.read && is_destructive(cat.get(&t.name).unwrap());
            assert_eq!(destructive, t.confirm.is_some(), "{}", t.name);
            if destructive {
                assert!(
                    requires(&t.def, "confirm_name"),
                    "{} : confirm_name obligatoire",
                    t.name
                );
                assert!(t.def["description"]
                    .as_str()
                    .unwrap()
                    .starts_with("⚠ DESTRUCTIF"));
            } else {
                assert!(
                    !has_prop(&t.def, "confirm_name") || t.official_confirm,
                    "{}",
                    t.name
                );
            }
        }
        let target = |n: &str| {
            view.tools
                .iter()
                .find(|t| t.name == n)
                .and_then(|t| t.confirm)
        };
        assert_eq!(target("reinstall_server"), Some(Target::Server));
        assert_eq!(
            target("restore_snapshot"),
            Some(Target::Server),
            "la cible écrasée est le serveur"
        );
        assert_eq!(target("delete_snapshot"), Some(Target::Snapshot));
        assert_eq!(target("restore_db_backup"), Some(Target::MyBox));
        assert_eq!(target("delete_webhosting"), Some(Target::Webhosting));
        assert!(
            view.tools
                .iter()
                .find(|t| t.name == "delete_webhosting")
                .unwrap()
                .official_confirm
        );
        // Remplacés par nos équivalents gardés.
        assert!(view.masked_official["delete_server_from_mybox"].contains("mybox_delete_server"));
        assert!(view.masked_official["delete_schedule"].contains("delete_schedule"));
        // Destructif sans ressource vérifiable (ou sans annotation) : masqué.
        let odd = Catalog {
            tools: vec![
                json!({ "name": "purge_all", "inputSchema": { "type": "object", "properties": { "scope": {} } } }),
                json!({ "name": "nouvel_outil", "inputSchema": { "type": "object", "properties": { "server_id": {} } } }),
            ],
            read: HashSet::new(),
        };
        let v = gateway_view(&odd, &HashSet::new());
        assert!(v.masked_official.contains_key("purge_all"));
        let nouveau = v.tools.iter().find(|t| t.name == "nouvel_outil").unwrap();
        assert!(
            !nouveau.read && nouveau.confirm == Some(Target::Server),
            "inconnu = modifiant et destructif"
        );
    }

    #[test]
    fn relay_mode_follows_env_then_setting() {
        assert_eq!(resolve_mode(None, None, false), RelayMode::Off);
        assert_eq!(resolve_mode(None, None, true), RelayMode::Gateway);
        assert_eq!(resolve_mode(Some("1"), None, false), RelayMode::Gateway);
        assert_eq!(resolve_mode(Some("off"), None, true), RelayMode::Off);
        assert_eq!(
            resolve_mode(Some("peut-être"), None, true),
            RelayMode::Gateway
        );
        // Processus du Copilote : jamais la passerelle ; relais seulement demandé, borné au serveur.
        assert_eq!(
            resolve_mode(Some("1"), Some("42"), false),
            RelayMode::Copilot(42)
        );
        assert_eq!(resolve_mode(None, Some("42"), true), RelayMode::Off);
        assert_eq!(resolve_mode(Some("0"), Some("42"), true), RelayMode::Off);
        assert_eq!(resolve_mode(Some("1"), Some("x"), true), RelayMode::Off);
    }

    #[test]
    fn confirmation_is_exact_and_webhosting_domain_is_read() {
        assert!(confirmation_matches("Survie", " Survie "));
        assert!(!confirmation_matches("Survie", "survie"));
        assert!(!confirmation_matches("", ""));
        assert_eq!(
            webhosting_domain(r#"{"webhosting":{"id":3,"domain":"site.example"}}"#).as_deref(),
            Some("site.example")
        );
        assert_eq!(
            webhosting_domain(r#"{"domain":"a.b"}"#).as_deref(),
            Some("a.b")
        );
        assert_eq!(webhosting_domain("pas du json"), None);
        assert_eq!(webhosting_domain(r#"{"webhosting":{"domain":""}}"#), None);
    }

    #[test]
    fn scope_violation_reads_the_server_id() {
        assert!(scope_violation(5, &json!({})).is_none());
        assert!(scope_violation(5, &json!({ "server_id": null })).is_none());
        assert!(scope_violation(5, &json!({ "server_id": 5 })).is_none());
        assert!(scope_violation(5, &json!({ "server_id": " 5 " })).is_none());
        assert!(scope_violation(5, &json!({ "server_id": 6 })).is_some());
        assert!(scope_violation(5, &json!({ "server_id": [5] })).is_some());
        // Second serveur des outils locaux (diff_files, copie) et listes (find_player).
        assert!(scope_violation(5, &json!({ "server_id": 5, "other_server_id": 5 })).is_none());
        assert!(scope_violation(5, &json!({ "server_id": 5, "other_server_id": 6 })).is_some());
        assert!(scope_violation(5, &json!({ "source_server_id": 5, "target_server_id": 6 })).is_some());
        assert!(scope_violation(5, &json!({ "server_ids": [5] })).is_none());
        assert!(scope_violation(5, &json!({ "server_ids": [5, 6] })).is_some());
        assert!(scope_violation(5, &json!({ "server_ids": "5" })).is_some());
        assert_eq!(bound_local_args(5, "find_player", json!({ "query": "Al" }))["server_ids"], json!([5]));
        assert_eq!(bound_local_args(5, "find_player", json!({ "server_ids": null }))["server_ids"], json!([5]));
        assert_eq!(bound_local_args(5, "list_players", json!({ "server_id": 5 })), json!({ "server_id": 5 }));
        assert_eq!(bound_local_args(5, "list_incidents", json!({ "limit": 3 }))["server_id"], json!(5));
    }
}
