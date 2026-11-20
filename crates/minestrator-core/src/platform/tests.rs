//! Tests de `set_server_platform` : parties pures, sur des réponses relevées en réel (lecture
//! seule, 2026-11-19 / 2026-11-20 : catalogue de l'hébergeur, versions de Java, API PaperMC et
//! Purpur ; identifiants et empreintes réels, noms de serveurs remplacés).

use super::jar::{self, Source};
use super::*;
use crate::models::Startup;
use sha2::Digest as _;

/// `search_game_content {type: version, query: velocity}`.
const CATALOG_VELOCITY: &str = r#"{ "versions": [ { "id": 3993, "name": "Velocity", "category": "velocity", "version": "4.3.1", "versionmc": "1.8 et plus", "ram": 1024, "forge": 0, "neoforge": 0, "fabric": 0 } ] }"#;
/// `search_game_content {type: version, query: bungeecord}`.
const CATALOG_BUNGEE: &str = r#"{ "versions": [ { "id": 4010, "name": "BungeeCord", "category": "bungeecord", "version": "", "versionmc": "1.8 et plus", "ram": 1024, "forge": 0, "neoforge": 0, "fabric": 0 } ] }"#;
/// `search_game_content {type: version, query: paper}` (extrait).
const CATALOG_PAPER: &str = r#"{ "versions": [
  { "id": 4114, "name": "Paper", "category": "paper", "version": "26.3", "versionmc": "26.3", "ram": 1024, "forge": 0, "neoforge": 0, "fabric": 0 },
  { "id": 3952, "name": "Paper", "category": "paper", "version": "1.21.11", "versionmc": "1.21.11", "ram": 1024, "forge": 0, "neoforge": 0, "fabric": 0 },
  { "id": 3804, "name": "Paper", "category": "paper", "version": "1.21.4", "versionmc": "1.21.4", "ram": 1024, "forge": 0, "neoforge": 0, "fabric": 0 } ] }"#;
/// Version absente : l'officiel renvoie d'autres versions avec un avertissement.
const CATALOG_PAPER_MISSING: &str = r#"{ "versions": [ { "id": 4114, "name": "Paper", "category": "paper", "version": "26.3", "versionmc": "26.3" } ], "warning": "⚠️ AUCUNE entrée du catalogue en version 1.21.99 pour « paper »." }"#;
/// `get_content_files {type: loader_builds, loader: fabric, game_version: 1.21.4}` (extrait).
const FABRIC_BUILDS: &str = r#"{ "type": "loader_builds", "loader": "fabric", "game_version": "1.21.4", "total_builds": 40, "returned": 40, "latest_stable": "0.20.1", "builds": [ { "build": "0.20.1", "game_version": "1.21.4" }, { "build": "0.20.0", "game_version": "1.21.4" } ] }"#;

/// `GET /site/java/versions` (extrait réel).
fn java_list() -> Vec<JavaOption> {
    parse_java_versions(&json!({ "versions": [
        { "id": 17, "name": "Java 8" }, { "id": 8, "name": "Java 8 - OpenJ9" },
        { "id": 20, "name": "Java 17" }, { "id": 53, "name": "Java 17 - Performance" },
        { "id": 35, "name": "Java 21" }, { "id": 38, "name": "Java 21 - GraalVM" }, { "id": 57, "name": "Java 21 - Performance" },
        { "id": 48, "name": "Java 25" }, { "id": 62, "name": "Java 25 - GraalVM PERF" }, { "id": 61, "name": "Java 25 - Performance" },
        { "id": 63, "name": "Java 26" }, { "id": 64, "name": "Java 26 - Performance" }
    ] }))
}

fn facts(loader: Option<&str>, version: Option<&str>, java_id: i64, image: &str) -> Facts {
    Facts {
        name: "Survie".into(),
        family: "minecraft".into(),
        egg_id: Some(1),
        egg_name: "Minecraft".into(),
        game_version: version.map(str::to_string),
        loader: loader.map(str::to_string),
        startup: Some(Startup {
            command: "java -Xms128M -Xmx4G -jar {{SERVER_JARFILE}}".into(),
            jar_file: "paper-1.21.4-232.jar".into(),
            java_memory: 30,
            image: image.into(),
            java_version: java_id,
        }),
    }
}

fn current(loader: Option<&str>, version: Option<&str>) -> Current {
    Current::from_facts(
        &facts(loader, version, 57, "reg.sttr.io/java/21_perf:latest"),
        &java_list(),
    )
}

fn req(v: Value) -> Request {
    Request::parse(&v).unwrap()
}

