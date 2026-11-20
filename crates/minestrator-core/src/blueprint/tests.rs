//! Tests du blueprint : format, modèles, export, puis plan et application contre un faux compte
//! qui imite les outils élémentaires (création, manifeste, liaison, préréglages, alimentation).

use super::apply::{apply_with, ApplyOptions};
use super::export::{build_export, build_template, template_names, ExportInput, Exported};
use super::plan::*;
use super::*;
use crate::local_sync::testing::{block, TempDir};
use crate::mcp_ctx::Confirmation;
use crate::mybox::{MyBoxCapacity, MyBoxServerSlot, MyBoxSlots, ResourceUsage};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

// --- Format ----------------------------------------------------------------------------------------

fn abs(dir: &str) -> String {
    std::env::temp_dir().join(dir).to_string_lossy().into_owned()
}

fn full() -> Value {
    json!({
        "version": 1,
        "name": "Réseau test",
        "mybox_id": 10,
        "proxy": {
            "name": "Proxy", "platform": "Velocity", "cpu": 1, "ram": 1, "mode": "modern",
            "try": ["lobby"], "forced_hosts": { "Play.Exemple.fr": "Lobby", "pvp.exemple.fr": ["Survie", "lobby"] }, "prune": true,
            "bedrock_port": 40001
        },
        "servers": [
            { "name": "Survie", "role": "survival", "platform": "paper", "version": "1.21.4", "cpu": 2, "ram": 4,
              "manifest": { "properties": { "motd": "Survie", "rcon.password": "{{secret:rcon}}" }, "content": [{ "kind": "plugin", "source": "modrinth", "project": "essentialsx" }] },
              "sync": [{ "local_dir": abs("bp-sync"), "remote_dir": "/plugins/Essentials", "exclude": ["*.log"] }],
              "world": { "local_path": abs("bp-monde"), "world_name": "monde" },
              "pregenerate": { "radius": 2000, "shape": "circle" } },
            { "name": "Lobby", "role": "lobby", "entry": "lobby", "platform": "paper", "cpu": 1, "ram": 2 }
        ],
        "presets": ["geyser", "luckperms"],
        "database": { "database_id": 5 }
    })
}

#[test]
fn a_complete_blueprint_is_validated_and_normalized() {
    let nw = Network::parse(&full()).unwrap();
    assert_eq!(nw.nodes.len(), 3);
    assert!(nw.nodes[0].proxy && nw.nodes[0].platform.as_deref() == Some("velocity"));
    assert_eq!(nw.proxy_index(), Some(0));
    // Le lobby passe en tête des serveurs de jeu (premier de la liste de repli par défaut).
    assert_eq!(nw.backends(), vec![2, 1]);
    let p = nw.proxy.as_ref().unwrap();
    assert_eq!(p.try_order, vec![2]);
    assert_eq!(p.forced_hosts[0], ("play.exemple.fr".to_string(), vec![2]));
    assert_eq!(p.forced_hosts[1].1, vec![1, 2]);
    // Ordre fixe des préréglages ; manifeste version 1 inséré.
    assert_eq!(nw.presets, vec!["luckperms", "geyser"]);
    let m = nw.nodes[1].manifest.as_ref().unwrap();
    assert_eq!(m.version, 1);
    assert_eq!(nw.nodes[1].syncs.len(), 1);
    // Texte JSON accepté aussi.
    assert!(Network::parse(&json!(full().to_string())).is_ok());
}

#[test]
fn unknown_fields_and_bad_values_are_refused() {
    let bad = |edit: &dyn Fn(&mut Value), needle: &str| {
        let mut v = full();
        edit(&mut v);
        let e = Network::parse(&v).unwrap_err();
        assert!(e.contains(needle), "« {needle} » attendu dans : {e}");
    };
    bad(&|v| v["extra"] = json!(1), "extra");
    bad(&|v| v["servers"][0]["worldd"] = json!({}), "worldd");
    bad(&|v| v["proxy"]["bogus"] = json!(true), "bogus");
    bad(&|v| v["servers"][0]["world"]["foo"] = json!(1), "foo");
    bad(&|v| v["version"] = json!(2), "version 2");
    bad(&|v| v["servers"] = json!([]), "au moins un serveur");
    bad(&|v| v["servers"][0]["manifest"]["server"] = json!({ "name": "x" }), "section `server`");
    bad(&|v| v["servers"][0]["manifest"]["version"] = json!(3), "seule la version");
    bad(&|v| v["servers"][0]["manifest"]["content"] = json!([{ "kind": "skin" }]), "skin");
    bad(&|v| v["proxy"]["platform"] = json!("paper"), "pas une plateforme de proxy");
    bad(&|v| v["servers"][1]["platform"] = json!("velocity"), "est un proxy");
    bad(&|v| v["servers"][1]["platform"] = json!("quilt"), "Quilt");
    bad(&|v| v["servers"][1]["entry"] = json!("lob by"), "servers[1].entry");
    bad(&|v| v["servers"][1]["name"] = json!("survie"), "deux fois");
    bad(&|v| { v["servers"][0]["server_id"] = json!(7); v["servers"][1]["server_id"] = json!(7) }, "donné deux fois");
    bad(&|v| v["servers"][1]["server_id"] = json!(-3), "identifiant invalide");
    bad(&|v| v["proxy"]["try"] = json!(["Créatif"]), "ni le nom ni l'entrée");
    bad(&|v| v["proxy"]["forced_hosts"] = json!({ "play.exemple.fr:25565": "Lobby" }), "forced_hosts");
    bad(&|v| v["proxy"]["forced_hosts"] = json!({ "play.exemple.fr": "Proxy" }), "ni le nom ni l'entrée");
    bad(&|v| { v["proxy"]["platform"] = json!("bungeecord"); v["proxy"]["mode"] = json!("modern") }, "propre à Velocity");
    bad(&|v| v["proxy"]["mode"] = json!("fast"), "modern ou legacy");
    bad(&|v| v["servers"][0]["sync"][0]["local_dir"] = json!("relatif/configs"), "absolu");
    bad(&|v| { v["servers"][0]["sync"][0]["remote_dir"] = json!("/"); v["servers"][0]["sync"][0]["delete_extra"] = json!(true) }, "racine");
    bad(&|v| v["servers"][0]["world"]["local_path"] = json!("monde"), "absolu");
    bad(&|v| v["servers"][0]["pregenerate"]["radius"] = json!(0), "positif");
    bad(&|v| v["servers"][0]["pregenerate"]["shape"] = json!("ellipse"), "shape");
    bad(&|v| v["servers"][0]["cpu"] = json!(0), "au moins 1");
    bad(&|v| v["servers"][0]["role"] = json!("Survie !"), "rôle");
    bad(&|v| v["presets"] = json!(["essentials"]), "inconnu");
    bad(&|v| v["presets"] = json!(["geyser", "geyser"]), "deux fois");
    bad(&|v| { v["presets"] = json!(["geyser"]); }, "database");
    bad(&|v| { v["presets"] = json!(["luckperms"]); }, "bedrock_port");
    bad(&|v| { v.as_object_mut().unwrap().remove("proxy"); }, "demandent un proxy");
    bad(&|v| v["mybox_id"] = json!(0), "mybox_id");
}

