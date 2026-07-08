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
    Ok(out)
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
