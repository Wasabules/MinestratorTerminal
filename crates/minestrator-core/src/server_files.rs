//! Opérations de fichiers **côté serveur** par l'API REST du panel MineStrator (endpoints non
//! documentés, format relevé dans le code public du panel — cf. `docs/api/REFERENCE.md`) :
//! compression en tâche de fond suivie (« job »), extraction sur place, analyse de l'espace
//! disque.
//!
//! Principe : le **SFTP reste le transport** (session poolée, flux binaire, reprise, aucune limite
//! de débit de l'API). L'API ne sert qu'à ce que le SFTP ne sait pas faire **sur le serveur** :
//! compresser un monde de plusieurs Go sans le rapatrier fichier par fichier, extraire une archive
//! sans la faire transiter par l'appareil, mesurer la taille des dossiers.
//!
//! Ce module est pur : chemins, corps de requête, validation (mêmes règles que le panel), lecture
//! tolérante des réponses, suivi générique d'un job et choix du repli. Le réseau vit dans
//! `api.rs`, l'orchestration (repli SFTP, téléchargement, nettoyage) dans `lib.rs`.
//!
//! Chemins : l'app manipule des chemins SFTP absolus (`/world/region`) ; le panel, des chemins
//! relatifs à la racine du serveur, sans `/` initial (`world/region`, racine = chaîne vide). Les
//! deux racines coïncident (dossier du conteneur).

use crate::error::{Error, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

/// Attente maximale d'un appel de suivi de job (`?wait=`, en secondes) : le serveur ne répond
/// qu'au changement d'état ou au bout de ce délai (« long polling »), comme dans le panel.
pub const JOB_WAIT_S: u64 = 20;
/// Appels de suivi au-delà desquels on cesse d'attendre (≈ 30 min, comme le panel) : la
/// compression continue sur le serveur, l'archive y apparaîtra une fois prête.
pub const JOB_MAX_POLLS: usize = 90;
/// Échecs de suivi **consécutifs** tolérés (coupure réseau passagère) avant d'abandonner.
pub const JOB_MAX_POLL_ERRORS: usize = 10;
/// Nombre d'entrées demandées par défaut à l'analyse du disque (valeur du panel).
pub const DISK_USAGE_DEFAULT_LIMIT: u32 = 20;
/// Plafond du nombre d'entrées d'une analyse du disque.
pub const DISK_USAGE_MAX_LIMIT: u32 = 100;

/// Extensions que l'extraction **côté serveur** accepte (liste du panel). Plus large que
/// l'extraction locale (`archive.rs` : `.zip`, `.tar`, `.tar.gz`).
pub const SERVER_EXTRACT_EXTS: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz2", ".txz", ".zip", ".jar",
    ".mcaddon", ".mcpack", ".mcworld", ".tar", ".gz", ".bz2", ".xz", ".zst", ".rar", ".7z",
];

/// Préfixe des archives temporaires créées par l'app pour un téléchargement (supprimées ensuite).
pub const TEMP_ARCHIVE_PREFIX: &str = "minestrator-temp-";

/// Longueur maximale d'un chemin de destination d'extraction (règle du panel).
const MAX_DEST_LEN: usize = 300;
/// Longueur maximale d'un segment de chemin, en octets (règle du panel).
const MAX_SEGMENT_BYTES: usize = 255;
/// Noms réservés refusés par le panel comme segment de destination.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
/// Caractères interdits dans un nom d'archive (règle du panel).
const FORBIDDEN_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Ponctuation admise dans une destination d'extraction, en plus des lettres et chiffres.
const DEST_PUNCT: &str = "/_. ()'-+,=!@#~[]{}^&$%?;\u{FFFD}";

fn invalid(msg: impl Into<String>) -> Error {
    Error::Unexpected(msg.into())
}

// --- Chemins ----------------------------------------------------------------

/// Chemin SFTP absolu (`/world/region`) → chemin du panel, relatif à la racine du serveur, sans
/// `/` initial ni final (`world/region`) ; la racine donne une chaîne vide.
pub fn panel_path(p: &str) -> String {
    p.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Chemin du panel (ou SFTP) → chemin SFTP absolu (`/` pour la racine).
pub fn sftp_path(p: &str) -> String {
    format!("/{}", panel_path(p))
}

fn has_parent_ref(p: &str) -> bool {
    p.replace('\\', "/").split('/').any(|s| s == "..")
}

/// Dernier segment d'un chemin.
fn base_name(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// Dossier parent d'un chemin du panel (`""` à la racine).
fn panel_parent(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
}

/// Joint un dossier SFTP et un nom.
fn sftp_join(dir: &str, name: &str) -> String {
    let d = sftp_path(dir);
    if d == "/" {
        format!("/{name}")
    } else {
        format!("{d}/{name}")
    }
}

/// L'archive peut-elle être extraite par le serveur (formats du panel) ?
pub fn server_extractable(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SERVER_EXTRACT_EXTS.iter().any(|ext| n.ends_with(ext))
}

// --- Compression --------------------------------------------------------------

/// Dossier commun d'une sélection à compresser (chemin du panel). Comme dans le panel, la
/// sélection doit tenir dans **un seul dossier** : l'archive y est créée. La racine elle-même ne
/// se compresse pas (on y sélectionne ses éléments).
pub fn selection_parent(paths: &[String]) -> Result<String> {
    let mut parent: Option<String> = None;
    if paths.is_empty() {
        return Err(invalid("Aucun fichier ni dossier sélectionné."));
    }
    for p in paths {
        if has_parent_ref(p) {
            return Err(invalid(format!("Chemin refusé (« .. ») : {p}")));
        }
        let rel = panel_path(p);
        if rel.is_empty() {
            return Err(invalid("La racine du serveur ne peut pas être compressée en un bloc : sélectionne ses éléments."));
        }
        let here = panel_parent(&rel).to_string();
        match &parent {
            None => parent = Some(here),
            Some(prev) if *prev != here => {
                return Err(invalid("La sélection doit se trouver dans un seul dossier."));
            }
            Some(_) => {}
        }
    }
    Ok(parent.unwrap_or_default())
}

/// Nom d'archive validé (règles du panel) : `.zip` ajouté au besoin, caractères `< > : " / \ | ? *`
/// et de contrôle refusés.
pub fn archive_name(name: &str) -> Result<String> {
    let mut n = name.trim().to_string();
    if !n.to_ascii_lowercase().ends_with(".zip") {
        n.push_str(".zip");
    }
    let stem = &n[..n.len() - 4];
    if stem.trim().is_empty() || stem.trim_matches('.').is_empty() {
        return Err(invalid("Nom d'archive vide."));
    }
    if n.chars().any(|c| FORBIDDEN_NAME_CHARS.contains(&c) || c.is_control()) {
        return Err(invalid("Le nom d'archive contient un caractère interdit (< > : \" / \\ | ? *)."));
    }
    if n.len() > MAX_SEGMENT_BYTES {
        return Err(invalid("Nom d'archive trop long."));
    }
    Ok(n)
}

/// Horodatage du nom par défaut d'une archive, au format du panel (`2026-11-18_15h04`).
pub fn panel_stamp(now: chrono::NaiveDateTime) -> String {
    now.format("%Y-%m-%d_%Hh%M").to_string()
}

/// Nom de base lisible d'une sélection : le dossier, ou le fichier sans son extension, s'il n'y
/// en a qu'un ; sinon le dossier qui la contient (`archive` à la racine).
fn selection_stem(paths: &[String]) -> String {
    let stem = match paths {
        [one] => {
            let name = base_name(one);
            match name.rfind('.') {
                Some(i) if i > 0 => name[..i].to_string(),
                _ => name.to_string(),
            }
        }
        _ => {
            let parent = paths.first().map(|p| panel_parent(&panel_path(p)).to_string()).unwrap_or_default();
            if parent.is_empty() {
                "archive".to_string()
            } else {
                base_name(&parent).to_string()
            }
        }
    };
    // Nom sûr pour l'API : caractères interdits remplacés, longueur bornée.
    let clean: String = stem
        .chars()
        .map(|c| if FORBIDDEN_NAME_CHARS.contains(&c) || c.is_control() { '_' } else { c })
        .take(80)
        .collect();
    if clean.trim().is_empty() {
        "archive".to_string()
    } else {
        clean
    }
}

/// Nom par défaut proposé pour « Compresser sur le serveur », comme le panel :
/// `world_2026-11-18_15h04.zip`.
pub fn default_archive_name(paths: &[String], now: chrono::NaiveDateTime) -> String {
    format!("{}_{}.zip", selection_stem(paths), panel_stamp(now))
}

/// Nom de l'archive **temporaire** créée pour un téléchargement : préfixe reconnaissable (on sait
/// d'un coup d'œil qu'elle appartient à l'app) et horodatage à la seconde (pas de collision avec
/// une archive existante : l'API refuse d'écraser).
pub fn temp_archive_name(paths: &[String], now: chrono::NaiveDateTime) -> String {
    format!("{TEMP_ARCHIVE_PREFIX}{}-{}.zip", selection_stem(paths), now.format("%Y%m%d-%H%M%S"))
}

/// Requête de compression prête à envoyer.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressRequest {
    /// Corps de `POST /server/{id}/files/compress/jobs`.
    pub body: Value,
    /// Chemin SFTP absolu attendu de l'archive (dossier de la sélection + nom).
    pub archive_path: String,
    /// Nom de l'archive.
    pub name: String,
    /// Dossier de la sélection (chemin SFTP absolu).
    pub dir: String,
}

/// Construit la requête de compression (format du panel) :
/// `{ path, files, name, cwd, overwrite: false }`, `files` = chemins relatifs à la racine.
pub fn compress_request(paths: &[String], name: &str) -> Result<CompressRequest> {
    let cwd = selection_parent(paths)?;
    let name = archive_name(name)?;
    let mut files: Vec<String> = Vec::with_capacity(paths.len());
    for p in paths {
        let rel = panel_path(p);
        if !files.contains(&rel) {
            files.push(rel);
        }
    }
    let dir = sftp_path(&cwd);
    Ok(CompressRequest {
        body: json!({ "path": cwd, "files": files, "name": name, "cwd": cwd, "overwrite": false }),
        archive_path: sftp_join(&dir, &name),
        name,
        dir,
    })
}

/// Explication lisible des refus connus de la compression (codes d'erreur du panel).
pub fn compress_error_message(code: &str) -> Option<&'static str> {
    match code {
        "API_ZIP_EXISTS" => Some("Une archive de ce nom existe déjà dans ce dossier."),
        "API_ZIP_BUSY" => Some("Une compression est déjà en cours sur ce serveur : réessaie quand elle sera terminée."),
        _ => None,
    }
}

