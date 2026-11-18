//! Mises à jour des mods, plugins et datapacks Minecraft installés, via l'API MineStrator.
//!
//! Tout passe par des endpoints **non documentés** que le panel web utilise (relevé du 2026-11-18,
//! code public du panel et appels réels).
//!
//! **Vérification** — `POST /server/{id}/{mods|plugins|datapacks}/updates` : une pure comparaison
//! côté hébergeur, sans effet de bord. Le panel l'appelle tout seul à l'ouverture de ses onglets
//! Mods, Plugins et Datapacks, puis à chaque changement de la version détectée, sans rien demander ;
//! il n'en garde qu'une table « clé → mise à jour » pour afficher un bouton, et applique une mise à
//! jour par d'autres appels. Corps : `{ items, mc_version }`, avec des éléments `{filename, sha1}`
//! (plugins), `{filename, sha1, loader}` (mods) ou `{name}` (datapacks). Réponse :
//! `{ updates: { clé: { has_update, source, target } }, updates_available }`, la clé étant le nom
//! de fichier (le nom pour un datapack). Liste vide → `API_MISSING_REQUIRED_FIELDS` ; version
//! absente → `API_INVALID_MC_VERSION`. La version vient de `GET /server/{id}/minecraft_version`.
//!
//! **Application** — comme le panel : installer la cible avec les endpoints d'installation, puis
//! supprimer l'ancien fichier si le nom change (sinon l'installation l'a déjà remplacé) :
//! - plugins : `POST /server/{id}/modrinth` `{slug, version_id}` (l'appel de l'installation depuis
//!   le catalogue), puis `DELETE /server/{id}/plugin/{fichier}` ;
//! - mods : `POST /server/{id}/modrinth/mod` `{slug, version_id, loader}` ou
//!   `POST /server/{id}/curseforge/mod` `{modId, fileId, loader}`, puis `DELETE /server/{id}/mod/{fichier}` ;
//! - datapacks : `POST /server/{id}/modrinth/datapack` `{slug, version_id}` ou
//!   `POST /server/{id}/curseforge/datapack` `{modId, fileId}`, puis
//!   `POST /server/{id}/files/delete` `{files: ["<monde>/datapacks/<nom>"]}`.
//!
//! **Garde-fou** — l'hébergeur propose parfois comme « mise à jour » la variante d'une autre
//! plateforme, à version égale. Relevé du 2026-11-18 sur un serveur Paper :
//! `LuckPerms-Bukkit-5.5.71.jar` → `LuckPerms-Velocity-5.5.71.jar` et
//! `packetevents-spigot-2.14.0.jar` → `packetevents-velocity-2.14.0.jar` (le corps des plugins ne
//! porte aucun loader). Appliquer remplacerait un plugin qui marche par un jar de proxy. Chaque
//! proposition passe donc par [`assess`] : plateforme différente, version identique ou plus
//! ancienne → écartée et signalée comme telle, jamais appliquée.

use crate::api::ApiClient;
use crate::error::{Error, ErrorPayload, Result};
use crate::models::InstalledItem;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// --- Types exposés ---------------------------------------------------------

/// Type de contenu Minecraft géré par l'hébergeur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ContentKind {
    Mods,
    Plugins,
    Datapacks,
}

impl ContentKind {
    /// Tous les types, dans l'ordre d'affichage.
    pub const ALL: [ContentKind; 3] = [ContentKind::Mods, ContentKind::Plugins, ContentKind::Datapacks];

    /// Segment d'URL de l'API (`/server/{id}/<segment>`), aussi forme JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::Mods => "mods",
            ContentKind::Plugins => "plugins",
            ContentKind::Datapacks => "datapacks",
        }
    }

    /// Lecture souple (singulier ou pluriel, casse ignorée), pour les arguments MCP.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mod" | "mods" => Some(ContentKind::Mods),
            "plugin" | "plugins" => Some(ContentKind::Plugins),
            "datapack" | "datapacks" => Some(ContentKind::Datapacks),
            _ => None,
        }
    }
}

/// Mise à jour applicable d'un élément installé.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContentUpdate {
    pub kind: ContentKind,
    /// Clé de l'élément installé : nom de fichier (mods, plugins) ou nom du datapack. Sert à
    /// l'associer à la liste des installés et à demander son application.
    pub key: String,
    pub name: String,
    /// Désactivé sur le serveur : la nouvelle version, elle, sera installée active.
    pub enabled: bool,
    /// Version installée (vide si l'hébergeur ne la connaît pas, ex. datapacks).
    pub current_version: String,
    pub new_version: String,
    /// Nom du fichier qui remplacera l'actuel.
    pub new_filename: String,
    /// `modrinth` | `curseforge`.
    pub source: String,
}

/// Pourquoi une proposition de l'hébergeur n'est pas appliquée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum IgnoreReason {
    /// Variante d'une autre plateforme (Velocity pour un plugin Paper, Forge pour un mod Fabric…).
    OtherPlatform,
    /// Même numéro de version que l'installé.
    SameVersion,
    /// Numéro de version plus ancien que l'installé.
    OlderVersion,
    /// Cible incomplète : rien à installer.
    Incomplete,
    /// Plus de mise à jour au moment de l'appliquer (déjà à jour).
    UpToDate,
}

/// Proposition de l'hébergeur écartée par le garde-fou local.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IgnoredUpdate {
    pub kind: ContentKind,
    pub key: String,
    pub name: String,
    pub current_version: String,
    pub new_version: String,
    pub new_filename: String,
    pub reason: IgnoreReason,
}

/// Échec de la vérification d'un type (les autres types restent valables).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateCheckError {
    pub kind: ContentKind,
    pub error: ErrorPayload,
}

/// Résultat de la vérification des mises à jour d'un serveur.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContentUpdates {
    /// Version de Minecraft détectée par l'hébergeur ; `null` : inconnue, rien n'a pu être vérifié.
    pub mc_version: Option<String>,
    /// Plateforme détectée (`paper`, `fabric`…) quand l'hébergeur la donne.
    pub platform: Option<String>,
    /// Horodatage de la vérification (secondes Unix).
    pub checked_at: i64,
    /// Éléments installés soumis à l'hébergeur.
    pub examined: usize,
    /// Éléments que l'hébergeur n'a pas identifiés (projet perso, source inconnue) : leurs mises à
    /// jour ne peuvent pas être suivies.
    pub untracked: usize,
    pub updates: Vec<ContentUpdate>,
    pub ignored: Vec<IgnoredUpdate>,
    pub errors: Vec<UpdateCheckError>,
}

/// Élément à mettre à jour (argument de [`crate::Core::apply_content_updates`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateSelection {
    pub kind: ContentKind,
    pub key: String,
}

/// Issue de la mise à jour d'un élément.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum UpdateStatus {
    /// Nouvelle version installée, ancien fichier remplacé ou supprimé.
    Updated,
    /// Nouvelle version installée, mais l'ancien fichier n'a pas pu être supprimé : les deux
    /// coexistent, à supprimer avant de redémarrer.
    OldKept,
    /// Installation refusée : rien n'a changé.
    Failed,
    /// Non appliquée (voir `reason`).
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateOutcome {
    pub kind: ContentKind,
    pub key: String,
    pub name: String,
    pub from_version: String,
    pub to_version: String,
    pub status: UpdateStatus,
    pub reason: Option<IgnoreReason>,
    pub error: Option<ErrorPayload>,
}

/// Bilan d'une mise à jour groupée.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateReport {
    pub outcomes: Vec<UpdateOutcome>,
    /// Au moins un fichier a changé : le serveur doit redémarrer pour charger les nouvelles versions.
    pub restart_needed: bool,
}

// --- Réponses brutes, lues avec tolérance ----------------------------------

