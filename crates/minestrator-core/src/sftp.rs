//! Client SFTP natif (SSH via `russh` + `russh-sftp`). Session poolée par serveur.
//!
//! Identifiants (hôte, port, utilisateur, **mot de passe**) : `GET /server/{id}/sftp`, endpoint non
//! documenté du panel web — le bloc `sftp` de `GET /server/{id}` ne contient plus le mot de passe
//! depuis fin 2026 (cf. `api::get_sftp_creds`). En secours, l'utilisateur peut saisir un mot de passe
//! par serveur : il est vérifié, rangé dans le stockage sécurisé (`secrets`) et ne quitte jamais le
//! cœur.
//!
//! **Identité du serveur** : l'empreinte de la clé d'hôte SSH est mémorisée au premier contact
//! (confiance au premier usage, comme `known_hosts` d'OpenSSH), par `hôte:port` — tous les serveurs
//! d'un même nœud la partagent. Une empreinte différente ensuite **bloque** la connexion avant
//! l'envoi du mot de passe (`SftpHostKeyChanged`) : il peut s'agir d'une réinstallation du nœud par
//! MineStrator, ou d'une interception. Seul l'utilisateur peut accepter la nouvelle empreinte.

use crate::api::ApiClient;
use crate::error::{Error, Result};
use crate::models::{SftpCreds, SftpEntry};
use crate::persist::PersistedConfig;
use russh::client;
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_EDIT_BYTES: u64 = 2 * 1024 * 1024;

/// Empreinte SHA-256 (format OpenSSH `SHA256:…`) et algorithme de la clé présentée par le serveur.
fn fingerprint_of(key: &PublicKeyOrCertificate) -> (String, String) {
    match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => (
            key.fingerprint(HashAlg::Sha256).to_string(),
            key.algorithm().to_string(),
        ),
        PublicKeyOrCertificate::Certificate(cert) => (
            cert.public_key().fingerprint(HashAlg::Sha256).to_string(),
            cert.algorithm().to_string(),
        ),
    }
}

/// Vérifie la clé d'hôte pendant la poignée de main : acceptée si aucune n'est connue (premier
/// contact) ou si elle correspond à celle mémorisée. Celle reçue est notée dans tous les cas, pour
/// la mémoriser ou expliquer le refus.
struct Handler {
    expected: Option<String>,
    seen: Arc<Mutex<Option<(String, String)>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let (fingerprint, algorithm) = fingerprint_of(server_public_key);
        let accepted = host_key_accepted(self.expected.as_deref(), &fingerprint);
        *self.seen.lock().unwrap() = Some((fingerprint, algorithm));
        Ok(accepted)
    }
}

fn host_key_accepted(expected: Option<&str>, found: &str) -> bool {
    expected.is_none_or(|e| e == found)
}

/// Empreinte connue d'un hôte SFTP (donnée publique : rien de secret ici).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownHost {
    pub fingerprint: String,
    pub algorithm: String,
    /// Première rencontre (secondes Unix).
    pub first_seen: i64,
}

/// Empreintes mémorisées (`sftp_known_hosts.json`) et empreintes refusées en attente de décision
/// de l'utilisateur (en mémoire : seule celle réellement présentée par le serveur peut être
/// acceptée, jamais une valeur arbitraire).
pub struct KnownHosts {
    known: PersistedConfig<BTreeMap<String, KnownHost>>,
    pending: Mutex<HashMap<String, (String, String)>>,
}

