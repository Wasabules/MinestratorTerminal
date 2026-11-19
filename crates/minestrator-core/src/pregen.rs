//! Outil MCP `pregenerate_world` : prégénération d'un monde Minecraft avec **Chunky** (plugin
//! Paper/Spigot/Folia, mod Fabric/Forge/NeoForge).
//!
//! Prégénérer un monde avant l'ouverture évite le lag de génération quand les joueurs explorent.
//! L'outil enchaîne ce qu'un agent ferait à la main, en vérifiant chaque réponse :
//!
//! - **détection** de Chunky dans le contenu installé (plugins ou mods selon la plateforme) ;
//!   absent : erreur qui explique `install_chunky=true`, qui l'installe depuis Modrinth (version
//!   compatible avec la plateforme et la version du jeu) par `install_mod`, passé par la garde des
//!   agents (`guarded_write`) : l'installation est une étape annulable ;
//! - **estimation du disque** avant `start` : nombre de chunks d'après la forme et le rayon, fois
//!   un ordre de grandeur par chunk ([`CHUNK_BYTES`]), comparé à l'espace libre du serveur et de
//!   la MyBox. Avertissement si cela peut manquer, refus seulement si même l'estimation basse
//!   dépasse l'espace libre ;
//! - **commandes console** par la boucle « agir puis vérifier » (`agent_loop.rs` :
//!   `run_command`, envoi par REST, réponse lue dans le tampon console par curseur) : sélection
//!   (`chunky world/shape/center|spawn/radius`) puis `chunky start`, `pause`, `continue`, `cancel`
//!   ou `progress`. La demande de confirmation de Chunky (tâche déjà existante) n'est acceptée
//!   (`chunky confirm`) qu'avec `replace_existing=true` pour `start` ; elle l'est toujours pour
//!   `cancel`, demandé explicitement ;
//! - **suivi** : les lignes de progression de Chunky sont analysées (pourcentage, chunks traités,
//!   vitesse, ETA, position courante) ; avec `wait_secs`, l'outil suit la console (curseur) en
//!   signalant l'avancement au client MCP, jusqu'à la fin de la tâche, l'échéance ou l'annulation.
//!
//! **Formats.** Progression et fin de tâche sont **relevées** dans les journaux d'un serveur réel
//! (Chunky 1.5.66 sur Paper, lecture seule des journaux) ; noter que Chunky y nomme le monde par
//! son identifiant (`minecraft:overworld`) même sur Paper, où on le sélectionne par son dossier
//! (`world`) — d'où [`same_world`] :
//!
//! ```text
//! [Chunky] Task running for minecraft:overworld. Processed: 9 chunks (0.83%), ETA: 0:00:13, Rate: 78.3 cps, Current: -11550, -45041
//! [Chunky] Task finished for minecraft:overworld. Processed: 1089 chunks (100.00%), Total time: 0:00:44
//! ```
//!
//! Relevées aussi en réel (Chunky 1.5.66, Paper 26.1.2, essai du 2026-11-19) : `Task started in
//! minecraft:overworld for the square region centered at 0, 0 with radius 200.`, `No tasks
//! running.`, et les réponses de sélection (`World changed to minecraft:overworld.`, `Shape
//! changed to square.`, `Center changed to 0, 0.`, `Radius changed to 200.`), qui ne sont pas
//! analysées, seulement attendues (une ligne `[Chunky]` qui n'est pas une progression). Relevés
//! aussi : `Task paused for minecraft:overworld.`, `No tasks to continue.`, la demande de
//! confirmation de `chunky cancel` (`… type '/chunky confirm'.`), puis `Cancelling all tasks.` et
//! `Task stopped for minecraft:overworld.`. Encore **supposés**, d'après les textes anglais de
//! Chunky : `Task continuing for <monde>.` et `Task already started for <monde>.`. L'ancienne
//! forme `Task started for <monde> at <x>, <z> with radius <r>.` reste reconnue.
//!
//! L'analyse est tolérante (casse, séparateurs de milliers, virgule décimale, préfixe de journal) ;
//! une réponse non reconnue est rendue telle quelle avec une note, jamais interprétée au hasard.
//!
//! Annulation (garde des agents) : `Plan::Fixed` — les chunks générés restent sur le disque.

use crate::agent_loop::normalize_state;
use crate::mcp_ctx;
use crate::models::{InstalledItem, MarketVersion};
use crate::Core;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Projet Modrinth de Chunky (plugin et mod dans le même projet, une version par plateforme).
const CHUNKY_SLUG: &str = "chunky";
/// Rayon maximal accepté (blocs) : au-delà, la demande est presque sûrement une erreur.
const MAX_RADIUS: u64 = 1_000_000;
/// Suivi de la progression (`wait_secs`) : maximum en secondes (0 par défaut : pas de suivi).
const MAX_WAIT_SECS: f64 = 600.0;
/// Attente de la réponse d'une commande de sélection, de `start` et des autres actions.
const SETTING_TIMEOUT: f64 = 8.0;
const ACTION_TIMEOUT: f64 = 10.0;
/// Lecture de console par appel de suivi (`tail_console`, max 60 s).
const FOLLOW_SLICE_SECS: f64 = 30.0;

/// Octets par chunk généré, **ordre de grandeur** (bas, typique, haut) : fichiers de région
/// compressés d'un monde 1.18+ (384 blocs de haut), entités et points d'intérêt compris. Les
/// mondes réels varient avec le générateur, les biomes et les mods ; les dimensions autres que
/// l'overworld pèsent moins. Sert à prévenir, pas à garantir.
pub(crate) const CHUNK_BYTES: (u64, u64, u64) = (6 * 1024, 12 * 1024, 20 * 1024);

// --- Arguments ------------------------------------------------------------------------------

/// Action demandée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Start,
    Status,
    Pause,
    Continue,
    Cancel,
}

impl Action {
    fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "start" => Ok(Action::Start),
            "status" | "progress" => Ok(Action::Status),
            "pause" => Ok(Action::Pause),
            "continue" | "resume" => Ok(Action::Continue),
            "cancel" | "stop" => Ok(Action::Cancel),
            other => Err(format!(
                "`action` inconnue : « {other} » (start, status, pause, continue ou cancel)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Status => "status",
            Action::Pause => "pause",
            Action::Continue => "continue",
            Action::Cancel => "cancel",
        }
    }
}

/// Forme de la zone (formes de Chunky à un seul rayon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Square,
    Circle,
    Diamond,
    Triangle,
    Pentagon,
    Hexagon,
    Star,
}

impl Shape {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "" | "square" | "carré" | "carre" => Shape::Square,
            "circle" | "cercle" => Shape::Circle,
            "diamond" | "losange" => Shape::Diamond,
            "triangle" => Shape::Triangle,
            "pentagon" => Shape::Pentagon,
            "hexagon" => Shape::Hexagon,
            "star" => Shape::Star,
            other => {
                return Err(format!(
                "`shape` non prise en charge : « {other} » (square, circle, diamond, triangle, pentagon, hexagon ou star ; rectangle et ellipse demandent deux rayons)"
            ))
            }
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Shape::Square => "square",
            Shape::Circle => "circle",
            Shape::Diamond => "diamond",
            Shape::Triangle => "triangle",
            Shape::Pentagon => "pentagon",
            Shape::Hexagon => "hexagon",
            Shape::Star => "star",
        }
    }

    /// Aire de la forme pour un rayon 1 (polygones réguliers inscrits dans le cercle du rayon ;
    /// étoile à cinq branches, rayon intérieur ≈ 0,382 × rayon).
    fn unit_area(self) -> f64 {
        match self {
            Shape::Square => 4.0,
            Shape::Circle => std::f64::consts::PI,
            Shape::Diamond => 2.0,
            Shape::Triangle => 1.299,
            Shape::Pentagon => 2.378,
            Shape::Hexagon => 2.598,
            Shape::Star => 1.123,
        }
    }
}

/// Centre de la zone : point d'apparition du monde, ou coordonnées en blocs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Center {
    Spawn,
    At(i64, i64),
}

impl Center {
    fn parse(v: Option<&Value>) -> Result<Self, String> {
        const HELP: &str = "`center` : \"spawn\", \"x,z\", [x, z] ou {\"x\": …, \"z\": …} (blocs)";
        let num = |v: &Value| -> Option<i64> {
            match v {
                Value::Number(n) => n
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.round() as i64),
                Value::String(s) => s
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(|f| f.round() as i64),
                _ => None,
            }
        };
        match v {
            None | Some(Value::Null) => Ok(Center::Spawn),
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() || t.eq_ignore_ascii_case("spawn") {
                    return Ok(Center::Spawn);
                }
                let parts: Vec<&str> = t
                    .split([',', ' ', ';'])
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .collect();
                match parts.as_slice() {
                    [x, z] => match (num(&json!(x)), num(&json!(z))) {
                        (Some(x), Some(z)) => Ok(Center::At(x, z)),
                        _ => Err(HELP.into()),
                    },
                    _ => Err(HELP.into()),
                }
            }
            Some(Value::Array(a)) => match a.as_slice() {
                [x, z] => match (num(x), num(z)) {
                    (Some(x), Some(z)) => Ok(Center::At(x, z)),
                    _ => Err(HELP.into()),
                },
                _ => Err(HELP.into()),
            },
            Some(Value::Object(m)) => match (m.get("x").and_then(num), m.get("z").and_then(num)) {
                (Some(x), Some(z)) => Ok(Center::At(x, z)),
                _ => Err(HELP.into()),
            },
            Some(_) => Err(HELP.into()),
        }
    }

    fn label(self) -> String {
        match self {
            Center::Spawn => "spawn".into(),
            Center::At(x, z) => format!("{x}, {z}"),
        }
    }
}

