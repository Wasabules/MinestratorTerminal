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
    "search_files",
    "disk_usage",
    "download_file",
    "file_stat",
    "read_startup",
    "read_properties",
    "mybox_info",
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
];

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
        "list_files" | "read_file" | "read_startup" | "read_properties" | "search_files" => 15,
        // Analyse du disque : calculée par le serveur sur tout un arbre, change lentement.
        "disk_usage" => 30,
        "mybox_info" => 30,
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
        tool("server_status", "État temps réel d'un serveur : état, consommation instantanée (CPU en centièmes de cœur dont FlexCore/`is_bursting`, RAM et disque en Mo), réseau cumulé, uptime, joueurs, version, MOTD.", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("list_schedules", "Tâches planifiées du panel MineStrator d'un serveur, exécutées par l'hébergeur même ordinateur éteint (à distinguer des automatisations de l'app) : nom, planification cron, active, seulement si en ligne, prochaine et dernière exécution, actions dans l'ordre (commande ou alimentation, délai en secondes).", obj(&[("server_id", prop_int("ID du serveur"))], &["server_id"])),
        tool("list_players", "Joueurs Minecraft (Java) déjà venus sur un serveur, pas seulement les connectés : statistiques (joueurs uniques, connectés, actifs sur 24 h / 7 j / 30 j, temps de jeu total) puis liste triée avec temps de jeu CUMULÉ (heures), dernière venue, op/whitelist/banni. Pour « qui a le plus joué cette semaine » : sort=play_time et active_days=7 (le temps de jeu reste cumulé depuis la première venue). Pseudos remplacés par des alias stables si l'anonymisation IA est active.", obj(&[("server_id", prop_int("ID du serveur")), ("sort", prop_str("play_time (défaut) | last_seen | name")), ("active_days", prop_int("Seulement les joueurs venus dans les N derniers jours (0 = tous)")), ("search", prop_str("Filtre sur le pseudo (sous-chaîne)")), ("limit", prop_int("Joueurs listés (défaut 25, max 100)"))], &["server_id"])),
        tool("mybox_info", "Détail d'une MyBox (`mybox_id` via list_servers) : échéance, renouvellement automatique, jours avant suppression si expirée, CPU/RAM encore allouables, offre (bases de données, IP dédiées), serveurs et ressources allouées.", obj(&[("mybox_id", prop_int("ID de la MyBox"))], &["mybox_id"])),
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
}
