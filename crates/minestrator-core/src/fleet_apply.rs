//! Outil MCP `apply_to_servers` : exécute le **même** outil modifiant sur plusieurs serveurs d'un
//! réseau (même plugin, même réglage, même dossier de configs…).
//!
//! - **Liste blanche** ([`RELAYABLE_TOOLS`]) : outils bornés à un serveur, jamais un outil de
//!   `INFRA_TOOLS` ni `apply_to_servers` lui-même (des tests le vérifient). `server_id` est injecté
//!   pour chaque serveur ; `arguments` ne peut désigner aucun serveur.
//! - **Contrôles** : ceux d'un appel direct (`mcp::call`) — groupe actif, liste blanche d'env,
//!   écritures permises — vérifiés une fois pour l'outil relayé ([`policy_refusal`]) ; par serveur :
//!   existence sur le compte, jeu (`tool_for_game`), mode constructeur (`agent_guard::admit`) et
//!   portée du Copilote. Chaque appel passe par `mcp::dispatch` : la garde des agents en fait une
//!   étape annulable (l'outil lui-même n'en a pas, `Plan::Skip`).
//! - **`dry_run`** : transmis aux outils qui savent simuler ([`DRY_RUN_TOOLS`]) ; pour les autres,
//!   `dry_run=true` n'exécute rien (« simulé »).
//! - **Rapport** par serveur (ok, erreur, ignoré, simulé, non exécuté), progression, annulation,
//!   arrêt au premier échec (`stop_on_error`, défaut).

use crate::local_sync::bool_arg;
use crate::mcp_ctx;
use crate::Core;
use serde_json::{json, Map, Value};
use std::future::Future;
use std::pin::Pin;

/// Outils relayables : modifiants, bornés à UN serveur (`server_id`), hors `INFRA_TOOLS`.
/// `set_startup_params` en est exclu : commande de démarrage = configuration persistante
/// (`INFRA_TOOLS`).
pub(crate) const RELAYABLE_TOOLS: &[&str] = &[
    "install_mod",
    "update_mods",
    "set_config",
    "set_property",
    "write_file",
    "edit_file",
    "upload_file",
    "delete_path",
    "rename_path",
    "create_dir",
    "sync_dir",
    "deploy_and_verify",
    "send_command",
];

/// Outils relayés qui savent simuler (`dry_run`).
pub(crate) const DRY_RUN_TOOLS: &[&str] = &["set_config", "sync_dir"];

/// Serveurs au plus par appel.
pub(crate) const MAX_SERVERS: usize = 20;

/// Clés qui désignent un serveur ou une MyBox : interdites dans `arguments` (alignées sur
/// `agent_guard::scope::SERVER_KEYS` et `SERVER_LIST_KEYS`).
const SERVER_ARG_KEYS: &[&str] = &[
    "server_id",
    "other_server_id",
    "source_server_id",
    "target_server_id",
    "proxy_server_id",
    "server_ids",
    "backend_server_ids",
    "mybox_id",
];

/// Sortie d'un outil gardée par serveur dans le rapport.
const OUTPUT_CAP: usize = 4000;

/// Arguments d'`apply_to_servers`, vérifiés.
#[derive(Debug, Clone)]
pub(crate) struct ApplyRequest {
    pub(crate) server_ids: Vec<i64>,
    pub(crate) tool: String,
    pub(crate) arguments: Map<String, Value>,
    pub(crate) stop_on_error: bool,
    pub(crate) dry_run: Option<bool>,
}

