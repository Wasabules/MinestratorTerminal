//! Plusieurs comptes MineStrator sur un même appareil, et changement de compte.
//!
//! **Stockage**
//! - La clé du compte **actif** reste sous `api-key` ([`crate::config::KEYRING_ACCOUNT`]) : le
//!   serveur MCP headless, les agents CLI du Copilote, le superviseur et l'activation du service
//!   d'arrière-plan la lisent là, sans rien savoir des autres comptes.
//! - La clé de chaque compte **inactif** attend sous `api-key-<idUtilisateur>`
//!   ([`crate::config::KEYRING_ACCOUNT_KEY_PREFIX`]).
//! - La liste, **non secrète** (id, pseudo, date d'ajout, compte actif), vit dans `accounts.json`.
//!   Elle se constitue d'elle-même au premier lancement d'une installation à compte unique : la
//!   première résolution du profil ([`Core::session_user`]) y inscrit le compte de `api-key`.
//!
//! **Échange des clés sans fenêtre de perte.** Basculer de A vers B recopie d'abord la clé de A à
//! l'écart (relue pour vérification), puis installe celle de B sous `api-key`, puis seulement
//! efface la copie de B devenue inutile. Un échec en route rétablit la clé active d'origine ; une
//! interruption brutale laisse au pire un doublon sans danger, résorbé à la mise en cohérence
//! suivante (la clé active fait foi, cf. [`AccountsFile::record_active`]).
//!
//! **Mots de passe SFTP.** Leur index ([`SftpIndex`], `sftp_passwords.json`) rattache chaque
//! serveur au compte qui l'a enregistré : retirer un compte n'efface que les siens. L'index hérité
//! (identifiants seuls) est rattaché au compte actif à la première résolution.
//!
//! Ces opérations sont réservées à l'app (desktop, mobile). Le service d'arrière-plan reste à clé
//! unique (`Core::authenticate_and_store` / `Core::logout`) : il ne garde jamais d'autre clé que
//! celle du compte qu'il surveille.

use crate::error::{Error, Result};
use crate::events::CoreEvent;
use crate::models::UserProfile;
use crate::secrets::{self, SecretStore, SystemStore};
use crate::store::now_secs;
use crate::Core;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// --- Liste des comptes (non secrète) -------------------------------------------------------

/// Compte enregistré, tel que persisté dans `accounts.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredAccount {
    pub id: i64,
    pub pseudo: String,
    /// Date d'ajout sur l'appareil (secondes Unix).
    pub added_at: i64,
}

/// Contenu de `accounts.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountsFile {
    /// Compte dont la clé est sous `api-key` (`None` : aucun, ou pas encore résolu).
    #[serde(default)]
    pub active: Option<i64>,
    /// Comptes dans l'ordre d'ajout.
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
}

impl AccountsFile {
    pub(crate) fn contains(&self, id: i64) -> bool {
        self.accounts.iter().any(|a| a.id == id)
    }

    fn get(&self, id: i64) -> Option<&StoredAccount> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// Le compte `id` (pseudo à jour) est celui dont la clé est sous `api-key`. Inscrit le compte
    /// s'il manque : c'est ainsi que la liste se constitue au premier lancement d'une installation
    /// à compte unique, et qu'elle se répare après un échange interrompu (la clé active fait foi).
    /// Renvoie `true` si le fichier a changé.
    pub(crate) fn record_active(&mut self, id: i64, pseudo: &str, now: i64) -> bool {
        let mut changed = self.active != Some(id);
        self.active = Some(id);
        match self.accounts.iter_mut().find(|a| a.id == id) {
            Some(a) if a.pseudo != pseudo => {
                a.pseudo = pseudo.to_string();
                changed = true;
            }
            Some(_) => {}
            None => {
                self.accounts.push(StoredAccount { id, pseudo: pseudo.to_string(), added_at: now });
                changed = true;
            }
        }
        changed
    }

    /// Ajoute un compte inactif. Refuse un doublon du même utilisateur.
    pub(crate) fn insert(&mut self, id: i64, pseudo: &str, now: i64) -> Result<()> {
        if self.contains(id) || self.active == Some(id) {
            return Err(Error::AccountExists);
        }
        self.accounts.push(StoredAccount { id, pseudo: pseudo.to_string(), added_at: now });
        Ok(())
    }

    /// Met à jour le pseudo d'un compte (il peut changer côté MineStrator).
    fn rename(&mut self, id: i64, pseudo: &str) -> bool {
        match self.accounts.iter_mut().find(|a| a.id == id) {
            Some(a) if a.pseudo != pseudo => {
                a.pseudo = pseudo.to_string();
                true
            }
            _ => false,
        }
    }

