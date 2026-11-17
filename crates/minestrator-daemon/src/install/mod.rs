//! Installation / désinstallation / pilotage du service (commandes d'administration).
//!
//! Appelées par les installeurs (hooks NSIS, scripts deb/rpm) et par l'app via une élévation
//! (UAC, `pkexec`, `osascript`). Les **propriétaires** autorisés sur le canal IPC sont écrits dans
//! la définition du service (arguments `--owner`), gérée par root/admin : le service ne peut pas
//! s'en ajouter lui-même. Une réinstallation conserve les propriétaires existants.

pub mod launchd;
pub mod systemd;
#[cfg(windows)]
pub mod windows;

use minestrator_core::service::InstallState;

/// Options d'installation.
pub struct InstallOptions {
    pub owners: Vec<String>,
    pub start: bool,
    pub allow_unsafe_path: bool,
}

/// Erreur d'administration, avec le code de sortie du processus.
#[derive(Debug)]
pub struct AdminError {
    pub exit_code: i32,
    pub message: String,
}

/// Codes de sortie (lus par l'app après une élévation).
pub const EXIT_FAILURE: i32 = 1;
/// Droits administrateur requis.
pub const EXIT_NOT_PRIVILEGED: i32 = 4;

impl AdminError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { exit_code: EXIT_FAILURE, message: message.into() }
    }
    pub fn code(exit_code: i32, message: impl Into<String>) -> Self {
        Self { exit_code, message: message.into() }
    }
}

impl From<std::io::Error> for AdminError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            AdminError::code(EXIT_NOT_PRIVILEGED, format!("droits administrateur requis : {e}"))
        } else {
            AdminError::new(e.to_string())
        }
    }
}

pub type AdminResult<T> = Result<T, AdminError>;

#[cfg(unix)]
fn require_root() -> AdminResult<()> {
    if crate::paths::is_root() {
        Ok(())
    } else {
        Err(AdminError::code(
            EXIT_NOT_PRIVILEGED,
            "à lancer en root (sudo, pkexec)",
        ))
    }
}

pub fn install(opts: &InstallOptions) -> AdminResult<()> {
    #[cfg(target_os = "linux")]
    {
        require_root()?;
        let _ = opts.allow_unsafe_path;
        systemd::install(opts)
    }
    #[cfg(target_os = "macos")]
    {
        require_root()?;
        let _ = opts.allow_unsafe_path;
        launchd::install(opts)
    }
    #[cfg(windows)]
    {
        windows::install(opts)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = opts;
        Err(AdminError::new("plateforme sans gestionnaire de services pris en charge"))
    }
}

pub fn uninstall(purge: bool) -> AdminResult<()> {
    #[cfg(target_os = "linux")]
    {
        require_root()?;
        systemd::uninstall(purge)
    }
    #[cfg(target_os = "macos")]
    {
        require_root()?;
        launchd::uninstall(purge)
    }
    #[cfg(windows)]
    {
        windows::uninstall(purge)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = purge;
        Err(AdminError::new("plateforme sans gestionnaire de services pris en charge"))
    }
}

/// `start` | `stop` | `restart` du service installé (sans effet s'il n'est pas installé).
pub fn control(action: &str) -> AdminResult<()> {
    if minestrator_core::service::query_install_state() == InstallState::NotInstalled {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        require_root()?;
        systemd::control(action)
    }
    #[cfg(target_os = "macos")]
    {
        require_root()?;
        launchd::control(action)
    }
    #[cfg(windows)]
    {
        windows::control(action)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = action;
        Err(AdminError::new("plateforme sans gestionnaire de services pris en charge"))
    }
}

/// Supprime un fichier s'il existe.
#[allow(dead_code)]
fn remove_if_exists(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Efface la clé API machine : `secrets.json`, et le fichier temporaire d'une écriture atomique
/// interrompue, qui peut lui aussi contenir la clé.
#[allow(dead_code)]
fn remove_secrets(data_dir: &std::path::Path) -> std::io::Result<()> {
    remove_if_exists(&data_dir.join("secrets.json"))?;
    remove_if_exists(&data_dir.join("secrets.json.tmp"))
}

/// Supprime un dossier et son contenu s'il existe.
#[allow(dead_code)]
fn remove_dir_if_exists(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Lance une commande système et échoue proprement si elle échoue.
#[allow(dead_code)]
fn run(program: &str, args: &[&str]) -> AdminResult<()> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| AdminError::new(format!("{program} : {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(AdminError::new(format!(
            "{program} {} : {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}
