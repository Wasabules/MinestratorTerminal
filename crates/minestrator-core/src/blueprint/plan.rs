//! Plan d'un blueprint (outil `plan_network`, et première étape d'`apply_network`) : lectures et
//! simulations seulement.
//!
//! Tout ce qui touche au compte passe par le trait [`Net`] : le vrai compte ([`CoreNet`] :
//! `mcp::dispatch`, logique de `set_server_platform`, SFTP, catalogue) ou un faux dans les tests.

use super::*;
use crate::mcp_ctx::Confirmation;
use crate::mybox::MyBoxCapacity;
use crate::Core;
use futures_util::{stream, StreamExt};
use std::future::Future;
use std::pin::Pin;

// --- Accès au compte ------------------------------------------------------------------------------

/// Serveur du compte vu par le blueprint.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Account {
    pub id: i64,
    pub name: String,
    pub mybox_id: i64,
    /// `active`, `suspended`, `expired`, `disabled`, `hibernation`.
    pub status: String,
    /// Minecraft Java (proxy compris) : les autres jeux ne font pas partie d'un réseau.
    pub minecraft_java: bool,
}

impl Account {
    pub(crate) fn usable(&self) -> bool {
        matches!(self.status.as_str(), "active" | "hibernation")
    }
}

/// Plateforme et version d'un serveur d'après la logique de `set_server_platform` (lectures).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PlatformView {
    pub current_platform: Option<String>,
    pub current_version: Option<String>,
    /// Opérations à faire, dans l'ordre : (outil, résumé).
    pub ops: Vec<(String, String)>,
    /// `change_version` est nécessaire (destructif : nom exact confirmé par l'humain).
    pub destructive: bool,
    pub blocking: Vec<String>,
    pub warnings: Vec<String>,
}

impl PlatformView {
    /// Plateforme et version sont-elles celles voulues (plus de `change_version` ni de JAR à
    /// envoyer) ?
    pub(crate) fn converged(&self) -> bool {
        !self.ops.iter().any(|(t, _)| t == "change_version" || t == "upload_file")
    }
}

/// Ce qu'a fait l'application de la plateforme : (outil, résultat, détail) par opération.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PlatformRun {
    pub results: Vec<(String, String, Option<String>)>,
    /// `change_version` lancé : l'hébergeur installe de façon asynchrone.
    pub async_change: bool,
}

/// Jeu et localisation des serveurs créés.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CreateDefaults {
    pub egg_id: i64,
    pub experimental: bool,
    pub location_id: Option<i64>,
    pub location: Option<String>,
}

/// Ce dont le blueprint a besoin du compte (lectures, simulations, écritures par la garde).
pub(crate) trait Net: Sync {
    fn account(&self) -> impl Future<Output = Result<Vec<Account>, String>> + Send;
    fn capacity(&self, mybox: i64) -> impl Future<Output = Result<MyBoxCapacity, String>> + Send;
    fn create_defaults(
        &self,
        mybox: i64,
        egg: Option<i64>,
        location: Option<i64>,
    ) -> impl Future<Output = Result<CreateDefaults, String>> + Send;
    /// Le chemin existe-t-il sur le serveur (SFTP) ?
    fn exists(&self, server: i64, path: &str) -> impl Future<Output = Result<bool, String>> + Send;
    fn read_text(&self, server: i64, path: &str) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// Plan de plateforme et de version (lectures). `fresh` : serveur créé par l'appel (une
    /// version plus ancienne que celle installée par défaut passe par une réinstallation propre).
    fn platform(
        &self,
        server: i64,
        platform: Option<&str>,
        version: Option<&str>,
        fresh: bool,
    ) -> impl Future<Output = Result<PlatformView, String>> + Send;
    /// Applique plateforme et version (sans confirmation : obtenue avant par l'appelant).
    fn apply_platform(
        &self,
        server: i64,
        platform: Option<&str>,
        version: Option<&str>,
        fresh: bool,
    ) -> impl Future<Output = Result<PlatformRun, String>> + Send;
    /// État normalisé : `running`, `starting`, `stopping`, `offline`.
    fn state(&self, server: i64) -> impl Future<Output = Result<String, String>> + Send;
    /// Empreinte d'un monde de l'ordinateur (chemins et tailles de ses fichiers).
    fn world_fingerprint(&self, local_path: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// Outil MCP par `mcp::dispatch` (garde des agents comprise pour un outil modifiant).
    fn call(&self, tool: &str, args: Value) -> impl Future<Output = Result<String, String>> + Send;
    /// Même appel, confirmations simples du sous-outil tenues pour accordées (`mcp_ctx::preapproved`).
    fn call_preapproved(&self, tool: &str, args: Value) -> impl Future<Output = Result<String, String>> + Send;
    /// Refus du mode constructeur (ou de la portée du Copilote) pour cet appel ; `None` : permis.
    fn refusal(&self, tool: &str, args: &Value) -> impl Future<Output = Option<String>> + Send;
    /// Serveur créé par l'appel : il rejoint la portée du mode constructeur.
    fn record_created(&self, server: i64);
    /// Attend la fin de l'installation d'un serveur créé, puis l'arrête s'il a démarré.
    fn wait_installed(&self, server: i64) -> impl Future<Output = Result<(), String>> + Send;
    fn confirm(&self, message: &str) -> impl Future<Output = Confirmation> + Send;
    /// Nom exact retapé par l'humain (élicitation), sinon `given` ; vérifié contre `expected`.
    fn confirm_name(
        &self,
        message: &str,
        expected: &str,
        given: Option<String>,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// Snapshot créée puis attendue.
    fn snapshot(&self, server: i64, name: &str) -> impl Future<Output = Result<(), String>> + Send;
    /// Pause entre deux relectures (attente d'une installation asynchrone).
    fn pause(&self, secs: u64) -> impl Future<Output = ()> + Send;
}

/// Le vrai compte.
pub(crate) struct CoreNet<'a> {
    pub(crate) core: &'a Core,
    pub(crate) cfg: crate::mcp::McpConfig,
}

/// Sous-outil, avec l'enveloppe de `mcp::dispatch` : masques des secrets des agents, garde des
/// agents autour d'une écriture (`before_write` / `after_write` : une étape par sous-écriture,
/// comme `platform::official_write`), cache purgé, valeurs des secrets effacées de la sortie. Les
/// outils composites appelés ici vont **directement** à leur module au lieu de repasser par le grand
/// `match` de `mcp::dispatch_inner` : en debug, son cadre de pile ne tient pas deux fois sur la pile
/// d'un fil tokio quand l'outil appelé est lui-même profond (plan d'un manifeste, préréglage qui
/// écrit à son tour par `mcp::dispatch`). Les autres (création de serveur, alimentation, fichier)
/// passent par `mcp::dispatch`. En boîte `dyn … + Send` : casse le cycle de types dispatch → outil
/// → dispatch.
pub(crate) fn sub_dispatch<'a>(core: &'a Core, tool: &'a str, args: Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        const DIRECT: &[&str] = &[
            "plan_manifest",
            "apply_manifest",
            "export_manifest",
            "sync_dir",
            "upload_world",
            "link_proxy",
            "network_preset",
            "network_topology",
            "ping_server",
            "pregenerate_world",
            "wait_for",
        ];
        if !DIRECT.contains(&tool) {
            return crate::mcp::dispatch(core, tool, args).await;
        }
        core.refresh_secret_masks();
        let write = crate::mcp::is_write_tool(tool);
        let step = if write { crate::agent_guard::before_write(core, tool, &args).await } else { None };
        let out = match tool {
            "plan_manifest" => Box::pin(crate::manifest::plan_manifest_tool(core, &args)).await,
            "apply_manifest" => Box::pin(crate::manifest::apply_manifest_tool(core, &args)).await,
            "export_manifest" => Box::pin(crate::manifest::export_manifest_tool(core, &args)).await,
            "sync_dir" => Box::pin(crate::local_sync::sync_dir_tool(core, &args)).await,
            "upload_world" => Box::pin(crate::world_upload::upload_world_tool(core, &args)).await,
            "link_proxy" => Box::pin(crate::network::link_proxy_tool(core, &args)).await,
            "network_preset" => Box::pin(crate::network::network_preset_tool(core, &args)).await,
            "network_topology" => Box::pin(crate::network::network_topology_tool(core, &args)).await,
            "ping_server" => Box::pin(crate::network::ping_server_tool(core, &args)).await,
            "pregenerate_world" => Box::pin(crate::pregen::pregenerate_world_tool(core, &args)).await,
            _ => Box::pin(core.agent_wait_for(&args)).await,
        };
        let out = if write {
            core.cache().clear();
            crate::agent_guard::after_write(core, step, out).await
        } else {
            out
        };
        let mask = |s: String| crate::redact::mask_known_secrets(&s).into_owned();
        out.map(mask).map_err(mask)
    })
}

