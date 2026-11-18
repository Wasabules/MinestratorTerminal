//! Journaux **des serveurs** (outils MCP « Journaux et historique » : `search_logs`,
//! `error_digest`). À ne pas confondre avec [`crate::logs`], les journaux de l'app elle-même.
//!
//! - **Lecture** ([`Core::scan_logs`], [`Core::fetch_log`]) : liste SFTP du dossier des journaux
//!   (par défaut celui du jeu, [`crate::games::log_dir`]), sélection par dates, téléchargement
//!   **plafonné** (nombre de fichiers, octets téléchargés, taille par fichier, décompression bornée
//!   par [`crate::archive::gunzip_capped`]). Un plafond atteint est toujours signalé dans la sortie.
//! - **Dates** : le jour vient du nom du fichier (`2026-11-17-2.log.gz` commence le 17), l'heure
//!   de l'horodatage des lignes (`[21:03:12]`), le passage de minuit étant suivi ligne à ligne.
//!   `latest.log` n'a pas de date : on remonte depuis sa date de modification (sa dernière
//!   écriture). Tout est en **heure du serveur** : son décalage sur UTC est déduit de la fin de
//!   `latest.log` (dernière ligne ↔ date de modification), à défaut l'heure de Paris.
//! - **Signatures d'erreur** ([`signature`]) : classe d'exception (cause première) + premières
//!   frames utiles (hors JDK, serveur, loader, bibliothèques), sans nombres, adresses ni
//!   horodatages ; sans exception, le message normalisé. **Source probable** ([`culprit`]) :
//!   message du serveur (« Could not pass event … to X », « Error occurred while enabling X »), jar
//!   ou module des frames, préfixe `[X]`, logger, paquet Java.
//! - **Confidentialité** : la recherche porte sur le texte brut ; la sortie passe par
//!   [`crate::redact`] et les pseudos par un alias stable ([`crate::redact::pseudonym`], le même
//!   que `list_players`) quand l'anonymisation IA est active ([`Names`]).
//!
//! Tout le parsing est pur et testé sur des extraits au format réel (Paper, Spigot, Fabric, Forge,
//! Unreal) ; seules les méthodes de [`Core`] font du réseau.

use crate::models::SftpEntry;
use crate::redact::pseudonym;
use crate::store::now_secs;
use crate::{sftp, Core, Error, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;

// --- Plafonds ---------------------------------------------------------------------------------

/// Fichiers lus au plus par `search_logs` (défaut, maximum).
pub(crate) const SEARCH_FILES: (usize, usize) = (20, 60);
/// Fichiers lus au plus par `error_digest` (défaut, maximum).
pub(crate) const DIGEST_FILES: (usize, usize) = (8, 30);
/// Correspondances affichées au plus (défaut, maximum).
pub(crate) const SEARCH_RESULTS: (usize, usize) = (50, 200);
/// Octets téléchargés au plus par appel, tous fichiers confondus.
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
/// Un journal non compressé plus gros n'est lu que sur sa fin (les lignes les plus récentes).
const MAX_PLAIN_BYTES: u64 = 8 * 1024 * 1024;
/// Un `.gz` plus gros est ignoré : il faudrait le télécharger en entier pour le décompresser.
const MAX_GZ_BYTES: u64 = 8 * 1024 * 1024;
/// Taille décompressée maximale d'un `.gz` (anti gzip-bomb) : au-delà, son début seulement.
const MAX_UNPACKED_BYTES: u64 = 48 * 1024 * 1024;
/// Lignes de contexte au plus autour d'une correspondance.
pub(crate) const MAX_CONTEXT: usize = 5;
/// Caractères affichés au plus par ligne.
const MAX_LINE_CHARS: usize = 400;
/// Longueur maximale du motif recherché.
const MAX_QUERY_LEN: usize = 500;
/// Taille maximale de la sortie de `search_logs` (octets).
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Lignes de continuation gardées au plus par enregistrement (pile d'appels).
const MAX_BODY_LINES: usize = 200;
/// Groupes d'erreurs renvoyés au plus (défaut, maximum).
pub(crate) const DIGEST_GROUPS: (usize, usize) = (15, 50);
/// Fenêtre maximale de `error_digest` (heures).
pub(crate) const DIGEST_MAX_HOURS: i64 = 30 * 24;
/// Longueur de l'exemple représentatif d'un groupe (caractères).
const EXAMPLE_CHARS: usize = 700;
/// Octets lus en fin de journal courant pour estimer le décalage horaire du serveur.
const OFFSET_PROBE_BYTES: u64 = 4096;
/// Marge du tri des fichiers par dates : la conversion de leur date de modification dépend d'un
/// décalage horaire estimé. Le filtre fin se fait ensuite ligne à ligne.
const FILE_MARGIN_S: i64 = 14 * 3600;

// --- Fichiers de journaux ----------------------------------------------------------------------

/// Un fichier de journal repéré dans le dossier.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// Dernière modification (secondes Unix). Fiable pour un journal en clair (dernière écriture) ;
    /// pour un `.gz`, c'est l'heure de la compression, postérieure à sa dernière ligne.
    pub modified: Option<i64>,
    pub gz: bool,
    /// Jour où commence le contenu, d'après le nom (`2026-11-17-2.log.gz`,
    /// `FactoryGame-backup-2026.11.17-21.33.55.log`).
    pub name_date: Option<NaiveDate>,
    /// Rang dans la journée (`-2`), pour départager deux fichiers du même jour.
    pub index: u32,
    /// Journal en cours d'écriture (`latest.log`, `FactoryGame.log`, `factorio-current.log`).
    pub current: bool,
    /// Journal de débogage (Forge/Fabric `debug.log`, `debug-N.log.gz`) : très verbeux, exclu par
    /// défaut.
    pub debug: bool,
}

impl LogFile {
    /// Fichier `.log` ou `.log.gz` d'un listing SFTP, sinon `None`.
    pub(crate) fn from_entry(e: &SftpEntry) -> Option<Self> {
        if e.is_dir {
            return None;
        }
        let lower = e.name.to_ascii_lowercase();
        let gz = lower.ends_with(".log.gz");
        if !gz && !lower.ends_with(".log") {
            return None;
        }
        let stem = lower.strip_suffix(".gz").unwrap_or(&lower);
        let stem = stem.strip_suffix(".log").unwrap_or(stem);
        let (name_date, index) = match date_in_name(stem) {
            Some((d, end)) => (Some(d), index_after(&stem[end..])),
            None => (None, 0),
        };
        let ended = gz
            || name_date.is_some()
            || ["previous", "backup", "old"].iter().any(|w| stem.contains(w));
        Some(Self {
            path: e.path.clone(),
            name: e.name.clone(),
            size: e.size,
            modified: e.modified,
            gz,
            name_date,
            index,
            current: !ended,
            debug: stem.starts_with("debug"),
        })
    }

    /// Clé de récence (tri décroissant = du plus récent au plus ancien) : journaux courants, puis
    /// archives datées (jour, rang), les autres à leur date de modification.
    fn sort_key(&self) -> (i64, u32) {
        if self.current {
            return (i64::MAX - 1 + i64::from(!self.debug), 0);
        }
        match self.name_date.and_then(|d| d.and_hms_opt(0, 0, 0)) {
            Some(day) => (day.and_utc().timestamp(), self.index),
            None => (self.modified.unwrap_or(0), 0),
        }
    }

    /// Période couverte (heure du serveur) : début d'après le nom, fin d'après la date de
    /// modification (borne supérieure, y compris pour un `.gz`).
    fn span(&self, offset: i64) -> (Option<NaiveDateTime>, Option<NaiveDateTime>) {
        let start = self.name_date.and_then(|d| d.and_hms_opt(0, 0, 0));
        let end = self.modified.and_then(|m| local_time(m, offset));
        (start, end)
    }

    /// Fin connue du contenu, pour dater les lignes en remontant : la dernière écriture d'un
    /// journal en clair (celle d'un `.gz` est l'heure de sa compression, pas de sa dernière ligne).
    fn last_write(&self, offset: i64) -> Option<NaiveDateTime> {
        if self.gz {
            return None;
        }
        self.modified.and_then(|m| local_time(m, offset))
    }
}

/// Date `AAAA-MM-JJ` (ou `AAAA.MM.JJ`, `AAAA_MM_JJ`) dans un nom de fichier, et l'indice qui suit.
fn date_in_name(name: &str) -> Option<(NaiveDate, usize)> {
    let b = name.as_bytes();
    if b.len() < 10 {
        return None;
    }
    (0..=b.len() - 10).find_map(|i| {
        if i > 0 && b[i - 1].is_ascii_digit() {
            return None;
        }
        let sep = b[i + 4];
        if !matches!(sep, b'-' | b'.' | b'_') || b[i + 7] != sep {
            return None;
        }
        let date = NaiveDate::from_ymd_opt(four(b, i)?, two(b, i + 5)?, two(b, i + 8)?)?;
        Some((date, i + 10))
    })
}

/// Rang `-N` qui suit la date d'un nom (`2026-11-17-2` → 2), 0 sinon.
fn index_after(rest: &str) -> u32 {
    rest.strip_prefix('-').and_then(|r| r.parse().ok()).unwrap_or(0)
}

/// Heure du serveur d'un instant Unix, pour un décalage donné.
pub(crate) fn local_time(ts: i64, offset: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp(ts + offset, 0).map(|d| d.naive_utc())
}

/// Décalage (secondes) de l'heure du serveur sur UTC, déduit de la dernière ligne horodatée d'un
/// journal et de sa date de modification (sa dernière écriture) : arrondi au quart d'heure, entre
/// −12 h et +14 h.
pub(crate) fn estimate_offset(last: NaiveTime, modified: i64) -> i64 {
    let mut diff = i64::from(last.num_seconds_from_midnight()) - modified.rem_euclid(86_400);
    if diff > 14 * 3600 {
        diff -= 86_400;
    } else if diff < -12 * 3600 {
        diff += 86_400;
    }
    (diff + 450).div_euclid(900) * 900
}

/// `UTC+01:00`.
fn offset_label(offset: i64) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let a = offset.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
}

// --- Période ------------------------------------------------------------------------------------

/// Période demandée (heure du serveur), bornes ouvertes si absentes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Window {
    pub since: Option<NaiveDateTime>,
    pub until: Option<NaiveDateTime>,
}

impl Window {
    /// Un fichier peut-il contenir des lignes de la période ? (marge sur sa fin estimée)
    fn admits_file(&self, start: Option<NaiveDateTime>, end: Option<NaiveDateTime>) -> bool {
        let after = match (self.since, end) {
            (Some(s), Some(e)) => e + Duration::seconds(FILE_MARGIN_S) >= s,
            _ => true,
        };
        let before = match (self.until, start) {
            (Some(u), Some(st)) => st <= u,
            _ => true,
        };
        after && before
    }

    /// Une ligne datée est-elle dans la période ? (date inconnue : gardée, le fichier l'est)
    fn admits(&self, t: Option<NaiveDateTime>) -> bool {
        let Some(t) = t else { return true };
        self.since.is_none_or(|s| t >= s) && self.until.is_none_or(|u| t <= u)
    }
}

/// Borne de période : `AAAA-MM-JJ`, `AAAA-MM-JJ HH:MM[:SS]` (ou `T`), ou relative à `now` (`30m`,
/// `12h`, `7d` / `7j`, `2w`). Un jour seul vaut son début (`since`) ou sa fin (`until`, `end`).
pub(crate) fn parse_bound(s: &str, now: NaiveDateTime, end: bool) -> std::result::Result<Option<NaiveDateTime>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if let Some(secs) = relative_secs(s) {
        return Ok(Some(now - Duration::seconds(secs)));
    }
    let norm = s.replace('T', " ");
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(&norm, fmt) {
            return Ok(Some(t));
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(&norm, "%Y-%m-%d") {
        let t = if end { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) };
        return Ok(t);
    }
    Err(format!("date « {s} » non comprise : AAAA-MM-JJ, AAAA-MM-JJ HH:MM ou relative (12h, 7d)"))
}

/// `12h` → secondes ; `None` si ce n'est pas une durée relative.
fn relative_secs(s: &str) -> Option<i64> {
    let digits = s.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let n: i64 = s[..digits].parse::<i64>().ok()?.min(100_000);
    let mult = match s[digits..].trim().to_ascii_lowercase().as_str() {
        "m" | "min" => 60,
        "h" => 3600,
        "d" | "j" => 86_400,
        "w" | "sem" => 7 * 86_400,
        _ => return None,
    };
    Some(n * mult)
}

// --- Horodatage et en-tête d'une ligne ----------------------------------------------------------

/// Horodatage en tête de ligne.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stamp {
    /// Jour, si le format le donne (Forge, Unreal, ISO).
    pub date: Option<NaiveDate>,
    pub time: NaiveTime,
    /// Octets de l'horodatage en tête de ligne (crochets compris).
    pub len: usize,
}

fn two(b: &[u8], i: usize) -> Option<u32> {
    let (x, y) = (*b.get(i)?, *b.get(i + 1)?);
    (x.is_ascii_digit() && y.is_ascii_digit()).then(|| u32::from(x - b'0') * 10 + u32::from(y - b'0'))
}

fn four(b: &[u8], i: usize) -> Option<i32> {
    let hi = two(b, i)?;
    let lo = two(b, i + 2)?;
    Some((hi * 100 + lo) as i32)
}

/// `HH:MM:SS` (séparateur `sep`) à la position `i`.
fn hms(b: &[u8], i: usize, sep: u8) -> Option<NaiveTime> {
    if *b.get(i + 2)? != sep || *b.get(i + 5)? != sep {
        return None;
    }
    NaiveTime::from_hms_opt(two(b, i)?, two(b, i + 3)?, two(b, i + 6)?)
}