#[test]
fn target_ids_list_the_existing_servers_of_a_blueprint() {
    assert_eq!(target_ids(&json!({ "blueprint": full() })).unwrap(), Vec::<i64>::new());
    let mut v = full();
    v["proxy"]["server_id"] = json!(9);
    v["servers"][1]["server_id"] = json!("3");
    assert_eq!(target_ids(&json!({ "blueprint": v })).unwrap(), vec![3, 9]);
    assert_eq!(target_ids(&json!({ "blueprint": "{\"servers\": [{\"server_id\": 4}]}" })).unwrap(), vec![4]);
    assert!(target_ids(&json!({ "blueprint": { "servers": [{ "server_id": "x" }] } })).is_err());
    assert!(target_ids(&json!({ "blueprint": "pas du json" })).is_err());
}

#[test]
fn the_effective_manifest_adds_the_name_and_chunky() {
    let nw = Network::parse(&full()).unwrap();
    let survie = &nw.nodes[1];
    // Serveur à créer : pas de section `server` ; Chunky ajouté (prégénération demandée).
    let m = effective_manifest(survie, false, Some("plugin")).unwrap();
    assert!(m.server.is_none());
    let (items, _) = m.content_items().unwrap();
    assert!(items.iter().any(|i| i.project.as_deref() == Some("chunky") && i.kind == "plugin"));
    // Serveur désigné par server_id : son nom est imposé (renommage par le manifeste).
    let mut node = survie.clone();
    node.server_id = Some(4);
    let m = effective_manifest(&node, true, Some("mod")).unwrap();
    assert_eq!(m.server.unwrap().name.as_deref(), Some("Survie"));
    // Chunky déjà listé : pas de doublon ; rien à appliquer : None.
    let lobby = &nw.nodes[2];
    assert!(effective_manifest(lobby, false, Some("plugin")).is_none());
    assert!(needs_start_hint("« x » absent : démarre le serveur une fois") && !needs_start_hint("conforme"));
}

#[test]
fn a_sub_tool_simple_confirmation_is_preapproved_but_not_a_typed_name() {
    let r = block(async {
        (
            crate::mcp_ctx::confirm("hors MCP", None).await,
            crate::mcp_ctx::preapproved(crate::mcp_ctx::confirm("préréglage", None)).await,
            crate::mcp_ctx::preapproved(crate::mcp_ctx::confirm("nom", Some("Survie"))).await,
        )
    });
    assert_eq!(r, (Confirmation::Unsupported, Confirmation::Accepted, Confirmation::Unsupported));
}

/// Classement des quatre outils : lectures et écriture, Copilote exclu, disque local, groupe,
/// annotations, règle d'annulation, schémas.
#[test]
fn the_network_tools_are_classified() {
    use crate::mcp::{copilot_forbidden, is_write_tool, BLUEPRINT_TOOLS, INFRA_TOOLS, LOCAL_DISK_TOOLS, READ_TOOLS, WRITE_TOOLS};
    for t in ["plan_network", "export_network", "network_template"] {
        assert!(READ_TOOLS.contains(&t) && !is_write_tool(t), "{t} : lecture");
    }
    assert!(WRITE_TOOLS.contains(&"apply_network") && INFRA_TOOLS.contains(&"apply_network"));
    for t in ["plan_network", "apply_network"] {
        assert!(LOCAL_DISK_TOOLS.contains(&t), "{t} : lit les dossiers et mondes de l'ordinateur");
    }
    for t in ["plan_network", "apply_network", "export_network", "network_template"] {
        assert!(BLUEPRINT_TOOLS.contains(&t) && copilot_forbidden(t), "{t} : jamais au Copilote");
        assert_eq!(crate::mcp_catalog::group_of(t), Some("build"), "{t}");
        assert!(crate::mcp::tool_for_game(t, "minecraft") && !crate::mcp::tool_for_game(t, "satisfactory"), "{t}");
    }
    assert_eq!(crate::agent_guard::plan_kind("apply_network"), "prévu", "outil composite : Plan::Skip");
    let a = crate::mcp_catalog::annotations("apply_network").unwrap();
    assert_eq!((a["destructiveHint"].clone(), a["idempotentHint"].clone(), a["openWorldHint"].clone()), (json!(true), json!(true), json!(true)));
    assert_eq!(crate::mcp_catalog::annotations("plan_network").unwrap()["openWorldHint"], true, "marketplaces");
    let tools = crate::mcp::tool_list().as_array().unwrap();
    let schema = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("{n} absent"))["inputSchema"].clone();
    assert_eq!(schema("plan_network")["required"], json!(["blueprint"]));
    assert_eq!(schema("apply_network")["required"], json!(["blueprint"]));
    assert_eq!(schema("apply_network")["properties"]["dry_run"]["type"], "boolean");
    assert_eq!(schema("apply_network")["properties"]["confirm_names"]["type"], "array");
    assert_eq!(schema("export_network")["required"], json!(["server_id"]));
    assert_eq!(schema("network_template")["properties"]["template"]["enum"], json!(template_names()));
}

// --- Modèles et export --------------------------------------------------------------------------------

fn capacity(cpu: i64, ram: i64) -> MyBoxCapacity {
    MyBoxCapacity {
        id: 10,
        name: "Test".into(),
        cpu: ResourceUsage { total: 16, allocated: 16 - cpu, remaining: cpu },
        ram: ResourceUsage { total: 64, allocated: 64 - ram, remaining: ram },
        slots: MyBoxSlots { max_active: 0, active: 0, max_paused: 0, paused: 0 },
        ..Default::default()
    }
}

