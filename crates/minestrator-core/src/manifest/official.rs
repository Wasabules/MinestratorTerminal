//! Étapes `manual` du plan reliées aux **outils du MCP officiel** qui les réalisent : version du
//! jeu et plateforme (`change_version`), Java (`manage_java`), fichier de démarrage
//! (`set_startup_config`). Le manifeste ne les applique pas lui-même (endpoints d'écriture non
//! documentés, jamais essayés : `POST /server/{id}/version`, `PATCH …/java/version`, `PATCH
//! …/startup/file`), mais l'étape dit quel outil appeler, avec quels arguments, et s'il est
//! appelable **maintenant** par ce client : passerelle vers le MCP officiel active
//! (`McpConfig.official_gateway`, Réglages → MCP), groupe `official` actif et outil relayé.
//!
//! Arguments tirés des schémas de l'officiel (relevé `tools/list` du 2026-11-19, voir
//! `official_relay.rs`) :
//! - `change_version` (destructif : `confirm_name` = nom exact du serveur, donné par l'utilisateur) :
//!   chargeur `forge` | `neoforge` | `fabric` → `loader` + `game_version` (+ `build` facultatif,
//!   `get_content_files type=loader_builds`) ; autre distribution (Paper, Spigot, Vanilla…) →
//!   `version_id` = `id` d'une entrée `search_game_content type=version`. Une version de Minecraft
//!   antérieure est refusée par la plateforme (seule une réinstallation propre le permet) ;
//! - `manage_java` : sans paramètre, liste les versions et leurs identifiants ; puis
//!   `java_version_id` ;
//! - `set_startup_config` : `file` (la passerelle le masque au profit de notre
//!   `set_startup_params`, qui ne change pas le JAR : le panel reste alors la voie).

use super::{Facts, ServerSpec, Status, Step};
use crate::Core;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Valeur à faire donner par l'utilisateur, jamais devinée par l'agent.
const CONFIRM_PLACEHOLDER: &str = "<nom exact du serveur, donné par l'utilisateur>";

/// Outil officiel qui réalise une étape `manual`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OfficialHint {
    pub tool: &'static str,
    /// Arguments à passer ; `<…>` : valeur à obtenir d'abord (`before`) ou de l'utilisateur.
    pub arguments: Value,
    /// Lectures à faire avant (identifiant à choisir…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Appelable maintenant par ce client (passerelle active, outil relayé) ?
    pub available: bool,
    pub availability: String,
}

/// Outil officiel d'une étape `manual`, d'après son identifiant.
fn tool_for(step_id: &str) -> Option<&'static str> {
    match step_id {
        "server.game_version" | "server.loader" => Some("change_version"),
        "server.java" => Some("manage_java"),
        "startup.jar_file" => Some("set_startup_config"),
        _ => None,
    }
}

/// Outils officiels utiles à ce plan (pour n'interroger la disponibilité que de ceux-là).
pub(crate) fn tools_needed(steps: &[Step]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = steps
        .iter()
        .filter(|s| s.status == Status::Manual)
        .filter_map(|s| tool_for(&s.id))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn nonempty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// `a` est-elle une version de Minecraft antérieure à `b` (`1.20.4` < `1.21`, `26.1` > `1.21.4`) ?
/// Faux si l'une des deux n'est pas numérique (snapshot…) : rien à conclure.
pub(crate) fn older(a: &str, b: &str) -> bool {
    let parse =
        |v: &str| -> Option<Vec<u64>> { v.trim().split('.').map(|p| p.parse().ok()).collect() };
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x < y,
        _ => false,
    }
}

