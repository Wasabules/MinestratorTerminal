//! Emplacements « machine » du service et préparation du dossier de données.
//!
//! | OS | Données + secrets | Binaire du service |
//! |---|---|---|
//! | Linux | `/var/lib/minestrator-daemon` (`StateDirectory=`, `DynamicUser=` → `/var/lib/private/…`) | `/usr/bin` (paquet) ou `/usr/local/lib/minestrator-daemon/` (AppImage) |
//! | macOS | `/Library/Application Support/MinestratorTerminal/service` (0700, root) | `/Library/PrivilegedHelperTools/<label>` |
//! | Windows | `%ProgramData%\MinestratorTerminal\service` (DACL : SYSTEM, Administrateurs, LocalService) | dossier d'installation (`Program Files`) |

use std::path::{Path, PathBuf};

/// Dossier de données du service installé.
pub fn machine_data_dir() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        base.join("MinestratorTerminal").join("service")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/MinestratorTerminal/service")
    } else {
        PathBuf::from("/var/lib/minestrator-daemon")
    }
}

/// Le processus a-t-il les droits administrateur (Unix : euid 0) ?
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY : geteuid() n'échoue jamais.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Dossier de données effectif : `MINESTRATOR_DATA_DIR` s'il est défini (l'unité systemd et le
/// plist le posent), sinon l'emplacement machine pour un service ou un daemon root, sinon le
/// dossier de données de l'utilisateur. **Jamais** le répertoire temporaire (partagé).
pub fn resolve_data_dir(managed: bool) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("MINESTRATOR_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if managed || is_root() {
        return Ok(machine_data_dir());
    }
    directories::ProjectDirs::from("com", "geoffreylecoq", "MinestratorTerminal")
        .map(|d| d.data_dir().join("daemon"))
        .ok_or_else(|| {
            "aucun dossier de données : définir MINESTRATOR_DATA_DIR".to_string()
        })
}

/// Crée le dossier de données et en restreint l'accès (0700 sous Unix ; DACL protégée sous
/// Windows pour le service). Les secrets (`secrets.json`) y sont écrits par le core.
pub fn prepare_data_dir(dir: &Path, managed: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = managed;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        if managed {
            crate::winsec::secure_dir(dir)?;
        }
    }
    Ok(())
}

/// Journal fichier du service Windows (session 0 : pas de console). Ailleurs : stderr
/// (journald sous systemd, `StandardErrorPath` sous launchd).
pub fn log_file(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dossier_machine_hors_temporaire() {
        let dir = machine_data_dir();
        assert!(!dir.starts_with(std::env::temp_dir()));
        if cfg!(windows) {
            assert!(dir.ends_with(r"MinestratorTerminal\service"));
        } else if cfg!(target_os = "linux") {
            assert_eq!(dir, PathBuf::from("/var/lib/minestrator-daemon"));
        }
    }
}
