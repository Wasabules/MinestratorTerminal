#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! Linux : unité systemd.
//!
//! Le service tourne sous un **utilisateur dynamique** (`DynamicUser=yes`) sans aucun privilège :
//! il n'a besoin que du réseau, de son dossier d'état et de son socket. systemd crée
//! `/var/lib/minestrator-daemon` (lien vers `/var/lib/private/…`, 0700) et
//! `/run/minestrator-daemon` ; la clé API (`secrets.json`, 0600) n'est lisible que par lui et root.

use std::path::Path;

/// Binaire recopié quand l'app n'est pas installée par paquet (AppImage) : un chemin stable et
/// réservé à root, contrairement au montage FUSE éphémère de l'AppImage.
pub const HELPER_DIR: &str = "/usr/local/lib/minestrator-daemon";
pub const HELPER_PATH: &str = "/usr/local/lib/minestrator-daemon/minestrator-daemon";
/// Dossier d'état (`StateDirectory=`) et sa cible réelle avec `DynamicUser=`.
pub const STATE_DIR: &str = "/var/lib/minestrator-daemon";
pub const PRIVATE_STATE_DIR: &str = "/var/lib/private/minestrator-daemon";

/// Contenu de l'unité. `exec` et `owners` sont validés en amont (chemin sans espace, uids
/// numériques) : rien ne peut s'injecter dans `ExecStart=`.
pub fn render_unit(exec: &Path, owners: &[String]) -> String {
    let mut exec_start = format!("{} service", exec.display());
    for o in owners {
        exec_start.push_str(" --owner ");
        exec_start.push_str(o);
    }
    format!(
        "\
# Généré par `minestrator-daemon install` — ne pas modifier à la main (réécrit à chaque
# installation). Réglages locaux : `systemctl edit minestrator-daemon` (ex. variables FCM_*).
[Unit]
Description=Minestrator Terminal — service de supervision
Documentation=https://github.com/Wasabules/MinestratorTerminal/blob/main/docs/SERVICE.md
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
ExecStart={exec_start}
Environment=MINESTRATOR_DATA_DIR={STATE_DIR}
DynamicUser=yes
StateDirectory=minestrator-daemon
StateDirectoryMode=0700
RuntimeDirectory=minestrator-daemon
RuntimeDirectoryMode=0755
UMask=0077
Restart=always
RestartSec=5
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RestrictSUIDSGID=yes
LockPersonality=yes

[Install]
WantedBy=multi-user.target
"
    )
}

