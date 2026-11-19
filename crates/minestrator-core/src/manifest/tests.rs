use super::clone::{choose_resources, decide, parse_request, CopyDecision};
use super::*;
use serde_json::json;

fn facts() -> Facts {
    Facts {
        name: "Survie".into(),
        family: "minecraft".into(),
        egg_id: Some(1),
        egg_name: "Minecraft".into(),
        game_version: Some("1.21.4".into()),
        loader: Some("paper".into()),
        startup: Some(Startup {
            command: "java -Xms128M -Xmx4G -jar {{SERVER_JARFILE}}".into(),
            jar_file: "server.jar".into(),
            java_memory: 4,
            image: "ghcr.io/pterodactyl/yolks:java_21".into(),
            java_version: 3,
        }),
    }
}

fn statuses(steps: &[Step]) -> Vec<(String, Status)> {
    steps.iter().map(|s| (s.id.clone(), s.status)).collect()
}

// --- Format ----------------------------------------------------------------------------------

#[test]
fn a_full_manifest_parses_and_round_trips() {
    let v = json!({
        "version": 1,
        "server": { "name": "Survie", "game": "Minecraft", "game_version": "1.21.4", "loader": "paper", "java_version": 21 },
        "properties": { "max-players": 50, "pvp": false, "motd": "Salut" },
        "startup": { "parameters": "java -Xmx4G -jar {{SERVER_JARFILE}}" },
        "content": [{ "kind": "plugin", "source": "modrinth", "project": "luckperms", "version": "v5.4.145-bukkit" }],
        "configs": [{ "path": "/plugins/LuckPerms/config.yml", "set": { "storage-method": "mysql" } }],
        "files": [{ "path": "/plugins/Essentials/motd.txt", "content": "Bienvenue !" }],
        "players": { "whitelist": ["Alice"], "ops": ["Bob"] },
        "schedules": { "items": [{ "name": "Redémarrage", "cron": "0 4 * * *", "tasks": [{ "action": "power", "payload": "restart" }] }], "remove_unlisted": true }
    });
    let m = Manifest::parse(&v).unwrap();
    assert_eq!(m.content_items().unwrap().0.len(), 1);
    let (sched, remove) = m.schedule_items().unwrap();
    assert!(remove && sched[0].is_active && !sched[0].only_when_online);
    // Texte JSON accepté (clients qui sérialisent l'argument).
    let again = Manifest::parse(&Value::String(serde_json::to_string(&m).unwrap())).unwrap();
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&m).unwrap()
    );
}

#[test]
fn bad_manifests_are_refused_with_a_reason() {
    let e = |v: Value| Manifest::parse(&v).unwrap_err();
    assert!(e(json!({ "version": 2 })).contains("version 2"));
    assert!(e(json!({ "version": 1, "propertie": {} })).contains("propertie"));
    assert!(
        e(json!({ "version": 1, "content": [{ "kind": "shader", "project": "x" }] }))
            .contains("shader")
    );
    assert!(
        e(json!({ "version": 1, "content": { "items": [], "remove": true } })).contains("remove")
    );
    assert!(
        e(json!({ "version": 1, "configs": [{ "path": "/eula.txt", "set": {} }] }))
            .contains("eula.txt")
    );
    assert!(e(json!([1, 2])).contains("objet"));
    assert!(e(json!({ "version": 1, "files": [{ "path": "/a", "content": "x".repeat(MAX_FILES_TOTAL + 1) }] })).contains("Kio"));
}

// --- Serveur, démarrage, réglages -------------------------------------------------------------------

#[test]
fn server_differences_are_manual_except_the_name() {
    let spec = ServerSpec {
        name: Some("Test".into()),
        game: Some("minecraft".into()),
        egg_id: None,
        game_version: Some("1.20.4".into()),
        loader: Some("Paper".into()),
        java_version: Some(17),
    };
    let steps = plan_server(&spec, &facts());
    assert_eq!(
        statuses(&steps),
        vec![
            ("server.name".into(), Status::Change),
            ("server.game".into(), Status::Ok),
            ("server.game_version".into(), Status::Manual),
            ("server.loader".into(), Status::Ok),
            ("server.java".into(), Status::Manual),
        ]
    );
    assert!(matches!(steps[0].action, Some(Action::Rename(ref n)) if n == "Test"));
    assert!(steps[2].summary.contains("1.21.4") && steps[2].summary.contains("1.20.4"));
    // Autre jeu : manuel (recréer le serveur).
    let other = plan_server(
        &ServerSpec {
            egg_id: Some(12),
            ..Default::default()
        },
        &facts(),
    );
    assert_eq!(other[0].status, Status::Manual);
}

