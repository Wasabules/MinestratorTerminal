//! `minestrator-core` — logique métier MineStrator, **indépendante de toute UI**.
//!
//! Réutilisable par plusieurs frontends (desktop Tauri, futur daemon Linux, CLI/TUI) :
//! chacun instancie un [`Core`], appelle ses méthodes, et s'abonne à ses [`CoreEvent`]
//! via [`Core::subscribe`].
//!
//! Le token API est géré en interne (trousseau OS) ; les appelants n'ont jamais à le manipuler.

mod api;
mod archive;
mod cache;
mod cli;
mod cli_agent;
mod cli_session;
mod config;
mod console;
mod copilot;
mod doctor;
mod error;
mod events;
mod llm;
mod mca;
pub mod mcp;
mod models;
mod official_mcp;
mod perf;
mod persist;
mod redact;
mod secrets;
mod sftp;
mod store;
mod supervisor;
mod util;
mod world;

pub use archive::ArchiveEntry;
pub use cli_agent::{detect_clis, CliAgent, CliStatus};
pub use copilot::{Autonomy, ChatReply, CopilotConfig, Effort};
pub use error::{Error, Result};
pub use events::{
    Alert, ChatDelta, ConsoleConnection, ConsoleOutput, ConsoleStats, ConsoleStatus,
    CopilotProgress, CopilotStarted, CoreEvent, Diagnosis, ProposedAction, SftpProgress,
};
pub use llm::Provider;
pub use mcp::McpConfig;
pub use redact::PrivacyConfig;
pub use models::{
    Backup, CpuLimits, InstalledItem, LimitMb, LiveLight, MarketItem, MarketPage, MarketVersion,
    MyBoxSummary, Players, ServerDetails, ServerListItem, ServersOverview, SftpEntry, Snapshot,
    Startup, UserProfile,
};
pub use store::MetricSample;
pub use supervisor::{Supervisor, SupervisorConfig, SupervisorState};

use api::ApiClient;
use console::ConsoleManager;
use copilot::ChatSession;
use persist::PersistedConfig;
use sftp::SftpManager;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use store::{now_secs, MetricsStore};
use tokio::sync::broadcast;

/// Façade métier : agrège client API, console temps réel, SFTP, historique et events.
pub struct Core {
    api: ApiClient,
    console: Arc<ConsoleManager>,
    sftp: SftpManager,
    store: Arc<MetricsStore>,
    sup_state: Arc<SupervisorState>,
    mcp: PersistedConfig<McpConfig>,
    copilot: PersistedConfig<CopilotConfig>,
    privacy: PersistedConfig<PrivacyConfig>,
    /// Une conversation = une cellule verrouillable (Mutex async). Le Mutex sync externe ne protège
    /// QUE l'accès à la map ; le tour, lui, tient le Mutex async de la cellule → warm et send (et
    /// deux envois concurrents) se sérialisent sur la MÊME session au lieu de s'écraser.
    chat_sessions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<ChatSession>>>>,
    /// Cache TTL des lectures d'API stables/semi-stables (voir [`cache`]). Purgé après toute écriture.
    cache: cache::TtlCache,
    /// Token API mémorisé → évite une lecture BLOQUANTE du trousseau OS (DPAPI/Keychain/D-Bus) à
    /// CHAQUE appel async. Invalidé au (re)login / logout.
    token_cache: Mutex<Option<String>>,
    /// Id utilisateur mémorisé (stable par token) → évite un `GET /user` de résolution à chaque
    /// `list_servers` / opération snapshot (2 RTT → 1).
    user_id_cache: Mutex<Option<i64>>,
    events: broadcast::Sender<CoreEvent>,
}

impl Core {
    pub fn new() -> Self {
        let (events, _rx) = broadcast::channel(1024);
        let api = ApiClient::new();
        let console = Arc::new(ConsoleManager::new(events.clone()));

        let dir = directories::ProjectDirs::from("com", "geoffreylecoq", "MinestratorTerminal")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| std::env::temp_dir().join("minestrator"));
        let _ = std::fs::create_dir_all(&dir);
        let store = Arc::new(MetricsStore::open(&dir));

