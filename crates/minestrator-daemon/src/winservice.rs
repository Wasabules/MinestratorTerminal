//! Point d'entrée « service Windows » : dialogue avec le SCM (démarrage, arrêt, extinction).
//!
//! Le SCM appelle `main` avec les arguments enregistrés (`service --owner …`) ; `main` prépare
//! les options puis confie le thread au dispatcher, qui rappelle [`service_main`] sur un thread
//! dédié. Un redémarrage demandé par l'app se traduit par un arrêt avec code d'erreur : les
//! actions de récupération (« redémarrer le service ») le relancent.

use crate::config::EnvConfig;
use crate::daemon::{self, Exit, Options};
use minestrator_core::service::WINDOWS_SERVICE_NAME;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

/// Options préparées par `main`, consommées une fois par `service_main`.
static OPTIONS: Mutex<Option<Options>> = Mutex::new(None);

define_windows_service!(ffi_service_main, service_main);

/// Confie le thread au dispatcher du SCM (bloquant jusqu'à l'arrêt du service).
pub fn run_dispatcher(opts: Options) -> windows_service::Result<()> {
    *OPTIONS.lock().unwrap() = Some(opts);
    service_dispatcher::start(WINDOWS_SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service Windows : {e}");
    }
}

fn status(state: ServiceState, exit_code: ServiceExitCode) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service() -> windows_service::Result<()> {
    let Some(opts) = OPTIONS.lock().unwrap().take() else {
        return Ok(());
    };
    let stop = Arc::new(Notify::new());
    let stop_handler = stop.clone();
    let handle = service_control_handler::register(WINDOWS_SERVICE_NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_handler.notify_one();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::Running, ServiceExitCode::NO_ERROR))?;

    let exit = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt.block_on(daemon::run(opts, EnvConfig::from_env(), async move {
            stop.notified().await;
        })),
        Err(e) => {
            tracing::error!("runtime tokio : {e}");
            Exit::Fatal
        }
    };
    let code = match exit {
        Exit::Stop => ServiceExitCode::NO_ERROR,
        // Code non nul : déclenche les actions de récupération (redémarrage).
        Exit::Restart => ServiceExitCode::ServiceSpecific(1),
        Exit::Fatal => ServiceExitCode::ServiceSpecific(2),
    };
    handle.set_service_status(status(ServiceState::Stopped, code))?;
    Ok(())
}
