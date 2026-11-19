//! **Ressources MCP** (`resources/list`, `resources/templates/list`, `resources/read`) : lecture
//! contextuelle sans appel d'outil. Chaque ressource **est** un outil de lecture existant, appelé
//! par le même chemin que `tools/call` (`mcp::call`) : mêmes gardes (activation, liste blanche
//! d'env, portée du Copilote, groupes d'outils), même anonymisation, même cache.
//!
//! - `minestrator://servers` → `list_servers`
//! - `minestrator://server/{server_id}/status` → `server_status`
//! - `minestrator://server/{server_id}/console` → `read_console` (dernières lignes, anonymisées)
//! - `minestrator://server/{server_id}/properties` → `read_properties` (secrets masqués)
//! - `minestrator://server/{server_id}/file/{+path}` → `read_file` (mêmes protections)
//!
//! Pas d'abonnement (`subscribe: false`) : un client relit la ressource quand il en a besoin.

use crate::Core;
use serde_json::{json, Value};

const SCHEME: &str = "minestrator://";

/// Une ressource résolue : l'outil de lecture qui la sert et ses arguments.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub uri: String,
    pub tool: &'static str,
    pub args: Value,
    pub mime: &'static str,
}

/// Modèles d'URI : (modèle, nom, titre, description, type MIME, outil).
const TEMPLATES: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "minestrator://server/{server_id}/status",
        "server-status",
        "État d'un serveur",
        "État temps réel (comme server_status) : état, consommation, joueurs, version.",
        "application/json",
        "server_status",
    ),
    (
        "minestrator://server/{server_id}/console",
        "server-console",
        "Console d'un serveur",
        "Dernières lignes de la console (comme read_console), anonymisées.",
        "text/plain",
        "read_console",
    ),
    (
        "minestrator://server/{server_id}/properties",
        "server-properties",
        "Réglages d'un serveur",
        "server.properties via l'API (comme read_properties), secrets masqués.",
        "application/json",
        "read_properties",
    ),
    (
        "minestrator://server/{server_id}/file/{+path}",
        "server-file",
        "Fichier d'un serveur",
        "Fichier texte lu par SFTP (comme read_file : 2 Mo au plus, secrets masqués). Chemin depuis la racine du serveur, ex. minestrator://server/12/file/config/paper-global.yml.",
        "text/plain",
        "read_file",
    ),
];

/// La ressource est-elle servie dans ce processus (outil autorisé et dans un groupe actif) ?
fn available(core: &Core, tool: &str) -> bool {
    crate::mcp::env_allows(tool)
        && crate::mcp_catalog::tool_in_toolsets(
            crate::mcp_catalog::active_toolsets(core).as_ref(),
            tool,
        )
}

/// `resources/list` : les ressources sans paramètre.
pub(crate) fn list(core: &Core, enabled: bool) -> Value {
    let mut resources = Vec::new();
    if enabled && available(core, "list_servers") {
        resources.push(json!({
            "uri": format!("{SCHEME}servers"),
            "name": "servers",
            "title": "Serveurs du compte",
            "description": "Liste des serveurs (comme list_servers) : id, nom, statut, adresse, MyBox.",
            "mimeType": "application/json",
        }));
    }
    json!({ "resources": resources })
}

/// `resources/templates/list` : les ressources paramétrées par serveur.
pub(crate) fn templates(core: &Core, enabled: bool) -> Value {
    let list: Vec<Value> = TEMPLATES
        .iter()
        .filter(|t| enabled && available(core, t.5))
        .map(|(uri, name, title, description, mime, _)| {
            json!({ "uriTemplate": uri, "name": name, "title": title, "description": description, "mimeType": mime })
        })
        .collect();
    json!({ "resourceTemplates": list })
}

/// Décode les `%XX` d'un segment d'URI (UTF-8 exigé, octet nul refusé).
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).ok()?;
    (!text.contains('\0')).then_some(text)
}

