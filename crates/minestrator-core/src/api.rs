//! Client HTTP typé vers l'API MineStrator (pur : le token est passé à chaque appel).
//!
//! Auth : `Authorization: Bearer <token>`. Le panel affiche la clé DÉJÀ encodée en base64 ;
//! `authenticate` essaie la valeur telle quelle puis, en secours, sa version base64.

use crate::config::{API_BASE_URL, USER_AGENT};
use crate::error::{Error, Result};
use crate::models::{
    Backup, BackupsData, ConsoleLogs, Envelope, InstalledItem, InstalledRaw, LiveLight,
    MarketListRaw, MarketPage, MarketVersion, MarketVersionsRaw, McVersionsData, ServerDetails,
    ServerFullData, ServersData, ServersOverview, SftpCreds, SftpData, Snapshot, SnapshotsData,
    Startup, StartupData, UserData, UserProfile,
};
use base64::Engine;
use reqwest::{header, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
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

    pub async fn get_sftp_creds(&self, token: &str, id: i64) -> Result<SftpCreds> {
        let data: SftpData = self.get(token, &format!("/server/{id}")).await?;
        data.sftp
            .filter(|c| !c.host.is_empty())
            .ok_or_else(|| Error::Unexpected("Accès SFTP indisponible pour ce serveur.".into()))
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