/// `change_version` pour la version du jeu et la plateforme voulues (celles du manifeste, à
/// défaut celles du serveur). `None` : aucune voie officielle (Quilt, version inconnue).
fn change_version(server_id: i64, spec: &ServerSpec, f: &Facts) -> Option<OfficialHint> {
    let gv = nonempty(spec.game_version.as_deref()).or(nonempty(f.game_version.as_deref()));
    let loader = nonempty(spec.loader.as_deref())
        .or(nonempty(f.loader.as_deref()))
        .map(str::to_ascii_lowercase);
    let mut note = vec![
        "destructif : `confirm_name` = nom exact du serveur, donné par l'utilisateur (demandé à l'humain si son client le permet) ; snapshot avant (create_snapshot) ; opération asynchrone : suivre avec server_status ou wait_for".to_string(),
    ];
    if let (Some(want), Some(have)) = (gv, nonempty(f.game_version.as_deref())) {
        if older(want, have) {
            note.push(format!(
                "{want} est ANTÉRIEURE à {have} : la plateforme refuse un retour en arrière (monde illisible) ; seule une réinstallation propre (`reinstall: true`, contenu perdu, snapshot obligatoire) le permet"
            ));
        }
    }
    let (arguments, before) = match loader.as_deref() {
        Some(l @ ("fabric" | "forge" | "neoforge")) => {
            let gv = gv?;
            (
                json!({ "server_id": server_id, "loader": l, "game_version": gv, "confirm_name": CONFIRM_PLACEHOLDER }),
                vec![format!(
                    "facultatif : get_content_files {{\"type\": \"loader_builds\", \"loader\": \"{l}\", \"game_version\": \"{gv}\"}} pour choisir `build` (sinon le dernier build stable)"
                )],
            )
        }
        // Quilt n'est pas proposé par change_version.
        Some("quilt") => return None,
        other => {
            // Plateforme inconnue (jar explicite) : à préciser dans la recherche.
            let platform = other.unwrap_or("<plateforme : paper, spigot, vanilla…>");
            let wanted = [other, gv]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let mut args = json!({
                "server_id": server_id,
                "version_id": if wanted.is_empty() {
                    "<id de l'entrée voulue, via search_game_content>".to_string()
                } else {
                    format!("<id de l'entrée {wanted}, via search_game_content>")
                },
                "confirm_name": CONFIRM_PLACEHOLDER,
            });
            if let Some(gv) = gv {
                args["game_version"] = json!(gv);
            }
            let filter = gv
                .map(|g| format!(", \"game_version\": \"{g}\""))
                .unwrap_or_default();
            (
                args,
                vec![format!(
                    "search_game_content {{\"type\": \"version\", \"query\": \"{platform}\"{filter}}} : prendre l'`id` de l'entrée voulue"
                )],
            )
        }
    };
    Some(OfficialHint {
        tool: "change_version",
        arguments,
        before,
        note: Some(note.join(" ; ")),
        available: false,
        availability: String::new(),
    })
}

fn manage_java(server_id: i64, want: i64) -> OfficialHint {
    OfficialHint {
        tool: "manage_java",
        arguments: json!({ "server_id": server_id, "java_version_id": format!("<id de Java {want}>") }),
        before: vec![format!(
            "manage_java {{\"server_id\": {server_id}}} (sans autre paramètre) : versions de Java disponibles et leurs identifiants"
        )],
        note: Some("redémarrage nécessaire après le changement".into()),
        available: false,
        availability: String::new(),
    }
}

fn set_startup_file(server_id: i64, file: &str) -> OfficialHint {
    OfficialHint {
        tool: "set_startup_config",
        arguments: json!({ "server_id": server_id, "file": file }),
        before: Vec::new(),
        note: Some(
            "le JAR doit exister sur le serveur (list_files) ; redémarrage nécessaire".into(),
        ),
        available: false,
        availability: String::new(),
    }
}

/// Texte de disponibilité d'un outil officiel.
fn availability(tool: &str, state: &Result<(), String>) -> (bool, String) {
    match state {
        Ok(()) => (true, "disponible maintenant : outil relayé par la passerelle vers le MCP officiel".into()),
        Err(why) if tool == "set_startup_config" && why.contains("set_startup_params") => (
            false,
            format!("non disponible ici : {why} — set_startup_params ne change pas le JAR : passer par le panel (Démarrage)"),
        ),
        Err(why) => (false, format!("non disponible ici : {why}")),
    }
}