impl CoreNet<'_> {
    async fn prepare(
        &self,
        server: i64,
        platform: Option<&str>,
        version: Option<&str>,
        fresh: bool,
    ) -> Result<crate::platform::Prepared, String> {
        let mut req = crate::platform::Request {
            server_id: server,
            platform: platform.map(crate::platform::normalize_platform),
            version: version.map(str::to_string),
            snapshot: false,
            dry_run: true,
            ..Default::default()
        };
        let p = Box::pin(crate::platform::prepare(self.core, &req)).await?;
        // Serveur neuf : l'hébergeur a installé sa version par défaut ; une version plus ancienne
        // ne perd rien (aucun monde, aucun contenu) et passe par une réinstallation propre.
        if fresh && p.blocking.iter().any(|b| b.contains("ANTÉRIEURE")) {
            req.reinstall = true;
            return Box::pin(crate::platform::prepare(self.core, &req)).await;
        }
        Ok(p)
    }
}

impl Net for CoreNet<'_> {
    async fn account(&self) -> Result<Vec<Account>, String> {
        let list = self.core.list_servers().await.map_err(|e| e.to_string())?;
        Ok(list
            .servers
            .into_iter()
            .map(|s| Account {
                id: s.id,
                minecraft_java: s.capabilities.is_minecraft_java(),
                name: s.name,
                mybox_id: s.mybox_id,
                status: s.status,
            })
            .collect())
    }

    async fn capacity(&self, mybox: i64) -> Result<MyBoxCapacity, String> {
        self.core.mybox_capacity(mybox).await.map_err(|e| e.to_string())
    }

    async fn create_defaults(&self, mybox: i64, egg: Option<i64>, location: Option<i64>) -> Result<CreateDefaults, String> {
        let catalog = self.core.server_catalog().await.map_err(|e| format!("catalogue illisible : {e}"))?;
        let chosen = match egg {
            Some(id) => catalog.eggs.iter().find(|e| e.id == id).ok_or_else(|| format!("egg {id} absent du catalogue (server_catalog)"))?,
            None => catalog
                .eggs
                .iter()
                .find(|e| !e.experimental && e.name.trim().eq_ignore_ascii_case("minecraft"))
                .ok_or("jeu « Minecraft » introuvable dans le catalogue : précise `egg_id` (server_catalog)")?,
        };
        if !chosen.required_fields.is_empty() {
            return Err(format!(
                "le jeu {} demande des champs à la création ({}) : crée ces serveurs avec mybox_create_server, puis donne leur server_id",
                chosen.name,
                chosen.required_fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        let cap = self.capacity(mybox).await?;
        let (location_id, city) = if cap.pro {
            (None, None)
        } else if let Some(id) = location {
            let l = catalog.locations.iter().find(|l| l.id == id).ok_or_else(|| format!("localisation {id} inconnue (server_catalog)"))?;
            (Some(id), Some(l.city.clone()))
        } else {
            // Celle des serveurs déjà dans la MyBox, sinon la première en stock.
            let here = cap.servers.iter().find_map(|s| catalog.locations.iter().find(|l| l.in_stock && l.city.eq_ignore_ascii_case(&s.location)));
            match here.or_else(|| catalog.locations.iter().find(|l| l.in_stock)) {
                Some(l) => (Some(l.id), Some(l.city.clone())),
                None => return Err("aucune localisation en stock : précise `location_id` (server_catalog)".into()),
            }
        };
        Ok(CreateDefaults { egg_id: chosen.id, experimental: chosen.experimental, location_id, location: city })
    }

    async fn exists(&self, server: i64, path: &str) -> Result<bool, String> {
        let p = path.to_string();
        self.core
            .with_sftp(server, |conn| {
                let p = p.clone();
                async move { crate::sftp::probe(&conn, &p).await }
            })
            .await
            .map(|o| o.is_some())
            .map_err(|e| e.to_string())
    }

    async fn read_text(&self, server: i64, path: &str) -> Result<Option<String>, String> {
        let p = path.to_string();
        self.core
            .with_sftp(server, |conn| {
                let p = p.clone();
                async move {
                    match crate::sftp::probe(&conn, &p).await? {
                        None => Ok(None),
                        Some(e) if e.kind == crate::sftp::EntryKind::Dir => Ok(None),
                        Some(_) => crate::sftp::read_text(&conn, &p).await.map(Some),
                    }
                }
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn platform(&self, server: i64, platform: Option<&str>, version: Option<&str>, fresh: bool) -> Result<PlatformView, String> {
        let p = self.prepare(server, platform, version, fresh).await?;
        Ok(PlatformView {
            current_platform: p.current.platform.clone(),
            current_version: p.current.game_version.clone(),
            ops: crate::platform::ops(&p).into_iter().map(|o| (o.tool.to_string(), o.summary)).collect(),
            destructive: crate::platform::needs_confirmation(&p),
            blocking: p.blocking.clone(),
            warnings: p.warnings.clone(),
        })
    }

    async fn apply_platform(&self, server: i64, platform: Option<&str>, version: Option<&str>, fresh: bool) -> Result<PlatformRun, String> {
        let p = self.prepare(server, platform, version, fresh).await?;
        if !p.blocking.is_empty() {
            return Err(p.blocking.join(" ; "));
        }
        let (results, _) = Box::pin(crate::platform::execute(self.core, &p, false)).await;
        let async_change = crate::platform::needs_confirmation(&p) && results.iter().any(|r| r.tool == "change_version" && r.outcome == "réussie");
        Ok(PlatformRun {
            results: results.into_iter().map(|r| (r.tool.to_string(), r.outcome.to_string(), r.detail)).collect(),
            async_change,
        })
    }

    async fn state(&self, server: i64) -> Result<String, String> {
        self.core
            .server_live(server)
            .await
            .map(|l| crate::agent_loop::normalize_state(&l.state))
            .map_err(|e| e.to_string())
    }

    async fn world_fingerprint(&self, local_path: &str) -> Result<String, String> {
        let path = std::path::PathBuf::from(local_path);
        tokio::task::spawn_blocking(move || crate::world_upload::inspect_source(&path).map(|s| fingerprint(&s)))
            .await
            .map_err(|e| format!("lecture du monde interrompue : {e}"))?
    }

    async fn call(&self, tool: &str, args: Value) -> Result<String, String> {
        sub_dispatch(self.core, tool, args).await
    }

    async fn call_preapproved(&self, tool: &str, args: Value) -> Result<String, String> {
        crate::mcp_ctx::preapproved(sub_dispatch(self.core, tool, args)).await
    }

    async fn refusal(&self, tool: &str, args: &Value) -> Option<String> {
        if let Some(r) = crate::official_relay::env_scope_violation(args) {
            return Some(r);
        }
        crate::agent_guard::admit(self.core, &self.cfg, tool, args).await
    }

    fn record_created(&self, server: i64) {
        crate::agent_guard::record_created_server_here(server);
    }

    async fn wait_installed(&self, server: i64) -> Result<(), String> {
        let (state, _) = self.core.wait_installed(server).await?;
        self.core.ensure_stopped(server, &state).await
    }

    async fn confirm(&self, message: &str) -> Confirmation {
        crate::mcp_ctx::confirm(message, None).await
    }

    async fn confirm_name(&self, message: &str, expected: &str, given: Option<String>) -> Result<(), String> {
        let want = expected.to_string();
        let typed = crate::mcp_ctx::confirm_or_given(message, || async move { Ok(want) }, given).await?;
        if crate::platform::confirmation_matches(expected, &typed) {
            Ok(())
        } else {
            Err("ne correspond pas au nom exact du serveur (casse comprise)".into())
        }
    }

    async fn snapshot(&self, server: i64, name: &str) -> Result<(), String> {
        let job = self.core.create_snapshot(server, name).await.map_err(|e| e.to_string())?;
        Box::pin(crate::manifest::wait_snapshot(self.core, job, name)).await
    }

    async fn pause(&self, secs: u64) {
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
}

/// Empreinte d'un monde : chemins et tailles de ses entrées, triés (le contenu n'est pas relu).
pub(crate) fn fingerprint(src: &crate::world_upload::WorldSource) -> String {
    use sha2::{Digest, Sha256};
    let mut lines: Vec<String> = src.entries.iter().map(|e| format!("{}\t{}\t{}", e.rel, e.is_dir, e.size)).collect();
    lines.sort();
    let mut h = Sha256::new();
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    format!("sha256:{}", crate::util::hex(&h.finalize()))
}

// --- Plan --------------------------------------------------------------------------------------------

/// Statut d'un élément du plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    /// Déjà conforme.
    Ok,
    /// Sera créé.
    Create,
    /// Sera modifié.
    Change,
    /// Action humaine : `apply_network` ne la fait pas, sans bloquer le reste.
    Manual,
    /// Empêche toute application (rien n'est fait tant qu'il en reste).
    Blocked,
}

/// Phases d'`apply_network`, dans l'ordre d'exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// Rapprochement, place libre de la MyBox, création et redimensionnement.
    Servers,
    Platform,
    /// Contenu (plugins, mods, datapacks) avant le premier démarrage, qui génère leurs configs.
    Content,
    World,
    FirstStart,
    Manifest,
    Sync,
    Link,
    Presets,
    Start,
    Pregenerate,
    Verify,
}

impl Phase {
    pub(crate) const ALL: [Phase; 12] = [
        Phase::Servers,
        Phase::Platform,
        Phase::Content,
        Phase::World,
        Phase::FirstStart,
        Phase::Manifest,
        Phase::Sync,
        Phase::Link,
        Phase::Presets,
        Phase::Start,
        Phase::Pregenerate,
        Phase::Verify,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Phase::Servers => "servers",
            Phase::Platform => "platform",
            Phase::Content => "content",
            Phase::World => "world",
            Phase::FirstStart => "first_start",
            Phase::Manifest => "manifest",
            Phase::Sync => "sync",
            Phase::Link => "link",
            Phase::Presets => "presets",
            Phase::Start => "start",
            Phase::Pregenerate => "pregenerate",
            Phase::Verify => "verify",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Phase::Servers => "création et ressources des serveurs (mybox_create_server, mybox_resize_server)",
            Phase::Platform => "plateforme et version (logique de set_server_platform)",
            Phase::Content => "contenu avant le premier démarrage (apply_manifest steps=content)",
            Phase::World => "mondes de l'ordinateur (upload_world, avant le premier démarrage)",
            Phase::FirstStart => "premier démarrage puis arrêt (fichiers générés : velocity.toml, paper-global.yml, configs des plugins)",
            Phase::Manifest => "manifestes complets (apply_manifest avec apply_official)",
            Phase::Sync => "dossiers de l'ordinateur (sync_dir)",
            Phase::Link => "liaison au proxy (link_proxy)",
            Phase::Presets => "préréglages de réseau (network_preset ; démarrage puis relance si un plugin vient d'être installé)",
            Phase::Start => "démarrage des serveurs de jeu puis du proxy (redémarrage de ceux modifiés)",
            Phase::Pregenerate => "prégénération (pregenerate_world, Chunky)",
            Phase::Verify => "vérification (network_topology avec ping, ping_server)",
        }
    }
}

/// Un élément du plan.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Item {
    /// Identifiant stable : `<nom du serveur>:<quoi>` ou `network:<quoi>`.
    pub id: String,
    pub phase: Phase,
    /// Serveur concerné (indice dans `Network::nodes`), `None` pour le réseau.
    pub node: Option<usize>,
    pub status: Status,
    pub summary: String,
    /// Extrait du plan du sous-outil (étapes non conformes, compteurs).
    pub detail: Option<Value>,
}

/// Ce que le plan sait d'un serveur.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NodeState {
    /// Serveur existant (rapproché) ; `None` : à créer.
    pub server_id: Option<i64>,
    /// Comment il a été rapproché : `server_id`, `nom`.
    pub matched_by: Option<&'static str>,
    pub current_name: Option<String>,
    pub mybox_id: Option<i64>,
    /// Ressources actuelles (MyBox du blueprint) : cœurs, Go, en pause.
    pub current: Option<(i64, i64, bool)>,
    /// Plateforme actuelle (plan de plateforme), pour le type de Chunky.
    pub platform_now: Option<String>,
    /// Un démarrage génèrera ce qui manque (jamais démarré, config de plugin absente).
    pub needs_start: bool,
    /// Plan de plateforme : opérations à faire, destructives ou non.
    pub platform_ops: bool,
    pub destructive: bool,
    /// Le manifeste a des étapes de contenu à faire.
    pub content_todo: bool,
    pub manifest_todo: bool,
    /// Le monde visé est absent (rien à remplacer).
    pub world_absent: bool,
    /// Le serveur tourne (arrêt nécessaire pour son monde).
    pub running: bool,
}

/// Plan complet.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Plan {
    pub items: Vec<Item>,
    pub nodes: Vec<NodeState>,
    pub capacity: Value,
    /// Ce que l'humain confirme avant tout effet (récapitulatif).
    pub to_confirm: Vec<String>,
    /// Effets de `network_preset` qui ne se défont pas (`to_confirm` du préréglage), approuvés avec
    /// le récapitulatif.
    pub preset_confirm: Vec<String>,
    /// Serveurs existants dont `change_version` exige le nom exact retapé : (indice, nom actuel).
    pub name_confirm: Vec<(usize, String)>,
    pub warnings: Vec<String>,
    /// Jeu et localisation des serveurs à créer.
    pub create: Option<CreateDefaults>,
}

