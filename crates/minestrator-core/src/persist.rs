//! Config persistée en JSON dans le dossier data de l'OS, protégée par mutex.
//! Chargée au démarrage (défaut si absente/illisible), réécrite à chaque `set`.
//! Mutualise le triplet load/get/set autrefois recopié (MCP, Copilote, superviseur).

use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) struct PersistedConfig<T> {
    value: Mutex<T>,
    path: PathBuf,
}

impl<T: Serialize + DeserializeOwned + Default + Clone> PersistedConfig<T> {
    pub(crate) fn load(dir: &Path, filename: &str) -> Self {
        let path = dir.join(filename);
        let value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            value: Mutex::new(value),
            path,
        }
    }

    pub(crate) fn get(&self) -> T {
        self.value.lock().unwrap().clone()
    }

    /// Accès EMPRUNTANT sous le lock : évite un `clone()` complet quand on ne lit que quelques
    /// champs (souvent `Copy`) — utile dans les boucles chaudes. La closure ne doit PAS `.await`
    /// (le mutex est synchrone et ne doit pas être tenu à travers un point d'attente).
    pub(crate) fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.value.lock().unwrap())
    }

    pub(crate) fn set(&self, v: T) {
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(&self.path, json);
        }
        *self.value.lock().unwrap() = v;
    }
}
