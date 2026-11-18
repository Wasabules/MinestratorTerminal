//! Opérations sur **plusieurs serveurs** du compte, exposées comme outils MCP (« Plusieurs
//! serveurs ») : copier des fichiers d'un serveur à un autre (`copy_between_servers`), comparer
//! des fichiers ou des dossiers (`diff_files`), retrouver un joueur partout (`find_player`),
//! envoyer une même commande à plusieurs serveurs (`broadcast_command`).
//!
//! Découpage : la partie **pure** (validation des demandes, plan de copie et collisions, choix de
//! la stratégie, comparaison de listings, diff texte, fusion des joueurs, choix des cibles d'une
//! commande, rendus) est séparée du réseau et testée sans lui. Le réseau réutilise les briques
//! existantes : flux SFTP (`sftp.rs`), compression et extraction faites par le serveur
//! (`server_files.rs`), joueurs déjà venus (`players.rs`, cache d'une minute), commande console
//! par l'endpoint REST (`api.rs` : le WebSocket reste en lecture seule).
//!
//! Garde-fous : les deux outils modifiants sont dans `WRITE_TOOLS` (soumis à `allow_writes`) **et**
//! dans `INFRA_TOOLS` (jamais proposés au Copilote, qui lit du contenu non fiable et n'agit que sur
//! un serveur). Les sorties de lecture passent par l'anonymisation IA (`redact.rs`).

use crate::error::{Error, Result};
use crate::models::ServerListItem;
use crate::players::{KnownPlayer, Roster};
use crate::server_files::{self, human_bytes, CompressOutcome, DiskSpace};
use crate::sftp::{self, EntryKind, SftpConn, TreeEntry};
use crate::Core;
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Chemins source au plus, par copie.
pub(crate) const MAX_SOURCES: usize = 50;
/// Entrées explorées au plus pour planifier une copie (sources, puis destination).
pub(crate) const MAX_WALK_ENTRIES: usize = 200_000;
/// Copie fichier par fichier : nombre de fichiers et volume au plus.
pub(crate) const MAX_COPY_FILES: usize = 5_000;
pub(crate) const MAX_COPY_BYTES: u64 = 2 * GIB;
/// Archive transférée d'un serveur à un autre : taille au plus (elle transite par la machine).
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 8 * GIB;
/// Au-delà (fichiers ou octets), le choix automatique passe par une archive.
const AUTO_ARCHIVE_FILES: usize = 300;
const AUTO_ARCHIVE_BYTES: u64 = 200 * MIB;
/// Copies simultanées (fichier par fichier) sur les mêmes sessions SFTP.
const COPY_CONCURRENCY: usize = 6;
/// Échecs tolérés avant d'interrompre une copie fichier par fichier.
const MAX_COPY_ERRORS: usize = 20;
/// Fichiers ignorés détaillés dans un rapport (le total est toujours donné).
const MAX_SKIPPED_SHOWN: usize = 50;
/// Requêtes simultanées vers l'API pour plusieurs serveurs (état, joueurs, commandes).
const API_CONCURRENCY: usize = 4;
/// Comparaison : taille maximale d'un fichier comparé ligne à ligne.
pub(crate) const MAX_DIFF_FILE_BYTES: u64 = MIB;
/// Lignes de diff renvoyées au plus.
const MAX_DIFF_LINES: usize = 400;
/// Entrées comparées au plus, par côté, pour des dossiers.
pub(crate) const MAX_DIFF_ENTRIES: usize = 5_000;
/// Entrées détaillées au plus par catégorie (ajoutés, absents, modifiés…).
const MAX_DIFF_LISTED: usize = 100;
/// Serveurs ciblés au plus par une commande groupée.
pub(crate) const MAX_BROADCAST: usize = 20;
/// Longueur maximale d'une commande groupée.
const MAX_COMMAND_CHARS: usize = 1_000;
/// Joueurs détaillés au plus par `find_player`.
pub(crate) const MAX_PLAYERS: usize = 25;
/// Préfixe du dossier de transit d'une copie par archive, côté cible (supprimé ensuite).
pub(crate) const STAGING_PREFIX: &str = "minestrator-temp-copy-";

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

/// Suffixe unique (horodatage + compteur) des noms temporaires.
fn stamp() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

// --- Arguments des outils ----------------------------------------------------------

fn arg_i64(args: &Value, key: &str) -> Result<i64> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("paramètre `{key}` (entier) requis")))
}

fn arg_opt_i64(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| invalid(format!("paramètre `{key}` : entier attendu"))),
    }
}

fn arg_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("paramètre `{key}` (texte) requis")))
}

fn arg_opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Booléen optionnel, lu avec tolérance (`true`, `"true"`, `1`) ; une valeur incompréhensible est
/// une erreur plutôt qu'un défaut silencieux (`overwrite` en dépend).
fn arg_bool(args: &Value, key: &str, default: bool) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::Number(n)) if n.as_i64() == Some(0) || n.as_i64() == Some(1) => Ok(n.as_i64() == Some(1)),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "oui" | "yes" => Ok(true),
            "false" | "0" | "non" | "no" => Ok(false),
            _ => Err(invalid(format!("paramètre `{key}` : booléen attendu"))),
        },
        Some(_) => Err(invalid(format!("paramètre `{key}` : booléen attendu"))),
    }
}

/// Liste de chaînes (tableau, ou chaîne seule).
fn arg_str_list(args: &Value, key: &str) -> Result<Vec<String>> {
    match args.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| invalid(format!("`{key}` : chemins en texte attendus"))))
            .collect(),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        _ => Err(invalid(format!("paramètre `{key}` (tableau de chemins) requis"))),
    }
}

/// Liste d'identifiants de serveurs (absente : `None`). Un élément non entier est une erreur :
/// une commande ne doit pas partir vers une liste devinée.
fn arg_id_list(args: &Value, key: &str) -> Result<Option<Vec<i64>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_i64().ok_or_else(|| invalid(format!("`{key}` : identifiants entiers attendus"))))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(Value::Number(n)) => n.as_i64().map(|id| Some(vec![id])).ok_or_else(|| invalid(format!("`{key}` : entier attendu"))),
        Some(_) => Err(invalid(format!("`{key}` : tableau d'identifiants attendu"))),
    }
}

// --- Chemins -------------------------------------------------------------------------

/// Chemin SFTP absolu normalisé (`/a/b`, `/` pour la racine). Refuse `..`, les caractères de
/// contrôle et un chemin vide.
pub(crate) fn normalize_path(p: &str) -> Result<String> {
    let p = p.trim();
    if p.is_empty() {
        return Err(invalid("chemin vide"));
    }
    if p.chars().any(char::is_control) {
        return Err(invalid(format!("chemin refusé (caractère de contrôle) : {p:?}")));
    }
    if p.replace('\\', "/").split('/').any(|s| s == "..") {
        return Err(invalid(format!("chemin refusé (« .. ») : {p}")));
    }
    Ok(server_files::sftp_path(p))
}

/// Dernier segment d'un chemin normalisé.
fn base_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn join(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// `path` est-il `ancestor` lui-même, ou situé dessous ?
fn is_within(path: &str, ancestor: &str) -> bool {
    ancestor == "/" || path == ancestor || path.starts_with(&format!("{ancestor}/"))
}

fn parent_of(p: &str) -> &str {
    match p.rfind('/') {
        None | Some(0) => "/",
        Some(i) => &p[..i],
    }
}

/// Chemin relatif d'une entrée sous `root` (`""` pour `root` lui-même).
fn rel_of<'a>(root: &str, path: &'a str) -> &'a str {
    if root == "/" {
        path.trim_start_matches('/')
    } else {
        path.strip_prefix(root).map(|r| r.trim_start_matches('/')).unwrap_or(path)
    }
}

/// Destination, côté cible, d'une source copiée dans `target_dir`.
pub(crate) fn dest_root(target_dir: &str, source_root: &str) -> String {
    join(target_dir, base_name(source_root))
}

/// Le dossier de transit d'une copie par archive (seul dossier que l'outil supprime en entier).
pub(crate) fn is_staging_dir(path: &str) -> bool {
    base_name(path).starts_with(STAGING_PREFIX) && !path.split('/').any(|s| s == "..")
}

// --- Copie : demande -------------------------------------------------------------------

/// Stratégie de copie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    /// Choix selon la taille et la faisabilité.
    Auto,
    /// Fichier par fichier, en flux SFTP de serveur à serveur.
    Files,
    /// Compression par le serveur source, transfert de l'archive, extraction par la cible.
    Archive,
}

impl Strategy {
    fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "files" | "file" | "fichiers" => Ok(Self::Files),
            "archive" | "zip" => Ok(Self::Archive),
            other => Err(invalid(format!("`strategy` inconnue : « {other} » (auto, files ou archive)"))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Files => "files",
            Self::Archive => "archive",
        }
    }
}

/// Demande de copie validée (chemins normalisés).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CopyRequest {
    pub source_server: i64,
    pub sources: Vec<String>,
    pub target_server: i64,
    pub target_dir: String,
    pub overwrite: bool,
    pub strategy: Strategy,
}

impl CopyRequest {
    fn same_server(&self) -> bool {
        self.source_server == self.target_server
    }
}

/// Lit et valide les arguments de `copy_between_servers`, avant tout appel réseau.
pub(crate) fn copy_request(args: &Value) -> Result<CopyRequest> {
    let raw = arg_str_list(args, "source_paths")?;
    let req = CopyRequest {
        source_server: arg_i64(args, "source_server_id")?,
        sources: raw.iter().map(|p| normalize_path(p)).collect::<Result<Vec<_>>>()?,
        target_server: arg_i64(args, "target_server_id")?,
        target_dir: normalize_path(&arg_str(args, "target_dir")?)?,
        overwrite: arg_bool(args, "overwrite", false)?,
        strategy: Strategy::parse(&arg_opt_str(args, "strategy").unwrap_or_default())?,
    };
    validate_copy(&req)?;
    Ok(req)
}

fn validate_copy(req: &CopyRequest) -> Result<()> {
    if req.sources.is_empty() {
        return Err(invalid("`source_paths` est vide : indique au moins un fichier ou dossier à copier."));
    }
    if req.sources.len() > MAX_SOURCES {
        return Err(invalid(format!("{} chemins source : {MAX_SOURCES} au plus par copie.", req.sources.len())));
    }
    let mut names = HashSet::new();
    for (i, s) in req.sources.iter().enumerate() {
        if s == "/" {
            return Err(invalid("La racine « / » ne se copie pas en bloc : liste ses éléments (ex. /world, /plugins)."));
        }
        if let Some(other) = req.sources[..i].iter().find(|o| is_within(s, o) || is_within(o, s)) {
            return Err(invalid(format!("« {s} » et « {other} » se recouvrent : garde le dossier englobant seulement.")));
        }
        if !names.insert(base_name(s)) {
            return Err(invalid(format!(
                "Deux sources s'appellent « {} » : elles arriveraient au même endroit dans « {} ».",
                base_name(s),
                req.target_dir
            )));
        }
        if req.same_server() {
            if dest_root(&req.target_dir, s) == *s {
                return Err(invalid(format!("Copie de « {s} » sur lui-même (même serveur, même dossier).")));
            }
            if is_within(&req.target_dir, s) {
                return Err(invalid(format!(
                    "Le dossier cible « {} » est dans la source « {s} » : copie d'un dossier dans lui-même refusée.",
                    req.target_dir
                )));
            }
        }
    }
    Ok(())
}

// --- Copie : plan et collisions ---------------------------------------------------------

/// Une source à copier, parcourue.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceTree {
    /// Chemin de la source (normalisé).
    pub root: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Descendants d'un dossier (chemins complets) ; vide pour un fichier.
    pub entries: Vec<TreeEntry>,
}

impl SourceTree {
    /// Fichiers, octets et dossiers (la source comprise si c'en est un).
    fn totals(&self) -> (usize, u64, usize) {
        let all = std::iter::once((self.kind, self.size)).chain(self.entries.iter().map(|e| (e.kind, e.size)));
        let (mut files, mut bytes, mut dirs) = (0, 0, 0);
        for (kind, size) in all {
            match kind {
                EntryKind::File => {
                    files += 1;
                    bytes += size;
                }
                EntryKind::Dir => dirs += 1,
                EntryKind::Other => {}
            }
        }
        (files, bytes, dirs)
    }
}

/// Ce qui existe déjà côté cible : chemin → (nature, taille).
pub(crate) type Existing = HashMap<String, (EntryKind, u64)>;

/// Un fichier à copier (ou à déplacer, pour la mise en place d'une archive).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileCopy {
    pub src: String,
    pub dst: String,
    pub size: u64,
    /// Un fichier existe déjà à `dst` et sera remplacé (`overwrite=true`).
    pub replace: bool,
}

/// Fichier ou dossier non copié, et pourquoi.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Skipped {
    pub path: String,
    pub reason: String,
}

/// Plan d'une copie : dossiers à créer (parents d'abord), fichiers, ignorés.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CopyPlan {
    pub dirs: Vec<String>,
    pub files: Vec<FileCopy>,
    pub skipped: Vec<Skipped>,
    /// Octets à transférer.
    pub bytes: u64,
    /// Place supplémentaire occupée côté cible (un remplacement compte pour sa différence).
    pub extra_bytes: u64,
}