    /// Compte qui prend la relève quand `removed` (actif) est retiré : le plus récemment ajouté
    /// des autres. `None` s'il n'en reste aucun.
    pub(crate) fn successor(&self, removed: i64) -> Option<i64> {
        self.accounts
            .iter()
            .filter(|a| a.id != removed)
            .max_by_key(|a| a.added_at)
            .map(|a| a.id)
    }

    /// Retire un compte de la liste ; le compte actif devient `next` si c'était lui.
    pub(crate) fn remove(&mut self, id: i64, next: Option<i64>) {
        self.accounts.retain(|a| a.id != id);
        if self.active == Some(id) {
            self.active = next;
        }
    }

    /// Vue pour l'UI.
    pub(crate) fn view(&self, id: i64) -> Option<Account> {
        self.get(id).map(|a| Account {
            id: a.id,
            pseudo: a.pseudo.clone(),
            added_at: a.added_at,
            active: self.active == Some(a.id),
        })
    }

    pub(crate) fn list(&self) -> Vec<Account> {
        self.accounts.iter().filter_map(|a| self.view(a.id)).collect()
    }
}

/// Compte MineStrator enregistré sur l'appareil (aucun secret : la clé reste dans le stockage
/// sécurisé).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Account {
    /// Identifiant utilisateur MineStrator.
    pub id: i64,
    pub pseudo: String,
    /// Date d'ajout sur l'appareil (secondes Unix).
    pub added_at: i64,
    /// Compte actif (celui de l'app, du MCP et des agents du Copilote).
    pub active: bool,
}

/// Payload de l'event `account://switched` : le compte actif a changé (bascule, ajout depuis
/// l'écran de connexion, retrait). L'UI ferme les onglets serveurs de l'ancien compte et
/// recharge ses listes ; `current = null` ramène à l'écran de connexion.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountSwitch {
    pub previous: Option<i64>,
    pub current: Option<Account>,
}

// --- Échange des clés --------------------------------------------------------------------

use crate::config::KEYRING_ACCOUNT as ACTIVE_KEY;

/// Écrit une clé puis la relit : une copie de sauvegarde ne compte que vérifiée.
fn write_verified(store: &dyn SecretStore, account: &str, value: &str) -> Result<()> {
    store.write(account, value)?;
    match store.read(account)? {
        Some(v) if v == value => Ok(()),
        _ => Err(Error::Keyring(format!("copie de la clé non vérifiée ({account})"))),
    }
}

/// Efface une entrée devenue inutile (doublon). Best-effort : un doublon est sans danger.
fn delete_leftover(store: &dyn SecretStore, account: &str) {
    if let Err(e) = store.delete(account) {
        tracing::warn!("doublon de clé non effacé ({account}) : {e}");
    }
}

/// Bascule du compte actif `from` vers `to`, dont la clé attend sous `api-key-<to>`. Renvoie la
/// clé désormais active.
///
/// Ordre : (1) copie vérifiée de la clé active sous `api-key-<from>` ; (2) clé de `to` sous
/// `api-key` — en cas d'échec, la clé d'origine y est réécrite ; (3) effacement de la copie de
/// `to`. À aucun moment une clé n'existe nulle part ailleurs que dans une écriture en cours.
pub(crate) fn swap_keys(store: &dyn SecretStore, from: Option<i64>, to: i64) -> Result<String> {
    let target = store.read(&secrets::account_key_name(to))?.ok_or(Error::AccountNotFound)?;
    let current = store.read(ACTIVE_KEY)?;
    if let Some(cur) = &current {
        match from {
            Some(from) if from != to => {
                write_verified(store, &secrets::account_key_name(from), cur)?;
            }
            // La clé active appartient déjà à `to` : rien à mettre de côté.
            Some(_) => {}
            // Clé active d'un compte non identifié : l'écraser la perdrait.
            None => {
                return Err(Error::Keyring(
                    "clé active d'un compte non identifié : bascule refusée".into(),
                ))
            }
        }
    }
    if let Err(e) = store.write(ACTIVE_KEY, &target) {
        if let Some(cur) = &current {
            if let Err(e2) = store.write(ACTIVE_KEY, cur) {
                // La copie vérifiée de l'étape 1 garde la clé : la mise en cohérence suivante
                // retrouvera le compte (et l'UI le proposera à la bascule).
                tracing::error!("clé active non rétablie après échec ({e2})");
            }
        }
        return Err(e);
    }
    delete_leftover(store, &secrets::account_key_name(to));
    Ok(target)
}