/// Chaîne depuis une valeur JSON quelconque (nombre accepté, `null`/absent → vide), telle quelle :
/// un nom de fichier sert de clé et doit rester exact.
fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Entier depuis un nombre ou une chaîne numérique.
fn int(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Booléen depuis `true`/`false`, `0`/`1` ou `"1"`/`"true"`.
fn flag(v: Option<&Value>, default: bool) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().map_or(default, |n| n != 0),
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true"),
        _ => default,
    }
}

/// Éléments d'une liste JSON ; un objet (tableau associatif PHP) donne ses valeurs, autre chose rien.
fn entries(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Object(m)) => m.values().collect(),
        _ => Vec::new(),
    }
}

/// Version de Minecraft détectée par l'hébergeur (`GET /server/{id}/minecraft_version`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct McVersion {
    pub version: String,
    /// `paper`, `fabric`… ou `explicit_jar` (jar désigné explicitement, plateforme inconnue).
    pub source: String,
}

impl McVersion {
    pub(crate) fn from_value(data: &Value) -> Self {
        McVersion { version: text(data.get("version")), source: text(data.get("source")) }
    }

    /// Plateforme du serveur quand `source` en nomme une connue.
    fn platform(&self) -> Option<String> {
        let s = self.source.to_ascii_lowercase();
        (!platforms_in(&s).is_empty()).then_some(s)
    }
}

/// Élément installé, normalisé (champs utiles à la vérification et à l'application).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Installed {
    pub kind: ContentKind,
    /// Nom de fichier (mods, plugins) ou nom du datapack.
    pub key: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub sha1: String,
    pub loader: String,
}

/// Liste des installés d'un type (`GET /server/{id}/{mods|plugins|datapacks}`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ContentList {
    pub items: Vec<Installed>,
    /// Dossier du monde (datapacks : `<monde>/datapacks/<nom>`).
    pub level_name: String,
}

impl ContentList {
    pub(crate) fn from_value(kind: ContentKind, data: &Value) -> Self {
        let items = entries(data.get(kind.as_str()))
            .into_iter()
            .filter_map(|v| {
                let name = text(v.get("name"));
                let filename = text(v.get("filename"));
                let key = match kind {
                    ContentKind::Datapacks => name.clone(),
                    _ => filename,
                };
                (!key.is_empty()).then(|| Installed {
                    kind,
                    name: if name.is_empty() { key.clone() } else { name },
                    key,
                    version: text(v.get("version")),
                    enabled: flag(v.get("enabled"), true),
                    sha1: text(v.get("sha1")),
                    loader: text(v.get("loader")),
                })
            })
            .collect();
        ContentList { items, level_name: text(data.get("level_name")) }
    }
}

/// Cible d'une mise à jour, telle que proposée par l'hébergeur.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Target {
    pub filename: String,
    /// Modrinth : slug ou identifiant du projet.
    pub slug: String,
    pub version_id: String,
    pub version_number: String,
    pub display_name: String,
    /// CurseForge.
    pub mod_id: Option<i64>,
    pub file_id: Option<i64>,
}

impl Target {
    fn from_value(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let t = Target {
            filename: text(o.get("filename")),
            slug: text(o.get("slug")),
            version_id: text(o.get("version_id")),
            version_number: text(o.get("version_number")),
            display_name: text(o.get("display_name")),
            mod_id: int(o.get("mod_id")),
            file_id: int(o.get("file_id")),
        };
        (t != Target::default()).then_some(t)
    }

    /// Version lisible de la cible.
    fn label(&self) -> String {
        [&self.version_number, &self.display_name, &self.filename]
            .into_iter()
            .find(|s| !s.is_empty())
            .cloned()
            .unwrap_or_default()
    }
}

/// Une entrée de la réponse de vérification.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Proposal {
    pub has_update: bool,
    /// `modrinth` | `curseforge` ; vide : élément non identifié par l'hébergeur.
    pub source: String,
    pub target: Option<Target>,
}

/// Table « clé → proposition » d'une réponse de vérification. Un `updates` vide arrive en `[]`
/// (tableau PHP) : toute forme autre qu'un objet donne une table vide.
pub(crate) fn parse_proposals(data: &Value) -> HashMap<String, Proposal> {
    let Some(map) = data.get("updates").and_then(Value::as_object) else {
        return HashMap::new();
    };
    map.iter()
        .map(|(key, v)| {
            let p = Proposal {
                has_update: flag(v.get("has_update"), false),
                source: text(v.get("source")).to_ascii_lowercase(),
                target: v.get("target").and_then(Target::from_value),
            };
            (key.clone(), p)
        })
        .collect()
}

/// Corps de la vérification d'un type, tel que l'envoie le panel ; `None` si aucun élément ne s'y
/// prête (l'API refuserait une liste vide).
pub(crate) fn check_body(kind: ContentKind, items: &[Installed], mc_version: &str) -> Option<Value> {
    let list: Vec<Value> = items
        .iter()
        .map(|i| match kind {
            ContentKind::Plugins => json!({ "filename": i.key, "sha1": i.sha1 }),
            ContentKind::Mods => json!({ "filename": i.key, "sha1": i.sha1, "loader": i.loader }),
            ContentKind::Datapacks => json!({ "name": i.key }),
        })
        .collect();
    (!list.is_empty() && !mc_version.trim().is_empty())
        .then(|| json!({ "items": list, "mc_version": mc_version.trim() }))
}

// --- Garde-fou : plateforme et numéro de version ---------------------------

/// Familles de plateformes et les mots qui les désignent dans un nom de fichier ou de version.
const PLATFORMS: &[(&str, &[&str])] = &[
    ("bukkit", &["bukkit", "spigot", "paper", "purpur", "folia", "pufferfish"]),
    ("velocity", &["velocity"]),
    ("bungeecord", &["bungeecord", "bungee", "waterfall"]),
    ("sponge", &["sponge", "spongeforge", "spongevanilla"]),
    ("fabric", &["fabric"]),
    ("quilt", &["quilt"]),
    ("forge", &["forge"]),
    ("neoforge", &["neoforge"]),
];

/// Familles de plateformes nommées dans un texte (mots séparés par tout caractère non
/// alphanumérique : `LuckPerms-Velocity-5.5.71.jar`, `2.14.0+velocity`).
pub(crate) fn platforms_in(text: &str) -> BTreeSet<&'static str> {
    let lower = text.to_ascii_lowercase();
    let words: BTreeSet<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    PLATFORMS
        .iter()
        .filter(|(_, names)| names.iter().any(|n| words.contains(n)))
        .map(|(family, _)| *family)
        .collect()
}

/// Familles qu'une plateforme sait charger (Quilt charge aussi les mods Fabric).
fn loadable_by(family: &str) -> &'static [&'static str] {
    match family {
        "quilt" => &["quilt", "fabric"],
        "bukkit" => &["bukkit"],
        "velocity" => &["velocity"],
        "bungeecord" => &["bungeecord"],
        "sponge" => &["sponge"],
        "fabric" => &["fabric"],
        "forge" => &["forge"],
        "neoforge" => &["neoforge"],
        _ => &[],
    }
}

/// La cible vise-t-elle une autre plateforme ? La référence est la plateforme nommée dans le
/// fichier installé, à défaut `fallback` (loader du mod, plateforme du serveur). Sans référence ou
/// sans plateforme nommée dans la cible : pas de conflit (rien ne permet de conclure).
pub(crate) fn platform_conflict(current: &str, fallback: &str, target: &str) -> bool {
    let wanted = platforms_in(target);
    if wanted.is_empty() {
        return false;
    }
    let mut reference = platforms_in(current);
    if reference.is_empty() {
        reference = platforms_in(fallback);
    }
    if reference.is_empty() {
        return false;
    }
    let loadable: BTreeSet<&str> = reference.iter().flat_map(|f| loadable_by(f).iter().copied()).collect();
    wanted.is_disjoint(&loadable)
}

