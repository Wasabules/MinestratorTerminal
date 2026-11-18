//! Gestion des MyBox : capacité (CPU, RAM, disque, emplacements de serveurs), catalogue de
//! création (jeux, champs requis, localisations), validations strictes **avant** envoi, requêtes
//! d'écriture conformes au panel web, et plans de placement (calcul pur, sans effet).
//!
//! Analyse complète, sources et incertitudes : `docs/api/MYBOX.md`. L'essentiel :
//! - CPU en **cœurs entiers**, RAM en **Go entiers**, alloués **par serveur** ; seuls les serveurs
//!   **actifs** consomment : un serveur en pause rend ses ressources à la MyBox ;
//! - le disque est **partagé** par tous les serveurs de la MyBox (aucune allocation par serveur) ;
//! - `offer.pods` = nombre maximal de serveurs **actifs** ; en pause : `⌊pods × 1,5⌋` ;
//! - FlexCore : cœurs de rafale partagés, ajoutés à la limite CPU de chaque serveur, non allouables ;
//! - MyBox « Pro » : ressources sur mesure, non bornées par l'offre (le panel plafonne à deux fois
//!   la recommandation du jeu) et sans choix de localisation.
//!
//! Les réponses sont lues en `serde_json::Value` avec tolérance (nombres en chaînes, drapeaux
//! `0/1`) : la plupart de ces endpoints ne sont pas documentés.

use crate::error::{Error, Result};
use crate::Core;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// Longueur maximale d'un nom de serveur ou de MyBox (champ du panel, `maxlength="50"`).
pub const MAX_NAME_LEN: usize = 50;
/// Durée de vie du catalogue de création en cache (jeux et localisations bougent rarement).
const CATALOG_TTL: Duration = Duration::from_secs(3600);
const CATALOG_KEY: &str = "mybox:catalog";
/// Délai maximal d'attente d'une snapshot avant suppression (outil MCP « supprimer »).
const SNAPSHOT_WAIT: Duration = Duration::from_secs(15 * 60);
const SNAPSHOT_POLL: Duration = Duration::from_secs(3);

// --- Modèles exposés ---------------------------------------------------------------

/// Une ressource allouable de la MyBox. CPU : **cœurs** ; RAM : **Go**.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ResourceUsage {
    /// Total de l'offre, options comprises.
    pub total: i64,
    /// Somme des allocations des serveurs **actifs** (les serveurs en pause ne comptent pas).
    pub allocated: i64,
    /// Encore allouable à un serveur (jamais plus que ce que l'API annonce).
    pub remaining: i64,
}

/// Disque de la MyBox : **partagé** par tous ses serveurs, rien n'est réservé par serveur.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MyBoxDisk {
    /// Capacité totale (Go), options comprises.
    pub total_gb: i64,
    /// Occupation actuelle de tous les serveurs (Mo) ; `None` si l'API ne l'a pas fournie.
    pub used_mb: Option<i64>,
}

/// Emplacements de serveurs (`pods`). `0` comme maximum = pas de limite connue (MyBox Pro).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MyBoxSlots {
    pub max_active: i64,
    pub active: i64,
    pub max_paused: i64,
    pub paused: i64,
}

/// Serveur d'une MyBox, avec ce qui lui est alloué.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MyBoxServerSlot {
    pub id: i64,
    pub name: String,
    pub egg_id: i64,
    pub egg_name: String,
    /// Cœurs alloués.
    pub cpu: i64,
    /// Go de RAM alloués.
    pub ram: i64,
    /// En pause (« désactivé » dans l'API) : ne consomme ni CPU ni RAM, ses fichiers sont gardés.
    pub paused: bool,
    pub expired: bool,
    /// Suspendu : l'API le signale aussi pour **tout** serveur en pause.
    pub suspended: bool,
    /// Ville de la localisation (`Toulouse`…).
    pub location: String,
    pub country_iso: String,
    /// Position dans la MyBox (ordre du panel).
    pub order: i64,
    /// Référence courte à donner au support.
    pub support_ref: String,
}

/// Capacité d'une MyBox : ressources totales, allouées et restantes, emplacements, serveurs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MyBoxCapacity {
    pub id: i64,
    pub name: String,
    /// Nom de l'offre (`MyBox 32`…).
    pub offer: String,
    /// Faux pour un sous-utilisateur.
    pub owner: bool,
    /// Permissions de sous-utilisateur sur la MyBox (`server_create`…), vides pour le propriétaire.
    pub permissions: Vec<String>,
    /// MyBox Pro : ressources sur mesure, non bornées par l'offre.
    pub pro: bool,
    pub free: bool,
    pub trial: bool,
    pub expired: bool,
    pub suspended: bool,
    /// Cœurs FlexCore : rafale partagée ajoutée à la limite de chaque serveur, **non allouable**.
    pub flexcore: i64,
    pub cpu: ResourceUsage,
    pub ram: ResourceUsage,
    pub disk: MyBoxDisk,
    pub slots: MyBoxSlots,
    /// Serveurs actifs (dans l'ordre du panel), puis serveurs en pause.
    pub servers: Vec<MyBoxServerSlot>,
    /// Ce qui empêche d'y créer un serveur maintenant (vide = possible) : `expired`, `suspended`,
    /// `forbidden`, `slots_full`, `no_cpu`, `no_ram`.
    pub blockers: Vec<String>,
}

/// Champ à renseigner à la création d'un serveur de ce jeu (variable d'environnement de l'egg).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EggField {
    /// Clé envoyée à l'API (`STEAM_USER`…).
    pub key: String,
    pub label: String,
    pub description: String,
}

/// Jeu (egg) proposé à la création.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CatalogEgg {
    pub id: i64,
    pub name: String,
    /// `games`, `programming`, `databases`, `custom` (PimpMyBox), `experimental` ou `other`.
    pub category: String,
    /// Type expérimental : non testé par l'hébergeur, sans support garanti ; envoyé à part.
    pub experimental: bool,
    pub icon_url: Option<String>,
    /// Ressources minimales recommandées : cœurs, Go de RAM, Go de disque (0 = inconnu).
    pub cpu: i64,
    pub ram: i64,
    pub disk: i64,
    /// Disque nécessaire à l'installation (Go, 0 = inconnu).
    pub install_disk: i64,
    /// Niveau de certification de l'hébergeur (1 à 3, 0 = aucun).
    pub certification: i64,
    pub required_fields: Vec<EggField>,
}

/// Localisation (datacenter) où créer un serveur.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CatalogLocation {
    pub id: i64,
    pub city: String,
    pub country: String,
    pub country_iso: String,
    pub in_stock: bool,
}

/// Catalogue de création : jeux (officiels, personnalisés, expérimentaux) et localisations.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ServerCatalog {
    pub eggs: Vec<CatalogEgg>,
    pub locations: Vec<CatalogLocation>,
}

/// Valeur d'un champ requis. `Debug` masque la valeur : ce peut être un mot de passe (Steam…).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FieldValue {
    pub key: String,
    pub value: String,
}

impl std::fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldValue").field("key", &self.key).field("value", &"<masqué>").finish()
    }
}

/// Demande de création d'un serveur dans une MyBox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreateServerInput {
    pub mybox_id: i64,
    pub name: String,
    /// Identifiant du jeu dans le catalogue ([`CatalogEgg::id`]).
    pub egg_id: i64,
    /// Le jeu vient du catalogue **expérimental** (identifiants distincts).
    #[serde(default)]
    pub experimental: bool,
    /// Localisation ([`CatalogLocation::id`]) ; ignorée pour une MyBox Pro.
    #[serde(default)]
    pub location_id: Option<i64>,
    /// Cœurs (entier ≥ 1).
    pub cpu: i64,
    /// Go de RAM (entier ≥ 1).
    pub ram: i64,
    /// Champs requis du jeu.
    #[serde(default)]
    pub fields: Vec<FieldValue>,
}

/// Serveur créé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreatedServer {
    pub mybox_id: i64,
    pub server_id: i64,
    /// Adresse de connexion attribuée (peut être vide le temps de l'installation).
    pub ip: String,
}

/// Résultat d'une mise en pause ou d'une reprise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PauseOutcome {
    /// Reprise asynchrone en cours (fichiers remis en place) : suivre `job_id` via `backup_job`.
    pub resuming: bool,
    pub job_id: Option<i64>,
}

/// Snapshot lancée avant une suppression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SnapshotJob {
    pub job_id: i64,
    pub snapshot_name: String,
    /// Snapshots déjà présentes sur le compte, et limite.
    pub current_count: i64,
    pub limit: i64,
}

/// Avancement d'un job de sauvegarde (snapshot, reprise d'un serveur en pause).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BackupJob {
    /// `pending`, `running`, `completed`, `failed` ou `cancelled`.
    pub status: String,
    /// Pourcentage (0 à 100).
    pub progress: i64,
    pub error: Option<String>,
}

impl BackupJob {
    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}

/// Serveur à placer (plan de placement).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlacementRequest {
    /// Libellé libre (« survie », « lobby »…), repris dans le plan.
    #[serde(default)]
    pub label: String,
    /// Cœurs.
    pub cpu: i64,
    /// Go de RAM.
    pub ram: i64,
    /// Disque libre souhaité dans la MyBox (Go, 0 = sans exigence). Le disque est partagé.
    #[serde(default)]
    pub disk_gb: i64,
}

/// Où placer un serveur demandé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlacementChoice {
    /// Position de la demande dans la liste reçue.
    pub index: i64,
    pub label: String,
    /// MyBox retenue (`None` : aucune ne peut l'accueillir).
    pub mybox_id: Option<i64>,
    pub mybox_name: Option<String>,
    /// Cœurs et Go encore libres dans cette MyBox **après** ce placement (et les précédents).
    pub cpu_left: i64,
    pub ram_left: i64,
    /// Pourquoi rien ne convient (par MyBox), ou remarques.
    pub reasons: Vec<String>,
}

/// Plan de placement : proposition seulement, rien n'est modifié.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlacementPlan {
    /// `pack` (remplir au plus juste) ou `spread` (répartir la charge).
    pub strategy: String,
    pub placed: i64,
    pub unplaced: i64,
    /// Une entrée par demande, dans l'ordre reçu.
    pub choices: Vec<PlacementChoice>,
    /// Pistes pour ce qui ne rentre pas (mises en pause qui libéreraient assez).
    pub suggestions: Vec<String>,
}

