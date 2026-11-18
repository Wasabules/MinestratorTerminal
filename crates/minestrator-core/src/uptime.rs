//! Suivi de disponibilité : pourcentage de disponibilité sur 24 h / 7 j / 30 j et journal des
//! coupures, calculés à partir des **observations d'état** que le superviseur consigne dans
//! `metrics.db` (table `uptime`, voir [`crate::store`]).
//!
//! Le superviseur écrit une ligne à chaque changement d'état d'un serveur (avec sa cause quand il
//! la connaît : `crash` ou `stop`), et une observation à chaque tour de poll (60 s) pour chaque
//! serveur sous connexion monitor dont l'état est connu. Deux observations distantes de plus de
//! [`MAX_GAP_S`] délimitent une période **non surveillée** (app fermée, ordinateur éteint,
//! connexion perdue) : elle ne compte ni comme disponible ni comme coupure.
//!
//! Tout ici est pur (aucune E/S) : les lignes viennent du stockage local et, quand le service
//! d'arrière-plan supervise, de son propre historique par IPC ([`merge_spans`]).
//!
//! ## Données de l'hébergeur ([`host_spans`], [`fill_unmonitored`])
//!
//! Les statistiques à la minute de l'hébergeur (voir [`crate::history`]) **réduisent** les
//! périodes non surveillées, selon une règle volontairement asymétrique :
//! - une minute **mesurée** (RAM, CPU ou disque non nuls) prouve que le serveur tournait : elle
//!   compte comme « en service » ; deux minutes mesurées distantes d'au plus [`HOST_MAX_GAP_S`] se
//!   rejoignent (même tolérance que la surveillance locale) ;
//! - une minute **à zéro** ne prouve rien : l'API complète de zéros aussi bien un serveur éteint
//!   qu'une minute non mesurée. Elle reste non surveillée — l'hébergeur ne crée **jamais** de
//!   coupure ;
//! - la surveillance locale fait foi : l'hébergeur ne comble que le temps qu'elle n'a pas couvert
//!   (y compris la « traîne » de ses périodes), sans jamais la contredire ;
//! - les moyennes horaires (hors de portée des requêtes de l'app) ne prouvent qu'une mesure dans
//!   l'heure : elles sont ignorées.
//!
//! Conséquence assumée : sur une période vue seulement par l'hébergeur, la disponibilité vaut
//! 100 % du temps **observé** ; la part d'hébergeur ([`UptimeWindow::host_s`]) est donc affichée.

use serde::{Deserialize, Serialize};

/// Écart maximal entre deux observations d'une même période surveillée. Le superviseur observe
/// chaque serveur une fois par tour de poll (60 s) : au-delà de trois tours sans nouvelle, la
/// surveillance est jugée interrompue.
pub const MAX_GAP_S: i64 = 180;
/// Écart maximal entre deux minutes mesurées par l'hébergeur d'une même période en service (deux
/// minutes manquantes au plus, comme la surveillance locale).
pub const HOST_MAX_GAP_S: i64 = 180;
/// Durée couverte par une mesure de l'hébergeur (une minute).
const HOST_STEP_S: i64 = 60;
/// Rétention des observations (un peu plus que la plus longue fenêtre affichée).
pub const RETENTION_S: i64 = 32 * 86_400;
/// Fenêtres du rapport : 24 h, 7 j, 30 j.
pub const WINDOWS: [i64; 3] = [86_400, 7 * 86_400, 30 * 86_400];
/// Coupures listées au plus (les plus récentes).
const MAX_OUTAGES: usize = 100;

/// Une observation d'état consignée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateRow {
    pub ts: i64,
    /// État Wings : `running` | `starting` | `stopping` | `offline` (ou `hibernated`).
    pub state: String,
    /// Transition vers un état hors service dont l'origine est connue : `crash` | `stop`.
    pub cause: Option<String>,
}

