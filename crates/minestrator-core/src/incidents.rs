//! Journal des **incidents** (outil MCP `list_incidents`, « Journaux et historique ») : alertes
//! du superviseur (crash, seuils CPU/RAM/disque, échéance d'une MyBox, redémarrage automatique,
//! tâche planifiée) et diagnostics du Copilote qui s'y rattachent, du plus récent au plus ancien.
//!
//! - **Consignation** ([`Core::start_incident_journal`], app de bureau) : chaque alerte du bus
//!   d'events (superviseur local, automatisations, alertes relayées du service d'arrière-plan) et
//!   chaque diagnostic du Copilote sont écrits dans `metrics.db` (table `incident_log`, 32 jours).
//!   Le service consigne de son côté ses alertes dans sa propre base, relue par IPC.
//! - **Lecture** ([`Core::incident_report`]) : journal local, plus les alertes du service quand il
//!   supervise le compte courant (sans doublon), plus les crashs relevés par le suivi de
//!   disponibilité (table `uptime`) qu'aucune alerte ne couvre (alerte de crash désactivée, ou
//!   antérieurs au journal). Chaque diagnostic est rattaché à l'incident qui l'a déclenché (même
//!   serveur, déclencheur compatible, dans les 30 minutes) ; [`assemble`] est pur et testé.
//! - **MCP headless** (`minestrator-mcp`, `--mcp`) : même dossier de données que l'app, donc même
//!   `metrics.db` et même journal, mais il ne surveille rien lui-même. Sans aucune donnée locale,
//!   la réponse le dit explicitement au lieu de présenter une liste vide comme « aucun incident ».

use crate::events::{Alert, CoreEvent, Diagnosis};
use crate::store::now_secs;
use crate::{Core, Result};
use chrono::DateTime;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Un diagnostic est rattaché à l'incident qui le précède de 30 minutes au plus.
const DIAGNOSIS_WINDOW_S: i64 = 1800;
/// Un crash du suivi de disponibilité à moins de 5 minutes d'une alerte de crash est le même.
const CRASH_MATCH_S: i64 = 300;
/// Longueurs conservées (caractères) : détail d'une alerte, cause et correctif d'un diagnostic.
const DETAILS_CHARS: usize = 4000;
const DIAGNOSIS_CHARS: usize = 2000;
/// Longueur du détail renvoyé à l'agent.
const OUTPUT_DETAILS_CHARS: usize = 1500;
/// Fenêtre maximale de `list_incidents` (jours) : rétention du journal et du suivi.
pub(crate) const MAX_DAYS: i64 = 32;

/// Ligne du journal local.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IncidentEntry {
    /// Serveur, ou MyBox pour une alerte d'échéance.
    pub server_id: i64,
    pub server_name: String,
    /// Genre d'alerte (`crash`, `cpu`, `ram`, `disk`, `expiry`, `auto_restart`, `task`…) ou
    /// `diagnosis`.
    pub kind: String,
    pub severity: String,
    pub ts: i64,
    /// Message de l'alerte, ou résumé du diagnostic.
    pub message: String,
    /// Alerte : dernières lignes de console (déjà anonymisées) ou erreur ; diagnostic : JSON
    /// `{ trigger, cause, suggested_fix, actions }`.
    pub details: Option<String>,
}

fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

impl IncidentEntry {
    pub(crate) fn from_alert(a: &Alert) -> Self {
        Self {
            server_id: a.server_id,
            server_name: a.server_name.clone(),
            kind: a.kind.clone(),
            severity: a.severity.clone(),
            ts: a.ts,
            message: a.message.clone(),
            details: a.details.as_deref().map(|d| cut(d, DETAILS_CHARS)),
        }
    }

    pub(crate) fn from_diagnosis(d: &Diagnosis) -> Self {
        let actions: Vec<Value> = d
            .actions
            .iter()
            .map(|a| json!({ "tool": a.tool, "label": a.label, "risk": a.risk }))
            .collect();
        let details = json!({
            "trigger": d.trigger,
            "cause": cut(&d.cause, DIAGNOSIS_CHARS),
            "suggested_fix": cut(&d.suggested_fix, DIAGNOSIS_CHARS),
            "actions": actions,
        });
        Self {
            server_id: d.server_id,
            server_name: d.server_name.clone(),
            kind: "diagnosis".into(),
            severity: d.severity.clone(),
            ts: d.ts,
            message: d.summary.clone(),
            details: Some(details.to_string()),
        }
    }
}