// --- Lecture tolérante ------------------------------------------------------------

fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn int(v: &Value, key: &str) -> i64 {
    num(v, key).map(|f| f.round() as i64).unwrap_or(0)
}

fn flag(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true"),
        _ => false,
    }
}

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Quantité apportée par une option payante de la MyBox (`CPU_2`, `RAM_2`, `DISK_SPACE_25`) : le
/// champ `amount` (lu par le panel), sinon le nombre en suffixe de l'identifiant.
fn option_amount(o: &Value) -> i64 {
    num(o, "amount").map(|f| f.round() as i64).unwrap_or_else(|| {
        text(o, "identifier").rsplit('_').next().and_then(|n| n.parse().ok()).unwrap_or(0)
    })
}

/// Occupation disque de `GET /mybox/{id}/disk` (`disk_servers` + `disk_servers_unit`), en Mo.
pub(crate) fn disk_used_mb(d: &Value) -> Option<i64> {
    let value = num(d, "disk_servers")?;
    let factor = match text(d, "disk_servers_unit").trim().to_ascii_uppercase().as_str() {
        "B" => 1e-6,
        "KB" | "KO" => 1e-3,
        "GB" | "GO" => 1e3,
        "TB" | "TO" => 1e6,
        _ => 1.0, // « MB », l'unité observée
    };
    Some((value * factor).round() as i64)
}

/// Construit la capacité depuis `GET /mybox/{id}` (`data`) et l'occupation disque.
pub(crate) fn capacity_from(data: &Value, fallback_id: i64, disk_used: Option<i64>) -> MyBoxCapacity {
    let info = data.get("mybox").cloned().unwrap_or(Value::Null);
    let offer = data.get("offer").cloned().unwrap_or(Value::Null);

    // Total = offre + options payantes (logique du panel) ; le disque peut déjà les inclure
    // (`disk_total`), d'où le maximum des deux.
    let (mut cpu_total, mut ram_total, mut disk_total) = (int(&offer, "cpu"), int(&offer, "ram"), int(&offer, "disk"));
    for o in list(data, "options") {
        let id = text(o, "identifier").to_ascii_uppercase();
        let amount = option_amount(o);
        if id.starts_with("CPU") {
            cpu_total += amount;
        } else if id.starts_with("RAM") {
            ram_total += amount;
        } else if id.starts_with("DISK") {
            disk_total += amount;
        }
    }
    disk_total = disk_total.max(int(&offer, "disk_total"));

    let mut servers: Vec<MyBoxServerSlot> = list(data, "servers")
        .iter()
        .map(|s| MyBoxServerSlot {
            id: int(s, "id"),
            name: text(s, "name"),
            egg_id: int(s, "egg"),
            egg_name: text(s, "egg_name"),
            cpu: int(s, "cpu"),
            ram: int(s, "ram"),
            paused: flag(s, "disabled"),
            expired: flag(s, "is_expired"),
            suspended: flag(s, "is_suspended"),
            location: text(s, "location"),
            country_iso: text(s, "country_iso").to_ascii_uppercase(),
            order: int(s, "order"),
            support_ref: text(s, "hashsupport"),
        })
        .collect();
    servers.sort_by_key(|s| (s.paused, s.order, s.id));

    let (cpu_alloc, ram_alloc) =
        servers.iter().filter(|s| !s.paused).fold((0, 0), |(c, r), s| (c + s.cpu, r + s.ram));
    let n_active = servers.iter().filter(|s| !s.paused).count() as i64;
    let n_paused = servers.len() as i64 - n_active;
    // Jamais plus que ce que l'API annonce : c'est elle qui refusera.
    let remaining = |total: i64, alloc: i64, api: Option<f64>| {
        let computed = (total - alloc).max(0);
        api.map(|a| computed.min(a.floor().max(0.0) as i64)).unwrap_or(computed)
    };
    let pro = flag(&info, "is_pro");
    let pods = int(&offer, "pods");
    let permissions: Vec<String> = info
        .get("perms")
        .and_then(|p| p.get("mybox"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let mut cap = MyBoxCapacity {
        id: match int(&info, "id") {
            0 => fallback_id,
            id => id,
        },
        name: text(&info, "name"),
        offer: text(&offer, "name"),
        owner: flag(&info, "owner"),
        permissions,
        pro,
        free: flag(&info, "is_free"),
        trial: flag(&info, "is_trial"),
        expired: flag(&info, "is_expired"),
        suspended: flag(&info, "is_suspended"),
        flexcore: int(&offer, "flexcore"),
        cpu: ResourceUsage {
            total: cpu_total,
            allocated: cpu_alloc,
            remaining: remaining(cpu_total, cpu_alloc, num(&info, "cpu_remaining")),
        },
        ram: ResourceUsage {
            total: ram_total,
            allocated: ram_alloc,
            remaining: remaining(ram_total, ram_alloc, num(&info, "ram_remaining")),
        },
        disk: MyBoxDisk { total_gb: disk_total, used_mb: disk_used },
        slots: MyBoxSlots {
            max_active: if pro { 0 } else { pods },
            active: n_active,
            max_paused: if pro { 0 } else { pods * 3 / 2 },
            paused: n_paused,
        },
        servers,
        blockers: Vec::new(),
    };
    cap.blockers = blockers(&cap);
    cap
}

/// Ce qui empêche de créer un serveur dans cette MyBox (codes stables, traduits par l'UI).
fn blockers(c: &MyBoxCapacity) -> Vec<String> {
    let mut out = Vec::new();
    if c.expired {
        out.push("expired");
    }
    if c.suspended {
        out.push("suspended");
    }
    if !c.owner && !c.permissions.iter().any(|p| p == "server_create") {
        out.push("forbidden");
    }
    if c.slots.max_active > 0 && c.slots.active >= c.slots.max_active {
        out.push("slots_full");
    }
    if !c.pro {
        if c.cpu.remaining < 1 {
            out.push("no_cpu");
        }
        if c.ram.remaining < 1 {
            out.push("no_ram");
        }
    }
    out.into_iter().map(str::to_string).collect()
}

/// Libellé d'un champ expérimental : `"Token - Discord Bot Token"` → (`Token`, `Discord Bot Token`).
fn split_label(key: &str, description: &str) -> (String, String) {
    match description.split_once(" - ") {
        Some((label, rest)) if !label.trim().is_empty() => (label.trim().to_string(), rest.trim().to_string()),
        _ => (key.to_string(), description.trim().to_string()),
    }
}

fn fields_of(raw: &[Value]) -> Vec<EggField> {
    raw.iter()
        .filter_map(|f| {
            // `/site/eggs` et PimpMyBox : `name` ; catalogue documenté : `key`.
            let key = [text(f, "name"), text(f, "key"), text(f, "env_variable")]
                .into_iter()
                .find(|k| !k.trim().is_empty())?;
            let label = text(f, "label");
            let description = [text(f, "label_description"), text(f, "description")]
                .into_iter()
                .find(|d| !d.trim().is_empty())
                .unwrap_or_default();
            let (label, description) = if label.trim().is_empty() {
                split_label(&key, &description)
            } else {
                (label, description)
            };
            Some(EggField { key: key.trim().to_string(), label, description })
        })
        .collect()
}

/// Catalogue depuis `/site/eggs`, `/site/eggs/experimentals`, `/server/catalog/locations` et les
/// eggs PimpMyBox validés (`/user/{id}/pimpmybox`). Les deux derniers sont facultatifs.
pub(crate) fn catalog_from(site: &Value, experimental: Option<&Value>, locations: &Value, custom: Option<&Value>) -> ServerCatalog {
    let category_of = |id: i64| {
        for c in list(site, "eggs_categories") {
            if list(c, "eggs").iter().any(|e| int(e, "id") == id) {
                return match text(c, "category").as_str() {
                    "TYPE_PODS_VIDEOGAMES" => "games",
                    "TYPE_PODS_PROGRAMMING" => "programming",
                    "TYPE_PODS_DATABASES" => "databases",
                    _ => "other",
                };
            }
        }
        "other"
    };
    let mut eggs: Vec<CatalogEgg> = list(site, "eggs")
        .iter()
        .map(|e| {
            let res = e.get("resources").cloned().unwrap_or(Value::Null);
            let icon = text(e, "icon");
            CatalogEgg {
                id: int(e, "id"),
                name: text(e, "name"),
                category: category_of(int(e, "id")).to_string(),
                experimental: false,
                icon_url: crate::models::egg_asset_url(Some(&icon), crate::config::EGG_ICON_BASE),
                cpu: int(&res, "cpu"),
                ram: int(&res, "ram"),
                disk: int(&res, "disk"),
                install_disk: int(&res, "disk_install"),
                certification: int(e, "certification"),
                required_fields: fields_of(list(e, "required_fields")),
            }
        })
        .collect();
    if let Some(custom) = custom {
        let validated = custom.get("eggs").map(|e| list(e, "validated")).unwrap_or(&[]);
        for e in validated {
            eggs.push(CatalogEgg {
                id: int(e, "id"),
                name: text(e, "name"),
                category: "custom".into(),
                required_fields: fields_of(list(e, "required_fields")),
                ..Default::default()
            });
        }
    }
    if let Some(groups) = experimental.and_then(|x| x.get("eggs")).and_then(Value::as_object) {
        for group in groups.values() {
            for e in group.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                eggs.push(CatalogEgg {
                    id: int(e, "id"),
                    name: text(e, "name"),
                    category: "experimental".into(),
                    experimental: true,
                    required_fields: fields_of(list(e, "fields")),
                    ..Default::default()
                });
            }
        }
    }
    eggs.retain(|e| e.id > 0 && !e.name.trim().is_empty());
    let locations = list(locations, "locations")
        .iter()
        .map(|l| CatalogLocation {
            id: int(l, "id"),
            city: [text(l, "city"), text(l, "location")].into_iter().find(|c| !c.is_empty()).unwrap_or_default(),
            country: text(l, "country"),
            country_iso: text(l, "country_iso").to_ascii_uppercase(),
            // Champ absent de la documentation : en stock tant que l'API ne dit pas le contraire.
            in_stock: l.get("in_stock").is_none_or(|_| flag(l, "in_stock")),
        })
        .filter(|l| l.id > 0)
        .collect();
    ServerCatalog { eggs, locations }
}

// --- Validations et requêtes d'écriture (pures, testées) ---------------------------

/// Requête d'écriture prête à envoyer : construite et validée sans réseau.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WriteRequest {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

/// Nom nettoyé : 1 à [`MAX_NAME_LEN`] caractères, sans caractère de contrôle.
pub fn valid_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control) {
        return Err(invalid(format!(
            "Le nom doit faire de 1 à {MAX_NAME_LEN} caractères, sur une seule ligne."
        )));
    }
    Ok(name.to_string())
}