#[test]
fn every_template_is_a_valid_blueprint_sized_on_the_free_space() {
    for name in template_names() {
        let (bp, res, notes) = build_template(name, 10, Some(&capacity(16, 64)), Some("1.21.4"), None, Some("play.exemple.fr")).unwrap();
        let nw = Network::parse(&bp).unwrap_or_else(|e| panic!("{name} : {e}"));
        assert!(res.get("warning").is_none(), "{name}");
        assert!(nw.nodes.iter().filter(|n| !n.proxy).all(|n| n.version.as_deref() == Some("1.21.4") && n.platform.as_deref() == Some("paper")));
        assert!(notes.iter().any(|n| n.contains("Modrinth")));
        if name.starts_with("proxy") {
            assert_eq!(nw.proxy.as_ref().unwrap().forced_hosts[0].0, "play.exemple.fr");
        }
    }
    // Place confortable : survie 2 cœurs / 6 Go, lobby 1 / 3, proxy 1 / 2.
    let (bp, _, _) = build_template("proxy-lobby-survie", 10, Some(&capacity(16, 64)), None, None, None).unwrap();
    assert_eq!((bp["servers"][1]["cpu"].clone(), bp["servers"][1]["ram"].clone()), (json!(2), json!(6)));
    assert_eq!((bp["proxy"]["cpu"].clone(), bp["proxy"]["ram"].clone()), (json!(1), json!(2)));
    // Place juste : minimums, puis la survie d'abord.
    let (bp, _, _) = build_template("proxy-lobby-survie", 10, Some(&capacity(3, 7)), None, None, None).unwrap();
    let ram: i64 = bp["servers"].as_array().unwrap().iter().map(|s| s["ram"].as_i64().unwrap()).sum::<i64>() + bp["proxy"]["ram"].as_i64().unwrap();
    assert_eq!(ram, 7);
    assert_eq!(bp["servers"][1]["ram"], json!(4), "survie servie avant le lobby");
    // Pas assez de place : minimums gardés, avertissement chiffré.
    let (_, res, notes) = build_template("proxy-lobby-survie", 10, Some(&capacity(0, 12)), None, None, None).unwrap();
    assert!(res["warning"].as_str().unwrap().contains("3 cœur(s) au minimum, 0 libre(s)"));
    assert!(notes.iter().any(|n| n.contains("pas assez de place")));
    assert!(build_template("inconnu", 10, None, None, None, None).is_err());
    assert!(build_template("survie-seul", 10, None, None, Some("velocity"), None).is_err());
}

#[test]
fn an_existing_network_is_exported_as_a_valid_blueprint() {
    let manifest = |loader: &str, version: &str, plugins: &[&str]| {
        json!({
            "version": 1,
            "server": { "name": "x", "loader": loader, "game_version": version },
            "properties": { "motd": "Bienvenue", "rcon.password": "[SECRET]" },
            "players": { "whitelist": ["joueur-ab12cd"] },
            "content": plugins.iter().map(|p| json!({ "kind": "plugin", "source": "modrinth", "project": p })).collect::<Vec<_>>(),
        })
    };
    let input = ExportInput {
        mybox_id: 10,
        proxy: Exported { server_id: 1, name: "Proxy".into(), manifest: manifest("velocity", "3.4.0", &["geyser", "luckperms"]), cpu: Some(1), ram: Some(2) },
        mode: Some("modern".into()),
        entries: vec![
            ("lobby".into(), Some(2), "1.2.3.4:25566".into()),
            ("survie".into(), Some(3), "1.2.3.4:25567".into()),
            ("factions".into(), None, "127.0.0.1:30068".into()),
        ],
        try_order: vec!["lobby".into()],
        forced_hosts: vec![("play.exemple.fr".into(), vec!["lobby".into(), "factions".into()])],
        servers: vec![
            Exported { server_id: 2, name: "Lobby".into(), manifest: manifest("paper", "1.21.4", &["luckperms"]), cpu: Some(1), ram: Some(3) },
            Exported { server_id: 3, name: "Survie".into(), manifest: manifest("paper", "1.21.4", &["luckperms", "coreprotect"]), cpu: Some(2), ram: Some(6) },
        ],
    };
    let (bp, notes) = build_export(&input);
    let nw = Network::parse(&bp).unwrap_or_else(|e| panic!("{e} — {notes:?}"));
    assert_eq!(nw.nodes.len(), 3);
    assert_eq!(nw.nodes[1].name, "Lobby");
    assert_eq!(nw.nodes[1].role.as_deref(), Some("lobby"));
    assert_eq!(nw.nodes[1].entry.as_deref(), Some("lobby"));
    assert_eq!(bp["proxy"]["try"], json!(["Lobby"]));
    assert_eq!(bp["proxy"]["forced_hosts"]["play.exemple.fr"], json!("Lobby"));
    assert_eq!(bp["proxy"]["platform"], "velocity");
    assert_eq!(bp["servers"][1]["version"], "1.21.4");
    // Ni `server` ni `players` dans les manifestes ; secrets restés masqués.
    assert!(bp["servers"][0]["manifest"].get("server").is_none() && bp["servers"][0]["manifest"].get("players").is_none());
    assert_eq!(bp["servers"][0]["manifest"]["properties"]["rcon.password"], "[SECRET]");
    // Préréglages repérés ; base à préciser (le blueprint tel quel le dit).
    assert_eq!(bp["presets"], json!(["luckperms", "geyser"]));
    assert!(notes.iter().any(|n| n.contains("factions")));
    assert!(!bp.to_string().contains("local_dir") && !bp.to_string().contains("local_path"));
}

// --- Faux compte ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Srv {
    id: i64,
    name: String,
    mybox: i64,
    status: String,
    java: bool,
    platform: String,
    version: String,
    state: String,
    cpu: i64,
    ram: i64,
    files: BTreeMap<String, String>,
    /// Fichiers que le prochain démarrage génère (configs des plugins installés).
    pending: Vec<String>,
    content: bool,
    manifest: bool,
    synced: bool,
    world: bool,
}

struct World {
    servers: Vec<Srv>,
    next_id: i64,
    cpu: i64,
    ram: i64,
    linked: bool,
    presets: BTreeSet<String>,
    /// Journal : `write:<outil>`, `pre:<outil>`, `confirm`, `name:<nom>`, `snapshot:<id>`,
    /// `platform:<id>`, `created:<id>`.
    events: Vec<String>,
    confirm: Confirmation,
    elicit: bool,
    forbidden: BTreeSet<i64>,
    fail_tool: Option<String>,
    /// `to_confirm` des simulations successives de network_preset (le dernier reste).
    preset_confirm: Vec<Vec<String>>,
}

struct Fake(Mutex<World>);

impl Fake {
    fn new(cpu: i64, ram: i64) -> Self {
        Fake(Mutex::new(World {
            servers: Vec::new(),
            next_id: 100,
            cpu,
            ram,
            linked: false,
            presets: BTreeSet::new(),
            events: Vec::new(),
            confirm: Confirmation::Accepted,
            elicit: false,
            forbidden: BTreeSet::new(),
            fail_tool: None,
            preset_confirm: Vec::new(),
        }))
    }

