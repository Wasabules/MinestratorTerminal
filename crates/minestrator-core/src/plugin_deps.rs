//! Contrôle des **dépendances des plugins et mods** d'un serveur Minecraft Java (outil MCP
//! `check_plugin_deps`) : chaque `.jar` de `plugins/` et `mods/` est lu à la demande (descripteurs
//! seulement, voir `jar_meta.rs`), puis des règles pures signalent :
//! - les dépendances obligatoires manquantes (ou présentes mais désactivées), les optionnelles
//!   absentes (info), les versions de dépendance hors contrainte ;
//! - une version de Minecraft ou d'API déclarée incompatible avec celle du serveur
//!   (`GET /server/{id}/minecraft_version`) ;
//! - les doublons, les jars d'une autre plateforme (plugin Velocity sur Paper, mod Forge sur
//!   Fabric…) ou rangés dans le mauvais dossier, les mods côté client, l'absence de prise en
//!   charge de Folia ;
//! - les fichiers désactivés (`.disabled`…) et les jars illisibles.
//!
//! La plateforme du serveur vient de l'hébergeur (`source` : `paper`, `fabric`…) ; quand il ne la
//! donne pas (`explicit_jar`), elle est déduite du contenu, et la sortie le dit. Lecture seule.

use crate::findings::{self, short_list, Finding, Severity};
use crate::jar_meta::{self, compare_versions, version_nums, DepKind, Descriptor, JarError, JarMeta, Platform, ZipProbe};
use crate::server_files::human_bytes;
use crate::{sftp, Core, Result};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Jars examinés au plus (au-delà : listés comme non analysés).
pub(crate) const MAX_JARS: usize = 400;
/// Octets lus au plus pour tout le contrôle (fins d'archive, répertoires centraux, descripteurs).
pub(crate) const READ_BUDGET: u64 = 192 * 1024 * 1024;
/// Jars lus en même temps sur la session SFTP.
const CONCURRENCY: usize = 6;
/// Lectures au plus par jar (fin, zip64, répertoire, quelques descripteurs).
const MAX_STEPS: usize = 16;

/// Dossier d'un jar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Folder {
    Plugins,
    Mods,
}

impl Folder {
    pub(crate) fn dir(self) -> &'static str {
        match self {
            Folder::Plugins => "plugins",
            Folder::Mods => "mods",
        }
    }
}

/// Fichier de `plugins/` ou `mods/` à examiner : `Some(false)` jar actif, `Some(true)` jar
/// désactivé (non chargé : `.disabled`, `.jar_off`, `.old`, `.bak`…), `None` autre fichier.
pub(crate) fn classify(name: &str) -> Option<bool> {
    let l = name.to_ascii_lowercase();
    if l.ends_with(".jar") {
        return Some(false);
    }
    let disabled_suffixes = [".disabled", ".disable", "_off", ".off", ".old", ".bak", ".backup"];
    (l.contains(".jar") && disabled_suffixes.iter().any(|s| l.ends_with(s))).then_some(true)
}

/// Un jar examiné.
#[derive(Debug)]
pub(crate) struct ScannedJar {
    pub folder: Folder,
    pub file: String,
    pub disabled: bool,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Read(JarMeta),
    /// Archive illisible (corrompue, tronquée, pas un zip).
    Unreadable(String),
    /// Non analysé : plafond atteint.
    Skipped(String),
}

/// Ce que l'hébergeur dit du serveur.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerInfo {
    pub mc_version: Option<String>,
    /// Plateforme détectée (`paper`, `fabric`…), `explicit_jar` ou vide.
    pub source: String,
}

/// Plateformes que le serveur charge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Accepted {
    pub platforms: BTreeSet<Platform>,
    pub folia: bool,
    /// Déduites du contenu (l'hébergeur ne nomme pas la plateforme).
    pub inferred: bool,
    /// Serveur vanilla : ni plugins ni mods.
    pub vanilla: bool,
}

impl Accepted {
    fn label(&self) -> String {
        if self.vanilla {
            return "vanilla (ni plugins ni mods)".into();
        }
        if self.platforms.is_empty() {
            return "inconnue".into();
        }
        let mut names: Vec<&str> = self.platforms.iter().map(|p| p.label()).collect();
        if self.folia {
            names.insert(0, "Folia");
        }
        names.join(" + ")
    }

    fn mods(&self) -> bool {
        self.platforms.iter().any(|p| !p.is_plugin())
    }
}

/// Fourches de Paper : chargent `plugin.yml` et `paper-plugin.yml`.
const PAPER_FORKS: &[&str] = &[
    "paper", "purpur", "pufferfish", "leaf", "leaves", "canvas", "gale", "divinemc", "sakura", "mirai",
    "airplane", "tuinity", "plazma", "kaiiju", "scissors",
];