impl Plan {
    pub(crate) fn blocked(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.status == Status::Blocked).collect()
    }

    pub(crate) fn todo(&self) -> bool {
        self.items.iter().any(|i| matches!(i.status, Status::Create | Status::Change))
    }

    pub(crate) fn item(&self, node: Option<usize>, phase: Phase) -> Option<&Item> {
        self.items.iter().find(|i| i.node == node && i.phase == phase)
    }

    /// L'élément est-il à faire (création ou modification) ?
    pub(crate) fn pending(&self, node: Option<usize>, phase: Phase) -> bool {
        self.items
            .iter()
            .any(|i| i.node == node && i.phase == phase && matches!(i.status, Status::Create | Status::Change))
    }

    fn push(&mut self, id: String, phase: Phase, node: Option<usize>, status: Status, summary: impl Into<String>, detail: Option<Value>) {
        self.items.push(Item { id, phase, node, status, summary: summary.into(), detail });
    }
}

/// Fichier généré au premier démarrage : sa présence dit qu'un serveur a déjà démarré.
pub(crate) fn generated_file(node: &Node) -> &'static str {
    match node.platform.as_deref() {
        Some("bungeecord" | "waterfall") => "/config.yml",
        Some("velocity") => "/velocity.toml",
        _ if node.proxy => "/velocity.toml",
        _ => "/server.properties",
    }
}

/// Type de Chunky (plugin ou mod) pour une plateforme ; `None` : Chunky ne s'y installe pas.
pub(crate) fn chunky_kind(platform: Option<&str>) -> Option<&'static str> {
    match platform {
        None => Some("plugin"),
        Some(p) => crate::pregen::platform_kind(p).map(|k| match k {
            crate::pregen::Kind::Plugin => "plugin",
            crate::pregen::Kind::Mod => "mod",
        }),
    }
}

/// Étapes non conformes d'un plan de sous-outil (`steps[]` : `{id, status, summary}`), compactées.
fn pending_steps(v: &Value) -> Vec<Value> {
    v.get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter(|s| s.get("status").and_then(Value::as_str).is_some_and(|st| st != "ok"))
                .map(|s| {
                    let mut o = json!({ "id": s["id"], "status": s["status"], "summary": s["summary"] });
                    if let Some(id) = s.get("server_id") {
                        o["server_id"] = id.clone();
                    }
                    o
                })
                .collect()
        })
        .unwrap_or_default()
}

