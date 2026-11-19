//! Voie « JAR » de `set_server_platform` : plateforme absente du catalogue de l'hébergeur (ou
//! version précise qu'il ne propose pas), installée en envoyant le JAR officiel sur le serveur puis
//! en le choisissant comme fichier de démarrage (`set_startup_config file`).
//!
//! Sources, interrogées en lecture et sans clé :
//! - **PaperMC** (API « Fill » v3, `https://fill.papermc.io/v3`) : Velocity, Waterfall, Folia,
//!   Paper. `GET /projects/{p}/versions` (versions de la plus récente à la plus ancienne, statut de
//!   support, Java minimal), `GET /projects/{p}/versions/{v}/builds` (builds, canal `STABLE` /
//!   `BETA`…, `downloads["server:default"]` : nom, taille, sha256, url). Format relevé le
//!   2026-11-20 ; le sha256 et la taille sont vérifiés après téléchargement.
//! - **Purpur** (`https://api.purpurmc.org/v2/purpur`) : `GET /purpur` (versions, de la plus
//!   ancienne à la plus récente), `GET /purpur/{v}` (`builds.latest`, `builds.all`), téléchargement
//!   `GET /purpur/{v}/{build}/download`. L'API ne publie qu'un md5, non vérifié (aucune dépendance
//!   md5 dans le cœur) : la taille annoncée et la lisibilité de l'archive le sont.
//!
//! Tout le choix (version, build, fichier) est pur et testé ; seul [`fetch_pick`] et
//! [`download`] touchent au réseau.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

const FILL_API: &str = "https://fill.papermc.io/v3/projects";
const PURPUR_API: &str = "https://api.purpurmc.org/v2/purpur";
/// Un JAR de serveur fait quelques dizaines de Mo (Velocity 4.3 : 42 Mo, Purpur : 54 Mo).
pub(crate) const MAX_JAR_BYTES: usize = 256 * 1024 * 1024;

/// Source officielle d'un JAR de plateforme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Projet PaperMC (API Fill v3).
    Fill(&'static str),
    Purpur,
}

impl Source {
    pub(crate) fn label(self) -> String {
        match self {
            Source::Fill(p) => format!("PaperMC ({p}, fill.papermc.io)"),
            Source::Purpur => "Purpur (api.purpurmc.org)".into(),
        }
    }
}

/// Source du JAR d'une plateforme (nom normalisé), `None` : pas de voie JAR.
pub(crate) fn source_for(platform: &str) -> Option<Source> {
    match platform {
        "velocity" => Some(Source::Fill("velocity")),
        "waterfall" => Some(Source::Fill("waterfall")),
        "folia" => Some(Source::Fill("folia")),
        "paper" => Some(Source::Fill("paper")),
        "purpur" => Some(Source::Purpur),
        _ => None,
    }
}

/// JAR retenu : de quoi le télécharger, le vérifier et le décrire.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JarPick {
    pub source: Source,
    pub version: String,
    pub build: String,
    /// Canal du build (`STABLE`, `BETA`…), si la source le dit.
    pub channel: Option<String>,
    pub file: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    /// Java minimal annoncé par la source.
    pub java_min: Option<i64>,
    pub notes: Vec<String>,
}

impl JarPick {
    pub(crate) fn describe(&self) -> Value {
        serde_json::json!({
            "source": self.source.label(),
            "version": self.version,
            "build": self.build,
            "channel": self.channel,
            "file": self.file,
            "size": self.size,
            "sha256": self.sha256,
            "java_minimum": self.java_min,
            "url": self.url,
        })
    }
}

fn is_prerelease(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    ["snapshot", "-pre", "-rc", "beta", "alpha"]
        .iter()
        .any(|m| l.contains(m))
}

/// Version d'un projet Fill d'après `GET /projects/{p}/versions` : `(id, Java minimal, notes)`.
/// Sans `wanted` : la plus récente version publiée (ni SNAPSHOT, ni pré-version) encore supportée,
/// à défaut la plus récente publiée.
pub(crate) fn fill_version(
    data: &Value,
    wanted: Option<&str>,
) -> Result<(String, Option<i64>, Vec<String>), String> {
    let list = data
        .get("versions")
        .and_then(Value::as_array)
        .ok_or("réponse PaperMC sans liste `versions`")?;
    let entries: Vec<(&str, &str, Option<i64>)> = list
        .iter()
        .filter_map(|e| {
            let v = e.get("version")?;
            let id = v.get("id")?.as_str()?;
            let status = v
                .pointer("/support/status")
                .and_then(Value::as_str)
                .unwrap_or("");
            let java = v.pointer("/java/version/minimum").and_then(Value::as_i64);
            Some((id, status, java))
        })
        .collect();
    let mut notes = Vec::new();
    let chosen = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => entries
            .iter()
            .find(|(id, ..)| id.eq_ignore_ascii_case(w))
            .ok_or_else(|| {
                let shown: Vec<&str> = entries.iter().take(12).map(|(id, ..)| *id).collect();
                format!(
                    "version « {w} » inconnue de PaperMC ; versions : {}",
                    shown.join(", ")
                )
            })?,
        None => entries
            .iter()
            .find(|(id, st, _)| !is_prerelease(id) && st.eq_ignore_ascii_case("SUPPORTED"))
            .or_else(|| entries.iter().find(|(id, ..)| !is_prerelease(id)))
            .ok_or("aucune version publiée chez PaperMC")?,
    };
    let (id, status, java) = *chosen;
    match status.to_ascii_uppercase().as_str() {
        "SUPPORTED" | "" => {}
        other => notes.push(format!(
            "version {id} marquée {other} par PaperMC (plus maintenue)"
        )),
    }
    if is_prerelease(id) {
        notes.push(format!(
            "{id} est une pré-version (SNAPSHOT) : builds non stables"
        ));
    }
    Ok((id.to_string(), java, notes))
}

