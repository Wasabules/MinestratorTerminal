//! Joueurs d'un serveur **Minecraft Java** : tous ceux déjà venus (pas seulement les connectés),
//! leur fiche et leur inventaire, lus par l'hébergeur dans le dossier `playerdata/` du monde.
//!
//! Endpoints non documentés du panel web (format relevé dans son code public, vérifié en lecture
//! avec une clé API le 2026-11-18, cf. `docs/api/REFERENCE.md`) :
//! - `GET /server/{id}/players/offline?limit&offset&search` : liste paginée (200 au plus par page),
//!   **toujours triée par date de dernière sauvegarde décroissante** — l'API ne sait pas trier
//!   autrement. Le temps de jeu (`play_time`) y est en **ticks** (statistique du jeu, 20 par
//!   seconde), `mtime` en secondes Unix (dernière écriture du fichier du joueur) ;
//! - `GET /server/{id}/players/{uuid}/profile` : statistiques, état, dates de venue ;
//! - `GET /server/{id}/players/{uuid}/inventory` : emplacements `0`–`35` (barre d'action puis sac)
//!   et équipement (`head`, `chest`, `legs`, `feet`, `offhand`).
//!
//! Pour trier par temps de jeu, l'app lit donc **toute** la liste (pages de 200, plafonnée à
//! [`ROSTER_MAX`]), la garde une minute en cache, et fait recherche, filtre, tri, pagination et
//! statistiques ici, en logique pure testable. L'édition d'inventaire du panel n'est pas reprise.

use crate::error::{Error, Result};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Taille de page maximale acceptée par l'API (une valeur plus grande est ramenée à 200).
pub(crate) const API_PAGE_MAX: i64 = 200;
/// Plafond de joueurs lus pour un serveur (25 pages) : au-delà, la liste est tronquée aux plus
/// récemment venus, ce que [`PlayersPage::truncated`] signale.
pub(crate) const ROSTER_MAX: usize = 5_000;
/// Pages lues en parallèle après la première.
const PAGE_CONCURRENCY: usize = 4;
/// Durée de vie de la liste en cache : la recherche, le tri et la pagination la relisent sans
/// rappeler l'API. Une action de modération l'invalide.
pub(crate) const ROSTER_TTL: Duration = Duration::from_secs(60);
/// Ticks de jeu par seconde.
const TICKS_PER_SEC: i64 = 20;
/// Taille de page par défaut et maximale d'une requête de l'UI.
const QUERY_LIMIT_MAX: i64 = 200;
/// Joueurs du classement « temps de jeu » des statistiques.
const TOP_COUNT: usize = 5;
/// Icônes des objets (les mêmes que le panel) : `<ID EN MAJUSCULES>.png`, objets vanilla seulement.
const ITEM_ICON_BASE: &str = "https://cdn.minestrator.com/mc/items";
/// Modes de déplacement retenus dans la fiche (statistiques `<mode>_one_cm` du jeu).
pub const DISTANCE_KINDS: &[&str] = &[
    "walk", "sprint", "crouch", "swim", "walk_on_water", "walk_under_water", "climb", "fall", "fly",
    "aviate", "boat", "minecart", "horse", "pig", "strider", "happy_ghast", "nautilus",
];

// --- Types exposés -----------------------------------------------------------

/// Statut de modération d'un joueur (lu par l'hébergeur dans `ops.json`, `whitelist.json` et
/// `banned-players.json`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayerFlags {
    pub op: bool,
    pub whitelisted: bool,
    pub banned: bool,
}

/// Joueur déjà venu sur le serveur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct KnownPlayer {
    pub uuid: String,
    /// Pseudo résolu par l'hébergeur ; `null` s'il n'a pas pu l'être.
    pub name: Option<String>,
    /// Connecté au moment de la lecture (si le serveur sait le dire, cf. `online_known`).
    pub online: bool,
    /// Temps de jeu **cumulé** depuis la première venue, en secondes.
    pub play_time_secs: Option<i64>,
    /// Dernière venue (secondes Unix) : dernière sauvegarde du fichier du joueur.
    pub last_seen: Option<i64>,
    pub flags: PlayerFlags,
}

/// Tri de la liste des joueurs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PlayerSort {
    /// Dernière venue, du plus récent au plus ancien (ordre de l'API).
    #[default]
    LastSeen,
    /// Temps de jeu cumulé, décroissant.
    PlayTime,
    /// Pseudo, alphabétique.
    Name,
}

/// Filtre de la liste des joueurs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PlayerFilter {
    #[default]
    All,
    Online,
    Op,
    Whitelisted,
    Banned,
}

/// Requête de l'UI sur la liste des joueurs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayersQuery {
    /// Sous-chaîne du pseudo (insensible à la casse) ou début d'UUID ; vide : tous.
    pub search: String,
    pub sort: PlayerSort,
    pub filter: PlayerFilter,
    pub offset: i64,
    /// Taille de page (1 à 200).
    pub limit: i64,
}

/// Petites statistiques sur l'ensemble des joueurs déjà venus (indépendantes de la recherche).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayerStats {
    /// Joueurs uniques.
    pub total: i64,
    pub online: i64,
    /// Venus dans les dernières 24 h, 7 jours, 30 jours.
    pub active_24h: i64,
    pub active_7d: i64,
    pub active_30d: i64,
    /// Temps de jeu cumulé de tous les joueurs, en secondes.
    pub total_play_time_secs: i64,
    pub op: i64,
    pub whitelisted: i64,
    pub banned: i64,
    /// Les joueurs au plus grand temps de jeu cumulé (5 au plus).
    pub top_play_time: Vec<KnownPlayer>,
}

/// Page de la liste des joueurs, avec les statistiques de l'ensemble.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayersPage {
    /// Joueurs correspondant à la recherche et au filtre.
    pub total: i64,
    pub offset: i64,
    pub players: Vec<KnownPlayer>,
    pub stats: PlayerStats,
    /// Le dossier `playerdata` du monde a été trouvé (sinon : monde pas encore créé, ou introuvable).
    pub playerdata_found: bool,
    /// D'autres mondes ont leur propre dossier `playerdata` (plugin multi-mondes) : seuls les
    /// joueurs du monde principal sont listés.
    pub other_worlds: bool,
    /// Le serveur sait dire qui est connecté (sinon `online` vaut toujours `false`).
    pub online_known: bool,
    /// Plus de joueurs que l'app n'en lit : seuls les plus récemment venus sont listés.
    pub truncated: bool,
}

/// Distance parcourue selon un mode de déplacement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayerDistance {
    /// Mode, parmi [`DISTANCE_KINDS`] (`walk`, `sprint`, `fly`…).
    pub kind: String,
    /// En blocs (mètres).
    pub blocks: f64,
}