fn server_of(cap: &MyBoxCapacity, server_id: i64) -> Result<&MyBoxServerSlot> {
    cap.servers.iter().find(|s| s.id == server_id).ok_or_else(|| {
        invalid(format!("Le serveur {server_id} n'appartient pas à la MyBox {} (« {} »).", cap.id, cap.name))
    })
}

fn ensure_usable(cap: &MyBoxCapacity) -> Result<()> {
    if cap.expired {
        return Err(invalid(format!("La MyBox « {} » est expirée : renouvelle-la d'abord.", cap.name)));
    }
    if cap.suspended {
        return Err(invalid(format!("La MyBox « {} » est suspendue.", cap.name)));
    }
    Ok(())
}

fn ensure_units(cpu: i64, ram: i64) -> Result<()> {
    if cpu < 1 || ram < 1 {
        return Err(invalid(
            "CPU et RAM sont des entiers d'au moins 1 : CPU en cœurs (1 = un cœur), RAM en Go (1 = 1 Go).",
        ));
    }
    Ok(())
}

/// Vérifie que `cpu`/`ram` tiennent dans ce qui reste, en comptant `freed` (les ressources que le
/// serveur concerné occupe déjà, s'il est actif).
fn ensure_fits(cap: &MyBoxCapacity, cpu: i64, ram: i64, freed: (i64, i64)) -> Result<()> {
    if cap.pro {
        return Ok(());
    }
    let (cpu_avail, ram_avail) = (cap.cpu.remaining + freed.0, cap.ram.remaining + freed.1);
    let mut missing = Vec::new();
    if cpu > cpu_avail {
        missing.push(format!("{cpu} cœur(s) demandé(s), {cpu_avail} disponible(s)"));
    }
    if ram > ram_avail {
        missing.push(format!("{ram} Go de RAM demandés, {ram_avail} Go disponibles"));
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(invalid(format!(
            "Ressources insuffisantes dans la MyBox « {} » : {}. Libère des ressources (mise en pause, réduction d'un autre serveur) ou choisis une autre MyBox.",
            cap.name,
            missing.join(" ; ")
        )))
    }
}

/// `POST /mybox/{id}/server` — corps du panel : `name`, `cpu`, `ram`, `location` (sauf Pro),
/// `id_egg` **ou** `id_egg_experimental`, `required_fields` (`[{ key, value }]`).
pub(crate) fn create_request(cap: &MyBoxCapacity, catalog: &ServerCatalog, input: &CreateServerInput) -> Result<WriteRequest> {
    if input.mybox_id != cap.id {
        return Err(invalid("La capacité lue ne correspond pas à la MyBox demandée."));
    }
    ensure_usable(cap)?;
    if cap.blockers.iter().any(|b| b == "forbidden") {
        return Err(invalid("Ton compte n'a pas le droit de créer un serveur dans cette MyBox."));
    }
    if cap.slots.max_active > 0 && cap.slots.active >= cap.slots.max_active {
        return Err(invalid(format!(
            "La MyBox « {} » a déjà {} serveur(s) actif(s) sur {} autorisé(s) : mets-en un en pause ou supprime-le d'abord.",
            cap.name, cap.slots.active, cap.slots.max_active
        )));
    }
    let name = valid_name(&input.name)?;
    let egg = catalog
        .eggs
        .iter()
        .find(|e| e.id == input.egg_id && e.experimental == input.experimental)
        .ok_or_else(|| {
            invalid(format!(
                "Jeu {} introuvable dans le catalogue{} : utilise un identifiant du catalogue.",
                input.egg_id,
                if input.experimental { " expérimental" } else { "" }
            ))
        })?;

    // Champs requis : tous renseignés, aucun inconnu.
    let mut fields = Vec::new();
    for f in &input.fields {
        if !egg.required_fields.iter().any(|r| r.key == f.key.trim()) {
            return Err(invalid(format!("Champ « {} » inconnu pour {}.", f.key, egg.name)));
        }
    }
    for r in &egg.required_fields {
        let value = input
            .fields
            .iter()
            .find(|f| f.key.trim() == r.key)
            .map(|f| f.value.trim())
            .unwrap_or_default();
        if value.is_empty() {
            return Err(invalid(format!("Champ requis manquant pour {} : « {} » ({}).", egg.name, r.label, r.key)));
        }
        fields.push(json!({ "key": r.key, "value": value }));
    }

    ensure_units(input.cpu, input.ram)?;
    if cap.pro {
        // Le panel plafonne une MyBox Pro à deux fois la recommandation du jeu (1 cœur et 4 Go
        // quand le jeu n'en donne pas).
        let rec = |v: i64, default: i64| if v > 0 { v } else { default };
        let (max_cpu, max_ram) = (rec(egg.cpu, 1) * 2, rec(egg.ram, 4) * 2);
        if input.cpu > max_cpu || input.ram > max_ram {
            return Err(invalid(format!(
                "MyBox Pro : au plus {max_cpu} cœur(s) et {max_ram} Go pour {} (deux fois la recommandation).",
                egg.name
            )));
        }
    } else {
        ensure_fits(cap, input.cpu, input.ram, (0, 0))?;
    }

    let mut body = json!({ "name": name, "cpu": input.cpu, "ram": input.ram });
    if !cap.pro {
        let id = input
            .location_id
            .ok_or_else(|| invalid("Localisation requise : choisis-en une dans le catalogue."))?;
        let loc = catalog
            .locations
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| invalid(format!("Localisation {id} inconnue du catalogue.")))?;
        if !loc.in_stock {
            return Err(invalid(format!("La localisation {} est en rupture de stock.", loc.city)));
        }
        if cap.free && !loc.city.eq_ignore_ascii_case("toulouse") {
            return Err(invalid("Une MyBox gratuite ne peut créer de serveur qu'à Toulouse."));
        }
        body["location"] = json!(id);
    }
    if egg.experimental {
        body["id_egg_experimental"] = json!(egg.id);
    } else {
        body["id_egg"] = json!(egg.id);
    }
    if !fields.is_empty() {
        body["required_fields"] = Value::Array(fields);
    }
    Ok(WriteRequest { method: Method::POST, path: format!("/mybox/{}/server", cap.id), body: Some(body) })
}

/// `PATCH /mybox/{id}/server` `{ id_server, cpu, ram }`. L'API peut exiger le serveur **arrêté**
/// (`API_MYBOX_NEED_OFFLINE_SERVER`).
pub(crate) fn resize_request(cap: &MyBoxCapacity, server_id: i64, cpu: i64, ram: i64) -> Result<WriteRequest> {
    ensure_usable(cap)?;
    let s = server_of(cap, server_id)?;
    if s.expired {
        return Err(invalid(format!("Le serveur « {} » est expiré.", s.name)));
    }
    ensure_units(cpu, ram)?;
    if cpu == s.cpu && ram == s.ram {
        return Err(invalid(format!("« {} » a déjà {cpu} cœur(s) et {ram} Go : rien à modifier.", s.name)));
    }
    // Un serveur actif libère d'abord ce qu'il occupe ; un serveur en pause n'occupe rien (la
    // nouvelle allocation ne compte qu'à sa reprise, mais le panel la borne de la même façon).
    let freed = if s.paused { (0, 0) } else { (s.cpu, s.ram) };
    ensure_fits(cap, cpu, ram, freed)?;
    Ok(WriteRequest {
        method: Method::PATCH,
        path: format!("/mybox/{}/server", cap.id),
        body: Some(json!({ "id_server": server_id, "cpu": cpu, "ram": ram })),
    })
}

/// `PATCH /mybox/{id}/server/disable|enable` `{ id_server }` (mise en pause / reprise).
pub(crate) fn pause_request(cap: &MyBoxCapacity, server_id: i64, paused: bool) -> Result<WriteRequest> {
    let s = server_of(cap, server_id)?;
    if paused {
        if s.paused {
            return Err(invalid(format!("« {} » est déjà en pause.", s.name)));
        }
        if cap.slots.max_paused > 0 && cap.slots.paused >= cap.slots.max_paused {
            return Err(invalid(format!(
                "Nombre maximal de serveurs en pause atteint ({}) dans la MyBox « {} ».",
                cap.slots.max_paused, cap.name
            )));
        }
    } else {
        ensure_usable(cap)?;
        if !s.paused {
            return Err(invalid(format!("« {} » est déjà actif.", s.name)));
        }
        if cap.slots.max_active > 0 && cap.slots.active >= cap.slots.max_active {
            return Err(invalid(format!(
                "La MyBox « {} » a déjà {} serveur(s) actif(s) sur {} : mets-en un autre en pause d'abord.",
                cap.name, cap.slots.active, cap.slots.max_active
            )));
        }
        ensure_fits(cap, s.cpu, s.ram, (0, 0))?;
    }
    let action = if paused { "disable" } else { "enable" };
    Ok(WriteRequest {
        method: Method::PATCH,
        path: format!("/mybox/{}/server/{action}", cap.id),
        body: Some(json!({ "id_server": server_id })),
    })
}

