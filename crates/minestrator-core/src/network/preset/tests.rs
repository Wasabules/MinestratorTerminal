//! Tests sur fixtures (sans réseau) des préréglages `luckperms` et `geyser`. Les secrets
//! ci-dessous sont des valeurs de test.

use super::*;
use crate::db_connect::Engine;

const PW: &str = "MotDePasseDeTest-9f3a1c";

fn no_redact(s: &str) -> String {
    s.to_string()
}

fn node(
    id: i64,
    name: &str,
    ty: ServerType,
    proxy: bool,
    entry: Option<&str>,
    files: NodeFiles,
) -> Node {
    Node {
        server_id: id,
        name: name.into(),
        ty: Some(ty),
        version: Some("1.21.4".into()),
        proxy,
        entry: entry.map(str::to_string),
        files: Ok(files),
    }
}

fn creds(password: Option<&str>) -> Creds {
    Creds {
        id: 7,
        label: Some("réseau".into()),
        engine: Engine::MariaDb,
        host: "mariadb1.exemple.net".into(),
        port: 3306,
        port_assumed: false,
        database: "s1_luckperms".into(),
        username: "u1_lp".into(),
        password: password.map(str::to_string),
    }
}

/// Config de LuckPerms générée (extrait).
fn lp_config(storage: &str) -> String {
    format!(
        "# LuckPerms\nserver: global\nuse-server-uuid-cache: false\nstorage-method: {storage}\ndata:\n  address: localhost\n  database: minecraft\n  username: root\n  password: ''\n  pool-settings:\n    maximum-pool-size: 10\nmessaging-service: auto\nsync-minutes: -1\n"
    )
}

fn step<'a>(plan: &'a PresetPlan, id: &str) -> &'a Step {
    plan.steps
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("étape {id} absente : {:?}", plan.steps))
}

#[test]
fn arguments_are_checked() {
    let a = parse_args(
        &json!({ "preset": "LuckPerms", "proxy_server_id": 1, "backend_server_ids": [2, "3"] }),
    )
    .unwrap();
    assert_eq!(a.preset, Preset::LuckPerms);
    assert_eq!(a.backends, [2, 3]);
    assert!(a.dry_run && a.snapshot, "simulation et snapshot par défaut");
    let g = parse_args(&json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [2], "bedrock_port": 40002, "dry_run": false })).unwrap();
    assert_eq!(
        (g.preset, g.bedrock_port, g.dry_run),
        (Preset::Geyser, Some(40002), false)
    );
    for bad in [
        json!({ "proxy_server_id": 1, "backend_server_ids": [2] }),
        json!({ "preset": "viaversion", "proxy_server_id": 1, "backend_server_ids": [2] }),
        json!({ "preset": "geyser", "backend_server_ids": [2] }),
        json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [] }),
        json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [1] }),
        json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [2, 2] }),
        json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [2], "bedrock_port": 70000 }),
        json!({ "preset": "luckperms", "proxy_server_id": 1, "backend_server_ids": [2], "bedrock_port": 40000 }),
        json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [2], "database_id": 3 }),
    ] {
        assert!(parse_args(&bad).is_err(), "{bad} accepté");
    }
}

#[test]
fn plugins_and_files_are_recognised() {
    let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(has_plugin(
        &p(&["LuckPerms-Velocity-5.5.71.jar"]),
        "luckperms"
    ));
    assert!(
        !has_plugin(&p(&["LuckPerms-Bukkit-5.5.71.jar.disabled"]), "luckperms"),
        "désactivé"
    );
    assert!(has_plugin(
        &p(&["Geyser-Velocity.jar", "floodgate-velocity.jar"]),
        "geyser"
    ));
    assert!(has_plugin(&p(&["floodgate-spigot.jar"]), "floodgate"));
    assert!(!has_plugin(
        &p(&[
            "GeyserSkinManager.jar",
            "geyser-and-floodgate-auto-uploader.jar"
        ]),
        "geyser"
    ));
    assert_eq!(
        luckperms_paths(Some(ServerType::Velocity))[0],
        "/plugins/luckperms/config.yml"
    );
    assert_eq!(
        luckperms_paths(Some(ServerType::Paper))[0],
        "/plugins/LuckPerms/config.yml"
    );
    assert_eq!(
        geyser_paths(Some(ServerType::Bungee)),
        ["/plugins/Geyser-BungeeCord/config.yml"]
    );
    assert_eq!(key_file(None), "key.pem");
    assert_eq!(key_file(Some("key-file-name: autre.pem\n")), "autre.pem");
    assert_eq!(
        key_file(Some("key-file-name: ../../etc/x\n")),
        "key.pem",
        "chemin refusé"
    );
}

