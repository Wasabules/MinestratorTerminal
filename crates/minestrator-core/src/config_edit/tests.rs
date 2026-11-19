use super::*;
use serde_json::json;

fn req(key: &str, value: Value) -> EditRequest {
    EditRequest {
        key: key.into(),
        value,
    }
}

fn no_redact(s: &str) -> String {
    s.to_string()
}

/// Édition vérifiée (options par défaut), panique sur refus.
fn edit(format: Format, text: &str, changes: &[(&str, Value)]) -> EditOutcome {
    let reqs: Vec<EditRequest> = changes.iter().map(|(k, v)| req(k, v.clone())).collect();
    edit_text(
        format,
        "/f",
        text,
        &reqs,
        EditOptions::default(),
        &no_redact,
    )
    .unwrap_or_else(|e| panic!("refus inattendu : {e}"))
}

fn refused(format: Format, text: &str, changes: &[(&str, Value)], opts: EditOptions) -> String {
    let reqs: Vec<EditRequest> = changes.iter().map(|(k, v)| req(k, v.clone())).collect();
    match edit_text(format, "/f", text, &reqs, opts, &no_redact) {
        Ok(o) => panic!("aurait dû être refusé ; résultat :\n{}", o.new_text),
        Err(e) => e,
    }
}

/// Lignes ajoutées et retirées entre deux textes (diff naïf ligne à ligne, ordre ignoré).
fn changed_lines(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();
    let removed = la
        .iter()
        .filter(|l| !lb.contains(l))
        .map(|l| l.to_string())
        .collect();
    let added = lb
        .iter()
        .filter(|l| !la.contains(l))
        .map(|l| l.to_string())
        .collect();
    (removed, added)
}

// --- Chemins, secrets, types ---------------------------------------------------------------------

#[test]
fn key_paths_cover_indexes_quotes_and_escapes() {
    use Seg::*;
    let k = |s: &str| Key(s.to_string());
    assert_eq!(
        parse_key_path("a.b.c").unwrap(),
        vec![k("a"), k("b"), k("c")]
    );
    assert_eq!(
        parse_key_path("a.list[0].name").unwrap(),
        vec![k("a"), k("list"), Index(0), k("name")]
    );
    assert_eq!(
        parse_key_path("forced-hosts.\"lobby.example.com\"").unwrap(),
        vec![k("forced-hosts"), k("lobby.example.com")]
    );
    assert_eq!(
        parse_key_path("a['b.c'].d").unwrap(),
        vec![k("a"), k("b.c"), k("d")]
    );
    assert_eq!(
        parse_key_path("permissions.essentials\\.home").unwrap(),
        vec![k("permissions"), k("essentials.home")]
    );
    assert_eq!(
        parse_key_path("m[1][2]").unwrap(),
        vec![k("m"), Index(1), Index(2)]
    );
    for bad in ["", "a..b", ".a", "a.", "a[x]", "a[0", "\"a", "[0]"] {
        assert!(
            parse_key_path(bad).is_err(),
            "{bad:?} aurait dû être refusé"
        );
    }
    // Aller-retour.
    for p in ["a.b", "forced-hosts.\"lobby.example.com\"", "a.list[3].x"] {
        assert_eq!(render_key_path(&parse_key_path(p).unwrap()), p);
    }
}

#[test]
fn secret_keys_are_detected_in_plugin_configs() {
    for k in [
        "password",
        "rcon.password",
        "db-password",
        "secret",
        "forwarding-secret",
        "bot-token",
        "apiKey",
        "api-key",
        "license_key",
        "key",
        "server-key",
        "mysql.pass",
        "pwd",
        "webhook-url",
        "client_secret",
        "private-key",
        "Authorization",
    ] {
        assert!(is_secret_key(k), "{k} devrait être secret");
    }
    for k in [
        "keep-inventory",
        "keepInventory",
        "monkey",
        "max-players",
        "storage-method",
        "address",
        "bypass",
        "keyboard",
        "view-distance",
    ] {
        assert!(!is_secret_key(k), "{k} ne devrait pas être secret");
    }
}

#[test]
fn values_are_masked_but_not_booleans_or_empty_strings() {
    let v = json!({ "data": { "password": "hunter2", "port": 3306, "use-ssl": true },
                     "discord": { "token": "abc", "enabled": false }, "tokens": ["a", "b"], "rcon-password": "" });
    let (m, n) = mask_value(&v, false);
    assert_eq!(m["data"]["password"], "[SECRET]");
    assert_eq!(m["data"]["port"], 3306);
    assert_eq!(m["discord"]["token"], "[SECRET]");
    assert_eq!(m["discord"]["enabled"], false);
    assert_eq!(m["tokens"], json!(["[SECRET]", "[SECRET]"]));
    assert_eq!(m["rcon-password"], "");
    assert_eq!(n, 4);
}

#[test]
fn yaml_scalars_resolve_like_snakeyaml() {
    use yaml::resolve_plain as r;
    assert_eq!(r("true"), json!(true));
    assert_eq!(r("yes"), json!(true));
    assert_eq!(r("Off"), json!(false));
    assert_eq!(r("~"), Value::Null);
    assert_eq!(r("12"), json!(12));
    assert_eq!(r("-3"), json!(-3));
    assert_eq!(r("0x1F"), json!(31));
    assert_eq!(r("1.5"), json!(1.5));
    assert_eq!(r("1e3"), json!(1000.0));
    assert_eq!(r("1.20"), json!(1.2));
    assert_eq!(r("default"), json!("default"));
    assert_eq!(r("1.2.3"), json!("1.2.3"));
    assert_eq!(r(".inf"), json!(".inf"));
}

