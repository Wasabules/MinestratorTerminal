//! Copilote : diagnostic automatique par un **agent LLM** (Claude, GPT, Gemini, local…),
//! déclenché par les alertes du superviseur (crash / seuils). **Indépendant de l'UI et du
//! fournisseur LLM.**
//!
//! Architecture :
//! - le déclencheur est un [`crate::events::Alert`] déjà émis par le superviseur ;
//! - le diagnostic réutilise la **couche d'outils MCP** ([`crate::mcp::tool_list`] +
//!   [`crate::mcp::dispatch`]) — le Copilote agit comme un client MCP interne ;
//! - le modèle est atteint via la couche **multi-fournisseur** [`crate::llm`] ;
//! - l'agent ne dispose que des outils de **lecture** ; il *propose* les actions modifiantes,
//!   dont l'exécution dépend du niveau d'[`Autonomy`] choisi.

use crate::events::{
    ChatDelta, CopilotProgress, CopilotStarted, CoreEvent, Diagnosis, ProposedAction,
};
use crate::llm::{LlmClient, Msg, Provider, ToolResult, ToolSpec};
use crate::mcp::{is_write_tool, READ_TOOLS, WRITE_TOOLS};
use crate::store::now_secs;
use crate::util::tail;
use crate::{cli, mcp, secrets, Core};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Compteur de séquence pour des identifiants de diagnostic uniques dans le process.
static SEQ: AtomicU64 = AtomicU64::new(1);

const MAX_TURNS: usize = 8;
const DIAGNOSE_COOLDOWN_S: i64 = 300;
/// Cooldown DÉDIÉ aux analyses de performance sur surcharge prolongée : bien plus long que pour un
/// crash/incident. Une surcharge (« ça rame ») peut durer des heures ; sans ce garde-fou, on
/// rejouerait — et paierait — une analyse Spark + LLM toutes les ~5 min. Un crash, lui, reste
/// réactif via `DIAGNOSE_COOLDOWN_S`.
const PERF_COOLDOWN_S: i64 = 1800; // 30 min entre deux analyses de perf d'un même serveur
const CONSOLE_TAIL: usize = 60;
const TOOL_OUTPUT_CAP: usize = 6000;
/// Contexte console (plus large) pour l'agent CLI, qui reçoit tout en un seul prompt.
const CLI_CONSOLE_TAIL: usize = 90;
const CLI_TIMEOUT_S: u64 = 150;
/// Mode agent : plus long (démarrage du serveur MCP + boucle d'appels d'outils).
const CLI_AGENTIC_TIMEOUT_S: u64 = 300;

/// Plafond de diagnostics/analyses agentiques CONCURRENTS déclenchés par le superviseur (chacun =
/// process Node + serveur MCP lourd) : une rafale de serveurs en panne simultanée ne doit pas
/// spawner N process d'un coup. Les tâches au-delà du plafond attendent leur tour.
static DIAG_LIMIT: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(3));

/// Une action proposée est-elle RÉELLEMENT sans perte, donc auto-applicable en mode AutoSafe sans
/// clic ? Décision codée en dur (jamais le `risk` du modèle) : seuls des outils additifs/réversibles,
/// avec garde sur les arguments (jamais `stop`/`kill` en automatique).
fn is_auto_safe(act: &ProposedAction) -> bool {
    match act.tool.as_str() {
        "create_snapshot" => true,
        "power_action" => matches!(
            act.args.get("action").and_then(|v| v.as_str()).unwrap_or(""),
            "start" | "restart" | "restart10"
        ),
        _ => false,
    }
}

/// Un outil est-il interdit par la politique de permissions ?
fn tool_disabled(name: &str, disabled: &[String]) -> bool {
    disabled.iter().any(|d| d == name)
}

/// Noms d'outils MCP autorisés pour l'agent CLI : lecture toujours, + écriture si `autonomous`,
/// moins ceux interdits par la politique. Sert de liste blanche côté serveur MCP (garde-fou
/// uniforme) et, pour Claude Code, de base à `--allowedTools`.
fn allowed_tool_names(cfg: &CopilotConfig, autonomous: bool) -> Vec<String> {
    let mut v: Vec<String> = READ_TOOLS.iter().map(|s| s.to_string()).collect();
    if autonomous {
        v.extend(WRITE_TOOLS.iter().map(|s| s.to_string()));
    }
    v.retain(|n| !tool_disabled(n, &cfg.disabled_tools));
    v
}

/// Binaire à lancer : `cli_command` s'il est renseigné, sinon le binaire par défaut de l'agent.
fn cli_command(cfg: &CopilotConfig) -> &str {
    let c = cfg.cli_command.trim();
    if c.is_empty() {
        cfg.cli_agent.default_command()
    } else {
        c
    }
}

/// Enum JSON des outils d'action (pour le schéma `report_diagnosis`), dérivé de la source unique.
fn action_enum() -> Value {
    Value::Array(WRITE_TOOLS.iter().map(|n| json!(n)).collect())
}

/// Liste lisible des outils MCP de lecture (pour les prompts), dérivée de la source unique.
fn read_tools_mcp_list() -> String {
    READ_TOOLS
        .iter()
        .map(|n| format!("mcp__minestrator__{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lit la clé LLM du fournisseur courant ; erreur amicale si requise et absente.
fn resolve_key(cfg: &CopilotConfig) -> Result<String, String> {
    let key = secrets::read_llm_key(cfg.provider.slug())
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if key.is_empty() && cfg.provider.requires_key() {
        return Err(format!(
            "Aucune clé API {} configurée (Réglages → Copilote).",
            cfg.provider.slug()
        ));
    }
    Ok(key)
}

/// Niveau d'autonomie du Copilote (choisi dans les réglages).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Diagnostique et propose, n'exécute jamais.
    #[default]
    SuggestOnly,
    /// Propose ; l'exécution demande une validation explicite (bouton dans l'UI).
    ApplyOnConfirm,
    /// Applique automatiquement les actions marquées « safe » ; le reste attend validation.
    AutoSafe,
}

/// Effort de raisonnement demandé au modèle. Mappé par fournisseur : niveau natif `--effort`
/// pour Claude Code (CLI), indice de prompt pour l'API HTTP.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
}

impl Effort {
    /// Niveau pour le flag `--effort` de Claude Code.
    fn claude_level(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }
    /// Indice de prompt pour les fournisseurs HTTP (pas de contrôle natif universel).
    fn hint(self) -> &'static str {
        match self {
            Effort::Low => "Sois bref et direct.",
            Effort::Medium => "",
            Effort::High => "Prends le temps de raisonner étape par étape avant de répondre.",
        }
    }
}

/// Applique l'indice d'effort à un prompt système (fournisseurs HTTP).
fn with_effort(system: &str, effort: Effort) -> String {
    let hint = effort.hint();
    if hint.is_empty() {
        system.to_string()
    } else {
        format!("{system}\n{hint}")
    }
}

/// Réglages du Copilote (persistés dans `copilot.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotConfig {
    pub enabled: bool,
    pub provider: Provider,
    /// URL de base LLM ; vide = valeur par défaut du fournisseur.
    pub base_url: String,
    pub model: String,
    /// Commande de l'agent CLI local (fournisseur `LocalCli`), ex. `claude`.
    pub cli_command: String,
    /// Arguments de la commande CLI (le prompt est passé sur stdin), ex. `["-p"]`.
    pub cli_args: Vec<String>,
    /// Mode agent : branche notre serveur MCP sur la CLI (Claude Code) pour qu'elle lise
    /// fichiers/console/métriques elle-même et croise, au lieu d'un prompt pré-rempli.
    pub cli_agentic: bool,
    pub autonomy: Autonomy,
    pub on_crash: bool,
    pub on_threshold: bool,
    /// Déclenche un diagnostic sur une ligne console classée ERROR (via le monitor).
    pub on_error: bool,
    /// Déclenche un diagnostic sur une ligne console classée WARN (plus bruyant).
    pub on_warn: bool,
    /// Sur **surcharge prolongée** CPU/RAM (pas un simple pic), lance une **analyse de
    /// performance Spark** : rapports Spark + profiler, puis suggestions.
    pub perf_on_overload: bool,
    /// Seuil (% CPU ou RAM) à partir duquel on considère le serveur « surchargé ».
    pub perf_overload_pct: f64,
    /// Durée minimale (minutes) de surcharge CONTINUE avant de déclencher (évite les pics).
    pub perf_overload_minutes: u32,
    pub disabled_servers: Vec<i64>,
    /// Autorise l'agent à faire des **recherches web** (mode CLI/Claude Code : outils
    /// WebSearch/WebFetch). Sans effet en mode API HTTP (pas d'outil web). Défaut : activé.
    #[serde(default = "default_true")]
    pub web_search: bool,
    /// Outils **interdits** à l'IA (Copilote/Assistant) — noms MCP sans préfixe (ex.
    /// `power_action`, `delete_path`). Vide = tous autorisés (selon lecture/écriture + autonomie).
    /// Permet un contrôle FIN des permissions quelle que soit la source (API HTTP ou CLI).
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Effort de raisonnement demandé au modèle.
    #[serde(default)]
    pub effort: Effort,
    /// Agent CLI utilisé quand `provider = LocalCli` (Claude Code / OpenCode / Gemini).
    #[serde(default)]
    pub cli_agent: crate::cli_agent::CliAgent,
}