/// Build d'une version Fill d'après `GET /projects/{p}/versions/{v}/builds`. `wanted` : numéro de
/// build précis ; sinon le plus récent du canal `STABLE`, à défaut le plus récent (signalé).
pub(crate) fn fill_build(
    project: &'static str,
    version: &str,
    data: &Value,
    wanted: Option<&str>,
) -> Result<JarPick, String> {
    let builds = data
        .as_array()
        .ok_or("réponse PaperMC : liste de builds attendue")?;
    let id_of = |b: &Value| b.get("id").and_then(Value::as_i64);
    let channel_of = |b: &Value| {
        b.get("channel")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase()
    };
    let mut notes = Vec::new();
    let chosen = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => {
            let n: i64 = w
                .parse()
                .map_err(|_| format!("`build` : numéro de build PaperMC attendu (reçu « {w} »)"))?;
            builds
                .iter()
                .find(|b| id_of(b) == Some(n))
                .ok_or_else(|| format!("build {n} introuvable pour {project} {version}"))?
        }
        None => {
            let newest = |stable: bool| {
                builds
                    .iter()
                    .filter(|b| !stable || channel_of(b) == "STABLE")
                    .filter(|b| id_of(b).is_some())
                    .max_by_key(|b| id_of(b))
            };
            match newest(true) {
                Some(b) => b,
                None => {
                    let b = newest(false)
                        .ok_or_else(|| format!("aucun build publié pour {project} {version}"))?;
                    notes.push(format!(
                        "aucun build STABLE pour {project} {version} : build {} du canal {}",
                        id_of(b).unwrap_or_default(),
                        channel_of(b)
                    ));
                    b
                }
            }
        }
    };
    let dl = chosen
        .pointer("/downloads/server:default")
        .ok_or("build PaperMC sans téléchargement `server:default`")?;
    let file = dl
        .get("name")
        .and_then(Value::as_str)
        .ok_or("téléchargement PaperMC sans nom de fichier")?;
    let url = dl
        .get("url")
        .and_then(Value::as_str)
        .ok_or("téléchargement PaperMC sans url")?;
    let channel = Some(channel_of(chosen)).filter(|c| !c.is_empty());
    Ok(JarPick {
        source: Source::Fill(project),
        version: version.to_string(),
        build: id_of(chosen).unwrap_or_default().to_string(),
        channel,
        file: file.to_string(),
        url: url.to_string(),
        size: dl.get("size").and_then(Value::as_u64),
        sha256: dl
            .pointer("/checksums/sha256")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase),
        java_min: None,
        notes,
    })
}

/// Version Purpur d'après `GET /purpur` : `wanted` exacte, sinon la plus récente publiée.
pub(crate) fn purpur_version(data: &Value, wanted: Option<&str>) -> Result<String, String> {
    let list: Vec<&str> = data
        .get("versions")
        .and_then(Value::as_array)
        .ok_or("réponse Purpur sans liste `versions`")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => list
            .iter()
            .find(|v| v.eq_ignore_ascii_case(w))
            .map(|v| v.to_string())
            .ok_or_else(|| {
                let recent: Vec<&str> = list.iter().rev().take(10).copied().collect();
                format!(
                    "Minecraft {w} : aucune version Purpur ; récentes : {}",
                    recent.join(", ")
                )
            }),
        None => data
            .pointer("/metadata/current")
            .and_then(Value::as_str)
            .or_else(|| list.last().copied())
            .map(str::to_string)
            .ok_or_else(|| "aucune version Purpur publiée".into()),
    }
}

