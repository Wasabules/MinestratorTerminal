//! Adaptateurs d'**agents CLI** (Claude Code, OpenCode, Gemini CLI) — pendant CLI de la couche
//! multi-LLM HTTP (`llm.rs`). Chaque agent a ses propres flags, son format de config MCP et son
//! schéma de sortie. On isole ces différences ici pour que `copilot.rs` reste agnostique.
//!
//! Sécurité, sur deux niveaux :
//! - **Outils MCP** : notre serveur MCP est lancé avec la liste blanche d'outils dans son env
//!   (`MCP_ALLOWED_TOOLS_ENV`) → un agent en mode suggestion ne PEUT PAS appeler un outil
//!   d'écriture MCP (l'outil n'existe même pas côté serveur).
//! - **Outils natifs** de l'agent (shell, disque local, web) : l'agent lit du contenu non fiable
//!   (console d'un joueur, crash-report, config), donc une injection de prompt pourrait lui faire
//!   exécuter une commande sur la machine de l'admin. Chaque agent les retire par son propre
//!   mécanisme (schémas vérifiés dans les docs officielles, cf. `plan_run`) : Claude Code
//!   `--tools` + `--strict-mcp-config`, OpenCode règles `permission: deny` (respectées par
//!   `--auto`), Gemini mode `default` (confirmations refusées en non-interactif) + `tools.exclude`.
//!
//! La config de l'agent (avec la clé API si le MCP officiel est branché) vit dans un [`RunDir`]
//! privé, effacé à la fin du processus.
//!
//! Robustesse : le parsing Claude Code (éprouvé) reste dans `copilot.rs` et est réutilisé tel
//! quel ; OpenCode/Gemini ont un parsing **défensif** (chaque champ optionnel, dégradation propre).

use crate::config::{MCP_ALLOWED_TOOLS_ENV, MCP_FORCE_ENABLED_ENV};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Agent CLI sélectionné. Le binaire concret vient de `cli_command` (surchargeable) ; l'enum
/// détermine flags, config MCP et parsing.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CliAgent {
    #[default]
    ClaudeCode,
    OpenCode,
    Gemini,
}

impl CliAgent {
    /// Binaire par défaut (pré-remplissage ; l'utilisateur peut le surcharger).
    pub fn default_command(self) -> &'static str {
        match self {
            CliAgent::ClaudeCode => "claude",
            CliAgent::OpenCode => "opencode",
            CliAgent::Gemini => "gemini",
        }
    }
}

/// Statut de disponibilité d'un agent CLI (exposé à l'UI Réglages → Copilote).
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CliStatus {
    pub agent: CliAgent,
    /// Binaire sondé (commande par défaut de l'agent).
    pub command: String,
    /// Présent et répondant à `--version` ?
    pub available: bool,
    /// Version brute rapportée (1re ligne de sortie), si disponible.
    pub version: Option<String>,
}

/// Détecte les trois agents CLI (Claude Code / OpenCode / Gemini) en parallèle, via leur binaire
/// par défaut. L'UI affiche un message minimaliste pour ceux qui sont absents.
pub async fn detect_clis() -> Vec<CliStatus> {
    async fn one(agent: CliAgent) -> CliStatus {
        let command = agent.default_command();
        let version = crate::cli::probe(command, 6).await;
        CliStatus {
            agent,
            command: command.to_string(),
            available: version.is_some(),
            version,
        }
    }
    let (claude, opencode, gemini) = tokio::join!(
        one(CliAgent::ClaudeCode),
        one(CliAgent::OpenCode),
        one(CliAgent::Gemini),
    );
    vec![claude, opencode, gemini]
}

