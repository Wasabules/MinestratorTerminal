//! Tests sur fixtures (sans réseau) : lecture des configs de proxy et de serveurs de jeu,
//! rapprochement des adresses, problèmes signalés, plan de `link_proxy`. Les secrets ci-dessous
//! sont des valeurs de test, jamais des vrais.

use super::link::*;
use super::*;
use serde_json::json;

/// `velocity.toml` par défaut de Velocity 3.x (commentaires abrégés).
const VELOCITY_DEFAULT: &str = r#"# Config version. Do not change this
config-version = "2.7"

# What port should the proxy be bound to? By default, we'll bind to all addresses on port 25565.
bind = "0.0.0.0:25565"
motd = "<#09add3>A Velocity Server"
show-max-players = 500

# Should we authenticate players with Mojang? By default, this is on.
online-mode = true
force-key-authentication = true
prevent-client-proxy-connections = false

# Should we forward IP addresses and other data to backend servers?
player-info-forwarding-mode = "NONE"

# If you are using modern or BungeeGuard IP forwarding, configure a file that contains a unique secret here.
forwarding-secret-file = "forwarding.secret"
announce-forge = false
kick-existing-players = false
ping-passthrough = "DISABLED"
enable-player-address-logging = true

[servers]
# Configure your servers here. Each key represents the server's name, and the value
# represents the IP address of the server to connect to.
lobby = "127.0.0.1:30066"
factions = "127.0.0.1:30067"
minigames = "127.0.0.1:30068"

# In what order we should try servers when a player logs in or is kicked from a server.
try = [
    "lobby"
]

[forced-hosts]
# Configure your forced hosts here.
"lobby.example.com" = [
    "lobby"
]
"factions.example.com" = [
    "factions"
]
"minigames.example.com" = [
    "minigames"
]

[advanced]
compression-threshold = 256
compression-level = -1
login-ratelimit = 3000
"#;

/// `config.yml` par défaut de BungeeCord.
const BUNGEE_DEFAULT: &str = "server_connect_timeout: 5000
remote_ping_cache: -1
forge_support: false
player_limit: -1
permissions:
  default:
  - bungeecord.command.server
  - bungeecord.command.list
  admin:
  - bungeecord.command.alert
  - bungeecord.command.end
timeout: 30000
log_commands: false
network_compression_threshold: 256
online_mode: true
disabled_commands:
- disabledcommandhere
servers:
  lobby:
    motd: '&1Just another BungeeCord - Forced Host'
    address: localhost:25565
    restricted: false
listeners:
- query_port: 25577
  motd: '&1Another Bungee server'
  tab_list: GLOBAL_PING
  query_enabled: false
  proxy_protocol: false
  forced_hosts:
    pvp.md-5.net: pvp
  ping_passthrough: false
  priorities:
  - lobby
  bind_local_address: true
  host: 0.0.0.0:25577
  max_players: 1
  tab_size: 60
  force_default_server: false
ip_forward: false
remote_ping_timeout: 5000
prevent_proxy_connections: false
groups:
  md_5:
  - admin
connection_throttle: 4000
connection_throttle_limit: 3
log_pings: true
";

/// `config/paper-global.yml` (extrait), transfert Velocity désactivé.
fn paper_global(enabled: bool, secret: &str) -> String {
    format!(
        "# This is the global configuration file for Paper.
_version: 31
chunk-loading-basic:
  player-max-chunk-load-rate: 100.0
proxies:
  bungee-cord:
    online-mode: true
  proxy-protocol: false
  velocity:
    enabled: {enabled}
    online-mode: true
    secret: '{secret}'
spam-limiter:
  incoming-packet-threshold: 300
"
    )
}

fn spigot(bungeecord: bool) -> String {
    format!(
        "config-version: 12\nsettings:\n  debug: false\n  bungeecord: {bungeecord}\n  restart-script: ./start.sh\n"
    )
}

const SECRET_PROXY: &str = "SECRET_DE_TEST_PROXY_0123456789";
const SECRET_OTHER: &str = "SECRET_DE_TEST_AUTRE_9876543210";
const SECRET_NEW: &str = "SECRET_DE_TEST_GENERE_ABCDEFGHIJ";

fn ep(id: i64, ip: &str, port: u16) -> Endpoint {
    Endpoint {
        server_id: id,
        ip: ip.into(),
        port,
        dns: None,
        secondary: Vec::new(),
    }
}

fn no_redact(s: &str) -> String {
    s.to_string()
}

fn proxy_state(text: &str, secret: Option<&str>) -> ProxyState {
    let conf = if text.contains("[servers]") {
        parse_velocity(text).unwrap()
    } else {
        parse_bungee(text).unwrap()
    };
    ProxyState {
        server_id: 100,
        name: "Proxy".into(),
        config_path: if conf.kind == ProxyKind::Velocity {
            VELOCITY_TOML
        } else {
            BUNGEE_CONFIG
        },
        conf,
        secret: secret.map(str::to_string),
        secret_note: "/forwarding.secret".into(),
        bungeeguard: false,
        bungeeguard_token: None,
    }
}

fn paper_backend(
    id: i64,
    name: &str,
    ip: &str,
    port: u16,
    files: BackendFiles,
    online: Option<bool>,
) -> BackendInput {
    let state = backend_state(
        Some(ServerType::Paper),
        Some("1.21.4".into()),
        online,
        &files,
    );
    BackendInput {
        server_id: id,
        server: name.into(),
        endpoint: ep(id, ip, port),
        state: Ok((state, files)),
    }
}

fn files(paper: Option<String>, spigot_yml: Option<String>) -> BackendFiles {
    BackendFiles {
        paper_global: paper,
        spigot: spigot_yml,
        ..Default::default()
    }
}

fn args(proxy: i64, backends: &[i64]) -> LinkArgs {
    parse_args(&json!({ "proxy_server_id": proxy, "backend_server_ids": backends })).unwrap()
}

fn step<'a>(plan: &'a Plan, id: &str) -> &'a Step {
    plan.steps
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("étape {id} absente : {:?}", plan.steps))
}

// --- Lecture des configurations ------------------------------------------------------------------

