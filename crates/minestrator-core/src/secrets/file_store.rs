//! Backend de secrets **fichier** (Android) : `keyring` n'a pas de backend Android.
//!
//! Stocke un JSON `{ compte: valeur }` dans le dossier privé de l'app — sandbox par-app,
//! chiffré au repos par le File-Based Encryption d'Android (lié au verrouillage de l'appareil).
//! Le dossier vient de `MINESTRATOR_DATA_DIR` (posé par `mobile/src-tauri`) ; repli temporaire.
//!
//! Non chargé sur desktop (backend `keyring`), mais compilé partout pour vérification.
//! Durcissement Keystore matériel : amélioration future (cf. `docs/PUSH.md` / mobile/README.md).
#![allow(dead_code)]

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

// Sérialise les accès concurrents au fichier (les écritures de secrets sont rares).
static LOCK: Mutex<()> = Mutex::new(());

fn base_dir() -> PathBuf {
    std::env::var_os("MINESTRATOR_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("minestrator"))
}

fn file_path() -> PathBuf {
    base_dir().join("secrets.json")
}

fn load() -> Result<BTreeMap<String, String>> {
    match std::fs::read(file_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| Error::Keyring(format!("secrets.json illisible : {e}"))),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(Error::Keyring(format!("accès à secrets.json : {e}"))),
    }
}

fn save(map: &BTreeMap<String, String>) -> Result<()> {
    let dir = base_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Keyring(format!("création du dossier de secrets : {e}")))?;
    let bytes = serde_json::to_vec(map).map_err(|e| Error::Keyring(e.to_string()))?;
    // Écriture atomique : fichier temporaire puis rename.
    let tmp = dir.join("secrets.json.tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| Error::Keyring(format!("écriture des secrets : {e}")))?;
    std::fs::rename(&tmp, file_path())
        .map_err(|e| Error::Keyring(format!("remplacement de secrets.json : {e}")))?;
    Ok(())
}

pub fn read(account: &str) -> Result<Option<String>> {
    let _g = LOCK.lock().unwrap();
    Ok(load()?.get(account).cloned())
}

pub fn write(account: &str, value: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut map = load()?;
    map.insert(account.to_string(), value.to_string());
    save(&map)
}

pub fn delete(account: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut map = load()?;
    map.remove(account);
    save(&map)
}