/// Arguments validés (sans réseau).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Request {
    pub server_id: i64,
    pub action: Action,
    pub radius: Option<u64>,
    pub world: Option<String>,
    pub center: Center,
    pub shape: Shape,
    pub install_chunky: bool,
    pub replace_existing: bool,
    pub wait_secs: f64,
}

fn int_arg(args: &Value, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn bool_arg(args: &Value, key: &str) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Nom de monde utilisable dans une commande console (un seul argument).
fn check_world(w: &str) -> Result<String, String> {
    let w = w.trim();
    if w.is_empty() || w.len() > 128 || w.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "`world` invalide : « {w} » (nom de dossier ou de dimension, sans espace)"
        ));
    }
    Ok(w.to_string())
}

impl Request {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        let server_id =
            int_arg(args, "server_id").ok_or("paramètre `server_id` (entier) requis")?;
        let action = Action::parse(
            args.get("action")
                .and_then(Value::as_str)
                .ok_or("paramètre `action` requis : start, status, pause, continue ou cancel")?,
        )?;
        let radius = match args.get("radius") {
            None | Some(Value::Null) => None,
            Some(_) => {
                let r = int_arg(args, "radius").ok_or("`radius` : entier (blocs) attendu")?;
                if r <= 0 {
                    return Err("`radius` doit être positif (blocs depuis le centre)".into());
                }
                let r = r as u64;
                if r > MAX_RADIUS {
                    return Err(format!(
                        "`radius` de {r} blocs refusé (au plus {MAX_RADIUS}) : vérifie l'unité, c'est un rayon en blocs"
                    ));
                }
                Some(r)
            }
        };
        if action == Action::Start && radius.is_none() {
            return Err("`radius` (blocs depuis le centre) requis pour start".into());
        }
        let world = match args.get("world").and_then(Value::as_str) {
            Some(w) if !w.trim().is_empty() => Some(check_world(w)?),
            _ => None,
        };
        let wait_secs = match args.get("wait_secs") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        };
        Ok(Request {
            server_id,
            action,
            radius,
            world,
            center: Center::parse(args.get("center"))?,
            shape: Shape::parse(args.get("shape").and_then(Value::as_str).unwrap_or(""))?,
            install_chunky: bool_arg(args, "install_chunky"),
            replace_existing: bool_arg(args, "replace_existing"),
            wait_secs: if wait_secs.is_finite() {
                wait_secs.clamp(0.0, MAX_WAIT_SECS)
            } else {
                0.0
            },
        })
    }
}

// --- Plateforme et contenu installé -----------------------------------------------------------

/// Chunky en plugin (Bukkit, Paper, Folia, Sponge) ou en mod (Fabric, Quilt, Forge, NeoForge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Plugin,
    Mod,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Plugin => "plugin",
            Kind::Mod => "mod",
        }
    }
}

/// Nature de Chunky pour une plateforme (`paper`, `fabric`…), `None` si Chunky ne s'y installe pas
/// (vanilla, proxy) ou si la plateforme est inconnue.
pub(crate) fn platform_kind(platform: &str) -> Option<Kind> {
    let families = crate::updates::platforms_in(platform);
    if families.contains("bukkit") || families.contains("sponge") {
        Some(Kind::Plugin)
    } else if ["fabric", "quilt", "forge", "neoforge"]
        .iter()
        .any(|f| families.contains(f))
    {
        Some(Kind::Mod)
    } else {
        None
    }
}

/// Loaders Modrinth acceptables pour une plateforme : une version de Chunky en déclare au moins un.
pub(crate) fn accepted_loaders(platform: &str) -> &'static [&'static str] {
    let p = platform.trim().to_ascii_lowercase();
    match p.as_str() {
        "folia" => &["folia"],
        "spigot" | "bukkit" | "craftbukkit" => &["spigot", "bukkit"],
        "fabric" => &["fabric"],
        "quilt" => &["quilt", "fabric"],
        "forge" => &["forge"],
        "neoforge" => &["neoforge"],
        "sponge" | "spongevanilla" | "spongeforge" => &["sponge"],
        _ if crate::updates::platforms_in(&p).contains("bukkit") => {
            &["paper", "purpur", "bukkit", "spigot"]
        }
        _ => &[],
    }
}

/// Version de Chunky à installer : la plus récente (ordre de la marketplace) qui déclare un loader
/// de la plateforme et la version du jeu (si connue), une version stable de préférence ; jamais un
/// fichier qui nomme une autre plateforme.
pub(crate) fn pick_version<'a>(
    versions: &'a [MarketVersion],
    platform: &str,
    game_version: &str,
) -> Option<&'a MarketVersion> {
    let accepted = accepted_loaders(platform);
    let gv = game_version.trim();
    let compatible = |v: &&MarketVersion| {
        let loader_ok = v
            .loaders
            .iter()
            .any(|l| accepted.contains(&l.to_ascii_lowercase().as_str()));
        let game_ok =
            gv.is_empty() || v.game_versions.is_empty() || v.game_versions.iter().any(|g| g == gv);
        let file_ok = v
            .filename
            .as_deref()
            .is_none_or(|f| !crate::updates::platform_conflict("", platform, f));
        loader_ok && game_ok && file_ok
    };
    let release = |v: &&MarketVersion| {
        v.release_type.is_empty() || v.release_type.eq_ignore_ascii_case("release")
    };
    versions
        .iter()
        .filter(compatible)
        .find(release)
        .or_else(|| versions.iter().find(compatible))
}