/// Plan d'une copie de `trees` dans `target_dir`, d'après ce qui existe déjà côté cible. Règles :
/// - un fichier existant n'est remplacé qu'avec `overwrite` ;
/// - un dossier n'est jamais remplacé par un fichier, ni un fichier par un dossier (le contenu
///   d'un dossier qui ne peut pas être créé est ignoré) ;
/// - liens symboliques et fichiers spéciaux ne sont pas copiés.
pub(crate) fn plan_copy(trees: &[SourceTree], target_dir: &str, existing: &Existing, overwrite: bool) -> CopyPlan {
    let mut plan = CopyPlan::default();
    for tree in trees {
        let dst_root = dest_root(target_dir, &tree.root);
        let to_dst = |src: &str| {
            let rel = rel_of(&tree.root, src);
            if rel.is_empty() {
                dst_root.clone()
            } else {
                join(&dst_root, rel)
            }
        };
        let mut items: Vec<(String, String, EntryKind, u64)> =
            vec![(tree.root.clone(), dst_root.clone(), tree.kind, tree.size)];
        items.extend(tree.entries.iter().map(|e| (e.path.clone(), to_dst(&e.path), e.kind, e.size)));
        // Un dossier est traité avant son contenu (tri stable par profondeur).
        items.sort_by_key(|(_, dst, ..)| dst.matches('/').count());
        let mut blocked: Vec<String> = Vec::new();
        for (src, dst, kind, size) in items {
            if let Some(b) = blocked.iter().find(|b| is_within(&dst, b)) {
                plan.skipped.push(Skipped {
                    path: src,
                    reason: format!("dossier « {b} » non créable côté cible"),
                });
                continue;
            }
            match (kind, existing.get(&dst)) {
                (EntryKind::Other, _) => plan.skipped.push(Skipped {
                    path: src,
                    reason: "lien symbolique ou fichier spécial : non copié".into(),
                }),
                (EntryKind::Dir, None) => plan.dirs.push(dst),
                (EntryKind::Dir, Some((EntryKind::Dir, _))) => {}
                (EntryKind::Dir, Some(_)) => {
                    plan.skipped.push(Skipped {
                        path: src,
                        reason: "un fichier porte déjà ce nom côté cible : dossier non créé, contenu ignoré".into(),
                    });
                    blocked.push(dst);
                }
                (EntryKind::File, None) => {
                    plan.bytes += size;
                    plan.extra_bytes += size;
                    plan.files.push(FileCopy { src, dst, size, replace: false });
                }
                (EntryKind::File, Some((EntryKind::File, old))) if overwrite => {
                    plan.bytes += size;
                    plan.extra_bytes += size.saturating_sub(*old);
                    plan.files.push(FileCopy { src, dst, size, replace: true });
                }
                (EntryKind::File, Some((EntryKind::File, _))) => plan.skipped.push(Skipped {
                    path: src,
                    reason: "existe déjà côté cible (overwrite=false)".into(),
                }),
                (EntryKind::File, Some(_)) => plan.skipped.push(Skipped {
                    path: src,
                    reason: "un dossier (ou un fichier spécial) porte ce nom côté cible : jamais remplacé".into(),
                }),
            }
        }
    }
    plan
}

/// Plafonds d'une copie fichier par fichier.
fn check_file_caps(files: usize, bytes: u64) -> Result<()> {
    if files > MAX_COPY_FILES || bytes > MAX_COPY_BYTES {
        return Err(invalid(format!(
            "Copie fichier par fichier trop volumineuse : {files} fichier(s), {} (plafonds : {MAX_COPY_FILES} fichiers, {}). Copie par archive (`strategy=archive`) ou sélection plus petite.",
            human_bytes(bytes),
            human_bytes(MAX_COPY_BYTES)
        )));
    }
    Ok(())
}

/// Ce qui décide de la stratégie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrategyFacts {
    /// Fichiers et octets des sources.
    pub files: usize,
    pub bytes: u64,
    /// Le parcours des sources a atteint son plafond d'entrées.
    pub truncated: bool,
    /// Toutes les sources sont dans le même dossier (exigence de la compression du panel).
    pub same_parent: bool,
    /// Une destination de premier niveau existe déjà côté cible.
    pub top_conflict: bool,
    pub overwrite: bool,
}

impl StrategyFacts {
    fn files_fit(&self) -> bool {
        !self.truncated && self.files <= MAX_COPY_FILES && self.bytes <= MAX_COPY_BYTES
    }

    /// Pourquoi une archive est impossible (`None` : possible).
    fn archive_blocker(&self) -> Option<&'static str> {
        if !self.same_parent {
            Some("les sources ne sont pas toutes dans le même dossier (exigence de la compression par le serveur)")
        } else if self.top_conflict && !self.overwrite {
            Some("la destination existe déjà et l'extraction écraserait des fichiers sans overwrite=true")
        } else {
            None
        }
    }
}

/// Stratégie retenue, et pourquoi.
pub(crate) fn choose_strategy(requested: Strategy, f: &StrategyFacts) -> Result<(Strategy, String)> {
    let size = format!("{} fichier(s), {}", f.files, human_bytes(f.bytes));
    match requested {
        Strategy::Files => {
            if f.truncated {
                return Err(invalid(format!(
                    "Plus de {MAX_WALK_ENTRIES} entrées à parcourir : copie fichier par fichier impossible (`strategy=archive`)."
                )));
            }
            check_file_caps(f.files, f.bytes)?;
            Ok((Strategy::Files, format!("demandée ({size})")))
        }
        Strategy::Archive => match f.archive_blocker() {
            Some(why) => Err(invalid(format!("Copie par archive impossible : {why}."))),
            None => Ok((Strategy::Archive, format!("demandée ({size})"))),
        },
        Strategy::Auto => {
            let big = f.truncated || f.files > AUTO_ARCHIVE_FILES || f.bytes > AUTO_ARCHIVE_BYTES;
            match (big, f.archive_blocker()) {
                (true, None) => Ok((Strategy::Archive, format!("automatique : {size}, plus rapide compressé par le serveur"))),
                (_, Some(why)) if !f.files_fit() => Err(invalid(format!(
                    "Copie trop volumineuse fichier par fichier ({size} ; plafonds {MAX_COPY_FILES} fichiers, {}) et archive impossible : {why}.",
                    human_bytes(MAX_COPY_BYTES)
                ))),
                (true, Some(why)) => Ok((Strategy::Files, format!("automatique : {size}, archive impossible ({why})"))),
                (false, _) => Ok((Strategy::Files, format!("automatique : {size}, fichier par fichier"))),
            }
        }
    }
}

/// Toutes les sources sont-elles dans le même dossier ?
fn same_parent(sources: &[String]) -> bool {
    sources.windows(2).all(|w| parent_of(&w[0]) == parent_of(&w[1]))
}

/// Où chercher, dans le dossier d'extraction, ce qui vient de `source_root` : sous son nom
/// (archive relative au dossier de la sélection) ou sous son chemin depuis la racine du serveur
/// (archive relative à la racine). Le format de l'API n'étant pas documenté, les deux sont acceptés.
pub(crate) fn extracted_candidates(out_dir: &str, source_root: &str) -> Vec<String> {
    let by_name = join(out_dir, base_name(source_root));
    let by_path = join(out_dir, &server_files::panel_path(source_root));
    if by_path == by_name {
        vec![by_name]
    } else {
        vec![by_name, by_path]
    }
}

/// Place manquante côté cible pour `needed` octets de plus (`None` : assez, ou limite inconnue).
pub(crate) fn space_shortfall(space: &DiskSpace, needed: u64) -> Option<String> {
    if needed == 0 {
        return None;
    }
    if space.limit_bytes > 0 && space.used_bytes.saturating_add(needed) > space.limit_bytes {
        return Some(format!(
            "Place insuffisante sur le serveur cible : {} occupés sur {}, il faudrait {} de plus.",
            human_bytes(space.used_bytes),
            human_bytes(space.limit_bytes),
            human_bytes(needed)
        ));
    }
    match &space.mybox {
        Some(m) if m.limit_bytes > 0 && m.used_bytes.saturating_add(needed) > m.limit_bytes => Some(format!(
            "Place insuffisante sur la MyBox du serveur cible (disque partagé) : {} occupés sur {}, il faudrait {} de plus.",
            human_bytes(m.used_bytes),
            human_bytes(m.limit_bytes),
            human_bytes(needed)
        )),
        _ => None,
    }
}

/// Bilan d'une copie.
#[derive(Debug, Default)]
pub(crate) struct CopyReport {
    pub strategy: &'static str,
    pub reason: String,
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub replaced_files: u64,
    pub created_dirs: u64,
    pub skipped: Vec<Skipped>,
    pub notes: Vec<String>,
    /// Nettoyage incomplet (archives ou dossier de transit restés en place).
    pub cleanup: Vec<String>,
    pub interrupted: Option<String>,
}

impl CopyReport {
    fn new(strategy: Strategy, reason: String) -> Self {
        Self { strategy: strategy.label(), reason, ..Default::default() }
    }

    fn skip(&mut self, path: &str, reason: String) {
        self.skipped.push(Skipped { path: path.to_string(), reason });
    }
}

fn server_label(names: &HashMap<i64, String>, id: i64) -> String {
    match names.get(&id) {
        Some(n) => format!("« {n} » (#{id})"),
        None => format!("#{id}"),
    }
}