/// Contexte de préparation d'un lancement.
pub struct RunCtx<'a> {
    /// Chemin de NOTRE exécutable (lancé en `--mcp` comme serveur MCP).
    pub exe: PathBuf,
    /// Noms d'outils MCP autorisés (bruts, ex. `read_file`). Injectés dans l'env du serveur MCP.
    pub allowed_tools: &'a [String],
    /// Autoriser les outils web natifs de l'agent (Claude Code : WebSearch/WebFetch).
    pub web_search: bool,
    /// Modèle (format selon l'agent : `opus`/`claude-…` ; `provider/model` OpenCode ; `gemini-…`).
    pub model: &'a str,
    /// Niveau d'effort Claude Code (`low|medium|high`). Ignoré par les autres agents.
    pub effort: &'a str,
    /// Id de session à reprendre (multi-tours), si l'agent le supporte.
    pub resume: Option<&'a str>,
    /// Diffuser la réponse token-par-token (chat) vs pas (diagnostic).
    pub streaming: bool,
    /// Brancher aussi le **MCP officiel** MineStrator (gestion) : `Some(readonly)` = ajouté avec ce
    /// mode (`readonly` ⇒ aucun outil modifiant), `None` = non branché. La clé vient du trousseau.
    pub official_mcp: Option<bool>,
}

/// Plan de lancement concret : arguments, répertoire de travail, variables d'env du processus.
pub struct RunPlan {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Dossier de la config de l'agent. **À garder vivant tant que le processus tourne** : sa
    /// destruction efface le dossier (et la clé API qu'il peut contenir).
    pub dir: RunDir,
}

/// Préfixe des dossiers éphémères d'agent dans le répertoire temporaire du système.
const RUN_DIR_PREFIX: &str = "minestrator-cli-";

/// Âge au-delà duquel un dossier d'agent orphelin (crash, arrêt brutal, anciennes versions qui ne
/// nettoyaient pas) est purgé. Largement au-dessus de la durée d'un tour d'agent ; une session
/// persistante a lu sa config au démarrage et n'en dépend plus ensuite.
const STALE_RUN_DIR: Duration = Duration::from_secs(24 * 3600);

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);
static PURGE_ONCE: Once = Once::new();

/// Dossier éphémère d'un lancement d'agent : il reçoit la config MCP de l'agent, qui contient la
/// clé API quand le MCP officiel est branché.
///
/// - Créé **neuf** (jamais réutilisé : un dossier pré-existant au même nom est refusé) et, sous
///   Unix, en `0700` : le répertoire temporaire est partagé entre utilisateurs (`/tmp`).
/// - **Effacé à la destruction** : le guard suit la durée de vie du processus de l'agent.
/// - Les orphelins (processus tué avant nettoyage, anciennes versions) sont purgés au premier
///   lancement d'agent de la session.
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    fn create() -> Result<Self, String> {
        let base = std::env::temp_dir();
        PURGE_ONCE.call_once(|| purge_stale_run_dirs(&base, SystemTime::now(), STALE_RUN_DIR));
        Self::create_in(&base)
    }

    fn create_in(base: &Path) -> Result<Self, String> {
        let mut last_err = None;
        for _ in 0..8 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let name = format!(
                "{RUN_DIR_PREFIX}{}-{}-{nanos:08x}",
                std::process::id(),
                RUN_SEQ.fetch_add(1, Ordering::Relaxed)
            );
            let path = base.join(name);
            match private_dir_builder().create(&path) {
                Ok(()) => return Ok(Self { path }),
                // Nom déjà pris (collision, ou dossier planté par un tiers) : on n'y écrit jamais.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
                Err(e) => return Err(format!("dossier temporaire de l'agent : {e}")),
            }
        }
        Err(format!(
            "dossier temporaire de l'agent : aucun nom libre ({})",
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // Typiquement Windows, si le processus n'est pas encore sorti : la purge des
            // orphelins le rattrapera.
            Err(e) => tracing::warn!(
                "dossier temporaire de l'agent non supprimé ({}) : {e}",
                self.path.display()
            ),
        }
    }
}

/// `DirBuilder` non récursif (échoue si le dossier existe), privé au propriétaire sous Unix.
fn private_dir_builder() -> std::fs::DirBuilder {
    #[allow(unused_mut)]
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b
}

/// Supprime les dossiers d'agent de `base` plus vieux que `max_age` (à la date `now`).
fn purge_stale_run_dirs(base: &Path, now: SystemTime, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(RUN_DIR_PREFIX) {
            continue;
        }
        // `symlink_metadata` : on ne suit pas un lien planté sous notre préfixe.
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                tracing::debug!("purge {} : {e}", entry.path().display());
            }
        }
    }
}

