//! Client minimal de l'API **FCM v1** (HTTP/JSON, rustls).

use crate::config::{AccessTokenSource, FcmConfig};
use serde_json::{json, Value};

/// Récupère l'access token OAuth2 (scope `firebase.messaging`) pour l'API FCM v1.
///
/// **Scaffold** : lit un token **déjà obtenu** (`FCM_ACCESS_TOKEN` ou fichier). En production,
/// le générer depuis un **compte de service** (JWT signé → token, ex. crate `gcp_auth`) et le
/// rafraîchir toutes les ~55 min. Voir `docs/PUSH.md`.
pub fn access_token(cfg: &FcmConfig) -> Result<String, String> {
    match &cfg.access_token_source {
        AccessTokenSource::Env => {
            std::env::var("FCM_ACCESS_TOKEN").map_err(|_| "FCM_ACCESS_TOKEN manquant.".to_string())
        }
        AccessTokenSource::File(path) => std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("lecture de FCM_ACCESS_TOKEN_FILE : {e}")),
    }
}

/// Envoie une notification à un appareil. `data` doit être un objet de paires chaîne→chaîne.
/// Renvoie `Ok(())` sur 2xx, sinon le statut + le corps d'erreur.
pub async fn send(
    client: &reqwest::Client,
    project_id: &str,
    access_token: &str,
    device_token: &str,
    title: &str,
    body: &str,
    data: Value,
) -> Result<(), String> {
    let url = format!("https://fcm.googleapis.com/v1/projects/{project_id}/messages:send");
    let payload = json!({
        "message": {
            "token": device_token,
            "notification": { "title": title, "body": body },
            "data": data,
            "android": { "priority": "high" }
        }
    });
    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("requête FCM : {e}"))?;

    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(format!("FCM {status} : {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::env_lock;
    use std::path::PathBuf;

    fn cfg(source: AccessTokenSource) -> FcmConfig {
        FcmConfig {
            project_id: "mon-projet".into(),
            access_token_source: source,
        }
    }

    #[test]
    fn token_file_is_read_and_trimmed() {
        let path = std::env::temp_dir()
            .join(format!("mst-test-fcm-token-{}.txt", std::process::id()));
        // Un helper de rafraîchissement écrit souvent le token suivi d'un saut de ligne.
        std::fs::write(&path, "  ya29.a0AfB_token \n").unwrap();
        let token = access_token(&cfg(AccessTokenSource::File(path.clone())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(token.unwrap(), "ya29.a0AfB_token");
    }

    #[test]
    fn missing_token_file_is_an_error() {
        let path = PathBuf::from("/chemin/inexistant/fcm-token");
        let err = access_token(&cfg(AccessTokenSource::File(path))).unwrap_err();
        assert!(err.starts_with("lecture de FCM_ACCESS_TOKEN_FILE"), "{err}");
    }

    #[test]
    fn env_source_reads_fcm_access_token() {
        let _guard = env_lock();
        let saved = std::env::var_os("FCM_ACCESS_TOKEN");

        std::env::set_var("FCM_ACCESS_TOKEN", "ya29.env");
        let ok = access_token(&cfg(AccessTokenSource::Env));
        std::env::remove_var("FCM_ACCESS_TOKEN");
        let missing = access_token(&cfg(AccessTokenSource::Env));
        if let Some(v) = saved {
            std::env::set_var("FCM_ACCESS_TOKEN", v);
        }

        assert_eq!(ok.unwrap(), "ya29.env");
        assert_eq!(missing.unwrap_err(), "FCM_ACCESS_TOKEN manquant.");
    }
}