fn default_true() -> bool {
    true
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: Provider::Anthropic,
            base_url: String::new(),
            model: crate::config::COPILOT_DEFAULT_MODEL.to_string(),
            cli_command: "claude".to_string(),
            cli_args: vec!["-p".to_string()],
            cli_agentic: true,
            autonomy: Autonomy::SuggestOnly,
            on_crash: true,
            on_threshold: false,
            on_error: false,
            on_warn: false,
            perf_on_overload: false,
            perf_overload_pct: 85.0,
            perf_overload_minutes: 3,
            disabled_servers: Vec::new(),
            web_search: true,
            disabled_tools: Vec::new(),
            effort: Effort::Medium,
            cli_agent: crate::cli_agent::CliAgent::default(),
        }
    }
}

/// Incident déclencheur d'un diagnostic.
pub struct Incident {
    pub server_id: i64,
    pub server_name: String,
    /// `crash` | `cpu` | `ram` | `disk` | `error` | `warn` | `manual` | `selection`
    pub trigger: String,
    /// `warning` | `critical`
    pub severity: String,
    pub message: String,
    /// Texte sélectionné par l'utilisateur à analyser (clic droit → Copilote), sinon `None`.
    pub selection: Option<String>,
}

/// Abonne le Copilote aux alertes du superviseur. Appeler avec un `Arc<Core>`.
pub fn spawn(core: Arc<Core>) {
    let mut rx = core.subscribe();
    tokio::spawn(async move {
        // Cooldowns séparés : les incidents (crash/seuil) et les logs (error/warn) ne se
        // privent pas mutuellement d'un diagnostic.
        let mut incident_cd: HashMap<i64, i64> = HashMap::new();
        let mut log_cd: HashMap<i64, i64> = HashMap::new();
        let mut perf_cd: HashMap<i64, i64> = HashMap::new();
        // Instant (secs) depuis lequel un serveur est en surcharge CONTINUE (None = pas surchargé).
        let mut overload_since: HashMap<i64, i64> = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(CoreEvent::ConsoleStats(s)) => {
                    // Suivi de surcharge PROLONGÉE (via le flux monitor du superviseur).
                    let Some(id) = s.conn_id.strip_prefix("mon:").and_then(|x| x.parse::<i64>().ok())
                    else {
                        continue;
                    };
                    // Lecture empruntante : pas de clone de toute la config à chaque tick de stats
                    // (arrive ~1/s par serveur monitoré) ; on n'extrait que les seuils si actif.
                    let Some((overload_pct, overload_minutes)) = core.copilot_config_with(|c| {
                        (c.enabled && c.perf_on_overload && !c.disabled_servers.contains(&id))
                            .then_some((c.perf_overload_pct, c.perf_overload_minutes))
                    }) else {
                        overload_since.remove(&id);
                        continue;
                    };
                    let ram = if s.memory_limit_bytes > 0 {
                        s.memory_bytes as f64 / s.memory_limit_bytes as f64 * 100.0
                    } else {
                        0.0
                    };
                    let cpu = core
                        .server_limits(id)
                        .filter(|(lim, _)| *lim > 0)
                        .map(|(lim, _)| s.cpu_absolute / lim as f64 * 100.0)
                        .unwrap_or(0.0);

                    if cpu >= overload_pct || ram >= overload_pct {
                        let now = now_secs();
                        let since = *overload_since.entry(id).or_insert(now);
                        if now - since >= overload_minutes as i64 * 60
                            && cooled(&mut perf_cd, id, PERF_COOLDOWN_S)
                        {
                            overload_since.remove(&id);
                            tracing::info!(server = id, "copilote : surcharge prolongée → analyse de performance");
                            let core2 = core.clone();
                            let name = core.server_name(id);
                            tokio::spawn(async move {
                                let Ok(_permit) = DIAG_LIMIT.acquire().await else { return };
                                run_performance(&core2, id, name, true).await;
                            });
                        }
                    } else {
                        overload_since.remove(&id);
                    }
                }
                Ok(CoreEvent::Alert(a)) => {
                    let cfg = core.get_copilot_config();
                    if !cfg.enabled || cfg.disabled_servers.contains(&a.server_id) {
                        continue;
                    }
                    let want = match a.kind.as_str() {
                        "crash" => cfg.on_crash,
                        "cpu" | "ram" | "disk" => cfg.on_threshold,
                        _ => false,
                    };
                    if !want || !cooled(&mut incident_cd, a.server_id, DIAGNOSE_COOLDOWN_S) {
                        continue;
                    }
                    let incident = Incident {
                        server_id: a.server_id,
                        server_name: a.server_name.clone(),
                        trigger: a.kind.clone(),
                        severity: a.severity.clone(),
                        message: a.message.clone(),
                        selection: None,
                    };
                    let core2 = core.clone();
                    tokio::spawn(async move {
                        let Ok(_permit) = DIAG_LIMIT.acquire().await else { return };
                        run(&core2, incident, false).await;
                    });
                }
                Ok(CoreEvent::ConsoleLog(l)) => {
                    let Some((on_error, on_warn)) = core.copilot_config_with(|c| {
                        (c.enabled && !c.disabled_servers.contains(&l.server_id))
                            .then_some((c.on_error, c.on_warn))
                    }) else {
                        continue;
                    };
                    let want = match l.level.as_str() {
                        "error" => on_error,
                        "warn" => on_warn,
                        _ => false,
                    };
                    if !want || !cooled(&mut log_cd, l.server_id, DIAGNOSE_COOLDOWN_S) {
                        continue;
                    }
                    tracing::info!(server = l.server_id, level = %l.level, "copilote : diagnostic déclenché par une ligne console");
                    let severity = if l.level == "error" { "critical" } else { "warning" };
                    let incident = Incident {
                        server_id: l.server_id,
                        server_name: core.server_name(l.server_id),
                        trigger: l.level.clone(),
                        severity: severity.into(),
                        message: l.line.clone(),
                        selection: None,
                    };
                    let core2 = core.clone();
                    tokio::spawn(async move {
                        let Ok(_permit) = DIAG_LIMIT.acquire().await else { return };
                        run(&core2, incident, false).await;
                    });
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Vrai si le serveur n'est pas en cooldown (et enregistre l'instant).
fn cooled(map: &mut HashMap<i64, i64>, server_id: i64, cooldown_s: i64) -> bool {
    let now = now_secs();
    if map.get(&server_id).is_some_and(|t| now - t < cooldown_s) {
        return false;
    }
    map.insert(server_id, now);
    true
}

/// Diagnostique un incident, applique les actions auto-safe le cas échéant, puis émet un
/// [`CoreEvent::Diagnosis`]. Émet aussi en cas d'échec (pour informer l'UI).
///
/// `force` : ignore le gate `enabled` (déclenchements manuels — bouton Tester, clic droit).
pub async fn run(core: &Core, inc: Incident, force: bool) {
    let cfg = core.get_copilot_config();
    if !cfg.enabled && !force {
        return;
    }
    let id = next_id(inc.server_id);
    emit_started(core, &id, &inc);
    finish(core, inc, &cfg, id).await;
}

/// Identifiant de diagnostic unique dans le process.
fn next_id(server_id: i64) -> String {
    format!("{}-{}", server_id, SEQ.fetch_add(1, Ordering::Relaxed))
}

fn emit_started(core: &Core, id: &str, inc: &Incident) {
    core.emit(CoreEvent::CopilotStarted(CopilotStarted {
        id: id.to_string(),
        server_id: inc.server_id,
        server_name: inc.server_name.clone(),
        trigger: inc.trigger.clone(),
    }));
}

/// Diagnostique (après émission du « démarré »), applique l'auto-safe, émet le rapport.
async fn finish(core: &Core, inc: Incident, cfg: &CopilotConfig, id: String) {
    core.emit(CoreEvent::CopilotProgress(CopilotProgress {
        id: id.clone(),
        phase: "Analyse par le Copilote…".into(),
    }));
    let mut diag = match diagnose(core, &inc, cfg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(server = inc.server_id, "copilote : diagnostic échoué : {e}");
            let hint = match cfg.provider {
                Provider::LocalCli => "Vérifie que la commande CLI est installée et connectée (ex. « claude login »). En cas de blocage/timeout en « Mode agent », désactive-le (Réglages → Copilote) pour le mode un-coup, plus rapide.",
                _ => "Vérifie la clé API, le modèle et l'URL de base dans Réglages → Copilote.",
            };
            error_diagnosis(&inc, &e, hint)
        }
    };
    diag.id = id;

    if matches!(cfg.autonomy, Autonomy::AutoSafe) {
        for act in &diag.actions {
            // Sécurité : on n'auto-exécute QUE des outils d'une liste blanche codée en dur — jamais
            // sur la seule foi du `risk` fourni par le modèle (qui pourrait mal étiqueter « safe »
            // une action destructive, par erreur de classification ou injection).
            if act.risk == "safe" && is_auto_safe(act) {
                let args = prepare_action_args(inc.server_id, &act.tool, act.args.clone());
                match mcp::dispatch(core, &act.tool, args).await {
                    Ok(_) => tracing::info!("copilote : action sûre appliquée — {}", act.label),
                    Err(e) => tracing::warn!("copilote : échec application « {} » : {e}", act.label),
                }
            }
        }
    }
    diag.summary = decorate_summary(cfg, &diag);
    core.emit(CoreEvent::Diagnosis(diag));
}

/// Analyse de performance : collecte les rapports Spark (~35 s avec profiler), puis diagnostic.
/// L'event « démarré » est émis AVANT la collecte pour que l'indicateur tourne pendant l'attente.
pub(crate) async fn run_performance(
    core: &Core,
    server_id: i64,
    server_name: String,
    with_profiler: bool,
) {
    let cfg = core.get_copilot_config();
    let id = next_id(server_id);
    core.emit(CoreEvent::CopilotStarted(CopilotStarted {
        id: id.clone(),
        server_id,
        server_name: server_name.clone(),
        trigger: "performance".into(),
    }));

    let spark = crate::perf::collect_spark(core, &id, server_id, with_profiler).await;
    let inc = Incident {
        server_id,
        server_name,
        trigger: "performance".into(),
        severity: "warning".into(),
        message: "Analyse de performance (Spark).".into(),
        // Le contexte Spark (inclut des lignes console) part vers le LLM → anonymise si activé.
        selection: Some(core.redact_ai(&spark)),
    };
    finish(core, inc, &cfg, id).await;
}

/// Aiguillage : agent CLI local (un seul prompt) vs. boucle HTTP multi-tours.
async fn diagnose(
    core: &Core,
    inc: &Incident,
    cfg: &CopilotConfig,
    id: &str,
) -> Result<Diagnosis, String> {
    if cfg.provider == Provider::LocalCli {
        return diagnose_cli(core, inc, cfg, id).await;
    }
    diagnose_http(core, inc, cfg, id).await
}

/// Agent CLI local (Claude Code…) : deux modes.
/// - **agent** (`cli_agentic`) : on branche notre serveur MCP → la CLI lit console/métriques/
///   fichiers elle-même et croise, puis rend le rapport.
/// - **un coup** : contexte pré-collecté dans un seul prompt.
async fn diagnose_cli(
    core: &Core,
    inc: &Incident,
    cfg: &CopilotConfig,
    id: &str,
) -> Result<Diagnosis, String> {
    let out = if cfg.cli_agentic {
        run_cli_agentic(core, cfg, inc, id).await?
    } else {
        run_cli_oneshot(core, cfg, inc).await?
    };

    match extract_report(&out) {
        Some(v) if v.get("summary").is_some() => Ok(build_diagnosis(inc, v)),
        _ => Ok(text_diagnosis(inc, out.trim())),
    }
}

/// Mode un coup : pas de MCP, tout le contexte est pré-collecté dans le prompt.
async fn run_cli_oneshot(core: &Core, cfg: &CopilotConfig, inc: &Incident) -> Result<String, String> {
    let console_tail = core
        .console_logs(inc.server_id)
        .await
        .map(|l| tail(&l, CLI_CONSOLE_TAIL))
        .unwrap_or_default();
    let console_tail = core.redact_ai(&console_tail);
    let metrics = mcp::dispatch(
        core,
        "server_metrics",
        json!({ "server_id": inc.server_id, "since_secs": 3600 }),
    )
    .await
    .unwrap_or_else(|_| "(métriques indisponibles)".into());

    let prompt = cli_prompt(inc, &console_tail, &metrics);
    cli::run(&cfg.cli_command, &cfg.cli_args, &prompt, CLI_TIMEOUT_S).await
}

/// Mode agent : Claude Code branché sur notre MCP (lecture seule), prompt sur stdin.
/// Args tenus « simples » (fichier + listes) pour rester robustes au passage par `cmd /c` sous
/// Windows ; le prompt (multi-ligne) passe par stdin.
async fn run_cli_agentic(
    core: &Core,
    cfg: &CopilotConfig,
    inc: &Incident,
    id: &str,
) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("chemin de l'exécutable : {e}"))?;
    let agent = cfg.cli_agent;
    // Diagnostic = LECTURE SEULE (non autonome), pas de streaming de réponse (phases seulement).
    let allowed = allowed_tool_names(cfg, false);
    let plan = crate::cli_agent::plan_run(
        agent,
        &crate::cli_agent::RunCtx {
            exe,
            allowed_tools: &allowed,
            web_search: cfg.web_search,
            model: &cfg.model,
            effort: cfg.effort.claude_level(),
            resume: None,
            streaming: false,
        },
    )?;
    let prompt = cli_agentic_prompt(inc);
    let mut last_phase = String::new();
    let mut final_result: Option<String> = None;
    let ndjson = cli::run_streaming(
        cli_command(cfg),
        &plan.args,
        &prompt,
        CLI_AGENTIC_TIMEOUT_S,
        plan.cwd.as_deref(),
        &plan.env,
        |line| {
            if let Some(phase) = crate::cli_agent::phase(agent, line) {
                if phase != last_phase {
                    last_phase = phase.clone();
                    core.emit(CoreEvent::CopilotProgress(CopilotProgress {
                        id: id.to_string(),
                        phase,
                    }));
                }
            } else if let Some(r) = crate::cli_agent::result_line(agent, line) {
                // `else if` : une ligne NDJSON a un seul `type` (phase XOR result) → un seul parse.
                final_result = Some(r);
            }
        },
    )
    .await?;
    // Résultat capturé au fil de l'eau en priorité ; repli sur le ndjson ; jamais de JSON brut.
    Ok(final_result
        .or_else(|| crate::cli_agent::final_text(agent, &ndjson))
        .unwrap_or_default())
}