impl KnownHosts {
    pub fn load(dir: &Path) -> Self {
        Self {
            known: PersistedConfig::load(dir, "sftp_known_hosts.json"),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, host: &str) -> Option<KnownHost> {
        self.known.with(|m| m.get(host).cloned())
    }

    fn remember(&self, host: &str, fingerprint: String, algorithm: String) {
        let mut map = self.known.get();
        map.insert(
            host.to_string(),
            KnownHost { fingerprint, algorithm, first_seen: crate::store::now_secs() },
        );
        self.known.set(map);
    }

    /// Empreinte refusée en attente de décision pour cet hôte, s'il y en a une.
    pub fn pending(&self, host: &str) -> Option<(String, String)> {
        self.pending.lock().unwrap().get(host).cloned()
    }

    /// Accepte la nouvelle empreinte d'un hôte. Elle doit être exactement celle que le serveur a
    /// présentée lors du refus : l'interface ne peut pas faire accepter autre chose.
    pub fn trust_pending(&self, host: &str, fingerprint: &str) -> Result<()> {
        let pending = self.pending.lock().unwrap().remove(host);
        match pending {
            Some((found, algorithm)) if found == fingerprint => {
                self.remember(host, found, algorithm);
                Ok(())
            }
            other => {
                if let Some(p) = other {
                    self.pending.lock().unwrap().insert(host.to_string(), p);
                }
                Err(Error::Unexpected(
                    "Cette empreinte n'est pas celle présentée par le serveur : reconnecte-toi pour la revoir.".into(),
                ))
            }
        }
    }
}

/// Identifiant d'hôte des empreintes : `hôte:port`.
pub fn host_id(creds: &SftpCreds) -> String {
    format!("{}:{}", creds.host, creds.port)
}

pub struct SftpConn {
    sftp: SftpSession,
    _handle: client::Handle<Handler>,
}

pub struct SftpManager {
    sessions: Mutex<HashMap<i64, Arc<SftpConn>>>,
    hosts: KnownHosts,
}

/// La session est-elle morte ? Seul ce cas justifie de purger le pool et de rejouer.
///
/// Distinction essentielle : une erreur **métier** (fichier absent, permission refusée) arrive
/// sur une session parfaitement saine. La confondre avec une panne de connexion fait fermer la
/// session et payer une poignée de main SSH complète pour re-produire la même erreur.
///
/// Sur mobile, le cas courant est le passage de l'app en arrière-plan : Android coupe son réseau
/// (`os error 103`) et la session meurt sans que rien ne le signale avant la prochaine opération.
pub fn is_session_dead(e: &Error) -> bool {
    match e {
        // Toute la couche connexion (SSH, canal, réseau) remonte en `Network`.
        Error::Network(_) => true,
        Error::Unexpected(m) => {
            let m = m.to_ascii_lowercase();
            [
                "session closed",
                "not connected",
                "connection reset",
                "connection abort",
                "broken pipe",
                "os error 103",
                "channel closed",
                "unexpected eof",
                "early eof",
            ]
            .iter()
            .any(|marker| m.contains(marker))
        }
        _ => false,
    }
}

impl SftpManager {
    pub fn new(dir: &Path) -> Self {
        Self { sessions: Mutex::new(HashMap::new()), hosts: KnownHosts::load(dir) }
    }

    pub fn hosts(&self) -> &KnownHosts {
        &self.hosts
    }

    fn cached(&self, id: i64) -> Option<Arc<SftpConn>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    pub fn drop_session(&self, id: i64) {
        self.sessions.lock().unwrap().remove(&id);
    }

    /// Range une session déjà ouverte (ex. celle qui vient de valider un mot de passe).
    pub fn insert(&self, id: i64, conn: SftpConn) -> Arc<SftpConn> {
        let conn = Arc::new(conn);
        self.sessions.lock().unwrap().insert(id, conn.clone());
        conn
    }

    /// Ferme toutes les sessions (déconnexion du compte).
    pub fn clear(&self) {
        self.sessions.lock().unwrap().clear();
    }