/// `PATCH /mybox/{id}/servers/order` `{ servers_ids }` : l'ordre voulu, puis les serveurs non cités
/// dans leur ordre actuel (le panel envoie toujours la liste complète).
pub(crate) fn order_request(cap: &MyBoxCapacity, ids: &[i64]) -> Result<WriteRequest> {
    let mut out: Vec<i64> = Vec::new();
    for id in ids {
        server_of(cap, *id)?;
        if out.contains(id) {
            return Err(invalid(format!("Le serveur {id} apparaît deux fois dans l'ordre demandé.")));
        }
        out.push(*id);
    }
    if out.is_empty() {
        return Err(invalid("Ordre vide."));
    }
    out.extend(cap.servers.iter().map(|s| s.id).filter(|id| !ids.contains(id)));
    Ok(WriteRequest {
        method: Method::PATCH,
        path: format!("/mybox/{}/servers/order", cap.id),
        body: Some(json!({ "servers_ids": out })),
    })
}

/// `PATCH /server/{id}/name` `{ name }`.
pub(crate) fn rename_server_request(server_id: i64, name: &str) -> Result<WriteRequest> {
    Ok(WriteRequest {
        method: Method::PATCH,
        path: format!("/server/{server_id}/name"),
        body: Some(json!({ "name": valid_name(name)? })),
    })
}

/// `PATCH /mybox/{id}/name` `{ name }`.
pub(crate) fn rename_mybox_request(mybox_id: i64, name: &str) -> Result<WriteRequest> {
    Ok(WriteRequest {
        method: Method::PATCH,
        path: format!("/mybox/{mybox_id}/name"),
        body: Some(json!({ "name": valid_name(name)? })),
    })
}

/// Garde de suppression : `confirm` doit reprendre **exactement** le nom du serveur.
pub(crate) fn check_delete_confirmation(s: &MyBoxServerSlot, confirm: &str) -> Result<()> {
    if s.name.is_empty() || confirm.trim() != s.name.trim() {
        return Err(invalid(format!(
            "Suppression refusée : la confirmation doit reprendre exactement le nom du serveur (« {} »).",
            s.name
        )));
    }
    Ok(())
}

/// `DELETE /mybox/{id}/server/{id}` — irréversible (fichiers, mondes, sauvegardes automatiques).
pub(crate) fn delete_request(cap: &MyBoxCapacity, server_id: i64, confirm: &str) -> Result<WriteRequest> {
    check_delete_confirmation(server_of(cap, server_id)?, confirm)?;
    Ok(WriteRequest { method: Method::DELETE, path: format!("/mybox/{}/server/{server_id}", cap.id), body: None })
}

/// Une snapshot avant suppression est-elle possible ? (Conditions du panel : MyBox payante, ni
/// expirée ni suspendue ; serveur sain, ou en pause.)
pub fn snapshot_possible(cap: &MyBoxCapacity, s: &MyBoxServerSlot) -> bool {
    !cap.free && !cap.expired && !cap.suspended && ((!s.expired && !s.suspended) || (s.suspended && s.paused))
}

/// `POST /mybox/{id}/server/{id}/snapshot-before-delete` (sans corps).
pub(crate) fn snapshot_request(cap: &MyBoxCapacity, server_id: i64) -> Result<WriteRequest> {
    let s = server_of(cap, server_id)?;
    if !snapshot_possible(cap, s) {
        return Err(invalid(format!(
            "Snapshot avant suppression impossible pour « {} » (MyBox gratuite, expirée ou suspendue, ou serveur expiré).",
            s.name
        )));
    }
    Ok(WriteRequest {
        method: Method::POST,
        path: format!("/mybox/{}/server/{server_id}/snapshot-before-delete", cap.id),
        body: None,
    })
}

/// Explication française d'un code d'erreur MineStrator lié aux MyBox (pour l'agent MCP).
pub fn explain_api_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "API_MYBOX_NEED_OFFLINE_SERVER" => "le serveur doit être arrêté pour cette opération (power_action stop, puis réessayer)",
        "API_MYBOX_NOT_ENOUGH_CPU" => "pas assez de CPU disponible dans la MyBox",
        "API_MYBOX_NOT_ENOUGH_RAM" => "pas assez de RAM disponible dans la MyBox",
        "API_MYBOX_FULL" => "nombre maximal de serveurs actifs atteint dans la MyBox",
        "API_MYBOX_DISABLED_SERVERS_COUNT_MAX" => "nombre maximal de serveurs en pause atteint",
        "API_MYBOX_EXPIRED" => "la MyBox est expirée",
        "API_MYBOX_SUSPENDED" => "la MyBox est suspendue",
        "API_MYBOX_BUSY" => "une opération est déjà en cours sur cette MyBox, réessayer dans quelques secondes",
        "API_MYBOX_OUT_OF_STOCK" | "API_MYBOX_SSH_ERROR" => "indisponibilité côté hébergeur, réessayer plus tard",
        "API_MYBOX_SERVER_ALREADY_DISABLED" => "le serveur est déjà en pause",
        "API_MYBOX_SERVER_ALREADY_ENABLED" => "le serveur est déjà actif",
        "API_INCORRECT_CPU_AMOUNT" => "quantité de CPU invalide (entier de cœurs)",
        "API_INCORRECT_RAM_AMOUNT" => "quantité de RAM invalide (entier de Go)",
        "API_INVALID_LOCATION" | "API_INVALID_DATACENTER" | "API_INVALID_DATACENTER_LOCATION" => "localisation invalide",
        "API_LOCATION_UNAVAILABLE_REGION" | "API_LOCATION_UNAVAILABLE_VPN" => "localisation refusée depuis la connexion actuelle",
        "API_EMPTY_EGG_OR_EXPERIMENTAL_EGG_ID" => "type de serveur non renseigné",
        "API_MISSING_REQUIRED_FIELDS" => "un champ requis du jeu manque",
        "API_NAME_TOO_LONG" => "nom trop long",
        "API_NAME_TOO_SHORT" => "nom trop court",
        "API_MISSING_INCORRECT_NAME" => "nom invalide",
        "API_SNAPSHOTS_COUNT_MAX" => "limite de snapshots atteinte : en supprimer une d'abord",
        "API_SNAPSHOT_ALREADY_PROCESSING" => "une sauvegarde est déjà en cours sur ce serveur",
        "API_SERVER_REACTIVATION_RETRY" => "la machine du serveur ne répond pas : réessayer dans quelques minutes",
        "API_SERVER_REACTIVATION_SUPPORT" => "la reprise demande une intervention du support",
        _ => return None,
    })
}

// --- Plan de placement (pur) -------------------------------------------------------

struct Avail<'a> {
    cap: &'a MyBoxCapacity,
    usable: bool,
    cpu: i64,
    ram: i64,
    slots: i64,
    disk_free_gb: Option<i64>,
}

/// Propose une MyBox pour chaque serveur demandé, selon les ressources **libres** (sans rien
/// modifier). `pack` : la MyBox où il reste le moins après placement (garde de grands blocs
/// libres ailleurs) ; `spread` : celle où il reste le plus (répartit la charge). Les demandes sont
/// traitées de la plus grosse à la plus petite ; une MyBox Pro ne sert qu'en dernier recours.
pub fn plan_placement(caps: &[MyBoxCapacity], requests: &[PlacementRequest], strategy: &str) -> Result<PlacementPlan> {
    let spread = match strategy.trim() {
        "" | "pack" => false,
        "spread" => true,
        other => return Err(invalid(format!("Stratégie « {other} » inconnue : pack ou spread."))),
    };
    if requests.is_empty() {
        return Err(invalid("Aucun serveur à placer."));
    }
    for r in requests {
        ensure_units(r.cpu, r.ram)?;
        if r.disk_gb < 0 {
            return Err(invalid("Le disque souhaité est en Go, positif."));
        }
    }
    let mut avail: Vec<Avail> = caps
        .iter()
        .map(|c| Avail {
            cap: c,
            usable: !c.expired && !c.suspended && !c.blockers.iter().any(|b| b == "forbidden"),
            cpu: c.cpu.remaining,
            ram: c.ram.remaining,
            slots: if c.slots.max_active > 0 { (c.slots.max_active - c.slots.active).max(0) } else { i64::MAX },
            disk_free_gb: c.disk.used_mb.map(|u| (c.disk.total_gb * 1000 - u).max(0) / 1000),
        })
        .collect();

    let mut order: Vec<usize> = (0..requests.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(requests[i].ram), std::cmp::Reverse(requests[i].cpu), i));
    let mut choices: Vec<Option<PlacementChoice>> = vec![None; requests.len()];
    let mut suggestions = Vec::new();

    for i in order {
        let r = &requests[i];
        let label = if r.label.trim().is_empty() { format!("serveur {}", i + 1) } else { r.label.trim().to_string() };
        let fits = |a: &Avail| {
            a.usable
                && a.slots > 0
                && (a.cap.pro || (r.cpu <= a.cpu && r.ram <= a.ram))
                && (r.disk_gb == 0 || a.disk_free_gb.is_none_or(|d| d >= r.disk_gb))
        };
        let score = |a: &Avail| -> f64 {
            if a.cap.pro {
                return f64::INFINITY; // dernier recours
            }
            let left = (a.cpu - r.cpu) as f64 / a.cap.cpu.total.max(1) as f64
                + (a.ram - r.ram) as f64 / a.cap.ram.total.max(1) as f64;
            if spread { -left } else { left }
        };
        let best = avail
            .iter()
            .enumerate()
            .filter(|(_, a)| fits(a))
            .min_by(|(_, a), (_, b)| score(a).total_cmp(&score(b)).then(a.cap.id.cmp(&b.cap.id)))
            .map(|(k, _)| k);
        let choice = match best {
            Some(k) => {
                let a = &mut avail[k];
                if !a.cap.pro {
                    a.cpu -= r.cpu;
                    a.ram -= r.ram;
                }
                a.slots = a.slots.saturating_sub(1);
                PlacementChoice {
                    index: i as i64,
                    label,
                    mybox_id: Some(a.cap.id),
                    mybox_name: Some(a.cap.name.clone()),
                    cpu_left: a.cpu,
                    ram_left: a.ram,
                    reasons: if a.cap.pro { vec!["MyBox Pro : ressources sur mesure, hors offre.".into()] } else { Vec::new() },
                }
            }
            None => {
                let reasons = avail.iter().map(|a| why_not(a, r)).collect();
                suggestions.extend(avail.iter().filter_map(|a| pause_suggestion(a, r, &label)));
                PlacementChoice { index: i as i64, label, mybox_id: None, mybox_name: None, cpu_left: 0, ram_left: 0, reasons }
            }
        };
        choices[i] = Some(choice);
    }
    let choices: Vec<PlacementChoice> = choices.into_iter().flatten().collect();
    let placed = choices.iter().filter(|c| c.mybox_id.is_some()).count() as i64;
    Ok(PlacementPlan {
        strategy: if spread { "spread" } else { "pack" }.into(),
        placed,
        unplaced: choices.len() as i64 - placed,
        choices,
        suggestions,
    })
}