/// Avancement d'un job de compression, lu avec tolérance (l'API est non documentée).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CompressJob {
    pub id: i64,
    /// `pending` | `running` | `completed` | `failed` (tel que renvoyé par l'API).
    pub status: String,
    /// En file d'attente derrière une autre compression.
    pub queued: bool,
    /// Fichiers traités.
    pub file_count: u64,
    /// Octets traités.
    pub total_bytes: u64,
    /// Durée écoulée (secondes).
    pub elapsed_s: u64,
    /// Chemin de l'archive tel que renvoyé (forme non garantie : seul son nom est exploité).
    pub archive_path: String,
    /// Archive créée mais incomplète (fichiers illisibles ignorés).
    pub partial: bool,
    /// Fichiers ignorés d'une archive partielle.
    pub skipped_count: u64,
    /// Raison d'un échec (`error_message`, à défaut `error_code`).
    pub error: Option<String>,
}

fn num(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64)),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| *f >= 0.0).map(|f| f.round() as u64),
        _ => None,
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true"),
        _ => false,
    }
}

impl CompressJob {
    /// Lit un job (`data` de la création ou du suivi). `None` sans identifiant exploitable.
    pub fn from_value(v: &Value) -> Option<Self> {
        let id = num(v.get("id")).filter(|id| *id > 0)? as i64;
        let progress = v.get("progress");
        let p = |k: &str| progress.and_then(|p| p.get(k));
        Some(Self {
            id,
            status: text(v.get("status")).unwrap_or_default().to_ascii_lowercase(),
            queued: flag(p("queued")),
            file_count: num(p("file_count")).or_else(|| num(v.get("file_count"))).unwrap_or(0),
            total_bytes: num(p("total_size")).or_else(|| num(v.get("total_bytes"))).unwrap_or(0),
            elapsed_s: num(p("elapsed_s")).unwrap_or(0),
            archive_path: text(v.get("archive_path")).unwrap_or_default(),
            partial: flag(v.get("partial")),
            skipped_count: num(v.get("skipped_count")).unwrap_or(0),
            error: text(v.get("error_message")).or_else(|| text(v.get("error_code"))),
        })
    }

    /// Le job est-il fini, d'une façon ou d'une autre ?
    pub fn is_terminal(&self) -> bool {
        self.succeeded() || self.failed()
    }

    pub fn succeeded(&self) -> bool {
        self.status == "completed"
    }

    /// Échec, y compris sous des libellés que le panel ne connaît pas encore.
    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "failed" | "error" | "cancelled" | "canceled")
    }

    /// En cours (création ou compression) — sert à repérer un job déjà lancé.
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "running" | "queued")
    }
}

/// Lit la liste `GET …/compress/jobs` : `{ jobs: [...], count }`.
pub fn parse_jobs(data: &Value) -> Vec<CompressJob> {
    data.get("jobs")
        .and_then(Value::as_array)
        .map(|jobs| jobs.iter().filter_map(CompressJob::from_value).collect())
        .unwrap_or_default()
}

/// Chemin réel de l'archive : celui attendu (dossier de la sélection + nom demandé), sauf si le
/// serveur annonce un autre nom de fichier (renommage) — seul le nom est repris, jamais le
/// dossier, dont la forme renvoyée n'est pas garantie.
pub fn resolve_archive_path(expected: &str, job: &CompressJob) -> String {
    let announced = base_name(&job.archive_path);
    if announced.is_empty() || announced == base_name(expected) || has_parent_ref(announced) {
        return expected.to_string();
    }
    let dir = expected.rfind('/').map_or("", |i| &expected[..i]);
    format!("{dir}/{announced}")
}

/// Fin du suivi d'un job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEnd {
    /// Archive créée (éventuellement partielle : voir `partial`/`skipped_count`).
    Done(CompressJob),
    /// Le serveur a renoncé ; l'archive n'existe pas ou est inutilisable.
    Failed(CompressJob),
    /// Toujours en cours au bout de [`JOB_MAX_POLLS`] appels : la compression continue sur le
    /// serveur.
    StillRunning(CompressJob),
    /// Suivi interrompu par l'utilisateur (la compression, elle, continue sur le serveur : l'API
    /// ne permet pas de l'arrêter).
    Cancelled(CompressJob),
}

/// Suit un job de compression jusqu'au bout, à la manière du panel : appels de suivi « longs »
/// (`poll`), courte pause (`pause`) après un appel raté, abandon après
/// [`JOB_MAX_POLL_ERRORS`] échecs consécutifs ou `max_polls` appels ([`JOB_MAX_POLLS`] pour un
/// suivi interactif). `on` reçoit chaque
/// état (progression). Générique sur le transport : testé sans réseau.
pub(crate) async fn follow_job<P, PF, S, SF>(
    first: CompressJob,
    mut poll: P,
    mut pause: S,
    on: &mut impl FnMut(&CompressJob),
    cancel: &AtomicBool,
    max_polls: usize,
) -> Result<JobEnd>
where
    P: FnMut(i64) -> PF,
    PF: Future<Output = Result<CompressJob>>,
    S: FnMut() -> SF,
    SF: Future<Output = ()>,
{
    let mut job = first;
    on(&job);
    let mut errors = 0usize;
    let mut polls = 0usize;
    while !job.is_terminal() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(JobEnd::Cancelled(job));
        }
        if polls >= max_polls {
            return Ok(JobEnd::StillRunning(job));
        }
        polls += 1;
        match poll(job.id).await {
            Ok(next) => {
                errors = 0;
                job = next;
                on(&job);
            }
            // La clé ne passe plus : insister n'y changera rien.
            Err(e @ (Error::Unauthorized | Error::NoKey)) => return Err(e),
            Err(e) => {
                errors += 1;
                if errors >= JOB_MAX_POLL_ERRORS {
                    return Err(e);
                }
                pause().await;
            }
        }
    }
    Ok(if job.succeeded() { JobEnd::Done(job) } else { JobEnd::Failed(job) })
}

// --- Extraction ---------------------------------------------------------------

/// Destination d'extraction validée (règles du panel), en chemin du panel (sans `/` initial).
pub fn extract_destination(dest: &str) -> Result<String> {
    let lower = dest.to_ascii_lowercase();
    if dest.chars().any(char::is_control)
        || ["%2e", "%2f", "%5c", "%00", "%0a", "%0d"].iter().any(|bad| lower.contains(bad))
    {
        return Err(invalid("La destination contient des caractères invalides."));
    }
    if has_parent_ref(dest) {
        return Err(invalid("La destination ne peut pas remonter hors du serveur (« .. »)."));
    }
    let path = panel_path(dest);
    if path.len() > MAX_DEST_LEN {
        return Err(invalid("Chemin de destination trop long."));
    }
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if RESERVED_NAMES.contains(&seg.to_ascii_uppercase().as_str()) {
            return Err(invalid(format!("« {seg} » est un nom réservé.")));
        }
        if seg.len() > MAX_SEGMENT_BYTES {
            return Err(invalid("Chemin de destination trop long."));
        }
    }
    if !path.chars().all(|c| c.is_alphanumeric() || DEST_PUNCT.contains(c)) {
        return Err(invalid("La destination contient des caractères invalides."));
    }
    Ok(path)
}

/// Corps de `POST /server/{id}/files/extract` (format du panel) :
/// `{ file: "dossier/archive.zip", destination: "/dossier" }`.
pub fn extract_body(archive: &str, dest_dir: &str) -> Result<Value> {
    if has_parent_ref(archive) {
        return Err(invalid(format!("Chemin refusé (« .. ») : {archive}")));
    }
    let file = panel_path(archive);
    if file.is_empty() {
        return Err(invalid("Aucune archive indiquée."));
    }
    let destination = format!("/{}", extract_destination(dest_dir)?);
    Ok(json!({ "file": file, "destination": destination }))
}