/// Extrait un fragment de texte de réponse d'une ligne `stream_event`/`text_delta` de Claude Code
/// (nécessite `--include-partial-messages`). Sert au streaming de la réponse dans le chat.
pub(crate) fn stream_text_delta(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "stream_event" {
        return None;
    }
    let delta = v.pointer("/event/delta")?;
    if delta.get("type")?.as_str()? != "text_delta" {
        return None;
    }
    let text = delta.get("text")?.as_str()?;
    (!text.is_empty()).then(|| text.to_string())
}

pub(crate) fn stream_phase(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        // Claude Code émet beaucoup d'events `system` : on ne garde que l'init réelle.
        "system" => match v.get("subtype").and_then(|s| s.as_str()) {
            Some("init") => Some("Initialisation de l'agent…".into()),
            _ => None,
        },
        "result" => Some("Finalisation…".into()),
        "assistant" => {
            let content = v.pointer("/message/content")?.as_array()?;
            for block in content {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("outil");
                        return Some(friendly_tool(name, block.get("input")));
                    }
                    Some("text") => {
                        let txt = block.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                        if !txt.is_empty() {
                            return Some(snippet(txt, 90));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

/// Décrit un appel d'outil de façon lisible (accepte les noms MCP `mcp__minestrator__…`).
fn friendly_tool(name: &str, input: Option<&Value>) -> String {
    let short = name.rsplit("__").next().unwrap_or(name);
    let path = input
        .and_then(|i| i.get("path"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    match short {
        "read_file" => format!("Lecture : {}", if path.is_empty() { "fichier" } else { path }),
        "list_files" => format!("Dossier : {}", if path.is_empty() { "/" } else { path }),
        "read_console" => "Lecture de la console".into(),
        "server_metrics" => "Analyse des métriques".into(),
        "server_status" => "État du serveur".into(),
        "list_servers" => "Liste des serveurs".into(),
        "read_startup" => "Commande de démarrage".into(),
        other => format!("Outil : {other}"),
    }
}

/// Extrait le texte final (`result`) du flux NDJSON `stream-json`.
pub(crate) fn extract_stream_result(ndjson: &str) -> Option<String> {
    // 1) Chemin nominal : la ligne finale `type:"result"` porte le texte complet.
    for line in ndjson.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("result") {
            if let Some(r) = v.get("result").and_then(|r| r.as_str()) {
                let r = r.trim();
                if !r.is_empty() {
                    return Some(r.to_string());
                }
            }
        }
    }
    // 2) Repli si la ligne `result` manque (sortie tronquée / run interrompu) : on reconstruit le
    //    texte du DERNIER message `assistant`. On ne renvoie JAMAIS le JSON brut à l'appelant.
    for line in ndjson.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        let text = blocks
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<String>();
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

/// Texte final capturé sur UNE ligne `type:"result"` de streaming. Permet de capturer le résultat
/// AU FIL DE L'EAU dans le callback, AVANT toute troncature de la sortie complète : une longue
/// session agentique dépasse la limite et perdrait sinon sa ligne `result` finale (→ dump brut).
pub(crate) fn stream_result_line(line: &str) -> Option<String> {
    let v = serde_json::from_str::<Value>(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("result") {
        return None;
    }
    let r = v.get("result").and_then(|r| r.as_str())?.trim();
    (!r.is_empty()).then(|| r.to_string())
}

/// `session_id` capturé sur UNE ligne (présent dès l'init du stream). Sert au process persistant à
/// mémoriser la session pour une reprise `--resume` après un éventuel crash.
pub(crate) fn stream_session_line(line: &str) -> Option<String> {
    let v = serde_json::from_str::<Value>(line).ok()?;
    v.get("session_id").and_then(|s| s.as_str()).map(str::to_string)
}

fn snippet(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let cut: String = one.chars().take(max).collect();
    format!("{cut}…")
}

/// Boucle agentique HTTP : le modèle interroge les outils de lecture, puis livre `report_diagnosis`.
async fn diagnose_http(
    core: &Core,
    inc: &Incident,
    cfg: &CopilotConfig,
    id: &str,
) -> Result<Diagnosis, String> {
    let key = resolve_key(cfg)?;

    let console_tail = core
        .console_logs(inc.server_id)
        .await
        .map(|l| tail(&l, CONSOLE_TAIL))
        .unwrap_or_default();
    let console_tail = core.redact_ai(&console_tail);

    let client = LlmClient::new(cfg.provider, &cfg.base_url, &key, &cfg.model);
    let tools = agent_tools(&cfg.disabled_tools);
    let system = with_effort(SYSTEM_PROMPT, cfg.effort);
    let mut transcript = vec![Msg::User(user_prompt(inc, &console_tail))];

    for _turn in 0..MAX_TURNS {
        let resp = client.complete(&system, &tools, &transcript).await?;

        if resp.calls.is_empty() {
            // Le modèle a répondu en texte sans appeler report_diagnosis → on prend ce texte.
            return Ok(text_diagnosis(inc, &resp.text));
        }

        let mut results = Vec::new();
        let mut report: Option<Value> = None;
        for call in &resp.calls {
            if call.name == "report_diagnosis" {
                report = Some(call.input.clone());
                results.push(ToolResult {
                    id: call.id.clone(),
                    content: "Diagnostic enregistré.".into(),
                });
            } else if READ_TOOLS.contains(&call.name.as_str()) {
                core.emit(CoreEvent::CopilotProgress(CopilotProgress {
                    id: id.to_string(),
                    phase: friendly_tool(&call.name, Some(&call.input)),
                }));
                let out = mcp::dispatch(core, &call.name, call.input.clone())
                    .await
                    .unwrap_or_else(|e| format!("Erreur outil : {e}"));
                results.push(ToolResult {
                    id: call.id.clone(),
                    content: truncate(&out, TOOL_OUTPUT_CAP),
                });
            } else {
                results.push(ToolResult {
                    id: call.id.clone(),
                    content: "Outil non autorisé pour le diagnostic (lecture seule).".into(),
                });
            }
        }

        transcript.push(Msg::Assistant {
            text: resp.text,
            calls: resp.calls,
        });
        if let Some(input) = report {
            return Ok(build_diagnosis(inc, input));
        }
        transcript.push(Msg::ToolResults(results));
    }

    Err("Le diagnostic n'a pas convergé (trop d'étapes).".into())
}

// --- Outils exposés à l'agent ---------------------------------------------

/// Construit des `ToolSpec` depuis le catalogue MCP, filtrés par `keep(nom)`.
fn tool_specs(keep: impl Fn(&str) -> bool) -> Vec<ToolSpec> {
    // Emprunte le catalogue statique (aucune reconstruction / clone du tableau par tour).
    mcp::tool_list()
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t.get("name")?.as_str()?;
            if !keep(name) {
                return None;
            }
            Some(ToolSpec {
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .to_string(),
                schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({ "type": "object" })),
                name: name.to_string(),
            })
        })
        .collect()
}

/// Outils de lecture (dérivés du catalogue MCP) + `report_diagnosis` (sortie structurée).
fn agent_tools(disabled: &[String]) -> Vec<ToolSpec> {
    let mut tools = tool_specs(|n| !tool_disabled(n, disabled) && READ_TOOLS.contains(&n));
    tools.push(report_tool());
    tools
}

fn report_tool() -> ToolSpec {
    ToolSpec {
        name: "report_diagnosis".into(),
        description: "À appeler UNE fois le diagnostic terminé pour livrer le rapport final à l'administrateur.".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "Résumé en une phrase du problème." },
                "cause": { "type": "string", "description": "Cause probable (markdown), justifiée par les logs/métriques observés." },
                "suggested_fix": { "type": "string", "description": "Correctif détaillé et étapes (markdown)." },
                "actions": {
                    "type": "array",
                    "description": "0 à 3 actions concrètes applicables via les outils.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": { "type": "string", "enum": action_enum(), "description": "Outil d'action." },
                            "args": { "type": "object", "description": ACTION_TOOLS_SPEC },
                            "label": { "type": "string", "description": "Description lisible de l'action." },
                            "risk": { "type": "string", "enum": ["safe", "caution", "danger"], "description": "safe = réversible/sans perte ; caution = modifie une config ; danger = destructif." }
                        },
                        "required": ["tool", "args", "label", "risk"]
                    }
                }
            },
            "required": ["summary", "cause", "suggested_fix", "actions"]
        }),
    }
}

