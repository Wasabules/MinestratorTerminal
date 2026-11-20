//! Outil MCP `sync_dir` : synchronise un dossier de l'**ordinateur** vers un dossier d'un serveur
//! (configs et plugins tenus dans un dépôt git, par exemple).
//!
//! - **Parcours local** ([`scan_local`]) : liens symboliques jamais suivis, `.git/` exclu d'office,
//!   motifs `include` / `exclude` ([`Filters`]), plafonds de nombre et de taille.
//! - **Plan** ([`plan_sync`], pur) : ajoutés, modifiés (taille différente), à départager (même
//!   taille), conflits de nature (fichier contre dossier), suppressions (`delete_extra` : seulement
//!   sous `remote_dir` et dans la portée des motifs ; jamais un fichier présent localement, même
//!   ignoré).
//! - **Lecture distante économe** : sans `delete_extra`, seuls les dossiers distants qui
//!   correspondent à un dossier local sont listés (non récursif) — synchroniser des configs vers la
//!   racine ne parcourt pas les mondes.
//! - **Empreintes** : à taille égale, sha256 du fichier local et du fichier distant. L'API ne donne
//!   aucune empreinte d'un fichier quelconque (seulement le sha1 des jars de `plugins/` et `mods/`) :
//!   le fichier distant est lu par SFTP, plafonné ([`HASH_FILE_BYTES`], [`HASH_TOTAL_BYTES`]) ;
//!   au-delà, il est renvoyé (« empreinte non vérifiée »).
//! - **Effets** : chaque envoi et chaque suppression passent par la garde des agents
//!   (`agent_guard::guarded_write` : `upload_file`, `write_file`, `delete_path`), une étape
//!   annulable chacun ; l'outil lui-même n'a pas d'étape (`Plan::Skip`). Envois d'abord, puis
//!   suppressions ; arrêt au premier échec.
//! - **Point d'accroche** [`TEXT_HOOK`] : traitement des fichiers texte avant comparaison et envoi
//!   (références de secrets `{{secret:NOM}}` résolues par `agent_secrets.rs`).
//!
//! Le côté serveur est abstrait par [`ServerSide`] (partagé avec `world_upload`) : SFTP et garde
//! en production ([`CoreSide`]), faux serveur de la garde dans les tests.

use crate::agent_guard::undo::{sha_hex, Meta};
use crate::mcp_ctx;
use crate::Core;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
/// Fichiers locaux synchronisés au plus (après filtres).
pub(crate) const MAX_FILES: usize = 5_000;
/// Taille d'un fichier envoyé au plus (au-delà : ignoré et signalé). Sous le plafond de
/// `upload_file` (512 Mio, fichier lu en mémoire).
pub(crate) const MAX_FILE_BYTES: u64 = 128 * MIB;
/// Octets envoyés au plus par appel.
pub(crate) const MAX_TOTAL_BYTES: u64 = 1024 * MIB;
/// Entrées distantes listées au plus.
pub(crate) const MAX_REMOTE_ENTRIES: usize = 20_000;
/// Fichier distant lu pour son empreinte au plus (à taille égale) ; au-delà, renvoyé.
pub(crate) const HASH_FILE_BYTES: u64 = 32 * MIB;
/// Octets distants lus pour les empreintes au plus, par appel.
pub(crate) const HASH_TOTAL_BYTES: u64 = 256 * MIB;
/// Suppressions au plus par appel (`delete_extra`) : au-delà, `remote_dir` est sans doute faux.
pub(crate) const MAX_DELETIONS: usize = 500;
/// Fichier texte soumis au point d'accroche au plus.
pub(crate) const TEXT_HOOK_MAX_BYTES: u64 = 2 * MIB;
/// Motifs exclus d'office.
pub(crate) const DEFAULT_EXCLUDES: &[&str] = &[".git/"];
/// Chemins détaillés par catégorie dans le rapport (le compte, lui, est toujours exact).
const LIST_CAP: usize = 200;
/// Motifs au plus par liste (`include`, `exclude`).
const MAX_PATTERNS: usize = 100;
/// Mentions de la garde gardées dans le rapport.
const GUARD_NOTES_CAP: usize = 20;

// --- Point d'accroche : fichiers texte avant envoi -------------------------------------------

/// Traitement d'un fichier **texte** local avant comparaison et envoi : `(core, server_id,
/// chemin relatif, texte)`.
/// - `Ok(None)` : fichier envoyé tel quel (`upload_file`) ;
/// - `Ok(Some(texte))` : CE texte est comparé au fichier distant puis écrit (`write_file`) à la
///   place du contenu local — il n'apparaît jamais dans la sortie (tailles seulement) ;
/// - `Err(raison)` : fichier refusé ; toute la synchronisation s'arrête avant le premier effet.
///
/// Appelé seulement pour les fichiers UTF-8 sans octet nul de [`TEXT_HOOK_MAX_BYTES`] au plus.
pub(crate) type TextHook = fn(&Core, i64, &str, &str) -> Result<Option<String>, String>;

/// **Point d'accroche** de `sync_dir` : résolution des références de secrets `{{secret:NOM}}`
/// (`agent_secrets.rs`) ; le texte résolu est comparé et écrit, jamais affiché.
pub(crate) const TEXT_HOOK: Option<TextHook> = Some(crate::agent_secrets::sync_text_hook);

/// Forme du point d'accroche vue par la logique générique (sans `Core`).
pub(crate) type Prepare<'a> = &'a (dyn Fn(&str, &str) -> Result<Option<String>, String> + Sync);

// --- Côté serveur ------------------------------------------------------------------------------

/// Côté serveur d'un envoi depuis l'ordinateur (`sync_dir`, `upload_world`) : lectures directes
/// (SFTP) et écritures nommées comme les outils MCP modifiants, passées par la garde des agents.
pub(crate) trait ServerSide: Sync {
    /// État d'un chemin ; `None` s'il n'existe pas.
    fn probe(&self, path: &str) -> impl Future<Output = Result<Option<Meta>, String>> + Send;
    /// Entrées SOUS `root` (dossiers compris, liens jamais suivis) : `(entrées, tronqué)`.
    fn walk(
        &self,
        root: &str,
        recursive: bool,
        max: usize,
    ) -> impl Future<Output = Result<(Vec<Meta>, bool), String>> + Send;
    fn read(&self, path: &str, cap: u64) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
    /// Sous-écriture : outil MCP modifiant et ses arguments (`server_id` compris). Une étape de
    /// la garde des agents ; rend sa mention (« (Garde des agents : …) »), s'il y en a une.
    fn write(
        &self,
        tool: &'static str,
        args: Value,
    ) -> impl Future<Output = Result<Option<String>, String>> + Send;
}

/// Serveur réel : SFTP du compte actif, écritures par `agent_guard::guarded_write`.
pub(crate) struct CoreSide<'a> {
    pub(crate) core: &'a Core,
    pub(crate) id: i64,
}

fn tree_meta(e: crate::sftp::TreeEntry) -> Meta {
    use crate::sftp::EntryKind;
    Meta {
        is_dir: e.kind == EntryKind::Dir,
        special: e.kind == EntryKind::Other,
        size: e.size,
        mtime: e.modified,
        path: e.path,
    }
}

impl ServerSide for CoreSide<'_> {
    async fn probe(&self, path: &str) -> Result<Option<Meta>, String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::probe(&conn, path).await })
            .await
            .map(|o| o.map(tree_meta))
            .map_err(|e| e.to_string())
    }
    async fn walk(&self, root: &str, recursive: bool, max: usize) -> Result<(Vec<Meta>, bool), String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::walk_tree(&conn, root, recursive, max).await })
            .await
            .map(|(entries, truncated)| (entries.into_iter().map(tree_meta).collect(), truncated))
            .map_err(|e| e.to_string())
    }
    async fn read(&self, path: &str, cap: u64) -> Result<Vec<u8>, String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::read_bytes(&conn, path, cap).await })
            .await
            .map_err(|e| e.to_string())
    }
    async fn write(&self, tool: &'static str, args: Value) -> Result<Option<String>, String> {
        crate::agent_guard::guarded_write(self.core, tool, args).await
    }
}

