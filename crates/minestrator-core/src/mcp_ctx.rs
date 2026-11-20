//! Contexte d'un appel d'outil MCP : session, progression, annulation et confirmation demandée
//! directement à l'humain.
//!
//! Les outils s'en servent **sans changer de signature** : `mcp::dispatch(core, name, args)` reste
//! celle du Copilote. Le transport MCP (`mcp_transport.rs`) pose le contexte autour de l'appel
//! (`tokio::task_local!`) ; hors de ce contexte (Copilote, tests, appel interne, sous-tâche lancée
//! par `tokio::spawn`), chaque fonction est sans effet : `session_id()` et `client_name()` valent
//! `None`, `progress` ne fait rien, `cancelled()` vaut `false` et `confirm` renvoie
//! [`Confirmation::Unsupported`] — l'outil retombe alors sur sa garde habituelle (`confirm_name`).
//!
//! **Confirmation et ère sans état (`2026-07-28`).** Le client n'y reçoit plus de requête du
//! serveur : `confirm` interrompt l'appel, le transport répond `input_required` et le client
//! **rejoue** la requête avec la réponse de l'humain. Tout ce qu'un outil fait **avant** `confirm`
//! est donc rejoué : un outil ne demande confirmation qu'avant tout effet.

use crate::mcp_transport::Peer;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};

/// Réponse de l'humain à une demande de confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// L'utilisateur a confirmé dans son client MCP.
    Accepted,
    /// L'utilisateur a refusé ou annulé : l'outil ne doit rien faire.
    Declined,
    /// Le client ne sait pas poser la question (ou appel hors MCP) : garde habituelle de l'outil.
    Unsupported,
}

/// Identifiant de la session MCP en cours (une connexion cliente), `None` hors transport MCP.
pub fn session_id() -> Option<String> {
    with(|c| c.session.clone())
}

/// Nom du client MCP (`clientInfo.name` de l'initialisation), `None` hors transport MCP.
pub fn client_name() -> Option<String> {
    with(|c| c.client.clone()).flatten()
}

/// Signale l'avancement d'une opération longue au client (si sa requête porte un
/// `progressToken`). `total` absent = avancement inconnu. Sans effet hors transport MCP.
///
/// La spécification exige une progression **croissante** : une valeur qui ne dépasse pas la
/// précédente est ignorée, et les envois sont espacés (le dernier pas, `done >= total`, part
/// toujours).
pub async fn progress(done: f64, total: Option<f64>, message: &str) {
    if let Some(ctx) = current() {
        ctx.report(done, total, Some(message));
    }
}

/// Le client a-t-il annulé l'appel en cours (`notifications/cancelled`) ? Un outil long le
/// consulte entre deux étapes pour s'arrêter proprement.
pub fn cancelled() -> bool {
    with(|c| c.is_cancelled()).unwrap_or(false)
}

/// Demande à l'humain de confirmer une action (élicitation MCP). `expected` : texte qu'il doit
/// retaper (nom du serveur, par exemple), `None` pour un simple oui/non.
pub async fn confirm(message: &str, expected: Option<&str>) -> Confirmation {
    // Confirmation simple déjà obtenue par l'outil composite qui appelle (voir `preapproved`).
    if expected.is_none() && PREAPPROVED.try_with(|_| ()).is_ok() {
        return Confirmation::Accepted;
    }
    match current() {
        Some(ctx) => ctx.confirm(message, expected).await,
        None => Confirmation::Unsupported,
    }
}

tokio::task_local! {
    static PREAPPROVED: ();
}

/// Exécute `fut` en tenant pour accordées les confirmations **simples** (oui/non) que demanderait
/// un sous-outil : l'outil composite qui l'appelle (`apply_network`) a fait accepter par l'humain,
/// **avant tout effet**, un récapitulatif qui les contient — en MCP sans état, une confirmation
/// demandée après un effet ferait rejouer tout l'appel. Une confirmation par nom retapé
/// (`expected`) n'est jamais pré-accordée.
pub(crate) async fn preapproved<F: Future>(fut: F) -> F::Output {
    PREAPPROVED.scope((), fut).await
}