/// Relie chaque étape `manual` de version, plateforme, Java ou JAR à son outil officiel. `state` :
/// disponibilité de chaque outil (voir [`crate::official_relay::gateway_availability`]). Pur.
pub(crate) fn annotate(
    steps: &mut [Step],
    server_id: i64,
    spec: Option<&ServerSpec>,
    startup_jar: Option<&str>,
    facts: &Facts,
    state: &HashMap<&'static str, Result<(), String>>,
) {
    let version = spec.and_then(|s| change_version(server_id, s, facts));
    for step in steps.iter_mut().filter(|s| s.status == Status::Manual) {
        let hint = match step.id.as_str() {
            "server.game_version" | "server.loader" => version.clone(),
            "server.java" => spec
                .and_then(|s| s.java_version)
                .map(|j| manage_java(server_id, j)),
            "startup.jar_file" => nonempty(startup_jar).map(|j| set_startup_file(server_id, j)),
            _ => None,
        };
        let Some(mut hint) = hint else { continue };
        let missing = Err("disponibilité inconnue".to_string());
        let (ok, text) = availability(hint.tool, state.get(hint.tool).unwrap_or(&missing));
        hint.available = ok;
        hint.availability = text;
        step.official = Some(hint);
    }
}

/// [`annotate`] avec la disponibilité réelle (passerelle, catalogue de l'officiel en cache).
pub(crate) async fn annotate_live(
    core: &Core,
    steps: &mut [Step],
    server_id: i64,
    spec: Option<&ServerSpec>,
    startup_jar: Option<&str>,
    facts: &Facts,
) {
    let needed = tools_needed(steps);
    if needed.is_empty() {
        return;
    }
    let mut state = HashMap::new();
    for tool in needed {
        state.insert(
            tool,
            crate::official_relay::gateway_availability(core, tool).await,
        );
    }
    annotate(steps, server_id, spec, startup_jar, facts, &state);
}

#[cfg(test)]
mod tests {
    use super::super::{plan_server, plan_startup, StartupSpec};
    use super::*;
    use crate::models::Startup;

    fn facts(loader: &str, version: &str) -> Facts {
        Facts {
            name: "Survie".into(),
            family: "minecraft".into(),
            egg_id: Some(1),
            egg_name: "Minecraft".into(),
            game_version: Some(version.into()),
            loader: Some(loader.into()),
            startup: Some(Startup {
                command: "java -jar {{SERVER_JARFILE}}".into(),
                jar_file: "server.jar".into(),
                java_memory: 4,
                image: "ghcr.io/pterodactyl/yolks:java_17".into(),
                java_version: 3,
            }),
        }
    }