pub(crate) fn parse_request(args: &Value) -> Result<ApplyRequest, String> {
    let raw = args
        .get("server_ids")
        .and_then(Value::as_array)
        .ok_or("paramètre `server_ids` (tableau d'identifiants de serveurs) requis")?;
    let mut server_ids: Vec<i64> = Vec::new();
    for v in raw {
        let id = v.as_i64().ok_or("`server_ids` : identifiants entiers attendus")?;
        if !server_ids.contains(&id) {
            server_ids.push(id);
        }
    }
    if server_ids.is_empty() {
        return Err("`server_ids` vide : donne au moins un serveur (list_servers)".into());
    }
    if server_ids.len() > MAX_SERVERS {
        return Err(format!("{} serveurs : {MAX_SERVERS} au plus par appel", server_ids.len()));
    }
    let tool = args
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("paramètre `tool` (nom de l'outil à appliquer) requis")?
        .to_string();
    if tool == "apply_to_servers" {
        return Err("apply_to_servers ne peut pas se relayer lui-même".into());
    }
    if crate::mcp::INFRA_TOOLS.contains(&tool.as_str()) {
        return Err(format!("« {tool} » touche l'infrastructure ou plusieurs serveurs : jamais relayé par apply_to_servers"));
    }
    if !RELAYABLE_TOOLS.contains(&tool.as_str()) {
        return Err(format!("« {tool} » n'est pas relayable ; outils permis : {}", RELAYABLE_TOOLS.join(", ")));
    }
    let arguments = match args.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err("`arguments` doit être un objet (arguments de l'outil, sans server_id)".into()),
    };
    if let Some(k) = SERVER_ARG_KEYS.iter().find(|k| arguments.contains_key(**k)) {
        return Err(format!(
            "`arguments` ne doit pas contenir `{k}` : server_id est injecté pour chaque serveur de server_ids"
        ));
    }
    Ok(ApplyRequest {
        server_ids,
        tool,
        arguments,
        stop_on_error: bool_arg(args, "stop_on_error")?.unwrap_or(true),
        dry_run: bool_arg(args, "dry_run")?,
    })
}

/// Contrôles d'un appel direct à l'outil relayé, indépendants du serveur (voir `mcp::call`) :
/// groupe actif, liste blanche d'env, écritures permises.
pub(crate) fn policy_refusal(core: &Core, tool: &str) -> Option<String> {
    if !crate::mcp_catalog::tool_in_toolsets(crate::mcp_catalog::active_toolsets(core).as_ref(), tool) {
        return Some(crate::mcp_catalog::inactive_message(tool));
    }
    if !crate::mcp::env_allows(tool) {
        return Some(format!("Outil « {tool} » non autorisé par la politique de permissions."));
    }
    if crate::mcp::is_write_tool(tool) && !core.get_mcp_config().allow_writes {
        return Some("Actions modifiantes désactivées (mode lecture seule) dans les réglages.".into());
    }
    None
}

/// Serveur du compte.
#[derive(Debug, Clone)]
pub(crate) struct AccountServer {
    pub(crate) id: i64,
    pub(crate) name: String,
    /// Famille de jeu (`games.rs`).
    pub(crate) family: String,
}

/// Ce dont l'outil a besoin : serveurs du compte, contrôles par serveur, appel d'un outil.
pub(crate) trait Relay: Sync {
    fn servers(&self) -> impl Future<Output = Result<Vec<AccountServer>, String>> + Send;
    /// Refus propre à cet appel (mode constructeur, portée du Copilote) ; `None` : permis.
    fn refusal(&self, tool: &str, args: &Value) -> impl Future<Output = Option<String>> + Send;
    fn call(&self, tool: &str, args: Value) -> impl Future<Output = Result<String, String>> + Send;
}

/// Compte réel : `mcp::dispatch` (garde des agents comprise) et `agent_guard::admit`.
struct CoreRelay<'a> {
    core: &'a Core,
    cfg: crate::mcp::McpConfig,
}

/// `mcp::dispatch` dans une boîte `dyn … + Send` : casse le cycle de types dispatch → outil →
/// dispatch (même raison que `agent_guard::guarded_write`).
fn dispatch_boxed<'a>(
    core: &'a Core,
    tool: &'a str,
    args: Value,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(crate::mcp::dispatch(core, tool, args))
}