fn count(v: &Value, key: &str) -> i64 {
    v.pointer(&format!("/counts/{key}")).and_then(Value::as_i64).unwrap_or(0)
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Serveurs (identifiants) qu'une étape `manual` « démarrer une fois » d'un sous-outil désigne.
pub(crate) fn start_needed_ids(v: &Value) -> Vec<i64> {
    let mut ids: Vec<i64> = v
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter(|s| s.get("status").and_then(Value::as_str) == Some("manual"))
                .filter(|s| s.get("summary").and_then(Value::as_str).is_some_and(needs_start_hint))
                .filter_map(|s| s.get("server_id").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Arguments de `link_proxy` (ids : identifiants résolus, `None` = pas encore créé).
pub(crate) fn link_args(nw: &Network, ids: &[Option<i64>], dry_run: bool) -> Option<Value> {
    let p = nw.proxy_index()?;
    let opts = nw.proxy.as_ref()?;
    let proxy_id = ids[p]?;
    let order = nw.backends();
    let backend_ids: Vec<i64> = order.iter().map(|i| ids[*i]).collect::<Option<_>>()?;
    let mut a = json!({ "proxy_server_id": proxy_id, "backend_server_ids": backend_ids, "prune": opts.prune, "dry_run": dry_run, "snapshot": false });
    let names: Map<String, Value> = order
        .iter()
        .filter_map(|i| nw.nodes[*i].entry.as_ref().map(|e| (ids[*i].unwrap_or_default().to_string(), json!(e))))
        .collect();
    if !names.is_empty() {
        a["names"] = Value::Object(names);
    }
    if !opts.try_order.is_empty() {
        a["try"] = json!(opts.try_order.iter().map(|i| ids[*i]).collect::<Vec<_>>());
    }
    if !opts.forced_hosts.is_empty() {
        let hosts: Map<String, Value> = opts
            .forced_hosts
            .iter()
            .map(|(h, list)| (h.clone(), json!(list.iter().map(|i| ids[*i]).collect::<Vec<_>>())))
            .collect();
        a["forced_hosts"] = Value::Object(hosts);
    }
    if let Some(m) = &opts.mode {
        a["mode"] = json!(m);
    }
    Some(a)
}

/// Arguments de `network_preset`.
pub(crate) fn preset_args(nw: &Network, ids: &[Option<i64>], preset: &str, dry_run: bool) -> Option<Value> {
    let p = nw.proxy_index()?;
    let backend_ids: Vec<i64> = nw.backends().iter().map(|i| ids[*i]).collect::<Option<_>>()?;
    let mut a = json!({ "preset": preset, "proxy_server_id": ids[p]?, "backend_server_ids": backend_ids, "dry_run": dry_run, "snapshot": false });
    if preset == "luckperms" {
        if let Some(d) = &nw.database {
            a["database_id"] = json!(d.database_id);
            if let Some(m) = d.mybox_id {
                a["mybox_id"] = json!(m);
            }
        }
    }
    if preset == "geyser" {
        if let Some(port) = nw.proxy.as_ref().and_then(|o| o.bedrock_port) {
            a["bedrock_port"] = json!(port);
        }
    }
    Some(a)
}

/// Marqueur d'un serveur (`/.minestrator-blueprint.json`) : objet, vide s'il est absent ou illisible.
pub(crate) async fn read_marker<N: Net>(net: &N, server: i64) -> Value {
    match net.read_text(server, MARKER_PATH).await {
        Ok(Some(t)) => serde_json::from_str::<Value>(&t).ok().filter(Value::is_object).unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}

/// Signature d'une prégénération (comparée au marqueur).
pub(crate) fn pregen_signature(p: &PregenSpec) -> Value {
    json!({
        "radius": p.radius,
        "world": p.world.as_deref().unwrap_or("level-name"),
        "shape": p.shape.as_deref().unwrap_or("square").to_ascii_lowercase(),
        "center": p.center.clone().unwrap_or_else(|| json!("spawn")),
    })
}

/// Le marqueur dit-il cette prégénération déjà lancée (mêmes réglages ; date ignorée) ?
pub(crate) fn pregen_marked(marker: &Value, p: &PregenSpec) -> bool {
    let Some(m) = marker.get("pregenerate") else { return false };
    let sig = pregen_signature(p);
    sig.as_object().is_some_and(|o| o.iter().all(|(k, v)| m.get(k) == Some(v)))
}

/// Rapprochement des serveurs du blueprint avec le compte.
fn match_nodes(nw: &Network, account: &[Account], plan: &mut Plan) {
    for (i, n) in nw.nodes.iter().enumerate() {
        let mut st = NodeState::default();
        let id_key = format!("{}:server", n.name);
        let found: Result<Option<&Account>, String> = match n.server_id {
            Some(id) => match account.iter().find(|a| a.id == id) {
                Some(a) => Ok(Some(a)),
                None => Err(format!("{} : serveur {id} introuvable sur ce compte (list_servers)", n.label())),
            },
            None => {
                let same: Vec<&Account> = account.iter().filter(|a| a.mybox_id == nw.mybox_id && a.name.trim() == n.name).collect();
                match same.as_slice() {
                    [] => Ok(None),
                    [a] => Ok(Some(*a)),
                    many => Err(format!(
                        "{} : {} serveurs portent ce nom dans la MyBox {} ({}) : précise `server_id`",
                        n.label(),
                        many.len(),
                        nw.mybox_id,
                        many.iter().map(|a| a.id.to_string()).collect::<Vec<_>>().join(", ")
                    )),
                }
            }
        };
        match found {
            Err(e) => plan.push(id_key, Phase::Servers, Some(i), Status::Blocked, e, None),
            Ok(None) => {
                let missing: Vec<&str> = [("cpu", n.cpu), ("ram", n.ram)].iter().filter(|(_, v)| v.is_none()).map(|(k, _)| *k).collect();
                if missing.is_empty() {
                    plan.push(
                        id_key,
                        Phase::Servers,
                        Some(i),
                        Status::Create,
                        format!("{} à créer dans la MyBox {} : {} cœur(s), {} Go de RAM", n.label(), nw.mybox_id, n.cpu.unwrap_or(0), n.ram.unwrap_or(0)),
                        None,
                    );
                } else {
                    plan.push(id_key, Phase::Servers, Some(i), Status::Blocked, format!("{} : aucun serveur de ce nom dans la MyBox {} ; pour le créer, `{}` requis", n.label(), nw.mybox_id, missing.join("` et `")), None);
                }
            }
            Ok(Some(a)) => {
                st.server_id = Some(a.id);
                st.matched_by = Some(if n.server_id.is_some() { "server_id" } else { "nom" });
                st.current_name = Some(a.name.clone());
                st.mybox_id = Some(a.mybox_id);
                if !a.minecraft_java {
                    plan.push(id_key, Phase::Servers, Some(i), Status::Blocked, format!("{} : « {} » ({}) n'est pas un serveur Minecraft Java", n.label(), a.name, a.id), None);
                } else if !a.usable() {
                    plan.push(
                        id_key,
                        Phase::Servers,
                        Some(i),
                        Status::Blocked,
                        format!("{} : « {} » ({}) est {} : fichiers inaccessibles — reprends-le d'abord (mybox_pause_server paused=false) ou retire-le du blueprint", n.label(), a.name, a.id, status_label(&a.status)),
                        None,
                    );
                } else {
                    plan.push(id_key, Phase::Servers, Some(i), Status::Ok, format!("{} : serveur {} rapproché par {}", n.label(), a.id, st.matched_by.unwrap_or_default()), None);
                }
            }
        }
        plan.nodes.push(st);
    }
    // Deux éléments du blueprint ne peuvent viser le même serveur (nom de l'un, identifiant de l'autre).
    for i in 0..plan.nodes.len() {
        for j in (i + 1)..plan.nodes.len() {
            if let (Some(a), Some(b)) = (plan.nodes[i].server_id, plan.nodes[j].server_id) {
                if a == b {
                    let id = format!("{}:server", nw.nodes[j].name);
                    plan.push(id, Phase::Servers, Some(j), Status::Blocked, format!("{} et {} désignent le même serveur ({a})", nw.nodes[i].label(), nw.nodes[j].label()), None);
                }
            }
        }
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "suspended" => "en pause ou suspendu",
        "expired" => "expiré",
        "disabled" => "désactivé",
        _ => "indisponible",
    }
}

/// Place libre de la MyBox : créations et redimensionnements (CPU en cœurs, RAM en Go, emplacements).
fn plan_capacity(nw: &Network, cap: &MyBoxCapacity, plan: &mut Plan) {
    let mut cpu = 0i64;
    let mut ram = 0i64;
    let mut creations = 0i64;
    for (i, n) in nw.nodes.iter().enumerate() {
        let st = &plan.nodes[i];
        match st.server_id {
            None => {
                if plan.pending(Some(i), Phase::Servers) {
                    cpu += n.cpu.unwrap_or(0);
                    ram += n.ram.unwrap_or(0);
                    creations += 1;
                }
            }
            Some(id) => {
                let Some(slot) = cap.servers.iter().find(|s| s.id == id) else {
                    if n.cpu.is_some() || n.ram.is_some() {
                        let sid = format!("{}:resources", n.name);
                        plan.push(sid, Phase::Servers, Some(i), Status::Manual, format!("{} : serveur hors de la MyBox {} : ressources à régler dans sa propre MyBox (mybox_resize_server)", n.label(), nw.mybox_id), None);
                    }
                    continue;
                };
                plan.nodes[i].current = Some((slot.cpu, slot.ram, slot.paused));
                let (want_cpu, want_ram) = (n.cpu.unwrap_or(slot.cpu), n.ram.unwrap_or(slot.ram));
                if (want_cpu, want_ram) == (slot.cpu, slot.ram) {
                    if n.cpu.is_some() || n.ram.is_some() {
                        plan.push(format!("{}:resources", n.name), Phase::Servers, Some(i), Status::Ok, format!("{} : {} cœur(s), {} Go de RAM", n.label(), slot.cpu, slot.ram), None);
                    }
                    continue;
                }
                if !slot.paused {
                    cpu += want_cpu - slot.cpu;
                    ram += want_ram - slot.ram;
                }
                plan.push(
                    format!("{}:resources", n.name),
                    Phase::Servers,
                    Some(i),
                    Status::Change,
                    format!("{} : redimensionner de {} à {} cœur(s), de {} à {} Go de RAM (mybox_resize_server ; l'hébergeur peut exiger le serveur arrêté)", n.label(), slot.cpu, want_cpu, slot.ram, want_ram),
                    None,
                );
            }
        }
    }
    let mut problems: Vec<String> = Vec::new();
    if creations > 0 {
        for b in &cap.blockers {
            match b.as_str() {
                "expired" => problems.push("MyBox expirée : renouvelle-la d'abord".into()),
                "suspended" => problems.push("MyBox suspendue".into()),
                "forbidden" => problems.push("création de serveurs non permise sur cette MyBox (sous-utilisateur sans droit server_create)".into()),
                _ => {}
            }
        }
        if cap.slots.max_active > 0 && cap.slots.active + creations > cap.slots.max_active {
            problems.push(format!(
                "emplacements actifs : {creations} serveur(s) à créer, {} libre(s) sur {} (mets en pause un serveur inutile)",
                (cap.slots.max_active - cap.slots.active).max(0),
                cap.slots.max_active
            ));
        }
    }
    if !cap.pro {
        if cpu > cap.cpu.remaining {
            problems.push(format!("CPU : {cpu} cœur(s) demandé(s), {} restant(s) — il manque {} cœur(s)", cap.cpu.remaining, cpu - cap.cpu.remaining));
        }
        if ram > cap.ram.remaining {
            problems.push(format!("RAM : {ram} Go demandés, {} Go restants — il manque {} Go", cap.ram.remaining, ram - cap.ram.remaining));
        }
    }
    plan.capacity = json!({
        "mybox_id": cap.id,
        "mybox": cap.name,
        "pro": cap.pro,
        "requested": { "cpu": cpu, "ram": ram, "servers": creations },
        "remaining": { "cpu": cap.cpu.remaining, "ram": cap.ram.remaining },
        "after": { "cpu": cap.cpu.remaining - cpu, "ram": cap.ram.remaining - ram },
        "slots": { "active": cap.slots.active, "max_active": cap.slots.max_active },
        "units": "CPU en cœurs entiers, RAM en Go entiers ; seuls les serveurs actifs consomment",
    });
    if problems.is_empty() {
        if creations > 0 || cpu != 0 || ram != 0 {
            plan.push("network:capacity".into(), Phase::Servers, None, Status::Ok, format!("place suffisante dans la MyBox « {} » : {cpu} cœur(s) et {ram} Go demandés en plus", cap.name), None);
        }
    } else {
        plan.push(
            "network:capacity".into(),
            Phase::Servers,
            None,
            Status::Blocked,
            format!("pas assez de place dans la MyBox « {} » : {} (plan_placement pour une autre répartition)", cap.name, problems.join(" ; ")),
            None,
        );
    }
}

/// Éléments d'un serveur, rassemblés par des fonctions **synchrones** : leurs temporaires (textes,
/// JSON) ne restent pas sur la pile pendant l'appel d'un sous-outil (cadres de pile de debug).
struct NodeItems<'a> {
    n: &'a Node,
    i: usize,
    items: Vec<Item>,
    confirm: Vec<String>,
}

impl NodeItems<'_> {
    fn add(&mut self, phase: Phase, what: &str, status: Status, summary: String, detail: Option<Value>) {
        self.items.push(Item { id: format!("{}:{what}", self.n.name), phase, node: Some(self.i), status, summary, detail });
    }
}

/// Plan d'un serveur existant ou à créer (hors réseau) : plateforme, premier démarrage,
/// manifeste, synchronisations, monde, prégénération. Seuls les appels de sous-outils sont ici ;
/// leur lecture est faite à côté, sans `await`.
async fn plan_node<N: Net>(net: &N, nw: &Network, i: usize, st: NodeState) -> (NodeState, Vec<Item>, Vec<String>) {
    let n = &nw.nodes[i];
    let mut st = st;
    let mut out = NodeItems { n, i, items: Vec::new(), confirm: Vec::new() };
    let Some(id) = st.server_id else {
        let world = match &n.world {
            Some(w) => Some(net.world_fingerprint(&w.local_path).await),
            None => None,
        };
        new_node_items(&mut out, &mut st, world);
        return (st, out.items, out.confirm);
    };
    let view = net.platform(id, n.platform.as_deref(), n.version.as_deref(), false).await;
    platform_items(&mut out, &mut st, view);
    let platform = n.platform.clone().or_else(|| st.platform_now.clone());
    let files = net.exists(id, generated_file(n)).await;
    files_items(&mut out, &mut st, files);
    if let Some(m) = effective_manifest(n, true, chunky_kind(platform.as_deref())) {
        let r = net.call("plan_manifest", manifest_args(id, m)).await;
        manifest_items(&mut out, &mut st, r);
    }
    first_start_item(&mut out, &st);
    for (k, s) in n.syncs.iter().enumerate() {
        let r = net.call("sync_dir", sync_args(id, s, true)).await;
        sync_items(&mut out, k, s, r);
    }
    let marker = if n.world.is_some() || n.pregen.is_some() { read_marker(net, id).await } else { Value::Null };
    if let Some(w) = &n.world {
        let fp = net.world_fingerprint(&w.local_path).await;
        let installed = fp.as_ref().is_ok_and(|fp| marker.pointer("/world/fingerprint").and_then(Value::as_str) == Some(fp.as_str()));
        let sim = if fp.is_ok() && !installed { Some(net.call("upload_world", world_args(id, w)).await) } else { None };
        world_items(&mut out, &mut st, w, fp, sim);
    }
    if let Some(p) = &n.pregen {
        pregen_items(&mut out, platform.as_deref(), &marker, p);
    }
    (st, out.items, out.confirm)
}

fn manifest_args(id: i64, m: Manifest) -> Value {
    json!({ "server_id": id, "manifest": m })
}

fn world_args(id: i64, w: &WorldSpec) -> Value {
    let mut a = json!({ "server_id": id, "local_path": w.local_path, "replace": true, "stop": true, "dry_run": true });
    if let Some(name) = &w.world_name {
        a["world_name"] = json!(name);
    }
    a
}

/// Serveur à créer : tout se fait après la création ; seules les sources locales se vérifient.
fn new_node_items(out: &mut NodeItems<'_>, st: &mut NodeState, world: Option<Result<String, String>>) {
    let n = out.n;
    if n.platform.is_some() || n.version.is_some() {
        out.add(
            Phase::Platform,
            "platform",
            Status::Create,
            format!("{} : {} {} après la création (voie résolue à ce moment-là : catalogue de l'hébergeur ou JAR officiel)", n.label(), n.platform.as_deref().unwrap_or("plateforme installée"), n.version.as_deref().unwrap_or("(version installée)")),
            None,
        );
    }
    let manifest = effective_manifest(n, false, chunky_kind(n.platform.as_deref()));
    if manifest.as_ref().is_some_and(has_content) {
        out.add(Phase::Content, "content", Status::Create, format!("{} : contenu du manifeste installé avant le premier démarrage", n.label()), None);
        st.content_todo = true;
    }
    st.needs_start = true;
    out.add(Phase::FirstStart, "first_start", Status::Create, format!("{} : premier démarrage puis arrêt (fichiers générés)", n.label()), None);
    if let Some(m) = &manifest {
        out.add(Phase::Manifest, "manifest", Status::Create, format!("{} : manifeste appliqué après la création ({})", n.label(), manifest_summary(m)), None);
        st.manifest_todo = true;
    }
    for (k, s) in n.syncs.iter().enumerate() {
        let (status, summary) = if std::path::Path::new(&s.local_dir).is_dir() {
            (Status::Create, format!("{} : « {} » synchronisé vers {}{}", n.label(), s.local_dir, s.remote_dir, if s.delete_extra { " (suppressions comprises)" } else { "" }))
        } else {
            (Status::Blocked, format!("{} : dossier local « {} » introuvable", n.label(), s.local_dir))
        };
        out.add(Phase::Sync, &format!("sync[{k}]"), status, summary, None);
    }
    if let (Some(w), Some(fp)) = (&n.world, world) {
        match fp {
            Ok(_) => out.add(Phase::World, "world", Status::Create, format!("{} : monde « {} » installé avant le premier démarrage", n.label(), w.local_path), None),
            Err(e) => out.add(Phase::World, "world", Status::Blocked, format!("{} : monde « {} » inutilisable : {e}", n.label(), w.local_path), None),
        }
        st.world_absent = true;
    }
    if let Some(p) = &n.pregen {
        out.add(Phase::Pregenerate, "pregenerate", Status::Create, pregen_summary(n, p), None);
    }
}

/// Plateforme et version (simulation de set_server_platform) ; lue aussi pour Chunky.
fn platform_items(out: &mut NodeItems<'_>, st: &mut NodeState, view: Result<PlatformView, String>) {
    let n = out.n;
    let wants = n.platform.is_some() || n.version.is_some();
    let v = match view {
        Ok(v) => v,
        Err(e) if wants => return out.add(Phase::Platform, "platform", Status::Blocked, format!("{} : plateforme illisible ({e})", n.label()), None),
        Err(_) => return,
    };
    st.platform_now = v.current_platform.clone();
    if !wants {
        return;
    }
    let detail = json!({ "current": { "platform": v.current_platform, "version": v.current_version }, "operations": v.ops.iter().map(|(t, s)| json!({ "tool": t, "summary": s })).collect::<Vec<_>>(), "warnings": v.warnings });
    if !v.blocking.is_empty() {
        out.add(Phase::Platform, "platform", Status::Blocked, format!("{} : {}", n.label(), v.blocking.join(" ; ")), Some(detail));
    } else if v.ops.is_empty() {
        out.add(Phase::Platform, "platform", Status::Ok, format!("{} : {} {}", n.label(), v.current_platform.as_deref().unwrap_or("?"), v.current_version.as_deref().unwrap_or("?")), None);
    } else {
        st.platform_ops = true;
        st.destructive = v.destructive;
        let what = v.ops.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" ; ");
        out.confirm.push(if v.destructive {
            format!("{} : {what} — DESTRUCTIF (nom exact du serveur à confirmer)", n.label())
        } else {
            format!("{} : {what}", n.label())
        });
        out.add(Phase::Platform, "platform", Status::Change, format!("{} : {what}", n.label()), Some(detail));
    }
}

/// Fichiers générés au premier démarrage.
fn files_items(out: &mut NodeItems<'_>, st: &mut NodeState, files: Result<bool, String>) {
    match files {
        Ok(false) => st.needs_start = true,
        Ok(true) => {}
        Err(e) => out.add(Phase::FirstStart, "files", Status::Blocked, format!("{} : fichiers illisibles par SFTP ({e})", out.n.label()), None),
    }
}

fn first_start_item(out: &mut NodeItems<'_>, st: &NodeState) {
    if st.needs_start {
        let n = out.n;
        out.add(Phase::FirstStart, "first_start", Status::Change, format!("{} : premier démarrage nécessaire (fichier absent : {} ou config de plugin), puis arrêt", n.label(), generated_file(n)), None);
    }
}

/// Manifeste (simulation de plan_manifest).
fn manifest_items(out: &mut NodeItems<'_>, st: &mut NodeState, r: Result<String, String>) {
    let n = out.n;
    let text = match r {
        Ok(t) => t,
        Err(e) => return out.add(Phase::Manifest, "manifest", Status::Blocked, format!("{} : manifeste refusé : {e}", n.label()), None),
    };
    let v = parse_out(&text);
    let todo = v.get("to_apply").and_then(Value::as_i64).unwrap_or(0);
    let steps = pending_steps(&v);
    let manual = steps.iter().filter(|s| matches!(s["status"].as_str(), Some("manual" | "unsupported"))).count();
    let starts = steps
        .iter()
        .any(|s| s["status"].as_str() == Some("manual") && s["summary"].as_str().is_some_and(needs_start_hint));
    if starts {
        st.needs_start = true;
    }
    let content = steps
        .iter()
        .filter(|s| s["id"].as_str().is_some_and(|x| x.starts_with("content.")) && matches!(s["status"].as_str(), Some("change" | "add" | "remove")))
        .count();
    st.content_todo = content > 0;
    if content > 0 {
        out.add(Phase::Content, "content", Status::Change, format!("{} : {content} contenu(s) installé(s), remplacé(s) ou retiré(s) d'abord (avant un premier démarrage éventuel)", n.label()), None);
    }
    let detail = json!({ "counts": v.get("counts"), "to_apply": todo, "steps": steps, "unlisted": v.get("unlisted") });
    if todo > 0 || starts {
        st.manifest_todo = true;
        let mut s = format!("{} : manifeste, {todo} étape(s) à appliquer", n.label());
        if starts {
            s.push_str(" ; des configs seront générées au premier démarrage");
        }
        out.add(Phase::Manifest, "manifest", Status::Change, s, Some(detail));
    } else if manual > 0 {
        out.add(Phase::Manifest, "manifest", Status::Manual, format!("{} : manifeste conforme hors étapes manuelles ({manual})", n.label()), Some(detail));
    } else {
        out.add(Phase::Manifest, "manifest", Status::Ok, format!("{} : manifeste conforme", n.label()), None);
    }
}

/// Synchronisation (simulation de sync_dir).
fn sync_items(out: &mut NodeItems<'_>, k: usize, s: &SyncSpec, r: Result<String, String>) {
    let n = out.n;
    let what = format!("sync[{k}]");
    let text = match r {
        Ok(t) => t,
        Err(e) => return out.add(Phase::Sync, &what, Status::Blocked, format!("{} : synchronisation de « {} » impossible : {e}", n.label(), s.local_dir), None),
    };
    let v = parse_out(&text);
    let (add_n, mod_n, del_n) = (count(&v, "added"), count(&v, "modified"), count(&v, "deleted"));
    let refusals = strings(&v, "refusals");
    let detail = json!({ "counts": v.get("counts"), "bytes_to_send": v.get("bytes_to_send"), "refusals": refusals });
    if !refusals.is_empty() {
        out.add(Phase::Sync, &what, Status::Blocked, format!("{} : synchronisation de « {} » refusée : {}", n.label(), s.local_dir, refusals.join(" ; ")), Some(detail));
    } else if add_n + mod_n + del_n == 0 {
        out.add(Phase::Sync, &what, Status::Ok, format!("{} : {} déjà synchronisé", n.label(), s.remote_dir), None);
    } else {
        out.add(Phase::Sync, &what, Status::Change, format!("{} : {} → {} : {add_n} ajouté(s), {mod_n} modifié(s), {del_n} supprimé(s)", n.label(), s.local_dir, s.remote_dir), Some(detail));
    }
}

/// Monde : déjà installé d'après le marqueur (`sim` absent), sinon simulation d'upload_world.
fn world_items(out: &mut NodeItems<'_>, st: &mut NodeState, w: &WorldSpec, fp: Result<String, String>, sim: Option<Result<String, String>>) {
    let n = out.n;
    if let Err(e) = fp {
        return out.add(Phase::World, "world", Status::Blocked, format!("{} : monde « {} » inutilisable : {e}", n.label(), w.local_path), None);
    }
    let text = match sim {
        None => return out.add(Phase::World, "world", Status::Ok, format!("{} : monde déjà installé depuis cette source (marqueur {MARKER_PATH})", n.label()), None),
        Some(Err(e)) => return out.add(Phase::World, "world", Status::Blocked, format!("{} : monde refusé : {e}", n.label()), None),
        Some(Ok(t)) => t,
    };
    let v = parse_out(&text);
    let problems = strings(&v, "problems");
    let aside: Vec<String> = v
        .get("aside")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.get("from").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default();
    st.running = matches!(v.get("server_state").and_then(Value::as_str), Some("running" | "starting"));
    st.world_absent = aside.is_empty();
    let target = v.get("target").and_then(Value::as_str).unwrap_or("?").to_string();
    let detail = json!({ "target": target, "aside": v.get("aside"), "source": v.get("source"), "warnings": v.get("warnings") });
    if !problems.is_empty() {
        out.add(Phase::World, "world", Status::Blocked, format!("{} : monde refusé : {}", n.label(), problems.join(" ; ")), Some(detail));
    } else if !aside.is_empty() && !w.replace {
        out.add(Phase::World, "world", Status::Manual, format!("{} : un monde existe déjà en {target}, non installé par ce blueprint : `world.replace: true` pour le remplacer (l'ancien est renommé, jamais supprimé), ou retire `world`", n.label()), Some(detail));
    } else {
        let mut s = format!("{} : monde « {} » installé en {target}", n.label(), w.local_path);
        if !aside.is_empty() {
            s.push_str(&format!(" ; ancien monde renommé ({})", aside.join(", ")));
            out.confirm.push(format!("{} : monde remplacé (ancien renommé, jamais supprimé)", n.label()));
        }
        if st.running {
            s.push_str(" ; serveur arrêté pour l'installer");
            out.confirm.push(format!("{} : arrêté pour installer son monde (joueurs déconnectés)", n.label()));
        }
        out.add(Phase::World, "world", Status::Change, s, Some(detail));
    }
}

/// Prégénération : déjà lancée d'après le marqueur (mêmes réglages), sinon après le démarrage.
fn pregen_items(out: &mut NodeItems<'_>, platform: Option<&str>, marker: &Value, p: &PregenSpec) {
    let n = out.n;
    if chunky_kind(platform).is_none() {
        out.add(Phase::Pregenerate, "pregenerate", Status::Blocked, format!("{} : Chunky ne s'installe pas sur la plateforme {} (vanilla ?)", n.label(), platform.unwrap_or("?")), None);
    } else if pregen_marked(marker, p) {
        out.add(Phase::Pregenerate, "pregenerate", Status::Ok, format!("{} : prégénération déjà lancée avec ces réglages (marqueur {MARKER_PATH} ; suivi : pregenerate_world action=status)", n.label()), None);
    } else {
        out.add(Phase::Pregenerate, "pregenerate", Status::Change, pregen_summary(n, p), None);
    }
}

fn manifest_summary(m: &Manifest) -> String {
    let mut parts = Vec::new();
    if let Some(p) = &m.properties {
        if !p.is_empty() {
            parts.push(format!("{} réglage(s)", p.len()));
        }
    }
    if let Ok((items, _)) = m.content_items() {
        if !items.is_empty() {
            parts.push(format!("{} contenu(s)", items.len()));
        }
    }
    if !m.configs.is_empty() {
        parts.push(format!("{} config(s)", m.configs.len()));
    }
    if !m.files.is_empty() {
        parts.push(format!("{} fichier(s)", m.files.len()));
    }
    if m.startup.is_some() {
        parts.push("démarrage".into());
    }
    if let Ok((s, _)) = m.schedule_items() {
        if !s.is_empty() {
            parts.push(format!("{} tâche(s) planifiée(s)", s.len()));
        }
    }
    if m.players.is_some() {
        parts.push("joueurs".into());
    }
    if parts.is_empty() {
        "nom".into()
    } else {
        parts.join(", ")
    }
}

fn pregen_summary(n: &Node, p: &PregenSpec) -> String {
    let shape = crate::pregen::Request::parse(&pregen_args(0, p)).map(|r| r.shape).unwrap_or(crate::pregen::Shape::Square);
    let chunks = crate::pregen::estimate_chunks(p.radius, shape);
    let disk = crate::pregen::disk_estimate(chunks, None);
    format!(
        "{} : prégénération de rayon {} blocs ({chunks} chunks, disque {}) lancée une fois le serveur démarré (Chunky installé par le manifeste s'il manque)",
        n.label(),
        p.radius,
        disk.summary.split(" ; ").next().unwrap_or_default()
    )
}

/// Plan complet : rapprochement, place libre, chaque serveur, puis le réseau.
pub(crate) async fn plan_with<N: Net>(net: &N, nw: &Network) -> Result<Plan, String> {
    crate::mcp_ctx::progress(0.0, None, "lecture du compte et de la MyBox").await;
    let account = net.account().await.map_err(|e| format!("liste des serveurs illisible : {e}"))?;
    let mut plan = Plan::default();
    match_nodes(nw, &account, &mut plan);
    match net.capacity(nw.mybox_id).await {
        Ok(cap) => plan_capacity(nw, &cap, &mut plan),
        Err(e) => plan.push("network:capacity".into(), Phase::Servers, None, Status::Blocked, format!("MyBox {} illisible : {e}", nw.mybox_id), None),
    }
    if plan.nodes.iter().any(|s| s.server_id.is_none()) {
        match net.create_defaults(nw.mybox_id, nw.egg_id, nw.location_id).await {
            Ok(d) => plan.create = Some(d),
            Err(e) => plan.push("network:create".into(), Phase::Servers, None, Status::Blocked, format!("création impossible : {e}"), None),
        }
        for (i, n) in nw.nodes.iter().enumerate() {
            if plan.pending(Some(i), Phase::Servers) && plan.nodes[i].server_id.is_none() {
                plan.to_confirm.push(format!("créer {} : {} cœur(s), {} Go de RAM", n.label(), n.cpu.unwrap_or(0), n.ram.unwrap_or(0)));
            }
        }
    }
    for i in plan.items.iter().filter(|i| i.id.ends_with(":resources") && i.status == Status::Change) {
        plan.to_confirm.push(i.summary.clone());
    }
    // Serveurs : en parallèle (4 à la fois), dans l'ordre du blueprint. Un serveur bloqué au
    // rapprochement n'est pas lu plus avant.
    crate::mcp_ctx::progress(1.0, None, "plan de chaque serveur").await;
    let usable: Vec<usize> = (0..nw.nodes.len())
        .filter(|i| !plan.items.iter().any(|it| it.node == Some(*i) && it.phase == Phase::Servers && it.status == Status::Blocked && it.id.ends_with(":server")))
        .collect();
    let tasks: Vec<_> = usable.iter().map(|i| plan_node(net, nw, *i, plan.nodes[*i].clone())).collect();
    let results: Vec<(NodeState, Vec<Item>, Vec<String>)> = stream::iter(tasks).buffered(4).collect().await;
    for (i, (st, items, confirm)) in usable.iter().zip(results) {
        if st.destructive {
            if let Some(name) = st.current_name.clone() {
                plan.name_confirm.push((*i, name));
            }
        }
        plan.nodes[*i] = st;
        plan.items.extend(items);
        plan.to_confirm.extend(confirm);
    }
    if crate::mcp_ctx::cancelled() {
        return Err("annulé".into());
    }
    crate::mcp_ctx::progress(2.0, None, "plan du réseau").await;
    plan_network_items(net, nw, &mut plan).await;
    // Liaison ou préréglage qui attend des fichiers générés au démarrage (Paper, plugin installé).
    for (i, n) in nw.nodes.iter().enumerate() {
        if plan.nodes[i].needs_start && plan.item(Some(i), Phase::FirstStart).is_none() {
            plan.push(format!("{}:first_start", n.name), Phase::FirstStart, Some(i), Status::Change, format!("{} : démarrage nécessaire pour générer des fichiers attendus par la liaison ou un préréglage, puis arrêt", n.label()), None);
        }
    }
    plan.items.sort_by_key(|i| (i.phase, i.node.unwrap_or(usize::MAX)));
    Ok(plan)
}

/// Liaison au proxy et préréglages : simulés si tous les serveurs existent, sinon décrits.
async fn plan_network_items<N: Net>(net: &N, nw: &Network, plan: &mut Plan) {
    let Some(opts) = &nw.proxy else { return };
    let ids: Vec<Option<i64>> = plan.nodes.iter().map(|s| s.server_id).collect();
    let usable = !plan.items.iter().any(|i| i.phase == Phase::Servers && i.status == Status::Blocked && i.id.ends_with(":server"));
    // Proxy jamais démarré : pas encore de velocity.toml (ou config.yml) à lire ; liaison et
    // préréglages se simulent après son premier démarrage.
    let proxy_unstarted = nw.proxy_index().is_some_and(|p| plan.nodes[p].server_id.is_some() && plan.nodes[p].needs_start);
    let names = |list: &[usize]| list.iter().map(|i| nw.nodes[*i].name.clone()).collect::<Vec<_>>().join(", ");
    if usable && proxy_unstarted {
        plan.push("network:link".into(), Phase::Link, None, Status::Change, format!("liaison de {} au proxy après son premier démarrage (sa configuration n'existe pas encore : simulée à ce moment-là)", names(&nw.backends())), None);
        for preset in &nw.presets {
            plan.push(format!("network:preset.{preset}"), Phase::Presets, None, Status::Change, format!("préréglage {preset} après le premier démarrage du proxy (simulé à ce moment-là)"), None);
        }
        return;
    }
    match link_args(nw, &ids, true).filter(|_| usable) {
        None => {
            let mut s = format!("serveurs déclarés sur le proxy après leur création : {}", names(&nw.backends()));
            if !opts.try_order.is_empty() {
                s.push_str(&format!(" ; repli : {}", names(&opts.try_order)));
            }
            if !opts.forced_hosts.is_empty() {
                s.push_str(&format!(" ; domaines : {}", opts.forced_hosts.iter().map(|(h, l)| format!("{h} → {}", names(l))).collect::<Vec<_>>().join(", ")));
            }
            s.push_str(&format!(" ; transfert {}", opts.mode.as_deref().unwrap_or("par défaut du proxy")));
            plan.push("network:link".into(), Phase::Link, None, if usable { Status::Create } else { Status::Manual }, if usable { s } else { "liaison simulée une fois corrigés les serveurs bloqués".into() }, None);
        }
        Some(args) => match net.call("link_proxy", args).await {
            Err(e) => plan.push("network:link".into(), Phase::Link, None, Status::Blocked, format!("liaison refusée : {e}"), None),
            Ok(text) => {
                let v = parse_out(&text);
                let steps = pending_steps(&v);
                let pruned: Vec<String> = v
                    .get("pruned")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.get("name").and_then(Value::as_str)).map(str::to_string).collect())
                    .unwrap_or_default();
                for id in start_needed_ids(&v) {
                    if let Some(i) = ids.iter().position(|x| *x == Some(id)) {
                        plan.nodes[i].needs_start = true;
                    }
                }
                let detail = json!({ "counts": v.get("counts"), "servers": v.get("servers"), "try": v.get("try"), "forced_hosts": v.get("forced_hosts"), "pruned": v.get("pruned"), "steps": steps, "warnings": v.get("warnings") });
                if count(&v, "refused") > 0 {
                    plan.push("network:link".into(), Phase::Link, None, Status::Blocked, "liaison refusée par link_proxy (fichier invalide ou chemin refusé, voir detail)".to_string(), Some(detail));
                } else if count(&v, "change") > 0 {
                    if !pruned.is_empty() {
                        plan.to_confirm.push(format!("entrées retirées du proxy : {}", pruned.join(", ")));
                    }
                    plan.push("network:link".into(), Phase::Link, None, Status::Change, format!("liaison : {} écriture(s) (proxy et serveurs de jeu), secret de transfert réutilisé ou généré, jamais affiché", count(&v, "change")), Some(detail));
                } else if count(&v, "manual") > 0 {
                    let starts = steps.iter().any(|s| s["summary"].as_str().is_some_and(needs_start_hint));
                    plan.push("network:link".into(), Phase::Link, None, if starts { Status::Change } else { Status::Manual }, if starts { "liaison : à terminer après le premier démarrage (fichiers Paper générés)".to_string() } else { "liaison conforme hors étapes manuelles (voir detail)".to_string() }, Some(detail));
                } else {
                    plan.push("network:link".into(), Phase::Link, None, Status::Ok, "réseau déjà relié (entrées, repli, domaines, transfert)".to_string(), None);
                }
            }
        },
    }
    for preset in &nw.presets {
        let id = format!("network:preset.{preset}");
        match preset_args(nw, &ids, preset, true).filter(|_| usable) {
            None if usable => plan.push(id, Phase::Presets, None, Status::Create, format!("préréglage {preset} appliqué au proxy et à chaque serveur de jeu après leur création"), None),
            None => plan.push(id, Phase::Presets, None, Status::Manual, format!("préréglage {preset} simulé une fois corrigés les serveurs bloqués"), None),
            Some(args) => match net.call("network_preset", args).await {
                Err(e) => plan.push(id, Phase::Presets, None, Status::Blocked, format!("préréglage {preset} refusé : {e}"), None),
                Ok(text) => {
                    let v = parse_out(&text);
                    let steps = pending_steps(&v);
                    let to_confirm = strings(&v, "to_confirm");
                    for id in start_needed_ids(&v) {
                        if let Some(i) = ids.iter().position(|x| *x == Some(id)) {
                            plan.nodes[i].needs_start = true;
                        }
                    }
                    let detail = json!({ "counts": v.get("counts"), "steps": steps, "to_confirm": to_confirm, "warnings": v.get("warnings"), "database": v.get("database"), "bedrock_port": v.get("bedrock_port") });
                    let starts = steps.iter().any(|s| s["summary"].as_str().is_some_and(needs_start_hint));
                    if count(&v, "refused") > 0 {
                        plan.push(id, Phase::Presets, None, Status::Blocked, format!("préréglage {preset} refusé (fichier invalide, voir detail)"), Some(detail));
                    } else if count(&v, "change") > 0 || starts {
                        for c in &to_confirm {
                            plan.to_confirm.push(format!("préréglage {preset} : {c}"));
                        }
                        plan.preset_confirm.extend(to_confirm);
                        plan.push(id, Phase::Presets, None, Status::Change, format!("préréglage {preset} : {} écriture(s){}", count(&v, "change"), if starts { " ; configs générées au démarrage puis réglées" } else { "" }), Some(detail));
                    } else if count(&v, "manual") > 0 {
                        plan.push(id, Phase::Presets, None, Status::Manual, format!("préréglage {preset} conforme hors étapes manuelles (voir detail)"), Some(detail));
                    } else {
                        plan.push(id, Phase::Presets, None, Status::Ok, format!("préréglage {preset} déjà en place"), None);
                    }
                }
            },
        }
    }
}

