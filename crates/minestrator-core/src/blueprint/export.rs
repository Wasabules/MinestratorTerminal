//! Blueprint d'un réseau existant (`export_network`) et modèles pré-remplis (`network_template`).
//! Lectures seules ; l'assemblage est pur ([`build_export`], [`build_template`]) et testé sans
//! réseau.

use super::plan::sub_dispatch;
use super::*;
use crate::mybox::MyBoxCapacity;
use crate::Core;

// --- Export ------------------------------------------------------------------------------------------

/// Un serveur exporté : son manifeste (`export_manifest`) et ses ressources.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Exported {
    pub server_id: i64,
    pub name: String,
    /// `manifest` de la sortie d'`export_manifest` (secrets déjà masqués).
    pub manifest: Value,
    pub cpu: Option<i64>,
    pub ram: Option<i64>,
}

/// Ce qu'on sait d'un réseau existant.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ExportInput {
    pub mybox_id: i64,
    pub proxy: Exported,
    /// `modern` ou `legacy` (d'après le transfert du proxy).
    pub mode: Option<String>,
    /// Entrées du proxy dans l'ordre : (nom d'entrée, serveur du compte s'il est reconnu, adresse).
    pub entries: Vec<(String, Option<i64>, String)>,
    pub try_order: Vec<String>,
    pub forced_hosts: Vec<(String, Vec<String>)>,
    /// Serveurs de jeu du compte reliés au proxy.
    pub servers: Vec<Exported>,
}

/// Plateforme, version et manifeste sans `server` (ni `players` : alias d'anonymisation
/// inutilisables ailleurs) d'un manifeste exporté.
fn split_manifest(m: &Value) -> (Option<String>, Option<String>, Option<Value>) {
    // Nom normalisé : l'hébergeur dit `proxy_velocity` pour un Velocity (relevé réel du 2026-11-20).
    let platform = m.pointer("/server/loader").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(crate::platform::normalize_platform);
    let version = m.pointer("/server/game_version").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
    let mut rest = m.as_object().cloned().unwrap_or_default();
    rest.remove("server");
    rest.remove("players");
    rest.remove("version");
    let keep = !rest.is_empty();
    rest.insert("version".into(), json!(crate::manifest::MANIFEST_VERSION));
    (platform, version, keep.then_some(Value::Object(rest)))
}

/// Le manifeste installe-t-il ce plugin (projet, nom ou fichier qui le contient) ?
fn has_plugin(m: &Value, needle: &str) -> bool {
    let items = match m.get("content") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    items.iter().any(|i| {
        ["project", "name", "file"]
            .iter()
            .filter_map(|k| i.get(*k).and_then(Value::as_str))
            .any(|t| t.to_ascii_lowercase().contains(needle))
    })
}

