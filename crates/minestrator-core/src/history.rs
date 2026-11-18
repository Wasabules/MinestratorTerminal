//! Historique croisé des métriques : échantillons **locaux** (superviseur de l'app ou de son
//! service, un toutes les 10 s, 14 jours) et statistiques **de l'hébergeur**
//! (`GET /server/{id}/stats/{début}/{fin}`, non documenté, une mesure par minute, 365 jours).
//!
//! ## Format de l'hébergeur (relevé le 2026-11-18 avec une clé API, lectures seules)
//!
//! - Réponse : `{ stats: [{ cpu, ram, disk, players, date }], retention_days: 365,
//!   data_available_from, date_start_clamped }`.
//! - Bornes : `AAAA-MM-JJTHH:MM:SS` **uniquement** (date seule, suffixe `Z` ou horodatage Unix :
//!   404 ; fin avant début : `API_INCORRECT_DATE_FORMAT`). Profondeur : 365 jours (une borne plus
//!   ancienne est ramenée).
//! - **Fuseau** : heure légale de Paris, changements d'heure compris, pour les bornes comme pour
//!   les dates renvoyées. La grille saute l'heure du passage à l'heure d'été et répète celle du
//!   retour à l'heure d'hiver, avec les mêmes valeurs (les mesures sont rangées par libellé).
//! - **Granularité** : une plage de moins de 7 jours renvoie une grille **à la minute**
//!   (`AAAA-MM-JJ HH:MM:SS`, entiers), **complétée de zéros** là où rien n'a été mesuré (serveur
//!   éteint, minute pas encore écrite, bord d'une plage qui ne commence pas à `:00`). À partir de
//!   7 jours : des **moyennes horaires** (`AAAA-MM-JJ HH:MM`, flottants) des seules minutes
//!   mesurées, lentes à obtenir (3 à 10 s pour 30 à 90 jours, 504 au-delà). L'app ne demande donc
//!   que des minutes, par tranches d'au plus [`FETCH_SPAN_MAX_S`].
//! - **Unités** : `cpu` en % d'un cœur (100 = un cœur, comme le `cpu_absolute` de Wings), arrondi
//!   à l'entier ; `ram` et `disk` en **Mo décimaux** (10⁶ octets — `/live` les donne en Mio) ;
//!   `players` : joueurs connectés.
//! - Serveur éteint : des zéros, indiscernables d'une minute non mesurée.
//!
//! ## Fusion ([`build_history`])
//!
//! - Le **local fait foi** là où il existe : il est plus fin (10 s), temps réel et porte l'état
//!   Wings. Un échantillon local couvre jusqu'à [`LOCAL_REACH_S`] après lui, et deux échantillons
//!   distants d'au plus [`LOCAL_GAP_S`] forment une même période couverte.
//! - Une minute de l'hébergeur n'est retenue que si elle est **mesurée** (une valeur non nulle) et
//!   ne chevauche aucune période couverte localement : elle comble les trous (app fermée,
//!   ordinateur éteint) et prolonge l'historique au-delà de la rétention locale.
//! - Unités ramenées à celles du local (octets, % d'un cœur) ; la limite de RAM d'un point de
//!   l'hébergeur est celle du dernier échantillon local, sinon celle de `/live/light`.
//! - Chaque point porte son origine ([`HistorySource`]) ; au-delà de `max_points`, regroupement
//!   par tranches de temps en gardant le **maximum** (un pic ne disparaît pas dans une moyenne).
//! - Les joueurs connectés ne viennent que de l'hébergeur (toutes ses minutes mesurées).
//!
//! ## Cache ([`HostStatsCache`])
//!
//! Par serveur, en blocs de [`BLOCK_S`] alignés sur l'heure UTC, en mémoire (durée de vie du
//! `Core`). Un bloc lu plus de [`SETTLE_S`] après sa fin est **définitif** et n'est jamais relu ;
//! le bloc en cours est relu au plus toutes les [`OPEN_TTL_S`]. Les blocs manquants contigus
//! partent en une seule requête. Après une erreur, plus d'appel pendant [`ERROR_BACKOFF_S`].
//!
//! ## Horloge
//!
//! Les échantillons locaux sont datés par l'horloge de la machine ; celles de l'hébergeur par la
//! sienne. Si elles divergent de plus de [`SKEW_THRESHOLD_S`] (mesuré par l'en-tête `Date` des
//! réponses), les points de l'hébergeur sont ramenés dans le repère de l'horloge locale : sans
//! cela, une machine à l'heure fausse décalerait les deux séries l'une par rapport à l'autre.

use crate::error::{Error, Result};
use crate::models::{lenient_f64, lenient_i64};
use crate::store::{now_secs, MetricSample};
use crate::uptime::{self, UptimeSpan};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Fenêtre maximale d'un historique demandé par l'app (30 jours, plus une marge).
pub const MAX_RANGE_S: i64 = 31 * 86_400;
/// Taille d'un bloc du cache (alignés sur l'heure UTC).
const BLOCK_S: i64 = 6 * 3600;
/// Plage maximale d'une requête : l'API passe aux moyennes horaires à partir de 7 jours.
const FETCH_SPAN_MAX_S: i64 = 3 * 86_400;
/// Un bloc lu au moins aussi longtemps après sa fin est définitif (minutes toutes écrites).
const SETTLE_S: i64 = 10 * 60;
/// Le bloc en cours n'est pas relu plus souvent.
const OPEN_TTL_S: i64 = 60;
/// Pause des appels pour un serveur après une erreur.
const ERROR_BACKOFF_S: i64 = 60;
/// Blocs gardés au plus (tous serveurs confondus : 180 jours de minutes).
const MAX_BLOCKS: usize = 720;
/// Rétention de l'hébergeur.
const HOST_RETENTION_S: i64 = 365 * 86_400;
/// Écart d'horloge en deçà duquel on ne corrige rien (latence, arrondi de l'en-tête `Date`).
const SKEW_THRESHOLD_S: i64 = 90;
/// Écart maximal entre deux échantillons locaux d'une même période couverte.
const LOCAL_GAP_S: i64 = 60;
/// Portée d'un échantillon local (après lui).
const LOCAL_REACH_S: i64 = 30;
/// Pas de mesure de l'hébergeur (minute) et du superviseur local.
const HOST_STEP_S: i64 = 60;
const LOCAL_STEP_S: i64 = 10;
/// Trou de surveillance trop court pour justifier une lecture chez l'hébergeur.
const MIN_UPTIME_GAP_S: i64 = 120;
/// Durée de validité de la limite de RAM lue sur `/live/light`.
const MEM_LIMIT_TTL_S: i64 = 600;

