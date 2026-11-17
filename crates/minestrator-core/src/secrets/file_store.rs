//! Backend de secrets **fichier** (mobile : Android/iOS) : `keyring` n'a pas de backend mobile.
//!
//! Stocke un JSON `{ compte: valeur }` dans le dossier privé de l'app — sandbox par-app,
//! chiffrée au repos (File-Based Encryption sur Android, Data Protection sur iOS ; liée au
//! verrouillage de l'appareil). Sert aussi au daemon headless (opt-in `MINESTRATOR_SECRETS_FILE`).
//!
//! Dossier : `MINESTRATOR_DATA_DIR` (posé par `mobile/src-tauri` et le daemon), sinon le dossier de
//! données standard de l'OS. **Jamais le répertoire temporaire** : il est partagé entre
//! utilisateurs sur Linux/macOS (`/tmp`). Sous Unix, le fichier est en `0600`.
//!
//! Non chargé sur desktop (backend `keyring`), mais compilé partout pour vérification.
//! Durcissement Keystore matériel : amélioration future (cf. `docs/PUSH.md` / mobile/README.md).
#![allow(dead_code)]

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// Sérialise les accès concurrents au fichier (les écritures de secrets sont rares).
static LOCK: Mutex<()> = Mutex::new(());

const FILE_NAME: &str = "secrets.json";
const TMP_NAME: &str = "secrets.json.tmp";

fn base_dir() -> Result<PathBuf> {
    crate::util::data_dir().ok_or_else(|| {
        Error::Keyring(
            "aucun dossier de données : définir MINESTRATOR_DATA_DIR pour stocker les secrets"
                .into(),
        )
    })
}

fn load_in(dir: &Path) -> Result<BTreeMap<String, String>> {
    let path = dir.join(FILE_NAME);
    match std::fs::read(&path) {
        Ok(bytes) => {
            restrict_permissions(&path);
            serde_json::from_slice(&bytes)
                .map_err(|e| Error::Keyring(format!("{FILE_NAME} illisible : {e}")))
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(Error::Keyring(format!("accès à {FILE_NAME} : {e}"))),
    }
}

/// Écriture atomique (fichier temporaire puis rename), en `0600` sous Unix. Le temporaire est
/// créé en exclusif (`create_new`) : un lien symbolique planté à sa place n'est jamais suivi.
fn save_in(dir: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Keyring(format!("création du dossier de secrets : {e}")))?;
    let bytes = serde_json::to_vec(map).map_err(|e| Error::Keyring(e.to_string()))?;
    let tmp = dir.join(TMP_NAME);
    // Reste d'une écriture interrompue (ou lien planté) : on retire l'entrée elle-même.
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(&tmp)
        .and_then(|mut f| {
            f.write_all(&bytes)?;
            f.sync_all()
        })
        .map_err(|e| Error::Keyring(format!("écriture des secrets : {e}")))?;
    std::fs::rename(&tmp, dir.join(FILE_NAME))
        .map_err(|e| Error::Keyring(format!("remplacement de {FILE_NAME} : {e}")))?;
    Ok(())
}

/// Ramène à `0600` un fichier de secrets créé trop ouvert (versions antérieures). Best-effort.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub fn read(account: &str) -> Result<Option<String>> {
    let _g = LOCK.lock().unwrap();
    Ok(load_in(&base_dir()?)?.get(account).cloned())
}

pub fn write(account: &str, value: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let dir = base_dir()?;
    let mut map = load_in(&dir)?;
    map.insert(account.to_string(), value.to_string());
    save_in(&dir, &map)
}

pub fn delete(account: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let dir = base_dir()?;
    let mut map = load_in(&dir)?;
    map.remove(account);
    save_in(&dir, &map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minestrator-test-secrets-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn roundtrip_and_no_leftover_tmp() {
        let dir = test_dir("roundtrip");
        let mut map = BTreeMap::new();
        map.insert("api".to_string(), "clé".to_string());
        save_in(&dir, &map).unwrap();
        assert_eq!(load_in(&dir).unwrap().get("api").map(String::as_str), Some("clé"));
        assert!(!dir.join(TMP_NAME).exists());
        // Un temporaire orphelin d'une écriture interrompue ne bloque pas la suivante.
        std::fs::write(dir.join(TMP_NAME), b"partiel").unwrap();
        save_in(&dir, &map).unwrap();
        assert_eq!(load_in(&dir).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = test_dir("missing");
        assert!(load_in(&dir).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_file_is_private_and_repaired() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("perms");
        save_in(&dir, &BTreeMap::new()).unwrap();
        let path = dir.join(FILE_NAME);
        let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o600);
        // Fichier hérité d'une version antérieure, lisible par tous : corrigé à la lecture.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        load_in(&dir).unwrap();
        assert_eq!(mode(), 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