#[test]
fn java_major_reads_common_images() {
    assert_eq!(java_major("ghcr.io/pterodactyl/yolks:java_21"), Some(21));
    assert_eq!(java_major("ghcr.io/minestrator/java:17"), Some(17));
    assert_eq!(java_major("eclipse-temurin:8-jre"), Some(8));
    assert_eq!(java_major("openjdk-11"), Some(11));
    assert_eq!(java_major("ghcr.io/parkervcp/yolks:debian"), None);
    // Image réelle de MineStrator (relevé du 2026-11-19, « Java 25 - Performance »).
    assert_eq!(java_major("reg.sttr.io/java/25_perf:latest"), Some(25));
}

#[test]
fn startup_changes_and_masked_commands() {
    let f = facts();
    let cur = f.startup.as_ref();
    let same = plan_startup(
        &StartupSpec {
            parameters: Some(" java -Xms128M -Xmx4G -jar {{SERVER_JARFILE}} ".into()),
            jar_file: Some("server.jar".into()),
        },
        cur,
    );
    assert!(same.iter().all(|s| s.status == Status::Ok));
    let change = plan_startup(
        &StartupSpec {
            parameters: Some("java -Xmx6G -jar {{SERVER_JARFILE}}".into()),
            jar_file: Some("paper.jar".into()),
        },
        cur,
    );
    assert_eq!(
        statuses(&change),
        vec![
            ("startup.parameters".into(), Status::Change),
            ("startup.jar_file".into(), Status::Manual)
        ]
    );
    let masked = plan_startup(
        &StartupSpec {
            parameters: Some("java -Dpassword=[SECRET] -jar x".into()),
            jar_file: None,
        },
        cur,
    );
    assert_eq!(masked[0].status, Status::Ok);
    assert!(masked[0].action.is_none());
}