/// Écrit la config de l'agent, lisible par le seul propriétaire sous Unix (elle peut porter la
/// clé API).
fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    use std::io::Write;
    let s = serde_json::to_string_pretty(v).map_err(|e| format!("config JSON : {e}"))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .and_then(|mut f| f.write_all(s.as_bytes()))
        .map_err(|e| format!("écriture config ({}) : {e}", path.display()))
}

/// Env du serveur MCP (embarqué dans le fichier de config de l'agent) : force l'activation +
/// impose la liste blanche d'outils.
fn mcp_server_env(allowed: &[String]) -> Value {
    json!({
        MCP_FORCE_ENABLED_ENV: "1",
        MCP_ALLOWED_TOOLS_ENV: allowed.join(","),
    })
}

/// Ajoute une paire `drapeau valeur` aux arguments CLI (évite le doublon `push`/`push`).
fn push_flag(args: &mut Vec<String>, flag: &str, value: &str) {
    args.push(flag.to_string());
    args.push(value.to_string());
}

/// URL + en-tête d'auth du MCP OFFICIEL pour un agent (toolsets de gestion, `readonly` optionnel).
/// `None` si aucune clé au trousseau (on ne branche alors pas l'officiel). La clé est écrite dans le
/// fichier de config de l'agent, dans son [`RunDir`] : privé, et effacé à la fin du processus.
fn official_url_auth(readonly: bool) -> Option<(String, String)> {
    let key = crate::secrets::read_key().ok().flatten()?;
    let ro = if readonly { "&readonly=1" } else { "" };
    let url = format!(
        "{}?toolsets=core,actions,content,backups,mybox,schedules,databases,config{}",
        crate::config::OFFICIAL_MCP_URL,
        ro
    );
    Some((url, format!("Bearer {key}")))
}

/// Outils natifs d'OpenCode **refusés** dans `opencode.json` (`permission`). Indispensable : `--auto`
/// approuve toute demande qui n'est pas explicitement refusée, et une règle `deny` reste appliquée
/// en mode `--auto` (doc OpenCode, « Permissions »). `edit` couvre aussi `write` et `patch`.
/// L'agent n'a besoin que de nos outils MCP : ni shell, ni disque local, ni sous-agents.
const OPENCODE_DENIED: &[&str] = &[
    "bash",
    "edit",
    "read",
    "glob",
    "grep",
    "lsp",
    "task",
    "skill",
    "question",
    "external_directory",
];

/// Outils web natifs d'OpenCode : autorisés seulement si la recherche web est activée.
const OPENCODE_WEB: &[&str] = &["webfetch", "websearch"];

/// Outils natifs de Gemini CLI **exclus** (`tools.exclude`) : exécution, écriture, lecture du
/// disque local, mémoire globale (`save_memory` écrit dans `~/.gemini`). Noms internes de la doc
/// Gemini CLI ; `search_file_content` est l'ancien nom de `grep_search` (un nom inconnu est ignoré).
const GEMINI_EXCLUDED: &[&str] = &[
    "run_shell_command",
    "write_file",
    "replace",
    "read_file",
    "read_many_files",
    "list_directory",
    "glob",
    "grep_search",
    "search_file_content",
    "save_memory",
];

/// Outils web natifs de Gemini CLI : exclus, ou autorisés sans confirmation si la recherche web
/// est activée.
const GEMINI_WEB: &[&str] = &["web_fetch", "google_web_search"];

/// Outils natifs de Claude Code rendus disponibles (`--tools`) : aucun, sauf le web si activé.
fn claude_builtin_tools(web_search: bool) -> String {
    if web_search {
        "WebSearch,WebFetch".to_string()
    } else {
        String::new()
    }
}

