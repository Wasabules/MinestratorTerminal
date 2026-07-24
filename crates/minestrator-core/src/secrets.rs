//! Stockage des secrets (clé API MineStrator, clés LLM du Copilote, secrets par jeu).
//!
//! - **Desktop** (Windows/macOS/Linux) : trousseau natif de l'OS via `keyring`.
//! - **Mobile** (Android/iOS) : `keyring` n'a pas de backend mobile → stockage **fichier** dans le
//!   dossier privé de l'app (sandbox par-app, chiffrée au repos : File-Based Encryption sur Android,
//!   Data Protection sur iOS).
//! - **Opt-in fichier (headless)** : sur les autres OS, définir `MINESTRATOR_SECRETS_FILE` force
//!   le backend fichier — utile pour le **daemon Linux** sans Secret Service/D-Bus.
//!
//! Dans les deux modes fichier, le dossier vient de `MINESTRATOR_DATA_DIR`.
//! Les identifiants de compte (`KEYRING_ACCOUNT*`) sont partagés par les backends.

use crate::config::{KEYRING_ACCOUNT, KEYRING_ACCOUNT_GAME_PREFIX, KEYRING_ACCOUNT_LLM_PREFIX};
use crate::error::Result;

// Toujours compilé (donc vérifiable sur host) ; utilisé sur Android et en mode fichier opt-in.
mod file_store;

/// Backend trousseau natif (desktop). Absent des cibles mobiles (pas de `keyring`).
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

// --- Aiguillage du backend (deux définitions cfg-gated, sans ambiguïté) -----

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn prefer_file_backend() -> bool {
    std::env::var_os("MINESTRATOR_SECRETS_FILE").is_some()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
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

#[cfg(any(target_os = "android", target_os = "ios"))]
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

#[cfg(any(target_os = "android", target_os = "ios"))]
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