/// Diagnostic du Copilote, tel que renvoyé à l'agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DiagnosisOut {
    pub at: String,
    /// Déclencheur (`crash`, `cpu`, `ram`, `disk`, `error`, `warn`, `performance`, `manual`…).
    pub trigger: String,
    pub summary: String,
    pub cause: String,
    pub suggested_fix: String,
    /// Actions proposées (`outil : libellé (risque)`).
    pub actions: Vec<String>,
    #[serde(skip)]
    ts: i64,
}

impl DiagnosisOut {
    fn from_entry(e: &IncidentEntry) -> Self {
        let v: Value = e.details.as_deref().and_then(|d| serde_json::from_str(d).ok()).unwrap_or(Value::Null);
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let actions = v
            .get("actions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| {
                        let s = |k: &str| x.get(k).and_then(Value::as_str).unwrap_or_default();
                        format!("{} : {} ({})", s("tool"), s("label"), s("risk"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            at: iso(e.ts),
            trigger: text("trigger"),
            summary: e.message.clone(),
            cause: text("cause"),
            suggested_fix: text("suggested_fix"),
            actions,
            ts: e.ts,
        }
    }
}

/// Un incident listé.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Incident {
    /// Date (UTC, RFC 3339).
    pub at: String,
    pub server_id: i64,
    pub server_name: String,
    /// Genre d'alerte (`crash` = arrêt inattendu, `cpu`/`ram`/`disk` = surcharge, `expiry`,
    /// `auto_restart`, `task`) ou `diagnosis` (diagnostic sans alerte : erreur de console…).
    pub kind: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// `app` (journal local) | `service` (service d'arrière-plan) | `uptime` (suivi de
    /// disponibilité : passage hors service sans arrêt demandé).
    pub source: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnoses: Vec<DiagnosisOut>,
    #[serde(skip)]
    ts: i64,
}

/// Date UTC lisible (`2026-11-18T17:05:00Z`).
pub(crate) fn iso(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// Un diagnostic de déclencheur `trigger` peut-il se rattacher à un incident de genre `kind` ?
fn diagnosis_fits(trigger: &str, kind: &str) -> bool {
    match trigger {
        // Analyse lancée à la main juste après un incident : elle le concerne.
        "manual" | "selection" => true,
        "performance" => matches!(kind, "cpu" | "ram"),
        t => t == kind,
    }
}

/// Assemble les sources (pur) : alertes du journal local et du service (sans doublon), crashs du
/// suivi de disponibilité non couverts par une alerte, diagnostics rattachés à leur incident (ou
/// listés seuls, sauf les analyses manuelles). Du plus récent au plus ancien.
pub(crate) fn assemble(
    journal: Vec<IncidentEntry>,
    service: Vec<Alert>,
    crashes: Vec<(i64, i64)>,
    names: &HashMap<i64, String>,
) -> Vec<Incident> {
    let (diagnoses, alerts): (Vec<IncidentEntry>, Vec<IncidentEntry>) =
        journal.into_iter().partition(|e| e.kind == "diagnosis");
    let mut out: Vec<Incident> = Vec::new();
    let incident = |e: IncidentEntry, source: &'static str| Incident {
        at: iso(e.ts),
        server_id: e.server_id,
        server_name: e.server_name,
        kind: e.kind,
        severity: e.severity,
        message: e.message,
        details: e.details,
        source,
        diagnoses: Vec::new(),
        ts: e.ts,
    };
    let mut seen: HashSet<(i64, String, i64)> = HashSet::new();
    for e in alerts {
        if seen.insert((e.server_id, e.kind.clone(), e.ts)) {
            out.push(incident(e, "app"));
        }
    }
    // Alertes du service : celles que l'app a relayées sont déjà dans le journal local.
    for a in service {
        if seen.insert((a.server_id, a.kind.clone(), a.ts)) {
            out.push(incident(IncidentEntry::from_alert(&a), "service"));
        }
    }
    for (server_id, ts) in crashes {
        let covered = out
            .iter()
            .any(|i| i.server_id == server_id && i.kind == "crash" && (i.ts - ts).abs() <= CRASH_MATCH_S);
        if covered {
            continue;
        }
        out.push(Incident {
            at: iso(ts),
            server_id,
            server_name: names.get(&server_id).cloned().unwrap_or_else(|| format!("serveur {server_id}")),
            kind: "crash".into(),
            severity: "critical".into(),
            message: "Arrêt inattendu : passage hors service sans arrêt demandé (suivi de disponibilité).".into(),
            details: None,
            source: "uptime",
            diagnoses: Vec::new(),
            ts,
        });
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.ts));
    for d in diagnoses {
        let diag = DiagnosisOut::from_entry(&d);
        let host = out.iter_mut().find(|i| {
            i.kind != "diagnosis"
                && i.server_id == d.server_id
                && i.ts <= d.ts
                && d.ts - i.ts <= DIAGNOSIS_WINDOW_S
                && diagnosis_fits(&diag.trigger, &i.kind)
        });
        match host {
            Some(i) => i.diagnoses.push(diag),
            None if matches!(diag.trigger.as_str(), "manual" | "selection") => {}
            None => {
                let trigger = diag.trigger.clone();
                let mut inc = incident(d, "app");
                inc.message = match trigger.as_str() {
                    "error" | "warn" => format!("Ligne {} de la console analysée par le Copilote.", trigger.to_uppercase()),
                    "performance" => "Surcharge prolongée analysée par le Copilote.".into(),
                    t => format!("Diagnostic du Copilote (déclencheur : {t})."),
                };
                inc.details = None;
                inc.diagnoses.push(diag);
                out.push(inc);
            }
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.ts));
    for i in &mut out {
        i.diagnoses.sort_by_key(|d| d.ts);
    }
    out
}