/// Période continue **surveillée** pendant laquelle le serveur est resté en service (`up`) ou
/// hors service. C'est aussi la forme compacte échangée avec le service d'arrière-plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UptimeSpan {
    pub start: i64,
    pub end: i64,
    pub up: bool,
    /// Hors service : `crash` | `stop` | `unknown`.
    pub cause: Option<String>,
    /// La période s'achève sur un changement d'état observé (sinon : fin de la surveillance, ou
    /// période encore en cours à `end`).
    pub ended: bool,
    /// Période déduite des statistiques de l'hébergeur (toujours « en service »), pas de la
    /// surveillance de l'app. Absent du JSON quand faux : forme inchangée pour l'IPC du service.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub host: bool,
}

/// Disponibilité sur une fenêtre glissante.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UptimeWindow {
    /// Durée de la fenêtre (secondes) : 86 400, 604 800 ou 2 592 000.
    pub seconds: i64,
    /// Temps effectivement surveillé dans la fenêtre.
    pub monitored_s: i64,
    /// Dont en service.
    pub up_s: i64,
    /// Part de `monitored_s` connue seulement par les statistiques de l'hébergeur (toujours en
    /// service : il ne prouve jamais une coupure).
    pub host_s: i64,
    /// `up_s / monitored_s` en % ; `null` si rien n'a été surveillé.
    pub percent: Option<f64>,
}

/// Une coupure (période hors service surveillée).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Outage {
    pub start: i64,
    /// `null` : coupure en cours.
    pub end: Option<i64>,
    pub duration_s: i64,
    /// `crash` | `stop` | `unknown`
    #[cfg_attr(feature = "ts", ts(type = "'crash' | 'stop' | 'unknown'"))]
    pub cause: String,
    /// `false` : la surveillance s'est interrompue pendant la coupure — sa fin réelle est inconnue
    /// (`end` est alors la dernière observation).
    pub end_known: bool,
}

/// Rapport de disponibilité d'un serveur.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UptimeReport {
    pub server_id: i64,
    pub generated_at: i64,
    /// 24 h, 7 j, 30 j (dans cet ordre).
    pub windows: Vec<UptimeWindow>,
    /// Coupures des 30 derniers jours, des plus récentes aux plus anciennes.
    pub outages: Vec<Outage>,
    /// Début de l'historique disponible (première observation), `null` si aucune.
    pub since: Option<i64>,
}

/// Un état compte-t-il comme « en service » ?
pub fn is_up(state: &str) -> bool {
    state == "running"
}

/// Cause d'une période hors service qui commence par `row`.
fn down_cause(row: &StateRow) -> String {
    match (&row.cause, row.state.as_str()) {
        (Some(c), _) => c.clone(),
        // Une hibernation est la mise en sommeil d'un serveur arrêté : pas une panne.
        (None, "hibernated") => "stop".into(),
        (None, _) => "unknown".into(),
    }
}

/// Observations (triées par date) → périodes surveillées, bornées à `now`.
///
/// Chaque observation couvre l'intervalle jusqu'à la suivante si elle arrive dans les
/// [`MAX_GAP_S`] secondes, sinon jusqu'à `ts + MAX_GAP_S` (au-delà : non surveillé). Les
/// observations consécutives de même statut (en service / hors service) fusionnent.
pub fn spans(rows: &[StateRow], now: i64) -> Vec<UptimeSpan> {
    let mut out: Vec<UptimeSpan> = Vec::new();
    let mut current: Option<UptimeSpan> = None;
    let mut last_ts = i64::MIN;
    for row in rows.iter().filter(|r| r.ts <= now) {
        let up = is_up(&row.state);
        if let Some(mut span) = current.take() {
            if row.ts - last_ts > MAX_GAP_S {
                // Trou de surveillance : la période s'arrête à la portée de la dernière observation.
                span.end = last_ts + MAX_GAP_S;
                span.ended = false;
                out.push(span);
            } else if span.up != up {
                span.end = row.ts;
                span.ended = true;
                out.push(span);
            } else {
                current = Some(span);
            }
        }
        if current.is_none() {
            current = Some(UptimeSpan {
                start: row.ts,
                end: row.ts,
                up,
                cause: (!up).then(|| down_cause(row)),
                ended: false,
                host: false,
            });
        }
        last_ts = row.ts;
    }
    if let Some(mut span) = current {
        span.end = (last_ts + MAX_GAP_S).min(now);
        span.ended = false;
        out.push(span);
    }
    out.retain(|s| s.end > s.start);
    out
}