/// Nombres d'une chaîne de version, dans l'ordre, après retrait de la version de Minecraft qui y
/// est souvent embarquée (`sodium-fabric-0.5.8+mc1.20.1`, `1.20.1-47.2.0`, `mod-1.21-2.0`). Une
/// suite pointée égale à `mc_version`, ou à l'un de ses préfixes (`1.21` pour `1.21.1`), est retirée.
pub(crate) fn version_numbers(text: &str, mc_version: &str) -> Vec<u64> {
    let mc = mc_version.trim();
    let is_mc = |run: &str| {
        !mc.is_empty() && run.contains('.') && (run == mc || mc.starts_with(&format!("{run}.")))
    };
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // Suite maximale `chiffres(.chiffres)*`.
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_digit()
                || (chars[i] == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())))
        {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        if !is_mc(&run) {
            out.extend(run.split('.').map(|n| n.parse::<u64>().unwrap_or(u64::MAX)));
        }
    }
    out
}

/// Comparaison de deux suites de nombres, complétées par des zéros (`1.0` = `1.0.0`).
pub(crate) fn compare_numbers(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0)))
        .find(|o| *o != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

/// Nom sans extension d'archive (`.jar`, `.jar_off`, `.zip`, `.disabled`).
fn stem(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".jar.disabled", ".jar_off", ".disabled", ".jar", ".zip"] {
        if lower.ends_with(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// Verdict du garde-fou sur une proposition de l'hébergeur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Apply,
    Ignore(IgnoreReason),
}

/// Juge une proposition : complète, même plateforme et numéro de version plus récent. Quand l'un
/// des deux numéros est illisible, la proposition de l'hébergeur est retenue.
pub(crate) fn assess(
    item: &Installed,
    source: &str,
    target: &Target,
    mc_version: &str,
    server_platform: &str,
) -> Verdict {
    if install_call(item.kind, source, target, &item.loader).is_none() {
        return Verdict::Ignore(IgnoreReason::Incomplete);
    }
    let fallback = match item.kind {
        ContentKind::Mods => item.loader.as_str(),
        ContentKind::Plugins => server_platform,
        ContentKind::Datapacks => "",
    };
    let current_text = format!("{} {}", item.key, item.version);
    let target_text = format!("{} {} {}", target.filename, target.version_number, target.display_name);
    if platform_conflict(&current_text, fallback, &target_text) {
        return Verdict::Ignore(IgnoreReason::OtherPlatform);
    }
    let current = if item.version.is_empty() { stem(&item.key) } else { item.version.as_str() };
    let current = version_numbers(current, mc_version);
    let label = target.label();
    let wanted = version_numbers(stem(&label), mc_version);
    if !current.is_empty() && !wanted.is_empty() {
        match compare_numbers(&wanted, &current) {
            Ordering::Equal => return Verdict::Ignore(IgnoreReason::SameVersion),
            Ordering::Less => return Verdict::Ignore(IgnoreReason::OlderVersion),
            Ordering::Greater => {}
        }
    }
    Verdict::Apply
}

// --- Installation et suppression ------------------------------------------

/// Appel d'installation de la nouvelle version (mêmes endpoints et corps que le panel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallCall {
    /// `POST /modrinth` — l'appel de l'installation d'un plugin depuis le catalogue.
    ModrinthPlugin { slug: String, version_id: String },
    /// `POST /modrinth/mod`.
    ModrinthMod { slug: String, version_id: String, loader: String },
    /// `POST /curseforge/mod`.
    CurseforgeMod { mod_id: i64, file_id: i64, loader: String },
    /// `POST /modrinth/datapack`.
    ModrinthDatapack { slug: String, version_id: String },
    /// `POST /curseforge/datapack`.
    CurseforgeDatapack { mod_id: i64, file_id: i64 },
}

/// Appel d'installation d'une cible, `None` si elle est incomplète. Les plugins passent toujours par
/// Modrinth (le panel ignore `source` pour eux).
pub(crate) fn install_call(kind: ContentKind, source: &str, t: &Target, loader: &str) -> Option<InstallCall> {
    let modrinth = !t.slug.is_empty() && !t.version_id.is_empty();
    let curseforge = source == "curseforge";
    let cf_ids = t.mod_id.zip(t.file_id);
    match kind {
        ContentKind::Plugins => modrinth.then(|| InstallCall::ModrinthPlugin {
            slug: t.slug.clone(),
            version_id: t.version_id.clone(),
        }),
        ContentKind::Mods if curseforge => cf_ids.map(|(mod_id, file_id)| InstallCall::CurseforgeMod {
            mod_id,
            file_id,
            loader: loader.to_string(),
        }),
        ContentKind::Mods => modrinth.then(|| InstallCall::ModrinthMod {
            slug: t.slug.clone(),
            version_id: t.version_id.clone(),
            loader: loader.to_string(),
        }),
        ContentKind::Datapacks if curseforge => {
            cf_ids.map(|(mod_id, file_id)| InstallCall::CurseforgeDatapack { mod_id, file_id })
        }
        ContentKind::Datapacks => modrinth.then(|| InstallCall::ModrinthDatapack {
            slug: t.slug.clone(),
            version_id: t.version_id.clone(),
        }),
    }
}

/// Suppression de l'ancien fichier après installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoveCall {
    /// `DELETE /mod/{fichier}`.
    Mod(String),
    /// `DELETE /plugin/{fichier}`.
    Plugin(String),
    /// `POST /files/delete` sur ce chemin (`<monde>/datapacks/<nom>`).
    File(String),
}

/// Ancien fichier à supprimer : seulement si la nouvelle version porte un autre nom (sinon
/// l'installation l'a déjà remplacé). Même comparaison exacte que le panel.
pub(crate) fn remove_call(kind: ContentKind, key: &str, new_filename: &str, level_name: &str) -> Option<RemoveCall> {
    if new_filename.is_empty() || new_filename == key {
        return None;
    }
    Some(match kind {
        ContentKind::Mods => RemoveCall::Mod(key.to_string()),
        ContentKind::Plugins => RemoveCall::Plugin(key.to_string()),
        ContentKind::Datapacks => {
            let level = if level_name.is_empty() { "world" } else { level_name };
            RemoveCall::File(format!("{level}/datapacks/{key}"))
        }
    })
}

/// Encode un segment de chemin comme `encodeURIComponent` (ce que fait le panel pour un nom de
/// fichier dans l'URL).
pub(crate) fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// --- Vérification ----------------------------------------------------------

/// Mise à jour applicable, avec ce qu'il faut pour l'appliquer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub update: ContentUpdate,
    pub target: Target,
    pub loader: String,
}

/// Bilan d'un type : propositions retenues et écartées.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct KindCheck {
    pub examined: usize,
    pub untracked: usize,
    pub candidates: Vec<Candidate>,
    pub ignored: Vec<IgnoredUpdate>,
}

/// Rapproche les installés d'un type et les propositions de l'hébergeur, en passant chaque
/// proposition au garde-fou.
pub(crate) fn evaluate(
    items: &[Installed],
    proposals: &HashMap<String, Proposal>,
    mc_version: &str,
    server_platform: &str,
) -> KindCheck {
    let mut out = KindCheck { examined: items.len(), ..Default::default() };
    for item in items {
        let Some(p) = proposals.get(&item.key) else {
            out.untracked += 1;
            continue;
        };
        if p.source.is_empty() {
            out.untracked += 1;
            continue;
        }
        let (true, Some(target)) = (p.has_update, p.target.as_ref()) else {
            continue;
        };
        match assess(item, &p.source, target, mc_version, server_platform) {
            Verdict::Apply => out.candidates.push(Candidate {
                update: ContentUpdate {
                    kind: item.kind,
                    key: item.key.clone(),
                    name: item.name.clone(),
                    enabled: item.enabled,
                    current_version: item.version.clone(),
                    new_version: target.label(),
                    new_filename: target.filename.clone(),
                    source: p.source.clone(),
                },
                target: target.clone(),
                loader: item.loader.clone(),
            }),
            Verdict::Ignore(reason) => out.ignored.push(IgnoredUpdate {
                kind: item.kind,
                key: item.key.clone(),
                name: item.name.clone(),
                current_version: item.version.clone(),
                new_version: target.label(),
                new_filename: target.filename.clone(),
                reason,
            }),
        }
    }
    out
}