// --- Rendu ----------------------------------------------------------------------------------------------

fn status_key(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Create => "create",
        Status::Change => "change",
        Status::Manual => "manual",
        Status::Blocked => "blocked",
    }
}

fn render_item(it: &Item) -> Value {
    let mut v = json!({ "id": it.id, "phase": it.phase.key(), "status": status_key(it.status), "summary": it.summary });
    if let Some(d) = &it.detail {
        v["detail"] = d.clone();
    }
    v
}

/// Libellé d'un élément de réseau dans `order` : `réseau` (liaison, place), ou le préréglage.
fn network_label(id: &str) -> String {
    match id.strip_prefix("network:preset.") {
        Some(p) => format!("préréglage {p}"),
        None => "réseau (proxy et serveurs de jeu)".into(),
    }
}

/// Ordre d'exécution : chaque phase et les serveurs qu'elle concerne.
pub(crate) fn order(nw: &Network, plan: &Plan) -> Vec<Value> {
    Phase::ALL
        .iter()
        .filter_map(|ph| {
            let servers: Vec<String> = plan
                .items
                .iter()
                .filter(|i| i.phase == *ph && matches!(i.status, Status::Create | Status::Change))
                .map(|i| i.node.map_or_else(|| network_label(&i.id), |n| nw.nodes[n].name.clone()))
                .fold(Vec::new(), |mut acc, s| {
                    if !acc.contains(&s) {
                        acc.push(s);
                    }
                    acc
                });
            match ph {
                Phase::Start | Phase::Verify => Some(json!({ "phase": ph.key(), "what": ph.label() })),
                _ if servers.is_empty() => None,
                _ => Some(json!({ "phase": ph.key(), "what": ph.label(), "for": servers })),
            }
        })
        .collect()
}

