//! Client de l'API Satisfactory Mod Repository (ficsit.app / SMR) — GraphQL public.
//! Réseau côté cœur (hors CSP du webview), calqué sur `paste.rs`. Sert à parcourir les mods,
//! lister leurs versions (avec cible serveur + dépendances) et — à terme — télécharger les
//! artefacts pour une installation via SFTP (orchestration dans `lib.rs`).
//!
//! Le serveur MineStrator est un serveur dédié **Linux** : on ciblera donc les artefacts
//! `LinuxServer` lors de l'installation.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;

/// Endpoint GraphQL public de SMR.
const FICSIT_API: &str = "https://api.ficsit.app/v2/query";
/// Base des liens relatifs de download renvoyés par l'API (`/v1/version/…`).
const FICSIT_BASE: &str = "https://api.ficsit.app";
/// Cible de compilation d'un serveur dédié Linux (celui de MineStrator).
pub const SERVER_TARGET: &str = "LinuxServer";
/// Plafond de taille d'un artefact téléchargé (mod/SML) — garde-fou.
const MAX_ARTIFACT: u64 = 512 * 1024 * 1024;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

// --- Modèles (Deserialize depuis GraphQL, Serialize vers le front) ---------

/// Un mod du catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FicsitMod {
    pub id: String,
    pub mod_reference: String,
    pub name: String,
    #[serde(default)]
    pub short_description: String,
    #[serde(default)]
    pub downloads: i64,
    #[serde(default)]
    pub logo: String,
}

/// Page de résultats de recherche.
#[derive(Debug, Clone, Serialize)]
pub struct FicsitModPage {
    pub mods: Vec<FicsitMod>,
    pub count: i64,
}

/// Artefact d'une version pour une cible donnée (Windows / WindowsServer / LinuxServer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FicsitTarget {
    #[serde(rename(deserialize = "targetName"))]
    pub target_name: String,
    pub link: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub size: i64,
}

/// Dépendance d'une version (`mod_id` = mod_reference de la dépendance ; `SML` pour le loader).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FicsitDep {
    pub mod_id: String,
    pub condition: String,
    #[serde(default)]
    pub optional: bool,
}

/// Une version publiée d'un mod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FicsitVersion {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub sml_version: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub targets: Vec<FicsitTarget>,
    #[serde(default)]
    pub dependencies: Vec<FicsitDep>,
}

/// Artefact SML pour une cible (le lien pointe vers une release GitHub).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmlTarget {
    #[serde(rename(deserialize = "targetName"))]
    pub target_name: String,
    pub link: String,
}

/// Une version du Satisfactory Mod Loader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmlVersion {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub satisfactory_version: i64,
    #[serde(default)]
    pub targets: Vec<SmlTarget>,
}

/// Un mod présent dans le dossier `Mods/` du serveur (déduit du listing SFTP).
#[derive(Debug, Clone, Serialize)]
pub struct FicsitInstalledMod {
    /// Référence du mod = nom du dossier (ex. `RefinedPower`).
    pub reference: String,
    pub name: String,
    /// Un mod désactivé a son dossier suffixé `.disabled`.
    pub enabled: bool,
}

/// Un mod à installer (référence + version choisie) — élément d'une installation par lot.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FicsitInstallItem {
    pub reference: String,
    pub version_id: String,
}

/// Un artefact à déposer sur le serveur : le zip d'une cible `LinuxServer` (mod, dépendance ou SML),
/// extrait dans `Mods/<reference>/`.
#[derive(Debug, Clone)]
pub struct InstallArtifact {
    pub reference: String,
    /// Lien de download (relatif à ficsit pour un mod, absolu GitHub pour SML).
    pub link: String,
    /// sha256 hex attendu (mods) ; `None` pour SML (release GitHub).
    pub hash: Option<String>,
}

// --- Transport GraphQL ----------------------------------------------------

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

