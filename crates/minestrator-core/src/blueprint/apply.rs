//! Application d'un blueprint (outil `apply_network`).
//!
//! Plan recalculé (lectures), refus s'il reste un élément bloqué, `dry_run` (défaut) = le plan.
//! Sinon, **avant tout effet** : mode constructeur vérifié pour chaque serveur rapproché (par son
//! nom comme par son identifiant), confirmation de l'humain (récapitulatif), nom exact retapé pour
//! chaque `change_version`, snapshots attendues. Puis les phases dans l'ordre ([`Phase::ALL`]),
//! chaque écriture par un outil élémentaire (`mcp::dispatch`, garde des agents). Arrêt au premier
//! échec avec un rapport qui permet la reprise : relancé, l'outil saute ce qui est conforme.

use super::plan::*;
use super::*;
use crate::mcp_ctx::Confirmation;
use crate::Core;
use std::collections::BTreeSet;

/// Attente d'un changement de version (`change_version`, asynchrone) : relectures espacées.
const PLATFORM_POLL_SECS: u64 = 10;
const PLATFORM_POLLS: u32 = 120;
/// Attentes d'état (`wait_for`).
const START_TIMEOUT_SECS: u64 = 600;
const STOP_TIMEOUT_SECS: u64 = 180;

/// Options d'`apply_network`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApplyOptions {
    pub dry_run: bool,
    pub snapshot: bool,
    /// Démarrer les serveurs à la fin (redémarrer ceux modifiés), pour charger les réglages,
    /// prégénérer et vérifier.
    pub start: bool,
    /// Noms exacts des serveurs, donnés par l'utilisateur, pour `change_version` (sans élicitation).
    pub confirm_names: Vec<String>,
}

impl ApplyOptions {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        let flag = |k: &str, d: bool| -> Result<bool, String> {
            match args.get(k) {
                None | Some(Value::Null) => Ok(d),
                Some(Value::Bool(b)) => Ok(*b),
                Some(_) => Err(format!("`{k}` : booléen attendu")),
            }
        };
        let confirm_names = match args.get("confirm_names") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("`confirm_names` : liste de noms (texte) attendue"))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("`confirm_names` : liste de noms exacts de serveurs attendue".into()),
        };
        Ok(Self { dry_run: flag("dry_run", true)?, snapshot: flag("snapshot", true)?, start: flag("start", true)?, confirm_names })
    }
}

/// Arguments d'un outil de réseau d'après les identifiants résolus (`dry_run` en second).
type ArgsFor<'f> = dyn Fn(&[Option<i64>], bool) -> Option<Value> + Sync + 'f;

/// Une étape exécutée (ou non) du rapport.
#[derive(Debug, Clone, PartialEq)]
struct StepOut {
    id: String,
    phase: Phase,
    server: Option<i64>,
    /// `réussie`, `conforme`, `ignorée`, `échouée`, `non exécutée`.
    outcome: &'static str,
    detail: Option<String>,
}

struct Run<'a, N: Net> {
    net: &'a N,
    nw: &'a Network,
    plan: &'a Plan,
    opts: &'a ApplyOptions,
    ids: Vec<Option<i64>>,
    /// Serveur créé par cet appel.
    fresh: Vec<bool>,
    /// Serveurs modifiés par cet appel (redémarrés à la fin).
    touched: BTreeSet<i64>,
    /// Serveurs où un préréglage vient d'installer son plugin (aucune donnée à préserver).
    fresh_plugin: BTreeSet<i64>,
    steps: Vec<StepOut>,
    created: Vec<Value>,
    failed: Option<(String, String)>,
    cancelled: bool,
    manual: Vec<String>,
    verify: Map<String, Value>,
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Sous-outil sorti en échec malgré une réponse (`status` d'un outil composite).
fn status_failed(v: &Value) -> Option<String> {
    let s = v.get("status").and_then(Value::as_str)?;
    (s.contains("échec") || s.starts_with("annulé") || s.starts_with("refusé")).then(|| s.to_string())
}

/// Étapes échouées d'un rapport de sous-outil (`steps[]` avec `outcome`), résumées.
fn failed_steps(v: &Value) -> String {
    v.get("steps")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("outcome").and_then(Value::as_str) == Some("échouée"))
                .map(|s| format!("{} : {}", s["id"].as_str().unwrap_or("?"), s.get("detail").and_then(Value::as_str).unwrap_or("échec")))
                .collect::<Vec<_>>()
                .join(" ; ")
        })
        .unwrap_or_default()
}