/// Lit la réponse d'une extraction : destination effective (chemin SFTP) et fichiers ignorés
/// (`skipped` : nombre, ou objet de compteurs par motif).
pub fn parse_extract_response(data: Option<&Value>, requested: &str) -> (String, u64) {
    let dest = data
        .and_then(|d| text(d.get("destination")))
        .map(|d| sftp_path(&d))
        .unwrap_or_else(|| sftp_path(requested));
    let skipped = match data.and_then(|d| d.get("skipped")) {
        Some(Value::Object(m)) => m.values().filter_map(|v| num(Some(v))).sum(),
        other => num(other).unwrap_or(0),
    };
    (dest, skipped)
}

/// Résultat d'une extraction.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExtractOutcome {
    /// Dossier où l'archive a été extraite (chemin SFTP absolu).
    pub destination: String,
    /// `true` : extraite par le serveur ; `false` : repli par l'appareil (SFTP).
    pub on_server: bool,
    /// Fichiers écrits (connu seulement pour le repli par l'appareil).
    pub files: Option<u64>,
    /// Fichiers ignorés par le serveur (extraction partielle).
    pub skipped: u64,
}

/// Résultat d'une compression côté serveur.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CompressOutcome {
    /// Chemin SFTP absolu de l'archive.
    pub archive_path: String,
    /// `true` : la compression continue sur le serveur (suivi arrêté) ; l'archive n'est pas
    /// encore complète.
    pub still_running: bool,
    /// Fichiers compressés.
    pub file_count: u64,
    /// Taille de l'archive (octets, lue par SFTP ; 0 si inconnue).
    pub size: u64,
    /// Fichiers ignorés (archive partielle).
    pub skipped: u64,
}

/// Faut-il retomber sur le chemin SFTP (local) après un échec de l'API ?
///
/// Oui dans presque tous les cas : endpoint retiré ou modifié (`Api`, `Unexpected`), refus de la
/// clé pour l'API fichiers (`Forbidden`), quota (`RateLimited` : le SFTP n'en a pas), API
/// injoignable (`Network` : le SFTP l'est peut-être). Non quand la clé elle-même est refusée : les
/// identifiants SFTP viennent aussi de l'API, le repli échouerait de la même façon.
pub fn fallback_on(err: &Error) -> bool {
    !matches!(
        err,
        Error::Unauthorized
            | Error::NoKey
            | Error::SftpAuth
            | Error::SftpPasswordRequired
            | Error::SftpHostKeyChanged { .. }
    )
}

// --- Analyse du disque ----------------------------------------------------------

/// Une entrée de l'analyse du disque (taille récursive pour un dossier).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiskUsageItem {
    /// Chemin SFTP absolu.
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    /// Octets (récursif pour un dossier).
    pub size: u64,
}

/// Ce qui occupe le plus de place dans un dossier, du plus gros au plus petit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiskUsage {
    /// Dossier analysé (chemin SFTP absolu, `/` pour la racine).
    pub path: String,
    /// Taille totale du dossier (octets, récursif).
    pub total_size: u64,
    /// Plus grosses entrées directes du dossier, triées par taille décroissante (bornées par
    /// `limit` : la somme peut être inférieure au total).
    pub items: Vec<DiskUsageItem>,
}

/// Lit `GET /server/{id}/disk/usage` : `{ analyzed_path, total_size, items: [{ path, name, type,
/// size }] }` (chemins du panel, `type` ∈ `folder` | `file`, tri décroissant déjà fait par l'API
/// mais refait ici par sûreté).
pub fn parse_disk_usage(data: &Value, requested: &str) -> DiskUsage {
    let analyzed = text(data.get("analyzed_path")).unwrap_or_else(|| panel_path(requested));
    let mut items: Vec<DiskUsageItem> = data
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    let name = text(i.get("name"))?;
                    let rel = text(i.get("path"))
                        .unwrap_or_else(|| format!("{}/{name}", panel_path(&analyzed)));
                    let kind = text(i.get("type")).unwrap_or_default().to_ascii_lowercase();
                    Some(DiskUsageItem {
                        path: sftp_path(&rel),
                        is_dir: matches!(kind.as_str(), "folder" | "directory" | "dir"),
                        size: num(i.get("size")).unwrap_or(0),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    DiskUsage {
        path: sftp_path(&analyzed),
        total_size: num(data.get("total_size")).unwrap_or_else(|| items.iter().map(|i| i.size).sum()),
        items,
    }
}

/// Paramètres de requête de l'analyse du disque (`limit`, et `path` hors racine).
pub fn disk_usage_query(path: &str, limit: u32) -> Vec<(&'static str, String)> {
    let limit = if limit == 0 { DISK_USAGE_DEFAULT_LIMIT } else { limit.min(DISK_USAGE_MAX_LIMIT) };
    let mut q = vec![("limit", limit.to_string())];
    let rel = panel_path(path);
    if !rel.is_empty() {
        q.push(("path", rel));
    }
    q
}

/// Espace disque d'un serveur : occupation comptée par l'hébergeur et limite de l'offre.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiskSpace {
    /// Occupation **comptée** (octets) : le disque brut, moins la franchise des fichiers de base
    /// du jeu quand elle s'applique (`GET /server/{id}/disk/total`).
    pub used_bytes: u64,
    /// Occupation brute du disque (octets).
    pub raw_bytes: u64,
    /// Franchise des fichiers de base du jeu (octets), déduite si `allowance_applied`.
    pub base_game_bytes: u64,
    pub allowance_applied: bool,
    /// Limite du serveur (octets ; franchise comprise). 0 si inconnue.
    pub limit_bytes: u64,
    /// Espace de la MyBox, partagé par ses serveurs (absent si indisponible).
    pub mybox: Option<MyBoxDiskSpace>,
}

/// Espace disque d'une MyBox : occupation de tous ses serveurs face au disque de l'offre.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MyBoxDiskSpace {
    pub used_bytes: u64,
    /// Disque de l'offre, extensions comprises (octets). 0 si inconnu.
    pub limit_bytes: u64,
}

/// Lit `GET /server/{id}/disk/total` : `{ total_bytes, raw_bytes, base_game_bytes,
/// overhead_applied }` (complété ensuite de la limite).
pub fn parse_disk_total(data: &Value) -> DiskSpace {
    let raw = num(data.get("raw_bytes")).unwrap_or(0);
    DiskSpace {
        used_bytes: num(data.get("total_bytes")).unwrap_or(raw),
        raw_bytes: raw,
        base_game_bytes: num(data.get("base_game_bytes")).unwrap_or(0),
        allowance_applied: flag(data.get("overhead_applied")),
        limit_bytes: 0,
        mybox: None,
    }
}

const MIB: u64 = 1024 * 1024;

/// Octets d'une quantité exprimée dans l'unité renvoyée par l'API (`MB` par défaut ; unités
/// binaires, comme les limites de `live/light`).
pub fn bytes_from_unit(value: u64, unit: &str) -> u64 {
    let factor = match unit.trim().to_ascii_uppercase().as_str() {
        "B" | "O" => 1,
        "KB" | "KO" | "KIB" => 1024,
        "GB" | "GO" | "GIB" => 1024 * MIB,
        "TB" | "TO" | "TIB" => 1024 * 1024 * MIB,
        _ => MIB,
    };
    value.saturating_mul(factor)
}

/// Lit `GET /mybox/{id}/disk` : `{ disk_servers, disk_servers_unit }` → octets occupés.
pub fn parse_mybox_disk(data: &Value) -> u64 {
    let value = num(data.get("disk_servers")).unwrap_or(0);
    let unit = text(data.get("disk_servers_unit")).unwrap_or_else(|| "MB".into());
    bytes_from_unit(value, &unit)
}

/// Octets d'une limite exprimée en Mio (`live/light` : `disk.limit`).
pub fn mib_to_bytes(mib: i64) -> u64 {
    u64::try_from(mib).unwrap_or(0).saturating_mul(MIB)
}

/// Octets d'une quantité en Gio (disque de l'offre d'une MyBox).
pub fn gib_to_bytes(gib: i64) -> u64 {
    u64::try_from(gib).unwrap_or(0).saturating_mul(1024 * MIB)
}

/// Taille lisible, en unités binaires (`o`, `Kio`, `Mio`, `Gio`, `Tio`).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{n} o"),
        _ if v >= 100.0 => format!("{v:.0} {}", UNITS[i]),
        _ => format!("{v:.1} {}", UNITS[i]),
    }
}

fn percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        ((part as f64 / whole as f64) * 100.0).round() as u64
    }
}

