//! Auto-update mobile : compare la version installée à la **dernière release publiée**
//! (API GitHub Releases) et, si une plus récente porte un APK, permet de la télécharger.
//! L'installation est ensuite lancée via `apk_installer::install_apk` (installeur système Android).

use serde::{Deserialize, Serialize};
use tauri::Manager;

const RELEASES_LATEST: &str =
    "https://api.github.com/repos/Wasabules/MinestratorTerminal/releases/latest";
const USER_AGENT: &str = concat!("MinestratorTerminalMobile/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize)]
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
    let mut it = s.split(|c: char| c == '.' || c == '-' || c == '+' || c == ' ');
    let n = |x: Option<&str>| x.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    (n(it.next()), n(it.next()), n(it.next()))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
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

    let Some(apk) = rel
        .assets
        .iter()
        .find(|a| a.name.to_lowercase().ends_with(".apk"))
    else {
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