/// Blueprint d'un réseau existant (pur). Rend aussi les notes : ce qui n'est pas exporté.
pub(crate) fn build_export(input: &ExportInput) -> (Value, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    // Entrée → nom du serveur du compte (dans le blueprint, un serveur se désigne par son nom).
    let name_of = |entry: &str| -> Option<String> {
        let (_, id, _) = input.entries.iter().find(|(e, _, _)| e.eq_ignore_ascii_case(entry))?;
        let id = (*id)?;
        input.servers.iter().find(|s| s.server_id == id).map(|s| s.name.clone())
    };
    let (platform, version, manifest) = split_manifest(&input.proxy.manifest);
    let mut proxy = json!({ "name": input.proxy.name, "server_id": input.proxy.server_id });
    if let Some(p) = platform {
        proxy["platform"] = json!(p);
    }
    if let Some(v) = version {
        proxy["version"] = json!(v);
    }
    if let Some(c) = input.proxy.cpu {
        proxy["cpu"] = json!(c);
    }
    if let Some(r) = input.proxy.ram {
        proxy["ram"] = json!(r);
    }
    if let Some(m) = &input.mode {
        proxy["mode"] = json!(m);
    }
    let mut hosts = Map::new();
    for (host, entries) in &input.forced_hosts {
        let names: Vec<String> = entries.iter().filter_map(|e| name_of(e)).collect();
        if names.len() < entries.len() {
            notes.push(format!("domaine « {host} » : entrée(s) qui ne mènent à aucun serveur du compte non exportées"));
        }
        if crate::network::valid_host(host).is_err() || names.is_empty() {
            continue;
        }
        hosts.insert(host.clone(), if names.len() == 1 { json!(names[0]) } else { json!(names) });
    }
    if !hosts.is_empty() {
        proxy["forced_hosts"] = Value::Object(hosts);
    }
    let try_names: Vec<String> = input.try_order.iter().filter_map(|e| name_of(e)).collect();
    if !try_names.is_empty() {
        proxy["try"] = json!(try_names);
    }
    if let Some(m) = manifest {
        proxy["manifest"] = m;
    }
    let mut servers = Vec::new();
    for (entry, id, address) in &input.entries {
        let Some(s) = id.and_then(|id| input.servers.iter().find(|s| s.server_id == id)) else {
            notes.push(format!("entrée « {entry} » ({address}) : ne mène à aucun serveur du compte, non exportée (prune=true la retirerait)"));
            continue;
        };
        if servers.iter().any(|x: &Value| x["server_id"] == json!(s.server_id)) {
            continue;
        }
        let (platform, version, manifest) = split_manifest(&s.manifest);
        let mut v = json!({ "name": s.name, "server_id": s.server_id });
        if try_names.first() == Some(&s.name) {
            v["role"] = json!("lobby");
        }
        match crate::network::valid_name(entry) {
            Ok(e) => v["entry"] = json!(e),
            Err(_) => notes.push(format!("entrée « {entry} » : nom non repris tel quel (lettres, chiffres, - et _), link_proxy en choisira un")),
        }
        if let Some(p) = platform {
            v["platform"] = json!(p);
        }
        if let Some(x) = version {
            v["version"] = json!(x);
        }
        if let Some(c) = s.cpu {
            v["cpu"] = json!(c);
        }
        if let Some(r) = s.ram {
            v["ram"] = json!(r);
        }
        if let Some(m) = manifest {
            v["manifest"] = m;
        }
        servers.push(v);
    }
    let mut presets = Vec::new();
    let all = std::iter::once(&input.proxy).chain(&input.servers);
    if !input.servers.is_empty() && all.clone().all(|s| has_plugin(&s.manifest, "luckperms")) {
        presets.push("luckperms");
        notes.push("préréglage luckperms repéré (LuckPerms partout) : ajoute `database: { database_id }` (bases de la MyBox : mybox_info) pour fixer la base partagée".into());
    }
    if has_plugin(&input.proxy.manifest, "geyser") {
        presets.push("geyser");
    }
    let mut bp = json!({
        "version": BLUEPRINT_VERSION,
        "name": format!("Réseau de {}", input.proxy.name),
        "mybox_id": input.mybox_id,
        "proxy": proxy,
        "servers": servers,
    });
    if !presets.is_empty() {
        bp["presets"] = json!(presets);
    }
    notes.push("secrets masqués en [SECRET] : à l'application, « garder la valeur actuelle » (ou {{secret:NOM}} pour en écrire une)".into());
    notes.push("non exportés : mondes, fichiers hors manifeste, liste blanche et opérateurs, chemins de l'ordinateur (sync, world)".into());
    if let Err(e) = Network::parse(&bp) {
        notes.push(format!("blueprint à compléter avant plan_network : {e}"));
    }
    (bp, notes)
}

/// Manifeste (`export_manifest`, sans joueurs) et ressources d'un serveur. Manifeste illisible :
/// `null` (le blueprint garde le serveur, sans manifeste).
async fn export_server(core: &Core, caps: &[MyBoxCapacity], sid: i64, name: String) -> Exported {
    let out = sub_dispatch(core, "export_manifest", json!({ "server_id": sid, "include_players": false })).await;
    let manifest = out.map(|o| parse_out(&o).get("manifest").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null);
    let slot = caps.iter().flat_map(|c| &c.servers).find(|s| s.id == sid);
    Exported { server_id: sid, name, manifest, cpu: slot.map(|s| s.cpu), ram: slot.map(|s| s.ram) }
}

