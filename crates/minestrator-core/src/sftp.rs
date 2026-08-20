//! Client SFTP natif (SSH via `russh` + `russh-sftp`). Session poolée par serveur.
//! Les identifiants viennent de `GET /server/{id}` et restent dans le cœur.

use crate::api::ApiClient;
use crate::error::{Error, Result};
use crate::models::{SftpCreds, SftpEntry};
use russh::client;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_EDIT_BYTES: u64 = 2 * 1024 * 1024;

struct Handler;

#[async_trait::async_trait]
impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct SftpConn {
    sftp: SftpSession,
    _handle: client::Handle<Handler>,
}

#[derive(Default)]
pub struct SftpManager {
    sessions: Mutex<HashMap<i64, Arc<SftpConn>>>,
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
    fn cached(&self, id: i64) -> Option<Arc<SftpConn>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    pub fn drop_session(&self, id: i64) {
        self.sessions.lock().unwrap().remove(&id);
    }

    pub async fn ensure(&self, api: &ApiClient, token: &str, id: i64) -> Result<Arc<SftpConn>> {
        if let Some(conn) = self.cached(id) {
            return Ok(conn);
        }
        let creds = api.get_sftp_creds(token, id).await?;
        let conn = Arc::new(open(creds).await?);
        self.sessions.lock().unwrap().insert(id, conn.clone());
        Ok(conn)
    }
}

async fn open(creds: SftpCreds) -> Result<SftpConn> {
    // Sonde périodique : sans elle, une session inactive meurt en silence (veille du téléphone,
    // NAT qui oublie la connexion, coupure réseau en arrière-plan) et l'app ne l'apprend qu'à la
    // prochaine opération, qui échoue. `keepalive_max` laisse tomber la session après quatre
    // sondes sans réponse, au lieu de la garder indéfiniment dans le pool.
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(20)),
        keepalive_max: 4,
        ..client::Config::default()
    });
    let mut handle = client::connect(config, (creds.host.as_str(), creds.port), Handler)
        .await
        .map_err(|e| Error::Network(format!("SSH: {e}")))?;

    let ok = handle
        .authenticate_password(&creds.user, &creds.password)
        .await
        .map_err(|e| Error::Network(format!("SSH auth: {e}")))?;
    if !ok {
        return Err(Error::Unauthorized);
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
