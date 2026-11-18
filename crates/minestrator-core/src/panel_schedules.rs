//! Tâches planifiées **du panel MineStrator** (`/server/{id}/schedule…`, endpoints non documentés
//! du panel web, modèle Pterodactyl — cf. `docs/api/REFERENCE.md`).
//!
//! Elles sont exécutées par l'hébergeur, 24 h/24, même ordinateur éteint : à ne pas confondre avec
//! les automatisations locales de l'app (`automation.rs`), exécutées par l'app ou son service.
//!
//! Une tâche planifiée = un nom, une expression cron en cinq champs, des drapeaux (active,
//! « seulement si le serveur est en ligne ») et une suite ordonnée d'actions : `command` (texte de
//! la commande) ou `power` (`start`, `restart`, `stop`, `kill`), chacune après un délai en secondes
//! (`time_offset`), avec « continuer si échec ». Tout est validé ici avant envoi : l'API reçoit
//! toujours une saisie bien formée.

use crate::error::{Error, Result};
use crate::models::{lenient_bool, lenient_i64, lenient_string};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Délai maximal d'une action après la précédente (limite de Pterodactyl).
pub const MAX_TIME_OFFSET_S: i64 = 900;
/// Longueur maximale d'une commande planifiée.
const MAX_COMMAND_LEN: usize = 500;
/// Longueur maximale du nom d'une tâche planifiée.
const MAX_NAME_LEN: usize = 100;
/// Actions d'alimentation acceptées par le panel.
pub const POWER_ACTIONS: &[&str] = &["start", "restart", "stop", "kill"];

/// Expression cron en cinq champs, telle que l'API la renvoie et l'attend.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PanelCron {
    #[serde(default, deserialize_with = "lenient_string")]
    pub minute: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub hour: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub day_of_month: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub month: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub day_of_week: String,
}

/// Une action d'une tâche planifiée.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PanelTask {
    #[serde(deserialize_with = "lenient_i64")]
    pub id: i64,
    /// Ordre d'exécution (croissant).
    #[serde(default, deserialize_with = "lenient_i64")]
    pub sequence_id: i64,
    /// `command` ou `power`.
    #[serde(default, deserialize_with = "lenient_string")]
    pub action: String,
    /// Texte de la commande, ou action d'alimentation.
    #[serde(default, deserialize_with = "lenient_string")]
    pub payload: String,
    /// Délai en secondes après l'action précédente (ou le déclenchement, pour la première).
    #[serde(default, deserialize_with = "lenient_i64")]
    pub time_offset: i64,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub continue_on_failure: bool,
}

/// Une tâche planifiée du panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PanelSchedule {
    #[serde(deserialize_with = "lenient_i64")]
    pub id: i64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub name: String,
    #[serde(default)]
    pub cron: PanelCron,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_active: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub only_when_online: bool,
    /// Exécution en cours côté hébergeur.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_processing: bool,
    #[serde(default)]
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    /// Actions, triées par `sequence_id` à la lecture.
    #[serde(default)]
    pub tasks: Vec<PanelTask>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SchedulesData {
    #[serde(default)]
    pub schedules: Vec<PanelSchedule>,
}

impl SchedulesData {
    /// Tâches planifiées, chacune avec ses actions dans l'ordre d'exécution.
    pub(crate) fn sorted(self) -> Vec<PanelSchedule> {
        self.schedules
            .into_iter()
            .map(|mut s| {
                s.tasks.sort_by_key(|t| (t.sequence_id, t.id));
                s
            })
            .collect()
    }
}

/// Saisie d'une tâche planifiée (création ou modification).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PanelScheduleInput {
    pub name: String,
    pub cron: PanelCron,
    pub is_active: bool,
    pub only_when_online: bool,
}

/// Saisie d'une action (ajout ou modification).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PanelTaskInput {
    /// `command` ou `power`.
    pub action: String,
    pub payload: String,
    pub time_offset: i64,
    pub continue_on_failure: bool,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Unexpected(msg.into())
}

