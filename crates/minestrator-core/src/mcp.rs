//! Logique MCP **indépendante du transport** : traite un message JSON-RPC et renvoie
//! la réponse. Réutilisée par le binaire stdio (`minestrator-mcp`), par le mode `--mcp`
//! de l'app desktop, et à terme par un daemon.
//!
//! Réglable via [`McpConfig`] (persistée) : activer/désactiver, et autoriser ou non les
//! actions modifiantes (mode lecture seule).

use crate::models::InstalledItem;
use crate::updates::ContentKind;
use crate::{Core, HistoryPoint, HistorySource, MetricsHistory};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

/// Réglages du serveur MCP (modifiables depuis l'UI, persistés dans `mcp.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct McpConfig {
    pub enabled: bool,
    /// Autorise les actions modifiantes (power, commandes, modération, écriture de fichiers).
    pub allow_writes: bool,
    /// Passerelle vers le MCP officiel MineStrator : relaie ses outils (tous toolsets sauf
    /// `files`) sur cette même connexion, la clé restant au trousseau. Désactivée par défaut (voir
    /// `official_relay.rs`) ; la variable `MINESTRATOR_MCP_OFFICIAL` prime sur ce réglage.
    #[serde(default)]
    pub official_gateway: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_writes: true,
            official_gateway: false,
        }
    }
}

/// Sert le protocole MCP sur **stdio** : lit stdin ligne à ligne, délègue à [`handle_message`],
/// écrit les réponses sur stdout. **stdout = protocole ; les logs vont sur stderr.** Partagé par
/// le binaire `minestrator-mcp` et le mode `--mcp` de l'app.
pub async fn serve_stdio(core: &Core) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(response) = handle_message(core, msg).await {
            if let Ok(mut s) = serde_json::to_string(&response) {
                s.push('\n');
                let _ = stdout.write_all(s.as_bytes()).await;
                let _ = stdout.flush().await;
            }
        }
    }
}

/// Traite un message JSON-RPC MCP. Renvoie la réponse, ou `None` pour une notification.
pub async fn handle_message(core: &Core, msg: Value) -> Option<Value> {
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => reply(id, initialize()),
        "tools/list" => {
            let tools = if mcp_enabled(core) {
                let mut tools = filter_tools_by_env(tool_list());
                // Passerelle / relais du Copilote vers le MCP officiel (official_relay.rs).
                crate::official_relay::extend_tool_list(core, &mut tools).await;
                tools
            } else {
                json!([])
            };
            reply(id, json!({ "tools": tools }))
        }
        "tools/call" => reply(id, call(core, params).await),
        "ping" => reply(id, json!({})),
        _ if method.starts_with("notifications/") => None,
        _ => id.map(|id| {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "méthode inconnue" } })
        }),
    }
}

fn reply(id: Option<Value>, result: Value) -> Option<Value> {
    id.map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn initialize() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "minestrator-mcp", "version": env!("CARGO_PKG_VERSION") }
    })
}

// --- Catalogue d'outils (source unique noms + genre) ----------------------

/// Outils de **lecture** (sûrs, autorisés même en mode lecture seule). **Source unique** :
/// le Copilote et les listes MCP dérivent de cette constante. Tout outil ABSENT d'ici est
/// traité comme **modifiant** (default-deny) → un nouvel outil est gaté par défaut.
pub(crate) const READ_TOOLS: &[&str] = &[
    "list_servers",
    "server_status",
    "server_metrics",
    "read_console",
    "list_files",
    "read_file",
    "read_gz",
    "list_archive",
    "read_archive_entry",
    "search_files",
    "disk_usage",
    "download_file",
    "file_stat",
    "read_startup",
    "read_properties",
    "mybox_info",
    // Architecture des MyBox (mybox.rs) : lectures et calcul pur, sans effet.
    "mybox_capacity",
    "server_catalog",
    "plan_placement",
    "mybox_smartscan",
    "list_schedules",
    "list_players",
    "list_installed_mods",
    "list_installed_plugins",
    // Vérification des mises à jour : pure comparaison côté hébergeur, rien n'est installé.
    "check_updates",
    "list_backups",
    "list_snapshots",
    "market_search",
    "list_mod_versions",
    "ficsit_search",
    "ficsit_versions",
    "analyze_performance",
    "parse_spark_report",
    "diagnose_startup",
    "inspect_region",
    // Plusieurs serveurs (fleet.rs) : lectures qui peuvent déborder du serveur de la session.
    "diff_files",
    "find_player",
];

/// Valeur renvoyée au modèle à la place d'un réglage secret (cf. `read_properties`).
const SECRET_MASK: &str = "[SECRET]";

/// Outils **modifiants** (soumis à `allow_writes`). Sert aux listes d'autorisation/schémas ;
/// la garde de sécurité, elle, repose sur le complément de [`READ_TOOLS`] (default-deny).
pub(crate) const WRITE_TOOLS: &[&str] = &[
    "power_action",
    "send_command",
    "player_action",
    "write_file",
    "upload_file",
    "edit_file",
    "create_dir",
    "delete_path",
    "rename_path",
    "extract_archive",
    // Compression côté serveur : crée une archive sur le serveur (fichier en plus, sans risque
    // pour l'existant, mais elle occupe du disque) → modifiant.
    "compress_files",
    "set_startup_params",
    "set_property",
    "install_mod",
    "update_mods",
    "install_ficsit_mod",
    // Snapshot = additif/sans risque → l'agent peut le créer (filet avant une intervention).
    // NB : restore_snapshot / restore_backup / delete_snapshot sont DESTRUCTIFS et volontairement
    // ABSENTS d'ici → jamais auto-exécutés par l'agent ; uniquement via « Appliquer » (intent user).
    "create_snapshot",
    // Infrastructure MyBox (voir INFRA_TOOLS) : jamais proposés au Copilote.
    "mybox_create_server",
    "mybox_resize_server",
    "rename_server",
    "rename_mybox",
    "mybox_pause_server",
    "mybox_reorder_servers",
    "mybox_delete_server",
    // Tâches planifiées du panel (configuration persistante, voir INFRA_TOOLS) : jamais au Copilote.
    "create_schedule",
    "update_schedule",
    "delete_schedule",
    "add_schedule_task",
    "update_schedule_task",
    "delete_schedule_task",
    "reorder_schedule_tasks",
    // Plusieurs serveurs (fleet.rs, voir INFRA_TOOLS) : jamais proposés au Copilote.
    "copy_between_servers",
    "broadcast_command",
];

/// Outils modifiants **jamais donnés au Copilote** : infrastructure (création, redimensionnement,
/// pause, suppression de serveurs, noms, ordre), actions multi-serveurs et configuration
/// persistante (tâches planifiées du panel, exécutées par l'hébergeur même app fermée). Exposés au
/// serveur MCP local (soumis à `allow_writes`), mais le Copilote lit du contenu non fiable (chat
/// des joueurs) : une injection ne doit pas pouvoir supprimer ou créer des serveurs, ni planter une
/// commande qui se rejouerait chaque nuit. Absents de son catalogue, de la liste blanche de ses
/// agents CLI et d'« Appliquer » (`copilot.rs`, `Core::copilot_apply`).
pub(crate) const INFRA_TOOLS: &[&str] = &[
    "mybox_create_server",
    "mybox_resize_server",
    "rename_server",
    "rename_mybox",
    "mybox_pause_server",
    "mybox_reorder_servers",
    "mybox_delete_server",
    "create_schedule",
    "update_schedule",
    "delete_schedule",
    "add_schedule_task",
    "update_schedule_task",
    "delete_schedule_task",
    "reorder_schedule_tasks",
    // Plusieurs serveurs (fleet.rs) : agissent au-delà du serveur du Copilote (copie entre
    // serveurs, commande groupée) → réservés eux aussi à l'app et au MCP local.
    "copy_between_servers",
    "broadcast_command",
    // Commande de démarrage : exécutée par le shell du conteneur à chaque démarrage, une injection
    // y deviendrait une commande persistante. Même règle que `set_startup_config` côté officiel.
    "set_startup_params",
];

/// Rappel d'unités joint aux sorties MyBox : un agent ne doit pas confondre cœurs et %, Go et Mo.
const MYBOX_UNITS: &str = "CPU en cœurs entiers (1 = un cœur, pas un pourcentage) ; RAM en Go entiers (pas en Mo) ; disque total en Go, occupation en Mo. Le disque est partagé par toute la MyBox ; seuls les serveurs actifs consomment CPU et RAM (un serveur en pause les rend) ; FlexCore = cœurs de rafale partagés, non allouables.";

/// Un outil est-il modifiant ? Default-deny : tout ce qui n'est PAS un outil de lecture connu.
pub(crate) fn is_write_tool(name: &str) -> bool {
    !READ_TOOLS.contains(&name)
}

/// Outils propres à **Minecraft** (marketplace, joueurs, Spark, flags JVM, régions…) — masqués pour
/// l'IA d'un serveur d'un autre jeu.
pub(crate) const MINECRAFT_ONLY_TOOLS: &[&str] = &[
    "player_action",
    "list_players",
    "market_search",
    "list_mod_versions",
    "install_mod",
    "list_installed_mods",
    "list_installed_plugins",
    "check_updates",
    "update_mods",
    "read_startup",
    "set_startup_params",
    "read_properties",
    "set_property",
    "analyze_performance",
    "parse_spark_report",
    "diagnose_startup",
    "inspect_region",
    "repair_region",
];

/// Outils propres à **Satisfactory** (mods via ficsit.app).
pub(crate) const SATISFACTORY_ONLY_TOOLS: &[&str] =
    &["ficsit_search", "ficsit_versions", "install_ficsit_mod"];

/// Un outil est-il pertinent pour la famille de jeu `family` ? Universel (console/SFTP/métriques/
/// backups/power…) par défaut ; sinon réservé au jeu correspondant.
pub(crate) fn tool_for_game(name: &str, family: &str) -> bool {
    if SATISFACTORY_ONLY_TOOLS.contains(&name) {
        return family == "satisfactory";
    }
    if MINECRAFT_ONLY_TOOLS.contains(&name) {
        return family == "minecraft";
    }
    true
}

/// Sous-ensemble que NOTRE MCP conserve quand le **MCP officiel** prend en charge la gestion : le
/// SFTP fin + nos outils EXCLUSIFS (absents de l'officiel). Tout le reste (serveurs, power, console,
/// joueurs, market, backups, démarrage) est alors délégué à l'officiel → pas de doublon pour l'IA.
pub(crate) const LOCAL_KEEP_TOOLS: &[&str] = &[
    "list_files",
    "read_file",
    "read_gz",
    "list_archive",
    "read_archive_entry",
    "search_files",
    "download_file",
    "file_stat",
    "write_file",
    "upload_file",
    "edit_file",
    "create_dir",
    "delete_path",
    "rename_path",
    "extract_archive",
    "compress_files",
    "disk_usage",
    "inspect_region",
    "diagnose_startup",
    "analyze_performance",
    "parse_spark_report",
    // Historique local (SQLite) : l'API n'a aucun historique, l'officiel non plus.
    "server_metrics",
    // Mises à jour des mods/plugins/datapacks, avec le garde-fou local : hors catalogue de l'officiel.
    "check_updates",
    "update_mods",
    // Joueurs déjà venus, temps de jeu, dernière venue (endpoint non documenté) : hors officiel.
    "list_players",
    // Mods Satisfactory (ficsit.app) : hors catalogue de l'officiel.
    "ficsit_search",
    "ficsit_versions",
    "install_ficsit_mod",
    // Plusieurs serveurs (fleet.rs) : comparaison et recherche de joueur, hors catalogue de l'officiel.
    "diff_files",
    "find_player",
];

// --- tools/call -----------------------------------------------------------

/// Le MCP est-il actif : réglage utilisateur OU forçage par variable d'env (lancement par le
/// Copilote en mode agent CLI).
fn mcp_enabled(core: &Core) -> bool {
    core.get_mcp_config().enabled || std::env::var(crate::config::MCP_FORCE_ENABLED_ENV).is_ok()
}

/// Liste blanche d'outils imposée par l'env (voir `MCP_ALLOWED_TOOLS_ENV`). `None` = aucune
/// restriction. Vide (`[]`) = aucun outil autorisé. C'est le garde-fou uniforme, honoré quel que
/// soit l'agent CLI qui se connecte.
fn parse_allowlist(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn env_tool_allowlist() -> Option<Vec<String>> {
    std::env::var(crate::config::MCP_ALLOWED_TOOLS_ENV)
        .ok()
        .map(|s| parse_allowlist(&s))
}

/// Un outil est-il autorisé par la liste blanche d'env ? (Vrai si aucune liste n'est posée.)
pub(crate) fn env_allows(name: &str) -> bool {
    match env_tool_allowlist() {
        Some(list) => list.iter().any(|t| t == name),
        None => true,
    }
}

/// Une liste blanche d'env est-elle posée (serveur lancé par le Copilote) ?
pub(crate) fn env_has_allowlist() -> bool {
    env_tool_allowlist().is_some()
}

/// Filtre un tableau d'outils (`tool_list()`) selon la liste blanche d'env.
fn filter_tools_by_env(tools: &Value) -> Value {
    // Résolution de la liste blanche UNE fois (sinon on relisait/reparsait l'env par outil).
    let Some(allow) = env_tool_allowlist() else {
        return tools.clone(); // aucune restriction → copie du catalogue
    };
    let Some(arr) = tools.as_array() else {
        return Value::Array(Vec::new());
    };
    let kept: Vec<Value> = arr
        .iter()
        .filter(|t| {
            t.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|name| allow.iter().any(|a| a == name))
        })
        .cloned()
        .collect();
    Value::Array(kept)
}

