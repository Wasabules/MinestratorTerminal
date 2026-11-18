//! Client HTTP typé vers l'API MineStrator (pur : le token est passé à chaque appel).
//!
//! Auth : `Authorization: Bearer <token>`. Le panel affiche la clé DÉJÀ encodée en base64 ;
//! `authenticate` essaie la valeur telle quelle puis, en secours, sa version base64.

use crate::config::{API_BASE_URL, USER_AGENT};
use crate::error::{Error, Result};
use crate::models::{
    ActivityData, ActivityEntry, ActivityPage, Backup, BackupsData, ConsoleLogs, Egg, EggsData, Envelope, InstalledItem, InstalledRaw,
    LiveBulkData, LiveLight, LivePlayers, MarketListRaw, MarketPage, MarketVersion,
    MarketVersionsRaw, McVersionsData, MyBoxDetails, ServerDetails, ServerFullData, ServerLive,
    ServerProperties, ServersData, ServersOverview,
    SftpCreds, SftpData, Snapshot, SnapshotsData, Startup, StartupData, UserData,
    UserNotification, UserProfile,
    WsBulkData, WsCredentials, WsCredentialsData,
};
use crate::history::HostStatsData;
use crate::panel_schedules::{PanelSchedule, PanelScheduleInput, PanelTaskInput, SchedulesData};
use crate::updates::{ContentKind, ContentList, McVersion};
use crate::server_files::{self, CompressJob, DiskSpace, DiskUsage};
use base64::Engine;
use reqwest::{header, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    ws_bulk: std::sync::Arc<WsBulkCache>,
}

/// Durée pendant laquelle les jetons WebSocket obtenus en groupe sont réutilisés : assez pour
/// servir toutes les connexions qui expirent ensemble, bien moins que la vie d'un jeton.
const WS_BULK_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Jetons WebSocket de tous les serveurs, obtenus en UN appel et partagés quelques secondes.
///
/// Le superviseur tient une connexion par serveur ; leurs jetons expirent ensemble, et chacune
/// redemandait le sien (`GET /server/{id}/websocket`) : N requêtes à chaque expiration. Le verrou
/// asynchrone fait qu'une seule d'entre elles interroge l'API, les autres lisent le résultat.
#[derive(Default)]
struct WsBulkCache {
    state: std::sync::Mutex<WsBulkState>,
    refresh: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct WsBulkState {
    /// Clé API pour laquelle `user_id` et `creds` valent (changement de compte → tout est revu).
    key: String,
    user_id: Option<i64>,
    fetched_at: Option<std::time::Instant>,
    creds: std::collections::HashMap<i64, WsCredentials>,
}

impl WsBulkState {
    fn fresh(&self, key: &str, id: i64) -> Option<WsCredentials> {
        if self.key != key || self.fetched_at.is_none_or(|t| t.elapsed() > WS_BULK_TTL) {
            return None;
        }
        self.creds.get(&id).filter(|c| c.token.is_some()).cloned()
    }
}

impl ApiClient {
    pub fn new() -> Self {
        let http = crate::http::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("construction du client HTTP");
        Self { http, ws_bulk: Default::default() }
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {}", token.trim())
    }