/// Exécute une requête GraphQL et renvoie la donnée typée.
async fn query<T: serde::de::DeserializeOwned>(
    gql: &str,
    variables: serde_json::Value,
) -> Result<T> {
    let resp = HTTP
        .post(FICSIT_API)
        .json(&serde_json::json!({ "query": gql, "variables": variables }))
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("ficsit.app : {e}")))?;
    let body: GqlResponse<T> = resp
        .json()
        .await
        .map_err(|e| Error::Unexpected(format!("réponse ficsit.app : {e}")))?;
    if let Some(err) = body.errors.first() {
        return Err(Error::Unexpected(format!("ficsit.app : {}", err.message)));
    }
    body.data
        .ok_or_else(|| Error::Unexpected("ficsit.app : réponse vide".into()))
}

// --- Requêtes -------------------------------------------------------------

/// Recherche paginée de mods. `search` vide (ou < 3 car.) = parcours trié par `order_by`.
/// `order_by` ∈ {popularity, hotness, downloads, views, name, updated_at, created_at,
/// last_version_date} ; `order` ∈ {asc, desc}.
pub async fn search_mods(
    search: &str,
    offset: i64,
    limit: i64,
    order_by: &str,
    order: &str,
) -> Result<FicsitModPage> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "getMods")]
        get_mods: Page,
    }
    #[derive(Deserialize)]
    struct Page {
        count: i64,
        mods: Vec<FicsitMod>,
    }
    let gql = r"query($search: String, $offset: Int, $limit: Int, $orderBy: ModFields, $order: Order) {
        getMods(filter: { search: $search, offset: $offset, limit: $limit, order_by: $orderBy, order: $order }) {
            count
            mods { id mod_reference name short_description downloads logo }
        }
    }";
    // L'API SMR exige un `search` d'au moins 3 caractères ; en deçà (ou vide) on l'omet
    // (`null`) pour parcourir le catalogue trié au lieu de déclencher une erreur de validation.
    let trimmed = search.trim();
    let search_val = if trimmed.chars().count() >= 3 {
        serde_json::Value::String(trimmed.to_string())
    } else {
        serde_json::Value::Null
    };
    let vars = serde_json::json!({
        "search": search_val, "offset": offset, "limit": limit,
        "orderBy": order_by, "order": order,
    });
    let data: Data = query(gql, vars).await?;
    Ok(FicsitModPage {
        mods: data.get_mods.mods,
        count: data.get_mods.count,
    })
}

/// Versions d'un mod (identifié par son `mod_id` ficsit), récentes d'abord.
pub async fn mod_versions(mod_id: &str) -> Result<Vec<FicsitVersion>> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "getMod")]
        get_mod: Option<ModVersions>,
    }
    #[derive(Deserialize)]
    struct ModVersions {
        versions: Vec<FicsitVersion>,
    }
    let gql = r"query($id: ModID!) {
        getMod(modId: $id) {
            versions {
                id version sml_version size hash link
                targets { targetName link hash size }
                dependencies { mod_id condition optional }
            }
        }
    }";
    let vars = serde_json::json!({ "id": mod_id });
    let data: Data = query(gql, vars).await?;
    Ok(data.get_mod.map(|m| m.versions).unwrap_or_default())
}

/// Versions du Satisfactory Mod Loader (récentes d'abord).
pub async fn sml_versions() -> Result<Vec<SmlVersion>> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "getSMLVersions")]
        get_sml_versions: SmlList,
    }
    #[derive(Deserialize)]
    struct SmlList {
        sml_versions: Vec<SmlVersion>,
    }
    let gql = r"query {
        getSMLVersions {
            sml_versions {
                id version satisfactory_version
                targets { targetName link }
            }
        }
    }";
    let data: Data = query(gql, serde_json::json!({})).await?;
    Ok(data.get_sml_versions.sml_versions)
}