#[test]
fn velocity_config_is_read() {
    let c = parse_velocity(VELOCITY_DEFAULT).unwrap();
    assert_eq!(c.kind, ProxyKind::Velocity);
    let names: Vec<&str> = c.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"lobby")
            && names.contains(&"factions")
            && names.contains(&"minigames")
            && names.len() == 3
    );
    assert_eq!(c.entry("LOBBY").unwrap().address, "127.0.0.1:30066");
    assert_eq!(c.try_order, ["lobby"]);
    assert_eq!(c.forwarding, Forwarding::None);
    assert_eq!(c.forwarding_raw.as_deref(), Some("NONE"));
    assert_eq!(c.online_mode, Some(true));
    assert_eq!(c.forced_hosts.len(), 3);
    assert_eq!(c.secret_path().as_deref(), Some("/forwarding.secret"));

    // Ancien format : secret écrit dans velocity.toml, sans fichier.
    let old = "player-info-forwarding-mode = \"modern\"\nforwarding-secret = \"ABCDEFGH\"\n[servers]\nlobby = \"203.0.113.10:25565\"\ntry = [\"lobby\"]\n";
    let c = parse_velocity(old).unwrap();
    assert_eq!(c.forwarding, Forwarding::Modern);
    assert!(c.secret_path().is_none() && c.inline_secret.is_some());
    // Fichier absolu dans le conteneur, ou hors du serveur.
    assert_eq!(
        proxy_file_path("/home/container/secrets/fwd.secret").as_deref(),
        Some("/secrets/fwd.secret")
    );
    assert_eq!(
        proxy_file_path("./forwarding.secret").as_deref(),
        Some("/forwarding.secret")
    );
    assert!(proxy_file_path("../../etc/passwd").is_none());
    assert!(parse_velocity("[servers\n").is_err());
}

#[test]
fn bungee_config_is_read() {
    let c = parse_bungee(BUNGEE_DEFAULT).unwrap();
    assert_eq!(c.kind, ProxyKind::Bungee);
    assert_eq!(
        c.entries,
        [Entry {
            name: "lobby".into(),
            address: "localhost:25565".into()
        }]
    );
    assert_eq!(c.try_order, ["lobby"]);
    assert_eq!(
        c.forced_hosts,
        [("pvp.md-5.net".to_string(), vec!["pvp".to_string()])]
    );
    assert_eq!(
        (c.forwarding.clone(), c.online_mode, c.listeners),
        (Forwarding::None, Some(true), 1)
    );
    assert!(c.secret_path().is_none());
    let forwarded = BUNGEE_DEFAULT.replace("ip_forward: false", "ip_forward: true");
    assert_eq!(
        parse_bungee(&forwarded).unwrap().forwarding,
        Forwarding::Legacy
    );
    // Un config.yml qui n'est pas celui de BungeeCord.
    assert!(parse_bungee("settings:\n  debug: false\n").is_err());
}

#[test]
fn server_type_comes_from_the_source_or_the_jar() {
    use ServerType::*;
    assert_eq!(ServerType::detect(Some("paper"), None), Some(Paper));
    assert_eq!(ServerType::detect(Some("purpur"), None), Some(Paper));
    assert_eq!(ServerType::detect(Some("velocity"), None), Some(Velocity));
    assert_eq!(ServerType::detect(Some("waterfall"), None), Some(Bungee));
    assert_eq!(ServerType::detect(Some("spigot"), None), Some(Spigot));
    assert_eq!(ServerType::detect(Some("fabric"), None), Some(Fabric));
    assert_eq!(ServerType::detect(Some("neoforge"), None), Some(Forge));
    assert_eq!(ServerType::detect(Some("vanilla"), None), Some(Vanilla));
    assert_eq!(
        ServerType::detect(
            Some("explicit_jar"),
            Some("velocity-3.4.0-SNAPSHOT-500.jar")
        ),
        Some(Velocity)
    );
    assert_eq!(
        ServerType::detect(Some(""), Some("BungeeCord.jar")),
        Some(Bungee)
    );
    assert_eq!(
        ServerType::detect(None, Some("canvas-build.876.jar")),
        Some(Paper)
    );
    assert_eq!(
        ServerType::detect(Some("explicit_jar"), Some("server.jar")),
        None
    );
    assert!(
        below_1_13(Some("1.12.2"))
            && !below_1_13(Some("1.13"))
            && !below_1_13(Some("26.1.2"))
            && !below_1_13(None)
    );
}

// --- Adresses ------------------------------------------------------------------------------------

#[test]
fn addresses_are_parsed_and_classified() {
    assert_eq!(
        Addr::parse(" 203.0.113.10:25565 "),
        Addr {
            host: "203.0.113.10".into(),
            port: Some(25565)
        }
    );
    assert_eq!(
        Addr::parse("Play.Example.com."),
        Addr {
            host: "play.example.com".into(),
            port: None
        }
    );
    assert_eq!(
        Addr::parse("[::1]:25577"),
        Addr {
            host: "::1".into(),
            port: Some(25577)
        }
    );
    assert_eq!(
        Addr::parse("2001:db8::1"),
        Addr {
            host: "2001:db8::1".into(),
            port: None
        }
    );
    assert_eq!(Addr::parse("lobby:abc").port, None);
    assert_eq!(
        Addr::parse("play.example.com").port_or_default(),
        DEFAULT_PORT
    );
    for local in ["127.0.0.1", "0.0.0.0", "localhost", "::1"] {
        assert_eq!(classify(local), HostClass::Local, "{local}");
    }
    for private in [
        "10.0.0.5",
        "172.18.0.1",
        "192.168.1.2",
        "100.64.1.1",
        "fd00::1",
    ] {
        assert_eq!(classify(private), HostClass::Private, "{private}");
    }
    assert_eq!(classify("203.0.113.10"), HostClass::Public);
    assert_eq!(classify("play.example.com"), HostClass::Name);
}

#[test]
fn endpoints_come_from_the_server_details_or_the_list() {
    let v = json!({
        "server": { "ip": "203.0.113.10", "port": 23628, "dns": "Survie.minesr.com." },
        "ports": [{ "port": 43748, "ip": "203.0.113.10", "description": "" }, { "port": 43749, "description": "" }],
        "sftp": { "user": "ne-doit-pas-etre-lu" },
    });
    let e = Endpoint::from_server_value(7, &v).unwrap();
    assert_eq!(
        (e.ip.as_str(), e.port, e.dns.as_deref()),
        ("203.0.113.10", 23628, Some("survie.minesr.com"))
    );
    assert_eq!(
        e.secondary,
        [
            ("203.0.113.10".to_string(), 43748),
            ("203.0.113.10".to_string(), 43749)
        ]
    );
    assert_eq!(e.address(), "203.0.113.10:23628");
    assert!(e.public());
    assert!(
        Endpoint::from_server_value(7, &json!({ "server": { "ip": "", "port": 1 } })).is_none()
    );
    assert_eq!(
        Endpoint::from_list_address(7, "203.0.113.10:23628")
            .unwrap()
            .port,
        23628
    );
    assert!(Endpoint::from_list_address(7, "survie.minesr.com").is_none());
}