/// Paramètres de `list_incidents`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IncidentQuery {
    pub server_id: Option<i64>,
    pub limit: usize,
    pub since_days: i64,
    /// Filtre sur le genre (`crash`, `cpu`, `diagnosis`…).
    pub kind: Option<String>,
}

impl IncidentQuery {
    /// Arguments de l'outil MCP `list_incidents` (tous facultatifs, bornés).
    pub(crate) fn from_args(args: &Value) -> Self {
        Self {
            server_id: args.get("server_id").and_then(Value::as_i64),
            limit: args.get("limit").and_then(Value::as_i64).unwrap_or(20).clamp(1, 100) as usize,
            since_days: args.get("since_days").and_then(Value::as_i64).unwrap_or(30).clamp(1, MAX_DAYS),
            kind: args
                .get("kind")
                .and_then(Value::as_str)
                .map(|k| k.trim().to_ascii_lowercase())
                .filter(|k| !k.is_empty() && k != "all"),
        }
    }
}

impl Core {
    /// Tient le journal local des incidents (app de bureau ; à démarrer dans un runtime tokio) :
    /// chaque alerte et chaque diagnostic du Copilote publiés sur le bus d'events sont consignés
    /// dans `metrics.db`, où l'outil MCP `list_incidents` les relit — y compris depuis le MCP
    /// headless, qui partage le dossier de données.
    pub fn start_incident_journal(self: Arc<Self>) {
        use tokio::sync::broadcast::error::RecvError;
        let mut rx = self.subscribe();
        tokio::spawn(async move {
            loop {
                let entry = match rx.recv().await {
                    Ok(CoreEvent::Alert(a)) => IncidentEntry::from_alert(&a),
                    Ok(CoreEvent::Diagnosis(d)) => IncidentEntry::from_diagnosis(&d),
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                if let Err(e) = self.store.insert_incident(&entry) {
                    tracing::warn!("incident non consigné : {e}");
                }
            }
        });
    }

    /// Outil `list_incidents` : incidents d'un serveur (ou de tous ceux du compte courant), du plus
    /// récent au plus ancien, avec les diagnostics rattachés et la couverture des données.
    pub(crate) async fn incident_report(&self, q: IncidentQuery) -> Result<String> {
        let now = now_secs();
        let since = now - q.since_days.clamp(1, MAX_DAYS) * 86_400;
        // Marge : diagnostics rattachés, filtrage par compte et par genre après lecture.
        let fetch = (q.limit * 5).clamp(200, 2000) as u32;
        let journal = self.store.incidents(q.server_id, since, fetch)?;
        let (journal_total, journal_oldest) = self.store.incident_log_extent()?;
        let crashes = self.store.crash_transitions(q.server_id, since, fetch)?;
        let tracked = self.store.has_state_rows(q.server_id, since)?;
        let service = self.service_alert_log(since).await;
        let service_alerts: Vec<Alert> = service
            .iter()
            .flatten()
            .filter(|a| q.server_id.is_none_or(|id| a.server_id == id))
            .cloned()
            .collect();

        // Serveurs (et MyBox, pour les échéances) du compte courant : noms, et filtre quand tous
        // les serveurs sont demandés (le journal garde aussi ceux d'un compte précédent).
        let overview = self.list_servers().await.ok();
        let mut names: HashMap<i64, String> = HashMap::new();
        let mut account_ids: HashSet<i64> = HashSet::new();
        if let Some(o) = &overview {
            for s in &o.servers {
                names.insert(s.id, s.name.clone());
                account_ids.insert(s.id);
            }
            account_ids.extend(o.myboxes.iter().map(|m| m.id));
        }

        let mut incidents = assemble(journal, service_alerts, crashes, &names);
        if q.server_id.is_none() && overview.is_some() {
            incidents.retain(|i| account_ids.contains(&i.server_id));
        }
        if let Some(kind) = q.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            incidents.retain(|i| i.kind == kind);
        }

        let no_data = journal_total == 0 && !tracked && service.is_none();
        if incidents.is_empty() && no_data {
            return Ok(format!(
                "Aucune donnée d'incident disponible{} : le journal des incidents et le suivi de \
                 disponibilité sont tenus par l'app de bureau MineStrator Terminal (ou par le service \
                 d'arrière-plan) pendant qu'elle surveille les serveurs. Ce serveur MCP les relit dans \
                 le même dossier de données, mais ne surveille rien lui-même, et le service \
                 d'arrière-plan n'est pas joignable (ou supervise un autre compte). Ce résultat ne \
                 signifie PAS qu'il n'y a eu aucun incident : lancer l'app (superviseur actif) pour \
                 constituer l'historique. Pour l'état présent, voir server_status ; pour les erreurs \
                 récentes, error_digest.",
                q.server_id.map(|id| format!(" pour le serveur {id}")).unwrap_or_default()
            ));
        }

        let total = incidents.len();
        incidents.truncate(q.limit);
        // Vers un LLM : chaque champ texte passe par l'anonymisation (secrets, IP, e-mails).
        for i in &mut incidents {
            i.message = self.redact_ai(&i.message);
            i.details = i.details.as_deref().map(|d| self.redact_ai(&cut(d, OUTPUT_DETAILS_CHARS)));
            for d in &mut i.diagnoses {
                d.summary = self.redact_ai(&d.summary);
                d.cause = self.redact_ai(&d.cause);
                d.suggested_fix = self.redact_ai(&d.suggested_fix);
            }
        }

        let mut notes: Vec<String> = Vec::new();
        match journal_oldest {
            None => notes.push(
                "journal local vide : il est tenu par l'app de bureau quand elle tourne (ce serveur MCP \
                 le relit sans rien surveiller) ; seuls les crashs du suivi de disponibilité et les \
                 alertes du service d'arrière-plan sont listés"
                    .into(),
            ),
            Some(oldest) if oldest > since => notes.push(format!(
                "journal local tenu depuis le {} seulement : avant, seuls les crashs du suivi de \
                 disponibilité sont listés",
                iso(oldest)
            )),
            Some(_) => {}
        }
        if !tracked {
            notes.push("aucune observation du suivi de disponibilité sur la période : les crashs survenus pendant que rien ne surveillait ne sont pas connus".into());
        }
        if overview.is_none() && q.server_id.is_none() {
            notes.push("liste des serveurs indisponible : incidents de tous les comptes consignés sur ce poste".into());
        }
        let v = json!({
            "scope": q.server_id.map_or_else(|| json!("tous les serveurs du compte"), |id| json!({ "server_id": id })),
            "since_days": q.since_days.clamp(1, MAX_DAYS),
            "kind_filter": q.kind,
            "incidents_total": total,
            "shown": incidents.len(),
            "incidents": incidents,
            "coverage": {
                "local_journal_entries": journal_total,
                "local_journal_since": journal_oldest.map(iso),
                "service_alerts": if service.is_some() { "incluses (le service supervise ce compte)" } else { "non consultées (service absent, injoignable ou d'un autre compte)" },
                "uptime_tracking": tracked,
                "diagnoses": "diagnostics du Copilote rattachés à leur incident (analyses manuelles sans incident omises)",
            },
            "notes": notes,
        });
        serde_json::to_string_pretty(&v).map_err(|e| crate::Error::Unexpected(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ProposedAction;

    fn alert(server_id: i64, kind: &str, ts: i64) -> Alert {
        Alert {
            server_id,
            server_name: format!("srv{server_id}"),
            kind: kind.into(),
            severity: "critical".into(),
            message: format!("{kind} sur srv{server_id}"),
            ts,
            details: None,
        }
    }

    fn diag(server_id: i64, trigger: &str, ts: i64) -> IncidentEntry {
        IncidentEntry::from_diagnosis(&Diagnosis {
            id: "x".into(),
            server_id,
            server_name: format!("srv{server_id}"),
            trigger: trigger.into(),
            severity: "warning".into(),
            summary: format!("résumé {trigger}"),
            cause: "cause".into(),
            suggested_fix: "correctif".into(),
            actions: vec![ProposedAction {
                tool: "power_action".into(),
                args: json!({ "action": "restart" }),
                label: "Redémarrer".into(),
                risk: "caution".into(),
            }],
            ts,
        })
    }

    #[test]
    fn entries_keep_alerts_and_diagnoses() {
        let mut a = alert(1, "crash", 100);
        a.details = Some("x".repeat(DETAILS_CHARS + 10));
        let e = IncidentEntry::from_alert(&a);
        assert_eq!(e.details.as_ref().unwrap().chars().count(), DETAILS_CHARS + 1, "tronqué (+ …)");
        let d = diag(1, "crash", 150);
        assert_eq!((d.kind.as_str(), d.ts, d.message.as_str()), ("diagnosis", 150, "résumé crash"));
        let out = DiagnosisOut::from_entry(&d);
        assert_eq!(out.trigger, "crash");
        assert_eq!(out.actions, vec!["power_action : Redémarrer (caution)".to_string()]);
        assert_eq!(out.at, "1970-01-01T00:02:30Z");
    }

    #[test]
    fn sources_merge_without_duplicates_and_diagnoses_attach() {
        let journal = vec![
            IncidentEntry::from_alert(&alert(1, "crash", 1_000)),
            diag(1, "crash", 1_100),
            // Analyse manuelle juste après un crash vu seulement par le suivi.
            diag(1, "manual", 5_100),
            // Erreur de console : pas d'alerte, diagnostic listé seul.
            diag(2, "error", 9_000),
            // Analyse d'une sélection sans incident : omise.
            diag(2, "selection", 9_500),
            // Diagnostic de surcharge trop tardif pour l'alerte CPU (> 30 min).
            diag(3, "cpu", 2_000 + DIAGNOSIS_WINDOW_S + 1),
        ];
        let service = vec![
            alert(1, "crash", 1_000), // relayée : déjà dans le journal
            alert(3, "cpu", 2_000),
        ];
        let crashes = vec![(1, 5_000), (1, 1_005)];
        let names = HashMap::from([(1, "Survie".to_string())]);
        let out = assemble(journal, service, crashes, &names);

        let shape: Vec<(i64, &str, &str, usize)> =
            out.iter().map(|i| (i.server_id, i.kind.as_str(), i.source, i.diagnoses.len())).collect();
        assert_eq!(
            shape,
            vec![
                (2, "diagnosis", "app", 1),
                (1, "crash", "uptime", 1),
                (3, "diagnosis", "app", 1),
                (3, "cpu", "service", 0),
                (1, "crash", "app", 1),
            ]
        );
        // Crash du suivi : nommé d'après la liste des serveurs ; celui couvert par l'alerte écarté.
        assert_eq!(out[1].server_name, "Survie");
        assert_eq!(out[1].diagnoses[0].trigger, "manual");
        assert!(out[0].message.contains("ERROR"), "{}", out[0].message);
        assert_eq!(out[4].diagnoses[0].summary, "résumé crash");
        assert_eq!(out[4].at, "1970-01-01T00:16:40Z");
        // Sérialisation : champs vides omis, dates internes non exposées.
        let v = serde_json::to_value(&out[3]).unwrap();
        assert!(v.get("diagnoses").is_none() && v.get("details").is_none() && v.get("ts").is_none());
        assert_eq!(v["source"], "service");
    }

    #[test]
    fn query_arguments_are_bounded() {
        let q = IncidentQuery::from_args(&json!({}));
        assert_eq!(q, IncidentQuery { server_id: None, limit: 20, since_days: 30, kind: None });
        let q = IncidentQuery::from_args(&json!({ "server_id": 7, "limit": 1000, "since_days": 365, "kind": " Crash " }));
        assert_eq!(q, IncidentQuery { server_id: Some(7), limit: 100, since_days: MAX_DAYS, kind: Some("crash".into()) });
        assert_eq!(IncidentQuery::from_args(&json!({ "kind": "all" })).kind, None);
    }

    #[test]
    fn diagnosis_triggers_fit_their_incidents() {
        assert!(diagnosis_fits("crash", "crash"));
        assert!(!diagnosis_fits("crash", "cpu"));
        assert!(diagnosis_fits("performance", "ram"));
        assert!(!diagnosis_fits("performance", "disk"));
        assert!(diagnosis_fits("manual", "disk"));
        assert!(assemble(Vec::new(), Vec::new(), Vec::new(), &HashMap::new()).is_empty());
    }
}
