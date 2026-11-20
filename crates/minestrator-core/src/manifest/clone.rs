//! `clone_server` : copie d'un serveur dans une MyBox, pour tester une intervention avant la prod.
//!
//! Étapes : vérifications (jeu, ressources, localisation, champs requis) → création du serveur
//! (`mybox_create_server`, mêmes validations que l'outil) → attente de la fin de l'installation
//! (sondage de l'état) → arrêt de la copie si l'hébergeur l'a démarrée → copie des fichiers de la
//! source (`copy_between_servers`, par archive au-delà d'un certain volume), mondes exclus par
//! défaut → application du manifeste exporté de la source (commande de démarrage, réglages,
//! tâches planifiées…), sans snapshot (serveur neuf).
//!
//! Dès la création, l'identifiant du nouveau serveur figure dans la sortie, **même en cas
//! d'échec** d'une étape suivante (message d'erreur `created_server_id=<id>`) : un serveur créé
//! ne passe jamais inaperçu. [`created_server_id`] le relit pour la garde des agents.

use super::{apply, export, ApplyOptions};
use crate::Core;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Attente maximale de l'installation par l'hébergeur.
const INSTALL_WAIT: Duration = Duration::from_secs(20 * 60);
const INSTALL_POLL: Duration = Duration::from_secs(5);
/// Attente de l'arrêt du serveur copié.
const STOP_WAIT: Duration = Duration::from_secs(120);
/// Dossiers jamais copiés (régénérés par le serveur).
const DEFAULT_EXCLUDES: &[&str] = &["/logs", "/crash-reports", "/debug"];
/// Sources par appel de copie (`copy_between_servers` en accepte 50).
const COPY_BATCH: usize = 50;

/// Demande validée.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CloneRequest {
    pub source: i64,
    pub mybox: i64,
    pub name: String,
    pub cpu: Option<i64>,
    pub ram: Option<i64>,
    pub include_world: bool,
    pub exclude: Vec<String>,
    pub location_id: Option<i64>,
    pub fields: Vec<crate::FieldValue>,
}

pub(crate) fn parse_request(args: &Value) -> Result<CloneRequest, String> {
    let int = |k: &str| args.get(k).and_then(Value::as_i64);
    let positive = |k: &str| -> Result<Option<i64>, String> {
        match int(k) {
            Some(n) if n < 1 => Err(format!("`{k}` : entier ≥ 1 attendu (cœurs ou Go)")),
            other => Ok(other),
        }
    };
    let exclude = match args.get("exclude") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or("`exclude` : tableau de chemins attendu".to_string())
                    .and_then(|p| crate::fleet::normalize_path(p).map_err(|e| e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("`exclude` : tableau de chemins attendu".into()),
    };
    if exclude.iter().any(|p| p == "/") {
        return Err("`exclude` : « / » exclurait tout".into());
    }
    let fields = match args.get("required_fields") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| crate::FieldValue {
                key: k.clone(),
                value: v
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            })
            .collect(),
        Some(_) => return Err("`required_fields` doit être un objet { CLÉ: valeur }".into()),
    };
    Ok(CloneRequest {
        source: int("source_server_id").ok_or("paramètre `source_server_id` (entier) requis")?,
        mybox: int("mybox_id").ok_or("paramètre `mybox_id` (entier) requis")?,
        name: crate::mybox::valid_name(
            args.get("name")
                .and_then(Value::as_str)
                .ok_or("paramètre `name` (texte) requis")?,
        )
        .map_err(|e| e.to_string())?,
        cpu: positive("cpu")?,
        ram: positive("ram")?,
        include_world: args
            .get("include_world")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        exclude,
        location_id: int("location_id"),
        fields,
    })
}