/// Plateformes d'après la `source` de l'hébergeur ; `None` si elle n'en nomme pas
/// (`explicit_jar`, vide).
pub(crate) fn platforms_from_source(source: &str, mc: Option<&str>) -> Option<Accepted> {
    use Platform::*;
    let s = source.to_ascii_lowercase();
    // `proxyvelocity`, `proxybungeecord`… (noms de l'hébergeur pour ses proxys) : le mot compte
    // aussi sans le préfixe.
    let words: BTreeSet<&str> = s
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .flat_map(|w| [Some(w), w.strip_prefix("proxy").filter(|r| !r.is_empty())])
        .flatten()
        .collect();
    let has = |list: &[&str]| list.iter().any(|w| words.contains(w));
    let mc_nums = mc.and_then(version_nums);
    let below = |v: &[u64]| mc_nums.as_ref().is_some_and(|m| crate::updates::compare_numbers(m, v) == Ordering::Less);
    let forge = if below(&[1, 13]) { LegacyForge } else { Forge };
    let mut folia = false;
    let mut vanilla = false;
    let list: Vec<Platform> = if has(&["mohist", "magma", "catserver", "ketting", "crucible", "thermos", "cauldron"]) {
        vec![forge, Bukkit]
    } else if has(&["youer"]) {
        vec![NeoForge, Bukkit]
    } else if has(&["arclight"]) {
        vec![Forge, NeoForge, Fabric, Bukkit]
    } else if has(&["banner", "cardboard"]) {
        vec![Fabric, Bukkit]
    } else if has(&["neoforge"]) {
        // NeoForge 1.20.1 est une fourche de Forge et charge ses mods.
        if below(&[1, 20, 2]) {
            vec![NeoForge, Forge]
        } else {
            vec![NeoForge]
        }
    } else if has(&["forge"]) {
        vec![forge]
    } else if has(&["quilt"]) {
        vec![Quilt, Fabric]
    } else if has(&["fabric"]) {
        vec![Fabric]
    } else if has(&["folia"]) {
        folia = true;
        vec![Bukkit, Paper]
    } else if has(PAPER_FORKS) {
        vec![Bukkit, Paper]
    } else if has(&["spigot", "bukkit", "craftbukkit"]) {
        vec![Bukkit]
    } else if has(&["velocity"]) {
        vec![Velocity]
    } else if has(&["bungeecord", "bungee", "waterfall", "flamecord", "travertine", "lightfall"]) {
        vec![Bungee]
    } else if has(&["vanilla"]) {
        vanilla = true;
        Vec::new()
    } else {
        return None;
    };
    Some(Accepted { platforms: list.into_iter().collect(), folia, inferred: false, vanilla })
}

/// Plateformes déduites du contenu : la famille la plus représentée parmi les jars actifs de
/// `plugins/` (Bukkit, Velocity, BungeeCord) et celle de `mods/` (Fabric/Quilt, Forge, NeoForge).
pub(crate) fn infer_platforms(jars: &[ScannedJar]) -> Accepted {
    use Platform::*;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut quilt_only = false;
    for jar in jars.iter().filter(|j| !j.disabled) {
        let Outcome::Read(meta) = &jar.outcome else { continue };
        let set: BTreeSet<Platform> = meta.descriptors.iter().map(|d| d.platform).collect();
        let mut families = BTreeSet::new();
        for p in &set {
            families.insert(match p {
                Bukkit | Paper => "bukkit",
                Velocity => "velocity",
                Bungee => "bungee",
                Fabric | Quilt => "fabric",
                Forge => "forge",
                NeoForge => "neoforge",
                LegacyForge => "legacy",
            });
        }
        quilt_only |= set.contains(&Quilt) && !set.contains(&Fabric);
        for f in families {
            *counts.entry(f).or_default() += 1;
        }
    }
    let best = |families: &[&'static str]| {
        families
            .iter()
            .filter_map(|f| counts.get(f).map(|n| (*n, *f)))
            .filter(|(n, _)| *n > 0)
            .max_by_key(|(n, _)| *n)
            .map(|(_, f)| f)
    };
    let mut platforms = BTreeSet::new();
    match best(&["bukkit", "velocity", "bungee"]) {
        Some("bukkit") => platforms.extend([Bukkit, Paper]),
        Some("velocity") => {
            platforms.insert(Velocity);
        }
        Some("bungee") => {
            platforms.insert(Bungee);
        }
        _ => {}
    }
    match best(&["fabric", "forge", "neoforge", "legacy"]) {
        Some("fabric") if quilt_only => platforms.extend([Quilt, Fabric]),
        Some("fabric") => {
            platforms.insert(Fabric);
        }
        Some("forge") => {
            platforms.insert(Forge);
        }
        Some("neoforge") => {
            platforms.insert(NeoForge);
        }
        Some("legacy") => {
            platforms.insert(LegacyForge);
        }
        _ => {}
    }
    Accepted { platforms, folia: false, inferred: true, vanilla: false }
}

/// Descripteurs qu'utilise la plateforme du serveur : Paper préfère `paper-plugin.yml`, Quilt
/// `quilt.mod.json`, NeoForge `neoforge.mods.toml` quand le jar porte les deux.
fn effective<'a>(descriptors: &'a [Descriptor], accepted: &BTreeSet<Platform>) -> Vec<&'a Descriptor> {
    use Platform::*;
    let has = |p: Platform| descriptors.iter().any(|d| d.platform == p) && accepted.contains(&p);
    descriptors
        .iter()
        .filter(|d| accepted.contains(&d.platform))
        .filter(|d| !(d.platform == Bukkit && has(Paper)))
        .filter(|d| !(d.platform == Fabric && has(Quilt)))
        .filter(|d| !(d.platform == Forge && has(NeoForge)))
        .collect()
}

/// Gravité d'un échec de chargement : un plugin manque seul, un mod empêche le démarrage.
fn load_failure(p: Platform) -> Severity {
    if p.is_plugin() {
        Severity::Important
    } else {
        Severity::Critical
    }
}

fn consequence(p: Platform) -> &'static str {
    if p.is_plugin() {
        "le plugin ne se chargera pas"
    } else {
        "le chargeur refusera de démarrer le serveur"
    }
}

/// Dépendance fournie par le chargeur ou le jeu lui-même.
fn builtin(id: &str, accepted: &Accepted) -> bool {
    use Platform::*;
    let p = &accepted.platforms;
    match id {
        "minecraft" | "java" => accepted.mods(),
        "fabricloader" | "fabric-loader" => p.contains(&Fabric) || p.contains(&Quilt),
        "quilt_loader" => p.contains(&Quilt),
        "forge" | "fml" | "javafml" => p.contains(&Forge) || p.contains(&LegacyForge),
        "mcp" => p.contains(&LegacyForge),
        "neoforge" => p.contains(&NeoForge),
        "velocity" => p.contains(&Velocity),
        _ => false,
    }
}

/// Un identifiant présent sur le serveur.
#[derive(Debug, Clone)]
struct Present {
    file: String,
    version: String,
    /// Identifiant propre (pas un alias `provides`) : compte pour les doublons.
    primary: bool,
}