/// Prépare le lancement de l'agent : écrit la config MCP adaptée et construit les arguments.
///
/// Sécurité, commune aux trois agents : l'agent lit du contenu non fiable (console où écrivent
/// les joueurs, crash-reports, fichiers de config), donc une injection de prompt est possible. Ses
/// outils **natifs** (shell, disque local) sont retirés ; seuls restent nos outils MCP (déjà bornés
/// par la liste blanche passée au serveur) et, sur option, le web.
pub fn plan_run(agent: CliAgent, ctx: &RunCtx) -> Result<RunPlan, String> {
    let dir = RunDir::create()?;
    let exe = ctx.exe.to_string_lossy().to_string();
    match agent {
        CliAgent::ClaudeCode => {
            let cfg = dir.path().join("mcp.json");
            let mut servers = serde_json::Map::new();
            servers.insert(
                "minestrator".into(),
                json!({ "type": "stdio", "command": exe, "args": ["--mcp"],
                        "env": mcp_server_env(ctx.allowed_tools) }),
            );
            let official = ctx.official_mcp.and_then(official_url_auth);
            if let Some((url, auth)) = &official {
                servers.insert(
                    "minestrator_official".into(),
                    json!({ "type": "http", "url": url, "headers": { "Authorization": auth } }),
                );
            }
            write_json(&cfg, &json!({ "mcpServers": servers }))?;
            // `--allowedTools` ne fait qu'AJOUTER des autorisations : un `Bash` autorisé dans les
            // réglages de l'utilisateur resterait utilisable. `--tools` retire les outils natifs
            // eux-mêmes, et `--strict-mcp-config` écarte les autres serveurs MCP de l'utilisateur.
            let mut args = vec![
                "-p".to_string(),
                "--mcp-config".to_string(),
                cfg.to_string_lossy().to_string(),
                "--strict-mcp-config".to_string(),
                "--tools".to_string(),
                claude_builtin_tools(ctx.web_search),
                "--allowedTools".to_string(),
                claude_allowed_tools(ctx.allowed_tools, ctx.web_search, official.is_some()),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
            ];
            if ctx.streaming {
                args.push("--include-partial-messages".to_string());
            }
            if !ctx.model.trim().is_empty() {
                push_flag(&mut args, "--model", ctx.model.trim());
            }
            push_flag(&mut args, "--effort", ctx.effort);
            if let Some(sid) = ctx.resume {
                push_flag(&mut args, "--resume", sid);
            }
            Ok(RunPlan { args, cwd: None, env: vec![], dir })
        }
        CliAgent::OpenCode => {
            // OpenCode lit `opencode.json` du répertoire projet (= CWD). `--auto` approuve les
            // appels, SAUF ceux refusés explicitement par `permission`.
            let cfg = dir.path().join("opencode.json");
            let mut mcp = serde_json::Map::new();
            mcp.insert(
                "minestrator".into(),
                json!({ "type": "local", "command": [exe, "--mcp"],
                        "environment": mcp_server_env(ctx.allowed_tools), "enabled": true }),
            );
            if let Some((url, auth)) = ctx.official_mcp.and_then(official_url_auth) {
                mcp.insert(
                    "minestrator_official".into(),
                    json!({ "type": "remote", "url": url,
                            "headers": { "Authorization": auth }, "enabled": true }),
                );
            }
            let mut permission = serde_json::Map::new();
            for tool in OPENCODE_DENIED {
                permission.insert((*tool).into(), json!("deny"));
            }
            let web = if ctx.web_search { "allow" } else { "deny" };
            for tool in OPENCODE_WEB {
                permission.insert((*tool).into(), json!(web));
            }
            write_json(
                &cfg,
                &json!({ "$schema": "https://opencode.ai/config.json",
                         "mcp": mcp, "permission": permission }),
            )?;
            let mut args = vec![
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--auto".to_string(),
            ];
            if !ctx.model.trim().is_empty() {
                push_flag(&mut args, "-m", ctx.model.trim());
            }
            if let Some(sid) = ctx.resume {
                push_flag(&mut args, "--session", sid);
            }
            let cwd = Some(dir.path().to_path_buf());
            Ok(RunPlan { args, cwd, env: vec![], dir })
        }
        CliAgent::Gemini => {
            // Gemini lit `.gemini/settings.json` du répertoire projet (= CWD). Clé `GEMINI_API_KEY`
            // attendue dans l'environnement de l'utilisateur (héritée par le sous-processus).
            // Mode d'approbation `default` (et non `yolo`) : en non-interactif, toute demande de
            // confirmation est refusée (doc Gemini CLI, « Policy engine »). Nos serveurs MCP sont
            // `trust: true` → leurs outils passent sans confirmation ; les outils natifs à risque
            // sont en plus exclus explicitement, au cas où un réglage utilisateur les autoriserait.
            let gdir = dir.path().join(".gemini");
            private_dir_builder()
                .create(&gdir)
                .map_err(|e| format!("dossier .gemini : {e}"))?;
            let mut servers = serde_json::Map::new();
            servers.insert(
                "minestrator".into(),
                json!({ "command": exe, "args": ["--mcp"],
                        "env": mcp_server_env(ctx.allowed_tools), "trust": true }),
            );
            if let Some((url, auth)) = ctx.official_mcp.and_then(official_url_auth) {
                servers.insert(
                    "minestrator_official".into(),
                    json!({ "httpUrl": url, "headers": { "Authorization": auth }, "trust": true }),
                );
            }
            let mut exclude: Vec<&str> = GEMINI_EXCLUDED.to_vec();
            let mut tools = serde_json::Map::new();
            if ctx.web_search {
                tools.insert("allowed".into(), json!(GEMINI_WEB));
            } else {
                exclude.extend_from_slice(GEMINI_WEB);
            }
            tools.insert("exclude".into(), json!(exclude));
            write_json(
                &gdir.join("settings.json"),
                &json!({ "mcpServers": servers, "tools": tools }),
            )?;
            let mut args = vec![
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--approval-mode".to_string(),
                "default".to_string(),
            ];
            if !ctx.model.trim().is_empty() {
                push_flag(&mut args, "-m", ctx.model.trim());
            }
            if let Some(sid) = ctx.resume {
                push_flag(&mut args, "-r", sid);
            }
            let cwd = Some(dir.path().to_path_buf());
            Ok(RunPlan { args, cwd, env: vec![], dir })
        }
    }
}

