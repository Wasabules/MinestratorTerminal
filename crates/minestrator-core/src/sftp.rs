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
    let config = Arc::new(client::Config::default());
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

pub async fn upload(conn: &SftpConn, local: &str, remote_dir: &str) -> Result<String> {
    let data = tokio::fs::read(local)
        .await
        .map_err(|e| Error::Unexpected(format!("lecture locale: {e}")))?;
    let name = std::path::Path::new(local)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("fichier")
        .to_string();
    let remote = join(remote_dir, &name);
    let mut file = conn.sftp.create(&remote).await.map_err(sftp_err)?;
    file.write_all(&data)
        .await
        .map_err(|e| Error::Unexpected(format!("écriture SFTP: {e}")))?;
    file.flush()
        .await
        .map_err(|e| Error::Unexpected(format!("flush SFTP: {e}")))?;
    let _ = file.shutdown().await;
    Ok(name)
}

pub async fn download(conn: &SftpConn, remote: &str, local: &str) -> Result<()> {
    let mut file = conn.sftp.open(remote).await.map_err(sftp_err)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| Error::Unexpected(format!("lecture SFTP: {e}")))?;
    tokio::fs::write(local, buf)
        .await
        .map_err(|e| Error::Unexpected(format!("écriture locale: {e}")))?;
    Ok(())
}

pub async fn mkdir(conn: &SftpConn, path: &str) -> Result<()> {
    conn.sftp.create_dir(path).await.map_err(sftp_err)
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