    fn all(state: Result<(), String>) -> HashMap<&'static str, Result<(), String>> {
        ["change_version", "manage_java", "set_startup_config"]
            .into_iter()
            .map(|t| (t, state.clone()))
            .collect()
    }

    fn hint<'a>(steps: &'a [Step], id: &str) -> &'a OfficialHint {
        steps
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.official.as_ref())
            .unwrap_or_else(|| panic!("{id} sans outil officiel"))
    }

    #[test]
    fn version_and_platform_point_to_change_version() {
        let f = facts("paper", "1.21.1");
        // Paper → Paper plus récent : entrée du catalogue des versions.
        let spec = ServerSpec {
            game_version: Some("1.21.4".into()),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        annotate(&mut steps, 42, Some(&spec), None, &f, &all(Ok(())));
        let h = hint(&steps, "server.game_version");
        assert_eq!(h.tool, "change_version");
        assert_eq!(h.arguments["server_id"], 42);
        assert_eq!(h.arguments["game_version"], "1.21.4");
        assert!(h.arguments["version_id"]
            .as_str()
            .unwrap()
            .contains("search_game_content"));
        assert_eq!(
            h.arguments["confirm_name"], CONFIRM_PLACEHOLDER,
            "jamais deviné"
        );
        assert!(
            h.before[0].contains("\"type\": \"version\"")
                && h.before[0].contains("\"query\": \"paper\"")
        );
        assert!(h.available && h.availability.contains("disponible maintenant"));
        assert!(!h.note.as_deref().unwrap().contains("ANTÉRIEURE"));
        // Paper → Fabric : chargeur par son nom, sans identifiant ; les deux étapes pointent le même appel.
        let spec = ServerSpec {
            game_version: Some("1.21.4".into()),
            loader: Some("Fabric".into()),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        annotate(&mut steps, 42, Some(&spec), None, &f, &all(Ok(())));
        let h = hint(&steps, "server.loader");
        assert_eq!(
            (
                h.arguments["loader"].as_str(),
                h.arguments["game_version"].as_str()
            ),
            (Some("fabric"), Some("1.21.4"))
        );
        assert!(h.arguments.get("version_id").is_none());
        assert!(h.before[0].contains("loader_builds"));
        assert_eq!(hint(&steps, "server.game_version"), h);
    }

    #[test]
    fn an_older_version_is_flagged_and_quilt_has_no_official_path() {
        let f = facts("fabric", "1.21.4");
        let spec = ServerSpec {
            game_version: Some("1.20.1".into()),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        annotate(&mut steps, 1, Some(&spec), None, &f, &all(Ok(())));
        let h = hint(&steps, "server.game_version");
        assert_eq!(h.arguments["loader"], "fabric");
        assert!(h.note.as_deref().unwrap().contains("ANTÉRIEURE"));
        assert!(older("1.20.4", "1.21") && older("1.21.4", "26.1") && !older("1.21.4", "1.21.4"));
        assert!(!older("24w14a", "1.21"), "snapshot : rien à conclure");
        let spec = ServerSpec {
            loader: Some("quilt".into()),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        annotate(&mut steps, 1, Some(&spec), None, &f, &all(Ok(())));
        let quilt = steps.iter().find(|s| s.id == "server.loader").unwrap();
        assert_eq!(quilt.status, Status::Manual);
        assert!(quilt.official.is_none());
    }

    #[test]
    fn java_and_jar_file_point_to_their_tools() {
        let f = facts("paper", "1.21.4");
        let spec = ServerSpec {
            java_version: Some(21),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        steps.extend(plan_startup(
            &StartupSpec {
                parameters: None,
                jar_file: Some("paper-1.21.4.jar".into()),
            },
            f.startup.as_ref(),
        ));
        let mut state = all(Err(
            "passerelle vers le MCP officiel désactivée : l'activer dans Réglages → MCP".into(),
        ));
        state.insert(
            "set_startup_config",
            Err("masqué par la passerelle (remplacé par l'outil `set_startup_params`)".into()),
        );
        annotate(
            &mut steps,
            9,
            Some(&spec),
            Some("paper-1.21.4.jar"),
            &f,
            &state,
        );
        let java = hint(&steps, "server.java");
        assert_eq!(java.tool, "manage_java");
        assert_eq!(java.arguments["java_version_id"], "<id de Java 21>");
        assert!(java.before[0].contains("manage_java"));
        assert!(!java.available && java.availability.contains("Réglages → MCP"));
        let jar = hint(&steps, "startup.jar_file");
        assert_eq!(
            (jar.tool, jar.arguments["file"].as_str()),
            ("set_startup_config", Some("paper-1.21.4.jar"))
        );
        assert!(!jar.available && jar.availability.contains("panel"));
        // Sérialisé dans l'étape (sortie de plan_manifest).
        let v =
            serde_json::to_value(steps.iter().find(|s| s.id == "server.java").unwrap()).unwrap();
        assert_eq!(v["official"]["tool"], "manage_java");
        assert_eq!(v["official"]["available"], false);
    }

    #[test]
    fn conforming_steps_and_unknown_availability_are_handled() {
        let f = facts("paper", "1.21.4");
        let spec = ServerSpec {
            game_version: Some("1.21.4".into()),
            java_version: Some(17),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        assert!(
            tools_needed(&steps).is_empty(),
            "rien de manuel : aucune requête de disponibilité"
        );
        annotate(&mut steps, 1, Some(&spec), None, &f, &HashMap::new());
        assert!(steps.iter().all(|s| s.official.is_none()));
        let spec = ServerSpec {
            java_version: Some(21),
            ..Default::default()
        };
        let mut steps = plan_server(&spec, &f);
        assert_eq!(tools_needed(&steps), vec!["manage_java"]);
        annotate(&mut steps, 1, Some(&spec), None, &f, &HashMap::new());
        assert!(!hint(&steps, "server.java").available);
        // Sans outil officiel sérialisé : pas de clé `official`.
        let v = serde_json::to_value(Step::new("x", Status::Ok, "conforme")).unwrap();
        assert!(v.get("official").is_none());
    }
}
