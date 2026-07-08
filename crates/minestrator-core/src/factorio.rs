//! Client du **Factorio Mod Portal** (`mods.factorio.com/api`). Read-path : listing/tri + versions
//! avec dépendances. API publique sans auth pour la navigation (le download nécessitera un token
//! factorio.com, géré à la phase install). Modèles normalisés dans [`crate::mods`].

use crate::error::{Error, Result};
use crate::mods::{get_bytes, get_bytes_bearer, get_json, MarketMod, MarketModPage, MarketModVersion};
use serde::Deserialize;
use sha1::{Digest, Sha1};

const BASE: &str = "https://mods.factorio.com/api";
const PAGE_SIZE: i64 = 20;
/// Plafond de taille d'un mod téléchargé.
const MAX_MOD_BYTES: u64 = 256 * 1024 * 1024;
/// Noms « intégrés » (jeu + DLC) : jamais des mods installables, à exclure des dépendances.
const BUILTINS: &[&str] = &["base", "space-age", "quality", "elevated-rails"];

/// `order` générique → `sort` Factorio. ⚠️ l'API ne trie PAS par téléchargements ; « popular »
/// retombe donc sur « mis à jour récemment » (fallback documenté).
fn sort(order: &str) -> (&'static str, &'static str) {
    match order {
        "recent" => ("created_at", "desc"),
        "name" => ("name", "asc"),
        _ => ("updated_at", "desc"),
    }
}

pub async fn search(query: &str, order: &str, page: i64) -> Result<MarketModPage> {
    #[derive(Deserialize)]
    struct Resp {
        pagination: Option<Pagination>,
        results: Vec<Item>,
    }
    #[derive(Deserialize)]
    struct Pagination {
        #[serde(default)]
        count: i64,
        #[serde(default)]
        page_count: i64,
    }
    #[derive(Deserialize)]
    struct Item {
        name: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        downloads_count: i64,
        #[serde(default)]
        thumbnail: String,
    }
    let (s, so) = sort(order);
    let page_s = page.to_string();
    let size_s = PAGE_SIZE.to_string();
    // L'API `/mods` n'a pas de recherche plein-texte ; on filtre côté client si `query` est fourni
    // (le portail officiel fait de même). Sans query : listing paginé trié.
    let data: Resp = get_json(
        &format!("{BASE}/mods"),
        &[
            ("page", &page_s),
            ("page_size", &size_s),
            ("sort", s),
            ("sort_order", so),
        ],
    )
    .await?;

    let q = query.trim().to_lowercase();
    let mods: Vec<MarketMod> = data
        .results
        .into_iter()
        .filter(|i| {
            q.is_empty()
                || i.name.to_lowercase().contains(&q)
                || i.title.to_lowercase().contains(&q)
        })
        .map(|i| MarketMod {
            reference: i.name,
            name: if i.title.is_empty() { String::new() } else { i.title },
            description: i.summary,
            downloads: i.downloads_count,
            icon_url: if i.thumbnail.is_empty() {
                String::new()
            } else {
                format!("https://assets-mod.factorio.com{}", i.thumbnail)
            },
            source: "factorio".into(),
        })
        .collect();

    let pag = data.pagination.unwrap_or(Pagination { count: 0, page_count: 0 });
    Ok(MarketModPage {
        mods,
        count: pag.count,
        has_more: page < pag.page_count,
    })
}