/// Installe une clé fraîchement validée comme clé active (écran de connexion). La clé active
/// précédente, si son compte `previous` est connu et différent, est mise de côté plutôt
/// qu'écrasée : le compte reste dans la liste.
pub(crate) fn install_active_key(
    store: &dyn SecretStore,
    previous: Option<i64>,
    owner: i64,
    token: &str,
) -> Result<()> {
    let current = store.read(ACTIVE_KEY)?;
    if let (Some(prev), Some(cur)) = (previous, &current) {
        if prev != owner {
            write_verified(store, &secrets::account_key_name(prev), cur)?;
        }
    }
    if let Err(e) = store.write(ACTIVE_KEY, token) {
        if let Some(cur) = &current {
            let _ = store.write(ACTIVE_KEY, cur);
        }
        return Err(e);
    }
    delete_leftover(store, &secrets::account_key_name(owner));
    Ok(())
}

/// Retire le compte actif `removed` : la clé de `next` (s'il y en a un) prend sa place sous
/// `api-key`, sinon `api-key` est effacée. Renvoie la clé désormais active.
///
/// Un doublon de la clé retirée (échange interrompu) est effacé **d'abord** : en cas d'échec,
/// rien n'a changé et l'opération peut être reprise.
pub(crate) fn drop_active_key(
    store: &dyn SecretStore,
    removed: i64,
    next: Option<i64>,
) -> Result<Option<String>> {
    store.delete(&secrets::account_key_name(removed))?;
    match next {
        Some(next) => {
            let target =
                store.read(&secrets::account_key_name(next))?.ok_or(Error::AccountNotFound)?;
            // Écrase (donc efface) la clé retirée ; en cas d'échec elle reste active, rien n'est perdu.
            store.write(ACTIVE_KEY, &target)?;
            delete_leftover(store, &secrets::account_key_name(next));
            Ok(Some(target))
        }
        None => {
            store.delete(ACTIVE_KEY)?;
            Ok(None)
        }
    }
}

/// Après résolution du compte actif : une copie de sa clé encore rangée à l'écart (échange
/// interrompu entre l'installation et le ménage) n'a plus lieu d'être.
pub(crate) fn settle_active_key(store: &dyn SecretStore, active: i64) {
    delete_leftover(store, &secrets::account_key_name(active));
}

// --- Index des mots de passe SFTP -----------------------------------------------------------

/// Index des mots de passe SFTP enregistrés : serveur → compte qui l'a enregistré (`None` :
/// entrée héritée d'une version à compte unique, pas encore rattachée). Les mots de passe sont
/// dans le stockage sécurisé ; l'index sert à l'UI et à les effacer avec leur compte (le trousseau
/// ne sait pas lister ses entrées).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "SftpIndexFile", into = "SftpIndexFile")]
pub(crate) struct SftpIndex(BTreeMap<i64, Option<i64>>);

/// Formats de `sftp_passwords.json`.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SftpIndexFile {
    /// Serveur → compte (format actuel). Clés en chaînes : un enum `untagged` relit le JSON via
    /// un tampon générique, qui ne convertit pas les clés d'objet en entiers.
    Owned(BTreeMap<String, Option<i64>>),
    /// Version 0.6.1 : identifiants de serveurs seuls (un seul compte).
    Legacy(BTreeSet<i64>),
}

impl From<SftpIndexFile> for SftpIndex {
    fn from(f: SftpIndexFile) -> Self {
        match f {
            SftpIndexFile::Owned(map) => SftpIndex(
                map.into_iter()
                    .filter_map(|(server, owner)| Some((server.parse().ok()?, owner)))
                    .collect(),
            ),
            SftpIndexFile::Legacy(ids) => SftpIndex(ids.into_iter().map(|id| (id, None)).collect()),
        }
    }
}

impl From<SftpIndex> for SftpIndexFile {
    fn from(i: SftpIndex) -> Self {
        SftpIndexFile::Owned(i.0.into_iter().map(|(s, o)| (s.to_string(), o)).collect())
    }
}

impl SftpIndex {
    pub(crate) fn contains(&self, server: i64) -> bool {
        self.0.contains_key(&server)
    }

    pub(crate) fn insert(&mut self, server: i64, owner: Option<i64>) {
        self.0.insert(server, owner);
    }

    pub(crate) fn remove(&mut self, server: i64) {
        self.0.remove(&server);
    }

    /// Rattache les entrées héritées au compte `owner`. Renvoie `true` si l'index a changé.
    pub(crate) fn adopt_orphans(&mut self, owner: i64) -> bool {
        let mut changed = false;
        for o in self.0.values_mut().filter(|o| o.is_none()) {
            *o = Some(owner);
            changed = true;
        }
        changed
    }

    /// Serveurs dont le mot de passe appartient à `owner` (entrées héritées comprises si
    /// `with_orphans`).
    pub(crate) fn servers_of(&self, owner: i64, with_orphans: bool) -> Vec<i64> {
        self.0
            .iter()
            .filter(|(_, o)| **o == Some(owner) || (with_orphans && o.is_none()))
            .map(|(s, _)| *s)
            .collect()
    }