#[test]
fn formats_are_detected_by_extension() {
    assert_eq!(
        Format::detect("/plugins/LuckPerms/config.yml"),
        Some(Format::Yaml)
    );
    assert_eq!(Format::detect("/velocity.TOML"), Some(Format::Toml));
    assert_eq!(
        Format::detect("/config/sodium-options.json"),
        Some(Format::Json)
    );
    assert_eq!(Format::detect("/config/x.json5"), Some(Format::Json));
    assert_eq!(
        Format::detect("/server.properties"),
        Some(Format::Properties)
    );
    assert_eq!(Format::detect("/eula.txt"), None);
    assert_eq!(Format::detect("/Makefile"), None);
}

// --- server.properties ----------------------------------------------------------------------------

const SERVER_PROPERTIES: &str = "#Minecraft server properties\n#Mon Nov 18 10:00:00 CET 2026\nallow-flight=false\nenable-rcon=true\ngenerator-settings={}\nlevel-name=world\nmax-players=20\nmotd=A Minecraft Server\nrcon.password=hunter2\nrcon.port=25575\nresource-pack=\nserver-port=25565\nview-distance=10\n";

#[test]
fn server_properties_edits_values_in_place() {
    let o = edit(
        Format::Properties,
        SERVER_PROPERTIES,
        &[
            ("max-players", json!(50)),
            ("motd", json!("Bienvenue · été")),
        ],
    );
    let (removed, added) = changed_lines(SERVER_PROPERTIES, &o.new_text);
    assert_eq!(removed, vec!["max-players=20", "motd=A Minecraft Server"]);
    assert_eq!(
        added,
        vec!["max-players=50", "motd=Bienvenue \\u00B7 \\u00E9t\\u00E9"]
    );
    // Relu : la valeur est bien celle demandée (texte).
    let t = Format::Properties.tree(&o.new_text).unwrap();
    assert_eq!(t["max-players"], "50");
    assert_eq!(t["motd"], "Bienvenue · été");
    assert!(o.new_text.starts_with("#Minecraft server properties\n"));
}

#[test]
fn server_properties_adds_missing_keys_and_masks_secrets() {
    let o = edit(
        Format::Properties,
        SERVER_PROPERTIES,
        &[
            ("rcon.password", json!("n0uveau")),
            ("hide-online-players", json!(true)),
        ],
    );
    assert!(o
        .new_text
        .ends_with("view-distance=10\nhide-online-players=true\n"));
    assert!(o.new_text.contains("rcon.password=n0uveau"));
    // Diff : secret masqué des deux côtés, l'ajout visible.
    assert!(
        !o.diff.contains("hunter2") && !o.diff.contains("n0uveau"),
        "{}",
        o.diff
    );
    assert!(o.diff.contains("+hide-online-players=true"));
    let c = &o.changes[0];
    assert_eq!(
        (c.action, c.old.clone(), c.new.clone()),
        ("modifiée", Some(json!("[SECRET]")), json!("[SECRET]"))
    );
    // Lecture : masquée elle aussi.
    let got = render_get(
        "/server.properties",
        SERVER_PROPERTIES,
        Some("rcon.password"),
    )
    .unwrap();
    assert_eq!(got["value"], "[SECRET]");
    let all = render_get("/server.properties", SERVER_PROPERTIES, None).unwrap();
    assert!(!all.to_string().contains("hunter2"));
    assert_eq!(all["secrets_masked"], 1);
}

#[test]
fn server_properties_keeps_crlf_and_escapes() {
    let text = "a=1\r\ngenerator-settings={\"x\"\\:1}\r\nb=2\r\n";
    let t = Format::Properties.tree(text).unwrap();
    assert_eq!(t["generator-settings"], "{\"x\":1}");
    let o = edit(
        Format::Properties,
        text,
        &[("b", json!("3")), ("c", json!("new"))],
    );
    assert_eq!(
        o.new_text,
        "a=1\r\ngenerator-settings={\"x\"\\:1}\r\nb=3\r\nc=new\r\n"
    );
}

#[test]
fn properties_continuation_lines_are_replaced_whole() {
    let text = "key = first \\\n    second\nother=x\n";
    assert_eq!(
        Format::Properties.tree(text).unwrap()["key"],
        "first second"
    );
    let o = edit(Format::Properties, text, &[("key", json!("single"))]);
    assert_eq!(o.new_text, "key = single\nother=x\n");
}

// --- bukkit.yml / spigot.yml ----------------------------------------------------------------------

const BUKKIT_YML: &str = "# This is the main configuration file for Bukkit.\n# As you can see, there's actually not that much to configure without any plugins.\n\nsettings:\n  allow-end: true\n  warn-on-overload: true\n  permissions-file: permissions.yml\n  update-folder: update\n  plugin-profiling: false\n  connection-throttle: 4000\n  query-plugins: true\n  deprecated-verbose: default\n  shutdown-message: Server closed\n  minimum-api: none\n  use-map-color-cache: true\nspawn-limits:\n  monsters: 70\n  animals: 10\n  water-animals: 5\n  water-ambient: 20\n  ambient: 15\nchunk-gc:\n  period-in-ticks: 600\nticks-per:\n  animal-spawns: 400\n  monster-spawns: 1\n  autosave: 6000\naliases: now-in-commands.yml\n";

