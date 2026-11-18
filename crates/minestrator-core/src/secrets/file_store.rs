//! Backend de secrets **fichier** : service d'arrière-plan (opt-in `MINESTRATOR_SECRETS_FILE`),
//! iOS, et source de la migration Android (l'app Android ≤ 0.6.0 y rangeait ses secrets).
//!
//! Un service système tourne sans session utilisateur, donc sans trousseau : ses secrets vivent
//! dans un JSON `{ compte: valeur }` du dossier de données du service, accessible au seul compte
//! du service (DACL sous Windows, `0700`/`0600` sous Unix, cf. le daemon).
//!
//! - **Windows** : le contenu est en plus **chiffré par DPAPI** avec la clé du compte qui exécute
//!   le processus (`LocalService` pour le service) : le fichier copié ailleurs, ou lu par un autre
//!   compte, est inexploitable. Un ancien fichier en clair est relu puis réécrit chiffré.
//! - **Unix** : fichier `0600` dans un dossier `0700`. Il n'existe pas de stockage plus sûr pour un
//!   service système : le trousseau système macOS et les secrets systemd reposent eux aussi sur
//!   l'accès réservé à root.
//!
//! Dossier : `MINESTRATOR_DATA_DIR`, sinon le dossier de données standard de l'OS. **Jamais le
//! répertoire temporaire** : il est partagé entre utilisateurs sur Linux/macOS (`/tmp`).
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

/// Contenu du fichier → table des secrets. Accepte le JSON en clair (Unix, ou fichier Windows
/// antérieur au chiffrement) et, sous Windows, le blob DPAPI.
fn decode(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let plain = bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    let json = if plain { bytes.to_vec() } else { seal::open(bytes)? };
    serde_json::from_slice(&json).map_err(|e| Error::Keyring(format!("{FILE_NAME} illisible : {e}")))
}

fn encode(map: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(map).map_err(|e| Error::Keyring(e.to_string()))?;
    seal::seal(&json)
}

fn load_in(dir: &Path) -> Result<BTreeMap<String, String>> {
    let path = dir.join(FILE_NAME);
    match std::fs::read(&path) {
        Ok(bytes) => {
            restrict_permissions(&path);
            decode(&bytes)
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
    let bytes = encode(map)?;
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

/// Le fichier est-il encore en clair alors que la plateforme sait le chiffrer ?
fn needs_reseal(dir: &Path) -> bool {
    seal::ENABLED
        && std::fs::read(dir.join(FILE_NAME))
            .map(|b| b.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'{'))
            .unwrap_or(false)
}

pub fn read(account: &str) -> Result<Option<String>> {
    let _g = LOCK.lock().unwrap();
    let dir = base_dir()?;
    let map = load_in(&dir)?;
    // Fichier hérité d'une version qui ne chiffrait pas : réécrit chiffré dès la première lecture.
    if needs_reseal(&dir) {
        if let Err(e) = save_in(&dir, &map) {
            tracing::warn!("chiffrement du fichier de secrets : {e}");
        }
    }
    Ok(map.get(account).cloned())
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

/// Tous les secrets du fichier, `None` s'il n'existe pas (migration vers un stockage sécurisé).
pub fn take_all() -> Result<Option<BTreeMap<String, String>>> {
    let _g = LOCK.lock().unwrap();
    let dir = base_dir()?;
    if !dir.join(FILE_NAME).exists() {
        return Ok(None);
    }
    load_in(&dir).map(Some)
}

/// Supprime le fichier de secrets (après migration).
pub fn remove_file() -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let dir = base_dir()?;
    let _ = std::fs::remove_file(dir.join(TMP_NAME));
    match std::fs::remove_file(dir.join(FILE_NAME)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Keyring(format!("suppression de {FILE_NAME} : {e}"))),
    }
}

/// Même backend, sur un dossier explicite plutôt que `MINESTRATOR_DATA_DIR` : les tests de
/// l'échange de clés de compte s'y exécutent en parallèle sans variable d'environnement partagée
/// et sans jamais toucher au trousseau réel.
#[cfg(test)]
pub(crate) struct DirStore {
    pub dir: PathBuf,
}

#[cfg(test)]
impl super::SecretStore for DirStore {
    fn read(&self, account: &str) -> Result<Option<String>> {
        let _g = LOCK.lock().unwrap();
        Ok(load_in(&self.dir)?.get(account).cloned())
    }
    fn write(&self, account: &str, value: &str) -> Result<()> {
        let _g = LOCK.lock().unwrap();
        let mut map = load_in(&self.dir)?;
        map.insert(account.to_string(), value.to_string());
        save_in(&self.dir, &map)
    }
    fn delete(&self, account: &str) -> Result<()> {
        let _g = LOCK.lock().unwrap();
        let mut map = load_in(&self.dir)?;
        map.remove(account);
        save_in(&self.dir, &map)
    }
}

/// Chiffrement au repos du fichier (DPAPI sous Windows ; aucun ailleurs, cf. doc du module).
#[cfg(windows)]
mod seal {
    use crate::error::{Error, Result};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    pub const ENABLED: bool = true;

    /// Entropie propre à l'app : un blob DPAPI d'un autre logiciel du même compte ne se
    /// déchiffre pas ici, et inversement.
    const ENTROPY: &[u8] = b"MinestratorTerminal/secrets/v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copie le résultat alloué par DPAPI puis le libère.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY : les blobs d'entrée pointent sur des tranches vivantes pendant l'appel ; la
        // sortie, allouée par DPAPI, est copiée puis libérée par `take`.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(Error::Keyring(format!(
                "chiffrement DPAPI : {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(unsafe { take(out) })
    }

    pub fn open(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = blob(sealed);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY : idem `seal`.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(Error::Keyring(format!(
                "déchiffrement DPAPI (fichier d'un autre compte ou corrompu) : {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod seal {
    use crate::error::{Error, Result};

    pub const ENABLED: bool = false;

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>> {
        Ok(plain.to_vec())
    }

    pub fn open(_: &[u8]) -> Result<Vec<u8>> {
        Err(Error::Keyring("fichier de secrets illisible : JSON attendu".into()))
    }
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

    #[test]
    fn plaintext_file_from_older_versions_is_still_read() {
        let dir = test_dir("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), br#"{"api-key":"ancienne"}"#).unwrap();
        assert_eq!(load_in(&dir).unwrap().get("api-key").map(String::as_str), Some("ancienne"));
        assert_eq!(needs_reseal(&dir), seal::ENABLED);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn secrets_are_encrypted_at_rest_on_windows() {
        let dir = test_dir("dpapi");
        let mut map = BTreeMap::new();
        map.insert("api-key".to_string(), "valeur-secrète-reconnaissable".to_string());
        save_in(&dir, &map).unwrap();
        let raw = std::fs::read(dir.join(FILE_NAME)).unwrap();
        let needle = b"valeur-secr";
        assert!(!raw.windows(needle.len()).any(|w| w == needle), "secret lisible en clair");
        assert!(!needs_reseal(&dir));
        assert_eq!(load_in(&dir).unwrap(), map);
        // Un blob altéré est refusé au lieu d'être lu de travers.
        let mut bad = raw.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        std::fs::write(dir.join(FILE_NAME), bad).unwrap();
        assert!(load_in(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
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