    /// Entrées héritées, pas encore rattachées à un compte.
    pub(crate) fn orphans(&self) -> Vec<i64> {
        self.0.iter().filter(|(_, o)| o.is_none()).map(|(s, _)| *s).collect()
    }

    pub(crate) fn all(&self) -> Vec<i64> {
        self.0.keys().copied().collect()
    }

    /// Efface les mots de passe de `servers` via `delete` ; un échec garde l'entrée, pour
    /// réessayer au prochain retrait.
    pub(crate) fn purge(&mut self, servers: &[i64], delete: impl Fn(i64) -> Result<()>) {
        for &id in servers {
            match delete(id) {
                Ok(()) => self.remove(id),
                Err(e) => tracing::warn!(server = id, "suppression du mot de passe SFTP : {e}"),
            }
        }
    }
}

// --- Opérations du Core (app) --------------------------------------------------------------

impl Core {
    /// Comptes enregistrés sur l'appareil, dans l'ordre d'ajout (le compte actif est marqué).
    pub fn accounts(&self) -> Vec<Account> {
        self.accounts.get().list()
    }

    /// Profil du compte actif, et mise en cohérence de la liste des comptes : constitution au
    /// premier lancement (compte unique), pseudo à jour, reprise après un échange interrompu,
    /// rattachement de l'index SFTP hérité. À appeler par l'app pour valider sa session.
    pub async fn session_user(&self) -> Result<UserProfile> {
        let profile = self.get_user().await?;
        let _op = self.account_op.lock().await;
        self.settle_active(&profile);
        Ok(profile)
    }

    /// Le compte de la clé active est `profile` : la liste et l'index SFTP s'alignent dessus.
    fn settle_active(&self, profile: &UserProfile) {
        let mut file = self.accounts.get();
        let previous = file.active;
        if file.record_active(profile.id, &profile.pseudo, now_secs()) {
            self.accounts.set(file);
        }
        if previous != Some(profile.id) {
            settle_active_key(&SystemStore, profile.id);
        }
        *self.user_id_cache.lock().unwrap() = Some(profile.id);
        let mut idx = self.sftp_saved.get();
        if idx.adopt_orphans(profile.id) {
            self.sftp_saved.set(idx);
        }
    }

    /// Compte propriétaire de la clé active. Si la liste ne le connaît pas (installation à compte
    /// unique dont la clé n'a jamais été résolue, typiquement parce qu'elle a cessé de valider), il
    /// est identifié auprès de l'API et inscrit, pour que sa clé soit mise de côté plutôt
    /// qu'écrasée. `None` : pas de clé active, ou clé refusée (sans valeur à préserver).
    async fn active_owner(&self, file: &mut AccountsFile) -> Result<Option<i64>> {
        if file.active.is_some() {
            return Ok(file.active);
        }
        let Some(current) = secrets::read_key()? else {
            return Ok(None);
        };
        Ok(match self.api.get_user(&current).await {
            Ok(u) => {
                let _ = file.insert(u.id, &u.pseudo, now_secs());
                Some(u.id)
            }
            Err(_) => None,
        })
    }

    /// Connexion depuis l'écran de connexion : valide la clé auprès de l'API et en fait la clé
    /// active. Le compte est ajouté s'il est nouveau, sa clé remplacée s'il était déjà enregistré
    /// (clé régénérée). La clé active précédente, si elle appartient à un autre compte, est mise de
    /// côté : ce compte reste disponible à la bascule.
    pub async fn sign_in(&self, key: &str) -> Result<UserProfile> {
        let (profile, token) = self.api.authenticate(key).await?;
        let _op = self.account_op.lock().await;
        let mut file = self.accounts.get();
        let owner = self.active_owner(&mut file).await?;
        install_active_key(&SystemStore, owner, profile.id, &token)?;
        file.record_active(profile.id, &profile.pseudo, now_secs());
        self.accounts.set(file.clone());
        tracing::info!(user = %profile.pseudo, "connexion : clé API validée et enregistrée");
        self.after_account_change(owner, file.view(profile.id), Some(token)).await;
        Ok(profile)
    }