    fn w(&self) -> MutexGuard<'_, World> {
        self.0.lock().unwrap()
    }

    fn add(&self, s: Srv) {
        self.w().servers.push(s);
    }

    fn events(&self) -> Vec<String> {
        self.w().events.clone()
    }

    fn writes(&self) -> Vec<String> {
        self.events().into_iter().filter(|e| e.starts_with("write:") || e.starts_with("pre:")).collect()
    }

    fn tool(&self, name: &str, a: &Value) -> Result<String, String> {
        let mut w = self.w();
        let dry = a.get("dry_run").and_then(Value::as_bool) == Some(true);
        if crate::mcp::is_write_tool(name) && !dry {
            w.events.push(format!("write:{name}"));
            if w.fail_tool.as_deref() == Some(name) {
                return if name == "apply_manifest" {
                    Ok(json!({ "status": "terminé avec des échecs", "applied": 0, "failed": 1, "steps": [{ "id": "content.plugin.essentialsx", "outcome": "échouée", "detail": "panne simulée" }] }).to_string())
                } else {
                    Err(format!("{name} : panne simulée"))
                };
            }
        }
        let sid = a.get("server_id").and_then(Value::as_i64);
        let idx = |w: &World, id: Option<i64>| w.servers.iter().position(|s| Some(s.id) == id);
        match name {
            "mybox_create_server" => {
                let id = w.next_id;
                w.next_id += 1;
                let (cpu, ram) = (a["cpu"].as_i64().unwrap(), a["ram"].as_i64().unwrap());
                w.cpu -= cpu;
                w.ram -= ram;
                w.servers.push(Srv {
                    id,
                    name: a["name"].as_str().unwrap().into(),
                    mybox: a["mybox_id"].as_i64().unwrap(),
                    status: "active".into(),
                    java: true,
                    platform: "paper".into(),
                    version: "1.21.10".into(),
                    state: "offline".into(),
                    cpu,
                    ram,
                    ..Default::default()
                });
                Ok(format!("Serveur {id} créé dans la MyBox {} (adresse : attribuée à l'installation). L'hébergeur l'installe maintenant : suis-le avec server_status.", a["mybox_id"]))
            }
            "mybox_resize_server" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                w.servers[i].cpu = a["cpu"].as_i64().unwrap();
                w.servers[i].ram = a["ram"].as_i64().unwrap();
                Ok("redimensionné".into())
            }
            "plan_manifest" | "apply_manifest" => {
                let i = idx(&w, sid).ok_or("serveur inconnu")?;
                let m = crate::manifest::Manifest::parse(&a["manifest"])?;
                let content = has_content(&m);
                let configs: Vec<String> = m.configs.iter().map(|c| c.path.clone()).collect();
                let rename = m.server.as_ref().and_then(|s| s.name.clone()).filter(|n| *n != w.servers[i].name);
                let s = &mut w.servers[i];
                if name == "plan_manifest" {
                    let mut steps = Vec::new();
                    if content && !s.content {
                        steps.push(json!({ "id": "content.plugin.essentialsx", "status": "add", "summary": "installer" }));
                    }
                    if rename.is_some() {
                        steps.push(json!({ "id": "server.name", "status": "change", "summary": "renommer" }));
                    }
                    if m.properties.as_ref().is_some_and(|p| !p.is_empty()) && !s.manifest {
                        steps.push(json!({ "id": "properties.motd", "status": "change", "summary": "motd" }));
                    }
                    for p in &configs {
                        if !s.files.contains_key(p) {
                            steps.push(json!({ "id": format!("configs.{p}"), "status": "manual", "summary": format!("« {p} » absent : démarre le serveur une fois (le plugin ou mod le génère), puis relance le plan") }));
                        } else if !s.manifest {
                            steps.push(json!({ "id": format!("configs.{p}"), "status": "change", "summary": "clés" }));
                        }
                    }
                    let todo = steps.iter().filter(|x| matches!(x["status"].as_str(), Some("add" | "change"))).count();
                    return Ok(json!({ "to_apply": todo, "steps": steps, "counts": {} }).to_string());
                }
                let only_content = a["steps"] == json!(["content"]);
                let mut applied = 0;
                if content && !s.content {
                    s.content = true;
                    s.pending.extend(configs.iter().cloned());
                    applied += 1;
                }
                let mut manual = Vec::new();
                if !only_content {
                    if let Some(n) = rename {
                        s.name = n;
                        applied += 1;
                    }
                    if !s.manifest {
                        s.manifest = true;
                        applied += 1;
                    }
                    for p in &configs {
                        if !s.files.contains_key(p) {
                            manual.push(format!("« {p} » absent"));
                        }
                    }
                }
                Ok(json!({ "status": "terminé", "applied": applied, "failed": 0, "already_ok": 0, "steps": [], "manual": manual }).to_string())
            }
            "sync_dir" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                if dry {
                    let n = if w.servers[i].synced { 0 } else { 2 };
                    return Ok(json!({ "counts": { "added": n, "modified": 0, "deleted": 0 } }).to_string());
                }
                w.servers[i].synced = true;
                Ok(json!({ "status": "terminée", "done": { "uploaded": 2, "deleted": 0 } }).to_string())
            }
            "upload_world" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                let s = &mut w.servers[i];
                if dry {
                    let aside = if s.world { json!([{ "from": "/monde", "to": "/monde.old-x" }]) } else { json!([]) };
                    return Ok(json!({ "problems": [], "aside": aside, "server_state": s.state, "target": "/monde" }).to_string());
                }
                if s.state != "offline" {
                    return Err("serveur démarré".into());
                }
                if s.world && a["replace"] != json!(true) {
                    return Err("monde existant".into());
                }
                s.world = true;
                Ok(json!({ "status": "monde installé", "target": "/monde" }).to_string())
            }
            "write_file" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                let (p, c) = (a["path"].as_str().unwrap().to_string(), a["content"].as_str().unwrap().to_string());
                w.servers[i].files.insert(p, c);
                Ok("écrit".into())
            }
            "power_action" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                let s = &mut w.servers[i];
                match a["action"].as_str().unwrap() {
                    "start" | "restart" => {
                        s.state = "running".into();
                        let generated: Vec<String> = if s.platform == "velocity" {
                            vec!["/velocity.toml".into()]
                        } else {
                            vec!["/server.properties".into(), "/config/paper-global.yml".into()]
                        };
                        for f in generated.into_iter().chain(s.pending.drain(..)) {
                            s.files.entry(f).or_default();
                        }
                    }
                    _ => s.state = "offline".into(),
                }
                Ok("ok".into())
            }
            "wait_for" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                Ok(json!({ "reached": w.servers[i].state == a["state"].as_str().unwrap() }).to_string())
            }
            "link_proxy" => {
                let p = idx(&w, a["proxy_server_id"].as_i64()).ok_or("proxy inconnu")?;
                if !w.servers[p].files.contains_key("/velocity.toml") {
                    return Err("ni velocity.toml ni config.yml de BungeeCord à sa racine".into());
                }
                let backends: Vec<i64> = a["backend_server_ids"].as_array().unwrap().iter().filter_map(Value::as_i64).collect();
                let unstarted: Vec<i64> = backends.iter().copied().filter(|b| w.servers.iter().any(|s| s.id == *b && !s.files.contains_key("/config/paper-global.yml"))).collect();
                let mut steps: Vec<Value> = unstarted
                    .iter()
                    .map(|b| json!({ "id": format!("{b}.paper"), "server_id": b, "status": "manual", "summary": "config/paper-global.yml absent (serveur jamais démarré ?). Démarre-le une fois, arrête-le, puis relance link_proxy" }))
                    .collect();
                if w.linked {
                    return Ok(json!({ "status": "déjà conforme", "counts": { "change": 0, "ok": 3, "manual": 0, "refused": 0 }, "steps": [] }).to_string());
                }
                let change = 1 + backends.len() - unstarted.len();
                if dry {
                    steps.push(json!({ "id": "proxy", "server_id": a["proxy_server_id"], "status": "change", "summary": "entrées" }));
                    return Ok(json!({ "counts": { "change": change, "ok": 0, "manual": unstarted.len(), "refused": 0 }, "steps": steps, "pruned": [{ "name": "factions" }] }).to_string());
                }
                if unstarted.is_empty() {
                    w.linked = true;
                }
                let mut done: Vec<Value> = backends.iter().filter(|b| !unstarted.contains(b)).map(|b| json!({ "id": format!("{b}.paper"), "server_id": b, "status": "change", "outcome": "réussie" })).collect();
                done.extend(steps);
                Ok(json!({ "status": "terminé", "applied": change, "counts": { "change": change, "manual": unstarted.len() }, "steps": done }).to_string())
            }
            "network_preset" => {
                let preset = a["preset"].as_str().unwrap().to_string();
                if w.presets.contains(&preset) {
                    return Ok(json!({ "status": "rien à écrire", "counts": { "change": 0, "ok": 2, "manual": 0, "refused": 0 }, "steps": [] }).to_string());
                }
                if dry {
                    let tc = if w.preset_confirm.len() > 1 { w.preset_confirm.remove(0) } else { w.preset_confirm.first().cloned().unwrap_or_default() };
                    return Ok(json!({ "counts": { "change": 2, "ok": 0, "manual": 0, "refused": 0 }, "steps": [{ "id": "1.install", "server_id": 1, "status": "change", "summary": "installer" }], "to_confirm": tc }).to_string());
                }
                w.presets.insert(preset);
                Ok(json!({ "status": "terminé", "applied": 2, "counts": { "change": 2 }, "steps": [] }).to_string())
            }
            "pregenerate_world" => {
                let i = idx(&w, sid).ok_or("inconnu")?;
                if w.servers[i].state != "running" {
                    return Err("serveur hors ligne".into());
                }
                Ok(json!({ "started": true }).to_string())
            }
            "network_topology" => Ok(json!({ "summary": "1 proxy, 2 serveur(s)", "problems": [] }).to_string()),
            "ping_server" => Ok(json!({ "online": true, "version": "Paper 1.21.4", "latency_ms": 30 }).to_string()),
            other => Err(format!("outil inattendu : {other}")),
        }
    }
}