/// Fiche d'un joueur. Tout est facultatif : l'hébergeur ne renvoie que ce que les fichiers du
/// joueur contiennent (une statistique jamais incrémentée est absente).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayerProfile {
    pub uuid: String,
    pub name: Option<String>,
    pub flags: PlayerFlags,
    /// Première venue, dernière connexion, dernière venue (secondes Unix).
    pub first_played: Option<i64>,
    pub last_login: Option<i64>,
    pub last_seen: Option<i64>,
    /// Temps de jeu cumulé, en secondes.
    pub play_time_secs: Option<i64>,
    pub sessions: Option<i64>,
    pub deaths: Option<i64>,
    pub mob_kills: Option<i64>,
    pub player_kills: Option<i64>,
    /// `survival` | `creative` | `adventure` | `spectator`.
    pub game_mode: Option<String>,
    /// Points de vie (2 par cœur) ; `max_health` absent = 20.
    pub health: Option<f64>,
    pub max_health: Option<f64>,
    /// Faim, de 0 à 20.
    pub food_level: Option<i64>,
    pub xp_level: Option<i64>,
    /// Distances parcourues, de la plus grande à la plus petite (nulles exclues).
    pub distances: Vec<PlayerDistance>,
}

/// Enchantement d'un objet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ItemEnchantment {
    /// Identifiant sans l'espace de noms `minecraft:` (`sharpness`).
    pub id: String,
    /// Nom lisible (`Sharpness`).
    pub name: String,
    pub level: i64,
}

/// Objet d'un emplacement d'inventaire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct InventoryItem {
    /// Identifiant, sans `minecraft:` pour les objets vanilla (`diamond_sword`, `create:wrench`).
    pub id: String,
    /// Nom personnalisé (renommé à l'enclume) ou, à défaut, nom tiré de l'identifiant.
    pub name: String,
    pub custom_name: bool,
    pub count: i64,
    pub enchantments: Vec<ItemEnchantment>,
    /// Usure restante, de 0 (cassé) à 1 ; `null` si l'objet n'est pas abîmé ou pas usable.
    pub durability: Option<f64>,
    /// Icône (objets vanilla seulement).
    pub icon_url: Option<String>,
}

/// Inventaire d'un joueur, lu dans son fichier `playerdata`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayerInventory {
    /// Barre d'action (9 emplacements).
    pub hotbar: Vec<Option<InventoryItem>>,
    /// Sac (27 emplacements, trois rangées de neuf).
    pub main: Vec<Option<InventoryItem>>,
    /// Casque, plastron, jambières, bottes.
    pub armor: Vec<Option<InventoryItem>>,
    pub offhand: Option<InventoryItem>,
    /// Le joueur est connecté : le fichier date de la dernière sauvegarde du monde, l'inventaire
    /// réel peut avoir changé depuis. `null` : l'hébergeur ne sait pas.
    pub online: Option<bool>,
    /// Date de la sauvegarde lue (secondes Unix).
    pub saved_at: Option<i64>,
    /// Version des données du jeu (`DataVersion`).
    pub data_version: Option<i64>,
}

// --- Lecture tolérante -------------------------------------------------------
//
// Réponses d'endpoints non documentés : chaque valeur est lue à part, et une valeur d'un type
// inattendu retombe sur « absent » au lieu de faire échouer toute la réponse.

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse::<f64>().ok().map(|f| f.round() as i64),
        _ => None,
    }
}

fn float(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
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

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn flags(v: Option<&Value>) -> PlayerFlags {
    PlayerFlags {
        op: flag(v.and_then(|f| f.get("op"))),
        whitelisted: flag(v.and_then(|f| f.get("whitelisted"))),
        banned: flag(v.and_then(|f| f.get("banned"))),
    }
}

/// UUID ramené à sa forme comparable : minuscules, sans tirets.
fn uuid_key(s: &str) -> String {
    s.chars().filter(|c| *c != '-').flat_map(char::to_lowercase).collect()
}

/// Ticks de jeu → secondes.
fn ticks_to_secs(ticks: Option<i64>) -> Option<i64> {
    ticks.map(|t| t.max(0) / TICKS_PER_SEC)
}

/// Vérifie qu'un UUID de joueur est bien formé avant de l'insérer dans un chemin d'URL : 32
/// chiffres hexadécimaux, avec ou sans les quatre tirets.
pub(crate) fn validate_uuid(uuid: &str) -> Result<String> {
    let u = uuid.trim();
    let hex = u.chars().filter(|c| *c != '-').count();
    let ok = (u.len() == 32 || u.len() == 36)
        && hex == 32
        && u.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if ok {
        Ok(u.to_ascii_lowercase())
    } else {
        Err(Error::Unexpected("identifiant de joueur (UUID) invalide".into()))
    }
}

// --- Liste des joueurs -------------------------------------------------------

/// Une page brute de `GET /server/{id}/players/offline`, décodée.
#[derive(Debug, Default)]
pub(crate) struct OfflinePage {
    total_all: i64,
    players: Vec<KnownPlayer>,
    online_known: bool,
    online_uuids: HashSet<String>,
    online_names: HashSet<String>,
    playerdata_found: bool,
    other_worlds: bool,
}

pub(crate) fn parse_offline_page(data: &Value) -> OfflinePage {
    let online = data.get("online");
    let list = |key: &str| -> Vec<String> {
        online
            .and_then(|o| o.get(key))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let players: Vec<KnownPlayer> = data
        .get("players")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            let uuid = text(p.get("uuid"))?;
            Some(KnownPlayer {
                uuid,
                name: text(p.get("name")),
                online: false,
                play_time_secs: ticks_to_secs(int(p.get("play_time"))),
                last_seen: int(p.get("mtime")).filter(|t| *t > 0),
                flags: flags(p.get("flags")),
            })
        })
        .collect();
    OfflinePage {
        total_all: int(data.get("total_all")).or_else(|| int(data.get("total"))).unwrap_or(0),
        online_known: flag(online.and_then(|o| o.get("available"))),
        online_uuids: list("uuids").iter().map(|u| uuid_key(u)).collect(),
        online_names: list("names").iter().map(|n| n.to_lowercase()).collect(),
        // Le panel ne signale l'absence de dossier que sur un `false` explicite.
        playerdata_found: !matches!(data.get("playerdata_dir_found"), Some(Value::Bool(false))),
        other_worlds: data
            .get("other_playerdata_dirs")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty()),
        players,
    }
}

/// Tous les joueurs déjà venus d'un serveur, tels que lus à un instant donné.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Roster {
    pub players: Vec<KnownPlayer>,
    pub playerdata_found: bool,
    pub other_worlds: bool,
    pub online_known: bool,
    pub truncated: bool,
}