/// Vérification complète d'un serveur (interne : garde les cibles pour l'application).
pub(crate) struct Check {
    pub public: ContentUpdates,
    pub candidates: Vec<Candidate>,
    pub level_name: String,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Vérifie les types demandés : version de Minecraft, installés, puis comparaison côté hébergeur.
/// L'échec d'un type est consigné dans `errors` sans empêcher les autres.
pub(crate) async fn check(api: &ApiClient, token: &str, server_id: i64, kinds: &[ContentKind]) -> Result<Check> {
    let mc = api.minecraft_version(token, server_id).await?;
    let platform = mc.platform();
    let mut public = ContentUpdates {
        mc_version: (!mc.version.is_empty()).then(|| mc.version.clone()),
        platform: platform.clone(),
        checked_at: now_secs(),
        examined: 0,
        untracked: 0,
        updates: Vec::new(),
        ignored: Vec::new(),
        errors: Vec::new(),
    };
    let mut candidates = Vec::new();
    let mut level_name = String::new();
    if mc.version.is_empty() {
        return Ok(Check { public, candidates, level_name });
    }
    let platform = platform.unwrap_or_default();
    let results = futures_util::future::join_all(
        kinds.iter().map(|&kind| check_kind(api, token, server_id, kind, &mc.version, &platform)),
    )
    .await;
    for (kind, result) in kinds.iter().zip(results) {
        match result {
            Ok((list, k)) => {
                if kind == &ContentKind::Datapacks {
                    level_name = list.level_name;
                }
                public.examined += k.examined;
                public.untracked += k.untracked;
                public.ignored.extend(k.ignored);
                candidates.extend(k.candidates);
            }
            Err(e) => public.errors.push(UpdateCheckError { kind: *kind, error: ErrorPayload::from(&e) }),
        }
    }
    candidates.sort_by(|a, b| {
        (a.update.kind, a.update.name.to_lowercase()).cmp(&(b.update.kind, b.update.name.to_lowercase()))
    });
    public.updates = candidates.iter().map(|c| c.update.clone()).collect();
    Ok(Check { public, candidates, level_name })
}

async fn check_kind(
    api: &ApiClient,
    token: &str,
    server_id: i64,
    kind: ContentKind,
    mc_version: &str,
    platform: &str,
) -> Result<(ContentList, KindCheck)> {
    let list = api.content_list(token, server_id, kind).await?;
    let Some(body) = check_body(kind, &list.items, mc_version) else {
        return Ok((list, KindCheck::default()));
    };
    let data = api.check_content_updates(token, server_id, kind, &body).await?;
    let k = evaluate(&list.items, &parse_proposals(&data), mc_version, platform);
    Ok((list, k))
}

/// Regroupe une sélection par type, sans doublon, dans l'ordre d'arrivée.
pub(crate) fn group_selection(items: &[UpdateSelection]) -> BTreeMap<ContentKind, Vec<String>> {
    let mut out: BTreeMap<ContentKind, Vec<String>> = BTreeMap::new();
    for s in items {
        let keys = out.entry(s.kind).or_default();
        if !s.key.is_empty() && !keys.contains(&s.key) {
            keys.push(s.key.clone());
        }
    }
    out.retain(|_, keys| !keys.is_empty());
    out
}

// --- Application -----------------------------------------------------------

async fn run_install(api: &ApiClient, token: &str, server_id: i64, call: &InstallCall) -> Result<()> {
    match call {
        // Même appel que l'installation d'un plugin depuis le catalogue (`Core::install_mod`).
        InstallCall::ModrinthPlugin { slug, version_id } => {
            api.install_modrinth(token, server_id, slug, version_id).await
        }
        InstallCall::ModrinthMod { slug, version_id, loader } => {
            api.install_modrinth_mod(token, server_id, slug, version_id, loader).await
        }
        InstallCall::CurseforgeMod { mod_id, file_id, loader } => {
            api.install_curseforge_mod(token, server_id, *mod_id, *file_id, loader).await
        }
        InstallCall::ModrinthDatapack { slug, version_id } => {
            api.install_modrinth_datapack(token, server_id, slug, version_id).await
        }
        InstallCall::CurseforgeDatapack { mod_id, file_id } => {
            api.install_curseforge_datapack(token, server_id, *mod_id, *file_id).await
        }
    }
}

async fn run_remove(api: &ApiClient, token: &str, server_id: i64, call: &RemoveCall) -> Result<()> {
    match call {
        RemoveCall::Mod(file) => api.delete_mod_file(token, server_id, file).await,
        RemoveCall::Plugin(file) => api.delete_plugin_file(token, server_id, file).await,
        RemoveCall::File(path) => api.delete_files(token, server_id, std::slice::from_ref(path)).await,
    }
}

/// Applique une sélection : nouvelle vérification des types concernés (l'état a pu changer depuis
/// l'affichage), puis, élément par élément et dans l'ordre, installation de la cible et suppression
/// de l'ancien fichier. Seules les mises à jour retenues par le garde-fou sont appliquées.
pub(crate) async fn apply(
    api: &ApiClient,
    token: &str,
    server_id: i64,
    selection: &[UpdateSelection],
) -> Result<UpdateReport> {
    let groups = group_selection(selection);
    if groups.is_empty() {
        return Err(Error::Unexpected("aucune mise à jour sélectionnée.".into()));
    }
    let kinds: Vec<ContentKind> = groups.keys().copied().collect();
    let fresh = check(api, token, server_id, &kinds).await?;
    if fresh.public.mc_version.is_none() {
        return Err(Error::Unexpected(
            "version de Minecraft du serveur inconnue : vérification des mises à jour impossible.".into(),
        ));
    }
    let mut outcomes = Vec::new();
    for (kind, keys) in groups {
        let failed_check = fresh.public.errors.iter().find(|e| e.kind == kind);
        for key in keys {
            let mut outcome = UpdateOutcome {
                kind,
                key: key.clone(),
                name: key.clone(),
                from_version: String::new(),
                to_version: String::new(),
                status: UpdateStatus::Skipped,
                reason: None,
                error: None,
            };
            if let Some(e) = failed_check {
                outcome.status = UpdateStatus::Failed;
                outcome.error = Some(e.error.clone());
                outcomes.push(outcome);
                continue;
            }
            if let Some(ig) = fresh.public.ignored.iter().find(|i| i.kind == kind && i.key == key) {
                outcome.name = ig.name.clone();
                outcome.from_version = ig.current_version.clone();
                outcome.to_version = ig.new_version.clone();
                outcome.reason = Some(ig.reason);
                outcomes.push(outcome);
                continue;
            }
            let Some(c) = fresh.candidates.iter().find(|c| c.update.kind == kind && c.update.key == key) else {
                outcome.reason = Some(IgnoreReason::UpToDate);
                outcomes.push(outcome);
                continue;
            };
            outcome.name = c.update.name.clone();
            outcome.from_version = c.update.current_version.clone();
            outcome.to_version = c.update.new_version.clone();
            // `assess` a déjà vérifié que l'appel existe.
            let Some(call) = install_call(kind, &c.update.source, &c.target, &c.loader) else {
                outcome.reason = Some(IgnoreReason::Incomplete);
                outcomes.push(outcome);
                continue;
            };
            if let Err(e) = run_install(api, token, server_id, &call).await {
                tracing::warn!(kind = kind.as_str(), %key, "mise à jour refusée : {e}");
                outcome.status = UpdateStatus::Failed;
                outcome.error = Some(ErrorPayload::from(&e));
                outcomes.push(outcome);
                continue;
            }
            outcome.status = UpdateStatus::Updated;
            if let Some(rm) = remove_call(kind, &key, &c.target.filename, &fresh.level_name) {
                if let Err(e) = run_remove(api, token, server_id, &rm).await {
                    tracing::warn!(kind = kind.as_str(), %key, "ancien fichier conservé : {e}");
                    outcome.status = UpdateStatus::OldKept;
                    outcome.error = Some(ErrorPayload::from(&e));
                }
            }
            tracing::info!(kind = kind.as_str(), %key, to = %outcome.to_version, "mise à jour appliquée");
            outcomes.push(outcome);
        }
    }
    let restart_needed = outcomes
        .iter()
        .any(|o| matches!(o.status, UpdateStatus::Updated | UpdateStatus::OldKept));
    Ok(UpdateReport { outcomes, restart_needed })
}

/// Datapacks installés, au format des autres listes d'installés (version inconnue de l'hébergeur).
pub(crate) fn datapacks_as_installed(list: &ContentList) -> Vec<InstalledItem> {
    list.items
        .iter()
        .map(|i| InstalledItem {
            name: stem(&i.name).to_string(),
            filename: i.key.clone(),
            version: i.version.clone(),
            enabled: i.enabled,
            loader: String::new(),
        })
        .collect()
}

// --- Cache -----------------------------------------------------------------

/// Durée de validité d'une vérification : l'ouverture d'une vue la réutilise, « Vérifier » la force.
pub(crate) const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// Dernière vérification par serveur.
#[derive(Default)]
pub(crate) struct UpdatesCache {
    map: Mutex<HashMap<i64, (Instant, ContentUpdates)>>,
}

impl UpdatesCache {
    pub(crate) fn get(&self, server_id: i64) -> Option<ContentUpdates> {
        let map = self.map.lock().ok()?;
        map.get(&server_id)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, v)| v.clone())
    }

    pub(crate) fn put(&self, server_id: i64, value: ContentUpdates) {
        if let Ok(mut map) = self.map.lock() {
            map.insert(server_id, (Instant::now(), value));
        }
    }

    /// Oublie un serveur (après une installation ou une mise à jour : son contenu a changé).
    pub(crate) fn forget(&self, server_id: i64) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&server_id);
        }
    }

    /// Oublie tout (changement de compte).
    pub(crate) fn clear(&self) {
        if let Ok(mut map) = self.map.lock() {
            map.clear();
        }
    }
}