#[test]
fn entries_are_matched_to_account_servers() {
    let mut a = ep(1, "203.0.113.10", 23628);
    a.secondary.push(("203.0.113.10".into(), 43748));
    let mut b = ep(2, "203.0.113.11", 24214);
    b.dns = Some("pvp.minesr.com".into());
    let eps = vec![a, b, ep(3, "203.0.113.12", 25000)];
    let m = |s: &str| match_entry(&Addr::parse(s), &[], &eps);
    assert_eq!(
        m("203.0.113.10:23628"),
        Some(Matched {
            server_id: 1,
            how: How::Primary,
            port_ok: true
        })
    );
    assert_eq!(
        m("203.0.113.10:43748").map(|x| (x.server_id, x.how)),
        Some((1, How::Secondary))
    );
    // Même IP partagée, autre port : un autre serveur, peut-être hors du compte.
    assert_eq!(m("203.0.113.10:25565"), None);
    assert_eq!(
        m("pvp.minesr.com:24214"),
        Some(Matched {
            server_id: 2,
            how: How::Dns,
            port_ok: true
        })
    );
    // Nom DNS sans port : le proxy utiliserait 25565.
    assert_eq!(m("pvp.minesr.com").map(|x| x.port_ok), Some(false));
    assert_eq!(m("127.0.0.1:23628"), None);
    assert_eq!(m("198.51.100.7:25565"), None);
    // Adresse privée : rapprochée par un port unique seulement.
    assert_eq!(
        m("172.18.0.1:25000").map(|x| (x.server_id, x.how)),
        Some((3, How::PortOnly))
    );
    let twice = vec![ep(1, "203.0.113.10", 25000), ep(2, "203.0.113.11", 25000)];
    assert_eq!(
        match_entry(&Addr::parse("10.0.0.2:25000"), &[], &twice),
        None
    );
    // Nom d'hôte résolu.
    let ip: IpAddr = "203.0.113.12".parse().unwrap();
    assert_eq!(
        match_entry(&Addr::parse("jeu.example.org:25000"), &[ip], &eps)
            .map(|x| (x.server_id, x.how)),
        Some((3, How::Resolved))
    );
}

#[tokio::test]
async fn proxy_entries_are_linked_to_named_account_servers() {
    let text = VELOCITY_DEFAULT.replace(
        "lobby = \"127.0.0.1:30066\"",
        "lobby = \"203.0.113.10:23628\"\njeu = \"pvp.minesr.com:24214\"",
    );
    let p = proxy_state(&text, None);
    let mut b = ep(2, "203.0.113.11", 24214);
    b.dns = Some("pvp.minesr.com".into());
    let mc = |id: i64, name: &str, endpoint: Endpoint| McServer {
        id,
        name: name.into(),
        status: "active".into(),
        address: endpoint.address(),
        ty: Some(ServerType::Paper),
        version: None,
        endpoint: Some(endpoint),
    };
    let servers = vec![
        mc(1, "Lobby", ep(1, "203.0.113.10", 23628)),
        mc(2, "PvP", b),
    ];
    let eps: Vec<Endpoint> = servers.iter().filter_map(|s| s.endpoint.clone()).collect();
    let links = match_links(std::slice::from_ref(&p), &eps, &servers).await;
    let by_name = |n: &str| links[0].iter().find(|l| l.entry.name == n).unwrap();
    let lobby = by_name("lobby");
    assert_eq!(
        (
            lobby.matched.map(|m| m.server_id),
            lobby.server_name.as_deref()
        ),
        (Some(1), Some("Lobby"))
    );
    assert_eq!(lobby.server_address.as_deref(), Some("203.0.113.10:23628"));
    assert!(lobby.reachable);
    let jeu = by_name("jeu");
    assert_eq!(
        jeu.matched.map(|m| (m.server_id, m.how)),
        Some((2, How::Dns))
    );
    assert!(by_name("factions").matched.is_none());
}

// --- Serveurs de jeu -----------------------------------------------------------------------------

#[test]
fn backend_settings_are_read_and_secrets_compared_internally() {
    let f = BackendFiles {
        paper_global: Some(paper_global(true, SECRET_PROXY)),
        spigot: Some(spigot(false)),
        plugins: vec!["BungeeGuard-1.3.0.jar".into(), "LuckPerms.jar".into()],
        bungeeguard_config: Some("allowed-tokens:\n- JETON_DE_TEST_1\n- JETON_DE_TEST_2\n".into()),
        ..Default::default()
    };
    let s = backend_state(None, None, Some(false), &f);
    assert_eq!(s.ty(), ServerType::Paper, "déduit de paper-global.yml");
    assert_eq!(s.paper, PaperFile::Global);
    assert_eq!(
        (
            s.velocity_enabled,
            s.velocity_online_mode,
            s.bungee_online_mode
        ),
        (Some(true), Some(true), Some(true))
    );
    assert_eq!(s.velocity_secret.as_deref(), Some(SECRET_PROXY));
    assert_eq!((s.spigot_file, s.bungeecord), (true, Some(false)));
    assert!(s.bungeeguard);
    assert_eq!(s.bungeeguard_tokens, ["JETON_DE_TEST_1", "JETON_DE_TEST_2"]);
    // Secret vide : absent.
    let empty = backend_state(
        None,
        None,
        None,
        &files(Some(paper_global(false, "")), None),
    );
    assert_eq!(empty.velocity_secret, None);
    // Paper d'avant 1.19.
    let legacy = BackendFiles {
        paper_legacy: Some("settings:\n  bungee-online-mode: true\n  velocity-support:\n    enabled: true\n    online-mode: false\n    secret: 'abc'\n".into()),
        ..Default::default()
    };
    let l = backend_state(Some(ServerType::Paper), None, None, &legacy);
    assert_eq!(
        (l.paper, l.velocity_enabled, l.velocity_online_mode),
        (PaperFile::Legacy, Some(true), Some(false))
    );
    // FabricProxy-Lite.
    let fabric = BackendFiles {
        fabric_proxy: Some("hackOnlineMode = true\nsecret = \"ABCDEF\"\n".into()),
        mods: vec!["FabricProxy-Lite-2.9.0.jar".into()],
        ..Default::default()
    };
    let fb = backend_state(None, None, None, &fabric);
    assert_eq!(fb.ty(), ServerType::Fabric);
    assert!(fb.fabric_proxy_mod && fb.fabric_proxy_config);
    assert_eq!(fb.fabric_proxy_secret.as_deref(), Some("ABCDEF"));

    assert_eq!(
        SecretMatch::compare(Some("a"), Some("a")),
        SecretMatch::Same
    );
    assert_eq!(
        SecretMatch::compare(Some("a"), Some("b")),
        SecretMatch::Different
    );
    assert_eq!(SecretMatch::compare(Some("a"), None), SecretMatch::Missing);
    assert_eq!(SecretMatch::compare(None, Some("b")), SecretMatch::Unknown);
}