/// Spécification EXACTE des arguments par outil, injectée dans le prompt pour que le modèle
/// n'ait pas à deviner les noms de paramètres (source du bug « paramètre `from` requis »).
const ACTION_TOOLS_SPEC: &str = "Arguments EXACTS par outil (n'inclus PAS server_id, il est ajouté automatiquement) : \
power_action → {\"action\": \"start|restart|restart10|stop|stop10|kill\"} ; \
send_command → {\"command\": \"...\"} ; \
write_file → {\"path\": \"/chemin\", \"content\": \"...\"} ; \
create_dir → {\"path\": \"/chemin\"} ; \
delete_path → {\"path\": \"/chemin\", \"is_dir\": false} ; \
rename_path → {\"from\": \"/chemin/source\", \"to\": \"/chemin/destination\"} (chemins COMPLETS depuis la racine /) ; \
player_action → {\"action\": \"kick|ban|unban|op_add|op_remove|whitelist_add|whitelist_remove\", \"player\": \"pseudo\"} ; \
set_startup_params → {\"parameters\": \"java -Xms… -Xmx… <flags> -jar {{SERVER_JARFILE}}\"} — commande Java COMPLÈTE (pars de la commande actuelle via read_startup, GARDE {{SERVER_JARFILE}}, ne change pas -Xmx sans raison) ; effet au PROCHAIN démarrage (risque caution) ; \
install_mod → {\"source\": \"modrinth|spigot\", \"kind\": \"mod|plugin\", \"slug\": \"slug-Modrinth-ou-id-numerique-Spigot\", \"version_id\": \"id-de-version\", \"loader\": \"fabric|neoforge|forge|quilt|paper|spigot|...\"} — obtiens le `version_id` via list_mod_versions AVANT ; sources installables : `modrinth` et `spigot` (CurseForge à venir) ; pour Spigot le `slug` est l'id numérique du plugin et le loader est ignoré ; effet au prochain démarrage (risque caution) ; \
create_snapshot → {\"name\": \"nom court\"} — crée un SNAPSHOT (point de sauvegarde à la demande) du serveur, le filet AVANT une intervention risquée ; additif, risque SAFE ; \
restore_snapshot → {\"snapshot_id\": id} — ÉCRASE le serveur avec ce snapshot (récupère l'id via list_snapshots) ; risque DANGER, perte de l'état actuel ; \
restore_backup → {\"backup_id\": id} — ÉCRASE le serveur avec ce backup quotidien (id via list_backups) ; risque DANGER ; \
delete_snapshot → {\"snapshot_id\": id} — supprime DÉFINITIVEMENT un snapshot (id via list_snapshots) ; risque DANGER ; \
repair_region → {\"path\": \"/world/region/r.X.Z.mca\", \"mode\": \"clear_corrupt|delete\"} — répare une région corrompue (repère les chunks via inspect_region AVANT) : `clear_corrupt` efface les chunks corrompus (régénération, perte minimale), `delete` supprime toute la région ; risque DANGER, propose un snapshot d'abord.";