fn month(b: &[u8]) -> Option<u32> {
    const MONTHS: [&[u8]; 12] =
        [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"];
    MONTHS.iter().position(|m| *m == b).map(|i| i as u32 + 1)
}

/// Fin de l'horodatage : crochet fermant si la ligne en ouvre un, sinon `fallback`.
fn stamp_end(line: &str, bracketed: bool, fallback: usize) -> usize {
    if bracketed {
        line.find(']').map_or(fallback, |p| p + 1)
    } else {
        fallback
    }
}

/// Horodatage de tête des formats courants :
/// - `[21:03:12]` (vanilla, Paper et Fabric en fichier), `[21:03:12 INFO]` (console Paper/Spigot) ;
/// - `[17Nov2026 21:03:12.345]` (Forge, NeoForge) ;
/// - `[2026.11.17-21.03.12:345]` (Unreal : Satisfactory) ;
/// - `2026-11-17 21:03:12`, `[2026-11-17T21:03:12.345Z]` (ISO : Bukkit ancien, nombreux jeux) ;
/// - `21:03:12 ` en tête, sans crochets.
pub(crate) fn parse_stamp(line: &str) -> Option<Stamp> {
    let b = line.as_bytes();
    let bracketed = b.first() == Some(&b'[');
    if bracketed {
        if let Some(time) = hms(b, 1, b':') {
            if matches!(b.get(9), Some(b']' | b' ' | b'.' | b',')) {
                return Some(Stamp { date: None, time, len: stamp_end(line, true, 10) });
            }
        }
        if let (Some(day), Some(m), Some(year)) = (two(b, 1), b.get(3..6).and_then(month), four(b, 6)) {
            if b.get(10) == Some(&b' ') {
                if let Some(time) = hms(b, 11, b':') {
                    let date = NaiveDate::from_ymd_opt(year, m, day);
                    return Some(Stamp { date, time, len: stamp_end(line, true, 20) });
                }
            }
        }
        if let (Some(year), Some(m), Some(d)) = (four(b, 1), two(b, 6), two(b, 9)) {
            if b.get(5) == Some(&b'.') && b.get(8) == Some(&b'.') && b.get(11) == Some(&b'-') {
                if let Some(time) = hms(b, 12, b'.') {
                    let date = NaiveDate::from_ymd_opt(year, m, d);
                    return Some(Stamp { date, time, len: stamp_end(line, true, 21) });
                }
            }
        }
    }
    let o = usize::from(bracketed);
    if let (Some(year), Some(m), Some(d)) = (four(b, o), two(b, o + 5), two(b, o + 8)) {
        if b.get(o + 4) == Some(&b'-') && b.get(o + 7) == Some(&b'-') && matches!(b.get(o + 10), Some(b' ' | b'T')) {
            if let Some(time) = hms(b, o + 11, b':') {
                let date = NaiveDate::from_ymd_opt(year, m, d);
                // Fraction de seconde ou fuseau éventuels jusqu'au premier espace.
                let plain = o + 19 + b[o + 19..].iter().take_while(|c| !c.is_ascii_whitespace()).count();
                return Some(Stamp { date, time, len: stamp_end(line, bracketed, plain) });
            }
        }
    }
    if let Some(time) = hms(b, 0, b':') {
        if matches!(b.get(8), Some(b' ')) {
            return Some(Stamp { date: None, time, len: 8 });
        }
    }
    None
}

/// Niveau d'une ligne, du moins au plus grave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
    Fatal,
}

/// Niveau d'un mot (`ERROR`, `SEVERE`, `Warning`…) et son libellé affiché.
fn level_word(w: &str) -> Option<(Level, &'static str)> {
    Some(match w {
        "FATAL" | "Fatal" => (Level::Fatal, "FATAL"),
        "SEVERE" => (Level::Error, "SEVERE"),
        "ERROR" | "Error" => (Level::Error, "ERROR"),
        "WARN" | "WARNING" | "Warning" => (Level::Warn, "WARN"),
        "INFO" | "Info" | "Display" | "Log" | "DEBUG" | "TRACE" | "FINE" | "Verbose" => (Level::Info, "INFO"),
        _ => return None,
    })
}

/// En-tête analysé d'une ligne.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Header<'a> {
    pub stamp: Option<Stamp>,
    pub level: Option<(Level, &'static str)>,
    /// Nom du logger quand le format l'affiche (Forge : `[net.foo.Bar/]`, Fabric : `(modid)`).
    pub logger: Option<&'a str>,
    /// Texte qui suit l'en-tête.
    pub message: &'a str,
}

/// Découpe une ligne en horodatage, groupes entre crochets (thread/niveau, logger) et message.
pub(crate) fn parse_header(line: &str) -> Header<'_> {
    let stamp = parse_stamp(line);
    let mut pos = stamp.map_or(0, |s| s.len.min(line.len()));
    // Groupes : celui de l'horodatage (`12:34:56 INFO` en console) puis ceux qui suivent.
    let mut groups: Vec<&str> = Vec::new();
    if let Some(s) = stamp.filter(|s| line.starts_with('[') && s.len >= 2) {
        groups.push(&line[1..s.len - 1]);
    }
    let first_after_stamp = groups.len();
    if stamp.is_some() {
        while groups.len() < 6 {
            let rest = &line[pos..];
            let trimmed = rest.trim_start_matches(' ');
            if !trimmed.starts_with('[') {
                break;
            }
            let Some(close) = trimmed.find(']') else { break };
            let start = pos + (rest.len() - trimmed.len());
            groups.push(&line[start + 1..start + close]);
            pos = start + close + 1;
        }
    }
    let mut rest = &line[pos..];
    let mut logger = None;
    if let Some(r) = rest.strip_prefix(':') {
        rest = r;
    } else if let Some(r) = rest.trim_start().strip_prefix('(') {
        // Fabric : `[12:34:56] [Server thread/INFO] (modid) message`.
        if let Some(end) = r.find(") ") {
            logger = Some(&r[..end]);
            rest = &r[end + 2..];
        }
    }
    let message = if stamp.is_some() { rest.trim_start() } else { rest };
    // Logger Forge : troisième groupe (`[net.minecraft.server.MinecraftServer/]`).
    if logger.is_none() && groups.len() >= first_after_stamp + 2 {
        logger = groups.last().copied().filter(|g| g.contains('.') || g.ends_with('/'));
    }
    let level = groups
        .iter()
        .find_map(|g| {
            let word = g.rsplit(['/', ' ']).next().unwrap_or(g);
            level_word(word.trim())
        })
        .or_else(|| message_level(message));
    Header { stamp, level, logger, message }
}

/// Niveau porté par le message lui-même : Unreal (`LogNet: Warning: …`), Factorio (`Error …`),
/// `ERROR: …`.
fn message_level(message: &str) -> Option<(Level, &'static str)> {
    if message.starts_with("Log") {
        if let Some((_, after)) = message.split_once(": ") {
            let word = after.split(':').next().unwrap_or("");
            return level_word(word).or(Some((Level::Info, "INFO")));
        }
    }
    let first = message.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
    level_word(first.trim_matches(|c| c == '[' || c == ']'))
}

// --- Datation des lignes ------------------------------------------------------------------------

/// Date et heure (heure du serveur) de chaque ligne d'un journal ; une ligne sans horodatage prend
/// celles de la précédente. `start` : jour où commence le fichier (nom daté) ; à défaut, `end` :
/// sa dernière écriture, dont on remonte. Une heure qui recule de plus d'une heure marque un
/// passage de minuit ; une ligne écrite en retard par un autre thread juste avant minuit (après
/// une ligne de 00:00:02, une de 23:59:58) reste à la veille sans changer le jour courant. Ni
/// `start` ni `end` : seules les lignes à date complète sont datées.
pub(crate) fn date_lines(
    stamps: &[Option<Stamp>],
    start: Option<NaiveDate>,
    end: Option<NaiveDateTime>,
) -> Vec<Option<NaiveDateTime>> {
    const HOUR: i64 = 3600;
    let secs = |t: NaiveTime| i64::from(t.num_seconds_from_midnight());
    let mut out: Vec<Option<NaiveDateTime>> = vec![None; stamps.len()];
    if start.is_some() || end.is_none() {
        let mut day = start;
        let mut prev: Option<i64> = None;
        for (i, s) in stamps.iter().enumerate() {
            let Some(s) = s else { continue };
            let cur = secs(s.time);
            if s.date.is_some() {
                day = s.date;
                prev = Some(cur);
            } else if let Some(p) = prev {
                if cur + HOUR < p {
                    day = day.and_then(|d| d.succ_opt());
                    prev = Some(cur);
                } else if cur > p + 23 * HOUR {
                    out[i] = day.and_then(|d| d.pred_opt()).map(|d| d.and_time(s.time));
                    continue;
                } else {
                    prev = Some(p.max(cur));
                }
            } else {
                prev = Some(cur);
            }
            out[i] = day.map(|d| d.and_time(s.time));
        }
    } else if let Some(end) = end {
        let mut day = end.date();
        let mut later = secs(end.time());
        for (i, s) in stamps.iter().enumerate().rev() {
            let Some(s) = s else { continue };
            let cur = secs(s.time);
            if let Some(d) = s.date {
                day = d;
                later = cur;
            } else if cur > later + HOUR {
                day = day.pred_opt().unwrap_or(day);
                later = cur;
            } else if cur + 23 * HOUR < later {
                out[i] = day.succ_opt().map(|d| d.and_time(s.time));
                continue;
            } else {
                later = later.min(cur);
            }
            out[i] = Some(day.and_time(s.time));
        }
    }
    // Lignes de continuation : datées comme la ligne horodatée qui les précède.
    let mut last = None;
    for (i, s) in stamps.iter().enumerate() {
        if s.is_some() {
            last = out[i];
        } else {
            out[i] = last;
        }
    }
    out
}

// --- Pseudos ------------------------------------------------------------------------------------

/// Pseudos repérés (liste des joueurs, lignes des journaux) et leur alias stable.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// Pseudo en minuscules → alias (`joueur-xxxxxx`).
    by_name: HashMap<String, String>,
    /// UUID de joueur en minuscules → alias : un UUID permet de retrouver le pseudo, il ne part
    /// pas en clair non plus.
    by_uuid: HashMap<String, String>,
}

/// Pseudo Minecraft plausible (3 à 16 caractères `[A-Za-z0-9_]`).
fn valid_name(s: &str) -> bool {
    (3..=16).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl Names {
    /// Pseudo dont l'UUID est connu : même alias que `list_players`.
    pub(crate) fn add_uuid(&mut self, name: &str, uuid: &str) {
        if valid_name(name) {
            let alias = self.by_name.entry(name.to_ascii_lowercase()).or_insert_with(|| pseudonym(uuid)).clone();
            self.by_uuid.entry(uuid_key(uuid)).or_insert(alias);
        }
    }

    /// Pseudo vu sans son UUID : alias stable dérivé du pseudo.
    fn add_name(&mut self, name: &str) {
        if valid_name(name) {
            let lower = name.to_ascii_lowercase();
            let seed = format!("pseudo:{lower}");
            self.by_name.entry(lower).or_insert_with(|| pseudonym(&seed));
        }
    }

    fn is_name(&self, token: &str) -> bool {
        token.len() >= 3 && self.by_name.contains_key(&token.to_ascii_lowercase())
    }

    /// Pseudos qui portent cet alias.
    fn names_for(&self, alias: &str) -> Vec<&str> {
        let mut v: Vec<&str> = self.by_name.iter().filter(|(_, a)| *a == alias).map(|(n, _)| n.as_str()).collect();
        v.sort_unstable();
        v
    }

    /// Remplace chaque pseudo connu (mot entier, casse ignorée) par son alias, et chaque UUID par
    /// l'alias de son joueur (`[UUID]` s'il est inconnu).
    pub(crate) fn mask<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let text = self.mask_uuids(text);
        if self.by_name.is_empty() {
            return text;
        }
        let mut out = String::new();
        let mut copied = 0;
        for (start, end) in word_spans(&text) {
            if let Some(alias) = self.by_name.get(&text[start..end].to_ascii_lowercase()) {
                out.push_str(&text[copied..start]);
                out.push_str(alias);
                copied = end;
            }
        }
        if copied == 0 {
            return text;
        }
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    }

    fn mask_uuids<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let spans = uuid_spans(text);
        if spans.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        for (start, end) in spans {
            out.push_str(&text[copied..start]);
            match self.by_uuid.get(&uuid_key(&text[start..end])) {
                Some(alias) => out.push_str(alias),
                None => out.push_str("[UUID]"),
            }
            copied = end;
        }
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    }

    /// Repère les pseudos d'un message courant de Minecraft : UUID annoncé à la connexion,
    /// arrivée/départ, connexion, commande, chat, sanctions.
    pub(crate) fn discover(&mut self, message: &str) {
        if let Some(rest) = message.split_once("UUID of player ").map(|(_, r)| r) {
            if let Some((name, uuid)) = rest.split_once(" is ") {
                self.add_uuid(name.trim(), uuid.trim());
                return;
            }
        }
        const AFTER_NAME: [&str; 7] = [
            " joined the game",
            " left the game",
            " lost connection",
            " issued server command",
            "[/",
            " has made the advancement",
            " has completed the challenge",
        ];
        for marker in AFTER_NAME {
            if let Some(i) = message.find(marker) {
                let before = &message[..i];
                let name = before.rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().unwrap_or("");
                self.add_name(name);
            }
        }
        let chat = message.strip_prefix("[Not Secure] ").unwrap_or(message);
        if let Some(rest) = chat.strip_prefix('<') {
            if let Some(end) = rest.find('>') {
                self.add_name(&rest[..end]);
            }
        }
        for verb in ["Banned ", "Kicked ", "Unbanned "] {
            if let Some(i) = message.find(verb) {
                let after = &message[i + verb.len()..];
                let name = after.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().unwrap_or("");
                self.add_name(name);
            }
        }
    }
}

/// Clé d'un UUID : minuscules, sans tirets (l'API et les journaux ne l'écrivent pas pareil).
fn uuid_key(uuid: &str) -> String {
    uuid.chars().filter(|c| *c != '-').flat_map(char::to_lowercase).collect()
}

/// Positions des UUID (`8-4-4-4-12` chiffres hexadécimaux) d'un texte.
fn uuid_spans(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 36 <= b.len() {
        let shape = b[i..i + 36]
            .iter()
            .enumerate()
            .all(|(k, c)| if matches!(k, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() });
        let bounded = (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && b.get(i + 36).is_none_or(|c| !c.is_ascii_alphanumeric());
        if shape && bounded {
            out.push((i, i + 36));
            i += 36;
        } else {
            i += 1;
        }
    }
    out
}

/// Positions des mots `[A-Za-z0-9_]+` d'un texte.
fn word_spans(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let b = text.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < b.len() && !is_word(b[i]) {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        let start = i;
        while i < b.len() && is_word(b[i]) {
            i += 1;
        }
        Some((start, i))
    })
}

// --- Recherche ----------------------------------------------------------------------------------

/// Motif de recherche → expression compilée. Texte littéral sauf `regex` ; insensible à la casse
/// sauf `case_sensitive`. Un alias `joueur-xxxxxx` (celui que l'agent voit) est remplacé par le
/// ou les pseudos réels qu'il désigne : la recherche porte sur le texte brut.
pub(crate) fn compile_query(
    query: &str,
    regex: bool,
    case_sensitive: bool,
    names: &Names,
) -> std::result::Result<(Regex, Vec<String>), String> {
    if query.trim().is_empty() {
        return Err("paramètre `query` vide".into());
    }
    if query.len() > MAX_QUERY_LEN {
        return Err(format!("motif trop long ({} caractères au plus)", MAX_QUERY_LEN));
    }
    let mut notes = Vec::new();
    let mut source = String::new();
    let mut copied = 0;
    for (start, end) in alias_spans(query) {
        let alias = &query[start..end];
        let part = &query[copied..start];
        source.push_str(&if regex { part.to_string() } else { regex::escape(part) });
        let real = names.names_for(alias);
        if real.is_empty() {
            notes.push(format!("alias « {alias} » inconnu : cherché tel quel"));
            source.push_str(&regex::escape(alias));
        } else {
            // Pseudos gardés en minuscules : casse toujours ignorée pour eux.
            let alt: Vec<String> = real.iter().map(|n| regex::escape(n)).collect();
            let _ = write!(source, "(?i:{})", alt.join("|"));
        }
        copied = end;
    }
    let tail = &query[copied..];
    source.push_str(&if regex { tail.to_string() } else { regex::escape(tail) });
    let re = RegexBuilder::new(&source)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(4 << 20)
        .build()
        .map_err(|e| format!("expression régulière invalide : {e}"))?;
    Ok((re, notes))
}

