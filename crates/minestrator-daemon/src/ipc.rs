//! Serveur IPC du service (socket Unix / canal nommé Windows). Protocole : `core::service`.
//!
//! **Contrôle d'accès** :
//! - Unix : socket `0666`, mais chaque connexion est filtrée sur l'uid du pair (`SO_PEERCRED` /
//!   `getpeereid`) : root, le compte du daemon lui-même, ou un propriétaire `--owner`.
//!   Le dossier du socket n'est inscriptible que par le service (pas d'usurpation possible).
//! - Windows : la DACL du canal ne laisse entrer que SYSTEM, les Administrateurs et les
//!   propriétaires ; l'app vérifie de son côté que le serveur est bien le processus du service.
//!
//! Un client autorisé peut tout faire (y compris transmettre la clé ou redémarrer) : les
//! propriétaires sont choisis par une installation élevée, jamais par le service lui-même.

use crate::daemon::Shared;
use minestrator_core::service::{
    read_frame, write_frame, Request, Response, MAX_REQUEST_BYTES, PROTOCOL_VERSION,
};
use minestrator_core::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::Semaphore;

/// Une connexion inactive plus longtemps est fermée.
const IDLE: Duration = Duration::from_secs(30);
/// Connexions simultanées au plus (au-delà, on attend).
const MAX_CONNECTIONS: usize = 16;
/// Profondeur maximale d'historique servie (rétention du superviseur).
const MAX_HISTORY_S: i64 = 14 * 24 * 3600;

/// Démarre le serveur ; ne rend la main qu'en cas d'erreur fatale (création du point d'écoute).
pub async fn serve(shared: Arc<Shared>, endpoint: String, owners: Vec<String>) -> std::io::Result<()> {
    imp::serve(shared, endpoint, owners).await
}

async fn handle<S>(shared: Arc<Shared>, stream: S, allowed: bool)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    loop {
        let frame = match tokio::time::timeout(IDLE, read_frame(&mut reader, MAX_REQUEST_BYTES)).await {
            Ok(Ok(Some(f))) => f,
            Ok(Err(e)) => {
                let _ = write_frame(&mut write, &error("bad_request", e.to_string())).await;
                return;
            }
            _ => return,
        };
        let response = if !allowed {
            error("forbidden", "compte non autorisé sur le service")
        } else {
            match serde_json::from_slice::<Request>(&frame) {
                Ok(req) => {
                    tracing::debug!(op = req.op(), "requête IPC");
                    dispatch(&shared, req).await
                }
                Err(e) => error("bad_request", format!("requête illisible : {e}")),
            }
        };
        if write_frame(&mut write, &response).await.is_err() || !allowed {
            return;
        }
    }
}

fn error(code: &str, message: impl Into<String>) -> Response {
    Response::Error { code: code.to_string(), message: message.into() }
}

fn core_error(e: Error) -> Response {
    let code = match &e {
        Error::Unauthorized => "invalid_key",
        Error::Service { code, .. } => code.as_str(),
        other => other.kind(),
    };
    error(code, e.to_string())
}