/// Rapport JSON d'une copie.
pub(crate) fn render_copy(req: &CopyRequest, names: &HashMap<i64, String>, r: &CopyReport, elapsed: Duration) -> String {
    let status = match (&r.interrupted, r.skipped.is_empty()) {
        (Some(_), _) => "interrompue",
        (None, true) => "terminée",
        (None, false) => "terminée, avec des éléments ignorés",
    };
    let out = json!({
        "status": status,
        "source": { "server": server_label(names, req.source_server), "paths": req.sources },
        "target": { "server": server_label(names, req.target_server), "dir": req.target_dir },
        "strategy": r.strategy,
        "strategy_reason": r.reason,
        "overwrite": req.overwrite,
        "copied_files": r.copied_files,
        "copied_bytes": r.copied_bytes,
        "copied": human_bytes(r.copied_bytes),
        "replaced_files": r.replaced_files,
        "created_dirs": r.created_dirs,
        "skipped_count": r.skipped.len(),
        "skipped": r.skipped.iter().take(MAX_SKIPPED_SHOWN).collect::<Vec<_>>(),
        "skipped_list_truncated": r.skipped.len() > MAX_SKIPPED_SHOWN,
        "interrupted": r.interrupted,
        "notes": r.notes,
        "cleanup_issues": r.cleanup,
        "duration_s": (elapsed.as_secs_f64() * 10.0).round() / 10.0,
        "limits": {
            "file_by_file": format!("{MAX_COPY_FILES} fichiers, {}", human_bytes(MAX_COPY_BYTES)),
            "archive_transfer": human_bytes(MAX_ARCHIVE_BYTES),
        },
    });
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

// --- Copie : réseau -----------------------------------------------------------------------

async fn conn(core: &Core, id: i64) -> Result<Arc<SftpConn>> {
    core.sftp.ensure(&core.api, &core.token()?, id).await
}

/// Session SFTP vérifiée : une session du pool morte en silence (veille, coupure) est rouverte.
async fn live_conn(core: &Core, id: i64) -> Result<Arc<SftpConn>> {
    let c = conn(core, id).await?;
    match sftp::probe(&c, "/").await {
        Err(e) if sftp::is_session_dead(&e) => {
            core.sftp.drop_session(id);
            conn(core, id).await
        }
        _ => Ok(c),
    }
}

/// Noms des serveurs du compte (pour les rapports) ; vide si la liste est indisponible.
async fn server_names(core: &Core) -> HashMap<i64, String> {
    core.list_servers()
        .await
        .map(|o| o.servers.into_iter().map(|s| (s.id, s.name)).collect())
        .unwrap_or_default()
}

/// Outil `copy_between_servers`.
pub(crate) async fn copy_tool(core: &Core, args: &Value) -> Result<String> {
    let req = copy_request(args)?;
    let started = Instant::now();
    let report = copy(core, &req).await?;
    let names = server_names(core).await;
    Ok(render_copy(&req, &names, &report, started.elapsed()))
}

async fn copy(core: &Core, req: &CopyRequest) -> Result<CopyReport> {
    let src = live_conn(core, req.source_server).await?;
    let dst = live_conn(core, req.target_server).await?;

    // Sources : nature, puis contenu des dossiers (plafonné).
    let mut trees = Vec::with_capacity(req.sources.len());
    let mut truncated = false;
    let mut budget = MAX_WALK_ENTRIES;
    for root in &req.sources {
        let top = sftp::probe(&src, root)
            .await?
            .ok_or_else(|| invalid(format!("« {root} » introuvable sur le serveur source.")))?;
        let entries = if top.kind == EntryKind::Dir && !truncated {
            let (entries, cut) = sftp::walk_tree(&src, root, true, budget).await?;
            truncated |= cut;
            budget = budget.saturating_sub(entries.len());
            entries
        } else {
            Vec::new()
        };
        trees.push(SourceTree { root: root.clone(), kind: top.kind, size: top.size, entries });
    }

    // Cible : le dossier doit être un dossier (créé au besoin) ; destinations de premier niveau.
    if let Some(t) = sftp::probe(&dst, &req.target_dir).await? {
        if t.kind != EntryKind::Dir {
            return Err(invalid(format!("« {} » n'est pas un dossier sur le serveur cible.", req.target_dir)));
        }
    }
    let mut tops = Vec::with_capacity(trees.len());
    for t in &trees {
        let d = dest_root(&req.target_dir, &t.root);
        let found = sftp::probe(&dst, &d).await?;
        tops.push((d, found));
    }

    let (files, bytes) = trees.iter().fold((0, 0), |(f, b), t| {
        let (tf, tb, _) = t.totals();
        (f + tf, b + tb)
    });
    let facts = StrategyFacts {
        files,
        bytes,
        truncated,
        same_parent: same_parent(&req.sources),
        top_conflict: tops.iter().any(|(_, t)| t.is_some()),
        overwrite: req.overwrite,
    };
    let (strategy, reason) = choose_strategy(req.strategy, &facts)?;
    if strategy == Strategy::Files {
        return copy_files(core, req, &trees, &tops, CopyReport::new(Strategy::Files, reason)).await;
    }

    // Archive : compression par le serveur source (une archive temporaire abandonnée en route est
    // retirée par `server_compress_temp` lui-même).
    let tid = format!("mcp-copy-{}", stamp());
    match core.server_compress_temp(req.source_server, req.sources.clone(), &tid).await {
        Ok(archive) => copy_archive(core, req, &trees, &tops, archive, CopyReport::new(Strategy::Archive, reason)).await,
        // Choix automatique : la compression a échoué avant toute écriture côté cible, on se replie
        // sur la copie fichier par fichier si elle tient dans ses plafonds.
        Err(e) if req.strategy == Strategy::Auto && facts.files_fit() => {
            let reason = format!("{reason} ; compression impossible ({e}) : repli fichier par fichier");
            copy_files(core, req, &trees, &tops, CopyReport::new(Strategy::Files, reason)).await
        }
        Err(e) => Err(e),
    }
}

/// Ce qui existe sous les destinations de premier niveau (parcours complet, sinon refus : une
/// collision non vue ferait écraser un fichier sans `overwrite`).
async fn existing_under(dst: &SftpConn, tops: &[(String, Option<TreeEntry>)]) -> Result<Existing> {
    let mut existing = Existing::new();
    for (path, top) in tops {
        let Some(top) = top else { continue };
        existing.insert(path.clone(), (top.kind, top.size));
        if top.kind == EntryKind::Dir {
            let (entries, cut) = sftp::walk_tree(dst, path, true, MAX_WALK_ENTRIES).await?;
            if cut {
                return Err(invalid(format!(
                    "« {path} » existe déjà côté cible et dépasse {MAX_WALK_ENTRIES} entrées : collisions invérifiables, copie refusée."
                )));
            }
            existing.extend(entries.into_iter().map(|e| (e.path, (e.kind, e.size))));
        }
    }
    Ok(existing)
}

async fn copy_files(
    core: &Core,
    req: &CopyRequest,
    trees: &[SourceTree],
    tops: &[(String, Option<TreeEntry>)],
    mut report: CopyReport,
) -> Result<CopyReport> {
    let dst = live_conn(core, req.target_server).await?;
    let existing = existing_under(&dst, tops).await?;
    let plan = plan_copy(trees, &req.target_dir, &existing, req.overwrite);
    check_file_caps(plan.files.len(), plan.bytes)?;
    if let Ok(space) = core.disk_space(req.target_server).await {
        if let Some(why) = space_shortfall(&space, plan.extra_bytes) {
            return Err(invalid(why));
        }
    }
    report.skipped = plan.skipped;

    sftp::ensure_dir(&dst, &req.target_dir).await;
    for dir in &plan.dirs {
        match sftp::mkdir(&dst, dir).await {
            Ok(()) => report.created_dirs += 1,
            Err(e) => match sftp::probe(&dst, dir).await {
                Ok(Some(t)) if t.kind == EntryKind::Dir => {}
                _ => report.skip(dir, format!("dossier non créé : {e}")),
            },
        }
    }
    run_file_copies(core, req, &plan.files, &mut report).await;
    Ok(report)
}

/// Copies en parallèle (bornées), rejouées une fois sur des sessions rouvertes si une session
/// est morte en route ; interrompues après [`MAX_COPY_ERRORS`] échecs.
async fn run_file_copies(core: &Core, req: &CopyRequest, files: &[FileCopy], report: &mut CopyReport) {
    let errors = AtomicUsize::new(0);
    let mut pending: Vec<&FileCopy> = files.iter().collect();
    for attempt in 0..2 {
        if pending.is_empty() {
            break;
        }
        let conns = async { Ok::<_, Error>((conn(core, req.source_server).await?, conn(core, req.target_server).await?)) };
        let (src, dst) = match conns.await {
            Ok(c) => c,
            Err(e) => {
                for f in pending {
                    report.skip(&f.src, format!("non copié : {e}"));
                }
                return;
            }
        };
        let errors = &errors;
        // Futures construites d'avance (pas de closure dans le type du flux) : sinon le compilateur
        // ne prouve plus que l'appelant, lancé par `tokio::spawn`, est `Send`.
        let tasks: Vec<_> = pending
            .iter()
            .copied()
            .map(|f| {
                let (src, dst) = (src.clone(), dst.clone());
                async move {
                    if errors.load(Ordering::Relaxed) >= MAX_COPY_ERRORS {
                        return (f, None);
                    }
                    let r = copy_one(&src, &dst, f).await;
                    if r.as_ref().is_err_and(|e| !sftp::is_session_dead(e)) {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    (f, Some(r))
                }
            })
            .collect();
        let results: Vec<(&FileCopy, Option<Result<u64>>)> =
            stream::iter(tasks).buffer_unordered(COPY_CONCURRENCY).collect().await;
        let mut retry = Vec::new();
        for (f, r) in results {
            match r {
                None => report.skip(&f.src, "non tenté : copie interrompue après trop d'erreurs".into()),
                Some(Ok(n)) => {
                    report.copied_files += 1;
                    report.copied_bytes += n;
                    if f.replace {
                        report.replaced_files += 1;
                    }
                }
                Some(Err(e)) if attempt == 0 && sftp::is_session_dead(&e) => retry.push(f),
                Some(Err(e)) => report.skip(&f.src, format!("erreur : {e}")),
            }
        }
        if !retry.is_empty() {
            core.sftp.drop_session(req.source_server);
            core.sftp.drop_session(req.target_server);
        }
        pending = retry;
    }
    if errors.load(Ordering::Relaxed) >= MAX_COPY_ERRORS {
        report.interrupted = Some(format!("{MAX_COPY_ERRORS} échecs : copie interrompue (voir `skipped`)."));
    }
}

/// Nom temporaire à côté d'un fichier, pour un remplacement sans jamais tronquer l'ancien.
fn temp_sibling(dst: &str) -> String {
    format!("{}/.{}.{STAGING_PREFIX}{}", parent_of(dst).trim_end_matches('/'), base_name(dst), stamp())
}

/// Copie un fichier. Un remplacement passe par un fichier temporaire voisin, échangé ensuite :
/// l'ancien fichier reste intact tant que le nouveau n'est pas complet.
async fn copy_one(src: &SftpConn, dst: &SftpConn, f: &FileCopy) -> Result<u64> {
    if !f.replace {
        return match sftp::copy_file(src, &f.src, dst, &f.dst).await {
            Ok(n) => Ok(n),
            Err(e) => {
                // Fichier partiel : il n'existait pas avant, on le retire.
                let _ = sftp::remove(dst, &f.dst, false).await;
                Err(e)
            }
        };
    }
    let tmp = temp_sibling(&f.dst);
    let n = match sftp::copy_file(src, &f.src, dst, &tmp).await {
        Ok(n) => n,
        Err(e) => {
            let _ = sftp::remove(dst, &tmp, false).await;
            return Err(e);
        }
    };
    replace_with(dst, &tmp, &f.dst).await?;
    Ok(n)
}

/// Met `from` à la place de `to` (fichier existant) sur le même serveur. Certains serveurs SFTP
/// refusent de renommer par-dessus un fichier : l'ancien est alors retiré juste avant.
async fn replace_with(conn: &SftpConn, from: &str, to: &str) -> Result<()> {
    if sftp::rename(conn, from, to).await.is_ok() {
        return Ok(());
    }
    sftp::remove(conn, to, false).await?;
    sftp::rename(conn, from, to)
        .await
        .map_err(|e| invalid(format!("ancien fichier retiré, nouvelle version laissée sous « {from} » : {e}")))
}

async fn copy_archive(
    core: &Core,
    req: &CopyRequest,
    trees: &[SourceTree],
    tops: &[(String, Option<TreeEntry>)],
    archive: CompressOutcome,
    mut report: CopyReport,
) -> Result<CopyReport> {
    if archive.skipped > 0 {
        report.notes.push(format!(
            "{} fichier(s) illisible(s) ignoré(s) par la compression du serveur source.",
            archive.skipped
        ));
    }
    let staging = join(&req.target_dir, &format!("{STAGING_PREFIX}{}", stamp()));
    let mut staging_used = false;
    let outcome = archive_steps(core, req, trees, tops, &archive, &staging, &mut staging_used, &mut report).await;
    // Nettoyage des deux côtés, que la copie ait abouti ou non.
    cleanup_archive(core, req, &archive.archive_path, &staging, staging_used, &mut report.cleanup).await;
    match outcome {
        Ok(()) => Ok(report),
        Err(e) if report.cleanup.is_empty() => Err(e),
        Err(e) => Err(invalid(format!("{e} — nettoyage incomplet : {}", report.cleanup.join(" ; ")))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn archive_steps(
    core: &Core,
    req: &CopyRequest,
    trees: &[SourceTree],
    tops: &[(String, Option<TreeEntry>)],
    archive: &CompressOutcome,
    staging: &str,
    staging_used: &mut bool,
    report: &mut CopyReport,
) -> Result<()> {
    let cross = !req.same_server();
    if cross && archive.size > MAX_ARCHIVE_BYTES {
        return Err(invalid(format!(
            "Archive de {} : au-delà du plafond de transfert ({}).",
            human_bytes(archive.size),
            human_bytes(MAX_ARCHIVE_BYTES)
        )));
    }
    // Côté cible, l'archive et son contenu extrait coexistent un moment.
    let extracted: u64 = trees.iter().map(|t| t.totals().1).sum();
    let needed = extracted + if cross { archive.size } else { 0 };
    if let Ok(space) = core.disk_space(req.target_server).await {
        if let Some(why) = space_shortfall(&space, needed) {
            return Err(invalid(why));
        }
    }

    let dst = live_conn(core, req.target_server).await?;
    *staging_used = true;
    sftp::ensure_dir(&dst, staging).await;
    if !matches!(sftp::probe(&dst, staging).await?, Some(t) if t.kind == EntryKind::Dir) {
        return Err(invalid(format!("dossier de transit « {staging} » non créé sur le serveur cible")));
    }

    // Même serveur : l'archive est déjà là, rien à transférer.
    let on_target = if cross {
        let to = join(staging, base_name(&archive.archive_path));
        let mut sent = transfer(core, req, &archive.archive_path, &to).await;
        if sent.as_ref().is_err_and(sftp::is_session_dead) {
            core.sftp.drop_session(req.source_server);
            core.sftp.drop_session(req.target_server);
            sent = transfer(core, req, &archive.archive_path, &to).await;
        }
        let sent = sent?;
        if archive.size > 0 && sent != archive.size {
            return Err(invalid(format!(
                "archive transférée incomplète ({} sur {})",
                human_bytes(sent),
                human_bytes(archive.size)
            )));
        }
        to
    } else {
        archive.archive_path.clone()
    };

    // Extraction par le serveur cible, dans le dossier de transit.
    let out_dir = join(staging, "contenu");
    let ext = core.extract_archive(req.target_server, &on_target, &out_dir).await?;
    if ext.skipped > 0 {
        report.notes.push(format!("{} fichier(s) ignoré(s) par l'extraction du serveur cible.", ext.skipped));
    }

    // Mise en place : un renommage si la destination est libre, sinon fusion entrée par entrée
    // (overwrite=true, vérifié au choix de la stratégie).
    let dst = live_conn(core, req.target_server).await?;
    for (tree, (dst_root, top)) in trees.iter().zip(tops) {
        let mut found = None;
        for cand in extracted_candidates(&out_dir, &tree.root) {
            if let Some(t) = sftp::probe(&dst, &cand).await? {
                found = Some(t);
                break;
            }
        }
        let staged_top = found.ok_or_else(|| {
            invalid(format!("« {} » introuvable dans l'archive extraite : format d'archive inattendu.", tree.root))
        })?;
        let entries = if staged_top.kind == EntryKind::Dir {
            let (entries, cut) = sftp::walk_tree(&dst, &staged_top.path, true, MAX_WALK_ENTRIES).await?;
            if cut {
                return Err(invalid(format!("contenu extrait de « {} » trop vaste pour être mis en place", tree.root)));
            }
            entries
        } else {
            Vec::new()
        };
        let staged = SourceTree { root: staged_top.path.clone(), kind: staged_top.kind, size: staged_top.size, entries };
        if top.is_none() {
            sftp::rename(&dst, &staged.root, dst_root).await?;
            let (files, bytes, dirs) = staged.totals();
            report.copied_files += files as u64;
            report.copied_bytes += bytes;
            report.created_dirs += dirs as u64;
            continue;
        }
        let existing = existing_under(&dst, std::slice::from_ref(&(dst_root.clone(), top.clone()))).await?;
        let plan = plan_copy(std::slice::from_ref(&staged), &req.target_dir, &existing, true);
        report.skipped.extend(plan.skipped.into_iter().map(|s| Skipped {
            path: source_path_of(&s.path, &staged.root, &tree.root),
            reason: s.reason,
        }));
        for dir in &plan.dirs {
            match sftp::mkdir(&dst, dir).await {
                Ok(()) => report.created_dirs += 1,
                Err(e) => report.skip(dir, format!("dossier non créé : {e}")),
            }
        }
        for f in &plan.files {
            let moved = if f.replace { replace_with(&dst, &f.src, &f.dst).await } else { sftp::rename(&dst, &f.src, &f.dst).await };
            match moved {
                Ok(()) => {
                    report.copied_files += 1;
                    report.copied_bytes += f.size;
                    if f.replace {
                        report.replaced_files += 1;
                    }
                }
                Err(e) => report.skip(&source_path_of(&f.src, &staged.root, &tree.root), format!("erreur : {e}")),
            }
        }
    }
    Ok(())
}

/// Chemin source correspondant à une entrée du dossier de transit (pour le rapport).
fn source_path_of(staged_path: &str, staged_root: &str, source_root: &str) -> String {
    match rel_of(staged_root, staged_path) {
        "" => source_root.to_string(),
        rel => join(source_root, rel),
    }
}

/// Transfère l'archive du serveur source vers le serveur cible, en flux.
async fn transfer(core: &Core, req: &CopyRequest, from: &str, to: &str) -> Result<u64> {
    let src = live_conn(core, req.source_server).await?;
    let dst = live_conn(core, req.target_server).await?;
    sftp::copy_file(&src, from, &dst, to).await
}

/// Retire le dossier de transit (cible) et l'archive temporaire (source), en notant ce qui reste.
async fn cleanup_archive(
    core: &Core,
    req: &CopyRequest,
    src_archive: &str,
    staging: &str,
    staging_used: bool,
    issues: &mut Vec<String>,
) {
    if staging_used {
        let mut res = remove_staging(core, req.target_server, staging).await;
        if res.as_ref().is_err_and(sftp::is_session_dead) {
            core.sftp.drop_session(req.target_server);
            res = remove_staging(core, req.target_server, staging).await;
        }
        if let Err(e) = res {
            issues.push(format!("dossier de transit « {staging} » non supprimé sur le serveur cible ({e})"));
        }
    }
    if let Err(e) = core.remove_temp_archive(req.source_server, src_archive).await {
        issues.push(format!("archive temporaire « {src_archive} » non supprimée sur le serveur source ({e})"));
    }
}

async fn remove_staging(core: &Core, id: i64, staging: &str) -> Result<()> {
    if !is_staging_dir(staging) {
        return Err(invalid(format!("« {staging} » n'est pas un dossier de transit de l'application")));
    }
    let c = conn(core, id).await?;
    match sftp::probe(&c, staging).await? {
        None => Ok(()),
        Some(_) => sftp::remove_recursive(&c, staging).await,
    }
}

// --- Comparaison (diff_files) --------------------------------------------------------------

/// Demande de comparaison : A (référence) et B.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DiffRequest {
    pub a_server: i64,
    pub a_path: String,
    pub b_server: i64,
    pub b_path: String,
    pub recursive: bool,
    pub context: usize,
}

pub(crate) fn diff_request(args: &Value) -> Result<DiffRequest> {
    let a_server = arg_i64(args, "server_id")?;
    let a_path = normalize_path(&arg_str(args, "path")?)?;
    let b_server = arg_opt_i64(args, "other_server_id")?.unwrap_or(a_server);
    let b_path = match arg_opt_str(args, "other_path") {
        Some(p) if !p.trim().is_empty() => normalize_path(&p)?,
        _ => a_path.clone(),
    };
    if a_server == b_server && a_path == b_path {
        return Err(invalid("Rien à comparer : même serveur et même chemin (précise `other_server_id` ou `other_path`)."));
    }
    let context = arg_opt_i64(args, "context_lines")?.unwrap_or(3).clamp(0, 10) as usize;
    Ok(DiffRequest { a_server, a_path, b_server, b_path, recursive: arg_bool(args, "recursive", false)?, context })
}

/// Contenu binaire (octet nul, ou UTF-8 invalide) : pas de diff ligne à ligne.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

/// Statistiques d'un diff.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffStats {
    pub hunks: usize,
    pub added: usize,
    pub removed: usize,
    pub truncated: bool,
}

/// Diff unifié (contexte `context`) de deux textes. Chaque ligne passe par `redact` **avant** de
/// recevoir son préfixe `-`/`+`/` ` : le filtre reconnaît ainsi `password: …` en début de ligne.
/// Plafonné à [`MAX_DIFF_LINES`] lignes (les statistiques, elles, sont complètes).
pub(crate) fn unified_diff(
    a: &str,
    b: &str,
    label_a: &str,
    label_b: &str,
    context: usize,
    redact: &dyn Fn(&str) -> String,
) -> (String, DiffStats) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::configure().timeout(Duration::from_secs(2)).diff_lines(a, b);
    let mut udiff = diff.unified_diff();
    udiff.context_radius(context);
    let mut out = format!("--- {label_a}\n+++ {label_b}\n");
    let mut stats = DiffStats::default();
    let mut lines = 0usize;
    for hunk in udiff.iter_hunks() {
        stats.hunks += 1;
        let changes: Vec<_> = hunk.iter_changes().collect();
        for c in &changes {
            match c.tag() {
                ChangeTag::Insert => stats.added += 1,
                ChangeTag::Delete => stats.removed += 1,
                ChangeTag::Equal => {}
            }
        }
        if stats.truncated {
            continue;
        }
        let _ = writeln!(out, "{}", hunk.header());
        lines += 1;
        for c in changes {
            if lines >= MAX_DIFF_LINES {
                stats.truncated = true;
                break;
            }
            let text = c.value().trim_end_matches(['\n', '\r']);
            let _ = writeln!(out, "{}{}", c.tag(), redact(text));
            lines += 1;
        }
    }
    (out, stats)
}

fn when(ts: Option<i64>) -> String {
    ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "date inconnue".into())
}

/// Un côté d'une comparaison, pour les rendus.
pub(crate) struct Side<'a> {
    /// « A : « Survie » (#12) — /chemin ».
    pub label: String,
    pub entry: &'a TreeEntry,
}

