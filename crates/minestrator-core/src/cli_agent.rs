//! Adaptateurs d'**agents CLI** (Claude Code, OpenCode, Gemini CLI) — pendant CLI de la couche
//! multi-LLM HTTP (`llm.rs`). Chaque agent a ses propres flags, son format de config MCP et son
//! schéma de sortie. On isole ces différences ici pour que `copilot.rs` reste agnostique.
//!
//! Sécurité : notre serveur MCP est lancé avec la liste blanche d'outils dans son env
//! (`MCP_ALLOWED_TOOLS_ENV`) → un agent en mode suggestion ne PEUT PAS appeler un outil d'écriture
//! MCP (l'outil n'existe même pas côté serveur). Claude Code est en plus borné par `--allowedTools`.
//!
//! ⚠ LIMITE : cette liste blanche ne borne QUE notre serveur MCP, **pas** les outils NATIFS de
//! l'agent (shell, lecture/écriture fichier local, web). Claude Code restreint ses outils natifs via
//! `--allowedTools`, mais OpenCode (`--auto`) et Gemini (`--approval-mode yolo`) sont lancés en
//! auto-approbation TOTALE de leurs outils natifs → un contenu injecté que l'agent lit (console d'un
//! joueur, crash-report, config) peut lui faire exécuter une commande shell sur la machine de l'admin.
//! Claude Code (le défaut) n'est PAS concerné. Fix propre = restreindre leurs outils natifs via leur
//! config (schéma à VÉRIFIER avant de l'écrire — ne pas deviner) ; en attendant, préférer Claude Code.
//!
//! Robustesse : le parsing Claude Code (éprouvé) reste dans `copilot.rs` et est réutilisé tel
//! quel ; OpenCode/Gemini ont un parsing **défensif** (chaque champ optionnel, dégradation propre).

use crate::config::{MCP_ALLOWED_TOOLS_ENV, MCP_FORCE_ENABLED_ENV};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Agent CLI sélectionné. Le binaire concret vient de `cli_command` (surchargeable) ; l'enum
/// détermine flags, config MCP et parsing.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
}

/// Plan de lancement concret : arguments, répertoire de travail, variables d'env du processus.
pub struct RunPlan {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Répertoire temporaire unique par lancement (pour y déposer la config de l'agent sans polluer
/// la config globale de l'utilisateur).
fn unique_temp_dir() -> Result<PathBuf, String> {
    let n = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("minestrator-cli-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).map_err(|e| format!("dossier temporaire : {e}"))?;
    Ok(dir)
}

fn write_json(path: &std::path::Path, v: &Value) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| format!("config JSON : {e}"))?;
    std::fs::write(path, s).map_err(|e| format!("écriture config ({}) : {e}", path.display()))
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

/// Prépare le lancement de l'agent : écrit la config MCP adaptée et construit les arguments.
pub fn plan_run(agent: CliAgent, ctx: &RunCtx) -> Result<RunPlan, String> {
    let dir = unique_temp_dir()?;
    let exe = ctx.exe.to_string_lossy().to_string();
    match agent {
        CliAgent::ClaudeCode => {
            let cfg = dir.join("mcp.json");
            write_json(
                &cfg,
                &json!({ "mcpServers": { "minestrator": {
                    "type": "stdio", "command": exe, "args": ["--mcp"],
                    "env": mcp_server_env(ctx.allowed_tools),
                }}}),
            )?;
            let mut args = vec![
                "-p".to_string(),
                "--mcp-config".to_string(),
                cfg.to_string_lossy().to_string(),
                "--allowedTools".to_string(),
                claude_allowed_tools(ctx.allowed_tools, ctx.web_search),
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
            Ok(RunPlan { args, cwd: None, env: vec![] })
        }
        CliAgent::OpenCode => {
            // OpenCode lit `opencode.json` du répertoire projet (= CWD). Sécurité via la liste
            // blanche d'env côté serveur MCP + `--auto` (auto-approuve les appels).
            let cfg = dir.join("opencode.json");
            write_json(
                &cfg,
                &json!({
                    "$schema": "https://opencode.ai/config.json",
                    "mcp": { "minestrator": {
                        "type": "local", "command": [exe, "--mcp"],
                        "environment": mcp_server_env(ctx.allowed_tools), "enabled": true,
                    }}
                }),
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
            Ok(RunPlan { args, cwd: Some(dir), env: vec![] })
        }
        CliAgent::Gemini => {
            // Gemini lit `.gemini/settings.json` du répertoire projet (= CWD). `trust:true` débloque
            // les outils MCP en headless ; `--approval-mode yolo` auto-approuve. Clé `GEMINI_API_KEY`
            // attendue dans l'environnement de l'utilisateur (héritée par le sous-processus).
            let gdir = dir.join(".gemini");
            std::fs::create_dir_all(&gdir).map_err(|e| format!("dossier .gemini : {e}"))?;
            write_json(
                &gdir.join("settings.json"),
                &json!({ "mcpServers": { "minestrator": {
                    "command": exe, "args": ["--mcp"],
                    "env": mcp_server_env(ctx.allowed_tools), "trust": true,
                }}}),
            )?;
            let mut args = vec![
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--approval-mode".to_string(),
                "yolo".to_string(),
            ];
            if !ctx.model.trim().is_empty() {
                push_flag(&mut args, "-m", ctx.model.trim());
            }
            if let Some(sid) = ctx.resume {
                push_flag(&mut args, "-r", sid);
            }
            Ok(RunPlan { args, cwd: Some(dir), env: vec![] })
        }
    }
}

/// Liste `--allowedTools` de Claude Code : outils MCP préfixés + outils web natifs si activés.
fn claude_allowed_tools(allowed: &[String], web_search: bool) -> String {
    let mut parts: Vec<String> = allowed
        .iter()
        .map(|t| format!("mcp__minestrator__{t}"))
        .collect();
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
        let s = claude_allowed_tools(&allowed, true);
        assert_eq!(
            s,
            "mcp__minestrator__read_file,mcp__minestrator__list_files,WebSearch,WebFetch"
        );
        let s2 = claude_allowed_tools(&allowed, false);
        assert_eq!(s2, "mcp__minestrator__read_file,mcp__minestrator__list_files");
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
}