/// Résultat de l'analyse.
#[derive(Debug)]
pub(crate) struct Analysis {
    pub accepted: Accepted,
    pub findings: Vec<Finding>,
    /// Inventaire (une ligne par descripteur effectif).
    pub inventory: Vec<String>,
}

fn subject(jar: &ScannedJar, d: &Descriptor) -> String {
    format!("« {}/{} » ({})", jar.folder.dir(), jar.file, d.label())
}

/// Applique les règles (pur).
pub(crate) fn analyze(server: &ServerInfo, jars: &[ScannedJar]) -> Analysis {
    let accepted = platforms_from_source(&server.source, server.mc_version.as_deref()).unwrap_or_else(|| infer_platforms(jars));
    let mc = server.mc_version.as_deref();
    let mut findings = Vec::new();
    let mut inventory = Vec::new();
    let mut present: BTreeMap<String, Vec<Present>> = BTreeMap::new();
    let mut nested: BTreeSet<String> = BTreeSet::new();
    let mut disabled_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut disabled_files = Vec::new();
    let mut skipped = Vec::new();
    let mut loaded: Vec<(&ScannedJar, &Descriptor)> = Vec::new();

    for jar in jars {
        let path = format!("{}/{}", jar.folder.dir(), jar.file);
        if jar.disabled {
            disabled_files.push(path.clone());
        }
        let meta = match &jar.outcome {
            Outcome::Unreadable(reason) => {
                if !jar.disabled {
                    findings.push(Finding::new(
                        Severity::Important,
                        format!("« {path} » illisible : {reason}."),
                        "Archive corrompue ou incomplète : retélécharge-la (un jar corrompu peut empêcher le démarrage).",
                    ));
                }
                continue;
            }
            Outcome::Skipped(reason) => {
                skipped.push(format!("{path} ({reason})"));
                continue;
            }
            Outcome::Read(meta) => meta,
        };
        if jar.disabled {
            for d in &meta.descriptors {
                disabled_ids.entry(jar_meta::norm_id(&d.id)).or_insert_with(|| path.clone());
            }
            continue;
        }
        for p in &meta.problems {
            findings.push(Finding::new(
                Severity::Info,
                format!("« {path} » : descripteur {p}."),
                "S'il ne se charge pas au démarrage, retélécharge ce jar depuis sa source officielle.",
            ));
        }
        let eff = effective(&meta.descriptors, &accepted.platforms);
        if eff.is_empty() {
            if accepted.vanilla {
                findings.push(Finding::new(
                    Severity::Info,
                    format!("« {path} » : un serveur vanilla ne charge ni plugins ni mods."),
                    "Installe une plateforme (Paper, Fabric, NeoForge…) ou retire ce fichier.",
                ));
            } else if !meta.descriptors.is_empty() {
                let mut kinds: Vec<&str> = meta.descriptors.iter().map(|d| d.platform.label()).collect();
                kinds.dedup();
                findings.push(Finding::new(
                    Severity::Important,
                    format!(
                        "« {path} » ({}) est fait pour {} : la plateforme du serveur ({}) ne le charge pas.",
                        meta.descriptors[0].label(),
                        kinds.join(" / "),
                        accepted.label()
                    ),
                    format!("Remplace-le par sa version {} ou retire-le.", accepted.label()),
                ));
            } else if meta.problems.is_empty() {
                findings.push(Finding::new(
                    Severity::Info,
                    format!("« {path} » : aucun descripteur de plugin ni de mod reconnu (bibliothèque, ou format non géré)."),
                    format!("Vérifie qu'il a sa place dans {}/ ; sinon retire-le.", jar.folder.dir()),
                ));
            }
            continue;
        }
        for d in eff {
            if d.client_only {
                // Un serveur ne charge pas un mod côté client : ni présent, ni contrôlé.
                findings.push(Finding::new(
                    Severity::Info,
                    format!("{} est un mod côté client uniquement : le serveur l'ignore.", subject(jar, d)),
                    "Retire-le du serveur (il reste utile côté joueurs).",
                ));
                continue;
            }
            if d.platform.is_plugin() != (jar.folder == Folder::Plugins) {
                let right = if d.platform.is_plugin() { "plugins" } else { "mods" };
                findings.push(Finding::new(
                    Severity::Important,
                    format!("{} est un {} rangé dans {}/ : il n'y sera pas chargé.", subject(jar, d), d.platform.label(), jar.folder.dir()),
                    format!("Déplace-le dans {right}/."),
                ));
                continue;
            }
            let id = jar_meta::norm_id(&d.id);
            present.entry(id).or_default().push(Present { file: path.clone(), version: d.version.clone(), primary: true });
            for alias in &d.provides {
                present
                    .entry(jar_meta::norm_id(alias))
                    .or_default()
                    .push(Present { file: path.clone(), version: d.version.clone(), primary: false });
            }
            if !d.platform.is_plugin() {
                nested.extend(meta.nested_ids.iter().cloned());
            }
            loaded.push((jar, d));
        }
    }

    // Doublons : un même identifiant propre dans plusieurs fichiers.
    for (id, list) in &present {
        let mut files: Vec<&str> = list.iter().filter(|p| p.primary).map(|p| p.file.as_str()).collect();
        files.sort_unstable();
        files.dedup();
        if files.len() < 2 {
            continue;
        }
        let platform = loaded
            .iter()
            .find(|(_, d)| jar_meta::norm_id(&d.id) == *id)
            .map(|(_, d)| d.platform)
            .unwrap_or(Platform::Bukkit);
        let why = if platform.is_plugin() {
            "un seul sera chargé (nom ambigu)"
        } else {
            "le chargeur refuse les doublons et le serveur ne démarre pas"
        };
        findings.push(Finding::new(
            load_failure(platform),
            format!("« {id} » est présent dans {} fichiers ({}) : {why}.", files.len(), files.join(", ")),
            "Garde un seul fichier (le plus récent) et supprime les autres.",
        ));
    }

    let nested_has = |id: &str| {
        nested.iter().any(|n| {
            n == id || n.replace('-', "_") == id || n.replace('_', "-") == id || n.starts_with(&format!("{id}-"))
        })
    };

    for (jar, d) in &loaded {
        let who = subject(jar, d);
        let mut optional_missing = Vec::new();
        for dep in &d.deps {
            if dep.client_only {
                continue;
            }
            let id = jar_meta::norm_id(&dep.id);
            if id == jar_meta::norm_id(&d.id) {
                continue;
            }
            let found = present.get(&id);
            if builtin(&id, &accepted) {
                if id == "minecraft" && dep.kind == DepKind::Required {
                    if let Some(mc) = mc {
                        if dep.req.satisfied_by(mc) == Some(false) {
                            findings.push(Finding::new(
                                Severity::Critical,
                                format!("{who} exige Minecraft {}, le serveur est en {mc} : {}.", dep.req.describe(), consequence(d.platform)),
                                format!("Installe la version de ce mod prévue pour Minecraft {mc}, ou retire-le."),
                            ));
                        }
                    }
                }
                continue;
            }
            match dep.kind {
                DepKind::Required => {
                    if let Some(list) = found {
                        let p = &list[0];
                        if !p.version.is_empty() && dep.req.satisfied_by(&p.version) == Some(false) {
                            findings.push(Finding::new(
                                load_failure(d.platform),
                                format!(
                                    "{who} exige « {} » {}, présent en {} ({}) : {}.",
                                    dep.id,
                                    dep.req.describe(),
                                    p.version,
                                    p.file,
                                    consequence(d.platform)
                                ),
                                format!("Mets « {} » à jour, ou installe une version de {} compatible.", dep.id, d.name),
                            ));
                        }
                    } else if nested_has(&id) {
                        // Fournie par un jar imbriqué d'un autre mod.
                    } else if let Some(file) = disabled_ids.get(&id) {
                        findings.push(Finding::new(
                            load_failure(d.platform),
                            format!("{who} dépend de « {} », présent mais désactivé ({file}) : {}.", dep.id, consequence(d.platform)),
                            format!("Réactive {file} (retire le suffixe de désactivation) ou retire {}.", d.name),
                        ));
                    } else {
                        let req = dep.req.describe();
                        findings.push(Finding::new(
                            load_failure(d.platform),
                            format!(
                                "{who} : dépendance obligatoire « {} »{} absente — {}.",
                                dep.id,
                                if req.is_empty() { String::new() } else { format!(" ({req})") },
                                consequence(d.platform)
                            ),
                            format!(
                                "Installe « {} »{} (market_search) ou retire {}.",
                                dep.id,
                                if req.is_empty() { String::new() } else { format!(" en version {req}") },
                                d.name
                            ),
                        ));
                    }
                }
                DepKind::Optional => {
                    if found.is_none() && !nested_has(&id) {
                        optional_missing.push(dep.id.clone());
                    }
                }
                DepKind::Breaks | DepKind::Conflicts => {
                    let Some(list) = found else { continue };
                    let p = &list[0];
                    // Version inconnue face à une contrainte précise : on ne conclut pas.
                    if dep.req.satisfied_by(&p.version) != Some(true) {
                        continue;
                    }
                    let (sev, effect) = if dep.kind == DepKind::Breaks {
                        (Severity::Critical, "le chargeur refusera de démarrer le serveur")
                    } else {
                        (Severity::Important, "le chargeur les déconseille ensemble")
                    };
                    findings.push(Finding::new(
                        sev,
                        format!("{who} se déclare incompatible avec « {} » {} (présent : {}) : {effect}.", dep.id, dep.req.describe(), p.file),
                        "Retire l'un des deux, ou installe des versions compatibles entre elles.",
                    ));
                }
            }
        }
        if !optional_missing.is_empty() {
            findings.push(Finding::new(
                Severity::Info,
                format!("{who} : dépendance(s) optionnelle(s) absente(s) : {} — fonctions associées inactives.", short_list(&optional_missing, 8)),
                "",
            ));
        }
        if let (Some(api), Some(mc)) = (&d.api_version, mc) {
            if matches!(d.platform, Platform::Bukkit | Platform::Paper) && compare_versions(api, mc) == Some(Ordering::Greater) {
                findings.push(Finding::new(
                    Severity::Important,
                    format!("{who} déclare api-version {api}, plus récente que le serveur ({mc}) : il sera refusé au chargement."),
                    format!("Installe une version du plugin compatible avec Minecraft {mc}, ou mets le serveur à jour."),
                ));
            }
        }
        if let (Some(target), Some(mc)) = (&d.mc_version, mc) {
            if compare_versions(target, mc).is_some_and(|o| o != Ordering::Equal) {
                findings.push(Finding::new(
                    Severity::Important,
                    format!("{who} est prévu pour Minecraft {target}, le serveur est en {mc}."),
                    format!("Installe la version de ce mod prévue pour Minecraft {mc}."),
                ));
            }
        }
        if accepted.folia && matches!(d.platform, Platform::Bukkit | Platform::Paper) && !d.folia {
            findings.push(Finding::new(
                Severity::Important,
                format!("{who} n'annonce pas la prise en charge de Folia (folia-supported) : Folia refusera de le charger."),
                "Cherche une version compatible Folia, ou passe le serveur sur Paper.",
            ));
        }
        inventory.push(inventory_line(jar, d));
    }

    if !disabled_files.is_empty() {
        findings.push(Finding::new(
            Severity::Info,
            format!("{} fichier(s) désactivé(s), non chargé(s) : {}.", disabled_files.len(), short_list(&disabled_files, 15)),
            "Supprime-les s'ils ne servent plus ; retire le suffixe pour en réactiver un.",
        ));
    }
    if !skipped.is_empty() {
        findings.push(Finding::new(
            Severity::Info,
            format!("{} jar(s) non analysé(s) (plafond) : {}.", skipped.len(), short_list(&skipped, 10)),
            "Vérifie-les à la main (list_archive / read_archive_entry) si besoin.",
        ));
    }
    Analysis { accepted, findings, inventory }
}

