//! Type d'erreur unifié du cœur métier.
//!
//! Sérialisable en `{ kind, message }` : n'importe quel frontend (Tauri, daemon, CLI)
//! peut réagir par cas sans parser une chaîne.

use serde::{Serialize, Serializer};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Clé API invalide ou révoquée.")]
    Unauthorized,

    #[error("Accès refusé : permission insuffisante.")]
    Forbidden,

    #[error("Trop de requêtes. Réessaie dans un instant.")]
    RateLimited,

    #[error("Aucune clé API enregistrée.")]
    NoKey,

    #[error("Erreur réseau : {0}")]
    Network(String),

    #[error("Réponse inattendue de l'API : {0}")]
    Unexpected(String),

    #[error("Erreur du trousseau système : {0}")]
    Keyring(String),

    #[error("Erreur API ({code}).")]
    Api { code: String },
}

impl Error {
    /// Discriminant stable consommé par les frontends.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Unauthorized => "unauthorized",
            Error::Forbidden => "forbidden",
            Error::RateLimited => "rate_limited",
            Error::NoKey => "no_key",
            Error::Network(_) => "network",
            Error::Unexpected(_) => "unexpected",
            Error::Keyring(_) => "keyring",
            Error::Api { .. } => "api",
        }
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("Error", 2)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<keyring::Error> for Error {
    fn from(e: keyring::Error) -> Self {
        Error::Keyring(e.to_string())
    }
}