// --- Problèmes -----------------------------------------------------------------------------------

fn link(
    name: &str,
    address: &str,
    matched: Option<(i64, &str)>,
    backend: Option<BackendState>,
) -> Link {
    Link {
        entry: Entry {
            name: name.into(),
            address: address.into(),
        },
        matched: matched.map(|(id, _)| Matched {
            server_id: id,
            how: How::Primary,
            port_ok: true,
        }),
        server_name: matched.map(|(_, n)| n.to_string()),
        server_address: matched.map(|_| address.to_string()),
        reachable: matched.is_some(),
        backend: backend.map(Ok),
    }
}

fn messages(problems: &[Problem], sev: Severity) -> Vec<String> {
    problems
        .iter()
        .filter(|p| p.severity == sev)
        .map(|p| p.message.clone())
        .collect()
}

#[test]
fn a_velocity_network_reports_its_problems_without_leaking_the_secret() {
    let text = VELOCITY_DEFAULT
        .replace("\"NONE\"", "\"modern\"")
        .replace("try = [\n    \"lobby\"\n]", "try = [\"lobby\", \"absent\"]")
        .replace("lobby = \"127.0.0.1:30066\"", "lobby = \"203.0.113.10:23628\"\nsurvie = \"203.0.113.11:25000\"\nexterne = \"198.51.100.7:25565\"");
    let p = proxy_state(&text, Some(SECRET_PROXY));
    let ok = backend_state(
        Some(ServerType::Paper),
        None,
        Some(false),
        &files(Some(paper_global(true, SECRET_PROXY)), Some(spigot(false))),
    );
    let bad = backend_state(
        Some(ServerType::Paper),
        None,
        Some(true),
        &files(Some(paper_global(true, SECRET_OTHER)), Some(spigot(true))),
    );
    let links = vec![
        link("lobby", "203.0.113.10:23628", Some((1, "Lobby")), Some(ok)),
        link(
            "survie",
            "203.0.113.11:25000",
            Some((2, "Survie")),
            Some(bad),
        ),
        link("externe", "198.51.100.7:25565", None, None),
        link("factions", "127.0.0.1:30067", None, None),
    ];
    let problems = evaluate(&p, &links);
    let critical = messages(&problems, Severity::Critical);
    assert!(
        critical
            .iter()
            .any(|m| m.contains("« Survie »") && m.contains("différent")),
        "{critical:?}"
    );
    assert!(
        critical
            .iter()
            .any(|m| m.contains("« absent »") && m.contains("refuse de démarrer")),
        "{critical:?}"
    );
    let important = messages(&problems, Severity::Important);
    assert!(
        important
            .iter()
            .any(|m| m.contains("online-mode=true sur « Survie »")),
        "{important:?}"
    );
    assert!(
        important
            .iter()
            .any(|m| m.contains("settings.bungeecord: true")),
        "{important:?}"
    );
    let info = messages(&problems, Severity::Info);
    assert!(
        info.iter()
            .any(|m| m.contains("« externe »") && m.contains("aucun serveur de ce compte")),
        "{info:?}"
    );
    assert!(
        info.iter()
            .any(|m| m.contains("« factions »") && m.contains("adresse locale")),
        "{info:?}"
    );
    // Le serveur bien réglé ne produit rien.
    assert!(
        !problems.iter().any(|x| x.server_id == Some(1)),
        "{problems:?}"
    );
    // Rien ne laisse passer un secret, ni les problèmes, ni le rendu.
    let rendered = render_proxy(&p, &links, &problems).to_string();
    let all = format!("{problems:?}{rendered}");
    assert!(!all.contains(SECRET_PROXY) && !all.contains(SECRET_OTHER));
    assert!(rendered.contains("DIFFÉRENT") && rendered.contains("identique"));
}

#[test]
fn a_default_proxy_without_forwarding_or_secret_is_flagged() {
    let p = proxy_state(VELOCITY_DEFAULT, None);
    let paper = backend_state(
        Some(ServerType::Paper),
        None,
        Some(false),
        &files(Some(paper_global(false, "")), None),
    );
    let links = vec![
        link("lobby", "127.0.0.1:30066", None, None),
        link("jeu", "203.0.113.10:23628", Some((1, "Jeu")), Some(paper)),
    ];
    let problems = evaluate(&p, &links);
    let important = messages(&problems, Severity::Important);
    assert!(
        important
            .iter()
            .any(|m| m.contains("player-info-forwarding-mode = none")),
        "{important:?}"
    );
    // Repli local : important (c'est là qu'arrivent les joueurs).
    assert!(
        important
            .iter()
            .any(|m| m.contains("« lobby »") && m.contains("serveur de repli")),
        "{important:?}"
    );
    // Mode modern sans secret : Velocity ne démarre pas.
    let modern = proxy_state(&VELOCITY_DEFAULT.replace("\"NONE\"", "\"MODERN\""), None);
    let problems = evaluate(&modern, &links);
    let critical = messages(&problems, Severity::Critical);
    assert!(
        critical.iter().any(|m| m.contains("sans secret")),
        "{critical:?}"
    );
    assert!(
        critical
            .iter()
            .any(|m| m.contains("« Jeu »") && m.contains("transfert Velocity désactivé")),
        "{critical:?}"
    );
}

