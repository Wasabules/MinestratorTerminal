//! **Plugins d'un réseau** : la bonne build par plateforme (outil `install_mod`, préréglages de
//! `network_preset`).
//!
//! Constat (relevé du 2026-11-19, lectures seules) : le catalogue Modrinth de MineStrator
//! (`/site/modrinth/{slug}/versions`, comme l'outil officiel `get_content_files`) ne propose que
//! les builds Bukkit/Paper d'un plugin — 5 versions « (Bukkit) » pour LuckPerms, même avec
//! `loader=velocity` ; 733 versions `paper, spigot` pour Geyser — et Floodgate n'a sur Modrinth
//! que des builds Fabric et NeoForge. Une version choisie dans ce catalogue et installée sur un
//! Velocity y déposerait donc la build Paper, que Velocity ne charge pas.
//!
//! - **Vérification** (`install_mod`, source `modrinth`) : la plateforme du serveur est reconnue
//!   comme dans `network.rs` (source de l'hébergeur, jar de démarrage, à défaut `velocity.toml` ou
//!   `config.yml` de BungeeCord) ; la version doit déclarer un loader de cette plateforme sur un
//!   proxy (`velocity` ; `bungeecord` ou `waterfall`), et une build réservée aux proxys est
//!   refusée sur un serveur de jeu. Les loaders de la version viennent du catalogue MineStrator,
//!   sinon de l'API publique de Modrinth. Refus : les builds compatibles sont proposées.
//! - **Build de proxy** (absente du catalogue MineStrator) : confiée d'abord à l'installation de
//!   l'hébergeur ; refusée par lui, elle est téléchargée depuis le CDN de Modrinth (empreinte
//!   sha512 vérifiée) et déposée dans `/plugins` par SFTP.
//! - **`list_mod_versions`** avec `loader` = `velocity`, `bungeecord` ou `waterfall` : API
//!   publique de Modrinth (le catalogue de l'hébergeur ignore ce filtre).
//! - **Source `geysermc`** (Geyser, Floodgate) : API de téléchargement officielle de GeyserMC
//!   (`download.geysermc.org`), build de la plateforme du serveur, empreinte sha256 vérifiée, dépôt
//!   dans `/plugins` (`/mods` pour Fabric ou NeoForge).
//!
//! Tout passe par `install_mod`, que la garde des agents encadre en listant `/mods` et `/plugins`
//! avant et après : le fichier ajouté est une étape annulable. Le réseau reste en Rust.

use super::*;
use crate::models::MarketVersion;
use serde::Deserialize;

/// API publique de Modrinth.
const MODRINTH_API: &str = "https://api.modrinth.com/v2";
/// Seul hôte d'où un fichier Modrinth est téléchargé.
const MODRINTH_CDN: &str = "https://cdn.modrinth.com/";
/// API de téléchargement de GeyserMC.
const GEYSERMC_API: &str = "https://download.geysermc.org/v2/projects";
/// Taille maximale d'un plugin téléchargé.
const MAX_JAR: u64 = 64 * 1024 * 1024;
/// Builds compatibles citées au plus dans un refus.
const SUGGESTED: usize = 5;

// --- Plateforme ------------------------------------------------------------------------------------

/// Loaders Modrinth d'un proxy (`None` : serveur de jeu ou plateforme inconnue).
pub(crate) fn proxy_loaders(ty: Option<ServerType>) -> Option<&'static [&'static str]> {
    match ty {
        Some(ServerType::Velocity) => Some(&["velocity"]),
        Some(ServerType::Bungee) => Some(&["bungeecord", "waterfall"]),
        _ => None,
    }
}

/// Un loader est-il celui d'un proxy ?
pub(crate) fn is_proxy_loader(l: &str) -> bool {
    matches!(
        l.trim().to_ascii_lowercase().as_str(),
        "velocity" | "bungeecord" | "waterfall" | "bungee"
    )
}

/// La version (ses loaders) convient-elle à la plateforme ? `Err` : pourquoi pas (pur).
pub(crate) fn loaders_fit(ty: Option<ServerType>, loaders: &[String]) -> Result<(), String> {
    let lower: Vec<String> = loaders.iter().map(|l| l.to_ascii_lowercase()).collect();
    let shown = if lower.is_empty() {
        "aucun loader déclaré".to_string()
    } else {
        lower.join(", ")
    };
    match proxy_loaders(ty) {
        Some(wanted) => {
            if lower.iter().any(|l| wanted.contains(&l.as_str())) {
                Ok(())
            } else {
                Err(format!(
                    "build {shown} : ce serveur est un proxy {}, il ne charge que les builds {}",
                    ty.map(ServerType::label).unwrap_or_default(),
                    wanted.join(" ou ")
                ))
            }
        }
        None if !lower.is_empty() && lower.iter().all(|l| is_proxy_loader(l)) => Err(format!(
            "build {shown} : réservée aux proxys, ce serveur de jeu ({}) ne la chargerait pas",
            ty.map(ServerType::label).unwrap_or("plateforme inconnue")
        )),
        None => Ok(()),
    }
}

