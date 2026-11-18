//! Client du **serveur MCP officiel** de MineStrator (`https://mcp.sttr.io/minestrator`).
//!
//! L'hébergeur expose **107 outils en 10 toolsets** (relevé du 2026-11-18) : `core` (15 : compte,
//! serveurs, console, tickets, diagnostic, Spark, historique), `actions` (5 : power, console, RCON,
//! joueurs, réinstallation), `config` (8 : paramètres, démarrage, Java, ports, adresse, renommage),
//! `content` (14 : versions, mods, plugins, datapacks, modpacks, mondes, Steam Workshop…),
//! `backups` (7), `mybox` (6), `schedules` (4), `databases` (4), `files` (14) et `webhosting` (30).
//! 43 d'entre eux restent disponibles en `readonly=1`. Transport **Streamable HTTP stateless** :
//! chaque appel est un POST JSON-RPC authentifié par `Authorization: Bearer <clé API>` — la même
//! clé qu'on stocke déjà au trousseau. Paramètres d'URL : `?toolsets=a,b` (sous-ensemble) et
//! `&readonly=1` (les outils modifiants n'existent alors pas pour la connexion).
//!
//! Ce module n'est que le **transport** ; la politique d'accès (ce que le Copilote ou un client du
//! MCP local peut voir et appeler, table d'équivalences avec nos outils, confirmations) vit dans
//! [`crate::official_relay`].
//!
//! Client volontairement minimal : pas de session (le serveur est stateless), pas de dépendance
//! MCP lourde — juste `reqwest` (déjà présent). Le parsing pur est isolé pour être testable.

use crate::config::{OFFICIAL_MCP_URL, USER_AGENT};
use crate::llm::ToolSpec;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    crate::http::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| crate::http::client())
});

/// Délai maximal d'un `tools/list` : le catalogue est demandé au démarrage d'un client MCP, qui
/// attend la réponse ; un officiel lent ne doit pas le bloquer (repli sur nos outils seuls).
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// URL d'appel : toolsets choisis, et `&readonly=1` pour une connexion sans aucun outil modifiant.
fn endpoint(toolsets: &str, readonly: bool) -> String {
    let ro = if readonly { "&readonly=1" } else { "" };
    format!("{OFFICIAL_MCP_URL}?toolsets={toolsets}{ro}")
}

/// Un appel JSON-RPC (stateless) au serveur officiel. Renvoie `result` ou une erreur lisible
/// (jamais la clé : elle ne voyage que dans l'en-tête).
async fn rpc(
    token: &str,
    toolsets: &str,
    readonly: bool,
    method: &str,
    params: Value,
    timeout: Option<Duration>,
) -> Result<Value, String> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut req = HTTP
        .post(endpoint(toolsets, readonly))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json, text/event-stream")
        .header("User-Agent", USER_AGENT)
        .json(&body);
    if let Some(t) = timeout {
        req = req.timeout(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("MCP officiel injoignable : {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("MCP officiel : HTTP {}", resp.status().as_u16()));
    }
    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains("text/event-stream"));
    let text = resp.text().await.map_err(|e| format!("lecture MCP officiel : {e}"))?;
    extract_result(&text, is_sse)
}

/// Extrait le `result` d'une réponse JSON-RPC — gère la réponse JSON directe et le fallback SSE
/// (on prend le 1er payload `data:`). Pur : testable sans réseau.
fn extract_result(text: &str, is_sse: bool) -> Result<Value, String> {
    let json_str = if is_sse {
        text.lines()
            .find_map(|l| l.strip_prefix("data:").map(str::trim))
            .unwrap_or(text)
    } else {
        text.trim()
    };
    let msg: Value =
        serde_json::from_str(json_str).map_err(|e| format!("MCP officiel illisible : {e}"))?;
    if let Some(err) = msg.get("error") {
        let m = err.get("message").and_then(|v| v.as_str()).unwrap_or("erreur inconnue");
        return Err(format!("MCP officiel : {m}"));
    }
    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
}

/// Définitions brutes d'un `result` de `tools/list` (celles qui ont un nom), annotations comprises.
/// Pur : testable sans réseau.
fn parse_tool_defs(result: &Value) -> Vec<Value> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|t| t.get("name").and_then(Value::as_str).is_some_and(|n| !n.is_empty()))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Mappe une définition d'outil officiel en `ToolSpec` (format LLM).
pub(crate) fn to_tool_spec(def: &Value) -> Option<ToolSpec> {
    let name = def.get("name")?.as_str()?.to_string();
    Some(ToolSpec {
        description: def
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string(),
        schema: def
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object" })),
        name,
    })
}