// --- Chemins -----------------------------------------------------------------------------------

/// Chemin distant d'un chemin relatif sous `dir` (`/` ou `/a/b`).
pub(crate) fn remote_path(dir: &str, rel: &str) -> String {
    if rel.is_empty() {
        dir.to_string()
    } else if dir == "/" {
        format!("/{rel}")
    } else {
        format!("{dir}/{rel}")
    }
}

/// Chemin relatif d'une entrée distante sous `dir` (`None` : hors de `dir`, ou `dir` lui-même).
fn rel_of(dir: &str, path: &str) -> Option<String> {
    let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
    path.strip_prefix(&prefix).filter(|r| !r.is_empty()).map(str::to_string)
}

/// Dossiers parents d'un chemin relatif, du plus proche de la racine au plus profond
/// (`a/b/c` → `a`, `a/b`).
fn ancestors(rel: &str) -> impl Iterator<Item = &str> {
    rel.match_indices('/').map(move |(i, _)| &rel[..i])
}

fn depth(rel: &str) -> usize {
    rel.matches('/').count()
}

/// Dossier distant normalisé (`/`, `/plugins/X`) ; `..` refusé.
pub(crate) fn remote_dir_of(raw: &str) -> Result<String, String> {
    let r = raw.trim().replace('\\', "/");
    if r.is_empty() {
        return Err("`remote_dir` vide : donne un dossier du serveur, ex. /plugins (ou / pour la racine)".into());
    }
    if r.split('/').any(|s| s == "..") {
        return Err(format!("`remote_dir` « {raw} » : `..` refusé"));
    }
    Ok(crate::server_files::sftp_path(&r))
}

/// Nom de fichier local acceptable côté serveur (Linux) : pas de séparateur ni de caractère de
/// contrôle, ni `.` / `..`.
fn name_ok(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|c| c == '\\' || c == '/' || c.is_control())
}

// --- Motifs glob -------------------------------------------------------------------------------

/// Motif glob sur un chemin relatif séparé par `/` : `*` (dans un segment), `?` (un caractère),
/// `**` (n'importe quel nombre de segments), `/` final = dossiers seulement. Sans `/` (hors final),
/// le motif vise le **nom** à n'importe quelle profondeur (`*.log`) ; avec, il part de la racine du
/// dossier synchronisé (`plugins/*.jar`, `/config/`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Glob {
    segs: Vec<String>,
    anchored: bool,
    dir_only: bool,
}

impl Glob {
    pub(crate) fn parse(raw: &str) -> Result<Glob, String> {
        let p = raw.trim().replace('\\', "/");
        if p.len() > 300 {
            return Err(format!("motif trop long : « {raw} »"));
        }
        let dir_only = p.ends_with('/');
        let body = p.trim_end_matches('/');
        let body = body.strip_prefix("./").unwrap_or(body);
        let anchored = body.starts_with('/') || body.trim_start_matches('/').contains('/');
        let segs: Vec<String> = body.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
        if segs.is_empty() {
            return Err(format!("motif vide : « {raw} »"));
        }
        if segs.iter().any(|s| s == "." || s == "..") {
            return Err(format!("motif « {raw} » : `.` et `..` refusés"));
        }
        Ok(Glob { segs, anchored, dir_only })
    }

    /// Le motif désigne-t-il ce chemin relatif (lui-même, sans ses parents) ?
    pub(crate) fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if self.anchored {
            match_segs(&self.segs, &parts)
        } else {
            parts.last().is_some_and(|name| seg_match(&self.segs[0], name))
        }
    }

    /// Le motif désigne-t-il ce chemin ou l'un de ses dossiers parents ?
    fn matches_any_level(&self, rel: &str, is_dir: bool) -> bool {
        self.matches(rel, is_dir) || ancestors(rel).any(|a| self.matches(a, true))
    }
}

fn match_segs(pat: &[String], parts: &[&str]) -> bool {
    match pat.split_first() {
        None => parts.is_empty(),
        Some((p, rest)) if p == "**" => (0..=parts.len()).any(|i| match_segs(rest, &parts[i..])),
        Some((p, rest)) => parts.first().is_some_and(|first| seg_match(p, first)) && match_segs(rest, &parts[1..]),
    }
}

/// `*` et `?` dans un segment (sensible à la casse, comme le serveur).
fn seg_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Portée d'une synchronisation : motifs `include` (vide = tout) et `exclude` (`.git/` d'office).
#[derive(Debug, Clone)]
pub(crate) struct Filters {
    include: Vec<Glob>,
    exclude: Vec<Glob>,
    pub(crate) include_raw: Vec<String>,
    pub(crate) exclude_raw: Vec<String>,
}

impl Filters {
    pub(crate) fn new(include: &[String], exclude: &[String]) -> Result<Self, String> {
        if include.len() > MAX_PATTERNS || exclude.len() > MAX_PATTERNS {
            return Err(format!("{MAX_PATTERNS} motifs au plus par liste"));
        }
        let mut exclude_raw: Vec<String> = DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect();
        exclude_raw.extend(exclude.iter().filter(|p| !DEFAULT_EXCLUDES.contains(&p.as_str())).cloned());
        Ok(Filters {
            include: include.iter().map(|p| Glob::parse(p)).collect::<Result<_, _>>()?,
            exclude: exclude_raw.iter().map(|p| Glob::parse(p)).collect::<Result<_, _>>()?,
            include_raw: include.to_vec(),
            exclude_raw,
        })
    }

    /// Écarté par `exclude` : le chemin lui-même ou l'un de ses dossiers parents.
    pub(crate) fn excluded(&self, rel: &str, is_dir: bool) -> bool {
        self.exclude.iter().any(|g| g.matches_any_level(rel, is_dir))
    }

    /// Fichier retenu par `include` (vide = tous) : lui-même ou l'un de ses dossiers parents.
    pub(crate) fn included(&self, rel: &str) -> bool {
        self.include.is_empty() || self.include.iter().any(|g| g.matches_any_level(rel, false))
    }

    /// Fichier dans la portée de la synchronisation (donc supprimable s'il manque en local).
    pub(crate) fn file_in_scope(&self, rel: &str) -> bool {
        !self.excluded(rel, false) && self.included(rel)
    }
}

// --- Parcours local ----------------------------------------------------------------------------

/// Fichier local à synchroniser.
#[derive(Debug, Clone)]
pub(crate) struct LocalFile {
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
    /// Taille envoyée (celle du texte préparé par le point d'accroche, le cas échéant).
    pub(crate) size: u64,
    /// Texte préparé par [`TEXT_HOOK`] : écrit à la place du fichier.
    pub(crate) content: Option<String>,
}

/// Ce que le parcours local a trouvé.
#[derive(Debug, Clone, Default)]
pub(crate) struct LocalScan {
    pub(crate) files: Vec<LocalFile>,
    pub(crate) dirs: BTreeSet<String>,
    /// Entrées vues mais non envoyées (raison) : jamais supprimées côté serveur.
    pub(crate) skipped: Vec<(String, String)>,
    /// Entrées écartées par les motifs (non comptées ailleurs).
    pub(crate) excluded: usize,
}