        Self {
            api,
            console,
            sftp: SftpManager::default(),
            store,
            sup_state: Arc::new(SupervisorState::load(&dir)),
            mcp: PersistedConfig::load(&dir, "mcp.json"),
            copilot: PersistedConfig::load(&dir, "copilot.json"),
            privacy: PersistedConfig::load(&dir, "privacy.json"),
            chat_sessions: Mutex::new(HashMap::new()),
            cache: cache::TtlCache::default(),
            token_cache: Mutex::new(None),
            user_id_cache: Mutex::new(None),
            events,
        }
    }

    /// Accès (interne) au cache TTL partagé, utilisé par le dispatch MCP.
    pub(crate) fn cache(&self) -> &cache::TtlCache {
        &self.cache
    }

    /// Construit un superviseur autonome (à démarrer dans un runtime tokio).
    pub fn supervisor(&self) -> Supervisor {
        Supervisor::new(
            self.api.clone(),
            self.console.clone(),
            self.store.clone(),
            self.events.clone(),
            self.sup_state.clone(),
        )
    }

    /// Historique de métriques d'un serveur, depuis `since_secs` secondes.
    pub fn metrics(&self, server_id: i64, since_secs: i64) -> Result<Vec<MetricSample>> {
        self.store.query(server_id, now_secs() - since_secs)
    }

    /// Réglages courants du superviseur.
    pub fn get_supervisor_config(&self) -> SupervisorConfig {
        self.sup_state.config()
    }

    /// Met à jour (et persiste) les réglages du superviseur ; pris en compte à chaud.
    pub fn set_supervisor_config(&self, cfg: SupervisorConfig) {
        self.sup_state.set_config(cfg);
    }

    /// Réglages du serveur MCP.
    pub fn get_mcp_config(&self) -> McpConfig {
        self.mcp.get()
    }

    /// Met à jour (et persiste) les réglages MCP.
    pub fn set_mcp_config(&self, cfg: McpConfig) {
        self.mcp.set(cfg);
    }

    /// Réglages de confidentialité (anonymisation).
    pub fn get_privacy_config(&self) -> PrivacyConfig {
        self.privacy.get()
    }

    /// Met à jour (et persiste) les réglages de confidentialité ; pris en compte à chaud.
    pub fn set_privacy_config(&self, cfg: PrivacyConfig) {
        self.privacy.set(cfg);
    }

    /// Anonymise un texte destiné à un agent IA **si** l'anonymisation IA est activée.
    pub(crate) fn redact_ai(&self, text: &str) -> String {
        if self.privacy.get().redact_ai {
            redact::redact(text)
        } else {
            text.to_string()
        }
    }

    // --- Copilote (agent LLM multi-fournisseur) ----------------------------

    /// Réglages du Copilote.
    pub fn get_copilot_config(&self) -> CopilotConfig {
        self.copilot.get()
    }

    /// Lecture EMPRUNTANTE de la config Copilote (pas de `clone()` de toute la struct) — pour les
    /// boucles chaudes qui n'extraient que quelques champs. La closure ne doit pas `.await`.
    pub(crate) fn copilot_config_with<R>(&self, f: impl FnOnce(&CopilotConfig) -> R) -> R {
        self.copilot.with(f)
    }

    /// Met à jour (et persiste) les réglages du Copilote ; pris en compte à chaud.
    pub fn set_copilot_config(&self, cfg: CopilotConfig) {
        self.copilot.set(cfg);
    }

    /// Une clé API LLM est-elle enregistrée pour le fournisseur actuellement sélectionné ?
    pub fn has_copilot_key(&self) -> Result<bool> {
        let slug = self.copilot.get().provider.slug();
        Ok(secrets::read_llm_key(slug)?.is_some())
    }

    /// Enregistre la clé API LLM du fournisseur actuellement sélectionné.
    pub fn set_copilot_key(&self, key: &str) -> Result<()> {
        let slug = self.copilot.get().provider.slug();
        secrets::store_llm_key(slug, key.trim())
    }

    /// Supprime la clé API LLM du fournisseur actuellement sélectionné.
    pub fn clear_copilot_key(&self) -> Result<()> {
        let slug = self.copilot.get().provider.slug();
        secrets::delete_llm_key(slug)
    }

    /// Exécute une action proposée par le Copilote (via la couche d'outils MCP). Le `server_id`
    /// vient du contexte du diagnostic (jamais de la supposition du modèle) et les synonymes
    /// d'arguments sont normalisés avant exécution.
    pub async fn copilot_apply(
        &self,
        server_id: i64,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<String> {
        let args = copilot::prepare_action_args(server_id, tool, args);
        mcp::dispatch(self, tool, args)
            .await
            .map_err(Error::Unexpected)
    }

    /// Démarre l'écoute des alertes par le Copilote (à appeler dans un runtime tokio).
    pub fn spawn_copilot(self: Arc<Self>) {
        copilot::spawn(self);
    }

    /// Déclenche manuellement un diagnostic (bouton « Diagnostiquer » / test).
    pub fn diagnose_now(self: Arc<Self>, server_id: i64, server_name: String) {
        tokio::spawn(async move {
            copilot::run(
                &self,
                copilot::Incident {
                    server_id,
                    server_name,
                    trigger: "manual".into(),
                    severity: "warning".into(),
                    message: "Diagnostic manuel demandé.".into(),
                    selection: None,
                },
                true,
            )
            .await;
        });
    }

    /// Analyse de performance (Spark) : collecte health/tps/gc + profiler, puis le Copilote
    /// analyse et propose. Déclenchée manuellement (bouton) ou sur surcharge (auto).
    pub fn analyze_performance(self: Arc<Self>, server_id: i64, server_name: String) {
        tokio::spawn(async move {
            copilot::run_performance(&self, server_id, server_name, true).await;
        });
    }

    /// Cellule verrouillable d'une conversation (créée à la volée). Le Mutex sync n'est tenu que le
    /// temps du get-or-create ; l'appelant verrouille ensuite le Mutex async pour toute la durée du tour.
    fn chat_session_cell(&self, session_id: &str) -> Arc<tokio::sync::Mutex<ChatSession>> {
        self.chat_sessions
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(ChatSession::new())))
            .clone()
    }

    /// Envoie un message dans une conversation assistant (onglet Assistant). Multi-tours par
    /// `session_id`. `autonomous` : l'agent peut exécuter les actions ; sinon il les propose.
    pub async fn chat_send(
        &self,
        session_id: String,
        server_id: i64,
        server_name: String,
        message: String,
        autonomous: bool,
    ) -> ChatReply {
        // Verrou (POSSÉDÉ) tenu tout le tour → sérialise contre le pré-chauffage et les envois
        // concurrents ; les mutations (cli_session, process) persistent via l'Arc partagé, sans
        // remove/insert. `lock_owned` = la garde détient l'Arc (pas d'emprunt d'un local).
        let mut session = self.chat_session_cell(&session_id).lock_owned().await;
        copilot::chat_turn(
            self,
            &mut session,
            server_id,
            &server_name,
            &message,
            autonomous,
            &session_id,
        )
        .await
    }

    /// Réinitialise une conversation assistant (retire la cellule → son process persistant est tué
    /// au drop ; un tour en cours la garde vivante jusqu'à sa fin grâce à l'Arc).
    pub fn chat_reset(&self, session_id: &str) {
        self.chat_sessions.lock().unwrap().remove(session_id);
    }

    /// F — pré-chauffe le process agent persistant d'une session **avant** le 1er message
    /// (best-effort). Masque le bootstrap Node + MCP derrière le temps d'ouverture de l'onglet.
    pub async fn chat_warm(&self, session_id: String, autonomous: bool) {
        let cfg = self.get_copilot_config();
        if cfg.provider != Provider::LocalCli || cfg.cli_agent != CliAgent::ClaudeCode {
            return; // pré-chauffe pertinente uniquement pour le process persistant Claude Code
        }
        // best-effort : si un tour est déjà en cours (cellule verrouillée), on NE pré-chauffe pas —
        // surtout, on n'écrase pas la session du tour (c'était la cause de la race warm/send).
        if let Ok(mut session) = self.chat_session_cell(&session_id).try_lock_owned() {
            copilot::chat_warm(&cfg, &mut session, autonomous).await;
        }
    }

    /// Analyse un extrait de texte sélectionné par l'utilisateur (clic droit → Copilote).
    pub fn copilot_analyze(self: Arc<Self>, server_id: i64, server_name: String, text: String) {
        // La sélection part vers un LLM → anonymise si activé.
        let selection = self.redact_ai(&text);
        tokio::spawn(async move {
            copilot::run(
                &self,
                copilot::Incident {
                    server_id,
                    server_name,
                    trigger: "selection".into(),
                    severity: "warning".into(),
                    message: "Analyse de sélection.".into(),
                    selection: Some(selection),
                },
                true,
            )
            .await;
        });
    }

    /// Nom connu d'un serveur (via le superviseur), sinon `#id`.
    pub(crate) fn server_name(&self, id: i64) -> String {
        self.sup_state.name_of(id)
    }

    /// Limites `(cpu_limit, disk_mb)` connues d'un serveur (via le superviseur).
    pub(crate) fn server_limits(&self, id: i64) -> Option<(i64, i64)> {
        self.sup_state.limits_of(id)
    }

    /// Émet un event métier (usage interne au cœur).
    pub(crate) fn emit(&self, ev: CoreEvent) {
        let _ = self.events.send(ev);
    }

    /// S'abonner au flux d'events métier (console, superviseur, copilote…).
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    fn token(&self) -> Result<String> {
        if let Some(t) = self.token_cache.lock().unwrap().clone() {
            return Ok(t);
        }
        let t = secrets::read_key()?.ok_or(Error::NoKey)?;
        *self.token_cache.lock().unwrap() = Some(t.clone());
        Ok(t)
    }

    /// Id utilisateur courant, mémorisé (stable par token). Évite le `GET /user` de résolution que
    /// répétaient `list_servers` et les opérations snapshot.
    async fn user_id(&self) -> Result<i64> {
        if let Some(id) = *self.user_id_cache.lock().unwrap() {
            return Ok(id);
        }
        let id = self.api.get_user(&self.token()?).await?.id;
        *self.user_id_cache.lock().unwrap() = Some(id);
        Ok(id)
    }

    // --- Authentification --------------------------------------------------

    pub async fn authenticate_and_store(&self, key: &str) -> Result<UserProfile> {
        let (profile, token) = self.api.authenticate(key).await?;
        secrets::store_key(&token)?;
        *self.token_cache.lock().unwrap() = Some(token);
        *self.user_id_cache.lock().unwrap() = Some(profile.id);
        tracing::info!(user = %profile.pseudo, "clé API validée et enregistrée");
        Ok(profile)
    }

    pub fn has_key(&self) -> Result<bool> {
        if self.token_cache.lock().unwrap().is_some() {
            return Ok(true);
        }
        Ok(secrets::read_key()?.is_some())
    }

    pub fn logout(&self) -> Result<()> {
        *self.token_cache.lock().unwrap() = None;
        *self.user_id_cache.lock().unwrap() = None;
        secrets::delete_key()
    }

    pub async fn get_user(&self) -> Result<UserProfile> {
        self.api.get_user(&self.token()?).await
    }

    // --- Serveurs ----------------------------------------------------------

    pub async fn list_servers(&self) -> Result<ServersOverview> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        self.api.list_servers(&token, user_id).await
    }

    pub async fn server_details(&self, id: i64) -> Result<ServerDetails> {
        self.api.get_server(&self.token()?, id).await
    }

    pub async fn live_light(&self, id: i64) -> Result<LiveLight> {
        self.api.get_live_light(&self.token()?, id).await
    }

    pub async fn console_logs(&self, id: i64) -> Result<Vec<String>> {
        self.api.get_console_logs(&self.token()?, id).await
    }

    pub async fn power_action(&self, id: i64, action: &str) -> Result<()> {
        // Marque un arrêt/redémarrage volontaire pour ne pas le confondre avec un crash.
        if matches!(action, "stop" | "stop10" | "kill" | "restart" | "restart10") {
            self.sup_state.mark_expected_stop(id);
        }
        self.api.power_action(&self.token()?, id, action).await
    }

    pub async fn send_command(&self, id: i64, command: &str) -> Result<()> {
        self.api.send_command(&self.token()?, id, command).await
    }

    pub async fn player_action(&self, id: i64, action: &str, player: &str) -> Result<()> {
        self.api.player_action(&self.token()?, id, action, player).await
    }

    /// Configuration de démarrage (commande Java + contexte).
    pub async fn get_startup(&self, id: i64) -> Result<Startup> {
        self.api.get_startup(&self.token()?, id).await
    }

    /// Modifie la commande de démarrage (flags JVM). Effet au prochain démarrage.
    pub async fn set_startup_params(&self, id: i64, parameters: &str) -> Result<()> {
        self.api.set_startup_params(&self.token()?, id, parameters).await
    }

    // --- Marketplace (mods & plugins) --------------------------------------

    /// Versions Minecraft connues du catalogue.
    pub async fn market_minecraft_versions(&self) -> Result<Vec<String>> {
        self.api.market_minecraft_versions(&self.token()?).await
    }

    /// Catalogue paginé de mods/plugins (voir `ApiClient::market_list`).
    pub async fn market_list(
        &self,
        kind: &str,
        source: &str,
        page: i64,
        query: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<MarketPage> {
        self.api
            .market_list(&self.token()?, kind, source, page, query, loader, game_version)
            .await
    }

    /// Versions disponibles d'un projet.
    pub async fn market_versions(
        &self,
        source: &str,
        slug_or_id: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<Vec<MarketVersion>> {
        self.api
            .market_versions(&self.token()?, source, slug_or_id, loader, game_version)
            .await
    }

    /// Installe un projet sur un serveur. Seule la source `modrinth` est vérifiée pour
    /// l'installation ; les autres renvoient une erreur explicite (corps à confirmer).
    pub async fn install_mod(
        &self,
        server_id: i64,
        source: &str,
        kind: &str,
        slug: &str,
        version_id: &str,
        loader: &str,
    ) -> Result<()> {
        match source {
            "modrinth" => {
                // `kind`/`loader` non requis par l'API Modrinth (placement déterminé côté serveur).
                let _ = (kind, loader);
                self.api
                    .install_modrinth(&self.token()?, server_id, slug, version_id)
                    .await
            }
            "spigot" => {
                // SpigotMC : identifiants numériques. `slug` porte l'id du plugin.
                let plugin_id = slug
                    .parse::<i64>()
                    .map_err(|_| Error::Unexpected("id plugin SpigotMC invalide".into()))?;
                let vid = version_id
                    .parse::<i64>()
                    .map_err(|_| Error::Unexpected("id version SpigotMC invalide".into()))?;
                self.api.install_spigot(&self.token()?, server_id, plugin_id, vid).await
            }
            other => Err(Error::Unexpected(format!(
                "Installation non encore prise en charge pour la source « {other} » (corps de requête à confirmer)."
            ))),
        }
    }

    /// Mods installés sur un serveur.
    pub async fn installed_mods(&self, id: i64) -> Result<Vec<InstalledItem>> {
        self.api.installed_mods(&self.token()?, id).await
    }

    /// Plugins installés sur un serveur.
    pub async fn installed_plugins(&self, id: i64) -> Result<Vec<InstalledItem>> {
        self.api.installed_plugins(&self.token()?, id).await
    }

    // --- Filet de sécurité : backups & snapshots ---------------------------

    /// Backups quotidiens automatiques d'un serveur (récents d'abord).
    pub async fn list_backups(&self, id: i64) -> Result<Vec<Backup>> {
        self.api.list_backups(&self.token()?, id).await
    }

    /// Snapshots (points de sauvegarde à la demande) de l'utilisateur courant.
    pub async fn list_snapshots(&self) -> Result<Vec<Snapshot>> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        self.api.list_snapshots(&token, user_id).await
    }

    /// Crée un snapshot du serveur (filet AVANT une intervention). Renvoie l'`job_id` asynchrone.
    pub async fn create_snapshot(&self, server_id: i64, name: &str) -> Result<i64> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        self.api.create_snapshot(&token, user_id, server_id, name).await
    }

    /// **Restaure** un snapshot sur un serveur (DESTRUCTIF). Renvoie l'`job_id` asynchrone.
    pub async fn restore_snapshot(&self, snapshot_id: i64, server_id: i64) -> Result<i64> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        self.api.restore_snapshot(&token, user_id, snapshot_id, server_id).await
    }

    /// Supprime définitivement un snapshot. Renvoie l'`job_id` asynchrone.
    pub async fn delete_snapshot(&self, snapshot_id: i64) -> Result<i64> {
        let token = self.token()?;
        let user_id = self.user_id().await?;
        self.api.delete_snapshot(&token, user_id, snapshot_id).await
    }

    /// **Restaure** un backup quotidien sur un serveur (DESTRUCTIF).
    pub async fn restore_backup(&self, server_id: i64, backup_id: i64) -> Result<()> {
        self.api.restore_backup(&self.token()?, server_id, backup_id).await
    }

    // --- Console WebSocket -------------------------------------------------

    pub fn console_connect(&self, conn_id: String, server_id: i64) -> Result<()> {
        let token = self.token()?;
        self.console
            .connect(self.api.clone(), token, conn_id, server_id, false);
        Ok(())
    }

    pub fn console_disconnect(&self, conn_id: &str) {
        self.console.disconnect(conn_id);
    }

    /// Ouvre une connexion monitor éphémère et renvoie le premier échantillon de stats
    /// (CPU/RAM/disque instantanés), ou `None` si le serveur est hors ligne/hiberné.
    /// Utile aux frontends « one-shot » (MCP) même sans superviseur actif.
    pub async fn sample_stats(&self, server_id: i64) -> Result<Option<ConsoleStats>> {
        let token = self.token()?;
        let conn_id = format!("sample:{server_id}:{}", now_secs());
        let mut rx = self.events.subscribe();
        self.console
            .connect(self.api.clone(), token, conn_id.clone(), server_id, true);

        let result = tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                match rx.recv().await {
                    Ok(CoreEvent::ConsoleStats(s)) if s.conn_id == conn_id => return Some(s),
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();

        self.console.disconnect(&conn_id);
        Ok(result)
    }

    // --- SFTP --------------------------------------------------------------

    /// Évince la session SFTP en cache si l'opération a échoué. Une session poolée peut mourir
    /// (idle timeout, redémarrage serveur) et resterait sinon empoisonnée pour TOUS les appels
    /// fichier suivants ; au prochain appel, `ensure` en rouvre une neuve.
    fn drop_on_err<T>(&self, id: i64, r: Result<T>) -> Result<T> {
        if r.is_err() {
            self.sftp.drop_session(id);
        }
        r
    }

    pub async fn sftp_list(&self, id: i64, path: &str) -> Result<Vec<SftpEntry>> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::list(&conn, path).await)
    }

    pub async fn sftp_read_text(&self, id: i64, path: &str) -> Result<String> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::read_text(&conn, path).await)
    }

    pub async fn sftp_write_text(&self, id: i64, path: &str, content: &str) -> Result<()> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::write_text(&conn, path, content).await)
    }

    pub async fn sftp_mkdir(&self, id: i64, path: &str) -> Result<()> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::mkdir(&conn, path).await)
    }

    pub async fn sftp_delete(&self, id: i64, path: &str, is_dir: bool) -> Result<()> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::remove(&conn, path, is_dir).await)
    }

    pub async fn sftp_rename(&self, id: i64, from: &str, to: &str) -> Result<()> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(id, sftp::rename(&conn, from, to).await)
    }

    // --- Transferts (upload / download / zip) : suivis via `CoreEvent::SftpProgress` --------------

    /// Téléverse un fichier local (transfert suivi). `tid` = id de transfert fourni par le front pour
    /// corréler les events de progression. À lancer en tâche de fond (fire-and-forget).
    pub async fn run_sftp_upload(&self, id: i64, local: &str, remote_dir: &str, tid: &str) {
        let name = base_name(local);
        let tx = self.events.clone();
        let (tid_s, name_s) = (tid.to_string(), name.clone());
        let mut last = 0u64;
        let mut on = move |done: u64, total: u64| {
            if done == total || done - last >= PROGRESS_STEP {
                last = done;
                let _ = tx.send(CoreEvent::SftpProgress(SftpProgress {
                    id: tid_s.clone(),
                    name: name_s.clone(),
                    direction: "up".into(),
                    done,
                    total,
                    status: "active".into(),
                    error: None,
                }));
            }
        };
        let res = async {
            let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
            let r = sftp::upload(&conn, local, remote_dir, &mut on).await;
            self.drop_on_err(id, r)
        }
        .await;
        self.emit_transfer_end(tid, &name, "up", res.err());
    }

    /// Télécharge un fichier distant vers `local` (transfert suivi).
    pub async fn run_sftp_download(&self, id: i64, remote: &str, local: &str, tid: &str) {
        let name = base_name(remote);
        let tx = self.events.clone();
        let (tid_s, name_s) = (tid.to_string(), name.clone());
        let mut last = 0u64;
        let mut on = move |done: u64, total: u64| {
            if done == total || done - last >= PROGRESS_STEP {
                last = done;
                let _ = tx.send(CoreEvent::SftpProgress(SftpProgress {
                    id: tid_s.clone(),
                    name: name_s.clone(),
                    direction: "down".into(),
                    done,
                    total,
                    status: "active".into(),
                    error: None,
                }));
            }
        };
        let res = async {
            let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
            let r = sftp::download(&conn, remote, local, &mut on).await;
            self.drop_on_err(id, r)
        }
        .await;
        self.emit_transfer_end(tid, &name, "down", res.err());
    }

    /// Télécharge une sélection (fichiers et/ou dossiers, frères sous un même parent) en UN `.zip`
    /// local, construit côté client (SFTP récursif). Transfert suivi (progression par octets).
    pub async fn run_sftp_download_zip(&self, id: i64, paths: Vec<String>, local_zip: &str, tid: &str) {
        let name = base_name(local_zip);
        let _ = self.events.send(CoreEvent::SftpProgress(SftpProgress {
            id: tid.to_string(),
            name: name.clone(),
            direction: "down".into(),
            done: 0,
            total: 0,
            status: "active".into(),
            error: None,
        }));
        let res = self.build_zip(id, &paths, local_zip, tid, &name).await;
        self.emit_transfer_end(tid, &name, "down", res.err());
    }

    async fn build_zip(
        &self,
        id: i64,
        paths: &[String],
        local_zip: &str,
        tid: &str,
        name: &str,
    ) -> Result<()> {
        use std::io::Write;
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let parent = paths.first().map(|p| parent_dir(p)).unwrap_or_else(|| "/".to_string());
        // Collecte des fichiers : (chemin distant complet, nom dans le zip, taille).
        let mut items: Vec<(String, String, u64)> = Vec::new();
        for p in paths {
            let (is_dir, size) = self.drop_on_err(id, sftp::stat(&conn, p).await)?;
            if is_dir {
                for (full, sz) in self.drop_on_err(id, sftp::walk(&conn, p).await)? {
                    let rel = rel_to(&full, &parent);
                    items.push((full, rel, sz));
                }
            } else {
                items.push((p.clone(), base_name(p), size));
            }
        }
        let total: u64 = items.iter().map(|(_, _, s)| s).sum();
        let file = std::fs::File::create(local_zip)
            .map_err(|e| Error::Unexpected(format!("création du zip: {e}")))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let tx = self.events.clone();
        let mut done = 0u64;
        for (full, rel, size) in items {
            zip.start_file(&rel, opts).map_err(|e| Error::Unexpected(format!("zip: {e}")))?;
            let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, &full, u64::MAX).await)?;
            zip.write_all(&bytes).map_err(|e| Error::Unexpected(format!("zip: {e}")))?;
            done += size;
            let _ = tx.send(CoreEvent::SftpProgress(SftpProgress {
                id: tid.to_string(),
                name: name.to_string(),
                direction: "down".into(),
                done,
                total,
                status: "active".into(),
                error: None,
            }));
        }
        zip.finish().map_err(|e| Error::Unexpected(format!("zip: {e}")))?;
        Ok(())
    }

    fn emit_transfer_end(&self, tid: &str, name: &str, dir: &str, err: Option<Error>) {
        let (status, error) = match err {
            Some(e) => ("error", Some(e.to_string())),
            None => ("done", None),
        };
        let _ = self.events.send(CoreEvent::SftpProgress(SftpProgress {
            id: tid.to_string(),
            name: name.to_string(),
            direction: dir.to_string(),
            done: 0,
            total: 0,
            status: status.to_string(),
            error,
        }));
    }

    // --- Archives (lecture seule) : .zip / .tar / .tar.gz / .gz -----------------------------------

    /// Liste les entrées d'une archive distante (.zip/.tar/.tar.gz) sans l'extraire sur disque.
    pub async fn sftp_archive_list(&self, id: i64, path: &str) -> Result<Vec<archive::ArchiveEntry>> {
        let kind = archive::kind_from_name(path)
            .ok_or_else(|| Error::Unexpected("Format d'archive non reconnu.".into()))?;
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, path, ARCHIVE_CAP).await)?;
        archive::list(&bytes, kind).map_err(Error::Unexpected)
    }

    /// Contenu TEXTE d'une entrée d'archive (pour l'affichage lecture seule).
    pub async fn sftp_archive_read_text(&self, id: i64, path: &str, entry: &str) -> Result<String> {
        let kind = archive::kind_from_name(path)
            .ok_or_else(|| Error::Unexpected("Format d'archive non reconnu.".into()))?;
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, path, ARCHIVE_CAP).await)?;
        let raw = archive::extract(&bytes, kind, entry).map_err(Error::Unexpected)?;
        String::from_utf8(raw)
            .map_err(|_| Error::Unexpected("Entrée binaire (non affichable).".into()))
    }

    /// Contenu TEXTE d'un `.gz` seul (ex. `latest.log.gz`), décompressé.
    pub async fn sftp_gz_text(&self, id: i64, path: &str) -> Result<String> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, path, ARCHIVE_CAP).await)?;
        let raw = archive::gunzip(&bytes).map_err(Error::Unexpected)?;
        String::from_utf8(raw)
            .map_err(|_| Error::Unexpected("Contenu binaire (non affichable).".into()))
    }

    /// Extrait UNE entrée d'archive vers un fichier local (téléchargement d'une entrée).
    pub async fn sftp_extract_entry(&self, id: i64, path: &str, entry: &str, local: &str) -> Result<()> {
        let kind = archive::kind_from_name(path)
            .ok_or_else(|| Error::Unexpected("Format d'archive non reconnu.".into()))?;
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, path, ARCHIVE_CAP).await)?;
        let raw = archive::extract(&bytes, kind, entry).map_err(Error::Unexpected)?;
        tokio::fs::write(local, raw)
            .await
            .map_err(|e| Error::Unexpected(format!("écriture locale: {e}")))
    }

    /// Recherche RÉCURSIVE (bornée) de fichiers/dossiers par sous-chaîne de nom sous `root`.
    /// Renvoie `(résultats, tronqué)`.
    pub async fn sftp_search(&self, id: i64, root: &str, query: &str) -> Result<(Vec<SftpEntry>, bool)> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        self.drop_on_err(
            id,
            sftp::search(&conn, root, query, SEARCH_MAX_RESULTS, SEARCH_MAX_SCAN).await,
        )
    }

    /// Extrait une archive distante (`.zip`/`.tar`/`.tar.gz`) DANS `dest_dir` sur le serveur (via
    /// SFTP). Anti zip-slip + plafonds (voir `archive::extract_all`). Renvoie le nombre de fichiers.
    pub async fn sftp_extract_archive(&self, id: i64, archive_path: &str, dest_dir: &str) -> Result<usize> {
        let kind = archive::kind_from_name(archive_path)
            .ok_or_else(|| Error::Unexpected("Format d'archive non reconnu.".into()))?;
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let bytes = self.drop_on_err(id, sftp::read_bytes(&conn, archive_path, ARCHIVE_CAP).await)?;
        let files = archive::extract_all(&bytes, kind, EXTRACT_MAX_ENTRIES, EXTRACT_MAX_TOTAL)
            .map_err(Error::Unexpected)?;
        let mut ensured: std::collections::HashSet<String> = std::collections::HashSet::new();
        let count = files.len();
        for (name, data) in files {
            let target = path_join(dest_dir, &name);
            let dir = parent_dir(&target);
            if ensured.insert(dir.clone()) {
                sftp::ensure_dir(&conn, &dir).await;
            }
            self.drop_on_err(id, sftp::write_bytes(&conn, &target, &data).await)?;
        }
        Ok(count)
    }

    /// Transfert simple AWAITÉ (sans progression) — usage INTERNE (world.rs : réparation `.mca`).
    pub(crate) async fn sftp_download_file(&self, id: i64, remote: &str, local: &str) -> Result<()> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let mut noop = |_: u64, _: u64| {};
        self.drop_on_err(id, sftp::download(&conn, remote, local, &mut noop).await)
    }

    pub(crate) async fn sftp_upload_file(&self, id: i64, local: &str, remote_dir: &str) -> Result<String> {
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        let mut noop = |_: u64, _: u64| {};
        self.drop_on_err(id, sftp::upload(&conn, local, remote_dir, &mut noop).await)
    }

    pub fn sftp_disconnect(&self, id: i64) {
        self.sftp.drop_session(id);
    }
}

