//! Métadonnées des `.jar` de plugins et de mods, lues **à la demande** : la fin du fichier (fin du
//! répertoire central zip), le répertoire central, puis seulement les quelques entrées utiles
//! (descripteurs) — jamais le jar entier. `archive.rs`, lui, charge l'archive complète en mémoire
//! (plafond 64 Mio) pour la parcourir : pour un modpack de 300 mods, ce serait des Gio à
//! télécharger, contre quelques dizaines de Mio ici.
//!
//! Tout est **pur** : [`ZipProbe`] indique la prochaine tranche à lire et décode ce qu'on lui
//! fournit ; l'appelant (SFTP, `plugin_deps.rs`) fait les lectures. Puis [`read_meta`] interprète
//! les descripteurs : `plugin.yml` (Bukkit/Spigot/Paper), `paper-plugin.yml`, `bungee.yml`,
//! `velocity-plugin.json`, `fabric.mod.json`, `quilt.mod.json`, `META-INF/mods.toml` (Forge, et
//! NeoForge jusqu'à 1.20.4), `META-INF/neoforge.mods.toml`, `mcmod.info` (Forge ≤ 1.12).
//!
//! On y trouve aussi la comparaison des versions et des contraintes (prédicats Fabric/Quilt,
//! intervalles Maven de Forge), utilisée par les règles de `plugin_deps.rs` et `server_audit.rs`.

use crate::server_files::human_bytes;
use crate::updates::compare_numbers;
use crate::yaml_lite;
use serde_json::Value;
use std::cmp::Ordering;
use std::io::Read;

// --- Plafonds -----------------------------------------------------------------

/// Un jar plus petit est lu d'un bloc (un seul aller-retour).
pub(crate) const SMALL_JAR: u64 = 256 * 1024;
/// Première lecture d'un gros jar : sa fin (fin du répertoire central, souvent tout le répertoire).
const FIRST_TAIL: u64 = 16 * 1024;
/// Fin de fichier où chercher la fin du répertoire central (22 o + commentaire ≤ 65 535 o).
const MAX_TAIL: u64 = 22 + 65_535;
/// Répertoire central lu au plus ; au-delà, le jar n'est pas analysé (signalé).
pub(crate) const MAX_DIRECTORY: u64 = 4 * 1024 * 1024;
/// Taille maximale d'un descripteur (compressé comme décompressé).
pub(crate) const MAX_ENTRY: u64 = 512 * 1024;
/// Marge lue après une entrée (en-tête local un peu plus long que l'annonce du répertoire).
const SLACK: u64 = 256;

/// Descripteurs reconnus (nom exact de l'entrée).
pub(crate) const DESCRIPTORS: &[&str] = &[
    "plugin.yml",
    "paper-plugin.yml",
    "bungee.yml",
    "velocity-plugin.json",
    "fabric.mod.json",
    "quilt.mod.json",
    "META-INF/mods.toml",
    "META-INF/neoforge.mods.toml",
    "mcmod.info",
];
/// Manifeste : lu seulement pour un mod Forge/NeoForge (résout `${file.jarVersion}`).
const MANIFEST: &str = "META-INF/MANIFEST.MF";

// --- Lecture du zip -----------------------------------------------------------

/// Échec de lecture d'un jar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JarError {
    /// Archive illisible (tronquée, corrompue, pas un zip).
    Corrupt(String),
    /// Hors plafond : non analysé.
    Limit(String),
}

impl std::fmt::Display for JarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JarError::Corrupt(m) | JarError::Limit(m) => f.write_str(m),
        }
    }
}

fn corrupt(m: impl Into<String>) -> JarError {
    JarError::Corrupt(m.into())
}

const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const MAX32: u64 = 0xFFFF_FFFF;

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(a)
}

/// Tranches déjà lues d'un fichier : triées, fusionnées dès qu'elles se touchent.
#[derive(Default)]
struct Sparse {
    parts: Vec<(u64, Vec<u8>)>,
}

impl Sparse {
    fn insert(&mut self, offset: u64, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        self.parts.push((offset, data));
        self.parts.sort_by_key(|p| p.0);
        let mut merged: Vec<(u64, Vec<u8>)> = Vec::with_capacity(self.parts.len());
        for (o, d) in std::mem::take(&mut self.parts) {
            if let Some((lo, ld)) = merged.last_mut() {
                let end = *lo + ld.len() as u64;
                if o <= end {
                    let new_end = o + d.len() as u64;
                    if new_end > end {
                        ld.extend_from_slice(&d[(end - o) as usize..]);
                    }
                    continue;
                }
            }
            merged.push((o, d));
        }
        self.parts = merged;
    }

    fn get(&self, offset: u64, len: u64) -> Option<&[u8]> {
        self.parts.iter().find_map(|(o, d)| {
            let end = *o + d.len() as u64;
            (offset >= *o && offset + len <= end)
                .then(|| &d[(offset - o) as usize..(offset - o + len) as usize])
        })
    }

    /// Plus petite tranche contiguë à lire pour couvrir `[offset, offset + len)`.
    fn missing(&self, offset: u64, len: u64) -> Option<(u64, u64)> {
        let end = offset + len;
        let mut start = offset;
        while let Some((o, d)) = self.parts.iter().find(|(o, d)| *o <= start && start < *o + d.len() as u64) {
            start = *o + d.len() as u64;
        }
        if start >= end {
            return None;
        }
        let mut stop = end;
        while let Some((o, _)) = self.parts.iter().find(|(o, d)| *o < stop && stop <= *o + d.len() as u64) {
            if *o <= start {
                break;
            }
            stop = *o;
        }
        Some((start, stop - start))
    }
}

/// Entrée du répertoire central.
#[derive(Debug, Clone)]
struct CentralEntry {
    name: String,
    flags: u16,
    method: u16,
    compressed: u64,
    size: u64,
    /// Position de l'en-tête local, relative au début de l'archive.
    local: u64,
    name_len: u64,
    extra_len: u64,
}

struct Layout {
    /// Octets préfixés avant l'archive (jar auto-extractible…), ajoutés aux positions.
    base: u64,
    entries: Vec<CentralEntry>,
}

enum Plan {
    Read(u64, u64),
    Ready(Layout),
}

/// Lecture pilotée d'un jar : `next_read` → lecture par l'appelant → `feed`, jusqu'à `None`, puis
/// `finish`.
pub(crate) struct ZipProbe {
    size: u64,
    have: Sparse,
}

/// Ce qu'un jar contient d'utile.
#[derive(Debug, Default)]
pub(crate) struct JarFiles {
    /// Descripteurs (et manifeste) : contenu décompressé, ou raison de l'échec.
    pub files: Vec<(String, Result<Vec<u8>, String>)>,
    /// Jars imbriqués (`META-INF/jars/…`, `META-INF/jarjar/…`) : mods fournis par ce jar.
    pub nested: Vec<String>,
}

impl ZipProbe {
    pub(crate) fn new(size: u64) -> Self {
        ZipProbe { size, have: Sparse::default() }
    }