/// Versions d'un mod par sa **référence** (`mod_reference`) — pour résoudre les dépendances.
pub async fn mod_versions_by_reference(reference: &str) -> Result<Vec<FicsitVersion>> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "getModByReference")]
        get_mod: Option<ModVersions>,
    }
    #[derive(Deserialize)]
    struct ModVersions {
        versions: Vec<FicsitVersion>,
    }
    let gql = r"query($ref: ModReference!) {
        getModByReference(modReference: $ref) {
            versions {
                id version sml_version size hash link
                targets { targetName link hash size }
                dependencies { mod_id condition optional }
            }
        }
    }";
    let vars = serde_json::json!({ "ref": reference });
    let data: Data = query(gql, vars).await?;
    Ok(data.get_mod.map(|m| m.versions).unwrap_or_default())
}

// --- Téléchargement + intégrité -------------------------------------------

/// Télécharge un artefact (mod/SML), plafonné, et vérifie le sha256 si `expected_hash` est fourni.
/// `link` peut être relatif (préfixé par [`FICSIT_BASE`]) ou absolu (release GitHub de SML).
pub async fn download(link: &str, expected_hash: Option<&str>) -> Result<Vec<u8>> {
    let url = if link.starts_with("http") {
        link.to_string()
    } else {
        format!("{FICSIT_BASE}{link}")
    };
    let resp = HTTP
        .get(&url)
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("téléchargement ficsit : {e}")))?
        .error_for_status()
        .map_err(|e| Error::Unexpected(format!("téléchargement ficsit : {e}")))?;
    if let Some(len) = resp.content_length() {
        if len > MAX_ARTIFACT {
            return Err(Error::Unexpected("artefact trop volumineux.".into()));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Unexpected(format!("lecture du téléchargement : {e}")))?;
    if bytes.len() as u64 > MAX_ARTIFACT {
        return Err(Error::Unexpected("artefact trop volumineux.".into()));
    }
    if let Some(expected) = expected_hash {
        let got = format!("{:x}", Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(expected) {
            return Err(Error::Unexpected(
                "empreinte sha256 de l'artefact invalide (téléchargement corrompu ?).".into(),
            ));
        }
    }
    Ok(bytes.to_vec())
}

// --- Résolution d'installation (mod + SML + dépendances) ------------------

/// Résout la liste des artefacts `LinuxServer` à installer pour un mod donné : le mod lui-même,
/// la version de SML compatible, et ses dépendances transitives non optionnelles (chacune à sa
/// dernière version satisfaisant la contrainte). Ordre = SML/deps avant le mod n'est pas garanti,
/// mais l'ordre d'écriture n'importe pas (dossiers indépendants).
pub async fn resolve_server_install(
    reference: &str,
    version_id: &str,
) -> Result<Vec<InstallArtifact>> {
    let mut out: Vec<InstallArtifact> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Version racine choisie par l'utilisateur.
    let root_versions = mod_versions_by_reference(reference).await?;
    let root = root_versions
        .iter()
        .find(|v| v.id == version_id)
        .ok_or_else(|| Error::Unexpected("version du mod introuvable.".into()))?;
    out.push(artifact_for(reference, root)?);
    seen.insert(reference.to_string());

    // SML compatible avec la contrainte du mod racine.
    let sml_req = if root.sml_version.is_empty() { "*" } else { &root.sml_version };
    let smls = sml_versions().await?;
    let sml = best_sml(&smls, sml_req)
        .ok_or_else(|| Error::Unexpected(format!("aucune version de SML ne satisfait « {sml_req} ».")))?;
    let sml_link = sml
        .targets
        .iter()
        .find(|t| t.target_name == SERVER_TARGET)
        .ok_or_else(|| Error::Unexpected("SML sans build serveur Linux.".into()))?
        .link
        .clone();
    out.push(InstallArtifact { reference: "SML".into(), link: sml_link, hash: None });
    seen.insert("SML".into());

    // Dépendances transitives (BFS), non optionnelles, hors SML.
    let mut queue: VecDeque<FicsitDep> = root.dependencies.iter().cloned().collect();
    while let Some(dep) = queue.pop_front() {
        if dep.optional || dep.mod_id == "SML" || seen.contains(&dep.mod_id) {
            continue;
        }
        seen.insert(dep.mod_id.clone());
        let versions = mod_versions_by_reference(&dep.mod_id).await?;
        let req = if dep.condition.is_empty() { "*" } else { &dep.condition };
        let picked = best_version(&versions, req).ok_or_else(|| {
            Error::Unexpected(format!(
                "aucune version de la dépendance « {} » ne satisfait « {} ».",
                dep.mod_id, req
            ))
        })?;
        out.push(artifact_for(&dep.mod_id, picked)?);
        queue.extend(picked.dependencies.iter().cloned());
    }
    Ok(out)
}

/// Construit l'artefact `LinuxServer` d'une version de mod (erreur claire si pas de build serveur).
fn artifact_for(reference: &str, v: &FicsitVersion) -> Result<InstallArtifact> {
    let target = v
        .targets
        .iter()
        .find(|t| t.target_name == SERVER_TARGET)
        .ok_or_else(|| {
            Error::Unexpected(format!(
                "« {reference} » n'a pas de build serveur Linux (mod client uniquement)."
            ))
        })?;
    Ok(InstallArtifact {
        reference: reference.to_string(),
        link: target.link.clone(),
        hash: if target.hash.is_empty() { None } else { Some(target.hash.clone()) },
    })
}

/// Dernière version (l'API renvoie récent d'abord) satisfaisant une contrainte semver.
fn best_version<'a>(versions: &'a [FicsitVersion], req: &str) -> Option<&'a FicsitVersion> {
    let req = semver::VersionReq::parse(req).ok()?;
    versions.iter().find(|v| {
        semver::Version::parse(&v.version)
            .map(|ver| req.matches(&ver))
            .unwrap_or(false)
    })
}