#[test]
fn properties_plan_masks_secrets_and_skips_masked_values() {
    let cur: BTreeMap<String, String> = [
        ("max-players", "20"),
        ("pvp", "true"),
        ("rcon.password", "hunter2"),
        ("server-ip", "1.2.3.4"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let spec: Map<String, Value> = serde_json::from_value(json!({
        "max-players": 50, "pvp": true, "rcon.password": "n3w", "server-ip": "[IP]", "hide-online-players": true, "bad": [1]
    }))
    .unwrap();
    let steps = plan_properties(&spec, Some(&cur));
    let by = |id: &str| {
        steps
            .iter()
            .find(|s| s.id == format!("properties.{id}"))
            .unwrap()
    };
    assert_eq!(by("max-players").status, Status::Change);
    assert!(
        matches!(&by("max-players").action, Some(Action::Property { value, .. }) if value == "50")
    );
    assert_eq!(by("pvp").status, Status::Ok);
    assert_eq!(by("rcon.password").status, Status::Change);
    assert!(!serde_json::to_string(by("rcon.password"))
        .unwrap()
        .contains("hunter2"));
    assert!(!serde_json::to_string(by("rcon.password"))
        .unwrap()
        .contains("n3w"));
    assert_eq!(by("server-ip").status, Status::Ok);
    assert_eq!(by("hide-online-players").status, Status::Add);
    assert!(matches!(
        &by("hide-online-players").action,
        Some(Action::PropertyFile { .. })
    ));
    assert_eq!(by("bad").status, Status::Unsupported);
    assert_eq!(plan_properties(&spec, None)[0].status, Status::Unsupported);
}

// --- Contenu --------------------------------------------------------------------------------------

fn installed() -> Vec<Installed> {
    let (mut p, _) = parse_installed(
        ContentKind::Plugins,
        &json!({ "plugins": [
            { "name": "LuckPerms", "filename": "LuckPerms-Bukkit-5.4.140.jar", "version": "5.4.140", "enabled": true, "sha1": "a", "source": "modrinth", "project_id": "Vebnzrzj" },
            { "name": "EssentialsX", "filename": "EssentialsX-2.21.0.jar", "version": "2.21.0", "enabled": true, "source": null, "project_id": null },
            { "name": "Vault", "filename": "Vault.jar", "version": "1.7.3", "enabled": false, "source": "spigot", "project_id": "34315" }
        ]}),
    );
    let (d, level) = parse_installed(
        ContentKind::Datapacks,
        &json!({ "level_name": "monde", "datapacks": [{ "name": "terralith.zip" }] }),
    );
    assert_eq!(level, "monde");
    p.extend(d);
    p
}

#[test]
fn installed_lists_are_read_with_their_source() {
    let i = installed();
    assert_eq!(i.len(), 4);
    assert_eq!(
        (i[0].source.as_deref(), i[0].project.as_deref()),
        (Some("modrinth"), Some("Vebnzrzj"))
    );
    assert_eq!(
        (i[1].source.as_deref(), i[1].project.as_deref()),
        (None, None)
    );
    assert!(!i[2].enabled);
    assert_eq!(
        (i[3].file.as_str(), i[3].kind),
        ("terralith.zip", Some(ContentKind::Datapacks))
    );
}

fn item(v: Value) -> ContentItem {
    serde_json::from_value(v).unwrap()
}

#[test]
fn content_plan_matches_by_source_file_or_name() {
    let items = vec![
        // Même projet, même fichier : conforme.
        item(
            json!({ "kind": "plugin", "source": "modrinth", "project": "Vebnzrzj", "file": "LuckPerms-Bukkit-5.4.140.jar" }),
        ),
        // Non identifié par l'hôte, rapproché par le nom ; autre fichier voulu → changement.
        item(
            json!({ "kind": "plugin", "source": "modrinth", "project": "essentialsx", "name": "EssentialsX", "file": "EssentialsX-2.21.1.jar" }),
        ),
        // Absent → ajout.
        item(json!({ "kind": "plugin", "source": "modrinth", "project": "chunky" })),
        // Absent et non identifié → manuel.
        item(json!({ "kind": "plugin", "name": "PluginMaison", "file": "PluginMaison.jar" })),
        // Plugin CurseForge : non pris en charge par l'hébergeur.
        item(json!({ "kind": "plugin", "source": "curseforge", "project": "123" })),
        // Datapack présent, sans version : conforme.
        item(
            json!({ "kind": "datapack", "source": "modrinth", "project": "terralith", "file": "terralith.zip" }),
        ),
    ];
    let (steps, unlisted) = plan_content(&items, false, &installed(), &[], "monde", "paper");
    assert_eq!(
        statuses(&steps),
        vec![
            ("content.plugin.Vebnzrzj".into(), Status::Ok),
            ("content.plugin.essentialsx".into(), Status::Change),
            ("content.plugin.chunky".into(), Status::Add),
            ("content.plugin.PluginMaison.jar".into(), Status::Manual),
            ("content.plugin.123".into(), Status::Unsupported),
            ("content.datapack.terralith".into(), Status::Ok),
        ]
    );
    let Some(Action::Install(i)) = &steps[1].action else {
        panic!("installation attendue")
    };
    assert_eq!(
        (i.replaces.as_deref(), i.file.as_deref()),
        (
            Some("EssentialsX-2.21.0.jar"),
            Some("EssentialsX-2.21.1.jar")
        )
    );
    assert_eq!(unlisted.len(), 1, "{unlisted:?}");
    assert!(unlisted[0].contains("Vault"));
}

#[test]
fn content_remove_unlisted_and_disabled_items() {
    let items = vec![item(
        json!({ "kind": "plugin", "source": "spigot", "project": "34315", "enabled": true }),
    )];
    let (steps, unlisted) = plan_content(&items, true, &installed(), &[], "monde", "paper");
    assert!(unlisted.is_empty());
    let by = |id: &str| {
        steps
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id} absent : {:?}", statuses(&steps)))
    };
    // Vault installé mais désactivé : (dés)activation manuelle.
    assert_eq!(by("content.plugin.34315").status, Status::Manual);
    assert_eq!(
        by("content.plugin.LuckPerms-Bukkit-5.4.140.jar").status,
        Status::Remove
    );
    assert_eq!(by("content.datapack.terralith.zip").status, Status::Remove);
    assert!(
        matches!(&by("content.datapack.terralith.zip").action, Some(Action::RemoveContent { level_name, .. }) if level_name == "monde")
    );
}

#[test]
fn unreadable_content_lists_make_their_steps_unsupported() {
    let items = vec![item(
        json!({ "kind": "mod", "source": "modrinth", "project": "sodium" }),
    )];
    let (steps, _) = plan_content(
        &items,
        false,
        &[],
        &[(ContentKind::Mods, "404".into())],
        "",
        "fabric",
    );
    assert_eq!(steps[0].status, Status::Unsupported);
    let (steps, _) = plan_content(&items, false, &[], &[], "", "fabric");
    let Some(Action::Install(i)) = &steps[0].action else {
        panic!()
    };
    assert_eq!(
        i.loader, "fabric",
        "loader du serveur par défaut pour un mod"
    );
}

// --- Tâches planifiées, joueurs ------------------------------------------------------------------------

fn schedule(id: i64, name: &str, cron: &str, tasks: &[(&str, &str, i64)]) -> PanelSchedule {
    serde_json::from_value(json!({
        "id": id, "name": name, "is_active": true, "only_when_online": false,
        "cron": { "minute": cron.split(' ').next(), "hour": cron.split(' ').nth(1), "day_of_month": "*", "month": "*", "day_of_week": "*" },
        "tasks": tasks.iter().enumerate().map(|(n, (a, p, o))| json!({ "id": 100 + n as i64, "sequence_id": n + 1, "action": a, "payload": p, "time_offset": o })).collect::<Vec<_>>()
    }))
    .unwrap()
}

#[test]
fn schedules_plan_add_change_remove_and_validation() {
    let current = vec![
        schedule(
            1,
            "Redémarrage",
            "0 4 * * *",
            &[("command", "say Redémarrage", 0), ("power", "restart", 60)],
        ),
        schedule(2, "Annonce", "*/30 * * * *", &[("command", "say Salut", 0)]),
        schedule(3, "Vieille tâche", "0 0 * * *", &[]),
    ];
    let wanted: Vec<ScheduleSpec> = serde_json::from_value(json!([
        { "name": "Redémarrage", "cron": "0  4 * * *", "tasks": [{ "action": "command", "payload": "say Redémarrage" }, { "action": "power", "payload": "restart", "time_offset": 60 }] },
        { "name": "Annonce", "cron": "*/15 * * * *", "tasks": [{ "action": "command", "payload": "say Salut" }] },
        { "name": "Sauvegarde", "cron": "0 3 * * *", "tasks": [{ "action": "command", "payload": "save-all" }] },
        { "name": "Cassée", "cron": "99 * * * *" },
        { "name": "Masquée", "cron": "0 1 * * *", "tasks": [{ "action": "command", "payload": "say [IP]" }] }
    ]))
    .unwrap();
    let (steps, unlisted) = plan_schedules(&wanted, false, &current);
    assert_eq!(
        statuses(&steps),
        vec![
            ("schedules.Redémarrage".into(), Status::Ok),
            ("schedules.Annonce".into(), Status::Change),
            ("schedules.Sauvegarde".into(), Status::Add),
            ("schedules.Cassée".into(), Status::Unsupported),
            ("schedules.Masquée".into(), Status::Unsupported),
        ]
    );
    assert_eq!(unlisted.len(), 1);
    let (steps, _) = plan_schedules(&wanted[..1], true, &current);
    assert_eq!(
        steps.iter().filter(|s| s.status == Status::Remove).count(),
        2
    );
}

#[test]
fn players_plan_handles_aliases_and_removals() {
    let wl = vec![
        ListedPlayer {
            name: "Alice".into(),
            uuid: "11111111-2222-3333-4444-555555555555".into(),
        },
        ListedPlayer {
            name: "Eve".into(),
            uuid: "99999999-2222-3333-4444-555555555555".into(),
        },
    ];
    let alias = crate::redact::pseudonym("11111111-2222-3333-4444-555555555555");
    let spec = PlayersSpec {
        whitelist: Some(vec![alias.clone(), "Bob".into(), "joueur-abcdef".into()]),
        ops: Some(vec!["alice".into()]),
        remove_unlisted: false,
    };
    let (steps, unlisted) = plan_players(&spec, &wl, &[], true, Some(false));
    assert_eq!(
        statuses(&steps),
        vec![
            (format!("players.whitelist.{alias}"), Status::Ok),
            ("players.whitelist.Bob".into(), Status::Add),
            ("players.whitelist.joueur-abcdef".into(), Status::Manual),
            ("players.ops.alice".into(), Status::Add),
        ]
    );
    assert!(steps[1].summary.contains("serveur arrêté"));
    // Eve, absente du manifeste : gardée, et montrée par son alias (anonymisation).
    assert_eq!(unlisted.len(), 1);
    assert!(!unlisted[0].contains("Eve") && unlisted[0].contains("joueur-"));
    let (steps, _) = plan_players(
        &PlayersSpec {
            whitelist: Some(vec!["Alice".into()]),
            ops: None,
            remove_unlisted: true,
        },
        &wl,
        &[],
        false,
        None,
    );
    assert!(
        matches!(&steps[1].action, Some(Action::Player { action: "whitelist_remove", name }) if name == "Eve")
    );
    let parsed =
        parse_player_list("[{\"uuid\":\"u1\",\"name\":\"Zed\"},{\"uuid\":\"u2\"}]").unwrap();
    assert_eq!(
        parsed,
        vec![ListedPlayer {
            name: "Zed".into(),
            uuid: "u1".into()
        }]
    );
}

// --- Dépendances, sélection --------------------------------------------------------------------------------

#[test]
fn configs_depend_on_the_plugin_they_belong_to() {
    let items = vec![
        item(
            json!({ "kind": "plugin", "source": "modrinth", "project": "luckperms", "name": "LuckPerms" }),
        ),
        item(json!({ "kind": "plugin", "source": "modrinth", "project": "chunky" })),
    ];
    let (steps, _) = plan_content(&items, false, &[], &[], "", "paper");
    assert_eq!(
        content_dependency("/plugins/LuckPerms/config.yml", &steps),
        vec!["content.plugin.luckperms".to_string()]
    );
    assert!(content_dependency("/plugins/Essentials/config.yml", &steps).is_empty());
    assert!(content_dependency("/server.properties", &steps).is_empty());
}

#[test]
fn step_selection_by_id_or_prefix() {
    assert!(selected("properties.max-players", &[]));
    assert!(selected("properties.max-players", &["properties".into()]));
    assert!(selected(
        "content.plugin.luckperms",
        &["content.plugin".into()]
    ));
    assert!(selected(
        "configs./plugins/LuckPerms/config.yml",
        &["configs./plugins/LuckPerms/config.yml".into()]
    ));
    assert!(!selected("properties.max-players", &["prop".into()]));
    assert!(!selected("schedules.A", &["content".into()]));
}

#[test]
fn leaves_are_flattened_for_config_export() {
    let tree = json!({ "a": { "b": 1, "c": [1, 2] }, "list": [{ "x": true }], "empty": {} });
    let (leaves, cut) = flatten_leaves(&tree, 10);
    assert!(!cut);
    assert_eq!(leaves.get("a.b"), Some(&json!(1)));
    assert_eq!(leaves.get("a.c"), Some(&json!([1, 2])));
    assert_eq!(leaves.get("list[0].x"), Some(&json!(true)));
    assert_eq!(leaves.get("empty"), Some(&json!({})));
    assert!(flatten_leaves(&tree, 2).1);
}

#[test]
fn config_selection_accepts_paths_or_objects() {
    let s = parse_config_selection(Some(
        &json!(["/a.yml", { "path": "/b.toml", "keys": ["x.y"] }]),
    ))
    .unwrap();
    assert_eq!((s[0].path.as_str(), s[0].keys.len()), ("/a.yml", 0));
    assert_eq!(s[1].keys, vec!["x.y".to_string()]);
    assert!(parse_config_selection(Some(&json!("x"))).is_err());
    assert!(parse_config_selection(None).unwrap().is_empty());
}

// --- clone_server -------------------------------------------------------------------------------------------

#[test]
fn clone_request_is_validated_before_anything() {
    let r = parse_request(&json!({ "source_server_id": 5, "mybox_id": 9, "name": " Copie ", "exclude": ["/plugins/dynmap/web", "logs/"] })).unwrap();
    assert_eq!(
        (r.source, r.mybox, r.name.as_str(), r.include_world),
        (5, 9, "Copie", false)
    );
    assert_eq!(
        r.exclude,
        vec!["/plugins/dynmap/web".to_string(), "/logs".to_string()]
    );
    assert!(
        parse_request(&json!({ "source_server_id": 5, "mybox_id": 9, "name": "x", "cpu": 0 }))
            .is_err()
    );
    assert!(parse_request(
        &json!({ "source_server_id": 5, "mybox_id": 9, "name": "x", "exclude": ["/"] })
    )
    .is_err());
    assert!(parse_request(
        &json!({ "source_server_id": 5, "mybox_id": 9, "name": "x", "exclude": ["/../etc"] })
    )
    .is_err());
    assert!(parse_request(&json!({ "mybox_id": 9, "name": "x" })).is_err());
}

#[test]
fn clone_resources_default_to_the_source_when_they_fit() {
    assert_eq!(
        choose_resources((2, 4), (6, 16), (None, None)).unwrap(),
        (2, 4, None)
    );
    let (cpu, ram, note) = choose_resources((4, 8), (2, 16), (None, None)).unwrap();
    assert_eq!((cpu, ram), (2, 8));
    assert!(note.is_some());
    assert_eq!(
        choose_resources((4, 8), (6, 16), (Some(1), Some(2))).unwrap(),
        (1, 2, None)
    );
    assert!(choose_resources((4, 8), (6, 16), (Some(8), None)).is_err());
    assert!(choose_resources((4, 8), (0, 16), (None, None)).is_err());
}

#[test]
fn clone_copy_skips_worlds_and_expands_partial_exclusions() {
    let excluded: HashSet<String> = ["/world", "/plugins/dynmap/web"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(decide("/world", true, &excluded), CopyDecision::Skip);
    assert_eq!(decide("/plugins", true, &excluded), CopyDecision::Expand);
    assert_eq!(
        decide("/plugins/dynmap", true, &excluded),
        CopyDecision::Expand
    );
    assert_eq!(
        decide("/plugins/dynmap/config.txt", false, &excluded),
        CopyDecision::Copy
    );
    assert_eq!(decide("/worldedit", true, &excluded), CopyDecision::Copy);
    assert_eq!(
        decide("/server.properties", false, &excluded),
        CopyDecision::Copy
    );
}

// --- apply_official (lot plateforme) ---------------------------------------------------------------

/// `plan_manifest` dit quelles étapes `manual` `apply_official: true` appliquerait : version,
/// plateforme, Java et JAR qui ont un outil officiel (Quilt n'en a pas).
#[test]
fn the_plan_lists_the_steps_apply_official_would_apply() {
    let f = facts();
    let spec = ServerSpec {
        game_version: Some("1.21.11".into()),
        loader: Some("purpur".into()),
        java_version: Some(25),
        ..Default::default()
    };
    let mut steps = plan_server(&spec, &f);
    let all: std::collections::HashMap<&'static str, Result<(), String>> =
        ["change_version", "manage_java", "set_startup_config"]
            .into_iter()
            .map(|t| (t, Err("passerelle désactivée".to_string())))
            .collect();
    official::annotate(&mut steps, 1, Some(&spec), None, &f, &all);
    let plan = Plan {
        steps,
        ..Default::default()
    };
    let out = render_plan(1, &plan);
    assert_eq!(
        out["apply_official"]["steps"],
        json!(["server.game_version", "server.loader", "server.java"])
    );
    assert!(out["apply_official"]["how"]
        .as_str()
        .unwrap()
        .contains("confirmation"));
    assert!(out["next"].as_str().unwrap().contains("apply_official=true"));
    assert!(out["steps"][0]["summary"]
        .as_str()
        .unwrap()
        .contains("apply_official=true"));
    // Quilt : aucune voie, rien d'applicable ; pas de clé.
    let quilt = ServerSpec {
        loader: Some("quilt".into()),
        ..Default::default()
    };
    let mut steps = plan_server(&quilt, &f);
    official::annotate(&mut steps, 1, Some(&quilt), None, &f, &all);
    assert!(official_applicable(&steps).is_empty());
    let out = render_plan(
        1,
        &Plan {
            steps,
            ..Default::default()
        },
    );
    assert!(out.get("apply_official").is_none());
}