#[test]
fn bukkit_yml_edits_nested_scalars_without_touching_the_rest() {
    let o = edit(
        Format::Yaml,
        BUKKIT_YML,
        &[
            ("settings.allow-end", json!(false)),
            ("spawn-limits.monsters", json!(50)),
            (
                "settings.shutdown-message",
                json!("Redémarrage : à tout de suite"),
            ),
        ],
    );
    let (removed, added) = changed_lines(BUKKIT_YML, &o.new_text);
    assert_eq!(
        removed,
        vec![
            "  allow-end: true",
            "  shutdown-message: Server closed",
            "  monsters: 70"
        ]
    );
    // `: ` dans le texte : guillemets obligatoires.
    assert_eq!(
        added,
        vec![
            "  allow-end: false",
            "  shutdown-message: 'Redémarrage : à tout de suite'",
            "  monsters: 50"
        ]
    );
    assert_eq!(o.new_text.lines().count(), BUKKIT_YML.lines().count());
    assert!(
        o.diff.contains("-  monsters: 70") && o.diff.contains("+  monsters: 50"),
        "{}",
        o.diff
    );
}

#[test]
fn bukkit_yml_inserts_missing_keys_at_the_right_level() {
    let o = edit(
        Format::Yaml,
        BUKKIT_YML,
        &[
            ("spawn-limits.axolotls", json!(5)),
            ("chunk-gc.load-threshold", json!(0)),
            ("new-section.a.b", json!("x")),
        ],
    );
    assert!(
        o.new_text
            .contains("  ambient: 15\n  axolotls: 5\nchunk-gc:\n"),
        "{}",
        o.new_text
    );
    assert!(o
        .new_text
        .contains("  period-in-ticks: 600\n  load-threshold: 0\n"));
    assert!(o
        .new_text
        .ends_with("aliases: now-in-commands.yml\nnew-section:\n  a:\n    b: x\n"));
    assert_eq!(
        o.changes.iter().map(|c| c.action).collect::<Vec<_>>(),
        vec!["créée", "créée", "créée"]
    );
}

#[test]
fn create_missing_false_refuses_unknown_keys() {
    let opts = EditOptions {
        create_missing: false,
        ..Default::default()
    };
    let e = refused(
        Format::Yaml,
        BUKKIT_YML,
        &[("settings.allow-nether", json!(true))],
        opts,
    );
    assert!(e.contains("absente"), "{e}");
}

#[test]
fn keep_type_converts_text_to_the_existing_number_or_boolean() {
    let o = edit(
        Format::Yaml,
        BUKKIT_YML,
        &[
            ("spawn-limits.monsters", json!("45")),
            ("settings.allow-end", json!("false")),
        ],
    );
    assert!(o.new_text.contains("  monsters: 45\n") && o.new_text.contains("  allow-end: false\n"));
    assert_eq!(
        o.changes[0].note.as_deref(),
        Some("type conservé : nombre entier")
    );
    // Sans keep_type : texte entre guillemets (sinon il serait relu comme un nombre).
    let reqs = vec![req("spawn-limits.monsters", json!("45"))];
    let opts = EditOptions {
        keep_type: false,
        ..Default::default()
    };
    let o = edit_text(Format::Yaml, "/b", BUKKIT_YML, &reqs, opts, &no_redact).unwrap();
    assert!(o.new_text.contains("  monsters: '45'\n"));
}

#[test]
fn spigot_view_distance_default_becomes_a_number() {
    let text = "config-version: 12\nworld-settings:\n  default:\n    view-distance: default\n    simulation-distance: default\n    mob-spawn-range: 8\n    verbose: false\nsettings:\n  bungeecord: false\n  restart-script: ./start.sh\n";
    let o = edit(
        Format::Yaml,
        text,
        &[
            ("world-settings.default.view-distance", json!(8)),
            ("settings.bungeecord", json!(true)),
        ],
    );
    assert!(
        o.new_text.contains("    view-distance: 8\n")
            && o.new_text.contains("  bungeecord: true\n")
    );
    assert_eq!(
        Format::Yaml.tree(&o.new_text).unwrap()["world-settings"]["default"]["view-distance"],
        8
    );
    assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
}

#[test]
fn yaml_strings_that_look_typed_are_quoted() {
    let text = "a: x\n";
    let o = edit(
        Format::Yaml,
        text,
        &[
            ("a", json!("true")),
            ("b", json!("1.20")),
            ("c", json!("2024-01-01")),
            ("d", json!("")),
            ("e", json!("it's")),
            ("f", json!("ligne1\nligne2")),
        ],
    );
    assert_eq!(
        o.new_text,
        "a: 'true'\nb: '1.20'\nc: '2024-01-01'\nd: ''\ne: it's\nf: \"ligne1\\nligne2\"\n"
    );
    let t = Format::Yaml.tree(&o.new_text).unwrap();
    assert_eq!(
        t,
        json!({ "a": "true", "b": "1.20", "c": "2024-01-01", "d": "", "e": "it's", "f": "ligne1\nligne2" })
    );
}