    pub async fn ensure(&self, api: &ApiClient, token: &str, id: i64) -> Result<Arc<SftpConn>> {
        if let Some(conn) = self.cached(id) {
            return Ok(conn);
        }
        let creds = api.get_sftp_creds(token, id).await?;
        let stored = match crate::secrets::read_sftp_password(id) {
            Ok(stored) => stored,
            // Sans mot de passe de l'API, l'erreur du stockage est la seule explication utile.
            Err(e) if creds.password.is_empty() => return Err(e),
            Err(e) => {
                tracing::warn!(server = id, "lecture du mot de passe SFTP enregistré : {e}");
                None
            }
        };
        let conn = connect(creds, stored, &self.hosts).await?;
        Ok(self.insert(id, conn))
    }
}

/// Mots de passe à essayer, dans l'ordre : celui de l'API (la référence, toujours à jour), puis
/// celui que l'utilisateur a enregistré en secours. Vide = rien à essayer.
fn password_candidates(stored: Option<String>, from_api: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !from_api.is_empty() {
        out.push(from_api.to_string());
    }
    if let Some(stored) = stored.filter(|p| !p.is_empty() && p != from_api) {
        out.push(stored);
    }
    out
}

/// Ouvre une session en essayant chaque mot de passe candidat. Seul un refus d'authentification
/// fait passer au suivant : une panne réseau est remontée telle quelle.
pub async fn connect(creds: SftpCreds, stored: Option<String>, hosts: &KnownHosts) -> Result<SftpConn> {
    let candidates = password_candidates(stored, &creds.password);
    if candidates.is_empty() {
        return Err(Error::SftpPasswordRequired);
    }
    for password in candidates {
        match open(SftpCreds { password, ..creds.clone() }, hosts).await {
            Err(Error::SftpAuth) => continue,
            other => return other,
        }
    }
    Err(Error::SftpAuth)
}

pub(crate) async fn open(creds: SftpCreds, hosts: &KnownHosts) -> Result<SftpConn> {
    // Sonde périodique : sans elle, une session inactive meurt en silence (veille du téléphone,
    // NAT qui oublie la connexion, coupure réseau en arrière-plan) et l'app ne l'apprend qu'à la
    // prochaine opération, qui échoue. `keepalive_max` laisse tomber la session après quatre
    // sondes sans réponse, au lieu de la garder indéfiniment dans le pool.
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(20)),
        keepalive_max: 4,
        ..client::Config::default()
    });
    let host = host_id(&creds);
    let expected = hosts.get(&host).map(|k| k.fingerprint);
    let seen = Arc::new(Mutex::new(None));
    let handler = Handler { expected: expected.clone(), seen: seen.clone() };
    let connected = client::connect(config, (creds.host.as_str(), creds.port), handler).await;
    let presented = seen.lock().unwrap().take();
    let mut handle = match connected {
        Ok(handle) => handle,
        Err(e) => {
            // Refus de notre fait : la clé présentée n'est pas celle mémorisée. Rien n'a été envoyé
            // au serveur (le mot de passe part après la poignée de main).
            if let (Some(expected), Some((found, algorithm))) = (&expected, presented) {
                if !host_key_accepted(Some(expected), &found) {
                    tracing::warn!(%host, "empreinte SFTP différente de celle mémorisée : connexion bloquée");
                    hosts.pending.lock().unwrap().insert(host.clone(), (found, algorithm));
                    return Err(Error::SftpHostKeyChanged { host });
                }
            }
            return Err(Error::Network(format!("SSH: {e}")));
        }
    };

    let auth = handle
        .authenticate_password(&creds.user, &creds.password)
        .await
        .map_err(|e| Error::Network(format!("SSH auth: {e}")))?;
    if !auth.success() {
        return Err(Error::SftpAuth);
    }
    // Premier contact réussi : l'empreinte devient la référence de cet hôte.
    if expected.is_none() {
        if let Some((fingerprint, algorithm)) = presented {
            tracing::info!(%host, %fingerprint, "empreinte SFTP mémorisée (premier contact)");
            hosts.remember(&host, fingerprint, algorithm);
        }
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| Error::Network(format!("canal SSH: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| Error::Network(format!("sous-système SFTP: {e}")))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| Error::Unexpected(format!("session SFTP: {e}")))?;

    Ok(SftpConn {
        sftp,
        _handle: handle,
    })
}

fn sftp_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Unexpected(format!("SFTP: {e}"))
}

/// Réduit un nom de fichier à un composant simple, sûr à concaténer à un dossier distant.
///
/// Le nom vient du système hôte : sur Android, c'est `OpenableColumns.DISPLAY_NAME`, une valeur
/// que l'app ne contrôle pas. Interpolé tel quel, un `../../authorized_keys` écrirait **hors du
/// dossier consulté**. On ne garde donc que le dernier segment, et on refuse ce qui ne peut pas
/// être un nom de fichier.
pub fn safe_file_name(name: &str) -> Result<String> {
    let last = name
        // '\u{5C}' = antislash, écrit ainsi pour rester lisible dans les scripts d'édition.
        .rsplit(['/', '\u{5C}'])
        .next()
        .unwrap_or("")
        .trim();
    if last.is_empty() || last == "." || last == ".." {
        return Err(Error::Unexpected(format!(
            "Nom de fichier invalide : « {name} »."
        )));
    }
    Ok(last.to_string())
}

fn join(dir: &str, name: &str) -> String {
    if dir == "/" || dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

pub async fn list(conn: &SftpConn, path: &str) -> Result<Vec<SftpEntry>> {
    let dir = conn.sftp.read_dir(path).await.map_err(sftp_err)?;
    let mut out = Vec::new();
    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata();
        out.push(SftpEntry {
            path: join(path, &name),
            is_dir: meta.is_dir(),
            size: meta.size.unwrap_or(0),
            modified: meta.mtime.map(|m| m as i64),
            name,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

pub async fn read_text(conn: &SftpConn, path: &str) -> Result<String> {
    let meta = conn.sftp.metadata(path).await.map_err(sftp_err)?;
    if meta.size.unwrap_or(0) > MAX_EDIT_BYTES {
        return Err(Error::Unexpected(
            "Fichier trop volumineux pour l'éditeur.".into(),
        ));
    }
    let mut file = conn.sftp.open(path).await.map_err(sftp_err)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
    String::from_utf8(buf).map_err(|_| Error::Unexpected("Fichier binaire (non éditable).".into()))
}

/// Lit les OCTETS bruts d'un fichier distant, plafonnés à `cap` (garde-fou OOM pour les archives).
pub async fn read_bytes(conn: &SftpConn, path: &str, cap: u64) -> Result<Vec<u8>> {
    let meta = conn.sftp.metadata(path).await.map_err(sftp_err)?;
    if meta.size.unwrap_or(0) > cap {
        return Err(Error::Unexpected(
            "Fichier trop volumineux à ouvrir ici — télécharge-le.".into(),
        ));
    }
    let mut file = conn.sftp.open(path).await.map_err(sftp_err)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
    Ok(buf)
}

/// Parcourt récursivement un dossier distant et renvoie tous ses FICHIERS `(chemin complet, taille)`.
/// Sert au téléchargement d'un dossier en `.zip` (les dossiers vides ne sont pas listés — le zip les
/// recrée implicitement via les chemins des fichiers).
pub async fn walk(conn: &SftpConn, dir: &str) -> Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_string()];
    while let Some(d) = stack.pop() {
        for entry in conn.sftp.read_dir(&d).await.map_err(sftp_err)? {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let full = join(&d, &name);
            let meta = entry.metadata();
            if meta.is_dir() {
                stack.push(full);
            } else {
                files.push((full, meta.size.unwrap_or(0)));
            }
        }
    }
    Ok(files)
}

/// `(est_un_dossier, taille)` d'un chemin distant.
pub async fn stat(conn: &SftpConn, path: &str) -> Result<(bool, u64)> {
    let m = conn.sftp.metadata(path).await.map_err(sftp_err)?;
    Ok((m.is_dir(), m.size.unwrap_or(0)))
}

/// Recherche RÉCURSIVE des entrées dont le nom contient `query` (insensible à la casse) sous `root`.
/// Bornée : `max_results` résultats et `max_scan` entrées explorées → renvoie `(hits, tronqué)`. Un
/// dossier illisible (permissions) est ignoré, pas fatal.
pub async fn search(
    conn: &SftpConn,
    root: &str,
    query: &str,
    max_results: usize,
    max_scan: usize,
) -> Result<(Vec<SftpEntry>, bool)> {
    let q = query.to_lowercase();
    let mut out: Vec<SftpEntry> = Vec::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    let mut stack = vec![root.to_string()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = conn.sftp.read_dir(&dir).await else {
            continue; // dossier illisible → on saute
        };
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            scanned += 1;
            if scanned > max_scan {
                truncated = true;
                break;
            }
            let meta = entry.metadata();
            let is_dir = meta.is_dir();
            let full = join(&dir, &name);
            if is_dir {
                stack.push(full.clone());
            }
            if name.to_lowercase().contains(&q) {
                out.push(SftpEntry {
                    path: full,
                    is_dir,
                    size: meta.size.unwrap_or(0),
                    modified: meta.mtime.map(|m| m as i64),
                    name,
                });
                if out.len() >= max_results {
                    truncated = true;
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }
    Ok((out, truncated))
}

pub async fn write_text(conn: &SftpConn, path: &str, content: &str) -> Result<()> {
    let mut file = conn.sftp.create(path).await.map_err(sftp_err)?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| Error::Unexpected(format!("écriture SFTP: {e}")))?;
    file.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush SFTP: {e}")))?;
    let _ = file.shutdown().await;
    Ok(())
}

/// Taille de bloc pour les transferts streamés (progression + mémoire bornée).
const CHUNK: usize = 64 * 1024;

/// Téléverse un fichier local par blocs (pas de chargement intégral en mémoire) ; `on_progress`
/// est appelé après chaque bloc avec `(octets_transférés, total)`.
pub async fn upload(
    conn: &SftpConn,
    local: &str,
    remote_dir: &str,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<String> {
    let total = tokio::fs::metadata(local).await.map(|m| m.len()).unwrap_or(0);
    let name = std::path::Path::new(local)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let remote = join(remote_dir, &name);
    let mut src = tokio::fs::File::open(local)
        .await
        .map_err(|e| Error::Unexpected(format!("lecture locale: {e}")))?;
    let mut dst = conn.sftp.create(&remote).await.map_err(sftp_err)?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| Error::Unexpected(format!("lecture locale: {e}")))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])
            .await
            .map_err(|e| Error::Unexpected(format!("écriture SFTP: {e}")))?;
        done += n as u64;
        on_progress(done, total);
    }
    dst.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush SFTP: {e}")))?;
    let _ = dst.shutdown().await;
    Ok(name)
}

/// Téléverse depuis un **fichier déjà ouvert**, en pouvant **reprendre** à `start`.
///
/// `start = 0` crée (et tronque) le fichier distant ; `start > 0` reprend un envoi interrompu en
/// écrivant à la suite. Le flux local est repositionné en conséquence, de sorte que ce qui a déjà
/// atteint le serveur n'est ni renvoyé ni dupliqué.
///
/// Renvoie le nombre total d'octets du fichier une fois l'envoi terminé (`start` inclus).
pub async fn upload_stream_from(
    conn: &SftpConn,
    src: &mut tokio::fs::File,
    remote: &str,
    start: u64,
    total: u64,
    on_progress: &mut impl FnMut(u64, u64),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<u64> {
    use tokio::io::AsyncSeekExt;

    // Le flux local doit repartir exactement là où le serveur s'est arrêté.
    src.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| Error::Unexpected(format!("positionnement local: {e}")))?;

    let mut dst = if start == 0 {
        conn.sftp.create(remote).await.map_err(sftp_err)?
    } else {
        conn.sftp
            .open_with_flags(
                remote,
                russh_sftp::protocol::OpenFlags::CREATE
                    | russh_sftp::protocol::OpenFlags::WRITE
                    | russh_sftp::protocol::OpenFlags::APPEND,
            )
            .await
            .map_err(sftp_err)?
    };

    let mut buf = vec![0u8; CHUNK];
    let mut done = start;
    loop {
        // Arrêt coopératif : vérifié entre deux blocs, jamais au milieu d'une écriture — le
        // fichier distant reste ainsi cohérent avec ce que le serveur a réellement reçu.
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = dst.shutdown().await;
            return Err(Error::Unexpected("Transfert annulé.".into()));
        }
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| Error::Unexpected(format!("lecture locale: {e}")))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])
            .await
            .map_err(|e| Error::Unexpected(format!("écriture SFTP: {e}")))?;
        done += n as u64;
        on_progress(done, total);
    }
    dst.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush SFTP: {e}")))?;
    let _ = dst.shutdown().await;
    Ok(done)
}