fn prepared(route: Route, platform: &str, version: Option<&str>) -> Prepared {
    Prepared {
        server_id: 42,
        current: current(Some("paper"), Some("1.21.4")),
        platform: Some(platform.into()),
        version: version.map(str::to_string),
        route,
        java: None,
        startup_file: None,
        reinstall: false,
        snapshot: true,
        warnings: Vec::new(),
        blocking: Vec::new(),
    }
}

fn velocity_pick(version: &str, build: &str, file: &str, java: i64) -> JarPick {
    JarPick {
        source: Source::Fill("velocity"),
        version: version.into(),
        build: build.into(),
        channel: Some("STABLE".into()),
        file: file.into(),
        url: format!("https://fill-data.papermc.io/v1/objects/x/{file}"),
        size: Some(17816778),
        sha256: Some("fe53021f3168322cb6cb68f78699866fd098df3c306e4359847a10b0d02689ef".into()),
        java_min: Some(java),
        notes: Vec::new(),
    }
}

#[test]
fn requests_parse_and_default_to_a_dry_run() {
    let r = req(json!({ "server_id": 7, "platform": "Paper" }));
    assert!(r.dry_run && r.snapshot && !r.reinstall);
    assert_eq!(
        (r.platform.as_deref(), r.method),
        (Some("paper"), Method::Auto)
    );
    assert_eq!(
        req(json!({ "server_id": 7, "platform": "Bungee Cord" }))
            .platform
            .as_deref(),
        Some("bungeecord")
    );
    assert_eq!(
        req(json!({ "server_id": 7, "platform": "NeoForged" }))
            .platform
            .as_deref(),
        Some("neoforge")
    );
    // `game_version` accepté comme alias, build numérique lu en texte.
    let r = req(json!({ "server_id": "7", "game_version": "1.21.4", "build": 563 }));
    assert_eq!(
        (r.server_id, r.version.as_deref(), r.build.as_deref()),
        (7, Some("1.21.4"), Some("563"))
    );
    let r = req(
        json!({ "server_id": 7, "jar_file": "/velocity.jar", "method": "JAR", "dry_run": false }),
    );
    assert_eq!(
        (r.jar_file.as_deref(), r.method, r.dry_run),
        (Some("velocity.jar"), Method::Jar, false)
    );
    let err = |v: Value| Request::parse(&v).unwrap_err();
    assert!(err(json!({ "platform": "paper" })).contains("server_id"));
    assert!(err(json!({ "server_id": 7 })).contains("rien à changer"));
    assert!(err(json!({ "server_id": 7, "java": 99 })).contains("java"));
    assert!(err(json!({ "server_id": 7, "jar_file": "plugins/x.jar" })).contains("jar_file"));
    assert!(err(json!({ "server_id": 7, "jar_file": "../server.jar" })).contains("jar_file"));
    assert!(
        err(json!({ "server_id": 7, "platform": "paper", "method": "panel" })).contains("method")
    );
    assert!(
        err(json!({ "server_id": 7, "version_id": 3993, "method": "jar" })).contains("version_id")
    );
}

#[test]
fn the_real_catalog_offers_velocity_and_bungeecord_but_not_every_version() {
    let (velocity, w) = parse_catalog(CATALOG_VELOCITY);
    assert!(w.is_none());
    // Proxy : une seule entrée Velocity, la dernière version.
    assert_eq!(
        choose_entry(&velocity, "velocity", None).map(|e| e.id),
        Some(3993)
    );
    assert_eq!(
        choose_entry(&velocity, "velocity", Some("4.3.1")).map(|e| e.id),
        Some(3993)
    );
    assert!(
        choose_entry(&velocity, "velocity", Some("3.4.0")).is_none(),
        "version précise : voie JAR"
    );
    // BungeeCord : entrée sans version, installée quelle que soit la version demandée.
    let (bungee, _) = parse_catalog(CATALOG_BUNGEE);
    assert_eq!(
        choose_entry(&bungee, "bungeecord", Some("1.21")).map(|e| e.id),
        Some(4010)
    );
    // Jeu : par version de Minecraft ; sans version, la plus récente.
    let (paper, _) = parse_catalog(CATALOG_PAPER);
    assert_eq!(
        choose_entry(&paper, "paper", Some("1.21.4")).map(|e| e.id),
        Some(3804)
    );
    assert_eq!(
        choose_entry(&paper, "paper", None).map(|e| e.id),
        Some(4114)
    );
    assert!(choose_entry(&paper, "spigot", Some("1.21.4")).is_none());
    // Version absente : autres versions et avertissement, jamais une autre version choisie.
    let (other, warning) = parse_catalog(CATALOG_PAPER_MISSING);
    assert!(warning.unwrap().contains("AUCUNE"));
    assert!(choose_entry(&other, "paper", Some("1.21.99")).is_none());
    // Texte illisible : rien.
    assert!(parse_catalog("pas du JSON").0.is_empty());
    let b = parse_loader_builds(FABRIC_BUILDS);
    assert_eq!(
        (b.builds.len(), b.latest_stable.as_deref()),
        (2, Some("0.20.1"))
    );
}