async fn dispatch(shared: &Shared, req: Request) -> Response {
    let core = &shared.core;
    match req {
        Request::Ping => Response::Pong {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        Request::Status => Response::Status(shared.status()),
        Request::History { server_id, since_secs } => {
            match core.metrics(server_id, since_secs.clamp(0, MAX_HISTORY_S)) {
                Ok(samples) => Response::History(samples),
                Err(e) => core_error(e),
            }
        }
        Request::Alerts { after_id, since_ts, limit } => {
            match core.stored_alerts(after_id, since_ts, limit.unwrap_or(200).min(1000)) {
                Ok((items, last_id)) => Response::Alerts { items, last_id },
                Err(e) => core_error(e),
            }
        }
        Request::GetConfig => Response::Config(core.get_supervisor_config()),
        Request::SetConfig { config } => {
            let valid = |v: f64| (0.0..=100.0).contains(&v);
            if !(valid(config.cpu_threshold) && valid(config.ram_threshold) && valid(config.disk_threshold)) {
                return error("bad_request", "seuils hors de [0, 100]");
            }
            core.set_supervisor_config(config);
            Response::Done
        }
        Request::SetKey { key } => {
            let key = key.trim();
            if key.is_empty() {
                return error("bad_request", "clé vide");
            }
            match core.authenticate_and_store(key).await {
                Ok(profile) => {
                    tracing::info!(user = %profile.pseudo, "clé API reçue par IPC et enregistrée");
                    shared.set_account(Some((profile.id, profile.pseudo)));
                    core.supervisor_wake();
                    Response::Done
                }
                Err(e) => {
                    tracing::warn!("clé API reçue par IPC refusée : {e}");
                    core_error(e)
                }
            }
        }
        Request::ClearKey => match core.logout() {
            Ok(()) => {
                tracing::info!("clé API oubliée à la demande de l'app : supervision suspendue");
                shared.set_account(None);
                core.supervisor_wake();
                Response::Done
            }
            Err(e) => core_error(e),
        },
        Request::Restart => {
            if !shared.managed {
                return error("unsupported", "daemon lancé hors gestionnaire de services");
            }
            shared.request_restart();
            Response::Done
        }
        Request::SetAutomations { config } => {
            let (tasks, rules) = (config.tasks.len(), config.restart_rules.len());
            match core.apply_automation_config(config) {
                Ok(()) => {
                    tracing::info!(tasks, rules, "automatisations reçues de l'app");
                    Response::Done
                }
                Err(e) => invalid_input(e),
            }
        }
        Request::SetWebhooks { config, urls } => {
            let n = config.destinations.len();
            // Ni les URL ni l'erreur détaillée (qui ne les contient pas) ne sont journalisées.
            match core.apply_webhooks(config, urls) {
                Ok(()) => {
                    tracing::info!(destinations = n, "destinations d'alertes reçues de l'app");
                    Response::Done
                }
                Err(e) => invalid_input(e),
            }
        }
        Request::Uptime { server_id, from, to } => match core.local_uptime_spans(server_id, from, to) {
            Ok(spans) => Response::Uptime(spans),
            Err(e) => core_error(e),
        },
        Request::AutomationRuns { account_id, server_id, limit } => {
            match core.local_automation_runs(account_id, server_id, limit.unwrap_or(50).min(500)) {
                Ok(runs) => Response::AutomationRuns(runs),
                Err(e) => core_error(e),
            }
        }
    }
}

/// Données refusées à la validation (`bad_request`), erreur de stockage sinon.
fn invalid_input(e: Error) -> Response {
    match e {
        Error::Unexpected(msg) => error("bad_request", msg),
        other => core_error(other),
    }
}

// --- Unix ------------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::path::Path;
    use tokio::net::UnixListener;

    pub async fn serve(shared: Arc<Shared>, endpoint: String, owners: Vec<String>) -> std::io::Result<()> {
        let path = Path::new(&endpoint);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Socket orphelin d'une exécution précédente : on le remplace (jamais un autre fichier).
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_socket() {
                std::fs::remove_file(path)?;
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{endpoint} existe et n'est pas un socket"),
                ));
            }
        }
        let listener = UnixListener::bind(path)?;
        // Le filtrage se fait sur l'uid du pair : le socket peut être ouvert à tous.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
        // SAFETY : geteuid() n'échoue jamais.
        let me = unsafe { libc::geteuid() };
        tracing::info!(%endpoint, "IPC : écoute sur le socket");
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            let (stream, _) = listener.accept().await?;
            let uid = stream.peer_cred().ok().map(|c| c.uid());
            let allowed = uid.is_some_and(|u| u == 0 || u == me || owners.contains(&u.to_string()));
            if !allowed {
                tracing::warn!(?uid, "IPC : connexion refusée (compte non autorisé)");
            }
            let shared = shared.clone();
            let Ok(permit) = permits.clone().acquire_owned().await else { break };
            tokio::spawn(async move {
                handle(shared, stream, allowed).await;
                drop(permit);
            });
        }
        Ok(())
    }
}