/// Positions des alias `joueur-xxxxxx` dans un texte.
fn alias_spans(text: &str) -> Vec<(usize, usize)> {
    const PREFIX: &str = "joueur-";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(PREFIX) {
        let start = from + i;
        let hex_start = start + PREFIX.len();
        let hex = text[hex_start..].bytes().take_while(u8::is_ascii_hexdigit).count();
        let boundary_before = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        if hex == 6 && boundary_before {
            out.push((start, hex_start + 6));
        }
        from = hex_start;
    }
    out
}

/// Indices des lignes qui correspondent et tombent dans la période.
pub(crate) fn matching_lines(lines: &[&str], dates: &[Option<NaiveDateTime>], re: &Regex, window: &Window) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| window.admits(dates.get(*i).copied().flatten()) && is_match(re, l))
        .map(|(i, _)| i)
        .collect()
}

fn is_match(re: &Regex, line: &str) -> bool {
    if line.contains('\u{1b}') {
        re.is_match(&crate::paste::strip_ansi(line))
    } else {
        re.is_match(line)
    }
}

/// Une ligne rendue d'un bloc de résultats (texte brut : anonymisé au rendu final).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Row {
    /// Séparateur entre deux blocs disjoints.
    Gap,
    Line { no: usize, hit: bool, day: Option<NaiveDate>, text: String },
}

/// Lignes à afficher pour des correspondances (et leur contexte), façon `grep -n -C`.
pub(crate) fn hit_rows(lines: &[&str], dates: &[Option<NaiveDateTime>], hits: &[usize], context: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut shown_until: Option<usize> = None;
    for (k, &h) in hits.iter().enumerate() {
        let from = h.saturating_sub(context);
        let next_hit = hits.get(k + 1).copied();
        let to = (h + context).min(lines.len().saturating_sub(1));
        let from = shown_until.map_or(from, |u| from.max(u + 1));
        if shown_until.is_some_and(|u| from > u + 1) {
            rows.push(Row::Gap);
        }
        for (i, line) in lines.iter().enumerate().take(to + 1).skip(from) {
            // La ligne suivante qui correspond sera rendue par son propre tour.
            if next_hit.is_some_and(|n| i >= n) {
                break;
            }
            rows.push(Row::Line {
                no: i + 1,
                hit: i == h,
                day: dates.get(i).copied().flatten().map(|d| d.date()),
                text: crate::util::truncate_on_boundary(&clean_line(line), MAX_LINE_CHARS * 4, "…"),
            });
            shown_until = Some(i);
        }
    }
    rows
}

/// Ligne sans couleurs ANSI ni fin de ligne.
fn clean_line(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.contains('\u{1b}') {
        crate::paste::strip_ansi(line)
    } else {
        line.to_string()
    }
}

/// Tronque à `max` caractères.
fn cut_chars(s: &str, max: usize) -> Cow<'_, str> {
    match s.char_indices().nth(max) {
        Some((i, _)) => Cow::Owned(format!("{}…", &s[..i])),
        None => Cow::Borrowed(s),
    }
}

// --- Enregistrements et signatures d'erreur -----------------------------------------------------

/// Un enregistrement de journal : une ligne d'en-tête et ses lignes de continuation (pile
/// d'appels, message sur plusieurs lignes).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record<'a> {
    /// Numéro (à partir de 1) de la ligne d'en-tête.
    pub line_no: usize,
    pub ts: Option<NaiveDateTime>,
    /// Heure seule (format sans date, ou date non déduite).
    pub time: Option<NaiveTime>,
    pub level: Option<(Level, &'static str)>,
    pub logger: Option<&'a str>,
    pub message: &'a str,
    pub body: Vec<&'a str>,
}

/// Ligne de pile d'appels (`at …`, `Caused by: …`, `... 12 more`, `Suppressed: …`).
fn is_trace_line(s: &str) -> bool {
    let t = s.trim_start();
    (t.starts_with("at ") && t.contains('('))
        || t.starts_with("Caused by:")
        || t.starts_with("Suppressed:")
        || (t.starts_with("... ") && t.ends_with(" more"))
}

/// Regroupe les lignes en enregistrements. Une ligne horodatée ouvre un enregistrement, sauf une
/// ligne de pile (Spigot préfixe chaque frame de l'en-tête) ; une ligne sans horodatage prolonge
/// le précédent. Journal sans aucun horodatage : une ligne = un enregistrement.
pub(crate) fn records<'a>(lines: &[&'a str], dates: &[Option<NaiveDateTime>]) -> Vec<Record<'a>> {
    let has_stamps = lines.iter().take(500).any(|l| parse_stamp(l).is_some());
    let mut out: Vec<Record<'a>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let h = parse_header(line);
        if has_stamps && (h.stamp.is_none() || is_trace_line(h.message)) {
            if let Some(cur) = out.last_mut() {
                if cur.body.len() < MAX_BODY_LINES {
                    cur.body.push(if h.stamp.is_some() { h.message } else { line });
                }
                continue;
            }
        }
        out.push(Record {
            line_no: i + 1,
            ts: dates.get(i).copied().flatten(),
            time: h.stamp.map(|s| s.time),
            level: h.level,
            logger: h.logger,
            message: h.message.trim(),
            body: Vec::new(),
        });
    }
    out
}

/// Une exception de la chaîne (`Caused by` compris) et ses frames.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Thrown<'a> {
    pub class: &'a str,
    pub message: &'a str,
    pub frames: Vec<Frame<'a>>,
}

/// Une frame de pile (`at pkg.Classe.méthode(Fichier.java:42) ~[plugin.jar:?]`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Frame<'a> {
    /// `pkg.Classe.méthode`.
    pub method: &'a str,
    /// Module (Forge : `TRANSFORMER/create@0.5.1/…` → `create` ; `java.base`).
    pub module: Option<&'a str>,
    /// Jar d'origine (`~[EssentialsX-2.20.1.jar:?]`, Paper : `EssentialsX-2.20.1.jar//…`).
    pub jar: Option<&'a str>,
}

/// Exception en tête de `s` : `java.lang.NullPointerException: …`, `Caused by: …`,
/// `Exception in thread "…" java.lang.X: …`. Renvoie la classe et le message.
pub(crate) fn exception_at(s: &str) -> Option<(&str, &str)> {
    let mut t = s.trim_start();
    for prefix in ["Caused by: ", "Suppressed: "] {
        if let Some(r) = t.strip_prefix(prefix) {
            t = r;
        }
    }
    if let Some(r) = t.strip_prefix("Exception in thread \"") {
        t = r.split_once("\" ").map(|(_, rest)| rest)?;
    }
    let end = t
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '$' | '_')))
        .unwrap_or(t.len());
    let (class, rest) = t.split_at(end);
    let simple = class.rsplit('.').next().unwrap_or(class);
    // `net.minecraft.class_148` : exception de Minecraft sous son nom intermédiaire (Fabric).
    let looks = ["Exception", "Error", "Throwable"].iter().any(|s| simple.ends_with(s))
        || (class.starts_with("net.minecraft.class_") && rest.starts_with(':'));
    if !(class.contains('.') && looks && (rest.is_empty() || rest.starts_with(':'))) {
        return None;
    }
    Some((class, rest.trim_start_matches(':').trim()))
}

/// Analyse une ligne `at …`.
pub(crate) fn parse_frame(line: &str) -> Option<Frame<'_>> {
    let rest = line.trim_start().strip_prefix("at ")?;
    let paren = rest.find('(')?;
    let full = &rest[..paren];
    if full.is_empty() || full.contains(' ') {
        return None;
    }
    // Dernier `/` hors classe cachée (`Foo$$Lambda$12/0x0000000801a2b3c4.run`) : il sépare le
    // module ou le jar de la méthode.
    let mut search_end = full.len();
    let split = loop {
        match full[..search_end].rfind('/') {
            Some(p) if full[p + 1..].starts_with("0x") => search_end = p,
            other => break other,
        }
    };
    let (prefix, method) = match split {
        Some(p) => (&full[..p], &full[p + 1..]),
        None => ("", full),
    };
    if !method.contains('.') {
        return None;
    }
    let mut module = None;
    let mut jar = None;
    for seg in prefix.split('/').filter(|s| !s.is_empty()) {
        if seg.ends_with(".jar") {
            jar = Some(seg);
        } else if let Some((id, _)) = seg.split_once('@') {
            module = Some(id);
        } else if seg.contains('.') {
            module = Some(seg);
        }
    }
    if jar.is_none() {
        let after = &rest[paren..];
        if let Some(open) = after.find('[') {
            let inner = &after[open + 1..];
            let name = &inner[..inner.find([':', ']']).unwrap_or(inner.len())];
            if let Some(j) = name.find(".jar") {
                jar = Some(&name[..j + 4]);
            }
        }
    }
    Some(Frame { method, module, jar })
}

/// Chaîne d'exceptions d'un enregistrement, dans l'ordre (la dernière est la cause première).
pub(crate) fn thrown<'a>(rec: &Record<'a>) -> Vec<Thrown<'a>> {
    let mut out: Vec<Thrown<'a>> = Vec::new();
    for line in std::iter::once(rec.message).chain(rec.body.iter().copied()) {
        if let Some((class, message)) = exception_at(line) {
            out.push(Thrown { class, message, frames: Vec::new() });
        } else if let Some(f) = parse_frame(line) {
            if let Some(last) = out.last_mut() {
                last.frames.push(f);
            }
        }
    }
    out
}

/// Paquets du JDK, des serveurs, des loaders et des bibliothèques courantes : leurs frames ne
/// disent pas d'où vient l'erreur.
const PLATFORM_PACKAGES: &[&str] = &[
    "java.", "javax.", "jdk.", "sun.", "com.sun.", "kotlin.", "kotlinx.", "scala.", "groovy.",
    "net.minecraft.", "com.mojang.", "org.bukkit.", "org.spigotmc.", "io.papermc.", "com.destroystokyo.",
    "co.aikar.", "net.md_5.", "org.purpurmc.", "ca.spottedleaf.", "net.minecrell.", "org.leavesmc.",
    "net.fabricmc.", "net.minecraftforge.", "net.neoforged.", "cpw.mods.", "org.spongepowered.asm.",
    "org.objectweb.asm.", "com.llamalad7.mixinextras.", "io.netty.", "com.google.", "org.apache.",
    "org.slf4j.", "it.unimi.dsi.", "org.jline.", "joptsimple.", "org.yaml.snakeyaml.", "net.kyori.",
    "com.zaxxer.hikari.", "org.sqlite.", "com.mysql.", "org.mariadb.", "org.h2.", "okhttp3.", "okio.",
    "org.jetbrains.", "com.velocitypowered.", "reactor.", "io.github.classgraph.",
];

fn platform_method(m: &str) -> bool {
    PLATFORM_PACKAGES.iter().any(|p| m.starts_with(p))
}

/// Jars des serveurs, loaders et bibliothèques (début du nom, minuscules).
const PLATFORM_JARS: &[&str] = &[
    "paper", "spigot", "purpur", "folia", "pufferfish", "craftbukkit", "bukkit", "minecraft", "server",
    "forge", "neoforge", "fabric-loader", "fabric-api", "mohist", "arclight", "magma", "intermediary",
    "patched", "mixin", "sponge-mixin", "fmlcore", "fmlloader", "javafmllanguage", "lowcodelanguage",
    "mclanguage", "modlauncher", "securejarhandler", "bootstraplauncher", "datafixerupper", "brigadier",
    "authlib", "netty", "guava", "log4j", "gson", "kotlin", "asm", "velocity", "waterfall", "bungeecord",
];

fn platform_jar(j: &str) -> bool {
    let l = j.to_ascii_lowercase();
    PLATFORM_JARS.iter().any(|p| l.starts_with(p))
}

/// Modules (Forge) des serveurs et loaders.
fn platform_module(m: &str) -> bool {
    let l = m.to_ascii_lowercase();
    l.contains('.')
        || ["minecraft", "forge", "neoforge", "fml", "mixin", "modlauncher", "app", "unnamed"]
            .iter()
            .any(|p| l.starts_with(p))
}

/// `pkg.Classe$1.lambda$run$3` → chiffres après `$` remplacés, identifiants uniques des handlers
/// Mixin (`$zza000$`) et adresses de classes cachées (`/0x…`) retirés.
fn normalize_frame(method: &str) -> String {
    let mut out = String::with_capacity(method.len());
    for (i, seg) in method.split('$').enumerate() {
        let seg = match seg.find("/0x") {
            Some(p) => {
                let after = &seg[p + 3..];
                let skip = after.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(after.len());
                Cow::Owned(format!("{}{}", &seg[..p], &after[skip..]))
            }
            None => Cow::Borrowed(seg),
        };
        if i > 0 {
            let b = seg.as_bytes();
            let mixin_id = b.len() == 6 && b[..3].iter().all(u8::is_ascii_lowercase) && b[3..].iter().all(u8::is_ascii_digit);
            if mixin_id {
                continue;
            }
            out.push('$');
            let digits = seg.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                out.push('#');
                out.push_str(&seg[digits..]);
                continue;
            }
        }
        out.push_str(&seg);
    }
    out
}

/// Message normalisé pour regrouper : pseudos → `<joueur>`, UUID → `<uuid>`, longs identifiants
/// hexadécimaux → `<hex>`, nombres (et adresses IP, versions) → `#`, espaces compactés ; tronqué.
pub(crate) fn normalize_message(msg: &str, names: &Names, max: usize) -> String {
    let msg = clean_line(msg);
    let mut out = String::with_capacity(msg.len());
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let mut chars = msg.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_tok(c) {
            if c.is_whitespace() {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            } else {
                out.push(c);
            }
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, d)) = chars.peek() {
            if !is_tok(d) {
                break;
            }
            end = j + d.len_utf8();
            chars.next();
        }
        let raw = &msg[i..end];
        // Ponctuation finale (`cycle.`) rendue telle quelle.
        let token = raw.trim_end_matches(['.', '-']);
        out.push_str(&normalize_token(token, names));
        out.push_str(&raw[token.len()..]);
    }
    cut_chars(out.trim(), max).into_owned()
}