#[test]
fn a_secondary_port_or_a_missing_port_is_reported() {
    let p = proxy_state(
        &VELOCITY_DEFAULT.replace("\"NONE\"", "\"modern\""),
        Some(SECRET_PROXY),
    );
    let mut a = link("hub", "203.0.113.10:43748", Some((1, "Lobby")), None);
    a.matched = Some(Matched {
        server_id: 1,
        how: How::Secondary,
        port_ok: true,
    });
    a.server_address = Some("203.0.113.10:23628".into());
    let mut b = link("jeu", "survie.minesr.com", Some((2, "Survie")), None);
    b.matched = Some(Matched {
        server_id: 2,
        how: How::Dns,
        port_ok: false,
    });
    b.server_address = Some("203.0.113.11:25000".into());
    let important = messages(&evaluate(&p, &[a, b]), Severity::Important);
    assert!(
        important
            .iter()
            .any(|m| m.contains("port secondaire") && m.contains("203.0.113.10:23628")),
        "{important:?}"
    );
    assert!(
        important.iter().any(|m| m.contains("« jeu »")
            && m.contains("25565")
            && m.contains("203.0.113.11:25000")),
        "{important:?}"
    );
}

#[test]
fn legacy_forwarding_without_bungeeguard_is_critical() {
    let text = BUNGEE_DEFAULT
        .replace("ip_forward: false", "ip_forward: true")
        .replace("address: localhost:25565", "address: 203.0.113.10:23628");
    let mut p = proxy_state(&text, None);
    let spigot_files = files(None, Some(spigot(true)));
    let open = backend_state(Some(ServerType::Spigot), None, Some(false), &spigot_files);
    let problems = evaluate(
        &p,
        &[link(
            "lobby",
            "203.0.113.10:23628",
            Some((1, "Lobby")),
            Some(open),
        )],
    );
    let critical = messages(&problems, Severity::Critical);
    assert!(
        critical
            .iter()
            .any(|m| m.contains("sans BungeeGuard") && m.contains("203.0.113.10:23628")),
        "{critical:?}"
    );
    // pvp.md-5.net → pvp, non déclaré : important pour BungeeCord (il démarre quand même).
    assert!(messages(&problems, Severity::Important)
        .iter()
        .any(|m| m.contains("pvp.md-5.net")));

    // BungeeGuard des deux côtés : le jeton est comparé sans être montré.
    let guarded_files = BackendFiles {
        spigot: Some(spigot(true)),
        plugins: vec!["BungeeGuard.jar".into()],
        bungeeguard_config: Some("allowed-tokens:\n- JETON_DE_TEST_SERVEUR\n".into()),
        ..Default::default()
    };
    let guarded = backend_state(Some(ServerType::Spigot), None, Some(false), &guarded_files);
    p.bungeeguard = true;
    p.bungeeguard_token = Some("JETON_DE_TEST_PROXY_X".into());
    let l = [link(
        "lobby",
        "203.0.113.10:23628",
        Some((1, "Lobby")),
        Some(guarded.clone()),
    )];
    let problems = evaluate(&p, &l);
    assert!(messages(&problems, Severity::Critical)
        .iter()
        .any(|m| m.contains("jeton BungeeGuard")));
    assert!(!format!("{problems:?}").contains("JETON_DE_TEST"));
    p.bungeeguard_token = Some("JETON_DE_TEST_SERVEUR".into());
    assert!(messages(&evaluate(&p, &l), Severity::Critical).is_empty());
}

#[test]
fn modern_forwarding_needs_paper_or_a_proxy_mod() {
    let p = proxy_state(
        &VELOCITY_DEFAULT.replace("\"NONE\"", "\"modern\""),
        Some(SECRET_PROXY),
    );
    let spigot_srv = backend_state(
        Some(ServerType::Spigot),
        None,
        Some(false),
        &files(None, Some(spigot(false))),
    );
    let fabric_srv = backend_state(
        Some(ServerType::Fabric),
        None,
        Some(false),
        &BackendFiles::default(),
    );
    let fabric_ok = backend_state(
        Some(ServerType::Fabric),
        None,
        Some(false),
        &BackendFiles {
            fabric_proxy: Some(format!("secret = \"{SECRET_PROXY}\"\n")),
            ..Default::default()
        },
    );
    let old = backend_state(
        Some(ServerType::Paper),
        Some("1.12.2".into()),
        Some(false),
        &files(Some(paper_global(true, SECRET_PROXY)), None),
    );
    let problems = evaluate(
        &p,
        &[
            link("a", "203.0.113.10:1", Some((1, "Spigot")), Some(spigot_srv)),
            link("b", "203.0.113.10:2", Some((2, "Fabric")), Some(fabric_srv)),
            link(
                "c",
                "203.0.113.10:3",
                Some((3, "FabricOk")),
                Some(fabric_ok),
            ),
            link("d", "203.0.113.10:4", Some((4, "Ancien")), Some(old)),
        ],
    );
    let critical = messages(&problems, Severity::Critical);
    assert!(
        critical
            .iter()
            .any(|m| m.contains("« Spigot »") && m.contains("Spigot")),
        "{critical:?}"
    );
    assert!(
        critical
            .iter()
            .any(|m| m.contains("« Ancien »") && m.contains("1.13")),
        "{critical:?}"
    );
    assert!(messages(&problems, Severity::Important)
        .iter()
        .any(|m| m.contains("FabricProxy-Lite absent")));
    assert!(
        !problems.iter().any(|x| x.server_id == Some(3)),
        "{problems:?}"
    );
}

#[test]
fn a_server_set_up_for_a_missing_proxy_is_noted() {
    let set_up = backend_state(
        Some(ServerType::Paper),
        None,
        Some(false),
        &files(Some(paper_global(true, SECRET_PROXY)), None),
    );
    let plain = backend_state(
        Some(ServerType::Paper),
        None,
        Some(true),
        &files(Some(paper_global(false, "")), None),
    );
    assert!(orphan_note("« Jeu » (1)", Some(&set_up)).contains("usurpable"));
    assert!(!orphan_note("« Jeu » (1)", Some(&plain)).contains("réglé pour un proxy"));
    let offline = backend_state(
        Some(ServerType::Paper),
        None,
        Some(false),
        &files(Some(paper_global(false, "")), None),
    );
    assert!(orphan_note("« Jeu » (1)", Some(&offline)).contains("n'importe quel pseudo"));
    assert!(orphan_note("« Jeu » (1)", None).contains("« Jeu » (1)"));
    let s = McServer {
        id: 1,
        name: "Jeu".into(),
        status: "active".into(),
        address: "203.0.113.10:23628".into(),
        ty: Some(ServerType::Paper),
        version: None,
        endpoint: None,
    };
    let brief = backend_brief(&s, &set_up).to_string();
    assert!(
        brief.contains("\"velocity_secret\":\"défini\"") && !brief.contains(SECRET_PROXY),
        "{brief}"
    );
}