/// Lit toute la liste, page après page (`fetch(offset)` renvoie le `data` d'une page de
/// [`API_PAGE_MAX`]). La première page donne le total ; les suivantes sont lues en parallèle.
/// Doublons éliminés : un joueur qui se connecte pendant la lecture change de rang.
pub(crate) async fn fetch_roster<F, Fut>(fetch: F) -> Result<Roster>
where
    F: Fn(i64) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let first = parse_offline_page(&fetch(0).await?);
    let step = first.players.len();
    let total = usize::try_from(first.total_all).unwrap_or(0).max(step);
    let wanted = total.min(ROSTER_MAX);
    let mut pages = vec![first];
    if step > 0 && wanted > step {
        let offsets: Vec<i64> = (step..wanted).step_by(step).map(|o| o as i64).collect();
        let rest: Vec<Result<Value>> = stream::iter(offsets.into_iter().map(&fetch))
            .buffered(PAGE_CONCURRENCY)
            .collect()
            .await;
        for page in rest {
            pages.push(parse_offline_page(&page?));
        }
    }
    Ok(merge_pages(pages, total > ROSTER_MAX))
}

fn merge_pages(pages: Vec<OfflinePage>, truncated: bool) -> Roster {
    let mut seen = HashSet::new();
    let mut online_uuids = HashSet::new();
    let mut online_names = HashSet::new();
    let mut roster = Roster { truncated, ..Default::default() };
    for (i, page) in pages.into_iter().enumerate() {
        if i == 0 {
            roster.playerdata_found = page.playerdata_found;
            roster.other_worlds = page.other_worlds;
            roster.online_known = page.online_known;
        }
        online_uuids.extend(page.online_uuids);
        online_names.extend(page.online_names);
        for p in page.players {
            if seen.insert(uuid_key(&p.uuid)) && roster.players.len() < ROSTER_MAX {
                roster.players.push(p);
            }
        }
    }
    for p in &mut roster.players {
        p.online = online_uuids.contains(&uuid_key(&p.uuid))
            || p.name.as_ref().is_some_and(|n| online_names.contains(&n.to_lowercase()));
    }
    roster
}

/// Le joueur correspond-il à la recherche ? (pseudo contenant le texte, ou UUID commençant par lui)
fn matches_search(p: &KnownPlayer, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if p.name.as_ref().is_some_and(|n| n.to_lowercase().contains(needle)) {
        return true;
    }
    let key = uuid_key(needle);
    key.len() >= 4 && uuid_key(&p.uuid).starts_with(&key)
}

fn matches_filter(p: &KnownPlayer, filter: PlayerFilter) -> bool {
    match filter {
        PlayerFilter::All => true,
        PlayerFilter::Online => p.online,
        PlayerFilter::Op => p.flags.op,
        PlayerFilter::Whitelisted => p.flags.whitelisted,
        PlayerFilter::Banned => p.flags.banned,
    }
}

/// Clé de tri alphabétique : pseudo en minuscules, les joueurs sans pseudo à la fin.
fn name_key(p: &KnownPlayer) -> (bool, String) {
    match &p.name {
        Some(n) => (false, n.to_lowercase()),
        None => (true, p.uuid.clone()),
    }
}

/// Trie des joueurs (tri stable : à égalité, l'ordre de l'API — dernière venue — est conservé).
pub(crate) fn sort_players(players: &mut [KnownPlayer], sort: PlayerSort) {
    match sort {
        PlayerSort::LastSeen => players.sort_by_key(|p| std::cmp::Reverse(p.last_seen.unwrap_or(0))),
        PlayerSort::PlayTime => {
            players.sort_by_key(|p| std::cmp::Reverse(p.play_time_secs.unwrap_or(0)));
        }
        PlayerSort::Name => players.sort_by_key(name_key),
    }
}

/// Statistiques de l'ensemble des joueurs, à l'instant `now` (secondes Unix).
pub(crate) fn compute_stats(players: &[KnownPlayer], now: i64) -> PlayerStats {
    let seen_within = |secs: i64| {
        players
            .iter()
            .filter(|p| p.online || p.last_seen.is_some_and(|t| now - t <= secs))
            .count() as i64
    };
    let count = |f: fn(&KnownPlayer) -> bool| players.iter().filter(|p| f(p)).count() as i64;
    let mut top: Vec<KnownPlayer> = players
        .iter()
        .filter(|p| p.play_time_secs.unwrap_or(0) > 0)
        .cloned()
        .collect();
    sort_players(&mut top, PlayerSort::PlayTime);
    top.truncate(TOP_COUNT);
    PlayerStats {
        total: players.len() as i64,
        online: count(|p| p.online),
        active_24h: seen_within(86_400),
        active_7d: seen_within(7 * 86_400),
        active_30d: seen_within(30 * 86_400),
        total_play_time_secs: players.iter().filter_map(|p| p.play_time_secs).sum(),
        op: count(|p| p.flags.op),
        whitelisted: count(|p| p.flags.whitelisted),
        banned: count(|p| p.flags.banned),
        top_play_time: top,
    }
}

/// Applique recherche, filtre, tri et pagination à la liste, et y joint les statistiques.
pub(crate) fn query_roster(roster: &Roster, q: &PlayersQuery, now: i64) -> PlayersPage {
    let needle = q.search.trim().to_lowercase();
    let mut matching: Vec<KnownPlayer> = roster
        .players
        .iter()
        .filter(|p| matches_filter(p, q.filter) && matches_search(p, &needle))
        .cloned()
        .collect();
    sort_players(&mut matching, q.sort);
    let total = matching.len() as i64;
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(QUERY_LIMIT_MAX) };
    // Au-delà de la fin (liste raccourcie depuis la page précédente) : dernière page.
    let offset = if q.offset >= total { ((total - 1).max(0) / limit) * limit } else { q.offset.max(0) };
    let players = matching
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    PlayersPage {
        total,
        offset,
        players,
        stats: compute_stats(&roster.players, now),
        playerdata_found: roster.playerdata_found,
        other_worlds: roster.other_worlds,
        online_known: roster.online_known,
        truncated: roster.truncated,
    }
}

// --- Rapport pour un agent IA (outil MCP `list_players`) ---------------------------

/// Paramètres de l'outil MCP `list_players`.
pub(crate) struct AiQuery {
    pub sort: PlayerSort,
    pub search: String,
    /// Seulement les joueurs venus dans les N derniers jours (connectés compris) ; 0 : tous.
    pub active_days: i64,
    pub limit: usize,
}

/// Heures arrondies au dixième.
fn hours(secs: i64) -> f64 {
    (secs as f64 / 360.0).round() / 10.0
}

/// Date `AAAA-MM-JJ` (UTC) d'un horodatage Unix.
fn day(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y-%m-%d").to_string())
}