// --- Interface du transport et aides internes ---------------------------------------------------

/// Champ du formulaire où l'humain retape le texte attendu.
pub(crate) const CONFIRM_FIELD: &str = "confirm_name";
/// Case à cocher d'une confirmation simple (sans texte à retaper).
pub(crate) const CHECK_FIELD: &str = "confirm";
/// Écart minimal entre deux notifications de progression (le dernier pas part toujours).
const MIN_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

tokio::task_local! {
    static CTX: Arc<CallCtx>;
}

/// Ce que le transport sait d'une requête au moment de la traiter.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallInit {
    /// Identifiant de la connexion (une session stdio).
    pub session: String,
    /// `clientInfo.name` (initialisation, ou `_meta` d'une requête sans état).
    pub client: Option<String>,
    /// Version du protocole : négociée à l'initialisation, ou portée par la requête.
    pub version: String,
    /// Requête sans état (`_meta` porte version et capacités ; confirmation par rejeu).
    pub stateless: bool,
    /// Le client a déclaré savoir afficher un formulaire (élicitation `form`).
    pub elicitation: bool,
    /// `params._meta.progressToken`, s'il y en a un.
    pub progress_token: Option<Value>,
    /// Réponses de l'humain jointes au rejeu d'une requête sans état (`params.inputResponses`).
    pub input_responses: Option<Value>,
}

#[derive(Default)]
struct ProgressState {
    last: Option<f64>,
    total: Option<f64>,
    /// Dernière notification, signe de vie compris.
    sent_at: Option<Instant>,
    /// Dernière progression réelle (l'espacement des envois ne tient pas compte des signes de vie).
    reported_at: Option<Instant>,
}

/// Contexte d'UNE requête MCP, posé par le transport autour de son traitement.
pub(crate) struct CallCtx {
    session: String,
    client: Option<String>,
    version: String,
    stateless: bool,
    elicitation: bool,
    progress_token: Option<Value>,
    input_responses: Option<Value>,
    cancel: watch::Receiver<bool>,
    peer: Arc<Peer>,
    progress: Mutex<ProgressState>,
    /// Rang de la prochaine confirmation demandée par cet appel (clé stable d'un rejeu à l'autre).
    confirmations: AtomicU32,
    /// Ère sans état : saisie demandée à l'humain (clé, requête d'élicitation), en attente du rejeu.
    wanted: Mutex<Option<(String, Value)>>,
    wanted_signal: Notify,
    started: Instant,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn current() -> Option<Arc<CallCtx>> {
    CTX.try_with(Arc::clone).ok()
}

fn with<R>(f: impl FnOnce(&CallCtx) -> R) -> Option<R> {
    CTX.try_with(|c| f(c)).ok()
}

/// Exécute `fut` avec le contexte d'appel `ctx` (lu par les fonctions publiques de ce module).
pub(crate) async fn scope<F: Future>(ctx: Arc<CallCtx>, fut: F) -> F::Output {
    CTX.scope(ctx, fut).await
}

/// Le client de l'appel en cours sait-il demander une confirmation à l'humain ?
pub(crate) fn elicitation_supported() -> bool {
    with(|c| c.elicitation && !c.is_cancelled()).unwrap_or(false)
}

/// L'appel en cours est-il une requête sans état (`2026-07-28`) ?
pub(crate) fn stateless() -> bool {
    with(|c| c.stateless).unwrap_or(false)
}

/// Confirmation d'une action destructive par le nom exact de sa cible : l'**humain** la retape si
/// son client sait le lui demander, sinon on garde le texte fourni par le modèle (`given`,
/// l'argument `confirm_name`). Renvoie le texte à passer à la garde de l'outil. `expected` (nom
/// réel, lu à la source) n'est évalué que si l'élicitation est possible.
pub(crate) async fn confirm_or_given<F, Fut>(
    message: &str,
    expected: F,
    given: Option<String>,
) -> Result<String, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let missing = || "paramètre `confirm_name` (texte) requis".to_string();
    if !elicitation_supported() {
        return given.ok_or_else(missing);
    }
    let expected = expected().await?;
    match confirm(message, Some(&expected)).await {
        Confirmation::Accepted => Ok(expected),
        Confirmation::Declined => {
            Err("Refusé par l'utilisateur (confirmation non donnée) : rien n'a été fait.".into())
        }
        Confirmation::Unsupported => given.ok_or_else(missing),
    }
}