#[test]
fn java_options_follow_the_current_variant() {
    let list = java_list();
    let cur = current(Some("paper"), Some("1.21.4"));
    assert_eq!(
        (cur.java, cur.java_label.as_deref()),
        (Some(21), Some("Java 21 - Performance"))
    );
    assert_eq!(
        java_option(&list, 25, Some("Java 21 - Performance")).map(|j| j.id),
        Some(61)
    );
    assert_eq!(
        java_option(&list, 25, None).map(|j| j.id),
        Some(48),
        "variante simple"
    );
    assert_eq!(
        java_option(&list, 21, Some("Java 17 - GraalVM")).map(|j| j.id),
        Some(38)
    );
    assert_eq!(
        java_option(&list, 17, Some("Java 21 - GraalVM")).map(|j| j.id),
        Some(20),
        "pas de GraalVM 17 ici : simple"
    );
    assert!(java_option(&list, 11, None).is_none());
    // Id inconnu de la liste : l'image (`reg.sttr.io/java/25_perf`) donne la version.
    let c = Current::from_facts(
        &facts(
            Some("paper"),
            Some("26.3"),
            999,
            "reg.sttr.io/java/25_perf:latest",
        ),
        &list,
    );
    assert_eq!((c.java, c.java_label), (Some(25), None));
    assert_eq!(label_major("Java 8 - OpenJ9"), Some(8));
    assert_eq!(label_major("Java 1.8"), Some(8));
    assert_eq!(java_for_minecraft("26.3"), Some(25));
    assert_eq!(java_for_minecraft("1.21.4"), Some(21));
    assert_eq!(java_for_minecraft("1.20.6"), Some(21));
    assert_eq!(java_for_minecraft("1.20.4"), Some(17));
    assert_eq!(java_for_minecraft("1.18.2"), Some(17));
    assert_eq!(java_for_minecraft("1.16.5"), None);
    assert_eq!(java_for_minecraft("24w14a"), None);
}

#[test]
fn java_is_raised_to_the_minimum_but_never_lowered_automatically() {
    let list = java_list();
    let cur = current(Some("paper"), Some("1.21.4"));
    let mut w = Vec::new();
    // Requis 25, actuel 21 : relevé, même variante.
    let c = java_change(
        None,
        Some((25, "Velocity 4.3.1".into())),
        &cur,
        &list,
        &mut w,
    )
    .unwrap()
    .unwrap();
    assert_eq!((c.major, c.option.id), (25, 61));
    assert!(c.reason.contains("Velocity 4.3.1"));
    // Requis 17, actuel 21 : rien.
    assert!(
        java_change(None, Some((17, "x".into())), &cur, &list, &mut w)
            .unwrap()
            .is_none()
    );
    // Demandé plus bas que le requis : appliqué, mais signalé.
    let c = java_change(
        Some(17),
        Some((21, "Minecraft 1.21.4".into())),
        &cur,
        &list,
        &mut w,
    )
    .unwrap()
    .unwrap();
    assert_eq!(c.major, 17);
    assert!(w.last().unwrap().contains("risque"));
    // Demandé = actuel : rien.
    assert!(java_change(Some(21), None, &cur, &list, &mut w)
        .unwrap()
        .is_none());
    // Non proposé par l'hébergeur : erreur qui liste les versions.
    let e = java_change(Some(11), None, &cur, &list, &mut w).unwrap_err();
    assert!(
        e.contains("Java 11") && e.contains("21") && e.contains("25"),
        "{e}"
    );
    // Actuel illisible : avertissement, pas de changement automatique.
    let unknown = Current { java: None, ..cur };
    let mut w = Vec::new();
    assert!(
        java_change(None, Some((21, "x".into())), &unknown, &list, &mut w)
            .unwrap()
            .is_none()
    );
    assert!(w[0].contains("illisible"));
}