fn lp_network() -> Vec<Node> {
    let installed = |path: &str, text: String| NodeFiles {
        plugins: vec!["LuckPerms.jar".into()],
        luckperms: Some((path.into(), text)),
        ..Default::default()
    };
    // « Lobby » utilise déjà la base du réseau.
    let shared = lp_config("mariadb")
        .replace("address: localhost", "address: mariadb1.exemple.net:3306")
        .replace("database: minecraft", "database: s1_luckperms");
    vec![
        node(
            1,
            "Proxy",
            ServerType::Velocity,
            true,
            None,
            installed("/plugins/luckperms/config.yml", lp_config("h2")),
        ),
        node(
            2,
            "Lobby",
            ServerType::Paper,
            false,
            Some("lobby"),
            installed("/plugins/LuckPerms/config.yml", shared),
        ),
        node(
            3,
            "Survie",
            ServerType::Paper,
            false,
            Some("survie"),
            NodeFiles::default(),
        ),
        node(
            4,
            "Moddé",
            ServerType::Fabric,
            false,
            Some("modde"),
            NodeFiles::default(),
        ),
        node(
            5,
            "Jamais démarré",
            ServerType::Paper,
            false,
            Some("neuf"),
            NodeFiles {
                plugins: vec!["LuckPerms-Bukkit-5.5.71.jar".into()],
                ..Default::default()
            },
        ),
    ]
}

#[test]
fn luckperms_shares_one_database_without_ever_showing_the_password() {
    let nodes = lp_network();
    let versions = HashMap::from([(
        3,
        Ok(("b0mk8uS6".to_string(), "v5.5.71-bukkit".to_string())),
    )]);
    let plan = plan_luckperms(&nodes, &creds(Some(PW)), Some(PW), &versions, &no_redact).unwrap();

    // Proxy : même base, messagerie par la base, pas de nom de serveur.
    let p = step(&plan, "1.luckperms");
    assert_eq!(p.status, Status::Change);
    let Some(Action::Config { path, changes, .. }) = &p.action else {
        panic!("{p:?}")
    };
    assert_eq!(path, "/plugins/luckperms/config.yml");
    let get = |k: &str| changes.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
    assert_eq!(get("storage-method"), Some(json!("mariadb")));
    assert_eq!(
        get("data.address"),
        Some(json!("mariadb1.exemple.net:3306"))
    );
    assert_eq!(
        get("data.password"),
        Some(json!(PW)),
        "le mot de passe n'est que dans l'écriture interne"
    );
    assert_eq!(get("messaging-service"), Some(json!("sql")));
    assert_eq!(get("server"), None, "le proxy garde son nom");
    // Serveur de jeu : son nom d'entrée sur le proxy.
    let Some(Action::Config { changes, .. }) = &step(&plan, "2.luckperms").action else {
        panic!()
    };
    assert!(changes.contains(&("server".to_string(), json!("lobby"))));
    // Absent : installé (build Bukkit choisie), puis config au prochain passage.
    let inst = step(&plan, "3.install");
    assert_eq!(
        inst.action,
        Some(Action::Install {
            source: "modrinth".into(),
            slug: "luckperms".into(),
            version_id: "b0mk8uS6".into()
        })
    );
    assert_eq!(step(&plan, "3.config").status, Status::Manual);
    assert!(step(&plan, "3.config")
        .summary
        .contains("démarre le serveur une fois"));
    // Fabric : à la main ; installé mais jamais démarré : à la main.
    assert_eq!(step(&plan, "4.platform").status, Status::Manual);
    assert!(step(&plan, "5.config")
        .summary
        .contains("n'existe pas encore"));
    // Bascule depuis H2 : à confirmer.
    assert!(
        plan.destructive
            .iter()
            .any(|d| d.contains("« Proxy »") && d.contains("h2")),
        "{:?}",
        plan.destructive
    );
    assert!(
        !plan.destructive.iter().any(|d| d.contains("« Lobby »")),
        "déjà sur une base"
    );

    // Le mot de passe n'apparaît nulle part : diffs, rendu, débogage.
    let a = parse_args(
        &json!({ "preset": "luckperms", "proxy_server_id": 1, "backend_server_ids": [2, 3, 4, 5] }),
    )
    .unwrap();
    let rendered = render(&a, &nodes, &plan, &HashMap::new()).to_string();
    let debug = format!("{plan:?}");
    for out in [&rendered, &debug] {
        assert!(!out.contains(PW), "mot de passe affiché");
    }
    assert!(
        p.diff.as_deref().unwrap().contains("[SECRET]"),
        "{:?}",
        p.diff
    );
}

