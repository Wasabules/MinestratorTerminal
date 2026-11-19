//! Outil MCP `upload_world` : installe un monde Minecraft depuis l'**ordinateur** (dossier du monde
//! ou archive `.zip` / `.mcworld`) sur un serveur.
//!
//! - **Source** ([`inspect_source`], bloquant) : `level.dat` exigé ; un dossier racine en trop
//!   (`MonMonde/level.dat`) est retiré ; plusieurs mondes, entrées dangereuses (`..`, chemin
//!   absolu) ou chiffrées sont refusés ; liens symboliques, `session.lock` et fichiers parasites
//!   (`__MACOSX`, `.DS_Store`…) écartés ; plafonds de fichiers et de taille.
//! - **Archive** ([`build_archive`]) : compressée côté machine dans un fichier temporaire (les
//!   entrées d'un `.zip` fourni sont recopiées sans recompression, renommées) ; le monde y est à la
//!   racine, prêt à être extrait dans son dossier. 512 Mio au plus (plafond de `upload_file`).
//! - **Étapes** ([`upload_world_with`]) : confirmation de l'humain si un arrêt est nécessaire
//!   (`mcp_ctx::confirm`, avant tout effet) → arrêt et attente → snapshot attendue → ancien monde et
//!   ses dimensions `_nether` / `_the_end` renommés `<nom>.old-<date>` (jamais supprimés) → envoi
//!   de l'archive → extraction par le serveur (`extract_archive`) → vérification de `level.dat` →
//!   archive temporaire supprimée → `level-name` (facultatif) → démarrage (facultatif). Chaque
//!   sous-écriture passe par la garde des agents ([`ServerSide::write`] → `guarded_write`), une
//!   étape annulable chacune ; l'outil lui-même n'a pas d'étape (`Plan::Skip`).
//! - **Simulation** (`dry_run`, défaut) : toutes les vérifications, aucun effet ; le plan dit ce
//!   qui serait fait et ce qui bloque.

use crate::agent_guard::undo::LIST_MAX_ENTRIES;
use crate::local_sync::{bool_arg, remote_path, CoreSide, ServerSide};
use crate::mcp_ctx::{self, Confirmation};
use crate::Core;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;
/// Archive envoyée au plus : plafond de `upload_file` (fichier lu en mémoire).
pub(crate) const ARCHIVE_MAX_BYTES: u64 = 512 * MIB;
/// Fichiers du monde au plus.
pub(crate) const WORLD_MAX_FILES: usize = 50_000;
/// Taille du monde (décompressé) au plus.
pub(crate) const WORLD_MAX_BYTES: u64 = 8 * 1024 * MIB;
/// Attente de l'arrêt du serveur au plus.
const STOP_WAIT: Duration = Duration::from_secs(180);
const STATE_POLL: Duration = Duration::from_secs(3);
/// Fichiers jamais envoyés : verrou du jeu (recréé au démarrage) et fichiers parasites des OS.
const JUNK_NAMES: &[&str] = &["session.lock", ".DS_Store", "Thumbs.db", "desktop.ini"];

// --- Côté serveur ------------------------------------------------------------------------------

/// Édition de Minecraft du serveur : dossier des mondes et forme attendue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edition {
    /// Monde à la racine (`/<nom>`), dimensions `_nether` / `_the_end` à côté sur Paper.
    Java,
    /// Monde sous `/worlds/<nom>`, base LevelDB `db/`.
    Bedrock,
}

/// Ce qu'`upload_world` lit du serveur en plus des fichiers.
pub(crate) trait WorldServer: ServerSide {
    /// Édition de Minecraft ; erreur pour un autre jeu.
    fn edition(&self) -> impl Future<Output = Result<Edition, String>> + Send;
    /// État normalisé (`running`, `starting`, `stopping`, `offline`…).
    fn state(&self) -> impl Future<Output = Result<String, String>> + Send;
    /// `level-name` de `server.properties` (`None` : absent ou illisible).
    fn level_name(&self) -> impl Future<Output = Option<String>> + Send;
    /// Place libre (le plus petit du serveur et de sa MyBox), `None` si inconnue.
    fn free_bytes(&self) -> impl Future<Output = Option<u64>> + Send;
    /// Snapshot créée puis attendue.
    fn snapshot(&self, name: &str) -> impl Future<Output = Result<(), String>> + Send;
}

impl WorldServer for CoreSide<'_> {
    async fn edition(&self) -> Result<Edition, String> {
        let caps = self.core.server_capabilities(self.id).await.map_err(|e| e.to_string())?;
        if caps.is_minecraft_bedrock() {
            Ok(Edition::Bedrock)
        } else if caps.family == "minecraft" {
            Ok(Edition::Java)
        } else {
            Err(format!(
                "serveur {} : jeu « {} », pas Minecraft — upload_world n'installe que des mondes Minecraft",
                self.id, caps.family
            ))
        }
    }
    async fn state(&self) -> Result<String, String> {
        match self.core.server_live(self.id).await {
            Ok(l) => Ok(crate::agent_loop::normalize_state(&l.state)),
            Err(e) => match self.core.live_light(self.id).await {
                Ok(ll) => Ok(crate::agent_loop::normalize_state(ll.status.as_deref().unwrap_or(""))),
                Err(_) => Err(e.to_string()),
            },
        }
    }
    async fn level_name(&self) -> Option<String> {
        let props = self.core.server_properties(self.id).await.ok()?;
        props.properties.get("level-name").map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
    async fn free_bytes(&self) -> Option<u64> {
        self.core.disk_space(self.id).await.ok().as_ref().and_then(crate::pregen::free_bytes)
    }
    async fn snapshot(&self, name: &str) -> Result<(), String> {
        let job = self
            .core
            .create_snapshot(self.id, name)
            .await
            .map_err(|e| format!("snapshot préalable impossible ({e})"))?;
        Box::pin(crate::manifest::wait_snapshot(self.core, job, name)).await
    }
}

// --- Source locale -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Dir,
    Zip,
}

/// Entrée retenue : chemin dans le monde (racine en trop retirée) et origine.
#[derive(Debug, Clone)]
pub(crate) struct SourceEntry {
    pub(crate) rel: String,
    pub(crate) is_dir: bool,
    pub(crate) size: u64,
    /// Fichier local (source dossier).
    pub(crate) abs: Option<PathBuf>,
    /// Index dans l'archive (source `.zip`).
    pub(crate) index: usize,
}