/// Normalise les arguments d'une action proposée avant exécution : résout les synonymes
/// courants vers les clés canoniques, puis force le `server_id` du contexte (l'agent peut
/// l'oublier ou se tromper). Filet de sécurité complémentaire au prompt.
pub(crate) fn prepare_action_args(server_id: i64, tool: &str, args: Value) -> Value {
    let mut map = match args {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    apply_aliases(tool, &mut map);
    map.insert("server_id".to_string(), json!(server_id));
    Value::Object(map)
}

fn apply_aliases(tool: &str, map: &mut serde_json::Map<String, Value>) {
    let rules: &[(&str, &[&str])] = match tool {
        "rename_path" => &[
            ("from", &["source", "src", "old", "old_path", "oldpath", "from_path", "path", "file", "original"]),
            ("to", &["destination", "dest", "new", "new_path", "newpath", "to_path", "target", "renamed"]),
        ],
        "write_file" => &[
            ("path", &["file", "filepath", "file_path", "filename"]),
            ("content", &["text", "data", "body", "contents"]),
        ],
        "create_dir" => &[("path", &["dir", "directory", "folder"])],
        "delete_path" => &[("path", &["file", "filepath", "file_path", "target"])],
        "send_command" => &[("command", &["cmd", "command_line", "commandline"])],
        "player_action" => &[("player", &["username", "user", "name", "pseudo"])],
        "power_action" => &[("action", &["poweraction", "signal"])],
        "set_startup_params" => &[("parameters", &["params", "command", "startup", "java", "cmd"])],
        "install_mod" => &[
            ("slug", &["id", "project", "project_id", "mod", "plugin", "name", "identifier"]),
            ("version_id", &["version", "versionId", "file_id", "fileId", "vid"]),
            ("loader", &["modloader", "mod_loader", "modLoader"]),
        ],
        "create_snapshot" => &[("name", &["label", "title", "snapshot_name", "nom"])],
        "restore_snapshot" | "delete_snapshot" => {
            &[("snapshot_id", &["id_snapshot", "snapshot", "id"])]
        }
        "restore_backup" => &[("backup_id", &["id_backup", "backup", "id"])],
        "repair_region" => &[
            ("path", &["file", "region", "mca", "filepath", "file_path"]),
            ("mode", &["action", "method"]),
        ],
        _ => &[],
    };
    for (canon, alts) in rules {
        if map.contains_key(*canon) {
            continue;
        }
        for a in *alts {
            if let Some(v) = map.remove(*a) {
                map.insert((*canon).to_string(), v);
                break;
            }
        }
    }
}

// --- Prompts ---------------------------------------------------------------

const SYSTEM_PROMPT: &str = "Tu es le copilote d'administration d'un serveur Minecraft hébergé chez MineStrator (base Pterodactyl). \
Un incident vient d'être détecté par le superviseur. Ta mission : diagnostiquer la cause avec les outils de LECTURE à ta disposition \
(console, historique de métriques CPU/RAM/disque, fichiers de configuration via SFTP), puis livrer un rapport actionnable en appelant \
l'outil report_diagnosis.\n\n\
Méthode : lis la console récente pour repérer erreurs/exceptions/stacktraces ; corrèle avec les métriques ; inspecte les fichiers pertinents \
(server.properties, config des plugins) si utile. Fais uniquement les appels nécessaires (sois économe). Puis appelle report_diagnosis avec : \
un résumé clair, la cause probable justifiée par ce que tu as observé, un correctif détaillé, et 0 à 3 actions concrètes applicables. \
Classe chaque action : safe (réversible, sans perte : redémarrer, ajuster une option), caution (modifie une config), danger (destructif). \
N'invente aucune donnée ; si une information te manque, dis-le. Réponds en français.";

fn user_prompt(inc: &Incident, console_tail: &str) -> String {
    let console = console_or_default(console_tail);
    if let Some(sel) = &inc.selection {
        return format!(
            "Analyse demandée par l'utilisateur sur le serveur « {} » (server_id = {}).\n\n\
             Extrait sélectionné à analyser :\n```\n{}\n```\n\n\
             Contexte console récent :\n```\n{}\n```\n\n\
             Analyse cet extrait (explication + diagnostic + correctif si pertinent), en \
             t'aidant des outils de lecture si besoin, puis livre ton rapport via report_diagnosis.",
            inc.server_name,
            inc.server_id,
            truncate(sel, 8000),
            console
        );
    }
    format!(
        "Incident sur le serveur « {} » (server_id = {}).\n\
         Déclencheur : {} — {}.\n\n\
         Dernières lignes de console :\n```\n{}\n```\n\n\
         Diagnostique la cause puis livre ton rapport via report_diagnosis.",
        inc.server_name, inc.server_id, inc.trigger, inc.message, console
    )
}

fn console_or_default(console_tail: &str) -> String {
    if console_tail.trim().is_empty() {
        "(console indisponible)".to_string()
    } else {
        console_tail.to_string()
    }
}

/// Prompt autonome pour un agent CLI local : tout le contexte + consigne de sortie JSON.
fn cli_prompt(inc: &Incident, console_tail: &str, metrics: &str) -> String {
    let console = console_or_default(console_tail);
    let focus = match &inc.selection {
        Some(sel) => format!(
            "Analyse demandée par l'utilisateur. Extrait sélectionné à analyser :\n```\n{}\n```\n\n",
            truncate(sel, 8000)
        ),
        None => format!("Déclencheur : {} — {}.\n\n", inc.trigger, inc.message),
    };
    format!(
        "{SYSTEM_PROMPT}\n\n\
         Serveur « {} » (server_id = {}).\n\
         {}\
         Dernières lignes de console :\n```\n{}\n```\n\n\
         Historique de métriques (JSON) :\n```json\n{}\n```\n\n\
         Réponds UNIQUEMENT avec un objet JSON valide (sans texte autour, sans balises ```), \
         de la forme : {{\"summary\": string, \"cause\": string, \"suggested_fix\": string, \
         \"actions\": [{{\"tool\": string, \"args\": object, \"label\": string, \"risk\": \"safe\"|\"caution\"|\"danger\"}}]}}. \
         `actions` peut être vide. {ACTION_TOOLS_SPEC}",
        inc.server_name, inc.server_id, focus, console, metrics
    )
}

/// Prompt pour l'agent CLI branché sur le MCP : il investigue lui-même via les outils.
fn cli_agentic_prompt(inc: &Incident) -> String {
    let focus = match &inc.selection {
        Some(sel) => format!(
            "Analyse demandée par l'utilisateur. Extrait à analyser :\n```\n{}\n```\n\n",
            truncate(sel, 8000)
        ),
        None => format!("Déclencheur : {} — {}.\n\n", inc.trigger, inc.message),
    };
    format!(
        "{SYSTEM_PROMPT}\n\n\
         Serveur « {} » (server_id = {}).\n\
         {}\
         Tu disposes d'outils MCP de LECTURE : {}. \
         Utilise-les pour investiguer et CROISER (console, métriques, et fichiers de config pertinents), \
         puis rends ton rapport.\n\n\
         Réponds UNIQUEMENT avec un objet JSON valide (sans texte ni balises autour), de la forme : \
         {{\"summary\": string, \"cause\": string, \"suggested_fix\": string, \"actions\": [{{\"tool\": string, \"args\": object, \"label\": string, \"risk\": \"safe\"|\"caution\"|\"danger\"}}]}}. \
         {ACTION_TOOLS_SPEC}",
        inc.server_name, inc.server_id, focus, read_tools_mcp_list()
    )
}

/// Extrait le rapport JSON d'une sortie CLI : gère l'enveloppe `--output-format json`
/// (champ `result`), un objet rapport direct, ou du texte contenant le JSON.
fn extract_report(out: &str) -> Option<Value> {
    if let Ok(env) = serde_json::from_str::<Value>(out) {
        if let Some(r) = env.get("result").and_then(|v| v.as_str()) {
            return extract_json(r);
        }
        if env.get("summary").is_some() {
            return Some(env);
        }
    }
    extract_json(out)
}

/// Extrait le premier objet JSON d'une sortie CLI (tolère du texte/balises autour).
fn extract_json(out: &str) -> Option<Value> {
    let start = out.find('{')?;
    let end = out.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&out[start..=end]).ok()
}