/// Fusionne deux chronologies (app et service d'arrière-plan). En cas de chevauchement (bascule
/// de supervision), la période qui commence la première l'emporte — sauf sur sa « traîne » non
/// observée (les [`MAX_GAP_S`] dernières secondes d'une période close par un trou de
/// surveillance), que remplace ce que l'autre source a réellement vu. Deux périodes contiguës de
/// même statut n'en font qu'une : une coupure vue par l'app puis par le service reste une coupure.
pub fn merge_spans(a: Vec<UptimeSpan>, b: Vec<UptimeSpan>) -> Vec<UptimeSpan> {
    let mut all: Vec<UptimeSpan> = a.into_iter().chain(b).collect();
    all.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<UptimeSpan> = Vec::new();
    for mut span in all {
        if let Some(last) = out.last_mut() {
            let in_tail = !last.ended
                && span.start < last.end
                && span.start >= last.end - MAX_GAP_S
                && span.start > last.start;
            if in_tail {
                last.end = span.start;
            }
            if span.end <= last.end {
                continue; // entièrement couverte
            }
            if span.start < last.end {
                span.start = last.end;
            }
            if span.start == last.end && span.up == last.up {
                last.end = span.end;
                last.ended = span.ended;
                continue;
            }
        }
        out.push(span);
    }
    out
}

/// Périodes « en service » prouvées par l'hébergeur : `minutes` (triées) sont les débuts des
/// minutes **mesurées** (voir l'en-tête du module). Une mesure couvre sa minute ; deux mesures
/// distantes d'au plus [`HOST_MAX_GAP_S`] se rejoignent. Jamais de période hors service.
pub fn host_spans(minutes: &[i64], now: i64) -> Vec<UptimeSpan> {
    let piece = |start: i64, end: i64| UptimeSpan {
        start,
        end: end.min(now),
        up: true,
        cause: None,
        ended: false,
        host: true,
    };
    let mut out = Vec::new();
    let mut current: Option<(i64, i64)> = None;
    for &ts in minutes.iter().filter(|&&ts| ts < now) {
        current = match current {
            Some((start, last)) if ts - last <= HOST_MAX_GAP_S => Some((start, ts.max(last))),
            Some((start, last)) => {
                out.push(piece(start, last + HOST_STEP_S));
                Some((ts, ts))
            }
            None => Some((ts, ts)),
        };
    }
    if let Some((start, last)) = current {
        out.push(piece(start, last + HOST_STEP_S));
    }
    out.retain(|s| s.end > s.start);
    out
}

/// Intervalles de `[from, to]` qu'aucune période ne couvre (`spans` triées, sans recouvrement :
/// sortie de [`spans`] ou de [`merge_spans`]).
pub fn unmonitored(spans: &[UptimeSpan], from: i64, to: i64) -> Vec<(i64, i64)> {
    let mut gaps = Vec::new();
    let mut cursor = from;
    for s in spans {
        if cursor >= to {
            break;
        }
        if s.end <= cursor {
            continue;
        }
        if s.start > cursor {
            gaps.push((cursor, s.start.min(to)));
        }
        cursor = cursor.max(s.end);
    }
    if cursor < to {
        gaps.push((cursor, to));
    }
    gaps.retain(|(a, b)| b > a);
    gaps
}

/// Comble les périodes non surveillées de `local` (triées, sans recouvrement) avec les périodes de
/// l'hébergeur ([`host_spans`]) : seul le temps que la surveillance locale n'a pas couvert est
/// repris, la surveillance locale fait toujours foi. Les périodes restent distinctes par origine.
pub fn fill_unmonitored(local: Vec<UptimeSpan>, host: Vec<UptimeSpan>) -> Vec<UptimeSpan> {
    let mut pieces: Vec<UptimeSpan> = Vec::new();
    for h in host {
        let mut start = h.start;
        for l in local.iter().filter(|l| l.end > h.start && l.start < h.end) {
            if l.start > start {
                pieces.push(UptimeSpan { start, end: l.start, ..h.clone() });
            }
            start = start.max(l.end);
            if start >= h.end {
                break;
            }
        }
        if start < h.end {
            pieces.push(UptimeSpan { start, ..h });
        }
    }
    let mut all: Vec<UptimeSpan> = local.into_iter().chain(pieces).collect();
    all.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<UptimeSpan> = Vec::with_capacity(all.len());
    for span in all {
        if let Some(last) = out.last_mut() {
            if last.host && span.host && last.end >= span.start {
                last.end = last.end.max(span.end);
                continue;
            }
        }
        out.push(span);
    }
    out
}