/// Rendu JSON du plan.
pub(crate) fn render_plan(nw: &Network, plan: &Plan, dry_run_note: bool) -> Value {
    let mut counts = Map::new();
    for s in [Status::Ok, Status::Create, Status::Change, Status::Manual, Status::Blocked] {
        let n = plan.items.iter().filter(|i| i.status == s).count();
        counts.insert(status_key(s).into(), json!(n));
    }
    let servers: Vec<Value> = nw
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let st = &plan.nodes[i];
            let mut v = json!({
                "name": n.name,
                "proxy": n.proxy,
                "server_id": st.server_id,
                "match": st.matched_by.unwrap_or("à créer"),
                "items": plan.items.iter().filter(|it| it.node == Some(i)).map(render_item).collect::<Vec<_>>(),
            });
            if let Some(r) = &n.role {
                v["role"] = json!(r);
            }
            if let Some(e) = &n.entry {
                v["entry"] = json!(e);
            }
            if let Some(cur) = &st.current_name {
                if *cur != n.name {
                    v["current_name"] = json!(cur);
                }
            }
            v
        })
        .collect();
    let blocked: Vec<&str> = plan.blocked().iter().map(|i| i.summary.as_str()).collect();
    let manual: Vec<&str> = plan.items.iter().filter(|i| i.status == Status::Manual).map(|i| i.summary.as_str()).collect();
    let status = if !blocked.is_empty() {
        "bloqué : rien ne sera fait tant que des éléments sont bloqués"
    } else if plan.todo() {
        "à appliquer"
    } else {
        "conforme : rien à faire"
    };
    let mut out = json!({
        "network": nw.name,
        "mybox_id": nw.mybox_id,
        "status": status,
        "counts": counts,
        "capacity": plan.capacity,
        "servers": servers,
        "network_items": plan.items.iter().filter(|i| i.node.is_none()).map(render_item).collect::<Vec<_>>(),
        "order": order(nw, plan),
    });
    if let Some(d) = &plan.create {
        out["create_with"] = json!({ "egg_id": d.egg_id, "experimental": d.experimental, "location_id": d.location_id, "location": d.location });
    }
    if !plan.to_confirm.is_empty() {
        out["to_confirm"] = json!(plan.to_confirm);
    }
    if !plan.name_confirm.is_empty() {
        out["confirm_names"] = json!({
            "servers": plan.name_confirm.iter().map(|(_, n)| n).collect::<Vec<_>>(),
            "why": "change_version est DESTRUCTIF : l'utilisateur retape le nom exact de chacun de ces serveurs (élicitation de son client), sinon `confirm_names` d'apply_network = ces noms exacts, donnés par lui — jamais devinés",
        });
    }
    if !blocked.is_empty() {
        out["blocked"] = json!(blocked);
    }
    if !manual.is_empty() {
        out["manual"] = json!(manual);
    }
    if !plan.warnings.is_empty() {
        out["warnings"] = json!(plan.warnings);
    }
    out["next"] = json!(if !blocked.is_empty() {
        "Corrige les éléments `blocked` (blueprint, place libre de la MyBox, serveurs en pause…), puis relance plan_network."
    } else if plan.todo() {
        if dry_run_note {
            "Montre ce plan à l'utilisateur (`to_confirm`, `order`) ; après son accord, apply_network avec le même blueprint et dry_run=false (confirmation demandée avant tout effet)."
        } else {
            "apply_network avec le même blueprint et dry_run=false applique ce plan (confirmation de l'utilisateur avant tout effet)."
        }
    } else {
        "Rien à faire : le réseau est conforme au blueprint (étapes manual éventuelles à traiter à la main) ; vérification : network_topology avec ping=true."
    });
    out
}

/// Outil MCP `plan_network`.
pub(crate) async fn plan_network_tool(core: &Core, args: &Value) -> Result<String, String> {
    let nw = Network::parse(args.get("blueprint").ok_or("paramètre `blueprint` (objet JSON) requis")?)?;
    let net = CoreNet { core, cfg: core.get_mcp_config() };
    let plan = plan_with(&net, &nw).await?;
    Ok(crate::config_edit::render_for_ai(core, crate::redact::mask_known_secrets_value(render_plan(&nw, &plan, true))))
}