#[test]
fn an_unreadable_password_writes_nothing() {
    let nodes = lp_network();
    let plan = plan_luckperms(&nodes, &creds(None), None, &HashMap::new(), &no_redact).unwrap();
    assert!(plan
        .steps
        .iter()
        .all(|s| !matches!(s.action, Some(Action::Config { .. }))));
    assert_eq!(step(&plan, "1.password").status, Status::Manual);
    assert!(plan.warnings.iter().any(|w| w.contains("rotate_password")));
}

/// Config de Geyser (ancien format `remote`).
const GEYSER_OLD: &str = "bedrock:\n  address: 0.0.0.0\n  port: 19132\n  clone-remote-port: true\nremote:\n  address: auto\n  port: 25565\n  auth-type: online\nfloodgate-key-file: key.pem\n";
/// Config de Geyser (format récent `java`).
const GEYSER_NEW: &str = "bedrock:\n  port: 19132\njava:\n  address: auto\n  auth-type: online\n";
const FLOODGATE: &str =
    "key-file-name: key.pem\nsend-floodgate-data: false\nusername-prefix: '.'\n";

fn geyser_proxy(config: &str) -> Node {
    node(
        1,
        "Proxy",
        ServerType::Velocity,
        true,
        None,
        NodeFiles {
            plugins: vec![
                "Geyser-Velocity.jar".into(),
                "floodgate-velocity.jar".into(),
            ],
            geyser: Some(("/plugins/geyser/config.yml".into(), config.into())),
            floodgate: Some(FLOODGATE.into()),
            key: Some("empreinte-proxy".into()),
            ..Default::default()
        },
    )
}

fn paper_with(id: i64, name: &str, floodgate: bool, key: Option<&str>) -> Node {
    node(
        id,
        name,
        ServerType::Paper,
        false,
        Some(name),
        NodeFiles {
            plugins: if floodgate {
                vec!["floodgate-spigot.jar".into()]
            } else {
                Vec::new()
            },
            key: key.map(str::to_string),
            ..Default::default()
        },
    )
}

#[test]
fn geyser_spreads_floodgate_and_its_key_without_reading_it() {
    let backends = vec![
        paper_with(2, "lobby", true, Some("empreinte-proxy")),
        paper_with(3, "survie", false, None),
        paper_with(4, "ancien", true, Some("autre-empreinte")),
    ];
    let port = PortChoice::Use {
        port: 40002,
        why: "test".into(),
    };
    let builds = HashMap::from([
        ("geyser", "2.12.1 (build 1305)".to_string()),
        ("floodgate", "2.2.5 (build 141)".to_string()),
    ]);
    let plan = plan_geyser(
        &geyser_proxy(GEYSER_OLD),
        &Forwarding::Modern,
        &backends,
        &port,
        &builds,
        &no_redact,
    );

    let cfg = step(&plan, "1.geyser-config");
    let Some(Action::Config { changes, .. }) = &cfg.action else {
        panic!("{cfg:?}")
    };
    assert!(changes.contains(&("remote.auth-type".to_string(), json!("floodgate"))));
    assert!(changes.contains(&("bedrock.port".to_string(), json!(40002))));
    assert!(changes.contains(&("bedrock.clone-remote-port".to_string(), json!(false))));
    let Some(Action::Config { changes, .. }) = &step(&plan, "1.floodgate-config").action else {
        panic!()
    };
    assert_eq!(changes, &[("send-floodgate-data".to_string(), json!(true))]);
    // Clé : identique (rien), absente (copiée), différente (remplacée, à confirmer).
    assert_eq!(step(&plan, "2.key").status, Status::Ok);
    assert_eq!(
        step(&plan, "3.floodgate").action,
        Some(Action::Install {
            source: "geysermc".into(),
            slug: "floodgate".into(),
            version_id: "latest".into()
        })
    );
    assert_eq!(
        step(&plan, "3.key").action,
        Some(Action::CopyFile {
            from_server: 1,
            from_path: "/plugins/floodgate/key.pem".into(),
            to_dir: "/plugins/floodgate".into(),
            overwrite: false
        })
    );
    assert!(matches!(
        step(&plan, "4.key").action,
        Some(Action::CopyFile {
            overwrite: true,
            ..
        })
    ));
    assert!(plan.destructive.iter().any(|d| d.contains("« ancien »")));
    // Aucune empreinte ni contenu de clé dans le rendu.
    let a = parse_args(
        &json!({ "preset": "geyser", "proxy_server_id": 1, "backend_server_ids": [2, 3, 4] }),
    )
    .unwrap();
    let mut nodes = vec![geyser_proxy(GEYSER_OLD)];
    nodes.extend(backends);
    let rendered = render(&a, &nodes, &plan, &HashMap::new()).to_string();
    assert!(
        !rendered.contains("empreinte-proxy") && !rendered.contains("autre-empreinte"),
        "{rendered}"
    );

    // Format récent : `java.auth-type`.
    let plan = plan_geyser(
        &geyser_proxy(GEYSER_NEW),
        &Forwarding::Modern,
        &[],
        &port,
        &builds,
        &no_redact,
    );
    let Some(Action::Config { changes, .. }) = &step(&plan, "1.geyser-config").action else {
        panic!()
    };
    assert!(changes.contains(&("java.auth-type".to_string(), json!("floodgate"))));
    assert!(!changes
        .iter()
        .any(|(k, _)| k == "bedrock.clone-remote-port"));
}