/// Relaie la progression d'une opération suivie par `SftpProgress` (compression, extraction,
/// copie par archive) vers le client MCP de l'appel en cours. Sans effet hors transport MCP.
pub(crate) fn sftp_progress(p: &crate::SftpProgress) {
    let Some(ctx) = current() else { return };
    if p.status != "active" {
        return; // la fin (succès ou échec) arrive avec la réponse de l'outil
    }
    let phase = match p.phase.as_deref() {
        Some("queued") => "en attente d'une autre compression",
        Some("compressing") => "compression sur le serveur",
        Some("extracting") => "extraction sur le serveur",
        Some("downloading") => "téléchargement",
        Some("local_zip") => "archive construite par l'appareil",
        Some("cleanup") => "nettoyage",
        _ if p.direction == "up" => "envoi",
        _ => "transfert",
    };
    let mut message = format!("{phase} : {}", p.name);
    if p.files > 0 {
        message.push_str(&format!(", {} fichier(s)", p.files));
    }
    if p.done > 0 {
        message.push_str(&format!(", {}", crate::server_files::human_bytes(p.done)));
        if p.total > 0 {
            message.push_str(&format!(
                " sur {}",
                crate::server_files::human_bytes(p.total)
            ));
        }
    }
    // Plusieurs phases aux unités différentes : un compteur d'étapes, le détail dans le message.
    let step = lock(&ctx.progress).last.map_or(1.0, |l| l.floor() + 1.0);
    ctx.report(step, None, Some(&message));
}

