//! Client du **serveur MCP officiel** de MineStrator (`https://mcp.sttr.io/minestrator`).
//!
//! L'hébergeur expose ~60 outils de GESTION (power, console, contenu, backups, MyBox, schedules,
//! bases de données, config Java/ports) via un transport **Streamable HTTP stateless** : chaque
//! appel est un POST JSON-RPC authentifié par `Authorization: Bearer <clé API>` — la même clé
//! qu'on stocke déjà au trousseau. On délègue la gestion à ce serveur (maintenu par l'hôte, donc
//! plus fiable) et on garde NOTRE MCP pour le SFTP fin + les outils exclusifs.
//!
//! Client volontairement minimal : pas de session (le serveur est stateless), pas de dépendance
//! MCP lourde — juste `reqwest` (déjà présent). Le parsing pur est isolé pour être testable.

use crate::config::{OFFICIAL_MCP_URL, USER_AGENT};
use crate::llm::ToolSpec;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Toolsets délégués à l'officiel : **tout sauf `files`** (on gère les fichiers en SFTP direct,
/// plus fin/stable) → aucune collision de noms avec nos outils locaux conservés.
const DELEGATED_TOOLSETS: &str = "core,actions,content,backups,mybox,schedules,databases,config";

/// `readonly` ⇒ `&readonly=1` : les outils MODIFIANTS n'existent alors pas pour la connexion (utilisé
/// pour le diagnostic auto et le chat non-autonome — impossible d'exécuter une action par mégarde).
fn endpoint(readonly: bool) -> String {
    let ro = if readonly { "&readonly=1" } else { "" };
    format!("{OFFICIAL_MCP_URL}?toolsets={DELEGATED_TOOLSETS}{ro}")
}

/// Un appel JSON-RPC (stateless) au serveur officiel. Renvoie `result` ou une erreur lisible.
async fn rpc(token: &str, readonly: bool, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp = HTTP
        .post(endpoint(readonly))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json, text/event-stream")
        .header("User-Agent", USER_AGENT)
        .json(&body)
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

/// Mappe le `result` d'un `tools/list` en `ToolSpec` (format LLM). Pur : testable sans réseau.
fn parse_tools(result: &Value) -> Vec<ToolSpec> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name")?.as_str()?.to_string();
                    Some(ToolSpec {
                        description: t
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object" })),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extrait le texte d'un `result` de `tools/call` (`content[].text` concaténé ; sinon JSON brut).
/// Pur : testable sans réseau.
fn parse_call_result(result: &Value) -> String {
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
    if text.trim().is_empty() {
        return result.to_string(); // pas de bloc texte → JSON brut (ex. structuredContent)
    }
    text
}

/// Catalogue d'outils de gestion exposé par l'officiel, mappé pour le LLM. `readonly` restreint aux
/// outils de lecture (diagnostic / chat non-autonome).
pub async fn list_tools(token: &str, readonly: bool) -> Result<Vec<ToolSpec>, String> {
    let result = rpc(token, readonly, "tools/list", json!({})).await?;
    Ok(parse_tools(&result))
}

/// Exécute un outil officiel et renvoie son texte de résultat. `readonly` doit correspondre à celui
/// du `list_tools` (même surface d'outils pour la connexion).
pub async fn call_tool(token: &str, readonly: bool, name: &str, args: Value) -> Result<String, String> {
    let result = rpc(token, readonly, "tools/call", json!({ "name": name, "arguments": args })).await?;
    Ok(parse_call_result(&result))
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
                { "name": "power_action", "description": "start/stop", "inputSchema": { "type": "object" } },
                { "name": "list_servers", "description": "liste" }, // inputSchema absent → défaut
                { "description": "sans nom → ignoré" }
            ]
        });
        let specs = parse_tools(&result);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "power_action");
        assert_eq!(specs[1].schema, json!({ "type": "object" }));
    }

    #[test]
    fn extracts_call_text_or_raw() {
        let with_text = json!({ "content": [{ "type": "text", "text": "ligne 1" }, { "type": "text", "text": "ligne 2" }] });
        assert_eq!(parse_call_result(&with_text), "ligne 1\nligne 2");
        // Pas de content textuel → JSON brut renvoyé (non vide).
        let structured = json!({ "structuredContent": { "id": 42 } });
        assert!(parse_call_result(&structured).contains("42"));
    }
}