/// Rapport texte de l'analyse du disque (outil MCP `disk_usage`) : total du dossier, occupation
/// face aux limites (à la racine), plus grosses entrées avec leur part du dossier.
pub fn disk_usage_report(u: &DiskUsage, space: Option<&DiskSpace>) -> String {
    use std::fmt::Write as _;
    let mut out = format!("Dossier « {} » : {} au total.", u.path, human_bytes(u.total_size));
    if let Some(s) = space {
        if s.limit_bytes > 0 {
            let _ = write!(
                out,
                "\nServeur : {} comptés sur {} autorisés ({} %).",
                human_bytes(s.used_bytes),
                human_bytes(s.limit_bytes),
                percent(s.used_bytes, s.limit_bytes)
            );
        } else {
            let _ = write!(out, "\nServeur : {} comptés.", human_bytes(s.used_bytes));
        }
        if s.allowance_applied {
            let _ = write!(
                out,
                " (disque brut {}, dont {} de fichiers du jeu non comptés)",
                human_bytes(s.raw_bytes),
                human_bytes(s.base_game_bytes)
            );
        }
        if let Some(m) = &s.mybox {
            if m.limit_bytes > 0 {
                let _ = write!(
                    out,
                    "\nMyBox (disque partagé par ses serveurs) : {} sur {} ({} %).",
                    human_bytes(m.used_bytes),
                    human_bytes(m.limit_bytes),
                    percent(m.used_bytes, m.limit_bytes)
                );
            } else {
                let _ = write!(out, "\nMyBox (disque partagé par ses serveurs) : {} occupés.", human_bytes(m.used_bytes));
            }
        }
    }
    if u.items.is_empty() {
        out.push_str("\n(dossier vide)");
        return out;
    }
    let _ = write!(out, "\nPlus grosses entrées ({}), de la plus grosse à la plus petite :", u.items.len());
    for i in &u.items {
        let _ = write!(
            out,
            "\n- {}{} — {} ({} %)",
            i.path,
            if i.is_dir { "/" } else { "" },
            human_bytes(i.size),
            percent(i.size, u.total_size)
        );
    }
    out
}

/// Le chemin désigne-t-il une archive **temporaire** de l'app ? Seules celles-ci sont supprimées
/// automatiquement : rien d'autre ne l'est jamais, même en cas de doute.
pub fn is_temp_archive(path: &str) -> bool {
    let name = base_name(path);
    name.starts_with(TEMP_ARCHIVE_PREFIX) && name.to_ascii_lowercase().ends_with(".zip") && !has_parent_ref(path)
}

/// Phase publiée pendant la compression (`queued` si le job attend son tour).
fn compress_phase(job: &CompressJob) -> &'static str {
    if job.queued {
        "queued"
    } else {
        "compressing"
    }
}

// --- Orchestration (Core) ---------------------------------------------------------

/// Pause après un appel de suivi raté (valeur du panel).
const POLL_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);
/// Budget de suivi pour retirer une archive temporaire dont le téléchargement a été abandonné
/// (≈ 1 h 30 en appels « longs ») : au-delà, elle reste sur le serveur, reconnaissable à son nom.
const CLEANUP_MAX_POLLS: usize = 270;

/// Issue d'une compression lancée par l'app.
enum Launch {
    /// Le serveur n'a pas accepté la demande : rien n'a été créé.
    Refused(Error),
    /// Job créé puis suivi jusqu'à sa fin (ou l'abandon du suivi).
    Followed(JobEnd),
    /// Job créé, mais suivi perdu (API injoignable) : il continue peut-être sur le serveur.
    Lost(CompressJob, Error),
}

/// Suite d'un téléchargement groupé une fois la compression côté serveur terminée (ou non).
#[derive(Debug)]
enum Route {
    /// Archive prête : on la télécharge.
    Download(CompressJob),
    /// Le serveur n'a pas pu : repli sur l'archive construite par l'appareil. Porte la raison
    /// (journal) et le job échoué dont l'archive incomplète est à retirer.
    Local(String, Option<CompressJob>),
    /// On s'arrête là (annulation, suivi perdu, trop long, clé refusée). Porte le job dont
    /// l'archive temporaire sera retirée une fois le serveur arrivé au bout.
    Stop(Error, Option<CompressJob>),
}

/// Choix « archive du serveur ou repli local » d'un téléchargement groupé. Une annulation l'emporte
/// toujours : ni téléchargement ni repli.
fn server_or_local(launch: Launch, cancelled: bool) -> Route {
    match launch {
        Launch::Followed(JobEnd::Done(job)) if !cancelled => Route::Download(job),
        Launch::Refused(e) if !cancelled && fallback_on(&e) => Route::Local(e.to_string(), None),
        Launch::Followed(JobEnd::Failed(job)) if !cancelled => {
            Route::Local(job.error.clone().unwrap_or_else(|| "échec du job".into()), Some(job))
        }
        other => {
            let (err, pending) = abandoned(other);
            Route::Stop(err, pending)
        }
    }
}

/// Erreur à remonter pour une compression temporaire qui n'aboutit pas à un téléchargement, et
/// job dont l'archive temporaire reste à retirer.
fn abandoned(launch: Launch) -> (Error, Option<CompressJob>) {
    match launch {
        Launch::Refused(e) => (e, None),
        Launch::Followed(JobEnd::Done(job) | JobEnd::Cancelled(job)) => {
            (Error::Unexpected("Transfert annulé.".into()), Some(job))
        }
        Launch::Followed(JobEnd::Failed(job)) => {
            let reason = job.error.clone().map(|r| format!(" : {r}")).unwrap_or_default();
            (Error::Unexpected(format!("Le serveur n'a pas pu créer l'archive{reason}.")), Some(job))
        }
        Launch::Followed(JobEnd::StillRunning(job)) => (
            Error::Unexpected(
                "La compression sur le serveur dure trop longtemps : téléchargement abandonné (l'archive temporaire sera retirée à la fin).".into(),
            ),
            Some(job),
        ),
        Launch::Lost(job, e) => (
            Error::Unexpected(format!("Suivi de la compression perdu ({e}) : téléchargement abandonné.")),
            Some(job),
        ),
    }
}

fn now_local() -> chrono::NaiveDateTime {
    chrono::Local::now().naive_local()
}

impl crate::Core {
    fn emit_progress(&self, p: crate::SftpProgress) {
        let _ = self.events.send(crate::CoreEvent::SftpProgress(p));
    }

    /// Fin d'une opération suivie (succès ou échec), avec les fichiers ignorés et une phase
    /// finale éventuelle (`still_running`).
    fn emit_end(&self, tid: &str, name: &str, direction: &str, res: std::result::Result<(u64, Option<&str>), &Error>) {
        let (status, error, skipped, phase) = match res {
            Ok((skipped, phase)) => ("done", None, skipped, phase.map(str::to_string)),
            Err(e) => ("error", Some(e.to_string()), 0, None),
        };
        self.emit_progress(crate::SftpProgress {
            id: tid.to_string(),
            name: name.to_string(),
            direction: direction.to_string(),
            status: status.to_string(),
            error,
            skipped,
            phase,
            ..Default::default()
        });
    }

    /// Lance la compression de `req` sur le serveur et la suit jusqu'au bout en publiant la
    /// progression (`direction` : `server` pour une archive conservée, `down` quand elle prépare
    /// un téléchargement).
    async fn launch_compression(
        &self,
        id: i64,
        req: &CompressRequest,
        tid: &str,
        label: &str,
        direction: &str,
        cancel: &AtomicBool,
    ) -> Launch {
        let token = match self.token() {
            Ok(t) => t,
            Err(e) => return Launch::Refused(e),
        };
        self.emit_progress(crate::SftpProgress {
            id: tid.to_string(),
            name: label.to_string(),
            direction: direction.to_string(),
            status: "active".into(),
            phase: Some("compressing".into()),
            ..Default::default()
        });
        let first = match self.api.compress_start(&token, id, &req.body).await {
            Ok(job) => job,
            Err(Error::Api { code }) => {
                let err = match compress_error_message(&code) {
                    Some(msg) => Error::Unexpected(msg.into()),
                    None => Error::Api { code },
                };
                return Launch::Refused(err);
            }
            Err(e) => return Launch::Refused(e),
        };
        tracing::info!(server = id, job = first.id, "compression lancée sur le serveur");
        let mut last = first.clone();
        let mut on = |job: &CompressJob| {
            last = job.clone();
            self.emit_progress(crate::SftpProgress {
                id: tid.to_string(),
                name: label.to_string(),
                direction: direction.to_string(),
                status: "active".into(),
                done: job.total_bytes,
                files: job.file_count,
                phase: Some(compress_phase(job).into()),
                ..Default::default()
            });
        };
        let token_ref = token.as_str();
        let followed = follow_job(
            first,
            |job_id| self.api.compress_job(token_ref, id, job_id),
            || tokio::time::sleep(POLL_RETRY_PAUSE),
            &mut on,
            cancel,
            JOB_MAX_POLLS,
        )
        .await;
        match followed {
            Ok(end) => Launch::Followed(end),
            Err(e) => Launch::Lost(last, e),
        }
    }