impl Net for Fake {
    async fn account(&self) -> Result<Vec<Account>, String> {
        Ok(self.w().servers.iter().map(|s| Account { id: s.id, name: s.name.clone(), mybox_id: s.mybox, status: s.status.clone(), minecraft_java: s.java }).collect())
    }
    async fn capacity(&self, mybox: i64) -> Result<MyBoxCapacity, String> {
        let w = self.w();
        let mut cap = capacity(w.cpu, w.ram);
        cap.id = mybox;
        cap.servers = w
            .servers
            .iter()
            .filter(|s| s.mybox == mybox)
            .map(|s| MyBoxServerSlot { id: s.id, name: s.name.clone(), cpu: s.cpu, ram: s.ram, paused: s.status == "suspended", ..Default::default() })
            .collect();
        Ok(cap)
    }
    async fn create_defaults(&self, _: i64, _: Option<i64>, _: Option<i64>) -> Result<CreateDefaults, String> {
        Ok(CreateDefaults { egg_id: 1, experimental: false, location_id: Some(1), location: Some("Toulouse".into()) })
    }
    async fn exists(&self, server: i64, path: &str) -> Result<bool, String> {
        Ok(self.w().servers.iter().any(|s| s.id == server && s.files.contains_key(path)))
    }
    async fn read_text(&self, server: i64, path: &str) -> Result<Option<String>, String> {
        Ok(self.w().servers.iter().find(|s| s.id == server).and_then(|s| s.files.get(path).cloned()))
    }
    async fn platform(&self, server: i64, platform: Option<&str>, version: Option<&str>, _fresh: bool) -> Result<PlatformView, String> {
        let w = self.w();
        let s = w.servers.iter().find(|s| s.id == server).ok_or("inconnu")?;
        let want = (platform.unwrap_or(&s.platform).to_string(), version.unwrap_or(&s.version).to_string());
        let mut v = PlatformView { current_platform: Some(s.platform.clone()), current_version: Some(s.version.clone()), ..Default::default() };
        if want != (s.platform.clone(), s.version.clone()) {
            v.ops.push(("change_version".into(), format!("installer {} {} (DESTRUCTIF, asynchrone)", want.0, want.1)));
            v.destructive = true;
        }
        Ok(v)
    }
    async fn apply_platform(&self, server: i64, platform: Option<&str>, version: Option<&str>, _fresh: bool) -> Result<PlatformRun, String> {
        let mut w = self.w();
        w.events.push(format!("platform:{server}"));
        let s = w.servers.iter_mut().find(|s| s.id == server).ok_or("inconnu")?;
        if let Some(p) = platform {
            s.platform = p.into();
        }
        if let Some(v) = version {
            s.version = v.into();
        }
        Ok(PlatformRun { results: vec![("change_version".into(), "réussie".into(), None)], async_change: true })
    }
    async fn state(&self, server: i64) -> Result<String, String> {
        self.w().servers.iter().find(|s| s.id == server).map(|s| s.state.clone()).ok_or_else(|| "inconnu".into())
    }
    async fn world_fingerprint(&self, local_path: &str) -> Result<String, String> {
        Ok(format!("sha256:{local_path}"))
    }
    async fn call(&self, tool: &str, args: Value) -> Result<String, String> {
        self.tool(tool, &args)
    }
    async fn call_preapproved(&self, tool: &str, args: Value) -> Result<String, String> {
        self.w().events.push(format!("pre:{tool}"));
        self.tool(tool, &args)
    }
    async fn refusal(&self, _tool: &str, args: &Value) -> Option<String> {
        let w = self.w();
        let mut ids: Vec<i64> = ["server_id", "proxy_server_id", "target_server_id"].iter().filter_map(|k| args.get(*k).and_then(Value::as_i64)).collect();
        ids.extend(args.get("backend_server_ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_i64));
        ids.extend(target_ids(args).unwrap_or_default());
        ids.iter().find(|id| w.forbidden.contains(id)).map(|id| format!("Refusé (mode constructeur) : serveur {id} hors de cette portée."))
    }
    fn record_created(&self, server: i64) {
        self.w().events.push(format!("created:{server}"));
    }
    async fn wait_installed(&self, _server: i64) -> Result<(), String> {
        Ok(())
    }
    async fn confirm(&self, _message: &str) -> Confirmation {
        let mut w = self.w();
        w.events.push("confirm".into());
        w.confirm
    }
    async fn confirm_name(&self, _message: &str, expected: &str, given: Option<String>) -> Result<(), String> {
        let mut w = self.w();
        w.events.push(format!("name:{expected}"));
        if w.elicit {
            return Ok(());
        }
        match given {
            None => Err("paramètre `confirm_name` (texte) requis".into()),
            Some(g) if g.trim() == expected => Ok(()),
            Some(_) => Err("ne correspond pas".into()),
        }
    }
    async fn snapshot(&self, server: i64, _name: &str) -> Result<(), String> {
        self.w().events.push(format!("snapshot:{server}"));
        Ok(())
    }
    async fn pause(&self, _secs: u64) {}
}

fn apply(fake: &Fake, bp: &Value, opts: ApplyOptions) -> Result<(Value, bool), String> {
    let nw = Network::parse(bp).unwrap();
    block(apply_with(fake, &nw, &opts))
}

fn real(confirm_names: &[&str]) -> ApplyOptions {
    ApplyOptions { dry_run: false, snapshot: true, start: true, confirm_names: confirm_names.iter().map(|s| s.to_string()).collect() }
}

fn plan(fake: &Fake, bp: &Value) -> Plan {
    let nw = Network::parse(bp).unwrap();
    block(plan_with(fake, &nw)).unwrap()
}

fn fresh_blueprint(dir: &str) -> Value {
    json!({
        "version": 1, "name": "Réseau test", "mybox_id": 10,
        "proxy": { "name": "Proxy", "platform": "velocity", "cpu": 1, "ram": 1, "try": ["Lobby"], "forced_hosts": { "play.exemple.fr": "Lobby" }, "prune": true },
        "servers": [
            { "name": "Survie", "role": "survival", "platform": "paper", "version": "1.21.4", "cpu": 2, "ram": 4,
              "manifest": { "properties": { "motd": "Survie" }, "content": [{ "kind": "plugin", "source": "modrinth", "project": "essentialsx" }], "configs": [{ "path": "/plugins/Essentials/config.yml", "set": { "ops-name-color": "4" } }] },
              "sync": { "local_dir": dir, "remote_dir": "/plugins/Essentials" },
              "pregenerate": { "radius": 1000 } },
            { "name": "Lobby", "role": "lobby", "entry": "lobby", "platform": "paper", "cpu": 1, "ram": 2 }
        ],
        "presets": ["luckperms"],
        "database": { "database_id": 5 }
    })
}

fn pos(events: &[String], e: &str) -> usize {
    events.iter().position(|x| x == e).unwrap_or_else(|| panic!("{e} absent de {events:?}"))
}

fn last(events: &[String], e: &str) -> usize {
    events.iter().rposition(|x| x == e).unwrap_or_else(|| panic!("{e} absent de {events:?}"))
}

// --- Plan et application --------------------------------------------------------------------------------

#[test]
fn a_new_network_is_created_then_mounted_in_order_and_a_rerun_does_nothing() {
    let dir = TempDir::new("bp-fresh");
    dir.put("config.yml", "a: 1");
    let bp = fresh_blueprint(&dir.0.to_string_lossy());
    let fake = Fake::new(8, 32);

    // Plan : trois serveurs à créer, place suffisante, liaison et préréglage décrits.
    let p = plan(&fake, &bp);
    assert!(p.blocked().is_empty(), "{:?}", p.blocked());
    assert_eq!(p.items.iter().filter(|i| i.id.ends_with(":server") && i.status == Status::Create).count(), 3);
    assert_eq!(p.capacity["requested"], json!({ "cpu": 4, "ram": 7, "servers": 3 }));
    assert!(p.to_confirm.iter().any(|c| c.contains("créer « Survie » : 2 cœur(s), 4 Go")));
    assert_eq!(p.item(None, Phase::Link).unwrap().status, Status::Create);

    // dry_run (défaut) : le plan, aucun effet.
    let (v, failed) = apply(&fake, &bp, ApplyOptions { dry_run: true, ..real(&[]) }).unwrap();
    assert!(!failed && v["dry_run"] == json!(true));
    assert!(fake.events().is_empty(), "rien avant dry_run=false : {:?}", fake.events());

    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(!failed, "{v:#}");
    assert_eq!(v["status"], "terminé");
    assert_eq!(v["created"].as_array().unwrap().len(), 3);
    let ev = fake.events();
    // Confirmation d'abord ; serveurs neufs : ni snapshot, ni nom à retaper.
    assert_eq!(ev[0], "confirm");
    assert!(!ev.iter().any(|e| e.starts_with("snapshot:") || e.starts_with("name:")));
    assert_eq!(ev.iter().filter(|e| e.starts_with("created:")).count(), 3, "serveurs créés rejoints à la portée");
    // Ordre des phases.
    let creates = last(&ev, "write:mybox_create_server");
    assert!(creates < pos(&ev, "platform:100"));
    let content = pos(&ev, "write:apply_manifest");
    assert!(pos(&ev, "platform:101") < content, "plateforme avant le contenu");
    let first_start = pos(&ev, "write:power_action");
    assert!(content < first_start, "contenu installé avant le premier démarrage");
    let manifest = last(&ev, "write:apply_manifest");
    assert!(first_start < manifest && manifest < pos(&ev, "write:sync_dir"));
    assert!(pos(&ev, "write:sync_dir") < pos(&ev, "write:link_proxy"));
    assert!(pos(&ev, "write:link_proxy") < pos(&ev, "pre:network_preset"), "préréglage après la liaison, confirmations pré-accordées");
    assert!(pos(&ev, "pre:network_preset") < pos(&ev, "write:pregenerate_world"));
    // Monde de survie : Chunky ajouté au contenu, prégénération lancée sur le serveur démarré, marqueur posé.
    let w = fake.w();
    let survie = w.servers.iter().find(|s| s.name == "Survie").unwrap();
    assert!(survie.files[MARKER_PATH].contains("\"radius\": 1000"));
    assert!(survie.files.contains_key("/plugins/Essentials/config.yml"), "config générée au premier démarrage");
    assert!(w.servers.iter().all(|s| s.state == "running"));
    assert!(w.linked && w.presets.contains("luckperms"));
    assert_eq!(survie.version, "1.21.4");
    drop(w);
    assert!(v["verify"]["ping"]["Lobby"]["online"] == json!(true));

    // Relance : tout est conforme, rien n'est écrit, aucune question.
    let p = plan(&fake, &bp);
    assert!(!p.todo(), "{:#?}", p.items.iter().filter(|i| i.status != Status::Ok).collect::<Vec<_>>());
    let before = fake.events().len();
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(!failed && v["status"] == "déjà conforme : rien à faire");
    assert_eq!(fake.events().len(), before, "aucune écriture ni confirmation à la relance");
}

#[test]
fn not_enough_room_is_blocking_and_quantified() {
    let dir = TempDir::new("bp-room");
    let bp = fresh_blueprint(&dir.0.to_string_lossy());
    let fake = Fake::new(0, 12);
    let p = plan(&fake, &bp);
    let blocked = p.blocked();
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].summary.contains("CPU : 4 cœur(s) demandé(s), 0 restant(s) — il manque 4 cœur(s)"), "{}", blocked[0].summary);
    assert!(!blocked[0].summary.contains("RAM"), "la RAM suffit");
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(failed && v["status"].as_str().unwrap().starts_with("refusé"));
    assert!(fake.events().is_empty(), "ni confirmation ni écriture");
}