// --- Rendu texte (MCP) -------------------------------------------------------

fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "mod",
        ContentKind::Plugins => "plugin",
        ContentKind::Datapacks => "datapack",
    }
}

fn reason_label(reason: IgnoreReason) -> &'static str {
    match reason {
        IgnoreReason::OtherPlatform => "variante d'une autre plateforme",
        IgnoreReason::SameVersion => "même version que l'installée",
        IgnoreReason::OlderVersion => "version plus ancienne",
        IgnoreReason::Incomplete => "cible incomplète",
        IgnoreReason::UpToDate => "déjà à jour",
    }
}

fn arrow(from: &str, to: &str) -> String {
    if from.is_empty() {
        format!("→ {to}")
    } else {
        format!("{from} → {to}")
    }
}

/// Résumé compact d'une vérification, pour un agent (outil MCP `check_updates`).
pub(crate) fn format_check(u: &ContentUpdates, kind: Option<ContentKind>) -> String {
    let Some(mc) = &u.mc_version else {
        return "Version de Minecraft du serveur inconnue : l'hébergeur ne peut pas vérifier les mises à jour.".into();
    };
    let keep = |k: ContentKind| kind.is_none_or(|w| w == k);
    let updates: Vec<&ContentUpdate> = u.updates.iter().filter(|x| keep(x.kind)).collect();
    let ignored: Vec<&IgnoredUpdate> = u.ignored.iter().filter(|x| keep(x.kind)).collect();
    let mut out = format!(
        "Minecraft {mc}{} — {} élément(s) examiné(s), {} non suivi(s) (projet non identifié).\n",
        u.platform.as_deref().map(|p| format!(" ({p})")).unwrap_or_default(),
        u.examined,
        u.untracked
    );
    if updates.is_empty() {
        out.push_str("Aucune mise à jour applicable.\n");
    } else {
        out.push_str(&format!("{} mise(s) à jour applicable(s) (clé entre crochets pour update_mods) :\n", updates.len()));
        for x in updates {
            out.push_str(&format!(
                "- [{}] {} : {} ({}){} [{}]\n",
                kind_label(x.kind),
                x.name,
                arrow(&x.current_version, &x.new_version),
                x.source,
                if x.enabled { "" } else { " — désactivé, sera réactivé" },
                x.key
            ));
        }
    }
    if !ignored.is_empty() {
        out.push_str(&format!("{} proposition(s) de l'hébergeur écartée(s), à ne PAS installer :\n", ignored.len()));
        for x in ignored {
            out.push_str(&format!(
                "- [{}] {} : {} — {}\n",
                kind_label(x.kind),
                x.name,
                arrow(&x.current_version, &x.new_version),
                reason_label(x.reason)
            ));
        }
    }
    for e in u.errors.iter().filter(|e| keep(e.kind)) {
        out.push_str(&format!("Vérification des {} impossible : {}\n", e.kind.as_str(), e.error.message));
    }
    out.trim_end().to_string()
}

