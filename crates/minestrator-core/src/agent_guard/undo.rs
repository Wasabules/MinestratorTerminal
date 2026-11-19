//! Annulation des modifications des agents : ce qu'il faut copier **avant** un outil modifiant
//! ([`plan_for`] puis [`capture`]), l'état constaté **après** ([`finalize`], qui produit les
//! opérations inverses) et l'annulation elle-même ([`undo_ops`]), refusée si la cible a changé
//! depuis (empreinte), sauf `force`.
//!
//! Tout passe par [`Remote`] : les vraies opérations (SFTP, API) côté [`super::CoreRemote`], un
//! faux système de fichiers dans les tests.

use crate::updates::{ContentKind, ContentUpdates};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

pub(crate) const MIB: u64 = 1024 * 1024;
/// Plafond d'un fichier copié pour l'annulation (au-delà : étape non annulable).
pub(crate) const FILE_CAP: u64 = 16 * MIB;
/// Plafond des copies d'une étape (dossier supprimé, mods remplacés).
pub(crate) const STEP_CAP: u64 = 32 * MIB;
/// Plafond des copies d'un changeset (une session).
pub(crate) const CHANGESET_CAP: u64 = 64 * MIB;
/// Entrées au plus d'un dossier supprimé copié en entier.
pub(crate) const TREE_MAX_ENTRIES: usize = 500;
/// Entrées au plus d'un dossier listé avant/après (extraction, copie, installation).
pub(crate) const LIST_MAX_ENTRIES: usize = 5000;
/// Raison donnée à un outil modifiant sans arme dans [`plan_for`] (un test de `mcp.rs` l'interdit
/// pour les outils exposés : chacun doit être capturé ou déclaré non annulable).
pub(crate) const NO_CAPTURE: &str = "aucune capture prévue pour cet outil";

/// Entrée distante.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Meta {
    pub path: String,
    pub is_dir: bool,
    /// Lien symbolique ou fichier spécial : jamais suivi ni copié.
    pub special: bool,
    pub size: u64,
    pub mtime: Option<i64>,
}

/// Opérations sur UN serveur dont la garde a besoin.
pub(crate) trait Remote: Sync {
    /// État d'un chemin ; `None` s'il n'existe pas.
    fn probe(&self, path: &str) -> impl Future<Output = Result<Option<Meta>, String>> + Send;
    fn read(&self, path: &str, cap: u64) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
    /// Crée ou tronque un fichier, dossiers parents compris.
    fn write(&self, path: &str, data: &[u8]) -> impl Future<Output = Result<(), String>> + Send;
    fn remove(&self, path: &str, is_dir: bool) -> impl Future<Output = Result<(), String>> + Send;
    fn remove_tree(&self, path: &str) -> impl Future<Output = Result<(), String>> + Send;
    fn rename(&self, from: &str, to: &str) -> impl Future<Output = Result<(), String>> + Send;
    /// Crée un dossier et ses parents.
    fn mkdir_all(&self, path: &str) -> impl Future<Output = Result<(), String>> + Send;
    /// Entrées SOUS `root` (dossiers compris), récursif ou non : `(entrées, tronqué)`.
    fn walk(
        &self,
        root: &str,
        recursive: bool,
        max: usize,
    ) -> impl Future<Output = Result<(Vec<Meta>, bool), String>> + Send;
    fn property(&self, key: &str) -> impl Future<Output = Result<Option<String>, String>> + Send;
    fn set_property(&self, key: &str, value: &str) -> impl Future<Output = Result<(), String>> + Send;
    fn startup(&self) -> impl Future<Output = Result<String, String>> + Send;
    fn set_startup(&self, command: &str) -> impl Future<Output = Result<(), String>> + Send;
}

// --- Plan : ce qu'un outil modifiant va toucher ------------------------------------------------

/// Ce qu'il faut capturer avant un outil modifiant, d'après son nom et ses arguments.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plan {
    /// Aucune étape : l'outil n'a pas d'effet propre à défaire (annulation elle-même, outil
    /// composite dont chaque sous-action passe par `mcp::dispatch`).
    Skip,
    /// Étape non annulable par nature (raison), sans capture.
    Fixed(&'static str),
    /// Arguments illisibles : l'outil va échouer ; rien à capturer.
    Unreadable,
    /// Fichiers modifiés en place : contenu précédent (ou absence).
    Files { server: i64, paths: Vec<String> },
    /// Suppression d'un fichier (contenu) ou d'un dossier (arborescence, plafonnée).
    Delete { server: i64, path: String },
    Rename { server: i64, from: String, to: String },
    CreateDir { server: i64, path: String },
    /// Fichiers créés sous des dossiers : listing avant / après.
    NewFiles { server: i64, dirs: Vec<String>, recursive: bool },
    Property { server: i64, key: String },
    Startup { server: i64 },
    /// Mises à jour de mods, à résoudre en fichiers ([`mod_files`]) avant capture.
    ModUpdates { server: i64, kind: Option<ContentKind>, keys: Vec<String> },
    /// Mises à jour résolues : anciens fichiers copiés, nouveaux listés dans `/mods` et `/plugins`.
    ModFiles { server: i64, files: Vec<String>, note: Option<String> },
}

impl Plan {
    /// Serveur touché, s'il y en a un.
    pub(crate) fn server(&self) -> Option<i64> {
        match self {
            Plan::Skip | Plan::Fixed(_) | Plan::Unreadable => None,
            Plan::Files { server, .. }
            | Plan::Delete { server, .. }
            | Plan::Rename { server, .. }
            | Plan::CreateDir { server, .. }
            | Plan::NewFiles { server, .. }
            | Plan::Property { server, .. }
            | Plan::Startup { server }
            | Plan::ModUpdates { server, .. }
            | Plan::ModFiles { server, .. } => Some(*server),
        }
    }

    /// Cible lisible de l'étape (chemin, réglage…).
    pub(crate) fn target(&self) -> String {
        let list = |v: &[String]| match v {
            [] => String::new(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} (+{})", rest.len()),
        };
        match self {
            Plan::Skip | Plan::Fixed(_) | Plan::Unreadable => String::new(),
            Plan::Files { paths, .. } => list(paths),
            Plan::Delete { path, .. } | Plan::CreateDir { path, .. } => path.clone(),
            Plan::Rename { from, to, .. } => format!("{from} → {to}"),
            Plan::NewFiles { dirs, .. } => list(dirs),
            Plan::Property { key, .. } => key.clone(),
            Plan::Startup { .. } => "commande de démarrage".into(),
            Plan::ModUpdates { .. } | Plan::ModFiles { .. } => "mods et plugins".into(),
        }
    }
}

/// Dossier parent d'un chemin distant (`/` pour un nom à la racine).
pub(crate) fn parent_dir(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some((head, _)) if !head.is_empty() => head.to_string(),
        _ => "/".to_string(),
    }
}