fn alnum_lower(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// L'élément installé est-il Chunky (et pas un module complémentaire comme ChunkyBorder) ?
pub(crate) fn is_chunky(name: &str, file: &str) -> bool {
    if alnum_lower(name) == "chunky" {
        return true;
    }
    let f = file.trim().to_ascii_lowercase();
    let stem = f.trim_end_matches(".disabled").trim_end_matches(".jar");
    stem.split(['-', '_', ' ', '+']).next() == Some("chunky")
}

/// Nom de monde pour Chunky. Plugin : nom de dossier (`level-name` par défaut, `<monde>_nether`,
/// `<monde>_the_end`). Mod : identifiant de dimension (`minecraft:overworld` par défaut). Les
/// alias `overworld`, `nether`, `end` et les identifiants de dimension sont traduits dans les deux
/// sens ; tout autre nom est passé tel quel.
pub(crate) fn chunky_world(world: Option<&str>, level_name: &str, kind: Kind) -> String {
    let level = level_or_default(level_name);
    let w = world.map(str::trim).filter(|w| !w.is_empty());
    let dim = match w {
        None => Some(0),
        Some(w) => dimension(w, level),
    };
    match (dim, kind) {
        (Some(0), Kind::Plugin) => level.to_string(),
        (Some(1), Kind::Plugin) => format!("{level}_nether"),
        (Some(2), Kind::Plugin) => format!("{level}_the_end"),
        (Some(0), Kind::Mod) => "minecraft:overworld".into(),
        (Some(1), Kind::Mod) => "minecraft:the_nether".into(),
        (Some(2), Kind::Mod) => "minecraft:the_end".into(),
        _ => w.unwrap_or(level).to_string(),
    }
}

fn level_or_default(level_name: &str) -> &str {
    match level_name.trim() {
        "" => "world",
        l => l,
    }
}

/// Dimension vanilla désignée par un nom (0 : overworld, 1 : Nether, 2 : End) : alias, identifiant
/// (`minecraft:the_nether`) ou dossier (`<level-name>_nether`). `None` : autre monde.
fn dimension(name: &str, level: &str) -> Option<u8> {
    let n = name.trim().to_ascii_lowercase();
    let level = level.to_ascii_lowercase();
    match n.as_str() {
        "overworld" | "minecraft:overworld" => Some(0),
        "nether" | "the_nether" | "minecraft:the_nether" => Some(1),
        "end" | "the_end" | "minecraft:the_end" => Some(2),
        l if l == level => Some(0),
        l if l == format!("{level}_nether") => Some(1),
        l if l == format!("{level}_the_end") => Some(2),
        _ => None,
    }
}

/// Deux noms désignent-ils le même monde ? Chunky 1.5 nomme les mondes par leur identifiant
/// (`minecraft:overworld`) dans ses messages, même sur Paper où on le sélectionne par son dossier
/// (`world`) : relevé sur un serveur réel.
pub(crate) fn same_world(a: &str, b: &str, level_name: &str) -> bool {
    let level = level_or_default(level_name);
    match (dimension(a, level), dimension(b, level)) {
        (Some(x), Some(y)) => x == y,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

// --- Estimation du disque ---------------------------------------------------------------------

/// Nombre de chunks d'une zone (`radius` en blocs).
pub(crate) fn estimate_chunks(radius: u64, shape: Shape) -> u64 {
    let r = radius as f64 / 16.0;
    (shape.unit_area() * r * r).ceil().max(1.0) as u64
}

/// Verdict de l'estimation face à l'espace libre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiskVerdict {
    /// La place devrait suffire.
    Ok,
    /// Elle pourrait manquer : avertissement, la tâche est lancée.
    Warning,
    /// Clairement insuffisante (même l'estimation basse dépasse) : refus.
    Insufficient,
    /// Espace libre inconnu.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DiskEstimate {
    pub chunks: u64,
    pub low_bytes: u64,
    pub typical_bytes: u64,
    pub high_bytes: u64,
    pub free_bytes: Option<u64>,
    pub verdict: DiskVerdict,
    pub summary: String,
}

/// Espace libre : le plus petit reste entre la limite du serveur et celle de la MyBox (connues).
pub(crate) fn free_bytes(space: &crate::server_files::DiskSpace) -> Option<u64> {
    let server =
        (space.limit_bytes > 0).then(|| space.limit_bytes.saturating_sub(space.used_bytes));
    let mybox = space
        .mybox
        .as_ref()
        .filter(|m| m.limit_bytes > 0)
        .map(|m| m.limit_bytes.saturating_sub(m.used_bytes));
    match (server, mybox) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

pub(crate) fn disk_estimate(chunks: u64, free: Option<u64>) -> DiskEstimate {
    let (lo, ty, hi) = CHUNK_BYTES;
    let (low, typical, high) = (
        chunks.saturating_mul(lo),
        chunks.saturating_mul(ty),
        chunks.saturating_mul(hi),
    );
    let h = crate::server_files::human_bytes;
    let range = format!(
        "≈ {} ({} à {}, ordre de grandeur)",
        h(typical),
        h(low),
        h(high)
    );
    let (verdict, summary) = match free {
        None => (DiskVerdict::Unknown, format!("{range} ; espace libre inconnu : vérifie avec disk_usage")),
        Some(f) if low > f => (
            DiskVerdict::Insufficient,
            format!("{range} ; espace libre {} : même l'estimation basse dépasse, prégénération refusée (réduis `radius` ou libère de la place)", h(f)),
        ),
        Some(f) if high > f || typical.saturating_mul(10) > f.saturating_mul(8) => (
            DiskVerdict::Warning,
            format!("{range} ; espace libre {} : la place pourrait manquer, surveille disk_usage (pause si besoin)", h(f)),
        ),
        Some(f) => (DiskVerdict::Ok, format!("{range} ; espace libre {}", h(f))),
    };
    DiskEstimate {
        chunks,
        low_bytes: low,
        typical_bytes: typical,
        high_bytes: high,
        free_bytes: free,
        verdict,
        summary,
    }
}

// --- Analyse des lignes de Chunky -----------------------------------------------------------------

/// Progression d'une tâche lue dans une ligne de Chunky.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TaskProgress {
    /// Déjà porté par la tâche ([`TaskView`], où la progression est mise à plat).
    #[serde(skip_serializing)]
    pub world: String,
    pub chunks: u64,
    pub percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_cps: Option<f64>,
    /// Chunk en cours (coordonnées de chunk, telles qu'affichées).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<(i64, i64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_time: Option<String>,
}

/// Ce qu'une ligne de console dit de Chunky.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Progress(TaskProgress),
    Finished(TaskProgress),
    Started {
        world: String,
        center: (i64, i64),
        radius: u64,
    },
    /// `paused`, `continuing`, `cancelled` ou `stopped`.
    State {
        world: String,
        state: &'static str,
    },
    AlreadyRunning {
        world: Option<String>,
    },
    ConfirmRequested,
    NoTasks,
    UnknownCommand,
}

const NUM: &str = r"[0-9]+(?:[.,][0-9]+)?";
const COUNT: &str = r"[0-9][0-9.,'\s\x{a0}\x{202f}]*?";

static PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)task running for (?P<world>.+?)\.\s*processed:\s*(?P<chunks>{COUNT})\s*chunks?\s*\(\s*(?P<pct>{NUM})\s*%\s*\)\s*(?:,\s*eta:\s*(?P<eta>[^,]+))?(?:,\s*rate:\s*(?P<rate>{COUNT})\s*cps\s*)?(?:,\s*current:\s*(?P<x>-?[0-9]+)\s*,\s*(?P<z>-?[0-9]+))?"
    ))
    .expect("regex de progression")
});
static DONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)task (?:finished|complete[d]?) for (?P<world>.+?)\.\s*processed:\s*(?P<chunks>{COUNT})\s*chunks?\s*\(\s*(?P<pct>{NUM})\s*%\s*\)\s*(?:,\s*total time:\s*(?P<time>[0-9:dhms ]+[0-9smh]))?"
    ))
    .expect("regex de fin")
});
static STARTED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)task started (?:for|in) (?P<world>\S+?)(?: for the \S+ region)? (?:centered )?at (?P<x>-?{NUM})\s*,\s*(?P<z>-?{NUM}) with radius (?P<r>{NUM})"
    ))
    .expect("regex de démarrage")
});
static STATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)task (?P<what>paused|continuing|continued|resumed|cancell?ed|stopped) for (?P<world>\S+?)\.?\s*$")
        .expect("regex d'état")
});
static ALREADY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)already (?:started|running)(?: for (?P<world>[^\s,!]+?)[.,!]?(?:\s|$))?")
        .expect("regex déjà en cours")
});
static CONFIRM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/?chunky confirm\b").expect("regex de confirmation"));
static NO_TASKS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bno (?:tasks?|task) (?:running|to|in progress|found)|\bno tasks\b")
        .expect("regex sans tâche")
});
static UNKNOWN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)unknown (?:or incomplete )?command|incorrect argument for command")
        .expect("regex commande inconnue")
});
/// Refus d'une commande de sélection par Chunky (monde introuvable, valeur invalide, aide
/// affichée) : `start` partirait sinon sur l'ancienne sélection.
static SETTING_REFUSED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\[chunky\].*\b(?:invalid|not found|not exist|does not exist|unknown|usage|error)\b",
    )
    .expect("regex de refus")
});

/// Ligne par laquelle Chunky refuse une commande de sélection, s'il y en a une.
fn setting_refusal(lines: &[String]) -> Option<&String> {
    lines.iter().find(|l| SETTING_REFUSED_RE.is_match(l))
}

/// Lignes de console qui concernent Chunky (transcription, suivi).
const CHUNKY_FILTER: &str =
    r"(?i)\[chunky\]|unknown (?:or incomplete )?command|incorrect argument for command";
/// Réponse à une commande de sélection : une ligne de Chunky qui n'est pas une progression
/// (« [Chunky] Task running… » d'une autre tâche ne compte pas), ou un refus de la commande.
const UNTIL_SETTING: &str = r"(?i)\[chunky\]\s*(?:[^t\s]|t[^a]|ta[^s]|tas[^k])|unknown (?:or incomplete )?command|incorrect argument for command";
const UNTIL_START: &str = r"(?i)task started (?:for|in) |already (?:started|running|exists)|chunky confirm|unknown (?:or incomplete )?command|incorrect argument for command|\[chunky\]\s*(?:invalid|no |not |error)";
const UNTIL_STATE: &str = r"(?i)task (?:paused|continuing|continued|resumed|cancell?ed|stopped) for|\bno tasks?\b|chunky confirm|unknown (?:or incomplete )?command|incorrect argument for command";
const UNTIL_PROGRESS: &str = r"(?i)task (?:running|finished) for|\bno tasks?\b|unknown (?:or incomplete )?command|incorrect argument for command";
const UNTIL_CONFIRM: &str = r"(?i)task (?:started|cancell?ed|stopped) for|\bno (?:action|pending|tasks?)\b|nothing to confirm|unknown (?:or incomplete )?command|incorrect argument for command";

/// Nombre affiché avec séparateurs éventuels (`1,234,567`, `1 234 567`).
fn parse_count(s: &str) -> Option<u64> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Décimal avec point ou virgule (`1.69`, `1,69`, `1 024,5`, `1,024.5`) : le dernier séparateur
/// est la marque décimale, les autres et les espaces sont des séparateurs de milliers.
fn parse_decimal(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\'')
        .collect();
    let normalized = match (t.rfind('.'), t.rfind(',')) {
        (Some(dot), Some(comma)) if comma > dot => t.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => t.replace(',', ""),
        (None, Some(_)) => t.replace(',', "."),
        _ => t,
    };
    normalized.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// Durée `H:MM:SS`, `MM:SS` ou `Nd H:MM:SS` en secondes.
pub(crate) fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, rest) = match s.split_once('d') {
        Some((d, r)) => (d.trim().parse::<u64>().ok()?, r.trim()),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.trim().parse::<u64>())
        .collect::<Result<_, _>>()
        .ok()?;
    let secs = match parts.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        [s] => *s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

fn clean_world(w: &str) -> String {
    w.trim().trim_end_matches(['.', ',', '!']).to_string()
}