/// Dernière version de SML satisfaisant une contrainte semver.
fn best_sml<'a>(versions: &'a [SmlVersion], req: &str) -> Option<&'a SmlVersion> {
    let req = semver::VersionReq::parse(req).ok()?;
    versions.iter().find(|v| {
        semver::Version::parse(&v.version)
            .map(|ver| req.matches(&ver))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_version_with_targets_and_deps() {
        // Fixture calquée sur une réponse réelle de l'API SMR.
        let json = r#"{
            "id": "7yHvcbbrzQgkVg", "version": "1.0.0", "sml_version": "^3.12.0",
            "size": 26016946, "hash": "abc", "link": "/v1/version/7yHvcbbrzQgkVg/Windows/download",
            "targets": [
                { "targetName": "LinuxServer", "link": "/v1/version/7yHvcbbrzQgkVg/LinuxServer/download", "hash": "def", "size": 685760 }
            ],
            "dependencies": [ { "mod_id": "SML", "condition": "^3.12.0", "optional": false } ]
        }"#;
        let v: FicsitVersion = serde_json::from_str(json).unwrap();
        assert_eq!(v.version, "1.0.0");
        assert_eq!(v.sml_version, "^3.12.0");
        assert_eq!(v.targets[0].target_name, "LinuxServer");
        assert_eq!(v.targets[0].size, 685760);
        assert_eq!(v.dependencies[0].mod_id, "SML");
        assert!(!v.dependencies[0].optional);
    }

    #[test]
    fn version_defaults_tolerate_missing_fields() {
        let v: FicsitVersion =
            serde_json::from_str(r#"{ "id": "x", "version": "0.1.0" }"#).unwrap();
        assert!(v.targets.is_empty());
        assert!(v.dependencies.is_empty());
        assert_eq!(v.sml_version, "");
    }

    #[test]
    fn best_version_picks_latest_matching() {
        let mk = |v: &str| FicsitVersion {
            id: v.into(),
            version: v.into(),
            sml_version: String::new(),
            size: 0,
            hash: String::new(),
            link: String::new(),
            targets: vec![],
            dependencies: vec![],
        };
        // L'API renvoie les versions récentes d'abord.
        let versions = vec![mk("2.0.0"), mk("1.5.0"), mk("1.0.0")];
        assert_eq!(best_version(&versions, "^1.0.0").unwrap().version, "1.5.0");
        assert_eq!(best_version(&versions, "^2.0.0").unwrap().version, "2.0.0");
        assert!(best_version(&versions, "^3.0.0").is_none());
    }
}