// --- Réponse brute de l'API -------------------------------------------------------------

/// `data` de `GET /server/{id}/stats/{début}/{fin}` : `{ stats: [...] }` (le panel accepte aussi
/// un tableau nu, d'où la tolérance).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum HostStatsData {
    Wrapped {
        #[serde(default)]
        stats: Vec<RawHostStat>,
    },
    Bare(Vec<RawHostStat>),
}

impl HostStatsData {
    fn stats(&self) -> &[RawHostStat] {
        match self {
            Self::Wrapped { stats } | Self::Bare(stats) => stats,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawHostStat {
    #[serde(default, deserialize_with = "lenient_f64")]
    cpu: f64,
    #[serde(default, deserialize_with = "lenient_f64")]
    ram: f64,
    #[serde(default, deserialize_with = "lenient_f64")]
    disk: f64,
    #[serde(default, deserialize_with = "lenient_i64")]
    players: i64,
    #[serde(default)]
    date: String,
}

/// Une mesure de l'hébergeur, convertie (horodatage Unix du début de la minute).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HostPoint {
    pub ts: i64,
    /// Durée couverte : 60 (minute) ou 3600 (moyenne horaire, si l'API en renvoyait).
    pub step: i64,
    /// % d'un cœur.
    pub cpu: f64,
    /// Mo décimaux (10⁶ octets).
    pub ram_mb: f64,
    pub disk_mb: f64,
    pub players: i64,
}

impl HostPoint {
    /// Une valeur non nulle : la minute a été **mesurée** (serveur en marche). Une minute à zéro
    /// est soit un serveur éteint, soit une minute sans mesure : l'API ne les distingue pas.
    fn measured(&self) -> bool {
        self.ram_mb > 0.0 || self.cpu > 0.0 || self.disk_mb > 0.0
    }
}

// --- Heure de Paris -----------------------------------------------------------------------

/// Dernier dimanche du mois `month` (mars ou octobre), à 01:00 UTC : instant des changements
/// d'heure de l'Union européenne.
fn eu_switch(year: i32, month: u32) -> i64 {
    let last = NaiveDate::from_ymd_opt(year, month + 1, 1)
        .and_then(|d| d.pred_opt())
        .expect("mois valide");
    let sunday = last - chrono::Days::new(u64::from(last.weekday().num_days_from_sunday()));
    sunday.and_hms_opt(1, 0, 0).expect("heure valide").and_utc().timestamp()
}

/// Décalage de l'heure de Paris sur UTC à l'instant `ts` : 2 h en heure d'été, 1 h sinon.
pub(crate) fn paris_offset(ts: i64) -> i64 {
    let year = DateTime::from_timestamp(ts, 0).map_or(1970, |d| d.year());
    if ts >= eu_switch(year, 3) && ts < eu_switch(year, 10) {
        7200
    } else {
        3600
    }
}

/// Borne de requête : `ts` en heure de Paris, au format exigé par l'API.
pub(crate) fn paris_label(ts: i64) -> String {
    DateTime::from_timestamp(ts + paris_offset(ts), 0)
        .map(|d| d.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Date renvoyée par l'API → (date naïve, durée couverte) : `AAAA-MM-JJ HH:MM:SS` pour une
/// minute, `AAAA-MM-JJ HH:MM` pour une moyenne horaire.
fn parse_label(s: &str) -> Option<(NaiveDateTime, i64)> {
    let s = s.trim().replace('T', " ");
    if let Ok(d) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S") {
        return Some((d, HOST_STEP_S));
    }
    NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M").ok().map(|d| (d, 3600))
}

/// Instants possibles d'une heure de Paris : (heure d'été, heure d'hiver). Les deux existent
/// pendant l'heure répétée d'octobre, aucun pendant l'heure sautée de mars.
fn paris_instants(naive: NaiveDateTime) -> (Option<i64>, Option<i64>) {
    let base = naive.and_utc().timestamp();
    let summer = base - 7200;
    let winter = base - 3600;
    (
        (paris_offset(summer) == 7200).then_some(summer),
        (paris_offset(winter) == 3600).then_some(winter),
    )
}

/// En-tête HTTP `Date` (`Wed, 18 Nov 2026 16:26:59 GMT`) → secondes Unix.
pub(crate) fn parse_http_date(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc2822(s.trim()).ok().map(|d| d.timestamp())
}

/// Mesures d'une réponse comprises dans `[start, end)` (secondes Unix de l'hébergeur), triées.
/// Les minutes à zéro sont écartées ; la 2e occurrence d'un libellé de l'heure répétée d'octobre
/// est rapportée à l'heure d'hiver.
fn parse_points(raw: &[RawHostStat], start: i64, end: i64) -> Vec<HostPoint> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let Some((naive, step)) = parse_label(&r.date) else {
            continue;
        };
        let (summer, winter) = paris_instants(naive);
        let ts = if seen.insert(naive) { summer.or(winter) } else { winter.or(summer) };
        let Some(ts) = ts.filter(|ts| (start..end).contains(ts)) else {
            continue;
        };
        let p = HostPoint {
            ts,
            step,
            cpu: r.cpu.max(0.0),
            ram_mb: r.ram.max(0.0),
            disk_mb: r.disk.max(0.0),
            players: r.players.max(0),
        };
        if p.measured() {
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.ts);
    out.dedup_by_key(|p| p.ts);
    out
}

// --- Types IPC ----------------------------------------------------------------------------

/// Origine d'un point d'historique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum HistorySource {
    /// Surveillance de l'app ou de son service d'arrière-plan (un point toutes les 10 s).
    Local,
    /// Statistiques de l'hébergeur (une mesure par minute).
    Host,
}

/// Un point de l'historique fusionné (unités du local).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HistoryPoint {
    pub ts: i64,
    /// CPU en % d'un cœur (100 = un cœur).
    pub cpu: f64,
    /// RAM, en octets.
    pub mem: i64,
    /// Limite de RAM, en octets (0 si inconnue).
    pub mem_limit: i64,
    /// Disque, en octets.
    pub disk: i64,
    /// État Wings du point local ; `running` pour une mesure de l'hébergeur.
    pub state: String,
    pub source: HistorySource,
}