// --- Construction du rapport ----------------------------------------------

/// Squelette commun d'un `Diagnosis` (champs contextuels). L'`id` est rempli par `finish()`.
fn diagnosis_base(inc: &Incident) -> Diagnosis {
    Diagnosis {
        id: String::new(),
        server_id: inc.server_id,
        server_name: inc.server_name.clone(),
        trigger: inc.trigger.clone(),
        severity: inc.severity.clone(),
        summary: String::new(),
        cause: String::new(),
        suggested_fix: String::new(),
        actions: Vec::new(),
        ts: now_secs(),
    }
}

/// Convertit un objet JSON en `ProposedAction` (défaut risque = `caution`). Source unique
/// utilisée par `build_diagnosis` et le parsing d'actions du chat.
fn parse_action(v: &Value) -> Option<ProposedAction> {
    Some(ProposedAction {
        tool: v.get("tool")?.as_str()?.to_string(),
        args: v.get("args").cloned().unwrap_or_else(|| json!({})),
        label: v.get("label").and_then(|l| l.as_str()).unwrap_or_default().to_string(),
        risk: v.get("risk").and_then(|r| r.as_str()).unwrap_or("caution").to_string(),
    })
}

fn build_diagnosis(inc: &Incident, input: Value) -> Diagnosis {
    let actions = input
        .get("actions")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(parse_action).collect())
        .unwrap_or_default();

    Diagnosis {
        summary: sstr(&input, "summary", "Diagnostic"),
        cause: sstr(&input, "cause", ""),
        suggested_fix: sstr(&input, "suggested_fix", ""),
        actions,
        ..diagnosis_base(inc)
    }
}

fn text_diagnosis(inc: &Incident, text: &str) -> Diagnosis {
    Diagnosis {
        summary: "Analyse du Copilote".into(),
        suggested_fix: text.to_string(),
        ..diagnosis_base(inc)
    }
}

fn error_diagnosis(inc: &Incident, err: &str, hint: &str) -> Diagnosis {
    Diagnosis {
        severity: "warning".into(),
        summary: "Le Copilote n'a pas pu diagnostiquer".into(),
        cause: err.to_string(),
        suggested_fix: hint.to_string(),
        ..diagnosis_base(inc)
    }
}

/// Préfixe le résumé d'une pastille selon l'autonomie (info UI légère).
fn decorate_summary(cfg: &CopilotConfig, diag: &Diagnosis) -> String {
    if matches!(cfg.autonomy, Autonomy::AutoSafe)
        && diag.actions.iter().any(|a| a.risk == "safe")
    {
        format!("{} (actions sûres appliquées)", diag.summary)
    } else {
        diag.summary.clone()
    }
}

// --- Helpers ---------------------------------------------------------------

fn sstr(v: &Value, key: &str, default: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or(default)
        .to_string()
}

fn truncate(s: &str, max: usize) -> String {
    crate::util::truncate_on_boundary(s, max, "…\n[tronqué]")
}

// ====================== CHAT — assistant conversationnel ======================
//
// Conversation multi-tours scopée à un serveur. Suggéré (propose des actions, boutons) ou
// autonome (l'agent exécute directement). Réutilise la couche LLM, les outils et le streaming.

const CHAT_MAX_TURNS: usize = 12;
const CHAT_MARK: &str = "===ACTIONS===";

/// Réponse d'un tour de chat : texte + actions proposées (vides en mode autonome, où l'agent agit).
#[derive(Debug, Clone, Serialize)]
pub struct ChatReply {
    pub text: String,
    pub actions: Vec<ProposedAction>,
}

/// État d'une conversation (transcript LLM pour HTTP ; identifiant de session Claude Code pour CLI).
pub struct ChatSession {
    transcript: Vec<Msg>,
    cli_session: Option<String>,
    /// D — process Claude Code maintenu vivant pour cette conversation (Claude Code uniquement).
    persistent: Option<crate::cli_session::PersistentCli>,
    /// Mode (autonome ?) pour lequel le process vivant a été lancé : le toolset MCP est figé au
    /// spawn, donc un changement de mode impose un respawn.
    persistent_autonomous: bool,
    /// La voie persistante s'est révélée inutilisable → on reste sur le one-shot pour cette session.
    no_persistent: bool,
    /// Échecs consécutifs de la voie persistante (2 → on bascule définitivement en one-shot).
    persistent_fails: u8,
}

impl ChatSession {
    pub fn new() -> Self {
        Self {
            transcript: Vec::new(),
            cli_session: None,
            persistent: None,
            persistent_autonomous: false,
            no_persistent: false,
            persistent_fails: 0,
        }
    }
}

/// Un tour de conversation : ajoute le message utilisateur, investigue via les outils, répond.
pub(crate) async fn chat_turn(
    core: &Core,
    session: &mut ChatSession,
    server_id: i64,
    server_name: &str,
    message: &str,
    autonomous: bool,
    progress_id: &str,
) -> ChatReply {
    let cfg = core.get_copilot_config();
    let res = if cfg.provider == Provider::LocalCli {
        chat_cli(core, &cfg, session, server_id, server_name, message, autonomous, progress_id).await
    } else {
        chat_http(core, &cfg, session, server_id, server_name, message, autonomous, progress_id).await
    };
    res.unwrap_or_else(|e| ChatReply { text: format!("⚠ {e}"), actions: Vec::new() })
}

// Les fonctions de chat orchestrent des paramètres hétérogènes (Core, session mutable, ids,
// drapeaux) : les regrouper en struct serait un « god-struct » artificiel.
#[allow(clippy::too_many_arguments)]
async fn chat_http(
    core: &Core,
    cfg: &CopilotConfig,
    session: &mut ChatSession,
    server_id: i64,
    server_name: &str,
    message: &str,
    autonomous: bool,
    id: &str,
) -> Result<ChatReply, String> {
    let key = resolve_key(cfg)?;
    let client = LlmClient::new(cfg.provider, &cfg.base_url, &key, &cfg.model);
    let tools = chat_tools(autonomous, &cfg.disabled_tools);
    let system = with_effort(&chat_system(autonomous), cfg.effort);

    let prompt = if session.transcript.is_empty() {
        format!("[Serveur « {server_name} », server_id = {server_id}]\n{message}")
    } else {
        message.to_string()
    };
    // Rollback transactionnel : si le tour échoue (erreur réseau/429 ou non-convergence), on retire
    // le message user et tout l'échange partiel. Sinon le transcript se termine sur un rôle « user »
    // et le tour suivant empile un 2ᵉ « user » → l'API Anthropic rejette (« roles must alternate »)
    // de façon PERMANENTE jusqu'au prochain reset de session.
    let base = session.transcript.len();
    session.transcript.push(Msg::User(prompt));

    for _ in 0..CHAT_MAX_TURNS {
        let resp = match client.complete(&system, &tools, &session.transcript).await {
            Ok(resp) => resp,
            Err(e) => {
                session.transcript.truncate(base);
                return Err(e);
            }
        };
        if resp.calls.is_empty() {
            session.transcript.push(Msg::Assistant {
                text: resp.text.clone(),
                calls: Vec::new(),
            });
            return Ok(parse_chat_reply(&resp.text, autonomous));
        }
        let mut results = Vec::new();
        for call in &resp.calls {
            core.emit(CoreEvent::CopilotProgress(CopilotProgress {
                id: id.to_string(),
                phase: friendly_tool(&call.name, Some(&call.input)),
            }));
            let content = if READ_TOOLS.contains(&call.name.as_str()) {
                mcp::dispatch(core, &call.name, call.input.clone())
                    .await
                    .unwrap_or_else(|e| format!("Erreur outil : {e}"))
            } else if autonomous && is_write_tool(&call.name) {
                let args = prepare_action_args(server_id, &call.name, call.input.clone());
                mcp::dispatch(core, &call.name, args)
                    .await
                    .unwrap_or_else(|e| format!("Erreur : {e}"))
            } else {
                format!("Action non exécutée (mode suggestion). Propose-la via {CHAT_MARK}.")
            };
            results.push(ToolResult {
                id: call.id.clone(),
                content: truncate(&content, TOOL_OUTPUT_CAP),
            });
        }
        session.transcript.push(Msg::Assistant {
            text: resp.text,
            calls: resp.calls,
        });
        session.transcript.push(Msg::ToolResults(results));
    }
    session.transcript.truncate(base);
    Err("La conversation n'a pas convergé (trop d'étapes).".into())
}

