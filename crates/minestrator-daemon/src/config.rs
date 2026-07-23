//! Configuration du daemon, lue depuis l'environnement.

use std::path::PathBuf;

pub struct DaemonConfig {
    /// Clé API MineStrator (validée au démarrage).
    pub api_key: String,
    /// Dossier de données/secrets du daemon (backend fichier headless).
    pub data_dir: PathBuf,
    /// Config FCM ; `None` = pas de push (les alertes sont seulement journalisées).
    pub fcm: Option<FcmConfig>,
    /// Fichier JSON des tokens d'appareil (tableau de chaînes), relu à chaque alerte.
    pub device_tokens_file: Option<PathBuf>,
}

pub struct FcmConfig {
    pub project_id: String,
    pub access_token_source: AccessTokenSource,
}

/// D'où vient l'access token OAuth2 de l'API FCM v1.
pub enum AccessTokenSource {
    /// Variable `FCM_ACCESS_TOKEN`.
    Env,
    /// Fichier `FCM_ACCESS_TOKEN_FILE` (rafraîchi par un helper externe).
    File(PathBuf),
}

impl DaemonConfig {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("MINESTRATOR_API_KEY")
            .map_err(|_| "MINESTRATOR_API_KEY manquant (clé API MineStrator).".to_string())?;
        if api_key.trim().is_empty() {
            return Err("MINESTRATOR_API_KEY est vide.".to_string());
        }

        let data_dir = std::env::var_os("MINESTRATOR_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("minestrator-daemon"));

        let fcm = match std::env::var("FCM_PROJECT_ID") {
            Ok(project_id) if !project_id.trim().is_empty() => {
                let access_token_source = match std::env::var_os("FCM_ACCESS_TOKEN_FILE") {
                    Some(path) => AccessTokenSource::File(PathBuf::from(path)),
                    None => AccessTokenSource::Env,
                };
                Some(FcmConfig { project_id, access_token_source })
            }
            _ => None,
        };

        let device_tokens_file = std::env::var_os("DEVICE_TOKENS_FILE").map(PathBuf::from);

        Ok(Self { api_key, data_dir, fcm, device_tokens_file })
    }
}