/// Plan de capture d'un outil modifiant. **Point d'extension** : un outil modifiant d'un autre
/// module se branche par une arme de ce `match` (voir la fin) ; sans arme, il est enregistré
/// comme étape non annulable — jamais bloqué.
pub(crate) fn plan_for(name: &str, args: &Value) -> Plan {
    let int = |k: &str| args.get(k).and_then(Value::as_i64);
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let files = |key: &str| match (int("server_id"), text(key)) {
        (Some(server), Some(path)) => Plan::Files { server, paths: vec![path] },
        _ => Plan::Unreadable,
    };
    match name {
        "write_file" | "edit_file" => files("path"),
        "upload_file" => files("remote_path"),
        "delete_path" => match (int("server_id"), text("path")) {
            (Some(server), Some(path)) => Plan::Delete { server, path },
            _ => Plan::Unreadable,
        },
        "rename_path" => match (int("server_id"), text("from"), text("to")) {
            (Some(server), Some(from), Some(to)) => Plan::Rename { server, from, to },
            _ => Plan::Unreadable,
        },
        "create_dir" => match (int("server_id"), text("path")) {
            (Some(server), Some(path)) => Plan::CreateDir { server, path },
            _ => Plan::Unreadable,
        },
        "extract_archive" => match (int("server_id"), text("dest_dir")) {
            (Some(server), Some(dest)) => Plan::NewFiles { server, dirs: vec![dest], recursive: true },
            _ => Plan::Unreadable,
        },
        // L'archive est créée dans le dossier des chemins compressés.
        "compress_files" => {
            let first = args
                .get("paths")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str);
            match (int("server_id"), first) {
                (Some(server), Some(p)) => Plan::NewFiles { server, dirs: vec![parent_dir(p)], recursive: false },
                _ => Plan::Unreadable,
            }
        }
        // Placement décidé par l'hébergeur : `/mods` ou `/plugins`.
        "install_mod" => match int("server_id") {
            Some(server) => Plan::NewFiles {
                server,
                dirs: vec!["/mods".into(), "/plugins".into()],
                recursive: false,
            },
            None => Plan::Unreadable,
        },
        "update_mods" => match int("server_id") {
            Some(server) => {
                let kind = text("kind").and_then(|k| match k.trim() {
                    "" | "all" => None,
                    k => ContentKind::parse(k),
                });
                let keys = match args.get("keys") {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
                    _ => Vec::new(),
                };
                Plan::ModUpdates { server, kind, keys }
            }
            None => Plan::Unreadable,
        },
        "set_property" => match (int("server_id"), text("property")) {
            (Some(server), Some(key)) => Plan::Property { server, key },
            _ => Plan::Unreadable,
        },
        "set_startup_params" => match int("server_id") {
            Some(server) => Plan::Startup { server },
            None => Plan::Unreadable,
        },
        // Copie entre serveurs : seuls les fichiers créés sur la cible sont défaits.
        "copy_between_servers" => match (int("target_server_id"), text("target_dir")) {
            (Some(server), Some(dir)) => Plan::NewFiles { server, dirs: vec![dir], recursive: true },
            _ => Plan::Unreadable,
        },
        "rollback_changeset" => Plan::Skip,
        "power_action" => Plan::Fixed("action d'alimentation, sans état à restaurer"),
        "send_command" | "broadcast_command" => Plan::Fixed("commande console : effet non réversible"),
        "player_action" => Plan::Fixed("modération : à défaire par l'action inverse"),
        "create_snapshot" => Plan::Fixed("snapshot créée : à supprimer depuis l'app si besoin"),
        "install_ficsit_mod" => Plan::Fixed("mod Satisfactory : SML, dépendances et redémarrage"),
        "mybox_create_server" | "mybox_resize_server" | "mybox_pause_server" | "mybox_reorder_servers"
        | "mybox_delete_server" | "rename_server" | "rename_mybox" => Plan::Fixed("infrastructure MyBox"),
        "create_schedule" | "update_schedule" | "delete_schedule" | "add_schedule_task"
        | "update_schedule_task" | "delete_schedule_task" | "reorder_schedule_tasks" => {
            Plan::Fixed("tâche planifiée du panel")
        }
        "restore_snapshot" | "restore_backup" | "delete_snapshot" | "repair_region" => {
            Plan::Fixed("restauration ou réparation : hors de portée de l'annulation")
        }
        // --- Points d'extension : outils des autres lots ----------------------------------------
        // Fichier de configuration modifié en place (édition structurée) : contenu précédent.
        "set_config" => files("path"),
        // Commande console avec sa sortie : effet non réversible.
        "run_command" => Plan::Fixed("commande console : effet non réversible"),
        // À brancher par l'intégrateur selon leurs arguments :
        // - `apply_manifest` / `deploy_and_verify` : `Plan::Skip` s'ils passent par
        //   `mcp::dispatch` pour chaque sous-action (une étape par sous-action), sinon
        //   `Plan::Files` / `Plan::NewFiles` sur les chemins qu'ils écrivent ;
        // - `clone_server` : `Plan::Fixed("infrastructure MyBox")`, et
        //   `super::record_created_server` après succès (mode constructeur).
        _ => Plan::Fixed(NO_CAPTURE),
    }
}

/// Anciens fichiers que `update_mods` remplacera, d'après la dernière vérification : mêmes
/// règles de sélection que l'outil (clés données, sinon toutes les mises à jour des éléments
/// actifs du type demandé). Les datapacks ne sont pas couverts (`true` en second : signalé).
pub(crate) fn mod_files(current: &ContentUpdates, kind: Option<ContentKind>, keys: &[String]) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut datapacks = false;
    for u in current.updates.iter().filter(|u| kind.is_none_or(|k| k == u.kind)) {
        let selected = if keys.is_empty() { u.enabled } else { keys.contains(&u.key) };
        if !selected {
            continue;
        }
        match u.kind {
            ContentKind::Mods => files.push(format!("/mods/{}", u.key)),
            ContentKind::Plugins => files.push(format!("/plugins/{}", u.key)),
            ContentKind::Datapacks => datapacks = true,
        }
    }
    (files, datapacks)
}

// --- Empreintes ------------------------------------------------------------------------------

/// Copie d'un fichier gardée pour l'annulation (`blob` : index dans les copies de l'étape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BlobFp {
    pub blob: usize,
    pub size: u64,
    pub sha256: String,
}

/// Empreinte d'un chemin : existence, nature, taille, date, et sha256 quand on l'a calculé.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fp {
    pub exists: bool,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    pub sha256: Option<String>,
}

pub(crate) fn sha_hex(data: &[u8]) -> String {
    use sha2::Digest;
    crate::util::hex(&sha2::Sha256::digest(data))
}

fn meta_fp(m: &Meta) -> Fp {
    Fp { exists: true, is_dir: m.is_dir, size: if m.is_dir { 0 } else { m.size }, mtime: if m.is_dir { None } else { m.mtime }, sha256: None }
}