/// Serveurs dont une étape a réussi dans le rapport d'un sous-outil (`steps[].server_id`).
fn succeeded_servers(v: &Value) -> Vec<i64> {
    v.get("steps")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("outcome").and_then(Value::as_str) == Some("réussie"))
                .filter_map(|s| s.get("server_id").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// Serveurs où un préréglage a installé son plugin (étape `<id>.install` réussie).
fn installed_servers(v: &Value) -> Vec<i64> {
    v.get("steps")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("outcome").and_then(Value::as_str) == Some("réussie"))
                .filter(|s| s.get("id").and_then(Value::as_str).is_some_and(|id| id.ends_with(".install")))
                .filter_map(|s| s.get("server_id").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

impl<'a, N: Net> Run<'a, N> {
    fn name(&self, i: usize) -> &str {
        &self.nw.nodes[i].name
    }

    /// Arrêt demandé (échec précédent, annulation du client) ?
    fn halted(&mut self) -> bool {
        if self.failed.is_none() && !self.cancelled && crate::mcp_ctx::cancelled() {
            self.cancelled = true;
        }
        self.failed.is_some() || self.cancelled
    }

    fn record(&mut self, phase: Phase, id: &str, server: Option<i64>, outcome: &'static str, detail: Option<String>) {
        self.steps.push(StepOut { id: id.to_string(), phase, server, outcome, detail });
    }

    fn skip(&mut self, phase: Phase, id: &str, server: Option<i64>) {
        let why = match &self.failed {
            Some((at, _)) => format!("arrêt après l'échec de « {at} »"),
            None => "annulé".to_string(),
        };
        self.record(phase, id, server, "non exécutée", Some(why));
    }

    fn fail(&mut self, phase: Phase, id: &str, server: Option<i64>, e: String) {
        self.record(phase, id, server, "échouée", Some(e.clone()));
        self.failed = Some((id.to_string(), e));
    }

    /// Écriture par un outil élémentaire : mode constructeur revérifié, puis `mcp::dispatch`.
    async fn write(&self, tool: &str, args: Value) -> Result<String, String> {
        if let Some(r) = self.net.refusal(tool, &args).await {
            return Err(r);
        }
        self.net.call(tool, args).await
    }

    async fn wait_state(&self, server: i64, state: &str, secs: u64) -> Result<bool, String> {
        let out = self.net.call("wait_for", json!({ "server_id": server, "state": state, "timeout_secs": secs })).await?;
        let v = parse_out(&out);
        Ok(v.get("reached").and_then(Value::as_bool).unwrap_or(false))
    }

    /// Serveur arrêté (arrêt propre, sinon forcé).
    async fn ensure_offline(&self, server: i64) -> Result<(), String> {
        let state = self.net.state(server).await.unwrap_or_default();
        if state == "offline" {
            return Ok(());
        }
        self.write("power_action", json!({ "server_id": server, "action": "stop" })).await?;
        if self.wait_state(server, "offline", STOP_TIMEOUT_SECS).await? {
            return Ok(());
        }
        self.write("power_action", json!({ "server_id": server, "action": "kill" })).await?;
        if self.wait_state(server, "offline", 60).await? {
            Ok(())
        } else {
            Err(format!("serveur {server} toujours démarré après arrêt et arrêt forcé"))
        }
    }

    /// Démarrage puis arrêt : le serveur génère ses fichiers (velocity.toml, paper-global.yml,
    /// configs des plugins installés).
    async fn cycle(&self, server: i64) -> Result<(), String> {
        let state = self.net.state(server).await.unwrap_or_default();
        if state != "running" {
            if state != "starting" {
                self.write("power_action", json!({ "server_id": server, "action": "start" })).await?;
            }
            if !self.wait_state(server, "running", START_TIMEOUT_SECS).await? {
                return Err(format!(
                    "le serveur {server} n'a pas fini de démarrer ({START_TIMEOUT_SECS} s) : diagnose_startup et error_digest pour la cause ; relance apply_network ensuite"
                ));
            }
        }
        self.ensure_offline(server).await
    }

    /// Manifeste appliqué à un serveur (Chunky selon sa plateforme).
    fn manifest(&self, i: usize) -> Option<Manifest> {
        let n = &self.nw.nodes[i];
        let platform = n.platform.clone().or_else(|| self.plan.nodes[i].platform_now.clone());
        effective_manifest(n, !self.fresh[i], chunky_kind(platform.as_deref()))
    }

    /// Met à jour une clé du marqueur du serveur (`write_file`, étape annulable).
    async fn mark(&self, server: i64, key: &str, value: Value) -> Result<(), String> {
        let mut m = read_marker(self.net, server).await;
        m["version"] = json!(1);
        m[key] = value;
        let content = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
        self.write("write_file", json!({ "server_id": server, "path": MARKER_PATH, "content": content })).await.map(|_| ())
    }

    // --- Phases --------------------------------------------------------------------------------------

    async fn phase_servers(&mut self) {
        let ph = Phase::Servers;
        // Redimensionnements d'abord : une réduction libère de la place pour les créations.
        for i in 0..self.nw.nodes.len() {
            let id = format!("{}:resources", self.name(i));
            if !self.plan.items.iter().any(|it| it.id == id && it.status == Status::Change) {
                continue;
            }
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let (Some(sid), Some((cpu, ram, _))) = (server, self.plan.nodes[i].current) else { continue };
            let n = &self.nw.nodes[i];
            let args = json!({
                "mybox_id": self.plan.nodes[i].mybox_id.unwrap_or(self.nw.mybox_id),
                "server_id": sid,
                "cpu": n.cpu.unwrap_or(cpu),
                "ram": n.ram.unwrap_or(ram),
            });
            match self.write("mybox_resize_server", args).await {
                Ok(_) => {
                    self.touched.insert(sid);
                    self.record(ph, &id, server, "réussie", None);
                }
                Err(e) => self.fail(ph, &id, server, e),
            }
        }
        for i in 0..self.nw.nodes.len() {
            if self.ids[i].is_some() {
                continue;
            }
            let id = format!("{}:server", self.name(i));
            if self.halted() {
                self.skip(ph, &id, None);
                continue;
            }
            let Some(d) = self.plan.create.clone() else {
                self.fail(ph, &id, None, "jeu et localisation des serveurs à créer inconnus".into());
                continue;
            };
            let n = &self.nw.nodes[i];
            let mut args = json!({
                "mybox_id": self.nw.mybox_id,
                "name": n.name,
                "egg_id": d.egg_id,
                "experimental": d.experimental,
                "cpu": n.cpu.unwrap_or(1),
                "ram": n.ram.unwrap_or(1),
            });
            if let Some(l) = d.location_id {
                args["location_id"] = json!(l);
            }
            crate::mcp_ctx::progress(0.5, None, &format!("création de « {} »", n.name)).await;
            let out = match self.write("mybox_create_server", args).await {
                Ok(out) => out,
                Err(e) => {
                    self.fail(ph, &id, None, e);
                    continue;
                }
            };
            let Some(sid) = crate::agent_guard::created_server_id(&out) else {
                self.fail(ph, &id, None, "serveur créé mais identifiant absent de la réponse : vérifie list_servers ; relancer apply_network le rapprochera par son nom".into());
                continue;
            };
            // En mode constructeur, le serveur créé devient modifiable par la session.
            self.net.record_created(sid);
            self.ids[i] = Some(sid);
            self.fresh[i] = true;
            self.touched.insert(sid);
            self.created.push(json!({ "name": n.name, "server_id": sid }));
            crate::mcp_ctx::progress(0.6, None, &format!("installation de « {} » par l'hébergeur", n.name)).await;
            match self.net.wait_installed(sid).await {
                Ok(()) => self.record(ph, &id, Some(sid), "réussie", Some(format!("serveur {sid} créé et installé"))),
                Err(e) => self.fail(ph, &id, Some(sid), format!("{e} (serveur {sid} créé : relancer apply_network le reprendra)")),
            }
        }
    }

    async fn phase_platform(&mut self) {
        let ph = Phase::Platform;
        for i in 0..self.nw.nodes.len() {
            let n = &self.nw.nodes[i];
            if n.platform.is_none() && n.version.is_none() {
                continue;
            }
            let id = format!("{}:platform", n.name);
            let server = self.ids[i];
            if !self.fresh[i] && !self.plan.pending(Some(i), ph) {
                self.record(ph, &id, server, "conforme", None);
                continue;
            }
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            let (platform, version, fresh) = (n.platform.clone(), n.version.clone(), self.fresh[i]);
            let run = match self.net.apply_platform(sid, platform.as_deref(), version.as_deref(), fresh).await {
                Ok(r) => r,
                Err(e) => {
                    self.fail(ph, &id, server, e);
                    continue;
                }
            };
            let detail: Vec<String> = run.results.iter().map(|(t, o, d)| format!("{t} : {o}{}", d.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())).collect();
            if !run.results.is_empty() {
                self.touched.insert(sid);
            }
            if run.results.iter().any(|(_, o, _)| o != "réussie") {
                self.fail(ph, &id, server, detail.join(" ; "));
                continue;
            }
            if run.async_change {
                // change_version est asynchrone : la suite (contenu résolu pour la nouvelle version)
                // attend que l'hébergeur ait fini.
                let mut converged = false;
                for k in 0..PLATFORM_POLLS {
                    if crate::mcp_ctx::cancelled() {
                        break;
                    }
                    crate::mcp_ctx::progress(1.0 + f64::from(k) / f64::from(PLATFORM_POLLS), None, &format!("installation de la version de « {} »", n.name)).await;
                    self.net.pause(PLATFORM_POLL_SECS).await;
                    if self.net.platform(sid, platform.as_deref(), version.as_deref(), false).await.is_ok_and(|v| v.converged()) {
                        converged = true;
                        break;
                    }
                }
                if !converged {
                    if self.halted() {
                        self.skip(ph, &id, server);
                    } else {
                        let mins = PLATFORM_POLL_SECS * u64::from(PLATFORM_POLLS) / 60;
                        self.fail(ph, &id, server, format!("{} ; installation de la nouvelle version toujours en cours après {mins} min : relance apply_network une fois terminée (idempotent)", detail.join(" ; ")));
                    }
                    continue;
                }
            }
            self.record(ph, &id, server, "réussie", Some(detail.join(" ; ")).filter(|d| !d.is_empty()));
        }
    }

    async fn phase_content(&mut self) {
        let ph = Phase::Content;
        for i in 0..self.nw.nodes.len() {
            let Some(m) = self.manifest(i).filter(has_content) else { continue };
            if !self.fresh[i] && !self.plan.nodes[i].content_todo {
                continue;
            }
            let id = format!("{}:content", self.name(i));
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            let args = json!({ "server_id": sid, "manifest": m, "steps": ["content"], "snapshot": false, "apply_official": false });
            self.apply_manifest(ph, &id, sid, args).await;
        }
    }

    /// `apply_manifest` et son rapport : échec si une étape a échoué.
    async fn apply_manifest(&mut self, ph: Phase, id: &str, sid: i64, args: Value) {
        match self.write("apply_manifest", args).await {
            Err(e) => self.fail(ph, id, Some(sid), e),
            Ok(out) => {
                let v = parse_out(&out);
                let failed = v.get("failed").and_then(Value::as_i64).unwrap_or(0);
                if failed > 0 || v.get("status").and_then(Value::as_str) == Some("annulé") {
                    self.fail(ph, id, Some(sid), format!("apply_manifest : {}", Some(failed_steps(&v)).filter(|s| !s.is_empty()).unwrap_or_else(|| "annulé".into())));
                    return;
                }
                let applied = v.get("applied").and_then(Value::as_i64).unwrap_or(0);
                if applied > 0 {
                    self.touched.insert(sid);
                }
                if ph == Phase::Manifest {
                    let who = self.nw.nodes.iter().zip(&self.ids).find(|(_, x)| **x == Some(sid)).map(|(n, _)| n.label()).unwrap_or_default();
                    self.manual.extend(strings_of(&v, "manual").into_iter().map(|m| format!("{who} : {m}")));
                }
                self.record(ph, id, Some(sid), if applied > 0 { "réussie" } else { "conforme" }, Some(format!("{applied} étape(s) appliquée(s), {} déjà conforme(s)", v.get("already_ok").and_then(Value::as_i64).unwrap_or(0))));
            }
        }
    }

    async fn phase_world(&mut self) {
        let ph = Phase::World;
        for i in 0..self.nw.nodes.len() {
            let Some(w) = self.nw.nodes[i].world.clone() else { continue };
            if !self.fresh[i] && !self.plan.pending(Some(i), ph) {
                continue;
            }
            let id = format!("{}:world", self.name(i));
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            if let Err(e) = self.ensure_offline(sid).await {
                self.fail(ph, &id, server, e);
                continue;
            }
            // Le monde visé était absent au plan (ou le serveur est neuf) : ce qui s'y trouve a été
            // généré par cet appel ; sinon, seulement si le blueprint le demande.
            let replace = w.replace || self.fresh[i] || self.plan.nodes[i].world_absent;
            let mut args = json!({ "server_id": sid, "local_path": w.local_path, "replace": replace, "stop": false, "snapshot": false, "dry_run": false });
            if let Some(name) = &w.world_name {
                args["world_name"] = json!(name);
                args["set_level_name"] = json!(true);
            }
            let out = match self.write("upload_world", args).await {
                Ok(o) => parse_out(&o),
                Err(e) => {
                    self.fail(ph, &id, server, e);
                    continue;
                }
            };
            self.touched.insert(sid);
            let fp = match self.net.world_fingerprint(&w.local_path).await {
                Ok(fp) => fp,
                Err(e) => {
                    self.fail(ph, &id, server, format!("monde installé, mais empreinte illisible : {e}"));
                    continue;
                }
            };
            let target = out.get("target").cloned().unwrap_or(Value::Null);
            match self.mark(sid, "world", json!({ "fingerprint": fp, "target": target, "installed": now() })).await {
                Ok(()) => self.record(ph, &id, server, "réussie", Some(format!("monde installé en {}", target.as_str().unwrap_or("?")))),
                Err(e) => self.fail(ph, &id, server, format!("monde installé, marqueur non écrit ({e}) : une relance le réinstallerait")),
            }
        }
    }

    async fn phase_first_start(&mut self) {
        let ph = Phase::FirstStart;
        for i in self.start_order() {
            if !self.fresh[i] && !self.plan.nodes[i].needs_start {
                continue;
            }
            let id = format!("{}:first_start", self.name(i));
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            crate::mcp_ctx::progress(5.0, None, &format!("premier démarrage de « {} »", self.name(i))).await;
            match self.cycle(sid).await {
                Ok(()) => self.record(ph, &id, server, "réussie", Some("démarré puis arrêté : fichiers générés".into())),
                Err(e) => self.fail(ph, &id, server, e),
            }
        }
    }

    async fn phase_manifest(&mut self) {
        let ph = Phase::Manifest;
        for i in 0..self.nw.nodes.len() {
            let Some(m) = self.manifest(i) else { continue };
            let server = self.ids[i];
            let changed = server.is_some_and(|s| self.touched.contains(&s));
            if !self.fresh[i] && !self.plan.nodes[i].manifest_todo && !changed {
                continue;
            }
            let id = format!("{}:manifest", self.name(i));
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            let args = json!({ "server_id": sid, "manifest": m, "snapshot": false, "apply_official": true });
            self.apply_manifest(ph, &id, sid, args).await;
        }
    }

    async fn phase_sync(&mut self) {
        let ph = Phase::Sync;
        for i in 0..self.nw.nodes.len() {
            for (k, s) in self.nw.nodes[i].syncs.clone().iter().enumerate() {
                let id = format!("{}:sync[{k}]", self.name(i));
                let pending = self.plan.items.iter().any(|it| it.id == id && matches!(it.status, Status::Create | Status::Change));
                if !self.fresh[i] && !pending {
                    continue;
                }
                let server = self.ids[i];
                if self.halted() {
                    self.skip(ph, &id, server);
                    continue;
                }
                let Some(sid) = server else { continue };
                match self.write("sync_dir", sync_args(sid, s, false)).await {
                    Err(e) => self.fail(ph, &id, server, e),
                    Ok(out) => {
                        let v = parse_out(&out);
                        if let Some(e) = v.get("error").and_then(Value::as_str) {
                            self.fail(ph, &id, server, format!("sync_dir : {e}"));
                            continue;
                        }
                        let done = v.pointer("/done/uploaded").and_then(Value::as_i64).unwrap_or(0) + v.pointer("/done/deleted").and_then(Value::as_i64).unwrap_or(0);
                        if done > 0 {
                            self.touched.insert(sid);
                        }
                        self.record(ph, &id, server, "réussie", Some(format!("{done} fichier(s) envoyé(s) ou supprimé(s)")));
                    }
                }
            }
        }
    }

    /// Outil de réseau (`link_proxy`, `network_preset`) : appliqué ; si des serveurs doivent d'abord
    /// générer des fichiers (plugin tout juste installé, Paper jamais démarré), ils démarrent puis
    /// s'arrêtent, et l'outil est relancé une fois.
    async fn network_tool(&mut self, ph: Phase, id: &str, tool: &str, args: &ArgsFor<'_>) {
        for attempt in 0..2 {
            if self.halted() {
                self.skip(ph, id, None);
                return;
            }
            let Some(dry) = args(&self.ids, true) else {
                self.fail(ph, id, None, "serveurs du réseau non résolus".into());
                return;
            };
            // Simulation d'abord : ce qui se confirme doit l'avoir été avant tout effet.
            let v = match self.net.call(tool, dry).await {
                Ok(o) => parse_out(&o),
                Err(e) => {
                    self.fail(ph, id, None, e);
                    return;
                }
            };
            // Effets non réversibles approuvés : ceux du récapitulatif, et ceux qui ne touchent qu'un
            // serveur créé par cet appel ou un plugin installé à l'instant (aucune donnée à perdre :
            // LuckPerms démarre sur son stockage local par défaut).
            let blank: Vec<i64> = self.ids.iter().zip(&self.fresh).filter(|(_, f)| **f).filter_map(|(id, _)| *id).chain(self.fresh_plugin.iter().copied()).collect();
            let fresh_confirm: Vec<String> = strings_of(&v, "to_confirm")
                .into_iter()
                .filter(|c| !self.plan.preset_confirm.contains(c) && !blank.iter().any(|id| c.contains(&format!("({id})"))))
                .collect();
            if !fresh_confirm.is_empty() {
                self.fail(ph, id, None, format!("nouvelle confirmation nécessaire ({}) : relance apply_network, le récapitulatif la présentera", fresh_confirm.join(" ; ")));
                return;
            }
            if count_of(&v, "refused") > 0 {
                self.fail(ph, id, None, format!("{tool} : étape refusée (fichier invalide, voir {tool} en dry_run)"));
                return;
            }
            let mut starts = start_needed_ids(&v);
            if count_of(&v, "change") > 0 {
                let Some(real) = args(&self.ids, false) else { return };
                if let Some(r) = self.net.refusal(tool, &real).await {
                    self.fail(ph, id, None, r);
                    return;
                }
                let out = match self.net.call_preapproved(tool, real).await {
                    Ok(o) => parse_out(&o),
                    Err(e) => {
                        self.fail(ph, id, None, e);
                        return;
                    }
                };
                if let Some(s) = status_failed(&out) {
                    self.fail(ph, id, None, format!("{tool} : {s} ; {}", failed_steps_network(&out)));
                    return;
                }
                self.touched.extend(succeeded_servers(&out));
                self.fresh_plugin.extend(installed_servers(&out));
                starts = start_needed_ids(&out);
                if starts.is_empty() || attempt == 1 {
                    self.manual.extend(strings_of(&out, "manual").into_iter().map(|m| format!("{tool} : {m}")));
                    self.record(ph, id, None, "réussie", Some(format!("{} écriture(s)", out.get("applied").and_then(Value::as_i64).unwrap_or(0))));
                    return;
                }
            } else if starts.is_empty() || attempt == 1 {
                self.manual.extend(strings_of(&v, "manual").into_iter().map(|m| format!("{tool} : {m}")));
                self.record(ph, id, None, "conforme", None);
                return;
            }
            // Démarrage puis arrêt des serveurs qui doivent générer leurs fichiers, puis relance.
            for sid in starts {
                if self.halted() {
                    break;
                }
                let step = format!("{id}:start[{sid}]");
                match self.cycle(sid).await {
                    Ok(()) => self.record(ph, &step, Some(sid), "réussie", Some("démarré puis arrêté : fichiers générés".into())),
                    Err(e) => self.fail(ph, &step, Some(sid), e),
                }
            }
        }
    }

    async fn phase_link(&mut self) {
        let ph = Phase::Link;
        if self.nw.proxy.is_none() {
            return;
        }
        let id = "network:link";
        if !self.plan.pending(None, ph) && !self.fresh.iter().any(|f| *f) {
            self.record(ph, id, None, "conforme", None);
            return;
        }
        let nw = self.nw;
        let args = move |ids: &[Option<i64>], dry: bool| link_args(nw, ids, dry);
        self.network_tool(ph, id, "link_proxy", &args).await;
    }

    async fn phase_presets(&mut self) {
        let ph = Phase::Presets;
        let nw = self.nw;
        for preset in &nw.presets {
            let id = format!("network:preset.{preset}");
            let pending = self.plan.items.iter().any(|it| it.id == id && matches!(it.status, Status::Create | Status::Change));
            if !pending && !self.fresh.iter().any(|f| *f) {
                self.record(ph, &id, None, "conforme", None);
                continue;
            }
            let p = preset.clone();
            let args = move |ids: &[Option<i64>], dry: bool| preset_args(nw, ids, &p, dry);
            self.network_tool(ph, &id, "network_preset", &args).await;
        }
    }

    /// Serveurs de jeu d'abord, proxy en dernier.
    fn start_order(&self) -> Vec<usize> {
        let mut v = self.nw.backends();
        v.extend(self.nw.proxy_index());
        v
    }

    async fn phase_start(&mut self) {
        let ph = Phase::Start;
        if !self.opts.start {
            self.record(ph, "network:start", None, "ignorée", Some("start=false : démarre les serveurs de jeu puis le proxy (power_action) pour charger les réglages".into()));
            return;
        }
        for i in self.start_order() {
            let id = format!("{}:start", self.name(i));
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            let state = self.net.state(sid).await.unwrap_or_default();
            let action = match state.as_str() {
                "offline" => Some("start"),
                "running" | "starting" if self.touched.contains(&sid) => Some("restart"),
                _ => None,
            };
            let Some(action) = action else {
                self.record(ph, &id, server, "conforme", Some(format!("déjà {state}")));
                continue;
            };
            crate::mcp_ctx::progress(9.0, None, &format!("démarrage de « {} »", self.name(i))).await;
            if let Err(e) = self.write("power_action", json!({ "server_id": sid, "action": action })).await {
                self.fail(ph, &id, server, e);
                continue;
            }
            match self.wait_state(sid, "running", START_TIMEOUT_SECS).await {
                Ok(true) => self.record(ph, &id, server, "réussie", Some(format!("{action} : en ligne"))),
                Ok(false) => self.fail(ph, &id, server, format!("pas en ligne après {START_TIMEOUT_SECS} s : diagnose_startup et error_digest")),
                Err(e) => self.fail(ph, &id, server, e),
            }
        }
    }

    async fn phase_pregenerate(&mut self) {
        let ph = Phase::Pregenerate;
        for i in 0..self.nw.nodes.len() {
            let Some(p) = self.nw.nodes[i].pregen.clone() else { continue };
            if !self.fresh[i] && !self.plan.pending(Some(i), ph) {
                continue;
            }
            let id = format!("{}:pregenerate", self.name(i));
            let server = self.ids[i];
            if self.halted() {
                self.skip(ph, &id, server);
                continue;
            }
            let Some(sid) = server else { continue };
            if self.net.state(sid).await.unwrap_or_default() != "running" {
                let why = "serveur arrêté : démarre-le (ou relance avec start=true), puis relance apply_network pour lancer la prégénération";
                self.manual.push(format!("{} : {why}", self.nw.nodes[i].label()));
                self.record(ph, &id, server, "ignorée", Some(why.into()));
                continue;
            }
            match self.write("pregenerate_world", pregen_args(sid, &p)).await {
                Err(e) => self.fail(ph, &id, server, e),
                Ok(_) => {
                    let mut sig = pregen_signature(&p);
                    sig["started"] = json!(now());
                    match self.mark(sid, "pregenerate", sig).await {
                        Ok(()) => self.record(ph, &id, server, "réussie", Some(format!("prégénération lancée (rayon {} blocs) ; suivi : pregenerate_world action=status", p.radius))),
                        Err(e) => self.fail(ph, &id, server, format!("prégénération lancée, marqueur non écrit ({e}) ; suivi : pregenerate_world action=status")),
                    }
                }
            }
        }
    }

    /// Vérification : topologie (problèmes, ping) et ping de chaque serveur. Lectures ; un échec de
    /// vérification n'est pas un échec de l'application, il est rapporté.
    async fn phase_verify(&mut self) {
        if self.failed.is_some() || self.cancelled {
            return;
        }
        if let Some(p) = self.nw.proxy_index().and_then(|p| self.ids[p]) {
            match self.net.call("network_topology", json!({ "server_id": p, "ping": self.opts.start })).await {
                Ok(out) => {
                    let v = parse_out(&out);
                    let problems: Vec<Value> = v
                        .get("problems")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter(|x| matches!(x["severity"].as_str(), Some("critique" | "important"))).cloned().collect())
                        .unwrap_or_default();
                    self.verify.insert("topology".into(), json!({ "summary": v.get("summary"), "problems": problems }));
                }
                Err(e) => {
                    self.verify.insert("topology".into(), json!({ "error": e }));
                }
            }
        }
        if !self.opts.start {
            return;
        }
        let mut pings = Map::new();
        for i in 0..self.nw.nodes.len() {
            let Some(sid) = self.ids[i] else { continue };
            let v = match self.net.call("ping_server", json!({ "server_id": sid })).await {
                Ok(out) => {
                    let v = parse_out(&out);
                    json!({ "online": v.get("online"), "version": v.get("version"), "players": v.get("players"), "latency_ms": v.get("latency_ms") })
                }
                Err(e) => json!({ "online": false, "error": e }),
            };
            pings.insert(self.nw.nodes[i].name.clone(), v);
        }
        self.verify.insert("ping".into(), Value::Object(pings));
    }
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn count_of(v: &Value, key: &str) -> i64 {
    v.pointer(&format!("/counts/{key}")).and_then(Value::as_i64).unwrap_or(0)
}

/// Étapes échouées d'un rapport de `link_proxy` / `network_preset` (`steps[].outcome`).
fn failed_steps_network(v: &Value) -> String {
    let s = failed_steps(v);
    if s.is_empty() {
        "voir list_changesets".into()
    } else {
        s
    }
}

/// Récapitulatif présenté à l'humain avant tout effet.
fn summary_message(nw: &Network, plan: &Plan, opts: &ApplyOptions) -> String {
    let mut lines = vec![format!("Monter le réseau {} (MyBox {}) :", nw.label(), nw.mybox_id)];
    lines.extend(plan.to_confirm.iter().map(|c| format!("- {c}")));
    let steps = order(nw, plan);
    let phases: Vec<&str> = steps.iter().filter_map(|o| o.get("phase").and_then(Value::as_str)).filter(|p| !matches!(*p, "servers" | "verify")).collect();
    if !phases.is_empty() {
        lines.push(format!("Puis, dans l'ordre : {}.", phases.join(", ")));
    }
    lines.push(if opts.snapshot {
        "Snapshot de chaque serveur existant modifié d'abord.".to_string()
    } else {
        "SANS snapshot préalable (snapshot=false).".to_string()
    });
    if !nw.presets.is_empty() {
        lines.push("Préréglages : sur un serveur créé par cet appel, ou dont le plugin vient d'être installé, la bascule vers la base partagée se fait sans autre question (aucune donnée à perdre).".into());
    }
    if opts.start {
        lines.push("À la fin : serveurs arrêtés démarrés, serveurs modifiés redémarrés (joueurs déconnectés).".into());
    }
    lines.join("\n")
}

/// Applique un blueprint. `Ok((rapport, échec))` ; `Err` : refus avant tout effet.
pub(crate) async fn apply_with<N: Net>(net: &N, nw: &Network, opts: &ApplyOptions) -> Result<(Value, bool), String> {
    let plan = plan_with(net, nw).await?;
    if !plan.blocked().is_empty() {
        let mut v = render_plan(nw, &plan, false);
        v["status"] = json!("refusé : des éléments sont bloqués, rien n'a été fait");
        return Ok((v, true));
    }
    if opts.dry_run {
        let mut v = render_plan(nw, &plan, true);
        v["dry_run"] = json!(true);
        return Ok((v, false));
    }
    if !plan.todo() {
        let mut v = render_plan(nw, &plan, false);
        v["status"] = json!("déjà conforme : rien à faire");
        return Ok((v, false));
    }

    // --- Avant tout effet (l'ère MCP sans état rejoue l'appel jusqu'ici) -----------------------
    // Mode constructeur : chaque serveur existant, qu'il soit désigné par son nom ou son identifiant.
    let existing: Vec<Value> = plan.nodes.iter().filter_map(|s| s.server_id).map(|id| json!({ "server_id": id })).collect();
    if !existing.is_empty() {
        if let Some(r) = net.refusal("apply_network", &json!({ "blueprint": { "servers": existing } })).await {
            return Err(r);
        }
    }
    if net.confirm(&summary_message(nw, &plan, opts)).await == Confirmation::Declined {
        return Err("Refusé par l'utilisateur : rien n'a été fait.".into());
    }
    for (i, name) in &plan.name_confirm {
        let message = format!(
            "{} : changement de plateforme ou de version (change_version, DESTRUCTIF : JAR et bibliothèques remplacés, serveur redémarré).",
            nw.nodes[*i].label()
        );
        let given = opts.confirm_names.iter().find(|g| g.trim() == name.trim()).cloned();
        net.confirm_name(&message, name, given).await.map_err(|e| {
            if e.contains("requis") {
                format!("Confirmation requise pour {} : `confirm_names` doit contenir le nom exact de ce serveur (casse comprise), donné par l'utilisateur — jamais deviné. Rien n'a été fait.", nw.nodes[*i].label())
            } else {
                format!("Refusé pour {} : {e}. Rien n'a été fait.", nw.nodes[*i].label())
            }
        })?;
    }

    let mut run = Run {
        net,
        nw,
        plan: &plan,
        opts,
        ids: plan.nodes.iter().map(|s| s.server_id).collect(),
        fresh: vec![false; nw.nodes.len()],
        touched: BTreeSet::new(),
        fresh_plugin: BTreeSet::new(),
        steps: Vec::new(),
        created: Vec::new(),
        failed: None,
        cancelled: false,
        manual: plan.items.iter().filter(|i| i.status == Status::Manual).map(|i| i.summary.clone()).collect(),
        verify: Map::new(),
    };

    // Snapshots des serveurs existants que le plan modifie, attendues : sans elles, rien n'est fait.
    let mut snapshots = Vec::new();
    if opts.snapshot {
        let network_change = plan.items.iter().any(|i| i.node.is_none() && matches!(i.phase, Phase::Link | Phase::Presets) && matches!(i.status, Status::Create | Status::Change));
        let name = format!("Avant apply_network {}", now());
        for (i, st) in plan.nodes.iter().enumerate() {
            let Some(sid) = st.server_id else { continue };
            let affected = network_change || plan.items.iter().any(|it| it.node == Some(i) && matches!(it.status, Status::Create | Status::Change));
            if !affected {
                continue;
            }
            crate::mcp_ctx::progress(0.0, None, &format!("snapshot de « {} »", nw.nodes[i].name)).await;
            net.snapshot(sid, &name).await.map_err(|e| {
                format!("snapshot préalable de {} impossible ({e}) : rien n'a été modifié ; relance plus tard, ou avec snapshot=false", nw.nodes[i].label())
            })?;
            snapshots.push(json!({ "server_id": sid, "server": nw.nodes[i].name, "name": name }));
        }
    }

    let total = Phase::ALL.len() as f64;
    for (k, ph) in Phase::ALL.iter().enumerate() {
        crate::mcp_ctx::progress(k as f64, Some(total), ph.label()).await;
        match ph {
            Phase::Servers => run.phase_servers().await,
            Phase::Platform => run.phase_platform().await,
            Phase::Content => run.phase_content().await,
            Phase::World => run.phase_world().await,
            Phase::FirstStart => run.phase_first_start().await,
            Phase::Manifest => run.phase_manifest().await,
            Phase::Sync => run.phase_sync().await,
            Phase::Link => run.phase_link().await,
            Phase::Presets => run.phase_presets().await,
            Phase::Start => run.phase_start().await,
            Phase::Pregenerate => run.phase_pregenerate().await,
            Phase::Verify => run.phase_verify().await,
        }
    }
    crate::mcp_ctx::progress(total, Some(total), "terminé").await;
    Ok(report(&run, snapshots))
}

fn report<N: Net>(run: &Run<'_, N>, snapshots: Vec<Value>) -> (Value, bool) {
    let steps: Vec<Value> = run
        .steps
        .iter()
        .map(|s| {
            let mut v = json!({ "id": s.id, "phase": s.phase.key(), "outcome": s.outcome });
            if let Some(id) = s.server {
                v["server_id"] = json!(id);
            }
            if let Some(d) = &s.detail {
                v["detail"] = json!(d);
            }
            v
        })
        .collect();
    let failed = run.failed.is_some();
    let status = if failed {
        "arrêté après un échec"
    } else if run.cancelled {
        "annulé"
    } else {
        "terminé"
    };
    let mut out = json!({
        "network": run.nw.name,
        "mybox_id": run.nw.mybox_id,
        "status": status,
        "servers": run.nw.nodes.iter().zip(&run.ids).map(|(n, id)| json!({ "name": n.name, "server_id": id })).collect::<Vec<_>>(),
        "steps": steps,
    });
    if !run.created.is_empty() {
        out["created"] = json!(run.created);
    }
    if !snapshots.is_empty() {
        out["snapshots"] = json!(snapshots);
    }
    if let Some((id, e)) = &run.failed {
        out["failed"] = json!({ "step": id, "error": e });
    }
    if !run.manual.is_empty() {
        out["manual"] = json!(run.manual);
    }
    if !run.verify.is_empty() {
        out["verify"] = Value::Object(run.verify.clone());
    }
    out["resume"] = json!("Relancer apply_network avec le même blueprint reprend là où le réseau en est : les serveurs créés sont rapprochés par leur nom, ce qui est conforme est sauté (plateforme, manifestes, synchronisations, liaison, préréglages ; monde et prégénération d'après le marqueur /.minestrator-blueprint.json).");
    out["next"] = json!(if failed || run.cancelled {
        "Corrige la cause (`failed`), puis relance apply_network (idempotent). Chaque écriture faite est une étape annulable de la garde des agents (list_changesets, rollback_changeset) ; les snapshots prises au début restent un filet."
    } else if run.manual.is_empty() {
        "Réseau monté : vérifie `verify` (network_topology, ping) ; plan_network avec le même blueprint ne doit plus rien trouver à faire. Annulation : list_changesets puis rollback_changeset, ou les snapshots."
    } else {
        "Réseau monté hors étapes manuelles (`manual`) : traite-les, puis relance plan_network pour vérifier. Annulation : list_changesets puis rollback_changeset, ou les snapshots."
    });
    (out, failed || run.cancelled)
}

/// Outil MCP `apply_network`.
pub(crate) async fn apply_network_tool(core: &Core, args: &Value) -> Result<String, String> {
    let nw = Network::parse(args.get("blueprint").ok_or("paramètre `blueprint` (objet JSON) requis")?)?;
    let opts = ApplyOptions::parse(args)?;
    let net = CoreNet { core, cfg: core.get_mcp_config() };
    let (v, failed) = Box::pin(apply_with(&net, &nw, &opts)).await?;
    let text = crate::config_edit::render_for_ai(core, crate::redact::mask_known_secrets_value(v));
    if failed {
        Err(text)
    } else {
        Ok(text)
    }
}