#[test]
fn the_target_keeps_the_world_version_when_the_platform_changes() {
    let cur = current(Some("paper"), Some("1.21.4"));
    let t = |v: Value| target(&req(v), &cur);
    // Paper → Purpur : même version de Minecraft que le monde.
    assert_eq!(
        t(json!({ "server_id": 1, "platform": "purpur" })).unwrap(),
        Some(("purpur".into(), Some("1.21.4".into())))
    );
    // Plateforme et version déjà en place : rien.
    assert_eq!(
        t(json!({ "server_id": 1, "platform": "paper" })).unwrap(),
        None
    );
    assert_eq!(
        t(json!({ "server_id": 1, "version": "1.21.4" })).unwrap(),
        None
    );
    assert_eq!(t(json!({ "server_id": 1, "java": 25 })).unwrap(), None);
    // Nouvelle version sur la même plateforme.
    assert_eq!(
        t(json!({ "server_id": 1, "version": "1.21.11" })).unwrap(),
        Some(("paper".into(), Some("1.21.11".into())))
    );
    // Vers un proxy : la version de Minecraft ne le concerne pas.
    assert_eq!(
        t(json!({ "server_id": 1, "platform": "velocity" })).unwrap(),
        Some(("velocity".into(), None))
    );
    // Plateforme actuelle inconnue (JAR explicite) : à préciser.
    let unknown = current(None, Some("1.21.4"));
    assert!(target(
        &req(json!({ "server_id": 1, "version": "1.21.5" })),
        &unknown
    )
    .unwrap_err()
    .contains("platform"));
    // Proxy : nouvelle version du proxy (voie JAR si le catalogue ne la propose pas).
    let velo = current(Some("velocity"), None);
    assert_eq!(
        target(&req(json!({ "server_id": 1, "version": "3.4.0" })), &velo).unwrap(),
        Some(("velocity".into(), Some("3.4.0".into())))
    );
}

#[test]
fn an_older_version_is_refused_unless_a_clean_reinstall_is_asked() {
    let cur = current(Some("paper"), Some("1.21.4"));
    let run = |p: &str, v: &str, reinstall: bool, jar: bool| {
        let (mut w, mut b) = (Vec::new(), Vec::new());
        version_checks(p, Some(v), &cur, reinstall, jar, &mut w, &mut b);
        (w, b)
    };
    let (_, b) = run("paper", "1.20.4", false, false);
    assert!(
        b[0].contains("ANTÉRIEURE") && b[0].contains("reinstall"),
        "{b:?}"
    );
    let (w, b) = run("paper", "1.20.4", true, false);
    assert!(b.is_empty() && w[0].contains("effacé"));
    let (_, b) = run("purpur", "1.20.4", true, true);
    assert!(b[0].contains("JAR"), "la voie JAR ne réinstalle pas");
    let (w, b) = run("paper", "1.21.11", false, false);
    assert!(w.is_empty() && b.is_empty());
    // Jeu → proxy : signalé, pas de comparaison de versions (celle du proxy n'est pas Minecraft).
    let (w, b) = run("velocity", "3.4.0", false, true);
    assert!(b.is_empty() && w[0].contains("PROXY"));
}

#[test]
fn change_version_args_never_carry_a_guessed_confirmation() {
    // Chargeur par son nom, build précis, Java avant le changement de version.
    let mut p = prepared(
        Route::Loader {
            loader: "fabric".into(),
            game_version: "1.21.4".into(),
            build: Some("0.20.1".into()),
            latest_stable: Some("0.20.1".into()),
        },
        "fabric",
        Some("1.21.4"),
    );
    p.java = Some(JavaChange {
        major: 21,
        option: JavaOption {
            id: 57,
            name: "Java 21 - Performance".into(),
            major: Some(21),
        },
        reason: "demandé".into(),
    });
    let list = ops(&p);
    assert_eq!(
        list.iter().map(|o| o.tool).collect::<Vec<_>>(),
        ["manage_java", "change_version"]
    );
    assert_eq!(
        list[0].args,
        json!({ "server_id": 42, "java_version_id": 57 })
    );
    assert_eq!(
        list[1].args,
        json!({ "server_id": 42, "loader": "fabric", "game_version": "1.21.4", "build": "0.20.1" })
    );
    assert_eq!(
        (list[0].toolset, list[1].toolset),
        (Some("config"), Some("content"))
    );
    assert!(needs_confirmation(&p));
    for o in &list {
        assert!(o.args.get("confirm_name").is_none(), "{}", o.tool);
    }
    // Entrée du catalogue d'un jeu : version de Minecraft contrôlée par l'officiel.
    let (paper, _) = parse_catalog(CATALOG_PAPER);
    let entry = choose_entry(&paper, "paper", Some("1.21.11"))
        .unwrap()
        .clone();
    let p = prepared(Route::Catalog { entry }, "paper", Some("1.21.11"));
    assert_eq!(
        change_version_args(&p).unwrap(),
        json!({ "server_id": 42, "version_id": 3952, "game_version": "1.21.11" })
    );
    // Proxy du catalogue : pas de version de Minecraft.
    let (velocity, _) = parse_catalog(CATALOG_VELOCITY);
    let mut p = prepared(
        Route::Catalog {
            entry: velocity[0].clone(),
        },
        "velocity",
        None,
    );
    assert_eq!(
        change_version_args(&p).unwrap(),
        json!({ "server_id": 42, "version_id": 3993 })
    );
    p.reinstall = true;
    assert_eq!(change_version_args(&p).unwrap()["reinstall"], true);
    let r = render(&p, true);
    assert!(r["confirmation"]
        .as_str()
        .unwrap()
        .contains("jamais deviné"));
    assert_eq!(r["operations"][0]["tool"], "change_version");
    assert_eq!(r["target"]["route"], "catalogue");
}