    /// Prochaine tranche à lire `(début, longueur)`, ou `None` : tout ce qu'il faut est là.
    pub(crate) fn next_read(&self) -> Result<Option<(u64, u64)>, JarError> {
        let layout = match self.plan()? {
            Plan::Read(o, l) => return Ok(Some((o, l))),
            Plan::Ready(layout) => layout,
        };
        for e in wanted(&layout.entries) {
            if let Some(r) = self.entry_need(&layout, e) {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    pub(crate) fn feed(&mut self, offset: u64, data: Vec<u8>) {
        self.have.insert(offset, data);
    }

    /// Décode ce qui a été lu (à appeler quand `next_read` rend `None`).
    pub(crate) fn finish(&self) -> Result<JarFiles, JarError> {
        let Plan::Ready(layout) = self.plan()? else {
            return Err(corrupt("lecture incomplète de l'archive"));
        };
        let files = wanted(&layout.entries)
            .into_iter()
            .map(|e| (e.name.clone(), self.extract(&layout, e)))
            .collect();
        let nested = layout.entries.iter().filter(|e| is_nested_jar(&e.name)).map(|e| e.name.clone()).collect();
        Ok(JarFiles { files, nested })
    }

    fn find_eocd(&self, from: u64) -> Option<u64> {
        let tail = self.have.get(from, self.size - from)?;
        if tail.len() < 22 {
            return None;
        }
        let mut fallback = None;
        for i in (0..=tail.len() - 22).rev() {
            if u32_at(tail, i) == EOCD_SIG {
                let pos = from + i as u64;
                if pos + 22 + u16_at(tail, i + 20) as u64 == self.size {
                    return Some(pos);
                }
                fallback.get_or_insert(pos);
            }
        }
        fallback
    }

    fn plan(&self) -> Result<Plan, JarError> {
        let size = self.size;
        if size < 22 {
            return Err(corrupt("fichier trop court pour une archive zip"));
        }
        let first = if size <= SMALL_JAR { (0, size) } else { (size - FIRST_TAIL, FIRST_TAIL) };
        if let Some((o, l)) = self.have.missing(first.0, first.1) {
            return Ok(Plan::Read(o, l));
        }
        let mut eocd = self.find_eocd(first.0);
        let big = size.min(MAX_TAIL);
        if eocd.is_none() && big > first.1 {
            if let Some((o, l)) = self.have.missing(size - big, big) {
                return Ok(Plan::Read(o, l));
            }
            eocd = self.find_eocd(size - big);
        }
        let eocd = eocd.ok_or_else(|| corrupt("pas une archive zip (fin de répertoire central introuvable)"))?;
        let rec = self.have.get(eocd, 22).ok_or_else(|| corrupt("fin de répertoire central tronquée"))?;
        let mut count = u16_at(rec, 10) as u64;
        let mut dir_size = u32_at(rec, 12) as u64;
        let mut dir_offset = u32_at(rec, 16) as u64;
        let mut base = 0;
        if dir_offset == MAX32 || dir_size == MAX32 || count == 0xFFFF {
            // Zip64 : le localisateur précède la fin de répertoire et pointe l'enregistrement 64 bits.
            let loc_at = eocd.checked_sub(20).ok_or_else(|| corrupt("zip64 : localisateur absent"))?;
            let Some(loc) = self.have.get(loc_at, 20) else {
                return Ok(Plan::Read(loc_at, 20));
            };
            if u32_at(loc, 0) != ZIP64_LOCATOR_SIG {
                return Err(corrupt("zip64 : localisateur absent"));
            }
            let rec_at = u64_at(loc, 8);
            if rec_at + 56 > size {
                return Err(corrupt("zip64 : enregistrement hors du fichier"));
            }
            let Some(r64) = self.have.get(rec_at, 56) else {
                return Ok(Plan::Read(rec_at, 56));
            };
            if u32_at(r64, 0) != ZIP64_EOCD_SIG {
                return Err(corrupt("zip64 : enregistrement invalide"));
            }
            count = u64_at(r64, 32);
            dir_size = u64_at(r64, 40);
            dir_offset = u64_at(r64, 48);
        } else {
            // Données préfixées : le répertoire central finit juste avant sa fin.
            base = eocd.saturating_sub(dir_size).saturating_sub(dir_offset);
        }
        if dir_size > MAX_DIRECTORY {
            return Err(JarError::Limit(format!(
                "répertoire central de {} (plafond {})",
                human_bytes(dir_size),
                human_bytes(MAX_DIRECTORY)
            )));
        }
        let start = base + dir_offset;
        if start + dir_size > size {
            return Err(corrupt("répertoire central hors du fichier (archive tronquée)"));
        }
        if let Some((o, l)) = self.have.missing(start, dir_size) {
            return Ok(Plan::Read(o, l));
        }
        let dir = self.have.get(start, dir_size).unwrap_or_default();
        let entries = parse_directory(dir, count)?;
        Ok(Plan::Ready(Layout { base, entries }))
    }

    /// Tranche manquante pour une entrée voulue (`None` : complète, ou hors plafond — `extract`
    /// le dira).
    fn entry_need(&self, layout: &Layout, e: &CentralEntry) -> Option<(u64, u64)> {
        if e.compressed > MAX_ENTRY || e.size > MAX_ENTRY {
            return None;
        }
        let at = layout.base + e.local;
        if at + 30 > self.size {
            return None;
        }
        match self.have.get(at, 30) {
            None => {
                let guess = (30 + e.name_len + e.extra_len + e.compressed + SLACK).min(self.size - at);
                self.have.missing(at, guess)
            }
            Some(h) => {
                if u32_at(h, 0) != LOCAL_SIG {
                    return None;
                }
                let data = at + 30 + u16_at(h, 26) as u64 + u16_at(h, 28) as u64;
                if data + e.compressed > self.size {
                    return None;
                }
                self.have.missing(data, e.compressed)
            }
        }
    }

    fn extract(&self, layout: &Layout, e: &CentralEntry) -> Result<Vec<u8>, String> {
        if e.flags & 1 != 0 {
            return Err("entrée chiffrée".into());
        }
        if e.compressed > MAX_ENTRY || e.size > MAX_ENTRY {
            return Err(format!("entrée de {} (plafond {})", human_bytes(e.size.max(e.compressed)), human_bytes(MAX_ENTRY)));
        }
        let at = layout.base + e.local;
        let h = self.have.get(at, 30).ok_or("en-tête local illisible")?;
        if u32_at(h, 0) != LOCAL_SIG {
            return Err("en-tête local corrompu".into());
        }
        let data = at + 30 + u16_at(h, 26) as u64 + u16_at(h, 28) as u64;
        let raw = self.have.get(data, e.compressed).ok_or("données tronquées")?;
        match e.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .take(MAX_ENTRY + 1)
                    .read_to_end(&mut out)
                    .map_err(|err| format!("décompression : {err}"))?;
                if out.len() as u64 > MAX_ENTRY {
                    return Err(format!("entrée de plus de {}", human_bytes(MAX_ENTRY)));
                }
                Ok(out)
            }
            m => Err(format!("méthode de compression {m} non gérée")),
        }
    }
}

fn parse_directory(dir: &[u8], expected: u64) -> Result<Vec<CentralEntry>, JarError> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 46 <= dir.len() && (out.len() as u64) < expected.max(1) * 2 {
        if u32_at(dir, i) != CENTRAL_SIG {
            if out.is_empty() {
                return Err(corrupt("répertoire central corrompu"));
            }
            break;
        }
        let flags = u16_at(dir, i + 8);
        let method = u16_at(dir, i + 10);
        let mut compressed = u32_at(dir, i + 20) as u64;
        let mut size = u32_at(dir, i + 24) as u64;
        let name_len = u16_at(dir, i + 28) as usize;
        let extra_len = u16_at(dir, i + 30) as usize;
        let comment_len = u16_at(dir, i + 32) as usize;
        let mut local = u32_at(dir, i + 42) as u64;
        let name_end = i + 46 + name_len;
        let next = name_end + extra_len + comment_len;
        if next > dir.len() {
            return Err(corrupt("répertoire central tronqué"));
        }
        let name = String::from_utf8_lossy(&dir[i + 46..name_end]).into_owned();
        if size == MAX32 || compressed == MAX32 || local == MAX32 {
            zip64_extra(&dir[name_end..name_end + extra_len], &mut size, &mut compressed, &mut local);
        }
        out.push(CentralEntry {
            name,
            flags,
            method,
            compressed,
            size,
            local,
            name_len: name_len as u64,
            extra_len: extra_len as u64,
        });
        i = next;
    }
    Ok(out)
}