/// Rendu de la comparaison de deux fichiers. `bytes` : contenus (absents s'ils dépassent
/// [`MAX_DIFF_FILE_BYTES`]).
pub(crate) fn render_file_comparison(
    a: &Side,
    b: &Side,
    bytes: Option<(&[u8], &[u8])>,
    context: usize,
    redact: &dyn Fn(&str) -> String,
) -> String {
    let mut out = String::from("Comparaison de fichiers\n");
    for s in [a, b] {
        let _ = writeln!(out, "{} — {}, modifié {}", s.label, human_bytes(s.entry.size), when(s.entry.modified));
    }
    let Some((ba, bb)) = bytes else {
        let _ = write!(
            out,
            "Résultat : trop gros pour un diff ligne à ligne (plafond {}) ; tailles {}.",
            human_bytes(MAX_DIFF_FILE_BYTES),
            if a.entry.size == b.entry.size { "identiques (contenu non comparé)" } else { "différentes : fichiers différents" }
        );
        return out;
    };
    if ba == bb {
        out.push_str("Résultat : identiques (même contenu, octet pour octet).");
        return out;
    }
    if is_binary(ba) || is_binary(bb) {
        out.push_str("Résultat : différents ; fichier binaire, pas de diff ligne à ligne.");
        return out;
    }
    // Fins de ligne ramenées à LF : un fichier CRLF face à un fichier LF ne doit pas sortir comme
    // « toutes les lignes ont changé ».
    let (ta, tb) = (String::from_utf8_lossy(ba).replace("\r\n", "\n"), String::from_utf8_lossy(bb).replace("\r\n", "\n"));
    if ta.trim_end_matches('\n') == tb.trim_end_matches('\n') {
        out.push_str("Résultat : même texte ; seules les fins de ligne diffèrent (CRLF/LF ou saut de ligne final).");
        return out;
    }
    let (diff, stats) = unified_diff(&ta, &tb, &format!("A:{}", a.entry.path), &format!("B:{}", b.entry.path), context, redact);
    let _ = writeln!(
        out,
        "Résultat : différents — {} bloc(s), +{} / −{} ligne(s) (B par rapport à A).",
        stats.hunks, stats.added, stats.removed
    );
    let crlf = |x: &[u8]| x.windows(2).any(|w| w == b"\r\n");
    if crlf(ba) != crlf(bb) {
        out.push_str("Note : fins de ligne différentes (CRLF/LF), ignorées dans le diff.\n");
    }
    out.push_str(&diff);
    if stats.truncated {
        let _ = writeln!(out, "… diff tronqué à {MAX_DIFF_LINES} lignes.");
    }
    if diff.contains("[SECRET]") {
        out.push_str("Note : secrets masqués ([SECRET]) — deux lignes qui ne diffèrent que par un secret paraissent identiques.\n");
    }
    out
}

/// Comparaison de deux listings, par chemin relatif.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DirComparison {
    /// Présents seulement dans B (chemins relatifs).
    pub only_b: Vec<TreeEntry>,
    /// Présents seulement dans A.
    pub only_a: Vec<TreeEntry>,
    /// Fichiers de taille différente (A, B).
    pub resized: Vec<(TreeEntry, TreeEntry)>,
    /// Fichiers de même taille mais de date différente (contenu non comparé).
    pub redated: Vec<(TreeEntry, TreeEntry)>,
    /// Fichier d'un côté, dossier (ou autre) de l'autre.
    pub type_changed: Vec<(TreeEntry, TreeEntry)>,
    /// Fichiers identiques (taille et date) et dossiers présents des deux côtés.
    pub identical: usize,
}

fn relative(root: &str, entries: &[TreeEntry]) -> BTreeMap<String, TreeEntry> {
    entries
        .iter()
        .map(|e| {
            let rel = rel_of(root, &e.path).to_string();
            (rel.clone(), TreeEntry { path: rel, ..e.clone() })
        })
        .collect()
}

/// Retire les entrées dont un dossier parent figure déjà dans la liste (« dossier ajouté » suffit).
fn collapse(list: Vec<TreeEntry>) -> Vec<TreeEntry> {
    let dirs: Vec<String> = list.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.path.clone()).collect();
    list.into_iter()
        .filter(|e| !dirs.iter().any(|d| e.path.starts_with(&format!("{d}/"))))
        .collect()
}

pub(crate) fn compare_listings(a_root: &str, a: &[TreeEntry], b_root: &str, b: &[TreeEntry]) -> DirComparison {
    let (ma, mb) = (relative(a_root, a), relative(b_root, b));
    let mut cmp = DirComparison::default();
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    for (rel, ea) in &ma {
        match mb.get(rel) {
            None => only_a.push(ea.clone()),
            Some(eb) if ea.kind != eb.kind => cmp.type_changed.push((ea.clone(), eb.clone())),
            Some(_) if ea.kind != EntryKind::File => cmp.identical += 1,
            Some(eb) if ea.size != eb.size => cmp.resized.push((ea.clone(), eb.clone())),
            Some(eb) if ea.modified != eb.modified => cmp.redated.push((ea.clone(), eb.clone())),
            Some(_) => cmp.identical += 1,
        }
    }
    for (rel, eb) in &mb {
        if !ma.contains_key(rel) {
            only_b.push(eb.clone());
        }
    }
    cmp.only_a = collapse(only_a);
    cmp.only_b = collapse(only_b);
    cmp
}

fn describe(e: &TreeEntry) -> String {
    match e.kind {
        EntryKind::Dir => format!("{}/ (dossier)", e.path),
        EntryKind::File => format!("{} ({}, {})", e.path, human_bytes(e.size), when(e.modified)),
        EntryKind::Other => format!("{} (lien ou fichier spécial)", e.path),
    }
}

fn kind_name(k: EntryKind) -> &'static str {
    match k {
        EntryKind::File => "fichier",
        EntryKind::Dir => "dossier",
        EntryKind::Other => "lien ou fichier spécial",
    }
}

/// Rendu texte d'une comparaison de dossiers (une ligne par entrée, listes plafonnées).
pub(crate) fn render_dir_comparison(
    a_label: &str,
    b_label: &str,
    recursive: bool,
    counts: (usize, usize),
    truncated: bool,
    cmp: &DirComparison,
) -> String {
    let mut out = format!(
        "Comparaison de dossiers ({})\n{a_label} — {} entrée(s)\n{b_label} — {} entrée(s)\n",
        if recursive { "récursive" } else { "premier niveau seulement ; `recursive=true` pour descendre" },
        counts.0,
        counts.1
    );
    if truncated {
        let _ = writeln!(out, "⚠ Comparaison PARTIELLE : plus de {MAX_DIFF_ENTRIES} entrées d'un côté, la suite n'a pas été lue.");
    }
    let _ = writeln!(
        out,
        "Résumé : {} ajouté(s) dans B, {} absent(s) de B, {} modifié(s) (taille), {} de même taille mais de date différente, {} de type différent, {} identique(s).",
        cmp.only_b.len(),
        cmp.only_a.len(),
        cmp.resized.len(),
        cmp.redated.len(),
        cmp.type_changed.len(),
        cmp.identical
    );
    fn section<T>(out: &mut String, title: &str, items: &[T], line: impl Fn(&T) -> String) {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(out, "{title} ({}) :", items.len());
        for it in items.iter().take(MAX_DIFF_LISTED) {
            let _ = writeln!(out, "{}", line(it));
        }
        if items.len() > MAX_DIFF_LISTED {
            let _ = writeln!(out, "… et {} autre(s).", items.len() - MAX_DIFF_LISTED);
        }
    }
    section(&mut out, "Ajoutés (seulement dans B)", &cmp.only_b, |e| format!("+ {}", describe(e)));
    section(&mut out, "Absents de B (seulement dans A)", &cmp.only_a, |e| format!("- {}", describe(e)));
    section(&mut out, "Modifiés (taille différente)", &cmp.resized, |(a, b)| {
        format!("~ {} : {} ({}) → {} ({})", a.path, human_bytes(a.size), when(a.modified), human_bytes(b.size), when(b.modified))
    });
    section(
        &mut out,
        "Même taille, date différente (contenu non comparé : diff_files sur le fichier pour trancher)",
        &cmp.redated,
        |(a, b)| format!("= {} : {} → {}", a.path, when(a.modified), when(b.modified)),
    );
    section(&mut out, "Type différent", &cmp.type_changed, |(a, b)| {
        format!("! {} : {} dans A, {} dans B", a.path, kind_name(a.kind), kind_name(b.kind))
    });
    out
}

