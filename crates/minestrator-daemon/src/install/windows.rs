//! Windows : service SCM `MinestratorDaemon`, compte `NT AUTHORITY\LocalService`.
//!
//! - Démarrage automatique **différé** (le réseau est prêt) et redémarrage sur échec (y compris
//!   un arrêt volontaire avec code d'erreur : c'est ainsi que l'app demande un redémarrage).
//! - Le binaire exécuté vit toujours sous `Program Files` : un service système qui exécuterait un
//!   binaire modifiable par un utilisateur serait une élévation de privilèges toute trouvée. Si
//!   l'app est installée « pour l'utilisateur » (`%LOCALAPPDATA%`, le mode par défaut de
//!   l'installeur), `install` **recopie** le binaire dans
//!   `%ProgramFiles%\MinestratorTerminal\Service` (écriture réservée aux administrateurs) et
//!   enregistre cette copie ; `uninstall` la retire.
//! - Dossier de données `%ProgramData%\MinestratorTerminal\service` à DACL restreinte
//!   (voir [`crate::winsec`]).

use super::{remove_dir_if_exists, remove_secrets, AdminError, AdminResult, InstallOptions};
use crate::cli::{merge_owners, owners_from_args};
use crate::paths::machine_data_dir;
use minestrator_core::service::{is_under_any, is_valid_sid, WINDOWS_SERVICE_NAME};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
    ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const DISPLAY_NAME: &str = "Minestrator Terminal — service d'arrière-plan";
const DESCRIPTION: &str = "Surveille les serveurs MineStrator (crash, seuils, expiration) même \
quand l'application est fermée. Désinstallation : depuis l'application ou « Applications et \
fonctionnalités ».";
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

fn win_err(e: windows_service::Error) -> AdminError {
    match e {
        windows_service::Error::Winapi(io) => AdminError::from(io),
        other => AdminError::new(other.to_string()),
    }
}

fn not_found(e: &windows_service::Error) -> bool {
    matches!(e, windows_service::Error::Winapi(io) if io.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST))
}

/// Arguments de lancement du service (lus par `main` avant le dispatcher SCM).
pub fn service_args(owners: &[String]) -> Vec<OsString> {
    let mut args = vec![OsString::from("service")];
    for o in owners {
        args.push(OsString::from("--owner"));
        args.push(OsString::from(o));
    }
    args
}

/// Propriétaires déclarés dans la ligne de commande enregistrée par le SCM.
pub fn parse_owners(command_line: &str) -> Vec<String> {
    owners_from_args(command_line.split_whitespace(), is_valid_sid)
}

fn program_files_roots() -> Vec<String> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .collect()
}

/// Dossier de la **copie** du binaire exécutée par le service quand l'app est installée pour
/// l'utilisateur courant : sous `Program Files` (64 bits si disponible), donc protégé.
pub fn service_binary_dir() -> Option<PathBuf> {
    std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .map(|p| PathBuf::from(p).join("MinestratorTerminal").join("Service"))
}

/// Recopie `exe` dans [`service_binary_dir`] et renvoie le chemin de la copie (inchangé si `exe`
/// EST déjà cette copie). Le service qui l'exécute doit être arrêté (fichier verrouillé).
fn stage_binary(exe: &Path) -> AdminResult<PathBuf> {
    let dir = service_binary_dir()
        .ok_or_else(|| AdminError::new("dossier Program Files introuvable"))?;
    std::fs::create_dir_all(&dir)?;
    let name = exe
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| OsString::from("minestrator-daemon.exe"));
    let dest = dir.join(name);
    let same = match (std::fs::canonicalize(exe), std::fs::canonicalize(&dest)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same {
        std::fs::copy(exe, &dest)?;
    }
    Ok(dest)
}