pub async fn mod_versions(reference: &str) -> Result<Vec<MarketModVersion>> {
    // ⚠️ `name` est sensible à la casse côté API.
    #[derive(Deserialize)]
    struct Full {
        #[serde(default)]
        releases: Vec<Release>,
    }
    #[derive(Deserialize)]
    struct Release {
        version: String,
        #[serde(default)]
        info_json: InfoJson,
    }
    #[derive(Deserialize, Default)]
    struct InfoJson {
        #[serde(default)]
        factorio_version: String,
        #[serde(default)]
        dependencies: Vec<String>,
    }
    let full: Full = get_json(&format!("{BASE}/mods/{reference}/full"), &[]).await?;
    // `releases` est trié du plus ancien au plus récent → on inverse.
    let mut out: Vec<MarketModVersion> = full
        .releases
        .into_iter()
        .rev()
        .map(|r| MarketModVersion {
            version: r.version,
            game_version: r.info_json.factorio_version,
            dependencies: r
                .info_json
                .dependencies
                .iter()
                .filter(|d| !d.trim_start().starts_with('!')) // conflits : pas des dépendances
                .filter(|d| !BUILTINS.contains(&dep_name(d)))
                .cloned()
                .collect(),
        })
        .collect();
    out.truncate(50);
    Ok(out)
}

/// Extrait le nom du mod d'une chaîne de dépendance Factorio (`"? flib >= 0.17"` → `"flib"`).
fn dep_name(dep: &str) -> &str {
    let d = dep.trim_start_matches(['!', '?', '~', '+', '(', ')', ' ']);
    d.split([' ', '<', '>', '='])
        .next()
        .unwrap_or(d)
        .trim()
}

// --- Installation (résolution + download + mod-list.json) ------------------

/// Un artefact Factorio à télécharger puis déposer dans `mods/`.
#[derive(Debug, Clone)]
pub struct FactorioArtifact {
    pub reference: String,
    pub download_url: String,
    pub file_name: String,
    pub sha1: String,
}

#[derive(Deserialize, Clone)]
struct Release {
    version: String,
    #[serde(default)]
    download_url: String,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    sha1: String,
    #[serde(default)]
    info_json: RelInfo,
}
#[derive(Deserialize, Default, Clone)]
struct RelInfo {
    #[serde(default)]
    dependencies: Vec<String>,
}

async fn fetch_releases(name: &str) -> Result<Vec<Release>> {
    #[derive(Deserialize)]
    struct Full {
        #[serde(default)]
        releases: Vec<Release>,
    }
    let full: Full = get_json(&format!("{BASE}/mods/{name}/full"), &[]).await?;
    Ok(full.releases)
}

fn artifact(reference: &str, r: &Release) -> FactorioArtifact {
    FactorioArtifact {
        reference: reference.to_string(),
        download_url: r.download_url.clone(),
        file_name: r.file_name.clone(),
        sha1: r.sha1.clone(),
    }
}

/// Version « x.y » → « x.y.0 » pour parser en semver.
fn norm_ver(v: &str) -> String {
    match v.split('.').count() {
        1 => format!("{v}.0.0"),
        2 => format!("{v}.0"),
        _ => v.to_string(),
    }
}

/// Parse une dépendance en (nom, contrainte). `None` = à ignorer (conflit `!`, optionnelle `? (?) +`,
/// ou jeu/DLC). Les requises (aucun préfixe) et `~` sont conservées.
fn parse_dep(dep: &str) -> Option<(String, semver::VersionReq)> {
    let d = dep.trim();
    if d.starts_with('!') || d.starts_with('?') || d.starts_with('(') || d.starts_with('+') {
        return None;
    }
    let d = d.trim_start_matches('~').trim();
    let name = dep_name(d);
    if name.is_empty() || BUILTINS.contains(&name) {
        return None;
    }
    let rest = d[name.len()..].trim();
    let req = if rest.is_empty() {
        semver::VersionReq::STAR
    } else {
        semver::VersionReq::parse(&norm_ver(&rest.replace(' ', ""))).unwrap_or(semver::VersionReq::STAR)
    };
    Some((name.to_string(), req))
}

/// Dernière release (l'API renvoie ancien→récent) satisfaisant la contrainte.
fn best_release(rels: &[Release], req: &semver::VersionReq) -> Option<Release> {
    rels.iter()
        .rev()
        .find(|r| {
            semver::Version::parse(&norm_ver(&r.version))
                .map(|v| req.matches(&v))
                .unwrap_or(false)
        })
        .cloned()
}

