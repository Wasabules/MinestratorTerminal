//! Manifeste déclaratif d'un serveur : export, plan (écart avec le serveur réel) et application.
//!
//! Un agent décrit bien mieux l'état voulu d'un serveur qu'il n'enchaîne quarante appels sans se
//! tromper. Le manifeste (JSON, `"version": 1`, chaque section facultative) décrit : le serveur
//! (nom, jeu, version du jeu, loader, Java), les réglages `server.properties`, la commande de
//! démarrage, le contenu (plugins, mods, datapacks et leur source), des clés de fichiers de config
//! (`set_config`), de petits fichiers texte, la liste blanche et les opérateurs, les tâches
//! planifiées du panel. Format détaillé dans `MCP.md` (« Construire et reproduire un serveur »).
//!
//! - [`export`] lit un serveur et en produit le manifeste (secrets masqués) ;
//! - [`build_plan`] compare un manifeste au serveur réel et donne les étapes ordonnées, chacune
//!   `ok` (déjà conforme), `change`, `add`, `remove`, `manual` (action humaine ou endpoint
//!   d'écriture non géré) ou `unsupported` ;
//! - [`apply`] exécute le plan, **idempotent** : il est recalculé à chaque fois, une étape déjà
//!   conforme n'est jamais refaite. Snapshot préalable par défaut, progression par étape, arrêt
//!   propre sur annulation, rapport étape par étape ; une étape échouée n'arrête que celles qui
//!   en dépendent.
//!
//! Une valeur masquée à l'export (`[SECRET]`, `[IP]`…) veut dire « garder la valeur actuelle » :
//! l'étape est ignorée, jamais le masque écrit. `clone_server` ([`clone`]) réutilise tout cela.

mod clone;
#[cfg(test)]
mod tests;

pub(crate) use clone::clone_server_tool;

use crate::config_edit::{self, EditOptions, EditRequest, Format};
use crate::models::Startup;
use crate::panel_schedules::{PanelCron, PanelSchedule, PanelScheduleInput, PanelTaskInput};
use crate::updates::ContentKind;
use crate::Core;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

/// Version du format.
pub(crate) const MANIFEST_VERSION: i64 = 1;
/// Taille maximale d'un fichier de la section `files`, et de leur total.
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_FILES_TOTAL: usize = 2 * 1024 * 1024;
/// Éléments au plus par section (contenu, configs, fichiers, joueurs, tâches).
const MAX_ITEMS: usize = 500;
/// Clés au plus exportées d'un fichier de config sans liste de clés.
const MAX_EXPORT_KEYS: usize = 300;
/// Attente d'une snapshot préalable.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(15 * 60);
const SNAPSHOT_POLL: Duration = Duration::from_secs(3);
/// Réglages propres à l'allocation réseau d'un serveur (ports et adresse attribués par
/// l'hébergeur) : jamais exportés, ni recopiés sur un clone.
pub(crate) const ALLOCATION_KEYS: &[&str] =
    &["server-port", "server-ip", "query.port", "rcon.port"];
/// Résolutions de version (marketplace) simultanées.
const RESOLVE_CONCURRENCY: usize = 4;

// --- Format ------------------------------------------------------------------------------------

/// Manifeste v1. Toutes les sections sont facultatives.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<StartupSpec>,
    /// Tableau d'éléments, ou `{ items, remove_unlisted }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configs: Vec<ConfigSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<PlayersSpec>,
    /// Tableau de tâches, ou `{ items, remove_unlisted }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Nom du jeu (egg), ex. `Minecraft`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egg_id: Option<i64>,
    /// Version du jeu détectée par l'hébergeur (Minecraft : `1.21.4`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_version: Option<String>,
    /// Plateforme (Minecraft : `paper`, `fabric`, `velocity`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<String>,
    /// Version majeure de Java (lue dans l'image d'exécution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_version: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartupSpec {
    /// Commande de démarrage complète (garder `{{SERVER_JARFILE}}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jar_file: Option<String>,
}

/// Un plugin, mod ou datapack.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContentItem {
    /// `plugin`, `mod` ou `datapack`.
    pub kind: String,
    /// `modrinth`, `curseforge` ou `spigot` ; `null` : non identifié (non installable).
    #[serde(default)]
    pub source: Option<String>,
    /// Slug ou identifiant Modrinth, identifiant numérique CurseForge ou SpigotMC.
    #[serde(default)]
    pub project: Option<String>,
    /// Version : identifiant de version de la source ou numéro de version ; absente = la plus
    /// récente compatible (à l'installation seulement : un élément présent est conforme).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Nom de fichier exact (clé de rapprochement la plus sûre).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Loader d'un mod (`fabric`, `forge`…) ; défaut : celui du serveur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigSpec {
    pub path: String,
    /// `{ "clé.chemin": valeur }`, appliqué comme `set_config`.
    pub set: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileSpec {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlayersSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whitelist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<String>>,
    /// Retirer les joueurs absents des listes données (défaut false).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_unlisted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduleSpec {
    pub name: String,
    /// Cinq champs : minute heure jour-du-mois mois jour-de-semaine.
    pub cron: String,
    #[serde(default = "yes")]
    pub is_active: bool,
    #[serde(default)]
    pub only_when_online: bool,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskSpec {
    /// `command` ou `power`.
    pub action: String,
    pub payload: String,
    #[serde(default)]
    pub time_offset: i64,
    #[serde(default)]
    pub continue_on_failure: bool,
}

fn yes() -> bool {
    true
}

/// Section en tableau, ou objet `{ items, remove_unlisted }`.
fn list_section<T: serde::de::DeserializeOwned>(
    v: Option<&Value>,
    what: &str,
) -> Result<(Vec<T>, bool), String> {
    let (items, remove) = match v {
        None | Some(Value::Null) => return Ok((Vec::new(), false)),
        Some(Value::Array(_)) => (v.cloned().unwrap_or_default(), false),
        Some(Value::Object(m)) => {
            if let Some(k) = m
                .keys()
                .find(|k| !matches!(k.as_str(), "items" | "remove_unlisted"))
            {
                return Err(format!(
                    "`{what}` : champ inconnu « {k} » (items, remove_unlisted)"
                ));
            }
            (
                m.get("items").cloned().unwrap_or(Value::Array(Vec::new())),
                m.get("remove_unlisted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        }
        Some(_) => {
            return Err(format!(
                "`{what}` : tableau ou objet {{ items, remove_unlisted }} attendu"
            ))
        }
    };
    let list: Vec<T> =
        serde_json::from_value(items).map_err(|e| format!("`{what}` invalide : {e}"))?;
    if list.len() > MAX_ITEMS {
        return Err(format!(
            "`{what}` : {} éléments, {MAX_ITEMS} au plus",
            list.len()
        ));
    }
    Ok((list, remove))
}

impl Manifest {
    /// Lit un manifeste (objet, ou texte JSON) et vérifie sa forme.
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let v = match v {
            Value::String(s) => serde_json::from_str::<Value>(s)
                .map_err(|e| format!("manifeste : JSON invalide ({e})"))?,
            other => other.clone(),
        };
        if !v.is_object() {
            return Err("manifeste : objet JSON attendu".into());
        }
        let m: Manifest =
            serde_json::from_value(v).map_err(|e| format!("manifeste invalide : {e}"))?;
        if m.version != MANIFEST_VERSION {
            return Err(format!("manifeste : version {} non prise en charge (seule la version {MANIFEST_VERSION} existe)", m.version));
        }
        m.content_items()?;
        m.schedule_items()?;
        for c in &m.configs {
            if Format::detect(&c.path).is_none() {
                return Err(format!("configs : « {} » n'est pas un format pris en charge (.yml, .yaml, .toml, .json, .properties)", c.path));
            }
        }
        if m.configs.len() > MAX_ITEMS || m.files.len() > MAX_ITEMS {
            return Err(format!(
                "manifeste : {MAX_ITEMS} configs ou fichiers au plus"
            ));
        }
        let total: usize = m.files.iter().map(|f| f.content.len()).sum();
        if total > MAX_FILES_TOTAL {
            return Err(format!(
                "files : {total} octets au total, {} Kio au plus",
                MAX_FILES_TOTAL / 1024
            ));
        }
        Ok(m)
    }

    pub(crate) fn content_items(&self) -> Result<(Vec<ContentItem>, bool), String> {
        let (items, remove): (Vec<ContentItem>, bool) =
            list_section(self.content.as_ref(), "content")?;
        for it in &items {
            ContentKind::parse(&it.kind).ok_or_else(|| {
                format!(
                    "content : type « {} » inconnu (plugin, mod ou datapack)",
                    it.kind
                )
            })?;
        }
        Ok((items, remove))
    }

    pub(crate) fn schedule_items(&self) -> Result<(Vec<ScheduleSpec>, bool), String> {
        list_section(self.schedules.as_ref(), "schedules")
    }
}

// --- Plan --------------------------------------------------------------------------------------------

/// Statut d'une étape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    /// Déjà conforme.
    Ok,
    Change,
    Add,
    Remove,
    /// À faire par l'humain (panel), ou endpoint d'écriture non géré.
    Manual,
    Unsupported,
}

impl Status {
    fn actionable(self) -> bool {
        matches!(self, Status::Change | Status::Add | Status::Remove)
    }
}

/// Ce que fait une étape.
#[derive(Debug, Clone)]
pub(crate) enum Action {
    Rename(String),
    StartupParams(String),
    /// Réglage existant de `server.properties` (API).
    Property {
        key: String,
        value: String,
    },
    /// Réglage absent : ajouté au fichier par SFTP (`set_config`).
    PropertyFile {
        key: String,
        value: String,
    },
    Install(Install),
    RemoveContent {
        kind: ContentKind,
        file: String,
        level_name: String,
    },
    Config {
        path: String,
        set: Vec<EditRequest>,
    },
    WriteFile {
        path: String,
        content: String,
    },
    Player {
        action: &'static str,
        name: String,
    },
    CreateSchedule(ScheduleSpec),
    UpdateSchedule {
        current: Box<PanelSchedule>,
        wanted: ScheduleSpec,
    },
    DeleteSchedule {
        id: i64,
    },
}

/// Installation d'un contenu (version résolue au plan).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Install {
    pub kind: ContentKind,
    pub source: String,
    pub project: String,
    pub version: Option<String>,
    pub file: Option<String>,
    pub loader: String,
    /// Version résolue (identifiant de la source) et son fichier.
    pub version_id: Option<String>,
    pub target_file: Option<String>,
    /// Fichier installé à remplacer (supprimé après l'installation s'il change de nom).
    pub replaces: Option<String>,
    pub level_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Step {
    pub id: String,
    pub status: Status,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(skip)]
    pub action: Option<Action>,
}

impl Step {
    fn new(id: impl Into<String>, status: Status, summary: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status,
            summary: summary.into(),
            current: None,
            target: None,
            diff: None,
            depends_on: Vec::new(),
            action: None,
        }
    }
    fn values(mut self, current: Option<Value>, target: Option<Value>) -> Self {
        self.current = current;
        self.target = target;
        self
    }
    fn act(mut self, a: Action) -> Self {
        self.action = Some(a);
        self
    }
}