pub fn install(opts: &InstallOptions) -> AdminResult<()> {
    let exe = std::env::current_exe()?;
    // Installation machine de l'app : son binaire est déjà sous Program Files. Sinon (installation
    // « utilisateur », le mode par défaut), le service exécutera une copie protégée.
    let in_place =
        opts.allow_unsafe_path || is_under_any(&exe.to_string_lossy(), &program_files_roots());

    let data = machine_data_dir();
    std::fs::create_dir_all(&data)?;
    crate::winsec::secure_dir(&data)?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(win_err)?;
    let access = ServiceAccess::QUERY_CONFIG
        | ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::QUERY_STATUS;

    let existing = match manager.open_service(WINDOWS_SERVICE_NAME, access) {
        Ok(s) => Some(s),
        Err(e) if not_found(&e) => None,
        Err(e) => return Err(win_err(e)),
    };
    let previous_owners = match &existing {
        Some(s) => s
            .query_config()
            .map(|c| parse_owners(&c.executable_path.to_string_lossy()))
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let owners = merge_owners(&previous_owners, &opts.owners);
    let service_exe = if in_place {
        exe
    } else {
        // La copie remplace le binaire d'un service peut-être en cours : l'arrêter d'abord.
        if let Some(s) = &existing {
            stop_and_wait(s)?;
        }
        stage_binary(&exe)?
    };
    let info = ServiceInfo {
        name: OsString::from(WINDOWS_SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: service_exe,
        launch_arguments: service_args(&owners),
        dependencies: vec![],
        account_name: Some(OsString::from(r"NT AUTHORITY\LocalService")),
        account_password: None,
    };
    let service = match existing {
        Some(s) => {
            s.change_config(&info).map_err(win_err)?;
            s
        }
        None => manager.create_service(&info, access).map_err(win_err)?,
    };
    let _ = service.set_description(DESCRIPTION);
    let _ = service.set_delayed_auto_start(true);
    let restart = |secs| ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(secs),
    };
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart(5), restart(15), restart(60)]),
        })
        .map_err(win_err)?;
    service
        .set_failure_actions_on_non_crash_failures(true)
        .map_err(win_err)?;

    let running = service
        .query_status()
        .is_ok_and(|s| s.current_state != ServiceState::Stopped);
    if opts.start || running {
        // Redémarre pour appliquer la nouvelle configuration (propriétaires, binaire).
        stop_and_wait(&service)?;
        service.start::<&str>(&[]).map_err(win_err)?;
    }
    println!(
        "Service installé : {WINDOWS_SERVICE_NAME} ({} propriétaire(s)).",
        owners.len()
    );
    Ok(())
}

fn stop_and_wait(service: &Service) -> AdminResult<()> {
    let status = service.query_status().map_err(win_err)?;
    if status.current_state == ServiceState::Stopped {
        return Ok(());
    }
    if status.current_state != ServiceState::StopPending {
        let _ = service.stop();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if service
            .query_status()
            .is_ok_and(|s| s.current_state == ServiceState::Stopped)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(AdminError::new("le service ne s'arrête pas (délai dépassé)"))
}

pub fn uninstall(purge: bool) -> AdminResult<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(win_err)?;
    match manager.open_service(
        WINDOWS_SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(service) => {
            stop_and_wait(&service)?;
            service.delete().map_err(win_err)?;
        }
        Err(e) if not_found(&e) => {}
        Err(e) => return Err(win_err(e)),
    }
    // Copie protégée du binaire (installation « utilisateur » de l'app), si elle existe.
    if let Some(dir) = service_binary_dir() {
        match remove_dir_if_exists(&dir) {
            Ok(()) => {
                // Dossier parent `MinestratorTerminal` : retiré s'il est vide.
                if let Some(parent) = dir.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
            Err(e) => eprintln!("binaire du service non supprimé ({}) : {e}", dir.display()),
        }
    }
    let data = machine_data_dir();
    if purge {
        remove_dir_if_exists(&data)?;
        // Dossier parent `MinestratorTerminal` : retiré s'il est vide.
        if let Some(parent) = data.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    } else {
        remove_secrets(&data)?;
    }
    println!("Service désinstallé{}.", if purge { " (données effacées)" } else { "" });
    Ok(())
}

pub fn control(action: &str) -> AdminResult<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(win_err)?;
    let service = manager
        .open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::START | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )
        .map_err(win_err)?;
    match action {
        "stop" => stop_and_wait(&service),
        "start" => {
            let state = service.query_status().map_err(win_err)?.current_state;
            if state == ServiceState::Stopped {
                service.start::<&str>(&[]).map_err(win_err)?;
            }
            Ok(())
        }
        _ => {
            stop_and_wait(&service)?;
            service.start::<&str>(&[]).map_err(win_err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ligne_de_commande_du_service_relue() {
        let owners = vec!["S-1-5-21-1-2-3-1001".to_string()];
        let args = service_args(&owners);
        assert_eq!(args, vec!["service", "--owner", "S-1-5-21-1-2-3-1001"]);
        let cmdline = r#""C:\Program Files\Minestrator Terminal\minestrator-daemon.exe" service --owner S-1-5-21-1-2-3-1001"#;
        assert_eq!(parse_owners(cmdline), owners);
        assert!(parse_owners(r#""C:\x.exe" service"#).is_empty());
    }
}
