//! Client Thunderstore (thunderstore.io) — mods BepInEx pour **Valheim** et **V Rising**.
//! Read-path : recherche via l'API Cyberstorm (joignable serveur, pagination + tri), versions +
//! dépendances via l'API experimental/package. Modèles normalisés dans [`crate::mods`].

use crate::error::{Error, Result};
use crate::mods::{get_json, MarketMod, MarketModPage, MarketModVersion};
use serde::Deserialize;

const BASE: &str = "https://thunderstore.io";

/// Communauté Thunderstore correspondant à la famille de jeu.
fn community(family: &str) -> &'static str {
    match family {
        "v_rising" => "v-rising",
        _ => "valheim",
    }
}

/// `order` (générique : popular/recent/rating/name) → `ordering` Thunderstore.
fn ordering(order: &str) -> &'static str {
    match order {
        "recent" => "newest",
        "rating" => "top-rated",
        "name" => "last-updated", // pas de tri par nom → repli raisonnable
        _ => "most-downloaded",    // « popular » (défaut)
    }
}

pub async fn search(family: &str, query: &str, order: &str, page: i64) -> Result<MarketModPage> {
    #[derive(Deserialize)]
    struct Resp {
        count: i64,
        next: Option<String>,
        results: Vec<Card>,
    }
    #[derive(Deserialize)]
    struct Card {
        namespace: String,
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        download_count: i64,
        #[serde(default)]
        icon_url: String,
    }
    let url = format!("{BASE}/api/cyberstorm/listing/{}/", community(family));
    let page_s = page.to_string();
    let data: Resp = get_json(
        &url,
        &[("q", query), ("ordering", ordering(order)), ("page", &page_s)],
    )
    .await?;
    let mods = data
        .results
        .into_iter()
        .map(|c| MarketMod {
            reference: format!("{}/{}", c.namespace, c.name),
            name: c.name,
            description: c.description,
            downloads: c.download_count,
            icon_url: c.icon_url,
            source: "thunderstore".into(),
        })
        .collect();
    Ok(MarketModPage {
        mods,
        count: data.count,
        has_more: data.next.is_some(),
    })
}

pub async fn mod_versions(reference: &str) -> Result<Vec<MarketModVersion>> {
    let (ns, name) = reference
        .split_once('/')
        .ok_or_else(|| Error::Unexpected("référence Thunderstore invalide (attendu ns/name)".into()))?;

    // Version la plus récente + ses dépendances (API experimental).
    #[derive(Deserialize)]
    struct Pkg {
        latest: Ver,
    }
    #[derive(Deserialize)]
    struct Ver {
        version_number: String,
        #[serde(default)]
        dependencies: Vec<String>,
    }
    let purl = format!("{BASE}/api/experimental/package/{ns}/{name}/");
    let pkg: Pkg = get_json(&purl, &[]).await?;
    // Le pack BepInEx est une dépendance « technique » (loader) → masqué de l'affichage.
    let deps: Vec<String> = pkg
        .latest
        .dependencies
        .iter()
        .filter(|d| !d.contains("BepInExPack"))
        .cloned()
        .collect();

    // Liste complète des versions (API Cyberstorm) ; deps affichées sur la plus récente.
    #[derive(Deserialize)]
    struct VerItem {
        version_number: String,
    }
    let vurl = format!("{BASE}/api/cyberstorm/package/{ns}/{name}/versions/");
    let list: Vec<VerItem> = get_json(&vurl, &[]).await.unwrap_or_default();

    if list.is_empty() {
        return Ok(vec![MarketModVersion {
            version: pkg.latest.version_number,
            game_version: String::new(),
            dependencies: deps,
        }]);
    }
    Ok(list
        .into_iter()
        .map(|v| {
            let latest = v.version_number == pkg.latest.version_number;
            MarketModVersion {
                version: v.version_number,
                game_version: String::new(),
                dependencies: if latest { deps.clone() } else { Vec::new() },
            }
        })
        .collect())
}