/// Plan : étapes ordonnées, plus ce qui existe sur le serveur sans figurer au manifeste.
#[derive(Debug, Clone, Default)]
pub(crate) struct Plan {
    pub steps: Vec<Step>,
    pub unlisted: Vec<String>,
    pub warnings: Vec<String>,
}

/// Contient un masque d'anonymisation : « garder la valeur actuelle ».
fn masked(s: &str) -> bool {
    config_edit::MASK_MARKERS.iter().any(|m| s.contains(m))
}

/// Texte d'une valeur de réglage (`20`, `true`, `texte`).
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

// --- Faits lus sur le serveur ----------------------------------------------------------------------

/// Ce que le plan compare au manifeste.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub name: String,
    pub family: String,
    pub egg_id: Option<i64>,
    pub egg_name: String,
    /// Minecraft : version et plateforme détectées par l'hébergeur.
    pub game_version: Option<String>,
    pub loader: Option<String>,
    pub startup: Option<Startup>,
}

impl Facts {
    fn minecraft(&self) -> bool {
        self.family == "minecraft"
    }
}

/// Version majeure de Java d'après l'image (`…/yolks:java_21` → 21, `eclipse-temurin:17-jre`).
pub(crate) fn java_major(image: &str) -> Option<i64> {
    let lower = image.to_ascii_lowercase();
    for marker in ["java", "jdk", "jre", "temurin", "graalvm"] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(marker) {
            let rest = &lower[from + p + marker.len()..];
            let digits: String = rest
                .trim_start_matches(['_', ':', '-', '.'])
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = digits.parse::<i64>() {
                if (8..=40).contains(&n) {
                    return Some(n);
                }
            }
            from += p + marker.len();
        }
    }
    None
}

// --- Plan : parties pures (testées sans réseau) -----------------------------------------------------

pub(crate) fn plan_server(spec: &ServerSpec, f: &Facts) -> Vec<Step> {
    let mut out = Vec::new();
    if let Some(name) = spec
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        out.push(if name == f.name.trim() {
            Step::new("server.name", Status::Ok, format!("nom « {name} »"))
        } else {
            Step::new(
                "server.name",
                Status::Change,
                format!("renommer « {} » en « {name} »", f.name),
            )
            .values(Some(json!(f.name)), Some(json!(name)))
            .act(Action::Rename(name.to_string()))
        });
    }
    let same_game = match (spec.egg_id, spec.game.as_deref()) {
        (Some(id), _) if f.egg_id.is_some() => Some(f.egg_id == Some(id)),
        (_, Some(g)) => Some(norm(g) == norm(&f.egg_name)),
        _ => None,
    };
    if let Some(same) = same_game {
        let want = spec
            .game
            .clone()
            .unwrap_or_else(|| format!("egg {}", spec.egg_id.unwrap_or_default()));
        out.push(if same {
            Step::new("server.game", Status::Ok, format!("jeu {}", f.egg_name))
        } else {
            Step::new("server.game", Status::Manual, format!("jeu « {} » au lieu de « {want} » : changer de jeu impose de recréer ou réinstaller le serveur (panel)", f.egg_name))
                .values(Some(json!(f.egg_name)), Some(json!(want)))
        });
    }
    let mut versioned =
        |id: &str, label: &str, want: Option<&str>, have: Option<&str>, how: &str| {
            let Some(want) = want.map(str::trim).filter(|w| !w.is_empty()) else {
                return;
            };
            out.push(match have {
                Some(h) if h.eq_ignore_ascii_case(want) => {
                    Step::new(id, Status::Ok, format!("{label} {h}"))
                }
                Some(h) => Step::new(
                    id,
                    Status::Manual,
                    format!("{label} {h} au lieu de {want} : {how}"),
                )
                .values(Some(json!(h)), Some(json!(want))),
                None => Step::new(
                    id,
                    Status::Manual,
                    format!(
                        "{label} actuelle illisible (voulue : {want}) : à vérifier dans le panel"
                    ),
                )
                .values(None, Some(json!(want))),
            });
        };
    versioned(
        "server.game_version",
        "version du jeu",
        spec.game_version.as_deref(),
        f.game_version.as_deref(),
        "changement de version non géré par l'outil (panel → Versions, après une sauvegarde)",
    );
    versioned(
        "server.loader",
        "plateforme",
        spec.loader.as_deref(),
        f.loader.as_deref(),
        "changement de plateforme non géré par l'outil (panel → Versions)",
    );
    if let Some(want) = spec.java_version {
        let have = f.startup.as_ref().and_then(|s| java_major(&s.image));
        out.push(match have {
            Some(h) if h == want => Step::new("server.java", Status::Ok, format!("Java {h}")),
            Some(h) => Step::new("server.java", Status::Manual, format!("Java {h} au lieu de {want} : changement de version de Java non géré par l'outil (panel → Démarrage)"))
                .values(Some(json!(h)), Some(json!(want))),
            None => Step::new("server.java", Status::Manual, format!("version de Java actuelle illisible (voulue : {want}) : à vérifier dans le panel")).values(None, Some(json!(want))),
        });
    }
    out
}

pub(crate) fn plan_startup(spec: &StartupSpec, current: Option<&Startup>) -> Vec<Step> {
    let mut out = Vec::new();
    let Some(cur) = current else {
        if spec.parameters.is_some() || spec.jar_file.is_some() {
            out.push(Step::new(
                "startup",
                Status::Unsupported,
                "configuration de démarrage illisible pour ce serveur",
            ));
        }
        return out;
    };
    if let Some(p) = spec.parameters.as_deref() {
        out.push(if masked(p) {
            Step::new(
                "startup.parameters",
                Status::Ok,
                "commande masquée à l'export : la commande actuelle est gardée",
            )
        } else if p.trim() == cur.command.trim() {
            Step::new(
                "startup.parameters",
                Status::Ok,
                "commande de démarrage identique",
            )
        } else if p.trim().is_empty() {
            Step::new(
                "startup.parameters",
                Status::Unsupported,
                "commande de démarrage vide refusée",
            )
        } else {
            Step::new(
                "startup.parameters",
                Status::Change,
                "remplacer la commande de démarrage (effet au prochain démarrage)",
            )
            .values(Some(json!(cur.command)), Some(json!(p)))
            .act(Action::StartupParams(p.to_string()))
        });
    }
    if let Some(j) = spec
        .jar_file
        .as_deref()
        .map(str::trim)
        .filter(|j| !j.is_empty())
    {
        out.push(if j == cur.jar_file.trim() {
            Step::new("startup.jar_file", Status::Ok, format!("JAR {j}"))
        } else {
            Step::new("startup.jar_file", Status::Manual, format!("JAR « {} » au lieu de « {j} » : changement du fichier de démarrage non géré par l'outil (panel → Démarrage)", cur.jar_file))
                .values(Some(json!(cur.jar_file)), Some(json!(j)))
        });
    }
    out
}

pub(crate) fn plan_properties(
    spec: &Map<String, Value>,
    current: Option<&BTreeMap<String, String>>,
) -> Vec<Step> {
    let Some(cur) = current else {
        return vec![Step::new(
            "properties",
            Status::Unsupported,
            "réglages server.properties indisponibles pour ce serveur (Minecraft seulement)",
        )];
    };
    let mut out = Vec::new();
    for (k, v) in spec {
        let id = format!("properties.{k}");
        if v.is_object() || v.is_array() {
            out.push(Step::new(
                id,
                Status::Unsupported,
                format!("« {k} » : valeur texte, nombre ou booléen attendue"),
            ));
            continue;
        }
        let want = as_text(v);
        let secret = crate::models::is_secret_property(k) || config_edit::is_secret_key(k);
        let show = |s: &str| {
            if secret && !s.is_empty() {
                json!(config_edit::SECRET_MASK)
            } else {
                json!(s)
            }
        };
        if masked(&want) {
            out.push(Step::new(
                id,
                Status::Ok,
                format!("« {k} » masqué à l'export : valeur actuelle gardée"),
            ));
            continue;
        }
        out.push(match cur.get(k) {
            Some(have) if *have == want => Step::new(id, Status::Ok, format!("« {k} » conforme")),
            Some(have) => Step::new(
                id,
                Status::Change,
                format!("« {k} » : {} → {}", show(have), show(&want)),
            )
            .values(Some(show(have)), Some(show(&want)))
            .act(Action::Property {
                key: k.clone(),
                value: want,
            }),
            None => Step::new(
                id,
                Status::Add,
                format!("« {k} » absent de server.properties : ajouté au fichier (SFTP)"),
            )
            .values(None, Some(show(&want)))
            .act(Action::PropertyFile {
                key: k.clone(),
                value: want,
            }),
        });
    }
    out
}