/// Champ supplémentaire zip64 (`0x0001`) : valeurs 64 bits des champs saturés, dans l'ordre.
fn zip64_extra(extra: &[u8], size: &mut u64, compressed: &mut u64, local: &mut u64) {
    let mut i = 0;
    while i + 4 <= extra.len() {
        let id = u16_at(extra, i);
        let len = u16_at(extra, i + 2) as usize;
        let data = &extra[(i + 4).min(extra.len())..(i + 4 + len).min(extra.len())];
        if id == 0x0001 {
            let mut j = 0;
            for field in [size, compressed, local] {
                if *field == MAX32 && j + 8 <= data.len() {
                    *field = u64_at(data, j);
                    j += 8;
                }
            }
            return;
        }
        i += 4 + len;
    }
}

fn wanted(entries: &[CentralEntry]) -> Vec<&CentralEntry> {
    let forge = entries
        .iter()
        .any(|e| e.name == "META-INF/mods.toml" || e.name == "META-INF/neoforge.mods.toml");
    entries
        .iter()
        .filter(|e| DESCRIPTORS.contains(&e.name.as_str()) || (forge && e.name == MANIFEST))
        .collect()
}

fn is_nested_jar(name: &str) -> bool {
    (name.starts_with("META-INF/jars/") || name.starts_with("META-INF/jarjar/")) && name.ends_with(".jar")
}

// --- Descripteurs ---------------------------------------------------------------

/// Plateforme qui charge un descripteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Platform {
    Bukkit,
    Paper,
    Bungee,
    Velocity,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    /// Forge jusqu'à Minecraft 1.12 (`mcmod.info`).
    LegacyForge,
}

impl Platform {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Platform::Bukkit => "Bukkit/Spigot",
            Platform::Paper => "Paper",
            Platform::Bungee => "BungeeCord",
            Platform::Velocity => "Velocity",
            Platform::Fabric => "Fabric",
            Platform::Quilt => "Quilt",
            Platform::Forge => "Forge",
            Platform::NeoForge => "NeoForge",
            Platform::LegacyForge => "Forge ≤ 1.12",
        }
    }

    /// Plugin (dossier `plugins/`) plutôt que mod (dossier `mods/`).
    pub(crate) fn is_plugin(self) -> bool {
        matches!(self, Platform::Bukkit | Platform::Paper | Platform::Bungee | Platform::Velocity)
    }
}

/// Nature d'une dépendance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DepKind {
    Required,
    Optional,
    /// Incompatible : le chargeur refuse les deux ensemble.
    Breaks,
    /// Déconseillé ensemble (avertissement du chargeur).
    Conflicts,
}

/// Contrainte de version d'une dépendance.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum VersionReq {
    Any,
    /// Prédicats Fabric/Quilt : chaque élément est une conjonction (`>=1.20 <1.21`), la liste une
    /// alternative.
    Fabric(Vec<String>),
    /// Intervalle Maven (Forge/NeoForge) : `[1.20,1.21)`, `[47,)`.
    Maven(String),
}

impl VersionReq {
    /// `Some(true/false)` si la version satisfait la contrainte ; `None` : indéterminé.
    pub(crate) fn satisfied_by(&self, version: &str) -> Option<bool> {
        match self {
            VersionReq::Any => Some(true),
            VersionReq::Fabric(preds) => fabric_satisfies(preds, version),
            VersionReq::Maven(spec) => maven_satisfies(spec, version),
        }
    }