#[test]
fn secrets_are_scrubbed_from_any_output() {
    let mut v = json!({
        "a": format!("début {SECRET_PROXY} fin"),
        "b": [{ "c": SECRET_PROXY }],
        "court": "abc",
    });
    v[SECRET_PROXY] = json!(1);
    let out = scrub_value(v, &[SECRET_PROXY.to_string(), "abc".to_string()]).to_string();
    assert!(!out.contains(SECRET_PROXY), "{out}");
    assert!(out.contains("début [SECRET] fin"));
    // Trop court pour être effacé sans risque (texte ordinaire).
    assert!(out.contains("\"abc\""));
    assert_eq!(
        scrub_text(&format!("erreur {SECRET_PROXY}"), &[SECRET_PROXY.into()]),
        "erreur [SECRET]"
    );
}

// --- link_proxy : arguments --------------------------------------------------------------------------

#[test]
fn link_arguments_are_checked() {
    let a = args(1, &[2, 3]);
    assert_eq!(
        (a.proxy, a.backends.clone(), a.dry_run, a.snapshot, a.mode),
        (1, vec![2, 3], true, true, None)
    );
    let err = |v: Value| parse_args(&v).unwrap_err();
    assert!(err(json!({ "backend_server_ids": [2] })).contains("proxy_server_id"));
    assert!(err(json!({ "proxy_server_id": 1 })).contains("backend_server_ids"));
    assert!(
        err(json!({ "proxy_server_id": 1, "backend_server_ids": [] }))
            .contains("backend_server_ids")
    );
    assert!(err(json!({ "proxy_server_id": 1, "backend_server_ids": [1] })).contains("son propre"));
    assert!(
        err(json!({ "proxy_server_id": 1, "backend_server_ids": [2, 2] })).contains("deux fois")
    );
    assert!(
        err(json!({ "proxy_server_id": 1, "backend_server_ids": [2], "mode": "bungeeguard" }))
            .contains("modern")
    );
    assert!(
        err(json!({ "proxy_server_id": 1, "backend_server_ids": [2], "names": ["a b"] }))
            .contains("refusé")
    );
    assert!(
        err(json!({ "proxy_server_id": 1, "backend_server_ids": [2], "names": { "9": "x" } }))
            .contains("pas dans")
    );
    let a = parse_args(&json!({
        "proxy_server_id": "1", "backend_server_ids": [2, 3], "names": ["lobby", null], "try": ["lobby", 3],
        "mode": "Legacy", "dry_run": false, "snapshot": false,
    }))
    .unwrap();
    assert_eq!(a.names, HashMap::from([(2, "lobby".to_string())]));
    assert_eq!(
        a.try_order,
        Some(vec![TryItem::Name("lobby".into()), TryItem::Server(3)])
    );
    assert_eq!(
        (a.mode, a.dry_run, a.snapshot),
        (Some(Mode::Legacy), false, false)
    );
    let a = parse_args(
        &json!({ "proxy_server_id": 1, "backend_server_ids": [2, 3], "names": { "3": "survie" } }),
    )
    .unwrap();
    assert_eq!(a.names, HashMap::from([(3, "survie".to_string())]));
}

#[test]
fn entry_names_are_derived_from_server_names() {
    assert_eq!(slug("Serveur Créatif #2", 5), "serveur-creatif-2");
    assert_eq!(slug("  Lobby  ", 5), "lobby");
    assert_eq!(slug("***", 5), "serveur-5");
    assert_eq!(slug("Try", 5), "serveur-5");
    assert_eq!(slug(&"a".repeat(50), 5).len(), 32);
    assert!(valid_name("survie_1-b").is_ok());
    for bad in ["", "a b", "try", "é", &"x".repeat(33)] {
        assert!(valid_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn generated_secrets_are_random_letters_and_digits() {
    let a = generate_secret().unwrap();
    let b = generate_secret().unwrap();
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
    assert_ne!(a, b);
}

// --- link_proxy : plan -----------------------------------------------------------------------------

#[test]
fn a_default_velocity_is_linked_in_modern_mode_without_showing_the_secret() {
    let p = proxy_state(VELOCITY_DEFAULT, None);
    let backends = vec![
        paper_backend(
            1,
            "Lobby",
            "203.0.113.10",
            23628,
            files(Some(paper_global(false, "")), Some(spigot(false))),
            Some(true),
        ),
        paper_backend(
            2,
            "Survie",
            "203.0.113.11",
            25000,
            files(Some(paper_global(false, "")), Some(spigot(true))),
            Some(false),
        ),
    ];
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: VELOCITY_DEFAULT,
        },
        &backends,
        &args(100, &[1, 2]),
        Some(SECRET_NEW),
        &no_redact,
    )
    .unwrap();
    assert_eq!(plan.mode, Mode::Modern);
    assert_eq!(plan.secret, SecretPlan::Generated);
    // L'entrée d'exemple « lobby » (127.0.0.1) est reprise ; « survie » est ajoutée.
    let entries: Vec<(&str, &str, &str)> = plan
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.address.as_str(), e.status))
        .collect();
    assert_eq!(
        entries,
        [
            ("lobby", "203.0.113.10:23628", "mise à jour"),
            ("survie", "203.0.113.11:25000", "ajoutée")
        ]
    );
    assert_eq!(plan.try_order, ["lobby"]);
    assert!(!plan.try_changed);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("factions, minigames")),
        "{:?}",
        plan.warnings
    );

    let cfg = step(&plan, "proxy.config");
    assert_eq!(cfg.status, Status::Change);
    let diff = cfg.diff.as_deref().unwrap();
    assert!(
        diff.contains("+lobby = \"203.0.113.10:23628\"")
            && diff.contains("+survie = \"203.0.113.11:25000\""),
        "{diff}"
    );
    assert!(
        diff.contains("+player-info-forwarding-mode = \"MODERN\""),
        "{diff}"
    );
    let secret_step = step(&plan, "proxy.secret");
    assert_eq!(secret_step.status, Status::Change);
    assert_eq!(
        secret_step.action,
        Some(Action::SecretFile {
            path: "/forwarding.secret".into()
        })
    );

    let paper1 = step(&plan, "1.paper");
    assert_eq!(paper1.status, Status::Change);
    assert!(paper1.diff.as_deref().unwrap().contains("[SECRET]"));
    match &paper1.action {
        Some(Action::Config { path, changes }) => {
            assert_eq!(path, PAPER_GLOBAL);
            assert!(changes.contains(&("proxies.velocity.secret".to_string(), json!(SECRET_NEW))));
            assert!(changes.contains(&("proxies.velocity.enabled".to_string(), json!(true))));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(step(&plan, "1.online-mode").status, Status::Change);
    assert!(
        plan.steps.iter().all(|s| s.id != "1.spigot"),
        "bungeecord déjà à false"
    );
    assert_eq!(step(&plan, "2.spigot").status, Status::Change);
    assert_eq!(step(&plan, "2.online-mode").status, Status::Ok);

    // Le secret n'apparaît nulle part : rendu, diffs, débogage.
    let rendered = render_plan(&p, &plan, true, &HashMap::new()).to_string();
    let debug = format!("{plan:?}");
    for out in [&rendered, &debug] {
        assert!(!out.contains(SECRET_NEW), "secret affiché");
    }
    assert!(rendered.contains("secret généré") && rendered.contains("jamais affiché"));
}

#[test]
fn a_linked_network_needs_nothing_more() {
    let text = VELOCITY_DEFAULT.replace("\"NONE\"", "\"modern\"").replace(
        "lobby = \"127.0.0.1:30066\"",
        "lobby = \"203.0.113.10:23628\"",
    );
    let p = proxy_state(&text, Some(SECRET_PROXY));
    let mut f = files(Some(paper_global(true, SECRET_PROXY)), Some(spigot(false)));
    f.plugins = vec!["LuckPerms.jar".into()];
    let backends = vec![paper_backend(
        1,
        "Lobby",
        "203.0.113.10",
        23628,
        f,
        Some(false),
    )];
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: &text,
        },
        &backends,
        &args(100, &[1]),
        None,
        &no_redact,
    )
    .unwrap();
    assert_eq!(plan.secret, SecretPlan::Reused("/forwarding.secret".into()));
    let not_ok: Vec<&Step> = plan
        .steps
        .iter()
        .filter(|s| s.status != Status::Ok)
        .collect();
    assert!(not_ok.is_empty(), "{not_ok:?}");
    assert_eq!(plan.entries[0].status, "inchangée");
}