/// Point d'entrée CLI : tente la voie **persistante** (D, Claude Code) et retombe sur le one-shot
/// éprouvé en cas d'échec — l'utilisateur obtient toujours une réponse.
#[allow(clippy::too_many_arguments)] // orchestration : paramètres hétérogènes (cf. chat_http)
async fn chat_cli(
    core: &Core,
    cfg: &CopilotConfig,
    session: &mut ChatSession,
    server_id: i64,
    server_name: &str,
    message: &str,
    autonomous: bool,
    id: &str,
) -> Result<ChatReply, String> {
    if cfg.cli_agent == crate::cli_agent::CliAgent::ClaudeCode && !session.no_persistent {
        match chat_cli_persistent(core, cfg, session, server_id, server_name, message, autonomous, id)
            .await
        {
            Ok(reply) => {
                session.persistent_fails = 0;
                return Ok(reply);
            }
            Err(_e) => {
                // Échec : on jette le process, on compte, et on retombe sur le one-shot pour CE
                // message. Deux échecs d'affilée → on abandonne la voie rapide pour la session.
                session.persistent = None;
                session.persistent_fails = session.persistent_fails.saturating_add(1);
                if session.persistent_fails >= 2 {
                    session.no_persistent = true;
                }
            }
        }
    }
    chat_cli_oneshot(core, cfg, session, server_id, server_name, message, autonomous, id).await
}

/// (Re)démarre le process persistant si nécessaire : absent, mort, ou lancé pour un **autre mode**
/// (le toolset MCP lecture-seule/autonome est figé au spawn → un changement de mode impose un
/// respawn, pour la parité de sécurité avec le one-shot). Restaure via `--resume` si une session
/// est déjà connue (reprise après un éventuel crash). L'ancien process est droppé → tué (kill_on_drop).
async fn ensure_persistent_spawn(
    cfg: &CopilotConfig,
    session: &mut ChatSession,
    autonomous: bool,
) -> Result<(), String> {
    let reusable = session.persistent.as_mut().map(|p| p.is_alive()).unwrap_or(false)
        && session.persistent_autonomous == autonomous;
    if reusable {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("chemin de l'exécutable : {e}"))?;
    let allowed = allowed_tool_names(cfg, autonomous);
    let restore = session.cli_session.as_deref();
    let plan = crate::cli_agent::plan_run(
        crate::cli_agent::CliAgent::ClaudeCode,
        &crate::cli_agent::RunCtx {
            exe,
            allowed_tools: &allowed,
            web_search: cfg.web_search,
            model: &cfg.model,
            effort: cfg.effort.claude_level(),
            resume: restore,
            streaming: true,
        },
    )?;
    let mut args = plan.args;
    args.push("--input-format".to_string());
    args.push("stream-json".to_string());
    session.persistent = Some(crate::cli_session::PersistentCli::spawn(
        cli_command(cfg),
        &args,
        plan.cwd.as_deref(),
        &plan.env,
        restore.is_some(),
    )?);
    session.persistent_autonomous = autonomous;
    Ok(())
}

/// F — pré-chauffe : démarre le process persistant AVANT le 1er message (best-effort, silencieux),
/// pour que celui-ci tape un process déjà chaud (Node + MCP prêts). Le contexte serveur et le
/// system prompt partent avec le 1er vrai message, pas ici.
pub(crate) async fn chat_warm(cfg: &CopilotConfig, session: &mut ChatSession, autonomous: bool) {
    if session.no_persistent {
        return;
    }
    let _ = ensure_persistent_spawn(cfg, session, autonomous).await;
}

/// D — voie persistante : réutilise (ou (re)démarre) le process `claude` de la session et lui pousse
/// le message via `--input-format stream-json`, sans relancer Node/MCP.
#[allow(clippy::too_many_arguments)] // orchestration : paramètres hétérogènes (cf. chat_http)
async fn chat_cli_persistent(
    core: &Core,
    cfg: &CopilotConfig,
    session: &mut ChatSession,
    server_id: i64,
    server_name: &str,
    message: &str,
    autonomous: bool,
    id: &str,
) -> Result<ChatReply, String> {
    let agent = crate::cli_agent::CliAgent::ClaudeCode;
    ensure_persistent_spawn(cfg, session, autonomous).await?;
    let p = session.persistent.as_mut().expect("process persistant présent");

    // 1er tour d'un process à froid → on préfixe system prompt + contexte serveur (comme le one-shot).
    let user_text = if p.primed {
        message.to_string()
    } else {
        format!(
            "{}\n\n[Serveur « {server_name} », server_id = {server_id}]\n\
             Utilise les outils MCP pour investiguer. Question :\n{message}",
            chat_system(autonomous)
        )
    };
    p.primed = true;

    let mut last = String::new();
    let mut final_result: Option<String> = None;
    let mut captured_sid: Option<String> = None;
    p.run_turn(&user_text, CLI_AGENTIC_TIMEOUT_S, |line| {
        // Une ligne NDJSON porte UN seul type d'event → `else if` : 1 parse au lieu de 3 sur le
        // chemin chaud (streaming token-par-token). Le `result` prime sur la phase « Finalisation… ».
        if let Some(delta) = crate::cli_agent::text_delta(agent, line) {
            core.emit(CoreEvent::ChatDelta(ChatDelta { id: id.to_string(), text: delta }));
        } else if let Some(r) = crate::cli_agent::result_line(agent, line) {
            final_result = Some(r);
        } else if let Some(phase) = crate::cli_agent::phase(agent, line) {
            if phase != last {
                last = phase.clone();
                core.emit(CoreEvent::CopilotProgress(CopilotProgress { id: id.to_string(), phase }));
            }
        }
        if captured_sid.is_none() {
            if let Some(sid) = stream_session_line(line) {
                captured_sid = Some(sid);
            }
        }
    })
    .await?;

    if let Some(sid) = captured_sid {
        session.cli_session = Some(sid);
    }
    let text = final_result.ok_or_else(|| "aucun résultat renvoyé par l'agent".to_string())?;
    Ok(parse_chat_reply(&text, autonomous))
}