/// Outil MCP `export_network`.
pub(crate) async fn export_network_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = args
        .get("server_id")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .ok_or("paramètre `server_id` (identifiant du proxy) requis")?;
    crate::mcp_ctx::progress(0.0, None, "topologie du réseau").await;
    let topo = parse_out(&sub_dispatch(core, "network_topology", json!({ "server_id": id })).await?);
    let proxies = topo.get("proxies").and_then(Value::as_array).cloned().unwrap_or_default();
    let proxy = proxies
        .iter()
        .find(|p| p.get("server_id").and_then(Value::as_i64) == Some(id))
        .or_else(|| (proxies.len() == 1).then(|| &proxies[0]))
        .cloned()
        .ok_or_else(|| {
            let note = topo.get("note").and_then(Value::as_str).unwrap_or("aucun proxy lisible ne le référence");
            format!("serveur {id} : ni proxy lisible, ni serveur relié à un seul proxy ({note}) ; network_topology pour voir les réseaux du compte")
        })?;
    let pid = proxy.get("server_id").and_then(Value::as_i64).ok_or("proxy sans identifiant")?;
    let pname = proxy.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    // Hôtes forcés et liste de repli par nom d'entrée (la topologie n'en donne que le nombre) ;
    // le secret lu au passage n'est jamais repris.
    let (state, _) = crate::network::read_proxy(core, pid, &pname, None)
        .await?
        .ok_or_else(|| format!("« {pname} » ({pid}) : configuration du proxy introuvable"))?;
    let mode = match state.conf.forwarding {
        crate::network::Forwarding::Modern => Some("modern".to_string()),
        crate::network::Forwarding::Legacy | crate::network::Forwarding::BungeeGuard => Some("legacy".to_string()),
        _ => None,
    };
    let entries: Vec<(String, Option<i64>, String)> = proxy
        .get("servers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    (
                        s.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        s.pointer("/account_server/server_id").and_then(Value::as_i64),
                        s.get("address").and_then(Value::as_str).unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let overview = core.list_servers().await.map_err(|e| e.to_string())?;
    let mybox_id = overview.servers.iter().find(|s| s.id == pid).map(|s| s.mybox_id).unwrap_or_default();
    let caps = core.mybox_capacities().await.unwrap_or_default();
    let mut ids: Vec<i64> = entries.iter().filter_map(|(_, id, _)| *id).filter(|id| *id != pid).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut servers = Vec::new();
    let total = ids.len() as f64 + 1.0;
    crate::mcp_ctx::progress(1.0, Some(total + 1.0), "manifeste du proxy").await;
    let proxy_export = export_server(core, &caps, pid, pname.clone()).await;
    for (k, sid) in ids.iter().enumerate() {
        let name = overview.servers.iter().find(|s| s.id == *sid).map(|s| s.name.clone()).unwrap_or_else(|| format!("serveur-{sid}"));
        crate::mcp_ctx::progress(k as f64 + 2.0, Some(total + 1.0), &format!("manifeste de « {name} »")).await;
        servers.push(export_server(core, &caps, *sid, name).await);
    }
    let input = ExportInput {
        mybox_id,
        proxy: proxy_export,
        mode,
        entries,
        try_order: state.conf.try_order.clone(),
        forced_hosts: state.conf.forced_hosts.clone(),
        servers,
    };
    let (bp, notes) = build_export(&input);
    let out = json!({
        "blueprint": bp,
        "notes": notes,
        "next": "plan_network avec ce blueprint (modifié au besoin) : sur le réseau inchangé, presque tout est `ok`. Pour reproduire le réseau ailleurs : retire les server_id, change mybox_id et les noms.",
    });
    Ok(crate::config_edit::render_for_ai(core, crate::redact::mask_known_secrets_value(out)))
}