fn normalize_token<'a>(t: &'a str, names: &Names) -> Cow<'a, str> {
    if t.is_empty() {
        return Cow::Borrowed(t);
    }
    if names.is_name(t) {
        return Cow::Borrowed("<joueur>");
    }
    let hex_dash = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if t.len() == 36 && hex_dash(t) && t.matches('-').count() == 4 {
        return Cow::Borrowed("<uuid>");
    }
    let has_digit = t.bytes().any(|b| b.is_ascii_digit());
    if !has_digit {
        return Cow::Borrowed(t);
    }
    if t.len() >= 8 && t.chars().all(|c| c.is_ascii_hexdigit()) && t.bytes().any(|b| b.is_ascii_alphabetic()) {
        return Cow::Borrowed("<hex>");
    }
    if t.starts_with("0x") {
        return Cow::Borrowed("<adr>");
    }
    // Suites de chiffres (avec points internes : versions, IP, décimaux) → `#`.
    let mut out = String::with_capacity(t.len());
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))) {
                i += 1;
            }
            out.push('#');
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    Cow::Owned(out)
}

/// Signature d'un enregistrement : classe de la cause première + deux premières frames utiles ;
/// sans frame utile, classe + message normalisé ; sans exception, message normalisé.
pub(crate) fn signature(rec: &Record, chain: &[Thrown], names: &Names) -> String {
    let Some(root) = chain.last() else {
        return normalize_message(rec.message, names, 160);
    };
    // Frames de la cause première, sinon des exceptions qui l'enveloppent (de la plus profonde).
    let frames: Vec<String> = chain
        .iter()
        .rev()
        .map(|t| t.frames.iter().filter(|f| !platform_method(f.method)).take(2).collect::<Vec<_>>())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
        .into_iter()
        .map(|f| normalize_frame(f.method))
        .collect();
    if frames.is_empty() {
        format!("{} : {}", root.class, normalize_message(root.message, names, 100))
    } else {
        format!("{} @ {}", root.class, frames.join(" < "))
    }
}

/// Plugin ou mod probable d'une erreur, et l'indice qui le désigne.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub(crate) struct Culprit {
    pub name: String,
    /// `event` | `enable` | `load` | `task` | `mod` | `jar` | `prefix` | `logger` | `package`
    pub evidence: &'static str,
}

impl Culprit {
    fn new(name: &str, evidence: &'static str) -> Option<Self> {
        let name = name.trim().trim_matches(|c: char| matches!(c, '\'' | '"' | ',' | '.' | ':' | '(' | ')'));
        (!name.is_empty() && name.len() <= 60).then(|| Self { name: name.to_string(), evidence })
    }
}

/// Plugin ou mod probable : d'abord ce que dit le serveur (« Could not pass event … to X »,
/// « Error occurred while enabling X », tâche planifiée, « from mod x »), puis le jar ou le module
/// des frames (cause première d'abord), le préfixe `[X]` du message, le logger, et en dernier
/// recours le paquet Java de la première frame utile.
pub(crate) fn culprit(rec: &Record, chain: &[Thrown]) -> Option<Culprit> {
    for text in std::iter::once(rec.message).chain(rec.body.iter().copied().take(8)) {
        if let Some(c) = culprit_in_text(text) {
            return Some(c);
        }
    }
    for t in chain.iter().rev() {
        for f in t.frames.iter().filter(|f| !platform_method(f.method)) {
            // Module Forge (identifiant du mod) plus sûr que le nom du jar.
            if let Some(m) = f.module.filter(|m| !platform_module(m)) {
                return Culprit::new(m, "mod");
            }
            if let Some(name) = f.jar.filter(|j| !platform_jar(j)).and_then(jar_name) {
                return Culprit::new(&name, "jar");
            }
        }
    }
    if let Some(p) = bracket_prefix(rec.message) {
        return Culprit::new(p, "prefix");
    }
    if let Some(l) = rec.logger.and_then(logger_name) {
        return Culprit::new(&l, "logger");
    }
    chain
        .iter()
        .rev()
        .find_map(|t| t.frames.iter().find(|f| !platform_method(f.method)))
        .and_then(|f| Culprit::new(&package_of(f.method), "package"))
}

/// Nom d'un plugin dans un message, jusqu'à sa version (` v1.2`) ou la fin du mot.
fn name_before_version(s: &str) -> &str {
    if let Some(i) = s.find(" v").filter(|&i| s[i + 2..].starts_with(|c: char| c.is_ascii_digit())) {
        return &s[..i];
    }
    s.split_whitespace().next().unwrap_or("")
}

fn culprit_in_text(t: &str) -> Option<Culprit> {
    if let Some((_, rest)) = t.split_once("Could not pass event ") {
        if let Some((_, to)) = rest.split_once(" to ") {
            return Culprit::new(name_before_version(to), "event");
        }
    }
    for marker in ["Error occurred while enabling ", "Error occurred while disabling ", "Error occurred while loading "] {
        if let Some((_, rest)) = t.split_once(marker) {
            return Culprit::new(name_before_version(rest), "enable");
        }
    }
    // Paper : deux jars pour un même plugin (« Ambiguous plugin name 'X' for files … »).
    if let Some((_, rest)) = t.split_once("Ambiguous plugin name '") {
        if let Some((name, _)) = rest.split_once('\'') {
            return Culprit::new(name, "load");
        }
    }
    if let Some((_, rest)) = t.split_once("Could not load ") {
        if let Some(quoted) = rest.split('\'').nth(1) {
            let file = quoted.rsplit('/').next().unwrap_or(quoted);
            if let Some(name) = jar_name(file) {
                return Culprit::new(&name, "load");
            }
        }
    }
    if t.contains("generated an exception") {
        if let Some((_, rest)) = t.split_once("Plugin ") {
            return Culprit::new(name_before_version(rest), "task");
        }
        if let Some((_, rest)) = t.split_once("Task #").and_then(|(_, r)| r.split_once(" for ")) {
            return Culprit::new(name_before_version(rest), "task");
        }
    }
    for marker in ["from mod ", "ModID: ", "Mod ID: "] {
        if let Some((_, rest)) = t.split_once(marker) {
            let id = rest.split(|c: char| c.is_whitespace() || c == ',' || c == ']' || c == ')').next().unwrap_or("");
            return Culprit::new(id, "mod");
        }
    }
    None
}

/// `EssentialsX-2.20.1.jar` → `EssentialsX` ; `sodium-fabric-0.5.8+mc1.20.4.jar` → `sodium-fabric`.
fn jar_name(jar: &str) -> Option<String> {
    let stem = jar.split(".jar").next().unwrap_or(jar);
    let stem = stem.split('%').next().unwrap_or(stem);
    let b = stem.as_bytes();
    let end = (1..b.len())
        .find(|&i| {
            let sep = matches!(b[i - 1], b'-' | b'_' | b'+' | b' ');
            let version = b[i].is_ascii_digit()
                || (matches!(b[i], b'v' | b'V') && b.get(i + 1).is_some_and(u8::is_ascii_digit));
            sep && version
        })
        .map_or(b.len(), |i| i - 1);
    let name = stem[..end].trim_end_matches(['-', '_', '+', ' ']);
    (!name.is_empty()).then(|| name.to_string())
}

/// Préfixe `[NomDuPlugin]` d'un message (Paper/Spigot), hors marqueurs génériques.
fn bracket_prefix(message: &str) -> Option<&str> {
    // Marqueurs génériques et composants internes de Paper (chargement des plugins).
    const IGNORED: &[&str] = &[
        "STDOUT", "STDERR", "Not Secure", "Server", "Rcon", "System", "minecraft", "Minecraft",
        "ModernPluginLoadingStrategy", "LegacyPluginLoadingStrategy", "PluginInitializerManager",
        "PaperPluginsStorage", "PaperPluginInstanceManager", "SpigotLibraryLoader", "PluginRemapper",
    ];
    let mut rest = message;
    for _ in 0..2 {
        let inner = rest.strip_prefix('[')?;
        let end = inner.find(']')?;
        let name = &inner[..end];
        rest = inner[end + 1..].trim_start();
        let plausible = (2..=40).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
        if !plausible {
            return None;
        }
        if !IGNORED.contains(&name) {
            return Some(name);
        }
    }
    None
}

/// Logger Forge (`com.simibubi.create.Create/`) ou Fabric (`sodium`) → nom utile, hors loggers
/// du serveur et des loaders.
fn logger_name(logger: &str) -> Option<String> {
    let l = logger.split('/').next().unwrap_or(logger).trim();
    if l.is_empty() {
        return None;
    }
    if l.contains('.') {
        let probe = format!("{l}.x");
        return (!platform_method(&probe)).then(|| package_of(&probe));
    }
    const IGNORED: &[&str] = &["Minecraft", "minecraft", "FabricLoader", "Fabric", "mixin", "Mixin", "main", "Server", "FML", "STDOUT", "STDERR"];
    (!IGNORED.iter().any(|i| l.starts_with(i))).then(|| l.to_string())
}

/// Paquet (trois premiers segments au plus) d'une méthode qualifiée `pkg.Classe.méthode`.
fn package_of(method: &str) -> String {
    let segs: Vec<&str> = method.split('.').collect();
    let pkg = &segs[..segs.len().saturating_sub(2)];
    pkg[..pkg.len().min(3)].join(".")
}

/// Exemple représentatif : en-tête et premières lignes de pile, tronqué.
fn example_of(rec: &Record) -> String {
    let mut s = rec.message.to_string();
    for line in rec.body.iter().take(6) {
        s.push('\n');
        s.push_str(line.trim_end());
    }
    if rec.body.len() > 6 {
        let _ = write!(s, "\n… (+{} lignes)", rec.body.len() - 6);
    }
    cut_chars(&clean_line(&s), EXAMPLE_CHARS).into_owned()
}

// --- Digest -------------------------------------------------------------------------------------

/// Un groupe d'erreurs de même signature.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ErrorGroup {
    /// Niveau le plus grave vu : `FATAL` | `SEVERE` | `ERROR` | `WARN`.
    pub level: &'static str,
    #[serde(skip)]
    pub severity: Level,
    pub count: usize,
    /// Première et dernière occurrence (heure du serveur ; `HH:MM:SS` seule si le jour est inconnu).
    pub first: Option<String>,
    pub last: Option<String>,
    #[serde(skip)]
    first_ts: Option<NaiveDateTime>,
    #[serde(skip)]
    last_ts: Option<NaiveDateTime>,
    pub signature: String,
    pub probable_source: Option<Culprit>,
    /// Exemple (la dernière occurrence), tronqué.
    pub example: String,
    /// Fichiers (ou `console`) où le groupe apparaît, trois au plus.
    pub seen_in: Vec<String>,
}

/// Accumulateur de groupes, par signature, sur plusieurs fichiers.
#[derive(Debug, Default)]
pub(crate) struct Digest {
    groups: HashMap<String, ErrorGroup>,
    /// Sources probables vues par groupe (la plus fréquente l'emporte).
    culprits: HashMap<String, HashMap<Culprit, usize>>,
    pub examined: usize,
}

fn when(rec: &Record) -> Option<String> {
    match (rec.ts, rec.time) {
        (Some(ts), _) => Some(ts.format("%Y-%m-%d %H:%M:%S").to_string()),
        (None, Some(t)) => Some(t.format("%H:%M:%S").to_string()),
        _ => None,
    }
}

impl Digest {
    /// Ajoute les enregistrements d'une source (`label` : chemin du fichier ou `console`) de niveau
    /// au moins `min` et dans la période.
    pub(crate) fn add(&mut self, recs: &[Record], label: &str, min: Level, window: &Window, names: &Names) {
        for rec in recs {
            let chain = thrown(rec);
            let level = match rec.level {
                // Une pile d'appels sans niveau (ou en INFO : `printStackTrace`) compte comme WARN.
                Some((l, label)) if l >= Level::Warn => (l, label),
                _ if !chain.is_empty() => (Level::Warn, "WARN"),
                _ => continue,
            };
            if level.0 < min || !window.admits(rec.ts) {
                continue;
            }
            self.examined += 1;
            let sig = signature(rec, &chain, names);
            let w = when(rec);
            let g = self.groups.entry(sig.clone()).or_insert_with(|| ErrorGroup {
                level: level.1,
                severity: level.0,
                count: 0,
                first: None,
                last: None,
                first_ts: None,
                last_ts: None,
                signature: sig.clone(),
                probable_source: None,
                example: String::new(),
                seen_in: Vec::new(),
            });
            g.count += 1;
            if level.0 > g.severity {
                g.severity = level.0;
                g.level = level.1;
            }
            match rec.ts {
                // Fichiers lus du plus récent au plus ancien : on compare les dates.
                Some(ts) => {
                    if g.first_ts.is_none_or(|f| ts < f) {
                        g.first_ts = Some(ts);
                        g.first = w.clone();
                    }
                    if g.last_ts.is_none_or(|l| ts >= l) {
                        g.last_ts = Some(ts);
                        g.last = w;
                        g.example = example_of(rec);
                    }
                }
                // Heure seule (console) : ordre d'apparition.
                None => {
                    if g.first.is_none() {
                        g.first = w.clone();
                    }
                    if g.last_ts.is_none() {
                        g.last = w;
                        g.example = example_of(rec);
                    }
                }
            }
            if g.seen_in.len() < 3 && !g.seen_in.iter().any(|s| s == label) {
                g.seen_in.push(label.to_string());
            }
            if let Some(c) = culprit(rec, &chain) {
                *self.culprits.entry(sig).or_default().entry(c).or_default() += 1;
            }
        }
    }

    /// Groupes triés par gravité, puis fréquence, puis récence.
    pub(crate) fn finish(mut self) -> Vec<ErrorGroup> {
        let mut out: Vec<ErrorGroup> = self
            .groups
            .drain()
            .map(|(sig, mut g)| {
                if let Some(c) = self.culprits.get(&sig) {
                    // Le plus fréquent ; à égalité, le plus probant (ordre des indices).
                    g.probable_source = c
                        .iter()
                        .max_by_key(|(c, n)| (**n, std::cmp::Reverse(evidence_rank(c.evidence)), std::cmp::Reverse(c.name.clone())))
                        .map(|(c, _)| c.clone());
                }
                g
            })
            .collect();
        out.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.count.cmp(&a.count))
                .then(b.last_ts.cmp(&a.last_ts))
                .then(a.signature.cmp(&b.signature))
        });
        out
    }
}

fn evidence_rank(e: &str) -> usize {
    ["event", "enable", "load", "task", "mod", "jar", "prefix", "logger", "package"]
        .iter()
        .position(|x| *x == e)
        .unwrap_or(99)
}

// --- Façade (réseau) ----------------------------------------------------------------------------