/// Contenu installé (lecture tolérante de `GET /server/{id}/{plugins|mods|datapacks}`).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Installed {
    pub kind: Option<ContentKind>,
    pub name: String,
    pub file: String,
    pub version: String,
    pub enabled: bool,
    pub source: Option<String>,
    pub project: Option<String>,
    pub loader: String,
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Lecture tolérante d'une liste d'installés : `source`/`project_id` donnés par l'hébergeur
/// quand il a identifié l'élément.
pub(crate) fn parse_installed(kind: ContentKind, data: &Value) -> (Vec<Installed>, String) {
    let list: Vec<&Value> = match data.get(kind.as_str()) {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Object(m)) => m.values().collect(),
        _ => Vec::new(),
    };
    let items = list
        .into_iter()
        .filter_map(|v| {
            let name = text_of(v.get("name"));
            let file = match kind {
                ContentKind::Datapacks => name.clone(),
                _ => text_of(v.get("filename")),
            };
            if file.is_empty() {
                return None;
            }
            let opt = |k: &str| Some(text_of(v.get(k))).filter(|s| !s.trim().is_empty());
            Some(Installed {
                kind: Some(kind),
                name: if name.is_empty() { file.clone() } else { name },
                file,
                version: text_of(v.get("version")),
                enabled: match v.get("enabled") {
                    Some(Value::Bool(b)) => *b,
                    Some(Value::Number(n)) => n.as_i64() != Some(0),
                    Some(Value::String(s)) => !matches!(s.as_str(), "0" | "false"),
                    _ => true,
                },
                source: opt("source").map(|s| s.to_ascii_lowercase()),
                project: opt("project_id"),
                loader: text_of(v.get("loader")),
            })
        })
        .collect();
    (items, text_of(data.get("level_name")))
}

/// Singulier lisible d'un type de contenu.
fn kind_label(k: ContentKind) -> &'static str {
    match k {
        ContentKind::Mods => "mod",
        ContentKind::Plugins => "plugin",
        ContentKind::Datapacks => "datapack",
    }
}

/// Un installé correspond-il à l'élément voulu ? (même source et projet, même fichier, ou même nom)
fn content_matches(i: &Installed, w: &ContentItem, kind: ContentKind) -> bool {
    if i.kind != Some(kind) {
        return false;
    }
    if let (Some(is), Some(ip), Some(ws), Some(wp)) = (&i.source, &i.project, &w.source, &w.project)
    {
        if is.eq_ignore_ascii_case(ws) && ip == wp {
            return true;
        }
    }
    if w.file.as_deref().is_some_and(|f| f == i.file) {
        return true;
    }
    let wanted_name = w
        .name
        .as_deref()
        .or(w.project.as_deref())
        .map(norm)
        .unwrap_or_default();
    !wanted_name.is_empty()
        && (wanted_name == norm(&i.name)
            || norm(i.file.trim_end_matches(".jar").trim_end_matches(".zip")) == wanted_name)
}

/// Identifiant stable d'une étape de contenu.
fn content_id(kind: ContentKind, w: &ContentItem) -> String {
    let key = w
        .project
        .clone()
        .or_else(|| w.file.clone())
        .or_else(|| w.name.clone())
        .unwrap_or_else(|| "?".into());
    format!("content.{}.{key}", kind_label(kind))
}

/// Plan du contenu : présent et conforme, à installer, à changer de version, à retirer.
/// Les versions des installations sont résolues ensuite ([`resolve_content`]).
pub(crate) fn plan_content(
    items: &[ContentItem],
    remove_unlisted: bool,
    installed: &[Installed],
    unavailable: &[(ContentKind, String)],
    level_name: &str,
    server_loader: &str,
) -> (Vec<Step>, Vec<String>) {
    let mut steps = Vec::new();
    let mut used: HashSet<usize> = HashSet::new();
    for w in items {
        let Some(kind) = ContentKind::parse(&w.kind) else {
            continue;
        };
        let id = content_id(kind, w);
        let label = w
            .name
            .clone()
            .or_else(|| w.project.clone())
            .or_else(|| w.file.clone())
            .unwrap_or_default();
        if let Some((_, why)) = unavailable.iter().find(|(k, _)| *k == kind) {
            steps.push(Step::new(
                id,
                Status::Unsupported,
                format!(
                    "{} « {label} » : liste des {}s illisible ({why})",
                    kind_label(kind),
                    kind_label(kind)
                ),
            ));
            continue;
        }
        let found = installed
            .iter()
            .enumerate()
            .find(|(n, i)| !used.contains(n) && content_matches(i, w, kind));
        let loader = w
            .loader
            .clone()
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| server_loader.to_string());
        let install = |replaces: Option<String>| -> Option<Install> {
            let source = w.source.clone()?.to_ascii_lowercase();
            let project = w.project.clone()?;
            Some(Install {
                kind,
                source,
                project,
                version: w.version.clone().filter(|v| !v.trim().is_empty()),
                file: w.file.clone().filter(|v| !v.trim().is_empty()),
                loader: if kind == ContentKind::Mods {
                    loader.clone()
                } else {
                    String::new()
                },
                version_id: None,
                target_file: None,
                replaces,
                level_name: level_name.to_string(),
            })
        };
        match found {
            Some((n, inst)) => {
                used.insert(n);
                let file_ok = w.file.as_deref().is_none_or(|f| f == inst.file);
                let version_ok = w.file.is_some() || w.version.as_deref().is_none_or(|v| v == inst.version);
                let current = json!({ "file": inst.file, "version": inst.version, "enabled": inst.enabled });
                if file_ok && version_ok {
                    let mut s = Step::new(id, Status::Ok, format!("{} « {} » installé ({})", kind_label(kind), inst.name, inst.file)).values(Some(current), None);
                    if w.enabled.is_some_and(|e| e != inst.enabled) {
                        s.status = Status::Manual;
                        s.summary = format!(
                            "{} « {} » installé mais {} : (dés)activation non gérée par l'outil (onglet {}s du panel)",
                            kind_label(kind),
                            inst.name,
                            if inst.enabled { "activé" } else { "désactivé" },
                            kind_label(kind)
                        );
                    }
                    steps.push(s);
                    continue;
                }
                let target = json!({ "file": w.file, "version": w.version });
                match install(Some(inst.file.clone())) {
                    Some(i) => steps.push(
                        Step::new(id, Status::Change, format!("{} « {} » : version {} → {}", kind_label(kind), inst.name, inst.version, w.version.clone().or(w.file.clone()).unwrap_or_default()))
                            .values(Some(current), Some(target))
                            .act(Action::Install(i)),
                    ),
                    None => steps.push(
                        Step::new(id, Status::Manual, format!("{} « {} » : autre version voulue, mais source ou projet manquant (non installable par l'outil)", kind_label(kind), inst.name))
                            .values(Some(current), Some(target)),
                    ),
                }
            }
            None => match install(None) {
                Some(i) if matches!((kind, i.source.as_str()), (ContentKind::Plugins, "modrinth" | "spigot") | (ContentKind::Mods, "modrinth" | "curseforge") | (ContentKind::Datapacks, "modrinth" | "curseforge")) => steps.push(
                    Step::new(id, Status::Add, format!("installer le {} « {label} » ({})", kind_label(kind), i.source))
                        .values(None, Some(json!({ "source": i.source, "project": i.project, "version": i.version, "file": i.file })))
                        .act(Action::Install(i)),
                ),
                Some(i) => steps.push(Step::new(id, Status::Unsupported, format!("{} depuis {} : installation non prise en charge par l'hébergeur", kind_label(kind), i.source))),
                None => steps.push(Step::new(
                    id,
                    Status::Manual,
                    format!("{} « {label} » absent et non identifié (pas de source/projet) : à copier à la main (copy_between_servers, upload_file)", kind_label(kind)),
                )),
            },
        }
    }
    let mut unlisted = Vec::new();
    for (n, inst) in installed.iter().enumerate() {
        if used.contains(&n) {
            continue;
        }
        let Some(kind) = inst.kind else { continue };
        if remove_unlisted {
            steps.push(
                Step::new(
                    format!("content.{}.{}", kind_label(kind), inst.file),
                    Status::Remove,
                    format!(
                        "retirer le {} « {} » ({}), absent du manifeste",
                        kind_label(kind),
                        inst.name,
                        inst.file
                    ),
                )
                .values(
                    Some(json!({ "file": inst.file, "version": inst.version })),
                    None,
                )
                .act(Action::RemoveContent {
                    kind,
                    file: inst.file.clone(),
                    level_name: level_name.to_string(),
                }),
            );
        } else {
            unlisted.push(format!(
                "{} « {} » ({}) installé, absent du manifeste : gardé (remove_unlisted=false)",
                kind_label(kind),
                inst.name,
                inst.file
            ));
        }
    }
    (steps, unlisted)
}

/// Cron d'une tâche du panel, en une chaîne.
fn cron_text(c: &PanelCron) -> String {
    format!(
        "{} {} {} {} {}",
        c.minute, c.hour, c.day_of_month, c.month, c.day_of_week
    )
}

pub(crate) fn parse_cron(cron: &str) -> Result<PanelCron, String> {
    let f: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, day_of_month, month, day_of_week] = f.as_slice() else {
        return Err(format!(
            "cron « {cron} » : 5 champs attendus (minute heure jour-du-mois mois jour-de-semaine)"
        ));
    };
    Ok(PanelCron {
        minute: minute.to_string(),
        hour: hour.to_string(),
        day_of_month: day_of_month.to_string(),
        month: month.to_string(),
        day_of_week: day_of_week.to_string(),
    })
}

fn schedule_input(w: &ScheduleSpec) -> Result<PanelScheduleInput, String> {
    PanelScheduleInput {
        name: w.name.clone(),
        cron: parse_cron(&w.cron)?,
        is_active: w.is_active,
        only_when_online: w.only_when_online,
    }
    .validated()
    .map_err(|e| e.to_string())
}

fn task_input(t: &TaskSpec) -> Result<PanelTaskInput, String> {
    PanelTaskInput {
        action: t.action.trim().to_string(),
        payload: t.payload.clone(),
        time_offset: t.time_offset,
        continue_on_failure: t.continue_on_failure,
    }
    .validated()
    .map_err(|e| e.to_string())
}

fn spec_of(s: &PanelSchedule) -> ScheduleSpec {
    ScheduleSpec {
        name: s.name.clone(),
        cron: cron_text(&s.cron),
        is_active: s.is_active,
        only_when_online: s.only_when_online,
        tasks: s
            .tasks
            .iter()
            .map(|t| TaskSpec {
                action: t.action.clone(),
                payload: t.payload.clone(),
                time_offset: t.time_offset,
                continue_on_failure: t.continue_on_failure,
            })
            .collect(),
    }
}