    /// Ajoute un compte (« Ajouter un compte ») : la clé est validée auprès de l'API, un doublon
    /// du même utilisateur est refusé. Le compte est rangé **inactif** — sauf s'il n'y en avait
    /// aucun, auquel cas il devient le compte actif.
    pub async fn add_account(&self, key: &str) -> Result<Account> {
        let (profile, token) = self.api.authenticate(key).await?;
        let _op = self.account_op.lock().await;
        let mut file = self.accounts.get();
        if file.contains(profile.id) || file.active == Some(profile.id) {
            return Err(Error::AccountExists);
        }
        if file.active.is_none() && secrets::read_key()?.is_none() {
            install_active_key(&SystemStore, None, profile.id, &token)?;
            file.record_active(profile.id, &profile.pseudo, now_secs());
            self.accounts.set(file.clone());
            let account = file.view(profile.id);
            self.after_account_change(None, account.clone(), Some(token)).await;
            return account.ok_or(Error::AccountNotFound);
        }
        secrets::store_account_key(profile.id, &token)?;
        file.insert(profile.id, &profile.pseudo, now_secs())?;
        self.accounts.set(file.clone());
        tracing::info!(user = %profile.pseudo, "compte ajouté");
        file.view(profile.id).ok_or(Error::AccountNotFound)
    }

    /// Bascule vers un autre compte enregistré. Sa clé est d'abord validée auprès de l'API
    /// (une clé révoquée est refusée, le compte actif reste en place), puis les clés sont échangées
    /// sans fenêtre de perte ; enfin consoles et sessions SFTP sont fermées, caches vidés,
    /// superviseur relancé pour le nouveau compte, et `CoreEvent::AccountSwitched` émis.
    pub async fn switch_account(&self, id: i64) -> Result<Account> {
        let _op = self.account_op.lock().await;
        let mut file = self.accounts.get();
        if file.active == Some(id) {
            return file.view(id).ok_or(Error::AccountNotFound);
        }
        if !file.contains(id) {
            return Err(Error::AccountNotFound);
        }
        let candidate = secrets::read_account_key(id)?.ok_or(Error::AccountNotFound)?;
        let profile = self.api.get_user(&candidate).await?;
        if profile.id != id {
            return Err(Error::Unexpected(
                "la clé enregistrée pour ce compte appartient à un autre utilisateur".into(),
            ));
        }
        // Clé active d'un compte non identifié et refusée par l'API : `swap_keys` refusera de
        // l'écraser (se déconnecter l'efface, puis la bascule devient possible).
        let previous = self.active_owner(&mut file).await?;
        let token = swap_keys(&SystemStore, previous, id)?;
        file.active = Some(id);
        file.rename(id, &profile.pseudo);
        self.accounts.set(file.clone());
        let account = file.view(id);
        tracing::info!(user = %profile.pseudo, "changement de compte");
        self.after_account_change(previous, account.clone(), Some(token)).await;
        account.ok_or(Error::AccountNotFound)
    }

    /// Retire un compte : efface sa clé et ses mots de passe SFTP. Retirer le compte actif bascule
    /// sur le plus récent des autres ; s'il n'en reste aucun, l'app revient à l'écran de connexion.
    /// Renvoie le compte actif après l'opération (`None` : plus aucun).
    pub async fn remove_account(&self, id: i64) -> Result<Option<Account>> {
        let _op = self.account_op.lock().await;
        let mut file = self.accounts.get();
        if file.active != Some(id) && !file.contains(id) {
            return Err(Error::AccountNotFound);
        }
        if file.active != Some(id) {
            secrets::delete_account_key(id)?;
            self.purge_sftp_passwords_of(id, false);
            file.remove(id, None);
            self.accounts.set(file.clone());
            return Ok(file.active.and_then(|a| file.view(a)));
        }
        let next = file.successor(id);
        let token = drop_active_key(&SystemStore, id, next)?;
        self.purge_sftp_passwords_of(id, true);
        file.remove(id, next);
        self.accounts.set(file.clone());
        let current = next.and_then(|n| file.view(n));
        tracing::info!(next = ?next, "compte actif retiré");
        self.after_account_change(Some(id), current.clone(), token).await;
        Ok(current)
    }

    /// Déconnexion de l'app : retire le compte actif (voir [`Self::remove_account`]). Sans compte
    /// actif identifié, efface la clé active et les mots de passe SFTP hérités.
    pub async fn sign_out(&self) -> Result<Option<Account>> {
        let active = self.accounts.get().active;
        if let Some(id) = active {
            return self.remove_account(id).await;
        }
        let _op = self.account_op.lock().await;
        secrets::delete_key()?;
        let mut idx = self.sftp_saved.get();
        let orphans = idx.orphans();
        idx.purge(&orphans, secrets::delete_sftp_password);
        self.sftp_saved.set(idx);
        self.after_account_change(None, None, None).await;
        Ok(None)
    }