/// Joueurs connectés (statistiques de l'hébergeur).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlayersPoint {
    pub ts: i64,
    pub count: i64,
}

/// Historique fusionné d'un serveur sur `[from, to]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetricsHistory {
    pub server_id: i64,
    pub from: i64,
    pub to: i64,
    /// Pas des points (s) : la tranche de regroupement, sinon le pas de mesure le plus large
    /// présent. Un écart nettement supérieur entre deux points est un trou de données.
    pub step_s: i64,
    /// Points chronologiques, origine comprise.
    pub points: Vec<HistoryPoint>,
    /// Joueurs connectés (hébergeur seulement), même regroupement (maximum par tranche).
    pub players: Vec<PlayersPoint>,
    /// Temps couvert par la surveillance locale dans la fenêtre (s).
    pub local_s: i64,
    /// Temps comblé par les mesures de l'hébergeur (s).
    pub host_s: i64,
    /// Les statistiques de l'hébergeur ont pu être lues (sinon : local seul, ou cache partiel).
    pub host_ok: bool,
}

// --- Fusion --------------------------------------------------------------------------------

/// Entrées de [`build_history`].
pub(crate) struct HistoryInput<'a> {
    pub server_id: i64,
    pub from: i64,
    pub to: i64,
    /// Échantillons locaux (app + service), triés.
    pub local: &'a [MetricSample],
    /// Mesures de l'hébergeur (horloge locale), triées.
    pub host: &'a [HostPoint],
    /// Limite de RAM (octets) à défaut d'échantillon local.
    pub mem_limit_fallback: i64,
    pub host_ok: bool,
}

/// Périodes couvertes par la surveillance locale (`[début, fin)`, triées), d'après les dates
/// (triées) de ses échantillons.
fn local_coverage(times: impl IntoIterator<Item = i64>) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for ts in times {
        match out.last_mut() {
            // `last.1 - LOCAL_REACH_S` : date du dernier échantillon de la période.
            Some(last) if ts - (last.1 - LOCAL_REACH_S) <= LOCAL_GAP_S => {
                last.1 = last.1.max(ts + LOCAL_REACH_S);
            }
            _ => out.push((ts, ts + LOCAL_REACH_S)),
        }
    }
    out
}

/// L'intervalle `[a, b)` chevauche-t-il une période de `cover` (triée, disjointe) ?
fn overlaps(cover: &[(i64, i64)], a: i64, b: i64) -> bool {
    let i = cover.partition_point(|&(_, end)| end <= a);
    cover.get(i).is_some_and(|&(start, _)| start < b)
}

fn clipped(a: i64, b: i64, from: i64, to: i64) -> i64 {
    (b.min(to) - a.max(from)).max(0)
}

/// Fusionne local et hébergeur (règles en tête du module), à pleine résolution.
pub(crate) fn build_history(input: HistoryInput) -> MetricsHistory {
    let HistoryInput { server_id, from, to, local, host, mem_limit_fallback, host_ok } = input;
    let local: Vec<&MetricSample> = local.iter().filter(|s| s.ts >= from && s.ts <= to).collect();
    let cover = local_coverage(local.iter().map(|s| s.ts));
    let local_s = cover.iter().map(|&(a, b)| clipped(a, b, from, to)).sum();
    let mem_limit = local
        .iter()
        .rev()
        .find(|s| s.mem_limit > 0)
        .map_or(mem_limit_fallback, |s| s.mem_limit);

    let in_range: Vec<&HostPoint> =
        host.iter().filter(|p| p.measured() && p.ts + p.step > from && p.ts <= to).collect();
    let mut host_s = 0;
    let mut points: Vec<HistoryPoint> = local
        .iter()
        .map(|s| HistoryPoint {
            ts: s.ts,
            cpu: s.cpu,
            mem: s.mem,
            mem_limit: s.mem_limit,
            disk: s.disk,
            state: s.state.clone(),
            source: HistorySource::Local,
        })
        .collect();
    for p in &in_range {
        if overlaps(&cover, p.ts, p.ts + p.step) {
            continue;
        }
        host_s += clipped(p.ts, p.ts + p.step, from, to);
        points.push(HistoryPoint {
            ts: p.ts.max(from),
            cpu: p.cpu,
            mem: (p.ram_mb * 1e6).round() as i64,
            mem_limit,
            disk: (p.disk_mb * 1e6).round() as i64,
            state: "running".into(),
            source: HistorySource::Host,
        });
    }
    // Tri stable : à date égale, le local d'abord.
    points.sort_by_key(|p| p.ts);
    let players: Vec<PlayersPoint> =
        in_range.iter().map(|p| PlayersPoint { ts: p.ts.max(from), count: p.players }).collect();
    let step_s = if points.iter().any(|p| p.source == HistorySource::Host) {
        in_range.iter().map(|p| p.step).max().unwrap_or(HOST_STEP_S)
    } else {
        LOCAL_STEP_S
    };
    MetricsHistory { server_id, from, to, step_s, points, players, local_s, host_s, host_ok }
}

/// Ramène l'historique à `max_points` points au plus (et autant de points de joueurs) par
/// tranches de temps égales ; `step_s` devient la largeur d'une tranche.
pub(crate) fn limit_points(mut h: MetricsHistory, max_points: usize) -> MetricsHistory {
    let max_points = max_points.max(10);
    if h.points.len() > max_points || h.players.len() > max_points {
        let bucket = ((h.to - h.from) as f64 / max_points as f64).ceil().max(1.0) as i64;
        h.step_s = bucket.max(h.step_s);
        h.points = downsample_points(std::mem::take(&mut h.points), h.from, h.step_s);
        h.players = downsample_players(std::mem::take(&mut h.players), h.from, h.step_s);
    }
    h
}

