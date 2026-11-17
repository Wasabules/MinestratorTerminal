//! `minestrator-daemon` — service d'arrière-plan de Minestrator Terminal.
//!
//! Réutilise `minestrator-core` exactement comme le desktop : il crée un `Core`, démarre le
//! **superviseur** (crash, seuils CPU/RAM/disque, expiration MyBox), **consigne** chaque alerte,
//! la sert à l'app par **IPC** (socket Unix / canal nommé Windows) et la pousse en **FCM** si
//! configuré. Un seul binaire pour les trois OS :
//!
//! - `run` (par défaut) : au premier plan, configuré par l'environnement (VPS, conteneur, dev) ;
//! - `service` : lancé par systemd (Linux), launchd (macOS) ou le SCM (Windows) ;
//! - `install` / `enable` / `uninstall` / `start` / `stop` / `restart` : administration, appelée
//!   par les installeurs et par l'app (élévation) ;
//! - `status` : état lisible ou JSON, sans privilège.
//!
//! Conception : `docs/SERVICE.md`.

mod cli;
mod config;
mod daemon;
mod fcm;
mod install;
mod ipc;
mod paths;
mod tokens;
#[cfg(windows)]
mod winservice;
mod winsec;

use cli::Command;
use config::EnvConfig;
use daemon::{Exit, Options};
use minestrator_core::service::{default_endpoint, query_install_state, request, Request, Response};
use std::path::Path;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    let code = match command {
        Command::Help => {
            println!("{}", cli::USAGE);
            0
        }
        Command::Version => {
            println!("minestrator-daemon {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Run { owners } => run_daemon(false, owners),
        Command::Service { owners } => run_daemon(true, owners),
        Command::Install { owners, start, allow_unsafe_path } => admin(install::install(
            &install::InstallOptions { owners, start, allow_unsafe_path },
        )),
        Command::Uninstall { purge } => admin(install::uninstall(purge)),
        Command::Start => admin(install::control("start")),
        Command::Stop => admin(install::control("stop")),
        Command::Restart => admin(install::control("restart")),
        Command::Status { json } => status(json),
    };
    std::process::exit(code);
}

fn admin(result: install::AdminResult<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Erreur : {}", e.message);
            e.exit_code
        }
    }
}

fn run_daemon(managed: bool, owners: Vec<String>) -> i32 {
    let data_dir = match paths::resolve_data_dir(managed) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Configuration invalide : {e}\nVoir crates/minestrator-daemon/README.md");
            return 1;
        }
    };
    // Avant tout thread : le core lit ces variables à la construction du `Core`. Secrets en
    // fichier dans le dossier machine (pas de trousseau utilisateur pour un service).
    std::env::set_var("MINESTRATOR_SECRETS_FILE", "1");
    std::env::set_var("MINESTRATOR_DATA_DIR", &data_dir);
    if let Err(e) = paths::prepare_data_dir(&data_dir, managed) {
        eprintln!("Dossier de données {} inutilisable : {e}", data_dir.display());
        return 1;
    }
    init_tracing(managed, &data_dir);

    let opts = Options {
        managed,
        owners,
        data_dir,
        endpoint: default_endpoint(),
    };

    #[cfg(windows)]
    if managed {
        return match winservice::run_dispatcher(opts) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!(
                    "« service » est réservé au gestionnaire de services Windows ({e}). \
                     Au premier plan : `minestrator-daemon run`."
                );
                1
            }
        };
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("runtime tokio : {e}");
            return 1;
        }
    };
    match runtime.block_on(daemon::run(opts, EnvConfig::from_env(), shutdown_signal())) {
        // systemd (`Restart=always`) et launchd (`KeepAlive`) relancent après toute sortie.
        Exit::Stop | Exit::Restart => 0,
        Exit::Fatal => 1,
    }
}

/// Ctrl+C, et SIGTERM sous Unix (`systemctl stop`, `launchctl bootout`).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Journaux : stderr (journald sous systemd, `StandardErrorPath` sous launchd), ou fichier pour
/// le service Windows (session 0, sans console). Indépendant des journaux de l'app.
fn init_tracing(managed: bool, data_dir: &Path) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("minestrator_daemon=info,minestrator_core=info,warn"));
    if cfg!(windows) && managed {
        const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
        let path = paths::log_file(data_dir);
        let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(!too_big)
            .write(true)
            .truncate(too_big)
            .open(&path);
        if let Ok(file) = file {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init();
            return;
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// `status` : état côté OS + état rapporté par le service (si le canal est accessible).
fn status(json: bool) -> i32 {
    let state = query_install_state();
    let response = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
        .and_then(|rt| {
            rt.block_on(request(&default_endpoint(), &Request::Status, Duration::from_secs(3)))
                .map_err(|e| e.to_string())
        });
    let (svc, link) = match response {
        Ok(Response::Status(s)) => (Some(s), "ok".to_string()),
        Ok(Response::Error { code, message }) => (None, format!("{code} : {message}")),
        Ok(other) => (None, format!("réponse inattendue : {other:?}")),
        Err(e) => (None, e),
    };
    if json {
        let v = serde_json::json!({ "install_state": state.as_str(), "link": link, "status": svc });
        println!("{v}");
        return 0;
    }
    let state_label = match state.as_str() {
        "running" => "en marche",
        "stopped" => "arrêté",
        "not_installed" => "non installé",
        _ => "inconnu",
    };
    println!("Service      : {state_label}");
    match svc {
        Some(s) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            println!("Canal IPC    : joignable ({})", default_endpoint());
            println!(
                "Version      : {} (pid {}, démarré il y a {} s)",
                s.version,
                s.pid,
                now - s.started_at
            );
            match (&s.account_name, s.account_id) {
                (Some(name), Some(id)) if s.has_key => {
                    println!("Clé API      : enregistrée — compte « {name} » (#{id})")
                }
                _ if s.has_key => println!("Clé API      : enregistrée (compte non résolu)"),
                _ => println!("Clé API      : absente — activer depuis l'app"),
            }
            let poll = s
                .heartbeat
                .last_poll
                .map(|t| format!("dernier poll il y a {} s", now - t))
                .unwrap_or_else(|| "aucun poll".to_string());
            println!(
                "Supervision  : {} — {} serveur(s), {poll}",
                if s.is_supervising(now) { "active" } else { "inactive" },
                s.heartbeat.monitored
            );
            println!("Données      : {}", s.data_dir);
            println!("Push FCM     : {}", if s.fcm { "configuré" } else { "non configuré" });
        }
        None => println!("Canal IPC    : {link}"),
    }
    0
}
