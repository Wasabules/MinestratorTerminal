//! Logique MCP **indépendante du transport** : traite un message JSON-RPC et renvoie
//! la réponse. Réutilisée par le binaire stdio (`minestrator-mcp`), par le mode `--mcp`
//! de l'app desktop, et à terme par un daemon.
//!
//! Réglable via [`McpConfig`] (persistée) : activer/désactiver, et autoriser ou non les
//! actions modifiantes (mode lecture seule).

use crate::models::InstalledItem;
use crate::{Core, MetricSample};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

/// Réglages du serveur MCP (modifiables depuis l'UI, persistés dans `mcp.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub enabled: bool,
    /// Autorise les actions modifiantes (power, commandes, modération, écriture de fichiers).
    pub allow_writes: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_writes: true,
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
                filter_tools_by_env(tool_list())
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
    "read_startup",
    "list_installed_mods",
    "list_installed_plugins",
    "list_backups",
    "list_snapshots",
    "market_search",
    "list_mod_versions",
    "analyze_performance",
    "parse_spark_report",
    "diagnose_startup",
    "inspect_region",
];

/// Outils **modifiants** (soumis à `allow_writes`). Sert aux listes d'autorisation/schémas ;
/// la garde de sécurité, elle, repose sur le complément de [`READ_TOOLS`] (default-deny).
pub(crate) const WRITE_TOOLS: &[&str] = &[
    "power_action",
    "send_command",
    "player_action",
    "write_file",
    "create_dir",
    "delete_path",
    "rename_path",
    "set_startup_params",
    "install_mod",
    // Snapshot = additif/sans risque → l'agent peut le créer (filet avant une intervention).
    // NB : restore_snapshot / restore_backup / delete_snapshot sont DESTRUCTIFS et volontairement
    // ABSENTS d'ici → jamais auto-exécutés par l'agent ; uniquement via « Appliquer » (intent user).
    "create_snapshot",
];