/// Bilan lisible d'une mise à jour groupée (outil MCP `update_mods`).
pub(crate) fn format_report(r: &UpdateReport) -> String {
    if r.outcomes.is_empty() {
        return "Aucune mise à jour applicable.".into();
    }
    let mut out = String::new();
    for o in &r.outcomes {
        let what = format!("[{}] {} ({})", kind_label(o.kind), o.name, arrow(&o.from_version, &o.to_version));
        let line = match o.status {
            UpdateStatus::Updated => format!("✓ {what} mis à jour"),
            UpdateStatus::OldKept => format!(
                "⚠ {what} installé, mais l'ancien fichier « {} » n'a pas pu être supprimé ({}) : supprime-le AVANT de redémarrer",
                o.key,
                o.error.as_ref().map(|e| e.message.as_str()).unwrap_or("?")
            ),
            UpdateStatus::Failed => format!(
                "✗ {what} : échec ({})",
                o.error.as_ref().map(|e| e.message.as_str()).unwrap_or("?")
            ),
            UpdateStatus::Skipped => format!(
                "– {what} : non appliqué ({})",
                o.reason.map(reason_label).unwrap_or("aucune mise à jour")
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    if r.restart_needed {
        out.push_str("Redémarrage du serveur nécessaire pour charger les nouvelles versions.");
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(key: &str, version: &str) -> Installed {
        Installed {
            kind: ContentKind::Plugins,
            key: key.into(),
            name: stem(key).into(),
            version: version.into(),
            enabled: true,
            sha1: "abc".into(),
            loader: String::new(),
        }
    }

    fn fabric_mod(key: &str, version: &str) -> Installed {
        Installed { kind: ContentKind::Mods, loader: "fabric".into(), ..plugin(key, version) }
    }

    fn modrinth(filename: &str, version_number: &str) -> Target {
        Target {
            filename: filename.into(),
            slug: "projet".into(),
            version_id: "v1".into(),
            version_number: version_number.into(),
            ..Default::default()
        }
    }

    // --- Désérialisation tolérante ---

    #[test]
    fn content_kind_parses_and_serializes() {
        assert_eq!(ContentKind::parse("Plugin"), Some(ContentKind::Plugins));
        assert_eq!(ContentKind::parse(" mods "), Some(ContentKind::Mods));
        assert_eq!(ContentKind::parse("datapack"), Some(ContentKind::Datapacks));
        assert_eq!(ContentKind::parse("all"), None);
        assert_eq!(serde_json::to_value(ContentKind::Datapacks).unwrap(), json!("datapacks"));
        let s: UpdateSelection = serde_json::from_value(json!({ "kind": "mods", "key": "a.jar" })).unwrap();
        assert_eq!(s, UpdateSelection { kind: ContentKind::Mods, key: "a.jar".into() });
    }

    #[test]
    fn installed_lists_are_read_leniently() {
        // Relevé du 2026-11-18 : `source`/`project_id` à null pour un plugin non identifié.
        let data = json!({ "plugins": [
            { "name": "LuckPerms", "filename": "LuckPerms-Bukkit-5.5.71.jar", "version": "5.5.71",
              "enabled": true, "sha1": "f00", "source": "modrinth", "project_id": "Vebnzrzj" },
            { "name": null, "filename": "Perso.jar", "version": null, "enabled": 0, "sha1": null },
            { "name": "Sans fichier" }
        ]});
        let list = ContentList::from_value(ContentKind::Plugins, &data);
        assert_eq!(list.items.len(), 2, "un élément sans fichier est ignoré");
        assert_eq!(list.items[0].key, "LuckPerms-Bukkit-5.5.71.jar");
        assert_eq!(list.items[0].sha1, "f00");
        assert_eq!(list.items[1].name, "Perso.jar", "nom absent : la clé le remplace");
        assert_eq!(list.items[1].version, "");
        assert!(!list.items[1].enabled);
        // Mods : le loader est conservé ; enveloppe d'un autre type ignorée.
        let mods = json!({ "mods": [{ "name": "Sodium", "filename": "sodium.jar", "loader": "fabric", "version": 5 }] });
        let list = ContentList::from_value(ContentKind::Mods, &mods);
        assert_eq!(list.items[0].loader, "fabric");
        assert_eq!(list.items[0].version, "5");
        assert!(ContentList::from_value(ContentKind::Plugins, &mods).items.is_empty());
        // Datapacks : clé = nom, dossier du monde.
        let dp = json!({ "level_name": "monde", "datapacks": [{ "name": "Terralith.zip", "size": 10 }] });
        let list = ContentList::from_value(ContentKind::Datapacks, &dp);
        assert_eq!(list.level_name, "monde");
        assert_eq!(list.items[0].key, "Terralith.zip");
        assert!(list.items[0].enabled);
        // Formes inattendues : liste vide, pas d'erreur.
        assert!(ContentList::from_value(ContentKind::Mods, &json!({ "mods": null })).items.is_empty());
        assert!(ContentList::from_value(ContentKind::Mods, &json!([])).items.is_empty());
    }

    #[test]
    fn proposals_are_read_leniently() {
        // Formes réelles (relevé du 2026-11-18) + variantes tolérées.
        let data = json!({ "updates_available": 2, "updates": {
            "LuckPerms-Bukkit-5.5.71.jar": { "has_update": true, "source": "modrinth", "target": {
                "filename": "LuckPerms-Velocity-5.5.71.jar", "slug": "Vebnzrzj",
                "version_id": "tamnmXad", "version_number": "v5.5.71-velocity" } },
            "Perso.jar": { "has_update": false, "source": null, "target": null },
            "cf.jar": { "has_update": 1, "source": "CurseForge", "target": {
                "mod_id": "1234", "file_id": 5678, "display_name": "Mod 2.0", "extra": [1] } },
            "vide.jar": { "has_update": true, "source": "modrinth", "target": [] }
        }});
        let p = parse_proposals(&data);
        assert_eq!(p.len(), 4);
        let lp = &p["LuckPerms-Bukkit-5.5.71.jar"];
        assert!(lp.has_update);
        assert_eq!(lp.target.as_ref().unwrap().version_id, "tamnmXad");
        assert_eq!(p["Perso.jar"], Proposal::default());
        let cf = &p["cf.jar"];
        assert!(cf.has_update);
        assert_eq!(cf.source, "curseforge");
        assert_eq!(cf.target.as_ref().unwrap().mod_id, Some(1234));
        assert_eq!(cf.target.as_ref().unwrap().file_id, Some(5678));
        assert_eq!(cf.target.as_ref().unwrap().label(), "Mod 2.0");
        assert_eq!(p["vide.jar"].target, None, "cible `[]` (tableau PHP vide) = aucune");
        // `updates` vide renvoyé en tableau, ou absent.
        assert!(parse_proposals(&json!({ "updates": [] })).is_empty());
        assert!(parse_proposals(&json!({})).is_empty());
    }

    #[test]
    fn mc_version_and_platform() {
        let v = McVersion::from_value(&json!({ "version": "26.1.2", "source": "paper", "confidence": "high" }));
        assert_eq!(v.version, "26.1.2");
        assert_eq!(v.platform().as_deref(), Some("paper"));
        let v = McVersion::from_value(&json!({ "version": "26.1.2", "source": "explicit_jar" }));
        assert_eq!(v.platform(), None, "jar explicite : plateforme inconnue");
        assert_eq!(McVersion::from_value(&json!({ "version": null })).version, "");
    }

    // --- Construction des corps ---

    #[test]
    fn check_bodies_match_the_panel() {
        let p = vec![plugin("A.jar", "1"), plugin("B.jar", "")];
        assert_eq!(
            check_body(ContentKind::Plugins, &p, "1.21.1").unwrap(),
            json!({ "items": [{ "filename": "A.jar", "sha1": "abc" }, { "filename": "B.jar", "sha1": "abc" }],
                    "mc_version": "1.21.1" })
        );
        let m = vec![fabric_mod("sodium.jar", "0.5")];
        assert_eq!(
            check_body(ContentKind::Mods, &m, " 1.21.1 ").unwrap(),
            json!({ "items": [{ "filename": "sodium.jar", "sha1": "abc", "loader": "fabric" }], "mc_version": "1.21.1" })
        );
        let d = vec![Installed { kind: ContentKind::Datapacks, ..plugin("Terralith.zip", "") }];
        assert_eq!(
            check_body(ContentKind::Datapacks, &d, "1.21.1").unwrap(),
            json!({ "items": [{ "name": "Terralith.zip" }], "mc_version": "1.21.1" })
        );
        // L'API refuse une liste vide ou une version absente : pas d'appel.
        assert_eq!(check_body(ContentKind::Plugins, &[], "1.21.1"), None);
        assert_eq!(check_body(ContentKind::Plugins, &p, ""), None);
    }

    #[test]
    fn install_calls_follow_the_panel() {
        let t = modrinth("new.jar", "2.0");
        assert_eq!(
            install_call(ContentKind::Plugins, "modrinth", &t, ""),
            Some(InstallCall::ModrinthPlugin { slug: "projet".into(), version_id: "v1".into() })
        );
        // Plugins : Modrinth même si la source annoncée diffère (comme le panel).
        assert!(matches!(install_call(ContentKind::Plugins, "curseforge", &t, ""), Some(InstallCall::ModrinthPlugin { .. })));
        assert_eq!(
            install_call(ContentKind::Mods, "modrinth", &t, "fabric"),
            Some(InstallCall::ModrinthMod { slug: "projet".into(), version_id: "v1".into(), loader: "fabric".into() })
        );
        let cf = Target { mod_id: Some(1), file_id: Some(2), ..Default::default() };
        assert_eq!(
            install_call(ContentKind::Mods, "curseforge", &cf, "forge"),
            Some(InstallCall::CurseforgeMod { mod_id: 1, file_id: 2, loader: "forge".into() })
        );
        assert_eq!(
            install_call(ContentKind::Datapacks, "curseforge", &cf, ""),
            Some(InstallCall::CurseforgeDatapack { mod_id: 1, file_id: 2 })
        );
        assert_eq!(
            install_call(ContentKind::Datapacks, "modrinth", &t, ""),
            Some(InstallCall::ModrinthDatapack { slug: "projet".into(), version_id: "v1".into() })
        );
        // Cibles incomplètes.
        assert_eq!(install_call(ContentKind::Mods, "curseforge", &t, "forge"), None);
        assert_eq!(install_call(ContentKind::Mods, "modrinth", &cf, "forge"), None);
        assert_eq!(install_call(ContentKind::Plugins, "modrinth", &Target { slug: "x".into(), ..Default::default() }, ""), None);
    }

    #[test]
    fn old_file_is_removed_only_when_the_name_changes() {
        assert_eq!(remove_call(ContentKind::Plugins, "A-1.jar", "A-2.jar", ""), Some(RemoveCall::Plugin("A-1.jar".into())));
        assert_eq!(remove_call(ContentKind::Mods, "a-1.jar", "a-2.jar", ""), Some(RemoveCall::Mod("a-1.jar".into())));
        assert_eq!(remove_call(ContentKind::Plugins, "A.jar", "A.jar", ""), None, "même nom : remplacé par l'installation");
        assert_eq!(remove_call(ContentKind::Plugins, "A.jar", "", ""), None);
        assert_eq!(
            remove_call(ContentKind::Datapacks, "T-1.zip", "T-2.zip", "monde"),
            Some(RemoveCall::File("monde/datapacks/T-1.zip".into()))
        );
        assert_eq!(
            remove_call(ContentKind::Datapacks, "T-1.zip", "T-2.zip", ""),
            Some(RemoveCall::File("world/datapacks/T-1.zip".into()))
        );
    }

    #[test]
    fn path_segment_is_encoded_like_encode_uri_component() {
        assert_eq!(encode_component("LuckPerms-Bukkit-5.5.71.jar"), "LuckPerms-Bukkit-5.5.71.jar");
        assert_eq!(encode_component("Mon plugin+1 (v2)/é.jar"), "Mon%20plugin%2B1%20(v2)%2F%C3%A9.jar");
        assert_eq!(encode_component("a?b#c&d"), "a%3Fb%23c%26d");
    }

    // --- Comparaison de versions et plateformes ---

    #[test]
    fn version_numbers_drop_the_minecraft_version() {
        assert_eq!(version_numbers("5.5.71", "26.1.2"), vec![5, 5, 71]);
        assert_eq!(version_numbers("v5.5.71-velocity", "26.1.2"), vec![5, 5, 71]);
        assert_eq!(version_numbers("sodium-fabric-0.5.8+mc1.20.1", "1.20.1"), vec![0, 5, 8]);
        assert_eq!(version_numbers("1.20.1-47.2.0", "1.20.1"), vec![47, 2, 0]);
        assert_eq!(version_numbers("mod-1.21-2.0", "1.21.1"), vec![2, 0], "préfixe de la version MC retiré");
        assert_eq!(version_numbers("1.20.10-3.0", "1.20.1"), vec![1, 20, 10, 3, 0], "pas un préfixe : conservé");
        assert_eq!(version_numbers("1.2.3", ""), vec![1, 2, 3]);
        assert_eq!(version_numbers("${project.version}", "1.21"), Vec::<u64>::new());
        assert_eq!(version_numbers("build 1.", "1.21"), vec![1], "point final ignoré");
        assert_eq!(version_numbers("99999999999999999999999", ""), vec![u64::MAX]);
    }

    #[test]
    fn numbers_compare_with_zero_padding() {
        assert_eq!(compare_numbers(&[1, 0], &[1, 0, 0]), Ordering::Equal);
        assert_eq!(compare_numbers(&[1, 0, 1], &[1, 0]), Ordering::Greater);
        assert_eq!(compare_numbers(&[1, 9], &[1, 10]), Ordering::Less, "numérique, pas lexical");
        assert_eq!(compare_numbers(&[2, 14, 0, 2], &[2, 14, 0]), Ordering::Greater);
        assert_eq!(compare_numbers(&[], &[]), Ordering::Equal);
    }

    #[test]
    fn platforms_are_detected_by_word() {
        let set = |s: &str| platforms_in(s).into_iter().collect::<Vec<_>>();
        assert_eq!(set("LuckPerms-Bukkit-5.5.71.jar"), vec!["bukkit"]);
        assert_eq!(set("2.14.0+velocity"), vec!["velocity"]);
        assert_eq!(set("XConomy-Paper-2.26.3.jar"), vec!["bukkit"]);
        assert_eq!(set("jei-1.20.1-neoforge-15.jar"), vec!["neoforge"], "« neoforge » n'est pas « forge »");
        assert_eq!(set("Geyser-Spigot.jar Waterfall"), vec!["bukkit", "bungeecord"]);
        assert!(set("Chunky-1.5.66.jar").is_empty());
        assert!(set("forgery-1.0.jar").is_empty(), "mot entier seulement");
    }

    #[test]
    fn platform_conflicts() {
        // Relevé réel : variante Velocity proposée pour un plugin Bukkit/Spigot.
        assert!(platform_conflict("LuckPerms-Bukkit-5.5.71.jar 5.5.71", "", "LuckPerms-Velocity-5.5.71.jar v5.5.71-velocity"));
        assert!(platform_conflict("packetevents-spigot-2.14.0.jar", "", "packetevents-velocity-2.14.0.jar 2.14.0+velocity"));
        // Même famille sous un autre nom.
        assert!(!platform_conflict("XConomy-Spigot.jar", "", "XConomy-Paper-2.jar"));
        // Fichier muet : la plateforme du serveur (ou le loader du mod) sert de référence.
        assert!(platform_conflict("LuckPerms.jar", "paper", "LuckPerms-Velocity.jar"));
        assert!(platform_conflict("lithium.jar", "fabric", "lithium-neoforge-1.0.jar"));
        assert!(!platform_conflict("lithium.jar", "fabric", "lithium-fabric-1.0.jar"));
        // Quilt charge les mods Fabric, pas l'inverse.
        assert!(!platform_conflict("x.jar", "quilt", "x-fabric.jar"));
        assert!(platform_conflict("x.jar", "fabric", "x-quilt.jar"));
        // Jar universel : une plateforme compatible suffit.
        assert!(!platform_conflict("x-bukkit.jar", "", "x-bukkit-velocity-bungee.jar"));
        // Rien pour conclure : pas de conflit.
        assert!(!platform_conflict("x.jar", "", "x-velocity.jar"));
        assert!(!platform_conflict("x-bukkit.jar", "", "x-2.0.jar"));
    }

    #[test]
    fn assess_rejects_the_real_false_positives() {
        // Relevé du 2026-11-18, serveur Paper 26.1.2 (source `explicit_jar`, plateforme inconnue).
        let lp = plugin("LuckPerms-Bukkit-5.5.71.jar", "5.5.71");
        let t = modrinth("LuckPerms-Velocity-5.5.71.jar", "v5.5.71-velocity");
        assert_eq!(assess(&lp, "modrinth", &t, "26.1.2", ""), Verdict::Ignore(IgnoreReason::OtherPlatform));
        // Même sans plateforme dans les noms, la version identique suffit à l'écarter.
        let t2 = modrinth("LuckPerms-5.5.71.jar", "v5.5.71");
        let lp2 = plugin("LuckPerms.jar", "5.5.71");
        assert_eq!(assess(&lp2, "modrinth", &t2, "26.1.2", ""), Verdict::Ignore(IgnoreReason::SameVersion));
    }

    #[test]
    fn assess_keeps_real_updates() {
        let p = plugin("Chunky-Paper-1.5.66.jar", "1.5.66");
        assert_eq!(assess(&p, "modrinth", &modrinth("Chunky-Paper-1.5.70.jar", "1.5.70"), "26.1.2", "paper"), Verdict::Apply);
        // Version MC embarquée des deux côtés, schémas différents.
        let m = fabric_mod("sodium-fabric-0.5.8+mc1.20.1.jar", "0.5.8+mc1.20.1");
        let t = modrinth("sodium-fabric-0.5.11+mc1.20.1.jar", "mc1.20.1-0.5.11");
        assert_eq!(assess(&m, "modrinth", &t, "1.20.1", ""), Verdict::Apply);
        // Version installée inconnue : lue dans le nom de fichier.
        let p = plugin("Vault-1.7.2.jar", "");
        assert_eq!(assess(&p, "modrinth", &modrinth("Vault-1.7.3.jar", "1.7.3"), "1.21", ""), Verdict::Apply);
        // Numéros illisibles : la proposition de l'hébergeur est retenue.
        let p = plugin("Geyser.jar", "${version}");
        assert_eq!(assess(&p, "modrinth", &modrinth("Geyser.jar", "latest"), "1.21", ""), Verdict::Apply);
        // Datapack (pas de version côté hébergeur).
        let d = Installed { kind: ContentKind::Datapacks, ..plugin("Terralith_1.21.x_v2.5.4.zip", "") };
        assert_eq!(assess(&d, "modrinth", &modrinth("Terralith_1.21.x_v2.5.5.zip", "2.5.5"), "1.21.1", ""), Verdict::Apply);
    }

    #[test]
    fn assess_rejects_downgrades_and_incomplete_targets() {
        let p = plugin("Vault-1.7.3.jar", "1.7.3");
        assert_eq!(assess(&p, "modrinth", &modrinth("Vault-1.7.2.jar", "1.7.2"), "1.21", ""), Verdict::Ignore(IgnoreReason::OlderVersion));
        let incomplete = Target { filename: "x.jar".into(), version_number: "9".into(), ..Default::default() };
        assert_eq!(assess(&p, "modrinth", &incomplete, "1.21", ""), Verdict::Ignore(IgnoreReason::Incomplete));
        // Mod Fabric : une cible NeoForge est écartée grâce au loader.
        let m = fabric_mod("lithium.jar", "0.11");
        assert_eq!(
            assess(&m, "modrinth", &modrinth("lithium-neoforge-0.12.jar", "0.12"), "1.21.1", ""),
            Verdict::Ignore(IgnoreReason::OtherPlatform)
        );
    }

    // --- Regroupement ---

    #[test]
    fn evaluate_sorts_proposals_into_updates_ignored_and_untracked() {
        let items = vec![
            plugin("LuckPerms-Bukkit-5.5.71.jar", "5.5.71"),
            plugin("Chunky-1.5.66.jar", "1.5.66"),
            plugin("Perso.jar", "1.0"),
            plugin("Vault-1.7.3.jar", "1.7.3"),
            plugin("Absent.jar", "1.0"),
        ];
        let data = json!({ "updates": {
            "LuckPerms-Bukkit-5.5.71.jar": { "has_update": true, "source": "modrinth", "target": {
                "filename": "LuckPerms-Velocity-5.5.71.jar", "slug": "a", "version_id": "b", "version_number": "v5.5.71-velocity" } },
            "Chunky-1.5.66.jar": { "has_update": true, "source": "modrinth", "target": {
                "filename": "Chunky-1.5.70.jar", "slug": "c", "version_id": "d", "version_number": "1.5.70" } },
            "Perso.jar": { "has_update": false, "source": null, "target": null },
            "Vault-1.7.3.jar": { "has_update": false, "source": "modrinth", "target": null }
        }});
        let k = evaluate(&items, &parse_proposals(&data), "26.1.2", "paper");
        assert_eq!(k.examined, 5);
        assert_eq!(k.untracked, 2, "source nulle + absent de la réponse");
        assert_eq!(k.candidates.len(), 1);
        let c = &k.candidates[0];
        assert_eq!(c.update.key, "Chunky-1.5.66.jar");
        assert_eq!(c.update.current_version, "1.5.66");
        assert_eq!(c.update.new_version, "1.5.70");
        assert_eq!(c.update.new_filename, "Chunky-1.5.70.jar");
        assert_eq!(k.ignored.len(), 1);
        assert_eq!(k.ignored[0].reason, IgnoreReason::OtherPlatform);
    }

    #[test]
    fn selection_is_grouped_by_kind_without_duplicates() {
        let sel = |kind, key: &str| UpdateSelection { kind, key: key.into() };
        let g = group_selection(&[
            sel(ContentKind::Plugins, "b.jar"),
            sel(ContentKind::Mods, "a.jar"),
            sel(ContentKind::Plugins, "a.jar"),
            sel(ContentKind::Plugins, "b.jar"),
            sel(ContentKind::Datapacks, ""),
        ]);
        assert_eq!(g.len(), 2, "clé vide ignorée, type sans clé retiré");
        assert_eq!(g[&ContentKind::Mods], vec!["a.jar"]);
        assert_eq!(g[&ContentKind::Plugins], vec!["b.jar", "a.jar"], "ordre d'arrivée conservé");
        assert!(group_selection(&[]).is_empty());
    }

    #[test]
    fn datapacks_map_to_installed_items() {
        let list = ContentList::from_value(
            ContentKind::Datapacks,
            &json!({ "level_name": "world", "datapacks": [{ "name": "Terralith.zip" }, { "name": "dossier" }] }),
        );
        let items = datapacks_as_installed(&list);
        assert_eq!(items[0].name, "Terralith");
        assert_eq!(items[0].filename, "Terralith.zip");
        assert_eq!(items[1].name, "dossier");
    }

    #[test]
    fn cache_forgets_and_clears() {
        let c = UpdatesCache::default();
        let v = ContentUpdates {
            mc_version: Some("1.21".into()),
            platform: None,
            checked_at: 1,
            examined: 0,
            untracked: 0,
            updates: vec![],
            ignored: vec![],
            errors: vec![],
        };
        c.put(1, v.clone());
        c.put(2, v);
        assert!(c.get(1).is_some());
        c.forget(1);
        assert!(c.get(1).is_none());
        assert!(c.get(2).is_some());
        c.clear();
        assert!(c.get(2).is_none());
    }

    #[test]
    fn text_summaries_name_keys_and_reasons() {
        let u = ContentUpdates {
            mc_version: Some("1.21.1".into()),
            platform: Some("fabric".into()),
            checked_at: 0,
            examined: 3,
            untracked: 1,
            updates: vec![ContentUpdate {
                kind: ContentKind::Mods,
                key: "sodium-0.5.jar".into(),
                name: "Sodium".into(),
                enabled: false,
                current_version: "0.5".into(),
                new_version: "0.6".into(),
                new_filename: "sodium-0.6.jar".into(),
                source: "modrinth".into(),
            }],
            ignored: vec![IgnoredUpdate {
                kind: ContentKind::Plugins,
                key: "LP.jar".into(),
                name: "LuckPerms".into(),
                current_version: "5.5.71".into(),
                new_version: "v5.5.71-velocity".into(),
                new_filename: "LP-Velocity.jar".into(),
                reason: IgnoreReason::OtherPlatform,
            }],
            errors: vec![],
        };
        let s = format_check(&u, None);
        assert!(s.contains("Minecraft 1.21.1 (fabric)"));
        assert!(s.contains("[mod] Sodium : 0.5 → 0.6 (modrinth) — désactivé, sera réactivé [sodium-0.5.jar]"));
        assert!(s.contains("variante d'une autre plateforme"));
        let only_mods = format_check(&u, Some(ContentKind::Mods));
        assert!(!only_mods.contains("LuckPerms"));
        let unknown = ContentUpdates { mc_version: None, ..u.clone() };
        assert!(format_check(&unknown, None).contains("inconnue"));

        let r = UpdateReport {
            outcomes: vec![
                UpdateOutcome {
                    kind: ContentKind::Mods,
                    key: "sodium-0.5.jar".into(),
                    name: "Sodium".into(),
                    from_version: "0.5".into(),
                    to_version: "0.6".into(),
                    status: UpdateStatus::OldKept,
                    reason: None,
                    error: Some(ErrorPayload::from(&Error::Forbidden)),
                },
                UpdateOutcome {
                    kind: ContentKind::Plugins,
                    key: "LP.jar".into(),
                    name: "LuckPerms".into(),
                    from_version: String::new(),
                    to_version: String::new(),
                    status: UpdateStatus::Skipped,
                    reason: Some(IgnoreReason::UpToDate),
                    error: None,
                },
            ],
            restart_needed: true,
        };
        let s = format_report(&r);
        assert!(s.contains("« sodium-0.5.jar » n'a pas pu être supprimé"));
        assert!(s.contains("déjà à jour"));
        assert!(s.ends_with("Redémarrage du serveur nécessaire pour charger les nouvelles versions."));
    }
}