/// Empreinte actuelle de `path` ; `hash` : calcule le sha256 (fichier de `FILE_CAP` au plus).
pub(crate) async fn fingerprint<R: Remote>(r: &R, path: &str, hash: bool) -> Result<Fp, String> {
    match r.probe(path).await? {
        None => Ok(Fp::default()),
        Some(m) if m.is_dir => Ok(meta_fp(&m)),
        Some(m) => {
            let mut fp = meta_fp(&m);
            if hash && !m.special && m.size <= FILE_CAP {
                fp.sha256 = Some(sha_hex(&r.read(path, FILE_CAP).await?));
            }
            Ok(fp)
        }
    }
}

/// Même état ? sha256 si les deux en ont un, sinon taille et date.
pub(crate) fn same(a: &Fp, b: &Fp) -> bool {
    if a.exists != b.exists || a.is_dir != b.is_dir {
        return false;
    }
    if !a.exists || a.is_dir {
        return true;
    }
    match (&a.sha256, &b.sha256) {
        (Some(x), Some(y)) => x == y,
        _ => a.size == b.size && (a.mtime.is_none() || b.mtime.is_none() || a.mtime == b.mtime),
    }
}

/// `cur` est-il l'état d'avant l'action (déjà annulé) ?
fn is_before(cur: &Fp, before: Option<&BlobFp>) -> bool {
    match before {
        None => !cur.exists,
        Some(b) => cur.exists && !cur.is_dir && cur.sha256.as_deref() == Some(b.sha256.as_str()),
    }
}

// --- Capture (avant l'action) ----------------------------------------------------------------

/// État capturé avant l'action.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pre {
    Nothing,
    Files(Vec<(String, Option<BlobFp>)>),
    Tree { root: String, dirs: Vec<String>, files: Vec<(String, BlobFp)> },
    Rename { from: String, to: String, overwritten: Option<BlobFp> },
    Dir { path: String, existed: bool },
    Listing { roots: Vec<(String, bool)>, recursive: bool, before: BTreeMap<String, Meta> },
    Property { key: String, before: String },
    Startup { before: String },
    Many(Vec<Pre>),
}

/// Résultat de la capture : état d'avant et copies, ou raison pour laquelle l'étape ne sera pas
/// annulable (capture impossible, plafond dépassé, réglage secret…).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Capture {
    pub pre: Pre,
    pub blobs: Vec<Vec<u8>>,
    pub refused: Option<String>,
    /// Avertissement à garder avec l'étape (annulation partielle).
    pub note: Option<String>,
}

/// Plafond des copies restant à l'étape.
pub(crate) struct Budget(pub u64);

impl Budget {
    fn take(&mut self, n: u64) -> bool {
        if n > self.0 {
            return false;
        }
        self.0 -= n;
        true
    }
}

pub(crate) fn human(bytes: u64) -> String {
    crate::server_files::human_bytes(bytes)
}

/// Copie un fichier (plafonds vérifiés) ; `Err` = raison d'un refus de capture.
async fn copy_file<R: Remote>(
    r: &R,
    m: &Meta,
    budget: &mut Budget,
    blobs: &mut Vec<Vec<u8>>,
) -> Result<BlobFp, String> {
    if m.special {
        return Err(format!("« {} » est un lien ou un fichier spécial", m.path));
    }
    if m.size > FILE_CAP {
        return Err(format!(
            "« {} » fait {}, au-delà du plafond de copie ({})",
            m.path,
            human(m.size),
            human(FILE_CAP)
        ));
    }
    if !budget.take(m.size) {
        return Err("copies au-delà du plafond de l'étape ou du changeset".into());
    }
    let data = r.read(&m.path, FILE_CAP).await.map_err(|e| format!("lecture de « {} » : {e}", m.path))?;
    let fp = BlobFp { blob: blobs.len(), size: data.len() as u64, sha256: sha_hex(&data) };
    blobs.push(data);
    Ok(fp)
}

/// Copie avant l'action. Ne bloque jamais l'outil : un échec donne une étape non annulable.
pub(crate) async fn capture<R: Remote>(r: &R, plan: &Plan, budget: &mut Budget) -> Capture {
    let mut blobs = Vec::new();
    let mut note = None;
    match capture_inner(r, plan, budget, &mut blobs, &mut note).await {
        Ok(pre) => Capture { pre, blobs, refused: None, note },
        Err(reason) => Capture { pre: Pre::Nothing, blobs: Vec::new(), refused: Some(reason), note },
    }
}

async fn capture_files<R: Remote>(
    r: &R,
    paths: &[String],
    budget: &mut Budget,
    blobs: &mut Vec<Vec<u8>>,
) -> Result<Vec<(String, Option<BlobFp>)>, String> {
    let mut out = Vec::new();
    for path in paths {
        match r.probe(path).await? {
            None => out.push((path.clone(), None)),
            Some(m) if m.is_dir => return Err(format!("« {path} » est un dossier")),
            Some(m) => out.push((path.clone(), Some(copy_file(r, &m, budget, blobs).await?))),
        }
    }
    Ok(out)
}

async fn capture_listing<R: Remote>(r: &R, dirs: &[String], recursive: bool) -> Result<Pre, String> {
    let mut roots = Vec::new();
    let mut before = BTreeMap::new();
    for dir in dirs {
        match r.probe(dir).await? {
            None => roots.push((dir.clone(), false)),
            Some(m) if !m.is_dir => roots.push((dir.clone(), true)),
            Some(_) => {
                let (entries, truncated) = r.walk(dir, recursive, LIST_MAX_ENTRIES).await?;
                if truncated {
                    return Err(format!("plus de {LIST_MAX_ENTRIES} entrées sous « {dir} » : listing refusé"));
                }
                roots.push((dir.clone(), true));
                before.extend(entries.into_iter().map(|e| (e.path.clone(), e)));
            }
        }
    }
    Ok(Pre::Listing { roots, recursive, before })
}

