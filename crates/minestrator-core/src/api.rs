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
    WsCredentials, WsCredentialsData,
};
use base64::Engine;
use reqwest::{header, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new() -> Self {
        let http = crate::http::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("construction du client HTTP");
        Self { http }
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

    /// Identifiants WebSocket seuls — réponse ~1 Kio, contre plusieurs Kio pour `get_server`
    /// (qui embarque SFTP, historique, ports…). À préférer partout où seul le couple url/token
    /// compte, en particulier au rafraîchissement sur `token expiring`.
    pub async fn get_ws_credentials(&self, token: &str, id: i64) -> Result<WsCredentials> {
        let raw: WsCredentialsData = self.get(token, &format!("/server/{id}/websocket")).await?;
        Ok(WsCredentials::from(raw))
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

    #[tokio::test]
    async fn bulk_players_with_no_ids_skips_the_request() {
        assert!(ApiClient::new().live_players_bulk("t", &[]).await.unwrap().is_empty());
    }
}