// --- Modèles -----------------------------------------------------------------------------------------

/// Serveur d'un modèle : ressources minimales et confortables (cœurs, Go), contenu proposé.
struct TplServer {
    name: &'static str,
    role: &'static str,
    min: (i64, i64),
    comfort: (i64, i64),
    plugins: &'static [&'static str],
    properties: &'static [(&'static str, &'static str)],
    pregen: Option<u64>,
}

struct Template {
    name: &'static str,
    description: &'static str,
    proxy: bool,
    servers: &'static [TplServer],
}

const PROXY_MIN: (i64, i64) = (1, 1);
const PROXY_COMFORT: (i64, i64) = (1, 2);

const LOBBY: TplServer = TplServer {
    name: "Lobby",
    role: "lobby",
    min: (1, 2),
    comfort: (1, 3),
    plugins: &[],
    properties: &[("gamemode", "adventure"), ("difficulty", "peaceful"), ("pvp", "false"), ("motd", "Lobby")],
    pregen: None,
};

const SURVIE: TplServer = TplServer {
    name: "Survie",
    role: "survival",
    min: (1, 3),
    comfort: (2, 6),
    plugins: &["essentialsx", "coreprotect"],
    properties: &[("gamemode", "survival"), ("difficulty", "normal"), ("motd", "Survie")],
    pregen: Some(3000),
};

const CREATIF: TplServer = TplServer {
    name: "Créatif",
    role: "creative",
    min: (1, 2),
    comfort: (1, 4),
    plugins: &["essentialsx", "worldedit"],
    properties: &[("gamemode", "creative"), ("difficulty", "peaceful"), ("pvp", "false"), ("motd", "Créatif")],
    pregen: None,
};

const TEMPLATES: &[Template] = &[
    Template {
        name: "proxy-lobby-survie",
        description: "Proxy Velocity, un lobby d'accueil et un serveur de survie (Paper).",
        proxy: true,
        servers: &[LOBBY, SURVIE],
    },
    Template {
        name: "proxy-lobby-survie-creatif",
        description: "Proxy Velocity, un lobby, un serveur de survie et un serveur créatif (Paper).",
        proxy: true,
        servers: &[LOBBY, SURVIE, CREATIF],
    },
    Template {
        name: "survie-seul",
        description: "Un seul serveur de survie (Paper), sans proxy.",
        proxy: false,
        servers: &[SURVIE],
    },
];

/// Noms des modèles.
pub(crate) fn template_names() -> Vec<&'static str> {
    TEMPLATES.iter().map(|t| t.name).collect()
}

/// Ressources : (cœurs, Go de RAM).
type Res = (i64, i64);