    /// Texte lisible (vide pour `Any`).
    pub(crate) fn describe(&self) -> String {
        match self {
            VersionReq::Any => String::new(),
            VersionReq::Fabric(p) => p.join(" ou "),
            VersionReq::Maven(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dep {
    pub id: String,
    pub kind: DepKind,
    pub req: VersionReq,
    /// Dépendance côté client seulement (Forge `side="CLIENT"`) : sans objet sur un serveur.
    pub client_only: bool,
}

/// Un plugin ou un mod décrit par un descripteur (un `mods.toml` peut en décrire plusieurs).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Descriptor {
    pub platform: Platform,
    /// Fichier source dans le jar (`plugin.yml`…).
    pub source: &'static str,
    /// Identifiant de chargement (nom Bukkit/Bungee, id Velocity/Fabric/Quilt, modId Forge).
    pub id: String,
    pub name: String,
    pub version: String,
    pub provides: Vec<String>,
    pub deps: Vec<Dep>,
    /// `api-version` Bukkit/Paper (version minimale de l'API visée).
    pub api_version: Option<String>,
    /// Mod côté client uniquement (`environment: client`).
    pub client_only: bool,
    /// `folia-supported: true`.
    pub folia: bool,
    /// Versions de Minecraft déclarées par `mcmod.info`.
    pub mc_version: Option<String>,
}

impl Descriptor {
    fn new(platform: Platform, source: &'static str, id: String) -> Self {
        Descriptor {
            platform,
            source,
            name: id.clone(),
            id,
            version: String::new(),
            provides: Vec::new(),
            deps: Vec::new(),
            api_version: None,
            client_only: false,
            folia: false,
            mc_version: None,
        }
    }

    /// Libellé court : « Nom version ».
    pub(crate) fn label(&self) -> String {
        if self.version.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.name, self.version)
        }
    }
}

/// Ce qu'un jar déclare.
#[derive(Debug, Default)]
pub(crate) struct JarMeta {
    pub descriptors: Vec<Descriptor>,
    /// Identifiants probables des jars imbriqués (dépendances embarquées).
    pub nested_ids: Vec<String>,
    /// Descripteurs présents mais illisibles : `fichier : raison`.
    pub problems: Vec<String>,
}

/// Interprète les fichiers lus dans un jar.
pub(crate) fn read_meta(files: &JarFiles) -> JarMeta {
    let mut meta = JarMeta::default();
    let manifest = files
        .files
        .iter()
        .find(|(n, _)| n == MANIFEST)
        .and_then(|(_, r)| r.as_ref().ok())
        .and_then(|b| manifest_version(&String::from_utf8_lossy(b)));
    for (name, content) in &files.files {
        if name == MANIFEST {
            continue;
        }
        match content {
            Err(e) => meta.problems.push(format!("{name} : {e}")),
            Ok(bytes) => match parse_descriptor(name, bytes, manifest.as_deref()) {
                Ok(mut d) => meta.descriptors.append(&mut d),
                Err(e) => meta.problems.push(format!("{name} : {e}")),
            },
        }
    }
    meta.nested_ids = files.nested.iter().map(|n| nested_id(n)).filter(|s| !s.is_empty()).collect();
    meta
}

/// `Implementation-Version` du manifeste.
fn manifest_version(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("Implementation-Version:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Identifiant probable d'un jar imbriqué d'après son nom (`fabric-api-base-0.4.42+…jar` →
/// `fabric-api-base`) : tout ce qui précède le premier `-` suivi d'un chiffre.
pub(crate) fn nested_id(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".jar").unwrap_or(file);
    let bytes = stem.as_bytes();
    let cut = (0..bytes.len().saturating_sub(1))
        .find(|&i| bytes[i] == b'-' && bytes[i + 1].is_ascii_digit())
        .unwrap_or(bytes.len());
    stem[..cut].to_ascii_lowercase()
}

/// Identifiant normalisé pour comparer (Bukkit remplace les espaces des noms par `_`).
pub(crate) fn norm_id(id: &str) -> String {
    id.trim().replace(' ', "_").to_lowercase()
}

/// Interprète un descripteur. Erreur : contenu illisible (le chargeur le refuserait sans doute).
pub(crate) fn parse_descriptor(name: &str, bytes: &[u8], jar_version: Option<&str>) -> Result<Vec<Descriptor>, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match name {
        "plugin.yml" => bukkit_like(&yaml_lite::parse(text), Platform::Bukkit, "plugin.yml").map(|d| vec![d]),
        "paper-plugin.yml" => bukkit_like(&yaml_lite::parse(text), Platform::Paper, "paper-plugin.yml").map(|d| vec![d]),
        "bungee.yml" => bukkit_like(&yaml_lite::parse(text), Platform::Bungee, "bungee.yml").map(|d| vec![d]),
        "velocity-plugin.json" => velocity(&lenient_json(text)?).map(|d| vec![d]),
        "fabric.mod.json" => fabric(&lenient_json(text)?).map(|d| vec![d]),
        "quilt.mod.json" => quilt(&lenient_json(text)?).map(|d| vec![d]),
        "META-INF/mods.toml" => forge_toml(text, false, jar_version),
        "META-INF/neoforge.mods.toml" => forge_toml(text, true, jar_version),
        "mcmod.info" => mcmod_info(&lenient_json(text)?),
        _ => Ok(Vec::new()),
    }
}

fn text_of(v: Option<&Value>) -> String {
    yaml_lite::as_text(v).unwrap_or_default().trim().to_string()
}

fn dep(id: &str, kind: DepKind, req: VersionReq) -> Dep {
    Dep { id: id.trim().to_string(), kind, req, client_only: false }
}

/// `plugin.yml`, `paper-plugin.yml`, `bungee.yml` (mêmes clés de base).
fn bukkit_like(v: &Value, platform: Platform, source: &'static str) -> Result<Descriptor, String> {
    let name = text_of(v.get("name"));
    if name.is_empty() {
        return Err("champ « name » absent".into());
    }
    let mut d = Descriptor::new(platform, source, name);
    d.version = text_of(v.get("version"));
    d.api_version = yaml_lite::as_text(v.get("api-version")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    d.folia = yaml_lite::as_bool(v.get("folia-supported")).unwrap_or(false);
    d.provides = yaml_lite::as_list(v.get("provides"));
    let (required, optional): (&[&str], &[&str]) = match platform {
        Platform::Bungee => (&["depends", "depend"], &["softDepends", "softdepends", "softdepend"]),
        _ => (&["depend"], &["softdepend"]),
    };
    for key in required {
        for id in yaml_lite::as_list(v.get(*key)) {
            d.deps.push(dep(&id, DepKind::Required, VersionReq::Any));
        }
    }
    for key in optional {
        for id in yaml_lite::as_list(v.get(*key)) {
            d.deps.push(dep(&id, DepKind::Optional, VersionReq::Any));
        }
    }
    if platform == Platform::Paper {
        paper_dependencies(v.get("dependencies"), &mut d.deps);
    }
    dedup_deps(&mut d.deps);
    Ok(d)
}

/// Dépendances de `paper-plugin.yml` : `dependencies.{server,bootstrap}.<Nom>.required` (défaut
/// `true`), ou l'ancienne liste `[{name, required}]`.
fn paper_dependencies(v: Option<&Value>, out: &mut Vec<Dep>) {
    let required = |cfg: &Value| yaml_lite::as_bool(cfg.get("required")).unwrap_or(true);
    match v {
        Some(Value::Object(sections)) => {
            for section in ["server", "bootstrap"] {
                if let Some(Value::Object(deps)) = sections.get(section) {
                    for (id, cfg) in deps {
                        let kind = if required(cfg) { DepKind::Required } else { DepKind::Optional };
                        out.push(dep(id, kind, VersionReq::Any));
                    }
                }
            }
        }
        Some(Value::Array(list)) => {
            for item in list {
                let id = text_of(item.get("name"));
                if !id.is_empty() {
                    let kind = if required(item) { DepKind::Required } else { DepKind::Optional };
                    out.push(dep(&id, kind, VersionReq::Any));
                }
            }
        }
        _ => {}
    }
}

/// Une même dépendance citée deux fois : on garde la plus forte (obligatoire > optionnelle).
fn dedup_deps(deps: &mut Vec<Dep>) {
    let mut out: Vec<Dep> = Vec::with_capacity(deps.len());
    for d in deps.drain(..) {
        if d.id.is_empty() {
            continue;
        }
        match out.iter_mut().find(|o| norm_id(&o.id) == norm_id(&d.id) && o.kind != DepKind::Breaks && d.kind != DepKind::Breaks) {
            Some(o) => {
                if d.kind == DepKind::Required {
                    o.kind = DepKind::Required;
                }
            }
            None => out.push(d),
        }
    }
    *deps = out;
}

/// JSON tel que l'écrivent les mods : on tente le strict, puis une lecture tolérante (virgules
/// finales, retours à la ligne bruts dans les chaînes), que le chargeur Fabric accepte.
fn lenient_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).or_else(|first| {
        let mut out = String::with_capacity(text.len());
        let mut in_str = false;
        let mut escaped = false;
        let chars: Vec<char> = text.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if in_str {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_str = false,
                    '\n' | '\r' | '\t' => {
                        out.push(' ');
                        continue;
                    }
                    _ => {}
                }
                out.push(c);
                continue;
            }
            if c == '"' {
                in_str = true;
            }
            if c == ',' {
                let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
                if matches!(next, Some('}' | ']')) {
                    continue;
                }
            }
            out.push(c);
        }
        serde_json::from_str(&out).map_err(|_| format!("JSON invalide ({first})"))
    })
}

fn velocity(v: &Value) -> Result<Descriptor, String> {
    let id = text_of(v.get("id"));
    if id.is_empty() {
        return Err("champ « id » absent".into());
    }
    let mut d = Descriptor::new(Platform::Velocity, "velocity-plugin.json", id);
    let name = text_of(v.get("name"));
    if !name.is_empty() {
        d.name = name;
    }
    d.version = text_of(v.get("version"));
    for item in v.get("dependencies").and_then(Value::as_array).into_iter().flatten() {
        let id = text_of(item.get("id"));
        let optional = item.get("optional").and_then(Value::as_bool).unwrap_or(false);
        if !id.is_empty() {
            d.deps.push(dep(&id, if optional { DepKind::Optional } else { DepKind::Required }, VersionReq::Any));
        }
    }
    Ok(d)
}