async fn capture_inner<R: Remote>(
    r: &R,
    plan: &Plan,
    budget: &mut Budget,
    blobs: &mut Vec<Vec<u8>>,
    note: &mut Option<String>,
) -> Result<Pre, String> {
    match plan {
        Plan::Skip | Plan::Fixed(_) | Plan::Unreadable => Ok(Pre::Nothing),
        Plan::ModUpdates { .. } => Err("mises à jour non résolues".into()),
        Plan::Files { paths, .. } => Ok(Pre::Files(capture_files(r, paths, budget, blobs).await?)),
        Plan::Delete { path, .. } => match r.probe(path).await? {
            None => Ok(Pre::Nothing),
            Some(m) if !m.is_dir => Ok(Pre::Files(vec![(path.clone(), Some(copy_file(r, &m, budget, blobs).await?))])),
            Some(_) => {
                let (entries, truncated) = r.walk(path, true, TREE_MAX_ENTRIES + 1).await?;
                if truncated || entries.len() > TREE_MAX_ENTRIES {
                    return Err(format!("dossier de plus de {TREE_MAX_ENTRIES} entrées : copie refusée"));
                }
                let total: u64 = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
                if total > budget.0 {
                    return Err(format!(
                        "dossier de {}, au-delà du plafond de copie restant ({})",
                        human(total),
                        human(budget.0)
                    ));
                }
                let mut dirs: Vec<String> = Vec::new();
                let mut files = Vec::new();
                for e in &entries {
                    if e.special {
                        return Err(format!("« {} » est un lien ou un fichier spécial", e.path));
                    }
                    if e.is_dir {
                        dirs.push(e.path.clone());
                    } else {
                        files.push((e.path.clone(), copy_file(r, e, budget, blobs).await?));
                    }
                }
                dirs.sort_by_key(|d| (d.matches('/').count(), d.clone()));
                Ok(Pre::Tree { root: path.clone(), dirs, files })
            }
        },
        Plan::Rename { from, to, .. } => {
            if r.probe(from).await?.is_none() {
                return Ok(Pre::Nothing);
            }
            let overwritten = match r.probe(to).await? {
                None => None,
                Some(m) if m.is_dir => return Err(format!("destination « {to} » déjà présente (dossier)")),
                Some(m) => Some(copy_file(r, &m, budget, blobs).await?),
            };
            Ok(Pre::Rename { from: from.clone(), to: to.clone(), overwritten })
        }
        Plan::CreateDir { path, .. } => Ok(Pre::Dir { path: path.clone(), existed: r.probe(path).await?.is_some() }),
        Plan::NewFiles { dirs, recursive, .. } => capture_listing(r, dirs, *recursive).await,
        Plan::Property { key, .. } => {
            if crate::models::is_secret_property(key) {
                return Err("réglage secret : son ancienne valeur n'est jamais conservée".into());
            }
            Ok(match r.property(key).await? {
                Some(before) => Pre::Property { key: key.clone(), before },
                None => Pre::Nothing,
            })
        }
        Plan::Startup { .. } => Ok(Pre::Startup { before: r.startup().await? }),
        Plan::ModFiles { files, note: n, .. } => {
            *note = n.clone();
            let mut existing = Vec::new();
            for f in files {
                if let Some(m) = r.probe(f).await? {
                    if !m.is_dir {
                        existing.push(f.clone());
                    }
                }
            }
            let copied = capture_files(r, &existing, budget, blobs).await?;
            let listing = capture_listing(r, &["/mods".into(), "/plugins".into()], false).await?;
            Ok(Pre::Many(vec![Pre::Files(copied), listing]))
        }
    }
}

// --- Après l'action : opérations inverses ------------------------------------------------------

/// Une opération inverse, sérialisée dans l'étape (sans contenu : les copies sont à part).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum UndoOp {
    /// Remettre `path` dans son état d'avant : copie `before`, ou absence.
    File { path: String, before: Option<BlobFp>, after: Fp },
    /// Dossier créé par l'action : le retirer s'il est vide (ou de force).
    RemoveDir { path: String },
    /// Dossier supprimé par l'action : le recréer avec son contenu.
    Tree { root: String, dirs: Vec<String>, files: Vec<(String, BlobFp)> },
    /// `from` renommé en `to` : remettre `to` en `from`, et le fichier écrasé à sa place.
    Rename { from: String, to: String, after: Fp, overwritten: Option<BlobFp> },
    Property { key: String, before: String, after: String },
    Startup { before: String, after: String },
}

impl UndoOp {
    fn label(&self) -> String {
        match self {
            UndoOp::File { path, .. } | UndoOp::RemoveDir { path } => format!("« {path} »"),
            UndoOp::Tree { root, .. } => format!("« {root} »"),
            UndoOp::Rename { from, to, .. } => format!("« {to} » → « {from} »"),
            UndoOp::Property { key, .. } => format!("réglage « {key} »"),
            UndoOp::Startup { .. } => "commande de démarrage".into(),
        }
    }
}

fn depth(p: &str) -> usize {
    p.matches('/').count()
}

/// Constate l'état après l'action et en déduit les opérations inverses, plus un avertissement
/// éventuel (fichiers existants remplacés, non restaurables).
pub(crate) async fn finalize<R: Remote>(r: &R, pre: &Pre) -> Result<(Vec<UndoOp>, Option<String>), String> {
    let mut ops = Vec::new();
    let mut notes = Vec::new();
    let mut covered = BTreeSet::new();
    // Composé (mises à jour de mods) : les fichiers copiés d'abord, pour qu'un listing ne les compte
    // pas comme « remplacés ». Pas d'imbrication plus profonde.
    let parts: Vec<&Pre> = match pre {
        Pre::Many(parts) => {
            let (files, rest): (Vec<&Pre>, Vec<&Pre>) = parts.iter().partition(|p| matches!(p, Pre::Files(_)));
            files.into_iter().chain(rest).collect()
        }
        one => vec![one],
    };
    for part in parts {
        finalize_part(r, part, &mut ops, &mut notes, &mut covered).await?;
    }
    Ok((ops, (!notes.is_empty()).then(|| notes.join(" ; "))))
}

async fn finalize_part<R: Remote>(
    r: &R,
    pre: &Pre,
    ops: &mut Vec<UndoOp>,
    notes: &mut Vec<String>,
    covered: &mut BTreeSet<String>,
) -> Result<(), String> {
    match pre {
        Pre::Nothing => {}
        Pre::Files(files) => {
            for (path, before) in files {
                covered.insert(path.clone());
                let after = fingerprint(r, path, true).await?;
                if is_before(&after, before.as_ref()) {
                    continue; // rien n'a changé
                }
                ops.push(UndoOp::File { path: path.clone(), before: before.clone(), after });
            }
        }
        Pre::Tree { root, dirs, files } => {
            if r.probe(root).await?.is_none() {
                ops.push(UndoOp::Tree { root: root.clone(), dirs: dirs.clone(), files: files.clone() });
            }
        }
        Pre::Rename { from, to, overwritten } => {
            let after = fingerprint(r, to, true).await?;
            if after.exists && r.probe(from).await?.is_none() {
                ops.push(UndoOp::Rename { from: from.clone(), to: to.clone(), after, overwritten: overwritten.clone() });
            }
        }
        Pre::Dir { path, existed } => {
            if !existed && r.probe(path).await?.is_some_and(|m| m.is_dir) {
                ops.push(UndoOp::RemoveDir { path: path.clone() });
            }
        }
        Pre::Listing { roots, recursive, before } => {
            let mut created_dirs = Vec::new();
            let mut created_files = Vec::new();
            let mut replaced = 0usize;
            for (root, existed) in roots {
                let Some(m) = r.probe(root).await? else { continue };
                if !m.is_dir {
                    continue;
                }
                if !existed {
                    created_dirs.push(root.clone());
                }
                let (entries, truncated) = r.walk(root, *recursive, LIST_MAX_ENTRIES).await?;
                if truncated {
                    return Err(format!("plus de {LIST_MAX_ENTRIES} entrées sous « {root} » après l'action"));
                }
                for e in entries {
                    if covered.contains(&e.path) {
                        continue;
                    }
                    match before.get(&e.path) {
                        None if e.is_dir => created_dirs.push(e.path.clone()),
                        None => created_files.push(UndoOp::File { path: e.path.clone(), before: None, after: meta_fp(&e) }),
                        Some(b) if !e.is_dir && !same(&meta_fp(b), &meta_fp(&e)) => replaced += 1,
                        Some(_) => {}
                    }
                }
            }
            created_dirs.sort_by_key(|d| (depth(d), d.clone()));
            ops.extend(created_dirs.into_iter().map(|path| UndoOp::RemoveDir { path }));
            ops.extend(created_files);
            if replaced > 0 {
                notes.push(format!(
                    "annulation partielle : {replaced} fichier(s) existant(s) remplacé(s), non restauré(s)"
                ));
            }
        }
        Pre::Property { key, before } => {
            let after = r.property(key).await?.unwrap_or_default();
            if &after != before {
                ops.push(UndoOp::Property { key: key.clone(), before: before.clone(), after });
            }
        }
        Pre::Startup { before } => {
            let after = r.startup().await?;
            if &after != before {
                ops.push(UndoOp::Startup { before: before.clone(), after });
            }
        }
        // Déplié par `finalize`.
        Pre::Many(_) => {}
    }
    Ok(())
}