fn inventory_line(jar: &ScannedJar, d: &Descriptor) -> String {
    let mut line = format!("{}/{} : {} [{}", jar.folder.dir(), jar.file, d.label(), d.platform.label());
    if let Some(api) = &d.api_version {
        let _ = write!(line, ", api {api}");
    }
    line.push(']');
    let list = |kind: DepKind| {
        d.deps
            .iter()
            .filter(|x| x.kind == kind && !x.client_only)
            .map(|x| {
                let r = x.req.describe();
                if r.is_empty() || r == "*" {
                    x.id.clone()
                } else {
                    format!("{} {r}", x.id)
                }
            })
            .collect::<Vec<_>>()
    };
    let (req, opt) = (list(DepKind::Required), list(DepKind::Optional));
    if !req.is_empty() {
        let _ = write!(line, " — dépend de : {}", short_list(&req, 8));
    }
    if !opt.is_empty() {
        let _ = write!(line, " — optionnel : {}", short_list(&opt, 8));
    }
    line
}

/// État d'un dossier examiné.
#[derive(Debug, Clone)]
pub(crate) struct FolderNote {
    pub folder: Folder,
    /// `None` : dossier absent ou illisible.
    pub jars: Option<usize>,
}

/// Rapport texte complet.
pub(crate) fn render(server: &ServerInfo, folders: &[FolderNote], jars: &[ScannedJar], bytes_read: u64, analysis: &Analysis, details: bool) -> String {
    let mut out = String::new();
    let mc = server.mc_version.as_deref().unwrap_or("inconnue");
    let _ = write!(out, "Dépendances des plugins et mods — Minecraft {mc} · plateforme : {}", analysis.accepted.label());
    if analysis.accepted.inferred {
        let src = if server.source.is_empty() { "rien".to_string() } else { format!("« {} »", server.source) };
        let _ = write!(out, " (déduite du contenu : l'hébergeur indique {src})");
    }
    out.push('\n');
    let folder_text: Vec<String> = folders
        .iter()
        .map(|f| match f.jars {
            Some(n) => format!("{}/ {n}", f.folder.dir()),
            None => format!("{}/ absent", f.folder.dir()),
        })
        .collect();
    let count = |pred: &dyn Fn(&ScannedJar) -> bool| jars.iter().filter(|j| pred(j)).count();
    let _ = writeln!(
        out,
        "{} fichier(s) examiné(s) ({}) : {} désactivé(s), {} illisible(s), {} non analysé(s).",
        jars.len(),
        folder_text.join(", "),
        count(&|j| j.disabled),
        count(&|j| matches!(j.outcome, Outcome::Unreadable(_))),
        count(&|j| matches!(j.outcome, Outcome::Skipped(_)))
    );
    let _ = writeln!(
        out,
        "Lecture à la demande (fin d'archive, répertoire central et descripteurs, jamais le jar entier) : {} lus. Plafonds : {MAX_JARS} jars, répertoire central {}, descripteur {}, {} lus au total.",
        human_bytes(bytes_read),
        human_bytes(jar_meta::MAX_DIRECTORY),
        human_bytes(jar_meta::MAX_ENTRY),
        human_bytes(READ_BUDGET)
    );
    if jars.is_empty() {
        out.push_str("Aucun plugin ni mod (.jar) dans plugins/ ni mods/.");
        return out;
    }
    let _ = writeln!(out, "Bilan : {}.", findings::tally(&analysis.findings));
    findings::render(&analysis.findings, "Aucun problème de dépendance détecté.", &mut out);
    if details {
        const CAP: usize = 200;
        let _ = writeln!(out, "\nInventaire ({} élément(s) chargé(s)) :", analysis.inventory.len());
        for line in analysis.inventory.iter().take(CAP) {
            let _ = writeln!(out, "- {line}");
        }
        if analysis.inventory.len() > CAP {
            let _ = writeln!(out, "… (+{})", analysis.inventory.len() - CAP);
        }
    }
    out.trim_end().to_string()
}