/// Outil `diff_files`.
pub(crate) async fn diff_tool(core: &Core, args: &Value) -> Result<String> {
    let req = diff_request(args)?;
    let names = server_names(core).await;
    let ca = live_conn(core, req.a_server).await?;
    let cb = if req.b_server == req.a_server { ca.clone() } else { live_conn(core, req.b_server).await? };
    let ea = sftp::probe(&ca, &req.a_path)
        .await?
        .ok_or_else(|| invalid(format!("« {} » introuvable sur le serveur A ({}).", req.a_path, server_label(&names, req.a_server))))?;
    let eb = sftp::probe(&cb, &req.b_path)
        .await?
        .ok_or_else(|| invalid(format!("« {} » introuvable sur le serveur B ({}).", req.b_path, server_label(&names, req.b_server))))?;
    let la = format!("A : {} — {}", server_label(&names, req.a_server), req.a_path);
    let lb = format!("B : {} — {}", server_label(&names, req.b_server), req.b_path);
    let redact = |s: &str| core.redact_ai(s);
    let out = match (ea.kind, eb.kind) {
        (EntryKind::File, EntryKind::File) => {
            let small = ea.size <= MAX_DIFF_FILE_BYTES && eb.size <= MAX_DIFF_FILE_BYTES;
            let contents = if small {
                Some((
                    sftp::read_bytes(&ca, &req.a_path, MAX_DIFF_FILE_BYTES).await?,
                    sftp::read_bytes(&cb, &req.b_path, MAX_DIFF_FILE_BYTES).await?,
                ))
            } else {
                None
            };
            render_file_comparison(
                &Side { label: la, entry: &ea },
                &Side { label: lb, entry: &eb },
                contents.as_ref().map(|(x, y)| (x.as_slice(), y.as_slice())),
                req.context,
                &redact,
            )
        }
        (EntryKind::Dir, EntryKind::Dir) => {
            let (a, cut_a) = sftp::walk_tree(&ca, &req.a_path, req.recursive, MAX_DIFF_ENTRIES).await?;
            let (b, cut_b) = sftp::walk_tree(&cb, &req.b_path, req.recursive, MAX_DIFF_ENTRIES).await?;
            let cmp = compare_listings(&req.a_path, &a, &req.b_path, &b);
            render_dir_comparison(&la, &lb, req.recursive, (a.len(), b.len()), cut_a || cut_b, &cmp)
        }
        (ka, kb) => format!(
            "Comparaison impossible : {} dans A, {} dans B.\n{la}\n{lb}",
            kind_name(ka),
            kind_name(kb)
        ),
    };
    // Le diff est déjà filtré ligne par ligne ; le reste (chemins, noms) l'est ici.
    Ok(core.redact_ai(&out))
}

// --- Recherche d'un joueur (find_player) ---------------------------------------------------

/// UUID ramené à sa forme comparable : minuscules, sans tirets.
fn uuid_key(s: &str) -> String {
    s.chars().filter(|c| *c != '-').flat_map(char::to_lowercase).collect()
}

/// Ce que l'on cherche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlayerQuery {
    /// Pseudo (minuscules), exact ou préfixe.
    Name { needle: String, exact: bool },
    /// UUID complet (forme comparable).
    Uuid(String),
    /// Alias d'anonymisation (`joueur-3fa2c1`) renvoyé par `list_players` ou `find_player`.
    Alias(String),
}

/// Lit la requête : UUID complet, alias d'anonymisation, sinon pseudo (préfixe d'au moins deux
/// caractères, ou exact).
pub(crate) fn player_query(raw: &str, exact: bool) -> Result<PlayerQuery> {
    let q = raw.trim();
    if q.is_empty() {
        return Err(invalid("`query` vide : indique un pseudo, un début de pseudo ou un UUID."));
    }
    if q.chars().any(char::is_control) || q.chars().count() > 64 {
        return Err(invalid("`query` invalide (64 caractères au plus, sans caractère de contrôle)."));
    }
    if let Ok(uuid) = crate::players::validate_uuid(q) {
        return Ok(PlayerQuery::Uuid(uuid_key(&uuid)));
    }
    let lower = q.to_lowercase();
    if lower.strip_prefix("joueur-").is_some_and(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())) {
        return Ok(PlayerQuery::Alias(lower));
    }
    if !exact && q.chars().count() < 2 {
        return Err(invalid("Recherche par début de pseudo : au moins 2 caractères (ou `exact=true`)."));
    }
    Ok(PlayerQuery::Name { needle: lower, exact })
}

/// Type de correspondance, du plus sûr au moins sûr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MatchKind {
    Exact,
    Uuid,
    Alias,
    Prefix,
}

impl MatchKind {
    fn label(self) -> &'static str {
        match self {
            Self::Exact => "pseudo exact",
            Self::Uuid => "uuid",
            Self::Alias => "alias",
            Self::Prefix => "début de pseudo",
        }
    }
}

pub(crate) fn match_player(p: &KnownPlayer, q: &PlayerQuery) -> Option<MatchKind> {
    match q {
        PlayerQuery::Uuid(key) => (uuid_key(&p.uuid) == *key).then_some(MatchKind::Uuid),
        PlayerQuery::Alias(alias) => (crate::redact::pseudonym(&p.uuid) == *alias).then_some(MatchKind::Alias),
        PlayerQuery::Name { needle, exact } => {
            let name = p.name.as_ref()?.to_lowercase();
            if name == *needle {
                Some(MatchKind::Exact)
            } else if !exact && name.starts_with(needle.as_str()) {
                Some(MatchKind::Prefix)
            } else {
                None
            }
        }
    }
}

/// Joueurs déjà venus d'un serveur, avec le nombre de connectés annoncé par l'hébergeur.
pub(crate) struct RosterView<'a> {
    pub server_id: i64,
    pub server: &'a str,
    pub roster: &'a Roster,
    /// Joueurs connectés d'après `live_players_bulk` (`None` : inconnu).
    pub online_count: Option<i64>,
}

/// Serveur non fouillé, et pourquoi.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ServerNote {
    pub server_id: i64,
    pub server: String,
    pub reason: String,
}

/// Connecté ? Oui/non si le serveur sait le dire, ou si personne n'y est connecté ; sinon inconnu.
fn online_state(p: &KnownPlayer, roster: &Roster, count: Option<i64>) -> Option<bool> {
    if roster.online_known {
        Some(p.online)
    } else if count == Some(0) {
        Some(false)
    } else {
        None
    }
}

fn hours(secs: i64) -> f64 {
    (secs as f64 / 360.0).round() / 10.0
}

fn day(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y-%m-%d").to_string())
}

struct Hit<'a> {
    server_id: i64,
    server: &'a str,
    player: &'a KnownPlayer,
    online: Option<bool>,
}

/// Fusionne les joueurs trouvés sur chaque serveur, regroupés par joueur (UUID). Pseudos
/// remplacés par un alias stable si `anonymize` (même politique que `list_players` : l'UUID
/// n'est jamais renvoyé).
pub(crate) fn player_report(
    q: &PlayerQuery,
    views: &[RosterView],
    notes: &[ServerNote],
    now: i64,
    anonymize: bool,
    limit: usize,
) -> Value {
    let mut groups: HashMap<String, (MatchKind, Vec<Hit>)> = HashMap::new();
    for v in views {
        for p in &v.roster.players {
            if let Some(kind) = match_player(p, q) {
                let g = groups.entry(uuid_key(&p.uuid)).or_insert((kind, Vec::new()));
                g.0 = g.0.min(kind);
                g.1.push(Hit { server_id: v.server_id, server: v.server, player: p, online: online_state(p, v.roster, v.online_count) });
            }
        }
    }
    let mut players: Vec<(MatchKind, Vec<Hit>)> = groups.into_values().collect();
    for (_, hits) in &mut players {
        hits.sort_by_key(|h| (std::cmp::Reverse(h.online == Some(true)), std::cmp::Reverse(h.player.last_seen.unwrap_or(0))));
    }
    let last_seen = |hits: &[Hit]| hits.iter().filter_map(|h| h.player.last_seen).max();
    let online_any = |hits: &[Hit]| hits.iter().any(|h| h.online == Some(true));
    let label = |hits: &[Hit]| -> String {
        let p = hits[0].player;
        if anonymize {
            crate::redact::pseudonym(&p.uuid)
        } else {
            // Pseudo de la venue la plus récente (un joueur peut avoir changé de pseudo).
            hits.iter()
                .filter(|h| h.player.name.is_some())
                .max_by_key(|h| h.player.last_seen.unwrap_or(0))
                .and_then(|h| h.player.name.clone())
                .unwrap_or_else(|| format!("(pseudo inconnu, UUID {})", p.uuid))
        }
    };
    players.sort_by(|(ka, a), (kb, b)| {
        ka.cmp(kb)
            .then_with(|| online_any(b).cmp(&online_any(a)))
            .then_with(|| last_seen(b).cmp(&last_seen(a)))
            .then_with(|| label(a).cmp(&label(b)))
    });
    let total = players.len();
    let list: Vec<Value> = players
        .iter()
        .take(limit)
        .map(|(kind, hits)| {
            let online_now = if online_any(hits) {
                Some(true)
            } else if hits.iter().all(|h| h.online == Some(false)) {
                Some(false)
            } else {
                None
            };
            json!({
                "player": label(hits),
                "match": kind.label(),
                "online_now": online_now,
                "online_on": hits.iter().filter(|h| h.online == Some(true)).map(|h| h.server).collect::<Vec<_>>(),
                "last_seen": last_seen(hits).and_then(day),
                "total_play_time_hours": hours(hits.iter().filter_map(|h| h.player.play_time_secs).sum()),
                "servers": hits.iter().map(|h| json!({
                    "server_id": h.server_id,
                    "server": h.server,
                    "online": h.online,
                    "last_seen": h.player.last_seen.and_then(day),
                    "days_since_last_seen": h.player.last_seen.map(|t| (now - t).max(0) / 86_400),
                    "play_time_hours": h.player.play_time_secs.map(hours),
                    "op": h.player.flags.op,
                    "whitelisted": h.player.flags.whitelisted,
                    "banned": h.player.flags.banned,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "matching_players": total,
        "shown": list.len(),
        "servers_searched": views.len(),
        "servers_skipped": notes,
        "lists_truncated_to_most_recent": views.iter().filter(|v| v.roster.truncated).map(|v| v.server).collect::<Vec<_>>(),
        "names_anonymized": anonymize,
        "note": "online = null : le serveur ne dit pas qui est connecté. play_time_hours = temps de jeu CUMULÉ ; \
                 last_seen = dernière sauvegarde du joueur (UTC). Joueurs Minecraft Java seulement (jeux sans \
                 liste de joueurs : voir servers_skipped).",
        "players": list,
    })
}

/// Serveur du compte, réduit à ce dont les outils de flotte ont besoin.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FleetServer {
    pub id: i64,
    pub name: String,
    /// Statut du compte : `active`, `hibernation`, `expired`, `suspended`, `disabled`.
    pub status: String,
    /// Le jeu expose la liste des joueurs déjà venus (Minecraft Java).
    pub player_data: bool,
    pub game: String,
}

impl From<&ServerListItem> for FleetServer {
    fn from(s: &ServerListItem) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            status: s.status.clone(),
            player_data: s.capabilities.player_data,
            game: s.egg_name.clone(),
        }
    }
}

/// Serveurs à fouiller pour un joueur (tous, ou ceux de `restrict`), et ceux écartés.
pub(crate) fn player_servers(servers: &[FleetServer], restrict: Option<&[i64]>) -> (Vec<FleetServer>, Vec<ServerNote>) {
    let mut keep = Vec::new();
    let mut notes = Vec::new();
    if let Some(ids) = restrict {
        for id in ids {
            if !servers.iter().any(|s| s.id == *id) {
                notes.push(ServerNote { server_id: *id, server: format!("#{id}"), reason: "serveur inconnu sur ce compte".into() });
            }
        }
    }
    for s in servers {
        if restrict.is_some_and(|ids| !ids.contains(&s.id)) {
            continue;
        }
        let reason = if !s.player_data {
            Some(format!("jeu sans liste de joueurs déjà venus ({})", s.game))
        } else if !matches!(s.status.as_str(), "active" | "hibernation") {
            Some(format!("serveur indisponible ({})", s.status))
        } else {
            None
        };
        match reason {
            Some(reason) => notes.push(ServerNote { server_id: s.id, server: s.name.clone(), reason }),
            None => keep.push(s.clone()),
        }
    }
    (keep, notes)
}

/// Outil `find_player`.
pub(crate) async fn find_player_tool(core: &Core, args: &Value) -> Result<String> {
    let query = player_query(&arg_str(args, "query")?, arg_bool(args, "exact", false)?)?;
    let restrict = arg_id_list(args, "server_ids")?.filter(|ids| !ids.is_empty());
    let limit = arg_opt_i64(args, "limit")?.unwrap_or(10).clamp(1, MAX_PLAYERS as i64) as usize;
    let overview = core.list_servers().await?;
    let servers: Vec<FleetServer> = overview.servers.iter().map(FleetServer::from).collect();
    let (searched, mut notes) = player_servers(&servers, restrict.as_deref());
    let ids: Vec<i64> = searched.iter().map(|s| s.id).collect();

    // Connectés de tous les serveurs en un appel (best-effort), listes en parallèle (bornées).
    let counts: HashMap<i64, i64> = core
        .live_players_bulk(&ids)
        .await
        .map(|l| l.into_iter().map(|p| (p.server_id, p.current)).collect())
        .unwrap_or_default();
    let tasks: Vec<_> = ids.iter().map(|&id| async move { (id, core.player_roster(id, false).await) }).collect();
    let rosters: Vec<(i64, Result<Arc<Roster>>)> = stream::iter(tasks).buffer_unordered(API_CONCURRENCY).collect().await;
    let mut loaded: Vec<(&FleetServer, Arc<Roster>)> = Vec::new();
    for (id, r) in rosters {
        let Some(s) = searched.iter().find(|s| s.id == id) else { continue };
        match r {
            Ok(roster) => loaded.push((s, roster)),
            Err(e) => notes.push(ServerNote { server_id: id, server: s.name.clone(), reason: format!("liste des joueurs indisponible : {e}") }),
        }
    }
    loaded.sort_by_key(|(s, _)| s.name.to_lowercase());
    let views: Vec<RosterView> = loaded
        .iter()
        .map(|(s, r)| RosterView { server_id: s.id, server: &s.name, roster: r, online_count: counts.get(&s.id).copied() })
        .collect();
    let anonymize = core.get_privacy_config().redact_ai;
    let report = player_report(&query, &views, &notes, crate::store::now_secs(), anonymize, limit);
    let text = serde_json::to_string_pretty(&report).map_err(|e| invalid(e.to_string()))?;
    Ok(core.redact_ai(&text))
}

// --- Commande groupée (broadcast_command) --------------------------------------------------

/// Commande console valide : non vide, une seule ligne, sans caractère de contrôle, bornée.
pub(crate) fn validate_command(raw: &str) -> Result<String> {
    let cmd = raw.trim();
    if cmd.is_empty() {
        return Err(invalid("`command` vide."));
    }
    if cmd.contains(['\n', '\r']) {
        return Err(invalid("`command` doit tenir sur une seule ligne (une commande par appel)."));
    }
    if cmd.chars().any(char::is_control) {
        return Err(invalid("`command` contient un caractère de contrôle."));
    }
    if cmd.chars().count() > MAX_COMMAND_CHARS {
        return Err(invalid(format!("`command` trop longue ({MAX_COMMAND_CHARS} caractères au plus).")));
    }
    Ok(cmd.to_string())
}

/// Serveurs visés par une commande groupée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Targets {
    /// Liste explicite (dédoublonnée, ordre conservé).
    Ids(Vec<i64>),
    /// Tous les serveurs démarrés du compte (demandé explicitement).
    AllRunning,
}

/// « Tous » n'est jamais déduit d'une liste vide : il faut `all_running=true`, sans liste.
pub(crate) fn broadcast_targets(ids: Option<Vec<i64>>, all_running: bool) -> Result<Targets> {
    let ids = ids.unwrap_or_default();
    match (all_running, ids.is_empty()) {
        (true, false) => Err(invalid("`server_ids` et `all_running=true` s'excluent : choisis l'un ou l'autre.")),
        (true, true) => Ok(Targets::AllRunning),
        (false, true) => Err(invalid(
            "Aucun serveur visé : donne `server_ids`, ou `all_running=true` pour tous les serveurs démarrés.",
        )),
        (false, false) => {
            let mut seen = HashSet::new();
            let ids: Vec<i64> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
            if ids.len() > MAX_BROADCAST {
                return Err(invalid(format!("{} serveurs visés : {MAX_BROADCAST} au plus par commande groupée.", ids.len())));
            }
            Ok(Targets::Ids(ids))
        }
    }
}

/// Serveurs dont l'état doit être lu avant l'envoi (connus et actifs).
pub(crate) fn broadcast_candidates(t: &Targets, servers: &[FleetServer]) -> Vec<i64> {
    let active = |s: &&FleetServer| s.status == "active";
    match t {
        Targets::AllRunning => servers.iter().filter(active).map(|s| s.id).collect(),
        Targets::Ids(ids) => ids
            .iter()
            .filter(|id| servers.iter().filter(active).any(|s| s.id == **id))
            .copied()
            .collect(),
    }
}

/// Résultat pour un serveur.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Outcome {
    pub server_id: i64,
    pub server: String,
    /// État au moment de l'envoi (`online`, `offline`, `starting`…), si connu.
    pub state: Option<String>,
    pub result: String,
}