/// Rapport compact des joueurs pour un agent IA : statistiques globales puis liste triée.
/// `anonymize` (réglage « anonymiser pour l'IA ») remplace chaque pseudo par un alias stable
/// ([`crate::redact::pseudonym`]) ; l'UUID n'est jamais envoyé.
pub(crate) fn ai_report(roster: &Roster, q: &AiQuery, now: i64, anonymize: bool) -> Value {
    let needle = q.search.trim().to_lowercase();
    let window = q.active_days.max(0) * 86_400;
    let mut matching: Vec<KnownPlayer> = roster
        .players
        .iter()
        .filter(|p| matches_search(p, &needle))
        .filter(|p| window == 0 || p.online || p.last_seen.is_some_and(|t| now - t <= window))
        .cloned()
        .collect();
    sort_players(&mut matching, q.sort);
    let label = |p: &KnownPlayer| -> String {
        if anonymize {
            crate::redact::pseudonym(&p.uuid)
        } else {
            p.name.clone().unwrap_or_else(|| format!("(pseudo inconnu, UUID {})", p.uuid))
        }
    };
    let list: Vec<Value> = matching
        .iter()
        .take(q.limit)
        .map(|p| {
            serde_json::json!({
                "player": label(p),
                "online": p.online,
                "play_time_hours": p.play_time_secs.map(hours),
                "last_seen": p.last_seen.and_then(day),
                "days_since_last_seen": p.last_seen.map(|t| (now - t).max(0) / 86_400),
                "op": p.flags.op,
                "whitelisted": p.flags.whitelisted,
                "banned": p.flags.banned,
            })
        })
        .collect();
    let s = compute_stats(&roster.players, now);
    serde_json::json!({
        "players_known": s.total,
        "online_now": s.online,
        "online_detection_available": roster.online_known,
        "active_last_24h": s.active_24h,
        "active_last_7d": s.active_7d,
        "active_last_30d": s.active_30d,
        "total_play_time_hours": hours(s.total_play_time_secs),
        "matching": matching.len(),
        "shown": list.len(),
        "names_anonymized": anonymize,
        "list_truncated_to_most_recent": roster.truncated,
        "note": "play_time_hours = temps de jeu CUMULÉ depuis la première venue (aucun détail par \
                 période : « cette semaine » se déduit de last_seen, pas du temps de jeu) ; \
                 last_seen = dernière sauvegarde du joueur (UTC).",
        "players": list,
    })
}

/// Listes de joueurs lues récemment, par serveur (voir [`ROSTER_TTL`]).
#[derive(Default)]
pub(crate) struct RosterCache {
    map: Mutex<HashMap<i64, (Instant, Arc<Roster>)>>,
}

impl RosterCache {
    pub(crate) fn get(&self, server_id: i64) -> Option<Arc<Roster>> {
        let map = self.map.lock().ok()?;
        map.get(&server_id)
            .filter(|(at, _)| at.elapsed() < ROSTER_TTL)
            .map(|(_, r)| r.clone())
    }

    pub(crate) fn put(&self, server_id: i64, roster: Arc<Roster>) {
        if let Ok(mut map) = self.map.lock() {
            map.retain(|_, (at, _)| at.elapsed() < ROSTER_TTL);
            map.insert(server_id, (Instant::now(), roster));
        }
    }

    /// Oublie la liste d'un serveur (après une action qui change les drapeaux d'un joueur).
    pub(crate) fn invalidate(&self, server_id: i64) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&server_id);
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut map) = self.map.lock() {
            map.clear();
        }
    }
}

// --- Fiche ---------------------------------------------------------------------

const GAME_MODES: &[&str] = &["survival", "creative", "adventure", "spectator"];

pub(crate) fn parse_profile(data: &Value, uuid: &str) -> PlayerProfile {
    let times = data.get("times");
    let state = data.get("state");
    let stats = data.get("stats");
    let time = |key: &str| int(times.and_then(|t| t.get(key))).filter(|t| *t > 0);
    let stat = |key: &str| int(stats.and_then(|s| s.get(key)));
    let mut distances: Vec<PlayerDistance> = stats
        .and_then(|s| s.get("distances"))
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let kind = k.trim_start_matches("minecraft:").trim_end_matches("_one_cm");
                    let cm = float(Some(v))?;
                    (DISTANCE_KINDS.contains(&kind) && cm > 0.0)
                        .then(|| PlayerDistance { kind: kind.to_string(), blocks: cm / 100.0 })
                })
                .collect()
        })
        .unwrap_or_default();
    distances.sort_by(|a, b| b.blocks.total_cmp(&a.blocks));
    PlayerProfile {
        uuid: text(data.get("uuid")).unwrap_or_else(|| uuid.to_string()),
        name: text(data.get("name")),
        flags: flags(data.get("flags")),
        first_played: time("first_played"),
        last_login: time("last_login"),
        last_seen: time("last_seen")
            .or_else(|| time("last_played"))
            .or_else(|| int(data.get("mtime")).filter(|t| *t > 0)),
        play_time_secs: ticks_to_secs(stat("play_time")),
        sessions: stat("sessions"),
        deaths: stat("deaths"),
        mob_kills: stat("mob_kills"),
        player_kills: stat("player_kills"),
        game_mode: int(state.and_then(|s| s.get("game_mode")))
            .and_then(|m| usize::try_from(m).ok())
            .and_then(|m| GAME_MODES.get(m))
            .map(|m| m.to_string()),
        health: float(state.and_then(|s| s.get("health"))),
        max_health: float(state.and_then(|s| s.get("max_health"))),
        food_level: int(state.and_then(|s| s.get("food_level"))),
        xp_level: int(state.and_then(|s| s.get("xp_level"))),
        distances,
    }
}

// --- Inventaire ----------------------------------------------------------------

/// `diamond_sword` → `Diamond Sword` (l'espace de noms est retiré).
pub(crate) fn humanize_id(id: &str) -> String {
    let base = id.rsplit(':').next().unwrap_or(id);
    base.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Texte brut d'un composant de texte Minecraft : chaîne simple, JSON sérialisé (formats
/// anciens), objet `{ text, extra }` ou tableau de composants.
fn plain_text(v: &Value) -> String {
    match v {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(parsed @ (Value::Object(_) | Value::Array(_) | Value::String(_))) => component_text(&parsed),
            _ => s.clone(),
        },
        other => component_text(other),
    }
}

fn component_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(component_text).collect(),
        Value::Object(o) => {
            let mut out = o.get("text").map(component_text).unwrap_or_default();
            if let Some(extra) = o.get("extra") {
                out.push_str(&component_text(extra));
            }
            out
        }
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
    }
}