async fn call(core: &Core, params: Value) -> Value {
    let cfg = core.get_mcp_config();
    if !mcp_enabled(core) {
        return tool_err("MCP désactivé dans les réglages de l'application.");
    }

    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    // Un outil atteignable par le PROTOCOLE MCP doit appartenir à la surface exposée (READ ∪ WRITE).
    // Les outils destructifs internes (restore_snapshot/restore_backup/delete_snapshot/repair_region),
    // routés uniquement via « Appliquer » (copilot_apply → dispatch, hors de ce handler), ne sont donc
    // PAS atteignables ici — y compris par un client MCP externe sans liste blanche d'env.
    if !READ_TOOLS.contains(&name.as_str()) && !WRITE_TOOLS.contains(&name.as_str()) {
        // Outil du MCP officiel relayé (passerelle ou Copilote) : gating propre, voir official_relay.rs.
        if let Some(out) = crate::official_relay::relay_call(core, &cfg, &name, args).await {
            return out;
        }
        return tool_err("Outil inconnu ou non exposé par le serveur MCP.");
    }
    // Serveur lancé par le Copilote pour un agent CLI : aucun appel hors du serveur de la session.
    if let Some(refus) = crate::official_relay::env_scope_violation(&args) {
        return tool_err(&refus);
    }

    // Garde-fou uniforme : la liste blanche d'env prime (posée par le Copilote selon lecture/écriture).
    if !env_allows(&name) {
        return tool_err("Outil non autorisé par la politique de permissions.");
    }
    if is_write_tool(&name) && !cfg.allow_writes {
        return tool_err("Actions modifiantes désactivées (mode lecture seule) dans les réglages.");
    }

    match dispatch(core, &name, args).await {
        Ok(text) => tool_text(text),
        Err(e) => tool_err(&format!("Erreur : {e}")),
    }
}

fn tool_text(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}
fn tool_err(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Rend une liste d'items installés de façon COMPACTE : le JSON complet dépassait 70 Ko (et la
/// limite de tokens de l'agent) sur un gros modpack, forçant l'agent à sauvegarder puis relire le
/// résultat par morceaux. Filtrable par `query` (sous-chaîne du nom, insensible à la casse) ;
/// sinon plafonné, avec invite à filtrer.
fn format_installed(items: &[InstalledItem], query: &str, kind: &str) -> String {
    const CAP: usize = 300;
    let q = query.trim().to_lowercase();
    let matches: Vec<&InstalledItem> = items
        .iter()
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q))
        .collect();

    // Loaders présents (souvent uniformes sur un modpack) : résumés en en-tête, pas répétés par ligne.
    let mut loaders: Vec<&str> = items
        .iter()
        .map(|m| m.loader.as_str())
        .filter(|l| !l.is_empty())
        .collect();
    loaders.sort_unstable();
    loaders.dedup();

    let mut out = if q.is_empty() {
        format!("{} {kind}(s) installé(s)", items.len())
    } else {
        format!(
            "{} {kind}(s) correspondant à « {query} » (sur {} installés)",
            matches.len(),
            items.len()
        )
    };
    if !loaders.is_empty() {
        let _ = write!(out, " — loaders : {}", loaders.join(", "));
    }
    if matches.is_empty() {
        out.push_str("\n(aucun)");
        return out;
    }
    for m in matches.iter().take(CAP) {
        let _ = write!(out, "\n- {} ({})", m.name, m.version);
        if !m.enabled {
            out.push_str(" [désactivé]");
        }
    }
    if matches.len() > CAP {
        let _ = write!(
            out,
            "\n… (+{} autres — précise « query » pour filtrer par nom)",
            matches.len() - CAP
        );
    }
    out
}

/// Point d'entrée : enrobe [`dispatch_inner`] d'un cache TTL (lectures stables) et **purge** le
/// cache après toute opération modifiante (l'état serveur lu a pu changer).
pub(crate) async fn dispatch(core: &Core, name: &str, args: Value) -> Result<String, String> {
    if is_write_tool(name) {
        let out = dispatch_inner(core, name, args).await;
        core.cache().clear();
        return out;
    }
    if let Some(ttl) = cache_ttl(name) {
        let key = format!("{name}\u{1}{args}");
        if let Some(hit) = core.cache().get(&key) {
            return Ok(hit);
        }
        let out = dispatch_inner(core, name, args).await?;
        core.cache().put(key, out.clone(), ttl);
        return Ok(out);
    }
    dispatch_inner(core, name, args).await
}