/// Contrainte Fabric : chaîne, ou tableau (alternative).
fn fabric_req(v: &Value) -> VersionReq {
    match v {
        Value::String(s) => VersionReq::Fabric(vec![s.clone()]),
        Value::Array(a) => VersionReq::Fabric(a.iter().filter_map(Value::as_str).map(str::to_string).collect()),
        _ => VersionReq::Any,
    }
}

fn fabric(v: &Value) -> Result<Descriptor, String> {
    let id = text_of(v.get("id"));
    if id.is_empty() {
        return Err("champ « id » absent".into());
    }
    let mut d = Descriptor::new(Platform::Fabric, "fabric.mod.json", id);
    let name = text_of(v.get("name"));
    if !name.is_empty() {
        d.name = name;
    }
    d.version = text_of(v.get("version"));
    d.provides = v
        .get("provides")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    d.client_only = v.get("environment").and_then(Value::as_str) == Some("client");
    for (key, kind) in [
        ("depends", DepKind::Required),
        ("recommends", DepKind::Optional),
        ("suggests", DepKind::Optional),
        ("breaks", DepKind::Breaks),
        ("conflicts", DepKind::Conflicts),
    ] {
        if let Some(Value::Object(m)) = v.get(key) {
            for (id, req) in m {
                d.deps.push(dep(id, kind, fabric_req(req)));
            }
        }
    }
    Ok(d)
}

/// Contrainte Quilt : chaîne, tableau (alternative), ou `{any: […]}` / `{all: […]}`.
fn quilt_req(v: Option<&Value>) -> VersionReq {
    match v {
        Some(Value::Object(m)) => {
            if let Some(any) = m.get("any") {
                return fabric_req(any);
            }
            match m.get("all").and_then(Value::as_array) {
                Some(all) => VersionReq::Fabric(vec![all.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")]),
                None => VersionReq::Any,
            }
        }
        Some(other) => fabric_req(other),
        None => VersionReq::Any,
    }
}

fn quilt(v: &Value) -> Result<Descriptor, String> {
    let ql = v.get("quilt_loader").ok_or("bloc « quilt_loader » absent")?;
    let id = text_of(ql.get("id"));
    if id.is_empty() {
        return Err("champ « id » absent".into());
    }
    let mut d = Descriptor::new(Platform::Quilt, "quilt.mod.json", id);
    d.version = text_of(ql.get("version"));
    let name = text_of(ql.get("metadata").and_then(|m| m.get("name")));
    if !name.is_empty() {
        d.name = name;
    }
    d.provides = ql
        .get("provides")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_string).or_else(|| p.get("id").and_then(Value::as_str).map(str::to_string)))
                .collect()
        })
        .unwrap_or_default();
    d.client_only = v.get("minecraft").and_then(|m| m.get("environment")).and_then(Value::as_str) == Some("client");
    for (key, kind) in [("depends", DepKind::Required), ("breaks", DepKind::Breaks)] {
        for item in ql.get(key).and_then(Value::as_array).into_iter().flatten() {
            match item {
                Value::String(id) => d.deps.push(dep(id, kind, VersionReq::Any)),
                Value::Object(m) => {
                    let id = text_of(m.get("id"));
                    if id.is_empty() {
                        continue;
                    }
                    let optional = m.get("optional").and_then(Value::as_bool).unwrap_or(false);
                    let kind = if kind == DepKind::Required && optional { DepKind::Optional } else { kind };
                    d.deps.push(dep(&id, kind, quilt_req(m.get("versions"))));
                }
                _ => {}
            }
        }
    }
    Ok(d)
}

/// `mods.toml` (Forge) / `neoforge.mods.toml` : un bloc `[[mods]]` par mod, dépendances sous
/// `[[dependencies.<modId>]]`.
fn forge_toml(text: &str, neoforge_file: bool, jar_version: Option<&str>) -> Result<Vec<Descriptor>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| {
        let msg = e.to_string();
        format!("TOML invalide ({})", msg.lines().next().unwrap_or("").trim())
    })?;
    let deps_table = table.get("dependencies").and_then(toml::Value::as_table);
    let all_deps = deps_table
        .into_iter()
        .flat_map(|t| t.values())
        .filter_map(toml::Value::as_array)
        .flatten();
    let neoforge = neoforge_file
        || all_deps.into_iter().any(|d| d.get("modId").and_then(toml::Value::as_str) == Some("neoforge"));
    let platform = if neoforge { Platform::NeoForge } else { Platform::Forge };
    let source = if neoforge_file { "META-INF/neoforge.mods.toml" } else { "META-INF/mods.toml" };
    let tstr = |v: Option<&toml::Value>| v.and_then(toml::Value::as_str).unwrap_or("").trim().to_string();
    let mods = table.get("mods").and_then(toml::Value::as_array).ok_or("aucun bloc [[mods]]")?;
    let mut out = Vec::new();
    for m in mods {
        let id = tstr(m.get("modId"));
        if id.is_empty() {
            continue;
        }
        let mut d = Descriptor::new(platform, source, id.clone());
        let name = tstr(m.get("displayName"));
        if !name.is_empty() {
            d.name = name;
        }
        d.version = tstr(m.get("version"));
        if d.version.contains("${file.jarVersion}") {
            d.version = jar_version.unwrap_or_default().to_string();
        }
        let list = deps_table.and_then(|t| t.get(&id)).and_then(toml::Value::as_array);
        for item in list.into_iter().flatten() {
            let dep_id = tstr(item.get("modId"));
            if dep_id.is_empty() {
                continue;
            }
            let kind = match item.get("type").and_then(toml::Value::as_str).map(str::to_ascii_lowercase).as_deref() {
                Some("required") => DepKind::Required,
                Some("optional") => DepKind::Optional,
                Some("incompatible") => DepKind::Breaks,
                Some("discouraged") => DepKind::Conflicts,
                _ => match item.get("mandatory").and_then(toml::Value::as_bool) {
                    Some(false) => DepKind::Optional,
                    _ => DepKind::Required,
                },
            };
            let range = tstr(item.get("versionRange"));
            let req = if range.is_empty() { VersionReq::Any } else { VersionReq::Maven(range) };
            let side = tstr(item.get("side")).to_ascii_uppercase();
            d.deps.push(Dep { id: dep_id, kind, req, client_only: side == "CLIENT" });
        }
        out.push(d);
    }
    if out.is_empty() {
        return Err("aucun mod déclaré (modId absent)".into());
    }
    Ok(out)
}

/// `mcmod.info` (Forge ≤ 1.12) : tableau de mods, ou `{modList: […]}`.
fn mcmod_info(v: &Value) -> Result<Vec<Descriptor>, String> {
    let list = match v {
        Value::Array(a) => a.clone(),
        Value::Object(m) => m.get("modList").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for m in &list {
        let id = text_of(m.get("modid"));
        if id.is_empty() {
            continue;
        }
        let mut d = Descriptor::new(Platform::LegacyForge, "mcmod.info", id);
        let name = text_of(m.get("name"));
        if !name.is_empty() {
            d.name = name;
        }
        d.version = text_of(m.get("version"));
        d.mc_version = Some(text_of(m.get("mcversion"))).filter(|s| !s.is_empty() && !s.contains("${"));
        for r in m.get("requiredMods").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            let (id, range) = r.split_once('@').unwrap_or((r, ""));
            if id.eq_ignore_ascii_case("forge") || id.eq_ignore_ascii_case("minecraft") {
                continue;
            }
            let req = if range.is_empty() { VersionReq::Any } else { VersionReq::Maven(range.to_string()) };
            d.deps.push(dep(id, DepKind::Required, req));
        }
        out.push(d);
    }
    if out.is_empty() {
        return Err("aucun mod déclaré".into());
    }
    Ok(out)
}