// --- Collecte (SFTP) -------------------------------------------------------------

/// Jar à lire.
#[derive(Debug, Clone)]
struct Candidate {
    folder: Folder,
    file: String,
    path: String,
    size: u64,
    disabled: bool,
}

async fn list_folders(conn: &sftp::SftpConn) -> Result<(Vec<Candidate>, Vec<FolderNote>)> {
    let mut candidates = Vec::new();
    let mut notes = Vec::new();
    for folder in [Folder::Plugins, Folder::Mods] {
        match sftp::list(conn, &format!("/{}", folder.dir())).await {
            Ok(entries) => {
                let before = candidates.len();
                for e in entries.into_iter().filter(|e| !e.is_dir) {
                    if let Some(disabled) = classify(&e.name) {
                        candidates.push(Candidate { folder, file: e.name, path: e.path, size: e.size, disabled });
                    }
                }
                notes.push(FolderNote { folder, jars: Some(candidates.len() - before) });
            }
            Err(e) if sftp::is_session_dead(&e) => return Err(e),
            Err(_) => notes.push(FolderNote { folder, jars: None }),
        }
    }
    // Les jars actifs d'abord : ce sont eux que le plafond doit couvrir.
    candidates.sort_by_key(|c| (c.disabled, c.folder, c.file.to_lowercase()));
    Ok((candidates, notes))
}