/// Serveurs qui recevront la commande, et ceux écartés (avec la raison).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Selection {
    pub send: Vec<Outcome>,
    pub skipped: Vec<Outcome>,
}

/// Choisit les destinataires. Par défaut (`only_running`), seulement les serveurs `online` ; un
/// état illisible n'est pas un feu vert. « Tous » est plafonné après sélection.
pub(crate) fn select_broadcast(
    t: &Targets,
    servers: &[FleetServer],
    states: &HashMap<i64, std::result::Result<String, String>>,
    only_running: bool,
) -> Result<Selection> {
    let only_running = only_running || *t == Targets::AllRunning;
    let order: Vec<i64> = match t {
        Targets::AllRunning => servers.iter().map(|s| s.id).collect(),
        Targets::Ids(ids) => ids.clone(),
    };
    let mut sel = Selection::default();
    for id in order {
        let Some(s) = servers.iter().find(|s| s.id == id) else {
            sel.skipped.push(Outcome { server_id: id, server: format!("#{id}"), state: None, result: "ignoré : serveur inconnu sur ce compte".into() });
            continue;
        };
        let state = states.get(&id).and_then(|r| r.as_ref().ok()).cloned();
        let mut out = Outcome { server_id: id, server: s.name.clone(), state: state.clone(), result: String::new() };
        if s.status != "active" {
            out.result = format!("ignoré : serveur indisponible ({})", s.status);
            sel.skipped.push(out);
        } else if only_running && state.as_deref() != Some("online") {
            out.result = match states.get(&id) {
                Some(Err(e)) => format!("ignoré : état illisible ({e})"),
                _ => format!("ignoré : serveur non démarré ({})", state.as_deref().unwrap_or("état inconnu")),
            };
            sel.skipped.push(out);
        } else {
            sel.send.push(out);
        }
    }
    if sel.send.len() > MAX_BROADCAST {
        return Err(invalid(format!(
            "{} serveurs démarrés : {MAX_BROADCAST} au plus par commande groupée (passe une liste `server_ids`).",
            sel.send.len()
        )));
    }
    Ok(sel)
}