/// Liste `--allowedTools` de Claude Code : outils MCP préfixés + serveur officiel (tous ses outils)
/// + outils web natifs si activés.
fn claude_allowed_tools(allowed: &[String], web_search: bool, official: bool) -> String {
    let mut parts: Vec<String> = allowed
        .iter()
        .map(|t| format!("mcp__minestrator__{t}"))
        .collect();
    if official {
        // `mcp__<serveur>` (sans nom d'outil) autorise TOUS les outils de ce serveur MCP.
        parts.push("mcp__minestrator_official".to_string());
    }
    if web_search {
        parts.push("WebSearch".to_string());
        parts.push("WebFetch".to_string());
    }
    parts.join(",")
}

// --- Parsing de sortie (dispatch par agent) --------------------------------

/// Texte final de la réponse.
pub fn final_text(agent: CliAgent, ndjson: &str) -> Option<String> {
    match agent {
        CliAgent::ClaudeCode => crate::copilot::extract_stream_result(ndjson),
        CliAgent::OpenCode => opencode_final_text(ndjson),
        CliAgent::Gemini => gemini_final_text(ndjson),
    }
}

/// Id de session à réutiliser au tour suivant (`None` = pas de reprise possible → nouveau tour).
pub fn session_id(agent: CliAgent, ndjson: &str) -> Option<String> {
    match agent {
        CliAgent::ClaudeCode => crate::copilot::extract_session_id(ndjson),
        CliAgent::OpenCode => last_str_field(ndjson, "sessionID"),
        CliAgent::Gemini => gemini_session_id(ndjson),
    }
}

/// Fragment de texte en streaming pour une ligne (seul Claude Code est supporté).
pub fn text_delta(agent: CliAgent, line: &str) -> Option<String> {
    match agent {
        CliAgent::ClaudeCode => crate::copilot::stream_text_delta(line),
        _ => None,
    }
}

/// Phase d'avancement lisible pour une ligne (appels d'outils, réflexion…).
pub fn phase(agent: CliAgent, line: &str) -> Option<String> {
    match agent {
        CliAgent::ClaudeCode => crate::copilot::stream_phase(line),
        CliAgent::OpenCode | CliAgent::Gemini => generic_phase(line),
    }
}