/// Lit un jar à la demande. `Err` seulement si la session SFTP est morte (à rejouer).
async fn scan_one(conn: &sftp::SftpConn, c: &Candidate, budget: &AtomicU64) -> Result<ScannedJar> {
    let mut probe = ZipProbe::new(c.size);
    let mut file: Option<sftp::RemoteFile> = None;
    let mut outcome = Outcome::Unreadable("lecture interrompue (trop d'étapes)".into());
    for _ in 0..MAX_STEPS {
        let (offset, len) = match probe.next_read() {
            Err(JarError::Corrupt(m)) => {
                outcome = Outcome::Unreadable(m);
                break;
            }
            Err(JarError::Limit(m)) => {
                outcome = Outcome::Skipped(m);
                break;
            }
            Ok(None) => {
                outcome = match probe.finish() {
                    Ok(files) => Outcome::Read(jar_meta::read_meta(&files)),
                    Err(e) => Outcome::Unreadable(e.to_string()),
                };
                break;
            }
            Ok(Some(range)) => range,
        };
        if budget.fetch_add(len, AtomicOrdering::Relaxed) + len > READ_BUDGET {
            outcome = Outcome::Skipped(format!("budget de lecture de {} atteint", human_bytes(READ_BUDGET)));
            break;
        }
        if file.is_none() {
            match sftp::open_read(conn, &c.path).await {
                Ok(f) => file = Some(f),
                Err(e) if sftp::is_session_dead(&e) => return Err(e),
                Err(e) => {
                    outcome = Outcome::Unreadable(format!("ouverture impossible ({e})"));
                    break;
                }
            }
        }
        let Some(f) = file.as_mut() else { break };
        match f.read_at(offset, len as usize).await {
            Ok(bytes) if bytes.is_empty() => {
                outcome = Outcome::Unreadable("fichier plus court que prévu".into());
                break;
            }
            Ok(bytes) => probe.feed(offset, bytes),
            Err(e) if sftp::is_session_dead(&e) => return Err(e),
            Err(e) => {
                outcome = Outcome::Unreadable(e.to_string());
                break;
            }
        }
    }
    Ok(ScannedJar { folder: c.folder, file: c.file.clone(), disabled: c.disabled, outcome })
}

async fn scan_all(conn: &sftp::SftpConn, candidates: &[Candidate], budget: &AtomicU64) -> Result<Vec<ScannedJar>> {
    use futures_util::stream::{self, StreamExt};
    // Futures construites d'abord : une fermeture dans le flux rendrait l'appelant non `Send`
    // (limite connue de rustc sur les durées de vie d'ordre supérieur).
    let futures: Vec<_> = candidates.iter().map(|c| Box::pin(scan_one(conn, c, budget))).collect();
    let results: Vec<Result<ScannedJar>> = stream::iter(futures).buffered(CONCURRENCY).collect().await;
    results.into_iter().collect()
}