fn existing(id: i64, name: &str, platform: &str, version: &str) -> Srv {
    let mut files = BTreeMap::new();
    let generated = if platform == "velocity" { "/velocity.toml" } else { "/server.properties" };
    files.insert(generated.to_string(), String::new());
    files.insert("/config/paper-global.yml".to_string(), String::new());
    Srv { id, name: name.into(), mybox: 10, status: "active".into(), java: true, platform: platform.into(), version: version.into(), state: "running".into(), cpu: 2, ram: 4, files, ..Default::default() }
}

fn solo(server: Value) -> Value {
    json!({ "version": 1, "mybox_id": 10, "servers": [server] })
}

#[test]
fn servers_are_matched_by_id_then_exact_name_in_the_mybox() {
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "paper", "1.21.4"));
    fake.add(existing(8, "Double", "paper", "1.21.4"));
    fake.add(existing(9, "Double", "paper", "1.21.4"));
    let mut paused = existing(11, "Pause", "paper", "1.21.4");
    paused.status = "suspended".into();
    fake.add(paused);
    let mut other_game = existing(12, "Usine", "paper", "1.21.4");
    other_game.java = false;
    fake.add(other_game);
    let mut elsewhere = existing(13, "Ailleurs", "paper", "1.21.4");
    elsewhere.mybox = 99;
    fake.add(elsewhere);
    let bp = json!({ "version": 1, "mybox_id": 10, "servers": [
        { "name": "Survie" },
        { "name": "Double" },
        { "name": "Pause" },
        { "name": "Autre", "server_id": 12 },
        { "name": "Inconnu", "server_id": 404 },
        { "name": "Ailleurs" },
    ] });
    let p = plan(&fake, &bp);
    let status = |name: &str| p.items.iter().find(|i| i.id == format!("{name}:server")).map(|i| (i.status, i.summary.clone())).unwrap();
    assert_eq!(status("Survie").0, Status::Ok);
    assert_eq!(p.nodes[0].matched_by, Some("nom"));
    assert!(status("Double").0 == Status::Blocked && status("Double").1.contains("précise `server_id`"));
    assert!(status("Pause").0 == Status::Blocked && status("Pause").1.contains("reprends-le"));
    assert!(status("Autre").1.contains("pas un serveur Minecraft Java"));
    assert!(status("Inconnu").1.contains("introuvable"));
    // Nom cherché dans la MyBox du blueprint seulement : « Ailleurs » (autre MyBox) est à créer,
    // et sans cpu ni ram, c'est bloquant.
    assert!(status("Ailleurs").0 == Status::Blocked && status("Ailleurs").1.contains("`cpu` et `ram` requis"));
}