/// Ressources du clone : celles demandées, sinon celles de la source si la MyBox a la place,
/// sinon ce qui reste (au moins 1 cœur et 1 Go). Renvoie aussi une remarque éventuelle.
pub(crate) fn choose_resources(
    source: (i64, i64),
    remaining: (i64, i64),
    requested: (Option<i64>, Option<i64>),
) -> Result<(i64, i64, Option<String>), String> {
    let pick = |want: Option<i64>,
                src: i64,
                left: i64,
                unit: &str|
     -> Result<(i64, bool), String> {
        match want {
            Some(w) if w > left => Err(format!("{w} {unit} demandé(s), il en reste {left} dans la MyBox")),
            Some(w) => Ok((w, false)),
            None if src.max(1) <= left => Ok((src.max(1), false)),
            None if left >= 1 => Ok((left, true)),
            None => Err(format!("plus aucun {unit} libre dans la MyBox (mybox_capacity ; mettre un serveur en pause en libère)")),
        }
    };
    let (cpu, cpu_cut) = pick(requested.0, source.0, remaining.0, "cœur")?;
    let (ram, ram_cut) = pick(requested.1, source.1, remaining.1, "Go de RAM")?;
    let note = (cpu_cut || ram_cut).then(|| format!("la MyBox n'a pas la place de copier les ressources de la source ({} cœur(s), {} Go) : clone créé avec {cpu} cœur(s) et {ram} Go", source.0, source.1));
    Ok((cpu, ram, note))
}

/// Sort d'un chemin de la source à copier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyDecision {
    Copy,
    Skip,
    /// Un chemin exclu est dedans : copier son contenu un niveau plus bas.
    Expand,
}

pub(crate) fn decide(path: &str, is_dir: bool, excluded: &HashSet<String>) -> CopyDecision {
    if excluded.contains(path) {
        return CopyDecision::Skip;
    }
    let prefix = format!("{path}/");
    if is_dir && excluded.iter().any(|e| e.starts_with(&prefix)) {
        return CopyDecision::Expand;
    }
    CopyDecision::Copy
}

/// Dossiers de monde à la racine : `level-name` (et ses dimensions) plus tout dossier qui
/// contient un `level.dat`.
fn world_dirs(level_name: &str, with_level_dat: &[String]) -> HashSet<String> {
    let mut out: HashSet<String> = with_level_dat.iter().cloned().collect();
    let base = if level_name.trim().is_empty() {
        "world"
    } else {
        level_name.trim()
    };
    for suffix in ["", "_nether", "_the_end"] {
        out.insert(format!("/{base}{suffix}"));
    }
    out
}

/// État d'installation lu dans `/live` : `Ok(Some(état))` installé, `Ok(None)` en cours.
fn install_state(status: Option<&str>, state: &str) -> Result<Option<String>, String> {
    let s = status.unwrap_or_default().to_ascii_lowercase();
    if s.contains("install") && (s.contains("fail") || s.contains("error")) {
        return Err(format!(
            "l'installation a échoué côté hébergeur (statut « {s} »)"
        ));
    }
    if s.contains("install") {
        return Ok(None);
    }
    Ok(Some(state.to_string()))
}