/// Résout un mod (`version` vide = dernière) + ses dépendances REQUISES transitives, en excluant le
/// jeu et les DLC. Chaque dépendance = dernière release satisfaisant la contrainte.
pub async fn resolve_install(reference: &str, version: &str) -> Result<Vec<FactorioArtifact>> {
    use std::collections::{HashSet, VecDeque};
    let mut out: Vec<FactorioArtifact> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, semver::VersionReq)> = VecDeque::new();

    let root_rels = fetch_releases(reference).await?;
    let root = if version.is_empty() {
        root_rels.last().cloned()
    } else {
        root_rels.iter().find(|r| r.version == version).cloned()
    }
    .ok_or_else(|| Error::Unexpected(format!("version « {version} » introuvable pour {reference}.")))?;
    out.push(artifact(reference, &root));
    seen.insert(reference.to_string());
    queue.extend(root.info_json.dependencies.iter().filter_map(|d| parse_dep(d)));

    while let Some((name, req)) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let rels = fetch_releases(&name).await?;
        let rel = best_release(&rels, &req).ok_or_else(|| {
            Error::Unexpected(format!("aucune version de la dépendance « {name} » n'est compatible."))
        })?;
        out.push(artifact(&name, &rel));
        queue.extend(rel.info_json.dependencies.iter().filter_map(|d| parse_dep(d)));
    }
    Ok(out)
}

/// Télécharge le zip d'un artefact et vérifie l'intégrité (magic zip + sha1). Deux méthodes d'auth
/// factorio.com sont tentées : d'abord `?username=&token=` (service-token de `player-data.json`), puis
/// en repli `Authorization: Bearer` (clé d'API du portail avec l'usage « Download mods »).
pub async fn download(art: &FactorioArtifact, username: &str, token: &str) -> Result<Vec<u8>> {
    let url = format!("https://mods.factorio.com{}", art.download_url);
    let via_query = get_bytes(&url, &[("username", username), ("token", token)], MAX_MOD_BYTES).await;
    let bytes = match via_query {
        Ok(b) => b,
        Err(first) => {
            let detail = first.to_string();
            // Cas très fréquent : compte factorio.com sans licence du jeu. Le portail réserve le
            // téléchargement (hors parcours) aux comptes propriétaires → message dédié, non trompeur.
            if detail.to_lowercase().contains("owning the game") {
                return Err(Error::Unexpected(
                    "Ton compte factorio.com ne possède pas le jeu : le Mod Portal réserve le \
                     TÉLÉCHARGEMENT de mods aux comptes propriétaires de Factorio (le parcours du \
                     catalogue, lui, reste libre). Active/achète Factorio sur ce compte, ou renseigne \
                     dans Paramètres → Jeux un compte qui possède le jeu."
                        .into(),
                ));
            }
            // Repli : clé d'API du portail (usage « Download mods ») en Bearer.
            get_bytes_bearer(&url, token, MAX_MOD_BYTES).await.map_err(|_| {
                Error::Unexpected(format!(
                    "Téléchargement Factorio refusé. Dans Paramètres → Jeux, vérifie que le « username » \
                     est bien ton identifiant de compte factorio.com (sensible à la casse) et que le \
                     « token » est le service-token de ton player-data.json (ou une clé d'API du portail \
                     avec l'usage « Download mods »). Détail de l'API : {detail}"
                ))
            })?
        }
    };
    // Réponse non-zip (page HTML de login) = auth acceptée mais mauvais contenu → refus explicite.
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        return Err(Error::Unexpected(
            "Téléchargement Factorio refusé — réponse inattendue (vérifie username/token dans Paramètres → Jeux).".into(),
        ));
    }
    if !art.sha1.is_empty() {
        let got = format!("{:x}", Sha1::digest(&bytes));
        if !got.eq_ignore_ascii_case(&art.sha1) {
            return Err(Error::Unexpected("Empreinte sha1 invalide (téléchargement corrompu ?).".into()));
        }
    }
    Ok(bytes)
}

