//! Marketplaces de mods « push SFTP » multi-sources (Thunderstore, Factorio, uMod…) — modèles
//! **normalisés** + aiguillage par source. Chaque source a son client (`thunderstore`/`factorio`/
//! `umod`), calqué sur `ficsit.rs`. Objectif : ajouter un jeu = 1 mapping `jeu→source` dans
//! `games.rs` + 1 module client ; la vue front générique et cette façade ne changent pas.
//!
//! Ici : le **read-path** (parcours + versions). L'installation (download + SFTP) viendra ensuite.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

// --- Réglages par jeu (persistés) -----------------------------------------

/// Réglages par jeu (partie **non-secrète**, persistée `game_settings.json`). Les secrets (token
/// Factorio) vont au trousseau (voir `secrets::store_game_secret`). Extensible : un champ par jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameSettings {
    #[serde(default)]
    pub factorio: FactorioSettings,
}

/// Réglages Factorio : le `username` du compte factorio.com (le token de download va au trousseau).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactorioSettings {
    #[serde(default)]
    pub username: String,
}

/// Client HTTP externe partagé (réseau côté cœur, hors CSP webview). UA « navigateur » : certaines
/// API (Cloudflare devant Thunderstore) refusent un UA vide/curl.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("MinestratorTerminal/0.2 (+https://minestrator.com)")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// GET + désérialisation JSON typée, avec `params` de query encodés proprement.
pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    params: &[(&str, &str)],
) -> Result<T> {
    let resp = HTTP
        .get(url)
        .query(params)
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("requête marketplace : {e}")))?
        .error_for_status()
        .map_err(|e| Error::Unexpected(format!("réponse marketplace : {e}")))?;
    resp.json()
        .await
        .map_err(|e| Error::Unexpected(format!("JSON marketplace : {e}")))
}

/// Exécute une requête de téléchargement binaire, plafonnée (garde-fou OOM), en **capturant le corps
/// d'erreur** (les API expliquent souvent le refus dedans → diagnostic exploitable au lieu d'un « 403 »
/// muet). Extrait ~300 caractères, espaces normalisés.
async fn fetch_bytes(req: reqwest::RequestBuilder, max: u64) -> Result<Vec<u8>> {
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("téléchargement : {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // Les pages d'erreur sont souvent en HTML → on retire les balises pour exposer le texte lisible.
        let text = strip_tags(&body);
        let snippet: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect();
        return Err(Error::Unexpected(if snippet.is_empty() {
            format!("HTTP {}", status.as_u16())
        } else {
            format!("HTTP {} — {snippet}", status.as_u16())
        }));
    }
    if resp.content_length().is_some_and(|l| l > max) {
        return Err(Error::Unexpected("fichier trop volumineux.".into()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Unexpected(format!("lecture du téléchargement : {e}")))?;
    if bytes.len() as u64 > max {
        return Err(Error::Unexpected("fichier trop volumineux.".into()));
    }
    Ok(bytes.to_vec())
}

/// Retire grossièrement les balises HTML (`<…>`) pour extraire le message lisible d'une page d'erreur.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// GET binaire avec `params` de query (ex. `?username=&token=`).
pub(crate) async fn get_bytes(url: &str, params: &[(&str, &str)], max: u64) -> Result<Vec<u8>> {
    fetch_bytes(HTTP.get(url).query(params), max).await
}

/// GET binaire avec header `Authorization: Bearer` (clés d'API de portail).
pub(crate) async fn get_bytes_bearer(url: &str, bearer: &str, max: u64) -> Result<Vec<u8>> {
    fetch_bytes(HTTP.get(url).bearer_auth(bearer), max).await
}

// --- Modèles normalisés (Serialize vers le front) -------------------------

/// Un mod du catalogue, quelle que soit la source.
#[derive(Debug, Clone, Serialize)]
pub struct MarketMod {
    /// Référence stable, source-spécifique (sert à récupérer les versions / installer).
    pub reference: String,
    pub name: String,
    pub description: String,
    pub downloads: i64,
    pub icon_url: String,
    /// `thunderstore` | `factorio` | `umod` | …
    pub source: String,
}

/// Page de résultats.
#[derive(Debug, Clone, Serialize)]
pub struct MarketModPage {
    pub mods: Vec<MarketMod>,
    pub count: i64,
    pub has_more: bool,
}

/// Une version publiée d'un mod (normalisée).
#[derive(Debug, Clone, Serialize)]
pub struct MarketModVersion {
    pub version: String,
    /// Contrainte/compatibilité de version de jeu (SML, `factorio_version`…), ou vide.
    pub game_version: String,
    /// Dépendances lisibles (hors loader/framework), pour affichage.
    pub dependencies: Vec<String>,
}

/// Un mod installé sur le serveur (normalisé), avec son état activé.
#[derive(Debug, Clone, Serialize)]
pub struct MarketInstalledMod {
    pub reference: String,
    pub name: String,
    pub enabled: bool,
}

/// Un mod à installer (référence + version voulue ; version vide = dernière).
#[derive(Debug, Clone, Deserialize)]
pub struct ModInstallItem {
    pub reference: String,
    #[serde(default)]
    pub version: String,
}

// --- Aiguillage par source ------------------------------------------------

/// Recherche / tri paginé dans le catalogue d'une source. `family` sert aux sources multi-jeux
/// (Thunderstore : valheim vs v_rising).
pub async fn search(
    source: &str,
    family: &str,
    query: &str,
    order: &str,
    page: i64,
) -> Result<MarketModPage> {
    match source {
        "thunderstore" => crate::thunderstore::search(family, query, order, page).await,
        "factorio" => crate::factorio::search(query, order, page).await,
        "umod" => crate::umod::search(query, page).await,
        other => Err(Error::Unexpected(format!("source de mods inconnue : « {other} »"))),
    }
}

/// Versions d'un mod (référence source-spécifique), récentes d'abord.
pub async fn mod_versions(source: &str, reference: &str) -> Result<Vec<MarketModVersion>> {
    match source {
        "thunderstore" => crate::thunderstore::mod_versions(reference).await,
        "factorio" => crate::factorio::mod_versions(reference).await,
        "umod" => crate::umod::mod_versions(reference).await,
        other => Err(Error::Unexpected(format!("source de mods inconnue : « {other} »"))),
    }
}
