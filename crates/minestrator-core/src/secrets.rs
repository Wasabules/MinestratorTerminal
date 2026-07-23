//! Stockage des secrets (clé API MineStrator, clés LLM du Copilote, secrets par jeu).
//!
//! - **Desktop** (Windows/macOS/Linux) : trousseau natif de l'OS via `keyring`.
//! - **Android** : `keyring` n'a pas de backend Android → stockage **fichier** dans le dossier
//!   privé de l'app (sandbox par-app, chiffré au repos par le File-Based Encryption d'Android).
//!   Le dossier vient de `MINESTRATOR_DATA_DIR`, posé par `mobile/src-tauri` avant `Core::new()`.
//!
//! Les identifiants de compte (`KEYRING_ACCOUNT*`) sont partagés par les deux backends.

use crate::config::{KEYRING_ACCOUNT, KEYRING_ACCOUNT_GAME_PREFIX, KEYRING_ACCOUNT_LLM_PREFIX};
use crate::error::Result;

// Toujours compilé (donc vérifiable sur host) ; effectivement utilisé sur Android.
mod file_store;

/// Backend trousseau natif (desktop). Absent de la cible Android (pas de `keyring`).
#[cfg(not(target_os = "android"))]
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

// Aiguillage du backend selon la plateforme.
#[cfg(not(target_os = "android"))]
use keyring_store as backend;
#[cfg(target_os = "android")]
use file_store as backend;

// --- Token API MineStrator -------------------------------------------------

/// Enregistre (ou remplace) le token API MineStrator prêt à l'emploi.
pub fn store_key(key: &str) -> Result<()> {
    backend::write(KEYRING_ACCOUNT, key)
}

/// Lit le token API MineStrator. `Ok(None)` si aucun n'est enregistré.
pub fn read_key() -> Result<Option<String>> {
    backend::read(KEYRING_ACCOUNT)
}

/// Supprime le token MineStrator. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_key() -> Result<()> {
    backend::delete(KEYRING_ACCOUNT)
}

// --- Clés LLM du Copilote (une par fournisseur) ----------------------------

fn llm_account(provider: &str) -> String {
    format!("{KEYRING_ACCOUNT_LLM_PREFIX}{provider}")
}

/// Enregistre (ou remplace) la clé API LLM d'un fournisseur (`anthropic`, `openai`, …).
pub fn store_llm_key(provider: &str, key: &str) -> Result<()> {
    backend::write(&llm_account(provider), key)
}

/// Lit la clé API LLM d'un fournisseur. `Ok(None)` si aucune n'est enregistrée.
pub fn read_llm_key(provider: &str) -> Result<Option<String>> {
    backend::read(&llm_account(provider))
}

/// Supprime la clé API LLM d'un fournisseur. Ne renvoie pas d'erreur si absente.
pub fn delete_llm_key(provider: &str) -> Result<()> {
    backend::delete(&llm_account(provider))
}

// --- Secrets par jeu (ex. token factorio.com) ------------------------------

fn game_account(game: &str) -> String {
    format!("{KEYRING_ACCOUNT_GAME_PREFIX}{game}")
}

/// Enregistre (ou remplace) un secret propre à un jeu (ex. `factorio` → token de download).
pub fn store_game_secret(game: &str, value: &str) -> Result<()> {
    backend::write(&game_account(game), value)
}

/// Lit le secret d'un jeu. `Ok(None)` si aucun n'est enregistré.
pub fn read_game_secret(game: &str) -> Result<Option<String>> {
    backend::read(&game_account(game))
}

/// Supprime le secret d'un jeu. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_game_secret(game: &str) -> Result<()> {
    backend::delete(&game_account(game))
}