// --- mod-list.json (liste blanche d'activation) ----------------------------

fn d_true() -> bool {
    true
}

fn parse_list(json: &str) -> Vec<(String, bool)> {
    #[derive(Deserialize)]
    struct L {
        #[serde(default)]
        mods: Vec<E>,
    }
    #[derive(Deserialize)]
    struct E {
        name: String,
        #[serde(default = "d_true")]
        enabled: bool,
    }
    serde_json::from_str::<L>(json)
        .map(|l| l.mods.into_iter().map(|e| (e.name, e.enabled)).collect())
        .unwrap_or_default()
}

fn serialize_list(entries: &[(String, bool)]) -> String {
    let mods: Vec<serde_json::Value> = entries
        .iter()
        .map(|(n, e)| serde_json::json!({ "name": n, "enabled": e }))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "mods": mods }))
        .unwrap_or_else(|_| "{\"mods\":[{\"name\":\"base\",\"enabled\":true}]}".to_string())
}

fn upsert(entries: &mut Vec<(String, bool)>, name: &str, enabled: bool) {
    if let Some(e) = entries.iter_mut().find(|(n, _)| n == name) {
        e.1 = enabled;
    } else {
        entries.push((name.to_string(), enabled));
    }
}

/// Mods installés listés dans `mod-list.json` (hors jeu/DLC), avec leur état activé.
pub fn mod_list_entries(json: &str) -> Vec<(String, bool)> {
    parse_list(json)
        .into_iter()
        .filter(|(n, _)| !BUILTINS.contains(&n.as_str()))
        .collect()
}

/// `mod-list.json` mis à jour : `base` activé + chaque mod de `add` présent et activé (conserve le reste).
pub fn mod_list_add(existing: Option<&str>, add: &[String]) -> String {
    let mut entries = existing.map(parse_list).unwrap_or_default();
    upsert(&mut entries, "base", true);
    for n in add {
        upsert(&mut entries, n, true);
    }
    serialize_list(&entries)
}

pub fn mod_list_set_enabled(existing: &str, name: &str, enabled: bool) -> String {
    let mut entries = parse_list(existing);
    upsert(&mut entries, name, enabled);
    serialize_list(&entries)
}

pub fn mod_list_remove(existing: &str, name: &str) -> String {
    let mut entries = parse_list(existing);
    entries.retain(|(n, _)| n != name);
    serialize_list(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_factorio_deps() {
        // Requise, optionnelle, conflit, ~, DLC → seules les requises hors DLC sont gardées.
        assert!(parse_dep("! Annotorio").is_none());
        assert!(parse_dep("? bullet-trails >= 0.7.0").is_none());
        assert!(parse_dep("(?) quality").is_none());
        assert!(parse_dep("+ ChangeInserterDropLane").is_none());
        assert!(parse_dep("base >= 2.1.0").is_none()); // jeu
        let (n, _) = parse_dep("flib >= 0.17.0").unwrap();
        assert_eq!(n, "flib");
        let (n, _) = parse_dep("~ space-exploration-postprocess >= 0.7.5").unwrap();
        assert_eq!(n, "space-exploration-postprocess");
    }

    #[test]
    fn mod_list_round_trip() {
        let j = mod_list_add(None, &["RateCalculator".to_string()]);
        let entries = mod_list_entries(&j);
        assert_eq!(entries, vec![("RateCalculator".to_string(), true)]); // base filtré
        let j = mod_list_set_enabled(&j, "RateCalculator", false);
        assert_eq!(mod_list_entries(&j), vec![("RateCalculator".to_string(), false)]);
        let j = mod_list_remove(&j, "RateCalculator");
        assert!(mod_list_entries(&j).is_empty());
    }
}