/// Fichiers d'un dossier de journaux, du plus récent au plus ancien, et heure du serveur.
pub(crate) struct LogDir {
    pub dir: String,
    pub files: Vec<LogFile>,
    /// Fichiers `.log`/`.log.gz` du dossier, journaux de débogage compris.
    pub listed: usize,
    /// Décalage de l'heure du serveur sur UTC (secondes).
    pub offset: i64,
    /// Décalage déduit du journal courant (sinon : heure de Paris supposée).
    pub offset_measured: bool,
}

impl LogDir {
    fn now(&self) -> NaiveDateTime {
        local_time(now_secs(), self.offset).unwrap_or_default()
    }

    fn offset_note(&self) -> String {
        if self.offset_measured {
            format!("heure du serveur ({}, déduite du journal courant)", offset_label(self.offset))
        } else {
            format!("heure du serveur supposée à l'heure de Paris ({})", offset_label(self.offset))
        }
    }
}

/// Lecture d'un fichier de journal.
pub(crate) enum Fetched {
    Text {
        text: String,
        /// Octets téléchargés.
        downloaded: u64,
        /// Lu en partie : `tail` (fin seulement) ou `head` (début seulement, décompression bornée).
        partial: Option<&'static str>,
    },
    /// Ignoré (raison).
    Skipped(String),
    /// Dépasserait le plafond d'octets téléchargés.
    OverBudget,
}

/// Bilan d'une lecture de journaux, pour signaler les plafonds atteints.
#[derive(Debug, Default)]
pub(crate) struct ReadReport {
    pub read: usize,
    pub bytes: u64,
    /// Fichiers retenus non lus : plafond de fichiers.
    pub over_files: usize,
    /// Fichiers retenus non lus : plafond d'octets.
    pub over_bytes: usize,
    /// Fichiers retenus non lus : plafond de résultats déjà atteint.
    pub after_results: usize,
    pub partial: Vec<String>,
    pub skipped: Vec<String>,
}

impl ReadReport {
    /// Lignes « plafond atteint » à joindre à la sortie.
    fn limits(&self, max_files: usize) -> Vec<String> {
        let mut out = Vec::new();
        if self.over_files > 0 {
            out.push(format!(
                "plafond de fichiers atteint ({max_files}) : {} fichier(s) de la période non lu(s) — resserre la période ou augmente max_files",
                self.over_files
            ));
        }
        if self.over_bytes > 0 {
            out.push(format!(
                "plafond de téléchargement atteint ({} Mio) : {} fichier(s) non lu(s) — resserre la période",
                MAX_TOTAL_BYTES >> 20,
                self.over_bytes
            ));
        }
        for p in &self.partial {
            out.push(p.clone());
        }
        for s in &self.skipped {
            out.push(s.clone());
        }
        out
    }
}

/// Dernière ligne horodatée (heure seule) d'un texte.
fn last_stamp(text: &str) -> Option<NaiveTime> {
    text.lines().rev().find_map(parse_stamp).map(|s| s.time)
}

impl Core {
    /// Dossier des journaux : celui demandé, sinon celui du jeu du serveur.
    pub(crate) async fn logs_dir(&self, id: i64, dir: Option<&str>) -> String {
        match dir.map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => d.to_string(),
            None => crate::games::log_dir(&self.server_family(id).await).to_string(),
        }
    }

    /// Journaux d'un dossier (débogage exclu sauf `include_debug`), du plus récent au plus ancien,
    /// et décalage horaire du serveur (fin du journal courant ↔ sa date de modification).
    pub(crate) async fn scan_logs(&self, id: i64, dir: &str, include_debug: bool) -> Result<LogDir> {
        let entries = self.sftp_list(id, dir).await.map_err(|e| {
            Error::Unexpected(format!("dossier des journaux « {dir} » illisible ({e}) — précise `dir`"))
        })?;
        let all: Vec<LogFile> = entries.iter().filter_map(LogFile::from_entry).collect();
        let listed = all.len();
        let mut files: Vec<LogFile> = all.into_iter().filter(|f| include_debug || !f.debug).collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.sort_key()));
        let mut measured = None;
        if let Some(f) = files.iter().find(|f| f.current && f.size > 0 && f.modified.is_some()) {
            let (path, size) = (f.path.clone(), f.size);
            let offset = size.saturating_sub(OFFSET_PROBE_BYTES);
            let len = (size - offset) as usize;
            let tail = self
                .with_sftp(id, |conn| {
                    let path = path.clone();
                    async move { sftp::read_range(&conn, &path, offset, len).await }
                })
                .await;
            if let (Ok(bytes), Some(m)) = (tail, f.modified) {
                measured = last_stamp(&String::from_utf8_lossy(&bytes)).map(|t| estimate_offset(t, m));
            }
        }
        Ok(LogDir {
            dir: dir.to_string(),
            files,
            listed,
            offset: measured.unwrap_or_else(|| crate::history::paris_offset(now_secs())),
            offset_measured: measured.is_some(),
        })
    }

    /// Télécharge un journal sous plafonds (`budget` : octets encore téléchargeables).
    pub(crate) async fn fetch_log(&self, id: i64, f: &LogFile, budget: u64) -> Fetched {
        let path = f.path.clone();
        if f.gz {
            if f.size > MAX_GZ_BYTES {
                return Fetched::Skipped(format!(
                    "{} ignoré : compressé, plus de {} Mio",
                    f.name,
                    MAX_GZ_BYTES >> 20
                ));
            }
            if f.size > budget {
                return Fetched::OverBudget;
            }
            let bytes = match self
                .with_sftp(id, |conn| {
                    let path = path.clone();
                    async move { sftp::read_bytes(&conn, &path, MAX_GZ_BYTES).await }
                })
                .await
            {
                Ok(b) => b,
                Err(e) => return Fetched::Skipped(format!("{} illisible : {e}", f.name)),
            };
            let downloaded = bytes.len() as u64;
            return match crate::archive::gunzip_capped(&bytes, MAX_UNPACKED_BYTES) {
                Ok((raw, cut)) => Fetched::Text {
                    text: String::from_utf8_lossy(&raw).into_owned(),
                    downloaded,
                    partial: cut.then_some("head"),
                },
                Err(e) => Fetched::Skipped(format!("{} illisible : {e}", f.name)),
            };
        }
        let want = f.size.min(MAX_PLAIN_BYTES);
        if want > budget {
            return Fetched::OverBudget;
        }
        let offset = f.size - want;
        // Petite marge : le journal courant a pu grandir depuis le listing.
        let len = (want + 64 * 1024) as usize;
        let bytes = match self
            .with_sftp(id, |conn| {
                let path = path.clone();
                async move { sftp::read_range(&conn, &path, offset, len).await }
            })
            .await
        {
            Ok(b) => b,
            Err(e) => return Fetched::Skipped(format!("{} illisible : {e}", f.name)),
        };
        let downloaded = bytes.len() as u64;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        let partial = (offset > 0).then_some("tail");
        if offset > 0 {
            // Première ligne coupée : écartée.
            let cut = text.find('\n').map_or(text.len(), |p| p + 1);
            text.drain(..cut);
        }
        Fetched::Text { text, downloaded, partial }
    }

    /// Pseudos connus d'un serveur Minecraft Java (liste des joueurs déjà venus), pour l'alias
    /// stable de `list_players`. Vide si la liste est indisponible (autre jeu, erreur).
    pub(crate) async fn roster_names(&self, id: i64) -> Names {
        let mut names = Names::default();
        if let Ok(roster) = self.player_roster(id, false).await {
            for p in &roster.players {
                if let Some(n) = &p.name {
                    names.add_uuid(n, &p.uuid);
                }
            }
        }
        names
    }

    /// Outil `search_logs` : voir [`SearchOptions`]. Sortie texte groupée par fichier.
    pub(crate) async fn search_logs(&self, id: i64, o: SearchOptions) -> Result<String> {
        let anonymize = self.get_privacy_config().redact_ai;
        let dir = self.logs_dir(id, o.dir.as_deref()).await;
        let scan = self.scan_logs(id, &dir, o.include_debug).await?;
        let now = scan.now();
        let window = Window {
            since: parse_bound(&o.since, now, false).map_err(Error::Invalid)?,
            until: parse_bound(&o.until, now, true).map_err(Error::Invalid)?,
        };
        let mut names = if anonymize { self.roster_names(id).await } else { Names::default() };
        let (re, notes) = compile_query(&o.query, o.regex, o.case_sensitive, &names).map_err(Error::Invalid)?;

        let mut plan: Vec<&LogFile> = scan
            .files
            .iter()
            .filter(|f| {
                let (start, end) = f.span(scan.offset);
                window.admits_file(start, end)
            })
            .collect();
        if !o.newest_first {
            plan.reverse();
        }
        let mut report = ReadReport::default();
        let mut left = o.max_results;
        let mut more_hits = 0usize;
        let mut blocks: Vec<Block> = Vec::new();
        for f in &plan {
            if left == 0 {
                report.after_results += 1;
                continue;
            }
            if report.read >= o.max_files {
                report.over_files += 1;
                continue;
            }
            let (text, partial) = match self.fetch_log(id, f, MAX_TOTAL_BYTES.saturating_sub(report.bytes)).await {
                Fetched::Text { text, downloaded, partial } => {
                    report.read += 1;
                    report.bytes += downloaded;
                    (text, partial)
                }
                Fetched::Skipped(why) => {
                    report.skipped.push(why);
                    continue;
                }
                Fetched::OverBudget => {
                    report.over_bytes += 1;
                    continue;
                }
            };
            note_partial(&mut report, f, partial);
            let lines: Vec<&str> = text.lines().collect();
            let stamps: Vec<Option<Stamp>> = lines.iter().map(|l| parse_stamp(l)).collect();
            let start = if partial == Some("tail") { None } else { f.name_date };
            let dates = date_lines(&stamps, start, f.last_write(scan.offset));
            if anonymize {
                for l in &lines {
                    names.discover(parse_header(l).message);
                }
            }
            let hits = matching_lines(&lines, &dates, &re, &window);
            let take = hits.len().min(left);
            more_hits += hits.len() - take;
            left -= take;
            let chosen = if o.newest_first { &hits[hits.len() - take..] } else { &hits[..take] };
            if !chosen.is_empty() {
                blocks.push(Block {
                    path: f.path.clone(),
                    day: f.name_date,
                    total: hits.len(),
                    rows: hit_rows(&lines, &dates, chosen, o.context),
                });
            }
        }

        // Rendu : anonymisation (secrets, IP, e-mails, pseudos) au dernier moment.
        let clean = |s: &str| -> String {
            let s = if anonymize { names.mask(s).into_owned() } else { s.to_string() };
            cut_chars(&self.redact_ai(&s), MAX_LINE_CHARS).into_owned()
        };
        let shown: usize = blocks.iter().map(|b| b.rows.iter().filter(|r| matches!(r, Row::Line { hit: true, .. })).count()).sum();
        let mut out = format!(
            "Recherche « {} » ({}, {}) dans {} — {} d'abord.\n",
            clean(&o.query),
            if o.regex { "expression régulière" } else { "texte" },
            if o.case_sensitive { "casse respectée" } else { "casse ignorée" },
            scan.dir,
            if o.newest_first { "plus récent" } else { "plus ancien" },
        );
        let _ = writeln!(
            out,
            "Dates : {} ; jour déduit du nom des fichiers et de l'horodatage des lignes.",
            scan.offset_note()
        );
        let debug = scan.listed - scan.files.len();
        let _ = writeln!(
            out,
            "{} fichier(s) de journal dans le dossier{}, {} dans la période, {} lu(s) ({} Kio). {} correspondance(s) affichée(s){}.",
            scan.files.len(),
            if debug > 0 { format!(" (+{debug} de débogage exclus : include_debug)") } else { String::new() },
            plan.len(),
            report.read,
            report.bytes / 1024,
            shown,
            if more_hits > 0 { format!(", {more_hits} autre(s) non affichée(s)") } else { String::new() }
        );
        if anonymize {
            out.push_str("Pseudos remplacés par des alias stables (joueur-xxxxxx, comme list_players) ; IP et secrets masqués.\n");
        }
        for n in &notes {
            let _ = writeln!(out, "Note : {n}.");
        }
        let mut truncated = false;
        for b in &blocks {
            if out.len() > MAX_OUTPUT_BYTES {
                truncated = true;
                break;
            }
            let day = b.day.map(|d| format!(" (commence le {d})")).unwrap_or_default();
            let _ = writeln!(out, "\n== {}{day} — {} correspondance(s)", b.path, b.total);
            for row in &b.rows {
                match row {
                    Row::Gap => out.push_str("  --\n"),
                    Row::Line { no, hit, day, text } => {
                        let mark = if *hit { ':' } else { '-' };
                        let day = day.map(|d| format!("{d} ")).unwrap_or_default();
                        let _ = writeln!(out, "{no:>7}{mark} {day}{}", clean(text));
                    }
                }
                if out.len() > MAX_OUTPUT_BYTES {
                    truncated = true;
                    break;
                }
            }
        }
        if blocks.is_empty() {
            out.push_str("\nAucune correspondance.\n");
        }
        let mut limits = report.limits(o.max_files);
        if more_hits > 0 || report.after_results > 0 {
            limits.insert(
                0,
                format!(
                    "plafond de résultats atteint ({}) : {more_hits} correspondance(s) non affichée(s), {} fichier(s) non lu(s) — affine le motif ou la période",
                    o.max_results, report.after_results
                ),
            );
        }
        if truncated {
            limits.push(format!("sortie tronquée à {} Kio — réduis max_results ou context", MAX_OUTPUT_BYTES >> 10));
        }
        if !limits.is_empty() {
            out.push_str("\nPlafonds et fichiers non lus :\n");
            for l in limits {
                let _ = writeln!(out, "- {l}");
            }
        }
        Ok(out)
    }

    /// Outil `error_digest` : voir [`DigestOptions`].
    pub(crate) async fn error_digest(&self, id: i64, o: DigestOptions) -> Result<Value> {
        let anonymize = self.get_privacy_config().redact_ai;
        let mut names = if anonymize { self.roster_names(id).await } else { Names::default() };
        let min = if o.include_warnings { Level::Warn } else { Level::Error };
        let mut digest = Digest::default();
        let mut sources: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut limits: Vec<String> = Vec::new();
        let mut window = Window::default();
        let mut time_note = String::from("heure du serveur");

        if o.source != DigestSource::Console {
            let dir = self.logs_dir(id, o.dir.as_deref()).await;
            match self.scan_logs(id, &dir, false).await {
                Ok(scan) => {
                    window.since = Some(scan.now() - Duration::hours(o.hours));
                    time_note = scan.offset_note();
                    let plan: Vec<&LogFile> = scan
                        .files
                        .iter()
                        .filter(|f| {
                            let (start, end) = f.span(scan.offset);
                            window.admits_file(start, end)
                        })
                        .collect();
                    let mut report = ReadReport::default();
                    for f in &plan {
                        if report.read >= o.max_files {
                            report.over_files += 1;
                            continue;
                        }
                        let (text, partial) = match self.fetch_log(id, f, MAX_TOTAL_BYTES.saturating_sub(report.bytes)).await {
                            Fetched::Text { text, downloaded, partial } => {
                                report.read += 1;
                                report.bytes += downloaded;
                                (text, partial)
                            }
                            Fetched::Skipped(why) => {
                                report.skipped.push(why);
                                continue;
                            }
                            Fetched::OverBudget => {
                                report.over_bytes += 1;
                                continue;
                            }
                        };
                        note_partial(&mut report, f, partial);
                        let lines: Vec<&str> = text.lines().collect();
                        let stamps: Vec<Option<Stamp>> = lines.iter().map(|l| parse_stamp(l)).collect();
                        let start = if partial == Some("tail") { None } else { f.name_date };
                        let dates = date_lines(&stamps, start, f.last_write(scan.offset));
                        for l in &lines {
                            names.discover(parse_header(l).message);
                        }
                        digest.add(&records(&lines, &dates), &f.path, min, &window, &names);
                        sources.push(f.path.clone());
                    }
                    limits = report.limits(o.max_files);
                    if plan.is_empty() {
                        notes.push(format!("aucun journal de la période dans {dir}"));
                    }
                }
                Err(e) if o.source == DigestSource::Auto => notes.push(e.to_string()),
                Err(e) => return Err(e),
            }
        }
        let console = o.source == DigestSource::Console || (o.source == DigestSource::Auto && sources.is_empty());
        if console {
            let lines: Vec<String> = self.console_logs(id).await?.iter().map(|l| clean_line(l)).collect();
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            for l in &refs {
                names.discover(parse_header(l).message);
            }
            // Console : heure seule, jour inconnu ; ce sont les dernières lignes, gardées en entier.
            let dates = vec![None; refs.len()];
            digest.add(&records(&refs, &dates), "console", min, &Window::default(), &names);
            sources.push(format!("console ({} dernières lignes)", refs.len()));
            notes.push("console : heures sans jour (lignes les plus récentes du serveur)".into());
        }

        let examined = digest.examined;
        let mut groups = digest.finish();
        let total = groups.len();
        groups.truncate(o.limit);
        // Vers un LLM : secrets, IP et e-mails masqués, pseudos remplacés par leur alias.
        for g in &mut groups {
            let mask = |s: &str| if anonymize { names.mask(s).into_owned() } else { s.to_string() };
            g.example = self.redact_ai(&mask(&g.example));
            g.signature = self.redact_ai(&mask(&g.signature));
        }
        let mut v = json!({
            "server_id": id,
            "window_hours": o.hours,
            "times": time_note,
            "sources": sources,
            "records_examined": examined,
            "groups_total": total,
            "groups_shown": groups.len(),
            "groups": groups,
        });
        if total > o.limit {
            limits.push(format!("{} groupe(s) de moindre gravité ou fréquence non affiché(s) (limit = {})", total - o.limit, o.limit));
        }
        if !limits.is_empty() {
            v["limits"] = json!(limits);
        }
        if !notes.is_empty() {
            v["notes"] = json!(notes);
        }
        if anonymize {
            v["names_anonymized"] = json!(true);
        }
        Ok(v)
    }
}