/// Regroupement par tranches de `bucket` secondes depuis `from` : maximum du CPU et de la RAM,
/// dernière valeur du disque et de l'état ; local si la tranche contient un point local.
fn downsample_points(points: Vec<HistoryPoint>, from: i64, bucket: i64) -> Vec<HistoryPoint> {
    let mut out: Vec<(i64, HistoryPoint)> = Vec::new();
    for p in points {
        let key = (p.ts - from).div_euclid(bucket);
        match out.last_mut() {
            Some((k, acc)) if *k == key => {
                acc.ts = acc.ts.max(p.ts);
                acc.cpu = acc.cpu.max(p.cpu);
                acc.mem = acc.mem.max(p.mem);
                acc.mem_limit = acc.mem_limit.max(p.mem_limit);
                acc.disk = p.disk;
                acc.state = p.state;
                if p.source == HistorySource::Local {
                    acc.source = HistorySource::Local;
                }
            }
            _ => out.push((key, p)),
        }
    }
    out.into_iter().map(|(_, p)| p).collect()
}

fn downsample_players(points: Vec<PlayersPoint>, from: i64, bucket: i64) -> Vec<PlayersPoint> {
    let mut out: Vec<(i64, PlayersPoint)> = Vec::new();
    for p in points {
        let key = (p.ts - from).div_euclid(bucket);
        match out.last_mut() {
            Some((k, acc)) if *k == key => {
                acc.ts = acc.ts.max(p.ts);
                acc.count = acc.count.max(p.count);
            }
            _ => out.push((key, p)),
        }
    }
    out.into_iter().map(|(_, p)| p).collect()
}

// --- Cache des statistiques de l'hébergeur -------------------------------------------------

struct Block {
    /// Lu à (horloge de l'hébergeur).
    fetched_at: i64,
    /// Lu après la fin du bloc (+ [`SETTLE_S`]) : ne changera plus.
    settled: bool,
    /// Minutes mesurées, horodatées par l'hébergeur.
    points: Vec<HostPoint>,
    used: u64,
}

#[derive(Default)]
struct CacheInner {
    blocks: HashMap<(i64, i64), Block>,
    tick: u64,
    /// Horloge de l'hébergeur − horloge locale (s), 0 sous [`SKEW_THRESHOLD_S`].
    skew: i64,
    /// Dernière erreur de lecture par serveur (horloge locale).
    errors: HashMap<i64, i64>,
    /// Limite de RAM (octets) par serveur, lue à (horloge locale).
    mem_limits: HashMap<i64, (i64, i64)>,
}

/// Cache des statistiques de l'hébergeur (voir l'en-tête du module).
#[derive(Default)]
pub(crate) struct HostStatsCache {
    inner: Mutex<CacheInner>,
    /// Une seule série de requêtes à la fois : deux vues qui demandent la même plage ne la
    /// téléchargent qu'une fois.
    fetch: tokio::sync::Mutex<()>,
}

fn block_of(ts: i64) -> i64 {
    ts.div_euclid(BLOCK_S) * BLOCK_S
}

/// Requêtes `[début, fin)` couvrant des blocs (triés) : les blocs contigus sont regroupés, par
/// tranches d'au plus [`FETCH_SPAN_MAX_S`] ; la dernière s'arrête à `now`.
fn coalesce(blocks: &[i64], now: i64) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &b in blocks {
        match out.last_mut() {
            Some(last) if last.1 == b && b + BLOCK_S - last.0 <= FETCH_SPAN_MAX_S => last.1 = b + BLOCK_S,
            _ => out.push((b, b + BLOCK_S)),
        }
    }
    for r in &mut out {
        r.1 = r.1.min(now);
    }
    out.retain(|r| r.1 > r.0);
    out
}

impl HostStatsCache {
    fn skew(&self) -> i64 {
        self.inner.lock().unwrap().skew
    }

    /// Mesure l'écart d'horloge à partir de l'heure du serveur (en-tête `Date`).
    fn observe_clock(&self, server_now: Option<i64>, local_now: i64) {
        if let Some(server_now) = server_now {
            let d = server_now - local_now;
            self.inner.lock().unwrap().skew = if d.abs() >= SKEW_THRESHOLD_S { d } else { 0 };
        }
    }