/// Répartit la place libre : chacun au minimum, puis vers le confortable par priorité (ordre
/// donné), unité par unité. `None` : place illimitée (MyBox Pro) ou inconnue — le confortable.
fn allocate(wants: &[(Res, Res)], free: Option<Res>) -> (Vec<Res>, Option<String>) {
    let Some((cpu, ram)) = free else {
        return (wants.iter().map(|(_, c)| *c).collect(), None);
    };
    let min_cpu: i64 = wants.iter().map(|(m, _)| m.0).sum();
    let min_ram: i64 = wants.iter().map(|(m, _)| m.1).sum();
    let mut out: Vec<(i64, i64)> = wants.iter().map(|(m, _)| *m).collect();
    if min_cpu > cpu || min_ram > ram {
        let mut missing = Vec::new();
        if min_cpu > cpu {
            missing.push(format!("{} cœur(s) au minimum, {cpu} libre(s)", min_cpu));
        }
        if min_ram > ram {
            missing.push(format!("{} Go au minimum, {ram} Go libre(s)", min_ram));
        }
        return (out, Some(format!("pas assez de place dans la MyBox : {} — libère des ressources (mybox_pause_server, mybox_resize_server) ou réduis le modèle ; plan_network le bloquera", missing.join(" ; "))));
    }
    let (mut left_cpu, mut left_ram) = (cpu - min_cpu, ram - min_ram);
    loop {
        let mut grew = false;
        for (k, (_, comfort)) in wants.iter().enumerate() {
            if left_cpu > 0 && out[k].0 < comfort.0 {
                out[k].0 += 1;
                left_cpu -= 1;
                grew = true;
            }
            if left_ram > 0 && out[k].1 < comfort.1 {
                out[k].1 += 1;
                left_ram -= 1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    (out, None)
}

/// Blueprint d'un modèle (pur). `cap` : capacité de la MyBox (ressources réparties sur sa place
/// libre), `None` : ressources confortables.
pub(crate) fn build_template(
    name: &str,
    mybox_id: i64,
    cap: Option<&MyBoxCapacity>,
    version: Option<&str>,
    platform: Option<&str>,
    domain: Option<&str>,
) -> Result<(Value, Value, Vec<String>), String> {
    let t = TEMPLATES
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("modèle « {name} » inconnu : {}", template_names().join(", ")))?;
    let platform = platform.map(crate::platform::normalize_platform).unwrap_or_else(|| "paper".into());
    if PROXY_PLATFORMS.contains(&platform.as_str()) {
        return Err(format!("`platform` : « {platform} » est un proxy ; plateforme des serveurs de jeu attendue (paper, purpur…)"));
    }
    let mut wants: Vec<(Res, Res)> = Vec::new();
    // Priorité de la répartition : serveurs de jeu (dans l'ordre du modèle, survie avant lobby),
    // proxy en dernier (il lui faut peu).
    let mut order: Vec<usize> = (0..t.servers.len()).collect();
    order.sort_by_key(|i| t.servers[*i].role == "lobby");
    for i in &order {
        wants.push((t.servers[*i].min, t.servers[*i].comfort));
    }
    if t.proxy {
        wants.push((PROXY_MIN, PROXY_COMFORT));
    }
    let free = cap.filter(|c| !c.pro).map(|c| (c.cpu.remaining.max(0), c.ram.remaining.max(0)));
    let (alloc, warning) = allocate(&wants, free);
    let res_of = |i: usize| alloc[order.iter().position(|x| *x == i).unwrap_or(0)];
    let mut notes: Vec<String> = Vec::new();
    let mut servers = Vec::new();
    for (i, s) in t.servers.iter().enumerate() {
        let (cpu, ram) = res_of(i);
        let mut props = Map::new();
        for (k, v) in s.properties {
            props.insert((*k).into(), match *v {
                "true" => json!(true),
                "false" => json!(false),
                other => json!(other),
            });
        }
        let content: Vec<Value> = s.plugins.iter().map(|p| json!({ "kind": "plugin", "source": "modrinth", "project": p })).collect();
        let mut manifest = json!({ "properties": props });
        if !content.is_empty() {
            manifest["content"] = json!(content);
        }
        let mut v = json!({ "name": s.name, "role": s.role, "platform": platform, "cpu": cpu, "ram": ram, "manifest": manifest });
        if let Some(ver) = version {
            v["version"] = json!(ver);
        }
        if let Some(r) = s.pregen {
            v["pregenerate"] = json!({ "radius": r });
        }
        servers.push(v);
    }
    let mut bp = json!({
        "version": BLUEPRINT_VERSION,
        "name": t.description.trim_end_matches('.'),
        "mybox_id": mybox_id,
        "servers": servers,
    });
    if t.proxy {
        let (cpu, ram) = alloc[alloc.len() - 1];
        let lobby = t.servers.iter().find(|s| s.role == "lobby").map(|s| s.name).unwrap_or(t.servers[0].name);
        let mut proxy = json!({ "name": "Proxy", "platform": "velocity", "cpu": cpu, "ram": ram, "mode": "modern", "try": [lobby], "prune": true });
        if let Some(d) = domain {
            let mut hosts = Map::new();
            hosts.insert(crate::network::valid_host(d)?, json!(lobby));
            proxy["forced_hosts"] = Value::Object(hosts);
        }
        bp["proxy"] = proxy;
        notes.push("préréglages possibles : `presets: [\"luckperms\"]` avec `database: { database_id }` (base de la MyBox, mybox_info) pour des permissions partagées ; `\"geyser\"` (et `proxy.bedrock_port`) pour accueillir Bedrock".into());
    } else if domain.is_some() {
        notes.push("`domain` ignoré : sans proxy, le domaine se règle dans la zone DNS (enregistrement vers l'adresse du serveur)".into());
    }
    if version.is_none() {
        notes.push("`version` absente : chaque serveur garde la version de Minecraft installée par l'hébergeur ; précise-la (celle des clients) pour la fixer".into());
    }
    notes.push("plugins proposés par slug Modrinth, sans version imposée (la plus récente compatible est installée) ; Chunky est ajouté automatiquement là où `pregenerate` est demandé".into());
    notes.push("serveurs désignés par leur nom : ceux qui existent déjà dans la MyBox sous ce nom sont repris, les autres créés".into());
    if let Some(w) = &warning {
        notes.push(w.clone());
    }
    let mut allocated = Map::new();
    for (i, s) in t.servers.iter().enumerate() {
        let (cpu, ram) = res_of(i);
        allocated.insert(s.name.into(), json!({ "cpu": cpu, "ram": ram }));
    }
    if t.proxy {
        let (cpu, ram) = alloc[alloc.len() - 1];
        allocated.insert("Proxy".into(), json!({ "cpu": cpu, "ram": ram }));
    }
    let total = alloc.iter().fold((0, 0), |(c, r), (x, y)| (c + x, r + y));
    let mut resources = json!({
        "allocated": allocated,
        "total": { "cpu": total.0, "ram": total.1 },
        "units": "CPU en cœurs entiers, RAM en Go entiers",
    });
    match cap {
        Some(c) if c.pro => resources["mybox"] = json!({ "id": c.id, "pro": true }),
        Some(c) => resources["mybox"] = json!({ "id": c.id, "name": c.name, "remaining": { "cpu": c.cpu.remaining, "ram": c.ram.remaining } }),
        None => {}
    }
    if let Some(w) = warning {
        resources["warning"] = json!(w);
    }
    Ok((bp, resources, notes))
}

/// Outil MCP `network_template`.
pub(crate) async fn network_template_tool(core: &Core, args: &Value) -> Result<String, String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let Some(name) = text("template") else {
        let list: Vec<Value> = TEMPLATES
            .iter()
            .map(|t| json!({ "template": t.name, "description": t.description, "proxy": t.proxy, "servers": t.servers.iter().map(|s| s.name).collect::<Vec<_>>() }))
            .collect();
        return Ok(crate::config_edit::render_for_ai(core, json!({ "templates": list, "next": "network_template avec `template` et `mybox_id` (ressources réparties selon sa place libre)" })));
    };
    let mybox_id = args
        .get("mybox_id")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .ok_or("paramètre `mybox_id` requis : MyBox du réseau (list_servers, mybox_capacity)")?;
    let cap = core.mybox_capacity(mybox_id).await.map_err(|e| format!("MyBox {mybox_id} illisible : {e}"))?;
    let version = text("version");
    if let Some(v) = &version {
        if v.len() > 32 || !v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_')) {
            return Err(format!("`version` « {v} » illisible (ex. 1.21.4)"));
        }
    }
    let (bp, resources, notes) = build_template(&name, mybox_id, Some(&cap), version.as_deref(), text("platform").as_deref(), text("domain").as_deref())?;
    let out = json!({
        "template": name,
        "blueprint": bp,
        "resources": resources,
        "notes": notes,
        "next": "Ajuste le blueprint (noms, plugins, mondes `world`, dossiers `sync`, préréglages), puis plan_network ; après accord de l'utilisateur, apply_network avec dry_run=false.",
    });
    Ok(crate::config_edit::render_for_ai(core, out))
}