#[test]
fn the_jar_route_uploads_then_selects_the_file_without_change_version() {
    let pick = velocity_pick(
        "3.4.0-SNAPSHOT",
        "563",
        "velocity-3.4.0-SNAPSHOT-563.jar",
        21,
    );
    let mut p = prepared(
        Route::Jar {
            pick: pick.clone(),
            remote: pick.file.clone(),
        },
        "velocity",
        Some("3.4.0-SNAPSHOT"),
    );
    p.startup_file = Some(pick.file.clone());
    let list = ops(&p);
    assert_eq!(
        list.iter().map(|o| o.tool).collect::<Vec<_>>(),
        ["upload_file", "set_startup_config"]
    );
    assert_eq!(list[0].toolset, None, "envoi local, par la garde");
    assert_eq!(
        list[0].args["remote_path"],
        "/velocity-3.4.0-SNAPSHOT-563.jar"
    );
    assert_eq!(list[0].args["sha256"], pick.sha256.clone().unwrap());
    assert_eq!(
        list[1].args,
        json!({ "server_id": 42, "file": "velocity-3.4.0-SNAPSHOT-563.jar" })
    );
    assert!(!needs_confirmation(&p), "ni change_version ni confirmation");
    assert!(change_version_args(&p).is_none());
    let r = render(&p, true);
    assert_eq!(r["target"]["route"], "jar");
    assert_eq!(
        r["resolved"]["remote_file"],
        "velocity-3.4.0-SNAPSHOT-563.jar"
    );
    assert!(r.get("confirmation").is_none());
    // Sources : PaperMC pour les proxys et Folia, Purpur à part ; ni BungeeCord ni les chargeurs.
    assert_eq!(jar::source_for("velocity"), Some(Source::Fill("velocity")));
    assert_eq!(
        jar::source_for("waterfall"),
        Some(Source::Fill("waterfall"))
    );
    assert_eq!(jar::source_for("folia"), Some(Source::Fill("folia")));
    assert_eq!(jar::source_for("purpur"), Some(Source::Purpur));
    for none in [
        "bungeecord",
        "fabric",
        "forge",
        "spigot",
        "vanilla",
        "quilt",
    ] {
        assert_eq!(jar::source_for(none), None, "{none}");
    }
}

/// `GET https://fill.papermc.io/v3/projects/velocity/versions` (extrait réel du 2026-11-20).
fn fill_versions() -> Value {
    json!({ "versions": [
        { "version": { "id": "4.3.2-SNAPSHOT", "support": { "status": "SUPPORTED" }, "java": { "version": { "minimum": 25 } } }, "builds": [46, 47, 48, 49, 50] },
        { "version": { "id": "4.3.1", "support": { "status": "SUPPORTED" }, "java": { "version": { "minimum": 25 } } }, "builds": [45] },
        { "version": { "id": "4.1.0", "support": { "status": "UNSUPPORTED", "end": "2026-09-14" }, "java": { "version": { "minimum": 25 } } }, "builds": [22] },
        { "version": { "id": "3.5.1", "support": { "status": "DEPRECATED", "end": "2026-09-30" }, "java": { "version": { "minimum": 21 } } }, "builds": [615] },
        { "version": { "id": "3.4.0-SNAPSHOT", "support": { "status": "DEPRECATED" }, "java": { "version": { "minimum": 21 } } }, "builds": [563] }
    ] })
}