// --- Windows ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::winsec::{pipe_sddl, PipeSecurity};
    use tokio::net::windows::named_pipe::ServerOptions;

    pub async fn serve(shared: Arc<Shared>, endpoint: String, owners: Vec<String>) -> std::io::Result<()> {
        let mut allowed: Vec<String> = owners;
        // Le compte du daemon lui-même (LocalService en service, l'utilisateur au premier plan).
        if let Ok(me) = minestrator_core::service::current_owner_id() {
            if !allowed.contains(&me) {
                allowed.push(me);
            }
        }
        let security = PipeSecurity::new(&pipe_sddl(allowed.iter().map(String::as_str)))?;
        let create = |first: bool| {
            // SAFETY : les attributs de sécurité vivent aussi longtemps que `security`.
            unsafe {
                ServerOptions::new()
                    .first_pipe_instance(first)
                    .reject_remote_clients(true)
                    .create_with_security_attributes_raw(&endpoint, security.as_ptr())
            }
        };
        // `first_pipe_instance` : échoue si quelqu'un tient déjà ce nom (usurpation).
        let mut server = create(true)?;
        tracing::info!(%endpoint, "IPC : écoute sur le canal nommé");
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            server.connect().await?;
            let client = server;
            server = create(false)?;
            let shared = shared.clone();
            let Ok(permit) = permits.clone().acquire_owned().await else { break };
            tokio::spawn(async move {
                handle(shared, client, true).await;
                drop(permit);
            });
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;
    pub async fn serve(_: Arc<Shared>, _: String, _: Vec<String>) -> std::io::Result<()> {
        Err(std::io::Error::other("IPC non pris en charge sur cette plateforme"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minestrator_core::service::{call, request, ENDPOINT_ENV};
    use minestrator_core::{
        Alert, AutomationConfig, Core, RestartRule, Schedule, ScheduledTask, SupervisorConfig,
        TaskAction, WebhookConfig, WebhookDestination, WebhookEvent, WebhookKind,
    };

    /// Bout en bout : vrai serveur (socket Unix ou canal nommé avec sa DACL) + client du core.
    /// Sans réseau : seules les opérations locales sont exercées.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ipc_de_bout_en_bout() {
        let tag = format!("{}-{}", std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos());
        let dir = std::env::temp_dir().join(format!("mnstr-daemon-ipc-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let endpoint = if cfg!(windows) {
            format!(r"\\.\pipe\minestrator-daemon-test-{tag}")
        } else {
            dir.join("daemon.sock").to_string_lossy().into_owned()
        };
        // Seul test du crate à créer un `Core` : l'isoler du dossier de données et du trousseau
        // réels. Point de contact explicite → le client ne vérifie pas le PID du service SCM.
        std::env::set_var("MINESTRATOR_DATA_DIR", &dir);
        std::env::set_var("MINESTRATOR_SECRETS_FILE", "1");
        std::env::set_var(ENDPOINT_ENV, &endpoint);

        let core = Arc::new(Core::new());
        let shared = Arc::new(Shared::new(core.clone(), false, &dir, false));
        let server = tokio::spawn(serve(shared, endpoint.clone(), Vec::new()));

        let t = Duration::from_secs(5);
        // Le serveur démarre en tâche de fond : on attend qu'il réponde.
        let mut last = None;
        for _ in 0..50 {
            assert!(!server.is_finished(), "le serveur IPC s'est arrêté au démarrage");
            match request(&endpoint, &Request::Ping, t).await {
                Ok(r @ Response::Pong { .. }) => {
                    last = Some(Ok(r));
                    break;
                }
                other => last = Some(other),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            matches!(last, Some(Ok(Response::Pong { protocol: PROTOCOL_VERSION, .. }))),
            "{last:?}"
        );

        match call(&endpoint, &Request::Status, t).await.unwrap() {
            Response::Status(s) => {
                assert!(!s.has_key);
                assert!(!s.managed);
                assert_eq!(s.account_id, None);
                assert!(!s.is_supervising(0));
            }
            other => panic!("inattendu : {other:?}"),
        }

        // Alertes consignées puis relues avec curseur.
        let alert = Alert {
            server_id: 9,
            server_name: "srv".into(),
            kind: "crash".into(),
            severity: "critical".into(),
            message: "arrêt".into(),
            ts: 1_000,
            details: Some("[ERROR] dernières lignes".into()),
        };
        let id = core.record_alert(&alert).unwrap();
        let req = Request::Alerts { after_id: None, since_ts: None, limit: None };
        match call(&endpoint, &req, t).await.unwrap() {
            Response::Alerts { items, last_id } => {
                assert_eq!(last_id, id);
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].alert.server_id, 9);
                // Le détail joint (logs du crash) traverse l'IPC.
                assert_eq!(items[0].alert.details.as_deref(), Some("[ERROR] dernières lignes"));
            }
            other => panic!("inattendu : {other:?}"),
        }
        let req = Request::Alerts { after_id: Some(id), since_ts: None, limit: None };
        assert!(matches!(
            call(&endpoint, &req, t).await.unwrap(),
            Response::Alerts { ref items, .. } if items.is_empty()
        ));

        // Réglages : validés, appliqués, relus.
        let mut cfg = SupervisorConfig { cpu_threshold: 150.0, ..Default::default() };
        let err = call(&endpoint, &Request::SetConfig { config: cfg.clone() }, t).await.unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "bad_request"));
        cfg.cpu_threshold = 75.0;
        assert!(matches!(
            call(&endpoint, &Request::SetConfig { config: cfg }, t).await.unwrap(),
            Response::Done
        ));
        match call(&endpoint, &Request::GetConfig, t).await.unwrap() {
            Response::Config(c) => assert_eq!(c.cpu_threshold, 75.0),
            other => panic!("inattendu : {other:?}"),
        }

        // Historique vide, clé vide refusée, redémarrage refusé hors service.
        assert!(matches!(
            call(&endpoint, &Request::History { server_id: 9, since_secs: 3600 }, t).await.unwrap(),
            Response::History(ref v) if v.is_empty()
        ));
        let err = call(&endpoint, &Request::SetKey { key: "  ".into() }, t).await.unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "bad_request"));
        let err = call(&endpoint, &Request::Restart, t).await.unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "unsupported"));
        assert!(matches!(call(&endpoint, &Request::ClearKey, t).await.unwrap(), Response::Done));

        // Automatisations : validées comme une saisie de l'UI, appliquées.
        let task = ScheduledTask {
            id: "task-1".into(),
            account_id: 7,
            server_id: 9,
            name: " Nuit ".into(),
            enabled: true,
            action: TaskAction::Commands { commands: vec![" save-all ".into()] },
            schedule: Schedule::Daily { hour: 4, minute: 0 },
            updated_at: 1,
        };
        let config = AutomationConfig {
            restart_rules: vec![RestartRule { account_id: 7, server_id: 9, enabled: true, max_per_hour: 99 }],
            tasks: vec![task.clone()],
        };
        assert!(matches!(
            call(&endpoint, &Request::SetAutomations { config }, t).await.unwrap(),
            Response::Done
        ));
        let applied = core.automation_config();
        assert_eq!(applied.tasks[0].name, "Nuit");
        assert_eq!(applied.restart_rules[0].max_per_hour, 10, "plafond borné");
        let bad = AutomationConfig {
            tasks: vec![ScheduledTask {
                action: TaskAction::Commands { commands: vec!["a\nb".into()] },
                ..task
            }],
            ..Default::default()
        };
        let err = call(&endpoint, &Request::SetAutomations { config: bad }, t).await.unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "bad_request"));

        // Webhooks : URL rangée dans le fichier de secrets du service (dossier temporaire du
        // test, jamais le trousseau), effacée quand la destination disparaît.
        let dest = WebhookDestination {
            id: "wh-test".into(),
            name: "Salon".into(),
            kind: WebhookKind::Discord,
            enabled: true,
            events: vec![WebhookEvent::Crash],
        };
        let url = "https://discord.com/api/webhooks/1/jeton-de-test";
        let req = Request::SetWebhooks {
            config: WebhookConfig { destinations: vec![dest.clone()] },
            urls: [("wh-test".to_string(), url.to_string())].into(),
        };
        assert!(matches!(call(&endpoint, &req, t).await.unwrap(), Response::Done));
        let infos = core.webhooks();
        assert_eq!(infos.len(), 1);
        assert!(infos[0].has_url);
        assert!(!std::fs::read_to_string(dir.join("webhooks.json")).unwrap().contains("jeton-de-test"),
            "l'URL n'est jamais dans la configuration persistée");
        let req = Request::SetWebhooks {
            config: WebhookConfig { destinations: vec![dest.clone()] },
            urls: [("wh-test".to_string(), "https://evil.example/x".to_string())].into(),
        };
        let err = call(&endpoint, &req, t).await.unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "bad_request"));
        let req = Request::SetWebhooks { config: WebhookConfig::default(), urls: Default::default() };
        assert!(matches!(call(&endpoint, &req, t).await.unwrap(), Response::Done));
        assert!(core.webhooks().is_empty());

        // Disponibilité et journal : vides, mais servis.
        let req = Request::Uptime { server_id: 9, from: 0, to: 10_000 };
        assert!(matches!(call(&endpoint, &req, t).await.unwrap(), Response::Uptime(ref v) if v.is_empty()));
        let req = Request::AutomationRuns { account_id: 7, server_id: Some(9), limit: None };
        assert!(matches!(
            call(&endpoint, &req, t).await.unwrap(),
            Response::AutomationRuns(ref v) if v.is_empty()
        ));

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
