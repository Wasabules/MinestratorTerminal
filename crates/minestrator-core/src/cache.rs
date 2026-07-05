//! Cache mémoire à expiration (TTL) pour les résultats d'API stables/semi-stables.
//!
//! Il vit aussi longtemps que le [`crate::Core`]. Aujourd'hui le serveur MCP est **respawné à
//! chaque message**, donc le cache ne dédoublonne que DANS un tour (deux appels identiques d'un
//! même agent). Dès que le `Core` deviendra persistant (process agent maintenu en arrière-plan,
//! ou serveur MCP in-process), il dédoublonnera **automatiquement entre les tours**, sans rien
//! changer ici — c'est le but de le placer au niveau du `Core`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Cache clé→valeur (chaînes) avec expiration par entrée. Sûr en concurrence (Mutex).
#[derive(Default)]
pub(crate) struct TtlCache {
    map: Mutex<HashMap<String, (Instant, String)>>,
}

impl TtlCache {
    /// Valeur encore valide pour `key`, sinon `None` (les entrées expirées sont nettoyées au vol).
    pub(crate) fn get(&self, key: &str) -> Option<String> {
        let mut map = self.map.lock().ok()?;
        match map.get(key) {
            Some((exp, val)) if *exp > Instant::now() => Some(val.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    /// Mémorise `val` sous `key` pendant `ttl`.
    pub(crate) fn put(&self, key: String, val: String, ttl: Duration) {
        if let Ok(mut map) = self.map.lock() {
            map.insert(key, (Instant::now() + ttl, val));
        }
    }

    /// Vide tout le cache. Appelé après une opération **modifiante** : l'état serveur lu (fichiers,
    /// mods, statut…) a pu changer, on évite de servir une valeur périmée.
    pub(crate) fn clear(&self) {
        if let Ok(mut map) = self.map.lock() {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_expires() {
        let c = TtlCache::default();
        c.put("k".into(), "v".into(), Duration::from_secs(60));
        assert_eq!(c.get("k").as_deref(), Some("v"));

        // TTL nul → immédiatement expiré au prochain get.
        c.put("z".into(), "old".into(), Duration::from_millis(0));
        assert_eq!(c.get("z"), None);
    }

    #[test]
    fn clear_empties_everything() {
        let c = TtlCache::default();
        c.put("a".into(), "1".into(), Duration::from_secs(60));
        c.put("b".into(), "2".into(), Duration::from_secs(60));
        c.clear();
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("b"), None);
    }
}