pub(crate) fn plan_schedules(
    items: &[ScheduleSpec],
    remove_unlisted: bool,
    current: &[PanelSchedule],
) -> (Vec<Step>, Vec<String>) {
    let mut steps = Vec::new();
    let mut used = HashSet::new();
    for w in items {
        let name = w.name.trim();
        let id = format!("schedules.{name}");
        if let Some(bad) = schedule_input(w)
            .err()
            .or_else(|| w.tasks.iter().find_map(|t| task_input(t).err()))
        {
            steps.push(Step::new(
                id,
                Status::Unsupported,
                format!("tâche « {name} » invalide : {bad}"),
            ));
            continue;
        }
        if w.tasks.iter().any(|t| masked(&t.payload)) {
            steps.push(Step::new(id, Status::Unsupported, format!("tâche « {name} » : commande masquée à l'export (anonymisation) ; donne la vraie commande")));
            continue;
        }
        let found = current
            .iter()
            .filter(|c| c.name.trim() == name)
            .max_by_key(|c| c.id);
        let target = serde_json::to_value(w).ok();
        match found {
            None => steps.push(
                Step::new(
                    id,
                    Status::Add,
                    format!(
                        "créer la tâche planifiée « {name} » ({}, {} action(s))",
                        w.cron,
                        w.tasks.len()
                    ),
                )
                .values(None, target)
                .act(Action::CreateSchedule(w.clone())),
            ),
            Some(c) => {
                used.insert(c.id);
                let have = spec_of(c);
                let mut want = w.clone();
                want.cron = parse_cron(&w.cron)
                    .map(|p| cron_text(&p))
                    .unwrap_or(w.cron.clone());
                for t in &mut want.tasks {
                    t.payload = t.payload.trim().to_string();
                    t.action = t.action.trim().to_string();
                }
                want.name = have.name.clone();
                if want == have {
                    steps.push(Step::new(
                        id,
                        Status::Ok,
                        format!("tâche planifiée « {name} » conforme"),
                    ));
                } else {
                    steps.push(
                        Step::new(
                            id,
                            Status::Change,
                            format!("mettre à jour la tâche planifiée « {name} »"),
                        )
                        .values(serde_json::to_value(&have).ok(), target)
                        .act(Action::UpdateSchedule {
                            current: Box::new(c.clone()),
                            wanted: w.clone(),
                        }),
                    );
                }
            }
        }
    }
    let mut unlisted = Vec::new();
    for c in current.iter().filter(|c| !used.contains(&c.id)) {
        if remove_unlisted {
            steps.push(
                Step::new(
                    format!("schedules.{}", c.name.trim()),
                    Status::Remove,
                    format!(
                        "supprimer la tâche planifiée « {} », absente du manifeste",
                        c.name.trim()
                    ),
                )
                .act(Action::DeleteSchedule { id: c.id }),
            );
        } else {
            unlisted.push(format!(
                "tâche planifiée « {} » absente du manifeste : gardée (remove_unlisted=false)",
                c.name.trim()
            ));
        }
    }
    (steps, unlisted)
}

/// Un joueur d'une liste du serveur (`whitelist.json`, `ops.json`).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ListedPlayer {
    pub name: String,
    pub uuid: String,
}