/// Analyse une ligne de console (préfixe `[HH:MM:SS INFO]: [Chunky]` toléré).
pub(crate) fn parse_line(line: &str) -> Option<Event> {
    if UNKNOWN_RE.is_match(line) {
        return Some(Event::UnknownCommand);
    }
    if CONFIRM_RE.is_match(line) {
        return Some(Event::ConfirmRequested);
    }
    let progress = |c: &regex::Captures| TaskProgress {
        world: clean_world(&c["world"]),
        chunks: c
            .name("chunks")
            .and_then(|m| parse_count(m.as_str()))
            .unwrap_or(0),
        percent: c
            .name("pct")
            .and_then(|m| parse_decimal(m.as_str()))
            .unwrap_or(0.0),
        eta: c.name("eta").map(|m| m.as_str().trim().to_string()),
        eta_secs: c.name("eta").and_then(|m| parse_duration(m.as_str())),
        rate_cps: c.name("rate").and_then(|m| parse_decimal(m.as_str())),
        current: match (c.name("x"), c.name("z")) {
            (Some(x), Some(z)) => x.as_str().parse().ok().zip(z.as_str().parse().ok()),
            _ => None,
        },
        total_time: c.name("time").map(|m| m.as_str().trim().to_string()),
    };
    if let Some(c) = PROGRESS_RE.captures(line) {
        return Some(Event::Progress(progress(&c)));
    }
    if let Some(c) = DONE_RE.captures(line) {
        let mut p = progress(&c);
        if p.percent == 0.0 {
            p.percent = 100.0;
        }
        return Some(Event::Finished(p));
    }
    if let Some(c) = STARTED_RE.captures(line) {
        let n = |k: &str| parse_decimal(&c[k]).map(|f| f.round() as i64).unwrap_or(0);
        return Some(Event::Started {
            world: clean_world(&c["world"]),
            center: (n("x"), n("z")),
            radius: parse_decimal(&c["r"])
                .map(|f| f.round() as u64)
                .unwrap_or(0),
        });
    }
    if let Some(c) = STATE_RE.captures(line) {
        let state = match c["what"].to_ascii_lowercase().as_str() {
            "paused" => "paused",
            "continuing" | "continued" | "resumed" => "running",
            "stopped" => "stopped",
            _ => "cancelled",
        };
        return Some(Event::State {
            world: clean_world(&c["world"]),
            state,
        });
    }
    if let Some(c) = ALREADY_RE.captures(line) {
        return Some(Event::AlreadyRunning {
            world: c.name("world").map(|m| clean_world(m.as_str())),
        });
    }
    if NO_TASKS_RE.is_match(line) {
        return Some(Event::NoTasks);
    }
    None
}

/// État d'une tâche d'après les dernières lignes vues.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TaskView {
    pub world: String,
    /// `running`, `finished`, `paused`, `cancelled`, `stopped` ou `started`.
    pub state: &'static str,
    #[serde(flatten)]
    pub progress: Option<TaskProgress>,
    /// Chunks au total, déduits du pourcentage (ordre de grandeur).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_chunks: Option<u64>,
}

/// Dernier état connu de chaque tâche (par monde), dans l'ordre des lignes.
pub(crate) fn summarize(events: &[Event]) -> Vec<TaskView> {
    let mut tasks: BTreeMap<String, TaskView> = BTreeMap::new();
    for e in events {
        match e {
            Event::Progress(p) | Event::Finished(p) => {
                let state = if matches!(e, Event::Finished(_)) {
                    "finished"
                } else {
                    "running"
                };
                let total = (p.percent > 0.0 && p.percent <= 100.0)
                    .then(|| (p.chunks as f64 * 100.0 / p.percent).round() as u64);
                tasks.insert(
                    p.world.clone(),
                    TaskView {
                        world: p.world.clone(),
                        state,
                        progress: Some(p.clone()),
                        total_chunks: total,
                    },
                );
            }
            Event::Started { world, .. } => {
                tasks.insert(
                    world.clone(),
                    TaskView {
                        world: world.clone(),
                        state: "started",
                        progress: None,
                        total_chunks: None,
                    },
                );
            }
            Event::State { world, state } => {
                let t = tasks.entry(world.clone()).or_insert_with(|| TaskView {
                    world: world.clone(),
                    state,
                    progress: None,
                    total_chunks: None,
                });
                t.state = *state;
            }
            _ => {}
        }
    }
    tasks.into_values().collect()
}

// --- Console : envoi et lecture (boucle « agir puis vérifier ») ---------------------------------------

/// Réponse d'une commande envoyée par `run_command`.
#[derive(Debug, Clone, Default)]
struct Reply {
    command: String,
    lines: Vec<String>,
    matched: bool,
    cursor: Option<String>,
    source: String,
}

impl Reply {
    fn events(&self) -> Vec<Event> {
        self.lines.iter().filter_map(|l| parse_line(l)).collect()
    }

    fn has(&self, f: impl Fn(&Event) -> bool) -> bool {
        self.events().iter().any(f)
    }

    /// Lignes de Chunky (ou refus de commande) pour la transcription.
    fn transcript(&self) -> Value {
        static FILTER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(CHUNKY_FILTER).expect("filtre Chunky"));
        let lines: Vec<&String> = self
            .lines
            .iter()
            .filter(|l| FILTER.is_match(l))
            .take(20)
            .collect();
        json!({ "command": self.command, "answered": self.matched, "lines": lines })
    }
}

/// Lecture tolérante de la sortie JSON de `run_command` / `tail_console`.
fn parse_reply(command: &str, text: &str) -> Result<Reply, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("réponse console illisible : {e}"))?;
    Ok(Reply {
        command: command.to_string(),
        lines: v
            .get("lines")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        matched: v.get("matched").and_then(Value::as_bool).unwrap_or(false),
        cursor: v.get("cursor").and_then(Value::as_str).map(str::to_string),
        source: v
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

const UNKNOWN_MSG: &str = "La commande `chunky` est inconnue du serveur : Chunky n'est pas chargé (installé sans redémarrage, désactivé ou incompatible avec cette version). Redémarre le serveur (power_action restart, puis wait_for state=running), vérifie avec list_installed_plugins / list_installed_mods et error_digest, puis relance.";

/// Envoie une commande console et lit sa réponse (REST pour l'envoi, console par curseur pour la
/// lecture) ; refus explicite si le serveur est hors ligne (rien n'est envoyé).
async fn send(
    core: &Core,
    id: i64,
    command: &str,
    until: &str,
    timeout: f64,
) -> Result<Reply, String> {
    let args = json!({
        "server_id": id,
        "command": command,
        "until": until,
        "regex": true,
        "timeout_secs": timeout,
        "max_lines": 200,
    });
    let out = Box::pin(core.agent_run_command(&args)).await?;
    let reply = parse_reply(command, &out)?;
    if reply.has(|e| *e == Event::UnknownCommand) {
        return Err(UNKNOWN_MSG.into());
    }
    Ok(reply)
}

/// Lignes de Chunky apparues après `cursor` (au plus `wait` secondes d'attente de la première).
async fn tail(core: &Core, id: i64, cursor: Option<&str>, wait: f64) -> Result<Reply, String> {
    let mut args =
        json!({ "server_id": id, "filter": CHUNKY_FILTER, "regex": true, "max_lines": 500 });
    if let Some(c) = cursor {
        args["after"] = json!(c);
        args["wait_secs"] = json!(wait);
    } else {
        args["max_lines"] = json!(60);
    }
    let out = Box::pin(core.agent_tail_console(&args)).await?;
    parse_reply("", &out)
}

/// Monde suivi : nom donné à Chunky et `level-name` (pour reconnaître `minecraft:overworld`).
#[derive(Clone, Copy)]
struct Followed<'a> {
    world: Option<&'a str>,
    level_name: &'a str,
}

impl Followed<'_> {
    fn covers(&self, w: &str) -> bool {
        self.world.is_none_or(|x| same_world(x, w, self.level_name))
    }
}

/// État d'arrêt d'une tâche (`continuing` n'en est pas un : la tâche repart).
fn is_stop(state: &str) -> bool {
    matches!(state, "paused" | "cancelled" | "stopped")
}

/// La tâche suivie est-elle déjà terminée ou arrêtée d'après les lignes déjà lues ? Seul le dernier
/// événement qui la concerne compte. Sert à ne pas attendre une fin déjà passée : une petite zone
/// se termine souvent dans la réponse même à `chunky start`.
fn ended(events: &[Event], followed: Followed<'_>) -> bool {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Finished(p) if followed.covers(&p.world) => Some(true),
            Event::State { world, state } if followed.covers(world) => Some(is_stop(state)),
            Event::Progress(p) if followed.covers(&p.world) => Some(false),
            Event::Started { world, .. } if followed.covers(world) => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

/// Suit la progression jusqu'à la fin de la tâche suivie (toutes si aucun monde), l'échéance ou
/// l'annulation par le client. Renvoie les événements vus.
async fn follow(
    core: &Core,
    id: i64,
    mut cursor: Option<String>,
    followed: Followed<'_>,
    wait_secs: f64,
    notes: &mut Vec<String>,
) -> Vec<Event> {
    let mut events = Vec::new();
    if cursor.is_none() {
        notes.push("suivi impossible : console en direct indisponible (lecture par l'API, sans curseur) ; relance action=status plus tard".into());
        return events;
    }
    let deadline = Instant::now() + Duration::from_secs_f64(wait_secs);
    let mut finished = false;
    while !finished {
        if mcp_ctx::cancelled() {
            notes.push("suivi interrompu par le client : la tâche continue sur le serveur".into());
            break;
        }
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        if left <= 0.0 {
            break;
        }
        let r = match tail(core, id, cursor.as_deref(), left.min(FOLLOW_SLICE_SECS)).await {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("suivi interrompu : {e}"));
                break;
            }
        };
        if r.cursor.is_none() {
            notes.push("suivi interrompu : la console en direct a été perdue (lecture par l'API) ; relance action=status".into());
            break;
        }
        cursor = r.cursor.clone();
        for e in r.events() {
            let ours = |w: &str| followed.covers(w);
            match &e {
                Event::Progress(p) if ours(&p.world) => {
                    let msg = format!(
                        "{} : {:.1} % ({} chunks){}",
                        p.world,
                        p.percent,
                        p.chunks,
                        p.eta
                            .as_deref()
                            .map(|e| format!(", fin estimée dans {e}"))
                            .unwrap_or_default()
                    );
                    mcp_ctx::progress(p.percent.min(99.9), Some(100.0), &msg).await;
                }
                Event::Finished(p) if ours(&p.world) => {
                    mcp_ctx::progress(100.0, Some(100.0), &format!("{} : terminé", p.world)).await;
                    finished = true;
                }
                Event::State { world: w, state } if ours(w) && is_stop(state) => finished = true,
                _ => {}
            }
            events.push(e);
        }
    }
    events
}