#[test]
fn existing_entries_are_reused_and_try_is_kept_or_set() {
    // Un réseau existant : « hub » vise déjà le serveur 1 (nom DNS et bon port).
    let text = VELOCITY_DEFAULT
        .replace("\"NONE\"", "\"modern\"")
        .replace(
            "lobby = \"127.0.0.1:30066\"",
            "hub = \"203.0.113.10:23628\"",
        )
        .replace("try = [\n    \"lobby\"\n]", "try = [\"hub\"]");
    let p = proxy_state(&text, Some(SECRET_PROXY));
    let paper = || files(Some(paper_global(true, SECRET_PROXY)), Some(spigot(false)));
    let backends = vec![
        paper_backend(1, "Lobby", "203.0.113.10", 23628, paper(), Some(false)),
        paper_backend(2, "Survie", "203.0.113.11", 25000, paper(), Some(false)),
    ];
    let proxy = ProxyInput {
        state: &p,
        config_text: &text,
    };
    let plan = build_plan(&proxy, &backends, &args(100, &[1, 2]), None, &no_redact).unwrap();
    assert_eq!(plan.entries[0].name, "hub", "entrée existante réutilisée");
    assert_eq!(plan.entries[0].status, "inchangée");
    assert_eq!(plan.try_order, ["hub"], "liste de repli valable : gardée");
    // `try` imposé : noms, identifiants (nombre ou texte).
    let mut a = args(100, &[1, 2]);
    a.try_order = Some(vec![TryItem::Server(2), TryItem::Name("HUB".into())]);
    let plan = build_plan(&proxy, &backends, &a, None, &no_redact).unwrap();
    assert_eq!(plan.try_order, ["survie", "hub"]);
    assert!(plan.try_changed);
    assert!(step(&plan, "proxy.config")
        .diff
        .as_deref()
        .unwrap()
        .contains("survie"));
    a.try_order = Some(vec![TryItem::Name("2".into())]);
    assert_eq!(
        build_plan(&proxy, &backends, &a, None, &no_redact)
            .unwrap()
            .try_order,
        ["survie"]
    );
    a.try_order = Some(vec![TryItem::Name("inconnu".into())]);
    assert!(build_plan(&proxy, &backends, &a, None, &no_redact)
        .unwrap_err()
        .contains("inconnu"));
    // Deux serveurs sous le même nom.
    let mut a = args(100, &[1, 2]);
    a.names = HashMap::from([(1, "x".to_string()), (2, "X".to_string())]);
    assert!(build_plan(&proxy, &backends, &a, None, &no_redact)
        .unwrap_err()
        .contains("deux serveurs"));
}

#[test]
fn an_entry_on_a_wrong_port_keeps_its_name_and_gets_the_right_address() {
    // « hub » vise un port secondaire du serveur 1 ; « jeu » vise le serveur 2 par son nom DNS, sans port.
    let text = VELOCITY_DEFAULT
        .replace(
            "lobby = \"127.0.0.1:30066\"",
            "hub = \"203.0.113.10:43748\"\njeu = \"survie.minesr.com\"",
        )
        .replace("try = [\n    \"lobby\"\n]", "try = [\"hub\"]");
    let p = proxy_state(&text, Some(SECRET_PROXY));
    let paper = || files(Some(paper_global(true, SECRET_PROXY)), Some(spigot(false)));
    let mut b1 = paper_backend(1, "Lobby", "203.0.113.10", 23628, paper(), Some(false));
    b1.endpoint.secondary.push(("203.0.113.10".into(), 43748));
    let mut b2 = paper_backend(2, "Survie", "203.0.113.11", 25000, paper(), Some(false));
    b2.endpoint.dns = Some("survie.minesr.com".into());
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: &text,
        },
        &[b1, b2],
        &args(100, &[1, 2]),
        None,
        &no_redact,
    )
    .unwrap();
    let entries: Vec<(&str, &str, &str)> = plan
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.address.as_str(), e.status))
        .collect();
    assert_eq!(
        entries,
        [
            ("hub", "203.0.113.10:23628", "mise à jour"),
            ("jeu", "203.0.113.11:25000", "mise à jour")
        ]
    );
    let diff = step(&plan, "proxy.config").diff.clone().unwrap();
    assert!(
        diff.contains("+hub = \"203.0.113.10:23628\"")
            && diff.contains("+jeu = \"203.0.113.11:25000\""),
        "{diff}"
    );
}