#[allow(clippy::too_many_arguments)] // orchestration : paramètres hétérogènes (cf. chat_http)
async fn chat_cli_oneshot(
    core: &Core,
    cfg: &CopilotConfig,
    session: &mut ChatSession,
    server_id: i64,
    server_name: &str,
    message: &str,
    autonomous: bool,
    id: &str,
) -> Result<ChatReply, String> {
    let exe = std::env::current_exe().map_err(|e| format!("chemin de l'exécutable : {e}"))?;
    let agent = cfg.cli_agent;
    let allowed = allowed_tool_names(cfg, autonomous);
    let plan = crate::cli_agent::plan_run(
        agent,
        &crate::cli_agent::RunCtx {
            exe,
            allowed_tools: &allowed,
            web_search: cfg.web_search,
            model: &cfg.model,
            effort: cfg.effort.claude_level(),
            resume: session.cli_session.as_deref(),
            streaming: true,
        },
    )?;

    let first = session.cli_session.is_none();
    let prompt = if first {
        format!(
            "{}\n\n[Serveur « {server_name} », server_id = {server_id}]\n\
             Utilise les outils MCP pour investiguer. Question :\n{message}",
            chat_system(autonomous)
        )
    } else {
        message.to_string()
    };

    let mut last = String::new();
    // Le texte final (avec l'éventuel bloc ===ACTIONS===) est capturé AU FIL DE L'EAU : une longue
    // session agentique dépasse la limite de sortie et sa ligne `result` finale serait tronquée,
    // ce qui faisait retomber l'appelant sur un dump de JSON brut (et perdait les actions proposées).
    let mut final_result: Option<String> = None;
    let ndjson = cli::run_streaming(
        cli_command(cfg),
        &plan.args,
        &prompt,
        CLI_AGENTIC_TIMEOUT_S,
        plan.cwd.as_deref(),
        &plan.env,
        |line| {
            // Streaming du texte de réponse dans la bulle du chat (Claude Code seulement).
            // Une ligne NDJSON porte UN seul type d'event → `else if` : 1 parse au lieu de 3.
            if let Some(delta) = crate::cli_agent::text_delta(agent, line) {
                core.emit(CoreEvent::ChatDelta(ChatDelta { id: id.to_string(), text: delta }));
            } else if let Some(r) = crate::cli_agent::result_line(agent, line) {
                final_result = Some(r);
            } else if let Some(phase) = crate::cli_agent::phase(agent, line) {
                if phase != last {
                    last = phase.clone();
                    core.emit(CoreEvent::CopilotProgress(CopilotProgress {
                        id: id.to_string(),
                        phase,
                    }));
                }
            }
        },
    )
    .await?;

    if let Some(sid) = crate::cli_agent::session_id(agent, &ndjson) {
        session.cli_session = Some(sid);
    }
    // Priorité au résultat capturé (intégral) ; repli sur le parsing du ndjson ; jamais de JSON brut.
    let text = final_result
        .or_else(|| crate::cli_agent::final_text(agent, &ndjson))
        .unwrap_or_default();
    Ok(parse_chat_reply(&text, autonomous))
}

/// Outils exposés au chat : lecture toujours, + actions si autonome.
fn chat_tools(autonomous: bool, disabled: &[String]) -> Vec<ToolSpec> {
    tool_specs(|n| {
        !tool_disabled(n, disabled) && (READ_TOOLS.contains(&n) || (autonomous && is_write_tool(n)))
    })
}

fn chat_system(autonomous: bool) -> String {
    let base = "Tu es l'assistant d'administration d'un serveur Minecraft MineStrator. L'utilisateur te pose des questions sur SON serveur. \
        Utilise les outils de LECTURE (console, métriques, fichiers de config, liste des plugins/mods installés, commande de démarrage) pour INVESTIGUER avant de répondre — ne suppose pas, vérifie sur le serveur. \
        Tu disposes AUSSI d'un MARKETPLACE : `market_search` (mods/plugins depuis Modrinth/CurseForge/SpigotMC, filtrable par `loader` et `game_version`), `list_mod_versions` (récupère le `version_id`), et `install_mod` (source `modrinth` = mods ET plugins ; `spigot` = plugins). \
        AVANT de proposer une installation, vérifie via `market_search` que le projet est INSTALLABLE : un item marqué `premium: true` (payant/restreint) ou `external: true` (téléchargement externe SpigotMC) N'EST PAS installable via MineStrator — l'API renvoie 403. Ne le propose pas : signale-le et choisis une alternative gratuite et non-externe. Ne te fie JAMAIS à ta mémoire pour l'existence, la compatibilité ou l'installabilité d'un plugin — passe toujours par market_search. \
        Pour une demande de type « quels plugins/mods pour un serveur PvP/faction ? » ou « les mods incontournables pour X » : si tu disposes d'un outil de RECHERCHE WEB (WebSearch/WebFetch), sers-t'en pour identifier ce qui se fait de mieux/de plus populaire actuellement (listes, guides, tendances) ; sinon appuie-toi sur ta connaissance. Ensuite retrouve SYSTÉMATIQUEMENT chaque projet via `market_search` (vérifie compatibilité loader/version + existence réelle dans le catalogue) avant de recommander ou de proposer l'installation. Détecte le loader/la version du serveur via la liste des installés ou la commande de démarrage. \
        Pour toute question de PERFORMANCE / lag / CPU / RAM : utilise `analyze_performance` (Spark natif, rapport PARSÉ : TPS/MSPT/GC + points chauds) — mets `with_profiler: true` pour l'analyse CPU approfondie. Si l'utilisateur te donne une URL de rapport Spark (`spark.lucko.me/…`), parse-la avec `parse_spark_report` (profiler OU heapsummary mémoire). N'essaie JAMAIS d'ouvrir/fetcher une URL `spark.lucko.me` avec un outil web : c'est une app JS servant du protobuf binaire, illisible ainsi. \
        Pour un serveur qui NE DÉMARRE PAS / crash-loop : utilise `diagnose_startup` (rassemble commande de démarrage + fin de latest.log + dernier crash-report + pré-scan des pannes connues) AVANT de conclure, puis propose un correctif ciblé (désactiver le mod fautif en renommant son .jar en .jar.disabled via `rename_path`, corriger une config, accepter l'EULA…). \
        FILET DE SÉCURITÉ : avant toute opération risquée/destructive (restauration, suppression de fichiers, réécriture massive de config), propose d'abord un snapshot via `create_snapshot` ; pour revenir en arrière, propose restore_snapshot ou restore_backup. \
        Réponds en français, clair, concret et sans blabla.";
    if autonomous {
        format!("{base} Tu es en mode AUTONOME : tu peux exécuter directement les actions nécessaires via les outils d'action ; explique ce que tu fais et pourquoi. {ACTION_TOOLS_SPEC}")
    } else {
        format!(
            "{base} Tu es en mode SUGGESTION : n'exécute aucune action modifiante toi-même. Les actions que tu proposes \
             apparaissent comme des boutons « Appliquer » cliquables DIRECTEMENT dans cette conversation (jamais dans une autre interface — ne renvoie PAS l'utilisateur vers le panel MineStrator). \
             Si l'utilisateur te demande d'agir/exécuter tout seul, dis-lui simplement d'activer l'interrupteur « Autonome » en haut de cet onglet. \
             Pour proposer des actions, termine ta réponse par une ligne EXACTEMENT `{CHAT_MARK}` suivie d'un tableau JSON \
             [{{\"tool\":..,\"args\":..,\"label\":..,\"risk\":\"safe|caution|danger\"}}]. Sinon n'ajoute rien. {ACTION_TOOLS_SPEC}"
        )
    }
}

/// Sépare le texte de réponse et les actions proposées (bloc `===ACTIONS===`).
fn parse_chat_reply(text: &str, autonomous: bool) -> ChatReply {
    match text.find(CHAT_MARK) {
        Some(pos) => {
            let (before, after) = text.split_at(pos);
            let actions = if autonomous {
                Vec::new()
            } else {
                parse_actions(&after[CHAT_MARK.len()..])
            };
            ChatReply {
                text: before.trim().to_string(),
                actions,
            }
        }
        None => ChatReply {
            text: text.trim().to_string(),
            actions: Vec::new(),
        },
    }
}

fn parse_actions(json: &str) -> Vec<ProposedAction> {
    let (Some(s), Some(e)) = (json.find('['), json.rfind(']')) else {
        return Vec::new();
    };
    if e < s {
        return Vec::new();
    }
    serde_json::from_str::<Value>(&json[s..=e])
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|arr| arr.iter().filter_map(parse_action).collect())
        .unwrap_or_default()
}

/// Identifiant de session à réutiliser pour `--resume` au tour suivant.
///
/// On privilégie le `session_id` de l'event `result` : lors d'un `--resume`, Claude Code
/// peut « forker » la session reprise vers un nouvel id, et c'est CET id (état après ce tour)
/// qu'il faut chaîner — sinon on repartirait d'une base périmée et le contexte n'accumulerait pas.
/// À défaut de `result`, on retombe sur le premier `session_id` rencontré (event `system/init`).
pub(crate) fn extract_session_id(ndjson: &str) -> Option<String> {
    let mut fallback = None;
    for line in ndjson.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("result") {
            return Some(sid.to_string());
        }
        fallback.get_or_insert_with(|| sid.to_string());
    }
    fallback
}