impl Relay for CoreRelay<'_> {
    async fn servers(&self) -> Result<Vec<AccountServer>, String> {
        let list = self.core.list_servers().await.map_err(|e| e.to_string())?;
        Ok(list
            .servers
            .into_iter()
            .map(|s| AccountServer { id: s.id, name: s.name, family: s.capabilities.family })
            .collect())
    }
    async fn refusal(&self, tool: &str, args: &Value) -> Option<String> {
        if let Some(refus) = crate::official_relay::env_scope_violation(args) {
            return Some(refus);
        }
        crate::agent_guard::admit(self.core, &self.cfg, tool, args).await
    }
    async fn call(&self, tool: &str, args: Value) -> Result<String, String> {
        dispatch_boxed(self.core, tool, args).await
    }
}

/// Sortie d'un outil pour le rapport : JSON gardé tel quel s'il est court, texte tronqué sinon.
fn output_value(out: &str) -> Value {
    if out.len() <= OUTPUT_CAP {
        if let Ok(v) = serde_json::from_str::<Value>(out) {
            if v.is_object() || v.is_array() {
                return v;
            }
        }
        return json!(out);
    }
    json!(crate::util::truncate_on_boundary(out, OUTPUT_CAP, "… (sortie tronquée)"))
}

fn row(id: i64, name: Option<&str>, status: &str, detail: Value) -> Value {
    let mut r = json!({ "server_id": id, "name": name, "status": status });
    if let (Some(obj), Value::Object(extra)) = (r.as_object_mut(), detail) {
        obj.extend(extra);
    }
    r
}

/// Applique l'outil serveur par serveur, dans l'ordre de `server_ids`.
pub(crate) async fn apply_with<R: Relay>(r: &R, req: &ApplyRequest) -> Result<Value, String> {
    let account = r.servers().await.map_err(|e| format!("liste des serveurs illisible : {e} ; rien n'a été fait"))?;
    let total = req.server_ids.len();
    let tool = req.tool.as_str();
    let simulates = DRY_RUN_TOOLS.contains(&tool);
    let mut rows = Vec::with_capacity(total);
    let mut stopped_by: Option<i64> = None;
    let mut cancelled = false;
    for (i, &id) in req.server_ids.iter().enumerate() {
        let known = account.iter().find(|s| s.id == id);
        let name = known.map(|s| s.name.as_str());
        if let Some(first) = stopped_by {
            let why = format!("arrêt après l'erreur sur le serveur {first} (stop_on_error)");
            rows.push(row(id, name, "non exécuté", json!({ "why": why })));
            continue;
        }
        if cancelled || mcp_ctx::cancelled() {
            cancelled = true;
            rows.push(row(id, name, "non exécuté", json!({ "why": "annulé" })));
            continue;
        }
        mcp_ctx::progress(i as f64, Some(total as f64), &format!("{tool} sur le serveur {id} ({}/{total})", i + 1)).await;
        let Some(server) = known else {
            rows.push(row(id, None, "ignoré", json!({ "why": "serveur introuvable sur ce compte (list_servers)" })));
            continue;
        };
        if !crate::mcp::tool_for_game(tool, &server.family) {
            let why = format!("{tool} ne s'applique pas à ce jeu ({})", server.family);
            rows.push(row(id, name, "ignoré", json!({ "why": why })));
            continue;
        }
        let mut args = Value::Object(req.arguments.clone());
        args["server_id"] = json!(id);
        match req.dry_run {
            Some(d) if simulates => args["dry_run"] = json!(d),
            Some(true) => {
                let why = format!("{tool} ne sait pas simuler : rien n'est exécuté (relance avec dry_run=false)");
                rows.push(row(id, name, "simulé", json!({ "why": why })));
                continue;
            }
            _ => {}
        }
        if let Some(refus) = r.refusal(tool, &args).await {
            rows.push(row(id, name, "ignoré", json!({ "why": refus })));
            continue;
        }
        match r.call(tool, args).await {
            Ok(out) => rows.push(row(id, name, "ok", json!({ "output": output_value(&out) }))),
            Err(e) => {
                let error = crate::util::truncate_on_boundary(&e, OUTPUT_CAP, "…");
                rows.push(row(id, name, "erreur", json!({ "error": error })));
                if req.stop_on_error {
                    stopped_by = Some(id);
                }
            }
        }
    }
    mcp_ctx::progress(total as f64, Some(total as f64), "terminé").await;
    let count = |s: &str| rows.iter().filter(|r| r["status"] == s).count();
    let counts = json!({
        "ok": count("ok"),
        "erreur": count("erreur"),
        "ignoré": count("ignoré"),
        "simulé": count("simulé"),
        "non exécuté": count("non exécuté"),
    });
    let next = if req.dry_run == Some(true) {
        "Simulation : montre ce rapport à l'utilisateur, puis relance avec dry_run=false pour appliquer."
    } else {
        "Chaque modification est une étape de la garde des agents : list_changesets puis rollback_changeset pour la défaire, serveur par serveur."
    };
    Ok(json!({
        "tool": tool,
        "server_ids": req.server_ids,
        "dry_run": req.dry_run,
        "stop_on_error": req.stop_on_error,
        "counts": counts,
        "servers": rows,
        "stopped_after_error_on": stopped_by,
        "cancelled": cancelled,
        "next": next,
    }))
}