// --- paper-global.yml ------------------------------------------------------------------------------

const PAPER_GLOBAL: &str = "# This is the global configuration file for Paper.\n# As you can see, there's a lot to configure. Some options may impact gameplay, so use\n# with caution, and make sure you know what each option does before configuring.\n\n_version: 31\nblock-updates:\n  disable-chorus-plant-updates: false\n  disable-noteblock-updates: false\nchunk-loading-basic:\n  player-max-chunk-generate-rate: -1.0\n  player-max-chunk-load-rate: 100.0\n  player-max-chunk-send-rate: 75.0\ncommands:\n  fix-target-selector-tag-completion: true\n  suggest-player-names-when-null-tab-completions: true\n  time-command-affects-all-worlds: false\nmessages:\n  kick:\n    authentication-servers-down: <lang:multiplayer.disconnect.authservers_down>\n    connection-throttle: Connection throttled! Please wait before reconnecting.\n    flying-player: <lang:multiplayer.disconnect.flying>\n  no-permission: <red>I'm sorry, but you do not have permission to perform this command.\n    Please contact the server administrators if you believe that this is in error.\n  use-display-name-in-quit-message: false\nproxies:\n  bungee-cord:\n    online-mode: true\n  proxy-protocol: false\n  velocity:\n    enabled: false\n    online-mode: true\n    secret: ''\nspam-limiter:\n  incoming-packet-threshold: 300\n  recipe-spam-increment: 1\n  recipe-spam-limit: 20\n  tab-spam-increment: 1\n  tab-spam-limit: 500\nunsupported-settings:\n  allow-headless-pistons: false\n  allow-permanent-block-break-exploits: false\n  compression-format: ZLIB\n  skip-tripwire-hook-placement-validation: false\n  update-equipment-on-player-actions: true\nwatchdog:\n  early-warning-delay: 10000\n  early-warning-every: 5000\n";

#[test]
fn paper_global_sets_the_velocity_secret_without_leaking_it() {
    let o = edit(
        Format::Yaml,
        PAPER_GLOBAL,
        &[
            ("proxies.velocity.enabled", json!(true)),
            ("proxies.velocity.secret", json!("s3cr3t-velocity")),
        ],
    );
    assert!(o.new_text.contains("  velocity:\n    enabled: true\n    online-mode: true\n    secret: 's3cr3t-velocity'\n"), "{}", o.new_text);
    assert!(!o.diff.contains("s3cr3t"), "{}", o.diff);
    assert!(o.diff.contains("+    secret: [SECRET]"), "{}", o.diff);
    // Le commentaire d'en-tête et le texte multiligne sont intacts.
    assert!(o
        .new_text
        .starts_with("# This is the global configuration file for Paper.\n"));
    assert!(o.new_text.contains(
        "    Please contact the server administrators if you believe that this is in error.\n"
    ));
}

#[test]
fn paper_global_floats_and_negative_numbers() {
    let o = edit(
        Format::Yaml,
        PAPER_GLOBAL,
        &[
            (
                "chunk-loading-basic.player-max-chunk-send-rate",
                json!(40.0),
            ),
            (
                "chunk-loading-basic.player-max-chunk-generate-rate",
                json!("5"),
            ),
        ],
    );
    assert!(o.new_text.contains("  player-max-chunk-send-rate: 40.0\n"));
    // Texte "5" sur un flottant : gardé flottant.
    assert!(
        o.new_text
            .contains("  player-max-chunk-generate-rate: 5.0\n"),
        "{}",
        o.new_text
    );
}

#[test]
fn paper_multiline_plain_scalar_is_replaced_whole() {
    let o = edit(
        Format::Yaml,
        PAPER_GLOBAL,
        &[("messages.no-permission", json!("<red>Accès refusé."))],
    );
    assert!(
        o.new_text.contains(
            "  no-permission: <red>Accès refusé.\n  use-display-name-in-quit-message: false\n"
        ),
        "{}",
        o.new_text
    );
    assert!(!o.new_text.contains("Please contact"));
}

// --- LuckPerms / EssentialsX -------------------------------------------------------------------------

const LUCKPERMS: &str = "####################################################################################################\n# +----------------------------------------------------------------------------------------------+ #\n# |                                       LuckPerms Configuration                                | #\n# +----------------------------------------------------------------------------------------------+ #\n####################################################################################################\n\n# The name of the server, used for server specific permissions.\nserver: global\n\n# How the plugin should store data\n#\n# - Possible options:\n#   |  => Remote databases - require connection information to be configured below\n#   |     - MySQL\n#   |     - MariaDB\nstorage-method: h2\n\n# The following block defines the settings for remote database storage methods.\ndata:\n\n  # Define the address and port for the database.\n  # - The standard DB engine port is used by default\n  #   (MySQL = 3306, PostgreSQL = 5432, MongoDB = 27017)\n  # - Specify as \"host:port\" if differs\n  address: localhost\n\n  # The name of the database to store LuckPerms data in.\n  database: minecraft\n\n  # Credentials for the database.\n  username: root\n  password: ''\n\n  pool-settings:\n    maximum-pool-size: 10\n    minimum-idle: 10\n    properties:\n      useUnicode: true\n      characterEncoding: utf8\n\n# Define settings for a \"messaging service\".\nmessaging-service: auto\n\nauto-push-updates: true\n";