    async fn get<T: DeserializeOwned>(&self, token: &str, path: &str) -> Result<T> {
        let resp = self
            .http
            .get(format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await?;
        let data = Self::read_envelope(resp)
            .await?
            .ok_or_else(|| Error::Unexpected("réponse sans champ `data`".into()))?;
        serde_json::from_value(data)
            .map_err(|e| Error::Unexpected(format!("structure `data` inattendue : {e}")))
    }

    /// GET avec paramètres de requête (encodés proprement par reqwest).
    async fn get_query<T: DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let resp = self
            .http
            .get(format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .query(params)
            .send()
            .await?;
        let data = Self::read_envelope(resp)
            .await?
            .ok_or_else(|| Error::Unexpected("réponse sans champ `data`".into()))?;
        serde_json::from_value(data)
            .map_err(|e| Error::Unexpected(format!("structure `data` inattendue : {e}")))
    }

    async fn post_ok<B: Serialize>(&self, token: &str, path: &str, body: &B) -> Result<()> {
        self.send_no_data(reqwest::Method::POST, token, path, body).await
    }

    /// POST dont la réponse porte des données utiles (à l'inverse de [`Self::post_ok`]).
    async fn post_data<T: DeserializeOwned, B: Serialize>(
        &self,
        token: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let resp = self
            .http
            .post(format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .json(body)
            .send()
            .await?;
        let data = Self::read_envelope(resp)
            .await?
            .ok_or_else(|| Error::Unexpected("réponse sans champ `data`".into()))?;
        serde_json::from_value(data)
            .map_err(|e| Error::Unexpected(format!("structure `data` inattendue : {e}")))
    }

    /// Envoie une requête à corps JSON sans exploiter la réponse (juste vérifier le succès).
    async fn send_no_data<B: Serialize>(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
        body: &B,
    ) -> Result<()> {
        let resp = self
            .http
            .request(method, format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .json(body)
            .send()
            .await?;
        Self::read_envelope(resp).await?;
        Ok(())
    }

    async fn put_ok<B: Serialize>(&self, token: &str, path: &str, body: &B) -> Result<()> {
        self.send_no_data(reqwest::Method::PUT, token, path, body).await
    }

    async fn patch_ok<B: Serialize>(&self, token: &str, path: &str, body: &B) -> Result<()> {
        self.send_no_data(reqwest::Method::PATCH, token, path, body).await
    }

    /// Requête AVEC corps qui renvoie l'`job_id` du job asynchrone (0 si absent).
    async fn send_job<B: Serialize>(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
        body: &B,
    ) -> Result<i64> {
        let resp = self
            .http
            .request(method, format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .json(body)
            .send()
            .await?;
        Ok(job_id(&Self::read_envelope(resp).await?))
    }

    /// Requête SANS corps (paramètres dans l'URL, ex. DELETE) qui renvoie l'`job_id`.
    async fn send_job_no_body(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
    ) -> Result<i64> {
        let resp = self
            .http
            .request(method, format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await?;
        Ok(job_id(&Self::read_envelope(resp).await?))
    }

    async fn read_envelope(resp: Response) -> Result<Option<Value>> {
        let status = resp.status();
        match status {
            StatusCode::UNAUTHORIZED => return Err(Error::Unauthorized),
            StatusCode::FORBIDDEN => return Err(Error::Forbidden),
            StatusCode::TOO_MANY_REQUESTS => return Err(Error::RateLimited),
            _ => {}
        }

        let body = resp.text().await?;
        let envelope: Envelope = serde_json::from_str(&body).map_err(|e| {
            Error::Unexpected(format!(
                "JSON illisible ({e}) — début: {}",
                crate::util::truncate_on_boundary(&body, 180, "…")
            ))
        })?;

        if let Some(code) = envelope.api.error {
            return Err(Error::Api { code });
        }
        if !status.is_success() {
            return Err(Error::Unexpected(format!("HTTP {}", status.as_u16())));
        }
        Ok(envelope.api.data)
    }

    // --- Authentification --------------------------------------------------

    pub async fn authenticate(&self, input: &str) -> Result<(UserProfile, String)> {
        let raw = input.trim().to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
        let candidates = if raw == encoded {
            vec![raw]
        } else {
            vec![raw, encoded]
        };

        for token in candidates {
            match self.get_user(&token).await {
                Ok(profile) => return Ok((profile, token)),
                Err(Error::Unauthorized) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(Error::Unauthorized)
    }

    // --- Endpoints ---------------------------------------------------------

    pub async fn get_user(&self, token: &str) -> Result<UserProfile> {
        let data: UserData = self.get(token, "/user").await?;
        Ok(UserProfile::from(data.user.datas))
    }

    /// Notifications de l'hébergeur. Elles voyagent avec le profil : `/user/notifications` et
    /// `/user/notifications/unreaded` (vus dans l'app mobile) ne répondent pas à une clé du
    /// panel — le premier renvoie le profil, le second un 404 (vérifié le 2026-08-19).
    pub async fn get_notifications(&self, token: &str) -> Result<Vec<UserNotification>> {
        let data: UserData = self.get(token, "/user").await?;
        Ok(data
            .user
            .notifications
            .into_iter()
            .map(UserNotification::from)
            .collect())
    }

    pub async fn list_servers(&self, token: &str, user_id: i64) -> Result<ServersOverview> {
        let raw: ServersData = self.get(token, &format!("/user/{user_id}/servers")).await?;
        Ok(ServersOverview::from(raw))
    }

    pub async fn get_server(&self, token: &str, id: i64) -> Result<ServerDetails> {
        let raw: ServerFullData = self.get(token, &format!("/server/{id}")).await?;
        Ok(ServerDetails::from(raw))
    }

    pub async fn get_live_light(&self, token: &str, id: i64) -> Result<LiveLight> {
        self.get(token, &format!("/server/{id}/live/light")).await
    }

    /// État temps réel complet (OpenAPI `getServerLiveData`) : consommation instantanée (dont
    /// FlexCore), réseau, uptime — sans WebSocket. Blocs absents d'un serveur éteint.
    pub async fn get_server_live(&self, token: &str, id: i64) -> Result<ServerLive> {
        self.get(token, &format!("/server/{id}/live")).await
    }

    /// Statistiques de l'hébergeur sur une période (non documenté, panel web) : `from` et `to` en
    /// heure de Paris, `AAAA-MM-JJTHH:MM:SS` (voir [`crate::history`] pour le format relevé).
    /// Renvoie aussi l'heure du serveur (en-tête `Date`, secondes Unix) : elle sert à mesurer le
    /// décalage de l'horloge locale.
    pub(crate) async fn get_server_stats(
        &self,
        token: &str,
        id: i64,
        from: &str,
        to: &str,
    ) -> Result<(HostStatsData, Option<i64>)> {
        let resp = self
            .http
            .get(format!("{API_BASE_URL}/server/{id}/stats/{from}/{to}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await?;
        let server_now = resp
            .headers()
            .get(header::DATE)
            .and_then(|v| v.to_str().ok())
            .and_then(crate::history::parse_http_date);
        let data = Self::read_envelope(resp)
            .await?
            .ok_or_else(|| Error::Unexpected("réponse sans champ `data`".into()))?;
        let stats = serde_json::from_value(data)
            .map_err(|e| Error::Unexpected(format!("structure `data` inattendue : {e}")))?;
        Ok((stats, server_now))
    }

    /// Réglages du serveur (`server.properties`) — permission `settings_view`, sans SFTP.
    pub async fn get_properties(&self, token: &str, id: i64) -> Result<ServerProperties> {
        self.get(token, &format!("/server/{id}/properties")).await
    }

    /// Modifie **un** réglage existant (valeur vide acceptée : vide le réglage). Effet au prochain
    /// démarrage. `enable-rcon`/`rcon.*` exigent en plus la permission `command`,
    /// `op-permission-level` la permission `players_operator`.
    pub async fn set_property(&self, token: &str, id: i64, property: &str, value: &str) -> Result<()> {
        self.patch_ok(
            token,
            &format!("/server/{id}/properties"),
            &serde_json::json!({ "property": property, "value": value }),
        )
        .await
    }

    /// Détail d'une MyBox (OpenAPI `getMyBox`) : abonnement, ressources restantes, offre, serveurs.
    pub async fn get_mybox(&self, token: &str, id: i64) -> Result<MyBoxDetails> {
        self.get(token, &format!("/mybox/{id}")).await
    }

    /// Lecture brute : le champ `data` tel quel, pour un module qui interprète lui-même la réponse
    /// avec tolérance (`mybox.rs`).
    pub(crate) async fn get_data(&self, token: &str, path: &str) -> Result<Value> {
        self.get(token, path).await
    }

    /// Écriture brute : méthode, chemin et corps construits (et testés) par l'appelant
    /// (`mybox::WriteRequest`). Renvoie le champ `data` s'il y en a un.
    pub(crate) async fn send_raw(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        let mut req = self
            .http
            .request(method, format!("{API_BASE_URL}{path}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json");
        if let Some(body) = body {
            req = req.json(body);
        }
        Self::read_envelope(req.send().await?).await
    }

    /// Identifiants WebSocket seuls — réponse ~1 Kio, contre plusieurs Kio pour `get_server`
    /// (qui embarque SFTP, historique, ports…). À préférer partout où seul le couple url/token
    /// compte, en particulier au rafraîchissement sur `token expiring`.
    /// Identifiants WebSocket d'un serveur (rafraîchissement de jeton). Servis par le cache groupé
    /// ([`WsBulkCache`]) ; repli sur l'endpoint par serveur si le serveur n'y figure pas (accès de
    /// sous-utilisateur, serveur ajouté entre-temps) ou si l'endpoint groupé échoue.
    pub async fn get_ws_credentials(&self, token: &str, id: i64) -> Result<WsCredentials> {
        if let Some(c) = self.ws_bulk.state.lock().unwrap().fresh(token, id) {
            return Ok(c);
        }
        {
            let _one_at_a_time = self.ws_bulk.refresh.lock().await;
            // Une autre connexion a peut-être rafraîchi pendant l'attente du verrou.
            if let Some(c) = self.ws_bulk.state.lock().unwrap().fresh(token, id) {
                return Ok(c);
            }
            match self.refresh_ws_bulk(token).await {
                Ok(()) => {
                    if let Some(c) = self.ws_bulk.state.lock().unwrap().fresh(token, id) {
                        return Ok(c);
                    }
                }
                Err(e @ (Error::Unauthorized | Error::RateLimited)) => return Err(e),
                Err(e) => tracing::debug!("jetons WebSocket groupés indisponibles, repli : {e}"),
            }
        }
        let raw: WsCredentialsData = self.get(token, &format!("/server/{id}/websocket")).await?;
        Ok(WsCredentials::from(raw))
    }

    /// Recharge le cache groupé : `GET /user/{id}/servers/websocket` (l'id utilisateur est résolu
    /// une fois par clé).
    async fn refresh_ws_bulk(&self, token: &str) -> Result<()> {
        let cached_uid = {
            let st = self.ws_bulk.state.lock().unwrap();
            (st.key == token).then_some(st.user_id).flatten()
        };
        let uid = match cached_uid {
            Some(uid) => uid,
            None => self.get_user(token).await?.id,
        };
        let data: WsBulkData = self.get(token, &format!("/user/{uid}/servers/websocket")).await?;
        let mut st = self.ws_bulk.state.lock().unwrap();
        st.key = token.to_string();
        st.user_id = Some(uid);
        st.fetched_at = Some(std::time::Instant::now());
        st.creds = data.servers.into_iter().map(|e| (e.id, WsCredentials::from(e))).collect();
        Ok(())
    }

    /// Compte de joueurs de plusieurs serveurs en un appel. Ne renvoie **que** les joueurs :
    /// pour le CPU/RAM/disque ou le statut, il faut toujours [`Self::get_live_light`] par serveur.
    pub async fn live_players_bulk(&self, token: &str, ids: &[i64]) -> Result<Vec<LivePlayers>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let raw: LiveBulkData = self
            .post_data(token, "/servers/live/light/bulk", &json!({ "server_ids": ids }))
            .await?;
        Ok(raw
            .servers
            .into_iter()
            .filter_map(|(id, entry)| {
                let players = entry?.players?;
                Some(LivePlayers {
                    server_id: id.parse().ok()?,
                    current: players.current,
                    limit: players.limit,
                })
            })
            .collect())
    }

    /// Journal d'audit d'un serveur (paginé, du plus récent au plus ancien). Couvre les actions
    /// faites hors de l'app — panel web, staff de l'hébergeur — qu'un journal local ne verrait pas.
    pub async fn server_activity(
        &self,
        token: &str,
        id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<ActivityPage> {
        let raw: ActivityData = self
            .get(token, &format!("/server/{id}/activity?limit={limit}&offset={offset}"))
            .await?;
        Ok(ActivityPage {
            total: raw.pagination.total,
            has_more: raw.pagination.has_more,
            items: raw.activity.into_iter().map(ActivityEntry::from).collect(),
        })
    }

    /// Catalogue des eggs de l'hébergeur (jeux proposés + bases de données).
    pub async fn list_eggs(&self, token: &str) -> Result<Vec<Egg>> {
        let raw: EggsData = self.get(token, "/site/eggs").await?;
        Ok(raw.eggs.into_iter().map(Egg::from).collect())
    }

    pub async fn get_console_logs(&self, token: &str, id: i64) -> Result<Vec<String>> {
        let logs: ConsoleLogs = self.get(token, &format!("/server/{id}/console/logs")).await?;
        Ok(logs.logs)
    }

    pub async fn power_action(&self, token: &str, id: i64, action: &str) -> Result<()> {
        self.put_ok(
            token,
            &format!("/server/{id}/poweraction"),
            &serde_json::json!({ "poweraction": action }),
        )
        .await
    }

    pub async fn send_command(&self, token: &str, id: i64, command: &str) -> Result<()> {
        self.put_ok(
            token,
            &format!("/server/{id}/command"),
            &serde_json::json!({ "command": command }),
        )
        .await
    }

    pub async fn player_action(
        &self,
        token: &str,
        id: i64,
        action: &str,
        player: &str,
    ) -> Result<()> {
        let path = match action {
            "kick" => "command/minecraft/kick",
            "ban" => "command/minecraft/ban",
            "unban" => "command/minecraft/unban",
            "op_add" => "command/minecraft/op/add",
            "op_remove" => "command/minecraft/op/remove",
            "whitelist_add" => "command/minecraft/whitelist/add",
            "whitelist_remove" => "command/minecraft/whitelist/remove",
            other => return Err(Error::Unexpected(format!("action joueur inconnue : {other}"))),
        };
        self.put_ok(
            token,
            &format!("/server/{id}/{path}"),
            &serde_json::json!({ "player": player }),
        )
        .await
    }

    // --- Joueurs déjà venus (non documenté, panel web ; décodage dans `players.rs`) ---------

    /// Une page de la liste des joueurs déjà venus (`limit` ≤ 200, triée par dernière venue).
    pub async fn players_offline(&self, token: &str, id: i64, limit: i64, offset: i64) -> Result<Value> {
        self.get_query(
            token,
            &format!("/server/{id}/players/offline"),
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    /// Fiche d'un joueur (`uuid` déjà validé par l'appelant).
    pub async fn player_profile(&self, token: &str, id: i64, uuid: &str) -> Result<Value> {
        self.get(token, &format!("/server/{id}/players/{uuid}/profile")).await
    }

    /// Inventaire d'un joueur, lu dans son fichier `playerdata` (`uuid` déjà validé).
    pub async fn player_inventory(&self, token: &str, id: i64, uuid: &str) -> Result<Value> {
        self.get(token, &format!("/server/{id}/players/{uuid}/inventory")).await
    }

    /// Lit la configuration de démarrage (commande Java) depuis `/server/{id}` (`settings`).
    pub async fn get_startup(&self, token: &str, id: i64) -> Result<Startup> {
        let raw: StartupData = self.get(token, &format!("/server/{id}")).await?;
        Ok(Startup::from(raw))
    }

    /// Modifie la commande de démarrage (endpoint `startup/params`, non documenté mais réel).
    /// Prend effet au **prochain démarrage** du serveur.
    pub async fn set_startup_params(&self, token: &str, id: i64, parameters: &str) -> Result<()> {
        self.patch_ok(
            token,
            &format!("/server/{id}/startup/params"),
            &serde_json::json!({ "parameters": parameters }),
        )
        .await
    }

    // --- Marketplace (mods & plugins) --------------------------------------

    /// Liste des versions Minecraft connues du catalogue.
    pub async fn market_minecraft_versions(&self, token: &str) -> Result<Vec<String>> {
        let raw: McVersionsData = self.get(token, "/site/minecraft/versions").await?;
        Ok(raw.versions)
    }

    /// Catalogue paginé. `kind` = `mods`|`plugins`, `source` = `modrinth`|`curseforge`|`spigot`.
    /// `query` vide ⇒ liste populaire ; sinon recherche par nom.
    // Paramètres de recherche hétérogènes issus de l'UI ; un « struct requête » serait artificiel.
    #[allow(clippy::too_many_arguments)]
    pub async fn market_list(
        &self,
        token: &str,
        kind: &str,
        source: &str,
        page: i64,
        query: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<MarketPage> {
        let q = query.trim();
        let (path, mut params): (String, Vec<(&str, String)>) = match (source, kind) {
            ("modrinth", "mods") if q.is_empty() => (format!("/site/modrinth/mods/{page}"), vec![]),
            ("modrinth", "mods") => {
                ("/site/modrinth/mods/search".into(), vec![("q", q.to_string())])
            }
            ("modrinth", _) if q.is_empty() => (format!("/site/modrinth/{page}"), vec![]),
            ("modrinth", _) => ("/site/modrinth/search".into(), vec![("q", q.to_string())]),
            ("curseforge", _) => (
                "/site/curseforge/mods/search".into(),
                vec![("q", q.to_string()), ("page", page.to_string())],
            ),
            ("spigot", _) if q.is_empty() => (format!("/site/plugins/{page}"), vec![]),
            ("spigot", _) => (
                "/site/plugins/search".into(),
                vec![("q", q.to_string()), ("page", page.to_string())],
            ),
            _ => return Err(Error::Unexpected(format!("source marketplace inconnue : {source}"))),
        };
        // SpigotMC n'a pas de notion de loader/version côté catalogue (ses endpoints ignorent
        // ces paramètres) → on ne les envoie que pour Modrinth/CurseForge.
        if source != "spigot" {
            if !loader.is_empty() {
                params.push(("loader", loader.to_string()));
            }
            if !game_version.is_empty() {
                params.push(("gameVersion", game_version.to_string()));
            }
        }
        let raw: MarketListRaw = self.get_query(token, &path, &params).await?;
        Ok(MarketPage::from(raw))
    }

    /// Versions d'un projet. `slug_or_id` = slug (Modrinth) ou id numérique (CurseForge/SpigotMC).
    pub async fn market_versions(
        &self,
        token: &str,
        source: &str,
        slug_or_id: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<Vec<MarketVersion>> {
        let path = match source {
            "modrinth" => format!("/site/modrinth/{slug_or_id}/versions"),
            "curseforge" => format!("/site/curseforge/{slug_or_id}/versions"),
            "spigot" => format!("/site/plugin/{slug_or_id}/versions"),
            _ => return Err(Error::Unexpected(format!("source marketplace inconnue : {source}"))),
        };
        let mut params = Vec::new();
        if source != "spigot" {
            if !loader.is_empty() {
                params.push(("loader", loader.to_string()));
            }
            if !game_version.is_empty() {
                params.push(("gameVersion", game_version.to_string()));
            }
        }
        let raw: MarketVersionsRaw = self.get_query(token, &path, &params).await?;
        Ok(raw.versions.into_iter().map(MarketVersion::from).collect())
    }

    /// Installe un projet Modrinth (`kind` = `mod`|`plugin`) sur un serveur.
    /// Corps vérifié en direct : `{slug, version_id, loader}`.
    /// Installe un projet Modrinth (mod OU plugin) sur un serveur. Corps vérifié en direct sur le
    /// panel : `POST /server/{id}/modrinth` avec `{slug, version_id}` — SANS suffixe `/mod|/plugin`
    /// (inexistants → 404) ni champ `loader` ; l'API détermine seule le placement mod/plugin.
    pub async fn install_modrinth(
        &self,
        token: &str,
        server_id: i64,
        slug: &str,
        version_id: &str,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/modrinth"),
            &serde_json::json!({ "slug": slug, "version_id": version_id }),
        )
        .await
    }

    /// Installe un plugin SpigotMC sur un serveur.
    /// Corps vérifié en direct : `{id_plugin, id_version}` (entiers).
    pub async fn install_spigot(
        &self,
        token: &str,
        server_id: i64,
        plugin_id: i64,
        version_id: i64,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/plugin"),
            &serde_json::json!({ "id_plugin": plugin_id, "id_version": version_id }),
        )
        .await
    }

    /// Mods installés (`/server/{id}/mods`).
    pub async fn installed_mods(&self, token: &str, id: i64) -> Result<Vec<InstalledItem>> {
        let raw: InstalledRaw = self.get(token, &format!("/server/{id}/mods")).await?;
        Ok(raw.mods.into_iter().map(InstalledItem::from).collect())
    }

    /// Plugins installés (`/server/{id}/plugins`).
    pub async fn installed_plugins(&self, token: &str, id: i64) -> Result<Vec<InstalledItem>> {
        let raw: InstalledRaw = self.get(token, &format!("/server/{id}/plugins")).await?;
        Ok(raw.mods.into_iter().map(InstalledItem::from).collect())
    }

    // --- Mises à jour des mods, plugins et datapacks (non documenté, voir `updates.rs`) ---

    /// Version de Minecraft détectée par l'hébergeur (`GET /server/{id}/minecraft_version`).
    pub(crate) async fn minecraft_version(&self, token: &str, id: i64) -> Result<McVersion> {
        let data: Value = self.get(token, &format!("/server/{id}/minecraft_version")).await?;
        Ok(McVersion::from_value(&data))
    }

    /// Contenu installé d'un type, lu avec tolérance (`GET /server/{id}/{mods|plugins|datapacks}`).
    pub(crate) async fn content_list(&self, token: &str, id: i64, kind: ContentKind) -> Result<ContentList> {
        let data: Value = self.get(token, &format!("/server/{id}/{}", kind.as_str())).await?;
        Ok(ContentList::from_value(kind, &data))
    }

    /// Vérification des mises à jour d'un type (`POST /server/{id}/{type}/updates`) : pure
    /// comparaison côté hébergeur, rien n'est installé. Réponse brute, lue par `updates.rs`.
    pub(crate) async fn check_content_updates(
        &self,
        token: &str,
        id: i64,
        kind: ContentKind,
        body: &Value,
    ) -> Result<Value> {
        self.post_data(token, &format!("/server/{id}/{}/updates", kind.as_str()), body).await
    }

    /// Installe une version Modrinth d'un mod (`POST /server/{id}/modrinth/mod`), comme le panel
    /// pour ses mises à jour de mods : `{slug, version_id, loader}`.
    pub async fn install_modrinth_mod(
        &self,
        token: &str,
        server_id: i64,
        slug: &str,
        version_id: &str,
        loader: &str,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/modrinth/mod"),
            &json!({ "slug": slug, "version_id": version_id, "loader": loader }),
        )
        .await
    }

    /// Installe un fichier CurseForge d'un mod (`POST /server/{id}/curseforge/mod`) :
    /// `{modId, fileId, loader}`.
    pub async fn install_curseforge_mod(
        &self,
        token: &str,
        server_id: i64,
        mod_id: i64,
        file_id: i64,
        loader: &str,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/curseforge/mod"),
            &json!({ "modId": mod_id, "fileId": file_id, "loader": loader }),
        )
        .await
    }

    /// Installe une version Modrinth d'un datapack (`POST /server/{id}/modrinth/datapack`).
    pub async fn install_modrinth_datapack(
        &self,
        token: &str,
        server_id: i64,
        slug: &str,
        version_id: &str,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/modrinth/datapack"),
            &json!({ "slug": slug, "version_id": version_id }),
        )
        .await
    }

    /// Installe un fichier CurseForge d'un datapack (`POST /server/{id}/curseforge/datapack`).
    pub async fn install_curseforge_datapack(
        &self,
        token: &str,
        server_id: i64,
        mod_id: i64,
        file_id: i64,
    ) -> Result<()> {
        self.post_ok(
            token,
            &format!("/server/{server_id}/curseforge/datapack"),
            &json!({ "modId": mod_id, "fileId": file_id }),
        )
        .await
    }

    /// Supprime un mod installé par son nom de fichier (`DELETE /server/{id}/mod/{fichier}`).
    pub async fn delete_mod_file(&self, token: &str, server_id: i64, filename: &str) -> Result<()> {
        let path = format!("/server/{server_id}/mod/{}", crate::updates::encode_component(filename));
        self.send_job_no_body(reqwest::Method::DELETE, token, &path).await.map(|_| ())
    }

    /// Supprime un plugin installé par son nom de fichier (`DELETE /server/{id}/plugin/{fichier}`).
    pub async fn delete_plugin_file(&self, token: &str, server_id: i64, filename: &str) -> Result<()> {
        let path = format!("/server/{server_id}/plugin/{}", crate::updates::encode_component(filename));
        self.send_job_no_body(reqwest::Method::DELETE, token, &path).await.map(|_| ())
    }

    /// Supprime des fichiers du serveur par l'API fichiers (`POST /server/{id}/files/delete`,
    /// chemins relatifs à la racine, comme le panel pour un datapack).
    pub async fn delete_files(&self, token: &str, server_id: i64, paths: &[String]) -> Result<()> {
        self.post_ok(token, &format!("/server/{server_id}/files/delete"), &json!({ "files": paths }))
            .await
    }

    /// Backups quotidiens automatiques d'un serveur (récents d'abord).
    pub async fn list_backups(&self, token: &str, id: i64) -> Result<Vec<Backup>> {
        let data: BackupsData = self.get(token, &format!("/server/{id}/backups")).await?;
        Ok(data.backups)
    }

    /// Snapshots (points de sauvegarde à la demande) de l'utilisateur.
    pub async fn list_snapshots(&self, token: &str, user_id: i64) -> Result<Vec<Snapshot>> {
        let data: SnapshotsData = self.get(token, &format!("/user/{user_id}/snapshots")).await?;
        Ok(data.snapshots)
    }

    /// Crée un snapshot du serveur (additif, sans risque). Renvoie l'`job_id` asynchrone.
    pub async fn create_snapshot(
        &self,
        token: &str,
        user_id: i64,
        server_id: i64,
        name: &str,
    ) -> Result<i64> {
        self.send_job(
            reqwest::Method::POST,
            token,
            &format!("/user/{user_id}/snapshot"),
            &serde_json::json!({ "id_server": server_id, "name": name }),
        )
        .await
    }

    /// **Restaure** un snapshot sur un serveur (DESTRUCTIF : écrase l'état actuel). `job_id` async.
    pub async fn restore_snapshot(
        &self,
        token: &str,
        user_id: i64,
        snapshot_id: i64,
        server_id: i64,
    ) -> Result<i64> {
        self.send_job(
            reqwest::Method::PUT,
            token,
            &format!("/user/{user_id}/snapshot"),
            &serde_json::json!({ "id_snapshot": snapshot_id, "id_server": server_id }),
        )
        .await
    }

    /// Supprime définitivement un snapshot. Renvoie l'`job_id` asynchrone.
    pub async fn delete_snapshot(&self, token: &str, user_id: i64, snapshot_id: i64) -> Result<i64> {
        self.send_job_no_body(
            reqwest::Method::DELETE,
            token,
            &format!("/user/{user_id}/snapshot/{snapshot_id}"),
        )
        .await
    }

    /// **Restaure** un backup quotidien sur un serveur (DESTRUCTIF : écrase l'état actuel).
    pub async fn restore_backup(&self, token: &str, server_id: i64, backup_id: i64) -> Result<()> {
        self.put_ok(
            token,
            &format!("/server/{server_id}/backup"),
            &serde_json::json!({ "id_backup": backup_id }),
        )
        .await
    }

    // --- Tâches planifiées du panel (non documentées, cf. panel_schedules.rs) ------

    pub async fn panel_schedules(&self, token: &str, server_id: i64) -> Result<Vec<PanelSchedule>> {
        let data: SchedulesData = self.get(token, &format!("/server/{server_id}/schedules")).await?;
        Ok(data.sorted())
    }

    pub async fn create_panel_schedule(&self, token: &str, server_id: i64, input: &PanelScheduleInput) -> Result<()> {
        self.post_ok(token, &format!("/server/{server_id}/schedule"), &input.body()).await
    }

    pub async fn update_panel_schedule(
        &self,
        token: &str,
        server_id: i64,
        schedule_id: i64,
        input: &PanelScheduleInput,
    ) -> Result<()> {
        self.patch_ok(token, &format!("/server/{server_id}/schedule/{schedule_id}"), &input.body()).await
    }

    pub async fn delete_panel_schedule(&self, token: &str, server_id: i64, schedule_id: i64) -> Result<()> {
        self.send_job_no_body(reqwest::Method::DELETE, token, &format!("/server/{server_id}/schedule/{schedule_id}"))
            .await
            .map(|_| ())
    }

    pub async fn add_panel_task(&self, token: &str, server_id: i64, schedule_id: i64, input: &PanelTaskInput) -> Result<()> {
        self.post_ok(token, &format!("/server/{server_id}/schedule/{schedule_id}/task"), &input.body()).await
    }

    pub async fn update_panel_task(
        &self,
        token: &str,
        server_id: i64,
        schedule_id: i64,
        task_id: i64,
        input: &PanelTaskInput,
    ) -> Result<()> {
        self.patch_ok(token, &format!("/server/{server_id}/schedule/{schedule_id}/task/{task_id}"), &input.body())
            .await
    }

    pub async fn delete_panel_task(&self, token: &str, server_id: i64, schedule_id: i64, task_id: i64) -> Result<()> {
        self.send_job_no_body(
            reqwest::Method::DELETE,
            token,
            &format!("/server/{server_id}/schedule/{schedule_id}/task/{task_id}"),
        )
        .await
        .map(|_| ())
    }

    pub async fn reorder_panel_tasks(&self, token: &str, server_id: i64, schedule_id: i64, task_ids: &[i64]) -> Result<()> {
        self.patch_ok(
            token,
            &format!("/server/{server_id}/schedule/{schedule_id}/tasks/order"),
            &json!({ "task_ids": task_ids }),
        )
        .await
    }

    // --- Fichiers côté serveur (non documentés, cf. server_files.rs) ----------------------------

    /// Plus grosses entrées d'un dossier (`GET /server/{id}/disk/usage?limit=&path=`).
    pub async fn disk_usage(&self, token: &str, id: i64, path: &str, limit: u32) -> Result<DiskUsage> {
        let query = server_files::disk_usage_query(path, limit);
        let data: Value = self.get_query(token, &format!("/server/{id}/disk/usage"), &query).await?;
        Ok(server_files::parse_disk_usage(&data, path))
    }

    /// Occupation comptée du disque d'un serveur (`GET /server/{id}/disk/total`), sans la limite.
    pub async fn disk_total(&self, token: &str, id: i64) -> Result<DiskSpace> {
        let data: Value = self.get(token, &format!("/server/{id}/disk/total")).await?;
        Ok(server_files::parse_disk_total(&data))
    }

    /// Octets occupés par l'ensemble des serveurs d'une MyBox (`GET /mybox/{id}/disk`).
    pub async fn mybox_disk(&self, token: &str, mybox_id: i64) -> Result<u64> {
        let data: Value = self.get(token, &format!("/mybox/{mybox_id}/disk")).await?;
        Ok(server_files::parse_mybox_disk(&data))
    }

    /// Lance une compression sur le serveur (`POST /server/{id}/files/compress/jobs`) : renvoie
    /// le job créé, à suivre avec [`Self::compress_job`].
    pub async fn compress_start(&self, token: &str, id: i64, body: &Value) -> Result<CompressJob> {
        let data: Value = self.post_data(token, &format!("/server/{id}/files/compress/jobs"), body).await?;
        CompressJob::from_value(&data)
            .ok_or_else(|| Error::Unexpected("compression lancée sans identifiant de suivi".into()))
    }

    /// État d'un job de compression, en « long polling » : le serveur répond au changement d'état
    /// ou au bout de `wait` secondes (comme le panel). Délai propre à la requête, au-delà de
    /// l'attente annoncée : une réponse qui ne vient jamais ne bloque pas le suivi.
    pub async fn compress_job(&self, token: &str, id: i64, job_id: i64) -> Result<CompressJob> {
        let wait = server_files::JOB_WAIT_S;
        let resp = self
            .http
            .get(format!("{API_BASE_URL}/server/{id}/files/compress/jobs/{job_id}"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .query(&[("wait", wait.to_string())])
            .timeout(std::time::Duration::from_secs(wait + 25))
            .send()
            .await?;
        let data = Self::read_envelope(resp)
            .await?
            .ok_or_else(|| Error::Unexpected("réponse sans champ `data`".into()))?;
        CompressJob::from_value(&data)
            .ok_or_else(|| Error::Unexpected("job de compression illisible".into()))
    }

    /// Derniers jobs de compression d'un serveur (`GET …/compress/jobs?limit=`).
    pub async fn compress_jobs(&self, token: &str, id: i64, limit: u32) -> Result<Vec<CompressJob>> {
        let data: Value = self
            .get_query(token, &format!("/server/{id}/files/compress/jobs"), &[("limit", limit.to_string())])
            .await?;
        Ok(server_files::parse_jobs(&data))
    }

    /// Extrait une archive sur le serveur (`POST /server/{id}/files/extract`) ; l'appel rend la
    /// main une fois l'extraction finie. Renvoie le `data` de la réponse (destination, ignorés).
    pub async fn extract(&self, token: &str, id: i64, body: &Value) -> Result<Option<Value>> {
        let resp = self
            .http
            .post(format!("{API_BASE_URL}/server/{id}/files/extract"))
            .header(header::AUTHORIZATION, Self::bearer(token))
            .header(header::ACCEPT, "application/json")
            .json(body)
            .send()
            .await?;
        Self::read_envelope(resp).await
    }

    /// Identifiants SFTP d'un serveur.
    ///
    /// `GET /server/{id}/sftp` (non documenté, utilisé par le panel web) est le seul endpoint qui
    /// renvoie encore le **mot de passe** à une clé API : le bloc `sftp` de `GET /server/{id}` ne
    /// le contient plus depuis fin 2026. Repli sur ce dernier (sans mot de passe) si l'endpoint
    /// dédié disparaît ; seules les erreurs d'accès (clé refusée, réseau, quota) remontent telles
    /// quelles. Voir `docs/api/REFERENCE.md`.
    pub async fn get_sftp_creds(&self, token: &str, id: i64) -> Result<SftpCreds> {
        let usable = |d: SftpData| d.sftp.filter(|c| !c.host.is_empty());
        match self.get::<SftpData>(token, &format!("/server/{id}/sftp")).await {
            Ok(d) => {
                if let Some(creds) = usable(d) {
                    return Ok(creds);
                }
            }
            Err(e @ (Error::Unauthorized | Error::Network(_) | Error::RateLimited)) => return Err(e),
            Err(e) => tracing::warn!(server = id, "GET /server/{{id}}/sftp indisponible, repli : {e}"),
        }
        let data: SftpData = self.get(token, &format!("/server/{id}")).await?;
        usable(data).ok_or_else(|| Error::Unexpected("Accès SFTP indisponible pour ce serveur.".into()))
    }
}

/// Extrait `data.job_id` d'une enveloppe (les opérations async de MineStrator le renvoient) ; 0 sinon.
fn job_id(data: &Option<Value>) -> i64 {
    data.as_ref()
        .and_then(|d| d.get("job_id"))
        .and_then(|j| j.as_i64())
        .unwrap_or(0)
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::http;

    /// Réponse HTTP construite en mémoire (aucune E/S réseau).
    fn response(status: u16, body: &str) -> Response {
        http::Response::builder()
            .status(status)
            .body(body.to_string())
            .unwrap()
            .into()
    }

    async fn envelope(status: u16, body: &str) -> Result<Option<Value>> {
        ApiClient::read_envelope(response(status, body)).await
    }

    fn unexpected_message(r: Result<Option<Value>>) -> String {
        match r {
            Err(Error::Unexpected(m)) => m,
            other => panic!("Error::Unexpected attendu, obtenu {other:?}"),
        }
    }

    #[tokio::test]
    async fn success_returns_the_data_field() {
        let data = envelope(
            200,
            r#"{"api":{"code":200,"endpoint":"/user","data":{"user":{"id":7,"pseudo":"Steve"}}}}"#,
        )
        .await
        .unwrap();
        assert_eq!(data, Some(json!({ "user": { "id": 7, "pseudo": "Steve" } })));
    }

    #[tokio::test]
    async fn success_without_data_is_none() {
        // Actions (commande, power action…) : l'enveloppe n'a pas de `data`.
        let data = envelope(200, r#"{"api":{"code":200,"endpoint":"/server/1/command"}}"#)
            .await
            .unwrap();
        assert_eq!(data, None);
        let data = envelope(200, r#"{"api":{"data":null}}"#).await.unwrap();
        assert_eq!(data, None);
    }

    #[tokio::test]
    async fn auth_and_rate_limit_statuses_win_over_the_body() {
        let valid = r#"{"api":{"code":200,"data":{"x":1}}}"#;
        assert!(matches!(envelope(401, valid).await, Err(Error::Unauthorized)));
        assert!(matches!(envelope(403, valid).await, Err(Error::Forbidden)));
        assert!(matches!(envelope(429, valid).await, Err(Error::RateLimited)));
        // Corps illisible (page HTML d'un proxy) : le statut suffit.
        assert!(matches!(envelope(401, "<html>401</html>").await, Err(Error::Unauthorized)));
        assert!(matches!(envelope(429, "").await, Err(Error::RateLimited)));
    }

    #[tokio::test]
    async fn api_error_code_is_surfaced_whatever_the_status() {
        for status in [200, 400, 404, 500] {
            match envelope(status, r#"{"api":{"code":404,"endpoint":"/server/9","error":"SERVER_NOT_FOUND"}}"#)
                .await
            {
                Err(Error::Api { code }) => assert_eq!(code, "SERVER_NOT_FOUND"),
                other => panic!("HTTP {status} : Error::Api attendu, obtenu {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn http_failure_without_error_code_is_unexpected() {
        let msg = unexpected_message(envelope(500, r#"{"api":{"code":500}}"#).await);
        assert_eq!(msg, "HTTP 500");
        // Même avec des données : un statut d'échec n'est jamais un succès.
        let msg = unexpected_message(envelope(502, r#"{"api":{"data":{"x":1}}}"#).await);
        assert_eq!(msg, "HTTP 502");
    }

    #[tokio::test]
    async fn unreadable_json_reports_a_truncated_excerpt() {
        let html = format!("<html><body>{}</body></html>", "x".repeat(400));
        let msg = unexpected_message(envelope(200, &html).await);
        assert!(msg.starts_with("JSON illisible ("), "{msg}");
        let excerpt = msg.split_once("— début: ").expect("extrait du corps").1;
        assert!(excerpt.starts_with("<html><body>xxx"), "{excerpt}");
        assert!(excerpt.ends_with('…'), "{excerpt}");
        assert_eq!(excerpt.len(), 180 + '…'.len_utf8());

        // Coupe sur une frontière de caractère (pas de panique sur l'UTF-8 multi-octets).
        let msg = unexpected_message(envelope(200, &"é".repeat(200)).await);
        let excerpt = msg.split_once("— début: ").unwrap().1;
        assert_eq!(excerpt, format!("{}…", "é".repeat(90)));

        // JSON valide mais sans enveloppe `api` → illisible aussi.
        let msg = unexpected_message(envelope(200, r#"{"data":{"x":1}}"#).await);
        assert!(msg.starts_with("JSON illisible ("), "{msg}");
        // Une passerelle en panne (502 HTML) remonte comme JSON illisible, pas comme « HTTP 502 ».
        let msg = unexpected_message(envelope(502, "<html>Bad Gateway</html>").await);
        assert!(msg.contains("Bad Gateway"), "{msg}");
    }

    #[test]
    fn job_id_is_read_from_data() {
        assert_eq!(job_id(&Some(json!({ "job_id": 4242 }))), 4242);
        assert_eq!(job_id(&Some(json!({ "job_id": "4242" }))), 0);
        assert_eq!(job_id(&Some(json!({}))), 0);
        assert_eq!(job_id(&None), 0);
    }

    #[test]
    fn bearer_header_trims_the_token() {
        assert_eq!(ApiClient::bearer("  abc123\n"), "Bearer abc123");
        assert_eq!(ApiClient::bearer("abc"), "Bearer abc");
    }

    #[tokio::test]
    async fn unknown_player_action_or_market_source_fail_without_network() {
        let api = ApiClient::new();
        match api.player_action("t", 1, "teleport", "Steve").await {
            Err(Error::Unexpected(m)) => assert_eq!(m, "action joueur inconnue : teleport"),
            other => panic!("{other:?}"),
        }
        match api.market_list("t", "mods", "nexus", 1, "", "", "").await {
            Err(Error::Unexpected(m)) => assert_eq!(m, "source marketplace inconnue : nexus"),
            other => panic!("{other:?}"),
        }
        match api.market_versions("t", "nexus", "x", "", "").await {
            Err(Error::Unexpected(m)) => assert_eq!(m, "source marketplace inconnue : nexus"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grouped_ws_tokens_are_served_only_fresh_and_for_the_same_key() {
        let creds = |t: Option<&str>| WsCredentials { url: Some("wss://n".into()), token: t.map(Into::into) };
        let mut st = WsBulkState {
            key: "cle-a".into(),
            user_id: Some(1),
            fetched_at: Some(std::time::Instant::now()),
            creds: [(10, creds(Some("jwt"))), (11, creds(None))].into_iter().collect(),
        };
        assert_eq!(st.fresh("cle-a", 10).and_then(|c| c.token).as_deref(), Some("jwt"));
        assert!(st.fresh("cle-b", 10).is_none(), "autre clé : rien ne fuit d'un compte à l'autre");
        assert!(st.fresh("cle-a", 11).is_none(), "sans jeton : repli sur l'endpoint par serveur");
        assert!(st.fresh("cle-a", 12).is_none(), "serveur absent de la réponse groupée");
        st.fetched_at = std::time::Instant::now().checked_sub(WS_BULK_TTL + std::time::Duration::from_secs(1));
        assert!(st.fetched_at.is_none() || st.fresh("cle-a", 10).is_none(), "périmé");
    }

    #[tokio::test]
    async fn bulk_players_with_no_ids_skips_the_request() {
        assert!(ApiClient::new().live_players_bulk("t", &[]).await.unwrap().is_empty());
    }
}