#[test]
fn papermc_versions_and_builds_are_chosen_and_verified() {
    let v = fill_versions();
    // Sans version : la plus récente publiée et supportée (pas la SNAPSHOT).
    let (id, java, notes) = jar::fill_version(&v, None).unwrap();
    assert_eq!((id.as_str(), java), ("4.3.1", Some(25)));
    assert!(notes.is_empty());
    // Version précise : Java 21, signalée comme plus maintenue.
    let (id, java, notes) = jar::fill_version(&v, Some("3.5.1")).unwrap();
    assert_eq!((id.as_str(), java), ("3.5.1", Some(21)));
    assert!(notes[0].contains("DEPRECATED"));
    assert!(jar::fill_version(&v, Some("9.9.9"))
        .unwrap_err()
        .contains("4.3.1"));
    // Builds : le dernier STABLE ; à défaut, le plus récent avec une note.
    let builds = json!([
        { "id": 562, "channel": "STABLE", "downloads": { "server:default": { "name": "velocity-3.4.0-SNAPSHOT-562.jar", "checksums": { "sha256": "AA" }, "size": 10, "url": "https://fill-data.papermc.io/v1/objects/aa/velocity-3.4.0-SNAPSHOT-562.jar" } } },
        { "id": 563, "time": "2026-01-21T19:00:31.648Z", "channel": "STABLE", "downloads": { "server:default": { "name": "velocity-3.4.0-SNAPSHOT-563.jar", "checksums": { "sha256": "fe53021f3168322cb6cb68f78699866fd098df3c306e4359847a10b0d02689ef" }, "size": 17816778, "url": "https://fill-data.papermc.io/v1/objects/fe53021f3168322cb6cb68f78699866fd098df3c306e4359847a10b0d02689ef/velocity-3.4.0-SNAPSHOT-563.jar" } } },
        { "id": 564, "channel": "BETA", "downloads": { "server:default": { "name": "velocity-3.4.0-SNAPSHOT-564.jar", "size": 1, "url": "u" } } }
    ]);
    let pick = jar::fill_build("velocity", "3.4.0-SNAPSHOT", &builds, None).unwrap();
    assert_eq!(
        (pick.build.as_str(), pick.file.as_str(), pick.size),
        ("563", "velocity-3.4.0-SNAPSHOT-563.jar", Some(17816778))
    );
    assert_eq!(
        pick.sha256.as_deref(),
        Some("fe53021f3168322cb6cb68f78699866fd098df3c306e4359847a10b0d02689ef")
    );
    assert_eq!(pick.channel.as_deref(), Some("STABLE"));
    assert_eq!(
        jar::fill_build("velocity", "3.4.0-SNAPSHOT", &builds, Some("562"))
            .unwrap()
            .sha256
            .as_deref(),
        Some("aa")
    );
    let beta_only = json!([{ "id": 50, "channel": "BETA", "downloads": { "server:default": { "name": "velocity-4.3.2-SNAPSHOT-50.jar", "size": 42392496, "url": "u" } } }]);
    let pick = jar::fill_build("velocity", "4.3.2-SNAPSHOT", &beta_only, None).unwrap();
    assert!(pick.notes[0].contains("BETA"));
    assert!(jar::fill_build("velocity", "x", &builds, Some("abc")).is_err());
    assert!(jar::fill_build("velocity", "x", &json!([]), None).is_err());
}

#[test]
fn purpur_versions_and_builds_are_chosen() {
    let versions = json!({ "project": "purpur", "metadata": { "current": "26.3" }, "versions": ["1.21.3", "1.21.4", "26.3"] });
    assert_eq!(jar::purpur_version(&versions, None).unwrap(), "26.3");
    assert_eq!(
        jar::purpur_version(&versions, Some("1.21.4")).unwrap(),
        "1.21.4"
    );
    assert!(jar::purpur_version(&versions, Some("1.21.2"))
        .unwrap_err()
        .contains("26.3"));
    let builds = json!({ "project": "purpur", "version": "1.21.4", "builds": { "latest": "2416", "all": ["2415", "2416"] } });
    let pick = jar::purpur_build("1.21.4", &builds, None).unwrap();
    assert_eq!(pick.file, "purpur-1.21.4-2416.jar");
    assert_eq!(
        pick.url,
        "https://api.purpurmc.org/v2/purpur/1.21.4/2416/download"
    );
    assert!(pick.sha256.is_none() && pick.notes[0].contains("md5"));
    assert_eq!(
        jar::purpur_build("1.21.4", &builds, Some("2415"))
            .unwrap()
            .build,
        "2415"
    );
    assert!(jar::purpur_build("1.21.4", &builds, Some("1")).is_err());
}

#[test]
fn downloaded_jars_are_checked_before_upload() {
    let body = b"PK\x03\x04 contenu du jar".to_vec();
    let hex: String = sha2::Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut pick = velocity_pick("3.4.0", "1", "velocity.jar", 21);
    pick.size = Some(body.len() as u64);
    pick.sha256 = Some(hex.to_ascii_uppercase());
    assert_eq!(jar::verify(&body, &pick).unwrap(), hex);
    let mut wrong = pick.clone();
    wrong.sha256 = Some("00".repeat(32));
    assert!(jar::verify(&body, &wrong).unwrap_err().contains("sha256"));
    let mut short = pick.clone();
    short.size = Some(3);
    assert!(jar::verify(&body, &short).unwrap_err().contains("octets"));
    assert!(jar::verify(b"<html>erreur</html>", &pick)
        .unwrap_err()
        .contains("JAR"));
    // Noms acceptés à la racine.
    assert!(jar::valid_file_name("velocity-3.4.0-SNAPSHOT-563.jar"));
    assert!(jar::valid_file_name("purpur-1.21.4-2416.jar"));
    for bad in [
        "",
        "server",
        "a/b.jar",
        "..jar",
        ".hidden.jar",
        "x y.jar",
        "x.jar;rm",
    ] {
        assert!(!jar::valid_file_name(bad), "{bad}");
    }
}