/// Propriétaires déclarés dans une unité générée (ligne `ExecStart=`).
pub fn parse_owners(unit: &str) -> Vec<String> {
    unit.lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))
        .map(|cmd| {
            crate::cli::owners_from_args(cmd.split_whitespace(), minestrator_core::service::is_valid_uid)
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use crate::cli::merge_owners;
    use crate::install::{
        remove_dir_if_exists, remove_if_exists, remove_secrets, run, AdminError, AdminResult,
        InstallOptions,
    };
    use minestrator_core::service::{SYSTEMD_UNIT, SYSTEMD_UNIT_PATH};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn require_systemd() -> AdminResult<()> {
        if Path::new("/run/systemd/system").is_dir() {
            Ok(())
        } else {
            Err(AdminError::new("systemd introuvable : service non installable sur ce système"))
        }
    }

    /// Binaire à référencer : celui du paquet tel quel, sinon une copie dans [`HELPER_PATH`].
    fn resolve_binary() -> AdminResult<PathBuf> {
        let exe = std::env::current_exe()?.canonicalize()?;
        if exe.starts_with("/usr/bin") || exe.starts_with("/usr/lib") || exe == Path::new(HELPER_PATH) {
            return Ok(exe);
        }
        std::fs::create_dir_all(HELPER_DIR)?;
        std::fs::set_permissions(HELPER_DIR, std::fs::Permissions::from_mode(0o755))?;
        let tmp = PathBuf::from(format!("{HELPER_PATH}.new"));
        std::fs::copy(&exe, &tmp)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, HELPER_PATH)?;
        Ok(PathBuf::from(HELPER_PATH))
    }

    pub fn install(opts: &InstallOptions) -> AdminResult<()> {
        require_systemd()?;
        let exec = resolve_binary()?;
        if exec.to_string_lossy().contains(char::is_whitespace) {
            return Err(AdminError::new("chemin du binaire avec espace : non pris en charge"));
        }
        let existing = std::fs::read_to_string(SYSTEMD_UNIT_PATH)
            .map(|u| parse_owners(&u))
            .unwrap_or_default();
        let owners = merge_owners(&existing, &opts.owners);
        let was_active = is_active();
        std::fs::write(SYSTEMD_UNIT_PATH, render_unit(&exec, &owners))?;
        std::fs::set_permissions(SYSTEMD_UNIT_PATH, std::fs::Permissions::from_mode(0o644))?;
        run("systemctl", &["daemon-reload"])?;
        run("systemctl", &["enable", SYSTEMD_UNIT])?;
        if opts.start || was_active {
            // `restart` démarre aussi un service arrêté et applique nouvelle unité et binaire.
            run("systemctl", &["restart", SYSTEMD_UNIT])?;
        }
        println!("Service installé : {SYSTEMD_UNIT} ({} propriétaire(s)).", owners.len());
        Ok(())
    }

    pub fn uninstall(purge: bool) -> AdminResult<()> {
        if Path::new(SYSTEMD_UNIT_PATH).exists() {
            let _ = run("systemctl", &["disable", "--now", SYSTEMD_UNIT]);
            remove_if_exists(Path::new(SYSTEMD_UNIT_PATH))?;
            let _ = run("systemctl", &["daemon-reload"]);
        }
        remove_dir_if_exists(Path::new(HELPER_DIR))?;
        // La clé API part toujours avec le service ; l'historique seulement sur `--purge`.
        for dir in [STATE_DIR, PRIVATE_STATE_DIR] {
            if purge {
                remove_dir_if_exists(Path::new(dir))?;
            } else {
                remove_secrets(Path::new(dir))?;
            }
        }
        println!("Service désinstallé{}.", if purge { " (données effacées)" } else { "" });
        Ok(())
    }

    pub fn control(action: &str) -> AdminResult<()> {
        run("systemctl", &[action, SYSTEMD_UNIT])
    }

    fn is_active() -> bool {
        std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", SYSTEMD_UNIT])
            .status()
            .is_ok_and(|s| s.success())
    }
}

#[cfg(target_os = "linux")]
pub use imp::{control, install, uninstall};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unite_generee_et_relue() {
        let owners = vec!["1000".to_string(), "1001".to_string()];
        let unit = render_unit(Path::new("/usr/bin/minestrator-daemon"), &owners);
        assert!(unit.contains(
            "ExecStart=/usr/bin/minestrator-daemon service --owner 1000 --owner 1001\n"
        ));
        assert!(unit.contains("DynamicUser=yes"));
        assert!(unit.contains("StateDirectory=minestrator-daemon"));
        assert!(unit.contains("RuntimeDirectory=minestrator-daemon"));
        assert!(unit.contains("Environment=MINESTRATOR_DATA_DIR=/var/lib/minestrator-daemon"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert_eq!(parse_owners(&unit), owners);
    }

    #[test]
    fn unite_sans_proprietaire() {
        let unit = render_unit(Path::new("/usr/local/lib/minestrator-daemon/minestrator-daemon"), &[]);
        assert!(unit.contains("ExecStart=/usr/local/lib/minestrator-daemon/minestrator-daemon service\n"));
        assert!(parse_owners(&unit).is_empty());
        assert!(parse_owners("[Unit]\nDescription=x\n").is_empty());
    }
}
