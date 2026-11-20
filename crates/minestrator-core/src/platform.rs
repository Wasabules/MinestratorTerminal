//! Outil MCP `set_server_platform` : choisir la **plateforme et la version** d'un serveur Minecraft
//! (Paper, Spigot, Vanilla, Fabric, Forge, NeoForge, Velocity, BungeeCord, Purpur…), et au besoin
//! **Java** et le **JAR de démarrage**, sans passer par le panel. Aussi utilisé par
//! `apply_manifest` avec `apply_official: true` (étapes `server.game_version`, `server.loader`,
//! `server.java`, `startup.jar_file`).
//!
//! **Outils officiels appelés en interne** (`official_mcp::call_tool`, clé du trousseau, sans la
//! passerelle) : lectures `search_game_content type=version` et `get_content_files
//! type=loader_builds` (connexion `readonly=1`) ; écritures `change_version` (destructif),
//! `manage_java`, `set_startup_config file`. Schémas relevés le 2026-11-19 (`tools/list`) :
//! `change_version { server_id, version_id: entier, game_version, loader: none|forge|neoforge|fabric,
//! build, reinstall, autostart }` — pas de `confirm_name` côté officiel : la confirmation est faite
//! ici, avant tout effet ; `set_startup_config { server_id, parameters?, file? }` (fichier nu à la
//! racine, parmi `available_files`) ; `manage_java { server_id, java_version_id? … }`. La liste
//! des versions de Java vient de `GET /site/java/versions` (lecture REST : `Java 25`,
//! `Java 25 - Performance`, `Java 21 - GraalVM`…), plutôt que de `manage_java` sans paramètre,
//! classé modifiant.
//!
//! **Catalogue des versions** (relevé du 2026-11-19, lecture seule) : Paper, Spigot, Vanilla,
//! Arclight, Spongeforge… par version de Minecraft (`versionmc`) ; **proxys** : `Velocity` (id
//! 3993, une seule entrée, la dernière version : 4.3.1, « 1.8 et plus ») et `BungeeCord` (id 4010,
//! version non précisée) ; **absents** : Waterfall, Purpur, Folia, Quilt, Mohist, Pufferfish. Fabric,
//! Forge et NeoForge s'installent par leur nom (`loader` + `game_version` + `build`).
//!
//! Deux voies :
//! - **catalogue** : `change_version` (entrée `version_id`, ou chargeur par son nom) ;
//! - **JAR** ([`jar`]) : plateforme absente du catalogue (Purpur, Folia, Waterfall) ou version
//!   précise qu'il ne propose pas (Velocity 3.4.0…) : JAR officiel téléchargé côté Rust (PaperMC
//!   Fill v3, Purpur), vérifié, envoyé par `upload_file`, puis choisi par `set_startup_config file`.
//!
//! **Garde-fous** : `dry_run` vrai par défaut (identifiants résolus, outils et arguments, rien
//! d'exécuté) ; `change_version` exige la confirmation de l'humain (élicitation) ou `confirm_name`
//! = nom exact du serveur, **avant tout effet** (l'ère MCP sans état rejoue l'appel : seules des
//! lectures précèdent la confirmation) ; une version de Minecraft **antérieure** à celle du monde
//! est refusée (la plateforme la refuse, monde illisible) sauf `reinstall: true` (réinstallation
//! propre, contenu perdu, snapshot forcée). Chaque sous-écriture est une étape de la garde des
//! agents : `upload_file` par [`crate::agent_guard::guarded_write`] (annulable), les outils
//! officiels par [`official_write`] (étapes non annulables, raison dans `undo::plan_for`).

mod jar;
#[cfg(test)]
mod tests;

use crate::manifest::{java_major, older, Facts, ServerSpec};
use crate::mcp_ctx;
use crate::Core;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) use jar::JarPick;

/// Chargeurs installés par leur nom (`change_version loader`).
const LOADERS: &[&str] = &["fabric", "forge", "neoforge"];
/// Proxys : leur « version » est celle du proxy, pas celle de Minecraft.
const PROXIES: &[&str] = &["velocity", "bungeecord", "waterfall"];
/// Identifiants des étapes du manifeste que cette logique applique (`apply_official`).
pub(crate) const MANIFEST_STEPS: &[&str] = &[
    "server.game_version",
    "server.loader",
    "server.java",
    "startup.jar_file",
];

fn is_loader(p: &str) -> bool {
    LOADERS.contains(&p)
}

fn is_proxy(p: &str) -> bool {
    PROXIES.contains(&p)
}

/// Nom de plateforme normalisé (`Paper` → `paper`, `Bungee` → `bungeecord`…).
pub(crate) fn normalize_platform(p: &str) -> String {
    let l: String = p
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-'))
        .collect();
    // L'hébergeur nomme ses proxys `proxyvelocity`, `proxybungeecord`… (relevé réel du
    // 2026-11-20 sur un Velocity installé par change_version).
    let l = match l.strip_prefix("proxy") {
        Some(rest @ ("velocity" | "bungee" | "bungeecord" | "waterfall")) => rest.to_string(),
        _ => l,
    };
    match l.as_str() {
        "bungee" | "bungeecord" => "bungeecord",
        "neoforged" => "neoforge",
        "minecraft" | "mojang" | "vanilla" => "vanilla",
        "papermc" => "paper",
        other => other,
    }
    .to_string()
}

// --- Arguments --------------------------------------------------------------------------------------

/// Voie demandée.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Method {
    /// Catalogue, puis JAR si la plateforme ou la version n'y est pas.
    #[default]
    Auto,
    Catalog,
    Jar,
}

/// Arguments validés (sans réseau).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Request {
    pub server_id: i64,
    pub platform: Option<String>,
    /// Minecraft pour un serveur de jeu, version du proxy pour un proxy.
    pub version: Option<String>,
    pub build: Option<String>,
    pub version_id: Option<i64>,
    pub java: Option<i64>,
    pub jar_file: Option<String>,
    pub method: Method,
    pub reinstall: bool,
    pub confirm_name: Option<String>,
    pub snapshot: bool,
    pub dry_run: bool,
    /// `apply_manifest` : un `jar_file` qui contredit la voie du catalogue est ignoré (l'hébergeur
    /// choisit le fichier) au lieu de bloquer.
    pub drop_jar_with_catalog: bool,
}

fn text(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn int(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(Some)
            .ok_or_else(|| format!("`{key}` : entier attendu")),
    }
}