/// Télécharge un fichier distant vers un chemin local par blocs ; `on_progress(done, total)`.
pub async fn download(
    conn: &SftpConn,
    remote: &str,
    local: &str,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<()> {
    let total = conn.sftp.metadata(remote).await.map_err(sftp_err)?.size.unwrap_or(0);
    let mut src = conn.sftp.open(remote).await.map_err(sftp_err)?;
    let mut dst = tokio::fs::File::create(local)
        .await
        .map_err(|e| Error::Unexpected(format!("écriture locale: {e}")))?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])
            .await
            .map_err(|e| Error::Unexpected(format!("écriture locale: {e}")))?;
        done += n as u64;
        on_progress(done, total);
    }
    dst.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush locale: {e}")))?;
    Ok(())
}

/// Télécharge `remote` dans un fichier local déjà ouvert **à partir de `start`** (reprise après
/// une coupure), par blocs ; `on_progress(done, total)`. Le fichier local est repositionné (et
/// tronqué) à `start` : ce qui a déjà été reçu n'est ni redemandé ni dupliqué. Arrêt coopératif
/// entre deux blocs via `cancel`. Renvoie le nombre d'octets du fichier une fois fini.
pub async fn download_from(
    conn: &SftpConn,
    remote: &str,
    dst: &mut tokio::fs::File,
    start: u64,
    on_progress: &mut impl FnMut(u64, u64),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<u64> {
    use tokio::io::AsyncSeekExt;
    let local_err = |e: std::io::Error| Error::Unexpected(format!("écriture locale: {e}"));
    let total = conn.sftp.metadata(remote).await.map_err(sftp_err)?.size.unwrap_or(0);
    let mut src = conn.sftp.open(remote).await.map_err(sftp_err)?;
    if start > 0 {
        src.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| Error::Unexpected(format!("positionnement SFTP: {e}")))?;
    }
    dst.set_len(start).await.map_err(local_err)?;
    dst.seek(std::io::SeekFrom::Start(start)).await.map_err(local_err)?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = start;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Unexpected("Transfert annulé.".into()));
        }
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(local_err)?;
        done += n as u64;
        on_progress(done, total);
    }
    dst.flush().await.map_err(local_err)?;
    Ok(done)
}