#[test]
fn geyser_without_port_plugins_or_forwarding_explains_what_to_do() {
    let missing = PortChoice::Missing {
        call: json!({ "server_id": 1, "action": "add", "type": "public", "description": "Geyser (Bedrock)" }),
    };
    let bare = node(
        1,
        "Proxy",
        ServerType::Velocity,
        true,
        None,
        NodeFiles::default(),
    );
    let plan = plan_geyser(
        &bare,
        &Forwarding::None,
        &[paper_with(2, "lobby", false, None)],
        &missing,
        &HashMap::new(),
        &no_redact,
    );
    assert!(step(&plan, "1.forwarding").summary.contains("link_proxy"));
    assert_eq!(step(&plan, "1.geyser").status, Status::Change);
    assert_eq!(step(&plan, "1.floodgate").status, Status::Change);
    assert!(step(&plan, "1.start")
        .summary
        .contains("démarre le proxy une fois"));
    let port = step(&plan, "1.port");
    assert!(
        port.summary.contains("manage_ports")
            && port.summary.contains("\"action\":\"add\"")
            && port.summary.contains("bedrock_port"),
        "{}",
        port.summary
    );
    // Clé du proxy inconnue : aucune copie vers les serveurs de jeu.
    assert!(!plan
        .steps
        .iter()
        .any(|s| matches!(s.action, Some(Action::CopyFile { .. }))));
    assert_eq!(step(&plan, "2.floodgate").status, Status::Change);
}

#[test]
fn the_bedrock_port_is_one_of_the_proxys_own() {
    let ep = Endpoint {
        server_id: 1,
        ip: "203.0.113.5".into(),
        port: 25565,
        dns: None,
        secondary: vec![("203.0.113.5".into(), 40001), ("203.0.113.5".into(), 40002)],
    };
    let raw = json!({ "ports": [
        { "port": 40001, "description": "Votifier" },
        { "port": 40002, "description": "Bedrock" }
    ] });
    assert!(choose_port(1, &ep, &raw, Some(19132), None)
        .unwrap_err()
        .contains("pas un port du proxy"));
    assert!(matches!(
        choose_port(1, &ep, &raw, Some(40001), None).unwrap(),
        PortChoice::Use { port: 40001, .. }
    ));
    match choose_port(1, &ep, &raw, Some(25565), None).unwrap() {
        PortChoice::Use { port: 25565, why } => assert!(why.contains("ping_server")),
        other => panic!("{other:?}"),
    }
    // Port actuel de Geyser, s'il est au proxy ; sinon le port décrit « Bedrock ».
    assert!(matches!(
        choose_port(1, &ep, &raw, None, Some("bedrock:\n  port: 40001\n")).unwrap(),
        PortChoice::Use { port: 40001, .. }
    ));
    assert!(matches!(
        choose_port(1, &ep, &raw, None, Some(GEYSER_OLD)).unwrap(),
        PortChoice::Use { port: 40002, .. }
    ));
    let none = json!({ "ports": [{ "port": 40001, "description": "Votifier" }] });
    match choose_port(1, &ep, &none, None, None).unwrap() {
        PortChoice::Missing { call } => assert_eq!(call["action"], json!("add")),
        other => panic!("{other:?}"),
    }
}