fn why_not(a: &Avail, r: &PlacementRequest) -> String {
    let c = a.cap;
    let mut why = Vec::new();
    if c.expired {
        why.push("expirée".to_string());
    }
    if c.suspended {
        why.push("suspendue".to_string());
    }
    if !a.usable && !c.expired && !c.suspended {
        why.push("droits insuffisants".to_string());
    }
    if a.slots <= 0 {
        why.push(format!("plus d'emplacement actif ({} max)", c.slots.max_active));
    }
    if !c.pro && r.cpu > a.cpu {
        why.push(format!("il manque {} cœur(s)", r.cpu - a.cpu));
    }
    if !c.pro && r.ram > a.ram {
        why.push(format!("il manque {} Go de RAM", r.ram - a.ram));
    }
    if let Some(d) = a.disk_free_gb.filter(|d| r.disk_gb > *d) {
        why.push(format!("{d} Go de disque libres pour {} demandés", r.disk_gb));
    }
    format!("MyBox {} (« {} ») : {}", c.id, c.name, why.join(", "))
}

/// Mettre en pause quelques serveurs actifs libérerait-il assez ? (Plus gros d'abord.)
fn pause_suggestion(a: &Avail, r: &PlacementRequest, label: &str) -> Option<String> {
    let c = a.cap;
    if !a.usable || c.pro {
        return None;
    }
    let (mut cpu, mut ram, mut slots) = (a.cpu, a.ram, a.slots);
    if cpu >= r.cpu && ram >= r.ram && slots > 0 {
        return None;
    }
    let mut actives: Vec<&MyBoxServerSlot> = c.servers.iter().filter(|s| !s.paused).collect();
    actives.sort_by_key(|s| (std::cmp::Reverse(s.ram), std::cmp::Reverse(s.cpu), s.id));
    let mut names = Vec::new();
    for s in actives {
        if cpu >= r.cpu && ram >= r.ram && slots > 0 {
            break;
        }
        cpu += s.cpu;
        ram += s.ram;
        slots = slots.saturating_add(1);
        names.push(format!("« {} » (id {})", s.name, s.id));
    }
    let paused_room = c.slots.max_paused == 0 || c.slots.paused + names.len() as i64 <= c.slots.max_paused;
    (cpu >= r.cpu && ram >= r.ram && slots > 0 && paused_room && !names.is_empty()).then(|| {
        format!(
            "MyBox {} (« {} ») : mettre en pause {} libérerait de quoi accueillir « {label} » ({} cœur(s), {} Go).",
            c.id,
            c.name,
            names.join(", "),
            r.cpu,
            r.ram
        )
    })
}

// --- SmartScan (lecture) -------------------------------------------------------------

/// Résumé compact de `GET /mybox/{id}/smartscan` (analyse p95 de l'hébergeur), sans noms : des
/// identifiants de serveurs, des allocations (cœurs, Go) et des usages mesurés.
pub(crate) fn smartscan_summary(d: &Value) -> Value {
    let pick = |v: &Value, keys: &[&str]| -> Value {
        let mut m = serde_json::Map::new();
        for k in keys {
            if let Some(x) = v.get(*k).filter(|x| !x.is_null()) {
                m.insert((*k).to_string(), x.clone());
            }
        }
        Value::Object(m)
    };
    let confidence = |s: &Value| s.get("data_window").and_then(|w| w.get("confidence")).cloned().unwrap_or(Value::Null);
    let shrink = d.get("shrink_recommendation").cloned().unwrap_or(Value::Null);
    let balance = d.get("balance_recommendation").cloned().unwrap_or(Value::Null);
    let mut shrink_out = pick(&shrink, &["can_optimize", "potential_cpu_savings", "potential_ram_savings"]);
    shrink_out["servers"] = Value::Array(
        list(&shrink, "servers")
            .iter()
            .map(|s| {
                let mut o = pick(s, &[
                    "id_server", "insufficient_data", "current_cpu", "current_ram", "suggested_cpu",
                    "suggested_ram", "p95_cpu_percent", "p95_ram_mb",
                ]);
                o["confidence"] = confidence(s);
                o
            })
            .collect(),
    );
    let mut balance_out = pick(&balance, &["can_rebalance", "rebalance_reason", "recoverable_cpu", "recoverable_ram"]);
    balance_out["plan"] = Value::Array(
        list(&balance, "reallocation_plan")
            .iter()
            .map(|s| {
                pick(s, &[
                    "id_server", "insufficient_data", "current_cpu", "current_ram", "target_cpu", "target_ram",
                    "reason",
                ])
            })
            .collect(),
    );
    json!({
        "units": "cpu/ram actuels et suggérés : cœurs et Go ; p95_cpu_percent : % d'UN cœur (100 = un cœur plein) ; p95_ram_mb : Mo",
        "shrink": shrink_out,
        "balance": balance_out,
        "scale": pick(d.get("scale_recommendation").unwrap_or(&Value::Null), &["action", "priority", "reason"]),
    })
}

fn backup_job_from(d: &Value) -> BackupJob {
    let error = [text(d, "error_message"), text(d, "error")].into_iter().find(|e| !e.trim().is_empty());
    BackupJob {
        status: match text(d, "status") {
            s if s.is_empty() => "running".into(),
            s => s,
        },
        progress: int(d, "progress").clamp(0, 100),
        error,
    }
}

// --- Façade Core -------------------------------------------------------------------

impl Core {
    /// Envoie une écriture MyBox, puis vide le cache TTL (les lectures en cache sont périmées).
    async fn mybox_send(&self, req: WriteRequest) -> Result<Option<Value>> {
        let token = self.token()?;
        let out = self.api.send_raw(req.method, &token, &req.path, req.body.as_ref()).await;
        self.cache.clear();
        out
    }

    /// Capacité d'une MyBox : totaux, allocations des serveurs actifs, restes, disque partagé,
    /// emplacements et serveurs.
    pub async fn mybox_capacity(&self, id: i64) -> Result<MyBoxCapacity> {
        let token = self.token()?;
        let (path, disk_path) = (format!("/mybox/{id}"), format!("/mybox/{id}/disk"));
        let (data, disk) =
            tokio::join!(self.api.get_data(&token, &path), self.api.get_data(&token, &disk_path));
        let used = disk.ok().and_then(|d| disk_used_mb(&d));
        Ok(capacity_from(&data?, id, used))
    }

    /// Capacité de toutes les MyBox du compte. Une MyBox illisible (droits de sous-utilisateur)
    /// est ignorée ; erreur seulement si aucune ne répond.
    pub async fn mybox_capacities(&self) -> Result<Vec<MyBoxCapacity>> {
        let overview = self.list_servers().await?;
        let results =
            futures_util::future::join_all(overview.myboxes.iter().map(|m| self.mybox_capacity(m.id))).await;
        let mut out = Vec::new();
        let mut last_err = None;
        for (r, summary) in results.into_iter().zip(&overview.myboxes) {
            match r {
                Ok(mut c) => {
                    if c.name.is_empty() {
                        c.name = summary.name.clone();
                    }
                    out.push(c);
                }
                Err(e) => {
                    tracing::warn!(mybox = summary.id, "capacité illisible : {e}");
                    last_err = Some(e);
                }
            }
        }
        match last_err {
            Some(e) if out.is_empty() => Err(e),
            _ => Ok(out),
        }
    }

    /// Catalogue de création (jeux, champs requis, ressources recommandées, localisations).
    /// Mis en cache une heure.
    pub async fn server_catalog(&self) -> Result<ServerCatalog> {
        if let Some(hit) = self.cache.get(CATALOG_KEY) {
            if let Ok(c) = serde_json::from_str(&hit) {
                return Ok(c);
            }
        }
        let token = self.token()?;
        let custom_path = format!("/user/{}/pimpmybox", self.user_id().await?);
        let (site, experimental, locations, custom) = tokio::join!(
            self.api.get_data(&token, "/site/eggs"),
            self.api.get_data(&token, "/site/eggs/experimentals"),
            self.api.get_data(&token, "/server/catalog/locations"),
            self.api.get_data(&token, &custom_path),
        );
        let catalog = catalog_from(&site?, experimental.ok().as_ref(), &locations?, custom.ok().as_ref());
        if let Ok(json) = serde_json::to_string(&catalog) {
            self.cache.put(CATALOG_KEY.into(), json, CATALOG_TTL);
        }
        Ok(catalog)
    }

    /// Crée un serveur après validation stricte (ressources restantes, emplacements, jeu, champs
    /// requis, localisation). L'installation se poursuit ensuite côté hébergeur.
    pub async fn mybox_create_server(&self, input: CreateServerInput) -> Result<CreatedServer> {
        let (cap, catalog) = tokio::join!(self.mybox_capacity(input.mybox_id), self.server_catalog());
        let req = create_request(&cap?, &catalog?, &input)?;
        let data = self.mybox_send(req).await?.unwrap_or(Value::Null);
        Ok(CreatedServer {
            mybox_id: match int(&data, "id_mybox") {
                0 => input.mybox_id,
                id => id,
            },
            server_id: int(&data, "id_server"),
            ip: text(&data, "ip"),
        })
    }

    /// Change le CPU (cœurs) et la RAM (Go) alloués à un serveur.
    pub async fn mybox_resize_server(&self, mybox_id: i64, server_id: i64, cpu: i64, ram: i64) -> Result<()> {
        let cap = self.mybox_capacity(mybox_id).await?;
        self.mybox_send(resize_request(&cap, server_id, cpu, ram)?).await.map(|_| ())
    }