#[test]
fn luckperms_switch_to_mysql_keeps_every_comment() {
    let o = edit(
        Format::Yaml,
        LUCKPERMS,
        &[
            ("storage-method", json!("mysql")),
            ("data.address", json!("db.example.net:3306")),
            ("data.password", json!("Pa55:word #1")),
            ("data.pool-settings.maximum-pool-size", json!(4)),
        ],
    );
    let (removed, added) = changed_lines(LUCKPERMS, &o.new_text);
    assert_eq!(
        removed,
        vec![
            "storage-method: h2",
            "  address: localhost",
            "  password: ''",
            "    maximum-pool-size: 10"
        ]
    );
    assert_eq!(
        added,
        vec![
            "storage-method: mysql",
            "  address: db.example.net:3306",
            "  password: 'Pa55:word #1'",
            "    maximum-pool-size: 4"
        ]
    );
    let t = Format::Yaml.tree(&o.new_text).unwrap();
    assert_eq!(t["data"]["password"], "Pa55:word #1");
    assert!(!o.diff.contains("Pa55"), "{}", o.diff);
    // Lecture de la section : mot de passe masqué, le reste visible.
    let got = render_get("/plugins/LuckPerms/config.yml", &o.new_text, Some("data")).unwrap();
    assert_eq!(got["value"]["password"], "[SECRET]");
    assert_eq!(got["value"]["address"], "db.example.net:3306");
}

#[test]
fn luckperms_inserts_after_blank_lines_and_nested_sections() {
    let o = edit(
        Format::Yaml,
        LUCKPERMS,
        &[
            ("data.pool-settings.properties.useSSL", json!(false)),
            ("data.table-prefix", json!("luckperms_")),
        ],
    );
    assert!(o.new_text.contains("      characterEncoding: utf8\n      useSSL: false\n  table-prefix: luckperms_\n\n# Define settings"), "{}", o.new_text);
}

const ESSENTIALS: &str = "############################################################\n# +------------------------------------------------------+ #\n# |                       Notes                          | #\n# +------------------------------------------------------+ #\n############################################################\n\n# A color code between 0-9 or a-f. Set to '' to disable.\nops-name-color: '4'\n\n# The character(s) to prefix all nicknames, so that you know they are not true usernames.\nnickname-prefix: '~'\n\n# The maximum length allowed in nicknames.\nmax-nick-length: 15\n\n# Disabled commands will be completely unavailable on the server.\ndisabled-commands:\n#  - nick\n#  - clear\n\n# Commands that will be shown in the help menu.\nplayer-commands:\n  - afk\n  - back\n  - spawn\n\nkits:\n  tools:\n    delay: 10\n    items:\n      - stone_sword 1\n      - stone_shovel 1\n\nchat:\n  format: '<{DISPLAYNAME}> {MESSAGE}'\n";

#[test]
fn essentials_keeps_quote_styles_and_edits_lists() {
    let o = edit(
        Format::Yaml,
        ESSENTIALS,
        &[
            ("ops-name-color", json!("c")),
            ("player-commands[1]", json!("home")),
            ("player-commands[3]", json!("warp")),
            ("kits.tools.items[2]", json!("stone_pickaxe 1")),
            ("chat.format", json!("&7{DISPLAYNAME}&8: &f{MESSAGE}")),
        ],
    );
    let (removed, added) = changed_lines(ESSENTIALS, &o.new_text);
    assert_eq!(
        removed,
        vec![
            "ops-name-color: '4'",
            "  - back",
            "  format: '<{DISPLAYNAME}> {MESSAGE}'"
        ]
    );
    assert_eq!(
        added,
        vec![
            "ops-name-color: 'c'",
            "  - home",
            "  - warp",
            "      - stone_pickaxe 1",
            "  format: '&7{DISPLAYNAME}&8: &f{MESSAGE}'"
        ]
    );
    assert_eq!(
        Format::Yaml.tree(&o.new_text).unwrap()["player-commands"],
        json!(["afk", "home", "spawn", "warp"])
    );
}

#[test]
fn essentials_fills_an_empty_section_and_replaces_whole_lists() {
    let o = edit(
        Format::Yaml,
        ESSENTIALS,
        &[
            ("disabled-commands", json!(["nick", "fly"])),
            ("player-commands", json!([])),
        ],
    );
    assert!(
        o.new_text
            .contains("disabled-commands:\n  - nick\n  - fly\n#  - nick\n"),
        "{}",
        o.new_text
    );
    assert!(
        o.new_text.contains("player-commands: []\n\nkits:"),
        "{}",
        o.new_text
    );
    let t = Format::Yaml.tree(&o.new_text).unwrap();
    assert_eq!(t["disabled-commands"], json!(["nick", "fly"]));
    assert_eq!(t["player-commands"], json!([]));
}

#[test]
fn yaml_replaces_a_section_with_an_object() {
    let o = edit(
        Format::Yaml,
        ESSENTIALS,
        &[(
            "kits.tools",
            json!({ "delay": 60, "items": ["wooden_axe 1"] }),
        )],
    );
    assert!(
        o.new_text
            .contains("kits:\n  tools:\n    delay: 60\n    items:\n      - wooden_axe 1\n\nchat:"),
        "{}",
        o.new_text
    );
}