// --- Façade : l'outil -------------------------------------------------------------------------------

/// État du serveur d'après l'API (`running`, `offline`, `starting`, `stopping`…).
async fn server_state(core: &Core, id: i64) -> Option<String> {
    match core.server_live(id).await {
        Ok(l) => Some(normalize_state(&l.state)),
        Err(_) => core
            .live_light(id)
            .await
            .ok()
            .and_then(|l| l.status)
            .map(|s| normalize_state(&s)),
    }
}

fn require_running(state: Option<&str>) -> Result<(), String> {
    match state {
        Some("offline") => Err("Serveur hors ligne : Chunky ne reçoit de commandes que serveur démarré. Démarre-le (power_action start, puis wait_for state=running) et relance ; rien n'a été envoyé.".into()),
        Some("starting") => Err("Serveur en cours de démarrage : attends wait_for state=running, puis relance ; rien n'a été envoyé.".into()),
        Some("stopping") => Err("Serveur en cours d'arrêt : rien n'a été envoyé.".into()),
        _ => Ok(()),
    }
}

/// Chunky installé : sa nature (plugin ou mod) et l'élément. Plateforme inconnue : plugins puis mods.
async fn find_chunky(core: &Core, id: i64, kind: Option<Kind>) -> Option<(Kind, InstalledItem)> {
    let kinds: &[Kind] = match kind {
        Some(Kind::Plugin) => &[Kind::Plugin],
        Some(Kind::Mod) => &[Kind::Mod],
        None => &[Kind::Plugin, Kind::Mod],
    };
    for k in kinds {
        let items = match k {
            Kind::Plugin => core.installed_plugins(id).await,
            Kind::Mod => core.installed_mods(id).await,
        };
        if let Some(item) = items
            .ok()
            .into_iter()
            .flatten()
            .find(|i| is_chunky(&i.name, &i.filename))
        {
            return Some((*k, item));
        }
    }
    None
}

/// Installe Chunky depuis Modrinth (passe par la garde des agents : étape annulable).
async fn install(
    core: &Core,
    id: i64,
    platform: &str,
    kind: Kind,
    game_version: Option<&str>,
) -> Result<(MarketVersion, Option<String>), String> {
    let gv = game_version.unwrap_or("").trim();
    let versions = core
        .market_versions("modrinth", CHUNKY_SLUG, "", gv)
        .await
        .map_err(|e| format!("versions de Chunky illisibles (Modrinth) : {e}"))?;
    let pick = pick_version(&versions, platform, gv).cloned().ok_or_else(|| {
        format!(
            "aucune version de Chunky pour {platform}{} sur Modrinth : installe une version compatible toi-même (market_search, list_mod_versions, install_mod), ou change de plateforme",
            if gv.is_empty() { String::new() } else { format!(" {gv}") }
        )
    })?;
    let note = crate::agent_guard::guarded_write(
        core,
        "install_mod",
        json!({
            "server_id": id,
            "source": "modrinth",
            "kind": kind.as_str(),
            "slug": CHUNKY_SLUG,
            "version_id": pick.id,
        }),
    )
    .await?;
    Ok((pick, note))
}

