//! Stockage des secrets : clé API MineStrator, clés LLM du Copilote, secrets par jeu, mots de passe
//! SFTP par serveur. **Aucun secret n'est écrit en clair par l'app** : chaque plateforme passe par
//! son stockage sécurisé natif.
//!
//! | Contexte | Backend |
//! |---|---|
//! | App desktop (Windows, macOS, Linux) | trousseau de l'OS via `keyring` : Gestionnaire d'identifiants (Windows), Trousseau (macOS), Secret Service (Linux) |
//! | App Android | Keystore Android via `android-native-keyring-store` : valeurs chiffrées (AES-GCM) par une clé non exportable du Keystore, rangées dans des `SharedPreferences` privées. L'ancien `secrets.json` en clair (≤ 0.6.0) y est migré puis supprimé au premier accès. |
//! | Service d'arrière-plan (`MINESTRATOR_SECRETS_FILE`) | fichier du dossier machine, accessible au seul compte du service ; **chiffré DPAPI** sous Windows (cf. [`file_store`]). Un service système n'a pas de session utilisateur, donc pas de trousseau. |
//! | iOS (non livré) | fichier du dossier privé de l'app (Data Protection) |
//!
//! Les identifiants de compte (`KEYRING_ACCOUNT*`) sont communs à tous les backends.

use crate::config::{
    KEYRING_ACCOUNT, KEYRING_ACCOUNT_GAME_PREFIX, KEYRING_ACCOUNT_LLM_PREFIX,
    KEYRING_ACCOUNT_SFTP_PREFIX, KEYRING_ACCOUNT_WEBHOOK_PREFIX,
};
use crate::error::Result;

// Toujours compilé (donc vérifiable sur host) : service d'arrière-plan, iOS, et source de la
// migration Android.
mod file_store;

/// Backend trousseau natif (app desktop).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod keyring_store {
    use crate::config::KEYRING_SERVICE;
    use crate::error::{Error, Result};
    use keyring::Entry;

    fn entry_for(account: &str) -> Result<Entry> {
        Entry::new(KEYRING_SERVICE, account).map_err(Error::from)
    }
    pub fn read(account: &str) -> Result<Option<String>> {
        match entry_for(account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::from(e)),
        }
    }
    pub fn write(account: &str, value: &str) -> Result<()> {
        entry_for(account)?.set_password(value).map_err(Error::from)
    }
    pub fn delete(account: &str) -> Result<()> {
        match entry_for(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::from(e)),
        }
    }
}

/// Backend Keystore Android.
///
/// Le contexte applicatif JNI vient de `ndk-context`, initialisé par Tauri (tao) au lancement :
/// rien à faire côté Kotlin. Le magasin est créé une seule fois, puis sert d'entrepôt par défaut à
/// `keyring-core`.
#[cfg(target_os = "android")]
mod android_store {
    use crate::config::KEYRING_SERVICE;
    use crate::error::{Error, Result};
    use keyring_core::Entry;
    use std::sync::LazyLock;

    static INIT: LazyLock<std::result::Result<(), String>> = LazyLock::new(|| {
        let store = android_native_keyring_store::Store::new().map_err(|e| e.to_string())?;
        keyring_core::set_default_store(store);
        // `raw_write` et non `write` : `write` attend `INIT`, qu'on est en train de calculer
        // (l'appel réentrant bloquerait le démarrage dès qu'un ancien fichier existe).
        super::migrate_legacy_file(raw_write);
        Ok(())
    });

    fn kerr(e: keyring_core::Error) -> Error {
        Error::Keyring(e.to_string())
    }

    /// Entrée du magasin par défaut, sans attendre `INIT` (à n'appeler qu'une fois le magasin posé).
    fn raw_entry(account: &str) -> Result<Entry> {
        Entry::new(KEYRING_SERVICE, account).map_err(kerr)
    }

    fn raw_write(account: &str, value: &str) -> Result<()> {
        raw_entry(account)?.set_password(value).map_err(kerr)
    }

    fn entry_for(account: &str) -> Result<Entry> {
        if let Err(e) = &*INIT {
            return Err(Error::Keyring(format!("Keystore Android indisponible : {e}")));
        }
        raw_entry(account)
    }
    pub fn read(account: &str) -> Result<Option<String>> {
        match entry_for(account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(kerr(e)),
        }
    }
    pub fn write(account: &str, value: &str) -> Result<()> {
        entry_for(account)?.set_password(value).map_err(kerr)
    }
    pub fn delete(account: &str) -> Result<()> {
        match entry_for(account)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(kerr(e)),
        }
    }
}

/// Reprend les secrets de l'ancien fichier en clair (`secrets.json`, app Android ≤ 0.6.0) dans le
/// stockage sécurisé, puis supprime le fichier. Il n'est supprimé que si **tout** a été recopié :
/// en cas d'échec partiel, la migration est retentée au prochain lancement, sans perte.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn migrate_legacy_file(write: fn(&str, &str) -> Result<()>) {
    let entries = match file_store::take_all() {
        Ok(Some(entries)) => entries,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("migration des secrets : ancien fichier illisible ({e})");
            return;
        }
    };
    let failed = entries.iter().filter(|(k, v)| write(k, v).is_err()).count();
    if failed == 0 {
        match file_store::remove_file() {
            Ok(()) => tracing::info!(n = entries.len(), "secrets migrés vers le stockage sécurisé"),
            Err(e) => tracing::warn!("migration des secrets : suppression de l'ancien fichier ({e})"),
        }
    } else {
        tracing::warn!(failed, "migration des secrets incomplète : nouvel essai au prochain lancement");
    }
}

// --- Aiguillage du backend ---------------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn prefer_file_backend() -> bool {
    std::env::var_os("MINESTRATOR_SECRETS_FILE").is_some()
}