/// Correspondances d'un fichier, à rendre.
struct Block {
    path: String,
    day: Option<NaiveDate>,
    total: usize,
    rows: Vec<Row>,
}

/// Note un fichier lu en partie.
fn note_partial(report: &mut ReadReport, f: &LogFile, partial: Option<&'static str>) {
    match partial {
        Some("tail") => report.partial.push(format!(
            "{} lu en partie : ses {} derniers Mio seulement",
            f.name,
            MAX_PLAIN_BYTES >> 20
        )),
        Some(_) => report.partial.push(format!(
            "{} décompressé en partie : ses {} premiers Mio seulement",
            f.name,
            MAX_UNPACKED_BYTES >> 20
        )),
        None => {}
    }
}

/// Paramètres de `search_logs`.
#[derive(Debug, Clone)]
pub(crate) struct SearchOptions {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub context: usize,
    pub since: String,
    pub until: String,
    pub newest_first: bool,
    pub dir: Option<String>,
    pub include_debug: bool,
    pub max_results: usize,
    pub max_files: usize,
}

/// Entier optionnel d'un argument MCP, borné.
fn arg_usize(args: &Value, key: &str, (default, max): (usize, usize)) -> usize {
    args.get(key).and_then(Value::as_i64).map_or(default, |n| n.clamp(1, max as i64) as usize)
}

fn arg_flag(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn arg_text(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

impl SearchOptions {
    /// Arguments de l'outil MCP `search_logs` (lecture tolérante, bornes appliquées).
    pub(crate) fn from_args(args: &Value) -> std::result::Result<Self, String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or("paramètre `query` (texte) requis")?
            .to_string();
        let order = arg_text(args, "order").to_ascii_lowercase();
        if !matches!(order.as_str(), "" | "newest" | "oldest") {
            return Err(format!("`order` inconnu : « {order} » (newest ou oldest)"));
        }
        Ok(Self {
            query,
            regex: arg_flag(args, "regex", false),
            case_sensitive: arg_flag(args, "case_sensitive", false),
            context: args.get("context").and_then(Value::as_i64).unwrap_or(0).clamp(0, MAX_CONTEXT as i64) as usize,
            since: arg_text(args, "since"),
            until: arg_text(args, "until"),
            newest_first: order != "oldest",
            dir: Some(arg_text(args, "dir")).filter(|d| !d.is_empty()),
            include_debug: arg_flag(args, "include_debug", false),
            max_results: arg_usize(args, "max_results", SEARCH_RESULTS),
            max_files: arg_usize(args, "max_files", SEARCH_FILES),
        })
    }
}

impl DigestOptions {
    /// Arguments de l'outil MCP `error_digest` (lecture tolérante, bornes appliquées).
    pub(crate) fn from_args(args: &Value) -> std::result::Result<Self, String> {
        let source = match arg_text(args, "source").to_ascii_lowercase().as_str() {
            "" | "auto" => DigestSource::Auto,
            "logs" | "journaux" => DigestSource::Logs,
            "console" => DigestSource::Console,
            other => return Err(format!("`source` inconnue : « {other} » (auto, logs ou console)")),
        };
        Ok(Self {
            source,
            hours: args.get("since_hours").and_then(Value::as_i64).unwrap_or(24).clamp(1, DIGEST_MAX_HOURS),
            include_warnings: arg_flag(args, "include_warnings", true),
            limit: arg_usize(args, "limit", DIGEST_GROUPS),
            dir: Some(arg_text(args, "dir")).filter(|d| !d.is_empty()),
            max_files: arg_usize(args, "max_files", DIGEST_FILES),
        })
    }
}

/// Source de `error_digest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DigestSource {
    /// Journaux s'il y en a dans la période, sinon la console.
    Auto,
    Logs,
    Console,
}

