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

    /// Le serveur SFTP a refusé le mot de passe (distinct d'une clé API invalide : l'API a
    /// répondu, c'est l'authentification SSH qui échoue).
    #[error("Le serveur SFTP a refusé le mot de passe. Vérifie-le dans l'onglet « Accès » du panel MineStrator.")]
    SftpAuth,

    /// Aucun mot de passe SFTP connu pour ce serveur : l'API ne le transmet plus et l'utilisateur
    /// ne l'a pas encore enregistré (onglet Fichiers du serveur dans l'app).
    #[error("Mot de passe SFTP requis : MineStrator ne le transmet plus. Saisis-le une fois dans l'onglet Fichiers du serveur (Minestrator Terminal) ; il est copié depuis l'onglet « Accès » du panel.")]
    SftpPasswordRequired,

    /// L'empreinte de la clé d'hôte du serveur SFTP n'est plus celle mémorisée : connexion bloquée
    /// avant l'envoi du mot de passe, jusqu'à ce que l'utilisateur accepte la nouvelle.
    #[error("L'identité du serveur SFTP {host} a changé depuis la dernière connexion (réinstallation du serveur, ou tentative d'interception). Connexion bloquée : vérifie et confirme la nouvelle empreinte dans l'onglet Fichiers du serveur (Minestrator Terminal).")]
    SftpHostKeyChanged { host: String },

    /// Service d'arrière-plan (IPC, installation, élévation). `code` est un identifiant stable
    /// (`unreachable`, `forbidden`, `elevation_cancelled`…) que l'UI traduit via `service.errors.*`.
    #[error("Service d'arrière-plan ({code}) : {detail}")]
    Service { code: String, detail: String },
}

impl Error {
    /// Raccourci de construction d'une [`Error::Service`].
    pub fn service(code: &str, detail: impl Into<String>) -> Self {
        Error::Service {
            code: code.to_string(),
            detail: detail.into(),
        }
    }
}

/// Discriminant d'une [`Error`], tel qu'il part dans le champ `kind` du JSON (type TypeScript
/// `AppErrorKind`). Un variant d'[`Error`] ajouté doit être rattaché ici via [`Error::error_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "AppErrorKind"))]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Unauthorized,
    Forbidden,
    RateLimited,
    NoKey,
    Network,
    Unexpected,
    Keyring,
    Api,
    SftpAuth,
    SftpPasswordRequired,
    SftpHostKeyChanged,
    /// Service d'arrière-plan : le sous-cas est dans `code` (cf. [`ErrorPayload::code`]).
    Service,
}

impl ErrorKind {
    /// Même valeur que la forme sérialisée (vérifié par un test).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::NoKey => "no_key",
            ErrorKind::Network => "network",
            ErrorKind::Unexpected => "unexpected",
            ErrorKind::Keyring => "keyring",
            ErrorKind::Api => "api",
            ErrorKind::SftpAuth => "sftp_auth",
            ErrorKind::SftpPasswordRequired => "sftp_password_required",
            ErrorKind::SftpHostKeyChanged => "sftp_host_key_changed",
            ErrorKind::Service => "service",
        }
    }
}

/// Forme JSON d'une [`Error`] reçue par les frontends (type TypeScript `AppError`). La
/// sérialisation d'[`Error`] passe par cette structure : le type généré ne peut pas diverger.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "AppError"))]
pub struct ErrorPayload {
    pub kind: ErrorKind,
    /// Message lisible (français), affiché quand le frontend n'a pas de traduction pour `kind`.
    pub message: String,
    /// Sous-cas stable d'une erreur `service` (`unreachable`, `forbidden`, `elevation_cancelled`…),
    /// traduit par l'UI via `service.errors.*`. Absent du JSON pour les autres erreurs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub code: Option<String>,
}

impl Error {
    /// Discriminant typé, consommé par les frontends.
    pub fn error_kind(&self) -> ErrorKind {
        match self {
            Error::Unauthorized => ErrorKind::Unauthorized,
            Error::Forbidden => ErrorKind::Forbidden,
            Error::RateLimited => ErrorKind::RateLimited,
            Error::NoKey => ErrorKind::NoKey,
            Error::Network(_) => ErrorKind::Network,
            Error::Unexpected(_) => ErrorKind::Unexpected,
            Error::Keyring(_) => ErrorKind::Keyring,
            Error::Api { .. } => ErrorKind::Api,
            Error::SftpAuth => ErrorKind::SftpAuth,
            Error::SftpPasswordRequired => ErrorKind::SftpPasswordRequired,
            Error::SftpHostKeyChanged { .. } => ErrorKind::SftpHostKeyChanged,
            Error::Service { .. } => ErrorKind::Service,
        }
    }