#[test]
fn builder_mode_refuses_a_server_reached_by_its_name_before_any_effect() {
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "paper", "1.21.4"));
    fake.w().forbidden.insert(7);
    let bp = solo(json!({ "name": "Survie", "manifest": { "properties": { "motd": "x" } } }));
    // Le blueprint ne cite aucun identifiant : la règle de write_targets dit « création ».
    assert!(target_ids(&json!({ "blueprint": bp })).unwrap().is_empty());
    let e = apply(&fake, &bp, real(&[])).unwrap_err();
    assert!(e.contains("mode constructeur") && e.contains('7'), "{e}");
    assert!(fake.writes().is_empty() && !fake.events().contains(&"confirm".to_string()));
}

#[test]
fn change_version_requires_the_exact_name_before_any_effect() {
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "spigot", "1.21.4"));
    let bp = solo(json!({ "name": "Survie", "platform": "paper" }));
    let p = plan(&fake, &bp);
    assert_eq!(p.name_confirm, vec![(0, "Survie".to_string())]);
    assert!(p.to_confirm.iter().any(|c| c.contains("DESTRUCTIF")));
    // Ni élicitation ni confirm_names : refus, rien n'est fait.
    let e = apply(&fake, &bp, real(&[])).unwrap_err();
    assert!(e.contains("confirm_names") && e.contains("Rien n'a été fait"), "{e}");
    assert!(fake.writes().is_empty() && !fake.events().iter().any(|e| e.starts_with("snapshot:") || e.starts_with("platform:")));
    // Mauvais nom : refus.
    assert!(apply(&fake, &bp, real(&["survie"])).is_err());
    assert!(!fake.events().iter().any(|e| e.starts_with("platform:")));
    // Nom exact donné par l'utilisateur : snapshot attendue, puis plateforme.
    let (v, failed) = apply(&fake, &bp, real(&["Survie"])).unwrap();
    assert!(!failed, "{v:#}");
    let ev = fake.events();
    assert!(pos(&ev, "snapshot:7") < pos(&ev, "platform:7"));
    assert_eq!(fake.w().servers[0].platform, "paper");
    // Client capable d'élicitation : l'humain retape le nom, confirm_names inutile.
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "spigot", "1.21.4"));
    fake.w().elicit = true;
    assert!(!apply(&fake, &bp, real(&[])).unwrap().1);
}