/// Plateforme d'un serveur du compte, reconnue comme dans `network.rs` : source de l'hébergeur,
/// jar de démarrage, à défaut ses fichiers de proxy. `None` : inconnue.
pub(crate) async fn server_platform(core: &Core, id: i64) -> Option<ServerType> {
    let token = core.token().ok()?;
    let (mc, raw) = tokio::join!(
        core.api.minecraft_version(&token, id),
        core.api.server_value(&token, id)
    );
    let jar = raw
        .as_ref()
        .ok()
        .and_then(|v| v.pointer("/settings/startup_file"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let ty = ServerType::detect(mc.ok().as_ref().map(|m| m.source.as_str()), jar.as_deref());
    if ty.is_some() {
        return ty;
    }
    match read_proxy(core, id, "", None).await {
        Ok(Some((p, _))) => Some(match p.conf.kind {
            ProxyKind::Velocity => ServerType::Velocity,
            ProxyKind::Bungee => ServerType::Bungee,
        }),
        _ => None,
    }
}

// --- API publique de Modrinth ----------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MrFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MrVersion {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub downloads: Option<i64>,
    #[serde(default)]
    pub files: Vec<MrFile>,
}

impl MrVersion {
    /// Fichier principal (le premier à défaut).
    pub(crate) fn primary(&self) -> Option<&MrFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
    }

    /// Forme normalisée de `list_mod_versions`.
    pub(crate) fn to_market(&self) -> MarketVersion {
        let file = self.primary();
        MarketVersion {
            id: self.id.clone(),
            version_number: self.version_number.clone(),
            name: self.name.clone(),
            game_versions: self.game_versions.clone(),
            loaders: self.loaders.clone(),
            release_type: self.version_type.clone(),
            downloads: self.downloads,
            date_published: self.date_published.clone(),
            filename: file.map(|f| f.filename.clone()),
            file_size: file.and_then(|f| i64::try_from(f.size).ok()),
        }
    }
}

/// Slug ou identifiant Modrinth : lettres, chiffres, `-`, `_`, `.` (jamais un chemin).
fn modrinth_ref(s: &str) -> Result<&str, String> {
    let s = s.trim();
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !s.contains("..");
    if ok {
        Ok(s)
    } else {
        Err(format!("référence Modrinth « {s} » invalide"))
    }
}

/// Versions d'un projet Modrinth pour des loaders donnés (plus récentes d'abord).
pub(crate) async fn modrinth_versions(
    slug: &str,
    loaders: &[&str],
    game_version: &str,
) -> Result<Vec<MrVersion>, String> {
    let slug = modrinth_ref(slug)?;
    let loaders = serde_json::to_string(loaders).unwrap_or_default();
    let gv = game_version.trim();
    let games = serde_json::to_string(&[gv]).unwrap_or_default();
    let mut params: Vec<(&str, &str)> = vec![("loaders", loaders.as_str())];
    if !gv.is_empty() {
        params.push(("game_versions", games.as_str()));
    }
    crate::mods::get_json(&format!("{MODRINTH_API}/project/{slug}/version"), &params)
        .await
        .map_err(|e| format!("versions Modrinth de « {slug} » illisibles : {e}"))
}

/// Une version Modrinth par son identifiant.
pub(crate) async fn modrinth_version(id: &str) -> Result<MrVersion, String> {
    let id = modrinth_ref(id)?;
    crate::mods::get_json(&format!("{MODRINTH_API}/version/{id}"), &[])
        .await
        .map_err(|e| format!("version Modrinth « {id} » illisible : {e}"))
}

/// La plus récente des versions, une stable de préférence (ordre de l'API : récentes d'abord).
pub(crate) fn newest(versions: &[MrVersion]) -> Option<&MrVersion> {
    versions
        .iter()
        .find(|v| v.version_type.eq_ignore_ascii_case("release"))
        .or_else(|| versions.first())
}