/// Un champ cron : `*`, `*/n`, `a`, `a-b`, `a-b/n`, ou une liste de ceux-là séparés par des virgules,
/// avec des valeurs comprises dans `[min, max]`.
fn valid_cron_field(field: &str, min: u32, max: u32) -> bool {
    let num = |s: &str| s.parse::<u32>().ok().filter(|n| (min..=max).contains(n));
    !field.is_empty()
        && field.split(',').all(|part| {
            let (range, step) = match part.split_once('/') {
                Some((r, s)) => (r, Some(s)),
                None => (part, None),
            };
            let step_ok = step.is_none_or(|s| s.parse::<u32>().is_ok_and(|n| n > 0 && n <= max.max(1)));
            let range_ok = match range {
                "*" => true,
                r => match r.split_once('-') {
                    Some((a, b)) => matches!((num(a), num(b)), (Some(a), Some(b)) if a <= b),
                    None => num(r).is_some(),
                },
            };
            step_ok && range_ok
        })
}

impl PanelCron {
    fn validated(self) -> Result<Self> {
        let c = PanelCron {
            minute: self.minute.trim().to_string(),
            hour: self.hour.trim().to_string(),
            day_of_month: self.day_of_month.trim().to_string(),
            month: self.month.trim().to_string(),
            day_of_week: self.day_of_week.trim().to_string(),
        };
        let checks = [
            (&c.minute, 0, 59, "minute"),
            (&c.hour, 0, 23, "heure"),
            (&c.day_of_month, 1, 31, "jour du mois"),
            (&c.month, 1, 12, "mois"),
            (&c.day_of_week, 0, 7, "jour de la semaine"),
        ];
        for (value, min, max, label) in checks {
            if !valid_cron_field(value, min, max) {
                return Err(invalid(format!(
                    "Planification invalide : champ « {label} » (« {value} »), valeurs de {min} à {max}."
                )));
            }
        }
        Ok(c)
    }
}

impl PanelScheduleInput {
    /// Saisie nettoyée et vérifiée, prête à envoyer.
    pub(crate) fn validated(self) -> Result<Self> {
        let name = self.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
            return Err(invalid(format!("Le nom doit faire de 1 à {MAX_NAME_LEN} caractères.")));
        }
        Ok(Self { name, cron: self.cron.validated()?, ..self })
    }

    /// Corps attendu par `POST /server/{id}/schedule` et `PATCH …/schedule/{id}`.
    pub(crate) fn body(&self) -> Value {
        json!({
            "name": self.name,
            "minute": self.cron.minute,
            "hour": self.cron.hour,
            "day_of_month": self.cron.day_of_month,
            "month": self.cron.month,
            "day_of_week": self.cron.day_of_week,
            "is_active": self.is_active,
            "only_when_online": self.only_when_online,
        })
    }
}

impl PanelTaskInput {
    /// Saisie nettoyée et vérifiée, prête à envoyer.
    pub(crate) fn validated(self) -> Result<Self> {
        if !(0..=MAX_TIME_OFFSET_S).contains(&self.time_offset) {
            return Err(invalid(format!("Le délai doit être compris entre 0 et {MAX_TIME_OFFSET_S} secondes.")));
        }
        let payload = match self.action.as_str() {
            "power" => {
                let p = self.payload.trim();
                if !POWER_ACTIONS.contains(&p) {
                    return Err(invalid("Action d'alimentation inconnue (start, restart, stop ou kill)."));
                }
                p.to_string()
            }
            "command" => {
                // Une seule ligne : un saut de ligne enverrait une seconde commande.
                let p = self.payload.trim();
                if p.is_empty() || p.contains(['\r', '\n']) || p.chars().count() > MAX_COMMAND_LEN {
                    return Err(invalid(format!(
                        "La commande doit tenir sur une ligne, de 1 à {MAX_COMMAND_LEN} caractères."
                    )));
                }
                p.to_string()
            }
            _ => return Err(invalid("Action inconnue (command ou power).")),
        };
        Ok(Self { payload, ..self })
    }