pub(crate) fn render_broadcast(sent: &[Outcome], skipped: &[Outcome]) -> String {
    let ok = sent.iter().filter(|o| o.result == "envoyée").count();
    let out = json!({
        "sent": ok,
        "failed": sent.len() - ok,
        "skipped": skipped.len(),
        "note": "Commande envoyée par l'API (REST), comme depuis le panel : sa sortie se lit avec read_console sur chaque serveur.",
        "results": sent.iter().chain(skipped).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

/// Outil `broadcast_command`.
pub(crate) async fn broadcast_tool(core: &Core, args: &Value) -> Result<String> {
    let command = validate_command(&arg_str(args, "command")?)?;
    let targets = broadcast_targets(arg_id_list(args, "server_ids")?, arg_bool(args, "all_running", false)?)?;
    let only_running = arg_bool(args, "only_running", true)?;
    let overview = core.list_servers().await?;
    let servers: Vec<FleetServer> = overview.servers.iter().map(FleetServer::from).collect();
    let candidates = broadcast_candidates(&targets, &servers);
    let tasks: Vec<_> = candidates
        .into_iter()
        .map(|id| async move { (id, core.server_live(id).await.map(|l| l.state).map_err(|e| e.to_string())) })
        .collect();
    let states: HashMap<i64, std::result::Result<String, String>> =
        stream::iter(tasks).buffer_unordered(API_CONCURRENCY).collect().await;
    let sel = select_broadcast(&targets, &servers, &states, only_running)?;
    let command = command.as_str();
    let tasks: Vec<_> = sel
        .send
        .into_iter()
        .enumerate()
        .map(|(i, mut o)| async move {
            o.result = match core.send_command(o.server_id, command).await {
                Ok(()) => "envoyée".into(),
                Err(e) => format!("échec : {e}"),
            };
            (i, o)
        })
        .collect();
    let mut sent: Vec<(usize, Outcome)> = stream::iter(tasks).buffer_unordered(API_CONCURRENCY).collect().await;
    sent.sort_by_key(|(i, _)| *i);
    let sent: Vec<Outcome> = sent.into_iter().map(|(_, o)| o).collect();
    Ok(render_broadcast(&sent, &sel.skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PlayerFlags;

    fn entry(path: &str, kind: EntryKind, size: u64, modified: Option<i64>) -> TreeEntry {
        TreeEntry { path: path.into(), kind, size, modified }
    }
    fn file(path: &str, size: u64) -> TreeEntry {
        entry(path, EntryKind::File, size, Some(1_000))
    }
    fn dir(path: &str) -> TreeEntry {
        entry(path, EntryKind::Dir, 0, Some(1_000))
    }

    fn copy_args(src: i64, paths: &[&str], dst: i64, target: &str) -> Value {
        json!({ "source_server_id": src, "source_paths": paths, "target_server_id": dst, "target_dir": target })
    }

    #[test]
    fn paths_are_normalized_and_parent_refs_refused() {
        assert_eq!(normalize_path("world/region/").unwrap(), "/world/region");
        assert_eq!(normalize_path(" /a//./b ").unwrap(), "/a/b");
        assert_eq!(normalize_path("/").unwrap(), "/");
        assert!(normalize_path("/a/../etc").is_err());
        assert!(normalize_path("..\\x").is_err());
        assert!(normalize_path("  ").is_err());
        assert!(normalize_path("/a\nb").is_err());
        assert!(is_within("/a/b", "/a") && is_within("/a", "/a") && !is_within("/ab", "/a"));
        assert_eq!(rel_of("/", "/x/y"), "x/y");
        assert_eq!(rel_of("/w", "/w/r/a.mca"), "r/a.mca");
        assert_eq!(rel_of("/w", "/w"), "");
        assert_eq!(dest_root("/", "/plugins/Ess"), "/Ess");
        assert_eq!(dest_root("/backup", "/world"), "/backup/world");
    }

    #[test]
    fn copy_request_refuses_root_self_copy_and_ambiguities() {
        let ok = copy_request(&copy_args(1, &["/world"], 2, "/")).unwrap();
        assert_eq!((ok.sources.clone(), ok.target_dir.as_str(), ok.overwrite, ok.strategy), (vec!["/world".to_string()], "/", false, Strategy::Auto));
        // Même chemin d'un serveur à un autre : légitime.
        assert!(copy_request(&copy_args(1, &["/plugins/x.yml"], 2, "/plugins")).is_ok());

        let err = |v: Value| copy_request(&v).unwrap_err().to_string();
        assert!(err(copy_args(1, &["/"], 2, "/x")).contains("racine"));
        assert!(err(copy_args(1, &[], 2, "/x")).contains("vide"));
        assert!(err(copy_args(1, &["/world"], 1, "/")).contains("lui-même"), "même serveur, même dossier");
        assert!(err(copy_args(1, &["/world"], 1, "/world/backup")).contains("dans lui-même"));
        assert!(err(copy_args(1, &["/world"], 1, "/world")).contains("dans lui-même"));
        assert!(err(copy_args(1, &["/a/x", "/b/x"], 2, "/")).contains("même endroit"));
        assert!(err(copy_args(1, &["/a", "/a/b"], 2, "/")).contains("recouvrent"));
        assert!(err(copy_args(1, &["/a/../b"], 2, "/")).contains(".."));
        let mut v = copy_args(1, &["/a"], 2, "/");
        v["strategy"] = json!("rsync");
        assert!(err(v).contains("strategy"));
        let mut v = copy_args(1, &["/a"], 2, "/");
        v["overwrite"] = json!("peut-être");
        assert!(err(v).contains("booléen"));
        // Même serveur, autre dossier : permis (dupliquer un monde).
        let dup = copy_request(&copy_args(1, &["/world"], 1, "/backups")).unwrap();
        assert!(dup.same_server());
        let mut v = copy_args(1, &["/a"], 2, "/");
        v["overwrite"] = json!("true");
        assert!(copy_request(&v).unwrap().overwrite);
    }

    fn tree(root: &str, entries: Vec<TreeEntry>) -> SourceTree {
        SourceTree { root: root.into(), kind: EntryKind::Dir, size: 0, entries }
    }

    #[test]
    fn plan_creates_dirs_first_and_never_overwrites_without_permission() {
        let src = tree("/plugins/Ess", vec![
            file("/plugins/Ess/config.yml", 10),
            dir("/plugins/Ess/data"),
            file("/plugins/Ess/data/u.yml", 5),
            entry("/plugins/Ess/lien", EntryKind::Other, 0, None),
        ]);
        // Cible vide : tout est créé, parents d'abord.
        let plan = plan_copy(std::slice::from_ref(&src), "/plugins", &Existing::new(), false);
        assert_eq!(plan.dirs, vec!["/plugins/Ess", "/plugins/Ess/data"]);
        let dsts: Vec<&str> = plan.files.iter().map(|f| f.dst.as_str()).collect();
        assert_eq!(dsts, vec!["/plugins/Ess/config.yml", "/plugins/Ess/data/u.yml"]);
        assert_eq!((plan.bytes, plan.extra_bytes), (15, 15));
        assert_eq!(plan.skipped.len(), 1, "lien symbolique ignoré");
        assert!(plan.skipped[0].reason.contains("lien"));

        // Un fichier existe déjà : ignoré sans overwrite, remplacé avec.
        let mut existing = Existing::new();
        existing.insert("/plugins/Ess".into(), (EntryKind::Dir, 0));
        existing.insert("/plugins/Ess/config.yml".into(), (EntryKind::File, 8));
        let plan = plan_copy(std::slice::from_ref(&src), "/plugins", &existing, false);
        assert_eq!(plan.dirs, vec!["/plugins/Ess/data"], "dossier existant réutilisé");
        assert!(plan.files.iter().all(|f| f.dst != "/plugins/Ess/config.yml"));
        assert!(plan.skipped.iter().any(|s| s.path == "/plugins/Ess/config.yml" && s.reason.contains("overwrite=false")));
        let plan = plan_copy(std::slice::from_ref(&src), "/plugins", &existing, true);
        let replaced = plan.files.iter().find(|f| f.dst == "/plugins/Ess/config.yml").unwrap();
        assert!(replaced.replace);
        assert_eq!(plan.extra_bytes, 2 + 5, "un remplacement compte pour sa différence");
    }

    #[test]
    fn plan_never_swaps_a_file_and_a_directory() {
        let src = tree("/w", vec![dir("/w/region"), file("/w/region/r.0.0.mca", 4), file("/w/level.dat", 1)]);
        let mut existing = Existing::new();
        existing.insert("/dst/w".into(), (EntryKind::Dir, 0));
        existing.insert("/dst/w/region".into(), (EntryKind::File, 3)); // un fichier à la place du dossier
        existing.insert("/dst/w/level.dat".into(), (EntryKind::Dir, 0)); // un dossier à la place du fichier
        let plan = plan_copy(std::slice::from_ref(&src), "/dst", &existing, true);
        assert!(plan.files.is_empty() && plan.dirs.is_empty(), "{plan:?}");
        let reason = |p: &str| plan.skipped.iter().find(|s| s.path == p).map(|s| s.reason.clone()).unwrap();
        assert!(reason("/w/region").contains("un fichier porte déjà ce nom"));
        assert!(reason("/w/region/r.0.0.mca").contains("non créable"), "contenu d'un dossier bloqué");
        assert!(reason("/w/level.dat").contains("jamais remplacé"));

        // Source fichier vers un dossier existant du même nom.
        let one = SourceTree { root: "/server.properties".into(), kind: EntryKind::File, size: 3, entries: vec![] };
        let mut existing = Existing::new();
        existing.insert("/server.properties".into(), (EntryKind::Dir, 0));
        let plan = plan_copy(&[one], "/", &existing, true);
        assert!(plan.files.is_empty() && plan.skipped.len() == 1);
    }

    fn facts(files: usize, bytes: u64) -> StrategyFacts {
        StrategyFacts { files, bytes, truncated: false, same_parent: true, top_conflict: false, overwrite: false }
    }

    #[test]
    fn strategy_auto_prefers_archive_for_big_copies_when_possible() {
        assert_eq!(choose_strategy(Strategy::Auto, &facts(10, MIB)).unwrap().0, Strategy::Files);
        assert_eq!(choose_strategy(Strategy::Auto, &facts(1_000, MIB)).unwrap().0, Strategy::Archive);
        assert_eq!(choose_strategy(Strategy::Auto, &facts(3, GIB)).unwrap().0, Strategy::Archive);
        // Destination existante sans overwrite : l'extraction écraserait → fichier par fichier.
        let f = StrategyFacts { top_conflict: true, ..facts(1_000, MIB) };
        let (s, why) = choose_strategy(Strategy::Auto, &f).unwrap();
        assert_eq!(s, Strategy::Files);
        assert!(why.contains("archive impossible"), "{why}");
        // … sauf au-delà des plafonds : refus motivé.
        let f = StrategyFacts { top_conflict: true, ..facts(MAX_COPY_FILES + 1, MIB) };
        assert!(choose_strategy(Strategy::Auto, &f).unwrap_err().to_string().contains("overwrite"));
        let f = StrategyFacts { top_conflict: true, overwrite: true, ..facts(MAX_COPY_FILES + 1, MIB) };
        assert_eq!(choose_strategy(Strategy::Auto, &f).unwrap().0, Strategy::Archive);
        // Sources de dossiers différents : pas d'archive possible.
        let f = StrategyFacts { same_parent: false, truncated: true, ..facts(10, MIB) };
        assert!(choose_strategy(Strategy::Auto, &f).unwrap_err().to_string().contains("même dossier"));
        // Explicite.
        assert!(choose_strategy(Strategy::Files, &facts(MAX_COPY_FILES + 1, 1)).is_err());
        assert!(choose_strategy(Strategy::Files, &facts(1, MAX_COPY_BYTES + 1)).is_err());
        assert!(choose_strategy(Strategy::Files, &StrategyFacts { truncated: true, ..facts(1, 1) }).is_err());
        assert_eq!(choose_strategy(Strategy::Archive, &facts(1, 1)).unwrap().0, Strategy::Archive);
        assert!(choose_strategy(Strategy::Archive, &StrategyFacts { top_conflict: true, ..facts(1, 1) }).is_err());
        assert!(same_parent(&["/a/x".into(), "/a/y".into()]) && !same_parent(&["/a/x".into(), "/b/y".into()]));
        assert!(same_parent(&["/world".into(), "/world_nether".into()]));
    }

    #[test]
    fn extracted_content_is_found_under_either_archive_layout() {
        assert_eq!(extracted_candidates("/t/s/contenu", "/world"), vec!["/t/s/contenu/world"]);
        assert_eq!(
            extracted_candidates("/t/s/contenu", "/plugins/Ess"),
            vec!["/t/s/contenu/Ess", "/t/s/contenu/plugins/Ess"]
        );
        assert_eq!(source_path_of("/s/c/Ess/a/b.yml", "/s/c/Ess", "/plugins/Ess"), "/plugins/Ess/a/b.yml");
        assert_eq!(source_path_of("/s/c/Ess", "/s/c/Ess", "/plugins/Ess"), "/plugins/Ess");
    }

    #[test]
    fn only_staging_dirs_and_temp_siblings_are_app_owned() {
        assert!(is_staging_dir("/plugins/minestrator-temp-copy-1a2b-0"));
        assert!(!is_staging_dir("/plugins"));
        assert!(!is_staging_dir("/world/region"));
        assert!(!is_staging_dir("/../minestrator-temp-copy-1"));
        let t = temp_sibling("/plugins/Ess/config.yml");
        assert!(t.starts_with("/plugins/Ess/.config.yml.minestrator-temp-copy-"), "{t}");
        assert!(temp_sibling("/a.txt").starts_with("/.a.txt."), "à la racine");
        assert_ne!(stamp(), stamp());
    }

    #[test]
    fn space_check_uses_server_then_mybox_limits() {
        let mut space = DiskSpace { used_bytes: 9 * GIB, limit_bytes: 10 * GIB, ..Default::default() };
        assert!(space_shortfall(&space, GIB / 2).is_none());
        assert!(space_shortfall(&space, 2 * GIB).unwrap().contains("serveur cible"));
        space.mybox = Some(server_files::MyBoxDiskSpace { used_bytes: 99 * GIB, limit_bytes: 100 * GIB });
        assert!(space_shortfall(&space, GIB / 2).is_none());
        space.limit_bytes = 0; // limite du serveur inconnue : celle de la MyBox s'applique
        assert!(space_shortfall(&space, 2 * GIB).unwrap().contains("MyBox"));
        assert!(space_shortfall(&DiskSpace::default(), 50 * GIB).is_none(), "limites inconnues");
        assert!(space_shortfall(&space, 0).is_none());
    }

    #[test]
    fn copy_report_lists_skipped_and_limits() {
        let req = copy_request(&copy_args(1, &["/world"], 2, "/")).unwrap();
        let mut r = CopyReport::new(Strategy::Files, "demandée".into());
        r.copied_files = 3;
        r.copied_bytes = 2048;
        for i in 0..60 {
            r.skip(&format!("/world/f{i}"), "existe déjà côté cible (overwrite=false)".into());
        }
        let names = HashMap::from([(1, "Survie".to_string())]);
        let v: Value = serde_json::from_str(&render_copy(&req, &names, &r, Duration::from_millis(1234))).unwrap();
        assert_eq!(v["status"], "terminée, avec des éléments ignorés");
        assert_eq!(v["source"]["server"], "« Survie » (#1)");
        assert_eq!(v["target"]["server"], "#2");
        assert_eq!(v["skipped_count"], 60);
        assert_eq!(v["skipped"].as_array().unwrap().len(), MAX_SKIPPED_SHOWN);
        assert_eq!(v["skipped_list_truncated"], true);
        assert_eq!(v["copied"], "2.0 Kio");
        assert_eq!(v["duration_s"], 1.2);
        assert!(v["limits"]["file_by_file"].as_str().unwrap().contains("5000"));
    }

    #[test]
    fn diff_request_defaults_and_refuses_identity() {
        let r = diff_request(&json!({ "server_id": 1, "path": "plugins/x.yml", "other_server_id": 2 })).unwrap();
        assert_eq!((r.a_path.as_str(), r.b_path.as_str(), r.b_server, r.context, r.recursive), ("/plugins/x.yml", "/plugins/x.yml", 2, 3, false));
        let r = diff_request(&json!({ "server_id": 1, "path": "/a", "other_path": "/b", "context_lines": 99 })).unwrap();
        assert_eq!((r.b_server, r.b_path.as_str(), r.context), (1, "/b", 10));
        assert!(diff_request(&json!({ "server_id": 1, "path": "/a" })).is_err(), "rien à comparer");
        assert!(diff_request(&json!({ "server_id": 1, "path": "/a", "other_server_id": "deux" })).is_err());
    }

    fn redact(s: &str) -> String {
        crate::redact::redact(s)
    }

    #[test]
    fn unified_diff_has_context_and_redacts_before_prefixing() {
        let a = "a: 1\nb: 2\npassword: hunter2\nc: 3\nd: 4\n";
        let b = "a: 1\nb: 20\npassword: autre\nc: 3\nd: 4\n";
        let (out, stats) = unified_diff(a, b, "A:/x", "B:/x", 1, &redact);
        assert_eq!((stats.hunks, stats.added, stats.removed, stats.truncated), (1, 2, 2, false));
        assert!(out.starts_with("--- A:/x\n+++ B:/x\n@@ -1,4 +1,4 @@\n"), "{out}");
        assert!(out.contains("-b: 2\n-password: [SECRET]\n+b: 20\n+password: [SECRET]\n"), "{out}");
        assert!(!out.contains("hunter2") && !out.contains("autre"), "secret jamais renvoyé : {out}");
        assert!(out.contains(" c: 3\n") && !out.contains("d: 4"), "contexte d'une ligne : {out}");
    }

    #[test]
    fn unified_diff_is_capped_but_counts_everything() {
        let a: String = (0..1_000).map(|i| format!("ligne {i}\n")).collect();
        let b: String = (0..1_000).map(|i| format!("LIGNE {i}\n")).collect();
        let (out, stats) = unified_diff(&a, &b, "A", "B", 3, &|s| s.to_string());
        assert!(stats.truncated);
        assert_eq!((stats.added, stats.removed), (1_000, 1_000));
        assert!(out.lines().count() <= MAX_DIFF_LINES + 2, "{}", out.lines().count());
    }

    #[test]
    fn file_comparison_covers_identical_binary_too_big_and_text() {
        let (ea, eb) = (file("/x", 3), file("/x", 4));
        let (a, b) = (Side { label: "A : #1 — /x".into(), entry: &ea }, Side { label: "B : #2 — /x".into(), entry: &eb });
        let same = render_file_comparison(&a, &b, Some((b"abc", b"abc")), 3, &redact);
        assert!(same.contains("identiques"), "{same}");
        let bin = render_file_comparison(&a, &b, Some((b"a\0c", b"a\0d")), 3, &redact);
        assert!(bin.contains("binaire") && !bin.contains("@@"), "{bin}");
        let big = render_file_comparison(&a, &b, None, 3, &redact);
        assert!(big.contains("trop gros") && big.contains("différentes"), "{big}");
        let text = render_file_comparison(&a, &b, Some((b"token: abc\nx\n", b"token: abd\nx\n")), 3, &redact);
        assert!(text.contains("+1 / −1") && text.contains("[SECRET]") && text.contains("secrets masqués"), "{text}");
        assert!(!text.contains("abc") && !text.contains("abd"), "{text}");
        let eol = render_file_comparison(&a, &b, Some((b"x\r\n", b"x\n")), 3, &redact);
        assert!(eol.contains("seules les fins de ligne") && !eol.contains("@@"), "{eol}");
        let mixed = render_file_comparison(&a, &b, Some((b"x\r\ny\r\n", b"x\nz\n")), 3, &redact);
        assert!(mixed.contains("+1 / −1") && mixed.contains("CRLF/LF), ignorées"), "{mixed}");
        assert!(mixed.contains(" x\n-y\n+z\n"), "seule la vraie différence : {mixed}");
        assert!(is_binary(&[0xff, 0xfe]) && !is_binary("éà".as_bytes()));
    }

    #[test]
    fn listings_compare_by_relative_path_and_collapse_new_dirs() {
        let a = vec![
            file("/a/same.yml", 5),
            file("/a/size.yml", 5),
            entry("/a/date.yml", EntryKind::File, 5, Some(1)),
            file("/a/gone.yml", 1),
            dir("/a/logs"),
            dir("/a/kept"),
            file("/a/swap", 1),
        ];
        let b = vec![
            file("/b/same.yml", 5),
            file("/b/size.yml", 6),
            entry("/b/date.yml", EntryKind::File, 5, Some(2)),
            dir("/b/new"),
            file("/b/new/a.txt", 1),
            file("/b/new/b.txt", 1),
            dir("/b/kept"),
            dir("/b/swap"),
            file("/b/logs", 3),
        ];
        let cmp = compare_listings("/a", &a, "/b", &b);
        let paths = |v: &[TreeEntry]| v.iter().map(|e| e.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&cmp.only_b), vec!["new"], "contenu d'un dossier ajouté replié");
        assert_eq!(paths(&cmp.only_a), vec!["gone.yml"]);
        assert_eq!(cmp.resized.len(), 1);
        assert_eq!(cmp.resized[0].0.path, "size.yml");
        assert_eq!(cmp.redated.len(), 1);
        assert_eq!(cmp.type_changed.len(), 2, "logs et swap");
        assert_eq!(cmp.identical, 2, "same.yml et le dossier kept");
        let text = render_dir_comparison("A : #1 — /a", "B : #2 — /b", true, (a.len(), b.len()), false, &cmp);
        assert!(text.contains("1 ajouté(s) dans B, 1 absent(s) de B, 1 modifié(s)"), "{text}");
        assert!(text.contains("+ new/ (dossier)") && text.contains("- gone.yml") && text.contains("~ size.yml"), "{text}");
        assert!(text.contains("! logs : dossier dans A, fichier dans B"), "{text}");
        let partial = render_dir_comparison("A", "B", false, (0, 0), true, &DirComparison::default());
        assert!(partial.contains("PARTIELLE") && partial.contains("premier niveau"));
    }

    fn player(uuid: &str, name: Option<&str>, online: bool, play: i64, last: i64) -> KnownPlayer {
        KnownPlayer {
            uuid: uuid.into(),
            name: name.map(str::to_string),
            online,
            play_time_secs: Some(play),
            last_seen: Some(last),
            flags: PlayerFlags { op: false, whitelisted: true, banned: false },
        }
    }

    const U1: &str = "0f9c1f9e-1111-4c4c-9b9b-000000000001";
    const U2: &str = "0f9c1f9e-1111-4c4c-9b9b-000000000002";

    #[test]
    fn player_query_understands_names_prefixes_uuids_and_aliases() {
        assert_eq!(player_query(" Steve ", false).unwrap(), PlayerQuery::Name { needle: "steve".into(), exact: false });
        assert_eq!(player_query(U1, false).unwrap(), PlayerQuery::Uuid(uuid_key(U1)));
        assert_eq!(player_query("0F9C1F9E11114C4C9B9B000000000001", true).unwrap(), PlayerQuery::Uuid(uuid_key(U1)));
        let alias = crate::redact::pseudonym(U1);
        assert_eq!(player_query(&alias.to_uppercase(), false).unwrap(), PlayerQuery::Alias(alias.clone()));
        assert!(player_query("s", false).is_err(), "préfixe trop court");
        assert!(player_query("s", true).is_ok(), "exact d'un caractère permis");
        assert!(player_query("   ", false).is_err());
        assert!(player_query("a\nb", false).is_err());

        let p = player(U1, Some("Steve_42"), false, 0, 0);
        assert_eq!(match_player(&p, &player_query("steve_42", false).unwrap()), Some(MatchKind::Exact));
        assert_eq!(match_player(&p, &player_query("ste", false).unwrap()), Some(MatchKind::Prefix));
        assert_eq!(match_player(&p, &player_query("ste", true).unwrap()), None);
        assert_eq!(match_player(&p, &player_query("eve", false).unwrap()), None, "préfixe, pas sous-chaîne");
        assert_eq!(match_player(&p, &player_query(U1, false).unwrap()), Some(MatchKind::Uuid));
        assert_eq!(match_player(&p, &PlayerQuery::Alias(alias)), Some(MatchKind::Alias));
        let unnamed = player(U2, None, false, 0, 0);
        assert_eq!(match_player(&unnamed, &player_query("ste", false).unwrap()), None);
    }

    fn roster(players: Vec<KnownPlayer>, online_known: bool) -> Roster {
        Roster { players, playerdata_found: true, other_worlds: false, online_known, truncated: false }
    }

    #[test]
    fn player_report_groups_by_player_across_servers() {
        let now = 10 * 86_400;
        let survie = roster(vec![player(U1, Some("Steve"), true, 7_200, now), player(U2, Some("Stevenson"), false, 360, now - 3 * 86_400)], true);
        let crea = roster(vec![player(U1, Some("Steve"), false, 3_600, now - 86_400)], false);
        let views = [
            RosterView { server_id: 1, server: "Survie", roster: &survie, online_count: Some(1) },
            RosterView { server_id: 2, server: "Créatif", roster: &crea, online_count: Some(0) },
        ];
        let notes = vec![ServerNote { server_id: 3, server: "Satis".into(), reason: "jeu sans liste".into() }];
        let q = player_query("steve", false).unwrap();
        let v = player_report(&q, &views, &notes, now, false, 10);
        assert_eq!(v["matching_players"], 2);
        assert_eq!(v["servers_searched"], 2);
        assert_eq!(v["servers_skipped"][0]["server"], "Satis");
        let first = &v["players"][0];
        assert_eq!(first["player"], "Steve", "pseudo exact d'abord");
        assert_eq!(first["match"], "pseudo exact");
        assert_eq!(first["online_now"], true);
        assert_eq!(first["online_on"], json!(["Survie"]));
        assert_eq!(first["total_play_time_hours"], 3.0);
        assert_eq!(first["servers"].as_array().unwrap().len(), 2);
        assert_eq!(first["servers"][0]["server"], "Survie", "connecté d'abord");
        // Le serveur ne dit pas qui est connecté, mais personne n'y est : hors ligne.
        assert_eq!(first["servers"][1]["online"], false);
        assert_eq!(first["servers"][1]["days_since_last_seen"], 1);
        assert_eq!(v["players"][1]["match"], "début de pseudo");
        // Inconnu si personne ne sait dire qui est connecté.
        let views2 = [RosterView { server_id: 2, server: "Créatif", roster: &crea, online_count: Some(3) }];
        let v2 = player_report(&q, &views2, &[], now, false, 10);
        assert!(v2["players"][0]["online_now"].is_null());
        // Plafond de la liste.
        assert_eq!(player_report(&q, &views, &[], now, false, 1)["shown"], 1);
    }

    #[test]
    fn player_report_hides_names_and_uuids_when_anonymized() {
        let now = 1_000_000;
        let r = roster(vec![player(U1, Some("Steve"), true, 3_600, now)], true);
        let views = [RosterView { server_id: 1, server: "Survie", roster: &r, online_count: None }];
        let v = player_report(&player_query("Steve", true).unwrap(), &views, &[], now, true, 10);
        let text = v.to_string();
        assert!(!text.contains("Steve") && !text.contains("0f9c1f9e"), "{text}");
        assert_eq!(v["players"][0]["player"], crate::redact::pseudonym(U1));
        assert_eq!(v["names_anonymized"], true);
        // L'alias renvoyé permet de relancer la recherche sur tous les serveurs.
        let alias = v["players"][0]["player"].as_str().unwrap();
        let again = player_report(&player_query(alias, false).unwrap(), &views, &[], now, true, 10);
        assert_eq!(again["matching_players"], 1);
        assert_eq!(again["players"][0]["match"], "alias");
    }

    fn server(id: i64, name: &str, status: &str, player_data: bool) -> FleetServer {
        FleetServer { id, name: name.into(), status: status.into(), player_data, game: "Paper".into() }
    }

    #[test]
    fn player_servers_skip_other_games_and_unavailable_servers() {
        let servers = [
            server(1, "Survie", "active", true),
            server(2, "Satis", "active", false),
            server(3, "Vieux", "expired", true),
            server(4, "Dodo", "hibernation", true),
        ];
        let (keep, notes) = player_servers(&servers, None);
        assert_eq!(keep.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1, 4]);
        assert_eq!(notes.len(), 2);
        let (keep, notes) = player_servers(&servers, Some(&[4, 99]));
        assert_eq!(keep.iter().map(|s| s.id).collect::<Vec<_>>(), vec![4]);
        assert!(notes.iter().any(|n| n.server_id == 99 && n.reason.contains("inconnu")));
    }

    #[test]
    fn commands_are_single_line_and_bounded() {
        assert_eq!(validate_command("  say Bonjour à tous  ").unwrap(), "say Bonjour à tous");
        assert!(validate_command("").is_err());
        assert!(validate_command("   ").is_err());
        assert!(validate_command("say a\nstop").is_err());
        assert!(validate_command("say a\rstop").is_err());
        assert!(validate_command("say \u{1b}[31m").is_err());
        assert!(validate_command(&"x".repeat(MAX_COMMAND_CHARS + 1)).is_err());
    }

    #[test]
    fn all_servers_must_be_asked_for_explicitly() {
        assert!(broadcast_targets(None, false).unwrap_err().to_string().contains("all_running"));
        assert!(broadcast_targets(Some(vec![]), false).is_err(), "liste vide ≠ tous");
        assert_eq!(broadcast_targets(Some(vec![]), true).unwrap(), Targets::AllRunning);
        assert_eq!(broadcast_targets(None, true).unwrap(), Targets::AllRunning);
        assert!(broadcast_targets(Some(vec![1]), true).is_err(), "ambigu");
        assert_eq!(broadcast_targets(Some(vec![3, 1, 3]), false).unwrap(), Targets::Ids(vec![3, 1]));
        let many: Vec<i64> = (0..=MAX_BROADCAST as i64).collect();
        assert!(broadcast_targets(Some(many), false).is_err());
        assert!(arg_id_list(&json!({ "server_ids": [1, "2"] }), "server_ids").is_err());
        assert_eq!(arg_id_list(&json!({ "server_ids": 7 }), "server_ids").unwrap(), Some(vec![7]));
        assert_eq!(arg_id_list(&json!({}), "server_ids").unwrap(), None);
    }

    fn states(list: &[(i64, &str)]) -> HashMap<i64, std::result::Result<String, String>> {
        list.iter().map(|(id, s)| (*id, Ok(s.to_string()))).collect()
    }

    #[test]
    fn broadcast_goes_only_to_running_servers_by_default() {
        let servers = [
            server(1, "Survie", "active", true),
            server(2, "Créa", "active", true),
            server(3, "Vieux", "expired", true),
            server(4, "Mod", "active", false),
        ];
        let mut st = states(&[(1, "online"), (2, "offline")]);
        st.insert(4, Err("délai dépassé".into()));
        let t = broadcast_targets(Some(vec![2, 1, 3, 4, 99]), false).unwrap();
        assert_eq!(broadcast_candidates(&t, &servers), vec![2, 1, 4], "connus et actifs");
        let sel = select_broadcast(&t, &servers, &st, true).unwrap();
        assert_eq!(sel.send.iter().map(|o| o.server_id).collect::<Vec<_>>(), vec![1]);
        let why = |id: i64| sel.skipped.iter().find(|o| o.server_id == id).unwrap().result.clone();
        assert!(why(2).contains("non démarré (offline)"));
        assert!(why(3).contains("indisponible (expired)"));
        assert!(why(4).contains("état illisible"), "un état illisible n'est pas un feu vert");
        assert!(why(99).contains("inconnu"));
        // Sans le filtre : même les serveurs arrêtés (connus et actifs) reçoivent la commande.
        let sel = select_broadcast(&t, &servers, &st, false).unwrap();
        assert_eq!(sel.send.iter().map(|o| o.server_id).collect::<Vec<_>>(), vec![2, 1, 4]);
        // « Tous » : seulement ceux qui tournent, même si only_running=false.
        let all = select_broadcast(&Targets::AllRunning, &servers, &st, false).unwrap();
        assert_eq!(all.send.iter().map(|o| o.server_id).collect::<Vec<_>>(), vec![1]);
        let report: Value = serde_json::from_str(&render_broadcast(
            &[Outcome { server_id: 1, server: "Survie".into(), state: Some("online".into()), result: "envoyée".into() }],
            &all.skipped,
        ))
        .unwrap();
        assert_eq!((report["sent"].as_u64(), report["failed"].as_u64(), report["skipped"].as_u64()), (Some(1), Some(0), Some(3)));
    }

    #[test]
    fn broadcasting_to_all_is_capped_after_selection() {
        let servers: Vec<FleetServer> = (0..=MAX_BROADCAST as i64).map(|i| server(i, &format!("s{i}"), "active", true)).collect();
        let st: HashMap<_, _> = servers.iter().map(|s| (s.id, Ok("online".to_string()))).collect();
        assert!(select_broadcast(&Targets::AllRunning, &servers, &st, true).is_err());
        let few: HashMap<_, _> = st.into_iter().filter(|(id, _)| *id < 3).collect();
        assert_eq!(select_broadcast(&Targets::AllRunning, &servers, &few, true).unwrap().send.len(), 3);
    }
}