/// Rapport (fenêtres 24 h / 7 j / 30 j et coupures) à partir de périodes déjà fusionnées.
pub fn report(server_id: i64, spans: &[UptimeSpan], now: i64) -> UptimeReport {
    let windows = WINDOWS
        .iter()
        .map(|&seconds| {
            let from = now - seconds;
            let (mut monitored_s, mut up_s, mut host_s) = (0, 0, 0);
            for s in spans {
                let len = (s.end.min(now) - s.start.max(from)).max(0);
                monitored_s += len;
                if s.up {
                    up_s += len;
                }
                if s.host {
                    host_s += len;
                }
            }
            UptimeWindow {
                seconds,
                monitored_s,
                up_s,
                host_s,
                percent: (monitored_s > 0).then(|| up_s as f64 / monitored_s as f64 * 100.0),
            }
        })
        .collect();
    let from = now - WINDOWS[2];
    let mut outages: Vec<Outage> = spans
        .iter()
        .filter(|s| !s.up && s.end > from)
        .map(|s| {
            // Encore en cours : la dernière période atteint `now` sans changement d'état.
            let ongoing = !s.ended && s.end >= now;
            let start = s.start.max(from);
            Outage {
                start,
                end: (!ongoing).then_some(s.end),
                duration_s: s.end - start,
                cause: s.cause.clone().unwrap_or_else(|| "unknown".into()),
                end_known: s.ended || ongoing,
            }
        })
        .collect();
    outages.reverse();
    outages.truncate(MAX_OUTAGES);
    UptimeReport {
        server_id,
        generated_at: now,
        windows,
        outages,
        since: spans.first().map(|s| s.start),
    }
}