    /// Met un serveur en pause (ses ressources reviennent à la MyBox, ses fichiers sont gardés) ou
    /// le reprend (il faut alors un emplacement actif et ses ressources libres).
    pub async fn mybox_set_server_paused(&self, mybox_id: i64, server_id: i64, paused: bool) -> Result<PauseOutcome> {
        let cap = self.mybox_capacity(mybox_id).await?;
        let data = self.mybox_send(pause_request(&cap, server_id, paused)?).await?.unwrap_or(Value::Null);
        let reactivation = data.get("reactivation").cloned().unwrap_or(Value::Null);
        let job_id = Some(int(&reactivation, "job_id")).filter(|id| *id > 0);
        Ok(PauseOutcome { resuming: text(&reactivation, "status") == "in_progress" && job_id.is_some(), job_id })
    }

    /// Réordonne les serveurs d'une MyBox (ceux non cités gardent leur ordre, à la suite).
    pub async fn mybox_reorder_servers(&self, mybox_id: i64, server_ids: Vec<i64>) -> Result<()> {
        let cap = self.mybox_capacity(mybox_id).await?;
        self.mybox_send(order_request(&cap, &server_ids)?).await.map(|_| ())
    }

    /// Renomme un serveur (1 à 50 caractères).
    pub async fn rename_server(&self, server_id: i64, name: &str) -> Result<()> {
        self.mybox_send(rename_server_request(server_id, name)?).await.map(|_| ())
    }

    /// Renomme une MyBox (1 à 50 caractères).
    pub async fn rename_mybox(&self, mybox_id: i64, name: &str) -> Result<()> {
        self.mybox_send(rename_mybox_request(mybox_id, name)?).await.map(|_| ())
    }

    /// Lance une snapshot du serveur avant sa suppression (suivre `job_id` via [`Core::backup_job`]).
    pub async fn mybox_snapshot_before_delete(&self, mybox_id: i64, server_id: i64) -> Result<SnapshotJob> {
        let cap = self.mybox_capacity(mybox_id).await?;
        let data = self.mybox_send(snapshot_request(&cap, server_id)?).await?.unwrap_or(Value::Null);
        Ok(SnapshotJob {
            job_id: int(&data, "job_id"),
            snapshot_name: text(&data, "snapshot_name"),
            current_count: int(&data, "current_count"),
            limit: int(&data, "limit"),
        })
    }

