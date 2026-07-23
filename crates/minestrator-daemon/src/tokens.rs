//! Registre des tokens d'appareil FCM.
//!
//! Simple fichier JSON (tableau de chaînes) que l'app mobile alimente (directement ou via un
//! relais). Relu à chaque alerte → un appareil fraîchement enregistré est pris en compte sans
//! redémarrer le daemon.

use std::path::Path;

pub fn load(path: Option<&Path>) -> Vec<String> {
    let Some(path) = path else {
        return Vec::new();
    };
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_else(|e| {
            tracing::warn!("tokens d'appareil illisibles ({}) : {e}", path.display());
            Vec::new()
        }),
        Err(e) => {
            tracing::debug!("pas de fichier de tokens ({}) : {e}", path.display());
            Vec::new()
        }
    }
}