impl Request {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        let server_id = int(args, "server_id")?.ok_or("paramètre `server_id` (entier) requis")?;
        let flag = |k: &str, d: bool| args.get(k).and_then(Value::as_bool).unwrap_or(d);
        let java = int(args, "java")?;
        if let Some(j) = java {
            if !(8..=40).contains(&j) {
                return Err(format!(
                    "`java` : version majeure de Java (8 à 40) attendue, reçu {j}"
                ));
            }
        }
        let jar_file = text(args, "jar_file").map(|j| j.trim_start_matches('/').to_string());
        if let Some(j) = &jar_file {
            if !jar::valid_file_name(j) {
                return Err(format!(
                    "`jar_file` : nom de fichier .jar à la racine du serveur attendu (lettres, chiffres, . - _ +), reçu « {j} »"
                ));
            }
        }
        let method = match text(args, "method")
            .map(|m| m.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("auto") => Method::Auto,
            Some("catalog" | "catalogue") => Method::Catalog,
            Some("jar") => Method::Jar,
            Some(other) => {
                return Err(format!(
                    "`method` : auto, catalog ou jar attendu (reçu « {other} »)"
                ))
            }
        };
        let req = Request {
            server_id,
            platform: text(args, "platform").map(|p| normalize_platform(&p)),
            version: text(args, "version").or_else(|| text(args, "game_version")),
            build: text(args, "build"),
            version_id: int(args, "version_id")?,
            java,
            jar_file,
            method,
            reinstall: flag("reinstall", false),
            confirm_name: text(args, "confirm_name"),
            snapshot: flag("snapshot", true),
            dry_run: flag("dry_run", true),
            drop_jar_with_catalog: false,
        };
        if req.platform.is_none()
            && req.version.is_none()
            && req.version_id.is_none()
            && req.java.is_none()
            && req.jar_file.is_none()
        {
            return Err("rien à changer : précise `platform`, `version`, `version_id`, `java` ou `jar_file`".into());
        }
        if req.method == Method::Jar && req.version_id.is_some() {
            return Err(
                "`version_id` désigne une entrée du catalogue : incompatible avec method=jar"
                    .into(),
            );
        }
        Ok(req)
    }

    /// Requête des étapes manuelles d'un manifeste (`apply_official`). `ids` : étapes `manual` à
    /// appliquer. `None` : aucune de ces étapes.
    pub(crate) fn from_manifest(
        server_id: i64,
        spec: Option<&ServerSpec>,
        jar_file: Option<&str>,
        ids: &[&str],
    ) -> Option<Self> {
        let has = |id: &str| ids.contains(&id);
        let nonempty = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let req = Request {
            server_id,
            platform: if has("server.loader") {
                nonempty(spec.and_then(|s| s.loader.as_deref())).map(|p| normalize_platform(&p))
            } else {
                None
            },
            version: if has("server.game_version") || has("server.loader") {
                nonempty(spec.and_then(|s| s.game_version.as_deref()))
            } else {
                None
            },
            java: if has("server.java") {
                spec.and_then(|s| s.java_version)
            } else {
                None
            },
            jar_file: if has("startup.jar_file") {
                nonempty(jar_file).map(|j| j.trim_start_matches('/').to_string())
            } else {
                None
            },
            snapshot: false,
            dry_run: false,
            drop_jar_with_catalog: true,
            ..Default::default()
        };
        (req.platform.is_some()
            || req.version.is_some()
            || req.java.is_some()
            || req.jar_file.is_some())
        .then_some(req)
    }
}

// --- État actuel ---------------------------------------------------------------------------------------

/// Ce que le serveur a aujourd'hui.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Current {
    pub name: String,
    pub family: String,
    /// Plateforme détectée par l'hébergeur (`paper`, `fabric`…).
    pub platform: Option<String>,
    pub game_version: Option<String>,
    pub java: Option<i64>,
    pub java_version_id: Option<i64>,
    /// Nom de la variante de Java (`Java 25 - Performance`), d'après la liste de l'hébergeur.
    pub java_label: Option<String>,
    pub jar_file: Option<String>,
}

impl Current {
    pub(crate) fn from_facts(f: &Facts, java: &[JavaOption]) -> Self {
        let st = f.startup.as_ref();
        let id = st.map(|s| s.java_version).filter(|id| *id > 0);
        let listed = id.and_then(|id| java.iter().find(|j| j.id == id));
        Current {
            name: f.name.clone(),
            family: f.family.clone(),
            platform: f.loader.as_deref().map(normalize_platform),
            game_version: f.game_version.clone(),
            java: listed
                .and_then(|j| j.major)
                .or_else(|| st.and_then(|s| java_major(&s.image))),
            java_version_id: id,
            java_label: listed.map(|j| j.name.clone()),
            jar_file: st.map(|s| s.jar_file.clone()).filter(|j| !j.is_empty()),
        }
    }
}

// --- Catalogue des versions -----------------------------------------------------------------------------

/// Entrée de `search_game_content type=version`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogEntry {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub version: String,
    pub versionmc: String,
}

impl CatalogEntry {
    fn describe(&self) -> Value {
        json!({ "version_id": self.id, "name": self.name, "category": self.category, "version": self.version, "versionmc": self.versionmc })
    }
}

/// Entrées d'une réponse `search_game_content` (texte JSON) et son avertissement éventuel (aucune
/// entrée exacte pour la version demandée).
pub(crate) fn parse_catalog(text: &str) -> (Vec<CatalogEntry>, Option<String>) {
    let Some(v) = crate::db_connect::json_in_text(text) else {
        return (Vec::new(), None);
    };
    let s = |e: &Value, k: &str| {
        e.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let entries = v
        .get("versions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(CatalogEntry {
                        id: e.get("id")?.as_i64()?,
                        name: s(e, "name"),
                        category: s(e, "category").to_ascii_lowercase(),
                        version: s(e, "version"),
                        versionmc: s(e, "versionmc"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let warning = v.get("warning").and_then(Value::as_str).map(str::to_string);
    (entries, warning)
}

/// Entrée du catalogue pour `platform` et `version` (Minecraft pour un jeu, version du proxy pour
/// un proxy). Sans version : la première (la plus récente). `None` : rien d'exact.
pub(crate) fn choose_entry<'a>(
    entries: &'a [CatalogEntry],
    platform: &str,
    version: Option<&str>,
) -> Option<&'a CatalogEntry> {
    let mine: Vec<&CatalogEntry> = entries
        .iter()
        .filter(|e| e.category == platform || normalize_platform(&e.name) == platform)
        .collect();
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        None => mine.first().copied(),
        // Proxy : sa propre version ; une entrée sans version (BungeeCord) installe la dernière.
        Some(v) if is_proxy(platform) => mine
            .iter()
            .copied()
            .find(|e| e.version.eq_ignore_ascii_case(v))
            .or_else(|| mine.iter().copied().find(|e| e.version.is_empty())),
        Some(v) => mine
            .iter()
            .copied()
            .find(|e| e.versionmc.eq_ignore_ascii_case(v) || e.version.eq_ignore_ascii_case(v)),
    }
}

