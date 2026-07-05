//! Validation et réparation minimale des fichiers de région Minecraft (`.mca`, format Anvil).
//!
//! Structure : en-tête de 8 KiB = 1024 entrées de **localisation** (4 o : offset sur 3 o + nombre
//! de secteurs sur 1 o, en secteurs de 4 KiB depuis le début du fichier) puis 1024 **timestamps**
//! (4 o). Viennent ensuite les données de chunk. On valide la **table de localisation** (pointeurs
//! dans les bornes, longueurs cohérentes) SANS décompresser — suffisant pour repérer la corruption
//! structurelle, cause n°1 des « Exception ticking world » / crash au chargement d'un chunk.
//!
//! La réparation « clear » remet à zéro l'entrée d'un chunk corrompu → le jeu le RÉGÉNÈRE au
//! prochain chargement (perte limitée à ce chunk ; le reste de la région est préservé). C'est la
//! même approche que MCA Selector.

const SECTOR: usize = 4096;
const HEADER: usize = SECTOR * 2; // 8 KiB : locations (4 KiB) + timestamps (4 KiB)

/// Bilan de validation d'un fichier de région.
pub(crate) struct RegionReport {
    pub file_len: usize,
    /// Nombre de chunks générés (entrée de localisation non vide).
    pub total: usize,
    pub corrupt: Vec<CorruptChunk>,
}

pub(crate) struct CorruptChunk {
    /// Index 0..1024 dans la table ; `usize::MAX` = corruption GLOBALE du fichier (en-tête illisible).
    pub index: usize,
    pub local_x: usize, // 0..32
    pub local_z: usize, // 0..32
    pub reason: &'static str,
}

/// Parse l'en-tête et repère les entrées de localisation incohérentes.
pub(crate) fn validate(bytes: &[u8]) -> RegionReport {
    let len = bytes.len();
    // En-tête tronqué ou taille non alignée sur un secteur → région entière suspecte.
    if len < HEADER || !len.is_multiple_of(SECTOR) {
        return RegionReport {
            file_len: len,
            total: 0,
            corrupt: vec![CorruptChunk {
                index: usize::MAX,
                local_x: 0,
                local_z: 0,
                reason: "taille de fichier invalide (en-tête tronqué ou non aligné sur 4 KiB)",
            }],
        };
    }

    let sectors = len / SECTOR;
    let mut corrupt = Vec::new();
    let mut total = 0;

    for i in 0..1024 {
        let e = i * 4;
        // Offset sur 3 octets grand-boutiste (secteur), nombre de secteurs sur 1 octet.
        let offset = u32::from_be_bytes([0, bytes[e], bytes[e + 1], bytes[e + 2]]) as usize;
        let count = bytes[e + 3] as usize;
        if offset == 0 && count == 0 {
            continue; // chunk non généré (normal)
        }
        total += 1;

        let reason = if offset < 2 {
            Some("pointeur situé dans l'en-tête")
        } else if count == 0 {
            Some("nombre de secteurs nul")
        } else if offset + count > sectors {
            Some("pointeur au-delà de la fin du fichier")
        } else {
            // Longueur de chunk déclarée en tête de secteur (grand-boutiste).
            let s = offset * SECTOR;
            let declared =
                u32::from_be_bytes([bytes[s], bytes[s + 1], bytes[s + 2], bytes[s + 3]]) as usize;
            if declared == 0 || declared > count * SECTOR {
                Some("longueur de chunk invalide")
            } else {
                None
            }
        };

        if let Some(reason) = reason {
            corrupt.push(CorruptChunk { index: i, local_x: i % 32, local_z: i / 32, reason });
        }
    }

    RegionReport { file_len: len, total, corrupt }
}

/// Remet à zéro l'entrée de localisation + le timestamp de chaque chunk corrompu → régénération au
/// prochain chargement. Ignore une corruption GLOBALE (index `usize::MAX`), non réparable ainsi.
/// Renvoie le nombre de chunks effacés.
pub(crate) fn clear_corrupt(bytes: &mut [u8], report: &RegionReport) -> usize {
    if bytes.len() < HEADER {
        return 0;
    }
    let mut n = 0;
    for c in &report.corrupt {
        if c.index == usize::MAX || c.index >= 1024 {
            continue;
        }
        let loc = c.index * 4;
        bytes[loc..loc + 4].fill(0);
        let ts = SECTOR + c.index * 4;
        bytes[ts..ts + 4].fill(0);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_entry(buf: &mut [u8], idx: usize, offset: u32, count: u8) {
        let e = idx * 4;
        buf[e] = (offset >> 16) as u8;
        buf[e + 1] = (offset >> 8) as u8;
        buf[e + 2] = offset as u8;
        buf[e + 3] = count;
    }
    fn write_len(buf: &mut [u8], sector: usize, len: u32) {
        let s = sector * SECTOR;
        buf[s] = (len >> 24) as u8;
        buf[s + 1] = (len >> 16) as u8;
        buf[s + 2] = (len >> 8) as u8;
        buf[s + 3] = len as u8;
        buf[s + 4] = 2; // type de compression (zlib)
    }

    #[test]
    fn valid_region_has_no_corruption() {
        let mut buf = vec![0u8; SECTOR * 3];
        set_entry(&mut buf, 0, 2, 1); // chunk 0 → secteur 2, 1 secteur
        write_len(&mut buf, 2, 200);
        let r = validate(&buf);
        assert_eq!(r.total, 1);
        assert!(r.corrupt.is_empty());
    }

    #[test]
    fn flags_out_of_bounds_and_bad_length() {
        let mut buf = vec![0u8; SECTOR * 3];
        set_entry(&mut buf, 1, 50, 1); // offset au-delà de la fin → corrompu
        set_entry(&mut buf, 2, 2, 1); // valide en pointeur mais longueur nulle → corrompu
        let r = validate(&buf);
        assert_eq!(r.corrupt.len(), 2);
        assert!(r.corrupt.iter().any(|c| c.index == 1));
        assert!(r.corrupt.iter().any(|c| c.index == 2));
    }

    #[test]
    fn truncated_file_is_global_corrupt() {
        let r = validate(&vec![0u8; 100]);
        assert!(r.corrupt.iter().any(|c| c.index == usize::MAX));
        assert_eq!(clear_corrupt(&mut vec![0u8; 100], &r), 0);
    }

    #[test]
    fn clear_corrupt_zeroes_the_entry() {
        let mut buf = vec![0u8; SECTOR * 3];
        set_entry(&mut buf, 5, 99, 1); // corrompu
        let r = validate(&buf);
        let n = clear_corrupt(&mut buf, &r);
        assert_eq!(n, 1);
        assert!(buf[5 * 4..5 * 4 + 4].iter().all(|&b| b == 0));
    }
}