    /// Discriminant stable consommé par les frontends, sous forme de chaîne.
    pub fn kind(&self) -> &'static str {
        self.error_kind().as_str()
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        ErrorPayload {
            kind: self.error_kind(),
            message: self.to_string(),
            code: match self {
                Error::Service { code, .. } => Some(code.clone()),
                _ => None,
            },
        }
        .serialize(serializer)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

// `keyring` (crate) n'existe pas sur mobile : Android passe par `keyring-core` directement, iOS par
// le backend fichier → pas de conversion là-bas.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl From<keyring::Error> for Error {
    fn from(e: keyring::Error) -> Self {
        Error::Keyring(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Un exemplaire de chaque variant, avec son `kind` et son message attendus. Le `kind` est un
    /// contrat avec les frontends (`humanizeError` + clés i18n `errors.<kind>`) : ne pas le changer.
    fn cases() -> Vec<(Error, &'static str, &'static str)> {
        vec![
            (Error::Unauthorized, "unauthorized", "Clé API invalide ou révoquée."),
            (Error::Forbidden, "forbidden", "Accès refusé : permission insuffisante."),
            (Error::RateLimited, "rate_limited", "Trop de requêtes. Réessaie dans un instant."),
            (Error::NoKey, "no_key", "Aucune clé API enregistrée."),
            (Error::Network("délai dépassé".into()), "network", "Erreur réseau : délai dépassé"),
            (
                Error::Unexpected("HTTP 500".into()),
                "unexpected",
                "Réponse inattendue de l'API : HTTP 500",
            ),
            (
                Error::Keyring("accès refusé".into()),
                "keyring",
                "Erreur du trousseau système : accès refusé",
            ),
            (Error::Api { code: "SERVER_NOT_FOUND".into() }, "api", "Erreur API (SERVER_NOT_FOUND)."),
            (
                Error::SftpAuth,
                "sftp_auth",
                "Le serveur SFTP a refusé le mot de passe. Vérifie-le dans l'onglet « Accès » du panel MineStrator.",
            ),
            (
                Error::SftpHostKeyChanged { host: "node1.exemple:2022".into() },
                "sftp_host_key_changed",
                "L'identité du serveur SFTP node1.exemple:2022 a changé depuis la dernière connexion (réinstallation du serveur, ou tentative d'interception). Connexion bloquée : vérifie et confirme la nouvelle empreinte dans l'onglet Fichiers du serveur (Minestrator Terminal).",
            ),
            (
                Error::SftpPasswordRequired,
                "sftp_password_required",
                "Mot de passe SFTP requis : MineStrator ne le transmet plus. Saisis-le une fois dans l'onglet Fichiers du serveur (Minestrator Terminal) ; il est copié depuis l'onglet « Accès » du panel.",
            ),
        ]
    }

    #[test]
    fn kind_and_message_of_each_variant() {
        for (err, kind, message) in cases() {
            assert_eq!(err.kind(), kind, "{err:?}");
            assert_eq!(err.to_string(), message, "{err:?}");
        }
    }

    #[test]
    fn serializes_as_kind_and_message() {
        for (err, kind, message) in cases() {
            assert_eq!(
                serde_json::to_value(&err).unwrap(),
                json!({ "kind": kind, "message": message })
            );
        }
        // Forme exacte attendue par le front (ordre des champs compris).
        assert_eq!(
            serde_json::to_string(&Error::NoKey).unwrap(),
            r#"{"kind":"no_key","message":"Aucune clé API enregistrée."}"#
        );
    }

    #[test]
    fn kinds_are_unique() {
        let mut kinds: Vec<&str> = cases().iter().map(|(e, _, _)| e.kind()).collect();
        let n = kinds.len();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), n);
    }

    #[test]
    fn reqwest_errors_become_network_errors() {
        // Erreur de construction de requête (URL invalide) : aucune E/S réseau.
        let e = crate::http::client().get("pas une url").build().unwrap_err();
        let err = Error::from(e);
        assert_eq!(err.kind(), "network");
        assert!(err.to_string().starts_with("Erreur réseau : "), "{err}");
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn keyring_errors_become_keyring_errors() {
        let err = Error::from(keyring::Error::NoEntry);
        assert_eq!(err.kind(), "keyring");
        assert!(err.to_string().starts_with("Erreur du trousseau système : "), "{err}");
    }

    /// Le `kind` sérialisé (source du type TS `AppErrorKind`) et la chaîne de [`Error::kind`]
    /// doivent rester identiques, et la forme JSON rester `{ kind, message }`.
    #[test]
    fn kind_serialise_comme_sa_chaine() {
        let cas = [
            Error::Unauthorized,
            Error::Forbidden,
            Error::RateLimited,
            Error::NoKey,
            Error::Network("x".into()),
            Error::Unexpected("x".into()),
            Error::Keyring("x".into()),
            Error::Api { code: "x".into() },
        ];
        for e in cas {
            let json = serde_json::to_value(&e).expect("sérialisation");
            assert_eq!(json["kind"], e.kind(), "{e:?}");
            assert_eq!(json["message"], e.to_string(), "{e:?}");
            assert_eq!(json.as_object().map(|o| o.len()), Some(2), "{e:?}");
        }
    }
}