#[test]
fn yaml_sequences_at_key_level_keep_their_style() {
    let text = "worlds:\n- world\n- world_nether\nother: 1\n";
    let o = edit(
        Format::Yaml,
        text,
        &[
            ("worlds[2]", json!("world_the_end")),
            ("extra", json!(["a"])),
        ],
    );
    assert_eq!(
        o.new_text,
        "worlds:\n- world\n- world_nether\n- world_the_end\nother: 1\nextra:\n- a\n"
    );
}

#[test]
fn yaml_flow_collections_stay_in_line() {
    let text = "depend: [Vault, ProtocolLib] # dépendances\nmeta: {a: 1}\n";
    let o = edit(
        Format::Yaml,
        text,
        &[
            ("depend[2]", json!("LuckPerms")),
            ("meta.b", json!("x y")),
            ("meta.a", json!(2)),
        ],
    );
    assert_eq!(
        o.new_text,
        "depend: [Vault, ProtocolLib, LuckPerms] # dépendances\nmeta: {a: 2, b: x y}\n"
    );
}

#[test]
fn yaml_block_scalars_are_replaced_and_followed_correctly() {
    let text = "motd: |\n  Ligne 1\n  Ligne 2\n# commentaire de la suite\nnext: 1\n";
    let o = edit(Format::Yaml, text, &[("motd", json!("Une ligne"))]);
    assert_eq!(
        o.new_text,
        "motd: Une ligne\n# commentaire de la suite\nnext: 1\n"
    );
    let o = edit(
        Format::Yaml,
        text,
        &[("next", json!(2)), ("after", json!(true))],
    );
    assert_eq!(
        o.new_text,
        "motd: |\n  Ligne 1\n  Ligne 2\n# commentaire de la suite\nnext: 2\nafter: true\n"
    );
}

#[test]
fn yaml_null_values_become_sections() {
    let text = "a:\nb: ~\n";
    let o = edit(
        Format::Yaml,
        text,
        &[("a.x", json!(1)), ("b.y", json!("z"))],
    );
    assert_eq!(o.new_text, "a:\n  x: 1\nb:\n  y: z\n");
}

#[test]
fn yaml_empty_or_comment_only_files_get_the_key() {
    assert_eq!(
        edit(Format::Yaml, "", &[("a.b", json!(1))]).new_text,
        "a:\n  b: 1\n"
    );
    assert_eq!(
        edit(Format::Yaml, "# rien\n", &[("a", json!("x"))]).new_text,
        "# rien\na: x\n"
    );
}

#[test]
fn yaml_detects_four_space_indentation() {
    let text = "root:\n    child: 1\n    list:\n        - a\n";
    let o = edit(
        Format::Yaml,
        text,
        &[("root.new.deep", json!(2)), ("root.list[1]", json!("b"))],
    );
    assert_eq!(
        o.new_text,
        "root:\n    child: 1\n    list:\n        - a\n        - b\n    new:\n        deep: 2\n"
    );
}

#[test]
fn yaml_keys_with_dots_and_crlf() {
    let text = "permissions:\r\n  essentials.home: true\r\n  essentials.fly: false\r\n";
    let o = edit(
        Format::Yaml,
        text,
        &[
            ("permissions.\"essentials.fly\"", json!(true)),
            ("permissions.'essentials.warp'", json!(true)),
        ],
    );
    assert_eq!(o.new_text, "permissions:\r\n  essentials.home: true\r\n  essentials.fly: true\r\n  essentials.warp: true\r\n");
}

#[test]
fn yaml_refuses_duplicates_anchors_and_invalid_files() {
    let dup = "a: 1\na: 2\n";
    assert!(refused(
        Format::Yaml,
        dup,
        &[("a", json!(3))],
        EditOptions::default()
    )
    .contains("double"));
    let anchors = "base: &b\n  x: 1\nother:\n  <<: *b\n";
    assert!(refused(
        Format::Yaml,
        anchors,
        &[("base.x", json!(2))],
        EditOptions::default()
    )
    .contains("ancr"));
    let broken = "a:\n  b: 1\n c: 2\n";
    let e = refused(
        Format::Yaml,
        broken,
        &[("a.b", json!(3))],
        EditOptions::default(),
    );
    assert!(
        e.contains("pas lisible") && e.contains("rien n'a été modifié"),
        "{e}"
    );
    // Lecture tolérante d'un YAML invalide : avertissement, pas d'échec.
    let got = render_get("/x.yml", broken, Some("a.b")).unwrap();
    assert_eq!(got["value"], "1");
    assert!(got["warnings"].is_array());
}

#[test]
fn mask_markers_are_refused_or_skipped() {
    let e = refused(
        Format::Yaml,
        LUCKPERMS,
        &[("data.password", json!("[SECRET]"))],
        EditOptions::default(),
    );
    assert!(e.contains("[SECRET]"), "{e}");
    let e = refused(
        Format::Yaml,
        LUCKPERMS,
        &[("data.address", json!("[IP]:3306"))],
        EditOptions::default(),
    );
    assert!(e.contains("[IP]"), "{e}");
    // Manifeste : une valeur masquée veut dire « ne pas toucher ».
    let opts = EditOptions {
        skip_masked: true,
        ..Default::default()
    };
    let reqs = vec![
        req("data.password", json!("[SECRET]")),
        req("server", json!("lobby")),
    ];
    let o = edit_text(Format::Yaml, "/l", LUCKPERMS, &reqs, opts, &no_redact).unwrap();
    assert_eq!(o.changes[0].action, "ignorée");
    assert!(o.new_text.contains("  password: ''\n") && o.new_text.contains("server: lobby\n"));
}