/// TTL de cache d'un outil de LECTURE cacheable (`None` = jamais caché). Toute lecture cachée est
/// de toute façon purgée dès la 1re écriture (voir [`dispatch`]) → pas de risque de valeur périmée
/// après une modification.
fn cache_ttl(tool: &str) -> Option<Duration> {
    let secs = match tool {
        // Données marketplace externes, très stables.
        "market_search" | "list_mod_versions" => 300,
        // Inventaire serveur : change rarement en session (et purgé après install_mod & co).
        "list_installed_mods" | "list_installed_plugins" => 30,
        // Backups (toutes les ~2 h) / snapshots (créés à la demande) : évoluent lentement.
        "list_backups" | "list_snapshots" => 30,
        // Inspection d'une région .mca : télécharge le fichier → on évite de le re-télécharger.
        "inspect_region" => 30,
        // Archives / gz : téléchargent + décompressent → on évite de re-télécharger dans la foulée.
        "list_archive" | "read_archive_entry" | "read_gz" => 30,
        // Config / fichiers / arborescence : purgés après toute écriture, donc sûrs à cacher un instant.
        "list_files" | "read_file" | "read_startup" | "read_properties" | "search_files" => 15,
        // Analyse du disque : calculée par le serveur sur tout un arbre, change lentement.
        "disk_usage" => 30,
        "mybox_info" => 30,
        "mybox_capacity" => 10,
        "server_catalog" => 300,
        "mybox_smartscan" => 60,
        "list_schedules" => 15,
        // Liste des joueurs : déjà gardée une minute par le cœur (purgée par une action joueur).
        "list_players" => 30,
        "list_servers" => 10,
        "server_status" => 5,
        // Volontairement absents : server_metrics (local, et cache propre des statistiques de
        // l'hébergeur), read_console (live),
        // analyze_performance (lance le profiler Spark), parse_spark_report (rare).
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

async fn dispatch_inner(core: &Core, name: &str, args: Value) -> Result<String, String> {
    match name {
        "list_servers" => json_pretty(&core.list_servers().await.map_err(es)?),

        "server_status" => {
            let id = req_i64(&args, "server_id")?;
            // `/live` : tout en un GET (consommation instantanée dont FlexCore, réseau, uptime,
            // joueurs). Repli sur l'ancien couple live/light + échantillon WebSocket si l'endpoint
            // échoue (lectures concurrentes : le RTT du GET se recouvre sous l'attente du WS).
            match core.server_live(id).await {
                Ok(live) => json_pretty(&live),
                Err(_) => {
                    let (live, stats) = tokio::join!(core.live_light(id), core.sample_stats(id));
                    json_pretty(&json!({ "live": live.map_err(es)?, "stats": stats.map_err(es)? }))
                }
            }
        }

        "read_properties" => {
            let id = req_i64(&args, "server_id")?;
            let mut props = core.server_properties(id).await.map_err(es)?;
            // Secrets (`rcon.password`…) jamais renvoyés au modèle.
            for (key, value) in props.properties.iter_mut() {
                if crate::models::is_secret_property(key) && !value.is_empty() {
                    *value = SECRET_MASK.to_string();
                }
            }
            json_pretty(&props)
        }

        "set_property" => {
            let id = req_i64(&args, "server_id")?;
            let property = req_str(&args, "property")?;
            let value = opt_str(&args, "value", "");
            // Le modèle ne voit que le masque : le réécrire écraserait le vrai secret.
            if value == SECRET_MASK {
                return Err(format!(
                    "« {property} » est masqué ({SECRET_MASK}) : sa valeur réelle n'est pas \
                     connue, ne pas la réécrire."
                ));
            }
            core.set_server_property(id, &property, &value).await.map_err(es)?;
            Ok(format!(
                "Réglage « {property} » modifié sur le serveur {id} ; effet au prochain démarrage."
            ))
        }

        "mybox_info" => {
            let id = req_i64(&args, "mybox_id")?;
            json_pretty(&core.mybox_details(id).await.map_err(es)?)
        }

        // --- Architecture des MyBox (mybox.rs) ---------------------------------------
        "mybox_capacity" => {
            let caps = match args.get("mybox_id").and_then(Value::as_i64) {
                Some(id) => vec![core.mybox_capacity(id).await.map_err(mybox_es)?],
                None => core.mybox_capacities().await.map_err(mybox_es)?,
            };
            json_pretty(&json!({ "units": MYBOX_UNITS, "myboxes": caps }))
        }

        "server_catalog" => {
            const CAP: usize = 80;
            let query = opt_str(&args, "query", "").trim().to_lowercase();
            let experimental = args.get("include_experimental").and_then(Value::as_bool).unwrap_or(false);
            let catalog = core.server_catalog().await.map_err(mybox_es)?;
            let eggs: Vec<Value> = catalog
                .eggs
                .iter()
                .filter(|e| (experimental || !e.experimental) && (query.is_empty() || e.name.to_lowercase().contains(&query)))
                .map(|e| {
                    json!({
                        "egg_id": e.id, "name": e.name, "category": e.category, "experimental": e.experimental,
                        "recommended": { "cpu": e.cpu, "ram": e.ram, "disk_gb": e.disk },
                        "install_disk_gb": e.install_disk,
                        "required_fields": e.required_fields.iter().map(|f| json!({ "key": f.key, "label": f.label })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let total = eggs.len();
            json_pretty(&json!({
                "units": "recommended : cœurs et Go (0 = non précisé)",
                "eggs": eggs.into_iter().take(CAP).collect::<Vec<_>>(),
                "truncated": total > CAP,
                "locations": catalog.locations,
            }))
        }

        "plan_placement" => {
            let requests: Vec<crate::PlacementRequest> =
                serde_json::from_value(args.get("servers").cloned().unwrap_or_else(|| json!([])))
                    .map_err(|e| format!("paramètre `servers` invalide : {e}"))?;
            let strategy = opt_str(&args, "strategy", "pack");
            let plan = core.mybox_plan_placement(requests, &strategy).await.map_err(mybox_es)?;
            json_pretty(&json!({ "units": MYBOX_UNITS, "plan": plan }))
        }

        "mybox_smartscan" => {
            let id = req_i64(&args, "mybox_id")?;
            json_pretty(&core.mybox_smartscan(id).await.map_err(mybox_es)?)
        }

        "mybox_create_server" => {
            let fields = match args.get("required_fields") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Object(map)) => map
                    .iter()
                    .map(|(k, v)| crate::FieldValue {
                        key: k.clone(),
                        value: v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
                    })
                    .collect(),
                Some(_) => return Err("`required_fields` doit être un objet { CLÉ: valeur }".into()),
            };
            let input = crate::CreateServerInput {
                mybox_id: req_i64(&args, "mybox_id")?,
                name: req_str(&args, "name")?,
                egg_id: req_i64(&args, "egg_id")?,
                experimental: args.get("experimental").and_then(Value::as_bool).unwrap_or(false),
                location_id: args.get("location_id").and_then(Value::as_i64),
                cpu: req_i64(&args, "cpu")?,
                ram: req_i64(&args, "ram")?,
                fields,
            };
            let created = core.mybox_create_server(input).await.map_err(mybox_es)?;
            Ok(core.redact_ai(&format!(
                "Serveur {} créé dans la MyBox {} (adresse : {}). L'hébergeur l'installe maintenant : suis-le avec server_status.",
                created.server_id,
                created.mybox_id,
                if created.ip.is_empty() { "attribuée à l'installation" } else { &created.ip }
            )))
        }

        "mybox_resize_server" => {
            let (mybox, server) = (req_i64(&args, "mybox_id")?, req_i64(&args, "server_id")?);
            let (cpu, ram) = (req_i64(&args, "cpu")?, req_i64(&args, "ram")?);
            core.mybox_resize_server(mybox, server, cpu, ram).await.map_err(mybox_es)?;
            Ok(format!("Serveur {server} : désormais {cpu} cœur(s) et {ram} Go de RAM."))
        }

        "rename_server" => {
            let id = req_i64(&args, "server_id")?;
            core.rename_server(id, &req_str(&args, "name")?).await.map_err(mybox_es)?;
            Ok(format!("Serveur {id} renommé."))
        }

        "rename_mybox" => {
            let id = req_i64(&args, "mybox_id")?;
            core.rename_mybox(id, &req_str(&args, "name")?).await.map_err(mybox_es)?;
            Ok(format!("MyBox {id} renommée."))
        }

        "mybox_pause_server" => {
            let (mybox, server) = (req_i64(&args, "mybox_id")?, req_i64(&args, "server_id")?);
            let paused = args
                .get("paused")
                .and_then(Value::as_bool)
                .ok_or("paramètre `paused` (booléen) requis : true = mettre en pause, false = reprendre")?;
            let out = core.mybox_set_server_paused(mybox, server, paused).await.map_err(mybox_es)?;
            Ok(match (paused, out.job_id) {
                (true, _) => format!("Serveur {server} mis en pause : ses ressources sont rendues à la MyBox, ses fichiers gardés."),
                (false, Some(job)) if out.resuming => format!("Reprise du serveur {server} lancée (job {job}) : ses fichiers sont remis en place, cela peut prendre quelques minutes."),
                (false, _) => format!("Serveur {server} de nouveau actif."),
            })
        }

        "mybox_reorder_servers" => {
            let mybox = req_i64(&args, "mybox_id")?;
            let ids: Vec<i64> = args
                .get("server_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .ok_or("paramètre `server_ids` (tableau d'entiers) requis")?;
            core.mybox_reorder_servers(mybox, ids).await.map_err(mybox_es)?;
            Ok(format!("Ordre des serveurs de la MyBox {mybox} enregistré."))
        }

        "mybox_delete_server" => {
            let (mybox, server) = (req_i64(&args, "mybox_id")?, req_i64(&args, "server_id")?);
            let confirm = req_str(&args, "confirm_name")?;
            let snapshot_first = args.get("snapshot_first").and_then(Value::as_bool).unwrap_or(true);
            let snapshot = core
                .mybox_delete_server_guarded(mybox, server, &confirm, snapshot_first)
                .await
                .map_err(mybox_es)?;
            Ok(match snapshot {
                Some(name) => format!("Snapshot « {name} » créée, puis serveur {server} supprimé définitivement."),
                None => format!("Serveur {server} supprimé définitivement (sans snapshot préalable)."),
            })
        }

        "list_schedules" => {
            let id = req_i64(&args, "server_id")?;
            // Les commandes planifiées sont du texte libre : même filtre que la console.
            json_pretty(&core.panel_schedules(id).await.map_err(es)?).map(|s| core.redact_ai(&s))
        }

        "list_players" => {
            let id = req_i64(&args, "server_id")?;
            let sort = match opt_str(&args, "sort", "play_time").as_str() {
                "last_seen" => crate::PlayerSort::LastSeen,
                "name" => crate::PlayerSort::Name,
                _ => crate::PlayerSort::PlayTime,
            };
            let query = crate::players::AiQuery {
                sort,
                search: opt_str(&args, "search", ""),
                active_days: opt_i64(&args, "active_days", 0).clamp(0, 3650),
                limit: opt_i64(&args, "limit", 25).clamp(1, 100) as usize,
            };
            let roster = core.player_roster(id, false).await.map_err(es)?;
            // Pseudos remplacés par un alias stable si l'anonymisation IA est active.
            let anonymize = core.get_privacy_config().redact_ai;
            let report = crate::players::ai_report(&roster, &query, crate::store::now_secs(), anonymize);
            json_pretty(&report).map(|s| core.redact_ai(&s))
        }

        "server_metrics" => {
            let id = req_i64(&args, "server_id")?;
            let since = opt_i64(&args, "since_secs", 3600).clamp(60, 30 * 24 * 3600);
            // Résumé sur la pleine résolution, échantillons regroupés (maximum par tranche).
            let full = core.merged_history(id, since).await.map_err(es)?;
            let summary = summarize(&full);
            let shown = crate::history::limit_points(full, 120);
            json_pretty(&json!({
                "since_secs": since,
                "coverage": {
                    "local_s": shown.local_s,
                    "host_s": shown.host_s,
                    "host_available": shown.host_ok,
                },
                "summary": summary,
                "step_s": shown.step_s,
                "samples": shown.points,
                "players": shown.players,
            }))
        }

        "power_action" => {
            let id = req_i64(&args, "server_id")?;
            let action = req_str(&args, "action")?;
            core.power_action(id, &action).await.map_err(es)?;
            Ok(format!("Action « {action} » envoyée au serveur {id}."))
        }

        "send_command" => {
            let id = req_i64(&args, "server_id")?;
            let command = req_str(&args, "command")?;
            core.send_command(id, &command).await.map_err(es)?;
            Ok("Commande envoyée.".into())
        }

        "read_console" => {
            let id = req_i64(&args, "server_id")?;
            let logs = core.console_logs(id).await.map_err(es)?;
            // Anonymise avant de renvoyer à l'IA (mdp de commandes, IP, e-mails) si activé.
            Ok(core.redact_ai(&logs.join("\n")))
        }

        "player_action" => {
            let id = req_i64(&args, "server_id")?;
            let action = req_str(&args, "action")?;
            let player = req_str(&args, "player")?;
            core.player_action(id, &action, &player).await.map_err(es)?;
            Ok(format!("« {action} » appliqué à {player}."))
        }

        "list_files" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            json_pretty(&core.sftp_list(id, &path).await.map_err(es)?)
        }

        "read_file" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            // Anonymisation en sortie : un fichier de config peut contenir des secrets (rcon.password,
            // tokens DB/Discord…) — ils ne doivent pas partir en clair vers le LLM.
            Ok(core.redact_ai(&core.sftp_read_text(id, &path).await.map_err(es)?))
        }

        "read_gz" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            // Log/texte gzippé (ex. rotation `latest.log.gz`) → décompressé + anonymisé.
            Ok(core.redact_ai(&core.sftp_gz_text(id, &path).await.map_err(es)?))
        }

        "list_archive" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            json_pretty(&core.sftp_archive_list(id, &path).await.map_err(es)?)
        }

        "read_archive_entry" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let entry = req_str(&args, "entry")?;
            Ok(core.redact_ai(&core.sftp_archive_read_text(id, &path, &entry).await.map_err(es)?))
        }

        "search_files" => {
            let id = req_i64(&args, "server_id")?;
            let root = opt_str(&args, "root", "/");
            let query = req_str(&args, "query")?;
            let (hits, truncated) = core.sftp_search(id, &root, &query).await.map_err(es)?;
            let mut out = format!(
                "{} résultat(s) pour « {query} » sous « {root} »{} :",
                hits.len(),
                if truncated { " (tronqué — affine la recherche)" } else { "" }
            );
            for e in &hits {
                let _ = write!(out, "\n{}{}", e.path, if e.is_dir { "/" } else { "" });
            }
            Ok(out)
        }

        "extract_archive" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let dest = req_str(&args, "dest_dir")?;
            let out = core.extract_archive(id, &path, &dest).await.map_err(es)?;
            Ok(match (out.on_server, out.files) {
                (true, _) if out.skipped > 0 => format!(
                    "Archive extraite par le serveur dans « {} » ; {} fichier(s) ignoré(s).",
                    out.destination, out.skipped
                ),
                (true, _) => format!("Archive extraite par le serveur dans « {} ».", out.destination),
                (false, n) => format!(
                    "{} fichier(s) extrait(s) dans « {} » (extraction par l'appareil : le serveur n'a pas pu le faire).",
                    n.unwrap_or(0),
                    out.destination
                ),
            })
        }

        "compress_files" => {
            let id = req_i64(&args, "server_id")?;
            let paths = req_str_array(&args, "paths")?;
            let name = args.get("name").and_then(|v| v.as_str()).map(str::to_string);
            // Identifiant unique : deux compressions simultanées ne partagent pas leur annulation.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let tid = format!("mcp-compress-{nanos}");
            let out = core.server_compress(id, paths, name, &tid).await.map_err(es)?;
            Ok(if out.still_running {
                format!(
                    "Compression toujours en cours sur le serveur après 30 min : « {} » sera complète à la fin (vérifie avec list_files).",
                    out.archive_path
                )
            } else {
                let skipped = if out.skipped > 0 {
                    format!(" ; {} fichier(s) illisible(s) ignoré(s) (archive partielle)", out.skipped)
                } else {
                    String::new()
                };
                format!(
                    "Archive « {} » créée sur le serveur : {} fichier(s), {}{skipped}.",
                    out.archive_path,
                    out.file_count,
                    crate::server_files::human_bytes(out.size)
                )
            })
        }

        "disk_usage" => {
            let id = req_i64(&args, "server_id")?;
            let path = opt_str(&args, "path", "/");
            let limit = opt_i64(&args, "limit", 20).clamp(1, 100) as u32;
            let usage = core.disk_usage(id, &path, limit).await.map_err(es)?;
            // À la racine : occupation face aux limites (serveur et MyBox), si disponible.
            let space = if crate::server_files::panel_path(&path).is_empty() {
                core.disk_space(id).await.ok()
            } else {
                None
            };
            Ok(crate::server_files::disk_usage_report(&usage, space.as_ref()))
        }

        "write_file" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let content = req_str(&args, "content")?;
            core.sftp_write_text(id, &path, &content).await.map_err(es)?;
            Ok(format!("Fichier « {path} » enregistré."))
        }

        "create_dir" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            core.sftp_mkdir(id, &path).await.map_err(es)?;
            Ok(format!("Dossier « {path} » créé."))
        }

        "delete_path" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let is_dir = args.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false);
            core.sftp_delete(id, &path, is_dir).await.map_err(es)?;
            Ok(format!("« {path} » supprimé."))
        }

        "rename_path" => {
            let id = req_i64(&args, "server_id")?;
            let from = req_str(&args, "from")?;
            let to = req_str(&args, "to")?;
            core.sftp_rename(id, &from, &to).await.map_err(es)?;
            Ok(format!("« {from} » → « {to} »."))
        }

        "upload_file" => {
            let id = req_i64(&args, "server_id")?;
            let local_path = req_str(&args, "local_path")?;
            let remote_path = req_str(&args, "remote_path")?;
            let n = core.sftp_put_local(id, &local_path, &remote_path).await.map_err(es)?;
            Ok(format!("{n} octet(s) téléversé(s) vers « {remote_path} »."))
        }

        "download_file" => {
            let id = req_i64(&args, "server_id")?;
            let remote_path = req_str(&args, "remote_path")?;
            let local_path = req_str(&args, "local_path")?;
            let n = core.sftp_get_local(id, &remote_path, &local_path).await.map_err(es)?;
            Ok(format!("{n} octet(s) téléchargé(s) vers « {local_path} »."))
        }

        "edit_file" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let find = req_str(&args, "find")?;
            let replace = req_str(&args, "replace")?;
            let n = core.sftp_edit_text(id, &path, &find, &replace).await.map_err(es)?;
            Ok(format!("{n} remplacement(s) effectué(s) dans « {path} »."))
        }

        "file_stat" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let (is_dir, size, sha256) = core.sftp_stat_info(id, &path).await.map_err(es)?;
            json_pretty(&json!({ "path": path, "is_dir": is_dir, "size": size, "sha256": sha256 }))
        }

        "read_startup" => {
            let id = req_i64(&args, "server_id")?;
            json_pretty(&core.get_startup(id).await.map_err(es)?)
        }

        "set_startup_params" => {
            let id = req_i64(&args, "server_id")?;
            let parameters = req_str(&args, "parameters")?;
            core.set_startup_params(id, &parameters).await.map_err(es)?;
            Ok("Commande de démarrage mise à jour (effet au prochain démarrage).".into())
        }

        "list_installed_mods" => {
            let id = req_i64(&args, "server_id")?;
            let query = opt_str(&args, "query", "");
            Ok(format_installed(&core.installed_mods(id).await.map_err(es)?, &query, "mod"))
        }

        "list_installed_plugins" => {
            let id = req_i64(&args, "server_id")?;
            let query = opt_str(&args, "query", "");
            Ok(format_installed(&core.installed_plugins(id).await.map_err(es)?, &query, "plugin"))
        }

        "check_updates" => {
            let id = req_i64(&args, "server_id")?;
            let kind = opt_kind(&args)?;
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let updates = core.content_updates(id, force).await.map_err(es)?;
            Ok(crate::updates::format_check(&updates, kind))
        }

        "update_mods" => {
            let id = req_i64(&args, "server_id")?;
            let kind = opt_kind(&args)?;
            let keys = str_list(&args, "keys");
            let report = if keys.is_empty() {
                let kinds: Vec<ContentKind> = kind.into_iter().collect();
                core.apply_all_content_updates(id, &kinds).await.map_err(es)?
            } else {
                let current = core.content_updates(id, false).await.map_err(es)?;
                let items = resolve_update_keys(&current, kind, &keys)?;
                core.apply_content_updates(id, items).await.map_err(es)?
            };
            Ok(crate::updates::format_report(&report))
        }

        "list_backups" => {
            let id = req_i64(&args, "server_id")?;
            json_pretty(&core.list_backups(id).await.map_err(es)?)
        }

        "list_snapshots" => json_pretty(&core.list_snapshots().await.map_err(es)?),

        "create_snapshot" => {
            let id = req_i64(&args, "server_id")?;
            let name = opt_str(&args, "name", "Filet de sécurité");
            let job = core.create_snapshot(id, &name).await.map_err(es)?;
            Ok(format!(
                "Snapshot « {name} » lancé (job #{job}) ; il apparaîtra dans list_snapshots une fois prêt."
            ))
        }

        // Restaurations / suppression — DESTRUCTIVES. Hors allowlist agent ; atteintes uniquement via
        // « Appliquer » (copilot_apply → dispatch), soit un intent utilisateur explicite.
        "restore_snapshot" => {
            let snapshot_id = req_i64(&args, "snapshot_id")?;
            let server = req_i64(&args, "server_id")?;
            let job = core.restore_snapshot(snapshot_id, server).await.map_err(es)?;
            Ok(format!(
                "Restauration du snapshot #{snapshot_id} lancée sur le serveur {server} (job #{job}) — l'état actuel va être écrasé."
            ))
        }

        "restore_backup" => {
            let server = req_i64(&args, "server_id")?;
            let backup_id = req_i64(&args, "backup_id")?;
            core.restore_backup(server, backup_id).await.map_err(es)?;
            Ok(format!(
                "Restauration du backup #{backup_id} lancée sur le serveur {server} — l'état actuel va être écrasé."
            ))
        }

        "delete_snapshot" => {
            let snapshot_id = req_i64(&args, "snapshot_id")?;
            let job = core.delete_snapshot(snapshot_id).await.map_err(es)?;
            Ok(format!("Snapshot #{snapshot_id} supprimé (job #{job})."))
        }

        "market_search" => {
            let kind = opt_str(&args, "kind", "mods");
            let source = opt_str(&args, "source", "modrinth");
            let query = opt_str(&args, "query", "");
            let loader = opt_str(&args, "loader", "");
            let game_version = opt_str(&args, "game_version", "");
            let page = opt_i64(&args, "page", 1).max(1);
            let page = core
                .market_list(&kind, &source, page, &query, &loader, &game_version)
                .await
                .map_err(es)?;
            json_pretty(&page)
        }

        "list_mod_versions" => {
            let source = opt_str(&args, "source", "modrinth");
            let slug_or_id = req_str(&args, "slug")?;
            let loader = opt_str(&args, "loader", "");
            let game_version = opt_str(&args, "game_version", "");
            let versions = core
                .market_versions(&source, &slug_or_id, &loader, &game_version)
                .await
                .map_err(es)?;
            json_pretty(&versions)
        }

        "analyze_performance" => {
            let id = req_i64(&args, "server_id")?;
            let with_profiler = args
                .get("with_profiler")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Rapport Spark natif PARSÉ (health/tps/gc + points chauds profiler si demandé).
            Ok(core.redact_ai(&crate::perf::collect_spark(core, "", id, with_profiler).await))
        }

        "parse_spark_report" => {
            let url = req_str(&args, "url")?;
            crate::perf::parse_report_url(&url).await
        }

        "diagnose_startup" => {
            let id = req_i64(&args, "server_id")?;
            Ok(crate::doctor::diagnose_startup(core, id).await)
        }

        "inspect_region" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            crate::world::inspect_region(core, id, &path).await.map(|s| core.redact_ai(&s))
        }

        // DESTRUCTIF (efface/supprime des chunks) — hors allowlist agent, atteint uniquement via
        // « Appliquer » (action « danger »), snapshot recommandé au préalable.
        "repair_region" => {
            let id = req_i64(&args, "server_id")?;
            let path = req_str(&args, "path")?;
            let mode = opt_str(&args, "mode", "clear_corrupt");
            crate::world::repair_region(core, id, &path, &mode).await
        }

        "install_mod" => {
            let id = req_i64(&args, "server_id")?;
            let source = opt_str(&args, "source", "modrinth");
            let kind = opt_str(&args, "kind", "mod");
            let slug = req_str(&args, "slug")?;
            let version_id = req_str(&args, "version_id")?;
            // Optionnel : ignoré par Modrinth, inutile pour Spigot (identifiants numériques).
            let loader = opt_str(&args, "loader", "");
            core.install_mod(id, &source, &kind, &slug, &version_id, &loader)
                .await
                .map_err(|e| {
                    let m = es(e);
                    if m.contains("Accès refusé") {
                        format!("{m} — projet probablement premium/payant ou en téléchargement externe (non installable via MineStrator) ; choisis une alternative gratuite et non-externe (vérifie `premium`/`external` via market_search).")
                    } else {
                        m
                    }
                })?;
            Ok(format!("« {slug} » ({version_id}) installé sur le serveur {id}."))
        }

        "ficsit_search" => {
            let query = opt_str(&args, "query", "");
            let order_by = opt_str(&args, "order_by", "popularity");
            let page = opt_i64(&args, "page", 1).max(1);
            let limit = 20;
            let res = core
                .ficsit_search(&query, (page - 1) * limit, limit, &order_by, "desc")
                .await
                .map_err(es)?;
            json_pretty(&res)
        }

        "ficsit_versions" => {
            let mod_id = req_str(&args, "mod_id")?;
            let versions = core.ficsit_mod_versions(&mod_id).await.map_err(es)?;
            json_pretty(&versions)
        }

        "install_ficsit_mod" => {
            let id = req_i64(&args, "server_id")?;
            let reference = req_str(&args, "reference")?;
            let version_id = req_str(&args, "version_id")?;
            core.ficsit_install_inner(id, &[(reference.clone(), version_id.clone())], "mcp")
                .await
                .map_err(es)?;
            Ok(format!(
                "« {reference} » ({version_id}) installé sur le serveur {id} (mod + SML + dépendances) ; le serveur a été redémarré."
            ))
        }

        // --- Tâches planifiées du panel (exécutées par l'hébergeur) --------------------
        "create_schedule" => {
            let id = req_i64(&args, "server_id")?;
            let input = crate::PanelScheduleInput {
                name: req_str(&args, "name")?,
                cron: parse_cron(&req_str(&args, "cron")?)?,
                is_active: args.get("is_active").and_then(Value::as_bool).unwrap_or(true),
                only_when_online: args.get("only_when_online").and_then(Value::as_bool).unwrap_or(false),
            };
            // Actions vérifiées AVANT de créer quoi que ce soit : une saisie fausse ne laisse pas
            // une tâche planifiée vide derrière elle.
            let tasks = match args.get("tasks") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|t| schedule_task_input(t, None).and_then(|i| i.validated().map_err(es)))
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => return Err("`tasks` doit être un tableau d'actions".into()),
            };
            let name = input.name.trim().to_string();
            core.create_panel_schedule(id, input).await.map_err(es)?;
            if tasks.is_empty() {
                return Ok(format!(
                    "Tâche planifiée « {name} » créée sur le serveur {id}, sans action : ajoute-en avec add_schedule_task (id via list_schedules)."
                ));
            }
            // L'API ne renvoie pas l'id créé : la plus récente portant ce nom.
            let created = core
                .panel_schedules(id)
                .await
                .map_err(es)?
                .into_iter()
                .filter(|s| s.name.trim() == name)
                .map(|s| s.id)
                .max()
                .ok_or("tâche créée mais introuvable : ajoute ses actions avec add_schedule_task")?;
            let total = tasks.len();
            for (done, task) in tasks.into_iter().enumerate() {
                core.add_panel_task(id, created, task).await.map_err(|e| {
                    format!("tâche « {name} » (id {created}) créée, {done}/{total} action(s) ajoutée(s) : {e}")
                })?;
            }
            Ok(format!("Tâche planifiée « {name} » (id {created}) créée sur le serveur {id} avec {total} action(s)."))
        }

        "update_schedule" => {
            let id = req_i64(&args, "server_id")?;
            let schedule = find_schedule(core, id, req_i64(&args, "schedule_id")?).await?;
            let input = crate::PanelScheduleInput {
                name: args.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or(schedule.name),
                cron: match args.get("cron").and_then(Value::as_str) {
                    Some(c) => parse_cron(c)?,
                    None => schedule.cron,
                },
                is_active: args.get("is_active").and_then(Value::as_bool).unwrap_or(schedule.is_active),
                only_when_online: args
                    .get("only_when_online")
                    .and_then(Value::as_bool)
                    .unwrap_or(schedule.only_when_online),
            };
            core.update_panel_schedule(id, schedule.id, input).await.map_err(es)?;
            Ok(format!("Tâche planifiée {} du serveur {id} modifiée.", schedule.id))
        }

        "delete_schedule" => {
            let id = req_i64(&args, "server_id")?;
            let schedule = find_schedule(core, id, req_i64(&args, "schedule_id")?).await?;
            let confirm = req_str(&args, "confirm_name")?;
            if schedule.name.trim().is_empty() || confirm.trim() != schedule.name.trim() {
                return Err(
                    "Suppression refusée : `confirm_name` doit reprendre exactement le nom de la tâche planifiée (casse comprise), donné par l'utilisateur."
                        .into(),
                );
            }
            core.delete_panel_schedule(id, schedule.id).await.map_err(es)?;
            Ok(format!("Tâche planifiée {} supprimée du serveur {id}, avec ses actions.", schedule.id))
        }

        "add_schedule_task" => {
            let id = req_i64(&args, "server_id")?;
            let schedule_id = req_i64(&args, "schedule_id")?;
            core.add_panel_task(id, schedule_id, schedule_task_input(&args, None)?).await.map_err(es)?;
            Ok(format!("Action ajoutée à la fin de la tâche planifiée {schedule_id}."))
        }

        "update_schedule_task" => {
            let id = req_i64(&args, "server_id")?;
            let schedule = find_schedule(core, id, req_i64(&args, "schedule_id")?).await?;
            let task_id = req_i64(&args, "task_id")?;
            let current = schedule.tasks.iter().find(|t| t.id == task_id).ok_or_else(|| {
                format!("action {task_id} introuvable dans la tâche planifiée {} (voir list_schedules)", schedule.id)
            })?;
            let input = schedule_task_input(&args, Some(current))?;
            core.update_panel_task(id, schedule.id, task_id, input).await.map_err(es)?;
            Ok(format!("Action {task_id} de la tâche planifiée {} modifiée.", schedule.id))
        }

        "delete_schedule_task" => {
            let id = req_i64(&args, "server_id")?;
            let (schedule_id, task_id) = (req_i64(&args, "schedule_id")?, req_i64(&args, "task_id")?);
            core.delete_panel_task(id, schedule_id, task_id).await.map_err(es)?;
            Ok(format!("Action {task_id} retirée de la tâche planifiée {schedule_id}."))
        }

        "reorder_schedule_tasks" => {
            let id = req_i64(&args, "server_id")?;
            let schedule = find_schedule(core, id, req_i64(&args, "schedule_id")?).await?;
            let ids: Vec<i64> = args
                .get("task_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .ok_or("paramètre `task_ids` (tableau d'entiers) requis")?;
            check_task_order(&schedule, &ids)?;
            core.reorder_panel_tasks(id, schedule.id, ids).await.map_err(es)?;
            Ok(format!("Actions de la tâche planifiée {} réordonnées.", schedule.id))
        }
        // --- Plusieurs serveurs (fleet.rs) : validation et rendu dans le module ---------
        "copy_between_servers" => crate::fleet::copy_tool(core, &args).await.map_err(es),
        "diff_files" => crate::fleet::diff_tool(core, &args).await.map_err(es),
        "find_player" => crate::fleet::find_player_tool(core, &args).await.map_err(es),
        "broadcast_command" => crate::fleet::broadcast_tool(core, &args).await.map_err(es),

        _ => Err(format!("outil inconnu : {name}")),
    }
}