/// Build Purpur d'après `GET /purpur/{v}` : `wanted` (présent dans `builds.all`), sinon `latest`.
pub(crate) fn purpur_build(
    version: &str,
    data: &Value,
    wanted: Option<&str>,
) -> Result<JarPick, String> {
    let all: Vec<&str> = data
        .pointer("/builds/all")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let build = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) if all.contains(&w) => w.to_string(),
        Some(w) => return Err(format!("build Purpur {w} introuvable pour {version}")),
        None => data
            .pointer("/builds/latest")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("aucun build Purpur pour {version}"))?,
    };
    Ok(JarPick {
        source: Source::Purpur,
        version: version.to_string(),
        file: format!("purpur-{version}-{build}.jar"),
        url: format!("{PURPUR_API}/{version}/{build}/download"),
        build,
        channel: None,
        size: None,
        sha256: None,
        java_min: None,
        notes: vec![
            "Purpur ne publie qu'un md5 : taille et archive vérifiées, pas l'empreinte".into(),
        ],
    })
}

/// Nom de fichier acceptable à la racine du serveur (`set_startup_config file` : nom nu).
pub(crate) fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.to_ascii_lowercase().ends_with(".jar")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// Vérifie un JAR téléchargé : taille annoncée, sha256 publié, archive zip (`PK\x03\x04`).
pub(crate) fn verify(bytes: &[u8], pick: &JarPick) -> Result<String, String> {
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        return Err(format!(
            "« {} » téléchargé n'est pas une archive JAR",
            pick.file
        ));
    }
    if let Some(size) = pick.size {
        if bytes.len() as u64 != size {
            return Err(format!(
                "« {} » : {} octets reçus au lieu de {size}",
                pick.file,
                bytes.len()
            ));
        }
    }
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(want) = &pick.sha256 {
        if !want.eq_ignore_ascii_case(&hex) {
            return Err(format!(
                "« {} » : empreinte sha256 différente de celle publiée, fichier rejeté",
                pick.file
            ));
        }
    }
    Ok(hex)
}

fn http() -> reqwest::Client {
    crate::http::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| crate::http::client())
}

async fn get_json(url: &str) -> Result<Value, String> {
    let resp = http()
        .get(url)
        .header("User-Agent", crate::config::USER_AGENT)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("{url} injoignable : {e}"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("{url} : réponse illisible ({e})"))?;
    if !status.is_success() {
        let why = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("erreur");
        return Err(format!("{url} : HTTP {} ({why})", status.as_u16()));
    }
    Ok(body)
}

/// Choisit le JAR à installer (lectures seules sur l'API de la source).
pub(crate) async fn fetch_pick(
    source: Source,
    version: Option<&str>,
    build: Option<&str>,
) -> Result<JarPick, String> {
    match source {
        Source::Fill(project) => {
            let versions = get_json(&format!("{FILL_API}/{project}/versions")).await?;
            let (v, java, mut notes) = fill_version(&versions, version)?;
            let builds = get_json(&format!("{FILL_API}/{project}/versions/{v}/builds")).await?;
            let mut pick = fill_build(project, &v, &builds, build)?;
            pick.java_min = java;
            notes.append(&mut pick.notes);
            pick.notes = notes;
            Ok(pick)
        }
        Source::Purpur => {
            let versions = get_json(PURPUR_API).await?;
            let v = purpur_version(&versions, version)?;
            let builds = get_json(&format!("{PURPUR_API}/{v}")).await?;
            purpur_build(&v, &builds, build)
        }
    }
}

/// Java minimal d'une version d'un projet PaperMC (proxy installé par le catalogue, dont l'entrée
/// ne le dit pas). `None` : projet sans source PaperMC, ou réponse illisible.
pub(crate) async fn fill_java_min(platform: &str, version: &str) -> Option<i64> {
    let Some(Source::Fill(project)) = source_for(platform) else {
        return None;
    };
    let versions = get_json(&format!("{FILL_API}/{project}/versions"))
        .await
        .ok()?;
    fill_version(&versions, Some(version)).ok()?.1
}

/// Télécharge et vérifie le JAR ; renvoie ses octets et son sha256.
pub(crate) async fn download(pick: &JarPick) -> Result<(Vec<u8>, String), String> {
    let resp = http()
        .get(&pick.url)
        .header("User-Agent", crate::config::USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("téléchargement de « {} » impossible : {e}", pick.file))?;
    if !resp.status().is_success() {
        return Err(format!(
            "téléchargement de « {} » : HTTP {}",
            pick.file,
            resp.status().as_u16()
        ));
    }
    let announced = resp.content_length();
    if announced.is_some_and(|n| n as usize > MAX_JAR_BYTES) {
        return Err(format!(
            "« {} » dépasse {} Mo",
            pick.file,
            MAX_JAR_BYTES >> 20
        ));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("téléchargement de « {} » interrompu : {e}", pick.file))?;
    if bytes.len() > MAX_JAR_BYTES {
        return Err(format!(
            "« {} » dépasse {} Mo",
            pick.file,
            MAX_JAR_BYTES >> 20
        ));
    }
    let mut checked = pick.clone();
    if checked.size.is_none() {
        checked.size = announced;
    }
    let sha = verify(&bytes, &checked)?;
    Ok((bytes.to_vec(), sha))
}