// --- Annulation --------------------------------------------------------------------------------

/// Échec d'une annulation d'étape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UndoError {
    /// Refus avant toute modification (cible changée depuis l'action).
    Refused(String),
    /// Échec en cours d'application (l'étape peut être partiellement défaite).
    Failed(String),
}

/// Défait les opérations d'une étape, dans l'ordre inverse. Vérifie d'abord TOUTES les cibles
/// (rien n'est touché si l'une a changé, sauf `force`) ; une opération déjà dans l'état d'origine
/// est sautée. Renvoie les remarques (dossier conservé…).
pub(crate) async fn undo_ops<R: Remote>(
    r: &R,
    ops: &[UndoOp],
    blobs: &[Vec<u8>],
    force: bool,
) -> Result<Vec<String>, UndoError> {
    let refused = |m: String| UndoError::Refused(format!("{m} (force=true pour passer outre)"));
    let io = |op: &UndoOp, e: String| UndoError::Failed(format!("{} : {e}", op.label()));
    let mut done_already = vec![false; ops.len()];
    // 1. Vérification.
    for (i, op) in ops.iter().enumerate() {
        match op {
            UndoOp::File { path, before, after } => {
                let hash = before.is_some() || after.sha256.is_some();
                let cur = fingerprint(r, path, hash).await.map_err(|e| io(op, e))?;
                if is_before(&cur, before.as_ref()) {
                    done_already[i] = true;
                } else if !force && !same(&cur, after) {
                    return Err(refused(format!("« {path} » a changé depuis l'action")));
                }
            }
            UndoOp::RemoveDir { .. } => {}
            UndoOp::Tree { root, .. } => {
                if !force && r.probe(root).await.map_err(|e| io(op, e))?.is_some() {
                    return Err(refused(format!("« {root} » existe de nouveau")));
                }
            }
            UndoOp::Rename { from, to, after, .. } => {
                let cur_to = fingerprint(r, to, after.sha256.is_some()).await.map_err(|e| io(op, e))?;
                let from_now = r.probe(from).await.map_err(|e| io(op, e))?;
                if !cur_to.exists && from_now.is_some() {
                    done_already[i] = true;
                } else if !force && !same(&cur_to, after) {
                    return Err(refused(format!("« {to} » a changé depuis l'action")));
                } else if !force && from_now.is_some() {
                    return Err(refused(format!("« {from} » existe de nouveau")));
                }
            }
            UndoOp::Property { key, before, after } => {
                let cur = r.property(key).await.map_err(|e| io(op, e))?.unwrap_or_default();
                if &cur == before {
                    done_already[i] = true;
                } else if !force && &cur != after {
                    return Err(refused(format!("le réglage « {key} » a changé depuis l'action")));
                }
            }
            UndoOp::Startup { before, after } => {
                let cur = r.startup().await.map_err(|e| io(op, e))?;
                if &cur == before {
                    done_already[i] = true;
                } else if !force && &cur != after {
                    return Err(refused("la commande de démarrage a changé depuis l'action".into()));
                }
            }
        }
    }
    // 2. Application, dans l'ordre inverse.
    let blob = |op: &UndoOp, b: &BlobFp| -> Result<Vec<u8>, UndoError> {
        let data = blobs.get(b.blob).ok_or_else(|| io(op, "copie introuvable".into()))?;
        if sha_hex(data) != b.sha256 {
            return Err(io(op, "copie altérée (empreinte différente)".into()));
        }
        Ok(data.clone())
    };
    let mut remarks = Vec::new();
    for (i, op) in ops.iter().enumerate().rev() {
        if done_already[i] {
            continue;
        }
        match op {
            UndoOp::File { path, before: Some(b), .. } => {
                let data = blob(op, b)?;
                r.write(path, &data).await.map_err(|e| io(op, e))?;
            }
            UndoOp::File { path, before: None, .. } => match r.probe(path).await.map_err(|e| io(op, e))? {
                None => {}
                Some(m) if m.is_dir => r.remove_tree(path).await.map_err(|e| io(op, e))?,
                Some(_) => r.remove(path, false).await.map_err(|e| io(op, e))?,
            },
            UndoOp::RemoveDir { path } => {
                if r.probe(path).await.map_err(|e| io(op, e))?.is_some_and(|m| m.is_dir) {
                    let (inside, _) = r.walk(path, false, 1).await.map_err(|e| io(op, e))?;
                    if inside.is_empty() {
                        r.remove(path, true).await.map_err(|e| io(op, e))?;
                    } else if force {
                        r.remove_tree(path).await.map_err(|e| io(op, e))?;
                    } else {
                        remarks.push(format!("dossier « {path} » conservé : il n'est pas vide"));
                    }
                }
            }
            UndoOp::Tree { root, dirs, files } => {
                r.mkdir_all(root).await.map_err(|e| io(op, e))?;
                for d in dirs {
                    r.mkdir_all(d).await.map_err(|e| io(op, e))?;
                }
                for (path, b) in files {
                    let data = blob(op, b)?;
                    r.write(path, &data).await.map_err(|e| io(op, e))?;
                }
            }
            UndoOp::Rename { from, to, overwritten, .. } => {
                r.rename(to, from).await.map_err(|e| io(op, e))?;
                if let Some(b) = overwritten {
                    let data = blob(op, b)?;
                    r.write(to, &data).await.map_err(|e| io(op, e))?;
                }
            }
            UndoOp::Property { key, before, .. } => r.set_property(key, before).await.map_err(|e| io(op, e))?,
            UndoOp::Startup { before, .. } => r.set_startup(before).await.map_err(|e| io(op, e))?,
        }
    }
    Ok(remarks)
}