/// Outil MCP `apply_to_servers`.
pub(crate) async fn apply_to_servers_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = parse_request(args)?;
    if let Some(refus) = policy_refusal(core, &req.tool) {
        return Err(refus);
    }
    let relay = CoreRelay { core, cfg: core.get_mcp_config() };
    let report = apply_with(&relay, &req).await?;
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    Ok(core.redact_ai(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_sync::testing::block;
    use std::sync::Mutex;

    /// Compte simulé : serveurs, refus par serveur, appels enregistrés, échecs choisis.
    struct FakeRelay {
        servers: Vec<AccountServer>,
        refuse: Vec<i64>,
        fail: Vec<i64>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl FakeRelay {
        fn new() -> Self {
            let s = |id: i64, family: &str| AccountServer { id, name: format!("srv{id}"), family: family.into() };
            FakeRelay {
                servers: vec![s(1, "minecraft"), s(2, "minecraft"), s(3, "satisfactory"), s(4, "minecraft")],
                refuse: Vec::new(),
                fail: Vec::new(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn called(&self) -> Vec<Value> {
            self.calls.lock().unwrap().iter().map(|(_, a)| a.clone()).collect()
        }
    }

    impl Relay for FakeRelay {
        async fn servers(&self) -> Result<Vec<AccountServer>, String> {
            Ok(self.servers.clone())
        }
        async fn refusal(&self, _tool: &str, args: &Value) -> Option<String> {
            let id = args["server_id"].as_i64()?;
            self.refuse.contains(&id).then(|| "Refusé (mode constructeur)".to_string())
        }
        async fn call(&self, tool: &str, args: Value) -> Result<String, String> {
            let id = args["server_id"].as_i64().unwrap_or_default();
            self.calls.lock().unwrap().push((tool.to_string(), args));
            if self.fail.contains(&id) {
                Err(format!("échec sur {id}"))
            } else {
                Ok(format!("fait sur {id}"))
            }
        }
    }

    fn req(v: Value) -> ApplyRequest {
        parse_request(&v).unwrap()
    }

    #[test]
    fn relayable_tools_are_single_server_writes_never_infrastructure() {
        let tools = crate::mcp::tool_list().as_array().unwrap();
        let schema = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("{n} absent"))["inputSchema"].clone();
        for t in RELAYABLE_TOOLS {
            assert!(crate::mcp::WRITE_TOOLS.contains(t), "{t} : modifiant exposé");
            assert!(!crate::mcp::INFRA_TOOLS.contains(t), "{t} : jamais un outil d'infrastructure");
            assert_ne!(*t, "apply_to_servers");
            let s = schema(t);
            assert!(s["required"].as_array().unwrap().contains(&json!("server_id")), "{t} : borné à un serveur");
            for k in SERVER_ARG_KEYS.iter().filter(|k| **k != "server_id") {
                assert!(s["properties"].get(*k).is_none(), "{t} désigne un autre serveur par `{k}`");
            }
        }
        for t in DRY_RUN_TOOLS {
            assert!(RELAYABLE_TOOLS.contains(t));
            assert_eq!(schema(t)["properties"]["dry_run"]["type"], "boolean", "{t} sait simuler");
        }
        // Peut relayer un outil qui lit le disque de l'ordinateur : il en hérite le classement.
        if RELAYABLE_TOOLS.iter().any(|t| crate::mcp::LOCAL_DISK_TOOLS.contains(t)) {
            assert!(crate::mcp::LOCAL_DISK_TOOLS.contains(&"apply_to_servers"));
        }
        assert!(!RELAYABLE_TOOLS.contains(&"set_startup_params"), "commande de démarrage : INFRA_TOOLS");
    }

    #[test]
    fn requests_are_checked() {
        let ok = req(json!({ "server_ids": [2, 1, 2], "tool": "set_property", "arguments": { "property": "motd", "value": "x" } }));
        assert_eq!(ok.server_ids, [2, 1], "doublons retirés, ordre gardé");
        assert!(ok.stop_on_error && ok.dry_run.is_none());
        let err = |v: Value| parse_request(&v).unwrap_err();
        assert!(err(json!({ "tool": "write_file" })).contains("server_ids"));
        assert!(err(json!({ "server_ids": [], "tool": "write_file" })).contains("vide"));
        assert!(err(json!({ "server_ids": ["1"], "tool": "write_file" })).contains("entiers"));
        let many: Vec<i64> = (1..=21).collect();
        assert!(err(json!({ "server_ids": many, "tool": "write_file" })).contains("au plus"));
        assert!(err(json!({ "server_ids": [1], "tool": "apply_to_servers" })).contains("lui-même"));
        assert!(err(json!({ "server_ids": [1], "tool": "mybox_delete_server" })).contains("infrastructure"));
        assert!(err(json!({ "server_ids": [1], "tool": "set_startup_params" })).contains("infrastructure"));
        assert!(err(json!({ "server_ids": [1], "tool": "read_file" })).contains("pas relayable"));
        let e = err(json!({ "server_ids": [1], "tool": "write_file", "arguments": { "server_id": 9, "path": "/a" } }));
        assert!(e.contains("server_id"), "{e}");
        assert!(err(json!({ "server_ids": [1], "tool": "write_file", "arguments": { "mybox_id": 9 } })).contains("mybox_id"));
        assert!(err(json!({ "server_ids": [1], "tool": "write_file", "arguments": [] })).contains("objet"));
        assert!(err(json!({ "server_ids": [1], "tool": "write_file", "dry_run": 1 })).contains("booléen"));
    }

    #[test]
    fn each_server_gets_its_own_call_and_its_own_line() {
        block(async {
            let mut r = FakeRelay::new();
            r.refuse = vec![4];
            let rq = req(json!({ "server_ids": [1, 9, 3, 4, 2], "tool": "set_property", "arguments": { "property": "motd", "value": "Réseau" } }));
            let out = apply_with(&r, &rq).await.unwrap();
            // server_id injecté ; les arguments recopiés à l'identique.
            assert_eq!(r.called(), [
                json!({ "property": "motd", "value": "Réseau", "server_id": 1 }),
                json!({ "property": "motd", "value": "Réseau", "server_id": 2 }),
            ]);
            let status: Vec<&str> = out["servers"].as_array().unwrap().iter().map(|s| s["status"].as_str().unwrap()).collect();
            assert_eq!(status, ["ok", "ignoré", "ignoré", "ignoré", "ok"]);
            let why = |i: usize| out["servers"][i]["why"].as_str().unwrap().to_string();
            assert!(why(1).contains("introuvable") && why(2).contains("satisfactory") && why(3).contains("constructeur"));
            assert_eq!(out["counts"]["ok"], 2);
            assert_eq!(out["counts"]["ignoré"], 3);
            assert_eq!(out["servers"][0]["name"], "srv1");
            assert_eq!(out["servers"][0]["output"], "fait sur 1");
        });
    }

    #[test]
    fn the_first_error_stops_unless_asked_otherwise() {
        block(async {
            let mut r = FakeRelay::new();
            r.fail = vec![2];
            let out = apply_with(&r, &req(json!({ "server_ids": [1, 2, 4], "tool": "write_file", "arguments": { "path": "/a", "content": "x" } })))
                .await
                .unwrap();
            let status: Vec<&str> = out["servers"].as_array().unwrap().iter().map(|s| s["status"].as_str().unwrap()).collect();
            assert_eq!(status, ["ok", "erreur", "non exécuté"]);
            assert_eq!(out["stopped_after_error_on"], 2);
            assert!(out["servers"][1]["error"].as_str().unwrap().contains("échec sur 2"));
            let r2 = FakeRelay { fail: vec![2], ..FakeRelay::new() };
            let out = apply_with(
                &r2,
                &req(json!({ "server_ids": [1, 2, 4], "tool": "write_file", "arguments": { "path": "/a", "content": "x" }, "stop_on_error": false })),
            )
            .await
            .unwrap();
            let status: Vec<&str> = out["servers"].as_array().unwrap().iter().map(|s| s["status"].as_str().unwrap()).collect();
            assert_eq!(status, ["ok", "erreur", "ok"]);
            assert!(out["stopped_after_error_on"].is_null());
        });
    }

    #[test]
    fn dry_run_is_passed_on_or_nothing_runs() {
        block(async {
            // Outil qui sait simuler : dry_run transmis (et prime sur celui des arguments).
            let r = FakeRelay::new();
            let rq = req(json!({ "server_ids": [1, 2], "tool": "sync_dir", "arguments": { "local_dir": "/repo", "remote_dir": "/plugins", "dry_run": false }, "dry_run": true }));
            apply_with(&r, &rq).await.unwrap();
            assert!(r.called().iter().all(|a| a["dry_run"] == true));
            // Outil sans simulation : rien n'est exécuté.
            let r = FakeRelay::new();
            let out = apply_with(&r, &req(json!({ "server_ids": [1, 2], "tool": "send_command", "arguments": { "command": "say hi" }, "dry_run": true })))
                .await
                .unwrap();
            assert!(r.called().is_empty());
            assert_eq!(out["counts"]["simulé"], 2);
            // dry_run=false : exécuté normalement ; absent : arguments inchangés.
            let r = FakeRelay::new();
            apply_with(&r, &req(json!({ "server_ids": [1], "tool": "send_command", "arguments": { "command": "say hi" }, "dry_run": false })))
                .await
                .unwrap();
            assert_eq!(r.called(), [json!({ "command": "say hi", "server_id": 1 })]);
        });
    }

    #[test]
    fn long_outputs_are_cut_and_json_outputs_kept() {
        assert_eq!(output_value("{\"a\": 1}"), json!({ "a": 1 }));
        assert_eq!(output_value("Fichier « /a » enregistré."), json!("Fichier « /a » enregistré."));
        let long = "x".repeat(OUTPUT_CAP + 10);
        assert!(output_value(&long).as_str().unwrap().ends_with("(sortie tronquée)"));
    }

    /// Journal des agents : les arguments relayés sont résumés comme un appel direct (contenu de
    /// fichier remplacé par sa taille).
    #[test]
    fn relayed_arguments_are_summarized_like_a_direct_call() {
        let args = json!({ "server_ids": [1, 2], "tool": "write_file", "arguments": { "path": "/a.yml", "content": "mot de passe: hunter2" } });
        let summary = crate::agent_guard::summarize_args("apply_to_servers", &args, true);
        assert!(!summary.contains("hunter2"), "{summary}");
        assert!(summary.contains("octets") && summary.contains("/a.yml"), "{summary}");
    }
}