impl CallCtx {
    pub(crate) fn new(init: CallInit, cancel: watch::Receiver<bool>, peer: Arc<Peer>) -> Self {
        Self {
            session: init.session,
            client: init.client,
            version: init.version,
            stateless: init.stateless,
            elicitation: init.elicitation,
            progress_token: init.progress_token,
            input_responses: init.input_responses,
            cancel,
            peer,
            progress: Mutex::new(ProgressState::default()),
            confirmations: AtomicU32::new(0),
            wanted: Mutex::new(None),
            wanted_signal: Notify::new(),
            started: Instant::now(),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub(crate) fn cancel_receiver(&self) -> watch::Receiver<bool> {
        self.cancel.clone()
    }

    /// Ère sans état : attend qu'un outil demande une saisie à l'humain (voir [`Self::take_wanted`]).
    pub(crate) async fn input_wanted(&self) {
        self.wanted_signal.notified().await;
    }

    /// Saisie demandée (clé, requête `elicitation/create`), à renvoyer dans `inputRequests`.
    pub(crate) fn take_wanted(&self) -> Option<(String, Value)> {
        lock(&self.wanted).take()
    }

    /// La version (AAAA-MM-JJ) de l'appel est-elle au moins `min` ?
    fn at_least(&self, min: &str) -> bool {
        self.version.as_str() >= min
    }

    fn report(&self, done: f64, total: Option<f64>, message: Option<&str>) {
        let Some(token) = &self.progress_token else {
            return;
        };
        if !done.is_finite() || self.is_cancelled() {
            return;
        }
        let total = total.filter(|t| t.is_finite() && *t > 0.0);
        let mut st = lock(&self.progress);
        if st.last.is_some_and(|last| done <= last) {
            return;
        }
        let last_step = total.is_some_and(|t| done >= t);
        if !last_step
            && st
                .reported_at
                .is_some_and(|at| at.elapsed() < MIN_PROGRESS_INTERVAL)
        {
            return;
        }
        st.last = Some(done);
        st.total = total;
        st.sent_at = Some(Instant::now());
        st.reported_at = st.sent_at;
        drop(st);
        self.send_progress(token, done, total, message);
    }

    /// Signe de vie d'un appel long sans progression récente (requête avec `progressToken`) :
    /// garde le client à l'écoute (délais d'attente remis à zéro) sans inventer d'avancement — la
    /// valeur ne croît que d'un epsilon, le message dit depuis combien de temps l'appel tourne.
    pub(crate) fn heartbeat(&self, period: Duration) {
        let Some(token) = &self.progress_token else {
            return;
        };
        if self.is_cancelled() {
            return;
        }
        let mut st = lock(&self.progress);
        if st.sent_at.is_some_and(|at| at.elapsed() < period / 2) {
            return;
        }
        let base = st.last.unwrap_or(0.0);
        let next = base + (base.abs() * 1e-9).max(1e-6);
        let total = st.total;
        st.last = Some(next);
        st.sent_at = Some(Instant::now());
        drop(st);
        let message = format!("toujours en cours ({} s)", self.started.elapsed().as_secs());
        self.send_progress(token, next, total, Some(&message));
    }

    fn send_progress(&self, token: &Value, done: f64, total: Option<f64>, message: Option<&str>) {
        let mut params = json!({ "progressToken": token, "progress": done });
        if let Some(t) = total {
            params["total"] = json!(t);
        }
        // `message` n'existe que depuis 2025-03-26.
        if let Some(m) = message.filter(|m| !m.is_empty() && self.at_least("2025-03-26")) {
            params["message"] = json!(m);
        }
        self.peer.notify("notifications/progress", params);
    }

    async fn confirm(&self, message: &str, expected: Option<&str>) -> Confirmation {
        if self.is_cancelled() {
            return Confirmation::Declined;
        }
        if !self.elicitation {
            return Confirmation::Unsupported;
        }
        let params = elicitation_params(message, expected, self.at_least("2025-11-25"));
        let rank = self.confirmations.fetch_add(1, Ordering::Relaxed) + 1;
        let answer = if self.stateless {
            // Rejeu : la réponse de l'humain accompagne la requête. Sinon on la demande et l'appel
            // s'arrête là (le transport répond `input_required` et abandonne cette exécution).
            let key = format!("confirmation-{rank}");
            match self.input_responses.as_ref().and_then(|r| r.get(&key)) {
                Some(result) => Ok(result.clone()),
                None => {
                    *lock(&self.wanted) = Some((
                        key,
                        json!({ "method": "elicitation/create", "params": params }),
                    ));
                    self.wanted_signal.notify_one();
                    return std::future::pending::<Confirmation>().await;
                }
            }
        } else {
            let mut cancel = self.cancel.clone();
            self.peer
                .request("elicitation/create", params, &mut cancel)
                .await
                .map_err(|e| e.to_string())
        };
        match answer {
            Ok(result) => evaluate(&result, expected),
            Err(e) => {
                tracing::info!("confirmation MCP non obtenue ({e}) : refus");
                Confirmation::Declined
            }
        }
    }
}

/// Paramètres de `elicitation/create` (mode formulaire) : un seul champ primitif, celui que la
/// spécification autorise. Le texte attendu n'est **jamais** une valeur par défaut : l'humain le
/// retape. `with_mode` : champ `mode` (depuis 2025-11-25).
pub(crate) fn elicitation_params(message: &str, expected: Option<&str>, with_mode: bool) -> Value {
    let (text, props, required) = match expected {
        Some(exp) => (
            format!("{message}\n\nPour confirmer, retape exactement : « {exp} »"),
            json!({ CONFIRM_FIELD: {
                "type": "string",
                "title": "Confirmation",
                "description": format!("Retape exactement « {exp} » (casse comprise) pour confirmer."),
                "minLength": 1
            } }),
            CONFIRM_FIELD,
        ),
        None => (
            message.to_string(),
            json!({ CHECK_FIELD: {
                "type": "boolean",
                "title": "Confirmer",
                "description": "Coche pour confirmer l'action.",
                "default": false
            } }),
            CHECK_FIELD,
        ),
    };
    let mut params = json!({
        "message": text,
        "requestedSchema": { "type": "object", "properties": props, "required": [required] }
    });
    if with_mode {
        params["mode"] = json!("form");
    }
    params
}

/// Décision d'après la réponse du client (`ElicitResult`) : seule une acceptation explicite, avec
/// le texte exact (espaces de bord ignorés, casse comprise) ou la case cochée, confirme.
pub(crate) fn evaluate(result: &Value, expected: Option<&str>) -> Confirmation {
    if result.get("action").and_then(Value::as_str) != Some("accept") {
        return Confirmation::Declined;
    }
    let content = result.get("content");
    let ok = match expected {
        Some(exp) => {
            let typed = content
                .and_then(|c| c.get(CONFIRM_FIELD))
                .and_then(Value::as_str)
                .unwrap_or_default();
            !exp.trim().is_empty() && typed.trim() == exp.trim()
        }
        None => {
            content
                .and_then(|c| c.get(CHECK_FIELD))
                .and_then(Value::as_bool)
                == Some(true)
        }
    };
    if ok {
        Confirmation::Accepted
    } else {
        Confirmation::Declined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn ctx(
        init: CallInit,
    ) -> (
        Arc<CallCtx>,
        mpsc::UnboundedReceiver<Value>,
        watch::Sender<bool>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let peer = Arc::new(Peer::new(tx, Duration::from_secs(5)));
        let (cancel_tx, cancel_rx) = watch::channel(false);
        (Arc::new(CallCtx::new(init, cancel_rx, peer)), rx, cancel_tx)
    }

    #[tokio::test]
    async fn outside_mcp_everything_is_inert() {
        assert_eq!(session_id(), None);
        assert_eq!(client_name(), None);
        assert!(!cancelled());
        progress(1.0, Some(2.0), "rien").await;
        assert_eq!(
            confirm("Supprimer ?", Some("survie")).await,
            Confirmation::Unsupported
        );
        assert!(!elicitation_supported());
        // Sans élicitation : le texte du modèle, comme avant.
        let given = confirm_or_given(
            "x",
            || async { Ok("survie".to_string()) },
            Some("survie".into()),
        )
        .await;
        assert_eq!(given.as_deref(), Ok("survie"));
        assert!(
            confirm_or_given("x", || async { Ok("survie".to_string()) }, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn context_exposes_session_client_and_cancellation() {
        let init = CallInit {
            session: "mcp-1".into(),
            client: Some("claude-code".into()),
            version: "2025-11-25".into(),
            ..Default::default()
        };
        let (c, _rx, cancel) = ctx(init);
        scope(c, async {
            assert_eq!(session_id().as_deref(), Some("mcp-1"));
            assert_eq!(client_name().as_deref(), Some("claude-code"));
            assert!(!cancelled());
            cancel.send(true).unwrap();
            assert!(cancelled());
            // Annulé : une confirmation ne peut plus être donnée.
            assert_eq!(confirm("x", None).await, Confirmation::Declined);
        })
        .await;
    }

    #[tokio::test]
    async fn progress_needs_a_token_and_only_increases() {
        let (c, mut rx, _cancel) = ctx(CallInit {
            version: "2024-11-05".into(),
            ..Default::default()
        });
        scope(c, progress(1.0, None, "sans jeton")).await;
        assert!(
            rx.try_recv().is_err(),
            "pas de progressToken : rien n'est envoyé"
        );

        let init = CallInit {
            version: "2025-06-18".into(),
            progress_token: Some(json!("tok")),
            ..Default::default()
        };
        let (c, mut rx, _cancel) = ctx(init);
        scope(c, async {
            progress(1.0, Some(4.0), "un quart").await;
            progress(1.0, Some(4.0), "répété").await; // pas croissant : ignoré
            progress(0.5, Some(4.0), "recul").await; // recul : ignoré
            progress(4.0, Some(4.0), "fini").await; // dernier pas : jamais retenu par l'espacement
        })
        .await;
        let first = rx.try_recv().unwrap();
        assert_eq!(first["method"], "notifications/progress");
        assert_eq!(
            first["params"],
            json!({ "progressToken": "tok", "progress": 1.0, "total": 4.0, "message": "un quart" })
        );
        let last = rx.try_recv().unwrap();
        assert_eq!(last["params"]["progress"], 4.0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn progress_message_is_omitted_before_2025_03_26() {
        let init = CallInit {
            version: "2024-11-05".into(),
            progress_token: Some(json!(7)),
            ..Default::default()
        };
        let (c, mut rx, _cancel) = ctx(init);
        scope(c, progress(1.0, None, "texte")).await;
        let n = rx.try_recv().unwrap();
        assert_eq!(n["params"], json!({ "progressToken": 7, "progress": 1.0 }));
    }

    #[tokio::test]
    async fn heartbeat_only_grows_by_an_epsilon_and_keeps_the_total() {
        let init = CallInit {
            version: "2025-11-25".into(),
            progress_token: Some(json!("t")),
            ..Default::default()
        };
        let (c, mut rx, _cancel) = ctx(init);
        c.heartbeat(Duration::ZERO);
        let first = rx.try_recv().unwrap();
        let v = first["params"]["progress"].as_f64().unwrap();
        assert!(v > 0.0 && v < 0.001, "{v}");
        assert!(first["params"]["message"]
            .as_str()
            .unwrap()
            .starts_with("toujours en cours"));
        scope(c.clone(), progress(50.0, Some(100.0), "moitié")).await;
        let _ = rx.try_recv().unwrap();
        // Progression récente : pas de signe de vie.
        c.heartbeat(Duration::from_secs(60));
        assert!(rx.try_recv().is_err());
        c.heartbeat(Duration::ZERO);
        let beat = rx.try_recv().unwrap();
        let p = beat["params"]["progress"].as_f64().unwrap();
        assert!(p > 50.0 && p < 50.001, "{p}");
        assert_eq!(beat["params"]["total"], 100.0);
    }

    #[test]
    fn elicitation_schema_is_flat_primitive_and_never_prefilled() {
        let p = elicitation_params("Supprimer le serveur ?", Some("Survie"), true);
        assert_eq!(p["mode"], "form");
        assert!(p["message"].as_str().unwrap().contains("« Survie »"));
        let schema = &p["requestedSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!([CONFIRM_FIELD]));
        let field = &schema["properties"][CONFIRM_FIELD];
        assert_eq!(field["type"], "string");
        assert!(
            field.get("default").is_none(),
            "le nom attendu n'est jamais pré-rempli"
        );
        for prop in schema["properties"].as_object().unwrap().values() {
            assert!(["string", "number", "integer", "boolean"]
                .contains(&prop["type"].as_str().unwrap()));
        }
        // Avant 2025-11-25 : pas de champ `mode`.
        let old = elicitation_params("Continuer ?", None, false);
        assert!(old.get("mode").is_none());
        assert_eq!(
            old["requestedSchema"]["properties"][CHECK_FIELD]["type"],
            "boolean"
        );
    }

    #[test]
    fn only_an_explicit_exact_acceptance_confirms() {
        let accept = |content: Value| json!({ "action": "accept", "content": content });
        assert_eq!(
            evaluate(
                &accept(json!({ CONFIRM_FIELD: " Survie " })),
                Some("Survie")
            ),
            Confirmation::Accepted
        );
        assert_eq!(
            evaluate(&accept(json!({ CONFIRM_FIELD: "survie" })), Some("Survie")),
            Confirmation::Declined,
            "casse"
        );
        assert_eq!(
            evaluate(&accept(json!({})), Some("Survie")),
            Confirmation::Declined
        );
        assert_eq!(
            evaluate(&accept(json!({ CONFIRM_FIELD: "" })), Some("")),
            Confirmation::Declined,
            "nom vide"
        );
        assert_eq!(
            evaluate(&json!({ "action": "decline" }), Some("Survie")),
            Confirmation::Declined
        );
        assert_eq!(
            evaluate(&json!({ "action": "cancel" }), None),
            Confirmation::Declined
        );
        assert_eq!(
            evaluate(&accept(json!({ CHECK_FIELD: true })), None),
            Confirmation::Accepted
        );
        assert_eq!(
            evaluate(&accept(json!({ CHECK_FIELD: false })), None),
            Confirmation::Declined
        );
    }

    #[tokio::test]
    async fn without_the_capability_the_tool_keeps_its_guard() {
        let init = CallInit {
            version: "2025-11-25".into(),
            elicitation: false,
            ..Default::default()
        };
        let (c, mut rx, _cancel) = ctx(init);
        scope(c, async {
            assert_eq!(
                confirm("x", Some("Survie")).await,
                Confirmation::Unsupported
            );
            let out = confirm_or_given(
                "x",
                || async { Ok("Survie".to_string()) },
                Some("Survie".into()),
            )
            .await;
            assert_eq!(out.as_deref(), Ok("Survie"));
        })
        .await;
        assert!(rx.try_recv().is_err(), "aucune requête envoyée au client");
    }

    #[tokio::test]
    async fn stateless_confirmation_uses_the_replayed_answer() {
        // Rejeu d'une requête sans état : la réponse de l'humain est jointe.
        let init = CallInit {
            version: "2026-07-28".into(),
            stateless: true,
            elicitation: true,
            input_responses: Some(
                json!({ "confirmation-1": { "action": "accept", "content": { CONFIRM_FIELD: "Survie" } } }),
            ),
            ..Default::default()
        };
        let (c, _rx, _cancel) = ctx(init);
        let out = scope(
            c,
            confirm_or_given("Supprimer ?", || async { Ok("Survie".to_string()) }, None),
        )
        .await;
        assert_eq!(out.as_deref(), Ok("Survie"));

        // Refus de l'humain : erreur, quel que soit le texte fourni par le modèle.
        let init = CallInit {
            version: "2026-07-28".into(),
            stateless: true,
            elicitation: true,
            input_responses: Some(json!({ "confirmation-1": { "action": "decline" } })),
            ..Default::default()
        };
        let (c, _rx, _cancel) = ctx(init);
        let out = scope(
            c,
            confirm_or_given(
                "Supprimer ?",
                || async { Ok("Survie".to_string()) },
                Some("Survie".into()),
            ),
        )
        .await;
        assert!(out.unwrap_err().contains("Refusé par l'utilisateur"));
    }

    #[tokio::test]
    async fn stateless_confirmation_without_answer_suspends_the_call() {
        let init = CallInit {
            version: "2026-07-28".into(),
            stateless: true,
            elicitation: true,
            ..Default::default()
        };
        let (c, _rx, _cancel) = ctx(init);
        let call = scope(c.clone(), confirm("Supprimer ?", Some("Survie")));
        tokio::select! {
            _ = call => panic!("l'appel doit rester suspendu"),
            _ = c.input_wanted() => {}
        }
        let (key, request) = c.take_wanted().unwrap();
        assert_eq!(key, "confirmation-1");
        assert_eq!(request["method"], "elicitation/create");
        assert_eq!(request["params"]["mode"], "form");
    }
}