/// URI → outil et arguments. `None` : ressource inconnue.
pub(crate) fn resolve(uri: &str) -> Option<Target> {
    let rest = uri.strip_prefix(SCHEME)?;
    let target = |tool, args, mime| {
        Some(Target {
            uri: uri.to_string(),
            tool,
            args,
            mime,
        })
    };
    if rest == "servers" {
        return target("list_servers", json!({}), "application/json");
    }
    let rest = rest.strip_prefix("server/")?;
    let (id, what) = rest.split_once('/')?;
    let id: i64 = id.parse().ok().filter(|n| *n > 0)?;
    match what {
        "status" => target(
            "server_status",
            json!({ "server_id": id }),
            "application/json",
        ),
        "console" => target("read_console", json!({ "server_id": id }), "text/plain"),
        "properties" => target(
            "read_properties",
            json!({ "server_id": id }),
            "application/json",
        ),
        _ => {
            let raw = what.strip_prefix("file/")?;
            let path = percent_decode(raw)?;
            let path = path.trim_start_matches('/');
            if path.is_empty() {
                return None;
            }
            target(
                "read_file",
                json!({ "server_id": id, "path": format!("/{path}") }),
                "text/plain",
            )
        }
    }
}

/// Code d'erreur « ressource introuvable » : `-32602` dans l'ère sans état, `-32002` avant.
pub(crate) fn not_found_code() -> i64 {
    if crate::mcp_ctx::stateless() {
        -32602
    } else {
        -32002
    }
}

/// Résultat de `resources/read` d'après la sortie de l'outil (`tools/call`).
pub(crate) fn render(target: &Target, call_output: &Value) -> Result<Value, (i64, String)> {
    let text = call_output
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if call_output.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err((-32603, text));
    }
    Ok(json!({ "contents": [{ "uri": target.uri, "mimeType": target.mime, "text": text }] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_map_to_read_tools() {
        let t = resolve("minestrator://servers").unwrap();
        assert_eq!((t.tool, t.args.clone()), ("list_servers", json!({})));
        let t = resolve("minestrator://server/42/console").unwrap();
        assert_eq!((t.tool, t.mime), ("read_console", "text/plain"));
        assert_eq!(t.args, json!({ "server_id": 42 }));
        assert_eq!(
            resolve("minestrator://server/42/status").unwrap().tool,
            "server_status"
        );
        assert_eq!(
            resolve("minestrator://server/42/properties").unwrap().tool,
            "read_properties"
        );
        let f = resolve("minestrator://server/42/file/config/paper-global.yml").unwrap();
        assert_eq!(
            f.args,
            json!({ "server_id": 42, "path": "/config/paper-global.yml" })
        );
        let f = resolve("minestrator://server/42/file/%2Fplugins%2FLuckPerms%2Fconfig%20a.yml")
            .unwrap();
        assert_eq!(f.args["path"], "/plugins/LuckPerms/config a.yml");
        for t in TEMPLATES {
            assert!(
                crate::mcp::READ_TOOLS.contains(&t.5),
                "{} : uniquement des lectures",
                t.5
            );
        }
    }

    #[test]
    fn unknown_or_malformed_uris_are_not_found() {
        for uri in [
            "https://example.com",
            "minestrator://server/abc/status",
            "minestrator://server/-1/status",
            "minestrator://server/42/delete",
            "minestrator://server/42/file/",
            "minestrator://server/42/file/%ZZ",
            "minestrator://server/42/file/a%00b",
            "minestrator://server/42",
        ] {
            assert!(resolve(uri).is_none(), "{uri}");
        }
        assert_eq!(not_found_code(), -32002, "hors ère sans état");
    }

    #[test]
    fn tool_output_becomes_resource_contents() {
        let t = resolve("minestrator://server/1/console").unwrap();
        let ok = render(
            &t,
            &json!({ "content": [{ "type": "text", "text": "[12:00] Done" }] }),
        )
        .unwrap();
        assert_eq!(
            ok["contents"][0],
            json!({ "uri": "minestrator://server/1/console", "mimeType": "text/plain", "text": "[12:00] Done" })
        );
        let err = render(&t, &json!({ "content": [{ "type": "text", "text": "Outil non autorisé" }], "isError": true })).unwrap_err();
        assert_eq!(err, (-32603, "Outil non autorisé".to_string()));
    }
}