pub async fn mkdir(conn: &SftpConn, path: &str) -> Result<()> {
    conn.sftp.create_dir(path).await.map_err(sftp_err)
}

/// Crée un dossier distant ET tous ses parents (`mkdir -p`) ; ignore « existe déjà ».
pub async fn ensure_dir(conn: &SftpConn, path: &str) {
    let mut acc = String::new();
    for part in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        let _ = conn.sftp.create_dir(&acc).await; // erreur ignorée si le dossier existe déjà
    }
}

/// Écrit des octets bruts dans un fichier distant (crée / tronque).
pub async fn write_bytes(conn: &SftpConn, path: &str, data: &[u8]) -> Result<()> {
    let mut file = conn.sftp.create(path).await.map_err(sftp_err)?;
    file.write_all(data)
        .await
        .map_err(|e| Error::Unexpected(format!("écriture SFTP: {e}")))?;
    file.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush SFTP: {e}")))?;
    let _ = file.shutdown().await;
    Ok(())
}


/// Taille d'un fichier distant, sans le lire.
pub async fn file_size(conn: &SftpConn, path: &str) -> Result<u64> {
    let meta = conn.sftp.metadata(path).await.map_err(sftp_err)?;
    Ok(meta.size.unwrap_or(0))
}

/// Lit une **tranche** d'un fichier distant (`offset`, `len`), pour un téléchargement progressif.
pub async fn read_range(conn: &SftpConn, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
    use tokio::io::AsyncSeekExt;
    let mut file = conn.sftp.open(path).await.map_err(sftp_err)?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| Error::Unexpected(format!("positionnement SFTP: {e}")))?;
    let mut buf = vec![0u8; len];
    let mut read = 0usize;
    // `read` peut rendre moins que demandé : on boucle jusqu'au bloc plein ou la fin du fichier.
    while read < len {
        let n = file
            .read(&mut buf[read..])
            .await
            .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
        if n == 0 {
            break;
        }
        read += n;
    }
    buf.truncate(read);
    Ok(buf)
}