#[test]
fn a_declined_confirmation_does_nothing() {
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "paper", "1.21.4"));
    fake.w().confirm = Confirmation::Declined;
    let bp = solo(json!({ "name": "Survie", "manifest": { "properties": { "motd": "x" } } }));
    let e = apply(&fake, &bp, real(&[])).unwrap_err();
    assert!(e.contains("Refusé par l'utilisateur"));
    assert_eq!(fake.events(), vec!["confirm".to_string()]);
}

#[test]
fn the_first_failure_stops_the_run_and_a_rerun_resumes_without_recreating() {
    let dir = TempDir::new("bp-fail");
    dir.put("config.yml", "a: 1");
    let bp = fresh_blueprint(&dir.0.to_string_lossy());
    let fake = Fake::new(8, 32);
    fake.w().fail_tool = Some("apply_manifest".into());
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(failed && v["status"] == "arrêté après un échec");
    assert_eq!(v["failed"]["step"], "Survie:content");
    assert!(v["failed"]["error"].as_str().unwrap().contains("panne simulée"));
    let steps = v["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["id"] == "Survie:first_start" && s["outcome"] == "non exécutée"));
    assert!(!fake.writes().contains(&"write:link_proxy".to_string()), "rien après l'échec");
    assert!(v["resume"].as_str().unwrap().contains("relancer") || v["resume"].as_str().unwrap().contains("Relancer"));
    assert_eq!(fake.w().servers.len(), 3);

    // Cause corrigée : la relance rapproche les serveurs créés par leur nom et termine.
    fake.w().fail_tool = None;
    let before = fake.events().len();
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(!failed, "{v:#}");
    let ev = fake.events()[before..].to_vec();
    assert!(!ev.contains(&"write:mybox_create_server".to_string()), "aucun serveur recréé : {ev:?}");
    assert_eq!(fake.w().servers.len(), 3);
    assert!(fake.w().linked);
    // Serveurs devenus existants : snapshot avant d'y toucher.
    assert!(ev.iter().any(|e| e.starts_with("snapshot:")));
    // Proxy jamais démarré à la relance : liaison simulée après son premier démarrage.
    assert!(ev.contains(&"write:link_proxy".to_string()));
}

#[test]
fn a_preset_effect_not_confirmed_beforehand_stops_the_run() {
    let fake = Fake::new(8, 32);
    fake.add(existing(1, "Proxy", "velocity", "3.4.0"));
    fake.add(existing(2, "Lobby", "paper", "1.21.4"));
    fake.w().linked = true;
    // Au plan : rien à confirmer ; à l'exécution, la simulation en trouve un (état changé depuis).
    fake.w().preset_confirm = vec![Vec::new(), vec!["« Lobby » (2) : stockage actuel « h2 » (local)".into()]];
    let bp = json!({ "version": 1, "mybox_id": 10, "proxy": { "name": "Proxy" }, "servers": [{ "name": "Lobby" }], "presets": ["geyser"] });
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(failed, "{v:#}");
    assert!(v["failed"]["error"].as_str().unwrap().contains("nouvelle confirmation"));
    assert!(!fake.events().contains(&"pre:network_preset".to_string()), "préréglage non appliqué");

    // Présenté au récapitulatif (plan), il est approuvé : appliqué avec les confirmations pré-accordées.
    let fake = Fake::new(8, 32);
    fake.add(existing(1, "Proxy", "velocity", "3.4.0"));
    fake.add(existing(2, "Lobby", "paper", "1.21.4"));
    fake.w().linked = true;
    fake.w().preset_confirm = vec![vec!["« Lobby » (2) : stockage actuel « h2 » (local)".into()]];
    let p = plan(&fake, &bp);
    assert!(p.to_confirm.iter().any(|c| c.contains("h2")));
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(!failed, "{v:#}");
    assert!(fake.events().contains(&"pre:network_preset".to_string()));
}

#[test]
fn a_world_is_installed_once_thanks_to_the_marker() {
    let fake = Fake::new(8, 32);
    fake.add(existing(7, "Survie", "paper", "1.21.4"));
    let bp = solo(json!({ "name": "Survie", "world": { "local_path": abs("bp-monde"), "world_name": "monde" } }));
    let p = plan(&fake, &bp);
    let world = p.item(Some(0), Phase::World).unwrap();
    assert_eq!(world.status, Status::Change);
    assert!(p.to_confirm.iter().any(|c| c.contains("arrêté pour installer son monde")));
    let (v, failed) = apply(&fake, &bp, real(&[])).unwrap();
    assert!(!failed, "{v:#}");
    let ev = fake.events();
    assert!(pos(&ev, "snapshot:7") < pos(&ev, "write:power_action"), "snapshot avant l'arrêt");
    assert!(pos(&ev, "write:power_action") < pos(&ev, "write:upload_world"), "arrêt avant l'envoi");
    assert!(fake.w().servers[0].files[MARKER_PATH].contains("sha256:"));
    assert_eq!(fake.w().servers[0].state, "running", "redémarré à la fin");
    // Relance : monde déjà installé depuis cette source.
    let p = plan(&fake, &bp);
    assert_eq!(p.item(Some(0), Phase::World).unwrap().status, Status::Ok);
    assert!(!p.todo());
    // Un monde présent que le blueprint n'a pas installé : manuel sans `replace`.
    let fake = Fake::new(8, 32);
    let mut s = existing(7, "Survie", "paper", "1.21.4");
    s.world = true;
    fake.add(s);
    let p = plan(&fake, &bp);
    assert_eq!(p.item(Some(0), Phase::World).unwrap().status, Status::Manual);
}

#[test]
fn resizing_counts_against_the_free_space() {
    let fake = Fake::new(1, 2);
    fake.add(existing(7, "Survie", "paper", "1.21.4"));
    let p = plan(&fake, &solo(json!({ "name": "Survie", "cpu": 3, "ram": 4 })));
    assert_eq!(p.items.iter().find(|i| i.id == "Survie:resources").unwrap().status, Status::Change);
    assert!(p.blocked().is_empty(), "un cœur de plus, un restant");
    let p = plan(&fake, &solo(json!({ "name": "Survie", "cpu": 4, "ram": 4 })));
    assert!(p.blocked()[0].summary.contains("il manque 1 cœur(s)"));
}
