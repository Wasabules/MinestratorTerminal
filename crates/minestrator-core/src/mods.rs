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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GameSettings {
    #[serde(default)]
    pub factorio: FactorioSettings,
}

/// Réglages Factorio : le `username` du compte factorio.com (le token de download va au trousseau).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
/// muet). Extrait ~400 caractères, espaces normalisés (voir [`http_error_message`]).
async fn fetch_bytes(req: reqwest::RequestBuilder, max: u64) -> Result<Vec<u8>> {
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("téléchargement : {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Unexpected(http_error_message(status.as_u16(), &body)));
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

/// Message d'un téléchargement refusé : le statut, suivi du début du texte lisible du corps (espaces
/// normalisés, 400 caractères au plus) s'il y en a un.
fn http_error_message(status: u16, body: &str) -> String {
    // Les pages d'erreur sont souvent en HTML → on retire les balises pour exposer le texte lisible.
    let text = strip_tags(body);
    let snippet: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect();
    if snippet.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status} — {snippet}")
    }
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MarketModPage {
    pub mods: Vec<MarketMod>,
    pub count: i64,
    pub has_more: bool,
}

/// Une version publiée d'un mod (normalisée).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MarketModVersion {
    pub version: String,
    /// Contrainte/compatibilité de version de jeu (SML, `factorio_version`…), ou vide.
    pub game_version: String,
    /// Dépendances lisibles (hors loader/framework), pour affichage.
    pub dependencies: Vec<String>,
}

/// Un mod installé sur le serveur (normalisé), avec son état activé.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MarketInstalledMod {
    pub reference: String,
    pub name: String,
    pub enabled: bool,
}

/// Un mod à installer (référence + version voulue ; version vide = dernière).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_tags_keeps_only_text() {
        assert_eq!(
            strip_tags("<html><body><h1>403</h1><p>Forbidden</p></body></html>"),
            "403Forbidden"
        );
        assert_eq!(strip_tags("pas de balise"), "pas de balise");
        assert_eq!(strip_tags("a <b>gras</b> é"), "a gras é");
        assert_eq!(strip_tags(""), "");
    }

    #[test]
    fn http_error_message_exposes_the_readable_body() {
        // Page HTML : balises retirées, espaces/retours à la ligne normalisés.
        assert_eq!(
            http_error_message(
                403,
                "<html>\n  <body>\n    <h1>Accès refusé</h1>\n    <p>Token   invalide.</p>\n  </body>\n</html>"
            ),
            "HTTP 403 — Accès refusé Token invalide."
        );
        // Balises collées sans blanc : les textes se touchent (retrait « grossier » assumé).
        assert_eq!(http_error_message(403, "<b>Accès</b><i>refusé</i>"), "HTTP 403 — Accèsrefusé");
        // Corps JSON : conservé tel quel.
        assert_eq!(
            http_error_message(401, r#"{"message":"Bad credentials"}"#),
            r#"HTTP 401 — {"message":"Bad credentials"}"#
        );
        // Corps vide ou sans texte → statut seul.
        assert_eq!(http_error_message(404, ""), "HTTP 404");
        assert_eq!(http_error_message(500, "<html> <br/> </html>"), "HTTP 500");
    }

    #[test]
    fn http_error_message_is_capped_at_400_characters() {
        let msg = http_error_message(502, &"é".repeat(1000));
        let snippet = msg.strip_prefix("HTTP 502 — ").unwrap();
        assert_eq!(snippet.chars().count(), 400);
    }

    #[test]
    fn game_settings_tolerate_missing_fields() {
        let s: GameSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(s.factorio.username, "");
        let s: GameSettings = serde_json::from_value(json!({ "factorio": {} })).unwrap();
        assert_eq!(s.factorio.username, "");
        let s: GameSettings =
            serde_json::from_value(json!({ "factorio": { "username": "steve" }, "autre": 1 })).unwrap();
        assert_eq!(s.factorio.username, "steve");
    }

    #[test]
    fn install_item_version_defaults_to_latest() {
        let items: Vec<ModInstallItem> = serde_json::from_value(json!([
            { "reference": "Azumatt/AzuCraftyBoxes" },
            { "reference": "Vanish", "version": "1.2.3" }
        ]))
        .unwrap();
        assert_eq!(items[0].version, "", "version vide = dernière");
        assert_eq!(items[1].reference, "Vanish");
        assert_eq!(items[1].version, "1.2.3");
        assert!(serde_json::from_value::<ModInstallItem>(json!({ "version": "1" })).is_err());
    }

    #[test]
    fn normalized_models_serialize_with_stable_keys() {
        let page = MarketModPage {
            mods: vec![MarketMod {
                reference: "ns/name".into(),
                name: "name".into(),
                description: "d".into(),
                downloads: 12,
                icon_url: "https://x/i.png".into(),
                source: "thunderstore".into(),
            }],
            count: 1,
            has_more: false,
        };
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            json!({
                "mods": [{
                    "reference": "ns/name", "name": "name", "description": "d",
                    "downloads": 12, "icon_url": "https://x/i.png", "source": "thunderstore"
                }],
                "count": 1,
                "has_more": false
            })
        );
    }

    #[tokio::test]
    async fn unknown_source_is_rejected_without_network() {
        match search("nexus", "", "", "popular", 1).await {
            Err(Error::Unexpected(m)) => assert_eq!(m, "source de mods inconnue : « nexus »"),
            other => panic!("{other:?}"),
        }
        match mod_versions("nexus", "x").await {
            Err(Error::Unexpected(m)) => assert_eq!(m, "source de mods inconnue : « nexus »"),
            other => panic!("{other:?}"),
        }
    }
}