#[test]
fn unchanged_values_write_nothing() {
    let o = edit(
        Format::Yaml,
        BUKKIT_YML,
        &[
            ("spawn-limits.monsters", json!(70)),
            ("settings.allow-end", json!(true)),
        ],
    );
    assert!(!o.changed && o.diff.is_empty());
    assert!(o.changes.iter().all(|c| c.action == "inchangée"));
}

#[test]
fn yaml_bom_is_preserved() {
    let text = "\u{feff}a: 1\n";
    let o = edit(Format::Yaml, text, &[("a", json!(2))]);
    assert_eq!(o.new_text, "\u{feff}a: 2\n");
}

// --- velocity.toml -------------------------------------------------------------------------------------

const VELOCITY: &str = "# Config version. Do not change this\nconfig-version = \"2.7\"\n\n# What port should the proxy be bound to? By default, we'll bind to all addresses on port 25565.\nbind = \"0.0.0.0:25565\"\n\n# What should be the MOTD? This gets displayed when the player adds your server to\n# their server list. Only MiniMessage format is accepted.\nmotd = \"<#09add3>A Velocity Server\"\n\n# What should we display for the maximum number of players?\nshow-max-players = 500\n\n# Should we authenticate players with Mojang? By default, this is on.\nonline-mode = true\n\n# Should the proxy enforce the new public key security standard? By default, this is on.\nforce-key-authentication = true\n\n# - \"none\":   No forwarding will be done.\n# - \"modern\": Forward player IPs and UUIDs as part of the login process using\n#             Velocity's native forwarding.\nplayer-info-forwarding-mode = \"NONE\"\n\n# If you are using modern or BungeeGuard IP forwarding, configure a file that contains a unique secret here.\nforwarding-secret-file = \"forwarding.secret\"\n\n[servers]\n# Configure your servers here. Each key represents the server's name, and the value\n# represents the IP address of the server to connect to.\nlobby = \"127.0.0.1:30066\"\nfactions = \"127.0.0.1:30067\"\n\n# In what order we should try servers when a player logs in or is kicked from a server.\ntry = [\n    \"lobby\"\n]\n\n[forced-hosts]\n# Configure your forced hosts here.\n\"lobby.example.com\" = [\n    \"lobby\"\n]\n\n[advanced]\n# How large a Minecraft packet has to be before we compress it. Setting this to zero will\n# compress all packets, and setting it to -1 will disable compression entirely.\ncompression-threshold = 256 # octets\n\n[query]\nenabled = false\nport = 25565\n";

#[test]
fn velocity_toml_edits_in_place_with_comments() {
    let o = edit(
        Format::Toml,
        VELOCITY,
        &[
            ("player-info-forwarding-mode", json!("modern")),
            ("servers.survival", json!("127.0.0.1:30068")),
            ("servers.try[1]", json!("survival")),
            ("forced-hosts.\"lobby.example.com\"[1]", json!("factions")),
            ("advanced.compression-threshold", json!(512)),
            ("query.enabled", json!("true")),
        ],
    );
    assert!(
        o.new_text
            .contains("player-info-forwarding-mode = \"modern\"\n"),
        "{}",
        o.new_text
    );
    assert!(
        o.new_text
            .contains("compression-threshold = 512 # octets\n"),
        "{}",
        o.new_text
    );
    assert!(o
        .new_text
        .contains("# Config version. Do not change this\n"));
    assert!(o
        .new_text
        .contains("# How large a Minecraft packet has to be before we compress it."));
    let t = Format::Toml.tree(&o.new_text).unwrap();
    assert_eq!(t["servers"]["survival"], "127.0.0.1:30068");
    assert_eq!(t["servers"]["try"], json!(["lobby", "survival"]));
    assert_eq!(
        t["forced-hosts"]["lobby.example.com"],
        json!(["lobby", "factions"])
    );
    assert_eq!(t["query"]["enabled"], true);
    assert_eq!(t["show-max-players"], 500);
}

#[test]
fn toml_creates_tables_and_refuses_null() {
    let o = edit(
        Format::Toml,
        VELOCITY,
        &[
            ("metrics.enabled", json!(false)),
            ("advanced.haproxy-protocol", json!(false)),
        ],
    );
    let t = Format::Toml.tree(&o.new_text).unwrap();
    assert_eq!(t["metrics"]["enabled"], false);
    assert_eq!(t["advanced"]["haproxy-protocol"], false);
    let e = refused(
        Format::Toml,
        VELOCITY,
        &[("bind", Value::Null)],
        EditOptions::default(),
    );
    assert!(e.contains("nulle"), "{e}");
}

#[test]
fn toml_secrets_are_masked_in_diffs() {
    let text = "[discord]\nbot-token = \"abc.def\"\nchannel = 123\n";
    let o = edit(
        Format::Toml,
        text,
        &[
            ("discord.bot-token", json!("xyz.123")),
            ("discord.channel", json!(456)),
        ],
    );
    assert!(
        !o.diff.contains("abc.def") && !o.diff.contains("xyz.123"),
        "{}",
        o.diff
    );
    assert!(o.diff.contains("+channel = 456"), "{}", o.diff);
    let got = render_get("/c.toml", text, None).unwrap();
    assert!(!got.to_string().contains("abc.def"));
}