#[test]
fn manifest_requests_take_only_the_selected_manual_steps() {
    let spec = ServerSpec {
        game_version: Some("1.21.4".into()),
        loader: Some("Purpur".into()),
        java_version: Some(21),
        ..Default::default()
    };
    let r = Request::from_manifest(
        9,
        Some(&spec),
        Some("/purpur.jar"),
        &["server.loader", "server.java", "startup.jar_file"],
    )
    .unwrap();
    assert_eq!(
        (r.platform.as_deref(), r.version.as_deref(), r.java),
        (Some("purpur"), Some("1.21.4"), Some(21))
    );
    assert_eq!(r.jar_file.as_deref(), Some("purpur.jar"));
    assert!(!r.dry_run && !r.snapshot && r.drop_jar_with_catalog && !r.reinstall);
    assert!(
        r.confirm_name.is_none(),
        "jamais devinée : vient de l'utilisateur"
    );
    let r = Request::from_manifest(9, Some(&spec), None, &["server.java"]).unwrap();
    assert_eq!((r.platform, r.version, r.java), (None, None, Some(21)));
    assert!(Request::from_manifest(9, Some(&spec), None, &[]).is_none());
    assert!(Request::from_manifest(9, None, None, &["server.loader"]).is_none());
}

#[test]
fn op_results_map_to_manifest_steps() {
    let (velocity, _) = parse_catalog(CATALOG_VELOCITY);
    let mut p = prepared(
        Route::Catalog {
            entry: velocity[0].clone(),
        },
        "velocity",
        None,
    );
    p.java = Some(JavaChange {
        major: 25,
        option: JavaOption {
            id: 61,
            name: "Java 25 - Performance".into(),
            major: Some(25),
        },
        reason: "x".into(),
    });
    p.warnings.push(
        "`jar_file` « a.jar » ignoré : change_version installe et choisit lui-même le fichier"
            .into(),
    );
    let res = vec![
        OpResult {
            tool: "manage_java",
            summary: "Java 25".into(),
            outcome: "réussie",
            detail: None,
        },
        OpResult {
            tool: "change_version",
            summary: "Velocity".into(),
            outcome: "échouée",
            detail: Some("refusé".into()),
        },
    ];
    let out = outcomes_for_steps(
        &p,
        &["server.loader", "server.java", "startup.jar_file"],
        &res,
    );
    assert_eq!(
        out[0],
        (
            "server.loader".into(),
            "échouée",
            Some("change_version : refusé".into())
        )
    );
    assert_eq!((out[1].0.as_str(), out[1].1), ("server.java", "réussie"));
    assert_eq!(
        (out[2].0.as_str(), out[2].1),
        ("startup.jar_file", "ignorée")
    );
    assert!(out[2].2.as_deref().unwrap().contains("ignoré"));
    // Voie JAR : envoi puis fichier de démarrage valent pour la plateforme et le JAR.
    let pick = velocity_pick("3.4.0", "1", "velocity.jar", 21);
    let mut p = prepared(
        Route::Jar {
            pick,
            remote: "velocity.jar".into(),
        },
        "velocity",
        Some("3.4.0"),
    );
    p.startup_file = Some("velocity.jar".into());
    let res = vec![
        OpResult {
            tool: "upload_file",
            summary: "envoi".into(),
            outcome: "réussie",
            detail: Some("10 octets".into()),
        },
        OpResult {
            tool: "set_startup_config",
            summary: "fichier".into(),
            outcome: "non exécutée",
            detail: Some("annulé".into()),
        },
    ];
    let out = outcomes_for_steps(&p, &["server.loader", "server.java"], &res);
    assert_eq!(out[0].1, "non exécutée");
    assert_eq!(out[1].1, "conforme", "Java déjà adapté : rien à faire");
    assert!(changes_platform(&p));
}

#[tokio::test]
async fn change_version_needs_the_exact_name_from_the_user() {
    let (velocity, _) = parse_catalog(CATALOG_VELOCITY);
    let p = prepared(
        Route::Catalog {
            entry: velocity[0].clone(),
        },
        "velocity",
        None,
    );
    // Hors MCP (pas d'élicitation) : `confirm_name` exigé, nom attendu jamais révélé.
    let e = confirm(&p, None).await.unwrap_err();
    assert!(
        e.contains("Confirmation requise") && !e.contains("Survie"),
        "{e}"
    );
    let e = confirm(&p, Some("survie".into())).await.unwrap_err();
    assert!(
        e.contains("ne correspond pas") && !e.contains("Survie"),
        "{e}"
    );
    assert!(confirm(&p, Some(" Survie ".into())).await.is_ok());
    assert!(confirmation_matches("Survie", "Survie") && !confirmation_matches("", ""));
}

