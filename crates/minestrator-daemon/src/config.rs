//! Configuration du daemon lue depuis l'environnement (FCM, clé API facultative).
//!
//! Le dossier de données se résout dans [`crate::paths`] ; les propriétaires autorisés sur le
//! canal IPC viennent de la ligne de commande (`--owner`), écrite par l'installation élevée.

use std::path::PathBuf;

pub struct EnvConfig {
    /// Clé API MineStrator fournie par l'environnement (`MINESTRATOR_API_KEY`). Facultative :
    /// en service, la clé arrive par IPC lors de l'activation depuis l'app.
    pub api_key: Option<String>,
    /// Config FCM ; `None` = pas de push (les alertes sont consignées et journalisées).
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

impl EnvConfig {
    pub fn from_env() -> Self {
        let api_key = std::env::var("MINESTRATOR_API_KEY")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

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

        Self { api_key, fcm, device_tokens_file }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// L'environnement est global au processus : tout test qui le modifie prend ce verrou
    /// (partagé avec les tests de `fcm`, via [`env_lock`]).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    const VARS: [&str; 4] = [
        "MINESTRATOR_API_KEY",
        "FCM_PROJECT_ID",
        "FCM_ACCESS_TOKEN_FILE",
        "DEVICE_TOKENS_FILE",
    ];

    /// Exécute `from_env` avec exactement ces variables définies (les autres effacées), puis
    /// restaure l'environnement d'origine.
    fn from_env_with(vars: &[(&str, &str)]) -> EnvConfig {
        let _guard = env_lock();
        let saved: Vec<_> = VARS.iter().map(|k| (*k, std::env::var_os(k))).collect();
        for k in VARS {
            std::env::remove_var(k);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        let out = EnvConfig::from_env();
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        out
    }

    #[test]
    fn api_key_is_optional_and_trimmed() {
        // En service, la clé arrive par IPC : son absence n'est pas une erreur.
        assert!(from_env_with(&[]).api_key.is_none());
        assert!(from_env_with(&[("MINESTRATOR_API_KEY", "  	 ")]).api_key.is_none());
        let cfg = from_env_with(&[("MINESTRATOR_API_KEY", "  clé 
")]);
        assert_eq!(cfg.api_key.as_deref(), Some("clé"));
    }

    #[test]
    fn minimal_config_only_logs_alerts() {
        let cfg = from_env_with(&[]);
        assert!(cfg.fcm.is_none(), "sans FCM_PROJECT_ID : pas de push");
        assert!(cfg.device_tokens_file.is_none());
    }

    #[test]
    fn device_tokens_file_comes_from_the_environment() {
        let cfg = from_env_with(&[("DEVICE_TOKENS_FILE", "/etc/minestrator/devices.json")]);
        assert_eq!(
            cfg.device_tokens_file,
            Some(PathBuf::from("/etc/minestrator/devices.json"))
        );
    }

    #[test]
    fn fcm_token_source_is_env_or_file() {
        let cfg = from_env_with(&[("FCM_PROJECT_ID", "mon-projet")]);
        let fcm = cfg.fcm.expect("FCM configuré");
        assert_eq!(fcm.project_id, "mon-projet");
        assert!(matches!(fcm.access_token_source, AccessTokenSource::Env));

        let cfg = from_env_with(&[
            ("FCM_PROJECT_ID", "mon-projet"),
            ("FCM_ACCESS_TOKEN_FILE", "/run/fcm-token"),
        ]);
        match cfg.fcm.expect("FCM configuré").access_token_source {
            AccessTokenSource::File(p) => assert_eq!(p, PathBuf::from("/run/fcm-token")),
            AccessTokenSource::Env => panic!("source fichier attendue"),
        }

        // Projet vide → FCM désactivé (même si un fichier de token est fourni).
        let cfg = from_env_with(&[("FCM_PROJECT_ID", "  "), ("FCM_ACCESS_TOKEN_FILE", "/run/fcm-token")]);
        assert!(cfg.fcm.is_none());
    }
}