    /// Corps attendu par `POST …/schedule/{id}/task` et `PATCH …/task/{id}`.
    pub(crate) fn body(&self) -> Value {
        json!({
            "action": self.action,
            "payload": self.payload,
            "time_offset": self.time_offset,
            "continue_on_failure": self.continue_on_failure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cron(m: &str, h: &str, dom: &str, mon: &str, dow: &str) -> PanelCron {
        PanelCron {
            minute: m.into(),
            hour: h.into(),
            day_of_month: dom.into(),
            month: mon.into(),
            day_of_week: dow.into(),
        }
    }

    #[test]
    fn reads_the_panel_format_leniently_and_sorts_tasks() {
        let raw = json!({ "schedules": [{
            "id": "12", "name": "Redémarrage", "is_active": 1, "only_when_online": "0",
            "is_processing": false, "next_run_at": "2026-11-19 04:00:00", "last_run_at": null,
            "cron": { "minute": 0, "hour": "4", "day_of_month": "*", "month": "*", "day_of_week": "*" },
            "tasks": [
                { "id": 2, "sequence_id": 2, "action": "power", "payload": "restart", "time_offset": 60, "continue_on_failure": 0 },
                { "id": 1, "sequence_id": 1, "action": "command", "payload": "say Redémarrage dans 1 min", "time_offset": 0, "continue_on_failure": 1 }
            ]
        }]});
        let list = serde_json::from_value::<SchedulesData>(raw).unwrap().sorted();
        let s = &list[0];
        assert_eq!(s.id, 12);
        assert!(s.is_active && !s.only_when_online);
        assert_eq!(s.cron.minute, "0");
        assert_eq!(s.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2]);
        assert!(s.tasks[0].continue_on_failure);
        assert_eq!(s.tasks[1].time_offset, 60);
    }

    #[test]
    fn empty_or_missing_list_is_empty() {
        assert!(serde_json::from_value::<SchedulesData>(json!({})).unwrap().sorted().is_empty());
        assert!(serde_json::from_value::<SchedulesData>(json!({ "schedules": [] })).unwrap().sorted().is_empty());
    }

    #[test]
    fn cron_fields_are_checked_against_their_range() {
        for ok in ["*", "*/5", "0", "59", "1-5", "0,15,30,45", "10-50/10"] {
            assert!(valid_cron_field(ok, 0, 59), "{ok}");
        }
        for bad in ["", "60", "-1", "5-1", "*/0", "a", "1,,2", "1-", "*/x", "0 0"] {
            assert!(!valid_cron_field(bad, 0, 59), "{bad}");
        }
        assert!(cron("0", "4", "*", "*", "1-5").validated().is_ok());
        assert!(cron("0", "24", "*", "*", "*").validated().is_err(), "heure hors bornes");
        assert!(cron("0", "4", "0", "*", "*").validated().is_err(), "jour du mois 0");
    }

    #[test]
    fn schedule_input_is_trimmed_and_serialized_like_the_panel() {
        let input = PanelScheduleInput {
            name: "  Nuit  ".into(),
            cron: cron(" 0 ", "4", "*", "*", "*"),
            is_active: true,
            only_when_online: false,
        }
        .validated()
        .unwrap();
        assert_eq!(
            input.body(),
            json!({ "name": "Nuit", "minute": "0", "hour": "4", "day_of_month": "*", "month": "*",
                    "day_of_week": "*", "is_active": true, "only_when_online": false })
        );
        let unnamed = PanelScheduleInput { name: "  ".into(), ..input.clone() };
        assert!(unnamed.validated().is_err());
    }

    #[test]
    fn task_input_rejects_what_the_panel_would_misread() {
        let task = |action: &str, payload: &str, offset: i64| PanelTaskInput {
            action: action.into(),
            payload: payload.into(),
            time_offset: offset,
            continue_on_failure: false,
        };
        assert_eq!(task("power", " restart ", 0).validated().unwrap().payload, "restart");
        assert!(task("power", "reboot", 0).validated().is_err());
        assert!(task("command", "say a\nop pirate", 0).validated().is_err(), "deux commandes");
        assert!(task("command", "   ", 0).validated().is_err());
        assert!(task("command", "say bonjour", MAX_TIME_OFFSET_S + 1).validated().is_err());
        assert!(task("command", "say bonjour", -1).validated().is_err());
        assert!(task("backup", "x", 0).validated().is_err());
        assert_eq!(
            task("command", "say bonjour", 30).validated().unwrap().body(),
            json!({ "action": "command", "payload": "say bonjour", "time_offset": 30, "continue_on_failure": false })
        );
    }
}
