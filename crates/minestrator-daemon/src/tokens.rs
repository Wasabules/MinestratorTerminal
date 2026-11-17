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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fichier temporaire unique (supprimé à la fin du test).
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(content: &str) -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("mst-test-tokens-{}-{n}.json", std::process::id()));
            std::fs::write(&path, content).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn no_configured_file_means_no_device() {
        assert!(load(None).is_empty());
    }

    #[test]
    fn reads_a_json_array_of_tokens() {
        let f = TempFile::new(r#"["fcm-token-a", "fcm-token-b"]"#);
        assert_eq!(load(Some(&f.0)), ["fcm-token-a", "fcm-token-b"]);
    }

    #[test]
    fn missing_or_malformed_file_yields_no_device() {
        let missing = std::env::temp_dir().join("mst-test-tokens-absent-0000.json");
        assert!(load(Some(&missing)).is_empty());
        for content in ["", "pas du json", r#"{"tokens":["a"]}"#, "[1, 2]", r#""a""#] {
            let f = TempFile::new(content);
            assert!(load(Some(&f.0)).is_empty(), "{content}");
        }
    }

    #[test]
    fn file_is_reread_on_each_call() {
        let f = TempFile::new(r#"["a"]"#);
        assert_eq!(load(Some(&f.0)), ["a"]);
        // Un appareil enregistré entre-temps est pris en compte sans redémarrage.
        std::fs::write(&f.0, r#"["a", "b"]"#).unwrap();
        assert_eq!(load(Some(&f.0)), ["a", "b"]);
    }
}