pub(crate) fn parse_player_list(json_text: &str) -> Result<Vec<ListedPlayer>, String> {
    let v: Value = serde_json::from_str(json_text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("JSON invalide ({e})"))?;
    let list = v.as_array().ok_or("liste attendue")?;
    Ok(list
        .iter()
        .filter_map(|p| {
            let name = p.get("name").and_then(Value::as_str)?.to_string();
            Some(ListedPlayer {
                name,
                uuid: p
                    .get("uuid")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// Nom montré d'un joueur lu sur le serveur (alias stable si l'anonymisation IA est active).
fn shown_player(p: &ListedPlayer, anonymize: bool) -> String {
    if anonymize {
        crate::redact::pseudonym(if p.uuid.is_empty() { &p.name } else { &p.uuid })
    } else {
        p.name.clone()
    }
}

fn is_alias(name: &str) -> bool {
    name.strip_prefix("joueur-")
        .is_some_and(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

pub(crate) fn plan_players(
    spec: &PlayersSpec,
    whitelist: &[ListedPlayer],
    ops: &[ListedPlayer],
    anonymize: bool,
    online: Option<bool>,
) -> (Vec<Step>, Vec<String>) {
    let mut steps = Vec::new();
    let mut unlisted = Vec::new();
    let note = match online {
        Some(false) => " (serveur arrêté : à appliquer serveur démarré)",
        _ => "",
    };
    for (list, current, add, remove) in [
        (
            "whitelist",
            spec.whitelist.as_ref(),
            "whitelist_add",
            "whitelist_remove",
        ),
        ("ops", spec.ops.as_ref(), "op_add", "op_remove"),
    ] {
        let Some(wanted) = current else { continue };
        let have = if list == "whitelist" { whitelist } else { ops };
        let mut used = HashSet::new();
        for w in wanted.iter().map(|w| w.trim()).filter(|w| !w.is_empty()) {
            let id = format!("players.{list}.{w}");
            let found = have.iter().position(|p| {
                p.name.eq_ignore_ascii_case(w)
                    || (is_alias(w)
                        && (crate::redact::pseudonym(&p.uuid) == w.to_ascii_lowercase()
                            || crate::redact::pseudonym(&p.name) == w.to_ascii_lowercase()))
            });
            match found {
                Some(n) => {
                    used.insert(n);
                    steps.push(Step::new(id, Status::Ok, format!("« {w} » déjà dans {list}")));
                }
                None if is_alias(w) => steps.push(Step::new(id, Status::Manual, format!("« {w} » est un alias d'anonymisation : pseudo réel inconnu, à ajouter à la main"))),
                None => steps.push(Step::new(id, Status::Add, format!("ajouter « {w} » à {list}{note}")).act(Action::Player { action: add, name: w.to_string() })),
            }
        }
        for (_, p) in have.iter().enumerate().filter(|(n, _)| !used.contains(n)) {
            let shown = shown_player(p, anonymize);
            if spec.remove_unlisted {
                steps.push(
                    Step::new(
                        format!("players.{list}.{shown}"),
                        Status::Remove,
                        format!("retirer « {shown} » de {list}{note}"),
                    )
                    .act(Action::Player {
                        action: remove,
                        name: p.name.clone(),
                    }),
                );
            } else {
                unlisted.push(format!(
                    "« {shown} » dans {list}, absent du manifeste : gardé"
                ));
            }
        }
    }
    (steps, unlisted)
}

/// Une config ou un fichier sous `/plugins/<X>/` ou `/config/<x>` dépend de l'installation du
/// contenu `<X>` dans le même manifeste.
fn content_dependency(path: &str, steps: &[Step]) -> Vec<String> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let dir = match parts.as_slice() {
        ["plugins", d, _, ..] | ["config", d, _, ..] => norm(d),
        ["config", f] => norm(f.split(['.', '-']).next().unwrap_or(f)),
        _ => return Vec::new(),
    };
    if dir.is_empty() {
        return Vec::new();
    }
    steps
        .iter()
        .filter(|s| {
            s.id.starts_with("content.") && matches!(s.status, Status::Add | Status::Change)
        })
        .filter(|s| match &s.action {
            Some(Action::Install(i)) => {
                let names = [
                    Some(norm(&i.project)),
                    i.file.as_deref().map(norm),
                    s.target
                        .as_ref()
                        .and_then(|t| t.get("name"))
                        .and_then(Value::as_str)
                        .map(norm),
                ];
                names
                    .iter()
                    .flatten()
                    .any(|n| !n.is_empty() && (n.starts_with(&dir) || dir.starts_with(n.as_str())))
                    || norm(&s.summary).contains(&dir)
            }
            _ => false,
        })
        .map(|s| s.id.clone())
        .collect()
}

// --- Plan : lectures ---------------------------------------------------------------------------------

impl Core {
    /// Faits du serveur comparés au manifeste.
    pub(crate) async fn manifest_facts(&self, server_id: i64) -> Result<Facts, String> {
        let overview = self.list_servers().await.map_err(|e| e.to_string())?;
        let item = overview
            .servers
            .into_iter()
            .find(|s| s.id == server_id)
            .ok_or_else(|| format!("serveur {server_id} introuvable sur ce compte"))?;
        let mut f = Facts {
            name: item.name.clone(),
            family: item.capabilities.family.clone(),
            egg_id: item.egg_id,
            egg_name: item.egg_name.clone(),
            ..Default::default()
        };
        let token = self.token().map_err(|e| e.to_string())?;
        let (mc, startup) = tokio::join!(
            async {
                if f.minecraft() {
                    self.api.minecraft_version(&token, server_id).await.ok()
                } else {
                    None
                }
            },
            self.get_startup(server_id)
        );
        if let Some(mc) = mc {
            f.game_version = Some(mc.version).filter(|v| !v.is_empty());
            f.loader = Some(mc.source.to_ascii_lowercase())
                .filter(|s| !s.is_empty() && s != "explicit_jar");
        }
        f.startup = startup.ok();
        Ok(f)
    }

    /// Contenu installé (plugins, mods, datapacks) et dossier du monde ; types illisibles à part.
    pub(crate) async fn manifest_installed(
        &self,
        server_id: i64,
    ) -> Result<(Vec<Installed>, String, Vec<(ContentKind, String)>), String> {
        let token = self.token().map_err(|e| e.to_string())?;
        let reads = futures_util::future::join_all(ContentKind::ALL.iter().map(|k| {
            let path = format!("/server/{server_id}/{}", k.as_str());
            let token = token.clone();
            async move { (*k, self.api.get_data(&token, &path).await) }
        }))
        .await;
        let (mut all, mut level, mut failed) = (Vec::new(), String::new(), Vec::new());
        for (kind, r) in reads {
            match r {
                Ok(data) => {
                    let (items, lvl) = parse_installed(kind, &data);
                    if kind == ContentKind::Datapacks && !lvl.is_empty() {
                        level = lvl;
                    }
                    all.extend(items);
                }
                Err(e) => failed.push((kind, e.to_string())),
            }
        }
        Ok((all, level, failed))
    }

    /// Joueurs d'une liste (`/whitelist.json`, `/ops.json`) ; fichier absent = liste vide.
    async fn listed_players(
        &self,
        server_id: i64,
        file: &str,
    ) -> Result<Vec<ListedPlayer>, String> {
        let path = file.to_string();
        let text = self
            .with_sftp(server_id, |conn| {
                let path = path.clone();
                async move {
                    match crate::sftp::probe(&conn, &path).await? {
                        None => Ok(None),
                        Some(_) => crate::sftp::read_text(&conn, &path).await.map(Some),
                    }
                }
            })
            .await
            .map_err(|e| e.to_string())?;
        match text {
            None => Ok(Vec::new()),
            Some(t) if t.trim().is_empty() => Ok(Vec::new()),
            Some(t) => parse_player_list(&t).map_err(|e| format!("{file} : {e}")),
        }
    }

    /// Texte d'un fichier distant, `None` s'il n'existe pas.
    async fn remote_text(&self, server_id: i64, path: &str) -> Result<Option<String>, String> {
        let p = path.to_string();
        self.with_sftp(server_id, |conn| {
            let p = p.clone();
            async move {
                match crate::sftp::probe(&conn, &p).await? {
                    None => Ok(None),
                    Some(e) if e.kind == crate::sftp::EntryKind::Dir => {
                        Err(crate::Error::Invalid(format!("« {p} » est un dossier")))
                    }
                    Some(_) => crate::sftp::read_text(&conn, &p).await.map(Some),
                }
            }
        })
        .await
        .map_err(|e| e.to_string())
    }
}

/// Résout la version à installer (identifiant et fichier) d'après la marketplace.
async fn resolve_install(core: &Core, i: &mut Install, game_version: &str) -> Result<(), String> {
    let spigot = i.source == "spigot";
    let loader = if i.kind == ContentKind::Mods && !spigot {
        i.loader.clone()
    } else {
        String::new()
    };
    let gv = if spigot {
        String::new()
    } else {
        game_version.to_string()
    };
    let mut list = core
        .market_versions(&i.source, &i.project, &loader, &gv)
        .await
        .map_err(|e| e.to_string())?;
    let pick = |list: &[crate::MarketVersion]| -> Option<crate::MarketVersion> {
        let by_version = i.version.as_deref().and_then(|v| {
            list.iter()
                .find(|x| x.id == v || x.version_number == v || x.name == v)
        });
        let by_file = i
            .file
            .as_deref()
            .and_then(|f| list.iter().find(|x| x.filename.as_deref() == Some(f)));
        match (&i.version, &i.file) {
            (Some(_), _) => by_version.or(by_file).cloned(),
            (None, Some(_)) => by_file.cloned(),
            (None, None) => list.first().cloned(),
        }
    };
    let mut found = pick(&list);
    if found.is_none() && (!loader.is_empty() || !gv.is_empty()) {
        // Version épinglée hors des filtres (autre version du jeu) : sans filtre.
        list = core
            .market_versions(&i.source, &i.project, "", "")
            .await
            .map_err(|e| e.to_string())?;
        found = pick(&list);
    }
    let v = found.ok_or_else(|| match (&i.version, &i.file) {
        (Some(v), _) => format!(
            "version « {v} » introuvable pour {} « {} »",
            i.source, i.project
        ),
        (None, Some(f)) => format!(
            "fichier « {f} » introuvable parmi les versions de {} « {} »",
            i.source, i.project
        ),
        _ => format!(
            "aucune version compatible de {} « {} »",
            i.source, i.project
        ),
    })?;
    i.version_id = Some(v.id.clone());
    i.target_file = v.filename.clone();
    Ok(())
}

/// Résout les versions des étapes d'installation ; une version épinglée déjà installée (même
/// fichier) rend l'étape conforme.
async fn resolve_content(core: &Core, steps: &mut [Step], game_version: &str) {
    use futures_util::StreamExt;
    let todo: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.action, Some(Action::Install(_))) && s.status.actionable())
        .map(|(n, _)| n)
        .collect();
    let results: Vec<(usize, Result<Install, String>)> =
        futures_util::stream::iter(todo.into_iter().map(|n| {
            let Some(Action::Install(mut i)) = steps[n].action.clone() else {
                unreachable!()
            };
            async move {
                let r = resolve_install(core, &mut i, game_version).await.map(|_| i);
                (n, r)
            }
        }))
        .buffer_unordered(RESOLVE_CONCURRENCY)
        .collect()
        .await;
    for (n, r) in results {
        let s = &mut steps[n];
        match r {
            Ok(i) => {
                if s.status == Status::Change
                    && i.target_file.is_some()
                    && i.target_file == i.replaces
                {
                    s.status = Status::Ok;
                    s.summary = format!("{} (version voulue déjà installée)", s.summary);
                    s.action = None;
                    continue;
                }
                if let Some(t) = s.target.as_mut().and_then(Value::as_object_mut) {
                    t.insert("version_id".into(), json!(i.version_id));
                    t.insert("resolved_file".into(), json!(i.target_file));
                }
                s.action = Some(Action::Install(i));
            }
            Err(e) => {
                s.status = Status::Unsupported;
                s.summary = format!("{} : {e}", s.summary);
                s.action = None;
            }
        }
    }
}

/// Plan complet : lit le serveur, compare au manifeste. Ne modifie rien.
pub(crate) async fn build_plan(core: &Core, server_id: i64, m: &Manifest) -> Result<Plan, String> {
    let anonymize = core.get_privacy_config().redact_ai;
    let facts = core.manifest_facts(server_id).await?;
    let mut plan = Plan::default();
    let redact = |s: &str| crate::redact::redact(s);

    if let Some(s) = &m.server {
        plan.steps.extend(plan_server(s, &facts));
    }
    if let Some(s) = &m.startup {
        plan.steps.extend(plan_startup(s, facts.startup.as_ref()));
    }
    if let Some(p) = &m.properties {
        let current = if facts.minecraft() {
            core.server_properties(server_id)
                .await
                .ok()
                .map(|p| p.properties)
        } else {
            None
        };
        plan.steps.extend(plan_properties(p, current.as_ref()));
    }

    let (items, remove_unlisted) = m.content_items()?;
    if !items.is_empty() || remove_unlisted {
        if facts.minecraft() {
            let (installed, level, failed) = core.manifest_installed(server_id).await?;
            let (mut steps, unlisted) = plan_content(
                &items,
                remove_unlisted,
                &installed,
                &failed,
                &level,
                facts.loader.as_deref().unwrap_or(""),
            );
            resolve_content(
                core,
                &mut steps,
                facts.game_version.as_deref().unwrap_or(""),
            )
            .await;
            plan.steps.extend(steps);
            plan.unlisted.extend(unlisted);
        } else {
            plan.steps.push(Step::new(
                "content",
                Status::Unsupported,
                format!(
                    "contenu par manifeste : Minecraft seulement (ce serveur : {})",
                    facts.egg_name
                ),
            ));
        }
    }

    for c in &m.configs {
        let path = crate::fleet::normalize_path(&c.path).map_err(|e| e.to_string())?;
        let id = format!("configs.{path}");
        let requests: Vec<EditRequest> = c
            .set
            .iter()
            .map(|(k, v)| EditRequest {
                key: k.clone(),
                value: v.clone(),
            })
            .collect();
        let depends = content_dependency(&path, &plan.steps);
        let format = Format::detect(&path).unwrap_or(Format::Yaml);
        let step = match core.remote_text(server_id, &path).await {
            Err(e) => Step::new(id, Status::Unsupported, format!("« {path} » illisible : {e}")),
            Ok(None) if !depends.is_empty() => Step::new(
                id,
                Status::Manual,
                format!("« {path} » sera généré par le contenu installé au premier démarrage : redémarre le serveur puis relance apply_manifest (idempotent)"),
            ),
            Ok(None) => Step::new(id, Status::Manual, format!("« {path} » absent : démarre le serveur une fois (le plugin ou mod le génère), puis relance le plan")),
            Ok(Some(text)) => {
                let opts = EditOptions { skip_masked: true, ..Default::default() };
                match config_edit::edit_text(format, &path, &text, &requests, opts, &redact) {
                    Err(e) => Step::new(id, Status::Unsupported, format!("« {path} » : {e}")),
                    Ok(o) if !o.changed => Step::new(id, Status::Ok, format!("« {path} » conforme ({} clé(s))", requests.len())),
                    Ok(o) => {
                        let mut s = Step::new(
                            id,
                            Status::Change,
                            format!("« {path} » : {} clé(s) à modifier", o.changes.iter().filter(|c| matches!(c.action, "modifiée" | "créée")).count()),
                        )
                        .act(Action::Config { path: path.clone(), set: requests });
                        s.diff = Some(o.diff);
                        s
                    }
                }
            }
        };
        let mut step = step;
        step.depends_on = depends;
        plan.steps.push(step);
    }

    for f in &m.files {
        let path = crate::fleet::normalize_path(&f.path).map_err(|e| e.to_string())?;
        let id = format!("files.{path}");
        let depends = content_dependency(&path, &plan.steps);
        let mut step = if path == "/" {
            Step::new(id, Status::Unsupported, "chemin « / » refusé")
        } else if masked(&f.content) {
            Step::new(id, Status::Unsupported, format!("« {path} » : le contenu contient un masque d'anonymisation ([SECRET], [IP]…) ; l'écrire écraserait les vraies valeurs"))
        } else if f.content.len() > MAX_FILE_BYTES {
            Step::new(
                id,
                Status::Unsupported,
                format!(
                    "« {path} » : {} octets, {} Kio au plus (utilise upload_file)",
                    f.content.len(),
                    MAX_FILE_BYTES / 1024
                ),
            )
        } else {
            match core.remote_text(server_id, &path).await {
                Err(e) => Step::new(
                    id,
                    Status::Unsupported,
                    format!("« {path} » illisible : {e}"),
                ),
                Ok(Some(t)) if t == f.content => {
                    Step::new(id, Status::Ok, format!("« {path} » identique"))
                }
                Ok(cur) => {
                    let (a, b) = (cur.clone().unwrap_or_default(), f.content.clone());
                    let mask = |t: &str| match Format::detect(&path) {
                        Some(fmt) => config_edit::masked_text(fmt, t),
                        None => t.to_string(),
                    };
                    let (diff, _) = crate::fleet::unified_diff(
                        &mask(&a),
                        &mask(&b),
                        &format!("a{path}"),
                        &format!("b{path}"),
                        3,
                        &redact,
                    );
                    let mut s = Step::new(
                        id,
                        if cur.is_some() {
                            Status::Change
                        } else {
                            Status::Add
                        },
                        format!(
                            "{} « {path} » ({} octets)",
                            if cur.is_some() {
                                "réécrire"
                            } else {
                                "créer"
                            },
                            b.len()
                        ),
                    )
                    .act(Action::WriteFile {
                        path: path.clone(),
                        content: f.content.clone(),
                    });
                    s.diff = Some(diff);
                    s
                }
            }
        };
        step.depends_on = depends;
        plan.steps.push(step);
    }

    if let Some(p) = &m.players {
        if facts.minecraft() {
            let (wl, ops) = tokio::join!(
                core.listed_players(server_id, "/whitelist.json"),
                core.listed_players(server_id, "/ops.json")
            );
            let online = core
                .server_live(server_id)
                .await
                .ok()
                .map(|l| l.state == "online");
            match (wl, ops) {
                (Ok(wl), Ok(ops)) => {
                    let (steps, unlisted) = plan_players(p, &wl, &ops, anonymize, online);
                    plan.steps.extend(steps);
                    plan.unlisted.extend(unlisted);
                }
                (Err(e), _) | (_, Err(e)) => plan.steps.push(Step::new(
                    "players",
                    Status::Unsupported,
                    format!("listes de joueurs illisibles : {e}"),
                )),
            }
        } else {
            plan.steps.push(Step::new(
                "players",
                Status::Unsupported,
                "liste blanche et opérateurs : Minecraft seulement",
            ));
        }
    }

    let (sched, remove_sched) = m.schedule_items()?;
    if !sched.is_empty() || remove_sched {
        match core.panel_schedules(server_id).await {
            Ok(current) => {
                let (steps, unlisted) = plan_schedules(&sched, remove_sched, &current);
                plan.steps.extend(steps);
                plan.unlisted.extend(unlisted);
            }
            Err(e) => plan.steps.push(Step::new(
                "schedules",
                Status::Unsupported,
                format!("tâches planifiées illisibles : {e}"),
            )),
        }
    }

    // Identifiants uniques (deux éléments du même nom) : suffixe de rang.
    let mut seen: HashSet<String> = HashSet::new();
    for s in &mut plan.steps {
        let base = s.id.clone();
        let mut n = 2;
        while !seen.insert(s.id.clone()) {
            s.id = format!("{base}#{n}");
            n += 1;
        }
    }
    Ok(plan)
}

/// Rendu JSON d'un plan.
pub(crate) fn render_plan(server_id: i64, plan: &Plan) -> Value {
    let mut counts = BTreeMap::new();
    for s in &plan.steps {
        *counts
            .entry(
                serde_json::to_value(s.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
            )
            .or_insert(0) += 1;
    }
    let todo = plan.steps.iter().filter(|s| s.status.actionable()).count();
    let mut out = json!({
        "server_id": server_id,
        "counts": counts,
        "to_apply": todo,
        "steps": plan.steps,
    });
    if !plan.unlisted.is_empty() {
        out["unlisted"] = json!(plan.unlisted);
    }
    if !plan.warnings.is_empty() {
        out["warnings"] = json!(plan.warnings);
    }
    out["next"] = json!(if todo == 0 {
        "rien à appliquer : le serveur est conforme (étapes manual/unsupported éventuelles à traiter à la main)"
    } else {
        "apply_manifest avec le même manifeste applique les étapes change/add/remove (snapshot préalable par défaut) ; `steps` pour n'en appliquer qu'une partie (identifiants ou préfixes : \"properties\", \"content.plugin\")"
    });
    out
}

// --- Application ---------------------------------------------------------------------------------------

/// Sélection d'étapes : identifiant exact, ou préfixe suivi d'un point (`properties`, `content.plugin`).
fn selected(id: &str, filter: &[String]) -> bool {
    filter.is_empty()
        || filter.iter().any(|f| {
            let f = f.trim().trim_end_matches('.');
            id == f || id.starts_with(&format!("{f}."))
        })
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StepResult {
    pub id: String,
    pub status: Status,
    /// `réussie`, `conforme`, `ignorée`, `échouée`, `non sélectionnée`, `non exécutée`.
    pub outcome: &'static str,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Exécute une action ; renvoie un détail lisible.
async fn execute(core: &Core, server_id: i64, action: &Action) -> Result<Option<String>, String> {
    let es = |e: crate::Error| e.to_string();
    match action {
        Action::Rename(name) => core
            .rename_server(server_id, name)
            .await
            .map(|_| None)
            .map_err(es),
        Action::StartupParams(p) => core
            .set_startup_params(server_id, p)
            .await
            .map(|_| None)
            .map_err(es),
        Action::Property { key, value } => core
            .set_server_property(server_id, key, value)
            .await
            .map(|_| None)
            .map_err(es),
        Action::PropertyFile { key, value } => {
            let reqs = vec![EditRequest {
                key: key.clone(),
                value: Value::String(value.clone()),
            }];
            core.config_edit(
                server_id,
                "/server.properties",
                &reqs,
                EditOptions::default(),
                false,
            )
            .await
            .map(|_| None)
        }
        Action::Install(i) => Box::pin(install(core, server_id, i)).await,
        Action::RemoveContent {
            kind,
            file,
            level_name,
        } => {
            let token = core.token().map_err(es)?;
            match kind {
                ContentKind::Mods => core.api.delete_mod_file(&token, server_id, file).await,
                ContentKind::Plugins => core.api.delete_plugin_file(&token, server_id, file).await,
                ContentKind::Datapacks => {
                    let level = if level_name.is_empty() {
                        "world"
                    } else {
                        level_name
                    };
                    core.api
                        .delete_files(&token, server_id, &[format!("{level}/datapacks/{file}")])
                        .await
                }
            }
            .map(|_| Some("effet au prochain démarrage".into()))
            .map_err(es)
        }
        Action::Config { path, set } => {
            let opts = EditOptions {
                skip_masked: true,
                ..Default::default()
            };
            let (_, o, written) = core.config_edit(server_id, path, set, opts, false).await?;
            Ok(Some(if written {
                format!(
                    "{} clé(s) écrite(s)",
                    o.changes
                        .iter()
                        .filter(|c| matches!(c.action, "modifiée" | "créée"))
                        .count()
                )
            } else {
                "déjà conforme".into()
            }))
        }
        Action::WriteFile { path, content } => {
            let (p, body) = (path.clone(), content.clone());
            core.with_sftp(server_id, |conn| {
                let (p, body) = (p.clone(), body.clone());
                async move {
                    let parent = p.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    if !parent.is_empty() {
                        crate::sftp::ensure_dir(&conn, parent).await;
                    }
                    crate::sftp::write_text(&conn, &p, &body).await
                }
            })
            .await
            .map(|_| None)
            .map_err(es)
        }
        Action::Player { action, name } => core
            .player_action(server_id, action, name)
            .await
            .map(|_| None)
            .map_err(es),
        Action::CreateSchedule(w) => {
            let input = schedule_input(w)?;
            let tasks = w
                .tasks
                .iter()
                .map(task_input)
                .collect::<Result<Vec<_>, _>>()?;
            let name = input.name.clone();
            core.create_panel_schedule(server_id, input)
                .await
                .map_err(es)?;
            if tasks.is_empty() {
                return Ok(None);
            }
            let created = core
                .panel_schedules(server_id)
                .await
                .map_err(es)?
                .into_iter()
                .filter(|s| s.name.trim() == name)
                .map(|s| s.id)
                .max()
                .ok_or(
                    "tâche créée mais introuvable : ajoute ses actions avec add_schedule_task",
                )?;
            let total = tasks.len();
            for (done, t) in tasks.into_iter().enumerate() {
                core.add_panel_task(server_id, created, t)
                    .await
                    .map_err(|e| {
                        format!("tâche {created} créée, {done}/{total} action(s) ajoutée(s) : {e}")
                    })?;
            }
            Ok(Some(format!(
                "tâche {created} créée avec {total} action(s)"
            )))
        }
        Action::UpdateSchedule { current, wanted } => {
            let input = schedule_input(wanted)?;
            let tasks = wanted
                .tasks
                .iter()
                .map(task_input)
                .collect::<Result<Vec<_>, _>>()?;
            let head_differs = cron_text(&current.cron) != cron_text(&input.cron)
                || current.is_active != input.is_active
                || current.only_when_online != input.only_when_online
                || current.name.trim() != input.name;
            if head_differs {
                core.update_panel_schedule(server_id, current.id, input)
                    .await
                    .map_err(es)?;
            }
            for (n, t) in tasks.into_iter().enumerate() {
                match current.tasks.get(n) {
                    Some(c)
                        if c.action == t.action
                            && c.payload == t.payload
                            && c.time_offset == t.time_offset
                            && c.continue_on_failure == t.continue_on_failure => {}
                    Some(c) => core
                        .update_panel_task(server_id, current.id, c.id, t)
                        .await
                        .map_err(es)?,
                    None => core
                        .add_panel_task(server_id, current.id, t)
                        .await
                        .map_err(es)?,
                }
            }
            for c in current.tasks.iter().skip(wanted.tasks.len()) {
                core.delete_panel_task(server_id, current.id, c.id)
                    .await
                    .map_err(es)?;
            }
            Ok(None)
        }
        Action::DeleteSchedule { id } => core
            .delete_panel_schedule(server_id, *id)
            .await
            .map(|_| None)
            .map_err(es),
    }
}

async fn install(core: &Core, server_id: i64, i: &Install) -> Result<Option<String>, String> {
    let es = |e: crate::Error| e.to_string();
    let version_id = i.version_id.clone().ok_or("version non résolue")?;
    let token = core.token().map_err(es)?;
    match (i.kind, i.source.as_str()) {
        (ContentKind::Plugins | ContentKind::Mods, "modrinth" | "spigot") => core
            .install_mod(
                server_id,
                &i.source,
                kind_label(i.kind),
                &i.project,
                &version_id,
                &i.loader,
            )
            .await
            .map_err(es)?,
        (ContentKind::Mods, "curseforge") => {
            let (m, f) = (
                i.project
                    .parse::<i64>()
                    .map_err(|_| "identifiant CurseForge numérique attendu")?,
                version_id
                    .parse::<i64>()
                    .map_err(|_| "fichier CurseForge numérique attendu")?,
            );
            core.api
                .install_curseforge_mod(&token, server_id, m, f, &i.loader)
                .await
                .map_err(es)?
        }
        (ContentKind::Datapacks, "modrinth") => core
            .api
            .install_modrinth_datapack(&token, server_id, &i.project, &version_id)
            .await
            .map_err(es)?,
        (ContentKind::Datapacks, "curseforge") => {
            let (m, f) = (
                i.project
                    .parse::<i64>()
                    .map_err(|_| "identifiant CurseForge numérique attendu")?,
                version_id
                    .parse::<i64>()
                    .map_err(|_| "fichier CurseForge numérique attendu")?,
            );
            core.api
                .install_curseforge_datapack(&token, server_id, m, f)
                .await
                .map_err(es)?
        }
        (k, s) => return Err(format!("{} depuis {s} : non pris en charge", kind_label(k))),
    }
    // Ancien fichier d'une autre version : retiré s'il porte un autre nom (comme les mises à jour).
    if let (Some(old), Some(new)) = (&i.replaces, &i.target_file) {
        if old != new {
            let r = match i.kind {
                ContentKind::Mods => core.api.delete_mod_file(&token, server_id, old).await,
                ContentKind::Plugins => core.api.delete_plugin_file(&token, server_id, old).await,
                ContentKind::Datapacks => {
                    let level = if i.level_name.is_empty() {
                        "world"
                    } else {
                        &i.level_name
                    };
                    core.api
                        .delete_files(&token, server_id, &[format!("{level}/datapacks/{old}")])
                        .await
                }
            };
            if let Err(e) = r {
                return Ok(Some(format!("installé, mais l'ancien fichier « {old} » n'a pas pu être retiré ({e}) : retire-le à la main")));
            }
        }
    }
    Ok(Some(format!(
        "version {} installée ; effet au prochain démarrage",
        i.target_file.clone().unwrap_or(version_id)
    )))
}

/// Attend la fin d'une snapshot, avec progression : suivi du job de sauvegarde, et à défaut la
/// liste des snapshots (la snapshot nommée `name` devient disponible).
async fn wait_snapshot(core: &Core, job: i64, name: &str) -> Result<(), String> {
    let start = Instant::now();
    loop {
        tokio::time::sleep(SNAPSHOT_POLL).await;
        if crate::mcp_ctx::cancelled() {
            return Err("annulé pendant la snapshot préalable".into());
        }
        if start.elapsed() > SNAPSHOT_WAIT {
            return Err(format!(
                "snapshot préalable (job {job}) toujours en cours après {} min",
                SNAPSHOT_WAIT.as_secs() / 60
            ));
        }
        match core.backup_job(job).await {
            Ok(s) => match s.status.as_str() {
                "completed" => return Ok(()),
                "failed" | "cancelled" => {
                    return Err(format!(
                        "snapshot préalable en échec ({})",
                        s.error.unwrap_or(s.status)
                    ))
                }
                _ => {
                    crate::mcp_ctx::progress(
                        0.0,
                        None,
                        &format!("snapshot préalable : {} %", s.progress),
                    )
                    .await
                }
            },
            Err(_) => {
                let list = core.list_snapshots().await.map_err(|e| e.to_string())?;
                let mine = list
                    .iter()
                    .find(|s| s.job_id == Some(job) || s.name == name);
                match mine {
                    Some(s) if s.id.is_some() && s.status == 1 => return Ok(()),
                    Some(s) => {
                        crate::mcp_ctx::progress(
                            0.0,
                            None,
                            &format!("snapshot préalable : {} %", s.progress.unwrap_or(0)),
                        )
                        .await
                    }
                    None => {
                        crate::mcp_ctx::progress(0.0, None, "snapshot préalable en attente").await
                    }
                }
            }
        }
    }
}

/// Options d'application.
#[derive(Debug, Clone, Default)]
pub(crate) struct ApplyOptions {
    pub steps: Vec<String>,
    pub snapshot: bool,
}

/// Applique un manifeste : plan recalculé, étapes sélectionnées exécutées dans l'ordre.
pub(crate) async fn apply(
    core: &Core,
    server_id: i64,
    m: &Manifest,
    opts: &ApplyOptions,
) -> Result<Value, String> {
    crate::mcp_ctx::progress(0.0, None, "lecture du serveur et calcul du plan").await;
    let plan = Box::pin(build_plan(core, server_id, m)).await?;
    let todo: Vec<&Step> = plan
        .steps
        .iter()
        .filter(|s| s.status.actionable() && selected(&s.id, &opts.steps) && s.action.is_some())
        .collect();
    let total = todo.len();
    let mut results: Vec<StepResult> = Vec::new();
    let mut snapshot = Value::Null;
    if total > 0 && opts.snapshot {
        crate::mcp_ctx::progress(0.0, Some(total as f64 + 1.0), "snapshot préalable").await;
        let name = format!(
            "Avant manifeste {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        let job = core.create_snapshot(server_id, &name).await.map_err(|e| format!("snapshot préalable impossible ({e}) : rien n'a été modifié ; relance avec snapshot=false pour passer outre"))?;
        wait_snapshot(core, job, &name).await.map_err(|e| format!("{e} : rien n'a été modifié ; relance plus tard (idempotent) ou avec snapshot=false"))?;
        snapshot = json!({ "name": name, "job_id": job });
    }
    let mut failed: HashSet<String> = HashSet::new();
    let mut done = 0usize;
    let mut cancelled = false;
    for s in &plan.steps {
        let base = |outcome: &'static str, detail: Option<String>| StepResult {
            id: s.id.clone(),
            status: s.status,
            outcome,
            summary: s.summary.clone(),
            detail,
        };
        if !s.status.actionable() {
            results.push(base(
                if s.status == Status::Ok {
                    "conforme"
                } else {
                    "ignorée"
                },
                None,
            ));
            continue;
        }
        if !selected(&s.id, &opts.steps) {
            results.push(base("non sélectionnée", None));
            continue;
        }
        if cancelled || crate::mcp_ctx::cancelled() {
            cancelled = true;
            results.push(base("non exécutée", Some("annulé".into())));
            continue;
        }
        if let Some(dep) = s.depends_on.iter().find(|d| failed.contains(*d)) {
            failed.insert(s.id.clone());
            results.push(base(
                "ignorée",
                Some(format!("dépend de « {dep} », en échec")),
            ));
            continue;
        }
        let Some(action) = &s.action else {
            results.push(base("ignorée", Some("aucune action".into())));
            continue;
        };
        crate::mcp_ctx::progress(done as f64 + 1.0, Some(total as f64 + 1.0), &s.summary).await;
        match Box::pin(execute(core, server_id, action)).await {
            Ok(detail) => results.push(base("réussie", detail)),
            Err(e) => {
                failed.insert(s.id.clone());
                results.push(base("échouée", Some(e)));
            }
        }
        done += 1;
    }
    let count = |o: &str| results.iter().filter(|r| r.outcome == o).count();
    let restart = results.iter().any(|r| {
        r.outcome == "réussie"
            && (r.id.starts_with("content.")
                || r.id.starts_with("properties.")
                || r.id.starts_with("startup.")
                || r.id.starts_with("configs."))
    });
    let mut out = json!({
        "server_id": server_id,
        "status": if cancelled { "annulé" } else if count("échouée") > 0 { "terminé avec des échecs" } else { "terminé" },
        "snapshot": snapshot,
        "applied": count("réussie"),
        "failed": count("échouée"),
        "skipped": count("ignorée") + count("non sélectionnée") + count("non exécutée"),
        "already_ok": count("conforme"),
        "steps": results,
    });
    if !plan.unlisted.is_empty() {
        out["unlisted"] = json!(plan.unlisted);
    }
    let manual: Vec<&str> = plan
        .steps
        .iter()
        .filter(|s| matches!(s.status, Status::Manual | Status::Unsupported))
        .map(|s| s.summary.as_str())
        .collect();
    if !manual.is_empty() {
        out["manual"] = json!(manual);
    }
    if restart {
        out["next"] = json!("redémarre le serveur (power_action restart) pour charger contenu, réglages et configs ; relance plan_manifest pour vérifier");
    }
    Ok(out)
}

// --- Export ------------------------------------------------------------------------------------------------

/// Sélection de config à exporter : chemin, et clés (toutes les valeurs si vide).
#[derive(Debug, Clone)]
pub(crate) struct ConfigSelection {
    pub path: String,
    pub keys: Vec<String>,
}

pub(crate) fn parse_config_selection(v: Option<&Value>) -> Result<Vec<ConfigSelection>, String> {
    let Some(Value::Array(items)) = v else {
        return match v {
            None | Some(Value::Null) => Ok(Vec::new()),
            _ => Err("`include_configs` : tableau de chemins ou de { path, keys } attendu".into()),
        };
    };
    items
        .iter()
        .map(|it| match it {
            Value::String(p) => Ok(ConfigSelection {
                path: p.clone(),
                keys: Vec::new(),
            }),
            Value::Object(m) => Ok(ConfigSelection {
                path: m
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("`include_configs[].path` requis")?
                    .to_string(),
                keys: m
                    .get("keys")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            _ => Err("`include_configs` : chemin ou { path, keys } attendu".into()),
        })
        .collect()
}

/// Clés « feuilles » d'un arbre (`a.b.c` → valeur), listes de scalaires d'un bloc.
fn flatten_leaves(tree: &Value, max: usize) -> (Map<String, Value>, bool) {
    fn walk(
        v: &Value,
        at: &mut Vec<config_edit::Seg>,
        out: &mut Map<String, Value>,
        max: usize,
    ) -> bool {
        if out.len() >= max {
            return true;
        }
        match v {
            Value::Object(m) if !m.is_empty() => {
                for (k, x) in m {
                    at.push(config_edit::Seg::Key(k.clone()));
                    let cut = walk(x, at, out, max);
                    at.pop();
                    if cut {
                        return true;
                    }
                }
                false
            }
            Value::Array(a) if a.iter().any(|x| x.is_object() || x.is_array()) => {
                for (i, x) in a.iter().enumerate() {
                    at.push(config_edit::Seg::Index(i));
                    let cut = walk(x, at, out, max);
                    at.pop();
                    if cut {
                        return true;
                    }
                }
                false
            }
            _ => {
                if !at.is_empty() {
                    out.insert(config_edit::render_key_path(at), v.clone());
                }
                false
            }
        }
    }
    let mut out = Map::new();
    let cut = walk(tree, &mut Vec::new(), &mut out, max);
    (out, cut)
}

/// Manifeste d'un serveur existant. `anonymize` : pseudos remplacés par leur alias. Secrets
/// toujours masqués. Renvoie le manifeste et des remarques.
pub(crate) async fn export(
    core: &Core,
    server_id: i64,
    configs: &[ConfigSelection],
    include_players: bool,
    anonymize: bool,
) -> Result<(Manifest, Vec<String>), String> {
    let facts = core.manifest_facts(server_id).await?;
    let mut notes = Vec::new();
    let mut m = Manifest {
        version: MANIFEST_VERSION,
        ..Default::default()
    };
    m.server = Some(ServerSpec {
        name: Some(facts.name.clone()),
        game: Some(facts.egg_name.clone()).filter(|g| !g.is_empty()),
        egg_id: facts.egg_id,
        game_version: facts.game_version.clone(),
        loader: facts.loader.clone(),
        java_version: facts.startup.as_ref().and_then(|s| java_major(&s.image)),
    });
    if let Some(s) = &facts.startup {
        m.startup = Some(StartupSpec {
            parameters: Some(s.command.clone()).filter(|c| !c.is_empty()),
            jar_file: Some(s.jar_file.clone()).filter(|j| !j.is_empty()),
        });
    }
    if facts.minecraft() {
        match core.server_properties(server_id).await {
            Ok(p) => {
                let mut secrets = 0;
                let props: Map<String, Value> = p
                    .properties
                    .into_iter()
                    .filter(|(k, _)| !ALLOCATION_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| {
                        let secret = (crate::models::is_secret_property(&k)
                            || config_edit::is_secret_key(&k))
                            && !v.is_empty();
                        secrets += usize::from(secret);
                        (
                            k,
                            Value::String(if secret {
                                config_edit::SECRET_MASK.into()
                            } else {
                                v
                            }),
                        )
                    })
                    .collect();
                if secrets > 0 {
                    notes.push(format!("{secrets} réglage(s) secret(s) masqué(s) ([SECRET] = valeur gardée à l'application)"));
                }
                notes.push(format!(
                    "non exportés car propres à l'allocation du serveur : {}",
                    ALLOCATION_KEYS.join(", ")
                ));
                m.properties = Some(props);
            }
            Err(e) => notes.push(format!("server.properties illisible : {e}")),
        }
        let (installed, _, failed) = core.manifest_installed(server_id).await?;
        for (k, e) in failed {
            notes.push(format!("liste des {}s illisible : {e}", kind_label(k)));
        }
        let unidentified = installed
            .iter()
            .filter(|i| i.source.is_none() || i.project.is_none())
            .count();
        if unidentified > 0 {
            notes.push(format!("{unidentified} contenu(s) non identifié(s) par l'hébergeur (source null) : non réinstallables par le manifeste, à copier (copy_between_servers)"));
        }
        let items: Vec<ContentItem> = installed
            .iter()
            .filter_map(|i| {
                Some(ContentItem {
                    kind: kind_label(i.kind?).into(),
                    source: i.source.clone(),
                    project: i.project.clone(),
                    version: Some(i.version.clone()).filter(|v| !v.is_empty()),
                    file: Some(i.file.clone()),
                    name: Some(i.name.clone()),
                    loader: Some(i.loader.clone()).filter(|l| !l.is_empty()),
                    enabled: (!i.enabled).then_some(false),
                })
            })
            .collect();
        if !items.is_empty() {
            m.content = Some(json!({ "items": items, "remove_unlisted": false }));
        }
        if include_players {
            let (wl, ops) = tokio::join!(
                core.listed_players(server_id, "/whitelist.json"),
                core.listed_players(server_id, "/ops.json")
            );
            match (wl, ops) {
                (Ok(wl), Ok(ops)) => {
                    let names = |l: &[ListedPlayer]| {
                        l.iter()
                            .map(|p| shown_player(p, anonymize))
                            .collect::<Vec<_>>()
                    };
                    if anonymize && (!wl.is_empty() || !ops.is_empty()) {
                        notes.push("pseudos remplacés par leur alias (anonymisation IA active) : reconnus sur ce serveur, à remplacer par les vrais pseudos pour un autre".into());
                    }
                    m.players = Some(PlayersSpec {
                        whitelist: Some(names(&wl)),
                        ops: Some(names(&ops)),
                        remove_unlisted: false,
                    });
                }
                (Err(e), _) | (_, Err(e)) => {
                    notes.push(format!("listes de joueurs illisibles : {e}"))
                }
            }
        }
    }
    match core.panel_schedules(server_id).await {
        Ok(list) if !list.is_empty() => {
            m.schedules = Some(
                json!({ "items": list.iter().map(spec_of).collect::<Vec<_>>(), "remove_unlisted": false }),
            )
        }
        Ok(_) => {}
        Err(e) => notes.push(format!("tâches planifiées illisibles : {e}")),
    }
    for sel in configs {
        let (path, format, text) = match core.config_read(server_id, &sel.path).await {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("config « {} » illisible : {e}", sel.path));
                continue;
            }
        };
        let tree = match format.tree(text.trim_start_matches('\u{feff}')) {
            Ok(t) => t,
            Err(e) => {
                notes.push(format!("config « {path} » illisible : {e}"));
                continue;
            }
        };
        let mut set = Map::new();
        if sel.keys.is_empty() {
            let (leaves, cut) = flatten_leaves(&tree, MAX_EXPORT_KEYS);
            if cut {
                notes.push(format!(
                    "config « {path} » : {MAX_EXPORT_KEYS} clés exportées au plus, précise `keys`"
                ));
            }
            for (k, v) in leaves {
                let segs = format.parse_key(&k).unwrap_or_default();
                set.insert(
                    k,
                    config_edit::mask_value(&v, config_edit::path_is_secret(&segs)).0,
                );
            }
        } else {
            for k in &sel.keys {
                let segs = format.parse_key(k)?;
                match config_edit::get_path(&tree, &segs) {
                    Some(v) => {
                        set.insert(
                            k.clone(),
                            config_edit::mask_value(v, config_edit::path_is_secret(&segs)).0,
                        );
                    }
                    None => notes.push(format!("config « {path} » : clé « {k} » absente")),
                }
            }
        }
        m.configs.push(ConfigSpec { path, set });
    }
    notes.push("non exporté : mondes et fichiers (copy_between_servers ou clone_server pour les reprendre)".into());
    Ok((m, notes))
}

// --- Outils MCP ----------------------------------------------------------------------------------------------

/// Schéma JSON du paramètre `manifest` (sections décrites, contenu libre : validé à la lecture).
pub(crate) fn manifest_schema() -> Value {
    let any_list = |d: &str| json!({ "description": d });
    json!({
        "type": "object",
        "description": "Manifeste v1 (voir la description de plan_manifest) ; toute section facultative",
        "properties": {
            "version": { "type": "integer", "description": "1" },
            "server": { "type": "object", "description": "{ name?, game?, egg_id?, game_version?, loader?, java_version? } : seul le nom est modifiable, le reste est comparé (manual si différent)" },
            "properties": { "type": "object", "description": "{ clé de server.properties: valeur }" },
            "startup": { "type": "object", "description": "{ parameters?: commande complète avec {{SERVER_JARFILE}}, jar_file? }" },
            "content": any_list("[{ kind: plugin|mod|datapack, source: modrinth|curseforge|spigot, project, version?, file?, name?, loader?, enabled? }] ou { items, remove_unlisted }"),
            "configs": { "type": "array", "description": "[{ path, set: { \"clé.chemin\": valeur } }]", "items": { "type": "object" } },
            "files": { "type": "array", "description": "[{ path, content }] : petits fichiers texte (512 Kio chacun)", "items": { "type": "object" } },
            "players": { "type": "object", "description": "{ whitelist?: [pseudos], ops?: [pseudos], remove_unlisted? }" },
            "schedules": any_list("[{ name, cron: \"m h jdm mois jds\", is_active?, only_when_online?, tasks: [{ action: command|power, payload, time_offset?, continue_on_failure? }] }] ou { items, remove_unlisted }")
        },
        "required": ["version"]
    })
}

/// Schéma JSON de `include_configs` (`export_manifest`).
pub(crate) fn config_selection_schema() -> Value {
    json!({
        "type": "array",
        "description": "Configs à inclure : chemins, ou { path, keys: [\"a.b\"] } pour quelques clés (sinon toutes les valeurs, 300 au plus)",
        "items": {}
    })
}

fn server_arg(args: &Value) -> Result<i64, String> {
    args.get("server_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| "paramètre `server_id` (entier) requis".into())
}

fn manifest_arg(args: &Value) -> Result<Manifest, String> {
    Manifest::parse(
        args.get("manifest")
            .ok_or("paramètre `manifest` (objet JSON) requis")?,
    )
}

/// Outil `export_manifest`.
pub(crate) async fn export_manifest_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = server_arg(args)?;
    let configs = parse_config_selection(args.get("include_configs"))?;
    let include_players = args
        .get("include_players")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let anonymize = core.get_privacy_config().redact_ai;
    let (m, notes) = Box::pin(export(core, id, &configs, include_players, anonymize)).await?;
    Ok(config_edit::render_for_ai(
        core,
        json!({ "manifest": m, "notes": notes }),
    ))
}

/// Outil `plan_manifest`.
pub(crate) async fn plan_manifest_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = server_arg(args)?;
    let m = manifest_arg(args)?;
    let plan = Box::pin(build_plan(core, id, &m)).await?;
    Ok(config_edit::render_for_ai(core, render_plan(id, &plan)))
}

/// Outil `apply_manifest`.
pub(crate) async fn apply_manifest_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = server_arg(args)?;
    let m = manifest_arg(args)?;
    let opts = ApplyOptions {
        steps: match args.get("steps") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        },
        snapshot: args
            .get("snapshot")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    let out = Box::pin(apply(core, id, &m, &opts)).await?;
    Ok(config_edit::render_for_ai(core, out))
}