/// Un outil est-il modifiant ? Default-deny : tout ce qui n'est PAS un outil de lecture connu.
pub(crate) fn is_write_tool(name: &str) -> bool {
    !READ_TOOLS.contains(&name)
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
    "write_file",
    "create_dir",
    "delete_path",
    "rename_path",
    "inspect_region",
    "diagnose_startup",
    "analyze_performance",
    "parse_spark_report",
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
fn env_allows(name: &str) -> bool {
    match env_tool_allowlist() {
        Some(list) => list.iter().any(|t| t == name),
        None => true,
    }
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
        return tool_err("Outil inconnu ou non exposé par le serveur MCP.");
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
        "list_files" | "read_file" | "read_startup" => 15,
        "list_servers" => 10,
        "server_status" => 5,
        // Volontairement absents : server_metrics (local, déjà rapide), read_console (live),
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
            // Lectures indépendantes → concurrentes : recouvre le RTT du GET « live » sous l'attente
            // d'échantillon (`sample_stats` ouvre un WS et patiente jusqu'à quelques secondes).
            let (live, stats) = tokio::join!(core.live_light(id), core.sample_stats(id));
            json_pretty(&json!({ "live": live.map_err(es)?, "stats": stats.map_err(es)? }))
        }

        "server_metrics" => {
            let id = req_i64(&args, "server_id")?;
            let since = opt_i64(&args, "since_secs", 3600).clamp(60, 14 * 24 * 3600);
            let samples = core.metrics(id, since).map_err(es)?;
            json_pretty(&json!({
                "since_secs": since,
                "count": samples.len(),
                "summary": summarize(&samples),
                "samples": downsample(&samples, 120),
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

        _ => Err(format!("outil inconnu : {name}")),
    }
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
        tool("server_status", "État détaillé d'un serveur : joueurs, version, MOTD, limites et consommation instantanée (échantillon live).", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("server_metrics", "Historique de consommation (CPU/RAM/disque) + résumé (moyennes/max) — pour analyser la charge et optimiser.", obj(&[("server_id", prop_int("ID du serveur")), ("since_secs", prop_int("Fenêtre en secondes (défaut 3600, max 14 j)"))], &["server_id"])),
        tool("power_action", "Démarre/redémarre/arrête un serveur. `action` ∈ start|restart|restart10|stop|stop10|kill.", obj(&[("server_id", prop_int("ID du serveur")), ("action", prop_str("start|restart|restart10|stop|stop10|kill"))], &["server_id", "action"])),
        tool("send_command", "Envoie une commande à la console (n'a d'effet que serveur en ligne).", obj(&[("server_id", prop_int("ID du serveur")), ("command", prop_str("Commande console"))], &["server_id", "command"])),
        tool("read_console", "100 dernières lignes de console — pour diagnostiquer un crash.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("player_action", "Modération Minecraft. `action` ∈ kick|ban|unban|op_add|op_remove|whitelist_add|whitelist_remove.", obj(&[("server_id", prop_int("ID du serveur")), ("action", prop_str("kick|ban|unban|op_add|op_remove|whitelist_add|whitelist_remove")), ("player", prop_str("Pseudo exact"))], &["server_id", "action", "player"])),
        tool("list_files", "Liste un répertoire via SFTP (racine = `/`).", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant"))], &["server_id", "path"])),
        tool("read_file", "Lit un fichier texte via SFTP (refuse binaires / > 2 Mo).", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant"))], &["server_id", "path"])),
        tool("read_gz", "Lit un fichier texte GZIPPÉ via SFTP en le décompressant (ex. log tourné `latest.log.gz`, `crash-report.gz`) — indispensable pour diagnostiquer sur des logs archivés. Complète read_file, qui refuse les binaires.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant du .gz"))], &["server_id", "path"])),
        tool("list_archive", "Liste les entrées d'une archive distante (`.zip` / `.tar` / `.tar.gz`) SANS l'extraire — pour repérer un fichier à inspecter dans un backup, un modpack, etc.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant de l'archive"))], &["server_id", "path"])),
        tool("read_archive_entry", "Lit le contenu TEXTE d'UNE entrée d'archive (`.zip`/`.tar`/`.tar.gz`) sans l'extraire sur disque. `entry` = nom exact renvoyé par list_archive.", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin de l'archive")), ("entry", prop_str("Nom de l'entrée dans l'archive"))], &["server_id", "path", "entry"])),
        tool("write_file", "Écrit/écrase un fichier de config via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin distant")), ("content", prop_str("Nouveau contenu"))], &["server_id", "path", "content"])),
        tool("create_dir", "Crée un dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin du nouveau dossier"))], &["server_id", "path"])),
        tool("delete_path", "Supprime un fichier ou un dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("path", prop_str("Chemin à supprimer")), ("is_dir", prop_bool("true si c'est un dossier"))], &["server_id", "path"])),
        tool("rename_path", "Renomme ou déplace un fichier/dossier via SFTP. ✎ (modifiant)", obj(&[("server_id", prop_int("ID du serveur")), ("from", prop_str("Chemin source")), ("to", prop_str("Chemin destination"))], &["server_id", "from", "to"])),
        tool("read_startup", "Lit la commande de démarrage (flags JVM : -Xmx, GC, Aikar…), le JAR, la mémoire et l'image.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
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
    ])
}

// --- Résumé / helpers -----------------------------------------------------

fn summarize(samples: &[MetricSample]) -> Value {
    if samples.is_empty() {
        return json!(null);
    }
    let n = samples.len() as f64;
    let mut cpu_sum = 0.0;
    let mut cpu_max = 0.0f64;
    let mut mem_sum = 0.0;
    let mut mem_max = 0.0f64;
    let mut disk_last = 0i64;
    for s in samples {
        cpu_sum += s.cpu;
        cpu_max = cpu_max.max(s.cpu);
        let memp = if s.mem_limit > 0 {
            s.mem as f64 / s.mem_limit as f64 * 100.0
        } else {
            0.0
        };
        mem_sum += memp;
        mem_max = mem_max.max(memp);
        disk_last = s.disk;
    }
    json!({
        "cpu_absolute_avg": (cpu_sum / n * 100.0).round() / 100.0,
        "cpu_absolute_max": (cpu_max * 100.0).round() / 100.0,
        "mem_pct_avg": mem_sum / n,
        "mem_pct_max": mem_max,
        "disk_bytes_last": disk_last,
    })
}

fn downsample(samples: &[MetricSample], max: usize) -> Vec<&MetricSample> {
    if samples.len() <= max {
        return samples.iter().collect();
    }
    let step = (samples.len() / max).max(1);
    samples.iter().step_by(step).collect()
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
fn prop_bool(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
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
    fn cache_ttl_covers_reads_not_live_or_writes() {
        assert!(cache_ttl("market_search").is_some());
        assert!(cache_ttl("list_installed_mods").is_some());
        // Jamais caché : live, profiler, modifiant, ou déjà local.
        assert!(cache_ttl("read_console").is_none());
        assert!(cache_ttl("analyze_performance").is_none());
        assert!(cache_ttl("install_mod").is_none());
        assert!(cache_ttl("server_metrics").is_none());
    }
}