#[cfg(target_os = "android")]
fn read_account(account: &str) -> Result<Option<String>> {
    android_store::read(account)
}
#[cfg(target_os = "ios")]
fn read_account(account: &str) -> Result<Option<String>> {
    file_store::read(account)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn read_account(account: &str) -> Result<Option<String>> {
    if prefer_file_backend() {
        file_store::read(account)
    } else {
        keyring_store::read(account)
    }
}

#[cfg(target_os = "android")]
fn write_account(account: &str, value: &str) -> Result<()> {
    android_store::write(account, value)
}
#[cfg(target_os = "ios")]
fn write_account(account: &str, value: &str) -> Result<()> {
    file_store::write(account, value)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_account(account: &str, value: &str) -> Result<()> {
    if prefer_file_backend() {
        file_store::write(account, value)
    } else {
        keyring_store::write(account, value)
    }
}

#[cfg(target_os = "android")]
fn delete_account(account: &str) -> Result<()> {
    android_store::delete(account)
}
#[cfg(target_os = "ios")]
fn delete_account(account: &str) -> Result<()> {
    file_store::delete(account)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn delete_account(account: &str) -> Result<()> {
    if prefer_file_backend() {
        file_store::delete(account)
    } else {
        keyring_store::delete(account)
    }
}

// --- Token API MineStrator -------------------------------------------------

/// Enregistre (ou remplace) le token API MineStrator prêt à l'emploi.
pub fn store_key(key: &str) -> Result<()> {
    write_account(KEYRING_ACCOUNT, key)
}

/// Lit le token API MineStrator. `Ok(None)` si aucun n'est enregistré.
pub fn read_key() -> Result<Option<String>> {
    read_account(KEYRING_ACCOUNT)
}

/// Supprime le token MineStrator. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_key() -> Result<()> {
    delete_account(KEYRING_ACCOUNT)
}

// --- Clés LLM du Copilote (une par fournisseur) ----------------------------

fn llm_account(provider: &str) -> String {
    format!("{KEYRING_ACCOUNT_LLM_PREFIX}{provider}")
}

/// Enregistre (ou remplace) la clé API LLM d'un fournisseur (`anthropic`, `openai`, …).
pub fn store_llm_key(provider: &str, key: &str) -> Result<()> {
    write_account(&llm_account(provider), key)
}

/// Lit la clé API LLM d'un fournisseur. `Ok(None)` si aucune n'est enregistrée.
pub fn read_llm_key(provider: &str) -> Result<Option<String>> {
    read_account(&llm_account(provider))
}

/// Supprime la clé API LLM d'un fournisseur. Ne renvoie pas d'erreur si absente.
pub fn delete_llm_key(provider: &str) -> Result<()> {
    delete_account(&llm_account(provider))
}

// --- Secrets par jeu (ex. token factorio.com) ------------------------------

fn game_account(game: &str) -> String {
    format!("{KEYRING_ACCOUNT_GAME_PREFIX}{game}")
}

/// Enregistre (ou remplace) un secret propre à un jeu (ex. `factorio` → token de download).
pub fn store_game_secret(game: &str, value: &str) -> Result<()> {
    write_account(&game_account(game), value)
}

/// Lit le secret d'un jeu. `Ok(None)` si aucun n'est enregistré.
pub fn read_game_secret(game: &str) -> Result<Option<String>> {
    read_account(&game_account(game))
}

/// Supprime le secret d'un jeu. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_game_secret(game: &str) -> Result<()> {
    delete_account(&game_account(game))
}

// --- Mots de passe SFTP (un par serveur) -----------------------------------

fn sftp_account(server_id: i64) -> String {
    format!("{KEYRING_ACCOUNT_SFTP_PREFIX}{server_id}")
}

/// Enregistre (ou remplace) le mot de passe SFTP d'un serveur.
pub fn store_sftp_password(server_id: i64, password: &str) -> Result<()> {
    write_account(&sftp_account(server_id), password)
}

/// Lit le mot de passe SFTP d'un serveur. `Ok(None)` si aucun n'est enregistré.
pub fn read_sftp_password(server_id: i64) -> Result<Option<String>> {
    read_account(&sftp_account(server_id))
}

/// Supprime le mot de passe SFTP d'un serveur. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_sftp_password(server_id: i64) -> Result<()> {
    delete_account(&sftp_account(server_id))
}

// --- URL des destinations d'alertes (webhooks) ------------------------------

fn webhook_account(id: &str) -> String {
    format!("{KEYRING_ACCOUNT_WEBHOOK_PREFIX}{id}")
}

/// Enregistre (ou remplace) l'URL d'une destination d'alertes (webhook Discord ou JSON).
pub fn store_webhook_url(id: &str, url: &str) -> Result<()> {
    write_account(&webhook_account(id), url)
}

/// Lit l'URL d'une destination d'alertes. `Ok(None)` si aucune n'est enregistrée.
pub fn read_webhook_url(id: &str) -> Result<Option<String>> {
    read_account(&webhook_account(id))
}

/// Supprime l'URL d'une destination d'alertes. Ne renvoie pas d'erreur si absente.
pub fn delete_webhook_url(id: &str) -> Result<()> {
    delete_account(&webhook_account(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_stable() {
        // Ces noms identifient les entrées déjà enregistrées chez les utilisateurs : les changer
        // rendrait leurs secrets introuvables.
        assert_eq!(llm_account("openai"), "llm-key-openai");
        assert_eq!(game_account("factorio"), "game-secret-factorio");
        assert_eq!(sftp_account(377829), "sftp-password-377829");
        assert_eq!(webhook_account("wh-1a2b"), "webhook-url-wh-1a2b");
    }
}