pub async fn remove(conn: &SftpConn, path: &str, is_dir: bool) -> Result<()> {
    if is_dir {
        conn.sftp.remove_dir(path).await.map_err(sftp_err)
    } else {
        conn.sftp.remove_file(path).await.map_err(sftp_err)
    }
}

pub async fn rename(conn: &SftpConn, from: &str, to: &str) -> Result<()> {
    conn.sftp.rename(from, to).await.map_err(sftp_err)
}

/// Supprime un dossier distant et TOUT son contenu (l'API `remove_dir` exige un dossier vide).
/// Récursif : vide les enfants (fichiers + sous-dossiers) avant de retirer le dossier lui-même.
pub async fn remove_recursive(conn: &SftpConn, dir: &str) -> Result<()> {
    let entries = list(conn, dir).await?;
    for e in entries {
        if e.is_dir {
            Box::pin(remove_recursive(conn, &e.path)).await?;
        } else {
            conn.sftp.remove_file(&e.path).await.map_err(sftp_err)?;
        }
    }
    conn.sftp.remove_dir(dir).await.map_err(sftp_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(password: &str) -> SftpCreds {
        SftpCreds {
            host: "sftp.invalid".into(),
            port: 2022,
            user: "joueur.abc12345".into(),
            password: password.into(),
        }
    }

    #[test]
    fn api_password_comes_first_then_the_saved_one() {
        assert_eq!(password_candidates(Some("perso".into()), "api"), ["api", "perso"]);
        // Identiques : un seul essai, pas deux poignées de main pour le même refus.
        assert_eq!(password_candidates(Some("même".into()), "même"), ["même"]);
        assert_eq!(password_candidates(None, "api"), ["api"]);
        assert_eq!(password_candidates(Some("perso".into()), ""), ["perso"]);
        assert!(password_candidates(Some(String::new()), "").is_empty());
        assert!(password_candidates(None, "").is_empty());
    }

    #[tokio::test]
    async fn no_password_anywhere_asks_the_user_without_connecting() {
        // L'hôte n'existe pas : si une connexion était tentée, l'erreur serait réseau.
        let dir = std::env::temp_dir().join(format!("minestrator-test-kh-none-{}", std::process::id()));
        let err = connect(creds(""), None, &KnownHosts::load(&dir)).await.err().expect("doit échouer");
        assert!(matches!(err, Error::SftpPasswordRequired), "{err:?}");
    }

    #[test]
    fn host_key_is_trusted_on_first_use_then_pinned() {
        assert!(host_key_accepted(None, "SHA256:abc"), "premier contact");
        assert!(host_key_accepted(Some("SHA256:abc"), "SHA256:abc"));
        assert!(!host_key_accepted(Some("SHA256:abc"), "SHA256:xyz"), "empreinte changée");
    }

    #[test]
    fn only_the_fingerprint_actually_presented_can_be_trusted() {
        let dir = std::env::temp_dir().join(format!("minestrator-test-kh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hosts = KnownHosts::load(&dir);
        let host = "node1.exemple:2022";
        hosts.remember(host, "SHA256:ancienne".into(), "ssh-ed25519".into());
        // Sans refus préalable, rien à accepter.
        assert!(hosts.trust_pending(host, "SHA256:nouvelle").is_err());
        hosts
            .pending
            .lock()
            .unwrap()
            .insert(host.into(), ("SHA256:nouvelle".into(), "ssh-ed25519".into()));
        // Une autre valeur que celle présentée est refusée, et la demande reste en attente.
        assert!(hosts.trust_pending(host, "SHA256:inventee").is_err());
        assert!(hosts.pending(host).is_some());
        hosts.trust_pending(host, "SHA256:nouvelle").unwrap();
        assert_eq!(hosts.get(host).unwrap().fingerprint, "SHA256:nouvelle");
        assert!(hosts.pending(host).is_none());
        // Persisté : relu tel quel par une nouvelle instance.
        assert_eq!(KnownHosts::load(&dir).get(host).unwrap().fingerprint, "SHA256:nouvelle");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn debug_never_prints_the_password() {
        let shown = format!("{:?}", creds("motdepasse-secret"));
        assert!(!shown.contains("motdepasse-secret"), "{shown}");
        assert!(shown.contains("<masqué>"));
        assert!(format!("{:?}", creds("")).contains("<absent>"));
    }
}