// --- Tâches planifiées du panel : saisie des outils -----------------------------

/// Expression cron en cinq champs (« minute heure jour-du-mois mois jour-de-semaine »). Les
/// valeurs sont vérifiées ensuite par `PanelScheduleInput::validated`.
fn parse_cron(cron: &str) -> Result<crate::PanelCron, String> {
    let f: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, day_of_month, month, day_of_week] = f.as_slice() else {
        return Err(format!(
            "`cron` : 5 champs attendus (minute heure jour-du-mois mois jour-de-semaine), ex. « 0 4 * * * » ; reçu « {cron} »"
        ));
    };
    Ok(crate::PanelCron {
        minute: minute.to_string(),
        hour: hour.to_string(),
        day_of_month: day_of_month.to_string(),
        month: month.to_string(),
        day_of_week: day_of_week.to_string(),
    })
}

/// Saisie d'une action : `action` (`command` | `power`), `payload`, `time_offset` (0 à 900 s),
/// `continue_on_failure`. Un champ absent reprend la valeur de `current` (modification), sinon
/// le défaut (délai 0, arrêt sur échec). Vérifiée par le cœur avant envoi.
fn schedule_task_input(v: &Value, current: Option<&crate::PanelTask>) -> Result<crate::PanelTaskInput, String> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let action = text("action")
        .or_else(|| current.map(|c| c.action.clone()))
        .ok_or("paramètre `action` (command ou power) requis")?;
    let payload = text("payload")
        .or_else(|| current.map(|c| c.payload.clone()))
        .ok_or("paramètre `payload` requis (commande, ou start|restart|stop|kill)")?;
    Ok(crate::PanelTaskInput {
        action: action.trim().to_string(),
        payload,
        time_offset: v
            .get("time_offset")
            .and_then(Value::as_i64)
            .or(current.map(|c| c.time_offset))
            .unwrap_or(0),
        continue_on_failure: v
            .get("continue_on_failure")
            .and_then(Value::as_bool)
            .or(current.map(|c| c.continue_on_failure))
            .unwrap_or(false),
    })
}

/// Tâche planifiée `schedule_id` du serveur, lue à la source.
async fn find_schedule(core: &Core, server_id: i64, schedule_id: i64) -> Result<crate::PanelSchedule, String> {
    core.panel_schedules(server_id)
        .await
        .map_err(es)?
        .into_iter()
        .find(|s| s.id == schedule_id)
        .ok_or_else(|| format!("tâche planifiée {schedule_id} introuvable sur le serveur {server_id} (voir list_schedules)"))
}

/// Un réordonnancement doit citer chaque action de la tâche exactement une fois.
fn check_task_order(schedule: &crate::PanelSchedule, ids: &[i64]) -> Result<(), String> {
    let mut want: Vec<i64> = schedule.tasks.iter().map(|t| t.id).collect();
    let mut got = ids.to_vec();
    want.sort_unstable();
    got.sort_unstable();
    if want != got {
        return Err(format!(
            "`task_ids` doit citer une fois chaque action de la tâche planifiée {} : {:?}",
            schedule.id, want
        ));
    }
    Ok(())
}

// --- Liste des outils -----------------------------------------------------

/// Catalogue des outils MCP. Construit UNE fois (statique) puis prêté : réutilisé à chaque
/// `tools/list` et à chaque tour d'agent sans reconstruire le JSON.
pub fn tool_list() -> &'static Value {
    static TOOLS: LazyLock<Value> = LazyLock::new(build_tool_list);
    &TOOLS
}