/// Extrait le texte d'un `result` de `tools/call` (`content[].text` concaténé ; sinon JSON brut).
/// `isError: true` (échec de l'OUTIL, distinct d'une erreur JSON-RPC) → `Err` : sans cela, l'échec
/// remontait au modèle comme un succès textuel. Pur : testable sans réseau.
fn parse_call_result(result: &Value) -> Result<String, String> {
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let text = if text.trim().is_empty() {
        result.to_string() // pas de bloc texte → JSON brut (ex. structuredContent)
    } else {
        text
    };
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(text);
    }
    Ok(text)
}

/// Catalogue brut (définitions JSON, annotations comprises) des `toolsets` demandés. `readonly`
/// restreint aux outils de lecture.
pub async fn list_tools(token: &str, toolsets: &str, readonly: bool) -> Result<Vec<Value>, String> {
    let result = rpc(token, toolsets, readonly, "tools/list", json!({}), Some(LIST_TIMEOUT)).await?;
    Ok(parse_tool_defs(&result))
}

/// Exécute un outil officiel et renvoie son texte de résultat. Avec `readonly`, la connexion n'a
/// aucun outil modifiant : un outil de lecture passe, un outil modifiant est refusé par l'officiel
/// lui-même (défense en profondeur derrière notre propre classement).
pub async fn call_tool(
    token: &str,
    toolsets: &str,
    readonly: bool,
    name: &str,
    args: Value,
) -> Result<String, String> {
    let params = json!({ "name": name, "arguments": args });
    let result = rpc(token, toolsets, readonly, "tools/call", params, None).await?;
    parse_call_result(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_result_from_json_and_sse() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert_eq!(extract_result(json, false).unwrap(), json!({ "ok": true }));
        // Fallback SSE : la charge utile est sur une ligne `data:`.
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(extract_result(sse, true).unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn surfaces_rpc_error() {
        let err = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Clé invalide"}}"#;
        assert!(extract_result(err, false).unwrap_err().contains("Clé invalide"));
    }

    #[test]
    fn maps_tools_list_to_toolspecs() {
        let result = json!({
            "tools": [
                { "name": "power_action", "description": "start/stop", "inputSchema": { "type": "object" },
                  "annotations": { "readOnlyHint": false, "destructiveHint": false } },
                { "name": "list_servers", "description": "liste" }, // inputSchema absent → défaut
                { "description": "sans nom → ignoré" }
            ]
        });
        let defs = parse_tool_defs(&result);
        assert_eq!(defs.len(), 2);
        // Les annotations sont conservées (la politique s'en sert pour les outils destructifs).
        assert_eq!(defs[0]["annotations"]["destructiveHint"], false);
        let specs: Vec<ToolSpec> = defs.iter().filter_map(to_tool_spec).collect();
        assert_eq!(specs[0].name, "power_action");
        assert_eq!(specs[1].schema, json!({ "type": "object" }));
    }

    #[test]
    fn endpoint_carries_toolsets_and_readonly() {
        assert_eq!(endpoint("core,actions", false), format!("{OFFICIAL_MCP_URL}?toolsets=core,actions"));
        assert!(endpoint("core", true).ends_with("?toolsets=core&readonly=1"));
    }

    #[test]
    fn extracts_call_text_or_raw() {
        let with_text = json!({ "content": [{ "type": "text", "text": "ligne 1" }, { "type": "text", "text": "ligne 2" }] });
        assert_eq!(parse_call_result(&with_text).unwrap(), "ligne 1\nligne 2");
        // Pas de content textuel → JSON brut renvoyé (non vide).
        let structured = json!({ "structuredContent": { "id": 42 } });
        assert!(parse_call_result(&structured).unwrap().contains("42"));
    }

    #[test]
    fn tool_failure_is_an_error_not_a_success() {
        let failed = json!({
            "isError": true,
            "content": [{ "type": "text", "text": "Serveur introuvable" }]
        });
        assert_eq!(parse_call_result(&failed).unwrap_err(), "Serveur introuvable");
        let ok = json!({ "isError": false, "content": [{ "type": "text", "text": "ok" }] });
        assert_eq!(parse_call_result(&ok).unwrap(), "ok");
    }
}