    /// Efface les mots de passe SFTP enregistrés par un compte (et les entrées héritées si
    /// `with_orphans` : elles datent du compte unique, donc du compte actif).
    fn purge_sftp_passwords_of(&self, owner: i64, with_orphans: bool) {
        let mut idx = self.sftp_saved.get();
        let servers = idx.servers_of(owner, with_orphans);
        for &id in &servers {
            self.sftp.drop_session(id);
        }
        idx.purge(&servers, secrets::delete_sftp_password);
        self.sftp_saved.set(idx);
    }

    /// Le compte actif a changé : tout ce qui portait l'ancien est remis à zéro, la supervision est
    /// redécidée (le service d'arrière-plan garde son compte : le superviseur local reprend si le
    /// nouveau compte n'est pas le sien), puis l'UI est prévenue.
    async fn after_account_change(
        &self,
        previous: Option<i64>,
        current: Option<Account>,
        token: Option<String>,
    ) {
        *self.token_cache.lock().unwrap() = token;
        *self.user_id_cache.lock().unwrap() = current.as_ref().map(|a| a.id);
        self.cache.clear();
        self.console.disconnect_all();
        self.sftp.clear();
        // Conversations de l'assistant : leurs agents persistants portent un MCP lancé avec l'ancienne clé.
        self.chat_sessions.lock().unwrap().clear();
        // Qui supervise, pour le nouveau compte — AVANT de réveiller le superviseur local, pour
        // qu'il n'ouvre pas de connexions monitor si le service surveille déjà ce compte.
        self.service_tick().await;
        self.sup_state.wake();
        self.emit(CoreEvent::AccountSwitched(AccountSwitch { previous, current }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::DirStore;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn temp_store(name: &str) -> DirStore {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "minestrator-test-accounts-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        DirStore { dir }
    }

    fn cleanup(store: DirStore) {
        let _ = std::fs::remove_dir_all(store.dir);
    }

    fn stash(id: i64) -> String {
        secrets::account_key_name(id)
    }

    fn get(store: &dyn SecretStore, account: &str) -> Option<String> {
        store.read(account).unwrap()
    }

    /// Stockage qui échoue sur l'écriture ou la suppression d'une entrée donnée.
    struct Faulty<'a> {
        inner: &'a DirStore,
        fail_write: Option<&'static str>,
        fail_delete: Option<&'static str>,
        /// Échecs restants de l'écriture fautive (`usize::MAX` : panne persistante).
        failures_left: Cell<usize>,
    }

    impl<'a> Faulty<'a> {
        fn new(inner: &'a DirStore) -> Self {
            Faulty {
                inner,
                fail_write: None,
                fail_delete: None,
                failures_left: Cell::new(usize::MAX),
            }
        }
    }

    impl SecretStore for Faulty<'_> {
        fn read(&self, account: &str) -> Result<Option<String>> {
            self.inner.read(account)
        }
        fn write(&self, account: &str, value: &str) -> Result<()> {
            if self.fail_write == Some(account) && self.failures_left.get() > 0 {
                self.failures_left.set(self.failures_left.get() - 1);
                return Err(Error::Keyring(format!("panne simulée ({account})")));
            }
            self.inner.write(account, value)
        }
        fn delete(&self, account: &str) -> Result<()> {
            if self.fail_delete == Some(account) {
                return Err(Error::Keyring(format!("panne simulée ({account})")));
            }
            self.inner.delete(account)
        }
    }

    // --- Liste --------------------------------------------------------------------------

    #[test]
    fn liste_constituee_au_premier_lancement() {
        // Installation à compte unique : pas de `accounts.json`, une clé sous `api-key`.
        let mut file = AccountsFile::default();
        assert!(file.record_active(42, "Steve", 1000));
        assert_eq!(file.active, Some(42));
        assert_eq!(
            file.list(),
            vec![Account { id: 42, pseudo: "Steve".into(), added_at: 1000, active: true }]
        );
        // Lancements suivants : rien à réécrire.
        assert!(!file.record_active(42, "Steve", 2000));
        // Pseudo changé côté MineStrator : mis à jour, date d'ajout conservée.
        assert!(file.record_active(42, "Steve2", 3000));
        assert_eq!(file.accounts[0].pseudo, "Steve2");
        assert_eq!(file.accounts[0].added_at, 1000);
    }

    #[test]
    fn fichier_absent_ou_ancien_illisible_donne_une_liste_vide() {
        let file: AccountsFile = serde_json::from_str("{}").unwrap();
        assert_eq!(file, AccountsFile::default());
        let json = serde_json::to_string(&AccountsFile {
            active: Some(1),
            accounts: vec![StoredAccount { id: 1, pseudo: "a".into(), added_at: 5 }],
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<AccountsFile>(&json).unwrap().list()[0],
            Account { id: 1, pseudo: "a".into(), added_at: 5, active: true }
        );
    }

    #[test]
    fn doublon_refuse() {
        let mut file = AccountsFile::default();
        file.record_active(1, "a", 10);
        file.insert(2, "b", 20).unwrap();
        assert!(matches!(file.insert(2, "b", 30), Err(Error::AccountExists)));
        assert!(matches!(file.insert(1, "a", 30), Err(Error::AccountExists)));
        assert_eq!(file.accounts.len(), 2);
        assert_eq!(file.list().iter().filter(|a| a.active).count(), 1);
    }

    #[test]
    fn retrait_du_compte_actif_designe_le_plus_recent() {
        let mut file = AccountsFile::default();
        file.record_active(1, "a", 10);
        file.insert(2, "b", 20).unwrap();
        file.insert(3, "c", 30).unwrap();
        assert_eq!(file.successor(1), Some(3));
        file.remove(1, file.successor(1));
        assert_eq!(file.active, Some(3));
        assert_eq!(file.list().len(), 2);
        // Retrait d'un compte inactif : le compte actif ne bouge pas.
        file.remove(2, None);
        assert_eq!(file.active, Some(3));
        // Dernier compte : plus de successeur.
        assert_eq!(file.successor(3), None);
        file.remove(3, None);
        assert_eq!(file.active, None);
        assert!(file.list().is_empty());
    }

    #[test]
    fn reprise_apres_echange_interrompu() {
        // Bascule 1 → 2 interrompue après l'installation de la clé : `accounts.json` désigne
        // encore 1, mais la clé active est celle de 2 (et sa copie traîne encore).
        let store = temp_store("reprise");
        store.write(ACTIVE_KEY, "K2").unwrap();
        store.write(&stash(1), "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut file = AccountsFile::default();
        file.record_active(1, "a", 10);
        file.insert(2, "b", 20).unwrap();

        // La résolution du profil de la clé active (2) remet tout d'aplomb.
        assert!(file.record_active(2, "b", 99));
        settle_active_key(&store, 2);
        assert_eq!(file.active, Some(2));
        assert_eq!(file.accounts.len(), 2);
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(1)).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)), None);
        // Et la bascule retour fonctionne.
        assert_eq!(swap_keys(&store, Some(2), 1).unwrap(), "K1");
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        cleanup(store);
    }

    // --- Échange des clés (backend fichier, dossier temporaire) ------------------------

    #[test]
    fn echange_nominal() {
        let store = temp_store("echange");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        assert_eq!(swap_keys(&store, Some(1), 2).unwrap(), "K2");
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(1)).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)), None);
        cleanup(store);
    }

    #[test]
    fn echec_de_la_mise_de_cote_ne_change_rien() {
        let store = temp_store("echec-copie");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut faulty = Faulty::new(&store);
        faulty.fail_write = Some("api-key-1");
        assert!(swap_keys(&faulty, Some(1), 2).is_err());
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(1)), None);
        cleanup(store);
    }

    #[test]
    fn echec_de_l_installation_retablit_la_cle_d_origine() {
        let store = temp_store("echec-install");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut faulty = Faulty::new(&store);
        // Seule la 1ʳᵉ écriture de `api-key` échoue : le rétablissement (2ᵉ) passe.
        faulty.fail_write = Some(ACTIVE_KEY);
        faulty.failures_left.set(1);
        assert!(swap_keys(&faulty, Some(1), 2).is_err());
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        // Aucune clé perdue : K2 toujours à l'écart, K1 active (et sa copie, sans danger).
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(1)).as_deref(), Some("K1"));
        cleanup(store);
    }

    #[test]
    fn panne_persistante_garde_toutes_les_cles() {
        // `api-key` refuse toute écriture : la clé d'origine n'est pas rétablie… mais elle n'a
        // jamais été effacée, et sa copie vérifiée existe.
        let store = temp_store("panne");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut faulty = Faulty::new(&store);
        faulty.fail_write = Some(ACTIVE_KEY);
        assert!(swap_keys(&faulty, Some(1), 2).is_err());
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(1)).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        cleanup(store);
    }

    #[test]
    fn echec_du_menage_laisse_un_doublon_sans_danger() {
        let store = temp_store("echec-menage");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut faulty = Faulty::new(&store);
        faulty.fail_delete = Some("api-key-2");
        assert_eq!(swap_keys(&faulty, Some(1), 2).unwrap(), "K2");
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        // Résorbé à la mise en cohérence suivante.
        settle_active_key(&store, 2);
        assert_eq!(get(&store, &stash(2)), None);
        cleanup(store);
    }

    #[test]
    fn bascule_refusee_sans_cle_cible_ou_proprietaire_inconnu() {
        let store = temp_store("refus");
        store.write(ACTIVE_KEY, "K1").unwrap();
        assert!(matches!(swap_keys(&store, Some(1), 2), Err(Error::AccountNotFound)));
        store.write(&stash(2), "K2").unwrap();
        // Clé active d'un compte non identifié : jamais écrasée.
        assert!(swap_keys(&store, None, 2).is_err());
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        cleanup(store);
    }

    #[test]
    fn connexion_met_l_ancienne_cle_de_cote() {
        let store = temp_store("connexion");
        store.write(ACTIVE_KEY, "K1").unwrap();
        install_active_key(&store, Some(1), 3, "K3").unwrap();
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K3"));
        assert_eq!(get(&store, &stash(1)).as_deref(), Some("K1"));
        // Même compte (clé régénérée) : remplacement simple, pas de copie.
        install_active_key(&store, Some(3), 3, "K3bis").unwrap();
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K3bis"));
        assert_eq!(get(&store, &stash(3)), None);
        cleanup(store);
    }

    #[test]
    fn retrait_du_compte_actif() {
        let store = temp_store("retrait");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(1), "K1-doublon").unwrap();
        store.write(&stash(2), "K2").unwrap();
        assert_eq!(drop_active_key(&store, 1, Some(2)).unwrap().as_deref(), Some("K2"));
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K2"));
        assert_eq!(get(&store, &stash(1)), None);
        assert_eq!(get(&store, &stash(2)), None);
        // Dernier compte : plus aucune clé.
        assert_eq!(drop_active_key(&store, 2, None).unwrap(), None);
        assert_eq!(get(&store, ACTIVE_KEY), None);
        cleanup(store);
    }

    #[test]
    fn retrait_en_panne_ne_perd_rien() {
        let store = temp_store("retrait-panne");
        store.write(ACTIVE_KEY, "K1").unwrap();
        store.write(&stash(2), "K2").unwrap();
        let mut faulty = Faulty::new(&store);
        faulty.fail_write = Some(ACTIVE_KEY);
        assert!(drop_active_key(&faulty, 1, Some(2)).is_err());
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        assert_eq!(get(&store, &stash(2)).as_deref(), Some("K2"));
        // Compte suivant sans clé : refus, rien n'est effacé.
        store.delete(&stash(2)).unwrap();
        assert!(matches!(drop_active_key(&store, 1, Some(2)), Err(Error::AccountNotFound)));
        assert_eq!(get(&store, ACTIVE_KEY).as_deref(), Some("K1"));
        cleanup(store);
    }

    // --- Index SFTP ---------------------------------------------------------------------

    #[test]
    fn index_sftp_herite_puis_rattache() {
        // Format 0.6.1 : une liste d'identifiants de serveurs.
        let mut idx: SftpIndex = serde_json::from_str("[377829, 12]").unwrap();
        assert!(idx.contains(377829) && idx.contains(12));
        assert_eq!(idx.servers_of(42, false), Vec::<i64>::new());
        assert!(idx.adopt_orphans(42));
        assert!(!idx.adopt_orphans(7), "déjà rattachées");
        assert_eq!(idx.servers_of(42, false), vec![12, 377829]);
        // Nouveau format : serveur → compte, relu tel quel.
        let json = serde_json::to_string(&idx).unwrap();
        assert_eq!(json, r#"{"12":42,"377829":42}"#);
        assert_eq!(serde_json::from_str::<SftpIndex>(&json).unwrap(), idx);
        // Fichier absent ou vide : index vide.
        assert_eq!(serde_json::from_str::<SftpIndex>("[]").unwrap(), SftpIndex::default());
        assert_eq!(serde_json::from_str::<SftpIndex>("{}").unwrap(), SftpIndex::default());
    }

    #[test]
    fn purge_par_compte() {
        let mut idx = SftpIndex::default();
        idx.insert(1, Some(10));
        idx.insert(2, Some(20));
        idx.insert(3, Some(10));
        idx.insert(4, None);
        let deleted = std::cell::RefCell::new(Vec::new());
        // Le compte 10 n'emporte que ses serveurs ; la suppression de 3 échoue (gardé pour réessayer).
        let servers = idx.servers_of(10, false);
        idx.purge(&servers, |id| {
            if id == 3 {
                return Err(Error::Keyring("verrouillé".into()));
            }
            deleted.borrow_mut().push(id);
            Ok(())
        });
        assert_eq!(*deleted.borrow(), vec![1]);
        assert_eq!(idx.all(), vec![2, 3, 4]);
        // Compte actif retiré avec les entrées héritées.
        assert_eq!(idx.servers_of(20, true), vec![2, 4]);
        let servers = idx.servers_of(20, true);
        idx.purge(&servers, |_| Ok(()));
        assert_eq!(idx.all(), vec![3]);
    }
}