/// Chaîne d'un outil MCP : classement, groupe, annotations, garde, schéma (ici plutôt que dans
/// `mcp.rs`, partagé par plusieurs lots).
#[test]
fn the_new_tools_are_classified_everywhere() {
    use crate::mcp::{
        copilot_forbidden, is_write_tool, tool_for_game, tool_list, INFRA_TOOLS, LOCAL_DISK_TOOLS,
        LOCAL_KEEP_TOOLS, MINECRAFT_ONLY_TOOLS, WRITE_TOOLS,
    };
    use crate::mcp_catalog::{annotations, group_of, HUMAN_CONFIRMED_TOOLS, OPEN_WORLD_TOOLS};
    for t in ["set_server_platform", "delete_database"] {
        assert!(
            is_write_tool(t) && WRITE_TOOLS.contains(&t),
            "{t} : modifiant"
        );
        assert!(
            INFRA_TOOLS.contains(&t) && copilot_forbidden(t),
            "{t} : jamais au Copilote"
        );
        assert!(
            !LOCAL_DISK_TOOLS.contains(&t) && !LOCAL_KEEP_TOOLS.contains(&t),
            "{t}"
        );
        assert!(
            HUMAN_CONFIRMED_TOOLS.contains(&t),
            "{t} : confirmation par l'humain"
        );
        assert_eq!(
            crate::agent_guard::plan_kind(t),
            "prévu",
            "{t} : règle d'annulation"
        );
        assert_eq!(annotations(t).unwrap()["destructiveHint"], true, "{t}");
    }
    assert_eq!(group_of("set_server_platform"), Some("build"));
    assert_eq!(group_of("delete_database"), Some("mybox"));
    assert!(MINECRAFT_ONLY_TOOLS.contains(&"set_server_platform"));
    assert!(
        tool_for_game("delete_database", "satisfactory"),
        "une base sert à tout jeu"
    );
    assert!(
        OPEN_WORLD_TOOLS.contains(&"set_server_platform")
            && !OPEN_WORLD_TOOLS.contains(&"delete_database")
    );
    assert_eq!(
        annotations("set_server_platform").unwrap()["idempotentHint"],
        true
    );
    assert_eq!(
        annotations("delete_database").unwrap()["idempotentHint"],
        false
    );
    // Les outils officiels appelés en interne ont une raison explicite (étapes non annulables).
    for t in ["change_version", "manage_java", "set_startup_config"] {
        assert_eq!(crate::agent_guard::plan_kind(t), "prévu", "{t}");
    }
    let tools = tool_list().as_array().unwrap();
    let schema = |n: &str| {
        tools
            .iter()
            .find(|t| t["name"] == n)
            .unwrap_or_else(|| panic!("{n} absent"))["inputSchema"]
            .clone()
    };
    let s = schema("set_server_platform");
    assert_eq!(s["required"], json!(["server_id"]));
    assert_eq!(
        s["properties"]["method"]["enum"],
        json!(["auto", "catalog", "jar"])
    );
    assert_eq!(s["properties"]["dry_run"]["type"], "boolean");
    assert_eq!(s["properties"]["java"]["type"], "integer");
    let d = schema("delete_database");
    assert_eq!(
        d["required"],
        json!(["mybox_id", "database_id", "confirm_name"])
    );
    assert!(d["properties"].get("password").is_none());
    let a = schema("apply_manifest");
    assert_eq!(a["properties"]["apply_official"]["type"], "boolean");
    assert_eq!(a["properties"]["confirm_name"]["type"], "string");
    assert!(!a["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "confirm_name"));
}

/// L'hébergeur nomme un Velocity installé par change_version `proxyvelocity` (relevé réel du
/// 2026-11-20) : sans cette normalisation, l'attente de fin d'installation d'apply_network ne
/// concluait jamais et set_server_platform proposait de réinstaller un proxy déjà conforme.
#[test]
fn host_proxy_names_are_normalized() {
    assert_eq!(normalize_platform("proxyvelocity"), "velocity");
    assert_eq!(normalize_platform("ProxyBungeeCord"), "bungeecord");
    assert_eq!(normalize_platform("proxy_waterfall"), "waterfall");
    assert_eq!(normalize_platform("proxy"), "proxy");
    assert_eq!(normalize_platform("paper"), "paper");
}