impl Core {
    /// Contrôle des dépendances des plugins et mods (outil MCP `check_plugin_deps`). Lecture seule.
    pub(crate) async fn check_plugin_deps(&self, id: i64, details: bool) -> Result<String> {
        let caps = self.server_capabilities(id).await?;
        if !caps.is_minecraft_java() {
            return Ok(crate::server_audit::unsupported_game(&caps, "le contrôle des dépendances"));
        }
        let token = self.token()?;
        let mc = self.api.minecraft_version(&token, id).await.ok();
        let server = ServerInfo {
            mc_version: mc.as_ref().map(|m| m.version.clone()).filter(|v| !v.is_empty()),
            source: mc.map(|m| m.source).unwrap_or_default(),
        };
        let (candidates, notes) = self.with_sftp(id, |conn| async move { list_folders(&conn).await }).await?;
        let (to_scan, over) = candidates.split_at(candidates.len().min(MAX_JARS));
        // Une session morte en cours de route : rejouée une fois sur une session neuve.
        let mut attempt = 0;
        let (mut jars, bytes) = loop {
            let budget = AtomicU64::new(0);
            let conn = self.sftp.ensure(&self.api, &token, id).await?;
            match scan_all(&conn, to_scan, &budget).await {
                Ok(jars) => break (jars, budget.load(AtomicOrdering::Relaxed)),
                Err(e) if attempt == 0 && sftp::is_session_dead(&e) => {
                    self.sftp.drop_session(id);
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        };
        jars.extend(over.iter().map(|c| ScannedJar {
            folder: c.folder,
            file: c.file.clone(),
            disabled: c.disabled,
            outcome: Outcome::Skipped(format!("au-delà des {MAX_JARS} premiers jars")),
        }));
        let analysis = analyze(&server, &jars);
        Ok(self.redact_ai(&render(&server, &notes, &jars, bytes, &analysis, details)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jar_meta::tests::{drive, make_jar};

    fn jar(folder: Folder, file: &str, entries: &[(&str, &[u8])]) -> ScannedJar {
        let owned: Vec<(&str, &[u8], bool)> = entries.iter().map(|(n, d)| (*n, *d, true)).collect();
        let bytes = make_jar(&owned);
        let (files, _) = drive(&bytes);
        ScannedJar {
            folder,
            file: file.into(),
            disabled: classify(file) == Some(true),
            outcome: Outcome::Read(jar_meta::read_meta(&files.unwrap())),
        }
    }

    fn plugin(file: &str, yml: &str) -> ScannedJar {
        jar(Folder::Plugins, file, &[("plugin.yml", yml.as_bytes())])
    }

    fn fabric_mod(file: &str, json: &str) -> ScannedJar {
        jar(Folder::Mods, file, &[("fabric.mod.json", json.as_bytes())])
    }

    fn paper(mc: &str) -> ServerInfo {
        ServerInfo { mc_version: Some(mc.into()), source: "paper".into() }
    }

    fn messages(a: &Analysis, sev: Severity) -> Vec<String> {
        a.findings.iter().filter(|f| f.severity == sev).map(|f| f.message.clone()).collect()
    }

    #[test]
    fn classifies_active_and_disabled_files() {
        assert_eq!(classify("LuckPerms-Bukkit-5.5.71.jar"), Some(false));
        assert_eq!(classify("Old.jar.disabled"), Some(true));
        assert_eq!(classify("x.jar_off"), Some(true));
        assert_eq!(classify("x.JAR.bak"), Some(true));
        assert_eq!(classify("config.yml"), None);
        assert_eq!(classify("notes.disabled"), None);
    }

    #[test]
    fn platform_comes_from_the_host_or_the_content() {
        let p = platforms_from_source("paper", Some("1.21.1")).unwrap();
        assert!(p.platforms.contains(&Platform::Paper) && p.platforms.contains(&Platform::Bukkit));
        assert_eq!(platforms_from_source("neoforge", Some("1.20.1")).unwrap().platforms.len(), 2);
        assert_eq!(platforms_from_source("neoforge", Some("1.21.1")).unwrap().platforms.len(), 1);
        assert!(platforms_from_source("forge", Some("1.12.2")).unwrap().platforms.contains(&Platform::LegacyForge));
        assert!(platforms_from_source("folia", None).unwrap().folia);
        assert!(platforms_from_source("vanilla", None).unwrap().vanilla);
        // Noms de l'hébergeur pour ses proxys (relevé réel : `proxyvelocity`).
        assert!(platforms_from_source("proxyvelocity", None).unwrap().platforms.contains(&Platform::Velocity));
        assert!(platforms_from_source("proxybungeecord", None).unwrap().platforms.contains(&Platform::Bungee));
        assert!(platforms_from_source("proxy", None).is_none());
        assert!(platforms_from_source("explicit_jar", Some("26.1.2")).is_none());

        // Relevé réel du 2026-11-18 : `explicit_jar` sur un serveur Paper 26.1.2 → déduit du contenu.
        let jars = vec![
            plugin("LuckPerms-Bukkit-5.5.71.jar", "name: LuckPerms\nversion: 5.5.71\napi-version: 1.13\n"),
            plugin("VaultUnlocked-2.20.3.jar", "name: Vault\nversion: 2.20.3\n"),
        ];
        let a = analyze(&ServerInfo { mc_version: Some("26.1.2".into()), source: "explicit_jar".into() }, &jars);
        assert!(a.accepted.inferred);
        assert!(a.accepted.platforms.contains(&Platform::Bukkit));
        assert!(a.findings.is_empty(), "{:?}", a.findings);
    }

    #[test]
    fn missing_disabled_and_optional_dependencies() {
        let jars = vec![
            plugin("EssentialsChat.jar", "name: EssentialsChat\nversion: 2.21.0\ndepend: [Essentials]\nsoftdepend: [Vault, LuckPerms]\n"),
            plugin("Shop.jar", "name: Shop\nversion: 1\ndepend: [Vault]\n"),
            plugin("Vault.jar.disabled", "name: Vault\nversion: 1.7\n"),
        ];
        let a = analyze(&paper("1.21.1"), &jars);
        let important = messages(&a, Severity::Important);
        assert!(important.iter().any(|m| m.contains("EssentialsChat") && m.contains("« Essentials »") && m.contains("absente")), "{important:?}");
        assert!(important.iter().any(|m| m.contains("Shop") && m.contains("désactivé") && m.contains("Vault.jar.disabled")), "{important:?}");
        let info = messages(&a, Severity::Info);
        assert!(info.iter().any(|m| m.contains("optionnelle") && m.contains("Vault, LuckPerms")), "{info:?}");
        assert!(info.iter().any(|m| m.contains("1 fichier(s) désactivé(s)")), "{info:?}");
        assert!(messages(&a, Severity::Critical).is_empty());
    }

    #[test]
    fn bukkit_names_match_case_insensitively_and_through_provides() {
        let jars = vec![
            plugin("A.jar", "name: A\nversion: 1\ndepend: [vault, Some Lib]\n"),
            plugin("VaultUnlocked.jar", "name: VaultUnlocked\nversion: 2\nprovides: [Vault]\n"),
            plugin("Lib.jar", "name: Some_Lib\nversion: 1\n"),
        ];
        let a = analyze(&paper("1.21.1"), &jars);
        assert!(a.findings.is_empty(), "{:?}", a.findings);
    }

    #[test]
    fn api_version_newer_than_the_server_is_refused() {
        let jars = vec![plugin("New.jar", "name: New\nversion: 1\napi-version: '1.21'\n"), plugin("Old.jar", "name: Old\nversion: 1\napi-version: 1.13\n")];
        let a = analyze(&paper("1.20.4"), &jars);
        let imp = messages(&a, Severity::Important);
        assert_eq!(imp.len(), 1, "{imp:?}");
        assert!(imp[0].contains("api-version 1.21") && imp[0].contains("1.20.4"));
        // Versions annuelles (26.x) : 1.21 reste plus ancien.
        assert!(analyze(&paper("26.1.2"), &jars).findings.is_empty());
    }

    #[test]
    fn duplicates_other_platforms_and_wrong_folders() {
        let velocity = jar(Folder::Plugins, "LuckPerms-Velocity-5.5.71.jar", &[("velocity-plugin.json", br#"{"id":"luckperms","name":"LuckPerms","version":"5.5.71"}"#)]);
        let jars = vec![
            plugin("Dup-1.jar", "name: Dup\nversion: 1\n"),
            plugin("Dup-2.jar", "name: Dup\nversion: 2\n"),
            velocity,
            jar(Folder::Mods, "Misplaced.jar", &[("plugin.yml", b"name: Misplaced\nversion: 1\n")]),
            jar(Folder::Plugins, "lib.jar", &[("a.txt", b"x")]),
        ];
        let a = analyze(&paper("1.21.1"), &jars);
        let imp = messages(&a, Severity::Important);
        assert!(imp.iter().any(|m| m.contains("« dup »") && m.contains("2 fichiers")), "{imp:?}");
        assert!(imp.iter().any(|m| m.contains("LuckPerms-Velocity") && m.contains("Velocity") && m.contains("ne le charge pas")), "{imp:?}");
        assert!(imp.iter().any(|m| m.contains("Misplaced") && m.contains("rangé dans mods/")), "{imp:?}");
        assert!(messages(&a, Severity::Info).iter().any(|m| m.contains("lib.jar") && m.contains("aucun descripteur")));
    }

    #[test]
    fn fabric_rules_minecraft_versions_and_nested_jars() {
        let api = jar(
            Folder::Mods,
            "fabric-api-0.92.2+1.20.1.jar",
            &[
                ("fabric.mod.json", br#"{"id":"fabric-api","version":"0.92.2+1.20.1","provides":["fabric"],"depends":{"fabricloader":">=0.14.22","minecraft":"~1.20.1"}}"#),
                ("META-INF/jars/fabric-api-base-0.4.31+ef105b4977.jar", b"j"),
            ],
        );
        let jars = vec![
            api,
            fabric_mod("lithium.jar", r#"{"id":"lithium","version":"0.11.2","depends":{"minecraft":">=1.20 <1.21"}}"#),
            fabric_mod("sodium.jar", r#"{"id":"sodium","version":"0.5.11","environment":"client","depends":{"minecraft":">=1.21","absent":"*"}}"#),
            fabric_mod("needs-base.jar", r#"{"id":"needs_base","version":"1","depends":{"fabric-api-base":"*","fabric":">=0.90"}}"#),
            fabric_mod("too-new.jar", r#"{"id":"too_new","version":"1","depends":{"minecraft":">=1.21","fabric-api":">=0.100"}}"#),
            fabric_mod("missing.jar", r#"{"id":"missing_dep","version":"1","depends":{"cloth-config":">=11"},"breaks":{"lithium":"*","sodium":"*"}}"#),
            fabric_mod("dup-a.jar", r#"{"id":"dup","version":"1"}"#),
            fabric_mod("dup-b.jar", r#"{"id":"dup","version":"2"}"#),
        ];
        let server = ServerInfo { mc_version: Some("1.20.1".into()), source: "fabric".into() };
        let a = analyze(&server, &jars);
        let crit = messages(&a, Severity::Critical);
        assert!(crit.iter().any(|m| m.contains("too-new.jar") && m.contains("exige Minecraft >=1.21")), "{crit:?}");
        assert!(crit.iter().any(|m| m.contains("too-new.jar") && m.contains("« fabric-api » >=0.100") && m.contains("0.92.2")), "{crit:?}");
        assert!(crit.iter().any(|m| m.contains("missing.jar") && m.contains("cloth-config") && m.contains("(>=11)")), "{crit:?}");
        assert!(crit.iter().any(|m| m.contains("missing.jar") && m.contains("incompatible avec « lithium »")), "{crit:?}");
        // Un mod côté client n'est ni chargé ni contrôlé : pas de faux conflit ni de dépendance.
        assert!(!crit.iter().any(|m| m.contains("sodium")), "{crit:?}");
        assert!(crit.iter().any(|m| m.contains("« dup »")), "{crit:?}");
        assert!(!crit.iter().any(|m| m.contains("needs-base.jar")), "fourni par un jar imbriqué et par provides : {crit:?}");
        assert!(messages(&a, Severity::Info).iter().any(|m| m.contains("sodium.jar") && m.contains("côté client")));
    }

    #[test]
    fn forge_mod_on_a_fabric_server_and_vanilla_servers() {
        let forge = jar(
            Folder::Mods,
            "jei-forge.jar",
            &[("META-INF/mods.toml", b"modLoader=\"javafml\"\nloaderVersion=\"[47,)\"\nlicense=\"MIT\"\n[[mods]]\nmodId=\"jei\"\nversion=\"15.2\"\n")],
        );
        let jars = vec![forge];
        let a = analyze(&ServerInfo { mc_version: Some("1.20.1".into()), source: "fabric".into() }, &jars);
        assert!(messages(&a, Severity::Important).iter().any(|m| m.contains("jei-forge.jar") && m.contains("Forge")));
        let v = analyze(&ServerInfo { mc_version: Some("1.21".into()), source: "vanilla".into() }, &jars);
        assert!(v.findings.iter().any(|f| f.message.contains("vanilla")));
    }

    #[test]
    fn folia_requires_an_explicit_flag() {
        let jars = vec![
            plugin("A.jar", "name: A\nversion: 1\nfolia-supported: true\n"),
            plugin("B.jar", "name: B\nversion: 1\n"),
        ];
        let a = analyze(&ServerInfo { mc_version: Some("1.21.4".into()), source: "folia".into() }, &jars);
        let imp = messages(&a, Severity::Important);
        assert_eq!(imp.len(), 1);
        assert!(imp[0].contains("B.jar") && imp[0].contains("Folia"));
    }

    #[test]
    fn unreadable_and_skipped_jars_are_reported() {
        let jars = vec![
            ScannedJar { folder: Folder::Plugins, file: "broken.jar".into(), disabled: false, outcome: Outcome::Unreadable("pas une archive zip".into()) },
            ScannedJar { folder: Folder::Mods, file: "huge.jar".into(), disabled: false, outcome: Outcome::Skipped("plafond".into()) },
        ];
        let a = analyze(&paper("1.21.1"), &jars);
        assert!(messages(&a, Severity::Important).iter().any(|m| m.contains("broken.jar") && m.contains("illisible")));
        assert!(messages(&a, Severity::Info).iter().any(|m| m.contains("huge.jar") && m.contains("non analysé")));
        let text = render(&paper("1.21.1"), &[FolderNote { folder: Folder::Plugins, jars: Some(1) }, FolderNote { folder: Folder::Mods, jars: None }], &jars, 1234, &a, true);
        assert!(text.contains("Minecraft 1.21.1"), "{text}");
        assert!(text.contains("plugins/ 1, mods/ absent"), "{text}");
        assert!(text.contains("1 illisible(s), 1 non analysé(s)"), "{text}");
        assert!(text.contains("Plafonds : 400 jars"), "{text}");
    }

    #[test]
    fn inventory_lists_loaded_descriptors() {
        let jars = vec![plugin("A.jar", "name: A\nversion: 1\napi-version: 1.20\ndepend: [B]\nsoftdepend: [C]\n"), plugin("B.jar", "name: B\nversion: 2\n")];
        let a = analyze(&paper("1.21.1"), &jars);
        let text = render(&paper("1.21.1"), &[], &jars, 0, &a, true);
        assert!(text.contains("- plugins/A.jar : A 1 [Bukkit/Spigot, api 1.20] — dépend de : B — optionnel : C"), "{text}");
        assert!(!render(&paper("1.21.1"), &[], &jars, 0, &a, false).contains("Inventaire"));
    }
}
