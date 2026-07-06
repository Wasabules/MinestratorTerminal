//! Boîte à outils « maps corrompues » : inspection et réparation des fichiers de région `.mca`
//! d'un serveur, via SFTP. Le parsing/validation pur vit dans [`crate::mca`] (testé) ; ici on gère
//! les échanges SFTP (télécharge/renvoie le fichier) et la mise en forme lisible.
//!
//! `inspect_region` est en LECTURE SEULE (sûr). `repair_region` est DESTRUCTIF (écrase / supprime
//! des chunks) → exposé uniquement en action « danger » à confirmer, snapshot recommandé d'abord.

use crate::Core;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Inspecte un fichier de région : télécharge, valide sa structure, renvoie un rapport lisible.
pub(crate) async fn inspect_region(core: &Core, id: i64, path: &str) -> Result<String, String> {
    let bytes = download(core, id, path).await?;
    Ok(format_report(path, &crate::mca::validate(&bytes)))
}

/// Répare un fichier de région. `mode` :
/// - `clear_corrupt` (défaut) : efface les entrées des chunks corrompus → régénération (perte
///   limitée à ces chunks, le reste de la région est préservé) ;
/// - `delete` : supprime tout le fichier → la zone 512×512 est régénérée.
pub(crate) async fn repair_region(
    core: &Core,
    id: i64,
    path: &str,
    mode: &str,
) -> Result<String, String> {
    if mode == "delete" {
        core.sftp_delete(id, path, false).await.map_err(|e| e.to_string())?;
        return Ok(format!(
            "Région {path} supprimée — la zone (jusqu'à 512×512 blocs) sera régénérée au prochain démarrage."
        ));
    }

    let mut bytes = download(core, id, path).await?;
    let report = crate::mca::validate(&bytes);
    if report.corrupt.iter().any(|c| c.index == usize::MAX) {
        return Err(
            "En-tête de région irréparable (fichier tronqué). Crée un snapshot puis utilise mode=delete pour régénérer toute la région.".into(),
        );
    }
    let n = crate::mca::clear_corrupt(&mut bytes, &report);
    if n == 0 {
        return Ok(format!("Aucun chunk corrompu à réparer dans {path}."));
    }
    upload(core, id, path, &bytes).await?;
    Ok(format!(
        "{n} chunk(s) corrompu(s) effacé(s) dans {path} — ils seront régénérés au prochain démarrage ; le reste de la région est préservé."
    ))
}

// --- SFTP (télécharge/renvoie via un fichier local temporaire au nom d'origine) ---------------

async fn download(core: &Core, id: i64, path: &str) -> Result<Vec<u8>, String> {
    let (_, name) = split_path(path)?;
    let tmp = temp_path(&name)?;
    core.sftp_download_file(id, path, &tmp.to_string_lossy())
        .await
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("lecture locale : {e}"));
    let _ = std::fs::remove_file(&tmp);
    bytes
}

async fn upload(core: &Core, id: i64, path: &str, bytes: &[u8]) -> Result<(), String> {
    let (dir, name) = split_path(path)?;
    let tmp = temp_path(&name)?; // le nom local DOIT être celui de la région (l'upload le conserve)
    std::fs::write(&tmp, bytes).map_err(|e| format!("écriture locale : {e}"))?;
    let res = core
        .sftp_upload_file(id, &tmp.to_string_lossy(), &dir)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Découpe `/world/region/r.0.0.mca` en (`/world/region`, `r.0.0.mca`).
fn split_path(path: &str) -> Result<(String, String), String> {
    let p = path.trim();
    let idx = p.rfind('/').ok_or("chemin de région invalide (absolu attendu)")?;
    let name = &p[idx + 1..];
    if name.is_empty() {
        return Err("chemin de région invalide".into());
    }
    Ok((p[..idx].to_string(), name.to_string()))
}

/// Chemin local temporaire UNIQUE dont le nom de fichier est exactement `name`.
fn temp_path(name: &str) -> Result<PathBuf, String> {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mst-mca-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).map_err(|e| format!("dossier temporaire : {e}"))?;
    Ok(dir.join(name))
}

// --- Mise en forme ---------------------------------------------------------

fn format_report(path: &str, r: &crate::mca::RegionReport) -> String {
    let (rx, rz) = region_coords(path);
    let mut out = format!(
        "Région {path} — {} o, {} chunk(s) généré(s).\n",
        r.file_len, r.total
    );
    if r.corrupt.is_empty() {
        out.push_str("✅ Aucune corruption structurelle détectée.");
        return out;
    }
    let _ = writeln!(out, "⚠ {} chunk(s) corrompu(s) :", r.corrupt.len());
    for c in &r.corrupt {
        if c.index == usize::MAX {
            let _ = writeln!(out, "  • FICHIER : {}", c.reason);
        } else if let (Some(rx), Some(rz)) = (rx, rz) {
            let (gx, gz) = (rx * 32 + c.local_x as i64, rz * 32 + c.local_z as i64);
            let _ = writeln!(out, "  • chunk ({gx}, {gz}) : {}", c.reason);
        } else {
            let _ = writeln!(out, "  • chunk local ({}, {}) : {}", c.local_x, c.local_z, c.reason);
        }
    }
    out.push_str(
        "Fix : crée un SNAPSHOT, puis propose repair_region — mode `clear_corrupt` (efface ces chunks pour régénération, perte minimale) ou `delete` (toute la région).",
    );
    out
}

/// Extrait (rx, rz) du nom `r.rx.rz.mca`.
pub(crate) fn region_coords(path: &str) -> (Option<i64>, Option<i64>) {
    let name = path.rsplit('/').next().unwrap_or("");
    let stem = name.strip_suffix(".mca").unwrap_or(name);
    let parts: Vec<&str> = stem.split('.').collect();
    if parts.len() == 3 && parts[0] == "r" {
        (parts[1].parse().ok(), parts[2].parse().ok())
    } else {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_parses_region_paths() {
        assert_eq!(
            split_path("/world/region/r.0.-1.mca").unwrap(),
            ("/world/region".to_string(), "r.0.-1.mca".to_string())
        );
        assert_eq!(region_coords("/world/region/r.3.-2.mca"), (Some(3), Some(-2)));
        assert_eq!(region_coords("/world/region/entities.dat"), (None, None));
    }
}