/// Outil MCP `pregenerate_world`.
pub(crate) async fn pregenerate_world_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = Request::parse(args)?;
    let id = req.server_id;
    let facts = core.manifest_facts(id).await?;
    if facts.family != "minecraft" {
        return Err(format!(
            "pregenerate_world : Minecraft Java seulement (Chunky) ; « {} » n'est pas géré",
            facts.egg_name
        ));
    }
    let platform = facts.loader.clone().unwrap_or_default();
    let declared = platform_kind(&platform);
    if declared.is_none()
        && !platform.is_empty()
        && !crate::updates::platforms_in(&platform).is_empty()
    {
        return Err(format!(
            "plateforme « {platform} » : Chunky s'installe sur le serveur de jeu (Paper, Spigot, Folia, Fabric, Forge, NeoForge), pas sur un proxy"
        ));
    }
    let found = find_chunky(core, id, declared).await;
    let kind = found.as_ref().map(|(k, _)| *k).or(declared);
    let mut notes: Vec<String> = Vec::new();
    let mut out = json!({
        "server_id": id,
        "action": req.action.as_str(),
        "platform": if platform.is_empty() { Value::Null } else { json!(platform) },
        "game_version": facts.game_version,
        "chunky": found.as_ref().map(|(k, i)| json!({ "installed": true, "kind": k.as_str(), "file": i.filename, "version": i.version, "enabled": i.enabled })).unwrap_or_else(|| json!({ "installed": false })),
    });
    if let Some((_, item)) = &found {
        if !item.enabled {
            return Err(format!(
                "Chunky est installé mais désactivé (« {} ») : réactive-le, redémarre le serveur, puis relance",
                item.filename
            ));
        }
    }
    let state = server_state(core, id).await;
    let level_name = core
        .server_properties(id)
        .await
        .ok()
        .and_then(|p| p.properties.get("level-name").cloned())
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "world".into());
    let world = kind.map(|k| chunky_world(req.world.as_deref(), &level_name, k));

    // --- start : estimation, installation éventuelle, sélection puis lancement -----------------
    if req.action == Action::Start {
        let radius = req.radius.unwrap_or_default();
        let chunks = estimate_chunks(radius, req.shape);
        let free = core.disk_space(id).await.ok().as_ref().and_then(free_bytes);
        let estimate = disk_estimate(chunks, free);
        out["selection"] = json!({ "world": world, "shape": req.shape.as_str(), "center": req.center.label(), "radius": radius });
        out["estimate"] = json!(estimate);
        out["estimate"]["note"] = json!("ordre de grandeur : chunks de la zone × 6 à 20 Kio par chunk (monde 1.18+, overworld) ; les chunks déjà générés ne reprennent pas de place");
        if estimate.verdict == DiskVerdict::Insufficient {
            return Err(format!(
                "Espace disque insuffisant pour prégénérer {chunks} chunks : {}",
                estimate.summary
            ));
        }
        if estimate.verdict == DiskVerdict::Warning {
            notes.push(estimate.summary.clone());
        }
        if found.is_none() {
            if !req.install_chunky {
                return Err(format!(
                    "Chunky n'est pas installé sur ce serveur (ni en plugin ni en mod). Relance avec `install_chunky=true` pour l'installer depuis Modrinth (version compatible avec la plateforme et la version du jeu, étape annulable avec rollback_changeset), puis redémarre le serveur ; ou installe-le toi-même (market_search « chunky », install_mod). Rien n'a été modifié. Estimation pour {chunks} chunks : {}",
                    estimate.summary
                ));
            }
            let Some(k) = declared else {
                return Err("plateforme du serveur non détectée (jar explicite ?) : impossible de choisir la version de Chunky ; installe-la toi-même (market_search « chunky », list_mod_versions, install_mod)".into());
            };
            let (version, guard) =
                install(core, id, &platform, k, facts.game_version.as_deref()).await?;
            out["chunky"] = json!({
                "installed": true,
                "just_installed": true,
                "kind": k.as_str(),
                "version": version.version_number,
                "version_id": version.id,
                "file": version.filename,
            });
            if let Some(g) = guard {
                notes.push(g);
            }
            out["started"] = json!(false);
            out["next"] = json!(match state.as_deref() {
                Some("offline") => "Chunky installé : démarre le serveur (power_action start, wait_for state=running), puis relance pregenerate_world avec les mêmes arguments (sans install_chunky).",
                _ => "Chunky installé : il se charge au prochain démarrage. Redémarre le serveur (power_action restart, puis wait_for state=running), puis relance pregenerate_world avec les mêmes arguments (sans install_chunky).",
            });
            if !notes.is_empty() {
                out["notes"] = json!(notes);
            }
            return Ok(crate::config_edit::render_for_ai(core, out));
        }
        require_running(state.as_deref())?;
        let world = world.unwrap_or_else(|| level_name.clone());
        let center_cmd = match req.center {
            Center::Spawn => "chunky spawn".to_string(),
            Center::At(x, z) => format!("chunky center {x} {z}"),
        };
        let selection = [
            format!("chunky world {world}"),
            format!("chunky shape {}", req.shape.as_str()),
            center_cmd,
            format!("chunky radius {radius}"),
        ];
        let mut transcript = Vec::new();
        for c in &selection {
            let r = send(core, id, c, UNTIL_SETTING, SETTING_TIMEOUT).await?;
            if let Some(line) = setting_refusal(&r.lines) {
                return Err(format!(
                    "Chunky a refusé « {c} » : {line} — rien n'a été lancé (`world` : dossier du monde sur Paper/Spigot, identifiant minecraft:… sur un serveur moddé)"
                ));
            }
            if !r.matched {
                notes.push(format!(
                    "pas de réponse reconnue de Chunky à « {c} » dans le délai"
                ));
            }
            transcript.push(r.transcript());
        }
        let mut reply = send(core, id, "chunky start", UNTIL_START, ACTION_TIMEOUT).await?;
        transcript.push(reply.transcript());
        if reply.has(|e| *e == Event::ConfirmRequested) {
            if !req.replace_existing {
                return Err(format!(
                    "Une tâche Chunky existe déjà pour « {world} » (en pause ou d'une session précédente). `action=continue` la reprend là où elle en est ; relance `start` avec `replace_existing=true` pour repartir de zéro (la progression est perdue, les chunks déjà générés restent). Rien n'a été lancé."
                ));
            }
            reply = send(core, id, "chunky confirm", UNTIL_CONFIRM, ACTION_TIMEOUT).await?;
            transcript.push(reply.transcript());
        }
        if reply.has(|e| matches!(e, Event::AlreadyRunning { .. })) {
            return Err(format!(
                "Une tâche Chunky est déjà en cours pour « {world} » : `action=status` pour sa progression, `pause` ou `cancel` pour l'arrêter."
            ));
        }
        let followed = Followed {
            world: Some(&world),
            level_name: &level_name,
        };
        let started = reply.has(|e| {
            matches!(e, Event::Started { .. })
                || matches!(e, Event::Progress(p) if followed.covers(&p.world))
        });
        if !started {
            notes
                .push("réponse de `chunky start` non reconnue : vérifie avec action=status".into());
        }
        out["started"] = json!(started);
        let mut events = reply.events();
        if req.wait_secs > 0.0 && started && !ended(&events, followed) {
            events.extend(
                follow(
                    core,
                    id,
                    reply.cursor.clone(),
                    followed,
                    req.wait_secs,
                    &mut notes,
                )
                .await,
            );
        }
        out["tasks"] = json!(summarize(&events));
        out["console"] = json!(transcript);
        out["next"] = json!("action=status (avec wait_secs pour suivre), pause, continue ou cancel ; la tâche continue même si cet appel se termine. Les chunks générés restent sur le disque (pas d'annulation par rollback_changeset).");
        if reply.source == "rest" {
            notes.push(
                "console en direct indisponible : réponses lues par l'API (100 dernières lignes)"
                    .into(),
            );
        }
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        return Ok(crate::config_edit::render_for_ai(core, out));
    }

    // --- status, pause, continue, cancel ------------------------------------------------------
    if found.is_none() {
        return Err("Chunky n'est pas installé sur ce serveur : `action=start` avec `install_chunky=true` l'installe (puis redémarrage).".into());
    }
    require_running(state.as_deref())?;
    // Monde visé : seulement s'il est donné (sinon toutes les tâches).
    let target = req.world.as_ref().and(world.clone());
    let mut transcript = Vec::new();
    let mut events = Vec::new();
    let mut cursor;
    match req.action {
        Action::Status => {
            let r = send(core, id, "chunky progress", UNTIL_PROGRESS, 5.0).await?;
            transcript.push(r.transcript());
            events.extend(r.events());
            cursor = r.cursor.clone();
            // Plusieurs tâches : une ligne chacune, juste après la première.
            if r.matched && cursor.is_some() {
                if let Ok(more) = tail(core, id, cursor.as_deref(), 1.0).await {
                    events.extend(more.events());
                    cursor = more.cursor.or(cursor);
                }
            }
            let known = events
                .iter()
                .any(|e| matches!(e, Event::Progress(_) | Event::Finished(_) | Event::NoTasks));
            if !known {
                // Chunky sans `progress`, ou réponse non reconnue : dernières lignes de la console.
                if let Ok(recent) = tail(core, id, None, 0.0).await {
                    events.extend(recent.events());
                }
                notes.push("progression lue dans les dernières lignes de Chunky de la console (réponse à `chunky progress` non reconnue)".into());
            }
        }
        Action::Pause | Action::Continue | Action::Cancel => {
            let verb = req.action.as_str();
            let cmd = match &target {
                Some(w) => format!("chunky {verb} {w}"),
                None => format!("chunky {verb}"),
            };
            let mut r = send(core, id, &cmd, UNTIL_STATE, ACTION_TIMEOUT).await?;
            transcript.push(r.transcript());
            if r.has(|e| *e == Event::ConfirmRequested) {
                // Annulation demandée explicitement : confirmée auprès de Chunky.
                r = send(core, id, "chunky confirm", UNTIL_CONFIRM, ACTION_TIMEOUT).await?;
                transcript.push(r.transcript());
            }
            if !r.matched {
                notes.push(format!(
                    "pas de réponse reconnue de Chunky à « {cmd} » : vérifie avec action=status"
                ));
            }
            events.extend(r.events());
            cursor = r.cursor.clone();
        }
        Action::Start => unreachable!("traité plus haut"),
    }
    if req.wait_secs > 0.0 && matches!(req.action, Action::Status | Action::Continue) {
        let running = events.iter().any(|e| {
            matches!(e, Event::Progress(_))
                || matches!(
                    e,
                    Event::State {
                        state: "running",
                        ..
                    }
                )
        });
        let followed = Followed {
            world: target.as_deref(),
            level_name: &level_name,
        };
        if running && !ended(&events, followed) {
            events.extend(
                follow(
                    core,
                    id,
                    cursor.clone(),
                    followed,
                    req.wait_secs,
                    &mut notes,
                )
                .await,
            );
        }
    }
    let tasks = summarize(&events);
    if events.contains(&Event::NoTasks) && tasks.is_empty() {
        out["no_tasks"] = json!(true);
    }
    out["tasks"] = json!(tasks);
    out["console"] = json!(transcript);
    if !notes.is_empty() {
        out["notes"] = json!(notes);
    }
    Ok(crate::config_edit::render_for_ai(core, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Option<Event> {
        parse_line(s)
    }

    /// Réponse réelle à `chunky start` sur une petite zone (Chunky 1.5.66, Paper 26.1.2) : la tâche
    /// se termine avant la fin de la réponse, le suivi ne doit pas attendre son échéance.
    #[test]
    fn a_task_finished_in_the_start_reply_is_not_followed() {
        let lines = [
            "[00:05:17 INFO]: [Chunky] Task started in minecraft:overworld for the square region centered at 0, 0 with radius 200.",
            "[00:05:17 INFO]: [Chunky] Task running for minecraft:overworld. Processed: 391 chunks (53.64%), ETA: 0:00:00, Rate: 925.9 cps, Current: 0, 0",
            "[00:05:21 INFO]: [Chunky] Task finished for minecraft:overworld. Processed: 729 chunks (100.00%), Total time: 0:00:04",
        ];
        let events: Vec<Event> = lines.iter().filter_map(|l| line(l)).collect();
        let world = Followed { world: Some("world"), level_name: "world" };
        assert!(ended(&events, world));
        // Encore en cours : suivi.
        assert!(!ended(&events[..2], world));
        // Une autre dimension terminée ne compte pas.
        let nether = Followed { world: Some("world_nether"), level_name: "world" };
        assert!(!ended(&events, nether));
        // Reprise : pas un arrêt.
        let resumed = [Event::State { world: "minecraft:overworld".into(), state: "continuing" }];
        assert!(!ended(&resumed, world));
        let paused = [Event::State { world: "minecraft:overworld".into(), state: "paused" }];
        assert!(ended(&paused, world));
    }

    #[test]
    fn progress_lines_are_parsed_with_their_numbers() {
        let Some(Event::Progress(p)) = line("[12:34:56 INFO]: [Chunky] Task running for world. Processed: 26394 chunks (1.69%), ETA: 1:02:22, Rate: 410.1 cps, Current: -114, 51") else {
            panic!("progression non reconnue")
        };
        assert_eq!(p.world, "world");
        assert_eq!((p.chunks, p.percent), (26394, 1.69));
        assert_eq!(
            (p.eta.as_deref(), p.eta_secs),
            (Some("1:02:22"), Some(3742))
        );
        assert_eq!((p.rate_cps, p.current), (Some(410.1), Some((-114, 51))));
        // Fabric : identifiant de dimension ; séparateurs de milliers et virgule décimale.
        let Some(Event::Progress(p)) = line("[Chunky] Task running for minecraft:the_nether. Processed: 1,234,567 chunks (45,5%), ETA: 0:10:00, Rate: 1 024,5 cps") else {
            panic!("variante non reconnue")
        };
        assert_eq!(p.world, "minecraft:the_nether");
        assert_eq!(
            (p.chunks, p.percent, p.eta_secs),
            (1_234_567, 45.5, Some(600))
        );
        assert_eq!(p.current, None);
        // Sans ETA ni vitesse (début de tâche) : le pourcentage suffit.
        let Some(Event::Progress(p)) =
            line("[Chunky] Task running for world_the_end. Processed: 0 chunks (0.00%)")
        else {
            panic!("ligne minimale non reconnue")
        };
        assert_eq!(
            (p.world.as_str(), p.chunks, p.eta),
            ("world_the_end", 0, None)
        );
    }

    #[test]
    fn other_chunky_messages_are_recognized() {
        let Some(Event::Finished(p)) = line("[Chunky] Task finished for world. Processed: 1560849 chunks (100.00%), Total time: 1:27:51") else {
            panic!("fin non reconnue")
        };
        assert_eq!(
            (p.chunks, p.percent, p.total_time.as_deref()),
            (1_560_849, 100.0, Some("1:27:51"))
        );
        assert_eq!(
            line("[Chunky] Task started for world at 0, 0 with radius 5000."),
            Some(Event::Started {
                world: "world".into(),
                center: (0, 0),
                radius: 5000
            })
        );
        // Forme réelle de Chunky 1.5.66.
        assert_eq!(
            line("[00:05:17 INFO]: [Chunky] Task started in minecraft:overworld for the square region centered at 0, 0 with radius 200."),
            Some(Event::Started { world: "minecraft:overworld".into(), center: (0, 0), radius: 200 })
        );
        assert_eq!(
            line("[Chunky] Task started in world_nether for the circle region centered at -16, 32 with radius 1000."),
            Some(Event::Started { world: "world_nether".into(), center: (-16, 32), radius: 1000 })
        );
        assert_eq!(
            line("[Chunky] Task started for minecraft:overworld at -250.0, 1000.0 with radius 2000.0."),
            Some(Event::Started { world: "minecraft:overworld".into(), center: (-250, 1000), radius: 2000 })
        );
        for (text, state) in [
            ("[Chunky] Task paused for world.", "paused"),
            ("[Chunky] Task continuing for world.", "running"),
            ("[Chunky] Task cancelled for world.", "cancelled"),
            ("[Chunky] Task canceled for world", "cancelled"),
            ("[Chunky] Task stopped for world.", "stopped"),
        ] {
            assert_eq!(
                line(text),
                Some(Event::State {
                    world: "world".into(),
                    state
                }),
                "{text}"
            );
        }
        assert_eq!(
            line("[Chunky] Task already exists for world. Type /chunky confirm to start over."),
            Some(Event::ConfirmRequested),
            "la confirmation l'emporte"
        );
        assert_eq!(
            line("[Chunky] Task already started for world."),
            Some(Event::AlreadyRunning {
                world: Some("world".into())
            })
        );
        assert_eq!(line("[Chunky] No tasks running."), Some(Event::NoTasks));
        assert_eq!(line("[Chunky] No tasks to pause."), Some(Event::NoTasks));
        assert_eq!(
            line("Unknown command. Type \"/help\" for help."),
            Some(Event::UnknownCommand)
        );
        assert_eq!(
            line("Unknown or incomplete command, see below for error"),
            Some(Event::UnknownCommand)
        );
        assert_eq!(
            line("[Server thread/INFO]: Done (3.2s)! For help, type \"help\""),
            None
        );
        assert_eq!(line("[Chunky] World changed to world."), None);
    }

    #[test]
    fn durations_and_counts_tolerate_variants() {
        assert_eq!(parse_duration("1:02:22"), Some(3742));
        assert_eq!(parse_duration("05:07"), Some(307));
        assert_eq!(parse_duration("2d 1:00:00"), Some(2 * 86_400 + 3600));
        assert_eq!(parse_duration("bientôt"), None);
        assert_eq!(parse_count("1 234 567"), Some(1_234_567));
        assert_eq!(parse_count("1.234.567"), Some(1_234_567));
        assert_eq!(parse_decimal("45,5"), Some(45.5));
    }

    #[test]
    fn the_task_table_keeps_the_latest_state_per_world() {
        let events: Vec<Event> = [
            "[Chunky] Task started for world at 0, 0 with radius 1000.",
            "[Chunky] Task running for world. Processed: 500 chunks (25.00%), ETA: 0:00:30, Rate: 50.0 cps, Current: 1, 2",
            "[Chunky] Task running for world_nether. Processed: 10 chunks (1.00%), ETA: 0:10:00, Rate: 5.0 cps, Current: 0, 0",
            "[Chunky] Task running for world. Processed: 1000 chunks (50.00%), ETA: 0:00:20, Rate: 50.0 cps, Current: 3, 4",
            "[Chunky] Task paused for world_nether.",
        ]
        .iter()
        .filter_map(|l| parse_line(l))
        .collect();
        let tasks = summarize(&events);
        assert_eq!(tasks.len(), 2);
        let world = tasks.iter().find(|t| t.world == "world").unwrap();
        assert_eq!((world.state, world.total_chunks), ("running", Some(2000)));
        assert_eq!(world.progress.as_ref().unwrap().chunks, 1000);
        let nether = tasks.iter().find(|t| t.world == "world_nether").unwrap();
        assert_eq!(nether.state, "paused");
        // Sérialisation à plat : percent, eta… au niveau de la tâche.
        let v = serde_json::to_value(world).unwrap();
        assert_eq!(v["percent"], 50.0);
        assert_eq!(v["state"], "running");
    }

    #[test]
    fn setting_responses_are_told_apart_from_progress_lines() {
        let re = Regex::new(UNTIL_SETTING).unwrap();
        assert!(re.is_match("[Chunky] World changed to world."));
        assert!(re.is_match("[Chunky] Radius changed to 5000 for world."));
        assert!(re.is_match("Unknown or incomplete command, see below for error"));
        assert!(!re.is_match("[Chunky] Task running for world. Processed: 1 chunks (0.01%)"));
        // Un refus de sélection arrête tout avant `chunky start` (format supposé).
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(setting_refusal(&lines(&["[Chunky] World not found: mondes"])).is_some());
        assert!(setting_refusal(&lines(&["[Chunky] Invalid input: abc"])).is_some());
        assert!(setting_refusal(&lines(&[
            "[Chunky] World changed to world.",
            "[Chunky] Task running for minecraft:overworld. Processed: 9 chunks (0.83%), ETA: 0:00:13, Rate: 78.3 cps, Current: -11550, -45041",
        ]))
        .is_none());
        let start = Regex::new(UNTIL_START).unwrap();
        assert!(start.is_match("[Chunky] Task started for world at 0, 0 with radius 10."));
        assert!(start.is_match("[Chunky] Task started in minecraft:overworld for the square region centered at 0, 0 with radius 200."));
        assert!(start.is_match(
            "[Chunky] Task already exists for world. Type /chunky confirm to start over."
        ));
        assert!(
            !start.is_match("[Chunky] Task running for world_nether. Processed: 1 chunks (0.01%)")
        );
    }

    #[test]
    fn requests_are_validated_before_anything_is_sent() {
        let ok =
            Request::parse(&json!({ "server_id": 7, "action": "start", "radius": 5000 })).unwrap();
        assert_eq!(
            (ok.shape, ok.center, ok.install_chunky, ok.wait_secs),
            (Shape::Square, Center::Spawn, false, 0.0)
        );
        let err = |v: Value| Request::parse(&v).unwrap_err();
        assert!(err(json!({ "server_id": 7, "action": "start" })).contains("radius"));
        assert!(
            err(json!({ "server_id": 7, "action": "start", "radius": -5 })).contains("positif")
        );
        assert!(
            err(json!({ "server_id": 7, "action": "start", "radius": 5_000_000 }))
                .contains("refusé")
        );
        assert!(err(json!({ "server_id": 7, "action": "explode" })).contains("inconnue"));
        assert!(err(
            json!({ "server_id": 7, "action": "start", "radius": 10, "shape": "rectangle" })
        )
        .contains("deux rayons"));
        assert!(err(
            json!({ "server_id": 7, "action": "start", "radius": 10, "world": "mon monde" })
        )
        .contains("sans espace"));
        assert!(
            err(json!({ "server_id": 7, "action": "start", "radius": 10, "center": "a,b" }))
                .contains("center")
        );
        let r = Request::parse(&json!({ "server_id": "7", "action": "STATUS", "wait_secs": 99999, "center": [100, -200] })).unwrap();
        assert_eq!(
            (r.server_id, r.action, r.wait_secs, r.center),
            (7, Action::Status, MAX_WAIT_SECS, Center::At(100, -200))
        );
        assert_eq!(
            Center::parse(Some(&json!("12, -8"))).unwrap(),
            Center::At(12, -8)
        );
        assert_eq!(
            Center::parse(Some(&json!({ "x": 1.6, "z": "2" }))).unwrap(),
            Center::At(2, 2)
        );
    }

    #[test]
    fn world_names_follow_the_platform() {
        assert_eq!(chunky_world(None, "world", Kind::Plugin), "world");
        assert_eq!(chunky_world(None, "Survie", Kind::Plugin), "Survie");
        assert_eq!(
            chunky_world(Some("nether"), "Survie", Kind::Plugin),
            "Survie_nether"
        );
        assert_eq!(
            chunky_world(Some("minecraft:the_end"), "world", Kind::Plugin),
            "world_the_end"
        );
        assert_eq!(
            chunky_world(Some("ressources"), "world", Kind::Plugin),
            "ressources"
        );
        assert_eq!(
            chunky_world(None, "world", Kind::Mod),
            "minecraft:overworld"
        );
        assert_eq!(
            chunky_world(Some("world_nether"), "world", Kind::Mod),
            "minecraft:the_nether"
        );
        assert_eq!(
            chunky_world(Some("end"), "world", Kind::Mod),
            "minecraft:the_end"
        );
        assert_eq!(
            chunky_world(Some("mondes:ressources"), "world", Kind::Mod),
            "mondes:ressources"
        );
        // Messages de Chunky (identifiant) face au nom choisi (dossier) : même monde.
        assert!(same_world("world", "minecraft:overworld", "world"));
        assert!(same_world(
            "Survie_nether",
            "minecraft:the_nether",
            "Survie"
        ));
        assert!(!same_world("world", "minecraft:the_nether", "world"));
        assert!(same_world("Ressources", "ressources", "world"));
        assert!(!same_world("ressources", "minecraft:overworld", "world"));
    }

    /// Lignes relevées telles quelles dans les journaux d'un serveur réel (Chunky 1.5.66, Paper).
    #[test]
    fn real_chunky_log_lines_are_parsed() {
        let running = "2026-11-11 [20:21:58] [Server thread/INFO]: [Chunky] Task running for minecraft:overworld. Processed: 9 chunks (0.83%), ETA: 0:00:13, Rate: 78.3 cps, Current: -11550, -45041";
        let Some(Event::Progress(p)) = parse_line(running) else {
            panic!("progression réelle non reconnue")
        };
        assert_eq!(
            (p.world.as_str(), p.chunks, p.percent),
            ("minecraft:overworld", 9, 0.83)
        );
        assert_eq!(
            (p.eta_secs, p.rate_cps, p.current),
            (Some(13), Some(78.3), Some((-11550, -45041)))
        );
        let done = "[20:22:39] [Server thread/INFO]: [Chunky] Task finished for minecraft:overworld. Processed: 1089 chunks (100.00%), Total time: 0:00:44";
        let Some(Event::Finished(p)) = parse_line(done) else {
            panic!("fin réelle non reconnue")
        };
        assert_eq!((p.chunks, p.total_time.as_deref()), (1089, Some("0:00:44")));
        // Chargement du plugin : rien à en conclure ; autre plugin au nom voisin : pas Chunky.
        assert_eq!(parse_line("[Chunky] Enabling Chunky v1.5.66"), None);
        assert!(!is_chunky(
            "VoxelBench-Chunky",
            "VoxelBench-Chunky-1.0.0.jar"
        ));
        let tasks = summarize(&[parse_line(running).unwrap(), parse_line(done).unwrap()]);
        assert_eq!(
            (tasks.len(), tasks[0].state, tasks[0].total_chunks),
            (1, "finished", Some(1089))
        );
    }

    #[test]
    fn disk_estimates_warn_and_refuse_only_when_clearly_short() {
        // Rayon 5000 en carré : 625 × 625 chunks.
        assert_eq!(estimate_chunks(5000, Shape::Square), 390_625);
        let circle = estimate_chunks(5000, Shape::Circle);
        assert!((306_000..=307_000).contains(&circle), "{circle}");
        assert!(estimate_chunks(1, Shape::Star) >= 1);
        let chunks = 390_625;
        let gib = 1024 * 1024 * 1024;
        assert_eq!(
            disk_estimate(chunks, Some(100 * gib)).verdict,
            DiskVerdict::Ok
        );
        // Entre l'estimation basse (≈ 2,2 Gio) et haute (≈ 7,5 Gio) : avertissement.
        assert_eq!(
            disk_estimate(chunks, Some(5 * gib)).verdict,
            DiskVerdict::Warning
        );
        let short = disk_estimate(chunks, Some(gib));
        assert_eq!(short.verdict, DiskVerdict::Insufficient);
        assert!(short.summary.contains("refusée"));
        assert_eq!(disk_estimate(chunks, None).verdict, DiskVerdict::Unknown);
        let space = crate::server_files::DiskSpace {
            used_bytes: 30 * gib,
            limit_bytes: 50 * gib,
            mybox: Some(crate::server_files::MyBoxDiskSpace {
                used_bytes: 140 * gib,
                limit_bytes: 150 * gib,
            }),
            ..Default::default()
        };
        assert_eq!(
            free_bytes(&space),
            Some(10 * gib),
            "la MyBox est plus juste que le serveur"
        );
        assert_eq!(free_bytes(&crate::server_files::DiskSpace::default()), None);
    }

    fn version(
        id: &str,
        loaders: &[&str],
        games: &[&str],
        kind: &str,
        file: &str,
    ) -> MarketVersion {
        MarketVersion {
            id: id.into(),
            version_number: id.into(),
            name: id.into(),
            game_versions: games.iter().map(|s| s.to_string()).collect(),
            loaders: loaders.iter().map(|s| s.to_string()).collect(),
            release_type: kind.into(),
            downloads: None,
            date_published: String::new(),
            filename: Some(file.into()),
            file_size: None,
        }
    }

    #[test]
    fn the_installed_version_matches_platform_and_game_version() {
        let list = vec![
            version(
                "fabric-new",
                &["fabric"],
                &["1.21.4"],
                "release",
                "Chunky-Fabric-1.4.30.jar",
            ),
            version(
                "paper-beta",
                &["paper", "folia"],
                &["1.21.4"],
                "beta",
                "Chunky-Paper-1.4.30.jar",
            ),
            version(
                "bukkit",
                &["bukkit", "spigot", "paper", "purpur", "folia"],
                &["1.21.4"],
                "release",
                "Chunky-Bukkit-1.4.28.jar",
            ),
            version(
                "neoforge",
                &["neoforge"],
                &["1.21.1"],
                "release",
                "Chunky-NeoForge-1.4.27.jar",
            ),
        ];
        assert_eq!(
            pick_version(&list, "paper", "1.21.4").unwrap().id,
            "bukkit",
            "stable d'abord"
        );
        assert_eq!(pick_version(&list, "folia", "1.21.4").unwrap().id, "bukkit");
        assert_eq!(
            pick_version(&list, "fabric", "1.21.4").unwrap().id,
            "fabric-new"
        );
        assert_eq!(
            pick_version(&list, "quilt", "1.21.4").unwrap().id,
            "fabric-new"
        );
        assert!(
            pick_version(&list, "neoforge", "1.21.4").is_none(),
            "pas pour cette version du jeu"
        );
        assert_eq!(
            pick_version(&list, "neoforge", "1.21.1").unwrap().id,
            "neoforge"
        );
        assert!(pick_version(&list, "forge", "").is_none());
        // Spigot ne charge pas un build réservé à Paper.
        let paper_only = vec![version(
            "p",
            &["paper"],
            &["1.21.4"],
            "release",
            "Chunky-Paper-1.4.30.jar",
        )];
        assert!(pick_version(&paper_only, "spigot", "1.21.4").is_none());
        // Faute de stable, la plus récente compatible.
        assert_eq!(
            pick_version(&list[1..2], "paper", "1.21.4").unwrap().id,
            "paper-beta"
        );
    }

    #[test]
    fn chunky_is_recognized_but_not_its_addons() {
        assert!(is_chunky("Chunky", "Chunky-Bukkit-1.4.28.jar"));
        assert!(is_chunky("", "Chunky-Fabric-1.4.30.jar"));
        assert!(is_chunky("", "chunky.jar.disabled"));
        assert!(!is_chunky("ChunkyBorder", "ChunkyBorder-1.2.jar"));
        assert!(!is_chunky("", "ChunkyBorder-Bukkit-1.2.jar"));
        assert_eq!(platform_kind("paper"), Some(Kind::Plugin));
        assert_eq!(platform_kind("folia"), Some(Kind::Plugin));
        assert_eq!(platform_kind("neoforge"), Some(Kind::Mod));
        assert_eq!(platform_kind("velocity"), None);
        assert_eq!(platform_kind("vanilla"), None);
    }

    #[test]
    fn offline_and_transitional_states_are_refused() {
        assert!(require_running(Some("offline"))
            .unwrap_err()
            .contains("hors ligne"));
        assert!(require_running(Some("starting")).is_err());
        assert!(require_running(Some("running")).is_ok());
        assert!(
            require_running(None).is_ok(),
            "état illisible : run_command tranchera"
        );
    }

    #[test]
    fn console_replies_are_read_back() {
        let text = r#"{ "server_id": 1, "sent": true, "lines": ["[INFO]: [Chunky] Task started for world at 0, 0 with radius 10.", "autre"], "matched": true, "cursor": "abc-12", "source": "websocket" }"#;
        let r = parse_reply("chunky start", text).unwrap();
        assert!(r.matched && r.cursor.as_deref() == Some("abc-12"));
        assert_eq!(r.events().len(), 1);
        let t = r.transcript();
        assert_eq!(
            t["lines"].as_array().unwrap().len(),
            1,
            "seules les lignes de Chunky"
        );
    }
}