/// Résultat final capturé sur UNE ligne de streaming, insensible à la troncature de la sortie
/// complète (Claude Code uniquement ; les autres agents retombent sur `final_text`).
pub fn result_line(agent: CliAgent, line: &str) -> Option<String> {
    match agent {
        CliAgent::ClaudeCode => crate::copilot::stream_result_line(line),
        _ => None,
    }
}

// --- OpenCode ---------------------------------------------------------------

fn opencode_final_text(ndjson: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for line in ndjson.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        // Texte tantôt sous `part.text`, tantôt sous `text` (défensif).
        let txt = v
            .pointer("/part/text")
            .or_else(|| v.get("text"))
            .and_then(|t| t.as_str());
        if let Some(t) = txt {
            if !t.trim().is_empty() {
                last = Some(t.to_string());
            }
        }
    }
    last
}

// --- Gemini -----------------------------------------------------------------

fn gemini_final_text(ndjson: &str) -> Option<String> {
    // stream-json : dernier event `result` → `response`/`result`/`text`. Repli : dernier `message`.
    let mut result_text: Option<String> = None;
    let mut last_message: Option<String> = None;
    for line in ndjson.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("result") => {
                let t = v
                    .get("response")
                    .or_else(|| v.get("result"))
                    .or_else(|| v.get("text"))
                    .and_then(|x| x.as_str());
                if let Some(t) = t {
                    result_text = Some(t.to_string());
                }
            }
            Some("message") => {
                let t = v
                    .pointer("/message/content")
                    .or_else(|| v.get("content"))
                    .or_else(|| v.get("text"))
                    .and_then(|x| x.as_str());
                if let Some(t) = t {
                    if !t.trim().is_empty() {
                        last_message = Some(t.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    result_text.or(last_message)
}

fn gemini_session_id(ndjson: &str) -> Option<String> {
    // L'id apparaît dans l'event `init` (nom de champ incertain → on tente plusieurs).
    for line in ndjson.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("init") {
            continue;
        }
        for key in ["sessionId", "session_id", "sessionID", "id"] {
            if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

// --- Helpers partagés OpenCode/Gemini --------------------------------------

/// Dernier `value` d'un champ chaîne présent sur n'importe quelle ligne NDJSON.
fn last_str_field(ndjson: &str, key: &str) -> Option<String> {
    let mut last = None;
    for line in ndjson.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
                last = Some(s.to_string());
            }
        }
    }
    last
}

/// Phase générique : un event d'appel d'outil → « Outil : <nom> ».
fn generic_phase(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        "tool_use" => {
            let name = v
                .get("tool")
                .or_else(|| v.get("name"))
                .or_else(|| v.pointer("/tool/name"))
                .and_then(|n| n.as_str())
                .unwrap_or("outil");
            Some(format!("Outil : {}", name.rsplit(['_', '/']).next().unwrap_or(name)))
        }
        "reasoning" => Some("Réflexion…".into()),
        "result" => Some("Finalisation…".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_extracts_last_text() {
        let nd = r#"{"type":"step_start","sessionID":"s1"}
{"type":"tool_use","tool":"minestrator_read_file","sessionID":"s1"}
{"type":"text","part":{"text":"Voici l'analyse."},"sessionID":"s1"}"#;
        assert_eq!(opencode_final_text(nd).as_deref(), Some("Voici l'analyse."));
        assert_eq!(last_str_field(nd, "sessionID").as_deref(), Some("s1"));
    }

    #[test]
    fn opencode_ignores_garbage_lines() {
        let nd = "pas du json\n{\"type\":\"text\",\"text\":\"ok\"}\n";
        assert_eq!(opencode_final_text(nd).as_deref(), Some("ok"));
    }

    #[test]
    fn gemini_prefers_result_event() {
        let nd = r#"{"type":"init","sessionId":"g9"}
{"type":"message","content":"intermédiaire"}
{"type":"result","response":"réponse finale"}"#;
        assert_eq!(gemini_final_text(nd).as_deref(), Some("réponse finale"));
        assert_eq!(gemini_session_id(nd).as_deref(), Some("g9"));
    }

    #[test]
    fn gemini_falls_back_to_message() {
        let nd = r#"{"type":"message","content":"seule réponse"}"#;
        assert_eq!(gemini_final_text(nd).as_deref(), Some("seule réponse"));
    }

    #[test]
    fn claude_allowlist_prefixes_and_adds_web() {
        let allowed = vec!["read_file".to_string(), "list_files".to_string()];
        let s = claude_allowed_tools(&allowed, true, false);
        assert_eq!(
            s,
            "mcp__minestrator__read_file,mcp__minestrator__list_files,WebSearch,WebFetch"
        );
        let s2 = claude_allowed_tools(&allowed, false, false);
        assert_eq!(s2, "mcp__minestrator__read_file,mcp__minestrator__list_files");
        // Officiel activé → tout le serveur `minestrator_official` est autorisé.
        let s3 = claude_allowed_tools(&allowed, false, true);
        assert_eq!(
            s3,
            "mcp__minestrator__read_file,mcp__minestrator__list_files,mcp__minestrator_official"
        );
    }

    #[test]
    fn claude_result_line_captures_final_text() {
        // Capture au fil de l'eau : une ligne `result` isolée suffit (insensible à la troncature).
        let line = r#"{"type":"result","result":"Réponse finale.","session_id":"c1"}"#;
        assert_eq!(
            result_line(CliAgent::ClaudeCode, line).as_deref(),
            Some("Réponse finale.")
        );
        // Une ligne non-`result` ou un résultat vide ne capture rien.
        assert_eq!(result_line(CliAgent::ClaudeCode, r#"{"type":"assistant"}"#), None);
        assert_eq!(result_line(CliAgent::ClaudeCode, r#"{"type":"result","result":"  "}"#), None);
        // Les autres agents n'ont pas ce mécanisme (repli sur final_text).
        assert_eq!(result_line(CliAgent::OpenCode, line), None);
    }

    #[test]
    fn claude_final_text_falls_back_to_assistant_never_raw() {
        // Sortie tronquée = PAS de ligne `result`, mais un message `assistant` présent. On doit
        // récupérer son texte — JAMAIS le JSON brut (ancien bug : le ndjson entier finissait dans le chat).
        let nd = concat!(
            r#"{"type":"system","subtype":"init","session_id":"c1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Diagnostic partiel."}]}}"#
        );
        assert_eq!(
            final_text(CliAgent::ClaudeCode, nd).as_deref(),
            Some("Diagnostic partiel.")
        );
        // La ligne `result` reste prioritaire quand elle est là.
        let full = format!("{nd}\n{}", r#"{"type":"result","result":"Diagnostic complet."}"#);
        assert_eq!(
            final_text(CliAgent::ClaudeCode, &full).as_deref(),
            Some("Diagnostic complet.")
        );
    }

    // --- Dossier éphémère & durcissement des outils natifs ------------------

    /// Contexte minimal : MCP officiel NON branché (`plan_run` ne lit alors pas le trousseau).
    fn ctx<'a>(allowed: &'a [String], web_search: bool) -> RunCtx<'a> {
        RunCtx {
            exe: PathBuf::from("minestrator-terminal"),
            allowed_tools: allowed,
            web_search,
            model: "",
            effort: "medium",
            resume: None,
            streaming: false,
            official_mcp: None,
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// Dossier de test propre, hors du répertoire temporaire partagé par les vrais lancements.
    fn test_base(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "minestrator-test-{name}-{}-{}",
            std::process::id(),
            RUN_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn run_dir_is_removed_on_drop() {
        let base = test_base("drop");
        let dir = RunDir::create_in(&base).unwrap();
        let path = dir.path().to_path_buf();
        write_json(&path.join("mcp.json"), &json!({ "secret": "Bearer x" })).unwrap();
        assert!(path.join("mcp.json").exists());
        drop(dir);
        assert!(!path.exists(), "le dossier (et la clé qu'il contient) doit disparaître");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn run_dir_names_are_unique() {
        let base = test_base("unique");
        let a = RunDir::create_in(&base).unwrap();
        let b = RunDir::create_in(&base).unwrap();
        assert_ne!(a.path(), b.path());
        drop((a, b));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn run_dir_and_config_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = test_base("perms");
        let dir = RunDir::create_in(&base).unwrap();
        let cfg = dir.path().join("mcp.json");
        write_json(&cfg, &json!({})).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&cfg), 0o600);
        drop(dir);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn purge_removes_only_stale_agent_dirs() {
        let base = test_base("purge");
        let agent = base.join(format!("{RUN_DIR_PREFIX}123-0-0"));
        let other = base.join("autre-dossier");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let max_age = Duration::from_secs(3600);
        // Récent : conservé (un autre processus peut s'en servir).
        purge_stale_run_dirs(&base, SystemTime::now(), max_age);
        assert!(agent.exists());
        // Vu depuis « demain » : périmé → purgé ; un dossier hors préfixe n'est jamais touché.
        let tomorrow = SystemTime::now() + Duration::from_secs(24 * 3600);
        purge_stale_run_dirs(&base, tomorrow, max_age);
        assert!(!agent.exists());
        assert!(other.exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn claude_code_gets_no_native_tools_nor_foreign_mcp() {
        let allowed = vec!["read_file".to_string()];
        let plan = plan_run(CliAgent::ClaudeCode, &ctx(&allowed, false)).unwrap();
        let tools = plan.args.iter().position(|a| a == "--tools").expect("--tools");
        assert_eq!(plan.args[tools + 1], "", "aucun outil natif sans recherche web");
        assert!(plan.args.iter().any(|a| a == "--strict-mcp-config"));

        let plan = plan_run(CliAgent::ClaudeCode, &ctx(&allowed, true)).unwrap();
        let tools = plan.args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(plan.args[tools + 1], "WebSearch,WebFetch");
    }

    #[test]
    fn opencode_denies_native_tools_explicitly() {
        let allowed = vec!["read_file".to_string()];
        let plan = plan_run(CliAgent::OpenCode, &ctx(&allowed, false)).unwrap();
        let cfg = read_json(&plan.dir.path().join("opencode.json"));
        for tool in ["bash", "edit", "read", "external_directory", "webfetch", "websearch"] {
            assert_eq!(cfg["permission"][tool], "deny", "{tool} doit être refusé");
        }
        assert_eq!(plan.cwd.as_deref(), Some(plan.dir.path()));

        let plan = plan_run(CliAgent::OpenCode, &ctx(&allowed, true)).unwrap();
        let cfg = read_json(&plan.dir.path().join("opencode.json"));
        assert_eq!(cfg["permission"]["webfetch"], "allow");
        assert_eq!(cfg["permission"]["bash"], "deny");
    }

    #[test]
    fn gemini_never_runs_in_yolo_and_excludes_native_tools() {
        let allowed = vec!["read_file".to_string()];
        let plan = plan_run(CliAgent::Gemini, &ctx(&allowed, false)).unwrap();
        assert!(!plan.args.iter().any(|a| a == "yolo" || a == "--yolo" || a == "-y"));
        let mode = plan.args.iter().position(|a| a == "--approval-mode").unwrap();
        assert_eq!(plan.args[mode + 1], "default");

        let cfg = read_json(&plan.dir.path().join(".gemini").join("settings.json"));
        let excluded: Vec<&str> = cfg["tools"]["exclude"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for tool in ["run_shell_command", "write_file", "replace", "read_file", "web_fetch"] {
            assert!(excluded.contains(&tool), "{tool} doit être exclu");
        }
        assert_eq!(cfg["mcpServers"]["minestrator"]["trust"], true);

        let plan = plan_run(CliAgent::Gemini, &ctx(&allowed, true)).unwrap();
        let cfg = read_json(&plan.dir.path().join(".gemini").join("settings.json"));
        assert!(!cfg["tools"]["exclude"].as_array().unwrap().contains(&json!("web_fetch")));
        assert_eq!(cfg["tools"]["allowed"], json!(GEMINI_WEB));
    }

    #[test]
    fn plan_dir_disappears_with_the_plan() {
        let allowed = vec![];
        let plan = plan_run(CliAgent::Gemini, &ctx(&allowed, false)).unwrap();
        let path = plan.dir.path().to_path_buf();
        assert!(path.join(".gemini").join("settings.json").exists());
        drop(plan);
        assert!(!path.exists());
    }
}