/// Durabilité maximale des objets usables courants (à défaut du composant `max_damage`).
fn max_durability(id: &str) -> Option<i64> {
    const TIERS: &[(&str, i64)] = &[
        ("wooden", 59),
        ("stone", 131),
        ("iron", 250),
        ("golden", 32),
        ("diamond", 1561),
        ("netherite", 2031),
    ];
    const ARMOR: &[(&str, [i64; 4])] = &[
        ("leather", [55, 80, 75, 65]),
        ("chainmail", [165, 240, 225, 195]),
        ("iron", [165, 240, 225, 195]),
        ("golden", [77, 112, 105, 91]),
        ("diamond", [363, 528, 495, 429]),
        ("netherite", [407, 592, 555, 481]),
    ];
    const OTHERS: &[(&str, i64)] = &[
        ("turtle_helmet", 275),
        ("elytra", 432),
        ("shield", 336),
        ("bow", 384),
        ("crossbow", 465),
        ("trident", 250),
        ("fishing_rod", 64),
        ("flint_and_steel", 64),
        ("shears", 238),
        ("carrot_on_a_stick", 25),
        ("warped_fungus_on_a_stick", 100),
        ("brush", 64),
        ("mace", 500),
    ];
    if let Some((_, d)) = OTHERS.iter().find(|(name, _)| *name == id) {
        return Some(*d);
    }
    let (material, kind) = id.split_once('_')?;
    if ["sword", "pickaxe", "axe", "shovel", "hoe"].contains(&kind) {
        return TIERS.iter().find(|(m, _)| *m == material).map(|(_, d)| *d);
    }
    let piece = ["helmet", "chestplate", "leggings", "boots"].iter().position(|p| *p == kind)?;
    ARMOR.iter().find(|(m, _)| *m == material).map(|(_, d)| d[piece])
}

/// Enchantements d'un objet : composants `minecraft:enchantments` / `stored_enchantments`
/// (`{ levels: { id: niveau } }` jusqu'en 1.21.4, `{ id: niveau }` ensuite) ou ancien `tag`
/// (`Enchantments: [{ id, lvl }]`).
fn enchantments(item: &Value) -> Vec<ItemEnchantment> {
    let mut out = Vec::new();
    let components = item.get("components");
    for key in ["minecraft:enchantments", "minecraft:stored_enchantments"] {
        let Some(e) = components.and_then(|c| c.get(key)) else { continue };
        let levels = e.get("levels").unwrap_or(e);
        if let Some(map) = levels.as_object() {
            for (id, level) in map {
                push_enchantment(&mut out, id, int(Some(level)).unwrap_or(1));
            }
        }
    }
    let tag = item.get("tag");
    for key in ["Enchantments", "StoredEnchantments"] {
        let Some(list) = tag.and_then(|t| t.get(key)).and_then(Value::as_array) else { continue };
        for e in list {
            if let Some(id) = text(e.get("id")) {
                let level = int(e.get("lvl")).or_else(|| int(e.get("level"))).unwrap_or(1);
                push_enchantment(&mut out, &id, level);
            }
        }
    }
    out
}

fn push_enchantment(out: &mut Vec<ItemEnchantment>, id: &str, level: i64) {
    let id = id.trim_start_matches("minecraft:").to_string();
    if !id.is_empty() {
        out.push(ItemEnchantment { name: humanize_id(&id), id, level: level.max(1) });
    }
}