impl crate::Core {
    /// Périodes surveillées d'un serveur d'après l'historique **local** (`to` borne la dernière ;
    /// le service d'arrière-plan y répond par IPC). Voir [`crate::Core::uptime_report`].
    pub fn local_uptime_spans(&self, server_id: i64, from: i64, to: i64) -> crate::Result<Vec<UptimeSpan>> {
        let from = from.max(to - RETENTION_S);
        Ok(spans(&self.store.state_rows(server_id, from)?, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ts: i64, state: &str) -> StateRow {
        StateRow { ts, state: state.into(), cause: None }
    }

    fn change(ts: i64, state: &str, cause: &str) -> StateRow {
        StateRow { ts, state: state.into(), cause: Some(cause.into()) }
    }

    /// Observations régulières (toutes les 60 s) d'un même état sur `[from, to]`.
    fn every_minute(from: i64, to: i64, state: &str) -> Vec<StateRow> {
        (from..=to).step_by(60).map(|ts| row(ts, state)).collect()
    }

    #[test]
    fn surveillance_continue_en_service() {
        let rows = every_minute(0, 3600, "running");
        let s = spans(&rows, 3600);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].start, s[0].end, s[0].up), (0, 3600, true));
        let r = report(1, &s, 3600);
        assert_eq!(r.windows[0].monitored_s, 3600);
        assert_eq!(r.windows[0].percent, Some(100.0));
        assert!(r.outages.is_empty());
        assert_eq!(r.since, Some(0));
    }

    #[test]
    fn un_crash_devient_une_coupure_datee() {
        let mut rows = every_minute(0, 600, "running");
        rows.push(change(630, "offline", "crash"));
        rows.extend(every_minute(660, 900, "offline"));
        rows.push(row(930, "starting"));
        rows.push(row(960, "running"));
        rows.extend(every_minute(1020, 1200, "running"));
        let s = spans(&rows, 1200);
        assert_eq!(s.iter().map(|s| s.up).collect::<Vec<_>>(), vec![true, false, true]);
        let down = &s[1];
        // `starting` est encore hors service : la coupure va du crash au retour en `running`.
        assert_eq!((down.start, down.end), (630, 960));
        assert_eq!(down.cause.as_deref(), Some("crash"));
        assert!(down.ended);

        let r = report(1, &s, 1200);
        assert_eq!(r.windows[0].monitored_s, 1200);
        assert_eq!(r.windows[0].up_s, 1200 - 330);
        assert_eq!(r.outages.len(), 1);
        let o = &r.outages[0];
        assert_eq!((o.start, o.end, o.duration_s), (630, Some(960), 330));
        assert_eq!(o.cause, "crash");
        assert!(o.end_known);
    }

    #[test]
    fn les_trous_de_surveillance_ne_comptent_pas() {
        // Surveillé 1 h, app fermée 5 h (serveur peut-être éteint : on n'en sait rien), surveillé 1 h.
        let mut rows = every_minute(0, 3600, "running");
        rows.extend(every_minute(6 * 3600, 7 * 3600, "running"));
        let s = spans(&rows, 7 * 3600);
        assert_eq!(s.len(), 2);
        // La première période s'arrête à la portée de la dernière observation (+ MAX_GAP_S).
        assert_eq!(s[0].end, 3600 + MAX_GAP_S);
        assert!(!s[0].ended);
        let r = report(1, &s, 7 * 3600);
        assert_eq!(r.windows[0].monitored_s, 3600 + MAX_GAP_S + 3600);
        assert_eq!(r.windows[0].percent, Some(100.0), "le trou n'est pas une coupure");
        assert!(r.outages.is_empty());
    }

    #[test]
    fn coupure_interrompue_par_un_trou_a_une_fin_inconnue() {
        let mut rows = every_minute(0, 600, "running");
        rows.push(change(660, "offline", "crash"));
        rows.push(row(720, "offline"));
        // Plus rien pendant 2 h, puis de nouveau en service.
        rows.extend(every_minute(720 + 7200, 720 + 7200 + 600, "running"));
        let now = 720 + 7200 + 600;
        let r = report(1, &spans(&rows, now), now);
        assert_eq!(r.outages.len(), 1);
        let o = &r.outages[0];
        assert_eq!(o.start, 660);
        assert_eq!(o.end, Some(720 + MAX_GAP_S));
        assert!(!o.end_known, "la surveillance s'est arrêtée avant la fin de la coupure");
    }

    #[test]
    fn coupure_en_cours() {
        let mut rows = every_minute(0, 300, "running");
        rows.push(change(330, "stopping", "stop"));
        rows.push(change(345, "offline", "stop"));
        rows.extend(every_minute(360, 600, "offline"));
        let r = report(1, &spans(&rows, 620), 620);
        let o = &r.outages[0];
        assert_eq!(o.end, None);
        assert_eq!(o.cause, "stop");
        assert_eq!(o.duration_s, 620 - 330);
        assert!(o.end_known);
        // Arrêt volontaire : il compte bien comme indisponibilité (le serveur ne répond pas).
        assert!(r.windows[0].percent.unwrap() < 100.0);
    }

    #[test]
    fn causes_inconnue_et_hibernation() {
        // Surveillance démarrée sur un serveur déjà éteint : cause inconnue.
        let s = spans(&every_minute(0, 300, "offline"), 300);
        assert_eq!(s[0].cause.as_deref(), Some("unknown"));
        // Serveur hiberné : arrêt, pas panne.
        let s = spans(&every_minute(0, 300, "hibernated"), 300);
        assert_eq!(s[0].cause.as_deref(), Some("stop"));
        assert!(!s[0].up);
    }

    #[test]
    fn fenetres_et_observations_futures() {
        let now = 40 * 86_400;
        // 10 jours en service puis 1 jour hors service, il y a longtemps (au-delà de 30 j : ignoré
        // dans la fenêtre 30 j mais `since` le signale).
        let mut rows = every_minute(0, 3600, "offline");
        rows.extend(every_minute(now - 2 * 86_400, now, "running"));
        // Observation horodatée dans le futur (horloge déréglée) : ignorée.
        rows.push(row(now + 500, "offline"));
        let s = spans(&rows, now);
        let r = report(1, &s, now);
        assert_eq!(r.windows[0].percent, Some(100.0));
        assert_eq!(r.windows[2].monitored_s, 2 * 86_400);
        assert!(r.outages.is_empty(), "la coupure de plus de 30 j n'est pas listée");
        assert_eq!(r.since, Some(0));
        assert_eq!(r.windows.iter().map(|w| w.seconds).collect::<Vec<_>>(), WINDOWS.to_vec());
    }

    #[test]
    fn rien_de_surveille() {
        let r = report(3, &[], 1000);
        assert!(r.windows.iter().all(|w| w.percent.is_none() && w.monitored_s == 0));
        assert_eq!(r.since, None);
    }

    #[test]
    fn fusion_app_puis_service() {
        // L'app surveille jusqu'à 600 (coupure dès 500), le service prend le relais à 590.
        let mut app_rows = every_minute(0, 480, "running");
        app_rows.push(change(500, "offline", "crash"));
        app_rows.push(row(560, "offline"));
        let app = spans(&app_rows, 2000);
        let mut svc_rows = every_minute(590, 650, "offline");
        svc_rows.extend(every_minute(710, 2000, "running"));
        let svc = spans(&svc_rows, 2000);
        let merged = merge_spans(app, svc);
        // Une seule coupure, de 500 (crash vu par l'app) à 710 (retour vu par le service).
        let downs: Vec<_> = merged.iter().filter(|s| !s.up).collect();
        assert_eq!(downs.len(), 1);
        assert_eq!((downs[0].start, downs[0].end), (500, 710));
        assert_eq!(downs[0].cause.as_deref(), Some("crash"));
        // Pas de recouvrement : le temps surveillé n'est pas compté deux fois.
        let r = report(1, &merged, 2000);
        assert_eq!(r.windows[0].monitored_s, 2000);
        for w in merged.windows(2) {
            assert!(w[0].end <= w[1].start);
        }
    }

    #[test]
    fn fusion_ignore_les_periodes_deja_couvertes() {
        // `b` tombe dans la partie réellement observée de `a` (hors traîne) : `a` fait foi.
        let a = vec![UptimeSpan { start: 0, end: 1000, up: true, cause: None, ended: false, host: false }];
        let b = vec![UptimeSpan { start: 10, end: 50, up: false, cause: Some("crash".into()), ended: true, host: false }];
        assert_eq!(merge_spans(a.clone(), b), a);
        assert_eq!(merge_spans(Vec::new(), Vec::new()), Vec::new());
    }

    #[test]
    fn periode_serialisee_pour_lipc() {
        let s = UptimeSpan { start: 1, end: 2, up: false, cause: Some("stop".into()), ended: true, host: false };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::json!({ "start": 1, "end": 2, "up": false, "cause": "stop", "ended": true }));
        let back: UptimeSpan = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
        // Période de l'hébergeur : le drapeau voyage (et un service ancien l'ignore).
        let h = UptimeSpan { host: true, up: true, cause: None, ..s };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["host"], true);
        assert_eq!(serde_json::from_value::<UptimeSpan>(v).unwrap(), h);
    }

    // --- Données de l'hébergeur ---

    /// Minutes mesurées consécutives sur `[from, to)`.
    fn minutes(from: i64, to: i64) -> Vec<i64> {
        (from..to).step_by(60).collect()
    }

    #[test]
    fn minutes_mesurees_en_periodes_en_service() {
        // 10:00 → 10:30 mesurées, deux minutes manquantes (tolérées), puis un trou de 10 min.
        let mut m = minutes(36_000, 36_000 + 1_200);
        m.extend(minutes(36_000 + 1_320, 36_000 + 1_800));
        m.extend(minutes(36_000 + 2_400, 36_000 + 3_000));
        let s = host_spans(&m, 100_000);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].start, s[0].end), (36_000, 36_000 + 1_800), "une minute couvre sa durée");
        assert_eq!((s[1].start, s[1].end), (36_000 + 2_400, 36_000 + 3_000));
        assert!(s.iter().all(|s| s.up && s.host && s.cause.is_none() && !s.ended));
        // Bornée à `now`, minutes futures ignorées.
        let s = host_spans(&minutes(0, 600), 330);
        assert_eq!((s[0].start, s[0].end), (0, 330));
        assert!(host_spans(&[], 100).is_empty());
    }

    #[test]
    fn periodes_non_surveillees() {
        let span = |start, end| UptimeSpan { start, end, up: true, cause: None, ended: false, host: false };
        let spans = vec![span(100, 200), span(300, 400)];
        assert_eq!(unmonitored(&spans, 0, 500), vec![(0, 100), (200, 300), (400, 500)]);
        assert_eq!(unmonitored(&spans, 150, 350), vec![(200, 300)]);
        assert_eq!(unmonitored(&[], 0, 10), vec![(0, 10)]);
        assert!(unmonitored(&spans, 100, 200).is_empty());
    }

    #[test]
    fn l_hebergeur_comble_les_trous_sans_contredire_le_local() {
        // Local : en service 0 → 1 h, coupure (crash) 2 h → 2 h 10, en service 2 h 10 → 3 h.
        let mut rows = every_minute(0, 3_600, "running");
        rows.push(change(7_200, "offline", "crash"));
        rows.extend(every_minute(7_260, 7_740, "offline"));
        rows.push(row(7_800, "running"));
        rows.extend(every_minute(7_860, 10_800, "running"));
        let now = 12 * 3_600;
        let local = spans(&rows, now);
        // L'hébergeur a mesuré toute la journée, y compris pendant la coupure vue localement.
        let host = host_spans(&minutes(0, now), now);
        let filled = fill_unmonitored(local.clone(), host);

        // Le local est intact : la coupure reste une coupure.
        for l in &local {
            assert!(filled.contains(l), "{l:?}");
        }
        // Aucun recouvrement, ordre chronologique.
        for w in filled.windows(2) {
            assert!(w[0].end <= w[1].start, "{w:?}");
        }
        // Les trous (1 h 03 → 2 h, après 3 h 03) sont comblés par l'hébergeur, en service.
        let host_parts: Vec<_> = filled.iter().filter(|s| s.host).collect();
        assert_eq!(host_parts.len(), 2);
        assert_eq!((host_parts[0].start, host_parts[0].end), (3_600 + MAX_GAP_S, 7_200));
        assert_eq!((host_parts[1].start, host_parts[1].end), (10_800 + MAX_GAP_S, now));

        let r = report(1, &filled, now);
        let day = &r.windows[0];
        assert_eq!(day.monitored_s, now, "toute la fenêtre observée");
        assert_eq!(day.host_s, (7_200 - 3_780) + (now - 10_980));
        assert_eq!(r.outages.len(), 1, "seule la coupure observée localement");
        assert_eq!(r.outages[0].cause, "crash");
        assert_eq!(r.since, Some(0));
    }

    #[test]
    fn l_absence_de_mesure_n_invente_aucune_coupure() {
        // Rien en local ; l'hébergeur a mesuré 1 h, puis plus rien (serveur éteint ? minute non
        // écrite ? indiscernable), puis 1 h.
        let now = 6 * 3_600;
        let mut m = minutes(0, 3_600);
        m.extend(minutes(5 * 3_600, now));
        let filled = fill_unmonitored(Vec::new(), host_spans(&m, now));
        let r = report(1, &filled, now);
        assert!(r.outages.is_empty());
        let day = &r.windows[0];
        assert_eq!(day.monitored_s, 2 * 3_600, "le trou reste non surveillé");
        assert_eq!(day.host_s, 2 * 3_600);
        assert_eq!(day.percent, Some(100.0));
        // Sans données de l'hébergeur, rien ne change.
        let local = spans(&every_minute(0, 600, "running"), 600);
        assert_eq!(fill_unmonitored(local.clone(), Vec::new()), local);
    }

    #[test]
    fn la_traine_locale_fait_foi() {
        // Le local s'arrête à 600 (traîne jusqu'à 780) ; l'hébergeur reprend dès 600.
        let local = spans(&every_minute(0, 600, "running"), 2_000);
        assert_eq!(local[0].end, 600 + MAX_GAP_S);
        let filled = fill_unmonitored(local, host_spans(&minutes(0, 2_000), 2_000));
        assert_eq!(filled.len(), 2);
        assert_eq!((filled[1].start, filled[1].end, filled[1].host), (780, 2_000, true));
    }
}