// --- Versions et contraintes ------------------------------------------------------

/// Nombres d'un numéro de version (`1.20.1`, `v2.3`, `0.15.11+build.1`, `1.21-pre1` → sa base).
/// `None` si le texte ne commence pas par un numéro lisible (snapshot `24w14a`, `${…}`).
pub(crate) fn version_nums(v: &str) -> Option<Vec<u64>> {
    let s = v.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let chars: Vec<char> = s.chars().collect();
    let mut nums = Vec::new();
    let mut i = 0;
    loop {
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            break;
        }
        nums.push(chars[start..i].iter().collect::<String>().parse::<u64>().ok()?);
        if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    if nums.is_empty() || chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(nums)
}

/// Compare deux numéros de version (`None` si l'un est illisible).
pub(crate) fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(compare_numbers(&version_nums(a)?, &version_nums(b)?))
}

/// Alternative de prédicats Fabric : vraie si l'un est satisfait.
pub(crate) fn fabric_satisfies(preds: &[String], version: &str) -> Option<bool> {
    if preds.is_empty() {
        return Some(true);
    }
    let mut unknown = false;
    for p in preds {
        match fabric_all(p, version) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

fn fabric_all(pred: &str, version: &str) -> Option<bool> {
    let mut result = Some(true);
    for c in pred.split_whitespace() {
        match fabric_one(c, version) {
            Some(true) => {}
            Some(false) => return Some(false),
            None => result = None,
        }
    }
    result
}

fn fabric_one(c: &str, version: &str) -> Option<bool> {
    if c.is_empty() || c == "*" {
        return Some(true);
    }
    let (op, rest) = ["<=", ">=", "<", ">", "=", "~", "^"]
        .iter()
        .find_map(|op| c.strip_prefix(op).map(|r| (*op, r.trim())))
        .unwrap_or(("", c));
    let v = version_nums(version)?;
    if let Some(prefix) = wildcard_prefix(rest) {
        let matches = v.len() >= prefix.len() && v[..prefix.len()] == prefix[..];
        return match op {
            "" | "=" => Some(matches),
            _ => None,
        };
    }
    let r = version_nums(rest)?;
    let ord = compare_numbers(&v, &r);
    Some(match op {
        "" | "=" => ord == Ordering::Equal,
        ">=" => ord != Ordering::Less,
        ">" => ord == Ordering::Greater,
        "<=" => ord != Ordering::Greater,
        "<" => ord == Ordering::Less,
        "~" => {
            let upper = if r.len() >= 2 { vec![r[0], r[1] + 1] } else { vec![r[0] + 1] };
            ord != Ordering::Less && compare_numbers(&v, &upper) == Ordering::Less
        }
        "^" => ord != Ordering::Less && compare_numbers(&v, &[r[0] + 1]) == Ordering::Less,
        _ => return None,
    })
}

/// `1.20.x` / `1.20.*` → `[1, 20]`.
fn wildcard_prefix(s: &str) -> Option<Vec<u64>> {
    let parts: Vec<&str> = s.split('.').collect();
    if !matches!(*parts.last()?, "x" | "X" | "*") {
        return None;
    }
    parts[..parts.len() - 1].iter().map(|p| p.parse::<u64>().ok()).collect()
}

/// Intervalle Maven (Forge) : union de `[a,b)`, `(,b]`, `[a]`… Une version nue est une simple
/// recommandation : toute version convient.
pub(crate) fn maven_satisfies(spec: &str, version: &str) -> Option<bool> {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" || !spec.starts_with(['[', '(']) {
        return Some(true);
    }
    let v = version_nums(version)?;
    let bound = |text: &str, inclusive: bool, lower: bool| -> Option<bool> {
        let text = text.trim();
        if text.is_empty() {
            return Some(true);
        }
        let b = version_nums(text)?;
        let o = compare_numbers(&v, &b);
        Some(match (lower, inclusive) {
            (true, true) => o != Ordering::Less,
            (true, false) => o == Ordering::Greater,
            (false, true) => o != Ordering::Greater,
            (false, false) => o == Ordering::Less,
        })
    };
    let mut rest = spec;
    let mut unknown = false;
    while let Some(start) = rest.find(['[', '(']) {
        let end = start + rest[start..].find([']', ')'])?;
        let (open, close) = (rest.as_bytes()[start] == b'[', rest.as_bytes()[end] == b']');
        let inner = &rest[start + 1..end];
        let ok = match inner.split_once(',') {
            Some((lo, hi)) => match (bound(lo, open, true), bound(hi, close, false)) {
                (Some(a), Some(b)) => Some(a && b),
                (Some(false), _) | (_, Some(false)) => Some(false),
                _ => None,
            },
            None => version_nums(inner).map(|x| compare_numbers(&v, &x) == Ordering::Equal),
        };
        match ok {
            Some(true) => return Some(true),
            None => unknown = true,
            Some(false) => {}
        }
        rest = &rest[end + 1..];
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    /// Construit un jar en mémoire : `(nom, contenu, compressé ?)`.
    pub(crate) fn make_jar(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data, deflate) in entries {
            let method = if *deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            w.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(method)).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// Pilote une sonde sur des octets en mémoire ; renvoie le résultat et les lectures faites.
    pub(crate) fn drive(bytes: &[u8]) -> (Result<JarFiles, JarError>, Vec<(u64, u64)>) {
        let mut probe = ZipProbe::new(bytes.len() as u64);
        let mut reads = Vec::new();
        for _ in 0..16 {
            match probe.next_read() {
                Ok(Some((o, l))) => {
                    reads.push((o, l));
                    let end = (o + l).min(bytes.len() as u64) as usize;
                    probe.feed(o, bytes[o as usize..end].to_vec());
                }
                Ok(None) => return (probe.finish(), reads),
                Err(e) => return (Err(e), reads),
            }
        }
        (Err(corrupt("boucle")), reads)
    }

    /// Octets pseudo-aléatoires (incompressibles), pour simuler du code compilé.
    fn noise(n: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x & 0xff) as u8
            })
            .collect()
    }

    #[test]
    fn small_jar_is_read_in_one_go() {
        let jar = make_jar(&[("plugin.yml", b"name: A\nversion: 1\n", true), ("a/B.class", b"xx", false)]);
        let (files, reads) = drive(&jar);
        let files = files.unwrap();
        assert_eq!(reads, vec![(0, jar.len() as u64)]);
        assert_eq!(files.files.len(), 1);
        assert_eq!(files.files[0].1.as_deref().unwrap(), b"name: A\nversion: 1\n");
    }

    #[test]
    fn big_jar_reads_only_the_directory_and_the_descriptor() {
        let blob = noise(2 * 1024 * 1024);
        let jar = make_jar(&[
            ("fabric.mod.json", br#"{"id":"demo","version":"1.0.0"}"#, true),
            ("assets/big.bin", &blob, false),
            ("META-INF/jars/fabric-api-base-0.4.42+6573ed8c19.jar", b"jar", false),
        ]);
        let (files, reads) = drive(&jar);
        let files = files.unwrap();
        let read: u64 = reads.iter().map(|r| r.1).sum();
        assert!(read < 64 * 1024, "lu {read} o sur {}", jar.len());
        assert_eq!(files.files[0].0, "fabric.mod.json");
        assert_eq!(files.nested, vec!["META-INF/jars/fabric-api-base-0.4.42+6573ed8c19.jar"]);
    }

    #[test]
    fn prefixed_archive_and_large_comment_are_handled() {
        let jar = make_jar(&[("bungee.yml", b"name: P\n", false)]);
        let mut prefixed = noise(300 * 1024);
        prefixed.extend_from_slice(&jar);
        let (files, _) = drive(&prefixed);
        assert_eq!(files.unwrap().files[0].1.as_deref().unwrap(), b"name: P\n");

        // Commentaire d'archive de 40 Kio : la fin de répertoire est hors de la première lecture.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.set_comment("c".repeat(40 * 1024)).unwrap();
        w.start_file("plugin.yml", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"name: C\n").unwrap();
        w.start_file("pad.bin", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        w.write_all(&noise(400 * 1024)).unwrap();
        let commented = w.finish().unwrap().into_inner();
        let (files, reads) = drive(&commented);
        assert_eq!(files.unwrap().files[0].1.as_deref().unwrap(), b"name: C\n");
        assert!(reads.len() >= 2);
    }

    #[test]
    fn rejects_non_zip_and_truncated_files() {
        let (r, _) = drive(&noise(10_000));
        assert!(matches!(r, Err(JarError::Corrupt(_))), "{r:?}");
        let jar = make_jar(&[("plugin.yml", b"name: A\n", false)]);
        let (r, _) = drive(&jar[..jar.len() - 10]);
        assert!(r.is_err());
        let (r, _) = drive(b"PK");
        assert!(r.is_err());
    }

    #[test]
    fn sparse_merges_and_finds_missing_ranges() {
        let mut s = Sparse::default();
        s.insert(10, vec![1; 10]);
        s.insert(30, vec![2; 10]);
        assert_eq!(s.missing(0, 50), Some((0, 50)));
        assert_eq!(s.missing(10, 10), None);
        assert_eq!(s.missing(15, 20), Some((20, 10)));
        s.insert(20, vec![3; 10]);
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.get(25, 10).unwrap(), &[3, 3, 3, 3, 3, 2, 2, 2, 2, 2]);
    }

    #[test]
    fn parses_bukkit_paper_and_bungee_descriptors() {
        let d = parse_descriptor(
            "plugin.yml",
            b"name: Essentials Chat\nversion: 2.21.0\nmain: x\napi-version: 1.13\ndepend: [Essentials]\nsoftdepend: [Vault, LuckPerms]\n",
            None,
        )
        .unwrap();
        assert_eq!(d[0].platform, Platform::Bukkit);
        assert_eq!(norm_id(&d[0].id), "essentials_chat");
        assert_eq!(d[0].api_version.as_deref(), Some("1.13"));
        assert_eq!(d[0].deps.iter().filter(|x| x.kind == DepKind::Required).count(), 1);
        assert_eq!(d[0].deps.iter().filter(|x| x.kind == DepKind::Optional).count(), 2);

        let p = parse_descriptor(
            "paper-plugin.yml",
            b"name: Chunky\nversion: '1.4.40'\napi-version: '1.20'\nfolia-supported: true\ndependencies:\n  server:\n    WorldBorder:\n      load: BEFORE\n      required: false\n    Vault:\n      load: BEFORE\n",
            None,
        )
        .unwrap();
        assert!(p[0].folia);
        assert_eq!(p[0].deps.len(), 2);
        assert_eq!(p[0].deps.iter().find(|x| x.id == "Vault").unwrap().kind, DepKind::Required);
        assert_eq!(p[0].deps.iter().find(|x| x.id == "WorldBorder").unwrap().kind, DepKind::Optional);

        let b = parse_descriptor("bungee.yml", b"name: BungeeGuard\nversion: 1.3\nmain: x\nsoftDepends:\n  - LuckPerms\n", None).unwrap();
        assert_eq!(b[0].platform, Platform::Bungee);
        assert_eq!(b[0].deps[0].kind, DepKind::Optional);

        assert!(parse_descriptor("plugin.yml", b"version: 1\nmain: x\n", None).is_err());
    }

    #[test]
    fn parses_velocity_fabric_and_quilt_descriptors() {
        let v = parse_descriptor(
            "velocity-plugin.json",
            br#"{"id":"luckperms","name":"LuckPerms","version":"5.5.71","main":"x","dependencies":[{"id":"miniplaceholders","optional":true}]}"#,
            None,
        )
        .unwrap();
        assert_eq!(v[0].platform, Platform::Velocity);
        assert_eq!(v[0].deps[0].kind, DepKind::Optional);

        // Forme réelle de Sodium 0.5 (extraits), avec virgule finale et retour à la ligne brut
        // dans une chaîne : le chargeur Fabric les tolère.
        let f = parse_descriptor(
            "fabric.mod.json",
            b"{\"schemaVersion\":1,\"id\":\"sodium\",\"version\":\"0.5.11+mc1.20.1\",\"name\":\"Sodium\",\n\
              \"description\":\"Ligne 1\nLigne 2\",\"environment\":\"client\",\n\
              \"depends\":{\"fabricloader\":\">=0.12.0\",\"minecraft\":[\"1.20\",\"1.20.1\"]},\n\
              \"breaks\":{\"optifabric\":\"*\"},\"provides\":[\"indium-compat\"],}",
            None,
        )
        .unwrap();
        assert_eq!(f[0].id, "sodium");
        assert!(f[0].client_only);
        assert_eq!(f[0].provides, vec!["indium-compat"]);
        let mc = f[0].deps.iter().find(|d| d.id == "minecraft").unwrap();
        assert_eq!(mc.req.satisfied_by("1.20.1"), Some(true));
        assert_eq!(mc.req.satisfied_by("1.21"), Some(false));
        assert_eq!(f[0].deps.iter().find(|d| d.id == "optifabric").unwrap().kind, DepKind::Breaks);

        let q = parse_descriptor(
            "quilt.mod.json",
            br#"{"schema_version":1,"quilt_loader":{"group":"org.x","id":"qsl_demo","version":"1.0.0",
               "metadata":{"name":"Demo"},"provides":["demo_alias",{"id":"other","version":"1"}],
               "depends":["quilt_loader",{"id":"minecraft","versions":">=1.20 <1.21"},{"id":"modmenu","optional":true}],
               "breaks":[{"id":"bad","versions":{"any":["<2"]}}]},"minecraft":{"environment":"*"}}"#,
            None,
        )
        .unwrap();
        assert_eq!(q[0].platform, Platform::Quilt);
        assert_eq!(q[0].name, "Demo");
        assert_eq!(q[0].provides, vec!["demo_alias", "other"]);
        assert_eq!(q[0].deps.iter().find(|d| d.id == "modmenu").unwrap().kind, DepKind::Optional);
        let mc = q[0].deps.iter().find(|d| d.id == "minecraft").unwrap();
        assert_eq!(mc.req.satisfied_by("1.20.4"), Some(true));
        assert_eq!(mc.req.satisfied_by("1.21.1"), Some(false));
        assert!(parse_descriptor("fabric.mod.json", b"{pas du json", None).is_err());
    }

    #[test]
    fn parses_forge_and_neoforge_tomls() {
        // Gabarit MDK Forge 1.20.1 (forme publiée), description multiligne comprise.
        let forge = br#"
modLoader="javafml" #mandatory
loaderVersion="[47,)"
license="MIT"
[[mods]]
modId="examplemod"
version="${file.jarVersion}"
displayName="Example Mod"
description='''
Une description
sur plusieurs lignes.
'''
[[dependencies.examplemod]]
    modId="forge"
    mandatory=true
    versionRange="[47,)"
    ordering="NONE"
    side="BOTH"
[[dependencies.examplemod]]
    modId="minecraft"
    mandatory=true
    versionRange="[1.20.1,1.21)"
    ordering="NONE"
    side="BOTH"
[[dependencies.examplemod]]
    modId="jei"
    mandatory=false
    versionRange="[15,)"
    side="CLIENT"
"#;
        let d = parse_descriptor("META-INF/mods.toml", forge, Some("1.2.3")).unwrap();
        assert_eq!(d[0].platform, Platform::Forge);
        assert_eq!(d[0].version, "1.2.3");
        assert_eq!(d[0].name, "Example Mod");
        let mc = d[0].deps.iter().find(|x| x.id == "minecraft").unwrap();
        assert_eq!(mc.req.satisfied_by("1.20.1"), Some(true));
        assert_eq!(mc.req.satisfied_by("1.21"), Some(false));
        assert!(d[0].deps.iter().find(|x| x.id == "jei").unwrap().client_only);

        // NeoForge 1.21 : fichier dédié et `type` à la place de `mandatory`.
        let neo = br#"
modLoader="javafml"
loaderVersion="[4,)"
license="MIT"
[[mods]]
modId="neomod"
version="2.0.0"
[[dependencies.neomod]]
    modId="neoforge"
    type="required"
    versionRange="[21.1,)"
[[dependencies.neomod]]
    modId="badmod"
    type="incompatible"
    versionRange="*"
"#;
        let n = parse_descriptor("META-INF/neoforge.mods.toml", neo, None).unwrap();
        assert_eq!(n[0].platform, Platform::NeoForge);
        assert_eq!(n[0].deps.iter().find(|x| x.id == "badmod").unwrap().kind, DepKind::Breaks);
        // Un mods.toml qui dépend de « neoforge » (NeoForge 20.2 à 20.4) est un mod NeoForge.
        let mixed = parse_descriptor("META-INF/mods.toml", neo, None).unwrap();
        assert_eq!(mixed[0].platform, Platform::NeoForge);
        assert!(parse_descriptor("META-INF/mods.toml", b"modLoader=", None).is_err());
    }

    #[test]
    fn reads_meta_from_a_real_shaped_forge_jar() {
        let toml = b"modLoader=\"javafml\"\nloaderVersion=\"[47,)\"\nlicense=\"x\"\n[[mods]]\nmodId=\"m\"\nversion=\"${file.jarVersion}\"\n";
        let jar = make_jar(&[
            ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\r\nImplementation-Version: 4.5.6\r\n", true),
            ("META-INF/mods.toml", toml, true),
            ("META-INF/jarjar/kotlinforforge-4.10.0-all.jar", b"j", false),
            ("mcmod.info", b"[{\"modid\":\"old\",\"name\":\"Old\",\"version\":\"1\",\"mcversion\":\"1.12.2\"}]", false),
        ]);
        let (files, _) = drive(&jar);
        let meta = read_meta(&files.unwrap());
        let forge = meta.descriptors.iter().find(|d| d.platform == Platform::Forge).unwrap();
        assert_eq!(forge.version, "4.5.6");
        assert_eq!(meta.nested_ids, vec!["kotlinforforge"]);
        let legacy = meta.descriptors.iter().find(|d| d.platform == Platform::LegacyForge).unwrap();
        assert_eq!(legacy.mc_version.as_deref(), Some("1.12.2"));
        assert!(meta.problems.is_empty());
    }

    #[test]
    fn nested_ids_come_from_file_names() {
        assert_eq!(nested_id("META-INF/jars/fabric-api-base-0.4.42+6573ed8c19.jar"), "fabric-api-base");
        assert_eq!(nested_id("META-INF/jars/fabric-networking-api-v1-4.0.0.jar"), "fabric-networking-api-v1");
        assert_eq!(nested_id("META-INF/jarjar/mixinextras-forge-0.3.5.jar"), "mixinextras-forge");
        assert_eq!(nested_id("META-INF/jars/sans-version.jar"), "sans-version");
    }

    #[test]
    fn version_numbers_and_comparisons() {
        assert_eq!(version_nums("1.20.1"), Some(vec![1, 20, 1]));
        assert_eq!(version_nums("v2.3-SNAPSHOT"), Some(vec![2, 3]));
        assert_eq!(version_nums("0.15.11+build.1"), Some(vec![0, 15, 11]));
        assert_eq!(version_nums("1.19.4-R0.1-SNAPSHOT"), Some(vec![1, 19, 4]));
        assert_eq!(version_nums("24w14a"), None);
        assert_eq!(version_nums("${file.jarVersion}"), None);
        assert_eq!(compare_versions("26.1.2", "1.21"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.20", "1.20.0"), Some(Ordering::Equal));
    }

    #[test]
    fn fabric_predicates() {
        let s = |p: &str, v: &str| fabric_satisfies(&[p.to_string()], v);
        assert_eq!(s(">=1.20 <1.21", "1.20.4"), Some(true));
        assert_eq!(s(">=1.20 <1.21", "1.21"), Some(false));
        assert_eq!(s("~1.20.1", "1.20.6"), Some(true));
        assert_eq!(s("~1.20.1", "1.21"), Some(false));
        assert_eq!(s("^0.15", "0.16.2"), Some(true));
        assert_eq!(s("^1.2", "2.0"), Some(false));
        assert_eq!(s("1.20.x", "1.20.4"), Some(true));
        assert_eq!(s("1.20.x", "1.21.1"), Some(false));
        assert_eq!(s("1.20.1", "1.20.1"), Some(true));
        assert_eq!(s("*", "anything"), Some(true));
        assert_eq!(s(">=1.20", "24w14a"), None);
        assert_eq!(fabric_satisfies(&["1.19.4".into(), "1.20.1".into()], "1.20.1"), Some(true));
    }

    #[test]
    fn maven_ranges() {
        assert_eq!(maven_satisfies("[1.20.1,1.21)", "1.20.4"), Some(true));
        assert_eq!(maven_satisfies("[1.20.1,1.21)", "1.21"), Some(false));
        assert_eq!(maven_satisfies("[47,)", "47.2.0"), Some(true));
        assert_eq!(maven_satisfies("(,1.20]", "1.20.1"), Some(false));
        assert_eq!(maven_satisfies("[1.20.1]", "1.20.1"), Some(true));
        assert_eq!(maven_satisfies("[1.18,1.19),[1.20,1.21)", "1.20.2"), Some(true));
        assert_eq!(maven_satisfies("1.20.1", "1.19"), Some(true), "version nue = recommandation");
        assert_eq!(maven_satisfies("[1.20,)", "24w14a"), None);
    }
}