/// Monde trouvé dans la source.
#[derive(Debug, Clone)]
pub(crate) struct WorldSource {
    pub(crate) kind: SourceKind,
    pub(crate) path: PathBuf,
    /// Dossier racine en trop retiré des chemins (`MonMonde/`), vide sinon.
    pub(crate) strip: String,
    pub(crate) entries: Vec<SourceEntry>,
    pub(crate) files: usize,
    /// Taille décompressée.
    pub(crate) bytes: u64,
    /// Taille de l'archive fournie (`None` pour un dossier, connue une fois compressée).
    pub(crate) archive_bytes: Option<u64>,
    /// Base LevelDB `db/` (Bedrock).
    pub(crate) has_db: bool,
    /// Régions Anvil (`region/`, `DIM-1/`, `DIM1/` : Java).
    pub(crate) has_region: bool,
    /// Entrées écartées (liens, parasites, hors du dossier du monde).
    pub(crate) ignored: usize,
}

struct RawEntry {
    name: String,
    is_dir: bool,
    size: u64,
    abs: Option<PathBuf>,
    index: usize,
}

fn base_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn junk(name: &str) -> bool {
    name.split('/').any(|c| c == "__MACOSX") || JUNK_NAMES.contains(&base_name(name))
}

/// Dossier du monde dans la source : `""` si `level.dat` est à la racine, `X/` s'il est dans un
/// unique dossier de premier niveau. Refus : aucun `level.dat`, trop profond, plusieurs mondes.
pub(crate) fn detect_root(files: &[&str]) -> Result<String, String> {
    if files.contains(&"level.dat") {
        return Ok(String::new());
    }
    let tops: BTreeSet<&str> = files
        .iter()
        .filter_map(|n| n.strip_suffix("/level.dat"))
        .filter(|p| !p.contains('/'))
        .collect();
    match tops.len() {
        1 => Ok(format!("{}/", tops.first().copied().unwrap_or_default())),
        0 => match files.iter().find(|n| n.ends_with("/level.dat")) {
            Some(deep) => Err(format!(
                "level.dat trop profond dans la source (« {deep} ») : donne directement le dossier du monde"
            )),
            None => Err(
                "aucun level.dat : ce n'est pas un monde Minecraft (donne le dossier du monde, ou une archive .zip qui le contient)"
                    .into(),
            ),
        },
        _ => Err(format!(
            "plusieurs mondes dans la source ({}) : envoie-les un par un (world_name)",
            tops.into_iter().collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Entrées d'un dossier local, liens jamais suivis.
fn collect_dir(root: &Path) -> Result<(Vec<RawEntry>, usize), String> {
    let mut out = Vec::new();
    let mut ignored = 0usize;
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
            .and_then(|rd| rd.collect::<std::io::Result<Vec<_>>>())
            .map_err(|e| format!("lecture du dossier local « {} » : {e}", dir.display()))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(name) = e.file_name().into_string() else {
                ignored += 1;
                continue;
            };
            let rel = if rel_dir.is_empty() { name } else { format!("{rel_dir}/{name}") };
            let ft = e.file_type().map_err(|err| format!("« {rel} » : {err}"))?;
            if ft.is_symlink() {
                ignored += 1;
            } else if ft.is_dir() {
                stack.push((e.path(), rel.clone()));
                out.push(RawEntry { name: rel, is_dir: true, size: 0, abs: None, index: 0 });
            } else if ft.is_file() {
                let size = e.metadata().map_err(|err| format!("« {rel} » : {err}"))?.len();
                out.push(RawEntry { name: rel, is_dir: false, size, abs: Some(e.path()), index: 0 });
            } else {
                ignored += 1;
            }
            if out.len() > WORLD_MAX_FILES * 2 {
                return Err(format!("plus de {WORLD_MAX_FILES} fichiers dans la source (plafond)"));
            }
        }
    }
    Ok((out, ignored))
}

/// Entrées d'une archive `.zip` (répertoire central seulement, rien n'est décompressé).
fn collect_zip(path: &Path) -> Result<(Vec<RawEntry>, usize, u64), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("archive « {} » illisible : {e}", path.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let mut ar = zip::ZipArchive::new(file).map_err(|e| format!("archive zip illisible : {e}"))?;
    let mut out = Vec::with_capacity(ar.len());
    let mut ignored = 0usize;
    for i in 0..ar.len() {
        let f = ar.by_index_raw(i).map_err(|e| format!("archive (entrée {i}) : {e}"))?;
        let name = f.name().map_err(|_| format!("archive (entrée {i}) : nom illisible"))?.replace('\\', "/");
        if f.encrypted() {
            return Err("archive chiffrée : non prise en charge".into());
        }
        if !crate::archive::safe_entry(&name) {
            return Err(format!("entrée dangereuse refusée dans l'archive : « {name} »"));
        }
        if f.is_symlink() {
            ignored += 1;
            continue;
        }
        let is_dir = f.is_dir() || name.ends_with('/');
        let name = name.trim_end_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        out.push(RawEntry { name, is_dir, size: f.size(), abs: None, index: i });
        if out.len() > WORLD_MAX_FILES * 2 {
            return Err(format!("plus de {WORLD_MAX_FILES} fichiers dans l'archive (plafond)"));
        }
    }
    Ok((out, ignored, size))
}

fn finish(kind: SourceKind, path: &Path, raw: Vec<RawEntry>, mut ignored: usize, archive_bytes: Option<u64>) -> Result<WorldSource, String> {
    let names: Vec<&str> = raw.iter().filter(|e| !e.is_dir && !junk(&e.name)).map(|e| e.name.as_str()).collect();
    let strip = detect_root(&names)?;
    let root_dir = strip.trim_end_matches('/');
    let (mut entries, mut files, mut bytes) = (Vec::new(), 0usize, 0u64);
    for e in raw {
        if junk(&e.name) {
            ignored += 1;
            continue;
        }
        if !strip.is_empty() && e.name == root_dir {
            continue;
        }
        let Some(rel) = e.name.strip_prefix(strip.as_str()) else {
            ignored += 1;
            continue;
        };
        if !e.is_dir {
            files += 1;
            bytes += e.size;
        }
        entries.push(SourceEntry { rel: rel.to_string(), is_dir: e.is_dir, size: e.size, abs: e.abs, index: e.index });
    }
    if files > WORLD_MAX_FILES {
        return Err(format!("{files} fichiers dans le monde : au-delà du plafond de {WORLD_MAX_FILES}"));
    }
    if bytes > WORLD_MAX_BYTES {
        return Err(format!("monde de {} : au-delà du plafond de {}", human(bytes), human(WORLD_MAX_BYTES)));
    }
    let under = |dir: &str| entries.iter().any(|e| e.rel == dir || e.rel.starts_with(&format!("{dir}/")));
    let has_db = under("db");
    let has_region = under("region") || under("DIM-1") || under("DIM1");
    Ok(WorldSource { kind, path: path.to_path_buf(), strip, entries, files, bytes, archive_bytes, has_db, has_region, ignored })
}

/// Lit la source (bloquant) : dossier du monde, ou archive `.zip` / `.mcworld`.
pub(crate) fn inspect_source(path: &Path) -> Result<WorldSource, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("source locale « {} » illisible : {e}", path.display()))?;
    if meta.is_dir() {
        let (raw, ignored) = collect_dir(path)?;
        return finish(SourceKind::Dir, path, raw, ignored, None);
    }
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if !(lower.ends_with(".zip") || lower.ends_with(".mcworld")) {
        return Err("source non prise en charge : donne le dossier du monde, ou une archive .zip (.mcworld pour Bedrock)".into());
    }
    let (raw, ignored, size) = collect_zip(path)?;
    finish(SourceKind::Zip, path, raw, ignored, Some(size))
}

