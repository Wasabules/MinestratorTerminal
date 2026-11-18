//! Boîte à outils « maps corrompues » : inspection et réparation des fichiers de région `.mca`
//! d'un serveur, via SFTP. Le parsing/validation pur vit dans [`crate::mca`] (testé) ; ici on gère
//! les échanges SFTP (télécharge/renvoie le fichier) et la mise en forme lisible.
//!
//! `inspect_region` est en LECTURE SEULE (sûr). `repair_region` est DESTRUCTIF (écrase / supprime
//! des chunks) → exposé uniquement en action « danger » à confirmer, snapshot recommandé d'abord.
//!
//! `world_info` (lecture seule) résume un monde : `level.dat` et, depuis Minecraft 26.1, les
//! fichiers `data/minecraft/*.dat` où le jeu a déplacé graine, règles, météo, bordure et horloges,
//! plus la taille de chaque dimension. Lecture des champs (pure, testée) via [`crate::nbt::parse`].

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

// --- Informations d'un monde (outil MCP `world_info`) ---------------------------
//
// Deux formats, relevés sur des serveurs réels :
// - jusqu'à 1.21.x, tout est dans `level.dat` (`Data.WorldGenSettings.seed` depuis 1.16,
//   `Data.RandomSeed` avant ; `GameRules`, `DayTime`, météo, bordure, `SpawnX/Y/Z`…) ;
// - depuis 26.1, `level.dat` ne garde que l'essentiel (`difficulty_settings`, `spawn`, datapacks)
//   et le reste passe dans `data/minecraft/{world_gen_settings,game_rules,weather,world_border,
//   world_clocks}.dat` — sous Paper, dans `dimensions/minecraft/overworld/data/minecraft/`. Les
//   régions vivent alors dans `dimensions/minecraft/<dimension>/region`.

use fastnbt::Value as Nbt;

/// Fichiers du format 26.1+ lus en complément de `level.dat`.
const SPLIT_FILES: &[&str] = &["world_gen_settings", "game_rules", "weather", "world_border", "world_clocks"];

/// Ce qu'on sait d'un monde.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct LevelInfo {
    pub name: Option<String>,
    pub seed: Option<i64>,
    pub structures: Option<bool>,
    pub version: Option<String>,
    pub data_version: Option<i64>,
    pub snapshot: bool,
    pub game_type: Option<i64>,
    pub difficulty: Option<String>,
    pub difficulty_locked: Option<bool>,
    pub hardcore: Option<bool>,
    pub commands: Option<bool>,
    pub spawn: Option<(i64, i64, i64)>,
    pub spawn_dimension: Option<String>,
    /// Âge du monde en ticks (`Time`).
    pub game_time: Option<i64>,
    /// Heure du jour cumulée en ticks (`DayTime`, ou horloge de l'overworld en 26.1+).
    pub day_time: Option<i64>,
    pub last_played_ms: Option<i64>,
    pub raining: Option<bool>,
    pub thundering: Option<bool>,
    pub border: Option<f64>,
    pub border_center: Option<(f64, f64)>,
    pub brands: Vec<String>,
    pub modded: Option<bool>,
    pub game_rules: Vec<(String, String)>,
    pub datapacks_enabled: Vec<String>,
    pub datapacks_disabled: Vec<String>,
    /// Fichiers `data/minecraft/*.dat` lus (format 26.1+).
    pub split_files: Vec<String>,
}

fn child<'a>(v: &'a Nbt, key: &str) -> Option<&'a Nbt> {
    match v {
        Nbt::Compound(m) => m.get(key),
        _ => None,
    }
}

