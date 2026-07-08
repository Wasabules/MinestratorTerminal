//! Stockage de la clé API dans le trousseau natif de l'OS
//! (Credential Manager / Keychain / Secret Service). Fonctionne aussi headless.

use crate::config::{
    KEYRING_ACCOUNT, KEYRING_ACCOUNT_GAME_PREFIX, KEYRING_ACCOUNT_LLM_PREFIX, KEYRING_SERVICE,
};
use crate::error::{Error, Result};
use keyring::Entry;

fn entry_for(account: &str) -> Result<Entry> {
    Entry::new(KEYRING_SERVICE, account).map_err(Error::from)
}

fn read(account: &str) -> Result<Option<String>> {
    match entry_for(account)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::from(e)),
    }
}

fn delete(account: &str) -> Result<()> {
    match entry_for(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::from(e)),
    }
}

/// Enregistre (ou remplace) le token API MineStrator prêt à l'emploi.
pub fn store_key(key: &str) -> Result<()> {
    entry_for(KEYRING_ACCOUNT)?.set_password(key).map_err(Error::from)
}

/// Lit le token API MineStrator. `Ok(None)` si aucun n'est enregistré.
pub fn read_key() -> Result<Option<String>> {
    read(KEYRING_ACCOUNT)
}

/// Supprime le token MineStrator. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_key() -> Result<()> {
    delete(KEYRING_ACCOUNT)
}

// --- Clés LLM du Copilote (une par fournisseur) ----------------------------

fn llm_account(provider: &str) -> String {
    format!("{KEYRING_ACCOUNT_LLM_PREFIX}{provider}")
}

/// Enregistre (ou remplace) la clé API LLM d'un fournisseur (`anthropic`, `openai`, …).
pub fn store_llm_key(provider: &str, key: &str) -> Result<()> {
    entry_for(&llm_account(provider))?
        .set_password(key)
        .map_err(Error::from)
}

/// Lit la clé API LLM d'un fournisseur. `Ok(None)` si aucune n'est enregistrée.
pub fn read_llm_key(provider: &str) -> Result<Option<String>> {
    read(&llm_account(provider))
}

/// Supprime la clé API LLM d'un fournisseur. Ne renvoie pas d'erreur si absente.
pub fn delete_llm_key(provider: &str) -> Result<()> {
    delete(&llm_account(provider))
}

// --- Secrets par jeu (ex. token factorio.com) ------------------------------

fn game_account(game: &str) -> String {
    format!("{KEYRING_ACCOUNT_GAME_PREFIX}{game}")
}

/// Enregistre (ou remplace) un secret propre à un jeu (ex. `factorio` → token de download).
pub fn store_game_secret(game: &str, value: &str) -> Result<()> {
    entry_for(&game_account(game))?
        .set_password(value)
        .map_err(Error::from)
}

/// Lit le secret d'un jeu. `Ok(None)` si aucun n'est enregistré.
pub fn read_game_secret(game: &str) -> Result<Option<String>> {
    read(&game_account(game))
}

/// Supprime le secret d'un jeu. Ne renvoie pas d'erreur s'il n'existait pas.
pub fn delete_game_secret(game: &str) -> Result<()> {
    delete(&game_account(game))
}