fn build_tool_list() -> Value {
    json!([
        tool("list_servers", "Liste tous les serveurs : id, nom, statut, adresse, MyBox. À appeler en premier pour obtenir les `server_id`.", obj(&[], &[])),
        tool("server_status", "État temps réel d'un serveur : état, consommation instantanée (CPU en centièmes de cœur dont FlexCore/`is_bursting`, RAM et disque en Mo), réseau cumulé, uptime, joueurs, version, MOTD.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("list_schedules", "Tâches planifiées du panel MineStrator d'un serveur, exécutées par l'hébergeur même ordinateur éteint (à distinguer des automatisations de l'app) : nom, planification cron, active, seulement si en ligne, prochaine et dernière exécution, actions dans l'ordre (commande ou alimentation, délai en secondes).", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("list_players", "Joueurs Minecraft (Java) déjà venus sur un serveur, pas seulement les connectés : statistiques (joueurs uniques, connectés, actifs sur 24 h / 7 j / 30 j, temps de jeu total) puis liste triée avec temps de jeu CUMULÉ (heures), dernière venue, op/whitelist/banni. Pour « qui a le plus joué cette semaine » : sort=play_time et active_days=7 (le temps de jeu reste cumulé depuis la première venue). Pseudos remplacés par des alias stables si l'anonymisation IA est active.", obj(&[("server_id", prop_int("ID du serveur")), ("sort", prop_str("play_time (défaut) | last_seen | name")), ("active_days", prop_int("Seulement les joueurs venus dans les N derniers jours (0 = tous)")), ("search", prop_str("Filtre sur le pseudo (sous-chaîne)")), ("limit", prop_int("Joueurs listés (défaut 25, max 100)"))], &["server_id"])),
        tool("mybox_info", "Détail d'une MyBox (`mybox_id` via list_servers) : échéance, renouvellement automatique, jours avant suppression si expirée, CPU/RAM encore allouables, offre (bases de données, nombre maximal de serveurs actifs), serveurs et ressources allouées.", obj(&[("mybox_id", prop_int("ID de la MyBox"))], &["mybox_id"])),
        tool("mybox_capacity", "Capacité des MyBox (toutes, ou une seule avec `mybox_id`) — POINT D'ENTRÉE pour architecturer : pour chaque MyBox, CPU et RAM totaux / alloués / restants (CPU en CŒURS entiers, RAM en GO entiers), disque PARTAGÉ (total en Go, occupé en Mo), emplacements de serveurs actifs et en pause (`max_active` 0 = sans limite), FlexCore, état (expirée, suspendue, Pro), serveurs (id, nom, jeu, cœurs, Go, en pause, localisation, ordre) et `blockers` (ce qui empêche d'y créer un serveur). Seuls les serveurs ACTIFS consomment CPU/RAM.", obj(&[("mybox_id", prop_int("ID d'une MyBox (optionnel : toutes par défaut)"))], &[])),
        tool("server_catalog", "Catalogue de création : jeux (`egg_id`, catégorie, ressources RECOMMANDÉES en cœurs et Go, champs requis à fournir) et localisations (`location_id`, ville, en stock). `query` filtre par nom ; les types expérimentaux (non testés, sans support) ne sont inclus qu'avec `include_experimental`.", obj(&[("query", prop_str("Filtre optionnel sur le nom du jeu")), ("include_experimental", prop_bool("Inclure les types expérimentaux (défaut false)"))], &[])),
        tool("plan_placement", "Calcule OÙ placer un ou plusieurs serveurs à créer selon les ressources LIBRES de toutes les MyBox — calcul pur, NE MODIFIE RIEN. Chaque demande : `cpu` en cœurs entiers, `ram` en Go entiers, `disk_gb` facultatif (disque libre souhaité, Go), `label`. `strategy` : `pack` (défaut : remplir la MyBox la plus juste, garder de grands blocs libres ailleurs) ou `spread` (répartir la charge). Renvoie la MyBox retenue par demande (et ce qu'il y restera), les raisons d'un refus et des pistes (serveurs à mettre en pause). Pour réduire des serveurs surdimensionnés, voir mybox_smartscan.", obj(&[("servers", json!({ "type": "array", "description": "Serveurs à placer", "items": { "type": "object", "properties": { "label": { "type": "string", "description": "Libellé libre" }, "cpu": { "type": "integer", "description": "Cœurs (entier ≥ 1)" }, "ram": { "type": "integer", "description": "Go de RAM (entier ≥ 1)" }, "disk_gb": { "type": "integer", "description": "Disque libre souhaité en Go (optionnel)" } }, "required": ["cpu", "ram"] } })), ("strategy", prop_str("pack (défaut) ou spread"))], &["servers"])),
        tool("mybox_smartscan", "Analyse SmartScan de l'hébergeur pour une MyBox (usage p95 sur 14 à 30 jours) : serveurs surdimensionnés et allocation suggérée (`shrink`), rééquilibrage possible entre serveurs (`balance`), conseil d'évolution (`scale`). Lecture seule : appliquer une suggestion = mybox_resize_server, avec l'accord de l'utilisateur.", obj(&[("mybox_id", prop_int("ID de la MyBox"))], &["mybox_id"])),
        tool("mybox_create_server", "CRÉE un serveur dans une MyBox. Vérifie AVANT envoi : ressources restantes de CETTE MyBox, emplacement actif libre, jeu du catalogue, champs requis, localisation en stock. `cpu` en CŒURS entiers (1 = un cœur, pas un %), `ram` en GO entiers (4 = 4 Go, pas 4096). `egg_id` et `location_id` viennent de server_catalog ; `mybox_id` de mybox_capacity (ou plan_placement). `location_id` est ignoré sur une MyBox Pro. Peut consommer toute la capacité restante : confirme le plan avec l'utilisateur d'abord. ✎ (modifiant)", obj(&[("mybox_id", prop_int("ID de la MyBox d'accueil")), ("name", prop_str("Nom du serveur (1 à 50 caractères)")), ("egg_id", prop_int("ID du jeu (server_catalog)")), ("experimental", prop_bool("true si egg_id vient du catalogue expérimental (défaut false)")), ("location_id", prop_int("ID de localisation (server_catalog) ; requis sauf MyBox Pro")), ("cpu", prop_int("Cœurs alloués (entier ≥ 1)")), ("ram", prop_int("Go de RAM alloués (entier ≥ 1)")), ("required_fields", json!({ "type": "object", "description": "Champs requis du jeu : { CLÉ: valeur } (clés de server_catalog)", "additionalProperties": { "type": "string" } }))], &["mybox_id", "name", "egg_id", "cpu", "ram"])),
        tool("mybox_resize_server", "Change le CPU et la RAM ALLOUÉS à un serveur (valeurs FINALES, pas des différences) : `cpu` en cœurs entiers, `ram` en Go entiers. Vérifie que la MyBox a de quoi (le serveur rend d'abord ce qu'il occupe). L'API peut exiger le serveur ARRÊTÉ (erreur API_MYBOX_NEED_OFFLINE_SERVER) : demander à l'utilisateur avant de l'arrêter. ✎ (modifiant)", obj(&[("mybox_id", prop_int("ID de la MyBox du serveur")), ("server_id", prop_int("ID du serveur")), ("cpu", prop_int("Nouveau total de cœurs (entier ≥ 1)")), ("ram", prop_int("Nouveau total de Go de RAM (entier ≥ 1)"))], &["mybox_id", "server_id", "cpu", "ram"])),
        tool("rename_server", "Renomme un serveur (1 à 50 caractères). ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("name", prop_str("Nouveau nom"))], &["server_id", "name"])),
        tool("rename_mybox", "Renomme une MyBox (1 à 50 caractères). ✎ (modifiant)", obj(&[("mybox_id", prop_int("ID de la MyBox")), ("name", prop_str("Nouveau nom"))], &["mybox_id", "name"])),
        tool("mybox_pause_server", "Met en PAUSE (`paused`: true) ou REPREND (`paused`: false) un serveur. En pause : arrêté, CPU/RAM rendus à la MyBox, fichiers conservés. Reprendre exige un emplacement actif libre et ses ressources disponibles ; la reprise peut durer quelques minutes (fichiers remis en place). ✎ (modifiant)", obj(&[("mybox_id", prop_int("ID de la MyBox du serveur")), ("server_id", prop_int("ID du serveur")), ("paused", prop_bool("true = mettre en pause, false = reprendre"))], &["mybox_id", "server_id", "paused"])),
        tool("mybox_reorder_servers", "Réordonne les serveurs d'une MyBox (affichage du panel) : `server_ids` dans l'ordre voulu ; ceux non cités suivent dans leur ordre actuel. ✎ (modifiant)", obj(&[("mybox_id", prop_int("ID de la MyBox")), ("server_ids", json!({ "type": "array", "description": "IDs des serveurs, dans l'ordre", "items": { "type": "integer" } }))], &["mybox_id", "server_ids"])),
        tool("mybox_delete_server", "SUPPRIME DÉFINITIVEMENT un serveur (fichiers, mondes et sauvegardes automatiques perdus). `confirm_name` doit reprendre EXACTEMENT le nom actuel du serveur (casse comprise), sinon refus. Par défaut (`snapshot_first` true), une snapshot est d'abord créée et attendue (jusqu'à 15 min) : la suppression est annulée si elle échoue. N'utiliser que sur demande EXPLICITE de l'utilisateur. ✎ (modifiant, irréversible)", obj(&[("mybox_id", prop_int("ID de la MyBox du serveur")), ("server_id", prop_int("ID du serveur à supprimer")), ("confirm_name", prop_str("Nom EXACT du serveur, en confirmation")), ("snapshot_first", prop_bool("Snapshot préalable (défaut true)"))], &["mybox_id", "server_id", "confirm_name"])),
        tool("server_metrics", "Historique de consommation (CPU en % d'un cœur, RAM et disque en octets) + résumé (moyennes/max, pic de joueurs) — pour analyser la charge et optimiser. Croise la surveillance de l'app (`source: local`, toutes les 10 s) et les statistiques de l'hébergeur (`source: host`, à la minute) qui comblent les périodes non surveillées ; `players` : joueurs connectés (hébergeur).", obj(&[("server_id", prop_int("ID du serveur")), ("since_secs", prop_int("Fenêtre en secondes (défaut 3600, max 30 j)"))], &["server_id"])),
        tool("power_action", "Démarre/redémarre/arrête un serveur. `action` ∈ start|restart|restart10|stop|stop10|kill.", obj(&[("server_id", prop_int("ID du serveur")), ("action", prop_str("start|restart|restart10|stop|stop10|kill"))], &["server_id", "action"])),
        tool("send_command", "Envoie une commande à la console (n'a d'effet que serveur en ligne).", obj(&[("server_id", prop_int("ID du serveur")), ("command", prop_str("Commande console"))], &["server_id", "command"])),
        tool("read_console", "100 dernières lignes de console — pour diagnostiquer un crash.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("player_action", "Modération Minecraft. `action` ∈ kick|ban|unban|op_add|op_remove|whitelist_add|whitelist_remove.", obj(&[("server_id", prop_int("ID du serveur")), ("action", prop_str("kick|ban|unban|op_add|op_remove|whitelist_add|whitelist_remove")), ("player", prop_str("Pseudo exact"))], &["server_id", "action", "player"])),
        tool("list_files", "Liste un répertoire via SFTP (racine = `/`).", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant"))], &["server_id", "path"])),
        tool("read_file", "Lit un fichier texte via SFTP (refuse binaires / > 2 Mo).", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant"))], &["server_id", "path"])),
        tool("read_gz", "Lit un fichier texte GZIPPÉ via SFTP en le décompressant (ex. log tourné `latest.log.gz`, `crash-report.gz`) — indispensable pour diagnostiquer sur des logs archivés. Complète read_file, qui refuse les binaires.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant du .gz"))], &["server_id", "path"])),
        tool("list_archive", "Liste les entrées d'une archive distante (`.zip` / `.tar` / `.tar.gz`) SANS l'extraire — pour repérer un fichier à inspecter dans un backup, un modpack, etc.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant de l'archive"))], &["server_id", "path"])),
        tool("read_archive_entry", "Lit le contenu TEXTE d'UNE entrée d'archive (`.zip`/`.tar`/`.tar.gz`) sans l'extraire sur disque. `entry` = nom exact renvoyé par list_archive.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin de l'archive")), ("entry", prop_str("Nom de l'entrée dans l'archive"))], &["server_id", "path", "entry"])),
        tool("search_files", "Recherche RÉCURSIVE des fichiers/dossiers dont le nom contient `query` (insensible à la casse) sous `root` (défaut `/`). Renvoie les chemins (dossiers suffixés `/`). Résultats et exploration plafonnés (« tronqué » → affine `query`/`root`). Idéal pour localiser une config sans connaître le chemin exact.", obj(&[("server_id", prop_int("ID du serveur")), ("query", prop_str("Sous-chaîne du nom à chercher")), ("root", prop_str("Dossier de départ (défaut /)"))], &["server_id", "query"])),
        tool("extract_archive", "Extrait une archive présente sur le serveur DANS `dest_dir` — p. ex. installer un modpack/monde/backup en place. Extraction faite PAR LE SERVEUR (rien ne transite par la machine ; formats .zip, .jar, .tar, .tar.gz/.bz2/.xz/.zst, .7z, .rar, .mcworld/.mcpack), avec repli automatique par SFTP pour .zip/.tar/.tar.gz si l'API échoue (plafonné, anti zip-slip). Les fichiers existants du même nom sont écrasés : vérifie la destination (list_files) avant. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin de l'archive sur le serveur")), ("dest_dir", prop_str("Dossier cible d'extraction (`/` = racine du serveur)"))], &["server_id", "path", "dest_dir"])),
        tool("compress_files", "Compresse des fichiers/dossiers EN .zip SUR LE SERVEUR (rapide même pour un monde de plusieurs Go : rien ne transite par la machine) — sauvegarde ponctuelle d'un monde ou d'une config AVANT une intervention risquée, ou archive à télécharger ensuite. Tous les `paths` doivent être dans le MÊME dossier : l'archive y est créée. Attend la fin (jusqu'à ~30 min) et renvoie son chemin, sa taille et le nombre de fichiers. Occupe de l'espace disque (vérifie avec disk_usage si le serveur est presque plein). N'écrase jamais une archive existante. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("paths", prop_str_array("Chemins absolus à inclure, tous dans le même dossier (ex. [\"/world\", \"/world_nether\"])")), ("name", prop_str("Nom de l'archive (optionnel ; `.zip` ajouté ; défaut : <nom>_<date>.zip)"))], &["server_id", "paths"])),
        tool("disk_usage", "Analyse de l'ESPACE DISQUE calculée par le serveur : ce qui occupe le plus de place dans un dossier (tailles récursives, du plus gros au plus petit, avec leur part en %). À la racine, ajoute l'occupation comptée face à la limite du serveur et à celle de la MyBox. Idéal pour un serveur plein (« disk full ») : descends dans les plus gros dossiers (`path`) pour trouver logs, backups, mondes ou dumps à nettoyer.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Dossier à analyser (défaut `/`)")), ("limit", prop_int("Nombre d'entrées (défaut 20, max 100)"))], &["server_id"])),
        tool("write_file", "Écrit/écrase un fichier de config via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant")), ("content", prop_str("Nouveau contenu"))], &["server_id", "path", "content"])),
        tool("create_dir", "Crée un dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin du nouveau dossier"))], &["server_id", "path"])),
        tool("delete_path", "Supprime un fichier ou un dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin à supprimer")), ("is_dir", prop_bool("true si c'est un dossier"))], &["server_id", "path"])),
        tool("rename_path", "Renomme ou déplace un fichier/dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("from", prop_str("Chemin source")), ("to", prop_str("Chemin destination"))], &["server_id", "from", "to"])),
        tool("upload_file", "Téléverse un fichier LOCAL (BINAIRE, sans la limite 2 Mo de write_file) vers un chemin distant précis via SFTP — pour déployer un .jar, un resource pack, un monde, etc. `local_path` = chemin sur la machine qui exécute ce serveur MCP. Crée le dossier parent au besoin, écrase la cible. Vérifie ensuite avec file_stat. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("local_path", prop_str("Chemin du fichier LOCAL (machine hôte du MCP)")), ("remote_path", prop_str("Chemin distant complet, fichier inclus"))], &["server_id", "local_path", "remote_path"])),
        tool("download_file", "Télécharge un fichier distant (BINAIRE) vers un chemin LOCAL via SFTP — pour récupérer le .jar/config actuel et le comparer avant de pousser, ou faire un backup ciblé. `local_path` = chemin sur la machine qui exécute ce serveur MCP. Crée le dossier parent local au besoin.", obj(&[("server_id", prop_int("ID du serveur")), ("remote_path", prop_str("Chemin distant du fichier")), ("local_path", prop_str("Chemin LOCAL de destination (machine hôte du MCP)"))], &["server_id", "remote_path", "local_path"])),
        tool("edit_file", "Édition PARTIELLE d'un fichier texte distant : remplace toutes les occurrences EXACTES de `find` par `replace`. Ne renvoie jamais le contenu → SÛR même si le fichier contient des secrets (contrairement à read_file+write_file, qui réécrirait `[SECRET]` par-dessus le vrai secret). Erreur si `find` est absent (aucun remplacement). À préférer à write_file pour patcher une config sans la réécrire en entier. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant du fichier")), ("find", prop_str("Texte exact à trouver (respecte casse/espaces)")), ("replace", prop_str("Texte de remplacement"))], &["server_id", "path", "find", "replace"])),
        tool("file_stat", "Métadonnées d'un fichier distant : taille (octets) + empreinte sha256 — pour vérifier qu'un upload_file a réussi / que le fichier distant correspond au local. Le sha256 est vide pour un dossier.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant"))], &["server_id", "path"])),
        tool("read_startup", "Lit la commande de démarrage (flags JVM : -Xmx, GC, Aikar…), le JAR, la mémoire et l'image.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("read_properties", "Lit les réglages du serveur (server.properties : max-players, difficulty, pvp, view-distance, white-list…) via l'API, sans SFTP. Les valeurs secrètes (rcon.password…) sont masquées.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("set_property", "Modifie UN réglage EXISTANT de server.properties (clé exacte, telle que renvoyée par read_properties). Valeur vide = vide le réglage. Effet au prochain démarrage. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("property", prop_str("Clé exacte (ex. max-players)")), ("value", prop_str("Nouvelle valeur, en texte"))], &["server_id", "property", "value"])),
        tool("set_startup_params", "Modifie la commande de démarrage Java (optimiser les flags JVM). Garder `{{SERVER_JARFILE}}`. Effet au prochain démarrage. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("parameters", prop_str("Commande Java COMPLÈTE"))], &["server_id", "parameters"])),
        tool("list_installed_mods", "Mods installés (nom + version, format compact ; loaders résumés en en-tête). Sur un gros modpack, utilise `query` pour filtrer par sous-chaîne de nom (ex. « est-ce que Sodium est installé ? ») plutôt que de tout lister.", obj(&[("server_id", prop_int("ID du serveur")), ("query", prop_str("Filtre optionnel : sous-chaîne du nom (insensible à la casse)"))], &["server_id"])),
        tool("list_installed_plugins", "Plugins installés (nom + version, format compact). `query` filtre par sous-chaîne du nom (insensible à la casse).", obj(&[("server_id", prop_int("ID du serveur")), ("query", prop_str("Filtre optionnel : sous-chaîne du nom"))], &["server_id"])),
        tool("list_backups", "Backups quotidiens automatiques d'un serveur (récents d'abord : id, taille en octets, date). Ils se restaurent (pas de création à la demande).", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("list_snapshots", "Snapshots de l'utilisateur : points de sauvegarde créés à la demande (id, nom, taille, date, statut). À consulter/créer comme filet AVANT une opération risquée.", obj(&[], &[])),
        tool("create_snapshot", "Crée un snapshot (point de sauvegarde à la demande) du serveur — le filet AVANT une intervention risquée. Additif, sans risque. Donne un `name` court et parlant.", obj(&[("server_id", prop_int("ID du serveur")), ("name", prop_str("Nom court du point de sauvegarde"))], &["server_id", "name"])),
        tool("market_search", "Cherche des mods/plugins dans le marketplace. `kind`=mods|plugins, `source`=modrinth|curseforge|spigot. `query` vide = populaires. Filtrer par `loader` (fabric/neoforge/forge/quilt/paper…) et `game_version` (ex. 1.21.1). Renvoie id/slug, name, downloads, loaders, game_versions.", obj(&[("kind", prop_str("mods|plugins (défaut mods)")), ("source", prop_str("modrinth|curseforge|spigot (défaut modrinth)")), ("query", prop_str("Recherche par nom (optionnel)")), ("loader", prop_str("Modloader (optionnel)")), ("game_version", prop_str("Version Minecraft (optionnel)")), ("page", prop_int("Page (défaut 1)"))], &[])),
        tool("list_mod_versions", "Versions disponibles d'un projet. `slug` = slug Modrinth ou id numérique CurseForge/SpigotMC. Filtrer par loader/game_version. Renvoie les `id` de version (nécessaires pour installer).", obj(&[("source", prop_str("modrinth|curseforge|spigot (défaut modrinth)")), ("slug", prop_str("Slug Modrinth ou id numérique")), ("loader", prop_str("Modloader (optionnel)")), ("game_version", prop_str("Version Minecraft (optionnel)"))], &["slug"])),
        tool("analyze_performance", "Analyse de PERFORMANCE via Spark : envoie `spark health/tps/gc` (+ profiler CPU 30 s si `with_profiler`) et renvoie un rapport PARSÉ (TPS, MSPT, GC, points chauds par méthode). À utiliser pour diagnostiquer lag / surcharge CPU. ⚠️ le profiler ajoute ~30 s et un léger à-coup serveur — ne pas répéter en boucle.", obj(&[("server_id", prop_int("ID du serveur")), ("with_profiler", prop_bool("Lancer aussi le profiler CPU 30 s (défaut false)"))], &["server_id"])),
        tool("parse_spark_report", "Télécharge et PARSE un rapport Spark EXISTANT depuis son URL (`https://spark.lucko.me/<clé>`) — profiler (points chauds CPU) OU heapsummary (plus gros consommateurs mémoire, en Mio). N'essaie JAMAIS d'ouvrir/fetcher cette URL avec un outil web (page JS + protobuf binaire) : utilise CET outil.", obj(&[("url", prop_str("URL du rapport Spark (spark.lucko.me/…)"))], &["url"])),
        tool("diagnose_startup", "Docteur démarrage : pour un serveur qui NE DÉMARRE PAS / crash-loop, rassemble en UN appel la commande de démarrage + la fin de logs/latest.log + le dernier crash-report, et pré-scanne les pannes connues (EULA, port occupé, OOM, version Java, dépendance/mixin de mod, monde corrompu). Utilise-le AVANT de proposer un correctif.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("inspect_region", "Inspecte un fichier de région Minecraft (`.mca`) et repère les chunks structurellement CORROMPUS (pointeurs/longueurs invalides — cause des crash au chargement de monde). Chemin ex. `/world/region/r.0.0.mca` (Nether : `/world/DIM-1/region/…`, End : `/world/DIM1/region/…`). Lecture seule ; croise avec les coordonnées du crash pour cibler la bonne région.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin absolu du fichier .mca"))], &["server_id", "path"])),
        tool("install_mod", "Installe un mod/plugin. `modrinth` = mods ET plugins (corps {slug, version_id}, PAS de loader) ; `spigot` = plugins (`slug` = id numérique de la ressource, `version_id` = id de version). Récupère `version_id` via list_mod_versions. N'installe PAS un projet premium/external (non installable → 403 ; vérifie via market_search). ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("source", prop_str("modrinth|spigot (défaut modrinth)")), ("kind", prop_str("mod|plugin (défaut mod)")), ("slug", prop_str("Slug Modrinth ou id numérique SpigotMC")), ("version_id", prop_str("ID de la version à installer")), ("loader", prop_str("Optionnel : modloader (ignoré par Modrinth et Spigot)"))], &["server_id", "slug", "version_id"])),
        tool("check_updates", "Mises à jour disponibles des mods, plugins et datapacks installés (Minecraft) : version installée → nouvelle, d'après l'hébergeur (version de Minecraft et loader du serveur pris en compte). Lecture seule ; résultat gardé 15 min, `force` pour revérifier. Les propositions ÉCARTÉES (variante d'une autre plateforme, même version ou plus ancienne) ne doivent PAS être installées, ni via install_mod. Donne la clé de chaque élément (entre crochets) pour update_mods.", obj(&[("server_id", prop_int("ID du serveur")), ("kind", prop_str("mods|plugins|datapacks (défaut : tous)")), ("force", prop_bool("Refaire la vérification au lieu du résultat gardé (défaut false)"))], &["server_id"])),
        tool("update_mods", "Met à jour des mods/plugins/datapacks : installe la nouvelle version retenue par check_updates puis supprime l'ancien fichier (revérifie avant ; une proposition écartée n'est jamais appliquée). `keys` = clés renvoyées par check_updates ; vide = toutes les mises à jour des éléments ACTIFS (filtrées par `kind`). Effet au PROCHAIN démarrage : redémarre ensuite (power_action). Crée un snapshot avant (create_snapshot). ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("kind", prop_str("mods|plugins|datapacks (optionnel)")), ("keys", json!({ "type": "array", "items": { "type": "string" }, "description": "Clés (noms de fichier) renvoyées par check_updates ; vide = toutes" }))], &["server_id"])),
        tool("ficsit_search", "Cherche des mods SATISFACTORY dans le catalogue ficsit.app (SMR). `query` vide = parcours trié. `order_by` ∈ popularity|hotness|downloads|last_version_date|name (défaut popularity). Renvoie id, mod_reference, name, downloads. À réserver aux serveurs Satisfactory (pas Minecraft).", obj(&[("query", prop_str("Recherche par nom (optionnel, ≥ 3 car.)")), ("order_by", prop_str("Tri (défaut popularity)")), ("page", prop_int("Page (défaut 1)"))], &[])),
        tool("ficsit_versions", "Versions d'un mod Satisfactory (par son `mod_id` ficsit, renvoyé par ficsit_search) : `id` de version + contrainte SML + dépendances — nécessaires pour installer.", obj(&[("mod_id", prop_str("id ficsit du mod"))], &["mod_id"])),
        tool("install_ficsit_mod", "Installe un mod SATISFACTORY : télécharge le mod + SML + dépendances (cible LinuxServer) et les écrit dans Mods/ via SFTP, avec ARRÊT puis REDÉMARRAGE automatiques du serveur. `reference` = mod_reference, `version_id` via ficsit_versions. ✎ (modifiant — redémarre le serveur)", obj(&[("server_id", prop_int("ID du serveur")), ("reference", prop_str("mod_reference du mod")), ("version_id", prop_str("ID de la version à installer"))], &["server_id", "reference", "version_id"])),
        tool("create_schedule", "Crée une tâche planifiée du PANEL MineStrator, exécutée par l'hébergeur 24 h/24 même ordinateur éteint (le QUAND), avec éventuellement ses actions (le QUOI, dans l'ordre). `cron` = 5 champs « minute heure jour-du-mois mois jour-de-semaine » (`*`, `*/n`, `a-b`, listes `a,b`), ex. « 0 4 * * * » = chaque jour à 4 h. Chaque action : `action` command|power, `payload` (commande sur UNE ligne, ou start|restart|stop|kill), `time_offset` 0 à 900 s après la précédente, `continue_on_failure`. Ex. redémarrage annoncé : [{command, « say Redémarrage dans 60 s »}, {power, restart, time_offset 60}]. Tout est vérifié avant envoi. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("name", prop_str("Nom (1 à 100 caractères)")), ("cron", prop_str("5 champs : minute heure jour-du-mois mois jour-de-semaine")), ("is_active", prop_bool("Active (défaut true)")), ("only_when_online", prop_bool("Seulement si le serveur est en ligne (défaut false)")), ("tasks", schedule_tasks_schema())], &["server_id", "name", "cron"])),
        tool("update_schedule", "Modifie une tâche planifiée du panel (`schedule_id` via list_schedules) : nom, `cron` (5 champs), active, seulement si en ligne. Les champs omis gardent leur valeur ; les actions se gèrent avec add_/update_/delete_/reorder_schedule_task(s). Pour suspendre sans supprimer : is_active=false. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("name", prop_str("Nouveau nom (optionnel)")), ("cron", prop_str("Nouvelle planification, 5 champs (optionnel)")), ("is_active", prop_bool("Active (optionnel)")), ("only_when_online", prop_bool("Seulement si en ligne (optionnel)"))], &["server_id", "schedule_id"])),
        tool("delete_schedule", "SUPPRIME une tâche planifiée du panel et toutes ses actions. `confirm_name` doit reprendre EXACTEMENT son nom (casse comprise), donné par l'utilisateur, sinon refus. Pour la suspendre sans la perdre : update_schedule avec is_active=false. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("confirm_name", prop_str("Nom EXACT de la tâche planifiée, en confirmation"))], &["server_id", "schedule_id", "confirm_name"])),
        tool("add_schedule_task", "Ajoute une action À LA FIN d'une tâche planifiée du panel : `action` command (commande console sur UNE ligne) ou power (start|restart|stop|kill), `time_offset` 0 à 900 s après l'action précédente (ou le déclenchement), `continue_on_failure`. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("action", prop_str("command | power")), ("payload", prop_str("Commande, ou start|restart|stop|kill")), ("time_offset", prop_int("Délai en secondes, 0 à 900 (défaut 0)")), ("continue_on_failure", prop_bool("Continuer si l'action échoue (défaut false)"))], &["server_id", "schedule_id", "action", "payload"])),
        tool("update_schedule_task", "Modifie UNE action d'une tâche planifiée (`task_id` via list_schedules) ; les champs omis gardent leur valeur. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("task_id", prop_int("ID de l'action")), ("action", prop_str("command | power (optionnel)")), ("payload", prop_str("Commande, ou start|restart|stop|kill (optionnel)")), ("time_offset", prop_int("Délai 0 à 900 s (optionnel)")), ("continue_on_failure", prop_bool("Continuer si échec (optionnel)"))], &["server_id", "schedule_id", "task_id"])),
        tool("delete_schedule_task", "Retire UNE action d'une tâche planifiée (une tâche sans action ne fait plus rien). ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("task_id", prop_int("ID de l'action"))], &["server_id", "schedule_id", "task_id"])),
        tool("reorder_schedule_tasks", "Réordonne les actions d'une tâche planifiée : `task_ids` cite CHAQUE action une fois, dans le nouvel ordre d'exécution. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("schedule_id", prop_int("ID de la tâche planifiée")), ("task_ids", json!({ "type": "array", "description": "IDs des actions, dans l'ordre", "items": { "type": "integer" } }))], &["server_id", "schedule_id", "task_ids"])),
        // --- Plusieurs serveurs (fleet.rs) ---------------------------------------------------
        tool("copy_between_servers", "COPIE des fichiers ou dossiers d'un serveur vers un autre (ou vers un autre dossier du même serveur) : récursif, en flux, rien d'entier en mémoire. Chaque source arrive dans `target_dir` sous son nom (`/plugins/Essentials` → `<target_dir>/Essentials`) ; le dossier cible est créé au besoin. N'écrase JAMAIS un fichier existant sans `overwrite=true` (il est ignoré et listé) ; ne remplace jamais un dossier par un fichier ni l'inverse ; refuse la racine `/` comme source et la copie d'un chemin sur lui-même ou dans lui-même. `strategy` : `files` = fichier par fichier (plafonds 5000 fichiers, 2 Gio), `archive` = compression par le serveur source, transfert de l'archive (8 Gio au plus), extraction par le serveur cible, puis suppression des archives temporaires des deux côtés même en cas d'échec (sources d'un même dossier ; destination existante seulement avec overwrite=true), `auto` (défaut) = archive au-delà de 300 fichiers ou 200 Mio quand c'est possible. Vérifie la place disque de la cible avant. Rapport : fichiers copiés, octets, fichiers ignorés et pourquoi, durée. Peut être long pour un gros monde. ✎ (modifiant)", obj(&[("source_server_id", prop_int("ID du serveur source")), ("source_paths", prop_str_array("Chemins absolus à copier (fichiers ou dossiers, 50 au plus), ex. [\"/world\"]")), ("target_server_id", prop_int("ID du serveur cible (peut être le même)")), ("target_dir", prop_str("Dossier cible, ex. `/` ou `/plugins`")), ("overwrite", prop_bool("Remplacer les fichiers déjà présents (défaut false)")), ("strategy", prop_str("auto (défaut) | files | archive"))], &["source_server_id", "source_paths", "target_server_id", "target_dir"])),
        tool("diff_files", "COMPARE un fichier ou un dossier entre deux serveurs, ou deux chemins d'un même serveur (A = `server_id`/`path`, référence ; B = `other_server_id`/`other_path`, par défaut le même serveur ou le même chemin). Fichiers texte : diff unifié avec contexte (B par rapport à A, 400 lignes au plus). Binaires ou > 1 Mio : signalés sans diff. Dossiers : ajoutés, absents, modifiés (taille, date), type différent ; premier niveau, ou `recursive=true` (5000 entrées par côté au plus). Secrets des configs masqués ([SECRET]). Lecture seule ; peut lire un autre serveur que celui de la session.", obj(&[("server_id", prop_int("ID du serveur A (référence)")), ("path", prop_str("Chemin sur A")), ("other_server_id", prop_int("ID du serveur B (défaut : le même que A)")), ("other_path", prop_str("Chemin sur B (défaut : le même que sur A)")), ("recursive", prop_bool("Dossiers : descendre dans les sous-dossiers (défaut false)")), ("context_lines", prop_int("Lignes de contexte du diff (défaut 3, max 10)"))], &["server_id", "path"])),
        tool("find_player", "RETROUVE un joueur Minecraft (Java) sur TOUS les serveurs du compte : pseudo exact, début de pseudo (2 caractères au moins), UUID, ou alias d'anonymisation (`joueur-xxxxxx`). Pour chaque serveur où il est déjà venu : connecté ou non, dernière venue, temps de jeu cumulé, op/whitelist/banni ; serveurs non fouillés (autres jeux, indisponibles) listés avec la raison. Pseudos remplacés par des alias stables si l'anonymisation IA est active (comme list_players). Lecture seule ; parcourt aussi d'autres serveurs que celui de la session.", obj(&[("query", prop_str("Pseudo, début de pseudo, UUID ou alias")), ("exact", prop_bool("Pseudo exact seulement (défaut false : début de pseudo)")), ("server_ids", json!({ "type": "array", "items": { "type": "integer" }, "description": "Restreindre à ces serveurs (défaut : tous)" })), ("limit", prop_int("Joueurs détaillés (défaut 10, max 25)"))], &["query"])),
        tool("broadcast_command", "ENVOIE la même commande console à plusieurs serveurs par l'API (REST), avec un résultat serveur par serveur. Cibles : `server_ids` (20 au plus), OU `all_running=true` pour tous les serveurs démarrés — jamais déduit d'une liste vide. Par défaut seulement les serveurs démarrés (`only_running`, défaut true) ; un état illisible n'est pas envoyé. Commande non vide, sur une seule ligne. Sa sortie se lit ensuite avec read_console. ✎ (modifiant)", obj(&[("command", prop_str("Commande console (une ligne), ex. `say Redémarrage dans 5 min`")), ("server_ids", json!({ "type": "array", "items": { "type": "integer" }, "description": "Serveurs visés (20 au plus)" })), ("all_running", prop_bool("true = tous les serveurs démarrés (sans server_ids)")), ("only_running", prop_bool("Ignorer les serveurs non démarrés (défaut true)"))], &["command"])),
    ])
}

// --- Résumé / helpers -----------------------------------------------------

/// Résumé d'un historique fusionné, pondéré par la durée que couvre chaque point (10 s en local,
/// une minute pour l'hébergeur) : les périodes vues seulement par l'hébergeur ne sont pas
/// écrasées par la densité du local.
fn summarize(h: &MetricsHistory) -> Value {
    if h.points.is_empty() {
        return json!(null);
    }
    let weight = |p: &HistoryPoint| if p.source == HistorySource::Local { 10.0 } else { 60.0 };
    let (mut total, mut mem_total) = (0.0, 0.0);
    let (mut cpu_sum, mut cpu_max) = (0.0, 0.0f64);
    let (mut mem_sum, mut mem_max) = (0.0, 0.0f64);
    for p in &h.points {
        let w = weight(p);
        total += w;
        cpu_sum += p.cpu * w;
        cpu_max = cpu_max.max(p.cpu);
        if p.mem_limit > 0 {
            let pct = p.mem as f64 / p.mem_limit as f64 * 100.0;
            mem_total += w;
            mem_sum += pct * w;
            mem_max = mem_max.max(pct);
        }
    }
    let round2 = |v: f64| (v * 100.0).round() / 100.0;
    let mut out = json!({
        "cpu_absolute_avg": round2(cpu_sum / total),
        "cpu_absolute_max": round2(cpu_max),
        "mem_pct_avg": (mem_total > 0.0).then(|| round2(mem_sum / mem_total)),
        "mem_pct_max": (mem_total > 0.0).then(|| round2(mem_max)),
        "disk_bytes_last": h.points.last().map(|p| p.disk),
        "local_points": h.points.iter().filter(|p| p.source == HistorySource::Local).count(),
        "host_points": h.points.iter().filter(|p| p.source == HistorySource::Host).count(),
    });
    if !h.players.is_empty() {
        let n = h.players.len() as f64;
        out["players_peak"] = json!(h.players.iter().map(|p| p.count).max());
        out["players_avg"] = json!(round2(h.players.iter().map(|p| p.count as f64).sum::<f64>() / n));
    }
    out
}

fn req_i64(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("paramètre `{key}` (entier) requis"))
}
fn req_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("paramètre `{key}` (texte) requis"))
}
/// Type de contenu optionnel (`kind`) ; vide ou `all` = tous.
fn opt_kind(args: &Value) -> Result<Option<ContentKind>, String> {
    match args.get("kind").and_then(Value::as_str).map(str::trim) {
        None | Some("") | Some("all") => Ok(None),
        Some(k) => ContentKind::parse(k)
            .map(Some)
            .ok_or_else(|| format!("`kind` inconnu : « {k} » (mods, plugins ou datapacks)")),
    }
}

/// Liste de chaînes (tableau, ou chaîne seule), sans les vides.
fn str_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => Vec::new(),
    }
}

/// Associe chaque clé à son type d'après la dernière vérification (une clé écartée garde son type :
/// `update_mods` dira pourquoi elle n'est pas appliquée). Une clé inconnue est une erreur.
fn resolve_update_keys(
    current: &crate::updates::ContentUpdates,
    kind: Option<ContentKind>,
    keys: &[String],
) -> Result<Vec<crate::updates::UpdateSelection>, String> {
    let known = current
        .updates
        .iter()
        .map(|u| (u.kind, u.key.as_str()))
        .chain(current.ignored.iter().map(|i| (i.kind, i.key.as_str())));
    let known: Vec<(ContentKind, &str)> = known.filter(|(k, _)| kind.is_none_or(|w| w == *k)).collect();
    let mut items = Vec::new();
    let mut unknown = Vec::new();
    for key in keys {
        match known.iter().find(|(_, k)| k == key) {
            Some((k, _)) => items.push(crate::updates::UpdateSelection { kind: *k, key: key.clone() }),
            None => unknown.push(key.as_str()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "aucune mise à jour connue pour : {} — utilise les clés entre crochets de check_updates (force=true pour revérifier).",
            unknown.join(", ")
        ));
    }
    Ok(items)
}

/// Tableau de chaînes requis (non vide).
fn req_str_array(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = args
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if list.is_empty() {
        return Err(format!("paramètre `{key}` (tableau de chemins, non vide) requis"));
    }
    Ok(list)
}
fn opt_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}
fn opt_str(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}
fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
/// Erreur d'un outil MyBox : le code MineStrator est suivi de son explication quand on la connaît.
fn mybox_es(e: crate::Error) -> String {
    match &e {
        crate::Error::Api { code } => match crate::mybox::explain_api_code(code) {
            Some(why) => format!("{e} — {why}"),
            None => e.to_string(),
        },
        _ => e.to_string(),
    }
}
fn json_pretty<T: serde::Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string_pretty(v).map_err(es)
}
fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}
fn obj(props: &[(&str, Value)], required: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in props {
        map.insert((*k).to_string(), v.clone());
    }
    json!({ "type": "object", "properties": Value::Object(map), "required": required })
}
fn prop_int(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}
fn prop_str(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}
fn prop_str_array(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}
fn prop_bool(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}
/// Schéma des actions d'une tâche planifiée (`create_schedule`).
fn schedule_tasks_schema() -> Value {
    json!({
        "type": "array",
        "description": "Actions dans l'ordre d'exécution (optionnel ; ajoutables ensuite avec add_schedule_task)",
        "items": {
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "command | power" },
                "payload": { "type": "string", "description": "Commande (une ligne), ou start|restart|stop|kill" },
                "time_offset": { "type": "integer", "description": "Délai en secondes après l'action précédente, 0 à 900 (défaut 0)" },
                "continue_on_failure": { "type": "boolean", "description": "Continuer si l'action échoue (défaut false)" }
            },
            "required": ["action", "payload"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_allowlist_csv() {
        assert_eq!(parse_allowlist("read_file, list_files ,, read_console"), vec![
            "read_file".to_string(),
            "list_files".to_string(),
            "read_console".to_string(),
        ]);
        assert!(parse_allowlist("").is_empty());
    }

    #[test]
    fn filter_tools_keeps_only_allowed() {
        // Sans restriction (env absente dans les tests) : liste complète conservée.
        let full = tool_list();
        let n = full.as_array().unwrap().len();
        assert_eq!(filter_tools_by_env(full).as_array().unwrap().len(), n);
    }

    #[test]
    fn write_tools_are_default_deny() {
        // Un outil inconnu est traité comme modifiant (garde-fou default-deny).
        assert!(is_write_tool("outil_inconnu"));
        assert!(!is_write_tool("read_file"));
    }

    #[test]
    fn sftp_bridge_tools_are_classified_and_exposed() {
        // upload/edit modifient le serveur → soumis à allow_writes ; download/stat lisent → autorisés
        // même en lecture seule. Tous doivent être exposés (READ ∪ WRITE) pour être atteignables.
        for w in ["upload_file", "edit_file"] {
            assert!(is_write_tool(w), "{w} doit être modifiant");
            assert!(WRITE_TOOLS.contains(&w), "{w} doit être exposé");
        }
        for r in ["download_file", "file_stat"] {
            assert!(!is_write_tool(r), "{r} doit être en lecture");
            assert!(READ_TOOLS.contains(&r), "{r} doit être exposé");
        }
        // Présents dans le catalogue JSON exposé par tools/list.
        let names: Vec<&str> = tool_list()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        for t in ["upload_file", "download_file", "edit_file", "file_stat"] {
            assert!(names.contains(&t), "{t} absent du catalogue");
        }
    }

    #[test]
    fn every_listed_tool_is_classified_and_every_classified_tool_is_listed() {
        let names: Vec<&str> = tool_list()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        for n in &names {
            assert!(READ_TOOLS.contains(n) ^ WRITE_TOOLS.contains(n), "{n} : lecture XOR écriture");
        }
        for n in READ_TOOLS.iter().chain(WRITE_TOOLS) {
            assert!(names.contains(n), "{n} sans schéma dans tool_list()");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "noms d'outils en double");
    }

    #[test]
    fn mybox_infrastructure_tools_are_writes_and_reads_stay_reads() {
        for t in INFRA_TOOLS {
            assert!(is_write_tool(t) && WRITE_TOOLS.contains(t), "{t} doit être modifiant");
        }
        for t in ["mybox_capacity", "server_catalog", "plan_placement", "mybox_smartscan"] {
            assert!(!is_write_tool(t), "{t} doit être en lecture");
        }
        // La suppression exige une confirmation explicite dans son schéma.
        let delete = tool_list()
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "mybox_delete_server")
            .unwrap();
        assert!(delete["inputSchema"]["required"].as_array().unwrap().contains(&json!("confirm_name")));
        // Les schémas rappellent les unités.
        let create = tool_list().as_array().unwrap().iter().find(|t| t["name"] == "mybox_create_server").unwrap();
        let d = create["description"].as_str().unwrap();
        assert!(d.contains("CŒURS") && d.contains("GO"), "{d}");
    }

    #[test]
    fn mybox_api_errors_carry_an_explanation() {
        let e = mybox_es(crate::Error::Api { code: "API_MYBOX_NOT_ENOUGH_RAM".into() });
        assert!(e.contains("API_MYBOX_NOT_ENOUGH_RAM") && e.contains("RAM"), "{e}");
        assert_eq!(mybox_es(crate::Error::Invalid("x".into())), "x");
    }

    fn item(name: &str, version: &str, enabled: bool) -> InstalledItem {
        InstalledItem {
            name: name.into(),
            filename: format!("{name}.jar"),
            version: version.into(),
            enabled,
            loader: "neoforge".into(),
        }
    }

    #[test]
    fn server_side_file_tools_are_classified_and_exposed() {
        // Compresser/extraire créent des fichiers sur le serveur → modifiants ; l'analyse du
        // disque ne fait que lire → autorisée en lecture seule. Tous exposés et gardés par le MCP
        // local quand le MCP officiel prend la main (outils de fichiers).
        for w in ["compress_files", "extract_archive"] {
            assert!(is_write_tool(w), "{w} doit être modifiant");
            assert!(WRITE_TOOLS.contains(&w), "{w} doit être exposé");
        }
        assert!(!is_write_tool("disk_usage") && READ_TOOLS.contains(&"disk_usage"));
        for t in ["compress_files", "extract_archive", "disk_usage"] {
            assert!(LOCAL_KEEP_TOOLS.contains(&t), "{t} doit rester local");
            assert!(tool_for_game(t, "satisfactory") && tool_for_game(t, "generic"), "{t} est universel");
        }
        let tools = tool_list().as_array().unwrap();
        let schema = |n: &str| {
            tools
                .iter()
                .find(|t| t["name"] == n)
                .unwrap_or_else(|| panic!("{n} absent"))["inputSchema"]
                .clone()
        };
        assert_eq!(schema("compress_files")["properties"]["paths"]["type"], "array");
        assert_eq!(schema("compress_files")["required"], json!(["server_id", "paths"]));
        assert_eq!(schema("disk_usage")["required"], json!(["server_id"]));
        assert!(cache_ttl("disk_usage").is_some() && cache_ttl("compress_files").is_none());
        assert_eq!(req_str_array(&json!({ "paths": ["/a", 3, "/b"] }), "paths").unwrap(), ["/a", "/b"]);
        assert!(req_str_array(&json!({ "paths": [] }), "paths").is_err());
        assert!(req_str_array(&json!({ "paths": "/a" }), "paths").is_err());
    }

    #[test]
    fn installed_list_is_compact_and_filterable() {
        let items = vec![item("Sodium", "0.5", true), item("Lithium", "0.11", false)];
        // Sans filtre : en-tête (compte + loaders) + une ligne par mod, désactivé marqué.
        let all = format_installed(&items, "", "mod");
        assert!(all.contains("2 mod(s) installé(s)"));
        assert!(all.contains("loaders : neoforge"));
        assert!(all.contains("- Sodium (0.5)"));
        assert!(all.contains("- Lithium (0.11) [désactivé]"));
        // Filtre par sous-chaîne, insensible à la casse.
        let filtered = format_installed(&items, "SOD", "mod");
        assert!(filtered.contains("Sodium") && !filtered.contains("Lithium"));
        // Aucun résultat.
        assert!(format_installed(&items, "absent", "mod").contains("(aucun)"));
    }

    #[test]
    fn update_tools_are_classified_and_scoped() {
        // La vérification n'installe rien (lecture) ; l'application est modifiante.
        assert!(!is_write_tool("check_updates"));
        assert!(READ_TOOLS.contains(&"check_updates"));
        assert!(is_write_tool("update_mods"));
        assert!(WRITE_TOOLS.contains(&"update_mods"));
        for t in ["check_updates", "update_mods"] {
            assert!(tool_for_game(t, "minecraft"), "{t} : Minecraft");
            assert!(!tool_for_game(t, "satisfactory"), "{t} : pas pour un autre jeu");
            assert!(LOCAL_KEEP_TOOLS.contains(&t), "{t} : exclusif, gardé face au MCP officiel");
        }
        // Résultat déjà gardé par le core : pas de second cache.
        assert!(cache_ttl("check_updates").is_none());
        assert!(cache_ttl("update_mods").is_none());
    }

    #[test]
    fn update_arguments_are_read_leniently() {
        use crate::updates::{ContentUpdate, ContentUpdates, IgnoreReason, IgnoredUpdate};
        assert_eq!(opt_kind(&json!({})), Ok(None));
        assert_eq!(opt_kind(&json!({ "kind": "all" })), Ok(None));
        assert_eq!(opt_kind(&json!({ "kind": "plugin" })), Ok(Some(ContentKind::Plugins)));
        assert!(opt_kind(&json!({ "kind": "texture" })).is_err());
        assert_eq!(str_list(&json!({ "keys": ["a.jar", " ", "b.jar"] }), "keys"), vec!["a.jar", "b.jar"]);
        assert_eq!(str_list(&json!({ "keys": "a.jar" }), "keys"), vec!["a.jar"]);
        assert!(str_list(&json!({}), "keys").is_empty());

        let current = ContentUpdates {
            mc_version: Some("1.21.1".into()),
            platform: None,
            checked_at: 0,
            examined: 2,
            untracked: 0,
            updates: vec![ContentUpdate {
                kind: ContentKind::Mods,
                key: "sodium.jar".into(),
                name: "Sodium".into(),
                enabled: true,
                current_version: "0.5".into(),
                new_version: "0.6".into(),
                new_filename: "sodium-0.6.jar".into(),
                source: "modrinth".into(),
            }],
            ignored: vec![IgnoredUpdate {
                kind: ContentKind::Plugins,
                key: "LP.jar".into(),
                name: "LuckPerms".into(),
                current_version: "5".into(),
                new_version: "5".into(),
                new_filename: "LP-Velocity.jar".into(),
                reason: IgnoreReason::OtherPlatform,
            }],
            errors: vec![],
        };
        let keys = vec!["sodium.jar".to_string(), "LP.jar".to_string()];
        let items = resolve_update_keys(&current, None, &keys).unwrap();
        assert_eq!(items[0].kind, ContentKind::Mods);
        assert_eq!(items[1].kind, ContentKind::Plugins, "clé écartée : transmise, update_mods dira pourquoi");
        // Filtre de type : la clé d'un autre type devient inconnue.
        let err = resolve_update_keys(&current, Some(ContentKind::Mods), &keys).unwrap_err();
        assert!(err.contains("LP.jar") && !err.contains("sodium.jar"));
    }

    #[test]
    fn cache_ttl_covers_reads_not_live_or_writes() {
        assert!(cache_ttl("market_search").is_some());
        assert!(cache_ttl("list_installed_mods").is_some());
        // Jamais caché : live, profiler, modifiant, ou déjà local.
        assert!(cache_ttl("read_console").is_none());
        assert!(cache_ttl("analyze_performance").is_none());
        assert!(cache_ttl("install_mod").is_none());
        assert!(cache_ttl("server_metrics").is_none());
    }

    const SCHEDULE_WRITES: [&str; 7] = [
        "create_schedule",
        "update_schedule",
        "delete_schedule",
        "add_schedule_task",
        "update_schedule_task",
        "delete_schedule_task",
        "reorder_schedule_tasks",
    ];

    #[test]
    fn schedule_writes_are_gated_and_never_given_to_the_copilot() {
        for t in SCHEDULE_WRITES {
            assert!(is_write_tool(t) && WRITE_TOOLS.contains(&t), "{t} : modifiant (allow_writes)");
            assert!(INFRA_TOOLS.contains(&t), "{t} : configuration persistante, jamais au Copilote");
            assert!(!LOCAL_KEEP_TOOLS.contains(&t), "{t}");
            assert!(tool_for_game(t, "generic") && tool_for_game(t, "satisfactory"), "{t} : tous les jeux");
            assert!(cache_ttl(t).is_none());
        }
        assert!(!is_write_tool("list_schedules"));
        let tools = tool_list().as_array().unwrap();
        let schema = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap()["inputSchema"].clone();
        assert_eq!(schema("create_schedule")["required"], json!(["server_id", "name", "cron"]));
        assert!(schema("delete_schedule")["required"].as_array().unwrap().contains(&json!("confirm_name")));
        assert_eq!(schema("create_schedule")["properties"]["tasks"]["type"], "array");
    }

    #[test]
    fn schedule_inputs_are_parsed_then_validated() {
        let cron = parse_cron(" 0  4 * * 1-5 ").unwrap();
        assert_eq!((cron.minute.as_str(), cron.hour.as_str(), cron.day_of_week.as_str()), ("0", "4", "1-5"));
        assert!(parse_cron("0 4 * *").is_err(), "4 champs");
        assert!(parse_cron("0 4 * * * *").is_err(), "6 champs");
        // Valeurs hors bornes : refusées par la validation du cœur (avant tout envoi).
        let bad = crate::PanelScheduleInput { name: "Nuit".into(), cron: parse_cron("0 24 * * *").unwrap(), is_active: true, only_when_online: false };
        assert!(bad.validated().is_err());

        let t = schedule_task_input(&json!({ "action": "power", "payload": "restart", "time_offset": 60 }), None).unwrap();
        assert_eq!((t.action.as_str(), t.payload.as_str(), t.time_offset, t.continue_on_failure), ("power", "restart", 60, false));
        assert!(schedule_task_input(&json!({ "payload": "say a" }), None).is_err(), "action requise");
        assert!(schedule_task_input(&json!({ "action": "command", "payload": "say a", "time_offset": 901 }), None)
            .unwrap()
            .validated()
            .is_err());
        assert!(schedule_task_input(&json!({ "action": "command", "payload": "say a\nop x" }), None)
            .unwrap()
            .validated()
            .is_err(), "une seule commande par action");
        // Modification : les champs omis gardent la valeur actuelle.
        let current = crate::PanelTask {
            id: 3,
            sequence_id: 1,
            action: "command".into(),
            payload: "say bonjour".into(),
            time_offset: 30,
            continue_on_failure: true,
        };
        let t = schedule_task_input(&json!({ "time_offset": 5 }), Some(&current)).unwrap();
        assert_eq!((t.payload.as_str(), t.time_offset, t.continue_on_failure), ("say bonjour", 5, true));
    }

    #[test]
    fn task_reorder_must_cite_each_task_once() {
        let task = |id| crate::PanelTask {
            id,
            sequence_id: id,
            action: "command".into(),
            payload: "say x".into(),
            time_offset: 0,
            continue_on_failure: false,
        };
        let schedule: crate::PanelSchedule = serde_json::from_value(json!({ "id": 9, "name": "Nuit" })).unwrap();
        let schedule = crate::PanelSchedule { tasks: vec![task(1), task(2), task(3)], ..schedule };
        assert!(check_task_order(&schedule, &[3, 1, 2]).is_ok());
        assert!(check_task_order(&schedule, &[3, 1]).is_err(), "oubli");
        assert!(check_task_order(&schedule, &[3, 1, 1, 2]).is_err(), "doublon");
        assert!(check_task_order(&schedule, &[3, 1, 4]).is_err(), "étrangère");
    }

    #[test]
    fn mcp_config_reads_older_files_without_the_gateway() {
        // Un `mcp.json` d'une version précédente : passerelle désactivée par défaut.
        let cfg: McpConfig = serde_json::from_value(json!({ "enabled": true, "allow_writes": false })).unwrap();
        assert!(!cfg.official_gateway && !cfg.allow_writes);
        assert!(!McpConfig::default().official_gateway);
    }

    // --- Plusieurs serveurs (fleet.rs) ---

    #[test]
    fn fleet_tools_are_classified_and_kept_from_the_copilot() {
        // Copie et commande groupée : modifiantes, et jamais au Copilote (INFRA_TOOLS).
        for w in ["copy_between_servers", "broadcast_command"] {
            assert!(is_write_tool(w) && WRITE_TOOLS.contains(&w), "{w} doit être modifiant");
            assert!(INFRA_TOOLS.contains(&w), "{w} hors de portée du Copilote");
            assert!(!LOCAL_KEEP_TOOLS.contains(&w));
        }
        // Comparaison et recherche : lectures, exclusives (gardées face au MCP officiel).
        for r in ["diff_files", "find_player"] {
            assert!(!is_write_tool(r) && READ_TOOLS.contains(&r), "{r} doit être en lecture");
            assert!(LOCAL_KEEP_TOOLS.contains(&r) && !INFRA_TOOLS.contains(&r));
        }
        for t in ["copy_between_servers", "broadcast_command", "diff_files", "find_player"] {
            assert!(tool_for_game(t, "minecraft") && tool_for_game(t, "satisfactory"), "{t} universel");
            assert!(cache_ttl(t).is_none(), "{t} jamais caché");
        }
        let tools = tool_list().as_array().unwrap();
        let schema = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("{n} absent"))["inputSchema"].clone();
        assert_eq!(
            schema("copy_between_servers")["required"],
            json!(["source_server_id", "source_paths", "target_server_id", "target_dir"])
        );
        assert_eq!(schema("copy_between_servers")["properties"]["source_paths"]["type"], "array");
        assert_eq!(schema("diff_files")["required"], json!(["server_id", "path"]));
        assert_eq!(schema("find_player")["required"], json!(["query"]));
        // « Tous » n'est pas une valeur par défaut : seule la commande est requise, la cible est
        // validée à part (server_ids OU all_running=true).
        assert_eq!(schema("broadcast_command")["required"], json!(["command"]));
        assert_eq!(schema("broadcast_command")["properties"]["server_ids"]["items"]["type"], "integer");
    }
}
