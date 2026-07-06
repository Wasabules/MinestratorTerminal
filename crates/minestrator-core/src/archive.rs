//! Lecture d'archives **en mémoire** (octets bruts lus par SFTP) : `.zip`, `.tar`, `.tar.gz`/`.tgz`,
//! et décompression d'un `.gz` seul. **Lecture seule** : lister les entrées, extraire une entrée,
//! décompresser un gzip — pour VOIR/extraire, jamais réécrire l'archive.
//!
//! Tout est synchrone et pur (sur `&[u8]`) → testable sans réseau. Le côté SFTP (lecture des octets)
//! vit dans `sftp.rs` / `lib.rs`.

use flate2::read::GzDecoder;
use serde::Serialize;
use std::io::{Cursor, Read};

/// Formats d'archive à ENTRÉES MULTIPLES (parcourus comme un dossier). Le `.gz` seul n'en fait pas
/// partie : c'est un fichier unique compressé, traité par [`gunzip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

/// Une entrée d'archive (pour l'UI, façon [`crate::models::SftpEntry`] mais interne à l'archive).
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Type d'archive multi-entrées d'après le nom, ou `None` si ce n'en est pas une.
pub fn kind_from_name(name: &str) -> Option<ArchiveKind> {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if n.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

/// Liste les entrées d'une archive multi-entrées.
pub fn list(bytes: &[u8], kind: ArchiveKind) -> Result<Vec<ArchiveEntry>, String> {
    match kind {
        ArchiveKind::Zip => list_zip(bytes),
        ArchiveKind::Tar => tar_list(tar::Archive::new(Cursor::new(bytes))),
        ArchiveKind::TarGz => tar_list(tar::Archive::new(GzDecoder::new(Cursor::new(bytes)))),
    }
}

/// Extrait les octets d'UNE entrée (par son nom exact tel que renvoyé par [`list`]).
pub fn extract(bytes: &[u8], kind: ArchiveKind, entry: &str) -> Result<Vec<u8>, String> {
    match kind {
        ArchiveKind::Zip => extract_zip(bytes, entry),
        ArchiveKind::Tar => tar_extract(tar::Archive::new(Cursor::new(bytes)), entry),
        ArchiveKind::TarGz => {
            tar_extract(tar::Archive::new(GzDecoder::new(Cursor::new(bytes))), entry)
        }
    }
}

/// Décompresse un `.gz` seul → octets d'origine.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut buf)
        .map_err(|e| format!("décompression gzip : {e}"))?;
    Ok(buf)
}

fn list_zip(bytes: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let mut ar =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip illisible : {e}"))?;
    let mut out = Vec::with_capacity(ar.len());
    for i in 0..ar.len() {
        let f = ar.by_index(i).map_err(|e| format!("zip (entrée {i}) : {e}"))?;
        out.push(ArchiveEntry {
            name: f.name().to_string(),
            size: f.size(),
            is_dir: f.is_dir(),
        });
    }
    Ok(out)
}

fn extract_zip(bytes: &[u8], entry: &str) -> Result<Vec<u8>, String> {
    let mut ar =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip illisible : {e}"))?;
    let mut f = ar
        .by_name(entry)
        .map_err(|_| format!("entrée « {entry} » introuvable dans l'archive"))?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).map_err(|e| format!("extraction : {e}"))?;
    Ok(buf)
}

fn tar_list<R: Read>(mut ar: tar::Archive<R>) -> Result<Vec<ArchiveEntry>, String> {
    let mut out = Vec::new();
    for e in ar.entries().map_err(|e| format!("tar illisible : {e}"))? {
        let e = e.map_err(|e| format!("tar (entrée) : {e}"))?;
        let name = e
            .path()
            .map_err(|e| format!("tar (chemin) : {e}"))?
            .to_string_lossy()
            .into_owned();
        out.push(ArchiveEntry {
            name,
            size: e.size(),
            is_dir: e.header().entry_type().is_dir(),
        });
    }
    Ok(out)
}

fn tar_extract<R: Read>(mut ar: tar::Archive<R>, entry: &str) -> Result<Vec<u8>, String> {
    for e in ar.entries().map_err(|e| format!("tar illisible : {e}"))? {
        let mut e = e.map_err(|e| format!("tar (entrée) : {e}"))?;
        let name = e.path().map_err(|e| format!("tar (chemin) : {e}"))?.to_string_lossy().into_owned();
        if name == entry {
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf).map_err(|e| format!("extraction : {e}"))?;
            return Ok(buf);
        }
    }
    Err(format!("entrée « {entry} » introuvable dans l'archive"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_kinds() {
        assert_eq!(kind_from_name("world.zip"), Some(ArchiveKind::Zip));
        assert_eq!(kind_from_name("backup.TAR.GZ"), Some(ArchiveKind::TarGz));
        assert_eq!(kind_from_name("x.tgz"), Some(ArchiveKind::TarGz));
        assert_eq!(kind_from_name("logs.tar"), Some(ArchiveKind::Tar));
        assert_eq!(kind_from_name("latest.log.gz"), None); // .gz seul → pas une archive multi-entrées
        assert_eq!(kind_from_name("server.properties"), None);
    }

    #[test]
    fn zip_roundtrip() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("a.txt", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"hello zip").unwrap();
        let bytes = w.finish().unwrap().into_inner();

        let entries = list(&bytes, ArchiveKind::Zip).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.txt");
        assert_eq!(extract(&bytes, ArchiveKind::Zip, "a.txt").unwrap(), b"hello zip");
        assert!(extract(&bytes, ArchiveKind::Zip, "absent").is_err());
    }

    #[test]
    fn tar_roundtrip() {
        let mut b = tar::Builder::new(Vec::new());
        let data = b"hello tar";
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "dir/b.txt", &data[..]).unwrap();
        let bytes = b.into_inner().unwrap();

        let entries = list(&bytes, ArchiveKind::Tar).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "dir/b.txt");
        assert_eq!(extract(&bytes, ArchiveKind::Tar, "dir/b.txt").unwrap(), b"hello tar");
    }

    #[test]
    fn gzip_roundtrip() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"log content").unwrap();
        let bytes = enc.finish().unwrap();
        assert_eq!(gunzip(&bytes).unwrap(), b"log content");
    }
}