fn nbt_int(v: Option<&Nbt>) -> Option<i64> {
    match v? {
        Nbt::Byte(x) => Some(*x as i64),
        Nbt::Short(x) => Some(*x as i64),
        Nbt::Int(x) => Some(*x as i64),
        Nbt::Long(x) => Some(*x),
        Nbt::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn nbt_flag(v: Option<&Nbt>) -> Option<bool> {
    match v? {
        Nbt::String(s) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        other => nbt_int(Some(other)).map(|x| x != 0),
    }
}

fn nbt_float(v: Option<&Nbt>) -> Option<f64> {
    match v? {
        Nbt::Float(x) => Some(*x as f64),
        Nbt::Double(x) => Some(*x),
        other => nbt_int(Some(other)).map(|x| x as f64),
    }
}

fn nbt_text(v: Option<&Nbt>) -> Option<String> {
    match v? {
        Nbt::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn nbt_strings(v: Option<&Nbt>) -> Vec<String> {
    match v {
        Some(Nbt::List(l)) => l.iter().filter_map(|x| nbt_text(Some(x))).collect(),
        _ => Vec::new(),
    }
}

/// Coordonnées `[x, y, z]` (tableau d'entiers ou liste).
fn nbt_pos(v: Option<&Nbt>) -> Option<(i64, i64, i64)> {
    let nums: Vec<i64> = match v? {
        Nbt::IntArray(a) => a.iter().map(|x| *x as i64).collect(),
        Nbt::List(l) => l.iter().filter_map(|x| nbt_int(Some(x))).collect(),
        _ => return None,
    };
    (nums.len() == 3).then(|| (nums[0], nums[1], nums[2]))
}

/// Règles de jeu triées : anciennes (`GameRules`, valeurs texte, `keepInventory`) comme
/// nouvelles (`minecraft:keep_inventory`, booléens en octet).
fn nbt_rules(v: Option<&Nbt>) -> Vec<(String, String)> {
    let Some(Nbt::Compound(m)) = v else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = m
        .iter()
        .map(|(k, v)| {
            let value = match v {
                Nbt::String(s) => s.clone(),
                Nbt::Byte(b) => (if *b != 0 { "true" } else { "false" }).to_string(),
                other => nbt_int(Some(other)).map(|x| x.to_string()).or_else(|| nbt_float(Some(other)).map(|x| x.to_string())).unwrap_or_else(|| "?".into()),
            };
            (k.strip_prefix("minecraft:").unwrap_or(k).to_string(), value)
        })
        .collect();
    out.sort();
    out
}

fn difficulty_name(n: i64) -> String {
    match n {
        0 => "peaceful",
        1 => "easy",
        2 => "normal",
        3 => "hard",
        _ => "?",
    }
    .to_string()
}

impl LevelInfo {
    /// Lit `level.dat` (racine `{Data: {…}}`).
    pub(crate) fn from_level_dat(root: &Nbt) -> Result<Self, String> {
        let data = child(root, "Data").unwrap_or(root);
        if !matches!(data, Nbt::Compound(_)) {
            return Err("level.dat sans compound Data".into());
        }
        let mut i = LevelInfo {
            name: nbt_text(child(data, "LevelName")),
            data_version: nbt_int(child(data, "DataVersion")),
            game_type: nbt_int(child(data, "GameType")),
            commands: nbt_flag(child(data, "allowCommands")),
            game_time: nbt_int(child(data, "Time")),
            day_time: nbt_int(child(data, "DayTime")),
            last_played_ms: nbt_int(child(data, "LastPlayed")),
            raining: nbt_flag(child(data, "raining")),
            thundering: nbt_flag(child(data, "thundering")),
            border: nbt_float(child(data, "BorderSize")),
            brands: nbt_strings(child(data, "ServerBrands")),
            modded: nbt_flag(child(data, "WasModded")),
            game_rules: nbt_rules(child(data, "GameRules")),
            ..Default::default()
        };
        if let Some(v) = child(data, "Version") {
            i.version = nbt_text(child(v, "Name"));
            i.snapshot = nbt_flag(child(v, "Snapshot")).unwrap_or(false);
        }
        let wgs = child(data, "WorldGenSettings");
        i.seed = wgs.and_then(|w| nbt_int(child(w, "seed"))).or_else(|| nbt_int(child(data, "RandomSeed")));
        i.structures = wgs
            .and_then(|w| nbt_flag(child(w, "generate_features")))
            .or_else(|| nbt_flag(child(data, "MapFeatures")));
        if let Some(ds) = child(data, "difficulty_settings") {
            i.difficulty = nbt_text(child(ds, "difficulty"));
            i.hardcore = nbt_flag(child(ds, "hardcore"));
            i.difficulty_locked = nbt_flag(child(ds, "locked"));
        } else {
            i.difficulty = nbt_int(child(data, "Difficulty")).map(difficulty_name);
            i.hardcore = nbt_flag(child(data, "hardcore"));
            i.difficulty_locked = nbt_flag(child(data, "DifficultyLocked"));
        }
        if let Some(sp) = child(data, "spawn") {
            i.spawn = nbt_pos(child(sp, "pos"));
            i.spawn_dimension = nbt_text(child(sp, "dimension"));
        } else if let (Some(x), Some(y), Some(z)) =
            (nbt_int(child(data, "SpawnX")), nbt_int(child(data, "SpawnY")), nbt_int(child(data, "SpawnZ")))
        {
            i.spawn = Some((x, y, z));
        }
        if let (Some(x), Some(z)) = (nbt_float(child(data, "BorderCenterX")), nbt_float(child(data, "BorderCenterZ"))) {
            i.border_center = Some((x, z));
        }
        if let Some(dp) = child(data, "DataPacks") {
            i.datapacks_enabled = nbt_strings(child(dp, "Enabled"));
            i.datapacks_disabled = nbt_strings(child(dp, "Disabled"));
        }
        Ok(i)
    }

    /// Complète avec un fichier `data/minecraft/<name>.dat` du format 26.1+ (racine `{data: …}`).
    pub(crate) fn apply_split(&mut self, name: &str, root: &Nbt) {
        let Some(data) = child(root, "data") else { return };
        match name {
            "world_gen_settings" => {
                self.seed = nbt_int(child(data, "seed")).or(self.seed);
                self.structures = nbt_flag(child(data, "generate_structures")).or(self.structures);
            }
            "game_rules" => self.game_rules = nbt_rules(Some(data)),
            "weather" => {
                self.raining = nbt_flag(child(data, "raining")).or(self.raining);
                self.thundering = nbt_flag(child(data, "thundering")).or(self.thundering);
            }
            "world_border" => {
                self.border = nbt_float(child(data, "size")).or(self.border);
                if let (Some(x), Some(z)) = (nbt_float(child(data, "center_x")), nbt_float(child(data, "center_z"))) {
                    self.border_center = Some((x, z));
                }
            }
            "world_clocks" => {
                let ticks = child(data, "minecraft:overworld").and_then(|c| nbt_int(child(c, "total_ticks")));
                // L'horloge globale reste à 0 sous Paper (horloge par monde) : une valeur nulle
                // n'écrase pas une valeur déjà connue.
                if ticks.is_some_and(|t| t > 0) || self.day_time.is_none() {
                    self.day_time = ticks.or(self.day_time);
                }
            }
            _ => return,
        }
        self.split_files.push(name.to_string());
    }

    /// Le monde est au format 26.1+ : graine et règles ne sont plus dans `level.dat`.
    pub(crate) fn needs_split_files(&self) -> bool {
        self.seed.is_none() || self.game_rules.is_empty()
    }
}

/// Jour et heure (`hh:mm`) d'un `DayTime` : le tick 0 est 6 h du matin.
pub(crate) fn day_clock(ticks: i64) -> (i64, i64, i64) {
    let day = ticks.div_euclid(24_000);
    let t = ticks.rem_euclid(24_000);
    (day, (t / 1000 + 6) % 24, (t % 1000) * 60 / 1000)
}

/// Une dimension et ses régions.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DimensionStat {
    pub label: &'static str,
    /// Dossier `region` trouvé (`None` : dimension jamais générée).
    pub region_path: Option<String>,
    pub region_files: usize,
    pub region_bytes: u64,
    /// Dossier de la dimension et sa taille totale (régions, entités, POI…), si connue.
    pub folder: Option<String>,
    pub folder_bytes: Option<u64>,
}

/// Dossiers où chercher les régions de chaque dimension, dans l'ordre : format 26.1+
/// (`dimensions/minecraft/<dim>`), puis vanilla (`DIM-1`, `DIM1` dans le monde), puis Bukkit
/// (`<monde>_nether/DIM-1`, `<monde>_the_end/DIM1`). Renvoie `(libellé, [dossiers de dimension])`.
pub(crate) fn dimension_folders(world: &str, split: bool) -> Vec<(&'static str, Vec<String>)> {
    let modern = |d: &str| format!("/{world}/dimensions/minecraft/{d}");
    let mut out = vec![
        ("Surface (overworld)", vec![format!("/{world}")]),
        ("Nether", vec![format!("/{world}/DIM-1"), format!("/{world}_nether/DIM-1")]),
        ("End", vec![format!("/{world}/DIM1"), format!("/{world}_the_end/DIM1")]),
    ];
    if split {
        for ((_, list), dim) in out.iter_mut().zip(["overworld", "the_nether", "the_end"]) {
            list.insert(0, modern(dim));
        }
    }
    out
}

/// Nom de monde sûr (relatif à la racine du serveur, sans `..`).
pub(crate) fn clean_world(name: &str) -> Result<String, String> {
    let n = name.trim().replace('\\', "/");
    let n = n.trim_matches('/');
    if n.is_empty() || n.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
        return Err(format!("nom de monde invalide : « {name} »"));
    }
    Ok(n.to_string())
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "oui"
    } else {
        "non"
    }
}

/// Rapport texte d'un monde (pur).
pub(crate) fn format_world(
    world: &str,
    info: &LevelInfo,
    dims: &[DimensionStat],
    others: &[(String, u64)],
    total: Option<u64>,
    notes: &[String],
) -> String {
    use crate::server_files::human_bytes;
    let mut out = format!("Monde « {} » (/{world}/level.dat)", info.name.as_deref().unwrap_or(world));
    match (&info.version, info.data_version) {
        (Some(v), Some(d)) => {
            let _ = write!(out, " — Minecraft {v}{} (DataVersion {d})", if info.snapshot { " (snapshot)" } else { "" });
        }
        (Some(v), None) => {
            let _ = write!(out, " — Minecraft {v}");
        }
        (None, Some(d)) => {
            let _ = write!(out, " — DataVersion {d}");
        }
        _ => {}
    }
    if !info.brands.is_empty() {
        let _ = write!(out, " · serveur {}", info.brands.join(", "));
    }
    if info.modded == Some(true) {
        out.push_str(" (modifié)");
    }
    out.push('\n');
    let _ = writeln!(
        out,
        "Graine : {}{}",
        info.seed.map(|s| s.to_string()).unwrap_or_else(|| "inconnue".into()),
        info.structures.map(|s| format!(" · structures : {}", yes_no(s))).unwrap_or_default()
    );
    let mode = match info.game_type {
        Some(0) => "survie",
        Some(1) => "créatif",
        Some(2) => "aventure",
        Some(3) => "spectateur",
        _ => "inconnu",
    };
    let difficulty = info.difficulty.as_deref().map(|d| match d {
        "peaceful" => "paisible",
        "easy" => "facile",
        "normal" => "normale",
        "hard" => "difficile",
        other => other,
    });
    let _ = writeln!(
        out,
        "Mode : {mode} · difficulté : {}{} · hardcore : {} · commandes (triche) : {}",
        difficulty.unwrap_or("inconnue"),
        match info.difficulty_locked {
            Some(true) => " (verrouillée)",
            _ => "",
        },
        info.hardcore.map(yes_no).unwrap_or("?"),
        info.commands.map(yes_no).unwrap_or("?")
    );
    if let Some((x, y, z)) = info.spawn {
        let _ = writeln!(
            out,
            "Point d'apparition : {x}, {y}, {z}{}",
            info.spawn_dimension.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
        );
    }
    let mut time = Vec::new();
    if let Some(dt) = info.day_time {
        let (day, h, m) = day_clock(dt);
        time.push(format!("jour {day}, {h:02}:{m:02}"));
    }
    if let Some(t) = info.game_time {
        time.push(format!("âge {} jours de jeu ({t} ticks)", t / 24_000));
    }
    if !time.is_empty() {
        let _ = writeln!(out, "Temps : {}", time.join(" · "));
    }
    if let Some(ms) = info.last_played_ms {
        if let Some(d) = chrono::DateTime::from_timestamp_millis(ms) {
            let _ = writeln!(out, "Dernière partie : {}", d.format("%Y-%m-%d %H:%M UTC"));
        }
    }
    let weather = match (info.raining, info.thundering) {
        (_, Some(true)) => Some("orage"),
        (Some(true), _) => Some("pluie"),
        (Some(false), _) => Some("dégagé"),
        _ => None,
    };
    let mut env = Vec::new();
    if let Some(w) = weather {
        env.push(format!("Météo : {w}"));
    }
    if let Some(b) = info.border {
        let center = info.border_center.map(|(x, z)| format!(", centre ({x}, {z})")).unwrap_or_default();
        env.push(format!("Bordure : {b} blocs{center}"));
    }
    if !env.is_empty() {
        let _ = writeln!(out, "{}", env.join(" · "));
    }
    let _ = writeln!(
        out,
        "Datapacks activés ({}) : {}",
        info.datapacks_enabled.len(),
        if info.datapacks_enabled.is_empty() { "—".to_string() } else { info.datapacks_enabled.join(", ") }
    );
    let _ = writeln!(
        out,
        "Datapacks désactivés ({}) : {}",
        info.datapacks_disabled.len(),
        if info.datapacks_disabled.is_empty() { "—".to_string() } else { info.datapacks_disabled.join(", ") }
    );
    if info.game_rules.is_empty() {
        out.push_str("Règles de jeu : non trouvées\n");
    } else {
        let rules: Vec<String> = info.game_rules.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = writeln!(out, "Règles de jeu ({}) : {}", rules.len(), rules.join(", "));
    }
    out.push_str("Dimensions (dossiers region) :\n");
    for d in dims {
        match &d.region_path {
            None => {
                let _ = writeln!(out, "- {} : jamais générée (aucun dossier region)", d.label);
            }
            Some(path) => {
                let _ = write!(out, "- {} : {} fichier(s) de région, {} ({path})", d.label, d.region_files, human_bytes(d.region_bytes));
                if let Some(total) = d.folder_bytes {
                    let _ = write!(out, " — dossier de la dimension {}", human_bytes(total));
                }
                out.push('\n');
            }
        }
    }
    if !others.is_empty() {
        let list: Vec<String> = others.iter().map(|(n, b)| format!("{n} {}", human_bytes(*b))).collect();
        let _ = writeln!(out, "Autres dimensions ou mondes rangés dans le monde : {}", crate::findings::short_list(&list, 12));
    }
    if let Some(t) = total {
        let _ = writeln!(out, "Taille totale du dossier /{world} : {}", human_bytes(t));
    }
    if !info.split_files.is_empty() {
        let _ = writeln!(out, "Format 26.1+ : complété par data/minecraft/{{{}}}.dat", info.split_files.join(","));
    }
    for n in notes {
        let _ = writeln!(out, "Note : {n}");
    }
    out.trim_end().to_string()
}

/// Fichiers lus par SFTP pour `world_info`.
struct WorldFiles {
    info: LevelInfo,
    dims: Vec<DimensionStat>,
    notes: Vec<String>,
}

async fn read_nbt(conn: &crate::sftp::SftpConn, path: &str) -> crate::Result<Nbt> {
    let bytes = crate::sftp::read_bytes(conn, path, crate::NBT_CAP).await?;
    crate::nbt::parse(&bytes).map_err(crate::Error::Invalid)
}

/// Statistiques d'un dossier `region` : nombre de fichiers `.mca` et leur taille.
async fn region_stat(conn: &crate::sftp::SftpConn, dir: &str) -> crate::Result<Option<(usize, u64)>> {
    match crate::sftp::list(conn, &format!("{dir}/region")).await {
        Ok(entries) => {
            let mca: Vec<_> = entries.iter().filter(|e| !e.is_dir && e.name.ends_with(".mca")).collect();
            Ok(Some((mca.len(), mca.iter().map(|e| e.size).sum())))
        }
        Err(e) if crate::sftp::is_session_dead(&e) => Err(e),
        Err(_) => Ok(None),
    }
}

async fn collect_world(conn: &crate::sftp::SftpConn, world: &str) -> crate::Result<WorldFiles> {
    let mut notes = Vec::new();
    let level = match read_nbt(conn, &format!("/{world}/level.dat")).await {
        Ok(v) => v,
        Err(e) if crate::sftp::is_session_dead(&e) => return Err(e),
        Err(first) => match read_nbt(conn, &format!("/{world}/level.dat_old")).await {
            Ok(v) => {
                notes.push(format!("level.dat illisible ({first}) : lu dans level.dat_old (dernière sauvegarde précédente)."));
                v
            }
            Err(_) => {
                return Err(crate::Error::Invalid(format!(
                    "level.dat introuvable ou illisible dans /{world} ({first}) — vérifie le nom du monde (paramètre `world`)."
                )))
            }
        },
    };
    let mut info = LevelInfo::from_level_dat(&level).map_err(crate::Error::Invalid)?;
    let split = info.needs_split_files();
    if split {
        // Format 26.1+ : fichier par monde (Paper) d'abord, puis fichier global.
        let dirs = [
            format!("/{world}/dimensions/minecraft/overworld/data/minecraft"),
            format!("/{world}/data/minecraft"),
        ];
        let mut listed: Vec<Vec<String>> = Vec::new();
        for d in &dirs {
            listed.push(match crate::sftp::list(conn, d).await {
                Ok(entries) => entries.into_iter().map(|e| e.name).collect(),
                Err(e) if crate::sftp::is_session_dead(&e) => return Err(e),
                Err(_) => Vec::new(),
            });
        }
        for name in SPLIT_FILES {
            let file = format!("{name}.dat");
            let Some(dir) = dirs.iter().zip(&listed).find(|(_, l)| l.contains(&file)).map(|(d, _)| d) else {
                continue;
            };
            match read_nbt(conn, &format!("{dir}/{file}")).await {
                Ok(v) => info.apply_split(name, &v),
                Err(e) if crate::sftp::is_session_dead(&e) => return Err(e),
                Err(e) => notes.push(format!("{file} illisible : {e}")),
            }
        }
    }
    let mut dims = Vec::new();
    for (label, candidates) in dimension_folders(world, split) {
        let mut stat = DimensionStat { label, region_path: None, region_files: 0, region_bytes: 0, folder: None, folder_bytes: None };
        for dir in candidates {
            if let Some((files, bytes)) = region_stat(conn, &dir).await? {
                stat.region_path = Some(format!("{dir}/region"));
                stat.region_files = files;
                stat.region_bytes = bytes;
                // La surface au format ancien EST le monde : pas de « dossier de dimension » à part.
                stat.folder = (dir != format!("/{world}")).then_some(dir);
                break;
            }
        }
        dims.push(stat);
    }
    Ok(WorldFiles { info, dims, notes })
}

/// Résumé d'un monde (outil MCP `world_info`). Lecture seule. `world` : dossier du monde ; par
/// défaut `level-name` de `server.properties`.
pub(crate) async fn world_info(core: &Core, id: i64, world: Option<&str>) -> Result<String, String> {
    let caps = core.server_capabilities(id).await.map_err(|e| e.to_string())?;
    if !caps.is_minecraft_java() {
        return Ok(crate::server_audit::unsupported_game(&caps, "la lecture de level.dat"));
    }
    let name = match world.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => w.to_string(),
        None => core
            .server_properties(id)
            .await
            .ok()
            .and_then(|p| p.properties.get("level-name").cloned())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "world".to_string()),
    };
    let world = clean_world(&name)?;
    let w = world.clone();
    let files = core
        .with_sftp(id, |conn| {
            let w = w.clone();
            async move { collect_world(&conn, &w).await }
        })
        .await
        .map_err(|e| e.to_string())?;
    let WorldFiles { info, mut dims, notes } = files;

    // Tailles calculées par le serveur (endpoint d'espace disque) : total du monde, dossiers des
    // dimensions, autres dimensions ou mondes rangés dans `dimensions/`.
    let usage = |path: String| async move { core.disk_usage(id, &path, 100).await.ok() };
    let (total, minecraft_ns, namespaces) = tokio::join!(
        usage(world.clone()),
        usage(format!("{world}/dimensions/minecraft")),
        usage(format!("{world}/dimensions")),
    );
    let folder_sizes: Vec<Option<u64>> = futures_util::future::join_all(dims.iter().map(|d| {
        let folder = d.folder.clone();
        let from_ns = minecraft_ns.as_ref();
        async move {
            let folder = folder?;
            if let Some(item) = from_ns.and_then(|u| u.items.iter().find(|i| i.path == folder)) {
                return Some(item.size);
            }
            core.disk_usage(id, &folder, 1).await.ok().map(|u| u.total_size)
        }
    }))
    .await;
    for (d, size) in dims.iter_mut().zip(folder_sizes) {
        d.folder_bytes = size;
    }
    let mut others: Vec<(String, u64)> = Vec::new();
    if let Some(ns) = &minecraft_ns {
        for item in ns.items.iter().filter(|i| i.is_dir && !["overworld", "the_nether", "the_end"].contains(&i.name.as_str())) {
            others.push((format!("minecraft:{}", item.name), item.size));
        }
    }
    if let Some(ns) = &namespaces {
        for item in ns.items.iter().filter(|i| i.is_dir && i.name != "minecraft") {
            others.push((format!("{}:*", item.name), item.size));
        }
    }
    let text = format_world(&world, &info, &dims, &others, total.map(|u| u.total_size), &notes);
    Ok(core.redact_ai(&text))
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

    use fastnbt::Value as N;
    use std::collections::HashMap;

    fn c(entries: Vec<(&str, N)>) -> N {
        N::Compound(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>())
    }
    fn s(v: &str) -> N {
        N::String(v.into())
    }
    fn list(v: &[&str]) -> N {
        N::List(v.iter().map(|x| s(x)).collect())
    }

    /// Encode puis gzippe comme le jeu, et relit par `nbt::parse` (chemin réel de l'outil).
    fn roundtrip(v: &N) -> N {
        use std::io::Write as _;
        let raw = fastnbt::to_bytes(v).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        crate::nbt::parse(&enc.finish().unwrap()).unwrap()
    }

    /// `level.dat` d'un serveur Paper 26.1.2 réel (relevé du 2026-11-18, valeurs conservées).
    fn level_26_1() -> N {
        c(vec![(
            "Data",
            c(vec![
                ("Bukkit.Version", s("Paper/26.1.2-74-e4e17fc (MC: 26.1.2)/26.1.2.build.74-stable")),
                ("DataPacks", c(vec![("Disabled", list(&["minecart_improvements", "redstone_experiments", "trade_rebalance"])), ("Enabled", list(&["vanilla", "file/bukkit", "paper"]))])),
                ("DataVersion", N::Int(4790)),
                ("GameType", N::Int(0)),
                ("LastPlayed", N::Long(1_795_027_858_871)),
                ("LevelName", s("world")),
                ("ServerBrands", list(&["Paper"])),
                ("Time", N::Long(62_366_286)),
                ("Version", c(vec![("Id", N::Int(4790)), ("Name", s("26.1.2")), ("Series", s("main")), ("Snapshot", N::Byte(0))])),
                ("WasModded", N::Byte(1)),
                ("allowCommands", N::Byte(0)),
                ("difficulty_settings", c(vec![("difficulty", s("easy")), ("hardcore", N::Byte(0)), ("locked", N::Byte(0))])),
                ("initialized", N::Byte(1)),
                ("spawn", c(vec![("dimension", s("minecraft:overworld")), ("pitch", N::Float(0.0)), ("pos", N::IntArray(fastnbt::IntArray::new(vec![0, 103, 0]))), ("yaw", N::Float(0.0))])),
                ("version", N::Int(19133)),
            ]),
        )])
    }

    #[test]
    fn reads_a_real_26_1_world_and_its_split_files() {
        let mut info = LevelInfo::from_level_dat(&roundtrip(&level_26_1())).unwrap();
        assert_eq!(info.name.as_deref(), Some("world"));
        assert_eq!(info.version.as_deref(), Some("26.1.2"));
        assert_eq!(info.data_version, Some(4790));
        assert_eq!(info.difficulty.as_deref(), Some("easy"));
        assert_eq!(info.hardcore, Some(false));
        assert_eq!(info.spawn, Some((0, 103, 0)));
        assert_eq!(info.datapacks_enabled, vec!["vanilla", "file/bukkit", "paper"]);
        assert_eq!(info.datapacks_disabled.len(), 3);
        assert!(info.needs_split_files(), "graine et règles ne sont plus dans level.dat en 26.1");

        // Fichiers `dimensions/minecraft/overworld/data/minecraft/*.dat` du même serveur.
        let wgs = c(vec![("DataVersion", N::Int(4790)), ("data", c(vec![("bonus_chest", N::Byte(0)), ("generate_structures", N::Byte(1)), ("seed", N::Long(-7_991_860_693_441_649_226))]))]);
        let rules = c(vec![("data", c(vec![("minecraft:keep_inventory", N::Byte(0)), ("minecraft:pvp", N::Byte(1)), ("minecraft:random_tick_speed", N::Int(3))]))]);
        let weather = c(vec![("data", c(vec![("raining", N::Byte(0)), ("thundering", N::Byte(0)), ("rain_time", N::Int(46500))]))]);
        let border = c(vec![("data", c(vec![("center_x", N::Double(0.0)), ("center_z", N::Double(0.0)), ("size", N::Double(59_999_968.0))]))]);
        let clocks = c(vec![("data", c(vec![("minecraft:overworld", c(vec![("total_ticks", N::Long(63_472_244))])), ("minecraft:the_end", c(vec![("total_ticks", N::Long(62_366_286))]))]))]);
        for (name, v) in [("world_gen_settings", wgs), ("game_rules", rules), ("weather", weather), ("world_border", border), ("world_clocks", clocks)] {
            info.apply_split(name, &roundtrip(&v));
        }
        // L'horloge globale à 0 (Paper) n'écrase pas celle du monde.
        info.apply_split("world_clocks", &c(vec![("data", c(vec![("minecraft:overworld", c(vec![("total_ticks", N::Long(0))]))]))]));
        assert_eq!(info.seed, Some(-7_991_860_693_441_649_226));
        assert_eq!(info.structures, Some(true));
        assert_eq!(info.game_rules, vec![("keep_inventory".into(), "false".into()), ("pvp".into(), "true".into()), ("random_tick_speed".into(), "3".into())]);
        assert_eq!(info.day_time, Some(63_472_244));
        assert_eq!(info.border, Some(59_999_968.0));
        assert!(!info.needs_split_files());

        let dims = vec![
            DimensionStat { label: "Surface (overworld)", region_path: Some("/world/dimensions/minecraft/overworld/region".into()), region_files: 12, region_bytes: 31_000_000, folder: Some("/world/dimensions/minecraft/overworld".into()), folder_bytes: Some(75_804_672) },
            DimensionStat { label: "Nether", region_path: None, region_files: 0, region_bytes: 0, folder: None, folder_bytes: None },
        ];
        let text = format_world("world", &info, &dims, &[("minecraft:spawn".into(), 9_613_312)], Some(318_783_488), &[]);
        assert!(text.contains("Minecraft 26.1.2 (DataVersion 4790) · serveur Paper (modifié)"), "{text}");
        assert!(text.contains("Graine : -7991860693441649226 · structures : oui"), "{text}");
        assert!(text.contains("Mode : survie · difficulté : facile · hardcore : non · commandes (triche) : non"), "{text}");
        assert!(text.contains("Point d'apparition : 0, 103, 0 (minecraft:overworld)"), "{text}");
        assert!(text.contains("jour 2644, 22:14"), "{text}");
        assert!(text.contains("Dernière partie : 2026-11-18"), "{text}");
        assert!(text.contains("Météo : dégagé · Bordure : 59999968 blocs, centre (0, 0)"), "{text}");
        assert!(text.contains("Règles de jeu (3) : keep_inventory=false, pvp=true, random_tick_speed=3"), "{text}");
        assert!(text.contains("- Surface (overworld) : 12 fichier(s) de région, 29.6 Mio"), "{text}");
        assert!(text.contains("- Nether : jamais générée"), "{text}");
        assert!(text.contains("minecraft:spawn 9.2 Mio"), "{text}");
        assert!(text.contains("Format 26.1+"), "{text}");
    }

    #[test]
    fn reads_legacy_level_dat_layouts() {
        // 1.20 : graine dans WorldGenSettings, règles en texte, apparition en SpawnX/Y/Z.
        let modern_legacy = c(vec![(
            "Data",
            c(vec![
                ("LevelName", s("survie")),
                ("Version", c(vec![("Name", s("1.20.4")), ("Snapshot", N::Byte(0))])),
                ("DataVersion", N::Int(3700)),
                ("WorldGenSettings", c(vec![("seed", N::Long(42)), ("generate_features", N::Byte(1))])),
                ("GameRules", c(vec![("keepInventory", s("true")), ("doDaylightCycle", s("false"))])),
                ("Difficulty", N::Byte(3)),
                ("DifficultyLocked", N::Byte(1)),
                ("hardcore", N::Byte(1)),
                ("SpawnX", N::Int(10)),
                ("SpawnY", N::Int(64)),
                ("SpawnZ", N::Int(-5)),
                ("DayTime", N::Long(30_000)),
                ("raining", N::Byte(1)),
                ("thundering", N::Byte(1)),
                ("BorderSize", N::Double(1000.0)),
                ("BorderCenterX", N::Double(5.0)),
                ("BorderCenterZ", N::Double(-5.0)),
            ]),
        )]);
        let i = LevelInfo::from_level_dat(&roundtrip(&modern_legacy)).unwrap();
        assert_eq!(i.seed, Some(42));
        assert_eq!(i.difficulty.as_deref(), Some("hard"));
        assert_eq!((i.hardcore, i.difficulty_locked), (Some(true), Some(true)));
        assert_eq!(i.spawn, Some((10, 64, -5)));
        assert_eq!(i.game_rules[0], ("doDaylightCycle".to_string(), "false".to_string()));
        assert!(!i.needs_split_files());
        let text = format_world("survie", &i, &[], &[], None, &[]);
        assert!(text.contains("difficulté : difficile (verrouillée) · hardcore : oui"), "{text}");
        assert!(text.contains("jour 1, 12:00"), "{text}");
        assert!(text.contains("Météo : orage · Bordure : 1000 blocs, centre (5, -5)"), "{text}");

        // ≤ 1.15 : RandomSeed et MapFeatures.
        let old = c(vec![("Data", c(vec![("RandomSeed", N::Long(-1)), ("MapFeatures", N::Byte(0)), ("GameType", N::Int(1))]))]);
        let o = LevelInfo::from_level_dat(&old).unwrap();
        assert_eq!((o.seed, o.structures, o.game_type), (Some(-1), Some(false), Some(1)));
        assert!(LevelInfo::from_level_dat(&s("x")).is_err());
    }

    #[test]
    fn day_clock_and_world_names() {
        assert_eq!(day_clock(0), (0, 6, 0));
        assert_eq!(day_clock(18_000), (0, 0, 0));
        assert_eq!(day_clock(24_000 * 3 + 6_500), (3, 12, 30));
        assert_eq!(clean_world("/world/").unwrap(), "world");
        assert_eq!(clean_world("worlds/main").unwrap(), "worlds/main");
        assert!(clean_world("../etc").is_err());
        assert!(clean_world("  ").is_err());
    }

    #[test]
    fn dimension_folders_cover_all_layouts() {
        let legacy = dimension_folders("world", false);
        assert_eq!(legacy[0].1, vec!["/world"]);
        assert_eq!(legacy[1].1, vec!["/world/DIM-1", "/world_nether/DIM-1"]);
        let split = dimension_folders("world", true);
        assert_eq!(split[1].1[0], "/world/dimensions/minecraft/the_nether");
        assert_eq!(split[2].1, vec!["/world/dimensions/minecraft/the_end", "/world/DIM1", "/world_the_end/DIM1"]);
    }
}