/// Parcourt `root` (bloquant) : fichiers retenus par les motifs, liens symboliques jamais suivis.
pub(crate) fn scan_local(root: &Path, filters: &Filters, max_files: usize, max_file_bytes: u64) -> Result<LocalScan, String> {
    let meta = std::fs::metadata(root).map_err(|e| format!("dossier local « {} » illisible : {e}", root.display()))?;
    if !meta.is_dir() {
        return Err(format!("« {} » n'est pas un dossier", root.display()));
    }
    let mut scan = LocalScan::default();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
            .and_then(|rd| rd.collect::<std::io::Result<Vec<_>>>())
            .map_err(|e| format!("lecture du dossier local « {} » : {e}", dir.display()))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let join = |name: &str| if rel_dir.is_empty() { name.to_string() } else { format!("{rel_dir}/{name}") };
            let name = match e.file_name().into_string() {
                Ok(n) => n,
                Err(raw) => {
                    scan.skipped.push((join(&raw.to_string_lossy()), "nom de fichier non UTF-8".into()));
                    continue;
                }
            };
            let rel = join(&name);
            if !name_ok(&name) {
                scan.skipped.push((rel, "nom de fichier refusé (séparateur ou caractère de contrôle)".into()));
                continue;
            }
            let ft = e.file_type().map_err(|err| format!("« {rel} » : {err}"))?;
            if ft.is_symlink() {
                if !scan_excluded(filters, &rel, false) {
                    scan.skipped.push((rel, "lien symbolique (jamais suivi)".into()));
                }
                continue;
            }
            if ft.is_dir() {
                if filters.excluded(&rel, true) {
                    scan.excluded += 1;
                    continue;
                }
                scan.dirs.insert(rel.clone());
                stack.push((e.path(), rel));
            } else if ft.is_file() {
                if scan_excluded(filters, &rel, false) {
                    scan.excluded += 1;
                    continue;
                }
                let size = e.metadata().map_err(|err| format!("« {rel} » : {err}"))?.len();
                if size > max_file_bytes {
                    scan.skipped.push((rel, format!("{} : au-delà du plafond de {} par fichier", human(size), human(max_file_bytes))));
                    continue;
                }
                scan.files.push(LocalFile { rel, abs: e.path(), size, content: None });
                if scan.files.len() > max_files {
                    return Err(format!(
                        "plus de {max_files} fichiers à synchroniser (plafond) : vise un sous-dossier ou affine include/exclude"
                    ));
                }
            } else if !scan_excluded(filters, &rel, false) {
                scan.skipped.push((rel, "fichier spécial (ni fichier ni dossier)".into()));
            }
        }
    }
    scan.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(scan)
}

fn scan_excluded(filters: &Filters, rel: &str, is_dir: bool) -> bool {
    filters.excluded(rel, is_dir) || (!is_dir && !filters.included(rel))
}

fn human(n: u64) -> String {
    crate::server_files::human_bytes(n)
}

// --- Plan (pur) --------------------------------------------------------------------------------

/// Fichier à envoyer (ou à départager par l'empreinte).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Upload {
    pub(crate) rel: String,
    pub(crate) size: u64,
    pub(crate) remote_size: Option<u64>,
    pub(crate) why: String,
}

/// Écart entre le dossier local et le dossier distant.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SyncPlan {
    pub(crate) add: Vec<Upload>,
    pub(crate) modify: Vec<Upload>,
    /// Même taille des deux côtés : à départager par l'empreinte.
    pub(crate) compare: Vec<Upload>,
    pub(crate) unchanged: usize,
    pub(crate) skipped: Vec<(String, String)>,
    pub(crate) delete_files: Vec<String>,
    /// Dossiers distants vidés par les suppressions et absents en local, du plus profond au moins.
    pub(crate) delete_dirs: Vec<String>,
}

/// Calcule l'écart. `remote` : entrées distantes connues, par chemin relatif (un chemin absent
/// n'existe pas sur le serveur). Les suppressions demandent un listing récursif complet.
pub(crate) fn plan_sync(local: &LocalScan, remote: &BTreeMap<String, Meta>, filters: &Filters, delete_extra: bool) -> SyncPlan {
    let mut plan = SyncPlan { skipped: local.skipped.clone(), ..SyncPlan::default() };
    for f in &local.files {
        if let Some(a) = ancestors(&f.rel).find(|a| remote.get(*a).is_some_and(|m| !m.is_dir)) {
            plan.skipped.push((f.rel.clone(), format!("« {a} » n'est pas un dossier sur le serveur")));
            continue;
        }
        let upload = |why: &str, remote_size: Option<u64>| Upload { rel: f.rel.clone(), size: f.size, remote_size, why: why.into() };
        match remote.get(&f.rel) {
            None => plan.add.push(upload("absent du serveur", None)),
            Some(m) if m.special => plan.skipped.push((f.rel.clone(), "lien ou fichier spécial sur le serveur (jamais remplacé)".into())),
            Some(m) if m.is_dir => plan.skipped.push((f.rel.clone(), "un dossier porte ce nom sur le serveur".into())),
            Some(m) if m.size != f.size => plan.modify.push(upload("taille différente", Some(m.size))),
            Some(m) => plan.compare.push(upload("même taille", Some(m.size))),
        }
    }
    if !delete_extra {
        return plan;
    }
    // Tout ce qui existe localement (envoyé ou non) protège son homologue distant.
    let present: BTreeSet<&str> = local
        .files
        .iter()
        .map(|f| f.rel.as_str())
        .chain(local.dirs.iter().map(String::as_str))
        .chain(local.skipped.iter().map(|(r, _)| r.as_str()))
        .collect();
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    for (rel, m) in remote {
        if m.is_dir || m.special || present.contains(rel.as_str()) || !filters.file_in_scope(rel) {
            continue;
        }
        plan.delete_files.push(rel.clone());
        deleted.insert(rel.clone());
    }
    // Dossiers : absents en local, hors exclusion, et dont tout le contenu distant part.
    let mut dirs: Vec<&String> = remote
        .iter()
        .filter(|(rel, m)| m.is_dir && !present.contains(rel.as_str()) && !filters.excluded(rel, true))
        .map(|(rel, _)| rel)
        .collect();
    dirs.sort_by(|a, b| depth(b).cmp(&depth(a)).then_with(|| a.cmp(b)));
    for d in dirs {
        let prefix = format!("{d}/");
        let emptied = remote
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .all(|(k, _)| deleted.contains(k));
        if emptied {
            plan.delete_dirs.push(d.clone());
            deleted.insert(d.clone());
        }
    }
    plan
}

// --- Requête -----------------------------------------------------------------------------------

/// Arguments de `sync_dir`, vérifiés.
#[derive(Debug, Clone)]
pub(crate) struct SyncRequest {
    pub(crate) server_id: i64,
    pub(crate) local_dir: PathBuf,
    pub(crate) remote_dir: String,
    pub(crate) delete_extra: bool,
    pub(crate) filters: Filters,
    pub(crate) dry_run: bool,
}