    /// Blocs à (re)lire pour couvrir `ranges` (horloge de l'hébergeur), triés.
    fn plan(&self, server_id: i64, ranges: &[(i64, i64)], now: i64) -> Vec<i64> {
        let inner = self.inner.lock().unwrap();
        let oldest = now - HOST_RETENTION_S + BLOCK_S;
        let mut out = Vec::new();
        for &(a, b) in ranges {
            let (a, b) = (a.max(oldest), b.min(now));
            if b <= a {
                continue;
            }
            let mut block = block_of(a);
            while block < b {
                let stale = match inner.blocks.get(&(server_id, block)) {
                    None => true,
                    Some(bl) => !bl.settled && now - bl.fetched_at >= OPEN_TTL_S,
                };
                if stale {
                    out.push(block);
                }
                block += BLOCK_S;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Range la réponse d'une requête `[start, end)` dans ses blocs.
    fn store(&self, server_id: i64, start: i64, end: i64, points: Vec<HostPoint>, now: i64) {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        let mut block = block_of(start);
        while block < end {
            let pts = points.iter().filter(|p| block_of(p.ts) == block).copied().collect();
            let settled = end >= block + BLOCK_S && now >= block + BLOCK_S + SETTLE_S;
            inner
                .blocks
                .insert((server_id, block), Block { fetched_at: now, settled, points: pts, used: tick });
            block += BLOCK_S;
        }
        // Éviction des blocs les moins récemment servis.
        let excess = inner.blocks.len().saturating_sub(MAX_BLOCKS);
        if excess > 0 {
            let mut by_use: Vec<((i64, i64), u64)> = inner.blocks.iter().map(|(k, b)| (*k, b.used)).collect();
            by_use.sort_by_key(|(_, used)| *used);
            for (key, _) in by_use.into_iter().take(excess) {
                inner.blocks.remove(&key);
            }
        }
    }

    /// Mesures en cache comprises dans `ranges` (horloge de l'hébergeur), triées.
    fn read(&self, server_id: i64, ranges: &[(i64, i64)]) -> Vec<HostPoint> {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        for &(a, b) in ranges {
            let mut block = block_of(a);
            while block < b {
                if visited.insert(block) {
                    if let Some(bl) = inner.blocks.get_mut(&(server_id, block)) {
                        bl.used = tick;
                        out.extend(bl.points.iter().copied());
                    }
                }
                block += BLOCK_S;
            }
        }
        out.retain(|p| ranges.iter().any(|&(a, b)| p.ts + p.step > a && p.ts < b));
        out.sort_by_key(|p| p.ts);
        out.dedup_by_key(|p| p.ts);
        out
    }

    fn in_backoff(&self, server_id: i64, now: i64) -> bool {
        self.inner.lock().unwrap().errors.get(&server_id).is_some_and(|t| now - t < ERROR_BACKOFF_S)
    }

    fn note_error(&self, server_id: i64, now: i64) {
        self.inner.lock().unwrap().errors.insert(server_id, now);
    }

    fn clear_error(&self, server_id: i64) {
        self.inner.lock().unwrap().errors.remove(&server_id);
    }
}

// --- Façade ---------------------------------------------------------------------------------

impl crate::Core {
    /// Historique d'un serveur sur les `since_secs` dernières secondes (30 jours au plus) :
    /// surveillance locale (app et service d'arrière-plan) complétée par les statistiques de
    /// l'hébergeur, au plus `max_points` points (600 par défaut). Une erreur côté hébergeur ne
    /// fait pas échouer l'appel : `host_ok` passe à faux et le local reste servi.
    pub async fn server_history(
        &self,
        server_id: i64,
        since_secs: i64,
        max_points: Option<u32>,
    ) -> Result<MetricsHistory> {
        let max_points = max_points.unwrap_or(600).clamp(10, 5000) as usize;
        Ok(limit_points(self.merged_history(server_id, since_secs).await?, max_points))
    }

    /// [`Self::server_history`] à pleine résolution (résumés de l'outil MCP `server_metrics`).
    pub(crate) async fn merged_history(&self, server_id: i64, since_secs: i64) -> Result<MetricsHistory> {
        let since = since_secs.clamp(60, MAX_RANGE_S);
        let to = now_secs();
        let from = to - since;
        let local = self.metrics_history(server_id, since).await?;
        let (host, error) = self.host_points(server_id, &[(from, to)]).await;
        if let Some(e) = &error {
            tracing::debug!("statistiques de l'hébergeur indisponibles ({server_id}) : {e}");
        }
        let fallback = if local.iter().any(|s| s.mem_limit > 0) || host.is_empty() {
            0
        } else {
            self.host_mem_limit(server_id).await
        };
        Ok(build_history(HistoryInput {
            server_id,
            from,
            to,
            local: &local,
            host: &host,
            mem_limit_fallback: fallback,
            host_ok: error.is_none(),
        }))
    }

    /// Mesures de l'hébergeur couvrant `ranges` (secondes Unix de l'horloge locale), servies par
    /// le cache et complétées au besoin. Renvoie aussi la dernière erreur rencontrée (les mesures
    /// déjà en cache restent servies).
    pub(crate) async fn host_points(&self, server_id: i64, ranges: &[(i64, i64)]) -> (Vec<HostPoint>, Option<Error>) {
        let cache = &self.host_stats;
        let token = match self.token() {
            Ok(t) => t,
            Err(e) => return (Vec::new(), Some(e)),
        };
        let mut error = None;
        // Une requête par tour, verrou repris à chaque fois : deux appelants (graphe et carte de
        // disponibilité) s'intercalent au lieu que l'un attende tout le rattrapage de l'autre, et
        // le plan, refait à chaque tour, ne relit pas ce que l'autre vient de ranger. L'écart
        // d'horloge révélé par une réponse est pris en compte dès le tour suivant.
        for _ in 0..64 {
            let _one_at_a_time = cache.fetch.lock().await;
            let skew = cache.skew();
            let now = now_secs() + skew;
            let wanted: Vec<(i64, i64)> = ranges.iter().map(|&(a, b)| (a + skew, (b + skew).min(now))).collect();
            let todo = cache.plan(server_id, &wanted, now);
            let Some(&(start, end)) = coalesce(&todo, now).first() else {
                break;
            };
            if cache.in_backoff(server_id, now_secs()) {
                error = Some(Error::Unexpected("statistiques de l'hébergeur en pause après une erreur".into()));
                break;
            }
            let res = self
                .api
                .get_server_stats(&token, server_id, &paris_label(start), &paris_label(end - 1))
                .await;
            match res {
                Ok((data, server_now)) => {
                    cache.observe_clock(server_now, now_secs());
                    cache.store(server_id, start, end, parse_points(data.stats(), start, end), now);
                    cache.clear_error(server_id);
                }
                Err(e) => {
                    cache.note_error(server_id, now_secs());
                    error = Some(e);
                    break;
                }
            }
        }
        let skew = cache.skew();
        let wanted: Vec<(i64, i64)> = ranges.iter().map(|&(a, b)| (a + skew, b + skew)).collect();
        let points = cache
            .read(server_id, &wanted)
            .into_iter()
            .map(|p| HostPoint { ts: p.ts - skew, ..p })
            .collect();
        (points, error)
    }

    /// Limite de RAM d'un serveur (octets) d'après `/live/light` (Mio), gardée 10 minutes ; 0 si
    /// elle est inconnue.
    async fn host_mem_limit(&self, server_id: i64) -> i64 {
        let now = now_secs();
        let cached = self.host_stats.inner.lock().unwrap().mem_limits.get(&server_id).copied();
        if let Some((at, bytes)) = cached {
            if now - at < MEM_LIMIT_TTL_S {
                return bytes;
            }
        }
        let bytes = self.live_light(server_id).await.map_or(0, |l| l.memory.limit.max(0) * 1024 * 1024);
        self.host_stats.inner.lock().unwrap().mem_limits.insert(server_id, (now, bytes));
        bytes
    }

    /// Comble les périodes non surveillées de `spans` (triées, sans recouvrement) avec les
    /// minutes mesurées par l'hébergeur (règle : [`crate::uptime`]). Sans données de l'hébergeur,
    /// `spans` est rendu tel quel.
    pub(crate) async fn fill_uptime_with_host(
        &self,
        server_id: i64,
        spans: Vec<UptimeSpan>,
        from: i64,
        to: i64,
    ) -> Vec<UptimeSpan> {
        let gaps: Vec<(i64, i64)> = uptime::unmonitored(&spans, from, to)
            .into_iter()
            .filter(|(a, b)| b - a >= MIN_UPTIME_GAP_S)
            .collect();
        if gaps.is_empty() {
            return spans;
        }
        let (points, error) = self.host_points(server_id, &gaps).await;
        if let Some(e) = error {
            tracing::debug!("disponibilité sans l'hébergeur ({server_id}) : {e}");
        }
        let minutes: Vec<i64> = points
            .iter()
            .filter(|p| p.step == HOST_STEP_S && p.measured())
            .map(|p| p.ts)
            .collect();
        uptime::fill_unmonitored(spans, uptime::host_spans(&minutes, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(date: &str, cpu: f64, ram: f64, disk: f64, players: i64) -> RawHostStat {
        RawHostStat { cpu, ram, disk, players, date: date.into() }
    }

    fn ts(s: &str) -> i64 {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap().and_utc().timestamp()
    }

    fn local(t: i64, cpu: f64) -> MetricSample {
        MetricSample { ts: t, cpu, mem: 2_000_000_000, mem_limit: 4_000_000_000, disk: 5_000_000_000, state: "running".into() }
    }

    fn host(t: i64, cpu: f64, ram_mb: f64) -> HostPoint {
        HostPoint { ts: t, step: 60, cpu, ram_mb, disk_mb: 3504.0, players: 2 }
    }

    fn input<'a>(from: i64, to: i64, local: &'a [MetricSample], host: &'a [HostPoint]) -> HistoryInput<'a> {
        HistoryInput { server_id: 1, from, to, local, host, mem_limit_fallback: 0, host_ok: true }
    }

    // --- Heure de Paris et format de l'API ---

    #[test]
    fn heure_de_paris_hiver_ete_et_bascules() {
        // Relevé réel : 16:20:46 UTC le 18 novembre = 17:20:46 à Paris (heure d'hiver).
        assert_eq!(paris_label(ts("2026-11-18 16:20:46")), "2026-11-18T17:20:46");
        assert_eq!(paris_label(ts("2026-07-01 12:00:00")), "2026-07-01T14:00:00");
        // Changements d'heure 2026 : 29 mars et 25 octobre à 01:00 UTC.
        assert_eq!(eu_switch(2026, 3), ts("2026-03-29 01:00:00"));
        assert_eq!(eu_switch(2026, 10), ts("2026-10-25 01:00:00"));
        assert_eq!(paris_offset(ts("2026-03-29 00:59:59")), 3600);
        assert_eq!(paris_offset(ts("2026-03-29 01:00:00")), 7200);
        assert_eq!(paris_offset(ts("2026-10-25 00:59:59")), 7200);
        assert_eq!(paris_offset(ts("2026-10-25 01:00:00")), 3600);
    }

    #[test]
    fn libelles_ambigus_et_inexistants() {
        let d = |s: &str| parse_label(s).unwrap().0;
        // Heure répétée d'octobre : deux instants possibles, l'heure d'été d'abord.
        let (a, b) = paris_instants(d("2026-10-25 02:30:00"));
        assert_eq!(a, Some(ts("2026-10-25 00:30:00")));
        assert_eq!(b, Some(ts("2026-10-25 01:30:00")));
        // Heure sautée de mars : aucun.
        assert_eq!(paris_instants(d("2026-03-29 02:30:00")), (None, None));
        // Heure ordinaire : un seul.
        assert_eq!(paris_instants(d("2026-11-18 17:00:00")), (None, Some(ts("2026-11-18 16:00:00"))));
    }

    #[test]
    fn formats_de_date_minute_et_heure() {
        assert_eq!(parse_label("2026-11-18 16:21:00").unwrap().1, 60);
        assert_eq!(parse_label("2026-11-18T16:21:00").unwrap().1, 60);
        assert_eq!(parse_label("2026-11-18 16:00").unwrap().1, 3600);
        assert!(parse_label("hier").is_none());
        assert_eq!(parse_http_date("Wed, 18 Nov 2026 16:26:59 GMT"), Some(ts("2026-11-18 16:26:59")));
        assert_eq!(parse_http_date("n'importe quoi"), None);
    }

    #[test]
    fn reponse_reelle_minutes_zeros_ecartes() {
        // Extrait conforme au relevé : bord de plage à zéro, puis minutes mesurées.
        let json = serde_json::json!({
            "stats": [
                { "cpu": 0, "date": "2026-11-18 16:00:00", "disk": 0, "players": 0, "ram": 0 },
                { "cpu": 3, "date": "2026-11-18 16:01:00", "disk": 3504, "players": 1, "ram": 3028 },
                { "cpu": "4", "date": "2026-11-18 16:02:00", "disk": 3504, "players": 1, "ram": 3029.5 }
            ],
            "retention_days": 365, "data_available_from": "2025-11-18 17:20:46", "date_start_clamped": false
        });
        let data: HostStatsData = serde_json::from_value(json).unwrap();
        let pts = parse_points(data.stats(), 0, i64::MAX);
        assert_eq!(pts.len(), 2, "la minute à zéro n'est pas une mesure");
        assert_eq!(pts[0].ts, ts("2026-11-18 15:01:00"));
        assert_eq!((pts[0].cpu, pts[0].ram_mb, pts[0].players), (3.0, 3028.0, 1));
        assert_eq!(pts[1].cpu, 4.0, "nombre en chaîne toléré");
        // Tableau nu (forme acceptée par le panel).
        let bare: HostStatsData =
            serde_json::from_value(serde_json::json!([{ "cpu": 1, "ram": 1, "disk": 1, "players": 0, "date": "2026-11-18 16:01:00" }])).unwrap();
        assert_eq!(parse_points(bare.stats(), 0, i64::MAX).len(), 1);
        // Bornes de la requête respectées.
        assert!(parse_points(data.stats(), 0, ts("2026-11-18 15:01:00")).is_empty());
    }

    #[test]
    fn heure_repetee_d_octobre_rapportee_aux_deux_instants() {
        // La grille répète 02:00 à 02:59 avec les mêmes valeurs (relevé réel).
        let rows = vec![
            raw("2026-10-25 01:59:00", 4.0, 2986.0, 3504.0, 0),
            raw("2026-10-25 02:00:00", 4.0, 2986.0, 3504.0, 0),
            raw("2026-10-25 02:00:00", 4.0, 2986.0, 3504.0, 0),
            raw("2026-10-25 03:00:00", 3.0, 2995.0, 3504.0, 0),
        ];
        let pts: Vec<i64> = parse_points(&rows, 0, i64::MAX).iter().map(|p| p.ts).collect();
        assert_eq!(
            pts,
            vec![ts("2026-10-24 23:59:00"), ts("2026-10-25 00:00:00"), ts("2026-10-25 01:00:00"), ts("2026-10-25 02:00:00")]
        );
    }

    // --- Fusion ---

    #[test]
    fn le_local_fait_foi_et_l_hebergeur_comble_les_trous() {
        // Local de 10:00 à 10:30 (toutes les 10 s) ; hébergeur toutes les minutes de 9:00 à 11:00.
        let t0 = 36_000;
        let loc: Vec<MetricSample> = (0..=180).map(|i| local(t0 + i * 10, 50.0)).collect();
        let hst: Vec<HostPoint> = (-60..=60).map(|i| host(t0 + i * 60, 7.0, 3000.0)).collect();
        let h = build_history(input(t0 - 3600, t0 + 3600, &loc, &hst));
        let host_pts: Vec<&HistoryPoint> = h.points.iter().filter(|p| p.source == HistorySource::Host).collect();
        // Aucun point de l'hébergeur dans la période couverte localement (10:00 → 10:30:30).
        assert!(host_pts.iter().all(|p| p.ts + 60 <= t0 || p.ts >= t0 + 1800 + LOCAL_REACH_S));
        assert_eq!(h.points.iter().filter(|p| p.source == HistorySource::Local).count(), 181);
        // Raccord sans trou : la dernière minute avant le local, la première après.
        assert!(host_pts.iter().any(|p| p.ts == t0 - 60));
        assert!(host_pts.iter().any(|p| p.ts == t0 + 1860));
        assert_eq!(h.local_s, 1800 + LOCAL_REACH_S);
        assert_eq!(h.host_s, (60 + 29) * 60, "60 min avant, 29 min après");
        // Chronologique.
        assert!(h.points.windows(2).all(|w| w[0].ts <= w[1].ts));
        assert_eq!(h.step_s, 60);
    }

    #[test]
    fn unites_normalisees_et_limite_de_ram() {
        let t0 = 100_000;
        let loc = vec![local(t0, 10.0)];
        let hst = vec![host(t0 - 600, 250.0, 3028.0)];
        let h = build_history(input(t0 - 3600, t0 + 60, &loc, &hst));
        let p = h.points.iter().find(|p| p.source == HistorySource::Host).unwrap();
        assert_eq!(p.cpu, 250.0, "même unité : % d'un cœur");
        assert_eq!(p.mem, 3_028_000_000, "Mo décimaux → octets");
        assert_eq!(p.disk, 3_504_000_000);
        assert_eq!(p.mem_limit, 4_000_000_000, "limite du dernier échantillon local");
        assert_eq!(p.state, "running");
        // Sans local : la limite de repli.
        let mut inp = input(t0 - 3600, t0 + 60, &[], &hst);
        inp.mem_limit_fallback = 6_000 * 1024 * 1024;
        let h = build_history(inp);
        assert_eq!(h.points[0].mem_limit, 6_000 * 1024 * 1024);
        assert_eq!(h.local_s, 0);
    }

    #[test]
    fn bornes_de_la_fenetre() {
        let hst: Vec<HostPoint> = (0..10).map(|i| host(1_000 + i * 60, 5.0, 100.0)).collect();
        let loc = vec![local(900, 1.0), local(2_000, 1.0)];
        // Fenêtre [1 100, 1 400] : seuls les points qui la touchent.
        let h = build_history(input(1_100, 1_400, &loc, &hst));
        assert!(h.points.iter().all(|p| p.ts >= 1_100 && p.ts <= 1_400));
        assert!(h.points.iter().all(|p| p.source == HistorySource::Host));
        // La minute commencée avant la fenêtre est ramenée à son début.
        assert_eq!(h.points[0].ts, 1_100);
        assert_eq!(h.host_s, 300);
        assert_eq!(h.players.len(), h.points.len());
    }

    #[test]
    fn joueurs_de_l_hebergeur_meme_sous_le_local() {
        let t0 = 50_000;
        let loc: Vec<MetricSample> = (0..=60).map(|i| local(t0 + i * 10, 1.0)).collect();
        let hst: Vec<HostPoint> = (0..10).map(|i| host(t0 + i * 60, 1.0, 100.0)).collect();
        let h = build_history(input(t0, t0 + 600, &loc, &hst));
        assert_eq!(h.players.len(), 10, "les joueurs ne viennent que de l'hébergeur");
        assert!(h.players.iter().all(|p| p.count == 2));
    }

    #[test]
    fn regroupement_garde_les_pics_et_l_origine() {
        let t0 = 0;
        let mut hst: Vec<HostPoint> = (0..1440).map(|i| host(t0 + i * 60, 5.0, 1000.0)).collect();
        hst[700].cpu = 380.0;
        hst[700].players = 9;
        let loc: Vec<MetricSample> = (0..30).map(|i| local(t0 + 43_200 + i * 10, 20.0)).collect();
        let full = build_history(input(t0, t0 + 86_400, &loc, &hst));
        assert_eq!(limit_points(full.clone(), 5_000), full, "sous la limite : inchangé");
        let h = limit_points(full, 96);
        assert!(h.points.len() <= 97, "{}", h.points.len());
        assert_eq!(h.step_s, 900);
        assert!(h.points.iter().any(|p| p.cpu == 380.0), "le pic survit au regroupement");
        assert!(h.players.iter().any(|p| p.count == 9));
        assert!(h.points.iter().any(|p| p.source == HistorySource::Local), "une tranche avec du local reste locale");
        assert!(h.points.windows(2).all(|w| w[0].ts < w[1].ts));
    }

    #[test]
    fn local_seul_et_rien_du_tout() {
        let loc: Vec<MetricSample> = (0..10).map(|i| local(i * 10, 1.0)).collect();
        let h = build_history(input(0, 100, &loc, &[]));
        assert_eq!(h.step_s, LOCAL_STEP_S);
        assert_eq!(h.host_s, 0);
        assert!(h.players.is_empty());
        let h = build_history(input(0, 100, &[], &[]));
        assert!(h.points.is_empty());
        assert_eq!((h.local_s, h.host_s), (0, 0));
    }

    #[test]
    fn historique_serialise_pour_le_front() {
        let h = build_history(input(0, 1000, &[], &[host(120, 3.0, 10.0)]));
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["points"][0]["source"], "host");
        assert_eq!(v["players"][0], serde_json::json!({ "ts": 120, "count": 2 }));
        assert_eq!(v["host_ok"], true);
    }

    // --- Cache ---

    #[test]
    fn regroupement_des_requetes() {
        let b = BLOCK_S;
        // Blocs contigus regroupés, coupés à 3 jours, le dernier arrêté à `now`.
        let blocks: Vec<i64> = (0..14).map(|i| i * b).collect();
        let reqs = coalesce(&blocks, 13 * b + 100);
        assert_eq!(reqs, vec![(0, 12 * b), (12 * b, 13 * b + 100)]);
        // Un trou sépare les requêtes.
        assert_eq!(coalesce(&[0, 2 * b], 10 * b), vec![(0, b), (2 * b, 3 * b)]);
        assert!(coalesce(&[], 0).is_empty());
    }

    #[test]
    fn cache_plage_ancienne_definitive_bloc_courant_relu() {
        let c = HostStatsCache::default();
        // Le bloc 99 s'est terminé il y a 5 minutes (moins que SETTLE_S).
        let now = 100 * BLOCK_S + 300;
        let range = [(now - 2 * BLOCK_S, now)];
        // Rien en cache : trois blocs à lire (deux passés, le courant).
        let todo = c.plan(7, &range, now);
        assert_eq!(todo, vec![98 * BLOCK_S, 99 * BLOCK_S, 100 * BLOCK_S]);
        let reqs = coalesce(&todo, now);
        assert_eq!(reqs, vec![(98 * BLOCK_S, now)]);
        c.store(7, reqs[0].0, reqs[0].1, vec![host(99 * BLOCK_S + 60, 1.0, 1.0), host(now - 60, 2.0, 2.0)], now);
        // Juste après : rien à relire.
        assert!(c.plan(7, &range, now + 10).is_empty());
        // Après le délai du bloc courant : 98 est définitif ; 99 a été lu moins de SETTLE_S après
        // sa fin (minutes peut-être pas toutes écrites) : il est relu avec le bloc courant.
        let later = now + OPEN_TTL_S;
        assert_eq!(c.plan(7, &range, later), vec![99 * BLOCK_S, 100 * BLOCK_S]);
        // Une relecture bien après la fin de 99 le fige.
        let much_later = 101 * BLOCK_S + SETTLE_S;
        c.store(7, 99 * BLOCK_S, much_later, vec![host(99 * BLOCK_S + 60, 1.0, 1.0)], much_later);
        assert_eq!(c.plan(7, &[(98 * BLOCK_S, 100 * BLOCK_S)], much_later + 10 * 86_400), Vec::<i64>::new());
        // Un autre serveur ne partage rien.
        assert_eq!(c.plan(8, &range, now).len(), 3);
        // Lecture : bornée aux plages demandées, triée.
        let pts = c.read(7, &[(99 * BLOCK_S, 99 * BLOCK_S + 120)]);
        assert_eq!(pts.len(), 1);
    }

    #[test]
    fn cache_retention_et_futur() {
        let c = HostStatsCache::default();
        let now = 2_000 * BLOCK_S;
        // Au-delà de 365 jours : rien n'est demandé ; après `now` non plus.
        assert!(c.plan(1, &[(now - 400 * 86_400, now - 380 * 86_400)], now).is_empty());
        assert!(c.plan(1, &[(now + 10, now + 1_000)], now).is_empty());
    }

    #[test]
    fn cache_borne_en_taille() {
        let c = HostStatsCache::default();
        let now = 10_000 * BLOCK_S;
        for i in 0..(MAX_BLOCKS as i64 + 50) {
            c.store(1, i * BLOCK_S, (i + 1) * BLOCK_S, Vec::new(), now);
        }
        assert_eq!(c.inner.lock().unwrap().blocks.len(), MAX_BLOCKS);
        // Les plus anciens servis sont partis.
        assert!(!c.inner.lock().unwrap().blocks.contains_key(&(1, 0)));
    }

    #[test]
    fn decalage_d_horloge_et_pause_apres_erreur() {
        let c = HostStatsCache::default();
        // Horloge locale en retard d'une heure (cas réel) : corrigé.
        c.observe_clock(Some(10_000 + 3_600), 10_000);
        assert_eq!(c.skew(), 3_600);
        // Écart de quelques secondes : ignoré.
        c.observe_clock(Some(10_005), 10_000);
        assert_eq!(c.skew(), 0);
        c.observe_clock(None, 10_000);
        assert_eq!(c.skew(), 0);
        c.note_error(3, 1_000);
        assert!(c.in_backoff(3, 1_030));
        assert!(!c.in_backoff(3, 1_000 + ERROR_BACKOFF_S));
        assert!(!c.in_backoff(4, 1_030));
        c.clear_error(3);
        assert!(!c.in_backoff(3, 1_030));
    }
}