#[test]
fn unsupported_backends_become_manual_steps() {
    let p = proxy_state(VELOCITY_DEFAULT, Some(SECRET_PROXY));
    let spigot_state = backend_state(
        Some(ServerType::Spigot),
        None,
        Some(true),
        &files(None, Some(spigot(false))),
    );
    let fabric_state = backend_state(
        Some(ServerType::Fabric),
        None,
        Some(true),
        &BackendFiles::default(),
    );
    let backends = vec![
        BackendInput {
            server_id: 1,
            server: "Spigot".into(),
            endpoint: ep(1, "203.0.113.10", 1),
            state: Ok((spigot_state, files(None, Some(spigot(false))))),
        },
        BackendInput {
            server_id: 2,
            server: "Fabric".into(),
            endpoint: ep(2, "203.0.113.10", 2),
            state: Ok((fabric_state, BackendFiles::default())),
        },
        paper_backend(3, "Neuf", "203.0.113.10", 3, BackendFiles::default(), None),
        BackendInput {
            server_id: 4,
            server: "Suspendu".into(),
            endpoint: ep(4, "203.0.113.10", 4),
            state: Err("serveur suspendu".into()),
        },
    ];
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: VELOCITY_DEFAULT,
        },
        &backends,
        &args(100, &[1, 2, 3, 4]),
        None,
        &no_redact,
    )
    .unwrap();
    for id in 1..=4 {
        let mine: Vec<&Step> = plan.steps.iter().filter(|s| s.server_id == id).collect();
        assert!(
            !mine.is_empty() && mine.iter().all(|s| s.status == Status::Manual),
            "{id} : {mine:?}"
        );
    }
    let manual = |id: i64| {
        plan.steps
            .iter()
            .find(|s| s.server_id == id)
            .unwrap()
            .summary
            .clone()
    };
    assert!(manual(1).contains("Paper"));
    assert!(manual(2).contains("FabricProxy-Lite") && manual(2).contains("jamais le secret"));
    assert!(manual(3).contains("paper-global.yml absent"));
    assert!(manual(4).contains("suspendu"));
    // Les serveurs restent déclarés sur le proxy (leur adresse est sûre).
    assert_eq!(plan.entries.len(), 4);
    assert_eq!(plan.secret, SecretPlan::Reused("/forwarding.secret".into()));
}

#[test]
fn bungeecord_is_linked_in_legacy_mode_and_asks_for_bungeeguard() {
    let p = proxy_state(BUNGEE_DEFAULT, None);
    let f = files(Some(paper_global(false, "")), Some(spigot(false)));
    let backends = vec![paper_backend(
        1,
        "Lobby",
        "203.0.113.10",
        23628,
        f,
        Some(true),
    )];
    let proxy = ProxyInput {
        state: &p,
        config_text: BUNGEE_DEFAULT,
    };
    let plan = build_plan(&proxy, &backends, &args(100, &[1]), None, &no_redact).unwrap();
    assert_eq!(
        (plan.mode, &plan.secret),
        (Mode::Legacy, &SecretPlan::NotUsed)
    );
    let cfg = step(&plan, "proxy.config");
    let diff = cfg.diff.as_deref().unwrap();
    assert!(
        diff.contains("203.0.113.10:23628") && diff.contains("+ip_forward: true"),
        "{diff}"
    );
    assert_eq!(step(&plan, "1.spigot").status, Status::Change);
    // proxies.bungee-cord.online-mode déjà à true : rien à écrire dans paper-global.yml.
    assert_eq!(step(&plan, "1.paper").status, Status::Ok);
    assert_eq!(step(&plan, "1.online-mode").status, Status::Change);
    assert_eq!(step(&plan, "1.bungeeguard").status, Status::Manual);
    assert!(plan.warnings.iter().any(|w| w.contains("sans BungeeGuard")));
    // Pas de transfert moderne sur BungeeCord.
    let mut a = args(100, &[1]);
    a.mode = Some(Mode::Modern);
    assert!(build_plan(&proxy, &backends, &a, None, &no_redact)
        .unwrap_err()
        .contains("BungeeCord"));
}

#[test]
fn velocity_legacy_mode_turns_modern_forwarding_off_on_paper() {
    let text = VELOCITY_DEFAULT.replace(
        "lobby = \"127.0.0.1:30066\"",
        "lobby = \"203.0.113.10:23628\"",
    );
    let p = proxy_state(&text, Some(SECRET_PROXY));
    let f = files(Some(paper_global(true, SECRET_PROXY)), Some(spigot(false)));
    let backends = vec![paper_backend(
        1,
        "Lobby",
        "203.0.113.10",
        23628,
        f,
        Some(false),
    )];
    let mut a = args(100, &[1]);
    a.mode = Some(Mode::Legacy);
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: &text,
        },
        &backends,
        &a,
        None,
        &no_redact,
    )
    .unwrap();
    assert!(step(&plan, "proxy.config")
        .diff
        .as_deref()
        .unwrap()
        .contains("+player-info-forwarding-mode = \"LEGACY\""));
    assert!(!plan.steps.iter().any(|s| s.id == "proxy.secret"));
    match &step(&plan, "1.paper").action {
        Some(Action::Config { changes, .. }) => {
            assert!(changes.contains(&("proxies.velocity.enabled".to_string(), json!(false))))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(step(&plan, "1.spigot").status, Status::Change);
}

#[test]
fn an_invalid_file_refuses_the_step() {
    let p = proxy_state(VELOCITY_DEFAULT, Some(SECRET_PROXY));
    let broken = files(
        Some("proxies:\n  velocity: [\n".into()),
        Some(spigot(false)),
    );
    let backends = vec![paper_backend(
        1,
        "Lobby",
        "203.0.113.10",
        23628,
        broken,
        Some(false),
    )];
    let plan = build_plan(
        &ProxyInput {
            state: &p,
            config_text: VELOCITY_DEFAULT,
        },
        &backends,
        &args(100, &[1]),
        None,
        &no_redact,
    )
    .unwrap();
    assert_eq!(step(&plan, "1.paper").status, Status::Refused);
}