    /// Supprime une archive **temporaire** de l'app (refuse tout autre chemin). Sans erreur si
    /// elle a déjà disparu.
    pub async fn remove_temp_archive(&self, id: i64, path: &str) -> Result<()> {
        if !is_temp_archive(path) {
            return Err(Error::Unexpected(format!(
                "« {path} » n'est pas une archive temporaire de l'application : non supprimée."
            )));
        }
        match self.sftp_delete(id, path, false).await {
            Ok(()) => Ok(()),
            // Déjà absente (job échoué avant d'écrire, ou nettoyage en double) : rien à faire.
            Err(e) if self.sftp_file_size(id, path).await.is_err() => {
                tracing::debug!(server = id, "archive temporaire déjà absente : {e}");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Attend la fin d'un job abandonné en route (annulation, suivi perdu, trop long) puis retire
    /// son archive temporaire. Borné ; journalise ce qui reste.
    async fn cleanup_temp_after(&self, id: i64, job: CompressJob, archive: &str) {
        let end = if job.is_terminal() {
            Ok(JobEnd::Done(job))
        } else {
            match self.token() {
                Ok(token) => {
                    let token = token.as_str();
                    follow_job(
                        job,
                        |job_id| self.api.compress_job(token, id, job_id),
                        || tokio::time::sleep(POLL_RETRY_PAUSE),
                        &mut |_| {},
                        &AtomicBool::new(false),
                        CLEANUP_MAX_POLLS,
                    )
                    .await
                }
                Err(e) => Err(e),
            }
        };
        match end {
            Ok(JobEnd::Done(_) | JobEnd::Failed(_)) => {
                if let Err(e) = self.remove_temp_archive(id, archive).await {
                    tracing::warn!(server = id, archive, "archive temporaire non supprimée : {e}");
                }
            }
            _ => tracing::warn!(server = id, archive, "compression toujours en cours : archive temporaire laissée sur le serveur"),
        }
    }

    // --- Compression : archive conservée ------------------------------------------

    /// Compresse une sélection (chemins SFTP d'un même dossier) **sur le serveur** : l'archive
    /// `.zip` est créée dans ce dossier et conservée (sauvegarde rapide d'un monde, par exemple).
    /// `name` vide ou absent : nom par défaut du panel (`world_2026-11-18_15h04.zip`). Suivi
    /// publié par `SftpProgress` (`direction = server`, corrélé par `tid`), annulable par
    /// [`Self::cancel_transfer`] — seul le suivi s'arrête : la compression continue sur le serveur.
    pub async fn server_compress(
        &self,
        id: i64,
        paths: Vec<String>,
        name: Option<String>,
        tid: &str,
    ) -> Result<CompressOutcome> {
        let name = name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| default_archive_name(&paths, now_local()));
        let label = archive_name(&name).unwrap_or_else(|_| name.clone());
        let res = match compress_request(&paths, &name) {
            Ok(req) => {
                let cancel = self.register_cancel(tid);
                let launch = self.launch_compression(id, &req, tid, &label, "server", &cancel).await;
                self.unregister_cancel(tid);
                self.compress_outcome(id, &req, launch).await
            }
            Err(e) => Err(e),
        };
        match &res {
            Ok(out) => self.emit_end(
                tid,
                &label,
                "server",
                Ok((out.skipped, out.still_running.then_some("still_running"))),
            ),
            Err(e) => self.emit_end(tid, &label, "server", Err(e)),
        }
        res
    }

    async fn compress_outcome(&self, id: i64, req: &CompressRequest, launch: Launch) -> Result<CompressOutcome> {
        let outcome = |job: &CompressJob, still_running: bool| CompressOutcome {
            archive_path: resolve_archive_path(&req.archive_path, job),
            still_running,
            file_count: job.file_count,
            size: 0,
            skipped: job.skipped_count,
        };
        match launch {
            Launch::Refused(e) => Err(e),
            Launch::Followed(JobEnd::Done(job)) => {
                let mut out = outcome(&job, false);
                out.size = self.sftp_file_size(id, &out.archive_path).await.unwrap_or(0);
                Ok(out)
            }
            Launch::Followed(JobEnd::Failed(job)) => Err(Error::Unexpected(format!(
                "Le serveur n'a pas pu créer l'archive{}.",
                job.error.map(|r| format!(" : {r}")).unwrap_or_default()
            ))),
            Launch::Followed(JobEnd::StillRunning(job)) => Ok(outcome(&job, true)),
            Launch::Followed(JobEnd::Cancelled(_)) => Err(Error::Unexpected(
                "Suivi interrompu : la compression continue sur le serveur.".into(),
            )),
            Launch::Lost(_, e) => Err(Error::Unexpected(format!(
                "Suivi de la compression perdu ({e}) : elle continue peut-être sur le serveur."
            ))),
        }
    }

    /// Compresse une sélection sur le serveur dans une archive **temporaire**, pour un
    /// téléchargement piloté ailleurs (mobile) : l'appelant la rapatrie puis la retire avec
    /// [`Self::remove_temp_archive`]. Un suivi abandonné (annulation, trop long, perdu) attend la
    /// fin du job pour retirer l'archive, après avoir publié l'échec.
    pub async fn server_compress_temp(&self, id: i64, paths: Vec<String>, tid: &str) -> Result<CompressOutcome> {
        let name = temp_archive_name(&paths, now_local());
        // SFTP vérifié d'abord : l'archive n'aurait servi à rien sans lui (voir `download_zip`).
        let ready = async { self.sftp.ensure(&self.api, &self.token()?, id).await.map(|_| ()) }.await;
        let req = match ready.and_then(|()| compress_request(&paths, &name)) {
            Ok(req) => req,
            Err(e) => {
                self.emit_end(tid, &name, "server", Err(&e));
                return Err(e);
            }
        };
        let cancel = self.register_cancel(tid);
        let launch = self.launch_compression(id, &req, tid, &name, "server", &cancel).await;
        self.unregister_cancel(tid);
        if let Launch::Followed(JobEnd::Done(job)) = launch {
            let res = self.compress_outcome(id, &req, Launch::Followed(JobEnd::Done(job))).await;
            match &res {
                Ok(out) => self.emit_end(tid, &name, "server", Ok((out.skipped, None))),
                Err(e) => self.emit_end(tid, &name, "server", Err(e)),
            }
            return res;
        }
        let (err, pending) = abandoned(launch);
        self.emit_end(tid, &name, "server", Err(&err));
        if let Some(job) = pending {
            self.cleanup_temp_after(id, job, &req.archive_path).await;
        }
        Err(err)
    }

    // --- Téléchargement groupé en .zip ----------------------------------------------

    /// Télécharge une sélection (fichiers et/ou dossiers d'un même dossier) en UN `.zip` local.
    ///
    /// L'archive est construite **par le serveur** (archive temporaire), rapatriée par SFTP avec
    /// reprise, puis supprimée du serveur. Si l'API ne peut pas compresser (endpoint indisponible,
    /// refus, échec du job), repli sur la construction par l'appareil (SFTP récursif, lent pour un
    /// gros monde mais sans dépendance à l'API). Suivi par `SftpProgress` (`direction = down`,
    /// phases `compressing` → `downloading` → `cleanup`, ou `local_zip`), annulable.
    pub async fn run_sftp_download_zip(&self, id: i64, paths: Vec<String>, local_zip: &str, tid: &str) {
        let name = base_name(local_zip).to_string();
        self.emit_progress(crate::SftpProgress {
            id: tid.to_string(),
            name: name.clone(),
            direction: "down".into(),
            status: "active".into(),
            ..Default::default()
        });
        let cancel = self.register_cancel(tid);
        let (res, pending) = self.download_zip(id, &paths, local_zip, tid, &name, &cancel).await;
        self.unregister_cancel(tid);
        match &res {
            Ok(skipped) => self.emit_end(tid, &name, "down", Ok((*skipped, None))),
            Err(e) => self.emit_end(tid, &name, "down", Err(e)),
        }
        // Compression abandonnée en route : la fin est publiée d'abord, le nettoyage attend que
        // le serveur ait fini d'écrire l'archive temporaire.
        if let Some((job, archive)) = pending {
            self.cleanup_temp_after(id, job, &archive).await;
        }
    }

    /// Corps de [`Self::run_sftp_download_zip`]. Renvoie aussi le job d'une compression
    /// abandonnée en route, dont l'archive temporaire reste à retirer.
    async fn download_zip(
        &self,
        id: i64,
        paths: &[String],
        local_zip: &str,
        tid: &str,
        name: &str,
        cancel: &AtomicBool,
    ) -> (Result<u64>, Option<(CompressJob, String)>) {
        let local = |reason: &dyn std::fmt::Display| {
            tracing::warn!(server = id, "compression côté serveur indisponible, archive construite par l'appareil : {reason}");
        };
        // Le SFTP d'abord : sans lui, ni téléchargement ni nettoyage possible — inutile de faire
        // créer au serveur une archive de plusieurs Go qu'on ne pourrait ni rapatrier ni retirer.
        if let Err(e) = async { self.sftp.ensure(&self.api, &self.token()?, id).await }.await {
            return (Err(e), None);
        }
        let req = match compress_request(paths, &temp_archive_name(paths, now_local())) {
            Ok(req) => req,
            Err(e) => {
                local(&e);
                return (self.build_zip(id, paths, local_zip, tid, name, cancel).await.map(|_| 0), None);
            }
        };
        let launch = self.launch_compression(id, &req, tid, name, "down", cancel).await;
        let cancelled = cancel.load(Ordering::Relaxed);
        let job = match server_or_local(launch, cancelled) {
            Route::Download(job) => job,
            Route::Local(reason, failed) => {
                local(&reason);
                // Un job échoué a pu laisser une archive incomplète : retirée avant le repli.
                if let Some(job) = failed {
                    let _ = self.remove_temp_archive(id, &resolve_archive_path(&req.archive_path, &job)).await;
                }
                return (self.build_zip(id, paths, local_zip, tid, name, cancel).await.map(|_| 0), None);
            }
            Route::Stop(err, pending) => {
                return (Err(err), pending.map(|job| (job, req.archive_path.clone())));
            }
        };

        let archive = resolve_archive_path(&req.archive_path, &job);
        let tx = self.events.clone();
        let (tid_s, name_s) = (tid.to_string(), name.to_string());
        let mut last = 0u64;
        let mut on = move |done: u64, total: u64| {
            if done == total || done.saturating_sub(last) >= 512 * 1024 {
                last = done;
                let _ = tx.send(crate::CoreEvent::SftpProgress(crate::SftpProgress {
                    id: tid_s.clone(),
                    name: name_s.clone(),
                    direction: "down".into(),
                    done,
                    total,
                    status: "active".into(),
                    phase: Some("downloading".into()),
                    ..Default::default()
                }));
            }
        };
        let downloaded = self.download_with_resume(id, &archive, local_zip, &mut on, cancel).await;
        // L'archive temporaire est retirée quoi qu'il arrive au téléchargement.
        self.emit_progress(crate::SftpProgress {
            id: tid.to_string(),
            name: name.to_string(),
            direction: "down".into(),
            status: "active".into(),
            phase: Some("cleanup".into()),
            ..Default::default()
        });
        if let Err(e) = self.remove_temp_archive(id, &archive).await {
            tracing::warn!(server = id, archive, "archive temporaire non supprimée : {e}");
        }
        (downloaded.map(|_| job.skipped_count), None)
    }

    // --- Extraction ---------------------------------------------------------------

    /// Extrait une archive du serveur DANS `dest_dir`, **par le serveur** (formats du panel :
    /// zip, tar.*, 7z, rar, mcworld…) : rien ne transite par l'appareil. Repli par l'appareil
    /// (SFTP : `.zip`/`.tar`/`.tar.gz`, plafonné) si l'API ne peut pas le faire.
    pub async fn extract_archive(&self, id: i64, archive: &str, dest_dir: &str) -> Result<ExtractOutcome> {
        let body = extract_body(archive, dest_dir)?;
        let local_ok = crate::archive::kind_from_name(archive).is_some();
        if server_extractable(archive) {
            match self.api.extract(&self.token()?, id, &body).await {
                Ok(data) => {
                    let (destination, skipped) = parse_extract_response(data.as_ref(), dest_dir);
                    return Ok(ExtractOutcome { destination, on_server: true, files: None, skipped });
                }
                Err(e) if !local_ok || !fallback_on(&e) => return Err(e),
                Err(e) => tracing::warn!(
                    server = id,
                    "extraction côté serveur indisponible, extraction par l'appareil : {e}"
                ),
            }
        } else if !local_ok {
            return Err(Error::Unexpected("Format d'archive non reconnu.".into()));
        }
        let files = self.sftp_extract_archive(id, archive, dest_dir).await?;
        Ok(ExtractOutcome {
            destination: sftp_path(dest_dir),
            on_server: false,
            files: Some(files as u64),
            skipped: 0,
        })
    }

    /// [`Self::extract_archive`] suivie dans le gestionnaire de transferts (`direction = server`,
    /// phase `extracting`).
    pub async fn run_extract_archive(&self, id: i64, archive: &str, dest_dir: &str, tid: &str) -> Result<ExtractOutcome> {
        let name = base_name(archive).to_string();
        self.emit_progress(crate::SftpProgress {
            id: tid.to_string(),
            name: name.clone(),
            direction: "server".into(),
            status: "active".into(),
            phase: Some("extracting".into()),
            ..Default::default()
        });
        let res = self.extract_archive(id, archive, dest_dir).await;
        match &res {
            Ok(out) => self.emit_end(tid, &name, "server", Ok((out.skipped, None))),
            Err(e) => self.emit_end(tid, &name, "server", Err(e)),
        }
        res
    }

    /// Compressions récentes ou en cours d'un serveur (les plus récentes d'abord) — par exemple
    /// une compression lancée depuis le panel web, qui rendrait la suivante `API_ZIP_BUSY`.
    pub async fn compress_jobs(&self, id: i64) -> Result<Vec<CompressJob>> {
        self.api.compress_jobs(&self.token()?, id, 5).await
    }

    // --- Espace disque -------------------------------------------------------------

    /// Ce qui occupe le plus de place dans `path` (chemin SFTP ; tailles récursives), du plus gros
    /// au plus petit — calculé par le serveur, là où le SFTP devrait tout parcourir.
    pub async fn disk_usage(&self, id: i64, path: &str, limit: u32) -> Result<DiskUsage> {
        self.api.disk_usage(&self.token()?, id, path, limit).await
    }

    /// Occupation du disque d'un serveur face à sa limite, et celle de sa MyBox (disque partagé
    /// par ses serveurs) face au disque de l'offre. Seule l'occupation est indispensable : le
    /// reste est omis s'il est indisponible.
    pub async fn disk_space(&self, id: i64) -> Result<DiskSpace> {
        let token = self.token()?;
        let (total, light, servers) = tokio::join!(
            self.api.disk_total(&token, id),
            self.api.get_live_light(&token, id),
            self.list_servers()
        );
        let mut space = total?;
        if let Ok(light) = light {
            space.limit_bytes = mib_to_bytes(light.disk.limit);
        }
        let mybox = servers
            .ok()
            .and_then(|o| o.servers.into_iter().find(|s| s.id == id))
            .map(|s| s.mybox_id)
            .filter(|m| *m > 0);
        if let Some(mybox) = mybox {
            let (used, details) = tokio::join!(self.api.mybox_disk(&token, mybox), self.mybox_details(mybox));
            if let Ok(used_bytes) = used {
                space.mybox = Some(MyBoxDiskSpace {
                    used_bytes,
                    limit_bytes: details.map(|d| gib_to_bytes(d.offer.disk)).unwrap_or(0),
                });
            }
        }
        Ok(space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    fn when() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 18)
            .unwrap()
            .and_hms_opt(15, 4, 9)
            .unwrap()
    }

    fn paths(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chemins_sftp_et_panel_se_convertissent() {
        assert_eq!(panel_path("/world/region/"), "world/region");
        assert_eq!(panel_path("/"), "");
        assert_eq!(panel_path(""), "");
        assert_eq!(panel_path("//a//./b"), "a/b");
        assert_eq!(panel_path("\\a\\b"), "a/b");
        assert_eq!(sftp_path("world"), "/world");
        assert_eq!(sftp_path(""), "/");
        assert_eq!(sftp_path("/a/b/"), "/a/b");
    }

    #[test]
    fn corps_de_compression_au_format_du_panel() {
        let req = compress_request(&paths(&["/world", "/world_nether"]), "sauvegarde").unwrap();
        assert_eq!(
            req.body,
            json!({ "path": "", "files": ["world", "world_nether"], "name": "sauvegarde.zip",
                    "cwd": "", "overwrite": false })
        );
        assert_eq!(req.archive_path, "/sauvegarde.zip");
        assert_eq!(req.dir, "/");

        // Sous-dossier : `files` reste relatif à la racine, `path`/`cwd` = dossier de la sélection.
        let req = compress_request(&paths(&["/plugins/Essentials", "/plugins/Essentials"]), "ess.ZIP").unwrap();
        assert_eq!(
            req.body,
            json!({ "path": "plugins", "files": ["plugins/Essentials"], "name": "ess.ZIP",
                    "cwd": "plugins", "overwrite": false })
        );
        assert_eq!(req.archive_path, "/plugins/ess.ZIP");
    }

    #[test]
    fn selection_invalide_refusee_avant_tout_appel() {
        assert!(compress_request(&[], "a").is_err());
        assert!(compress_request(&paths(&["/"]), "a").is_err(), "racine entière");
        assert!(compress_request(&paths(&["/a/x", "/b/y"]), "a").is_err(), "deux dossiers");
        assert!(compress_request(&paths(&["/a/../etc"]), "a").is_err(), "remontée");
        assert!(compress_request(&paths(&["/world"]), "a/b").is_err(), "nom avec /");
        assert!(compress_request(&paths(&["/world"]), "  ").is_err(), "nom vide");
        assert!(compress_request(&paths(&["/world"]), ".zip").is_err(), "nom vide");
        assert!(compress_request(&paths(&["/world"]), "a*?").is_err());
        assert!(archive_name(&"x".repeat(300)).is_err());
        assert_eq!(archive_name(" monde ").unwrap(), "monde.zip");
    }

    #[test]
    fn noms_d_archive_par_defaut_et_temporaire() {
        assert_eq!(default_archive_name(&paths(&["/world"]), when()), "world_2026-11-18_15h04.zip");
        assert_eq!(
            default_archive_name(&paths(&["/plugins/config.yml"]), when()),
            "config_2026-11-18_15h04.zip"
        );
        assert_eq!(
            default_archive_name(&paths(&["/plugins/a", "/plugins/b"]), when()),
            "plugins_2026-11-18_15h04.zip"
        );
        assert_eq!(default_archive_name(&paths(&["/a", "/b"]), when()), "archive_2026-11-18_15h04.zip");
        assert_eq!(default_archive_name(&paths(&["/.env"]), when()), ".env_2026-11-18_15h04.zip");

        let tmp = temp_archive_name(&paths(&["/world"]), when());
        assert_eq!(tmp, "minestrator-temp-world-20261118-150409.zip");
        assert!(tmp.starts_with(TEMP_ARCHIVE_PREFIX));
        // Un nom temporaire est toujours accepté par l'API (caractères interdits remplacés).
        let odd = temp_archive_name(&paths(&["/dir/we:ird*name"]), when());
        assert!(archive_name(&odd).is_ok(), "{odd}");
        assert!(odd.contains("we_ird_name"));
    }

    #[test]
    fn job_lu_avec_tolerance() {
        let v = json!({
            "id": 42, "status": "RUNNING", "archive_path": "/home/container/w.zip",
            "progress": { "queued": 0, "file_count": "12", "total_size": 2048.0, "elapsed_s": 3 },
            "file_count": 1, "total_bytes": 1
        });
        let job = CompressJob::from_value(&v).unwrap();
        assert_eq!((job.id, job.status.as_str()), (42, "running"));
        assert_eq!((job.file_count, job.total_bytes, job.elapsed_s), (12, 2048, 3), "progress prime");
        assert!(!job.queued && job.active() && !job.is_terminal());

        let done = CompressJob::from_value(&json!({
            "id": "7", "status": "completed", "partial": 1, "skipped_count": 2, "file_count": 5, "total_bytes": 10
        }))
        .unwrap();
        assert!(done.succeeded() && done.partial);
        assert_eq!((done.file_count, done.total_bytes, done.skipped_count), (5, 10, 2));

        let failed = CompressJob::from_value(&json!({ "id": 3, "status": "failed", "error_code": "DISK_FULL" })).unwrap();
        assert!(failed.failed() && failed.is_terminal());
        assert_eq!(failed.error.as_deref(), Some("DISK_FULL"));
        let failed = CompressJob::from_value(&json!({ "id": 3, "status": "failed", "error_message": "plus de place", "error_code": "X" })).unwrap();
        assert_eq!(failed.error.as_deref(), Some("plus de place"));

        assert!(CompressJob::from_value(&json!({ "status": "running" })).is_none(), "sans id");
        assert!(CompressJob::from_value(&json!({ "id": 0 })).is_none());
        assert_eq!(
            parse_jobs(&json!({ "jobs": [{ "id": 1, "status": "pending" }, { "x": 1 }], "count": 2 })).len(),
            1
        );
        assert!(parse_jobs(&json!({ "jobs": [], "count": 0 })).is_empty());
    }

    #[test]
    fn chemin_de_l_archive_reste_dans_le_dossier_attendu() {
        let mut job = CompressJob { id: 1, ..Default::default() };
        assert_eq!(resolve_archive_path("/plugins/a.zip", &job), "/plugins/a.zip");
        job.archive_path = "/home/container/plugins/a.zip".into();
        assert_eq!(resolve_archive_path("/plugins/a.zip", &job), "/plugins/a.zip");
        job.archive_path = "plugins/a (1).zip".into();
        assert_eq!(resolve_archive_path("/plugins/a.zip", &job), "/plugins/a (1).zip");
        job.archive_path = "/a.zip".into();
        assert_eq!(resolve_archive_path("/a.zip", &job), "/a.zip");
    }

    /// Exécute `follow_job` sur un script de réponses, sans réseau ni attente réelle.
    fn run_follow(first: CompressJob, script: Vec<Result<CompressJob>>, cancel_after: Option<usize>) -> (Result<JobEnd>, usize, Vec<String>, usize) {
        let script = RefCell::new(VecDeque::from(script));
        let polls = RefCell::new(0usize);
        let pauses = RefCell::new(0usize);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let mut on = |j: &CompressJob| seen.push(j.status.clone());
        let fut = follow_job(
            first,
            |_id| {
                *polls.borrow_mut() += 1;
                if cancel_after.is_some_and(|n| *polls.borrow() >= n) {
                    cancel.store(true, Ordering::Relaxed);
                }
                let next = script.borrow_mut().pop_front().unwrap_or_else(|| Ok(job("running")));
                async move { next }
            },
            || {
                *pauses.borrow_mut() += 1;
                async {}
            },
            &mut on,
            &cancel,
            JOB_MAX_POLLS,
        );
        let res = futures_util::FutureExt::now_or_never(fut).expect("aucune attente réelle");
        let polls = *polls.borrow();
        let pauses = *pauses.borrow();
        (res, polls, seen, pauses)
    }

    fn job(status: &str) -> CompressJob {
        CompressJob { id: 9, status: status.into(), ..Default::default() }
    }

    #[test]
    fn suivi_de_job_jusqu_a_la_fin() {
        let (res, polls, seen, _) = run_follow(
            job("pending"),
            vec![Ok(job("running")), Ok(job("running")), Ok(job("completed"))],
            None,
        );
        assert_eq!(res.unwrap(), JobEnd::Done(job("completed")));
        assert_eq!(polls, 3);
        assert_eq!(seen, ["pending", "running", "running", "completed"], "chaque état est publié");

        // Déjà terminé à la création : aucun appel de suivi.
        let (res, polls, _, _) = run_follow(job("completed"), vec![], None);
        assert!(matches!(res.unwrap(), JobEnd::Done(_)));
        assert_eq!(polls, 0);

        let (res, _, _, _) = run_follow(job("running"), vec![Ok(job("failed"))], None);
        assert!(matches!(res.unwrap(), JobEnd::Failed(_)));
    }

    #[test]
    fn suivi_tolere_les_coupures_passageres() {
        let (res, polls, _, pauses) = run_follow(
            job("running"),
            vec![Err(Error::Network("coupure".into())), Err(Error::RateLimited), Ok(job("completed"))],
            None,
        );
        assert!(matches!(res.unwrap(), JobEnd::Done(_)));
        assert_eq!((polls, pauses), (3, 2), "une pause après chaque appel raté");

        // Trop d'échecs consécutifs : abandon avec la dernière erreur.
        let errs = (0..JOB_MAX_POLL_ERRORS).map(|_| Err(Error::Network("x".into()))).collect();
        let (res, polls, _, _) = run_follow(job("running"), errs, None);
        assert!(matches!(res, Err(Error::Network(_))));
        assert_eq!(polls, JOB_MAX_POLL_ERRORS);

        // Clé refusée : abandon immédiat.
        let (res, polls, _, _) = run_follow(job("running"), vec![Err(Error::Unauthorized)], None);
        assert!(matches!(res, Err(Error::Unauthorized)));
        assert_eq!(polls, 1);
    }

    #[test]
    fn suivi_borne_et_annulable() {
        // Jamais fini : on rend la main au bout du plafond, la compression continue sur le serveur.
        let (res, polls, _, _) = run_follow(job("running"), vec![], None);
        assert!(matches!(res.unwrap(), JobEnd::StillRunning(_)));
        assert_eq!(polls, JOB_MAX_POLLS);

        let (res, polls, _, _) = run_follow(job("running"), vec![], Some(2));
        assert!(matches!(res.unwrap(), JobEnd::Cancelled(_)));
        assert_eq!(polls, 2);
    }

    #[test]
    fn corps_d_extraction_au_format_du_panel() {
        assert_eq!(
            extract_body("/modpacks/pack.zip", "/").unwrap(),
            json!({ "file": "modpacks/pack.zip", "destination": "/" })
        );
        assert_eq!(
            extract_body("/a.tar.gz", "/world/datapacks/").unwrap(),
            json!({ "file": "a.tar.gz", "destination": "/world/datapacks" })
        );
        assert!(extract_body("/", "/").is_err());
        assert!(extract_body("/../x.zip", "/").is_err());
    }

    #[test]
    fn destination_validee_comme_le_panel() {
        assert_eq!(extract_destination("").unwrap(), "");
        assert_eq!(extract_destination("\\mods\\").unwrap(), "mods");
        assert_eq!(extract_destination("/Mon monde (1)/été").unwrap(), "Mon monde (1)/été");
        for bad in ["/a/../b", "/./..", "/con", "/a/LPT1", "/a%2e%2e", "/a\nb", "/a\"b", "/a<b", "/a|b", "/a*b"] {
            assert!(extract_destination(bad).is_err(), "{bad:?}");
        }
        assert!(extract_destination(&format!("/{}", "a".repeat(301))).is_err());
        assert!(extract_destination(&format!("/{}", "é".repeat(128))).is_err(), "segment > 255 octets");
    }

    #[test]
    fn reponse_d_extraction() {
        let d = json!({ "destination": "/world", "skipped": { "symlink": 1, "exists": "2" } });
        assert_eq!(parse_extract_response(Some(&d), "/x"), ("/world".to_string(), 3));
        let d = json!({ "skipped": 4 });
        assert_eq!(parse_extract_response(Some(&d), "/x/"), ("/x".to_string(), 4));
        assert_eq!(parse_extract_response(None, ""), ("/".to_string(), 0));
    }

    #[test]
    fn formats_extractibles_par_le_serveur() {
        for ok in ["a.zip", "A.TAR.GZ", "b.tar.zst", "c.7z", "d.rar", "e.mcworld", "f.jar", "g.gz", "h.tgz"] {
            assert!(server_extractable(ok), "{ok}");
        }
        for ko in ["a.txt", "zip", "a.zipx", "region.mca"] {
            assert!(!server_extractable(ko), "{ko}");
        }
    }

    #[test]
    fn repli_local_sauf_si_la_cle_est_refusee() {
        assert!(fallback_on(&Error::Api { code: "API_ROUTE_NOT_FOUND".into() }));
        assert!(fallback_on(&Error::Api { code: "API_ZIP_BUSY".into() }));
        assert!(fallback_on(&Error::Unexpected("HTTP 500".into())));
        assert!(fallback_on(&Error::Forbidden));
        assert!(fallback_on(&Error::RateLimited));
        assert!(fallback_on(&Error::Network("délai".into())));
        assert!(!fallback_on(&Error::Unauthorized));
        assert!(!fallback_on(&Error::NoKey));
        assert!(!fallback_on(&Error::SftpAuth));
        assert!(compress_error_message("API_ZIP_EXISTS").is_some());
        assert!(compress_error_message("AUTRE").is_none());
    }

    #[test]
    fn telechargement_groupe_serveur_ou_repli_local() {
        let done = || Launch::Followed(JobEnd::Done(job("completed")));
        assert!(matches!(server_or_local(done(), false), Route::Download(j) if j.id == 9));

        // L'API ne sait pas (endpoint retiré, occupé, refus) : repli local, rien à nettoyer.
        let r = server_or_local(Launch::Refused(Error::Api { code: "API_ROUTE_NOT_FOUND".into() }), false);
        assert!(matches!(r, Route::Local(ref why, None) if why.contains("API_ROUTE_NOT_FOUND")), "{r:?}");
        assert!(matches!(server_or_local(Launch::Refused(Error::Network("x".into())), false), Route::Local(_, None)));
        // Le job a échoué : repli local, et son archive incomplète est à retirer.
        let mut failed = job("failed");
        failed.error = Some("disque plein".into());
        assert!(matches!(
            server_or_local(Launch::Followed(JobEnd::Failed(failed)), false),
            Route::Local(ref why, Some(_)) if why == "disque plein"
        ));

        // Clé refusée : le SFTP échouerait aussi, pas de repli.
        assert!(matches!(server_or_local(Launch::Refused(Error::Unauthorized), false), Route::Stop(Error::Unauthorized, None)));
        // Trop long, suivi perdu, annulé : arrêt, l'archive sera retirée quand le job finira.
        for launch in [
            Launch::Followed(JobEnd::StillRunning(job("running"))),
            Launch::Followed(JobEnd::Cancelled(job("running"))),
            Launch::Lost(job("running"), Error::Network("coupure".into())),
        ] {
            assert!(matches!(server_or_local(launch, false), Route::Stop(_, Some(_))));
        }
        // Une annulation l'emporte, même sur une archive prête ou un échec : ni téléchargement
        // ni repli, mais l'archive est retirée.
        assert!(matches!(server_or_local(done(), true), Route::Stop(_, Some(j)) if j.succeeded()));
        assert!(matches!(server_or_local(Launch::Followed(JobEnd::Failed(job("failed"))), true), Route::Stop(_, Some(_))));
        assert!(matches!(server_or_local(Launch::Refused(Error::Network("x".into())), true), Route::Stop(_, None)));
    }

    #[test]
    fn seules_les_archives_temporaires_sont_supprimables() {
        let tmp = temp_archive_name(&paths(&["/world"]), when());
        assert!(is_temp_archive(&format!("/{tmp}")));
        assert!(is_temp_archive(&format!("/plugins/{tmp}")));
        assert!(!is_temp_archive("/world.zip"));
        assert!(!is_temp_archive("/minestrator-temp-x.tar.gz"));
        assert!(!is_temp_archive("/minestrator-temp-x.zip/../world"));
        assert!(!is_temp_archive("/a/../minestrator-temp-x.zip"));
        assert!(!is_temp_archive("/backup-minestrator-temp-x.zip"));
    }

    #[test]
    fn rapport_texte_de_l_espace_disque() {
        assert_eq!(human_bytes(0), "0 o");
        assert_eq!(human_bytes(1536), "1.5 Kio");
        assert_eq!(human_bytes(300 * 1024 * MIB), "300 Gio");
        assert_eq!(human_bytes(2984175068), "2.8 Gio");

        let usage = DiskUsage {
            path: "/".into(),
            total_size: 4 * MIB,
            items: vec![
                DiskUsageItem { path: "/world".into(), name: "world".into(), is_dir: true, size: 3 * MIB },
                DiskUsageItem { path: "/server.jar".into(), name: "server.jar".into(), is_dir: false, size: MIB },
            ],
        };
        let space = DiskSpace {
            used_bytes: 2 * 1024 * MIB,
            raw_bytes: 2 * 1024 * MIB + 500 * MIB,
            base_game_bytes: 500 * MIB,
            allowance_applied: true,
            limit_bytes: 8 * 1024 * MIB,
            mybox: Some(MyBoxDiskSpace { used_bytes: 3 * 1024 * MIB, limit_bytes: 300 * 1024 * MIB }),
        };
        let r = disk_usage_report(&usage, Some(&space));
        assert!(r.starts_with("Dossier « / » : 4.0 Mio au total."), "{r}");
        assert!(r.contains("Serveur : 2.0 Gio comptés sur 8.0 Gio autorisés (25 %)"), "{r}");
        assert!(r.contains("dont 500 Mio de fichiers du jeu non comptés"), "{r}");
        assert!(r.contains("MyBox (disque partagé par ses serveurs) : 3.0 Gio sur 300 Gio (1 %)."), "{r}");
        assert!(r.contains("\n- /world/ — 3.0 Mio (75 %)\n- /server.jar — 1.0 Mio (25 %)"), "{r}");

        let empty = DiskUsage { path: "/logs".into(), total_size: 0, items: vec![] };
        assert_eq!(disk_usage_report(&empty, None), "Dossier « /logs » : 0 o au total.\n(dossier vide)");
    }

    #[test]
    fn phase_de_compression() {
        let mut j = job("pending");
        j.queued = true;
        assert_eq!(compress_phase(&j), "queued");
        j.queued = false;
        assert_eq!(compress_phase(&j), "compressing");
    }

    #[test]
    fn analyse_du_disque_lue_et_triee() {
        let data = json!({
            "analyzed_path": "", "total_size": 3000, "total_size_human": "3 KB", "items_count": 3,
            "items": [
                { "path": "logs", "name": "logs", "type": "folder", "size": 500, "size_human": "" },
                { "path": "world", "name": "world", "type": "folder", "size": 2000, "size_human": "" },
                { "path": "server.jar", "name": "server.jar", "type": "file", "size": "400" },
                { "path": "sans-nom" }
            ]
        });
        let u = parse_disk_usage(&data, "/");
        assert_eq!(u.path, "/");
        assert_eq!(u.total_size, 3000);
        assert_eq!(u.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["world", "logs", "server.jar"]);
        assert_eq!(u.items[0], DiskUsageItem { path: "/world".into(), name: "world".into(), is_dir: true, size: 2000 });
        assert!(!u.items[2].is_dir);

        // Sous-dossier : `analyzed_path` absolu, chemins des entrées relatifs à la racine.
        let data = json!({ "analyzed_path": "/world", "total_size": 2000,
                           "items": [{ "path": "world/region", "name": "region", "type": "folder", "size": 1500 }] });
        let u = parse_disk_usage(&data, "/world");
        assert_eq!((u.path.as_str(), u.items[0].path.as_str()), ("/world", "/world/region"));

        // Réponse minimale : total recalculé, dossier demandé repris.
        let u = parse_disk_usage(&json!({ "items": [{ "name": "a", "size": 5 }] }), "/x/");
        assert_eq!((u.path.as_str(), u.total_size, u.items[0].path.as_str()), ("/x", 5, "/x/a"));
    }

    #[test]
    fn parametres_de_l_analyse_du_disque() {
        assert_eq!(disk_usage_query("/", 0), vec![("limit", "20".to_string())]);
        assert_eq!(
            disk_usage_query("/world/region/", 500),
            vec![("limit", "100".to_string()), ("path", "world/region".to_string())]
        );
    }

    #[test]
    fn espace_disque_et_unites() {
        let d = parse_disk_total(&json!({
            "total_bytes": 2984175068u64, "raw_bytes": 3508463068u64, "base_game_bytes": 524288000,
            "overhead_applied": true, "total_human": "2.78 GB"
        }));
        assert_eq!((d.used_bytes, d.raw_bytes, d.base_game_bytes), (2984175068, 3508463068, 524288000));
        assert!(d.allowance_applied);
        let d = parse_disk_total(&json!({ "raw_bytes": 10 }));
        assert_eq!(d.used_bytes, 10, "repli sur le brut");

        assert_eq!(parse_mybox_disk(&json!({ "disk_servers": 3768, "disk_servers_unit": "MB" })), 3768 * MIB);
        assert_eq!(parse_mybox_disk(&json!({ "disk_servers": 2, "disk_servers_unit": "GB" })), 2048 * MIB);
        assert_eq!(parse_mybox_disk(&json!({})), 0);
        assert_eq!(mib_to_bytes(8500), 8500 * MIB);
        assert_eq!(mib_to_bytes(-1), 0);
        assert_eq!(gib_to_bytes(300), 300 * 1024 * MIB);
    }
}