/// Builds d'un chargeur (`get_content_files type=loader_builds`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LoaderBuilds {
    pub builds: Vec<String>,
    pub latest_stable: Option<String>,
}

pub(crate) fn parse_loader_builds(text: &str) -> LoaderBuilds {
    let Some(v) = crate::db_connect::json_in_text(text) else {
        return LoaderBuilds::default();
    };
    LoaderBuilds {
        builds: v
            .get("builds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|b| b.get("build").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        latest_stable: v
            .get("latest_stable")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

// --- Java -----------------------------------------------------------------------------------------------

/// Version de Java proposée par l'hébergeur (`GET /site/java/versions`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JavaOption {
    pub id: i64,
    pub name: String,
    pub major: Option<i64>,
}

/// Version majeure d'un libellé (`Java 21 - GraalVM` → 21, `Java 1.8` → 8).
fn label_major(name: &str) -> Option<i64> {
    name.split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse::<i64>().ok())
        .find(|n| (7..=40).contains(n))
}

pub(crate) fn parse_java_versions(data: &Value) -> Vec<JavaOption> {
    data.get("versions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|j| {
                    let name = j.get("name")?.as_str()?.trim().to_string();
                    Some(JavaOption {
                        id: j.get("id")?.as_i64()?,
                        major: label_major(&name),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Variante d'un libellé (`Java 25 - Performance` → `performance`, `Java 25` → ``).
fn variant(name: &str) -> String {
    name.split_once('-')
        .map(|(_, v)| v.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Option de Java pour `major` : même variante que l'actuelle si elle existe (Performance,
/// GraalVM…), sinon la variante simple (`Java 21`), sinon la première de cette version.
pub(crate) fn java_option(
    list: &[JavaOption],
    major: i64,
    current_label: Option<&str>,
) -> Option<JavaOption> {
    let same: Vec<&JavaOption> = list.iter().filter(|j| j.major == Some(major)).collect();
    let want = current_label.map(variant).unwrap_or_default();
    same.iter()
        .find(|j| !want.is_empty() && variant(&j.name) == want)
        .or_else(|| same.iter().find(|j| variant(&j.name).is_empty()))
        .or_else(|| same.first())
        .map(|j| (*j).clone())
}

/// Java minimal d'une version de Minecraft (`None` : pas de règle sûre, Java actuel gardé).
/// 26.x : Java 25 ; 1.20.5 à 1.21.x : 21 ; 1.18 à 1.20.4 : 17 ; 1.17 : 16 (17 proposé).
pub(crate) fn java_for_minecraft(version: &str) -> Option<i64> {
    let parts: Vec<u64> = version
        .trim()
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<u64>>>()?;
    match parts.as_slice() {
        [major, ..] if *major >= 26 => Some(25),
        [1, 21, ..] => Some(21),
        [1, 20, patch, ..] if *patch >= 5 => Some(21),
        [1, 18..=20, ..] => Some(17),
        [1, 17, ..] => Some(17),
        _ => None,
    }
}

/// Changement de Java à faire.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JavaChange {
    pub major: i64,
    pub option: JavaOption,
    pub reason: String,
}

/// Java à appliquer : `wanted` (demandé) s'il diffère de l'actuel ; sinon, automatiquement, le
/// minimum `required` si l'actuel est plus ancien (jamais de retour en arrière automatique).
/// Avertissements dans `warnings` ; erreur si la version voulue n'est pas proposée.
pub(crate) fn java_change(
    wanted: Option<i64>,
    required: Option<(i64, String)>,
    current: &Current,
    list: &[JavaOption],
    warnings: &mut Vec<String>,
) -> Result<Option<JavaChange>, String> {
    let (major, reason) = match (wanted, &required) {
        (Some(w), Some((r, why))) if w < *r => {
            warnings.push(format!(
                "Java {w} demandé alors que {why} exige Java {r} au moins : le serveur risque de ne pas démarrer"
            ));
            (w, "demandé".to_string())
        }
        (Some(w), _) => (w, "demandé".to_string()),
        (None, Some((r, why))) => match current.java {
            Some(c) if c >= *r => return Ok(None),
            Some(c) => (*r, format!("Java {c} trop ancien : {why} exige Java {r}")),
            None => {
                warnings.push(format!(
                    "Java actuel illisible ; {why} exige Java {r} au moins : précise `java` si le serveur ne démarre pas"
                ));
                return Ok(None);
            }
        },
        (None, None) => return Ok(None),
    };
    if current.java == Some(major) {
        return Ok(None);
    }
    let option = java_option(list, major, current.java_label.as_deref()).ok_or_else(|| {
        let majors: BTreeSet<i64> = list.iter().filter_map(|j| j.major).collect();
        format!(
            "Java {major} n'est pas proposé par l'hébergeur (versions : {})",
            majors
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    Ok(Some(JavaChange {
        major,
        option,
        reason,
    }))
}

// --- Plan ---------------------------------------------------------------------------------------------------

/// Voie retenue pour la plateforme.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Route {
    /// Plateforme et version inchangées (Java ou JAR seulement).
    Unchanged,
    /// Chargeur par son nom.
    Loader {
        loader: String,
        game_version: String,
        build: Option<String>,
        latest_stable: Option<String>,
    },
    /// Entrée du catalogue des versions.
    Catalog { entry: CatalogEntry },
    /// Entrée donnée explicitement (`version_id`), non retrouvée dans une recherche.
    CatalogId { version_id: i64 },
    /// JAR officiel envoyé puis choisi comme fichier de démarrage.
    Jar { pick: JarPick, remote: String },
}

impl Route {
    fn label(&self) -> &'static str {
        match self {
            Route::Unchanged => "inchangée",
            Route::Loader { .. } => "catalogue (chargeur)",
            Route::Catalog { .. } | Route::CatalogId { .. } => "catalogue",
            Route::Jar { .. } => "jar",
        }
    }
}

/// Plan résolu (lectures faites, aucun effet).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prepared {
    pub server_id: i64,
    pub current: Current,
    pub platform: Option<String>,
    pub version: Option<String>,
    pub route: Route,
    pub java: Option<JavaChange>,
    /// Fichier de démarrage à choisir (`set_startup_config file`).
    pub startup_file: Option<String>,
    pub reinstall: bool,
    pub snapshot: bool,
    pub warnings: Vec<String>,
    /// Ce qui empêche d'appliquer (rien n'est fait tant qu'il en reste).
    pub blocking: Vec<String>,
}

/// Opération d'écriture, dans l'ordre d'exécution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Op {
    /// `upload_file` (local, garde) ou outil officiel.
    pub tool: &'static str,
    /// Toolset de l'outil officiel (`None` : outil local).
    pub toolset: Option<&'static str>,
    pub args: Value,
    pub summary: String,
}

/// Arguments de `change_version` (sans `confirm_name` : l'officiel ne le prend pas, la
/// confirmation est faite localement avant).
pub(crate) fn change_version_args(p: &Prepared) -> Option<Value> {
    let mut args = match &p.route {
        Route::Loader {
            loader,
            game_version,
            build,
            ..
        } => {
            let mut a =
                json!({ "server_id": p.server_id, "loader": loader, "game_version": game_version });
            if let Some(b) = build {
                a["build"] = json!(b);
            }
            a
        }
        Route::Catalog { entry } => {
            let mut a = json!({ "server_id": p.server_id, "version_id": entry.id });
            // Hors proxy, la version de Minecraft est contrôlée par l'officiel si elle est donnée.
            if let (Some(v), Some(pf)) = (&p.version, &p.platform) {
                if !is_proxy(pf) && entry.versionmc.eq_ignore_ascii_case(v) {
                    a["game_version"] = json!(v);
                }
            }
            a
        }
        Route::CatalogId { version_id } => {
            let mut a = json!({ "server_id": p.server_id, "version_id": version_id });
            if let (Some(v), false) = (&p.version, p.platform.as_deref().is_some_and(is_proxy)) {
                a["game_version"] = json!(v);
            }
            a
        }
        Route::Unchanged | Route::Jar { .. } => return None,
    };
    if p.reinstall {
        args["reinstall"] = json!(true);
    }
    Some(args)
}

/// Opérations dans l'ordre : envoi du JAR, Java (avant que `change_version` ne redémarre le
/// serveur), changement de version, fichier de démarrage.
pub(crate) fn ops(p: &Prepared) -> Vec<Op> {
    let id = p.server_id;
    let mut out = Vec::new();
    if let Route::Jar { pick, remote } = &p.route {
        out.push(Op {
            tool: "upload_file",
            toolset: None,
            args: json!({ "server_id": id, "remote_path": format!("/{remote}"), "source": pick.url, "sha256": pick.sha256, "size": pick.size }),
            summary: format!(
                "télécharger {} {} (build {}) depuis {} puis l'envoyer en /{remote}",
                p.platform.as_deref().unwrap_or("?"),
                pick.version,
                pick.build,
                pick.source.label()
            ),
        });
    }
    if let Some(j) = &p.java {
        out.push(Op {
            tool: "manage_java",
            toolset: Some("config"),
            args: json!({ "server_id": id, "java_version_id": j.option.id }),
            summary: format!("Java {} ({}) : {}", j.major, j.option.name, j.reason),
        });
    }
    if let Some(args) = change_version_args(p) {
        let what = match &p.route {
            Route::Loader {
                loader,
                game_version,
                build,
                latest_stable,
            } => format!(
                "{loader} {game_version} (build {})",
                build
                    .clone()
                    .or_else(|| latest_stable
                        .clone()
                        .map(|b| format!("{b}, dernier stable")))
                    .unwrap_or_else(|| "dernier stable".into())
            ),
            Route::Catalog { entry } => format!(
                "{} {} (entrée {} du catalogue)",
                entry.name,
                if entry.version.is_empty() {
                    &entry.versionmc
                } else {
                    &entry.version
                },
                entry.id
            ),
            Route::CatalogId { version_id } => format!("entrée {version_id} du catalogue"),
            _ => String::new(),
        };
        out.push(Op {
            tool: "change_version",
            toolset: Some("content"),
            args,
            summary: format!(
                "installer {what}{} (DESTRUCTIF, asynchrone)",
                if p.reinstall {
                    " par réinstallation propre : contenu effacé"
                } else {
                    ""
                }
            ),
        });
    }
    if let Some(file) = &p.startup_file {
        out.push(Op {
            tool: "set_startup_config",
            toolset: Some("config"),
            args: json!({ "server_id": id, "file": file }),
            summary: format!("fichier de démarrage : {file}"),
        });
    }
    out
}

/// Une confirmation humaine est-elle exigée (outil destructif dans le plan) ?
pub(crate) fn needs_confirmation(p: &Prepared) -> bool {
    change_version_args(p).is_some()
}

/// Ce que demande la requête, avant toute lecture du catalogue : plateforme et version visées.
/// `Ok(None)` : plateforme et version inchangées.
pub(crate) fn target(
    req: &Request,
    cur: &Current,
) -> Result<Option<(String, Option<String>)>, String> {
    if req.platform.is_none() && req.version.is_none() && req.version_id.is_none() {
        return Ok(None);
    }
    let platform = match (&req.platform, &cur.platform) {
        (Some(p), _) => p.clone(),
        (None, Some(c)) => c.clone(),
        (None, None) if req.version_id.is_some() => String::new(),
        (None, None) => {
            return Err(
                "plateforme actuelle non détectée par l'hébergeur : précise `platform`".into(),
            )
        }
    };
    let same_platform = cur.platform.as_deref() == Some(platform.as_str());
    let version = match &req.version {
        Some(v) => Some(v.clone()),
        // Changement de plateforme de jeu : même version de Minecraft que le monde.
        None if !is_proxy(&platform)
            && !cur.platform.as_deref().is_some_and(is_proxy)
            && !same_platform =>
        {
            cur.game_version.clone()
        }
        None => None,
    };
    if req.version_id.is_none()
        && same_platform
        && (version.is_none() || version == cur.game_version)
        && req.method != Method::Jar
    {
        return Ok(None);
    }
    Ok(Some((platform, version)))
}

/// Problèmes de version (pur) : retour en arrière, changement de nature (jeu ↔ proxy).
pub(crate) fn version_checks(
    platform: &str,
    version: Option<&str>,
    cur: &Current,
    reinstall: bool,
    route_is_jar: bool,
    warnings: &mut Vec<String>,
    blocking: &mut Vec<String>,
) {
    let proxy = is_proxy(platform);
    let cur_proxy = cur.platform.as_deref().is_some_and(is_proxy);
    if proxy != cur_proxy && cur.platform.is_some() && !platform.is_empty() {
        warnings.push(if proxy {
            format!("le serveur devient un PROXY ({platform}) : ses plugins et son monde ne servent plus ; plugins de proxy à installer ensuite (network_topology, link_proxy)")
        } else {
            format!("le proxy devient un serveur de JEU ({platform}) : sa configuration de proxy ne sert plus")
        });
    }
    if proxy || cur_proxy {
        return;
    }
    let (Some(want), Some(have)) = (version, cur.game_version.as_deref()) else {
        return;
    };
    if older(want, have) {
        if reinstall && !route_is_jar {
            warnings.push(format!(
                "{want} est ANTÉRIEURE à {have} : réinstallation propre demandée (reinstall=true) — tout le contenu du serveur (monde, plugins, configs) est effacé ; snapshot préalable forcée"
            ));
        } else if reinstall {
            blocking.push(format!(
                "{want} est ANTÉRIEURE à {have} et la voie JAR ne réinstalle pas : impossible sans perdre le monde ; passer par la voie catalogue (reinstall=true) ou garder {have}"
            ));
        } else {
            blocking.push(format!(
                "{want} est ANTÉRIEURE à la version du monde ({have}) : la plateforme refuse un retour en arrière (chunks illisibles). Garde {have} ou plus récent ; seule une réinstallation propre (`reinstall: true`, contenu PERDU, snapshot forcée) le permet"
            ));
        }
    }
}

// --- Lectures -------------------------------------------------------------------------------------------------

/// Appel officiel en lecture (`readonly=1`).
async fn official_read(
    core: &Core,
    toolset: &str,
    tool: &str,
    args: Value,
) -> Result<String, String> {
    let token = core.token().map_err(|e| e.to_string())?;
    crate::official_mcp::call_tool(&token, toolset, true, tool, args)
        .await
        .map_err(|e| format!("{tool} (MCP officiel) : {}", core.redact_ai(&e)))
}

async fn java_versions(core: &Core) -> Result<Vec<JavaOption>, String> {
    let token = core.token().map_err(|e| e.to_string())?;
    let data = core
        .api
        .get_data(&token, "/site/java/versions")
        .await
        .map_err(|e| format!("versions de Java illisibles : {e}"))?;
    Ok(parse_java_versions(&data))
}

/// Le fichier existe-t-il à la racine du serveur ? `None` : illisible.
async fn root_file(core: &Core, server_id: i64, name: &str) -> Option<Option<u64>> {
    let path = format!("/{name}");
    let path = path.as_str();
    core.with_sftp(server_id, |conn| async move {
        crate::sftp::probe(&conn, path).await
    })
    .await
    .ok()
    .map(|o| {
        o.filter(|e| e.kind != crate::sftp::EntryKind::Dir)
            .map(|e| e.size)
    })
}

/// Résout la requête : état actuel, plateforme, voie, Java, fichier de démarrage. Lectures seules.
pub(crate) async fn prepare(core: &Core, req: &Request) -> Result<Prepared, String> {
    let facts = core.manifest_facts(req.server_id).await?;
    if facts.family != "minecraft" {
        return Err(format!(
            "serveur {} : jeu « {} » ; set_server_platform ne gère que Minecraft (change_version aussi)",
            req.server_id, facts.egg_name
        ));
    }
    let java_list = java_versions(core).await;
    let list = java_list.as_ref().map(Vec::as_slice).unwrap_or(&[]);
    let current = Current::from_facts(&facts, list);
    let mut p = Prepared {
        server_id: req.server_id,
        current,
        platform: None,
        version: None,
        route: Route::Unchanged,
        java: None,
        startup_file: None,
        reinstall: req.reinstall,
        snapshot: req.snapshot,
        warnings: Vec::new(),
        blocking: Vec::new(),
    };
    let mut required_java: Option<(i64, String)> = None;

    match target(req, &p.current)? {
        None => {
            if req.platform.is_some() || req.version.is_some() {
                p.warnings.push(format!(
                    "plateforme et version déjà en place ({} {}) : rien à changer de ce côté",
                    p.current.platform.as_deref().unwrap_or("?"),
                    p.current.game_version.as_deref().unwrap_or("?")
                ));
            }
        }
        Some((platform, version)) => {
            p.platform = Some(platform.clone()).filter(|s| !s.is_empty());
            p.version = version.clone();
            resolve_route(core, req, &mut p, &platform, version.as_deref()).await;
            let is_jar = matches!(p.route, Route::Jar { .. });
            let mut w = std::mem::take(&mut p.warnings);
            let mut b = std::mem::take(&mut p.blocking);
            version_checks(
                &platform,
                p.version.as_deref(),
                &p.current,
                req.reinstall,
                is_jar,
                &mut w,
                &mut b,
            );
            p.warnings = w;
            p.blocking = b;
            required_java = match &p.route {
                Route::Jar { pick, .. } => pick
                    .java_min
                    .map(|j| (j, format!("{platform} {}", pick.version)))
                    .or_else(|| {
                        (!is_proxy(&platform))
                            .then(|| p.version.as_deref().and_then(java_for_minecraft))
                            .flatten()
                            .map(|j| {
                                (
                                    j,
                                    format!("Minecraft {}", p.version.clone().unwrap_or_default()),
                                )
                            })
                    }),
                Route::Catalog { entry } if is_proxy(&platform) => {
                    jar::fill_java_min(&platform, &entry.version)
                        .await
                        .map(|j| (j, format!("{} {}", entry.name, entry.version)))
                }
                Route::Unchanged => None,
                _ if is_proxy(&platform) => None,
                _ => p.version.as_deref().and_then(java_for_minecraft).map(|j| {
                    (
                        j,
                        format!("Minecraft {}", p.version.clone().unwrap_or_default()),
                    )
                }),
            };
        }
    }

    // Java.
    if req.java.is_some() || required_java.is_some() {
        match &java_list {
            Ok(list) => {
                let mut w = std::mem::take(&mut p.warnings);
                match java_change(req.java, required_java, &p.current, list, &mut w) {
                    Ok(change) => p.java = change,
                    Err(e) => p.blocking.push(e),
                }
                p.warnings = w;
            }
            Err(e) if req.java.is_some() => p.blocking.push(e.clone()),
            Err(e) => p.warnings.push(format!("{e} : Java non vérifié")),
        }
    }

    // Fichier de démarrage.
    match (&p.route, &req.jar_file) {
        (Route::Jar { remote, .. }, _) => p.startup_file = Some(remote.clone()),
        (Route::Unchanged, Some(file)) => {
            if p.current.jar_file.as_deref() == Some(file.as_str()) {
                p.warnings
                    .push(format!("« {file} » est déjà le fichier de démarrage"));
            } else {
                match root_file(core, req.server_id, file).await {
                    Some(Some(_)) => p.startup_file = Some(file.clone()),
                    Some(None) => p.blocking.push(format!(
                        "« {file} » absent de la racine du serveur : l'envoyer d'abord (upload_file), ou passer `platform` pour que l'outil l'installe"
                    )),
                    None => p.blocking.push(format!(
                        "racine du serveur illisible (SFTP) : impossible de vérifier « {file} »"
                    )),
                }
            }
        }
        (_, Some(file)) if req.drop_jar_with_catalog => p.warnings.push(format!(
            "`jar_file` « {file} » ignoré : change_version installe et choisit lui-même le fichier de démarrage (relance plan_manifest une fois l'opération terminée)"
        )),
        (_, Some(file)) => p.blocking.push(format!(
            "`jar_file` « {file} » avec la voie du catalogue : l'hébergeur choisit lui-même le fichier ; retire `jar_file`, ou force method=jar"
        )),
        _ => {}
    }
    if p.reinstall && needs_confirmation(&p) {
        p.snapshot = true;
    }
    if !p.blocking.is_empty() {
        return Ok(p);
    }
    if ops(&p).is_empty() && p.warnings.is_empty() {
        p.warnings.push("rien à changer".into());
    }
    Ok(p)
}

/// Voie pour `platform` / `version` : catalogue (lecture `search_game_content`), chargeur (lecture
/// `get_content_files`), ou JAR officiel (lecture de l'API de la source). Résultat dans `p`.
async fn resolve_route(
    core: &Core,
    req: &Request,
    p: &mut Prepared,
    platform: &str,
    version: Option<&str>,
) {
    if platform == "quilt" {
        p.blocking.push("Quilt : ni catalogue de l'hébergeur ni JAR unique (installateur) ; à faire dans le panel, ou Fabric à la place".into());
        return;
    }
    if is_loader(platform) {
        if req.method == Method::Jar {
            p.blocking.push(format!("{platform} s'installe par son installateur, pas par un JAR unique : voie catalogue seulement (method=auto)"));
            return;
        }
        let Some(gv) = version else {
            p.blocking.push(format!(
                "{platform} : précise `version` (version de Minecraft, ex. 1.21.4)"
            ));
            return;
        };
        let builds = official_read(
            core,
            "content",
            "get_content_files",
            json!({ "type": "loader_builds", "loader": platform, "game_version": gv }),
        )
        .await
        .map(|t| parse_loader_builds(&t));
        match builds {
            Ok(b) if b.builds.is_empty() => p.blocking.push(format!(
                "aucun build {platform} pour Minecraft {gv} au catalogue de l'hébergeur"
            )),
            Ok(b) => {
                if let Some(want) = &req.build {
                    if !b.builds.iter().any(|x| x == want) {
                        p.blocking.push(format!(
                            "build {platform} « {want} » introuvable pour {gv} ; récents : {}",
                            b.builds
                                .iter()
                                .take(8)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                        return;
                    }
                }
                p.route = Route::Loader {
                    loader: platform.to_string(),
                    game_version: gv.to_string(),
                    build: req.build.clone(),
                    latest_stable: b.latest_stable,
                };
            }
            Err(e) => p.blocking.push(e),
        }
        return;
    }
    if let Some(id) = req.version_id {
        p.route = Route::CatalogId { version_id: id };
        p.warnings.push(format!(
            "entrée {id} du catalogue donnée telle quelle : change_version la vérifie à l'application"
        ));
        return;
    }
    let source = jar::source_for(platform);
    let mut catalog_note = None;
    if req.method != Method::Jar {
        let mut args = json!({ "type": "version", "query": platform });
        if let (Some(v), false) = (version, is_proxy(platform)) {
            args["game_version"] = json!(v);
        }
        match official_read(core, "content", "search_game_content", args).await {
            Ok(text) => {
                let (entries, _) = parse_catalog(&text);
                if let Some(e) = choose_entry(&entries, platform, version) {
                    if platform == "bungeecord" && version.is_some() {
                        p.warnings.push("BungeeCord : le catalogue installe sa dernière version (la version demandée n'est pas choisissable)".into());
                    }
                    p.route = Route::Catalog { entry: e.clone() };
                    return;
                }
                let available: Vec<String> = entries
                    .iter()
                    .filter(|e| e.category == platform || normalize_platform(&e.name) == platform)
                    .take(10)
                    .map(|e| {
                        if is_proxy(platform) {
                            format!("{} {}", e.name, e.version)
                        } else {
                            e.versionmc.clone()
                        }
                    })
                    .collect();
                catalog_note = Some(if available.is_empty() {
                    format!("« {platform} » absent du catalogue de l'hébergeur")
                } else {
                    format!(
                        "{platform} {} absent du catalogue de l'hébergeur (proposé : {})",
                        version.unwrap_or(""),
                        available.join(", ")
                    )
                });
            }
            Err(e) if source.is_none() || req.method == Method::Catalog => {
                p.blocking.push(e);
                return;
            }
            Err(e) => catalog_note = Some(format!("catalogue illisible ({e})")),
        }
    }
    match (source, req.method) {
        (_, Method::Catalog) | (None, _) => p.blocking.push(format!(
            "{} ; {}",
            catalog_note.unwrap_or_else(|| format!("« {platform} » sans entrée au catalogue")),
            if source.is_some() {
                "voie JAR possible : method=jar (ou auto)".to_string()
            } else {
                "aucune voie JAR connue pour cette plateforme : envoyer son JAR (upload_file) puis jar_file, ou le panel".to_string()
            }
        )),
        (Some(src), _) => {
            // Version d'un JAR de jeu : celle de Minecraft ; d'un proxy : la sienne.
            match jar::fetch_pick(src, version, req.build.as_deref()).await {
                Ok(pick) => {
                    if let Some(n) = catalog_note {
                        p.warnings.push(format!("{n} : voie JAR ({})", src.label()));
                    }
                    p.warnings.extend(pick.notes.iter().cloned());
                    if p.version.is_none() || is_proxy(platform) {
                        p.version = Some(pick.version.clone());
                    }
                    let remote = req.jar_file.clone().unwrap_or_else(|| pick.file.clone());
                    if !jar::valid_file_name(&remote) {
                        p.blocking.push(format!("nom de fichier « {remote} » refusé"));
                        return;
                    }
                    p.route = Route::Jar { pick, remote };
                }
                Err(e) => p.blocking.push(format!(
                    "{}{e}",
                    catalog_note.map(|n| format!("{n} ; voie JAR : ")).unwrap_or_default()
                )),
            }
        }
    }
}

// --- Écritures ------------------------------------------------------------------------------------------------

/// Écriture par un outil du MCP officiel, comme une étape de la garde des agents (journal du
/// changeset ; non annulable, raison dans `agent_guard::undo::plan_for`) : même enveloppe que
/// `mcp::dispatch` pour un outil local (`before_write` / `after_write`, cache purgé). Sortie
/// anonymisée.
pub(crate) async fn official_write(
    core: &Core,
    toolset: &str,
    tool: &str,
    args: Value,
) -> Result<String, String> {
    let token = core.token().map_err(|e| e.to_string())?;
    let step = crate::agent_guard::before_write(core, tool, &args).await;
    let out = crate::official_mcp::call_tool(&token, toolset, false, tool, args)
        .await
        .map(|t| core.redact_ai(&t))
        .map_err(|e| core.redact_ai(&e));
    core.cache().clear();
    crate::agent_guard::after_write(core, step, out).await
}

/// `confirm_name` correspond-il au nom exact (espaces de bord ignorés, casse comprise) ?
pub(crate) fn confirmation_matches(expected: &str, given: &str) -> bool {
    let expected = expected.trim();
    !expected.is_empty() && given.trim() == expected
}

/// Confirmation de `change_version` : l'humain retape le nom (élicitation), sinon `given`
/// (`confirm_name` donné par l'utilisateur), vérifié contre le nom réel. Aucun effet avant.
pub(crate) async fn confirm(p: &Prepared, given: Option<String>) -> Result<(), String> {
    let name = p.current.name.clone();
    let what: Vec<String> = ops(p).iter().map(|o| o.summary.clone()).collect();
    let message = format!(
        "Changement de plateforme ou de version du serveur {} : {}. Opération DESTRUCTIVE de l'hébergeur{}.",
        p.server_id,
        what.join(" ; "),
        if p.snapshot { " (snapshot prise d'abord)" } else { ", SANS snapshot préalable" }
    );
    let typed = mcp_ctx::confirm_or_given(&message, || async { Ok(name.clone()) }, given)
        .await
        .map_err(|e| {
            if e.contains("requis") {
                "Confirmation requise : `confirm_name` = nom exact du serveur (casse comprise), à demander à l'utilisateur — jamais deviné. Rien n'a été modifié.".to_string()
            } else {
                e
            }
        })?;
    if confirmation_matches(&name, &typed) {
        Ok(())
    } else {
        Err("Refusé : `confirm_name` ne correspond pas au nom exact du serveur (casse comprise). Rien n'a été modifié.".into())
    }
}

/// Résultat d'une opération.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpResult {
    pub tool: &'static str,
    pub summary: String,
    /// `réussie`, `échouée`, `non exécutée`.
    pub outcome: &'static str,
    pub detail: Option<String>,
}

impl OpResult {
    fn json(&self) -> Value {
        json!({ "tool": self.tool, "summary": self.summary, "outcome": self.outcome, "detail": self.detail })
    }
}

/// Envoie le JAR choisi : téléchargement vérifié, fichier temporaire local, `upload_file` passé
/// par la garde (étape annulable : le fichier est retiré), fichier temporaire effacé.
async fn upload_jar(
    core: &Core,
    server_id: i64,
    pick: &JarPick,
    remote: &str,
) -> Result<Option<String>, String> {
    if let Some(Some(size)) = root_file(core, server_id, remote).await {
        if pick.size == Some(size) {
            return Ok(Some(format!(
                "« {remote} » déjà présent (même taille) : non renvoyé"
            )));
        }
    }
    let (bytes, sha) = jar::download(pick).await?;
    let local = std::env::temp_dir().join(format!(
        "minestrator-{}-{}-{remote}",
        std::process::id(),
        crate::store::now_secs()
    ));
    tokio::fs::write(&local, &bytes)
        .await
        .map_err(|e| format!("fichier temporaire impossible : {e}"))?;
    let args = json!({ "server_id": server_id, "local_path": local.to_string_lossy(), "remote_path": format!("/{remote}") });
    let out = crate::agent_guard::guarded_write(core, "upload_file", args).await;
    let _ = tokio::fs::remove_file(&local).await;
    let note = out?;
    Ok(Some(format!(
        "{} octets envoyés (sha256 {}){}",
        bytes.len(),
        &sha[..16.min(sha.len())],
        note.map(|n| format!(" ; {n}")).unwrap_or_default()
    )))
}

/// Exécute les opérations du plan, dans l'ordre ; arrêt au premier échec (les suivantes en
/// dépendent) ou sur annulation. `snapshot` : snapshot attendue avant un `change_version`.
pub(crate) async fn execute(core: &Core, p: &Prepared, snapshot: bool) -> (Vec<OpResult>, Value) {
    let list = ops(p);
    let total = list.len() as f64 + 1.0;
    let mut results = Vec::new();
    let mut snap = Value::Null;
    let mut stop: Option<String> = None;
    if snapshot && needs_confirmation(p) {
        mcp_ctx::progress(0.0, Some(total), "snapshot préalable").await;
        let name = format!(
            "Avant changement de version {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        match core.create_snapshot(p.server_id, &name).await {
            Ok(job) => match Box::pin(crate::manifest::wait_snapshot(core, job, &name)).await {
                Ok(()) => snap = json!({ "name": name, "job_id": job }),
                Err(e) => stop = Some(format!("{e} : rien n'a été modifié (snapshot=false pour s'en passer)")),
            },
            Err(e) => stop = Some(format!("snapshot préalable impossible ({e}) : rien n'a été modifié (snapshot=false pour s'en passer)")),
        }
    }
    for (i, op) in list.iter().enumerate() {
        let base = |outcome: &'static str, detail: Option<String>| OpResult {
            tool: op.tool,
            summary: op.summary.clone(),
            outcome,
            detail,
        };
        if let Some(why) = &stop {
            results.push(base("non exécutée", Some(why.clone())));
            continue;
        }
        if mcp_ctx::cancelled() {
            stop = Some("annulé".into());
            results.push(base("non exécutée", Some("annulé".into())));
            continue;
        }
        mcp_ctx::progress(i as f64 + 1.0, Some(total), &op.summary).await;
        let out = match (&p.route, op.tool, op.toolset) {
            (Route::Jar { pick, remote }, "upload_file", _) => {
                upload_jar(core, p.server_id, pick, remote).await
            }
            (_, tool, Some(toolset)) => official_write(core, toolset, tool, op.args.clone())
                .await
                .map(|t| Some(crate::util::truncate_on_boundary(t.trim(), 600, "…"))),
            _ => Err("opération inconnue".into()),
        };
        match out {
            Ok(detail) => results.push(base("réussie", detail)),
            Err(e) => {
                stop = Some(format!("arrêt après l'échec de {}", op.tool));
                results.push(base("échouée", Some(e)));
            }
        }
    }
    mcp_ctx::progress(total, Some(total), "terminé").await;
    (results, snap)
}

/// La voie change-t-elle la plateforme ou la version (contenu à résoudre de nouveau ensuite) ?
pub(crate) fn changes_platform(p: &Prepared) -> bool {
    !matches!(p.route, Route::Unchanged)
}

/// Résultat de chaque étape d'un manifeste (`apply_official`) d'après les opérations exécutées :
/// `(identifiant, outcome, détail)`. Une étape sans opération est `conforme` (rien à faire) ou
/// `ignorée` (avec la raison).
pub(crate) fn outcomes_for_steps(
    p: &Prepared,
    ids: &[&str],
    results: &[OpResult],
) -> Vec<(String, &'static str, Option<String>)> {
    let jar_route = matches!(p.route, Route::Jar { .. });
    ids.iter()
        .map(|id| {
            let tools: &[&str] = match *id {
                "server.game_version" | "server.loader" if jar_route => {
                    &["upload_file", "set_startup_config"]
                }
                "server.game_version" | "server.loader" => &["change_version"],
                "server.java" => &["manage_java"],
                "startup.jar_file" => &["upload_file", "set_startup_config"],
                _ => &[],
            };
            let mine: Vec<&OpResult> = results.iter().filter(|r| tools.contains(&r.tool)).collect();
            let details = |rs: &[&OpResult]| {
                let d: Vec<String> = rs
                    .iter()
                    .map(|r| match &r.detail {
                        Some(x) => format!("{} : {x}", r.tool),
                        None => r.tool.to_string(),
                    })
                    .collect();
                Some(d.join(" ; ")).filter(|s| !s.is_empty())
            };
            let (outcome, detail) = if mine.is_empty() {
                let unchanged = match *id {
                    "server.java" => p.java.is_none() && p.blocking.is_empty(),
                    "startup.jar_file" => false,
                    _ => matches!(p.route, Route::Unchanged),
                };
                let why = if p.warnings.is_empty() {
                    None
                } else {
                    Some(p.warnings.join(" ; "))
                };
                (if unchanged { "conforme" } else { "ignorée" }, why)
            } else if let Some(f) = mine.iter().find(|r| r.outcome == "échouée") {
                ("échouée", details(&[*f]))
            } else if mine.iter().any(|r| r.outcome != "réussie") {
                ("non exécutée", details(&mine))
            } else {
                ("réussie", details(&mine))
            };
            (id.to_string(), outcome, detail)
        })
        .collect()
}

// --- Rendu et outil -------------------------------------------------------------------------------------------

/// Rendu JSON du plan (avant ou après exécution).
pub(crate) fn render(p: &Prepared, dry_run: bool) -> Value {
    let route = match &p.route {
        Route::Unchanged => Value::Null,
        Route::Loader {
            loader,
            game_version,
            build,
            latest_stable,
        } => {
            json!({ "loader": loader, "game_version": game_version, "build": build, "latest_stable": latest_stable })
        }
        Route::Catalog { entry } => entry.describe(),
        Route::CatalogId { version_id } => json!({ "version_id": version_id }),
        Route::Jar { pick, remote } => {
            let mut d = pick.describe();
            d["remote_file"] = json!(remote);
            d
        }
    };
    let list: Vec<Value> = ops(p)
        .iter()
        .map(|o| json!({ "tool": o.tool, "official": o.toolset.is_some(), "arguments": o.args, "summary": o.summary }))
        .collect();
    let mut out = json!({
        "server_id": p.server_id,
        "server": p.current.name,
        "current": {
            "platform": p.current.platform,
            "game_version": p.current.game_version,
            "java": p.current.java,
            "java_label": p.current.java_label,
            "jar_file": p.current.jar_file,
        },
        "target": {
            "platform": p.platform,
            "version": p.version,
            "route": p.route.label(),
            "java": p.java.as_ref().map(|j| j.major),
            "jar_file": p.startup_file,
        },
        "resolved": route,
        "operations": list,
        "dry_run": dry_run,
    });
    if let Some(j) = &p.java {
        out["resolved_java"] = json!({ "major": j.major, "java_version_id": j.option.id, "name": j.option.name, "reason": j.reason });
    }
    if needs_confirmation(p) {
        out["confirmation"] = json!("change_version est DESTRUCTIF : confirmation de l'utilisateur exigée avant tout effet (son client la lui demande s'il sait le faire, sinon `confirm_name` = nom exact du serveur, donné par lui — jamais deviné)");
        out["snapshot"] = json!(p.snapshot);
    }
    if !p.warnings.is_empty() {
        out["warnings"] = json!(p.warnings);
    }
    if !p.blocking.is_empty() {
        out["blocking"] = json!(p.blocking);
    }
    out
}

/// Suite à donner après une application réussie.
fn next_after(p: &Prepared) -> String {
    let mut s = Vec::new();
    if needs_confirmation(p) {
        s.push("change_version est asynchrone : suis l'installation avec server_status ou wait_for (state running), puis vérifie la version (export_manifest ou plan_manifest)");
    }
    if p.java.is_some() || p.startup_file.is_some() {
        s.push("redémarre le serveur (power_action restart) pour appliquer Java et le fichier de démarrage, puis tail_console / error_digest");
    }
    if matches!(p.route, Route::Jar { .. }) {
        s.push("le JAR envoyé est une étape annulable (rollback_changeset retire le fichier) ; le fichier de démarrage, lui, se remet avec set_server_platform jar_file=<ancien>");
    }
    if p.platform.as_deref().is_some_and(is_proxy) {
        s.push("proxy : network_topology puis link_proxy pour relier les serveurs de jeu");
    }
    s.join(" ; ")
}

/// Outil MCP `set_server_platform`.
pub(crate) async fn set_server_platform_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = Request::parse(args)?;
    mcp_ctx::progress(0.0, None, "lecture du serveur et du catalogue").await;
    let p = Box::pin(prepare(core, &req)).await?;
    let ops_list = ops(&p);
    let mut out = render(&p, req.dry_run);
    if !p.blocking.is_empty() {
        out["next"] = json!("rien n'a été modifié : corrige les points de `blocking` puis relance (dry_run d'abord)");
        let text = crate::config_edit::render_for_ai(core, out);
        return if req.dry_run { Ok(text) } else { Err(text) };
    }
    if ops_list.is_empty() {
        out["next"] = json!("rien à changer");
        return Ok(crate::config_edit::render_for_ai(core, out));
    }
    if req.dry_run {
        out["next"] = json!(if needs_confirmation(&p) {
            "simulation : montre ce plan à l'utilisateur ; après son accord, relance avec dry_run=false (confirmation du nom exact du serveur demandée, ou `confirm_name` donné par lui)"
        } else {
            "simulation : relance avec dry_run=false pour appliquer"
        });
        return Ok(crate::config_edit::render_for_ai(core, out));
    }
    // Confirmation AVANT tout effet (seules des lectures précèdent : rejouables sans risque).
    if needs_confirmation(&p) {
        confirm(&p, req.confirm_name.clone()).await?;
    }
    let (results, snap) = Box::pin(execute(core, &p, p.snapshot)).await;
    let failed = results.iter().any(|r| r.outcome != "réussie");
    out["results"] = json!(results.iter().map(OpResult::json).collect::<Vec<_>>());
    if !snap.is_null() {
        out["snapshot_taken"] = snap;
    }
    out["status"] = json!(if failed {
        "terminé avec un échec"
    } else {
        "terminé"
    });
    out["next"] = json!(if failed {
        "une opération a échoué : les suivantes n'ont pas été faites ; corrige puis relance (les étapes réussies sont dans list_changesets)".to_string()
    } else {
        next_after(&p)
    });
    let text = crate::config_edit::render_for_ai(core, out);
    if failed {
        Err(text)
    } else {
        Ok(text)
    }
}
