#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
//! macOS : LaunchDaemon (`/Library/LaunchDaemons/<label>.plist`), exécuté par root.
//!
//! Le binaire est recopié dans `/Library/PrivilegedHelperTools` (réservé à root) : le plist ne
//! doit pas pointer dans un `.app` que l'utilisateur peut déplacer, remplacer ou supprimer.

use minestrator_core::service::LAUNCHD_LABEL;
use std::path::Path;

pub const HELPER_PATH: &str =
    "/Library/PrivilegedHelperTools/com.geoffreylecoq.minestratorterminal.daemon";
pub const DATA_DIR: &str = "/Library/Application Support/MinestratorTerminal/service";
pub const LOG_DIR: &str = "/Library/Logs/MinestratorTerminal";
pub const LOG_PATH: &str = "/Library/Logs/MinestratorTerminal/daemon.log";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Contenu du plist.
pub fn render_plist(exec: &Path, owners: &[String]) -> String {
    let mut args = vec![exec.to_string_lossy().into_owned(), "service".to_string()];
    for o in owners {
        args.push("--owner".to_string());
        args.push(o.clone());
    }
    let args_xml: String = args
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", xml_escape(a)))
        .collect();
    format!(
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<!-- Généré par `minestrator-daemon install` : réécrit à chaque installation. -->
<plist version=\"1.0\">
<dict>
\t<key>Label</key>
\t<string>{label}</string>
\t<key>ProgramArguments</key>
\t<array>
{args_xml}\t</array>
\t<key>EnvironmentVariables</key>
\t<dict>
\t\t<key>MINESTRATOR_DATA_DIR</key>
\t\t<string>{data}</string>
\t</dict>
\t<key>RunAtLoad</key>
\t<true/>
\t<key>KeepAlive</key>
\t<true/>
\t<key>ThrottleInterval</key>
\t<integer>10</integer>
\t<key>Umask</key>
\t<integer>63</integer>
\t<key>StandardOutPath</key>
\t<string>{log}</string>
\t<key>StandardErrorPath</key>
\t<string>{log}</string>
</dict>
</plist>
",
        label = xml_escape(LAUNCHD_LABEL),
        data = xml_escape(DATA_DIR),
        log = xml_escape(LOG_PATH),
    )
}

/// Propriétaires déclarés dans un plist généré (`ProgramArguments`).
pub fn parse_owners(plist: &str) -> Vec<String> {
    let Some(start) = plist.find("<key>ProgramArguments</key>") else {
        return Vec::new();
    };
    let rest = &plist[start..];
    let end = rest.find("</array>").unwrap_or(rest.len());
    let items: Vec<String> = rest[..end]
        .split("<string>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</string>").next())
        .map(xml_unescape)
        .collect();
    crate::cli::owners_from_args(items.iter().map(String::as_str), minestrator_core::service::is_valid_uid)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::cli::merge_owners;
    use crate::install::{
        remove_dir_if_exists, remove_if_exists, remove_secrets, run, AdminResult, InstallOptions,
    };
    use minestrator_core::service::LAUNCHD_PLIST_PATH;
    use std::os::unix::fs::PermissionsExt;

    fn target() -> String {
        format!("system/{LAUNCHD_LABEL}")
    }

    pub fn install(opts: &InstallOptions) -> AdminResult<()> {
        let exe = std::env::current_exe()?.canonicalize()?;
        if exe != Path::new(HELPER_PATH) {
            let tmp = format!("{HELPER_PATH}.new");
            std::fs::create_dir_all("/Library/PrivilegedHelperTools")?;
            std::fs::copy(&exe, &tmp)?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(&tmp, HELPER_PATH)?;
        }
        std::fs::create_dir_all(DATA_DIR)?;
        std::fs::set_permissions(DATA_DIR, std::fs::Permissions::from_mode(0o700))?;
        std::fs::create_dir_all(LOG_DIR)?;

        let existing = std::fs::read_to_string(LAUNCHD_PLIST_PATH)
            .map(|p| parse_owners(&p))
            .unwrap_or_default();
        let owners = merge_owners(&existing, &opts.owners);
        std::fs::write(LAUNCHD_PLIST_PATH, render_plist(Path::new(HELPER_PATH), &owners))?;
        std::fs::set_permissions(LAUNCHD_PLIST_PATH, std::fs::Permissions::from_mode(0o644))?;

        // Recharge : applique nouveau plist et nouveau binaire. Sans `--no-start`, le daemon
        // démarre aussitôt (RunAtLoad) ; sinon il démarrera au prochain boot.
        let _ = run("launchctl", &["bootout", &target()]);
        if opts.start {
            run("launchctl", &["bootstrap", "system", LAUNCHD_PLIST_PATH])?;
            let _ = run("launchctl", &["enable", &target()]);
        }
        println!("Service installé : {LAUNCHD_LABEL} ({} propriétaire(s)).", owners.len());
        Ok(())
    }

    pub fn uninstall(purge: bool) -> AdminResult<()> {
        let _ = run("launchctl", &["bootout", &target()]);
        remove_if_exists(Path::new(LAUNCHD_PLIST_PATH))?;
        remove_if_exists(Path::new(HELPER_PATH))?;
        if purge {
            remove_dir_if_exists(Path::new(DATA_DIR))?;
            remove_dir_if_exists(Path::new(LOG_DIR))?;
        } else {
            remove_secrets(Path::new(DATA_DIR))?;
        }
        println!("Service désinstallé{}.", if purge { " (données effacées)" } else { "" });
        Ok(())
    }

    pub fn control(action: &str) -> AdminResult<()> {
        match action {
            "start" => run("launchctl", &["bootstrap", "system", LAUNCHD_PLIST_PATH]),
            "stop" => run("launchctl", &["bootout", &target()]),
            _ => run("launchctl", &["kickstart", "-k", &target()]),
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{control, install, uninstall};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_genere_et_relu() {
        let owners = vec!["501".to_string()];
        let plist = render_plist(Path::new(HELPER_PATH), &owners);
        assert!(plist.contains("<string>com.geoffreylecoq.minestratorterminal.daemon</string>"));
        assert!(plist.contains("\t\t<string>service</string>\n\t\t<string>--owner</string>\n\t\t<string>501</string>\n"));
        assert!(plist.contains("<key>KeepAlive</key>\n\t<true/>"));
        assert!(plist.contains(
            "<string>/Library/Application Support/MinestratorTerminal/service</string>"
        ));
        assert_eq!(parse_owners(&plist), owners);
    }

    #[test]
    fn plist_echappe_le_xml() {
        let plist = render_plist(Path::new("/tmp/a&b<c>/daemon"), &[]);
        assert!(plist.contains("<string>/tmp/a&amp;b&lt;c&gt;/daemon</string>"));
        assert!(parse_owners(&plist).is_empty());
        assert!(parse_owners("<plist></plist>").is_empty());
    }
}