fn hex(data: &[u8]) -> String {
    crate::util::hex(data)
}

/// Nom de fichier d'un plugin déposé dans `/plugins` : sans chemin, `.jar`.
pub(crate) fn jar_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.len() <= 128
        && n.to_ascii_lowercase().ends_with(".jar")
        && !n.contains(['/', '\\', '\0'])
        && !n.starts_with('.');
    if ok {
        Ok(n.to_string())
    } else {
        Err(format!("nom de fichier « {n} » refusé"))
    }
}

/// Dépose un fichier dans un dossier du serveur par SFTP ; un fichier de même nom déjà présent
/// n'est jamais écrasé (identique : rien à faire).
async fn upload_jar(
    core: &Core,
    id: i64,
    dir: &str,
    name: &str,
    data: Vec<u8>,
) -> Result<bool, String> {
    let name = jar_name(name)?;
    let path = format!("{dir}/{name}");
    let digest = hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    core.with_sftp(id, |conn| {
        let (path, data, dir, digest) = (path.clone(), data.clone(), dir.to_string(), digest.clone());
        async move {
            if let Ok(old) = sftp::read_bytes(&conn, &path, MAX_JAR).await {
                if hex(&<sha2::Sha256 as sha2::Digest>::digest(&old)) == digest {
                    return Ok(false);
                }
                return Err(crate::Error::Invalid(format!(
                    "« {path} » existe déjà avec un autre contenu : rien n'a été écrasé (supprime-le d'abord avec delete_path, ou mets-le à jour avec update_mods)"
                )));
            }
            sftp::ensure_dir(&conn, &dir).await;
            sftp::write_bytes(&conn, &path, &data).await?;
            Ok(true)
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Télécharge le fichier principal d'une version Modrinth (CDN de Modrinth seulement, sha512 ou
/// sha1 vérifié) et le dépose dans `/plugins`.
async fn install_from_modrinth_cdn(core: &Core, id: i64, v: &MrVersion) -> Result<String, String> {
    let f = v
        .primary()
        .ok_or_else(|| format!("la version {} n'a aucun fichier", v.id))?;
    if !f.url.starts_with(MODRINTH_CDN) {
        return Err(format!(
            "fichier hors du CDN de Modrinth refusé ({})",
            f.url.split('/').take(3).collect::<Vec<_>>().join("/")
        ));
    }
    if f.size > MAX_JAR {
        return Err(format!("fichier de {} octets : trop gros", f.size));
    }
    let data = crate::mods::get_bytes(&f.url, &[], MAX_JAR)
        .await
        .map_err(|e| format!("téléchargement depuis Modrinth impossible : {e}"))?;
    let ok = match (f.hashes.get("sha512"), f.hashes.get("sha1")) {
        (Some(h), _) => {
            h.eq_ignore_ascii_case(&hex(&<sha2::Sha512 as sha2::Digest>::digest(&data)))
        }
        (None, Some(h)) => {
            h.eq_ignore_ascii_case(&hex(&<sha1::Sha1 as sha1::Digest>::digest(&data)))
        }
        (None, None) => false,
    };
    if !ok {
        return Err("empreinte du fichier téléchargé différente de celle annoncée par Modrinth : rien n'a été installé".into());
    }
    let written = upload_jar(core, id, "/plugins", &f.filename, data).await?;
    Ok(if written {
        format!("/plugins/{}", f.filename)
    } else {
        format!("/plugins/{} (déjà présent, identique)", f.filename)
    })
}

/// Builds Modrinth compatibles d'un projet pour un proxy, citées dans un refus.
async fn suggestions(slug: &str, ty: Option<ServerType>) -> String {
    let Some(loaders) = proxy_loaders(ty) else {
        return String::new();
    };
    match modrinth_versions(slug, loaders, "").await {
        Ok(v) if !v.is_empty() => {
            let list: Vec<String> = v
                .iter()
                .take(SUGGESTED)
                .map(|x| {
                    format!(
                        "{} (version_id {}, {})",
                        x.version_number, x.id, x.version_type
                    )
                })
                .collect();
            format!(
                " Builds {} sur Modrinth (absentes du catalogue MineStrator) : {}. Relance install_mod avec l'une d'elles, ou list_mod_versions avec loader={}.",
                loaders[0],
                list.join(" ; "),
                loaders[0]
            )
        }
        Ok(_) => format!(" Aucune build {} de « {slug} » sur Modrinth.", loaders[0]),
        Err(e) => format!(" (builds compatibles non lues : {e})"),
    }
}

// --- GeyserMC ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GeyserDownload {
    pub name: String,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GeyserBuild {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub build: i64,
    #[serde(default)]
    pub downloads: HashMap<String, GeyserDownload>,
}

/// Projet GeyserMC d'un slug (`geyser`, `floodgate`).
pub(crate) fn geyser_project(slug: &str) -> Result<&'static str, String> {
    match slug.trim().to_ascii_lowercase().as_str() {
        "geyser" | "geysermc" => Ok("geyser"),
        "floodgate" => Ok("floodgate"),
        other => Err(format!(
            "source geysermc : `slug` geyser ou floodgate attendu (« {other} »)"
        )),
    }
}

/// Build GeyserMC d'une plateforme et dossier où la déposer (pur).
pub(crate) fn geyser_target(
    project: &str,
    ty: Option<ServerType>,
) -> Result<(&'static str, &'static str), String> {
    let t = match ty {
        Some(ServerType::Paper | ServerType::Spigot) => ("spigot", "/plugins"),
        Some(ServerType::Velocity) => ("velocity", "/plugins"),
        Some(ServerType::Bungee) if project == "floodgate" => ("bungee", "/plugins"),
        Some(ServerType::Bungee) => ("bungeecord", "/plugins"),
        Some(ServerType::Fabric) if project == "geyser" => ("fabric", "/mods"),
        Some(ServerType::Forge) if project == "geyser" => ("neoforge", "/mods"),
        other => {
            return Err(format!(
                "{project} : aucune build GeyserMC pour la plateforme {}",
                other.map(ServerType::label).unwrap_or("inconnue")
            ))
        }
    };
    Ok(t)
}

/// Build GeyserMC : `latest`, ou un numéro de build de la dernière version.
pub(crate) async fn geyser_build(project: &str, build: &str) -> Result<GeyserBuild, String> {
    let b = build.trim();
    let b = if b.is_empty() || b.eq_ignore_ascii_case("latest") {
        "latest".to_string()
    } else {
        b.parse::<u32>()
            .map_err(|_| format!("build GeyserMC « {b} » : `latest` ou un numéro"))?
            .to_string()
    };
    crate::mods::get_json(
        &format!("{GEYSERMC_API}/{project}/versions/latest/builds/{b}"),
        &[],
    )
    .await
    .map_err(|e| format!("API de GeyserMC illisible : {e}"))
}

/// Installe Geyser ou Floodgate (build de la plateforme du serveur).
async fn install_geysermc(
    core: &Core,
    id: i64,
    slug: &str,
    build: &str,
    ty: Option<ServerType>,
) -> Result<String, String> {
    let project = geyser_project(slug)?;
    let (platform, dir) = geyser_target(project, ty)?;
    let b = geyser_build(project, build).await?;
    let d = b.downloads.get(platform).ok_or_else(|| {
        format!(
            "{project} {} (build {}) : pas de fichier pour {platform}",
            b.version, b.build
        )
    })?;
    let url = format!(
        "{GEYSERMC_API}/{project}/versions/{}/builds/{}/downloads/{platform}",
        b.version, b.build
    );
    let data = crate::mods::get_bytes(&url, &[], MAX_JAR)
        .await
        .map_err(|e| format!("téléchargement depuis GeyserMC impossible : {e}"))?;
    if d.sha256.is_empty()
        || !d
            .sha256
            .eq_ignore_ascii_case(&hex(&<sha2::Sha256 as sha2::Digest>::digest(&data)))
    {
        return Err("empreinte du fichier téléchargé différente de celle annoncée par GeyserMC : rien n'a été installé".into());
    }
    let written = upload_jar(core, id, dir, &d.name, data).await?;
    Ok(format!(
        "{project} {} (build {}, {platform}) {} {dir}/{} sur le serveur {id} ; effet au prochain démarrage.",
        b.version,
        b.build,
        if written { "installé dans" } else { "déjà présent, identique :" },
        d.name
    ))
}

// --- Outils ---------------------------------------------------------------------------------------------

fn arg(args: &Value, k: &str) -> Option<String> {
    args.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Outil MCP `install_mod` : installation par l'hébergeur, avec la vérification de plateforme
/// (Modrinth) et la source `geysermc`.
pub(crate) async fn install_mod_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = args
        .get("server_id")
        .and_then(Value::as_i64)
        .ok_or("paramètre `server_id` (entier) requis")?;
    let source = arg(args, "source").unwrap_or_else(|| "modrinth".into());
    let kind = arg(args, "kind").unwrap_or_else(|| "mod".into());
    let slug = arg(args, "slug").ok_or("paramètre `slug` requis")?;
    let version_id = arg(args, "version_id").unwrap_or_default();
    let loader = arg(args, "loader").unwrap_or_default();
    if source != "geysermc" && version_id.is_empty() {
        return Err("paramètre `version_id` requis (list_mod_versions)".into());
    }
    let host = |e: crate::Error| {
        let m = e.to_string();
        if m.contains("Accès refusé") {
            format!("{m} — projet probablement premium/payant ou en téléchargement externe (non installable via MineStrator) ; choisis une alternative gratuite et non-externe (vérifie `premium`/`external` via market_search).")
        } else {
            m
        }
    };
    match source.as_str() {
        "geysermc" => {
            let ty = server_platform(core, id).await;
            install_geysermc(core, id, &slug, &version_id, ty).await
        }
        "modrinth" => {
            let ty = server_platform(core, id).await;
            // Loaders de la version : catalogue MineStrator, sinon API publique de Modrinth.
            let catalog = core
                .market_versions("modrinth", &slug, "", "")
                .await
                .ok()
                .and_then(|v| v.into_iter().find(|x| x.id == version_id));
            let public = match &catalog {
                Some(_) => None,
                None => modrinth_version(&version_id).await.ok(),
            };
            let loaders = catalog
                .as_ref()
                .map(|v| v.loaders.clone())
                .or_else(|| public.as_ref().map(|v| v.loaders.clone()));
            match &loaders {
                Some(l) => {
                    if let Err(why) = loaders_fit(ty, l) {
                        return Err(format!(
                            "« {slug} » ({version_id}) non installé : {why}.{}",
                            suggestions(&slug, ty).await
                        ));
                    }
                }
                None if proxy_loaders(ty).is_some() => {
                    return Err(format!(
                        "« {slug} » ({version_id}) : version introuvable (catalogue MineStrator et Modrinth) ; impossible de vérifier qu'elle convient au proxy.{}",
                        suggestions(&slug, ty).await
                    ));
                }
                None => {}
            }
            // Build de proxy absente du catalogue de l'hébergeur : son installation d'abord, puis
            // dépôt direct du fichier de Modrinth si elle est refusée.
            if let (true, Some(v)) = (proxy_loaders(ty).is_some(), &public) {
                return match core.install_mod(id, "modrinth", &kind, &slug, &version_id, &loader).await {
                    Ok(()) => Ok(format!("« {slug} » ({version_id}, build {}) installé sur le serveur {id} par l'hébergeur.", v.loaders.join("/"))),
                    Err(e) => {
                        let first = host(e);
                        let path = install_from_modrinth_cdn(core, id, v)
                            .await
                            .map_err(|e2| format!("installation refusée par l'hébergeur ({first}), puis dépôt direct impossible : {e2}"))?;
                        Ok(format!("« {slug} » ({version_id}, build {}) : l'hébergeur l'a refusé ({first}) ; fichier de Modrinth vérifié (sha512) et déposé dans {path} sur le serveur {id}. Effet au prochain démarrage.", v.loaders.join("/")))
                    }
                };
            }
            core.install_mod(id, &source, &kind, &slug, &version_id, &loader)
                .await
                .map_err(host)?;
            Ok(format!(
                "« {slug} » ({version_id}) installé sur le serveur {id}."
            ))
        }
        _ => {
            core.install_mod(id, &source, &kind, &slug, &version_id, &loader)
                .await
                .map_err(host)?;
            Ok(format!(
                "« {slug} » ({version_id}) installé sur le serveur {id}."
            ))
        }
    }
}

/// `list_mod_versions` avec un loader de proxy : API publique de Modrinth (`None` : autre cas,
/// catalogue de l'hébergeur).
pub(crate) async fn proxy_versions(
    source: &str,
    slug: &str,
    loader: &str,
    game_version: &str,
) -> Option<Result<Vec<MarketVersion>, String>> {
    let l = loader.trim().to_ascii_lowercase();
    if source != "modrinth" || !is_proxy_loader(&l) {
        return None;
    }
    let loaders: &[&str] = if l == "velocity" {
        &["velocity"]
    } else {
        &["bungeecord", "waterfall"]
    };
    // Une build de proxy ne déclare pas toujours de version du jeu : filtre seulement si demandé.
    Some(
        modrinth_versions(slug, loaders, game_version)
            .await
            .map(|v| v.iter().map(MrVersion::to_market).collect()),
    )
}