/// Paramètres de `error_digest`.
#[derive(Debug, Clone)]
pub(crate) struct DigestOptions {
    pub source: DigestSource,
    pub hours: i64,
    pub include_warnings: bool,
    pub limit: usize,
    pub dir: Option<String>,
    pub max_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64, modified: Option<i64>) -> SftpEntry {
        SftpEntry { name: name.into(), path: format!("/logs/{name}"), is_dir: false, size, modified }
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn t(s: &str) -> NaiveTime {
        NaiveTime::parse_from_str(s, "%H:%M:%S").unwrap()
    }

    /// Lignes datées comme le ferait la recherche (`start` : jour du nom de fichier).
    fn parse(text: &str, start: Option<NaiveDate>) -> (Vec<&str>, Vec<Option<NaiveDateTime>>) {
        let lines: Vec<&str> = text.lines().collect();
        let stamps: Vec<Option<Stamp>> = lines.iter().map(|l| parse_stamp(l)).collect();
        let dates = date_lines(&stamps, start, None);
        (lines, dates)
    }

    // --- Fichiers ---

    #[test]
    fn classifies_minecraft_and_other_log_files() {
        let latest = LogFile::from_entry(&entry("latest.log", 410, Some(100))).unwrap();
        assert!(latest.current && !latest.gz && latest.name_date.is_none());
        let arch = LogFile::from_entry(&entry("2026-09-05-6.log.gz", 10_269, Some(50))).unwrap();
        assert!(arch.gz && !arch.current);
        assert_eq!((arch.name_date, arch.index), (Some(d("2026-09-05")), 6));
        let ue = LogFile::from_entry(&entry("FactoryGame-backup-2026.11.17-21.33.55.log", 1, None)).unwrap();
        assert_eq!((ue.name_date, ue.index, ue.current), (Some(d("2026-11-17")), 0, false));
        let prev = LogFile::from_entry(&entry("factorio-previous.log", 1, Some(1))).unwrap();
        assert!(!prev.current && prev.name_date.is_none());
        let dbg = LogFile::from_entry(&entry("debug-3.log.gz", 1, Some(1))).unwrap();
        assert!(dbg.debug && !dbg.current);
        assert!(LogFile::from_entry(&entry("server.properties", 1, None)).is_none());
        assert!(LogFile::from_entry(&entry("crash.txt", 1, None)).is_none());
        let mut dir = entry("old.log", 0, None);
        dir.is_dir = true;
        assert!(LogFile::from_entry(&dir).is_none());
        // Pas de date collée à d'autres chiffres.
        assert_eq!(date_in_name("12026-01-01"), None);
        assert_eq!(date_in_name("x_2026.13.01"), None, "mois invalide");
    }

    #[test]
    fn files_sort_newest_first() {
        let mut files: Vec<LogFile> = ["2026-09-05-1.log.gz", "latest.log", "2026-09-06-1.log.gz", "2026-09-05-10.log.gz", "2026-09-05-2.log.gz"]
            .iter()
            .map(|n| LogFile::from_entry(&entry(n, 1, Some(1))).unwrap())
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.sort_key()));
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["latest.log", "2026-09-06-1.log.gz", "2026-09-05-10.log.gz", "2026-09-05-2.log.gz", "2026-09-05-1.log.gz"]);
    }

    #[test]
    fn window_selects_files_and_lines() {
        let now = dt("2026-11-18 19:00:00");
        let w = Window {
            since: parse_bound("2026-11-17", now, false).unwrap(),
            until: parse_bound("2026-11-17", now, true).unwrap(),
        };
        assert_eq!(w.since, Some(dt("2026-11-17 00:00:00")));
        assert_eq!(w.until, Some(dt("2026-11-17 23:59:59")));
        let old = LogFile::from_entry(&entry("2026-11-10-1.log.gz", 1, Some(dt("2026-11-10 22:00:00").and_utc().timestamp()))).unwrap();
        let (s, e) = old.span(0);
        assert!(!w.admits_file(s, e), "fini une semaine avant");
        let later = LogFile::from_entry(&entry("2026-11-18-1.log.gz", 1, None)).unwrap();
        let (s, e) = later.span(0);
        assert!(!w.admits_file(s, e), "commence après");
        let latest = LogFile::from_entry(&entry("latest.log", 1, Some(now.and_utc().timestamp()))).unwrap();
        let (s, e) = latest.span(0);
        assert!(w.admits_file(s, e), "courant : début inconnu");
        assert!(w.admits(Some(dt("2026-11-17 12:00:00"))));
        assert!(!w.admits(Some(dt("2026-11-18 00:00:01"))));
        assert!(w.admits(None));
        // Relatif.
        assert_eq!(parse_bound("12h", now, false).unwrap(), Some(dt("2026-11-18 07:00:00")));
        assert_eq!(parse_bound("7j", now, false).unwrap(), Some(dt("2026-11-11 19:00:00")));
        assert_eq!(parse_bound("2026-11-17T08:30", now, false).unwrap(), Some(dt("2026-11-17 08:30:00")));
        assert_eq!(parse_bound("", now, false).unwrap(), None);
        assert!(parse_bound("hier", now, false).is_err());
    }

    // --- Horodatages ---

    #[test]
    fn parses_common_stamps() {
        let s = parse_stamp("[04:40:05] [Server thread/INFO]: vb_a6ff74 joined the game").unwrap();
        assert_eq!((s.date, s.time, s.len), (None, t("04:40:05"), 10));
        let s = parse_stamp("[12:34:56 ERROR]: Could not pass event").unwrap();
        assert_eq!((s.time, s.len), (t("12:34:56"), 16));
        let s = parse_stamp("[17Nov2026 21:03:12.345] [Server thread/INFO] [net.minecraft.server.MinecraftServer/]: x").unwrap();
        assert_eq!((s.date, s.time), (Some(d("2026-11-17")), t("21:03:12")));
        let s = parse_stamp("[2026.11.17-21.33.55:123][  0]LogNet: Warning: x").unwrap();
        assert_eq!((s.date, s.time, s.len), (Some(d("2026-11-17")), t("21:33:55"), 25));
        let s = parse_stamp("2013-01-01 12:00:00 [SEVERE] Could not load").unwrap();
        assert_eq!((s.date, s.time, s.len), (Some(d("2013-01-01")), t("12:00:00"), 19));
        let s = parse_stamp("[2026-11-17T08:00:00.123Z] [main] x").unwrap();
        assert_eq!((s.date, s.len), (Some(d("2026-11-17")), 26));
        assert!(parse_stamp("\tat com.example.Foo.bar(Foo.java:1)").is_none());
        assert!(parse_stamp("[Server thread/INFO]: x").is_none());
        assert!(parse_stamp("[25:00:00] x").is_none());
    }

    #[test]
    fn parses_headers_of_each_platform() {
        let h = parse_header("[05:34:42] [Folia Region Scheduler Thread #0/WARN]: [PvpFactionZaap] PANNE RÉSEAU GLOBALE!");
        assert_eq!(h.level, Some((Level::Warn, "WARN")));
        assert_eq!(h.message, "[PvpFactionZaap] PANNE RÉSEAU GLOBALE!");
        let h = parse_header("[12:34:56 ERROR]: Could not pass event PlayerJoinEvent to Essentials v2.20.1");
        assert_eq!(h.level, Some((Level::Error, "ERROR")));
        assert!(h.message.starts_with("Could not pass"));
        let h = parse_header("[17Nov2026 21:03:12.345] [Server thread/ERROR] [net.minecraft.server.MinecraftServer/]: Encountered an unexpected exception");
        assert_eq!(h.level, Some((Level::Error, "ERROR")));
        assert_eq!(h.logger, Some("net.minecraft.server.MinecraftServer/"));
        assert_eq!(h.message, "Encountered an unexpected exception");
        let h = parse_header("[12:00:00] [Server thread/INFO] (sodium) Loaded configuration file");
        assert_eq!((h.logger, h.message), (Some("sodium"), "Loaded configuration file"));
        let h = parse_header("[2026.11.17-21.33.55:123][  0]LogNet: Warning: Network failure");
        assert_eq!(h.level, Some((Level::Warn, "WARN")));
        let h = parse_header("2013-01-01 12:00:00 [SEVERE] Could not load 'plugins/Foo.jar'");
        assert_eq!(h.level, Some((Level::Error, "SEVERE")));
        assert_eq!(h.message, "Could not load 'plugins/Foo.jar'");
        // Le mot ERROR dans un message INFO ne fait pas une erreur.
        let h = parse_header("[10:00:00] [Server thread/INFO]: [Shop] ERROR count = 0");
        assert_eq!(h.level, Some((Level::Info, "INFO")));
        let h = parse_header("\tat com.example.Foo.bar(Foo.java:1)");
        assert_eq!((h.stamp, h.level), (None, None));
    }

    #[test]
    fn dates_follow_midnight_forward_and_backward() {
        let text = "[23:58:00] [Server thread/INFO]: a\n\tat x.y.Z.w(Z.java:1)\n[23:59:59] [Server thread/INFO]: b\n[00:00:02] [Server thread/INFO]: c\n[23:59:58] [Async/INFO]: tardive\n[00:10:00] [Server thread/INFO]: d";
        let (_, dates) = parse(text, Some(d("2026-09-05")));
        assert_eq!(dates[0], Some(dt("2026-09-05 23:58:00")));
        assert_eq!(dates[1], dates[0], "continuation : date de la ligne précédente");
        assert_eq!(dates[3], Some(dt("2026-09-06 00:00:02")));
        // Une ligne légèrement en retard (autre thread) reste à la veille sans inventer de jour.
        assert_eq!(dates[4], Some(dt("2026-09-05 23:59:58")));
        assert_eq!(dates[5], Some(dt("2026-09-06 00:10:00")));
        // latest.log : on remonte depuis la dernière écriture.
        let lines: Vec<&str> = text.lines().collect();
        let stamps: Vec<Option<Stamp>> = lines.iter().map(|l| parse_stamp(l)).collect();
        let back = date_lines(&stamps, None, Some(dt("2026-11-18 00:10:05")));
        assert_eq!(back[5], Some(dt("2026-11-18 00:10:00")));
        assert_eq!(back[3], Some(dt("2026-11-18 00:00:02")));
        assert_eq!(back[2], Some(dt("2026-11-17 23:59:59")));
        assert_eq!(back[0], Some(dt("2026-11-17 23:58:00")));
        // Dernière ligne de la veille au soir, fichier modifié juste après minuit.
        let late = date_lines(&[parse_stamp("[23:50:00] x")], None, Some(dt("2026-11-18 00:05:00")));
        assert_eq!(late[0], Some(dt("2026-11-17 23:50:00")));
        // Ni jour ni fin : heure seule, non datée ; formats à date complète datés quand même.
        assert_eq!(date_lines(&[parse_stamp("[10:00:00] x")], None, None), vec![None]);
        let forge = date_lines(&[parse_stamp("[17Nov2026 21:03:12.345] [main/INFO] [x/]: y")], None, None);
        assert_eq!(forge, vec![Some(dt("2026-11-17 21:03:12"))]);
    }

    #[test]
    fn offset_is_deduced_from_the_last_line() {
        // Dernière ligne [19:08:34] écrite à 18:08:34 UTC : serveur à l'heure de Paris (hiver).
        let m = dt("2026-11-18 18:08:34").and_utc().timestamp();
        assert_eq!(estimate_offset(t("19:08:34"), m), 3600);
        // Autour de minuit UTC.
        let m = dt("2026-11-18 23:30:00").and_utc().timestamp();
        assert_eq!(estimate_offset(t("01:30:10"), m), 7200);
        assert_eq!(estimate_offset(t("18:29:50"), m), -5 * 3600);
        assert_eq!(offset_label(3600), "UTC+01:00");
        assert_eq!(offset_label(-16_200), "UTC-04:30");
    }

    // --- Arguments des outils ---

    #[test]
    fn tool_arguments_are_bounded() {
        let o = SearchOptions::from_args(&json!({ "query": "ban", "context": 99, "max_results": 10_000, "max_files": 0, "order": "OLDEST", "dir": " " })).unwrap();
        assert_eq!((o.context, o.max_results, o.max_files), (MAX_CONTEXT, SEARCH_RESULTS.1, 1));
        assert!(!o.newest_first && o.dir.is_none() && !o.regex && !o.case_sensitive);
        let o = SearchOptions::from_args(&json!({ "query": "x" })).unwrap();
        assert_eq!((o.max_results, o.max_files, o.context), (SEARCH_RESULTS.0, SEARCH_FILES.0, 0));
        assert!(o.newest_first);
        assert!(SearchOptions::from_args(&json!({})).is_err());
        assert!(SearchOptions::from_args(&json!({ "query": "x", "order": "random" })).is_err());
        let d = DigestOptions::from_args(&json!({ "since_hours": 100_000, "source": "Console" })).unwrap();
        assert_eq!((d.hours, d.source, d.limit, d.max_files), (DIGEST_MAX_HOURS, DigestSource::Console, DIGEST_GROUPS.0, DIGEST_FILES.0));
        assert!(d.include_warnings);
        let d = DigestOptions::from_args(&json!({ "include_warnings": false, "dir": "/FactoryGame/Saved/Logs" })).unwrap();
        assert_eq!((d.source, d.include_warnings, d.dir.as_deref()), (DigestSource::Auto, false, Some("/FactoryGame/Saved/Logs")));
        assert!(DigestOptions::from_args(&json!({ "source": "disque" })).is_err());
    }

    // --- Recherche ---

    #[test]
    fn search_query_literal_regex_case_and_aliases() {
        let mut names = Names::default();
        names.add_uuid("Steve", "069a79f4-44e9-4726-a5be-fca90e38aaf5");
        let alias = pseudonym("069a79f4-44e9-4726-a5be-fca90e38aaf5");
        let (re, notes) = compile_query("banned a.b", false, false, &names).unwrap();
        assert!(re.is_match("[x]: BANNED A.B") && !re.is_match("banned axb"), "littéral, casse ignorée");
        assert!(notes.is_empty());
        let (re, _) = compile_query("Banned", false, true, &names).unwrap();
        assert!(!re.is_match("banned"));
        let (re, _) = compile_query(r"lost connection: (Timed out|Disconnected)", true, false, &names).unwrap();
        assert!(re.is_match("Steve lost connection: Timed out"));
        // L'agent ne voit que l'alias : il est traduit en pseudo réel pour chercher.
        let (re, _) = compile_query(&format!("Banned {alias}"), false, false, &names).unwrap();
        assert!(re.is_match("[Rcon: Banned Steve: Griefing]"));
        let (_, notes) = compile_query("joueur-abcdef joined", false, false, &names).unwrap();
        assert_eq!(notes.len(), 1, "alias inconnu signalé");
        assert!(compile_query("(", true, false, &names).is_err());
        assert!(compile_query(" ", false, false, &names).is_err());
        assert!(compile_query(&"a".repeat(MAX_QUERY_LEN + 1), false, false, &names).is_err());
    }

    #[test]
    fn hits_render_with_context_and_gaps() {
        let text = "[10:00:00] [Server thread/INFO]: a\n[10:00:01] [Server thread/INFO]: Steve joined the game\n[10:00:02] [Server thread/INFO]: b\n[10:00:03] [Server thread/INFO]: c\n[10:00:04] [Server thread/INFO]: d\n[10:00:05] [Server thread/INFO]: Steve left the game\n[10:00:06] [Server thread/INFO]: e";
        let (lines, dates) = parse(text, Some(d("2026-11-17")));
        let (re, _) = compile_query("steve", false, false, &Names::default()).unwrap();
        let w = Window { since: Some(dt("2026-11-17 10:00:00")), until: None };
        let hits = matching_lines(&lines, &dates, &re, &w);
        assert_eq!(hits, vec![1, 5]);
        let rows = hit_rows(&lines, &dates, &hits, 1);
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Gap => "--".into(),
                Row::Line { no, hit, .. } => format!("{no}{}", if *hit { ':' } else { '-' }),
            })
            .collect();
        assert_eq!(shape, ["1-", "2:", "3-", "--", "5-", "6:", "7-"]);
        // Contextes qui se touchent : pas de séparateur ni de doublon.
        let rows = hit_rows(&lines, &dates, &hits, 2);
        assert!(!rows.contains(&Row::Gap));
        assert_eq!(rows.len(), 7);
        let Row::Line { day, .. } = &rows[1] else { panic!() };
        assert_eq!(*day, Some(d("2026-11-17")));
        // Fenêtre : rien avant 10:00:03.
        let w = Window { since: Some(dt("2026-11-17 10:00:03")), until: None };
        assert_eq!(matching_lines(&lines, &dates, &re, &w), vec![5]);
    }

    #[test]
    fn names_are_discovered_and_masked() {
        let mut names = Names::default();
        for line in [
            "[04:40:04] [User Authenticator #1/INFO]: UUID of player vb_a6ff74 is 2d75beb0-7011-3785-b332-13534c25dcc8",
            "[04:40:05] [Server thread/INFO]: vb_a6ff74[/46.225.215.80:54526] logged in with entity id 234319",
            "[04:41:00] [Server thread/INFO]: Alex joined the game",
            "[04:41:10] [Async Chat Thread - #0/INFO]: <Alex> salut",
            "[04:42:00] [Server thread/INFO]: [Rcon: Banned Griefer_1: Griefing]",
        ] {
            names.discover(parse_header(line).message);
        }
        let alias = pseudonym("2d75beb0-7011-3785-b332-13534c25dcc8");
        let masked = names.mask("vb_a6ff74 lost connection; alex and Griefer_1 too; vb_a6ff74x stays");
        assert!(masked.starts_with(&alias), "{masked}");
        assert!(!masked.contains("vb_a6ff74 ") && !masked.to_lowercase().contains("alex") && !masked.contains("Griefer_1"), "{masked}");
        assert!(masked.contains("vb_a6ff74x"), "mot entier seulement : {masked}");
        // L'UUID d'un joueur permettrait de retrouver son pseudo : remplacé aussi.
        let masked = names.mask("UUID of player vb_a6ff74 is 2d75beb0-7011-3785-b332-13534c25dcc8, job 2a7caa77-5efd-4af3-a33f-0e7ec8c4fda2");
        assert_eq!(masked, format!("UUID of player {alias} is {alias}, job [UUID]"));
        // Le même joueur garde l'alias de list_players (UUID, avec ou sans tirets).
        let mut roster = Names::default();
        roster.add_uuid("vb_a6ff74", "2d75beb070113785b33213534c25dcc8");
        assert_eq!(roster.mask("vb_a6ff74"), alias);
        assert_eq!(roster.mask("2D75BEB0-7011-3785-B332-13534C25DCC8."), format!("{alias}."));
        assert_eq!(Names::default().mask("Alex"), "Alex");
        assert_eq!(uuid_spans("x2d75beb0-7011-3785-b332-13534c25dcc8"), vec![], "collé à un mot");
    }

    // --- Digest : extraits au format réel ---

    /// Paper (fichier) : événement qui plante dans un plugin, frames suffixées du jar.
    const PAPER: &str = "\
[12:30:01] [Server thread/INFO]: Steve joined the game
[12:30:01] [Server thread/ERROR]: Could not pass event PlayerJoinEvent to Essentials v2.20.1
org.bukkit.event.EventException: null
\tat org.bukkit.plugin.java.JavaPluginLoader$1.execute(JavaPluginLoader.java:306) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:?]
\tat co.aikar.timings.TimedEventExecutor.execute(TimedEventExecutor.java:80) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:git-Paper-496]
\tat org.bukkit.plugin.RegisteredListener.callEvent(RegisteredListener.java:70) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:?]
Caused by: java.lang.NullPointerException: Cannot invoke \"String.length()\" because \"s\" is null
\tat com.earth2me.essentials.EssentialsPlayerListener.onPlayerJoin(EssentialsPlayerListener.java:123) ~[EssentialsX-2.20.1.jar:?]
\tat com.earth2me.essentials.EssentialsPlayerListener$$Lambda$4521/0x0000000801a2b3c4.run(Unknown Source) ~[EssentialsX-2.20.1.jar:?]
\tat org.bukkit.plugin.java.JavaPluginLoader$1.execute(JavaPluginLoader.java:302) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:?]
\t... 12 more
[12:45:10] [Server thread/INFO]: Alex joined the game
[12:45:10] [Server thread/ERROR]: Could not pass event PlayerJoinEvent to Essentials v2.20.1
org.bukkit.event.EventException: null
\tat org.bukkit.plugin.java.JavaPluginLoader$1.execute(JavaPluginLoader.java:306) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:?]
Caused by: java.lang.NullPointerException: Cannot invoke \"String.length()\" because \"s\" is null
\tat com.earth2me.essentials.EssentialsPlayerListener.onPlayerJoin(EssentialsPlayerListener.java:124) ~[EssentialsX-2.20.1.jar:?]
\tat com.earth2me.essentials.EssentialsPlayerListener$$Lambda$4522/0x0000000801a2ffff.run(Unknown Source) ~[EssentialsX-2.20.1.jar:?]
\t... 12 more
[12:50:00] [Server thread/ERROR]: Error occurred while enabling WorldGuard v7.0.9+5934e49 (Is it up to date?)
java.lang.NoClassDefFoundError: com/sk89q/worldedit/WorldEdit
\tat com.sk89q.worldguard.bukkit.WorldGuardPlugin.onEnable(WorldGuardPlugin.java:150) ~[worldguard-bukkit-7.0.9-dist.jar:?]
\tat org.bukkit.plugin.java.JavaPlugin.setEnabled(JavaPlugin.java:281) ~[paper-api-1.20.4-R0.1-SNAPSHOT.jar:?]
[12:55:00] [Folia Async Scheduler Thread #32/WARN]: [VoxelBench] [RemoteMonitoring] Send error: Connection reset. Data kept for next cycle.
[13:05:00] [Folia Async Scheduler Thread #31/WARN]: [VoxelBench] [RemoteMonitoring] Send error: Connection reset. Data kept for next cycle.
[13:06:00] [Server thread/WARN]: Can't keep up! Is the server overloaded? Running 5123ms or 102 ticks behind
[13:07:00] [Server thread/WARN]: Can't keep up! Is the server overloaded? Running 2087ms or 41 ticks behind
[13:08:00] [Server thread/INFO]: Alex lost connection: Disconnected
";

    /// Spigot : pile écrite sur System.err, chaque frame préfixée de l'en-tête ; tâche planifiée.
    const SPIGOT: &str = "\
[08:00:00 WARN]: [Shop] Plugin Shop v1.4.2 generated an exception while executing task 1234
[08:00:00 WARN]: java.lang.IllegalStateException: Asynchronous getEntities call!
[08:00:00 WARN]: \tat org.spigotmc.AsyncCatcher.catchOp(AsyncCatcher.java:15)
[08:00:00 WARN]: \tat org.bukkit.craftbukkit.v1_20_R3.CraftWorld.getEntities(CraftWorld.java:1021)
[08:00:00 WARN]: \tat fr.example.shop.tasks.CleanupTask.run(CleanupTask.java:42)
[08:00:00 WARN]: \tat fr.example.shop.tasks.CleanupTask.lambda$start$0(CleanupTask.java:20)
2013-01-01 12:00:00 [SEVERE] Could not load 'plugins/OldThing-1.0.jar' in folder 'plugins'
";

    /// Fabric : exception dans un mod (intermediary + mixin).
    const FABRIC: &str = "\
[14:00:00] [Server thread/INFO]: Starting minecraft server version 1.20.1
[14:20:00] [Server thread/ERROR]: Encountered an unexpected exception
net.minecraft.class_148: Ticking entity
\tat net.minecraft.server.MinecraftServer.method_3813(MinecraftServer.java:892) ~[server-intermediary.jar:?]
Caused by: java.lang.ArrayIndexOutOfBoundsException: Index 5 out of bounds for length 5
\tat net.minecraft.class_1297.handler$zza000$examplemod$onTick(class_1297.java:42) ~[server-intermediary.jar:?]
\tat com.example.examplemod.util.Ring.get(Ring.java:31) ~[examplemod-1.2.0+1.20.1.jar:?]
\tat com.example.examplemod.mixin.EntityMixin.onTick(EntityMixin.java:18) ~[examplemod-1.2.0+1.20.1.jar:?]
\t... 9 more
";

    /// Forge : modules `TRANSFORMER/modid@version/`, logger en troisième groupe, échec de mod.
    const FORGE: &str = "\
[17Nov2026 21:03:12.345] [Server thread/INFO] [net.minecraft.server.dedicated.DedicatedServer/]: Starting minecraft server version 1.20.1
[17Nov2026 21:05:00.001] [Server thread/ERROR] [net.minecraft.server.MinecraftServer/]: Encountered an unexpected exception
net.minecraft.ReportedException: Ticking block entity
\tat TRANSFORMER/minecraft@1.20.1/net.minecraft.server.MinecraftServer.m_5703_(MinecraftServer.java:828) ~[forge-1.20.1-47.2.0-universal.jar%23188!/:?] {re:mixin,pl:accesstransformer:B}
Caused by: java.lang.NullPointerException: Cannot read field \"level\" because \"this.target\" is null
\tat TRANSFORMER/create@0.5.1.f/com.simibubi.create.content.kinetics.belt.BeltBlockEntity.tick(BeltBlockEntity.java:120) ~[create-1.20.1-0.5.1.f.jar%23200!/:0.5.1.f] {re:classloading}
\tat TRANSFORMER/minecraft@1.20.1/net.minecraft.world.level.Level.m_46653_(Level.java:479) ~[server-1.20.1-20230612.114412-srg.jar%23195!/:?] {re:mixin}
\t... 7 more
[17Nov2026 21:06:00.000] [main/ERROR] [net.minecraftforge.fml.javafmlmod.FMLModContainer/LOADING]: Failed to create mod instance. ModID: badmod, class com.example.badmod.BadMod
[17Nov2026 21:07:00.000] [Server thread/WARN] [com.simibubi.create.Create/]: Belt network desync
";

    fn digest_of(text: &str, start: Option<NaiveDate>, min: Level) -> Vec<ErrorGroup> {
        let (lines, dates) = parse(text, start);
        let mut names = Names::default();
        for l in &lines {
            names.discover(parse_header(l).message);
        }
        let mut dg = Digest::default();
        dg.add(&records(&lines, &dates), "/logs/latest.log", min, &Window::default(), &names);
        dg.finish()
    }

    fn group<'a>(groups: &'a [ErrorGroup], needle: &str) -> &'a ErrorGroup {
        groups
            .iter()
            .find(|g| g.signature.contains(needle))
            .unwrap_or_else(|| panic!("{needle} absent de {:#?}", groups.iter().map(|g| &g.signature).collect::<Vec<_>>()))
    }

    #[test]
    fn records_group_stack_traces() {
        let (lines, dates) = parse(PAPER, Some(d("2026-11-17")));
        let recs = records(&lines, &dates);
        let ev = recs.iter().find(|r| r.message.starts_with("Could not pass")).unwrap();
        assert_eq!(ev.line_no, 2);
        assert_eq!(ev.body.len(), 9);
        assert_eq!(ev.ts, Some(dt("2026-11-17 12:30:01")));
        let chain = thrown(ev);
        assert_eq!(chain.iter().map(|t| t.class).collect::<Vec<_>>(), ["org.bukkit.event.EventException", "java.lang.NullPointerException"]);
        assert_eq!(chain[1].frames.len(), 3);
        assert_eq!(chain[1].frames[0].jar, Some("EssentialsX-2.20.1.jar"));
        // Spigot : les frames préfixées rejoignent l'exception.
        let (lines, dates) = parse(SPIGOT, None);
        let recs = records(&lines, &dates);
        assert_eq!(recs.len(), 3, "{recs:#?}");
        assert_eq!(recs[1].body.len(), 4);
    }

    #[test]
    fn paper_digest() {
        let groups = digest_of(PAPER, Some(d("2026-11-17")), Level::Warn);
        let npe = group(&groups, "NullPointerException");
        // Numéros de ligne, lambdas et adresses de classes cachées ne séparent pas les occurrences.
        assert_eq!(npe.count, 2, "{npe:#?}");
        assert_eq!(npe.signature, "java.lang.NullPointerException @ com.earth2me.essentials.EssentialsPlayerListener.onPlayerJoin < com.earth2me.essentials.EssentialsPlayerListener$$Lambda$#.run");
        assert_eq!(npe.level, "ERROR");
        assert_eq!(npe.first.as_deref(), Some("2026-11-17 12:30:01"));
        assert_eq!(npe.last.as_deref(), Some("2026-11-17 12:45:10"));
        assert_eq!(npe.probable_source, Some(Culprit { name: "Essentials".into(), evidence: "event" }));
        assert!(npe.example.starts_with("Could not pass event PlayerJoinEvent"), "{}", npe.example);
        let wg = group(&groups, "NoClassDefFoundError");
        assert_eq!(wg.probable_source, Some(Culprit { name: "WorldGuard".into(), evidence: "enable" }));
        let vb = group(&groups, "Send error");
        assert_eq!(vb.count, 2);
        assert_eq!(vb.level, "WARN");
        assert_eq!(vb.probable_source, Some(Culprit { name: "VoxelBench".into(), evidence: "prefix" }));
        let lag = group(&groups, "Can't keep up");
        assert_eq!(lag.count, 2, "nombres retirés : {}", lag.signature);
        assert_eq!(lag.signature, "Can't keep up! Is the server overloaded? Running #ms or # ticks behind");
        assert_eq!(lag.probable_source, None);
        // Tri : erreurs d'abord (les plus fréquentes en tête), avertissements ensuite ; INFO exclu.
        assert_eq!(groups[0].signature, npe.signature);
        assert!(groups.iter().all(|g| g.level != "INFO"));
        assert!(!groups.iter().any(|g| g.signature.contains("lost connection")));
        // Sans avertissements.
        let errors = digest_of(PAPER, Some(d("2026-11-17")), Level::Error);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn spigot_digest() {
        let groups = digest_of(SPIGOT, None, Level::Warn);
        let task = group(&groups, "Plugin Shop");
        assert_eq!(task.probable_source, Some(Culprit { name: "Shop".into(), evidence: "task" }));
        let ise = group(&groups, "IllegalStateException");
        assert_eq!(ise.signature, "java.lang.IllegalStateException @ fr.example.shop.tasks.CleanupTask.run < fr.example.shop.tasks.CleanupTask.lambda$start$#");
        assert_eq!(ise.probable_source, Some(Culprit { name: "fr.example.shop".into(), evidence: "package" }));
        let old = group(&groups, "Could not load");
        assert_eq!(old.level, "SEVERE");
        assert_eq!(old.first.as_deref(), Some("2013-01-01 12:00:00"));
        assert_eq!(old.probable_source, Some(Culprit { name: "OldThing".into(), evidence: "load" }));
        assert_eq!(old.signature, "Could not load 'plugins/OldThing-#.jar' in folder 'plugins'");
    }

    #[test]
    fn fabric_digest() {
        let groups = digest_of(FABRIC, Some(d("2026-11-17")), Level::Warn);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.signature, "java.lang.ArrayIndexOutOfBoundsException @ com.example.examplemod.util.Ring.get < com.example.examplemod.mixin.EntityMixin.onTick");
        assert_eq!(g.probable_source, Some(Culprit { name: "examplemod".into(), evidence: "jar" }));
        assert_eq!(normalize_frame("net.minecraft.class_1297.handler$zza000$examplemod$onTick"), "net.minecraft.class_1297.handler$examplemod$onTick");
    }

    #[test]
    fn forge_digest() {
        let groups = digest_of(FORGE, None, Level::Warn);
        let npe = group(&groups, "NullPointerException");
        assert_eq!(npe.signature, "java.lang.NullPointerException @ com.simibubi.create.content.kinetics.belt.BeltBlockEntity.tick");
        assert_eq!(npe.probable_source, Some(Culprit { name: "create".into(), evidence: "mod" }));
        assert_eq!(npe.first.as_deref(), Some("2026-11-17 21:05:00"), "date Forge lue dans la ligne");
        let bad = group(&groups, "Failed to create mod instance");
        assert_eq!(bad.probable_source, Some(Culprit { name: "badmod".into(), evidence: "mod" }));
        let desync = group(&groups, "Belt network desync");
        assert_eq!(desync.probable_source, Some(Culprit { name: "com.simibubi.create".into(), evidence: "logger" }));
        assert_eq!(desync.level, "WARN");
    }

    #[test]
    fn unreal_and_console_formats() {
        let ue = "[2026.11.17-21.33.55:123][  0]LogNet: Warning: Network failure: ConnectionLost\n[2026.11.17-21.34.55:123][  0]LogNet: Warning: Network failure: ConnectionLost\n[2026.11.17-21.35.00:000][  0]LogInit: Display: Engine is initialized";
        let groups = digest_of(ue, None, Level::Warn);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].count, 2);
        assert_eq!(groups[0].last.as_deref(), Some("2026-11-17 21:34:55"));
        // Console Paper : heures sans jour, couleurs ANSI retirées.
        let console = "\u{1b}[33m[19:08:34 WARN]: [VoxelBench] Send error: Connection reset.\u{1b}[0m";
        let line = clean_line(console);
        let groups = digest_of(&line, None, Level::Warn);
        assert_eq!(groups[0].last.as_deref(), Some("19:08:34"));
        assert_eq!(groups[0].probable_source, Some(Culprit { name: "VoxelBench".into(), evidence: "prefix" }));
    }

    #[test]
    fn messages_and_frames_normalize() {
        let mut names = Names::default();
        names.add_name("vb_a6ff74");
        assert_eq!(
            normalize_message("vb_a6ff74[/46.225.215.80:54526] logged in with entity id 234319 at ([minecraft:overworld]489.5, 72.0, 473.5)", &names, 200),
            "<joueur>[/#:#] logged in with entity id # at ([minecraft:overworld]#, #, #)"
        );
        assert_eq!(
            normalize_message("Auto-bench job 2a7caa77-5efd-4af3-a33f-0e7ec8c4fda2 validated, key 50c93a0bbc4e9927fd34a5b411a20fc7, obj Foo@1b6d3586", &names, 200),
            "Auto-bench job <uuid> validated, key <hex>, obj Foo@<hex>"
        );
        assert_eq!(normalize_message("world 'voxelbench_temp_1792899608657' created.", &names, 200), "world 'voxelbench_temp_#' created.");
        assert_eq!(normalize_frame("com.x.Foo$1.run"), "com.x.Foo$#.run");
        assert_eq!(normalize_frame("com.x.Foo.lambda$onEnable$12"), "com.x.Foo.lambda$onEnable$#");
        assert_eq!(jar_name("LuckPerms-Bukkit-5.4.102.jar").as_deref(), Some("LuckPerms-Bukkit"));
        assert_eq!(jar_name("sodium-fabric-0.5.8+mc1.20.4.jar").as_deref(), Some("sodium-fabric"));
        assert_eq!(jar_name("create-1.20.1-0.5.1.f.jar%23200!/").as_deref(), Some("create"));
        assert_eq!(jar_name("Vault.jar").as_deref(), Some("Vault"));
        assert_eq!(jar_name("ViaVersion-v5.0.jar").as_deref(), Some("ViaVersion"));
        assert_eq!(bracket_prefix("[STDERR] [MyPlugin] boom"), Some("MyPlugin"));
        assert_eq!(bracket_prefix("[Not Secure] <Alex> salut"), None);
        assert_eq!(bracket_prefix("[minecraft:overworld] x"), None);
        // Relevé sur un vrai journal Paper : le préfixe est un composant interne, le plugin est cité.
        let ambiguous = "[ModernPluginLoadingStrategy] Ambiguous plugin name 'VoxelBench' for files 'plugins/VoxelBench-2.6.0.jar' and 'plugins/VoxelBench-2.6.1.jar' in 'plugins'";
        assert_eq!(bracket_prefix(ambiguous), None);
        assert_eq!(culprit_in_text(ambiguous), Culprit::new("VoxelBench", "load"));
        assert_eq!(package_of("com.earth2me.essentials.Essentials.onEnable"), "com.earth2me.essentials");
        assert_eq!(package_of("a.B.c"), "a");
        let f = parse_frame("\tat EssentialsX-2.21.0.jar//com.earth2me.essentials.Essentials.onEnable(Essentials.java:123)").unwrap();
        assert_eq!((f.method, f.jar), ("com.earth2me.essentials.Essentials.onEnable", Some("EssentialsX-2.21.0.jar")));
        let f = parse_frame("at java.base/java.lang.Thread.run(Thread.java:840) [?:?]").unwrap();
        assert_eq!((f.module, f.jar), (Some("java.base"), None));
        assert!(parse_frame("at the end of the day (really)").is_none());
        assert_eq!(exception_at("Exception in thread \"Craft Scheduler Thread - 12\" java.lang.OutOfMemoryError: Java heap space"), Some(("java.lang.OutOfMemoryError", "Java heap space")));
        assert_eq!(exception_at("org.bukkit.event.EventException: null"), Some(("org.bukkit.event.EventException", "null")));
        assert_eq!(exception_at("Error occurred while enabling X"), None);
        assert_eq!(exception_at("java.lang.Errors are bad"), None);
    }
}