    /// Avancement d'un job de sauvegarde (snapshot, reprise d'un serveur en pause).
    pub async fn backup_job(&self, job_id: i64) -> Result<BackupJob> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        let data = self.api.get_data(&token, &format!("/user/{user_id}/backup_job/{job_id}")).await?;
        Ok(backup_job_from(&data))
    }

    /// Supprime **définitivement** un serveur. `confirm` doit reprendre exactement son nom.
    pub async fn mybox_delete_server(&self, mybox_id: i64, server_id: i64, confirm: &str) -> Result<()> {
        let cap = self.mybox_capacity(mybox_id).await?;
        self.mybox_send(delete_request(&cap, server_id, confirm)?).await.map(|_| ())
    }

    /// Suppression pour un agent : confirmation vérifiée **d'abord**, puis (si demandé) snapshot
    /// préalable attendue jusqu'au bout — un échec ou un délai dépassé annule la suppression.
    /// Renvoie le nom de la snapshot créée.
    pub async fn mybox_delete_server_guarded(
        &self,
        mybox_id: i64,
        server_id: i64,
        confirm: &str,
        snapshot_first: bool,
    ) -> Result<Option<String>> {
        let cap = self.mybox_capacity(mybox_id).await?;
        let req = delete_request(&cap, server_id, confirm)?;
        let mut snapshot = None;
        if snapshot_first {
            let job = self.mybox_snapshot_before_delete(mybox_id, server_id).await?;
            let start = std::time::Instant::now();
            loop {
                tokio::time::sleep(SNAPSHOT_POLL).await;
                let state = self.backup_job(job.job_id).await?;
                match state.status.as_str() {
                    "completed" => break,
                    "failed" | "cancelled" => {
                        return Err(invalid(format!(
                            "La snapshot préalable a échoué ({}) : suppression annulée.",
                            state.error.unwrap_or(state.status)
                        )))
                    }
                    _ if start.elapsed() > SNAPSHOT_WAIT => {
                        return Err(invalid(format!(
                            "Snapshot « {} » toujours en cours (job {}) après {} min : suppression annulée. Relance-la une fois la snapshot terminée.",
                            job.snapshot_name,
                            job.job_id,
                            SNAPSHOT_WAIT.as_secs() / 60
                        )))
                    }
                    _ => {}
                }
            }
            snapshot = Some(job.snapshot_name);
        }
        self.mybox_send(req).await?;
        Ok(snapshot)
    }

    /// Plan de placement de serveurs à créer, d'après les ressources libres de toutes les MyBox.
    /// Calcul pur : rien n'est modifié.
    pub async fn mybox_plan_placement(&self, requests: Vec<PlacementRequest>, strategy: &str) -> Result<PlacementPlan> {
        let caps = self.mybox_capacities().await?;
        plan_placement(&caps, &requests, strategy)
    }

    /// Analyse SmartScan de l'hébergeur (usage p95 → réductions et rééquilibrages suggérés), résumée.
    pub async fn mybox_smartscan(&self, mybox_id: i64) -> Result<Value> {
        let token = self.token()?;
        let data = self.api.get_data(&token, &format!("/mybox/{mybox_id}/smartscan")).await?;
        Ok(smartscan_summary(&data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MyBox 32 observée le 2026-11-18 (valeurs anonymisées) : 8 cœurs, 32 Go, 300 Go, 8 pods,
    /// 3 serveurs actifs (8 cœurs, 20 Go) et 1 en pause.
    fn raw_mybox() -> Value {
        json!({
            "mybox": { "id": 42, "name": "Principale", "owner": 1, "is_pro": 0, "is_free": "0",
                       "is_expired": 0, "is_suspended": 0, "cpu_remaining": 0, "ram_remaining": "12" },
            "offer": { "name": "MyBox 32", "cpu": 8, "ram": 32, "disk": 300, "disk_total": 300,
                       "pods": 8, "flexcore": 2 },
            "options": [],
            "servers": [
                { "id": 3, "name": "Créatif", "egg": 1, "egg_name": "Minecraft", "cpu": 2, "ram": 4,
                  "disabled": 0, "is_expired": 0, "is_suspended": 0, "location": "Toulouse",
                  "country_iso": "fr", "order": 2, "hashsupport": "C3" },
                { "id": 1, "name": "Survie", "egg": 1, "egg_name": "Minecraft", "cpu": 4, "ram": 10,
                  "disabled": 0, "is_expired": 0, "is_suspended": 0, "location": "Toulouse",
                  "country_iso": "FR", "order": 0, "hashsupport": "A1" },
                { "id": 2, "name": "Lobby", "egg": 1, "egg_name": "Minecraft", "cpu": 2, "ram": 6,
                  "disabled": "0", "location": "Toulouse", "order": 1 },
                { "id": 9, "name": "Ancien", "egg": 12, "egg_name": "Rust", "cpu": 3, "ram": 8,
                  "disabled": 1, "is_suspended": 1, "location": "Toulouse", "order": 0 }
            ]
        })
    }

    fn cap() -> MyBoxCapacity {
        capacity_from(&raw_mybox(), 42, Some(3768))
    }

    fn catalog() -> ServerCatalog {
        let site = json!({
            "eggs": [
                { "id": 1, "name": "Minecraft", "icon": "minecraft.webp",
                  "resources": { "cpu": "2", "ram": "4", "disk": "10", "disk_install": "5" },
                  "certification": 3, "required_fields": [] },
                { "id": 4592, "name": "DayZ", "resources": { "cpu": "2", "ram": "8", "disk": "20", "disk_install": "20" },
                  "required_fields": [
                    { "name": "STEAM_USER", "label": "Identifiant Steam", "label_description": "Compte possédant le jeu" },
                    { "name": "STEAM_PASS", "label": "Mot de passe Steam" }
                  ] }
            ],
            "eggs_categories": [
                { "category": "TYPE_PODS_VIDEOGAMES", "eggs": [{ "id": 1 }, { "id": 4592 }] }
            ]
        });
        let experimental = json!({ "eggs": { "APPLICATION": [
            { "id": 169, "name": "Aoede", "fields": [{ "name": "DISCORD_TOKEN", "description": "Token - Discord Bot Token" }] }
        ], "GENERIC": [] } });
        let locations = json!({ "locations": [
            { "id": 1, "city": "Toulouse", "country": "France", "country_iso": "FR", "in_stock": true },
            { "id": 2, "city": "New York", "country": "Etats-Unis", "country_iso": "US", "in_stock": true },
            { "id": 8, "city": "Sydney", "country": "Australie", "country_iso": "AU", "in_stock": false }
        ] });
        let custom = json!({ "eggs": { "validated": [{ "id": 777, "name": "Mon egg", "required_fields": [] }], "history": [] } });
        catalog_from(&site, Some(&experimental), &locations, Some(&custom))
    }

    fn input(cpu: i64, ram: i64) -> CreateServerInput {
        CreateServerInput {
            mybox_id: 42,
            name: "  Nouveau  ".into(),
            egg_id: 1,
            experimental: false,
            location_id: Some(1),
            cpu,
            ram,
            fields: vec![],
        }
    }

    #[test]
    fn capacity_counts_only_active_servers_and_reads_leniently() {
        let c = cap();
        assert_eq!(c.cpu, ResourceUsage { total: 8, allocated: 8, remaining: 0 });
        assert_eq!(c.ram, ResourceUsage { total: 32, allocated: 20, remaining: 12 });
        assert_eq!(c.disk, MyBoxDisk { total_gb: 300, used_mb: Some(3768) });
        assert_eq!(c.slots, MyBoxSlots { max_active: 8, active: 3, max_paused: 12, paused: 1 });
        assert_eq!(c.flexcore, 2);
        // Actifs dans l'ordre du panel, puis ceux en pause.
        assert_eq!(c.servers.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 2, 3, 9]);
        assert_eq!(c.servers[2].country_iso, "FR");
        assert!(c.servers[3].paused && c.servers[3].suspended);
        assert_eq!(c.blockers, ["no_cpu"]);
    }

    #[test]
    fn options_add_to_totals_and_api_remaining_caps_the_computed_one() {
        let mut raw = raw_mybox();
        raw["options"] = json!([
            { "identifier": "CPU_2", "amount": "2" },
            { "identifier": "RAM_2" },
            { "identifier": "DISK_SPACE_25", "amount": 25 }
        ]);
        raw["mybox"]["cpu_remaining"] = json!(1); // l'API annonce moins que le calcul (2)
        let c = capacity_from(&raw, 42, None);
        assert_eq!((c.cpu.total, c.ram.total, c.disk.total_gb), (10, 34, 325));
        assert_eq!(c.cpu.remaining, 1, "jamais plus que ce que l'API annonce");
        assert_eq!(c.ram.remaining, 12, "34 - 20 = 14, borné par les 12 annoncés");
        // Champ absent : le calcul seul fait foi ; données vides : tout à zéro, sans panique.
        let empty = capacity_from(&json!({}), 7, None);
        assert_eq!((empty.id, empty.cpu.total, empty.servers.len()), (7, 0, 0));
        assert!(empty.blockers.contains(&"forbidden".to_string()));
    }

    #[test]
    fn disk_units_are_normalized_to_megabytes() {
        assert_eq!(disk_used_mb(&json!({ "disk_servers": 3768, "disk_servers_unit": "MB" })), Some(3768));
        assert_eq!(disk_used_mb(&json!({ "disk_servers": "1.5", "disk_servers_unit": "GB" })), Some(1500));
        assert_eq!(disk_used_mb(&json!({ "disk_servers_unit": "MB" })), None);
    }

    #[test]
    fn pro_mybox_is_unbounded_and_subusers_need_the_create_permission() {
        let mut raw = raw_mybox();
        raw["mybox"]["is_pro"] = json!(1);
        let c = capacity_from(&raw, 42, None);
        assert_eq!((c.slots.max_active, c.slots.max_paused), (0, 0));
        assert!(c.blockers.is_empty(), "{:?}", c.blockers);

        let mut sub = raw_mybox();
        sub["mybox"]["owner"] = json!(0);
        sub["mybox"]["perms"] = json!({ "mybox": ["server_create"] });
        let c = capacity_from(&sub, 42, None);
        assert_eq!(c.permissions, ["server_create"]);
        assert!(!c.blockers.contains(&"forbidden".to_string()));
    }

    #[test]
    fn catalog_merges_official_custom_and_experimental_eggs() {
        let c = catalog();
        let mc = &c.eggs[0];
        assert_eq!((mc.category.as_str(), mc.cpu, mc.ram, mc.install_disk), ("games", 2, 4, 5));
        assert!(mc.icon_url.as_deref().is_some_and(|u| u.ends_with("minecraft.webp")));
        let dayz = c.eggs.iter().find(|e| e.id == 4592).unwrap();
        assert_eq!(dayz.required_fields[0], EggField {
            key: "STEAM_USER".into(),
            label: "Identifiant Steam".into(),
            description: "Compte possédant le jeu".into()
        });
        let custom = c.eggs.iter().find(|e| e.id == 777).unwrap();
        assert_eq!(custom.category, "custom");
        let exp = c.eggs.iter().find(|e| e.id == 169).unwrap();
        assert!(exp.experimental);
        assert_eq!((exp.required_fields[0].label.as_str(), exp.required_fields[0].description.as_str()), ("Token", "Discord Bot Token"));
        assert_eq!(c.locations.len(), 3);
        assert!(!c.locations[2].in_stock);
        // Catalogue documenté : `required_fields[{ key, label }]` ; stock absent = en stock.
        let doc = catalog_from(
            &json!({ "eggs": [{ "id": 18, "name": "BeamMP", "required_fields": [{ "key": "AUTHKEY", "label": "Authentication Key" }] }] }),
            None,
            &json!({ "locations": [{ "id": 1, "city": "Toulouse", "country_iso": "fr" }] }),
            None,
        );
        assert_eq!(doc.eggs[0].required_fields[0].key, "AUTHKEY");
        assert_eq!(doc.eggs[0].category, "other");
        assert!(doc.locations[0].in_stock);
        assert_eq!(doc.locations[0].country_iso, "FR");
    }

    #[test]
    fn create_body_matches_the_panel() {
        let mut c = cap();
        c.cpu.remaining = 2; // libère de quoi créer
        c.blockers = blockers(&c);
        let req = create_request(&c, &catalog(), &input(2, 4)).unwrap();
        assert_eq!(req.method, Method::POST);
        assert_eq!(req.path, "/mybox/42/server");
        assert_eq!(req.body.unwrap(), json!({ "name": "Nouveau", "cpu": 2, "ram": 4, "location": 1, "id_egg": 1 }));

        // Jeu expérimental + champ requis : `id_egg_experimental` et `required_fields`.
        let exp = CreateServerInput {
            egg_id: 169,
            experimental: true,
            fields: vec![FieldValue { key: "DISCORD_TOKEN".into(), value: " abc ".into() }],
            ..input(1, 1)
        };
        let body = create_request(&c, &catalog(), &exp).unwrap().body.unwrap();
        assert_eq!(body["id_egg_experimental"], 169);
        assert!(body.get("id_egg").is_none());
        assert_eq!(body["required_fields"], json!([{ "key": "DISCORD_TOKEN", "value": "abc" }]));
    }

    #[test]
    fn create_is_refused_before_sending_when_something_is_wrong() {
        let mut c = cap();
        c.cpu.remaining = 2;
        c.blockers = blockers(&c);
        let cat = catalog();
        let err = |i: CreateServerInput| create_request(&c, &cat, &i).unwrap_err().to_string();

        assert!(err(input(3, 4)).contains("3 cœur(s) demandé(s), 2 disponible(s)"));
        assert!(err(input(1, 13)).contains("13 Go de RAM demandés, 12 Go disponibles"));
        assert!(err(input(0, 4)).contains("entiers d'au moins 1"));
        assert!(err(CreateServerInput { location_id: None, ..input(1, 1) }).contains("Localisation requise"));
        assert!(err(CreateServerInput { location_id: Some(8), ..input(1, 1) }).contains("rupture de stock"));
        assert!(err(CreateServerInput { location_id: Some(99), ..input(1, 1) }).contains("inconnue"));
        assert!(err(CreateServerInput { egg_id: 4592, ..input(1, 1) }).contains("Champ requis manquant"));
        assert!(err(CreateServerInput { egg_id: 169, ..input(1, 1) }).contains("introuvable"), "id expérimental sans le drapeau");
        assert!(err(CreateServerInput {
            fields: vec![FieldValue { key: "INCONNU".into(), value: "x".into() }],
            ..input(1, 1)
        })
        .contains("inconnu"));
        assert!(err(CreateServerInput { name: "x".repeat(51), ..input(1, 1) }).contains("1 à 50"));
        assert!(err(CreateServerInput { name: "   ".into(), ..input(1, 1) }).contains("1 à 50"));

        // Plus d'emplacement actif, MyBox expirée, MyBox gratuite hors Toulouse.
        let mut full = c.clone();
        full.slots.active = full.slots.max_active;
        assert!(create_request(&full, &cat, &input(1, 1)).unwrap_err().to_string().contains("serveur(s) actif(s)"));
        let expired = MyBoxCapacity { expired: true, ..c.clone() };
        assert!(create_request(&expired, &cat, &input(1, 1)).unwrap_err().to_string().contains("expirée"));
        let free = MyBoxCapacity { free: true, ..c.clone() };
        assert!(create_request(&free, &cat, &CreateServerInput { location_id: Some(2), ..input(1, 1) })
            .unwrap_err()
            .to_string()
            .contains("Toulouse"));
        assert!(create_request(&free, &cat, &input(1, 1)).is_ok());
    }

    #[test]
    fn pro_creation_skips_location_and_mybox_bounds_but_keeps_the_panel_ceiling() {
        let mut raw = raw_mybox();
        raw["mybox"]["is_pro"] = json!(1);
        let c = capacity_from(&raw, 42, None);
        let body = create_request(&c, &catalog(), &CreateServerInput { location_id: None, ..input(4, 8) })
            .unwrap()
            .body
            .unwrap();
        assert!(body.get("location").is_none());
        assert!(create_request(&c, &catalog(), &input(5, 8)).is_err(), "au-delà de 2 × 2 cœurs recommandés");
    }

    #[test]
    fn resize_counts_what_the_server_already_holds() {
        let c = cap(); // 0 cœur et 12 Go libres ; « Survie » (id 1) tient 4 cœurs et 10 Go.
        let req = resize_request(&c, 1, 4, 22).unwrap();
        assert_eq!((req.method, req.path.as_str()), (Method::PATCH, "/mybox/42/server"));
        assert_eq!(req.body.unwrap(), json!({ "id_server": 1, "cpu": 4, "ram": 22 }));
        assert!(resize_request(&c, 1, 5, 10).unwrap_err().to_string().contains("5 cœur(s) demandé(s), 4 disponible(s)"));
        assert!(resize_request(&c, 1, 4, 10).unwrap_err().to_string().contains("rien à modifier"));
        assert!(resize_request(&c, 1, 4, 0).is_err());
        // Serveur en pause : n'occupe rien, borné par ce qui reste (0 cœur).
        assert!(resize_request(&c, 9, 1, 1).is_err());
        assert!(resize_request(&c, 404, 1, 1).unwrap_err().to_string().contains("n'appartient pas"));
    }

    #[test]
    fn pause_and_resume_check_slots_and_resources() {
        let c = cap();
        let pause = pause_request(&c, 1, true).unwrap();
        assert_eq!((pause.path.as_str(), pause.body.unwrap()), ("/mybox/42/server/disable", json!({ "id_server": 1 })));
        assert!(pause_request(&c, 9, true).unwrap_err().to_string().contains("déjà en pause"));
        // Reprendre « Ancien » (3 cœurs, 8 Go) : il ne reste aucun cœur.
        assert!(pause_request(&c, 9, false).unwrap_err().to_string().contains("Ressources insuffisantes"));
        let mut roomy = c.clone();
        roomy.cpu.remaining = 3;
        let resume = pause_request(&roomy, 9, false).unwrap();
        assert_eq!(resume.path, "/mybox/42/server/enable");
        assert!(pause_request(&c, 1, false).unwrap_err().to_string().contains("déjà actif"));
        let mut many_paused = c.clone();
        many_paused.slots.paused = many_paused.slots.max_paused;
        assert!(pause_request(&many_paused, 1, true).unwrap_err().to_string().contains("en pause atteint"));
    }

    #[test]
    fn order_sends_the_full_list_like_the_panel() {
        let c = cap();
        let req = order_request(&c, &[3, 1]).unwrap();
        assert_eq!(req.path, "/mybox/42/servers/order");
        assert_eq!(req.body.unwrap(), json!({ "servers_ids": [3, 1, 2, 9] }));
        assert!(order_request(&c, &[1, 1]).is_err());
        assert!(order_request(&c, &[404]).is_err());
        assert!(order_request(&c, &[]).is_err());
    }

    #[test]
    fn renames_are_trimmed_and_bounded() {
        let r = rename_server_request(5, "  Survie 2 ").unwrap();
        assert_eq!((r.method, r.path.as_str(), r.body.unwrap()), (Method::PATCH, "/server/5/name", json!({ "name": "Survie 2" })));
        let r = rename_mybox_request(42, "Réseau").unwrap();
        assert_eq!((r.path.as_str(), r.body.unwrap()), ("/mybox/42/name", json!({ "name": "Réseau" })));
        assert!(rename_mybox_request(42, &"é".repeat(50)).is_ok(), "50 caractères, pas 50 octets");
        assert!(rename_mybox_request(42, &"é".repeat(51)).is_err());
        assert!(rename_server_request(5, "a\nb").is_err());
    }

    #[test]
    fn deletion_requires_the_exact_server_name() {
        let c = cap();
        assert!(delete_request(&c, 1, "survie").is_err(), "casse comprise");
        assert!(delete_request(&c, 1, "").is_err());
        assert!(delete_request(&c, 1, "Lobby").is_err(), "le nom d'un autre serveur");
        assert!(delete_request(&c, 404, "Survie").is_err());
        let req = delete_request(&c, 1, " Survie ").unwrap();
        assert_eq!((req.method, req.path.as_str(), req.body), (Method::DELETE, "/mybox/42/server/1", None));
    }

    #[test]
    fn snapshot_before_delete_follows_the_panel_conditions() {
        let c = cap();
        let req = snapshot_request(&c, 1).unwrap();
        assert_eq!((req.method, req.path.as_str()), (Method::POST, "/mybox/42/server/1/snapshot-before-delete"));
        assert!(snapshot_request(&c, 9).is_ok(), "en pause (donc « suspendu ») : autorisé");
        let free = MyBoxCapacity { free: true, ..c.clone() };
        assert!(snapshot_request(&free, 1).is_err());
        let mut expired_server = c.clone();
        expired_server.servers[0].expired = true;
        assert!(snapshot_request(&expired_server, 1).is_err());
    }

    #[test]
    fn backup_jobs_are_read_leniently() {
        let j = backup_job_from(&json!({ "status": "completed", "progress": "100" }));
        assert!(j.finished() && j.progress == 100 && j.error.is_none());
        let j = backup_job_from(&json!({ "status": "failed", "error_message": "disque plein" }));
        assert_eq!((j.status.as_str(), j.error.as_deref()), ("failed", Some("disque plein")));
        assert!(!backup_job_from(&json!({})).finished());
    }

    fn mybox(id: i64, cpu: i64, ram: i64, slots_left: i64) -> MyBoxCapacity {
        let mut c = MyBoxCapacity {
            id,
            name: format!("Box {id}"),
            owner: true,
            cpu: ResourceUsage { total: 8, allocated: 8 - cpu, remaining: cpu },
            ram: ResourceUsage { total: 32, allocated: 32 - ram, remaining: ram },
            slots: MyBoxSlots { max_active: 4, active: 4 - slots_left, max_paused: 6, paused: 0 },
            ..Default::default()
        };
        c.blockers = blockers(&c);
        c
    }

    fn req(label: &str, cpu: i64, ram: i64) -> PlacementRequest {
        PlacementRequest { label: label.into(), cpu, ram, disk_gb: 0 }
    }

    #[test]
    fn placement_pack_fills_the_tightest_box_and_spread_the_emptiest() {
        let caps = [mybox(1, 2, 6, 2), mybox(2, 6, 24, 2)];
        let pack = plan_placement(&caps, &[req("lobby", 2, 4)], "pack").unwrap();
        assert_eq!(pack.choices[0].mybox_id, Some(1));
        assert_eq!((pack.choices[0].cpu_left, pack.choices[0].ram_left), (0, 2));
        let spread = plan_placement(&caps, &[req("lobby", 2, 4)], "spread").unwrap();
        assert_eq!(spread.choices[0].mybox_id, Some(2));
        assert!(plan_placement(&caps, &[req("x", 1, 1)], "aléatoire").is_err());
        assert!(plan_placement(&caps, &[], "pack").is_err());
        assert!(plan_placement(&caps, &[req("x", 0, 1)], "pack").is_err(), "unités invalides");
    }

    #[test]
    fn placement_handles_several_servers_biggest_first_and_tracks_what_is_left() {
        let caps = [mybox(1, 4, 12, 3), mybox(2, 2, 8, 1)];
        let plan = plan_placement(&caps, &[req("petit", 1, 2), req("gros", 3, 10), req("moyen", 2, 6)], "pack").unwrap();
        // Ordre de la réponse = ordre reçu ; les plus gros sont placés d'abord.
        assert_eq!(plan.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["petit", "gros", "moyen"]);
        assert_eq!(plan.choices[1].mybox_id, Some(1)); // gros : seule la box 1 peut
        assert_eq!(plan.choices[2].mybox_id, Some(2)); // moyen : la box 2 juste à sa taille
        assert_eq!(plan.choices[0].mybox_id, Some(1)); // petit : reste 1 cœur et 2 Go en box 1
        assert_eq!((plan.placed, plan.unplaced), (3, 0));
    }

    #[test]
    fn placement_explains_failures_and_suggests_pauses() {
        let mut full = cap(); // 0 cœur libre, « Survie » 4 cœurs/10 Go, « Lobby » 2/6, « Créatif » 2/4
        full.name = "Principale".into();
        let mut expired = mybox(7, 8, 32, 4);
        expired.expired = true;
        let plan = plan_placement(&[full, expired], &[req("arène", 3, 8)], "pack").unwrap();
        let c = &plan.choices[0];
        assert_eq!((c.mybox_id, plan.unplaced), (None, 1));
        assert!(c.reasons.iter().any(|r| r.contains("il manque 3 cœur(s)")), "{:?}", c.reasons);
        assert!(c.reasons.iter().any(|r| r.contains("expirée")), "{:?}", c.reasons);
        assert_eq!(plan.suggestions.len(), 1, "{:?}", plan.suggestions);
        assert!(plan.suggestions[0].contains("« Survie » (id 1)"), "{:?}", plan.suggestions);
    }

    #[test]
    fn placement_respects_slots_disk_and_uses_pro_last() {
        let mut no_slot = mybox(1, 8, 32, 0);
        no_slot.blockers = blockers(&no_slot);
        let mut disk = mybox(2, 8, 32, 2);
        disk.disk = MyBoxDisk { total_gb: 10, used_mb: Some(9_000) };
        let mut pro = mybox(3, 0, 0, 2);
        pro.pro = true;
        pro.slots.max_active = 0;
        pro.blockers = blockers(&pro);
        let plan = plan_placement(&[no_slot, disk, pro], &[PlacementRequest { disk_gb: 5, ..req("monde", 2, 4) }], "spread").unwrap();
        assert_eq!(plan.choices[0].mybox_id, Some(3), "seule la Pro peut, en dernier recours");
        let plan = plan_placement(&[mybox(2, 8, 32, 2), pro_like()], &[req("x", 1, 1)], "spread").unwrap();
        assert_eq!(plan.choices[0].mybox_id, Some(2), "une MyBox classique passe avant la Pro");
    }

    fn pro_like() -> MyBoxCapacity {
        let mut pro = mybox(3, 0, 0, 2);
        pro.pro = true;
        pro.blockers = blockers(&pro);
        pro
    }

    #[test]
    fn smartscan_summary_keeps_numbers_and_units_only() {
        let raw = json!({
            "shrink_recommendation": { "can_optimize": true, "potential_cpu_savings": 3, "potential_ram_savings": 6,
                "servers": [{ "id_server": 1, "name": "Survie", "current_cpu": 4, "current_ram": 10, "suggested_cpu": 1,
                              "suggested_ram": 4, "p95_cpu_percent": 4, "p95_ram_mb": 3011,
                              "data_window": { "confidence": "excellent" } }] },
            "balance_recommendation": { "can_rebalance": false, "rebalance_reason": "ALL_SERVERS_OPTIMAL", "reallocation_plan": [] },
            "scale_recommendation": { "action": "optimize_first", "priority": "low", "reason": "RESOURCES_UNDERUTILIZED" }
        });
        let s = smartscan_summary(&raw);
        assert_eq!(s["shrink"]["servers"][0]["suggested_ram"], 4);
        assert_eq!(s["shrink"]["servers"][0]["confidence"], "excellent");
        assert!(s["shrink"]["servers"][0].get("name").is_none(), "pas de nom de serveur");
        assert_eq!(s["scale"]["action"], "optimize_first");
        assert!(s["units"].as_str().unwrap().contains("Go"));
    }

    #[test]
    fn known_api_codes_are_explained() {
        assert!(explain_api_code("API_MYBOX_NEED_OFFLINE_SERVER").unwrap().contains("arrêté"));
        assert!(explain_api_code("API_INCONNU").is_none());
    }

    #[test]
    fn field_values_never_show_in_debug_output() {
        let f = FieldValue { key: "STEAM_PASS".into(), value: "secret".into() };
        assert!(!format!("{f:?}").contains("secret"));
    }
}