/// Booléen facultatif : absent → `None`, autre chose qu'un booléen → erreur.
pub(crate) fn bool_arg(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` doit être un booléen (true ou false)")),
    }
}

fn patterns(args: &Value, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.trim().to_string()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(|| format!("`{key}` : liste de motifs (texte) attendue")))
            .filter(|r| !matches!(r, Ok(s) if s.is_empty()))
            .collect(),
        Some(_) => Err(format!("`{key}` : liste de motifs (texte) attendue")),
    }
}

pub(crate) fn parse_request(args: &Value) -> Result<SyncRequest, String> {
    let server_id = args.get("server_id").and_then(Value::as_i64).ok_or("paramètre `server_id` (entier) requis")?;
    let local = args
        .get("local_dir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("paramètre `local_dir` (dossier de l'ordinateur, chemin absolu) requis")?;
    let local_dir = PathBuf::from(local);
    if !local_dir.is_absolute() {
        return Err(format!("`local_dir` doit être un chemin absolu (reçu « {local} »)"));
    }
    let remote_dir = remote_dir_of(
        args.get("remote_dir")
            .and_then(Value::as_str)
            .ok_or("paramètre `remote_dir` (dossier du serveur, ex. /plugins ou /) requis")?,
    )?;
    let delete_extra = bool_arg(args, "delete_extra")?.unwrap_or(false);
    if delete_extra && remote_dir == "/" {
        return Err(
            "`delete_extra` refusé à la racine du serveur : mondes, journaux et fichiers du jeu seraient supprimés. Vise un sous-dossier (ex. /plugins/MonPlugin) ou synchronise sans delete_extra."
                .into(),
        );
    }
    let filters = Filters::new(&patterns(args, "include")?, &patterns(args, "exclude")?)?;
    Ok(SyncRequest {
        server_id,
        local_dir,
        remote_dir,
        delete_extra,
        filters,
        dry_run: bool_arg(args, "dry_run")?.unwrap_or(true),
    })
}

// --- Exécution ---------------------------------------------------------------------------------

/// Entrées distantes utiles, par chemin relatif. Avec `full` (suppressions) : tout `dir`,
/// récursivement ; sinon seulement les dossiers qui correspondent à un dossier local, sans
/// descendre ailleurs (synchroniser des configs vers `/` ne parcourt pas les mondes).
async fn remote_listing<S: ServerSide>(
    side: &S,
    dir: &str,
    local_dirs: &BTreeSet<String>,
    full: bool,
) -> Result<BTreeMap<String, Meta>, String> {
    let mut map = BTreeMap::new();
    match side.probe(dir).await? {
        None => return Ok(map),
        Some(m) if !m.is_dir => return Err(format!("« {dir} » est un fichier sur le serveur, pas un dossier")),
        Some(_) => {}
    }
    let too_many = || {
        format!("plus de {MAX_REMOTE_ENTRIES} entrées sous « {dir} » sur le serveur (plafond de lecture) : vise un sous-dossier")
    };
    if full {
        let (entries, truncated) = side.walk(dir, true, MAX_REMOTE_ENTRIES).await?;
        if truncated {
            return Err(too_many());
        }
        map.extend(entries.into_iter().filter_map(|m| rel_of(dir, &m.path).map(|r| (r, m))));
        return Ok(map);
    }
    let mut todo: Vec<&str> = std::iter::once("").chain(local_dirs.iter().map(String::as_str)).collect();
    todo.sort_by_key(|d| depth(d) + usize::from(!d.is_empty()));
    let mut seen = 0usize;
    for rel_dir in todo {
        // Un dossier n'est listé que si son homologue distant est un dossier (parent déjà listé).
        if !rel_dir.is_empty() && !map.get(rel_dir).is_some_and(|m: &Meta| m.is_dir && !m.special) {
            continue;
        }
        let abs = remote_path(dir, rel_dir);
        let (entries, truncated) = side.walk(&abs, false, MAX_REMOTE_ENTRIES - seen).await?;
        seen += entries.len();
        if truncated || seen >= MAX_REMOTE_ENTRIES {
            return Err(too_many());
        }
        map.extend(entries.into_iter().filter_map(|m| rel_of(dir, &m.path).map(|r| (r, m))));
    }
    Ok(map)
}

/// Empreinte du contenu envoyé d'un fichier local.
async fn local_sha(f: &LocalFile) -> Result<String, String> {
    match &f.content {
        Some(text) => Ok(sha_hex(text.as_bytes())),
        None => tokio::fs::read(&f.abs)
            .await
            .map(|data| sha_hex(&data))
            .map_err(|e| format!("lecture de « {} » : {e}", f.rel)),
    }
}

/// Départage les fichiers de même taille : sha256 local contre sha256 distant (lecture plafonnée).
async fn resolve_same_size<S: ServerSide>(side: &S, req: &SyncRequest, local: &BTreeMap<&str, &LocalFile>, plan: &mut SyncPlan) -> Result<(), String> {
    let mut budget = HASH_TOTAL_BYTES;
    let total = plan.compare.len();
    for (i, mut u) in std::mem::take(&mut plan.compare).into_iter().enumerate() {
        if mcp_ctx::cancelled() {
            return Err("annulé pendant la comparaison des empreintes : rien n'a été envoyé".into());
        }
        if i % 20 == 0 {
            mcp_ctx::progress(i as f64, Some(total as f64), &format!("empreintes : {i}/{total}")).await;
        }
        let size = u.remote_size.unwrap_or(u.size);
        if size > HASH_FILE_BYTES {
            u.why = format!("même taille, empreinte non vérifiée (fichier de plus de {}) : renvoyé", human(HASH_FILE_BYTES));
            plan.modify.push(u);
            continue;
        }
        if size > budget {
            u.why = format!("même taille, empreinte non vérifiée (plafond de lecture de {} atteint) : renvoyé", human(HASH_TOTAL_BYTES));
            plan.modify.push(u);
            continue;
        }
        budget -= size;
        let f = local.get(u.rel.as_str()).ok_or("fichier local perdu")?;
        let mine = local_sha(f).await?;
        let theirs = side.read(&remote_path(&req.remote_dir, &u.rel), HASH_FILE_BYTES).await.map(|d| sha_hex(&d));
        match theirs {
            Ok(h) if h == mine => plan.unchanged += 1,
            Ok(_) => {
                u.why = "contenu différent (sha256)".into();
                plan.modify.push(u);
            }
            Err(e) => {
                u.why = format!("même taille, fichier distant illisible ({e}) : renvoyé");
                plan.modify.push(u);
            }
        }
    }
    plan.modify.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(())
}

/// Ce que l'exécution a fait.
#[derive(Debug, Default)]
struct Exec {
    uploaded: usize,
    bytes_sent: u64,
    deleted: usize,
    error: Option<String>,
    cancelled: bool,
    not_done: Vec<String>,
    guard: Vec<String>,
}

impl Exec {
    fn note(&mut self, note: Option<String>) {
        if let Some(n) = note {
            if !self.guard.contains(&n) && self.guard.len() < GUARD_NOTES_CAP {
                self.guard.push(n);
            }
        }
    }
}

/// Synchronisation complète sur un côté serveur quelconque. `prepare` : point d'accroche des
/// fichiers texte (voir [`TEXT_HOOK`]).
pub(crate) async fn sync_with<S: ServerSide>(side: &S, req: &SyncRequest, prepare: Option<Prepare<'_>>) -> Result<Value, String> {
    const STEPS: f64 = 4.0;
    mcp_ctx::progress(0.0, Some(STEPS), "parcours du dossier local").await;
    let (root, filters) = (req.local_dir.clone(), req.filters.clone());
    let mut scan = tokio::task::spawn_blocking(move || scan_local(&root, &filters, MAX_FILES, MAX_FILE_BYTES))
        .await
        .map_err(|e| format!("parcours local interrompu : {e}"))??;
    if let Some(prepare) = prepare {
        apply_prepare(&mut scan, prepare).await?;
    }
    if scan.files.iter().any(|f| f.content.is_none() && f.abs.to_str().is_none()) {
        return Err("chemin local non UTF-8 : déplace le dossier vers un chemin sans caractères spéciaux".into());
    }

    mcp_ctx::progress(1.0, Some(STEPS), "lecture du dossier distant").await;
    let remote = remote_listing(side, &req.remote_dir, &scan.dirs, req.delete_extra).await?;
    let mut plan = plan_sync(&scan, &remote, &req.filters, req.delete_extra);

    mcp_ctx::progress(2.0, Some(STEPS), "comparaison des empreintes").await;
    let by_rel: BTreeMap<&str, &LocalFile> = scan.files.iter().map(|f| (f.rel.as_str(), f)).collect();
    resolve_same_size(side, req, &by_rel, &mut plan).await?;

    let to_send: u64 = plan.add.iter().chain(&plan.modify).map(|u| u.size).sum();
    let deletions = plan.delete_files.len() + plan.delete_dirs.len();
    let mut refusals = Vec::new();
    if to_send > MAX_TOTAL_BYTES {
        refusals.push(format!("{} à envoyer : au-delà du plafond de {} par appel (vise un sous-dossier ou affine include/exclude)", human(to_send), human(MAX_TOTAL_BYTES)));
    }
    if deletions > MAX_DELETIONS {
        refusals.push(format!("{deletions} suppressions : au-delà du plafond de {MAX_DELETIONS} (vérifie remote_dir, ou affine include/exclude)"));
    }
    if req.dry_run || !refusals.is_empty() {
        if !req.dry_run {
            return Err(format!("refusé, rien n'a été envoyé : {}", refusals.join(" ; ")));
        }
        mcp_ctx::progress(STEPS, Some(STEPS), "simulation terminée").await;
        return Ok(report(req, &scan, &plan, to_send, None, &refusals));
    }

    mcp_ctx::progress(3.0, Some(STEPS), "envoi").await;
    let mut exec = Exec::default();
    let mut uploads: Vec<&Upload> = plan.add.iter().chain(&plan.modify).collect();
    uploads.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut deletes: Vec<(String, bool)> = plan.delete_files.iter().map(|r| (r.clone(), false)).collect();
    deletes.extend(plan.delete_dirs.iter().map(|r| (r.clone(), true)));
    let total = (uploads.len() + deletes.len()) as f64;
    let mut done = 0f64;
    let stop = |exec: &Exec| exec.error.is_some() || exec.cancelled;
    for u in &uploads {
        if stop(&exec) {
            exec.not_done.push(remote_path(&req.remote_dir, &u.rel));
            continue;
        }
        if mcp_ctx::cancelled() {
            exec.cancelled = true;
            exec.not_done.push(remote_path(&req.remote_dir, &u.rel));
            continue;
        }
        let f = by_rel.get(u.rel.as_str()).ok_or("fichier local perdu")?;
        let path = remote_path(&req.remote_dir, &u.rel);
        mcp_ctx::progress(done, Some(total), &format!("envoi de {path}")).await;
        let (tool, args) = match &f.content {
            Some(text) => ("write_file", json!({ "server_id": req.server_id, "path": path, "content": text })),
            None => (
                "upload_file",
                json!({ "server_id": req.server_id, "local_path": f.abs.to_str().unwrap_or_default(), "remote_path": path }),
            ),
        };
        match side.write(tool, args).await {
            Ok(note) => {
                exec.uploaded += 1;
                exec.bytes_sent += f.size;
                exec.note(note);
            }
            Err(e) => exec.error = Some(format!("envoi de « {path} » : {e}")),
        }
        done += 1.0;
    }
    for (rel, is_dir) in &deletes {
        let path = remote_path(&req.remote_dir, rel);
        if stop(&exec) || mcp_ctx::cancelled() {
            exec.cancelled |= exec.error.is_none();
            exec.not_done.push(path);
            continue;
        }
        mcp_ctx::progress(done, Some(total), &format!("suppression de {path}")).await;
        match side.write("delete_path", json!({ "server_id": req.server_id, "path": path, "is_dir": is_dir })).await {
            Ok(note) => {
                exec.deleted += 1;
                exec.note(note);
            }
            Err(e) => exec.error = Some(format!("suppression de « {path} » : {e}")),
        }
        done += 1.0;
    }
    mcp_ctx::progress(total, Some(total), "synchronisation terminée").await;
    Ok(report(req, &scan, &plan, to_send, Some(&exec), &refusals))
}

/// Point d'accroche appliqué aux fichiers texte ; une erreur arrête tout avant le premier effet.
async fn apply_prepare(scan: &mut LocalScan, prepare: Prepare<'_>) -> Result<(), String> {
    let mut refused = Vec::new();
    for f in scan.files.iter_mut() {
        if f.size > TEXT_HOOK_MAX_BYTES {
            continue;
        }
        let data = tokio::fs::read(&f.abs).await.map_err(|e| format!("lecture de « {} » : {e}", f.rel))?;
        let Ok(text) = std::str::from_utf8(&data) else { continue };
        if text.contains('\0') {
            continue;
        }
        match prepare(&f.rel, text) {
            Ok(None) => {}
            Ok(Some(t)) => {
                f.size = t.len() as u64;
                f.content = Some(t);
            }
            Err(e) => refused.push(format!("{} : {e}", f.rel)),
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(format!("fichier(s) texte refusé(s) avant envoi, rien n'a été envoyé : {}", refused.join(" ; ")))
    }
}

/// Liste plafonnée pour le rapport.
fn capped<T>(items: impl Iterator<Item = T>, total: usize, render: impl Fn(T) -> Value) -> Value {
    let mut out: Vec<Value> = items.take(LIST_CAP).map(render).collect();
    if total > LIST_CAP {
        out.push(json!(format!("… (+{} autres)", total - LIST_CAP)));
    }
    Value::Array(out)
}

fn report(req: &SyncRequest, scan: &LocalScan, plan: &SyncPlan, to_send: u64, exec: Option<&Exec>, refusals: &[String]) -> Value {
    let rp = |rel: &str| remote_path(&req.remote_dir, rel);
    let upload = |u: &Upload| {
        let mut v = json!({ "path": rp(&u.rel), "size": u.size });
        if let Some(r) = u.remote_size {
            v["remote_size"] = json!(r);
            v["why"] = json!(u.why);
        }
        v
    };
    let status = match exec {
        None => "simulation : rien n'a été envoyé",
        Some(e) if e.error.is_some() => "interrompue par une erreur",
        Some(e) if e.cancelled => "annulée en cours de route",
        Some(_) => "terminée",
    };
    let mut out = json!({
        "server_id": req.server_id,
        "local_dir": req.local_dir.display().to_string(),
        "remote_dir": req.remote_dir,
        "dry_run": req.dry_run,
        "status": status,
        "counts": {
            "added": plan.add.len(),
            "modified": plan.modify.len(),
            "unchanged": plan.unchanged,
            "deleted": plan.delete_files.len() + plan.delete_dirs.len(),
            "skipped": plan.skipped.len(),
            "excluded_by_patterns": scan.excluded,
        },
        "bytes_to_send": to_send,
        "added": capped(plan.add.iter(), plan.add.len(), upload),
        "modified": capped(plan.modify.iter(), plan.modify.len(), upload),
        "deleted": capped(
            plan.delete_files.iter().map(|r| (r, false)).chain(plan.delete_dirs.iter().map(|r| (r, true))),
            plan.delete_files.len() + plan.delete_dirs.len(),
            |(r, d)| json!({ "path": rp(r), "is_dir": d }),
        ),
        "skipped": capped(plan.skipped.iter(), plan.skipped.len(), |(r, why)| json!({ "path": rp(r), "why": why })),
        "patterns": { "include": req.filters.include_raw, "exclude": req.filters.exclude_raw },
        "delete_extra": req.delete_extra,
        "limits": {
            "max_files": MAX_FILES,
            "max_file_bytes": MAX_FILE_BYTES,
            "max_total_bytes": MAX_TOTAL_BYTES,
            "max_deletions": MAX_DELETIONS,
            "hash_read_per_file_bytes": HASH_FILE_BYTES,
            "hash_read_total_bytes": HASH_TOTAL_BYTES,
            "max_remote_entries": MAX_REMOTE_ENTRIES,
        },
    });
    if plan.add.iter().chain(&plan.modify).any(|u| by_text(scan, &u.rel)) {
        out["notes"] = json!(["fichier(s) texte préparé(s) avant envoi (point d'accroche) : écrits par write_file, contenu jamais affiché"]);
    }
    match exec {
        None => {
            if !refusals.is_empty() {
                out["refusals"] = json!(refusals);
            }
            out["next"] = json!(if refusals.is_empty() {
                "Montre ce plan à l'utilisateur, puis relance avec dry_run=false pour l'appliquer."
            } else {
                "L'application serait refusée (voir refusals) : corrige d'abord."
            });
        }
        Some(e) => {
            out["done"] = json!({ "uploaded": e.uploaded, "bytes_sent": e.bytes_sent, "deleted": e.deleted });
            if let Some(err) = &e.error {
                out["error"] = json!(err);
            }
            if !e.not_done.is_empty() {
                out["not_done"] = capped(e.not_done.iter(), e.not_done.len(), |p| json!(p));
            }
            if !e.guard.is_empty() {
                out["guard"] = json!(e.guard);
            }
            out["next"] = json!(
                "Chaque envoi et chaque suppression est une étape annulable : list_changesets puis rollback_changeset pour revenir en arrière. Un redémarrage (ou un rechargement du plugin) applique les configs."
            );
        }
    }
    out
}

fn by_text(scan: &LocalScan, rel: &str) -> bool {
    scan.files.iter().any(|f| f.rel == rel && f.content.is_some())
}

/// Outil MCP `sync_dir`.
pub(crate) async fn sync_dir_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = parse_request(args)?;
    let side = CoreSide { core, id: req.server_id };
    let hook = |rel: &str, text: &str| match TEXT_HOOK {
        Some(h) => h(core, req.server_id, rel, text),
        None => Ok(None),
    };
    let prepare: Option<Prepare<'_>> = TEXT_HOOK.map(|_| &hook as Prepare<'_>);
    let report = sync_with(&side, &req, prepare).await?;
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    Ok(core.redact_ai(&text))
}

// --- Faux serveur des tests (partagé avec `world_upload`) ---------------------------------------

#[cfg(test)]
pub(crate) mod testing {
    //! Faux serveur dont chaque écriture passe par la capture de la garde des agents, comme
    //! `mcp::dispatch` (`plan_for` → copie avant → action → constat) : les étapes enregistrées
    //! permettent de vérifier l'annulation de bout en bout.
    use super::*;
    use crate::agent_guard::undo::fake::FakeServer;
    use crate::agent_guard::undo::{capture, finalize, plan_for, undo_ops, Budget, Remote, UndoOp, STEP_CAP};
    use std::sync::Mutex;

    /// Étape enregistrée : outil, opérations inverses, copies.
    type Step = (String, Vec<UndoOp>, Vec<Vec<u8>>);

    pub(crate) struct GuardedFake {
        pub(crate) s: FakeServer,
        pub(crate) calls: Mutex<Vec<(String, Value)>>,
        steps: Mutex<Vec<Step>>,
        /// Chemin dont l'écriture échoue (simulation d'une panne).
        pub(crate) fail_on: Mutex<Option<String>>,
        /// État d'alimentation (`running`, `offline`…).
        pub(crate) state: Mutex<String>,
    }

    impl GuardedFake {
        pub(crate) fn new() -> Self {
            GuardedFake {
                s: FakeServer::new(),
                calls: Mutex::new(Vec::new()),
                steps: Mutex::new(Vec::new()),
                fail_on: Mutex::new(None),
                state: Mutex::new("offline".into()),
            }
        }

        pub(crate) fn tools(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().map(|(t, _)| t.clone()).collect()
        }

        async fn act(&self, tool: &str, args: &Value) -> Result<(), String> {
            let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            match tool {
                "upload_file" => {
                    let data = std::fs::read(text("local_path")).map_err(|e| e.to_string())?;
                    self.s.write(&text("remote_path"), &data).await
                }
                "write_file" => self.s.write(&text("path"), text("content").as_bytes()).await,
                "delete_path" => self.s.remove(&text("path"), args["is_dir"].as_bool().unwrap_or(false)).await,
                "rename_path" => self.s.rename(&text("from"), &text("to")).await,
                "create_dir" => self.s.mkdir_all(&text("path")).await,
                "extract_archive" => {
                    let bytes = self.s.read(&text("path"), u64::MAX).await?;
                    let dest = text("dest_dir");
                    for (name, data) in crate::archive::extract_all(&bytes, crate::archive::ArchiveKind::Zip, 100_000, u64::MAX)? {
                        self.s.write(&remote_path(&dest, &name), &data).await?;
                    }
                    Ok(())
                }
                "set_property" => self.s.set_property(&text("property"), &text("value")).await,
                "power_action" => {
                    let next = match text("action").as_str() {
                        "stop" | "kill" => "offline",
                        _ => "running",
                    };
                    *self.state.lock().unwrap() = next.into();
                    Ok(())
                }
                other => Err(format!("outil non simulé : {other}")),
            }
        }

        /// Défait toutes les étapes, de la plus récente à la plus ancienne.
        pub(crate) async fn undo_all(&self) {
            let steps: Vec<Step> = std::mem::take(&mut *self.steps.lock().unwrap());
            for (tool, ops, blobs) in steps.into_iter().rev() {
                undo_ops(&self.s, &ops, &blobs, false).await.unwrap_or_else(|e| panic!("annulation de {tool} : {e:?}"));
            }
        }
    }

    impl ServerSide for GuardedFake {
        async fn probe(&self, path: &str) -> Result<Option<Meta>, String> {
            self.s.probe(path).await
        }
        async fn walk(&self, root: &str, recursive: bool, max: usize) -> Result<(Vec<Meta>, bool), String> {
            self.s.walk(root, recursive, max).await
        }
        async fn read(&self, path: &str, cap: u64) -> Result<Vec<u8>, String> {
            self.s.read(path, cap).await
        }
        async fn write(&self, tool: &'static str, args: Value) -> Result<Option<String>, String> {
            self.calls.lock().unwrap().push((tool.to_string(), args.clone()));
            let failing = self.fail_on.lock().unwrap().clone();
            if let Some(p) = failing {
                if args.to_string().contains(&p) {
                    return Err("panne simulée".into());
                }
            }
            let plan = plan_for(tool, &args);
            let cap = capture(&self.s, &plan, &mut Budget(STEP_CAP)).await;
            self.act(tool, &args).await?;
            let (ops, _) = finalize(&self.s, &cap.pre).await?;
            if cap.refused.is_none() {
                self.steps.lock().unwrap().push((tool.to_string(), ops, cap.blobs));
            }
            Ok(None)
        }
    }

    /// Dossier temporaire supprimé à la fin du test.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let p = std::env::temp_dir().join(format!("mst-test-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub(crate) fn put(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn block<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{block, GuardedFake, TempDir};
    use super::*;

    fn filters(include: &[&str], exclude: &[&str]) -> Filters {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        Filters::new(&s(include), &s(exclude)).unwrap()
    }

    fn meta(path: &str, is_dir: bool, size: u64) -> Meta {
        Meta { path: path.into(), is_dir, special: false, size, mtime: None }
    }

    fn file(rel: &str, size: u64) -> LocalFile {
        LocalFile { rel: rel.into(), abs: PathBuf::from(rel), size, content: None }
    }

    #[test]
    fn globs_follow_the_documented_rules() {
        let g = |p: &str| Glob::parse(p).unwrap();
        // Sans `/` : le nom, à toute profondeur.
        assert!(g("*.log").matches("a/b/latest.log", false) && g("*.log").matches("x.log", false));
        assert!(!g("*.log").matches("a/log", false));
        assert!(g("config.y?l").matches("plugins/X/config.yml", false));
        // Avec `/` : depuis la racine du dossier synchronisé.
        assert!(g("plugins/*.jar").matches("plugins/A.jar", false));
        assert!(!g("plugins/*.jar").matches("x/plugins/A.jar", false) && !g("plugins/*.jar").matches("plugins/X/A.jar", false));
        assert!(g("/config").matches("config", true) && !g("/config").matches("a/config", true));
        // `**` : n'importe quel nombre de segments.
        assert!(g("plugins/**/*.yml").matches("plugins/a.yml", false) && g("plugins/**/*.yml").matches("plugins/X/Y/a.yml", false));
        assert!(g("**/cache").matches("a/b/cache", true) && g("**/cache").matches("cache", true));
        // `/` final : dossiers seulement.
        assert!(g(".git/").matches(".git", true) && !g(".git/").matches(".git", false));
        assert!(g("*").matches("anything", false));
        assert!(Glob::parse("").is_err() && Glob::parse("a/../b").is_err() && Glob::parse("/").is_err());
        assert_eq!(Glob::parse("a\\b").unwrap(), g("a/b"));
    }

    #[test]
    fn filters_reach_ancestors_and_exclude_git_by_default() {
        let f = filters(&[], &[]);
        assert!(f.excluded(".git", true) && f.excluded(".git/config", false) && f.excluded("sub/.git/HEAD", false));
        assert!(!f.excluded("gitignore", false) && f.file_in_scope(".gitignore"));
        assert_eq!(f.exclude_raw, vec![".git/"]);
        let f = filters(&["plugins/", "server.properties"], &["*.log", "plugins/Big/"]);
        assert!(f.included("plugins/A.jar") && f.included("plugins/X/config.yml") && f.included("server.properties"));
        assert!(!f.included("world/level.dat") && !f.file_in_scope("ops.json"));
        assert!(f.excluded("plugins/X/latest.log", false) && f.excluded("plugins/Big/data.db", false));
        assert!(!f.file_in_scope("plugins/Big/x.yml") && f.file_in_scope("plugins/A.jar"));
        // `.git/` n'est pas dupliqué s'il est redonné.
        assert_eq!(filters(&[], &[".git/"]).exclude_raw, vec![".git/"]);
    }

    #[test]
    fn requests_are_checked_before_anything_else() {
        let abs = if cfg!(windows) { "C:/repo" } else { "/repo" };
        let ok = parse_request(&json!({ "server_id": 1, "local_dir": abs, "remote_dir": "plugins/X/" })).unwrap();
        assert_eq!(ok.remote_dir, "/plugins/X");
        assert!(ok.dry_run && !ok.delete_extra, "simulation par défaut, rien supprimé par défaut");
        assert_eq!(parse_request(&json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/" })).unwrap().remote_dir, "/");
        let err = |a: Value| parse_request(&a).unwrap_err();
        assert!(err(json!({ "server_id": 1, "local_dir": "repo", "remote_dir": "/" })).contains("absolu"));
        assert!(err(json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/a/../b" })).contains(".."));
        assert!(err(json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/", "delete_extra": true })).contains("racine"));
        assert!(err(json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/p", "dry_run": "non" })).contains("booléen"));
        assert!(err(json!({ "local_dir": abs, "remote_dir": "/p" })).contains("server_id"));
        assert!(err(json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/p", "exclude": [3] })).contains("motifs"));
        // Motif vide ignoré ; `.git/` toujours exclu.
        let r = parse_request(&json!({ "server_id": 1, "local_dir": abs, "remote_dir": "/p", "exclude": ["", "*.log"] })).unwrap();
        assert_eq!(r.filters.exclude_raw, [".git/", "*.log"]);
    }

    #[test]
    fn plan_sorts_files_and_never_deletes_what_exists_locally() {
        let local = LocalScan {
            files: vec![file("a.yml", 3), file("b.yml", 5), file("c.yml", 4), file("dir/x.yml", 1), file("f/y.yml", 1), file("link", 1)],
            dirs: ["dir".to_string(), "f".to_string()].into(),
            skipped: vec![("big.jar".into(), "trop gros".into())],
            excluded: 0,
        };
        let mut remote = BTreeMap::new();
        for (rel, is_dir, size) in [
            ("a.yml", false, 3),
            ("b.yml", false, 9),
            ("dir", true, 0),
            ("f", false, 7),
            ("big.jar", false, 1),
            ("old.yml", false, 2),
            ("old", true, 0),
            ("old/z.yml", false, 2),
            ("keep", true, 0),
            ("keep/k.log", false, 2),
            ("dir/stale.yml", false, 1),
        ] {
            remote.insert(rel.to_string(), meta(&format!("/p/{rel}"), is_dir, size));
        }
        remote.insert("link".into(), Meta { special: true, ..meta("/p/link", false, 1) });
        let f = filters(&[], &["*.log"]);
        let plan = plan_sync(&local, &remote, &f, true);
        let rels = |v: &[Upload]| v.iter().map(|u| u.rel.clone()).collect::<Vec<_>>();
        assert_eq!(rels(&plan.add), ["c.yml", "dir/x.yml"]);
        assert_eq!(rels(&plan.modify), ["b.yml"]);
        assert_eq!(rels(&plan.compare), ["a.yml"]);
        let skipped: Vec<&str> = plan.skipped.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(skipped, ["big.jar", "f/y.yml", "link"]);
        // Supprimés : seulement ce qui manque en local et reste dans la portée.
        assert_eq!(plan.delete_files, ["dir/stale.yml", "old.yml", "old/z.yml"]);
        // `old` est vidé ; `keep` garde un fichier exclu ; `dir` existe en local.
        assert_eq!(plan.delete_dirs, ["old"]);
        // Sans delete_extra : aucune suppression.
        let plan = plan_sync(&local, &remote, &f, false);
        assert!(plan.delete_files.is_empty() && plan.delete_dirs.is_empty());
        // `include` borne aussi les suppressions.
        let plan = plan_sync(&local, &remote, &filters(&["*.yml"], &[]), true);
        assert_eq!(plan.delete_files, ["dir/stale.yml", "old.yml", "old/z.yml"]);
        assert!(plan.delete_dirs.contains(&"old".to_string()));
    }

    #[test]
    fn deepest_emptied_directories_go_first() {
        let local = LocalScan::default();
        let mut remote = BTreeMap::new();
        for (rel, is_dir) in [("a", true), ("a/b", true), ("a/b/c.txt", false), ("a/d.txt", false)] {
            remote.insert(rel.to_string(), meta(&format!("/x/{rel}"), is_dir, 1));
        }
        let plan = plan_sync(&local, &remote, &filters(&[], &[]), true);
        assert_eq!(plan.delete_files, ["a/b/c.txt", "a/d.txt"]);
        assert_eq!(plan.delete_dirs, ["a/b", "a"]);
    }

    fn repo() -> TempDir {
        let t = TempDir::new("sync");
        t.put("server.properties", "motd=Réseau\n");
        t.put("plugins/LuckPerms/config.yml", "storage-method: h2\n");
        t.put("plugins/Essentials/motd.txt", "Bienvenue\n");
        t.put(".git/HEAD", "ref: refs/heads/main\n");
        t.put("notes.log", "local\n");
        t
    }

    fn request(dir: &Path, remote: &str, extra: Value) -> SyncRequest {
        let mut args = json!({ "server_id": 7, "local_dir": dir.to_str().unwrap(), "remote_dir": remote, "exclude": ["*.log"] });
        for (k, v) in extra.as_object().unwrap() {
            args[k] = v.clone();
        }
        parse_request(&args).unwrap()
    }

    /// Bout en bout sur le faux serveur : simulation sans effet, application, idempotence,
    /// suppressions bornées, puis annulation de toutes les étapes par la garde.
    #[test]
    fn sync_applies_then_is_idempotent_then_undoes() {
        block(async {
            let t = repo();
            let side = GuardedFake::new();
            side.s.put("/server.properties", "motd=Ancien\n");
            side.s.put("/plugins/Essentials/motd.txt", "Bienvenue\n");
            side.s.put("/world/level.dat", "monde");
            side.s.put("/logs/latest.log", "journal");
            let orig = side.s.snapshot();

            // Simulation : rien n'est écrit.
            let r = sync_with(&side, &request(&t.0, "/", json!({})), None).await.unwrap();
            assert_eq!(side.tools(), Vec::<String>::new());
            assert_eq!(r["counts"]["added"], 1);
            assert_eq!(r["counts"]["modified"], 1);
            assert_eq!(r["counts"]["unchanged"], 1);
            assert_eq!(r["counts"]["excluded_by_patterns"], 2, ".git et *.log");
            assert!(r["status"].as_str().unwrap().starts_with("simulation"));
            assert_eq!(side.s.snapshot(), orig);
            // Le contenu des fichiers n'est jamais rendu.
            let text = r.to_string();
            assert!(!text.contains("storage-method") && !text.contains("Réseau") && !text.contains("Bienvenue"));

            // Application.
            let r = sync_with(&side, &request(&t.0, "/", json!({ "dry_run": false })), None).await.unwrap();
            assert_eq!(r["status"], "terminée");
            assert_eq!(r["done"]["uploaded"], 2);
            assert_eq!(side.s.text("/server.properties").unwrap(), "motd=Réseau\n");
            assert_eq!(side.s.text("/plugins/LuckPerms/config.yml").unwrap(), "storage-method: h2\n");
            assert!(side.s.exists("/world/level.dat") && !side.s.exists("/.git/HEAD"));
            assert!(side.tools().iter().all(|t| t == "upload_file"));

            // Relance : tout est conforme, rien n'est envoyé.
            side.calls.lock().unwrap().clear();
            let r = sync_with(&side, &request(&t.0, "/", json!({ "dry_run": false })), None).await.unwrap();
            assert_eq!(r["counts"]["unchanged"], 3);
            assert_eq!(side.tools(), Vec::<String>::new());

            // Annulation de toutes les étapes : état d'origine, dossiers créés compris.
            side.undo_all().await;
            assert_eq!(side.s.snapshot(), orig);
        });
    }

    #[test]
    fn delete_extra_stays_under_remote_dir_and_inside_the_patterns() {
        block(async {
            let t = TempDir::new("sync-del");
            t.put("config.yml", "a: 1\n");
            t.put("lang/fr.yml", "x: y\n");
            let side = GuardedFake::new();
            side.s.put("/plugins/X/config.yml", "a: 0\n");
            side.s.put("/plugins/X/old.yml", "vieux");
            side.s.put("/plugins/X/data/old.db", "base");
            side.s.put("/plugins/X/debug.log", "journal");
            side.s.put("/plugins/Other.jar", "jar");
            let orig = side.s.snapshot();
            let req = request(&t.0, "/plugins/X", json!({ "delete_extra": true, "dry_run": false }));
            let r = sync_with(&side, &req, None).await.unwrap();
            assert_eq!(r["status"], "terminée", "{r}");
            assert_eq!(r["done"]["deleted"], 3, "old.yml, data/old.db, puis data/");
            assert!(!side.s.exists("/plugins/X/old.yml") && !side.s.exists("/plugins/X/data"));
            // Exclu (*.log) et hors de remote_dir : intacts.
            assert!(side.s.exists("/plugins/X/debug.log") && side.s.exists("/plugins/Other.jar"));
            assert_eq!(side.s.text("/plugins/X/lang/fr.yml").unwrap(), "x: y\n");
            // Suppressions après les envois.
            let tools = side.tools();
            let first_delete = tools.iter().position(|t| t == "delete_path").unwrap();
            assert!(tools[..first_delete].iter().all(|t| t == "upload_file"));
            side.undo_all().await;
            assert_eq!(side.s.snapshot(), orig);
        });
    }

    #[test]
    fn a_failure_stops_the_sync_and_lists_what_was_not_done() {
        block(async {
            let t = TempDir::new("sync-fail");
            t.put("a.yml", "a");
            t.put("b.yml", "b");
            t.put("c.yml", "c");
            let side = GuardedFake::new();
            *side.fail_on.lock().unwrap() = Some("/cfg/b.yml".into());
            let r = sync_with(&side, &request(&t.0, "/cfg", json!({ "dry_run": false })), None).await.unwrap();
            assert_eq!(r["status"], "interrompue par une erreur");
            assert!(r["error"].as_str().unwrap().contains("/cfg/b.yml"));
            assert_eq!(r["not_done"], json!(["/cfg/c.yml"]));
            assert!(side.s.exists("/cfg/a.yml") && !side.s.exists("/cfg/c.yml"));
        });
    }

    #[test]
    fn the_text_hook_prepares_compares_and_writes_without_showing_the_text() {
        block(async {
            let t = TempDir::new("sync-hook");
            t.put("config.yml", "password: {{secret:DB}}\n");
            t.put("plain.txt", "rien\n");
            let side = GuardedFake::new();
            let hook = |_rel: &str, text: &str| -> Result<Option<String>, String> {
                Ok(text.contains("{{secret:").then(|| text.replace("{{secret:DB}}", "s3cr3t-val")))
            };
            let req = request(&t.0, "/p", json!({ "dry_run": false }));
            let r = sync_with(&side, &req, Some(&hook)).await.unwrap();
            assert_eq!(side.s.text("/p/config.yml").unwrap(), "password: s3cr3t-val\n");
            assert_eq!(side.tools(), ["write_file", "upload_file"]);
            assert!(!r.to_string().contains("s3cr3t"), "jamais le texte préparé");
            // Comparé sur le texte préparé : relance sans envoi.
            side.calls.lock().unwrap().clear();
            let r = sync_with(&side, &req, Some(&hook)).await.unwrap();
            assert_eq!(r["counts"]["unchanged"], 2);
            assert!(side.tools().is_empty());
            // Refus : rien n'est envoyé.
            let refuse = |_rel: &str, _text: &str| -> Result<Option<String>, String> { Err("secret DB inconnu".into()) };
            let e = sync_with(&GuardedFake::new(), &req, Some(&refuse)).await.unwrap_err();
            assert!(e.contains("rien n'a été envoyé") && e.contains("DB"), "{e}");
        });
    }

    #[test]
    fn local_scan_applies_caps_and_skips_links() {
        let t = TempDir::new("scan");
        t.put("a.txt", "12345");
        t.put("big.bin", &"x".repeat(64));
        t.put("sub/c.txt", "c");
        let scan = scan_local(&t.0, &filters(&[], &[]), 10, 32).unwrap();
        let rels: Vec<&str> = scan.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["a.txt", "sub/c.txt"]);
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].1.contains("plafond"));
        assert!(scan_local(&t.0, &filters(&[], &[]), 1, 1024).unwrap_err().contains("plus de 1 fichiers"));
        assert!(scan_local(&t.0.join("a.txt"), &filters(&[], &[]), 10, 1024).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.0.join("a.txt"), t.0.join("lien")).unwrap();
            let scan = scan_local(&t.0, &filters(&[], &[]), 10, 1024).unwrap();
            assert!(scan.skipped.iter().any(|(r, why)| r == "lien" && why.contains("symbolique")));
            assert!(!scan.files.iter().any(|f| f.rel == "lien"));
        }
    }

    #[test]
    fn listing_stays_inside_the_local_directories() {
        block(async {
            let t = TempDir::new("listing");
            t.put("plugins/X/config.yml", "a");
            let side = GuardedFake::new();
            side.s.put("/plugins/X/config.yml", "a");
            side.s.put("/world/region/r.0.0.mca", "région");
            side.s.put("/plugins/Y/config.yml", "b");
            let scan = scan_local(&t.0, &filters(&[], &[]), 10, 1024).unwrap();
            let map = remote_listing(&side, "/", &scan.dirs, false).await.unwrap();
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            // Racine, /plugins et /plugins/X seulement : ni le monde ni les autres plugins.
            assert_eq!(keys, ["plugins", "plugins/X", "plugins/X/config.yml", "plugins/Y", "world"]);
            // Dossier distant absent : rien à lister.
            assert!(remote_listing(&side, "/absent", &scan.dirs, true).await.unwrap().is_empty());
            // Fichier à la place du dossier : erreur.
            assert!(remote_listing(&side, "/plugins/X/config.yml", &scan.dirs, false).await.is_err());
        });
    }
}