#[cfg(test)]
pub(crate) mod fake {
    //! Faux serveur en mémoire : système de fichiers, réglages et commande de démarrage.
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Node {
        Dir,
        File(Vec<u8>, i64),
    }

    #[derive(Default)]
    pub(crate) struct FakeServer {
        pub fs: Mutex<BTreeMap<String, Node>>,
        pub props: Mutex<BTreeMap<String, String>>,
        pub startup: Mutex<String>,
        clock: Mutex<i64>,
    }

    impl FakeServer {
        pub(crate) fn new() -> Self {
            let s = Self::default();
            s.fs.lock().unwrap().insert("/".into(), Node::Dir);
            s
        }
        fn tick(&self) -> i64 {
            let mut c = self.clock.lock().unwrap();
            *c += 1;
            *c
        }
        fn parents(&self, path: &str) {
            let mut fs = self.fs.lock().unwrap();
            let mut acc = String::new();
            let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            for part in &parts[..parts.len().saturating_sub(1)] {
                acc.push('/');
                acc.push_str(part);
                fs.entry(acc.clone()).or_insert(Node::Dir);
            }
        }
        pub(crate) fn put(&self, path: &str, content: &str) {
            self.parents(path);
            let t = self.tick();
            self.fs.lock().unwrap().insert(path.into(), Node::File(content.as_bytes().to_vec(), t));
        }
        pub(crate) fn dir(&self, path: &str) {
            self.parents(path);
            self.fs.lock().unwrap().insert(path.into(), Node::Dir);
        }
        pub(crate) fn text(&self, path: &str) -> Option<String> {
            match self.fs.lock().unwrap().get(path) {
                Some(Node::File(d, _)) => Some(String::from_utf8_lossy(d).into_owned()),
                _ => None,
            }
        }
        pub(crate) fn exists(&self, path: &str) -> bool {
            self.fs.lock().unwrap().contains_key(path)
        }
        /// Renommage synchrone (action simulée).
        pub(crate) fn mv(&self, from: &str, to: &str) {
            let mut fs = self.fs.lock().unwrap();
            let node = fs.remove(from).expect("source");
            fs.insert(to.into(), node);
        }
        pub(crate) fn delete(&self, path: &str) {
            let prefix = format!("{path}/");
            self.fs.lock().unwrap().retain(|k, _| k != path && !k.starts_with(&prefix));
        }
        /// Instantané comparable (contenus, sans les dates).
        pub(crate) fn snapshot(&self) -> BTreeMap<String, Option<Vec<u8>>> {
            self.fs
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), match v { Node::Dir => None, Node::File(d, _) => Some(d.clone()) }))
                .collect()
        }
    }

    fn child_of(path: &str, root: &str) -> bool {
        let prefix = if root == "/" { "/".to_string() } else { format!("{}/", root.trim_end_matches('/')) };
        path != root && path.starts_with(&prefix)
    }

    impl Remote for FakeServer {
        async fn probe(&self, path: &str) -> Result<Option<Meta>, String> {
            Ok(self.fs.lock().unwrap().get(path).map(|n| match n {
                Node::Dir => Meta { path: path.into(), is_dir: true, special: false, size: 0, mtime: None },
                Node::File(d, t) => Meta { path: path.into(), is_dir: false, special: false, size: d.len() as u64, mtime: Some(*t) },
            }))
        }
        async fn read(&self, path: &str, cap: u64) -> Result<Vec<u8>, String> {
            match self.fs.lock().unwrap().get(path) {
                Some(Node::File(d, _)) if d.len() as u64 <= cap => Ok(d.clone()),
                Some(Node::File(..)) => Err("trop gros".into()),
                _ => Err(format!("{path} introuvable")),
            }
        }
        async fn write(&self, path: &str, data: &[u8]) -> Result<(), String> {
            self.parents(path);
            let t = self.tick();
            self.fs.lock().unwrap().insert(path.into(), Node::File(data.to_vec(), t));
            Ok(())
        }
        async fn remove(&self, path: &str, is_dir: bool) -> Result<(), String> {
            let mut fs = self.fs.lock().unwrap();
            if is_dir && fs.keys().any(|k| child_of(k, path)) {
                return Err("dossier non vide".into());
            }
            fs.remove(path).map(|_| ()).ok_or_else(|| "introuvable".into())
        }
        async fn remove_tree(&self, path: &str) -> Result<(), String> {
            self.delete(path);
            Ok(())
        }
        async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
            let mut fs = self.fs.lock().unwrap();
            let node = fs.remove(from).ok_or("source introuvable")?;
            let moved: Vec<(String, Node)> = fs
                .iter()
                .filter(|(k, _)| child_of(k, from))
                .map(|(k, v)| (format!("{to}{}", &k[from.len()..]), v.clone()))
                .collect();
            fs.retain(|k, _| !child_of(k, from));
            fs.insert(to.into(), node);
            fs.extend(moved);
            Ok(())
        }
        async fn mkdir_all(&self, path: &str) -> Result<(), String> {
            self.dir(path);
            Ok(())
        }
        async fn walk(&self, root: &str, recursive: bool, max: usize) -> Result<(Vec<Meta>, bool), String> {
            let fs = self.fs.lock().unwrap();
            let mut out = Vec::new();
            for (k, n) in fs.iter() {
                if !child_of(k, root) {
                    continue;
                }
                let rest = &k[root.trim_end_matches('/').len() + 1..];
                if !recursive && rest.contains('/') {
                    continue;
                }
                if out.len() >= max {
                    return Ok((out, true));
                }
                out.push(match n {
                    Node::Dir => Meta { path: k.clone(), is_dir: true, special: false, size: 0, mtime: None },
                    Node::File(d, t) => Meta { path: k.clone(), is_dir: false, special: false, size: d.len() as u64, mtime: Some(*t) },
                });
            }
            Ok((out, false))
        }
        async fn property(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.props.lock().unwrap().get(key).cloned())
        }
        async fn set_property(&self, key: &str, value: &str) -> Result<(), String> {
            self.props.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        async fn startup(&self) -> Result<String, String> {
            Ok(self.startup.lock().unwrap().clone())
        }
        async fn set_startup(&self, command: &str) -> Result<(), String> {
            *self.startup.lock().unwrap() = command.into();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeServer;
    use super::*;
    use serde_json::json;

    fn block<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// Capture, action simulée, constat, puis annulation : l'état d'origine doit revenir.
    async fn roundtrip(s: &FakeServer, plan: Plan, act: impl FnOnce(&FakeServer)) -> (Vec<UndoOp>, Option<String>, Capture) {
        let cap = capture(s, &plan, &mut Budget(STEP_CAP)).await;
        act(s);
        let (ops, note) = finalize(s, &cap.pre).await.unwrap();
        (ops, note, cap)
    }

    #[test]
    fn plans_cover_the_file_tools() {
        let a = json!({ "server_id": 3, "path": "/a.yml", "content": "x" });
        assert_eq!(plan_for("write_file", &a), Plan::Files { server: 3, paths: vec!["/a.yml".into()] });
        assert_eq!(plan_for("edit_file", &a), Plan::Files { server: 3, paths: vec!["/a.yml".into()] });
        assert_eq!(
            plan_for("upload_file", &json!({ "server_id": 3, "remote_path": "/plugins/x.jar", "local_path": "C:/x.jar" })),
            Plan::Files { server: 3, paths: vec!["/plugins/x.jar".into()] }
        );
        assert_eq!(plan_for("delete_path", &a), Plan::Delete { server: 3, path: "/a.yml".into() });
        assert_eq!(
            plan_for("rename_path", &json!({ "server_id": 3, "from": "/a", "to": "/b" })),
            Plan::Rename { server: 3, from: "/a".into(), to: "/b".into() }
        );
        assert_eq!(
            plan_for("compress_files", &json!({ "server_id": 3, "paths": ["/world", "/world_nether"] })),
            Plan::NewFiles { server: 3, dirs: vec!["/".into()], recursive: false }
        );
        assert_eq!(
            plan_for("compress_files", &json!({ "server_id": 3, "paths": ["/plugins/A/data"] })),
            Plan::NewFiles { server: 3, dirs: vec!["/plugins/A".into()], recursive: false }
        );
        assert_eq!(
            plan_for("copy_between_servers", &json!({ "source_server_id": 1, "target_server_id": 2, "target_dir": "/plugins" })),
            Plan::NewFiles { server: 2, dirs: vec!["/plugins".into()], recursive: true }
        );
        assert_eq!(
            plan_for("update_mods", &json!({ "server_id": 3, "kind": "plugins", "keys": ["A.jar"] })),
            Plan::ModUpdates { server: 3, kind: Some(ContentKind::Plugins), keys: vec!["A.jar".into()] }
        );
        assert_eq!(plan_for("write_file", &json!({ "path": "/a" })), Plan::Unreadable);
        assert_eq!(plan_for("rollback_changeset", &json!({})), Plan::Skip);
        assert!(matches!(plan_for("send_command", &json!({ "server_id": 1 })), Plan::Fixed(_)));
        assert!(matches!(plan_for("outil_futur", &json!({ "server_id": 1 })), Plan::Fixed(_)));
        // Point d'extension pré-branché : édition structurée d'un fichier de configuration.
        assert_eq!(
            plan_for("set_config", &json!({ "server_id": 3, "path": "/config/x.toml" })),
            Plan::Files { server: 3, paths: vec!["/config/x.toml".into()] }
        );
        assert_eq!(Plan::Rename { server: 1, from: "/a".into(), to: "/b".into() }.target(), "/a → /b");
        assert_eq!(Plan::Files { server: 1, paths: vec!["/a".into(), "/b".into()] }.target(), "/a (+1)");
    }

    #[test]
    fn written_file_is_restored_and_new_file_removed() {
        block(async {
            let s = FakeServer::new();
            s.put("/server.properties", "motd=avant");
            let orig = s.snapshot();
            let plan = Plan::Files { server: 1, paths: vec!["/server.properties".into(), "/nouveau.yml".into()] };
            let (ops, _, cap) = roundtrip(&s, plan, |s| {
                s.put("/server.properties", "motd=après");
                s.put("/nouveau.yml", "a: 1");
            })
            .await;
            assert_eq!(ops.len(), 2);
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
            // Une seconde annulation ne fait rien (déjà dans l'état d'origine).
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
        });
    }

    #[test]
    fn changed_target_is_refused_unless_forced() {
        block(async {
            let s = FakeServer::new();
            s.put("/a.yml", "v1");
            let plan = Plan::Files { server: 1, paths: vec!["/a.yml".into()] };
            let (ops, _, cap) = roundtrip(&s, plan, |s| s.put("/a.yml", "v2")).await;
            s.put("/a.yml", "v3 (modifié par quelqu'un d'autre)");
            let e = undo_ops(&s, &ops, &cap.blobs, false).await.unwrap_err();
            assert!(matches!(&e, UndoError::Refused(m) if m.contains("/a.yml") && m.contains("force")), "{e:?}");
            assert_eq!(s.text("/a.yml").unwrap(), "v3 (modifié par quelqu'un d'autre)", "rien touché");
            undo_ops(&s, &ops, &cap.blobs, true).await.unwrap();
            assert_eq!(s.text("/a.yml").unwrap(), "v1");
        });
    }

    #[test]
    fn unchanged_file_gives_no_operation() {
        block(async {
            let s = FakeServer::new();
            s.put("/a", "x");
            let (ops, _, _) = roundtrip(&s, Plan::Files { server: 1, paths: vec!["/a".into(), "/absent".into()] }, |_| {}).await;
            assert!(ops.is_empty());
        });
    }

    #[test]
    fn deleted_file_and_folder_come_back() {
        block(async {
            let s = FakeServer::new();
            s.put("/plugins/A/config.yml", "a: 1");
            s.put("/plugins/A/data/x.db", "binaire");
            s.dir("/plugins/A/vide");
            s.put("/ops.json", "[]");
            let orig = s.snapshot();
            let (ops, _, cap) = roundtrip(&s, Plan::Delete { server: 1, path: "/plugins/A".into() }, |s| s.delete("/plugins/A")).await;
            assert!(matches!(ops.as_slice(), [UndoOp::Tree { .. }]));
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
            let (ops, _, cap) = roundtrip(&s, Plan::Delete { server: 1, path: "/ops.json".into() }, |s| s.delete("/ops.json")).await;
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
        });
    }

    #[test]
    fn oversized_folder_or_file_is_not_captured() {
        block(async {
            let s = FakeServer::new();
            for i in 0..=TREE_MAX_ENTRIES {
                s.put(&format!("/world/region/r.{i}.mca"), "x");
            }
            let cap = capture(&s, &Plan::Delete { server: 1, path: "/world".into() }, &mut Budget(STEP_CAP)).await;
            assert!(cap.refused.as_deref().is_some_and(|r| r.contains("entrées")), "{cap:?}");
            assert!(cap.blobs.is_empty());
            // Budget dépassé : un fichier trop gros pour ce qui reste.
            s.put("/gros.jar", &"x".repeat(100));
            let cap = capture(&s, &Plan::Files { server: 1, paths: vec!["/gros.jar".into()] }, &mut Budget(10)).await;
            assert!(cap.refused.is_some());
        });
    }

    #[test]
    fn rename_is_reversed_with_the_overwritten_file() {
        block(async {
            let s = FakeServer::new();
            s.put("/a.yml", "A");
            s.put("/b.yml", "B (écrasé)");
            let orig = s.snapshot();
            let (ops, _, cap) = roundtrip(&s, Plan::Rename { server: 1, from: "/a.yml".into(), to: "/b.yml".into() }, |s| {
                s.delete("/b.yml");
                s.mv("/a.yml", "/b.yml");
            })
            .await;
            assert_eq!(ops.len(), 1);
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
            // Source recréée entre-temps : refus.
            let (ops, _, cap) = roundtrip(&s, Plan::Rename { server: 1, from: "/a.yml".into(), to: "/c.yml".into() }, |s| {
                s.mv("/a.yml", "/c.yml");
            })
            .await;
            s.put("/a.yml", "nouveau");
            assert!(matches!(undo_ops(&s, &ops, &cap.blobs, false).await, Err(UndoError::Refused(_))));
        });
    }

    #[test]
    fn created_dir_is_removed_only_when_empty() {
        block(async {
            let s = FakeServer::new();
            let (ops, _, cap) = roundtrip(&s, Plan::CreateDir { server: 1, path: "/plugins/New".into() }, |s| s.dir("/plugins/New")).await;
            assert_eq!(ops, vec![UndoOp::RemoveDir { path: "/plugins/New".into() }]);
            s.put("/plugins/New/autre.txt", "ajouté ensuite");
            let remarks = undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert!(remarks[0].contains("conservé") && s.exists("/plugins/New"));
            s.delete("/plugins/New/autre.txt");
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert!(!s.exists("/plugins/New"));
            // Dossier déjà présent : rien à défaire.
            s.dir("/mods");
            let (ops, _, _) = roundtrip(&s, Plan::CreateDir { server: 1, path: "/mods".into() }, |_| {}).await;
            assert!(ops.is_empty());
        });
    }

    #[test]
    fn extracted_files_are_removed_and_overwrites_reported() {
        block(async {
            let s = FakeServer::new();
            s.put("/world/level.dat", "ancien");
            let (ops, note, cap) = roundtrip(&s, Plan::NewFiles { server: 1, dirs: vec!["/world".into()], recursive: true }, |s| {
                s.put("/world/level.dat", "remplacé");
                s.put("/world/region/r.0.0.mca", "nouveau");
                s.put("/world/data/raids.dat", "nouveau");
            })
            .await;
            assert!(note.is_some_and(|n| n.contains("1 fichier")));
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert!(!s.exists("/world/region") && !s.exists("/world/data") && !s.exists("/world/region/r.0.0.mca"));
            assert_eq!(s.text("/world/level.dat").unwrap(), "remplacé", "non restaurable : signalé");
            // Dossier de destination créé par l'extraction : retiré aussi.
            let (ops, _, cap) = roundtrip(&s, Plan::NewFiles { server: 1, dirs: vec!["/modpack".into()], recursive: true }, |s| {
                s.put("/modpack/mods/a.jar", "a");
            })
            .await;
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert!(!s.exists("/modpack"));
        });
    }

    #[test]
    fn property_and_startup_are_restored_but_secrets_never_kept() {
        block(async {
            let s = FakeServer::new();
            s.props.lock().unwrap().insert("max-players".into(), "20".into());
            *s.startup.lock().unwrap() = "java -Xmx2G -jar {{SERVER_JARFILE}}".into();
            let (ops, _, cap) = roundtrip(&s, Plan::Property { server: 1, key: "max-players".into() }, |s| {
                s.props.lock().unwrap().insert("max-players".into(), "50".into());
            })
            .await;
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.props.lock().unwrap()["max-players"], "20");
            let (ops, _, cap) = roundtrip(&s, Plan::Startup { server: 1 }, |s| {
                *s.startup.lock().unwrap() = "java -Xmx4G -jar {{SERVER_JARFILE}}".into();
            })
            .await;
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert!(s.startup.lock().unwrap().contains("-Xmx2G"));
            s.props.lock().unwrap().insert("rcon.password".into(), "hunter2".into());
            let cap = capture(&s, &Plan::Property { server: 1, key: "rcon.password".into() }, &mut Budget(STEP_CAP)).await;
            assert!(cap.refused.is_some_and(|r| r.contains("secret")));
            assert_eq!(cap.pre, Pre::Nothing, "jamais l'ancienne valeur d'un secret");
        });
    }

    #[test]
    fn installed_and_updated_mods_are_undone() {
        block(async {
            let s = FakeServer::new();
            s.put("/plugins/Old-1.0.jar", "ancienne version");
            s.put("/plugins/Keep.jar", "inchangé");
            let orig = s.snapshot();
            // install_mod : un nouveau jar.
            let plan = plan_for("install_mod", &json!({ "server_id": 1, "slug": "x", "version_id": "y" }));
            let (ops, _, cap) = roundtrip(&s, plan, |s| s.put("/plugins/New-2.0.jar", "nouveau")).await;
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
            // update_mods : ancien remplacé par un nouveau nom.
            let plan = Plan::ModFiles { server: 1, files: vec!["/plugins/Old-1.0.jar".into()], note: None };
            let (ops, note, cap) = roundtrip(&s, plan, |s| {
                s.put("/plugins/Old-2.0.jar", "nouvelle version");
                s.delete("/plugins/Old-1.0.jar");
            })
            .await;
            assert!(note.is_none(), "{note:?}");
            undo_ops(&s, &ops, &cap.blobs, false).await.unwrap();
            assert_eq!(s.snapshot(), orig);
        });
    }

    #[test]
    fn mod_files_follow_the_update_selection() {
        use crate::updates::ContentUpdate;
        let up = |kind, key: &str, enabled| ContentUpdate {
            kind,
            key: key.into(),
            name: key.into(),
            enabled,
            current_version: "1".into(),
            new_version: "2".into(),
            new_filename: format!("new-{key}"),
            source: "modrinth".into(),
        };
        let current = ContentUpdates {
            mc_version: None,
            platform: None,
            checked_at: 0,
            examined: 3,
            untracked: 0,
            updates: vec![
                up(ContentKind::Mods, "sodium.jar", true),
                up(ContentKind::Plugins, "LP.jar", false),
                up(ContentKind::Datapacks, "pack", true),
            ],
            ignored: vec![],
            errors: vec![],
        };
        assert_eq!(mod_files(&current, None, &[]), (vec!["/mods/sodium.jar".to_string()], true));
        assert_eq!(mod_files(&current, None, &["LP.jar".into()]), (vec!["/plugins/LP.jar".to_string()], false));
        assert_eq!(mod_files(&current, Some(ContentKind::Plugins), &[]).0, Vec::<String>::new());
    }

    #[test]
    fn altered_copy_is_never_written() {
        block(async {
            let s = FakeServer::new();
            s.put("/a", "v1");
            let (ops, _, mut cap) = roundtrip(&s, Plan::Files { server: 1, paths: vec!["/a".into()] }, |s| s.put("/a", "v2")).await;
            cap.blobs[0] = b"autre".to_vec();
            assert!(matches!(undo_ops(&s, &ops, &cap.blobs, false).await, Err(UndoError::Failed(m)) if m.contains("altérée")));
            assert_eq!(s.text("/a").unwrap(), "v2");
        });
    }
}
