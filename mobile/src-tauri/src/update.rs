//! Auto-update mobile : compare la version installée à la **dernière release publiée**
//! (API GitHub Releases) et, si une plus récente porte un APK, permet de la télécharger.
//! L'installation est ensuite lancée via `apk_installer::install_apk` (installeur système Android).

use serde::{Deserialize, Serialize};
use tauri::Manager;

const RELEASES_LATEST: &str =
    "https://api.github.com/repos/Wasabules/MinestratorTerminal/releases/latest";
const USER_AGENT: &str = concat!("MinestratorTerminalMobile/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateInfo {
    /// Version disponible (ex. `0.4.1`).
    pub version: String,
    /// Corps de la release (changelog markdown).
    pub notes: String,
    /// URL de l'APK à télécharger.
    pub apk_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Parse « v0.4.1 » / « 0.4.1 » → (0,4,1) pour comparaison.
fn parse_ver(s: &str) -> (u64, u64, u64) {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split(['.', '-', '+', ' ']);
    let n = |x: Option<&str>| x.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    (n(it.next()), n(it.next()), n(it.next()))
}

fn client() -> Result<reqwest::Client, String> {
    minestrator_core::http::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// Renvoie les infos de MAJ si une release **plus récente** avec un APK existe, sinon `None`.
/// Sur iOS, les MAJ passent par l'App Store (pas d'installation d'APK) → toujours `None`,
/// donc ni bandeau ni bouton « Mettre à jour » côté UI.
#[tauri::command]
pub async fn check_update() -> Result<Option<UpdateInfo>, String> {
    if cfg!(target_os = "ios") {
        return Ok(None);
    }

    let rel: GhRelease = client()?
        .get(RELEASES_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let Some(apk) = pick_apk(&rel.assets, std::env::consts::ARCH) else {
        return Ok(None);
    };

    if parse_ver(&rel.tag_name) > parse_ver(env!("CARGO_PKG_VERSION")) {
        Ok(Some(UpdateInfo {
            version: rel.tag_name.trim_start_matches('v').to_string(),
            notes: rel.body.unwrap_or_default(),
            apk_url: apk.browser_download_url.clone(),
        }))
    } else {
        Ok(None)
    }
}

/// Suffixes d'architecture des APK publiés. Depuis la 0.8.1, la release porte un APK par
/// architecture : `…_android-arm64.apk` (presque tous les téléphones) et `…_android-armv7.apk`
/// (anciens téléphones 32 bits). Les versions ≤ 0.8.0 prennent le premier `.apk` renvoyé par
/// l'API GitHub, qui trie les fichiers par nom : `-arm64` passant avant `-armv7`, elles reçoivent
/// l'APK 64 bits. Un nouveau nom d'APK doit conserver cet ordre.
const ABI_SUFFIXES: &[&str] = &["-armv7", "-x86_64", "-x86"];

/// APK de la release qui convient à l'architecture du téléphone (`std::env::consts::ARCH`).
/// Aucun → pas de mise à jour proposée plutôt qu'un APK que le téléphone refuserait.
fn pick_apk<'a>(assets: &'a [GhAsset], arch: &str) -> Option<&'a GhAsset> {
    let wanted = match arch {
        "arm" => Some("-armv7"),
        "x86_64" => Some("-x86_64"),
        "x86" => Some("-x86"),
        // aarch64 : `-arm64`, ou l'APK universel sans suffixe des releases ≤ 0.8.0.
        _ => None,
    };
    assets.iter().find(|a| {
        let name = a.name.to_lowercase();
        let Some(stem) = name.strip_suffix(".apk") else {
            return false;
        };
        match wanted {
            Some(suffix) => stem.ends_with(suffix),
            None => !ABI_SUFFIXES.iter().any(|s| stem.ends_with(s)),
        }
    })
}

/// Télécharge l'APK dans le cache de l'app (exposé par le FileProvider) et renvoie son chemin.
/// L'appelant JS le passe ensuite à `install_apk` → installeur système Android.
#[tauri::command]
pub async fn download_update(app: tauri::AppHandle, url: String) -> Result<String, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("MinestratorTerminal-update.apk");

    let bytes = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GhAsset {
        GhAsset { name: name.into(), browser_download_url: format!("https://x/{name}") }
    }

    fn release() -> Vec<GhAsset> {
        vec![
            asset("latest.json"),
            asset("Minestrator-Terminal_v0.8.1_android-armv7.apk"),
            asset("Minestrator-Terminal_v0.8.1_android-arm64.apk"),
            asset("Minestrator.Terminal_0.8.1_x64-setup.exe"),
        ]
    }

    #[test]
    fn each_architecture_gets_its_own_apk() {
        let assets = release();
        let name = |arch| pick_apk(&assets, arch).map(|a| a.name.as_str());
        assert_eq!(name("aarch64"), Some("Minestrator-Terminal_v0.8.1_android-arm64.apk"));
        assert_eq!(name("arm"), Some("Minestrator-Terminal_v0.8.1_android-armv7.apk"));
        // Aucun APK x86 publié : pas de mise à jour proposée plutôt qu'un APK refusé.
        assert_eq!(name("x86_64"), None);
        assert_eq!(name("x86"), None);
    }

    #[test]
    fn a_universal_apk_of_an_older_release_still_serves_arm64_only() {
        let assets = vec![asset("Minestrator-Terminal_v0.8.0_android.apk")];
        assert!(pick_apk(&assets, "aarch64").is_some());
        assert!(pick_apk(&assets, "arm").is_none());
    }

    /// L'API GitHub renvoie les fichiers triés par nom (et non dans l'ordre d'envoi) ; les
    /// versions ≤ 0.8.0 installent le premier `.apk` : ce doit être l'APK 64 bits.
    #[test]
    fn older_versions_taking_the_first_apk_by_name_get_arm64() {
        let mut names: Vec<String> =
            release().into_iter().map(|a| a.name.to_lowercase()).collect();
        names.sort();
        let first = names.iter().find(|n| n.ends_with(".apk"));
        assert_eq!(
            first.map(String::as_str),
            Some("minestrator-terminal_v0.8.1_android-arm64.apk")
        );
    }
}