/// Compresse le monde dans `out` (bloquant), à la racine de l'archive. Rend sa taille.
pub(crate) fn build_archive(src: &WorldSource, out: &Path) -> Result<u64, String> {
    use zip::write::SimpleFileOptions;
    let zerr = |e: zip::result::ZipError| format!("compression : {e}");
    let file = std::fs::File::create(out).map_err(|e| format!("archive temporaire « {} » : {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    match src.kind {
        SourceKind::Dir => {
            for e in &src.entries {
                if e.is_dir {
                    zip.add_directory(e.rel.clone(), SimpleFileOptions::default()).map_err(zerr)?;
                    continue;
                }
                let abs = e.abs.as_ref().ok_or("fichier local perdu")?;
                let opts = SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .large_file(e.size >= u64::from(u32::MAX));
                zip.start_file(e.rel.as_str(), opts).map_err(zerr)?;
                let mut f = std::fs::File::open(abs).map_err(|err| format!("lecture de « {} » : {err}", e.rel))?;
                std::io::copy(&mut f, &mut zip).map_err(|err| format!("compression de « {} » : {err}", e.rel))?;
            }
        }
        SourceKind::Zip => {
            let input = std::fs::File::open(&src.path).map_err(|e| format!("archive illisible : {e}"))?;
            let mut ar = zip::ZipArchive::new(input).map_err(|e| format!("archive zip illisible : {e}"))?;
            for e in &src.entries {
                let f = ar.by_index_raw(e.index).map_err(zerr)?;
                let name = if e.is_dir { format!("{}/", e.rel) } else { e.rel.clone() };
                zip.raw_copy_file_rename(f, name).map_err(zerr)?;
            }
        }
    }
    let file = zip.finish().map_err(zerr)?;
    file.metadata().map(|m| m.len()).map_err(|e| e.to_string())
}

/// Archive temporaire locale, supprimée quoi qu'il arrive.
struct TempArchive(PathBuf);

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn human(n: u64) -> String {
    crate::server_files::human_bytes(n)
}

// --- Requête et plan ---------------------------------------------------------------------------

/// Nom de dossier de monde acceptable : un seul segment, sans caractère spécial.
pub(crate) fn world_name_ok(name: &str) -> Result<String, String> {
    let n = name.trim();
    let bad = |c: char| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control();
    if n.is_empty() || n.chars().count() > 64 || n.starts_with('.') || n.chars().any(bad) {
        return Err(format!(
            "nom de monde « {name} » refusé : un seul dossier, sans / \\ : * ? \" < > |, ne commençant pas par un point, 64 caractères au plus"
        ));
    }
    Ok(n.to_string())
}

/// Arguments d'`upload_world`, vérifiés.
#[derive(Debug, Clone)]
pub(crate) struct WorldRequest {
    pub(crate) server_id: i64,
    pub(crate) local_path: PathBuf,
    pub(crate) world_name: Option<String>,
    pub(crate) replace: bool,
    pub(crate) stop: bool,
    pub(crate) set_level_name: bool,
    pub(crate) start_after: bool,
    pub(crate) snapshot: bool,
    pub(crate) dry_run: bool,
}

pub(crate) fn parse_request(args: &Value) -> Result<WorldRequest, String> {
    let server_id = args.get("server_id").and_then(Value::as_i64).ok_or("paramètre `server_id` (entier) requis")?;
    let local = args
        .get("local_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("paramètre `local_path` (dossier du monde ou archive .zip de l'ordinateur, chemin absolu) requis")?;
    let local_path = PathBuf::from(local);
    if !local_path.is_absolute() {
        return Err(format!("`local_path` doit être un chemin absolu (reçu « {local} »)"));
    }
    let world_name = match args.get("world_name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(world_name_ok(s)?),
        Some(_) => return Err("`world_name` doit être un texte".into()),
    };
    Ok(WorldRequest {
        server_id,
        local_path,
        world_name,
        replace: bool_arg(args, "replace")?.unwrap_or(false),
        stop: bool_arg(args, "stop")?.unwrap_or(false),
        set_level_name: bool_arg(args, "set_level_name")?.unwrap_or(false),
        start_after: bool_arg(args, "start_after")?.unwrap_or(false),
        snapshot: bool_arg(args, "snapshot")?.unwrap_or(true),
        dry_run: bool_arg(args, "dry_run")?.unwrap_or(true),
    })
}

/// Ce qui sera fait, et ce qui l'empêche.
#[derive(Debug, Clone)]
pub(crate) struct WorldPlan {
    pub(crate) edition: Edition,
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) current_level: Option<String>,
    pub(crate) state: String,
    pub(crate) need_stop: bool,
    /// Dossiers mis de côté : (chemin actuel, nouveau chemin).
    pub(crate) aside: Vec<(String, String)>,
    pub(crate) set_level: bool,
    pub(crate) problems: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

/// Nom libre `<chemin>.old-<date>` (suffixe `-2`, `-3`… au besoin).
async fn aside_name<W: WorldServer>(w: &W, path: &str, stamp: &str) -> Result<String, String> {
    let base = format!("{path}.old-{stamp}");
    if w.probe(&base).await?.is_none() {
        return Ok(base);
    }
    for n in 2..100 {
        let candidate = format!("{base}-{n}");
        if w.probe(&candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(format!("aucun nom libre pour mettre « {path} » de côté"))
}

/// Vérifications (lectures seulement) et plan.
pub(crate) async fn plan_world<W: WorldServer>(w: &W, req: &WorldRequest, src: &WorldSource, stamp: &str) -> Result<WorldPlan, String> {
    let edition = w.edition().await?;
    let current_level = w.level_name().await;
    let name = match (&req.world_name, &current_level) {
        (Some(n), _) => n.clone(),
        (None, Some(l)) => world_name_ok(l).map_err(|e| format!("level-name du serveur inutilisable ({e}) : donne `world_name`"))?,
        (None, None) => match edition {
            Edition::Java => "world".to_string(),
            Edition::Bedrock => "Bedrock level".to_string(),
        },
    };
    let (target, siblings) = match edition {
        Edition::Java => {
            let t = format!("/{name}");
            let s = vec![format!("{t}_nether"), format!("{t}_the_end")];
            (t, s)
        }
        Edition::Bedrock => (format!("/worlds/{name}"), Vec::new()),
    };
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    match edition {
        Edition::Java if src.has_db && !src.has_region => {
            problems.push("monde Bedrock (base db/) : ce serveur est en Java".to_string())
        }
        Edition::Bedrock if !src.has_db => {
            problems.push("monde Java (sans base db/) : ce serveur est en Bedrock".to_string())
        }
        _ => {}
    }
    let state = match w.state().await {
        Ok(s) => s,
        Err(e) => {
            problems.push(format!("état du serveur illisible ({e}) : rien n'est fait sans le connaître"));
            "unknown".to_string()
        }
    };
    let running = state != "offline" && state != "unknown";
    if running && !req.stop {
        problems.push(format!(
            "serveur démarré (état {state}) : arrête-le d'abord, ou relance avec stop=true (arrêt confirmé par l'utilisateur)"
        ));
    }
    let mut existing = Vec::new();
    for p in std::iter::once(&target).chain(&siblings) {
        if w.probe(p).await?.is_some() {
            existing.push(p.clone());
        }
    }
    let mut aside = Vec::new();
    if !existing.is_empty() {
        if req.replace {
            for p in &existing {
                aside.push((p.clone(), aside_name(w, p, stamp).await?));
            }
        } else {
            problems.push(format!(
                "un monde existe déjà ({}) : relance avec replace=true — il sera renommé en « <nom>.old-{stamp} », jamais supprimé",
                existing.iter().map(|p| format!("« {p} »")).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let set_level = req.set_level_name && current_level.as_deref() != Some(name.as_str());
    if let Some(l) = current_level.as_deref().filter(|l| *l != name && !req.set_level_name) {
        warnings.push(format!(
            "le serveur charge « {l} » (level-name) : set_level_name=true pour qu'il charge « {name} »"
        ));
    }
    let archive = src.archive_bytes.unwrap_or(src.bytes);
    let need = src.bytes.saturating_add(archive);
    match w.free_bytes().await {
        Some(free) if need > free => problems.push(format!(
            "place insuffisante sur le serveur : {} nécessaires (monde {} + archive temporaire), {} libres",
            human(need),
            human(src.bytes),
            human(free)
        )),
        Some(_) => {}
        None => warnings.push("espace disque libre inconnu : non vérifié".into()),
    }
    match src.archive_bytes {
        Some(a) if a > ARCHIVE_MAX_BYTES => problems.push(format!(
            "archive de {} : au-delà du plafond d'envoi de {}",
            human(a),
            human(ARCHIVE_MAX_BYTES)
        )),
        None if src.bytes > ARCHIVE_MAX_BYTES => warnings.push(format!(
            "monde de {} : l'archive compressée doit rester sous {} (vérifié à la compression, avant tout effet)",
            human(src.bytes),
            human(ARCHIVE_MAX_BYTES)
        )),
        _ => {}
    }
    if src.entries.len() > LIST_MAX_ENTRIES {
        warnings.push(format!(
            "plus de {LIST_MAX_ENTRIES} entrées : l'extraction ne sera pas annulable par rollback_changeset (la snapshot préalable reste le filet)"
        ));
    }
    if !req.snapshot {
        warnings.push("sans snapshot préalable (snapshot=false)".into());
    }
    Ok(WorldPlan {
        edition,
        name,
        target,
        current_level,
        need_stop: running && req.stop,
        state,
        aside,
        set_level,
        problems,
        warnings,
    })
}

// --- Exécution ---------------------------------------------------------------------------------

/// Attend l'état `offline` (sondé), annulable.
async fn wait_offline<W: WorldServer>(w: &W) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if matches!(w.state().await.as_deref(), Ok("offline")) {
            return Ok(());
        }
        if mcp_ctx::cancelled() {
            return Err("annulé pendant l'arrêt du serveur".into());
        }
        if start.elapsed() >= STOP_WAIT {
            return Err(format!("serveur toujours pas arrêté après {} s", STOP_WAIT.as_secs()));
        }
        mcp_ctx::progress(start.elapsed().as_secs_f64(), None, "attente de l'arrêt du serveur").await;
        tokio::time::sleep(STATE_POLL).await;
    }
}

/// Ce que l'exécution a fait.
#[derive(Debug, Default)]
struct Done {
    steps: Vec<String>,
    snapshot: Option<String>,
    archive_bytes: u64,
    guard: Vec<String>,
}

impl Done {
    fn note(&mut self, note: Option<String>) {
        if let Some(n) = note.filter(|n| !self.guard.contains(n)) {
            self.guard.push(n);
        }
    }

    /// Erreur après un début d'exécution : ce qui est fait, et comment revenir en arrière.
    fn failure(&self, e: &str) -> String {
        let done = if self.steps.is_empty() { "rien".to_string() } else { self.steps.join(" ; ") };
        let snap = self
            .snapshot
            .as_ref()
            .map(|s| format!(" ; snapshot « {s} » prise avant toute modification"))
            .unwrap_or_default();
        format!("{e}. Déjà fait : {done}. Chaque étape est annulable (list_changesets puis rollback_changeset){snap}.")
    }
}

/// `upload_world` complet sur un serveur quelconque. `stamp` : date des noms (`AAAAMMJJ-HHMMSS`).
pub(crate) async fn upload_world_with<W: WorldServer>(w: &W, req: &WorldRequest, stamp: &str) -> Result<Value, String> {
    const STEPS: f64 = 8.0;
    mcp_ctx::progress(0.0, Some(STEPS), "lecture de la source locale").await;
    let path = req.local_path.clone();
    let src = tokio::task::spawn_blocking(move || inspect_source(&path))
        .await
        .map_err(|e| format!("lecture de la source interrompue : {e}"))??;
    let plan = plan_world(w, req, &src, stamp).await?;
    if req.dry_run {
        return Ok(report(req, &src, &plan, None, stamp));
    }
    if !plan.problems.is_empty() {
        return Err(format!("refusé, rien n'a été modifié : {}", plan.problems.join(" ; ")));
    }
    // Confirmation AVANT tout effet (l'ère sans état rejoue l'appel jusqu'ici).
    if plan.need_stop {
        let message = format!(
            "Arrêter le serveur {} (les joueurs connectés seront déconnectés) pour y installer le monde « {} »{} ?",
            req.server_id,
            plan.name,
            if plan.aside.is_empty() { "" } else { " (l'ancien monde est renommé, pas supprimé)" }
        );
        if mcp_ctx::confirm(&message, None).await == Confirmation::Declined {
            return Err("Refusé par l'utilisateur : rien n'a été modifié.".into());
        }
    }

    mcp_ctx::progress(1.0, Some(STEPS), "compression du monde").await;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp = TempArchive(std::env::temp_dir().join(format!("minestrator-world-{}-{nanos}.zip", std::process::id())));
    let (src_c, out) = (src.clone(), tmp.0.clone());
    let archive_bytes = tokio::task::spawn_blocking(move || build_archive(&src_c, &out))
        .await
        .map_err(|e| format!("compression interrompue : {e}"))??;
    if archive_bytes > ARCHIVE_MAX_BYTES {
        return Err(format!(
            "archive de {} : au-delà du plafond d'envoi de {} ; rien n'a été modifié",
            human(archive_bytes),
            human(ARCHIVE_MAX_BYTES)
        ));
    }
    let local = tmp.0.to_str().ok_or("chemin temporaire non UTF-8")?.to_string();
    if mcp_ctx::cancelled() {
        return Err("annulé avant toute modification".into());
    }

    let id = req.server_id;
    let mut done = Done { archive_bytes, ..Done::default() };
    if plan.need_stop {
        mcp_ctx::progress(2.0, Some(STEPS), "arrêt du serveur").await;
        w.write("power_action", json!({ "server_id": id, "action": "stop" }))
            .await
            .map_err(|e| format!("arrêt refusé ({e}) : rien n'a été modifié"))?;
        done.steps.push("serveur arrêté".into());
        wait_offline(w).await.map_err(|e| done.failure(&e))?;
    }
    if req.snapshot {
        mcp_ctx::progress(3.0, Some(STEPS), "snapshot préalable").await;
        let name = format!("Avant upload_world {stamp}");
        w.snapshot(&name)
            .await
            .map_err(|e| done.failure(&format!("{e} ; aucun fichier n'a été touché (relance plus tard, ou avec snapshot=false)")))?;
        done.steps.push(format!("snapshot « {name} »"));
        done.snapshot = Some(name);
    }
    for (from, to) in &plan.aside {
        if mcp_ctx::cancelled() {
            return Err(done.failure("annulé"));
        }
        let note = w
            .write("rename_path", json!({ "server_id": id, "from": from, "to": to }))
            .await
            .map_err(|e| done.failure(&format!("mise de côté de « {from} » : {e}")))?;
        done.note(note);
        done.steps.push(format!("« {from} » renommé en « {to} »"));
    }
    mcp_ctx::progress(4.0, Some(STEPS), "envoi de l'archive").await;
    let remote_archive = format!("/.minestrator-world-{stamp}.zip");
    let note = w
        .write("upload_file", json!({ "server_id": id, "local_path": local, "remote_path": remote_archive }))
        .await
        .map_err(|e| done.failure(&format!("envoi de l'archive : {e}")))?;
    done.note(note);
    done.steps.push(format!("archive envoyée ({})", human(archive_bytes)));
    // Archive temporaire retirée même après un échec (sa mention de garde n'est pas reprise).
    let cleanup = |path: String| w.write("delete_path", json!({ "server_id": id, "path": path, "is_dir": false }));

    mcp_ctx::progress(5.0, Some(STEPS), "extraction par le serveur").await;
    let extracted = w
        .write("extract_archive", json!({ "server_id": id, "path": remote_archive, "dest_dir": plan.target }))
        .await;
    let check = match &extracted {
        Ok(_) => match w.probe(&remote_path(&plan.target, "level.dat")).await {
            Ok(Some(m)) if !m.is_dir => Ok(()),
            Ok(_) => Err("level.dat absent après l'extraction".to_string()),
            Err(e) => Err(format!("vérification de level.dat : {e}")),
        },
        Err(e) => Err(format!("extraction : {e}")),
    };
    if let Err(e) = check {
        let cleaned = cleanup(remote_archive.clone()).await.is_ok();
        if let Ok(note) = extracted {
            done.note(note);
            done.steps.push(format!("archive extraite dans « {} »", plan.target));
        }
        let left = if cleaned { String::new() } else { format!(" ; archive temporaire « {remote_archive} » restée sur le serveur") };
        return Err(done.failure(&format!("{e}{left}")));
    }
    if let Ok(note) = extracted {
        done.note(note);
    }
    done.steps.push(format!("monde extrait dans « {} », level.dat présent", plan.target));
    mcp_ctx::progress(6.0, Some(STEPS), "nettoyage").await;
    match cleanup(remote_archive.clone()).await {
        Ok(_) => done.steps.push("archive temporaire supprimée".into()),
        Err(e) => done.guard.push(format!("archive temporaire « {remote_archive} » non supprimée ({e}) : à retirer avec delete_path")),
    }
    if plan.set_level {
        let note = w
            .write("set_property", json!({ "server_id": id, "property": "level-name", "value": plan.name }))
            .await
            .map_err(|e| done.failure(&format!("réglage de level-name : {e}")))?;
        done.note(note);
        done.steps.push(format!("level-name = « {} »", plan.name));
    }
    if req.start_after {
        mcp_ctx::progress(7.0, Some(STEPS), "démarrage").await;
        w.write("power_action", json!({ "server_id": id, "action": "start" }))
            .await
            .map_err(|e| done.failure(&format!("démarrage refusé : {e}")))?;
        done.steps.push("démarrage demandé".into());
    }
    mcp_ctx::progress(STEPS, Some(STEPS), "monde installé").await;
    Ok(report(req, &src, &plan, Some(&done), stamp))
}

/// Étapes prévues (simulation).
fn planned_steps(req: &WorldRequest, src: &WorldSource, plan: &WorldPlan, stamp: &str) -> Vec<String> {
    let mut steps = Vec::new();
    if plan.need_stop {
        steps.push("confirmation de l'utilisateur, puis arrêt du serveur et attente".into());
    }
    if req.snapshot {
        steps.push(format!("snapshot « Avant upload_world {stamp} », attendue (sans elle, rien n'est modifié)"));
    }
    for (from, to) in &plan.aside {
        steps.push(format!("renommer « {from} » en « {to} » (rien n'est supprimé)"));
    }
    steps.push(format!("compresser le monde ({} fichiers, {}) et envoyer l'archive", src.files, human(src.bytes)));
    steps.push(format!("extraire par le serveur dans « {} », puis vérifier level.dat", plan.target));
    steps.push("supprimer l'archive temporaire".into());
    if plan.set_level {
        steps.push(format!("régler level-name sur « {} »", plan.name));
    }
    if req.start_after {
        steps.push("démarrer le serveur".into());
    }
    steps
}

fn report(req: &WorldRequest, src: &WorldSource, plan: &WorldPlan, done: Option<&Done>, stamp: &str) -> Value {
    let status = match (done, plan.problems.is_empty()) {
        (Some(_), _) => "monde installé",
        (None, true) => "simulation : prêt, rien n'a été modifié",
        (None, false) => "simulation : refusé tel quel (voir problems), rien n'a été modifié",
    };
    let level_after = if plan.set_level { Some(plan.name.clone()) } else { plan.current_level.clone() };
    let mut out = json!({
        "server_id": req.server_id,
        "dry_run": req.dry_run,
        "status": status,
        "edition": match plan.edition { Edition::Java => "java", Edition::Bedrock => "bedrock" },
        "world": plan.name,
        "target": plan.target,
        "server_state": plan.state,
        "level_name": { "current": plan.current_level, "after": level_after },
        "source": {
            "kind": match src.kind { SourceKind::Dir => "dossier", SourceKind::Zip => "archive zip" },
            "files": src.files,
            "bytes": src.bytes,
            "archive_bytes": src.archive_bytes,
            "root_stripped": (!src.strip.is_empty()).then(|| src.strip.trim_end_matches('/').to_string()),
            "ignored": src.ignored,
        },
        "aside": plan.aside.iter().map(|(from, to)| json!({ "from": from, "to": to })).collect::<Vec<_>>(),
        "problems": plan.problems,
        "warnings": plan.warnings,
        "limits": {
            "archive_max_bytes": ARCHIVE_MAX_BYTES,
            "world_max_files": WORLD_MAX_FILES,
            "world_max_bytes": WORLD_MAX_BYTES,
        },
    });
    match done {
        None => {
            out["steps"] = json!(planned_steps(req, src, plan, stamp));
            out["next"] = json!(if plan.problems.is_empty() {
                "Montre ce plan à l'utilisateur, puis relance avec dry_run=false pour l'appliquer."
            } else {
                "Corrige d'abord les problèmes listés (replace, stop, world_name…), puis relance."
            });
        }
        Some(d) => {
            out["steps"] = json!(d.steps);
            out["snapshot"] = json!(d.snapshot);
            out["uploaded_archive_bytes"] = json!(d.archive_bytes);
            if !d.guard.is_empty() {
                out["guard"] = json!(d.guard);
            }
            out["next"] = json!(if req.start_after {
                "Vérifie le démarrage (wait_for state=running), puis le monde (world_info)."
            } else {
                "Démarre le serveur (power_action start), vérifie avec wait_for state=running puis world_info. Chaque étape est annulable : list_changesets puis rollback_changeset."
            });
        }
    }
    out
}

/// Outil MCP `upload_world`.
pub(crate) async fn upload_world_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = parse_request(args)?;
    let side = CoreSide { core, id: req.server_id };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let report = upload_world_with(&side, &req, &stamp).await?;
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    Ok(core.redact_ai(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_guard::undo::{Meta, Remote};
    use crate::local_sync::testing::{block, GuardedFake, TempDir};
    use std::io::Write;

    /// Serveur Minecraft simulé : fichiers et garde (`GuardedFake`), édition, place libre.
    struct World {
        g: GuardedFake,
        edition: Edition,
        free: Option<u64>,
    }

    impl World {
        fn new(edition: Edition) -> Self {
            World { g: GuardedFake::new(), edition, free: Some(1 << 30) }
        }
    }

    impl ServerSide for World {
        async fn probe(&self, path: &str) -> Result<Option<Meta>, String> {
            self.g.probe(path).await
        }
        async fn walk(&self, root: &str, recursive: bool, max: usize) -> Result<(Vec<Meta>, bool), String> {
            self.g.walk(root, recursive, max).await
        }
        async fn read(&self, path: &str, cap: u64) -> Result<Vec<u8>, String> {
            self.g.read(path, cap).await
        }
        async fn write(&self, tool: &'static str, args: Value) -> Result<Option<String>, String> {
            self.g.write(tool, args).await
        }
    }

    impl WorldServer for World {
        async fn edition(&self) -> Result<Edition, String> {
            Ok(self.edition)
        }
        async fn state(&self) -> Result<String, String> {
            Ok(self.g.state.lock().unwrap().clone())
        }
        async fn level_name(&self) -> Option<String> {
            self.g.s.property("level-name").await.ok().flatten()
        }
        async fn free_bytes(&self) -> Option<u64> {
            self.free
        }
        async fn snapshot(&self, name: &str) -> Result<(), String> {
            self.g.calls.lock().unwrap().push(("snapshot".into(), json!({ "name": name })));
            Ok(())
        }
    }

    fn zip_of(path: &Path, entries: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, content) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, zip::write::SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(content.as_bytes()).unwrap();
            }
        }
        w.finish().unwrap();
    }

    fn names_in(zip_path: &Path) -> Vec<(String, String)> {
        let mut ar = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        let mut out = Vec::new();
        for i in 0..ar.len() {
            let mut f = ar.by_index(i).unwrap();
            let mut s = String::new();
            if !f.is_dir() {
                std::io::Read::read_to_string(&mut f, &mut s).unwrap();
            }
            out.push((f.name().unwrap().into_owned(), s));
        }
        out.sort();
        out
    }

    #[test]
    fn the_world_root_is_found_or_refused() {
        assert_eq!(detect_root(&["level.dat", "region/r.0.0.mca"]).unwrap(), "");
        assert_eq!(detect_root(&["Monde/level.dat", "Monde/region/r.0.0.mca", "lisez-moi.txt"]).unwrap(), "Monde/");
        assert!(detect_root(&["a/b/level.dat"]).unwrap_err().contains("trop profond"));
        assert!(detect_root(&["region/r.0.0.mca"]).unwrap_err().contains("aucun level.dat"));
        let e = detect_root(&["world/level.dat", "world_nether/level.dat"]).unwrap_err();
        assert!(e.contains("plusieurs mondes") && e.contains("world_nether"), "{e}");
    }

    #[test]
    fn world_names_are_single_safe_folders() {
        assert_eq!(world_name_ok(" Survie 2 ").unwrap(), "Survie 2");
        assert_eq!(world_name_ok("Bedrock level").unwrap(), "Bedrock level");
        for bad in ["", "a/b", "..", ".cache", "c:x", "x\\y", "a*"] {
            assert!(world_name_ok(bad).is_err(), "{bad}");
        }
        assert!(world_name_ok(&"x".repeat(65)).is_err());
    }

    #[test]
    fn requests_default_to_a_safe_simulation() {
        let abs = if cfg!(windows) { "C:/mondes/survie" } else { "/mondes/survie" };
        let r = parse_request(&json!({ "server_id": 3, "local_path": abs })).unwrap();
        assert!(r.dry_run && r.snapshot && !r.replace && !r.stop && !r.set_level_name && !r.start_after);
        assert!(r.world_name.is_none());
        assert!(parse_request(&json!({ "server_id": 3, "local_path": "survie" })).unwrap_err().contains("absolu"));
        assert!(parse_request(&json!({ "server_id": 3, "local_path": abs, "world_name": "../x" })).is_err());
        assert!(parse_request(&json!({ "server_id": 3, "local_path": abs, "stop": "oui" })).unwrap_err().contains("booléen"));
    }

    #[test]
    fn a_folder_with_an_extra_root_is_packed_at_the_archive_root() {
        let t = TempDir::new("world-dir");
        t.put("Monde/level.dat", "niveau");
        t.put("Monde/region/r.0.0.mca", "région");
        t.put("Monde/session.lock", "verrou");
        t.put("Monde/DIM-1/region/r.0.0.mca", "nether");
        let src = inspect_source(&t.0).unwrap();
        assert_eq!(src.kind, SourceKind::Dir);
        assert_eq!(src.strip, "Monde/");
        assert_eq!(src.files, 3, "session.lock écarté");
        assert!(src.has_region && !src.has_db);
        let out = t.0.join("out.zip");
        build_archive(&src, &out).unwrap();
        let files: Vec<(String, String)> = names_in(&out).into_iter().filter(|(n, _)| !n.ends_with('/')).collect();
        assert_eq!(
            files,
            [
                ("DIM-1/region/r.0.0.mca".to_string(), "nether".to_string()),
                ("level.dat".into(), "niveau".into()),
                ("region/r.0.0.mca".into(), "région".into()),
            ]
        );
    }

    #[test]
    fn a_zip_is_recopied_without_its_extra_root_and_dangerous_zips_are_refused() {
        let t = TempDir::new("world-zip");
        let src_zip = t.0.join("monde.zip");
        zip_of(&src_zip, &[("Monde/", ""), ("Monde/level.dat", "niveau"), ("Monde/region/r.0.0.mca", "région"), ("__MACOSX/._level.dat", "x")]);
        let src = inspect_source(&src_zip).unwrap();
        assert_eq!((src.kind, src.strip.as_str(), src.files), (SourceKind::Zip, "Monde/", 2));
        assert!(src.archive_bytes.is_some() && src.ignored == 1);
        let out = t.0.join("out.zip");
        build_archive(&src, &out).unwrap();
        let files: Vec<(String, String)> = names_in(&out).into_iter().filter(|(n, _)| !n.ends_with('/')).collect();
        assert_eq!(files, [("level.dat".to_string(), "niveau".to_string()), ("region/r.0.0.mca".into(), "région".into())]);

        let evil = t.0.join("evil.zip");
        zip_of(&evil, &[("level.dat", "x"), ("../../.ssh/authorized_keys", "clé")]);
        assert!(inspect_source(&evil).unwrap_err().contains("dangereuse"));
        let none = t.0.join("vide.zip");
        zip_of(&none, &[("readme.txt", "x")]);
        assert!(inspect_source(&none).unwrap_err().contains("level.dat"));
        t.put("monde.rar", "x");
        assert!(inspect_source(&t.0.join("monde.rar")).unwrap_err().contains("non prise en charge"));
    }

    fn request(path: &Path, extra: Value) -> WorldRequest {
        let mut args = json!({ "server_id": 4, "local_path": path.to_str().unwrap() });
        for (k, v) in extra.as_object().unwrap() {
            args[k] = v.clone();
        }
        parse_request(&args).unwrap()
    }

    fn local_world(tag: &str) -> TempDir {
        let t = TempDir::new(tag);
        t.put("Nouveau/level.dat", "nouveau niveau");
        t.put("Nouveau/region/r.0.0.mca", "nouvelle région");
        t
    }

    #[test]
    fn a_simulation_reports_what_blocks_and_changes_nothing() {
        block(async {
            let t = local_world("world-dry");
            let w = World::new(Edition::Java);
            *w.g.state.lock().unwrap() = "running".into();
            w.g.s.put("/world/level.dat", "ancien");
            w.g.s.props.lock().unwrap().insert("level-name".into(), "world".into());
            let before = w.g.s.snapshot();
            let r = upload_world_with(&w, &request(&t.0.join("Nouveau"), json!({})), "20261120-101500").await.unwrap();
            assert!(w.g.tools().is_empty(), "aucun effet en simulation");
            assert_eq!(w.g.s.snapshot(), before);
            let problems = r["problems"].to_string();
            assert!(problems.contains("stop=true") && problems.contains("replace=true"), "{problems}");
            assert!(r["status"].as_str().unwrap().contains("refusé"));
            assert_eq!(r["target"], "/world");
            // Sans dry_run : refusé avant tout effet.
            let e = upload_world_with(&w, &request(&t.0.join("Nouveau"), json!({ "dry_run": false })), "x").await.unwrap_err();
            assert!(e.contains("rien n'a été modifié"), "{e}");
            assert!(w.g.tools().is_empty());
            // Monde Bedrock sur un serveur Java : refusé.
            let b = TempDir::new("world-bedrock");
            b.put("level.dat", "x");
            b.put("db/CURRENT", "x");
            let w = World::new(Edition::Java);
            let r = upload_world_with(&w, &request(&b.0, json!({})), "x").await.unwrap();
            assert!(r["problems"].to_string().contains("Bedrock"));
            // Place insuffisante : refusé.
            let mut w = World::new(Edition::Java);
            w.free = Some(4);
            let r = upload_world_with(&w, &request(&t.0, json!({})), "x").await.unwrap();
            assert!(r["problems"].to_string().contains("place insuffisante"));
            assert_eq!(r["source"]["root_stripped"], "Nouveau");
        });
    }

    /// Bout en bout : arrêt, snapshot, ancien monde et dimensions renommés, envoi, extraction,
    /// level-name, démarrage ; puis annulation de toutes les étapes par la garde.
    #[test]
    fn a_world_replaces_the_old_one_which_is_kept_then_everything_undoes() {
        block(async {
            let t = local_world("world-apply");
            let w = World::new(Edition::Java);
            *w.g.state.lock().unwrap() = "running".into();
            w.g.s.put("/world/level.dat", "ancien");
            w.g.s.put("/world_nether/DIM-1/region/r.0.0.mca", "ancien nether");
            w.g.s.put("/plugins/A.jar", "jar");
            w.g.s.props.lock().unwrap().insert("level-name".into(), "world".into());
            let orig = w.g.s.snapshot();
            let stamp = "20261120-101500";
            let req = request(
                &t.0,
                json!({ "dry_run": false, "stop": true, "replace": true, "world_name": "survie", "set_level_name": true, "start_after": true }),
            );
            let r = upload_world_with(&w, &req, stamp).await.unwrap();
            assert_eq!(r["status"], "monde installé", "{r}");
            assert_eq!(
                w.g.tools(),
                ["power_action", "snapshot", "upload_file", "extract_archive", "delete_path", "set_property", "power_action"],
                "monde « survie » absent : rien à mettre de côté"
            );
            assert_eq!(w.g.s.text("/survie/level.dat").unwrap(), "nouveau niveau");
            assert_eq!(w.g.s.text("/survie/region/r.0.0.mca").unwrap(), "nouvelle région");
            assert!(w.g.s.exists("/world/level.dat"), "l'ancien monde par défaut reste en place");
            assert!(!w.g.s.exists(&format!("/.minestrator-world-{stamp}.zip")), "archive temporaire retirée");
            assert_eq!(w.g.s.property("level-name").await.unwrap().as_deref(), Some("survie"));
            assert_eq!(*w.g.state.lock().unwrap(), "running");

            // Remplacement du monde courant : lui et sa dimension sont renommés, jamais supprimés.
            w.g.calls.lock().unwrap().clear();
            let req = request(&t.0, json!({ "dry_run": false, "replace": true, "stop": true }));
            let r = upload_world_with(&w, &req, stamp).await.unwrap();
            assert_eq!(r["world"], "survie");
            assert_eq!(r["aside"].as_array().unwrap().len(), 1);
            assert_eq!(w.g.s.text(&format!("/survie.old-{stamp}/level.dat")).unwrap(), "nouveau niveau");
            assert_eq!(w.g.s.text("/survie/level.dat").unwrap(), "nouveau niveau");
            assert!(w.g.tools().contains(&"rename_path".to_string()));

            // Annulation de toutes les étapes : fichiers et réglage d'origine.
            w.g.undo_all().await;
            assert_eq!(w.g.s.snapshot(), orig);
            assert_eq!(w.g.s.property("level-name").await.unwrap().as_deref(), Some("world"));
        });
    }

    #[test]
    fn replacing_the_current_java_world_moves_its_dimensions_aside() {
        block(async {
            let t = local_world("world-dims");
            let w = World::new(Edition::Java);
            w.g.s.put("/world/level.dat", "ancien");
            w.g.s.put("/world_nether/level.dat", "nether");
            w.g.s.put("/world_the_end/level.dat", "end");
            w.g.s.props.lock().unwrap().insert("level-name".into(), "world".into());
            let stamp = "20261120-111111";
            w.g.s.put(&format!("/world.old-{stamp}/x"), "déjà pris");
            let r = upload_world_with(&w, &request(&t.0, json!({ "dry_run": false, "replace": true, "snapshot": false })), stamp)
                .await
                .unwrap();
            let aside: Vec<String> = r["aside"].as_array().unwrap().iter().map(|a| a["to"].as_str().unwrap().to_string()).collect();
            assert_eq!(
                aside,
                [format!("/world.old-{stamp}-2"), format!("/world_nether.old-{stamp}"), format!("/world_the_end.old-{stamp}")]
            );
            assert_eq!(w.g.s.text(&format!("/world.old-{stamp}-2/level.dat")).unwrap(), "ancien");
            assert_eq!(w.g.s.text(&format!("/world_nether.old-{stamp}/level.dat")).unwrap(), "nether");
            assert!(!w.g.s.exists("/world_nether") && !w.g.s.exists("/world_the_end"));
            assert_eq!(w.g.s.text("/world/level.dat").unwrap(), "nouveau niveau");
            assert!(!w.g.tools().contains(&"snapshot".to_string()) && !w.g.tools().contains(&"power_action".to_string()));
        });
    }

    #[test]
    fn a_failed_extraction_reports_what_was_done_and_cleans_up() {
        block(async {
            let w = World::new(Edition::Bedrock);
            w.g.s.put("/worlds/Bedrock level/db/CURRENT", "ancien");
            let b = TempDir::new("world-fail-bedrock");
            b.put("level.dat", "x");
            b.put("db/CURRENT", "nouveau");
            *w.g.fail_on.lock().unwrap() = Some("extract_archive-impossible".into());
            // Panne simulée à l'extraction : le chemin cible contient le marqueur.
            let req = request(&b.0, json!({ "dry_run": false, "replace": true, "world_name": "extract_archive-impossible" }));
            let e = upload_world_with(&w, &req, "20261120-121212").await.unwrap_err();
            assert!(e.contains("extraction") && e.contains("Déjà fait") && e.contains("rollback_changeset"), "{e}");
            assert!(!w.g.s.exists("/.minestrator-world-20261120-121212.zip"), "archive temporaire retirée");
            assert_eq!(w.g.s.text("/worlds/Bedrock level/db/CURRENT").unwrap(), "ancien", "autre monde intact");
        });
    }
}
