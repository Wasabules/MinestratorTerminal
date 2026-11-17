//! Client **uMod** (umod.org / assets.umod.org) — plugins Oxide/Carbon pour **Rust**.
//! Read-path : recherche via `search.json` (catégorie `rust`), détail/versions via le CDN statique
//! `assets.umod.org` (fiable). Pas de résolution de dépendances côté API (limite connue).
//! Modèles normalisés dans [`crate::mods`].

use crate::error::Result;
use crate::mods::{get_json, MarketMod, MarketModPage, MarketModVersion};
use serde::Deserialize;

pub async fn search(query: &str, page: i64) -> Result<MarketModPage> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        data: Vec<Item>,
        #[serde(default)]
        total: i64,
        #[serde(default)]
        last_page: i64,
        #[serde(default)]
        current_page: i64,
    }
    #[derive(Deserialize)]
    struct Item {
        #[serde(default)]
        name: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        downloads: i64,
        #[serde(default)]
        icon_url: String,
        #[serde(default)]
        download_url: String,
    }
    let page_s = page.to_string();
    let data: Resp = get_json(
        "https://umod.org/plugins/search.json",
        &[
            ("query", query),
            ("page", &page_s),
            ("sort", "latest_release_at"),
            ("sortdir", "desc"),
            ("categories[]", "rust"),
        ],
    )
    .await?;
    let mods = data
        .data
        .into_iter()
        .map(|i| MarketMod {
            // Le fichier .cs / le détail CDN utilisent le Nom en TitleCase → on le tire du download_url.
            reference: ref_from_url(&i.download_url, &i.name),
            name: if i.title.is_empty() { i.name } else { i.title },
            description: i.description,
            downloads: i.downloads,
            icon_url: i.icon_url,
            source: "umod".into(),
        })
        .collect();
    let last = data.last_page.max(1);
    Ok(MarketModPage {
        mods,
        count: data.total,
        has_more: data.current_page < last,
    })
}

pub async fn mod_versions(reference: &str) -> Result<Vec<MarketModVersion>> {
    // Détail CDN : structure défensive (le schéma exact varie) — on extrait les versions trouvées.
    let v: serde_json::Value = get_json(
        &format!("https://assets.umod.org/plugins/{reference}.json"),
        &[],
    )
    .await?;
    Ok(versions_from_manifest(&v))
}

/// Versions trouvées dans le manifeste CDN d'un plugin, quelle que soit sa forme.
fn versions_from_manifest(v: &serde_json::Value) -> Vec<MarketModVersion> {
    let mut out: Vec<MarketModVersion> = Vec::new();
    let push = |out: &mut Vec<MarketModVersion>, ver: &str| {
        let ver = ver.trim_start_matches('v').trim();
        if !ver.is_empty() {
            out.push(MarketModVersion {
                version: ver.to_string(),
                game_version: String::new(),
                dependencies: Vec::new(),
            });
        }
    };
    // Formes possibles : tableau `versions`/`branches`, ou objet map de versions.
    for key in ["versions", "branches", "releases"] {
        if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
            for it in arr {
                if let Some(s) = it
                    .get("version")
                    .or_else(|| it.get("formatted"))
                    .and_then(|x| x.as_str())
                {
                    push(&mut out, s);
                }
            }
        } else if let Some(map) = v.get(key).and_then(|x| x.as_object()) {
            for it in map.values() {
                if let Some(s) = it
                    .get("version")
                    .or_else(|| it.get("formatted"))
                    .and_then(|x| x.as_str())
                {
                    push(&mut out, s);
                }
            }
        }
    }
    // Repli : le champ « dernière version » du manifeste.
    if out.is_empty() {
        if let Some(s) = v
            .get("latest_release_version")
            .or_else(|| v.get("version"))
            .and_then(|x| x.as_str())
        {
            push(&mut out, s);
        }
    }
    out
}

/// Nom de plugin (TitleCase) à partir de l'URL de download `.../Vanish.cs`, repli sur `fallback`.
fn ref_from_url(url: &str, fallback: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|f| f.strip_suffix(".cs"))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn versions(v: serde_json::Value) -> Vec<String> {
        versions_from_manifest(&v).into_iter().map(|m| m.version).collect()
    }

    #[test]
    fn reference_comes_from_the_download_url() {
        assert_eq!(ref_from_url("https://umod.org/plugins/Vanish.cs", "vanish"), "Vanish");
        assert_eq!(
            ref_from_url("https://umod.org/plugins/BetterChat.cs", "better-chat"),
            "BetterChat"
        );
        // Pas de `.cs` exploitable → repli sur le nom de l'API.
        assert_eq!(ref_from_url("", "vanish"), "vanish");
        assert_eq!(ref_from_url("https://umod.org/plugins/", "vanish"), "vanish");
        assert_eq!(ref_from_url("https://umod.org/plugins/.cs", "vanish"), "vanish");
        assert_eq!(ref_from_url("https://umod.org/plugins/Vanish.zip", "vanish"), "vanish");
    }

    #[test]
    fn versions_from_an_array() {
        let v = json!({
            "name": "Vanish",
            "versions": [
                { "version": "v2.1.0", "released_at": "2026-01-01" },
                { "formatted": "2.0.0" },
                { "version": "   " },
                { "autre": "x" }
            ]
        });
        // Préfixe `v` retiré ; entrées vides ou sans version ignorées.
        assert_eq!(versions(v), ["2.1.0", "2.0.0"]);
    }

    #[test]
    fn versions_from_a_map_and_several_keys() {
        let mut found = versions(json!({
            "branches": { "master": { "version": "1.4.0" }, "beta": { "formatted": "1.5.0-beta" } }
        }));
        found.sort();
        assert_eq!(found, ["1.4.0", "1.5.0-beta"]);

        // Plusieurs formes présentes : tout est collecté, dans l'ordre versions → branches → releases.
        assert_eq!(
            versions(json!({
                "versions": [{ "version": "3.0.0" }],
                "releases": [{ "version": "2.9.0" }]
            })),
            ["3.0.0", "2.9.0"]
        );
    }

    #[test]
    fn falls_back_to_the_latest_version_field() {
        assert_eq!(
            versions(json!({ "latest_release_version": "v1.0.3", "version": "0.9" })),
            ["1.0.3"]
        );
        assert_eq!(versions(json!({ "version": "0.9" })), ["0.9"]);
        // Le repli ne sert QUE si aucune version n'a été trouvée ailleurs.
        assert_eq!(
            versions(json!({ "versions": [{ "version": "2.0" }], "latest_release_version": "9.9" })),
            ["2.0"]
        );
        assert!(versions(json!({})).is_empty());
        assert!(versions(json!({ "versions": [] })).is_empty());
    }

    #[test]
    fn versions_carry_no_game_version_nor_dependencies() {
        let v = versions_from_manifest(&json!({ "version": "1.0" }));
        assert_eq!(v[0].game_version, "");
        assert!(v[0].dependencies.is_empty());
    }
}