#[test]
fn toml_datetimes_stay_datetimes() {
    let text = "reset = 2026-01-01T00:00:00Z\n";
    let o = edit(
        Format::Toml,
        text,
        &[("reset", json!("2026-06-01T12:00:00Z"))],
    );
    assert_eq!(o.new_text, "reset = 2026-06-01T12:00:00Z\n");
}

// --- JSON de mod ----------------------------------------------------------------------------------------

const MOD_JSON: &str = "{\n  // Options du mod (JSONC)\n  \"general\": {\n    \"enabled\": true,\n    \"radius\": 64,\n    \"blacklist\": [\"minecraft:bedrock\", \"minecraft:barrier\"]\n  },\n  \"webhook\": \"https://discord.com/api/webhooks/1/abc\",\n  \"database\": {\n    \"user\": \"mod\",\n    \"password\": \"pw\",\n  }\n}\n";

#[test]
fn mod_json_is_edited_in_place_with_comments_and_trailing_commas() {
    let o = edit(
        Format::Json,
        MOD_JSON,
        &[
            ("general.radius", json!(32)),
            ("general.blacklist[2]", json!("minecraft:spawner")),
            ("general.mode", json!("fast")),
            ("database.port", json!(3306)),
        ],
    );
    assert!(o.new_text.contains("  // Options du mod (JSONC)\n"));
    assert!(o.new_text.contains("    \"radius\": 32,\n"));
    assert!(
        o.new_text
            .contains("[\"minecraft:bedrock\", \"minecraft:barrier\", \"minecraft:spawner\"]"),
        "{}",
        o.new_text
    );
    assert!(
        o.new_text
            .contains("\"minecraft:spawner\"],\n    \"mode\": \"fast\"\n  },"),
        "{}",
        o.new_text
    );
    // Virgule finale gardée.
    assert!(
        o.new_text
            .contains("    \"password\": \"pw\",\n    \"port\": 3306,\n  }"),
        "{}",
        o.new_text
    );
    assert!(
        !o.diff.contains("\"pw\"") && !o.diff.contains("api/webhooks"),
        "{}",
        o.diff
    );
}

#[test]
fn json_objects_are_rendered_with_the_file_indentation() {
    let text = "{\n\t\"a\": 1\n}\n";
    let o = edit(
        Format::Json,
        text,
        &[("b", json!({ "c": [1, 2], "d": {} }))],
    );
    assert_eq!(o.new_text, "{\n\t\"a\": 1,\n\t\"b\": {\n\t\t\"c\": [\n\t\t\t1,\n\t\t\t2\n\t\t],\n\t\t\"d\": {}\n\t}\n}\n");
    assert_eq!(
        edit(Format::Json, "{}", &[("x", json!(1))]).new_text,
        "{\n  \"x\": 1\n}"
    );
}

#[test]
fn json_refuses_broken_files() {
    let e = refused(
        Format::Json,
        "{\"a\": 1,, }",
        &[("a", json!(2))],
        EditOptions::default(),
    );
    assert!(e.contains("pas lisible"), "{e}");
}

// --- Lecture --------------------------------------------------------------------------------------------

#[test]
fn outline_lists_leaves_and_suggests_keys() {
    let got = render_get("/plugins/LuckPerms/config.yml", LUCKPERMS, None).unwrap();
    let keys: Vec<&str> = got["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"data.pool-settings.properties.useUnicode"));
    assert!(keys.contains(&"storage-method"));
    assert_eq!(
        got["secrets_masked"], 0,
        "mot de passe vide : rien à masquer"
    );
    let missing = render_get(
        "/plugins/LuckPerms/config.yml",
        LUCKPERMS,
        Some("data.adress"),
    )
    .unwrap();
    assert_eq!(missing["found"], false);
    assert_eq!(missing["deepest_existing"], "data");
    assert!(missing["available_keys"]
        .as_array()
        .unwrap()
        .contains(&json!("address")));
    assert!(render_get("/eula.txt", "eula=true", None).is_err());
}

#[test]
fn ai_redaction_keeps_the_json_valid() {
    // Relevé réel : le filtre appliqué au JSON sérialisé donnait `"secret": [SECRET]` (invalide).
    let v = json!({ "management-server-secret": "", "rcon.password": "[SECRET]", "server-ip": "1.2.3.4", "motd": "écris à admin@example.com" });
    let text = serde_json::to_string_pretty(&redact_value(v)).unwrap();
    let back: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back["management-server-secret"], "");
    assert_eq!(back["rcon.password"], "[SECRET]");
    assert_eq!(back["server-ip"], "[IP]");
    assert!(!back["motd"].as_str().unwrap().contains("admin@"));
}

#[test]
fn parse_changes_accepts_list_or_object() {
    let l = parse_changes(Some(
        &json!([{ "key": "a.b", "value": 1 }, { "key": "c", "value": null }]),
    ))
    .unwrap();
    assert_eq!(l.len(), 2);
    assert_eq!(l[1].value, Value::Null);
    let o = parse_changes(Some(&json!({ "a": true }))).unwrap();
    assert_eq!(o[0].key, "a");
    assert!(parse_changes(Some(&json!([{ "key": "a" }]))).is_err());
    assert!(parse_changes(None).is_err());
}
