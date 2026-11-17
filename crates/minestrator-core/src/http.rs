//! Construction des clients HTTP (reqwest 0.13), en un seul endroit.
//!
//! reqwest est compilé en `rustls-no-provider`, comme Tauri : aucun backend crypto en C
//! (`aws-lc-rs` exige CMake/NASM et complique la compilation croisée Android). Il reçoit une
//! configuration rustls **préconstruite** : fournisseur `ring` + racines Mozilla embarquées
//! (`webpki-roots`). C'est le comportement de reqwest 0.12 `rustls-tls`, identique sur toutes les
//! plateformes — en particulier Android, où le vérificateur « magasin du système » de reqwest 0.13
//! exige une initialisation JNI que l'app ne fait pas.
//!
//! Toute construction de client passe par [`builder`] ou [`client`].

use std::sync::{Arc, LazyLock, Once};

static TLS: LazyLock<rustls::ClientConfig> = LazyLock::new(|| {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions TLS par défaut prises en charge par ring")
        .with_root_certificates(roots)
        .with_no_client_auth()
});

/// Installe `ring` comme fournisseur crypto par défaut du processus, pour les bibliothèques qui
/// construisent leur propre configuration rustls (WebSocket de la console). Sans effet s'il y en a
/// déjà un ; appelé par `Core::new`.
pub fn ensure_default_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// `ClientBuilder` au TLS configuré, à compléter (timeouts, user-agent…).
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_backend_preconfigured(TLS.clone())
}

/// Client par défaut.
pub fn client() -> reqwest::Client {
    builder()
        .build()
        .expect("client HTTP (configuration TLS préconstruite)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_build_with_the_preconfigured_tls() {
        // Un backend TLS non reconnu ferait échouer `build` : vérifie que la config est acceptée.
        assert!(builder().build().is_ok());
        let _ = client();
        ensure_default_provider();
        ensure_default_provider(); // idempotent
    }
}