impl Core {
    /// Attend la fin de l'installation d'un serveur neuf ; renvoie son état (`offline`…). Partagé
    /// avec `apply_network` (`blueprint.rs`).
    pub(crate) async fn wait_installed(&self, id: i64) -> Result<(String, u64), String> {
        let start = Instant::now();
        let mut last_err = String::new();
        loop {
            match self.server_live(id).await {
                Ok(live) => {
                    if let Some(state) = install_state(live.status.as_deref(), &live.state)? {
                        return Ok((state, start.elapsed().as_secs()));
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
            if start.elapsed() > INSTALL_WAIT {
                return Err(format!(
                    "installation toujours en cours après {} min{}",
                    INSTALL_WAIT.as_secs() / 60,
                    if last_err.is_empty() {
                        String::new()
                    } else {
                        format!(" ({last_err})")
                    }
                ));
            }
            if crate::mcp_ctx::cancelled() {
                return Err("annulé pendant l'installation".into());
            }
            crate::mcp_ctx::progress(
                start.elapsed().as_secs_f64(),
                None,
                &format!(
                    "installation du serveur {id} par l'hébergeur ({} s)",
                    start.elapsed().as_secs()
                ),
            )
            .await;
            tokio::time::sleep(INSTALL_POLL).await;
        }
    }

    /// Arrête le serveur s'il tourne (l'hébergeur peut démarrer un serveur neuf). Partagé avec
    /// `apply_network` (`blueprint.rs`).
    pub(crate) async fn ensure_stopped(&self, id: i64, state: &str) -> Result<(), String> {
        if !matches!(state, "online" | "starting" | "running") {
            return Ok(());
        }
        self.power_action(id, "stop")
            .await
            .map_err(|e| e.to_string())?;
        let start = Instant::now();
        while start.elapsed() < STOP_WAIT {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if self
                .server_live(id)
                .await
                .is_ok_and(|l| l.state == "offline")
            {
                return Ok(());
            }
        }
        self.power_action(id, "kill")
            .await
            .map_err(|e| e.to_string())
    }

    /// Chemins de la source à copier, groupés par dossier parent (cible = même dossier).
    async fn clone_sources(
        &self,
        source: i64,
        excluded: &HashSet<String>,
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        let mut stack = vec!["/".to_string()];
        let mut guard = 0;
        while let Some(dir) = stack.pop() {
            guard += 1;
            if guard > 200 {
                return Err(
                    "trop de dossiers à détailler pour les exclusions : simplifie `exclude`".into(),
                );
            }
            let entries = self
                .sftp_list(source, &dir)
                .await
                .map_err(|e| format!("liste de « {dir} » : {e}"))?;
            let mut keep = Vec::new();
            for e in entries {
                let path = if dir == "/" {
                    format!("/{}", e.name)
                } else {
                    format!("{dir}/{}", e.name)
                };
                match decide(&path, e.is_dir, excluded) {
                    CopyDecision::Copy => keep.push(path),
                    CopyDecision::Skip => {}
                    CopyDecision::Expand => stack.push(path),
                }
            }
            if !keep.is_empty() {
                groups.push((dir, keep));
            }
        }
        Ok(groups)
    }
}

/// Outil `clone_server`.
pub(crate) async fn clone_server_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = parse_request(args)?;
    let mut warnings: Vec<String> = Vec::new();

    // 1. Source, ressources, jeu, localisation : tout est vérifié avant de créer quoi que ce soit.
    let overview = core.list_servers().await.map_err(|e| e.to_string())?;
    let src = overview
        .servers
        .iter()
        .find(|s| s.id == req.source)
        .ok_or_else(|| format!("serveur source {} introuvable sur ce compte", req.source))?
        .clone();
    let egg_id = src
        .egg_id
        .ok_or("jeu (egg) de la source inconnu : création impossible")?;
    let (src_cap, dst_cap, catalog) = tokio::join!(
        core.mybox_capacity(src.mybox_id),
        core.mybox_capacity(req.mybox),
        core.server_catalog()
    );
    let (src_cap, dst_cap, catalog) = (
        src_cap.map_err(|e| e.to_string())?,
        dst_cap.map_err(|e| e.to_string())?,
        catalog.map_err(|e| e.to_string())?,
    );
    let slot = src_cap
        .servers
        .iter()
        .find(|s| s.id == req.source)
        .cloned()
        .unwrap_or_default();
    let (cpu, ram, note) = choose_resources(
        (slot.cpu, slot.ram),
        (dst_cap.cpu.remaining, dst_cap.ram.remaining),
        (req.cpu, req.ram),
    )?;
    warnings.extend(note);
    let egg = catalog
        .eggs
        .iter()
        .filter(|e| e.id == egg_id)
        .min_by_key(|e| e.experimental)
        .ok_or_else(|| {
            format!(
                "jeu {egg_id} ({}) absent du catalogue de création",
                src.egg_name
            )
        })?;
    let missing: Vec<&str> = egg
        .required_fields
        .iter()
        .filter(|f| {
            !req.fields
                .iter()
                .any(|v| v.key == f.key && !v.value.trim().is_empty())
        })
        .map(|f| f.key.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(format!("le jeu {} demande des champs à la création : {} ; passe-les dans `required_fields` (les valeurs de la source ne sont pas lisibles)", egg.name, missing.join(", ")));
    }
    let location_id = match (req.location_id, dst_cap.pro) {
        (Some(id), _) => Some(id),
        (None, true) => None,
        (None, false) => {
            let same = catalog.locations.iter().find(|l| {
                l.in_stock
                    && l.city.eq_ignore_ascii_case(&slot.location)
                    && (slot.country_iso.is_empty()
                        || l.country_iso.eq_ignore_ascii_case(&slot.country_iso))
            });
            let any = catalog.locations.iter().find(|l| l.in_stock);
            match same.or(any) {
                Some(l) => {
                    if same.is_none() {
                        warnings.push(format!(
                            "localisation de la source ({}) indisponible : clone créé à {}",
                            slot.location, l.city
                        ));
                    }
                    Some(l.id)
                }
                None => {
                    return Err(
                        "aucune localisation en stock : précise `location_id` (server_catalog)"
                            .into(),
                    )
                }
            }
        }
    };

    // 2. Manifeste de la source (lecture seule, interne : jamais montré tel quel au modèle).
    crate::mcp_ctx::progress(0.0, None, "lecture de la source").await;
    let (mut manifest, notes) = Box::pin(export(core, req.source, &[], true, false)).await?;
    warnings.extend(notes.into_iter().filter(|n| n.contains("illisible")));
    if let Some(s) = manifest.server.as_mut() {
        s.name = Some(req.name.clone());
    }
    let level_name = manifest
        .properties
        .as_ref()
        .and_then(|p| p.get("level-name"))
        .and_then(Value::as_str)
        .unwrap_or("world")
        .to_string();

    // Mondes à exclure : `level-name` et tout dossier racine qui contient un level.dat.
    let mut excluded: HashSet<String> = DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect();
    excluded.extend(req.exclude.iter().cloned());
    if !req.include_world {
        let root = core
            .sftp_list(req.source, "/")
            .await
            .map_err(|e| format!("source illisible par SFTP : {e}"))?;
        let mut with_level = Vec::new();
        for e in root.iter().filter(|e| e.is_dir) {
            let p = format!("/{}/level.dat", e.name);
            if core.sftp_file_size(req.source, &p).await.is_ok() {
                with_level.push(format!("/{}", e.name));
            }
        }
        excluded.extend(world_dirs(&level_name, &with_level));
    } else if core
        .server_live(req.source)
        .await
        .is_ok_and(|l| l.state == "online")
    {
        warnings.push("monde copié pendant que la source tourne : fais `save-all` avant pour un monde cohérent (ou arrête-la)".into());
    }
    if crate::mcp_ctx::cancelled() {
        return Err("annulé avant la création : rien n'a été créé".into());
    }

    // 3. Création. À partir d'ici, l'identifiant figure dans toute sortie.
    crate::mcp_ctx::progress(0.0, None, "création du serveur").await;
    let created = core
        .mybox_create_server(crate::CreateServerInput {
            mybox_id: req.mybox,
            name: req.name.clone(),
            egg_id,
            experimental: egg.experimental,
            location_id,
            cpu,
            ram,
            fields: req.fields.clone(),
        })
        .await
        .map_err(|e| format!("création refusée : {e} (rien n'a été créé)"))?;
    let new_id = created.server_id;
    let fail = |step: &str, e: String| {
        format!("{step} : {e} — le serveur a bien été créé (created_server_id={new_id}) : reprends avec copy_between_servers / apply_manifest, ou supprime-le (mybox_delete_server)")
    };
    if new_id <= 0 {
        return Err(
            "serveur créé mais identifiant non renvoyé par l'API : vérifie list_servers".into(),
        );
    }
    // Garde des agents : en mode constructeur, le serveur créé devient modifiable par la session,
    // même si la suite du clonage échoue (l'agent peut reprendre ou le supprimer).
    crate::agent_guard::record_created_server_here(new_id);

    // 4. Installation par l'hébergeur, puis arrêt s'il a été démarré.
    let (state, install_s) = core
        .wait_installed(new_id)
        .await
        .map_err(|e| fail("installation", e))?;
    core.ensure_stopped(new_id, &state)
        .await
        .map_err(|e| fail("arrêt du clone", e))?;
    if crate::mcp_ctx::cancelled() {
        return Err(fail(
            "annulé",
            "après l'installation, avant la copie".into(),
        ));
    }

    // Ports et adresse du clone (attribués par l'hébergeur), à remettre après la copie de
    // server.properties : sinon le clone écouterait sur le port de la source.
    let own_network: Vec<crate::config_edit::EditRequest> =
        match core.server_properties(new_id).await {
            Ok(p) => p
                .properties
                .into_iter()
                .filter(|(k, _)| super::ALLOCATION_KEYS.contains(&k.as_str()))
                .map(|(key, v)| crate::config_edit::EditRequest {
                    key,
                    value: Value::String(v),
                })
                .collect(),
            Err(_) => Vec::new(),
        };

    // 5. Copie des fichiers.
    let groups = core
        .clone_sources(req.source, &excluded)
        .await
        .map_err(|e| fail("préparation de la copie", e))?;
    let total_batches: usize = groups
        .iter()
        .map(|(_, p)| p.len().div_ceil(COPY_BATCH))
        .sum();
    let mut batches = Vec::new();
    let mut n = 0;
    for (dir, paths) in &groups {
        for chunk in paths.chunks(COPY_BATCH) {
            if crate::mcp_ctx::cancelled() {
                return Err(fail(
                    "annulé",
                    format!("pendant la copie ({n}/{total_batches} lot(s) copiés)"),
                ));
            }
            n += 1;
            crate::mcp_ctx::progress(
                n as f64,
                Some(total_batches as f64),
                &format!("copie vers « {dir} » ({n}/{total_batches})"),
            )
            .await;
            let out = Box::pin(crate::fleet::copy_tool(
                core,
                &json!({ "source_server_id": req.source, "source_paths": chunk, "target_server_id": new_id, "target_dir": dir, "overwrite": true, "strategy": "auto" }),
            ))
            .await
            .map_err(|e| fail(&format!("copie vers « {dir} »"), e.to_string()))?;
            let r: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
            batches.push(json!({
                "target_dir": dir,
                "status": r["status"],
                "strategy": r["strategy"],
                "copied_files": r["copied_files"],
                "copied": r["copied"],
                "skipped_count": r["skipped_count"],
                "skipped": r["skipped"],
                "interrupted": r["interrupted"],
            }));
            if !r["interrupted"].is_null() {
                return Err(fail(
                    &format!("copie vers « {dir} » interrompue"),
                    r["interrupted"].to_string(),
                ));
            }
        }
    }

    if own_network.is_empty() {
        warnings.push("server.properties du clone absent avant la copie : ports de la source recopiés, vérifie server-port et query.port au premier démarrage (le panel les réécrit en principe)".into());
    } else if let Err(e) = core
        .config_edit(
            new_id,
            "/server.properties",
            &own_network,
            crate::config_edit::EditOptions::default(),
            false,
        )
        .await
    {
        warnings.push(format!("ports du clone non rétablis dans server.properties ({e}) : vérifie server-port et query.port"));
    }

    // 6. Manifeste de la source appliqué au clone (démarrage, réglages, tâches…), sans snapshot.
    let report = Box::pin(apply(
        core,
        new_id,
        &manifest,
        &ApplyOptions {
            steps: Vec::new(),
            snapshot: false,
            // Version, plateforme, Java, JAR : jamais appliqués au clone (même jeu que la source).
            ..Default::default()
        },
    ))
    .await
    .map_err(|e| fail("application du manifeste", e))?;
    let mut excluded_list: Vec<String> = excluded.into_iter().collect();
    excluded_list.sort();
    let out = json!({
        "created_server_id": new_id,
        "name": req.name,
        "mybox_id": created.mybox_id,
        "cpu": cpu,
        "ram": ram,
        "address": created.ip,
        "source_server_id": req.source,
        "installation": { "duration_s": install_s, "state_after": state },
        "copy": { "excluded": excluded_list, "worlds_copied": req.include_world, "batches": batches },
        "manifest": report,
        "warnings": warnings,
        "next": "clone arrêté : vérifie les étapes `manual` du rapport (version du jeu, Java), puis démarre-le (power_action start). Les mondes ne sont copiés qu'avec include_world=true.",
    });
    Ok(crate::config_edit::render_for_ai(core, out))
}