/// Plafond de lecture d'une archive en mémoire pour l'ouverture virtuelle (64 Mio).
const ARCHIVE_CAP: u64 = 64 * 1024 * 1024;
/// Pas minimal (octets) entre deux events de progression d'un transfert (anti-spam du broadcast).
const PROGRESS_STEP: u64 = 512 * 1024;
/// Plafonds de `search_files` : résultats renvoyés et entrées explorées.
const SEARCH_MAX_RESULTS: usize = 200;
const SEARCH_MAX_SCAN: usize = 20_000;
/// Plafonds d'extraction d'archive (nb de fichiers, taille décompressée totale) — anti zip-bomb.
const EXTRACT_MAX_ENTRIES: usize = 4000;
const EXTRACT_MAX_TOTAL: u64 = 256 * 1024 * 1024;

/// Dernier segment d'un chemin distant (nom de fichier/dossier).
fn base_name(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p).to_string()
}

/// Dossier parent d'un chemin distant (`/` à la racine).
fn parent_dir(p: &str) -> String {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        None | Some(0) => "/".to_string(),
        Some(i) => t[..i].to_string(),
    }
}

/// Chemin `full` rendu relatif à `parent` (nom d'entrée dans le zip).
fn rel_to(full: &str, parent: &str) -> String {
    let parent = parent.trim_end_matches('/');
    full.strip_prefix(parent).unwrap_or(full).trim_start_matches('/').to_string()
}

/// Joint un dossier distant et un chemin relatif (nom d'entrée d'archive à extraire).
fn path_join(dir: &str, rel: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), rel.trim_start_matches('/'))
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}
