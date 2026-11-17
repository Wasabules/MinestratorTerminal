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

/// Page de résultats de l'API Cyberstorm (`/api/cyberstorm/listing/…`).
#[derive(Deserialize)]
struct ListingResp {
    count: i64,
    next: Option<String>,
    results: Vec<ListingCard>,
}

#[derive(Deserialize)]
struct ListingCard {
    namespace: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    download_count: i64,
    #[serde(default)]
    icon_url: String,
}

pub async fn search(family: &str, query: &str, order: &str, page: i64) -> Result<MarketModPage> {
    let url = format!("{BASE}/api/cyberstorm/listing/{}/", community(family));
    let page_s = page.to_string();
    let data: ListingResp = get_json(
        &url,
        &[("q", query), ("ordering", ordering(order)), ("page", &page_s)],
    )
    .await?;
    Ok(page_from_listing(data))
}

/// Page Cyberstorm → page normalisée (référence `namespace/nom`).
fn page_from_listing(data: ListingResp) -> MarketModPage {
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
    MarketModPage {
        mods,
        count: data.count,
        has_more: data.next.is_some(),
    }
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

    // Liste complète des versions (API Cyberstorm) ; deps affichées sur la plus récente.
    #[derive(Deserialize)]
    struct VerItem {
        version_number: String,
    }
    let vurl = format!("{BASE}/api/cyberstorm/package/{ns}/{name}/versions/");
    let list: Vec<VerItem> = get_json(&vurl, &[]).await.unwrap_or_default();

    Ok(merge_versions(
        pkg.latest.version_number,
        &pkg.latest.dependencies,
        list.into_iter().map(|v| v.version_number).collect(),
    ))
}

/// Fusionne la version la plus récente (API experimental, seule à porter les dépendances) et la
/// liste complète des versions (API Cyberstorm) : les dépendances ne sont affichées que sur la plus
/// récente. Liste vide (API indisponible) → la seule version la plus récente.
fn merge_versions(latest: String, latest_deps: &[String], all: Vec<String>) -> Vec<MarketModVersion> {
    // Le pack BepInEx est une dépendance « technique » (loader) → masqué de l'affichage.
    let deps: Vec<String> = latest_deps
        .iter()
        .filter(|d| !d.contains("BepInExPack"))
        .cloned()
        .collect();

    if all.is_empty() {
        return vec![MarketModVersion {
            version: latest,
            game_version: String::new(),
            dependencies: deps,
        }];
    }
    all.into_iter()
        .map(|version| {
            let is_latest = version == latest;
            MarketModVersion {
                version,
                game_version: String::new(),
                dependencies: if is_latest { deps.clone() } else { Vec::new() },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn community_and_ordering_mappings() {
        assert_eq!(community("v_rising"), "v-rising");
        assert_eq!(community("valheim"), "valheim");
        assert_eq!(community("inconnu"), "valheim");

        assert_eq!(ordering("popular"), "most-downloaded");
        assert_eq!(ordering(""), "most-downloaded");
        assert_eq!(ordering("recent"), "newest");
        assert_eq!(ordering("rating"), "top-rated");
        assert_eq!(ordering("name"), "last-updated");
    }

    #[test]
    fn listing_page_is_normalized() {
        let data: ListingResp = serde_json::from_value(json!({
            "count": 2,
            "next": "https://thunderstore.io/api/cyberstorm/listing/valheim/?page=2",
            "previous": null,
            "results": [
                {
                    "namespace": "denikson", "name": "BepInExPack_Valheim",
                    "description": "BepInEx pack", "download_count": 9000000,
                    "icon_url": "https://gcdn.thunderstore.io/i.png", "rating_count": 1
                },
                { "namespace": "Azumatt", "name": "AzuCraftyBoxes" }
            ]
        }))
        .unwrap();
        let page = page_from_listing(data);
        assert_eq!(page.count, 2);
        assert!(page.has_more);
        assert_eq!(page.mods.len(), 2);
        let m = &page.mods[0];
        assert_eq!(m.reference, "denikson/BepInExPack_Valheim");
        assert_eq!(m.name, "BepInExPack_Valheim");
        assert_eq!(m.downloads, 9_000_000);
        assert_eq!(m.source, "thunderstore");
        // Champs optionnels absents → valeurs par défaut.
        let m = &page.mods[1];
        assert_eq!(m.reference, "Azumatt/AzuCraftyBoxes");
        assert_eq!((m.description.as_str(), m.downloads, m.icon_url.as_str()), ("", 0, ""));

        let last: ListingResp =
            serde_json::from_value(json!({ "count": 0, "next": null, "results": [] })).unwrap();
        assert!(!page_from_listing(last).has_more);
    }

    #[test]
    fn dependencies_only_on_the_latest_version_without_bepinex() {
        let deps = vec![
            "denikson-BepInExPack_Valheim-5.4.2202".to_string(),
            "ValheimModding-Jotunn-2.20.0".to_string(),
        ];
        let all = vec!["1.2.0".to_string(), "1.1.0".to_string(), "1.0.0".to_string()];
        let v = merge_versions("1.2.0".into(), &deps, all);
        let names: Vec<&str> = v.iter().map(|m| m.version.as_str()).collect();
        assert_eq!(names, ["1.2.0", "1.1.0", "1.0.0"]);
        assert_eq!(v[0].dependencies, ["ValheimModding-Jotunn-2.20.0"]);
        assert!(v[1].dependencies.is_empty());
        assert!(v[2].dependencies.is_empty());
        assert!(v.iter().all(|m| m.game_version.is_empty()));
    }

    #[test]
    fn missing_version_list_falls_back_to_the_latest() {
        let v = merge_versions("3.0.1".into(), &["a-B-1.0.0".to_string()], Vec::new());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].version, "3.0.1");
        assert_eq!(v[0].dependencies, ["a-B-1.0.0"]);
    }

    #[tokio::test]
    async fn invalid_reference_fails_without_network() {
        match mod_versions("sans-barre").await {
            Err(Error::Unexpected(m)) => assert!(m.contains("ns/name"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