/// Décode un objet d'inventaire (`{ id, count, components }`, ou ancien `{ id, Count, tag }`).
/// `None` pour un emplacement vide (`air` compris).
pub(crate) fn parse_item(item: &Value) -> Option<InventoryItem> {
    let raw_id = text(item.get("id"))?;
    let id = raw_id.strip_prefix("minecraft:").unwrap_or(&raw_id).to_string();
    if matches!(id.as_str(), "air" | "cave_air" | "void_air") {
        return None;
    }
    let count = int(item.get("count")).or_else(|| int(item.get("Count"))).unwrap_or(1);
    if count <= 0 {
        return None;
    }
    let components = item.get("components");
    let tag = item.get("tag");
    let custom = components
        .and_then(|c| c.get("minecraft:custom_name"))
        .or_else(|| tag.and_then(|t| t.get("display")).and_then(|d| d.get("Name")))
        .map(plain_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let damage = int(components.and_then(|c| c.get("minecraft:damage")))
        .or_else(|| int(tag.and_then(|t| t.get("Damage"))));
    let durability = damage.filter(|d| *d > 0).and_then(|d| {
        let max = int(components.and_then(|c| c.get("minecraft:max_damage")))
            .or_else(|| max_durability(&id))?;
        (max > 0).then(|| ((max - d) as f64 / max as f64).clamp(0.0, 1.0))
    });
    let vanilla = !id.contains(':');
    Some(InventoryItem {
        name: custom.clone().unwrap_or_else(|| humanize_id(&id)),
        custom_name: custom.is_some(),
        count,
        enchantments: enchantments(item),
        durability,
        icon_url: vanilla.then(|| format!("{ITEM_ICON_BASE}/{}.png", id.to_ascii_uppercase())),
        id,
    })
}

/// Décode l'inventaire : emplacements `0`–`8` (barre d'action), `9`–`35` (sac), équipement par
/// nom. Les numéros d'emplacement des anciens formats (`100`–`103` armure, `-106` main
/// secondaire) sont aussi compris.
pub(crate) fn parse_inventory(data: &Value) -> PlayerInventory {
    let mut inv = PlayerInventory {
        hotbar: vec![None; 9],
        main: vec![None; 27],
        armor: vec![None; 4],
        offhand: None,
        online: data.get("online").and_then(|o| o.get("is_online")).and_then(Value::as_bool),
        saved_at: int(data.get("mtime")).filter(|t| *t > 0),
        data_version: int(data.get("data_version")),
    };
    if let Some(slots) = data.get("inventory").and_then(Value::as_object) {
        for (slot, raw) in slots {
            let (Ok(n), Some(item)) = (slot.trim().parse::<i64>(), parse_item(raw)) else { continue };
            match n {
                0..=8 => inv.hotbar[n as usize] = Some(item),
                9..=35 => inv.main[(n - 9) as usize] = Some(item),
                100..=103 => inv.armor[(103 - n) as usize] = Some(item),
                -106 => inv.offhand = Some(item),
                _ => {}
            }
        }
    }
    if let Some(eq) = data.get("equipment").and_then(Value::as_object) {
        for (i, piece) in ["head", "chest", "legs", "feet"].iter().enumerate() {
            if let Some(item) = eq.get(*piece).and_then(parse_item) {
                inv.armor[i] = Some(item);
            }
        }
        if let Some(item) = eq.get("offhand").and_then(parse_item) {
            inv.offhand = Some(item);
        }
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const U1: &str = "0f9c1f9e-1111-4c4c-9b9b-000000000001";
    const U2: &str = "0f9c1f9e-2222-4c4c-9b9b-000000000002";
    const U3: &str = "0f9c1f9e-3333-4c4c-9b9b-000000000003";

    fn player(uuid: &str, name: Option<&str>, play_ticks: i64, mtime: i64) -> Value {
        json!({
            "uuid": uuid, "name": name, "size": 1024, "mtime": mtime, "play_time": play_ticks,
            "flags": { "op": false, "whitelisted": false, "banned": false }
        })
    }

    fn known(uuid: &str, name: &str, play: i64, seen: i64) -> KnownPlayer {
        KnownPlayer {
            uuid: uuid.into(),
            name: Some(name.into()),
            online: false,
            play_time_secs: Some(play),
            last_seen: Some(seen),
            flags: PlayerFlags::default(),
        }
    }

    #[test]
    fn page_is_read_leniently() {
        // Forme relevée le 2026-11-18, avec des écarts de type que l'API (PHP) peut produire.
        let data = json!({
            "level_name": "world", "total": "3", "total_all": 3.0, "limit": 200, "offset": 0,
            "resolved_names": 2,
            "players": [
                player(U1, Some("Alex"), 72_000, 1_700_000_000),
                { "uuid": U2, "name": null, "mtime": "1699999000", "play_time": null,
                  "flags": { "op": 1, "whitelisted": "true", "banned": false } },
                { "name": "sans uuid" },
                "pas un objet"
            ],
            "other_playerdata_dirs": ["world_nether/playerdata"],
            "playerdata_dir_found": true,
            "online": { "available": true, "names": ["ALEX"], "uuids": [], "count": 1 }
        });
        let page = parse_offline_page(&data);
        assert_eq!(page.total_all, 3);
        assert_eq!(page.players.len(), 2, "entrées sans UUID ignorées");
        let alex = &page.players[0];
        assert_eq!(alex.play_time_secs, Some(3600), "72 000 ticks = 1 h");
        assert_eq!(alex.last_seen, Some(1_700_000_000));
        let anon = &page.players[1];
        assert_eq!(anon.name, None);
        assert_eq!(anon.play_time_secs, None);
        assert_eq!(anon.last_seen, Some(1_699_999_000));
        assert_eq!(anon.flags, PlayerFlags { op: true, whitelisted: true, banned: false });
        assert!(page.online_known && page.other_worlds && page.playerdata_found);
        assert!(page.online_names.contains("alex"));
    }

    #[test]
    fn empty_or_unexpected_page_does_not_fail() {
        let page = parse_offline_page(&json!({}));
        assert!(page.players.is_empty());
        assert!(page.playerdata_found, "seul un `false` explicite signale l'absence du dossier");
        assert!(!page.online_known);
        assert!(!parse_offline_page(&json!({ "playerdata_dir_found": false })).playerdata_found);
        assert!(parse_offline_page(&json!({ "players": null, "online": [] })).players.is_empty());
    }

    #[tokio::test]
    async fn roster_reads_every_page_and_dedups() {
        // 5 joueurs, pages de 2 : offsets 0, 2, 4. U2 « remonte » entre deux lectures (doublon).
        let all: Vec<Value> = (1..=5)
            .map(|i| player(&format!("00000000-0000-0000-0000-00000000000{i}"), Some(format!("J{i}").as_str()), i * 20, 100 - i))
            .collect();
        let calls = std::sync::Mutex::new(Vec::new());
        let roster = fetch_roster(|offset| {
            calls.lock().unwrap().push(offset);
            let mut slice: Vec<Value> = all.iter().skip(offset as usize).take(2).cloned().collect();
            if offset == 2 {
                slice[0] = all[1].clone(); // doublon de la page précédente
            }
            let page = json!({
                "total_all": 5, "players": slice,
                "online": { "available": true, "uuids": ["00000000000000000000000000000005"], "names": [] }
            });
            async move { Ok(page) }
        })
        .await
        .unwrap();
        let mut offsets = calls.into_inner().unwrap();
        offsets.sort_unstable();
        assert_eq!(offsets, vec![0, 2, 4]);
        let names: Vec<_> = roster.players.iter().filter_map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["J1", "J2", "J4", "J5"], "doublon éliminé, ordre conservé");
        assert!(roster.players.iter().find(|p| p.name.as_deref() == Some("J5")).unwrap().online);
        assert!(!roster.truncated);
    }

    #[tokio::test]
    async fn roster_stops_on_error_and_on_empty_first_page() {
        let err = fetch_roster(|offset| async move {
            if offset == 0 {
                Ok(json!({ "total_all": 4, "players": [player(U1, Some("A"), 0, 1), player(U2, Some("B"), 0, 1)] }))
            } else {
                Err(Error::RateLimited)
            }
        })
        .await;
        assert!(matches!(err, Err(Error::RateLimited)));
        let empty = fetch_roster(|_| async { Ok(json!({ "total_all": 10, "players": [] })) }).await.unwrap();
        assert!(empty.players.is_empty());
    }

    fn roster() -> Roster {
        let mut a = known(U1, "Alex", 10 * 3600, 1_000_000);
        a.flags.op = true;
        let mut b = known(U2, "bob", 50 * 3600, 900_000);
        b.online = true;
        let mut c = known(U3, "Charlie", 0, 100_000);
        c.flags.banned = true;
        c.play_time_secs = None;
        Roster { players: vec![a, b, c], playerdata_found: true, ..Default::default() }
    }

    fn query(search: &str, sort: PlayerSort, filter: PlayerFilter, offset: i64, limit: i64) -> PlayersQuery {
        PlayersQuery { search: search.into(), sort, filter, offset, limit }
    }

    fn names(page: &PlayersPage) -> Vec<String> {
        page.players.iter().filter_map(|p| p.name.clone()).collect()
    }

    #[test]
    fn sorts_searches_filters_and_paginates() {
        let r = roster();
        let now = 1_000_000;
        let by = |sort| names(&query_roster(&r, &query("", sort, PlayerFilter::All, 0, 50), now));
        assert_eq!(by(PlayerSort::LastSeen), vec!["Alex", "bob", "Charlie"]);
        assert_eq!(by(PlayerSort::PlayTime), vec!["bob", "Alex", "Charlie"]);
        assert_eq!(by(PlayerSort::Name), vec!["Alex", "bob", "Charlie"], "insensible à la casse");

        let found = query_roster(&r, &query("  LI ", PlayerSort::Name, PlayerFilter::All, 0, 50), now);
        assert_eq!(names(&found), vec!["Charlie"]);
        // Recherche par début d'UUID (4 caractères au moins, tirets ignorés).
        let by_uuid = query_roster(&r, &query("0F9C1F9E2222", PlayerSort::Name, PlayerFilter::All, 0, 50), now);
        assert_eq!(names(&by_uuid), vec!["bob"]);

        let only = |filter| names(&query_roster(&r, &query("", PlayerSort::Name, filter, 0, 50), now));
        assert_eq!(only(PlayerFilter::Online), vec!["bob"]);
        assert_eq!(only(PlayerFilter::Op), vec!["Alex"]);
        assert_eq!(only(PlayerFilter::Banned), vec!["Charlie"]);
        assert!(only(PlayerFilter::Whitelisted).is_empty());

        let page = query_roster(&r, &query("", PlayerSort::PlayTime, PlayerFilter::All, 1, 1), now);
        assert_eq!((page.total, page.offset), (3, 1));
        assert_eq!(names(&page), vec!["Alex"]);
        let past_end = query_roster(&r, &query("", PlayerSort::Name, PlayerFilter::All, 99, 2), now);
        assert_eq!((past_end.offset, past_end.players.len()), (2, 1), "au-delà de la fin : dernière page");
        let none = query_roster(&r, &query("zzz", PlayerSort::Name, PlayerFilter::All, 5, 2), now);
        assert_eq!((none.total, none.offset), (0, 0));
        let bounded = query_roster(&r, &query("", PlayerSort::Name, PlayerFilter::All, -5, 0), now);
        assert_eq!((bounded.offset, bounded.players.len()), (0, 3), "limite 0 → défaut");
    }

    #[test]
    fn stats_cover_the_whole_roster() {
        let r = roster();
        let now = 1_000_000;
        let s = compute_stats(&r.players, now);
        assert_eq!((s.total, s.online, s.op, s.banned, s.whitelisted), (3, 1, 1, 1, 0));
        // Alex vu à l'instant ; bob vu il y a 100 000 s, mais connecté : compté actif.
        assert_eq!(s.active_24h, 2);
        assert_eq!(s.active_7d, 2);
        assert_eq!(s.active_30d, 3, "Charlie : vu il y a 10 jours");
        assert_eq!(s.total_play_time_secs, 60 * 3600);
        let top: Vec<_> = s.top_play_time.iter().filter_map(|p| p.name.clone()).collect();
        assert_eq!(top, vec!["bob", "Alex"], "temps de jeu nul exclu du classement");
        // Les statistiques ignorent la recherche.
        let page = query_roster(&r, &query("zzz", PlayerSort::Name, PlayerFilter::All, 0, 10), now);
        assert_eq!((page.total, page.stats.total), (0, 3));
    }

    #[test]
    fn ai_report_ranks_and_anonymizes() {
        let r = roster();
        let now = 1_000_000;
        let q = AiQuery { sort: PlayerSort::PlayTime, search: String::new(), active_days: 7, limit: 10 };
        let report = ai_report(&r, &q, now, true);
        assert_eq!(report["players_known"], 3);
        assert_eq!(report["matching"], 2, "Charlie, vu il y a 10 jours, est hors fenêtre");
        assert_eq!(report["names_anonymized"], true);
        let players = report["players"].as_array().unwrap();
        assert_eq!(players[0]["player"], crate::redact::pseudonym(U2), "bob, plus gros temps de jeu");
        assert_eq!(players[0]["play_time_hours"], 50.0);
        assert_eq!(players[0]["days_since_last_seen"], 1);
        let text = report.to_string();
        for leak in ["bob", "Alex", "0f9c1f9e"] {
            assert!(!text.contains(leak), "{leak} ne doit pas partir vers l'IA");
        }

        let clear = ai_report(&r, &AiQuery { active_days: 0, limit: 1, ..q }, now, false);
        assert_eq!((clear["matching"].as_i64(), clear["shown"].as_i64()), (Some(3), Some(1)));
        assert_eq!(clear["players"][0]["player"], "bob");
        assert_eq!(clear["players"][0]["last_seen"], "1970-01-11");
    }

    #[test]
    fn roster_cache_expires_and_invalidates() {
        let cache = RosterCache::default();
        assert!(cache.get(1).is_none());
        cache.put(1, Arc::new(roster()));
        assert_eq!(cache.get(1).map(|r| r.players.len()), Some(3));
        cache.invalidate(1);
        assert!(cache.get(1).is_none());
        cache.put(2, Arc::new(Roster::default()));
        cache.clear();
        assert!(cache.get(2).is_none());
    }

    #[test]
    fn uuid_validation_blocks_path_injection() {
        assert_eq!(validate_uuid(U1).unwrap(), U1);
        assert_eq!(validate_uuid(" 0F9C1F9E11114C4C9B9B000000000001 ").unwrap(), "0f9c1f9e11114c4c9b9b000000000001");
        for bad in ["", "../../user", "0f9c1f9e-1111-4c4c-9b9b-00000000000g", "0f9c1f9e/1111-4c4c-9b9b-000000000001", "abc"] {
            assert!(validate_uuid(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn profile_is_read_leniently() {
        // Forme relevée le 2026-11-18 (statistiques jamais incrémentées : `null`).
        let data = json!({
            "uuid": U1, "name": "Alex", "data_version": 4790, "mtime": 1_700_000_500,
            "flags": { "op": true, "whitelisted": false, "banned": false },
            "state": { "food_level": 18, "game_mode": 1, "health": 19.5, "max_health": null,
                       "xp_level": 7, "xp_progress": 0, "xp_total": 120 },
            "stats": { "deaths": null, "mob_kills": "4", "player_kills": null, "play_time": 214_174,
                       "sessions": 3,
                       "distances": { "walk_one_cm": 87_500, "fall_one_cm": 875, "fly_one_cm": 0,
                                      "inconnu_one_cm": 5, "sprint_one_cm": "120000" } },
            "times": { "first_played": 1_699_000_000, "last_login": 1_700_000_000,
                       "last_played": 1_700_000_400, "last_seen": 0 }
        });
        let p = parse_profile(&data, "ignoré");
        assert_eq!(p.uuid, U1);
        assert_eq!(p.name.as_deref(), Some("Alex"));
        assert!(p.flags.op);
        assert_eq!(p.game_mode.as_deref(), Some("creative"));
        assert_eq!((p.health, p.max_health, p.food_level, p.xp_level), (Some(19.5), None, Some(18), Some(7)));
        assert_eq!(p.play_time_secs, Some(10_708), "214 174 ticks");
        assert_eq!((p.sessions, p.deaths, p.mob_kills), (Some(3), None, Some(4)));
        assert_eq!(p.first_played, Some(1_699_000_000));
        assert_eq!(p.last_seen, Some(1_700_000_400), "last_seen nul → last_played");
        let kinds: Vec<_> = p.distances.iter().map(|d| (d.kind.as_str(), d.blocks)).collect();
        assert_eq!(kinds, vec![("sprint", 1200.0), ("walk", 875.0), ("fall", 8.75)]);

        let empty = parse_profile(&json!({}), U2);
        assert_eq!(empty.uuid, U2);
        assert_eq!(empty.game_mode, None);
        assert!(empty.distances.is_empty());
        assert_eq!(parse_profile(&json!({ "state": { "game_mode": 9 } }), U2).game_mode, None);
    }

    #[test]
    fn items_are_rendered_readably() {
        let plain = parse_item(&json!({ "id": "minecraft:stone_sword", "count": 1 })).unwrap();
        assert_eq!(plain.id, "stone_sword");
        assert_eq!(plain.name, "Stone Sword");
        assert!(!plain.custom_name);
        assert_eq!(plain.durability, None, "intact : pas de barre d'usure");
        assert_eq!(plain.icon_url.as_deref(), Some("https://cdn.minestrator.com/mc/items/STONE_SWORD.png"));

        let worn = parse_item(&json!({
            "id": "minecraft:diamond_pickaxe", "count": 1,
            "components": {
                "minecraft:damage": 1561 / 2,
                "minecraft:custom_name": { "text": "Pioche ", "extra": [{ "text": "de Bob", "bold": true }] },
                "minecraft:enchantments": { "minecraft:efficiency": 5, "minecraft:unbreaking": "3" }
            }
        }))
        .unwrap();
        assert_eq!(worn.name, "Pioche de Bob");
        assert!(worn.custom_name);
        assert!((worn.durability.unwrap() - 0.5).abs() < 0.01);
        let ench: Vec<_> = worn.enchantments.iter().map(|e| (e.name.as_str(), e.level)).collect();
        assert_eq!(ench, vec![("Efficiency", 5), ("Unbreaking", 3)]);

        // Formats 1.20.5–1.21.4 (`levels`) et anciens (`tag`, nom en JSON sérialisé, `Count`).
        let book = parse_item(&json!({
            "id": "minecraft:enchanted_book", "count": 1,
            "components": { "minecraft:stored_enchantments": { "levels": { "minecraft:mending": 1 } } }
        }))
        .unwrap();
        assert_eq!(book.enchantments[0].id, "mending");
        let legacy = parse_item(&json!({
            "id": "minecraft:iron_sword", "Count": 1,
            "tag": { "Damage": 125, "display": { "Name": "{\"text\":\"Lame\"}" },
                     "Enchantments": [{ "id": "minecraft:sharpness", "lvl": 2 }] }
        }))
        .unwrap();
        assert_eq!(legacy.name, "Lame");
        assert_eq!(legacy.enchantments[0].level, 2);
        assert!((legacy.durability.unwrap() - 0.5).abs() < 0.01);

        let stack = parse_item(&json!({ "id": "minecraft:bread", "count": "16" })).unwrap();
        assert_eq!(stack.count, 16);
        let modded = parse_item(&json!({ "id": "create:wrench", "count": 1 })).unwrap();
        assert_eq!((modded.id.as_str(), modded.name.as_str(), modded.icon_url), ("create:wrench", "Wrench", None));
        for empty in [json!({ "id": "minecraft:air", "count": 1 }), json!({ "count": 3 }), json!(null), json!({ "id": "minecraft:dirt", "count": 0 })] {
            assert!(parse_item(&empty).is_none(), "{empty}");
        }
    }

    #[test]
    fn inventory_slots_are_mapped() {
        // Forme relevée le 2026-11-18 (`format: { stack: components, equipment: equipment }`).
        let data = json!({
            "uuid": U1, "data_version": 4790, "mtime": 1_700_000_000, "sha256": "x", "roundtrip_ok": true,
            "format": { "equipment": "equipment", "stack": "components" },
            "inventory": {
                "0": { "id": "minecraft:written_book", "count": 1,
                       "components": { "minecraft:custom_name": "Guide" } },
                "8": { "id": "minecraft:bread", "count": 16 },
                "9": { "id": "minecraft:dirt", "count": 64 },
                "35": { "id": "minecraft:stone", "count": 2 },
                "36": { "id": "minecraft:stone", "count": 2 },
                "x": { "id": "minecraft:stone", "count": 2 }
            },
            "equipment": {
                "head": { "id": "minecraft:leather_helmet", "count": 1 },
                "feet": { "id": "minecraft:leather_boots", "count": 1 },
                "offhand": { "id": "minecraft:shield", "count": 1 }
            },
            "online": { "is_online": false, "players_online": 0, "query_available": true, "uuids_available": false }
        });
        let inv = parse_inventory(&data);
        assert_eq!((inv.hotbar.len(), inv.main.len(), inv.armor.len()), (9, 27, 4));
        assert_eq!(inv.hotbar[0].as_ref().unwrap().name, "Guide");
        assert_eq!(inv.hotbar[8].as_ref().unwrap().count, 16);
        assert_eq!(inv.main[0].as_ref().unwrap().id, "dirt");
        assert_eq!(inv.main[26].as_ref().unwrap().id, "stone");
        assert_eq!(inv.main.iter().flatten().count(), 2, "emplacements hors bornes ignorés");
        assert_eq!(inv.armor[0].as_ref().unwrap().id, "leather_helmet");
        assert!(inv.armor[1].is_none() && inv.armor[2].is_none());
        assert_eq!(inv.armor[3].as_ref().unwrap().id, "leather_boots");
        assert_eq!(inv.offhand.as_ref().unwrap().id, "shield");
        assert_eq!((inv.online, inv.saved_at, inv.data_version), (Some(false), Some(1_700_000_000), Some(4790)));

        // Ancien format : armure et main secondaire dans les emplacements numérotés.
        let legacy = parse_inventory(&json!({
            "inventory": { "103": { "id": "minecraft:iron_helmet", "Count": 1 },
                           "100": { "id": "minecraft:iron_boots", "Count": 1 },
                           "-106": { "id": "minecraft:torch", "Count": 12 } }
        }));
        assert_eq!(legacy.armor[0].as_ref().unwrap().id, "iron_helmet");
        assert_eq!(legacy.armor[3].as_ref().unwrap().id, "iron_boots");
        assert_eq!(legacy.offhand.as_ref().unwrap().count, 12);
        assert_eq!(legacy.online, None);
        assert!(parse_inventory(&json!({})).hotbar.iter().all(Option::is_none));
    }

    #[test]
    fn durability_table_covers_tools_and_armor() {
        assert_eq!(max_durability("netherite_sword"), Some(2031));
        assert_eq!(max_durability("golden_axe"), Some(32));
        assert_eq!(max_durability("diamond_chestplate"), Some(528));
        assert_eq!(max_durability("leather_boots"), Some(65));
        assert_eq!(max_durability("elytra"), Some(432));
        assert_eq!(max_durability("stone"), None);
        assert_eq!(max_durability("iron_ingot"), None);
        assert_eq!(humanize_id("minecraft:light_blue_wool"), "Light Blue Wool");
    }
}
