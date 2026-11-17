//! Couche LLM **multi-fournisseur** du Copilote. Deux adaptateurs couvrent l'essentiel du
//! marché :
//!
//! - [`Provider::Anthropic`] : API *Messages* native (Claude).
//! - [`Provider::OpenaiCompatible`] : API *Chat Completions* — couvre **OpenAI (GPT)**,
//!   **Google Gemini** (endpoint compatible), **Mistral**, **Groq**, **xAI**, **DeepSeek**,
//!   **OpenRouter**, et les serveurs **locaux** (Ollama, LM Studio, vLLM…). Il suffit de
//!   régler l'URL de base + le modèle.
//!
//! L'agent parle un **format normalisé** ([`ToolSpec`], [`Msg`], [`ToolCall`], [`ToolResult`]) ;
//! chaque adaptateur (dé)sérialise vers/depuis le format natif du fournisseur. Ajouter un
//! nouveau fournisseur = ajouter un variant + son (dé)sérialiseur, sans toucher à l'agent.

use crate::config::ANTHROPIC_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

const MAX_TOKENS: u32 = 1600;

/// Texte de remplacement d'un tour assistant vide (ni texte ni appel d'outil) : les deux API
/// refusent un message assistant sans contenu.
const EMPTY_ASSISTANT_TURN: &str = "(réponse vide)";

/// Client HTTP **partagé** (pool de connexions + TLS réutilisés) pour tous les appels LLM — évite
/// de reconstruire un `reqwest::Client` à chaque message de chat / diagnostic.
static LLM_HTTP: LazyLock<reqwest::Client> = LazyLock::new(crate::http::client);

/// Fournisseur LLM. Le transport `OpenaiCompatible` est volontairement générique
/// (URL de base configurable) pour couvrir tout service exposant l'API OpenAI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "LlmProvider"))]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenaiCompatible,
    /// Agent CLI local (Claude Code, Gemini CLI…) : ne passe PAS par le client HTTP,
    /// géré à part par `crate::cli` (utilise l'abonnement/l'auth locale, sans clé API).
    LocalCli,
}

impl Provider {
    /// Slug stable (clé de trousseau, logs).
    pub fn slug(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenaiCompatible => "openai",
            Provider::LocalCli => "cli",
        }
    }
    /// URL de base par défaut si l'utilisateur n'en fournit pas.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Provider::Anthropic => "https://api.anthropic.com/v1",
            Provider::OpenaiCompatible => "https://api.openai.com/v1",
            Provider::LocalCli => "",
        }
    }
    /// Le fournisseur exige-t-il une clé API ? (Faux pour le local type Ollama ou CLI.)
    pub fn requires_key(self) -> bool {
        matches!(self, Provider::Anthropic)
    }
}

// --- Format normalisé ------------------------------------------------------

/// Spécification d'un outil exposé au modèle (JSON Schema d'entrée).
#[derive(Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Appel d'outil émis par le modèle. `id` est l'identifiant natif à ré-émettre.
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Résultat d'un outil renvoyé au modèle.
pub struct ToolResult {
    pub id: String,
    pub content: String,
}

/// Un tour de conversation normalisé.
pub enum Msg {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    ToolResults(Vec<ToolResult>),
}

/// Réponse normalisée d'un tour de modèle.
pub struct LlmResponse {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

// --- Client ----------------------------------------------------------------

/// Client LLM prêt à dialoguer, indépendant du fournisseur pour l'appelant.
pub struct LlmClient {
    http: reqwest::Client,
    provider: Provider,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn new(provider: Provider, base_url: &str, api_key: &str, model: &str) -> Self {
        let base = base_url.trim().trim_end_matches('/');
        let base_url = if base.is_empty() {
            provider.default_base_url().to_string()
        } else {
            base.to_string()
        };
        Self {
            http: LLM_HTTP.clone(),
            provider,
            base_url,
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
        }
    }

    /// Un tour : envoie `system` + `tools` + le `transcript` normalisé, renvoie la réponse.
    pub async fn complete(
        &self,
        system: &str,
        tools: &[ToolSpec],
        transcript: &[Msg],
    ) -> Result<LlmResponse, String> {
        let (path, body) = match self.provider {
            Provider::Anthropic => ("/messages", self.build_anthropic(system, tools, transcript)),
            Provider::OpenaiCompatible => (
                "/chat/completions",
                self.build_openai(system, tools, transcript),
            ),
            Provider::LocalCli => {
                return Err("Le fournisseur « CLI locale » ne passe pas par le client HTTP.".into())
            }
        };

        let mut req = self
            .http
            .post(format!("{}{path}", self.base_url))
            .header("content-type", "application/json");
        req = match self.provider {
            Provider::Anthropic => req
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            _ => {
                if self.api_key.is_empty() {
                    req
                } else {
                    req.header("authorization", format!("Bearer {}", self.api_key))
                }
            }
        };

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("appel LLM ({}) : {e}", self.provider.slug()))?;
        let status = resp.status();
        let val: Value = resp
            .json()
            .await
            .map_err(|e| format!("réponse LLM illisible : {e}"))?;

        if !status.is_success() {
            let msg = error_message(&val);
            return Err(format!("API LLM ({}) : {msg}", status.as_u16()));
        }

        Ok(match self.provider {
            Provider::Anthropic => parse_anthropic(&val),
            _ => parse_openai(&val),
        })
    }

    // --- Anthropic (Messages API) -----------------------------------------

    fn build_anthropic(&self, system: &str, tools: &[ToolSpec], transcript: &[Msg]) -> Value {
        let messages: Vec<Value> = transcript
            .iter()
            .map(|m| match m {
                Msg::User(t) => json!({ "role": "user", "content": t }),
                Msg::Assistant { text, calls } => {
                    let mut content = Vec::new();
                    if !text.is_empty() {
                        content.push(json!({ "type": "text", "text": text }));
                    }
                    for c in calls {
                        content.push(json!({
                            "type": "tool_use", "id": c.id, "name": c.name, "input": c.input
                        }));
                    }
                    // Tour vide (le modèle n'a rien répondu) : l'API rejette `content: []`, ce qui
                    // casserait la conversation à chaque tour suivant.
                    if content.is_empty() {
                        content.push(json!({ "type": "text", "text": EMPTY_ASSISTANT_TURN }));
                    }
                    json!({ "role": "assistant", "content": content })
                }
                Msg::ToolResults(rs) => {
                    let content: Vec<Value> = rs
                        .iter()
                        .map(|r| {
                            json!({ "type": "tool_result", "tool_use_id": r.id, "content": r.content })
                        })
                        .collect();
                    json!({ "role": "user", "content": content })
                }
            })
            .collect();

        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
            .collect();

        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "tools": tools_json,
            "messages": messages,
        })
    }

    // --- OpenAI-compatible (Chat Completions) -----------------------------

    fn build_openai(&self, system: &str, tools: &[ToolSpec], transcript: &[Msg]) -> Value {
        let mut messages = vec![json!({ "role": "system", "content": system })];
        for m in transcript {
            match m {
                Msg::User(t) => messages.push(json!({ "role": "user", "content": t })),
                Msg::Assistant { text, calls } => {
                    let tool_calls: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": { "name": c.name, "arguments": c.input.to_string() }
                            })
                        })
                        .collect();
                    let mut msg = serde_json::Map::new();
                    msg.insert("role".into(), json!("assistant"));
                    // `null` n'est admis qu'avec des `tool_calls` ; un tour entièrement vide reçoit
                    // un texte de remplacement (sinon requête rejetée à chaque tour suivant).
                    let content = match (text.is_empty(), tool_calls.is_empty()) {
                        (false, _) => json!(text),
                        (true, false) => Value::Null,
                        (true, true) => json!(EMPTY_ASSISTANT_TURN),
                    };
                    msg.insert("content".into(), content);
                    if !tool_calls.is_empty() {
                        msg.insert("tool_calls".into(), json!(tool_calls));
                    }
                    messages.push(Value::Object(msg));
                }
                Msg::ToolResults(rs) => {
                    for r in rs {
                        messages.push(json!({
                            "role": "tool", "tool_call_id": r.id, "content": r.content
                        }));
                    }
                }
            }
        }

        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.schema,
                    }
                })
            })
            .collect();

        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": messages,
            "tools": tools_json,
        })
    }
}

// --- Désérialisation des réponses ------------------------------------------

/// Message lisible d'une réponse d'erreur, quel que soit le fournisseur : `{error: {message}}`
/// (Anthropic, OpenAI) ou `{error: "…"}` (Ollama et d'autres serveurs compatibles).
fn error_message(val: &Value) -> &str {
    val.pointer("/error/message")
        .or_else(|| val.pointer("/error"))
        .and_then(|m| m.as_str())
        .unwrap_or("erreur inconnue")
}

fn parse_anthropic(val: &Value) -> LlmResponse {
    let mut text = String::new();
    let mut calls = Vec::new();
    if let Some(content) = val.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                Some("tool_use") => calls.push(ToolCall {
                    id: block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                }),
                _ => {}
            }
        }
    }
    LlmResponse { text, calls }
}

fn parse_openai(val: &Value) -> LlmResponse {
    // Emprunt (pas de `.cloned()`) : on ne fait que lire dans le message, potentiellement volumineux.
    let msg = val.pointer("/choices/0/message");
    let text = msg
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    let mut calls = Vec::new();
    if let Some(tcs) = msg.and_then(|m| m.get("tool_calls")).and_then(|t| t.as_array()) {
        for tc in tcs {
            let args = tc
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            calls.push(ToolCall {
                id: tc.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string(),
                name: tc
                    .pointer("/function/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string(),
                input: serde_json::from_str(args).unwrap_or_else(|_| json!({})),
            });
        }
    }
    LlmResponse { text, calls }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(provider: Provider) -> LlmClient {
        LlmClient::new(provider, "", "clé-test", "modele-test")
    }

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("outil {name}"),
            schema: json!({
                "type": "object",
                "properties": { "server_id": { "type": "integer" } },
                "required": ["server_id"]
            }),
        }
    }

    fn call(id: &str, name: &str, input: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    fn result(id: &str, content: &str) -> ToolResult {
        ToolResult {
            id: id.into(),
            content: content.into(),
        }
    }

    /// Conversation multi-tours typique : question → 2 appels d'outils → résultats → réponse → relance.
    fn multi_turn() -> Vec<Msg> {
        vec![
            Msg::User("Pourquoi le serveur lag ?".into()),
            Msg::Assistant {
                text: "Je regarde la console.".into(),
                calls: vec![
                    call("call_1", "read_console", json!({ "server_id": 42 })),
                    call("call_2", "server_metrics", json!({ "server_id": 42, "range": "1h" })),
                ],
            },
            Msg::ToolResults(vec![
                result("call_1", "[12:00 WARN] Can't keep up!"),
                result("call_2", "cpu 95 %"),
            ]),
            Msg::Assistant {
                text: "Le CPU sature.".into(),
                calls: Vec::new(),
            },
            Msg::User("Que faire ?".into()),
        ]
    }

    // --- Fournisseur / client ----------------------------------------------------

    #[test]
    fn provider_serde_slugs_and_defaults() {
        assert_eq!(serde_json::to_value(Provider::Anthropic).unwrap(), json!("anthropic"));
        assert_eq!(
            serde_json::to_value(Provider::OpenaiCompatible).unwrap(),
            json!("openai_compatible")
        );
        assert_eq!(serde_json::to_value(Provider::LocalCli).unwrap(), json!("local_cli"));
        let back: Provider = serde_json::from_value(json!("openai_compatible")).unwrap();
        assert_eq!(back, Provider::OpenaiCompatible);
        assert!(serde_json::from_value::<Provider>(json!("OpenAI")).is_err());

        assert_eq!(Provider::Anthropic.slug(), "anthropic");
        assert_eq!(Provider::OpenaiCompatible.slug(), "openai");
        assert_eq!(Provider::LocalCli.slug(), "cli");

        assert_eq!(Provider::Anthropic.default_base_url(), "https://api.anthropic.com/v1");
        assert_eq!(Provider::OpenaiCompatible.default_base_url(), "https://api.openai.com/v1");
        assert_eq!(Provider::LocalCli.default_base_url(), "");

        // Seul Anthropic exige une clé : un serveur local (Ollama…) n'en a pas.
        assert!(Provider::Anthropic.requires_key());
        assert!(!Provider::OpenaiCompatible.requires_key());
        assert!(!Provider::LocalCli.requires_key());
    }

    #[test]
    fn client_normalizes_base_url_key_and_model() {
        let c = LlmClient::new(
            Provider::OpenaiCompatible,
            "  http://localhost:11434/v1/  ",
            " sk-abc \n",
            " llama3.1 ",
        );
        assert_eq!(c.base_url, "http://localhost:11434/v1");
        assert_eq!(c.api_key, "sk-abc");
        assert_eq!(c.model, "llama3.1");

        // URL vide (ou faite d'espaces / de barres) → URL par défaut du fournisseur.
        assert_eq!(client(Provider::Anthropic).base_url, "https://api.anthropic.com/v1");
        let c = LlmClient::new(Provider::OpenaiCompatible, "   ", "", "m");
        assert_eq!(c.base_url, "https://api.openai.com/v1");
        let c = LlmClient::new(Provider::OpenaiCompatible, "///", "", "m");
        assert_eq!(c.base_url, "https://api.openai.com/v1");
    }

    #[tokio::test]
    async fn local_cli_never_goes_through_http() {
        let err = client(Provider::LocalCli)
            .complete("sys", &[tool("read_console")], &[Msg::User("salut".into())])
            .await
            .err()
            .expect("le fournisseur CLI doit être refusé par le client HTTP");
        assert!(err.contains("CLI locale"), "{err}");
    }

    // --- Anthropic : sérialisation ------------------------------------------------

    #[test]
    fn anthropic_request_envelope() {
        let body = client(Provider::Anthropic).build_anthropic(
            "Tu es le Copilote.",
            &[tool("read_console"), tool("power_action")],
            &[Msg::User("salut".into())],
        );
        assert_eq!(body["model"], "modele-test");
        assert_eq!(body["max_tokens"], MAX_TOKENS);
        // Anthropic : le prompt système est un champ de premier niveau, pas un message.
        assert_eq!(body["system"], "Tu es le Copilote.");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(
            tools[0],
            json!({
                "name": "read_console",
                "description": "outil read_console",
                "input_schema": tool("read_console").schema,
            })
        );
        assert_eq!(tools[1]["name"], "power_action");
    }

    #[test]
    fn anthropic_multi_turn_transcript() {
        let body = client(Provider::Anthropic).build_anthropic("sys", &[], &multi_turn());
        let msgs = body["messages"].as_array().unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        // Les résultats d'outils voyagent dans un message `user` : les rôles alternent strictement.
        assert_eq!(roles, ["user", "assistant", "user", "assistant", "user"]);

        // Message utilisateur : contenu texte brut.
        assert_eq!(msgs[0], json!({ "role": "user", "content": "Pourquoi le serveur lag ?" }));

        // Tour assistant : bloc texte PUIS blocs tool_use, entrée JSON conservée telle quelle.
        assert_eq!(
            msgs[1]["content"],
            json!([
                { "type": "text", "text": "Je regarde la console." },
                { "type": "tool_use", "id": "call_1", "name": "read_console", "input": { "server_id": 42 } },
                {
                    "type": "tool_use", "id": "call_2", "name": "server_metrics",
                    "input": { "server_id": 42, "range": "1h" }
                },
            ])
        );

        // Résultats : un bloc tool_result par appel, corrélé par `tool_use_id`, dans l'ordre.
        assert_eq!(
            msgs[2]["content"],
            json!([
                { "type": "tool_result", "tool_use_id": "call_1", "content": "[12:00 WARN] Can't keep up!" },
                { "type": "tool_result", "tool_use_id": "call_2", "content": "cpu 95 %" },
            ])
        );

        // Réponse finale sans outil : un seul bloc texte.
        assert_eq!(msgs[3]["content"], json!([{ "type": "text", "text": "Le CPU sature." }]));
        assert_eq!(msgs[4]["content"], "Que faire ?");
    }

    #[test]
    fn anthropic_assistant_without_text_only_has_tool_use() {
        let body = client(Provider::Anthropic).build_anthropic(
            "sys",
            &[],
            &[
                Msg::User("q".into()),
                Msg::Assistant {
                    text: String::new(),
                    calls: vec![call("toolu_1", "list_files", json!({ "path": "/" }))],
                },
            ],
        );
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "aucun bloc texte vide ne doit être émis");
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["input"], json!({ "path": "/" }));
    }

    #[test]
    fn anthropic_empty_assistant_turn_is_not_sent_empty() {
        let body = client(Provider::Anthropic).build_anthropic(
            "sys",
            &[],
            &[
                Msg::User("q".into()),
                Msg::Assistant {
                    text: String::new(),
                    calls: Vec::new(),
                },
                Msg::User("relance".into()),
            ],
        );
        for m in body["messages"].as_array().unwrap() {
            assert_ne!(m["content"], json!([]), "message au contenu vide : {m}");
        }
    }

    // --- OpenAI-compatible : sérialisation ----------------------------------------

    #[test]
    fn openai_request_envelope() {
        let body = client(Provider::OpenaiCompatible).build_openai(
            "Tu es le Copilote.",
            &[tool("read_console")],
            &[Msg::User("salut".into())],
        );
        assert_eq!(body["model"], "modele-test");
        assert_eq!(body["max_tokens"], MAX_TOKENS);
        // Chat Completions : le prompt système est le PREMIER message, pas un champ dédié.
        assert!(body.get("system").is_none());
        assert_eq!(
            body["messages"][0],
            json!({ "role": "system", "content": "Tu es le Copilote." })
        );
        assert_eq!(body["messages"][1], json!({ "role": "user", "content": "salut" }));
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": {
                    "name": "read_console",
                    "description": "outil read_console",
                    "parameters": tool("read_console").schema,
                }
            }])
        );
    }

    #[test]
    fn openai_multi_turn_transcript() {
        let body = client(Provider::OpenaiCompatible).build_openai("sys", &[], &multi_turn());
        let msgs = body["messages"].as_array().unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        // Un message `tool` PAR résultat (pas un message groupé comme chez Anthropic).
        assert_eq!(roles, ["system", "user", "assistant", "tool", "tool", "assistant", "user"]);

        let assistant = &msgs[2];
        assert_eq!(assistant["content"], "Je regarde la console.");
        let calls = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "read_console");
        // Les arguments voyagent en CHAÎNE JSON (format OpenAI), qui se relit en l'entrée d'origine.
        let args = calls[1]["function"]["arguments"].as_str().expect("arguments en chaîne");
        assert_eq!(
            serde_json::from_str::<Value>(args).unwrap(),
            json!({ "server_id": 42, "range": "1h" })
        );

        assert_eq!(
            msgs[3],
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "[12:00 WARN] Can't keep up!" })
        );
        assert_eq!(msgs[4]["tool_call_id"], "call_2");
        assert_eq!(msgs[4]["content"], "cpu 95 %");

        // Tour assistant sans appel : pas de clé `tool_calls` du tout (plutôt qu'un tableau vide).
        assert_eq!(msgs[5], json!({ "role": "assistant", "content": "Le CPU sature." }));
        assert_eq!(msgs[6], json!({ "role": "user", "content": "Que faire ?" }));
    }

    #[test]
    fn openai_assistant_with_only_calls_has_null_content() {
        let body = client(Provider::OpenaiCompatible).build_openai(
            "sys",
            &[],
            &[
                Msg::User("q".into()),
                Msg::Assistant {
                    text: String::new(),
                    calls: vec![call("c1", "list_files", json!({}))],
                },
            ],
        );
        let assistant = &body["messages"][2];
        assert!(assistant["content"].is_null());
        assert_eq!(assistant["tool_calls"][0]["function"]["arguments"], "{}");
    }

    #[test]
    fn openai_empty_assistant_turn_has_content() {
        let body = client(Provider::OpenaiCompatible).build_openai(
            "sys",
            &[],
            &[
                Msg::User("q".into()),
                Msg::Assistant {
                    text: String::new(),
                    calls: Vec::new(),
                },
                Msg::User("relance".into()),
            ],
        );
        for m in body["messages"].as_array().unwrap() {
            assert!(
                !m["content"].is_null() || m.get("tool_calls").is_some(),
                "message assistant sans contenu ni appel : {m}"
            );
        }
    }

    // --- Désérialisation des réponses ---------------------------------------------

    #[test]
    fn parse_anthropic_text_and_tool_calls() {
        let resp = parse_anthropic(&json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [
                { "type": "thinking", "thinking": "réflexion interne", "signature": "x" },
                { "type": "text", "text": "Je vais " },
                { "type": "text", "text": "vérifier." },
                { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "path": "/server.properties" } },
                { "type": "tool_use", "id": "toolu_2", "name": "server_status" }
            ]
        }));
        // Les blocs texte sont concaténés ; les blocs inconnus (thinking…) ignorés.
        assert_eq!(resp.text, "Je vais vérifier.");
        assert_eq!(resp.calls.len(), 2);
        assert_eq!(resp.calls[0].id, "toolu_1");
        assert_eq!(resp.calls[0].name, "read_file");
        assert_eq!(resp.calls[0].input, json!({ "path": "/server.properties" }));
        // `input` absent → objet vide (jamais `null`).
        assert_eq!(resp.calls[1].name, "server_status");
        assert_eq!(resp.calls[1].input, json!({}));
    }

    #[test]
    fn parse_anthropic_without_content_is_empty() {
        for val in [json!({}), json!({ "content": null }), json!({ "content": "pas un tableau" })] {
            let resp = parse_anthropic(&val);
            assert!(resp.text.is_empty());
            assert!(resp.calls.is_empty());
        }
    }

    #[test]
    fn parse_openai_text_and_tool_calls() {
        let resp = parse_openai(&json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": "Je vérifie.",
                    "tool_calls": [
                        { "id": "call_a", "type": "function",
                          "function": { "name": "read_console", "arguments": "{\"server_id\":42,\"lines\":50}" } },
                        { "id": "call_b", "type": "function",
                          "function": { "name": "broken", "arguments": "{pas du json" } },
                        { "id": "call_c", "type": "function", "function": { "name": "no_args" } }
                    ]
                }
            }]
        }));
        assert_eq!(resp.text, "Je vérifie.");
        assert_eq!(resp.calls.len(), 3);
        assert_eq!(resp.calls[0].id, "call_a");
        assert_eq!(resp.calls[0].name, "read_console");
        assert_eq!(resp.calls[0].input, json!({ "server_id": 42, "lines": 50 }));
        // Arguments illisibles ou absents → objet vide, l'appel est quand même conservé.
        assert_eq!(resp.calls[1].name, "broken");
        assert_eq!(resp.calls[1].input, json!({}));
        assert_eq!(resp.calls[2].input, json!({}));
    }

    #[test]
    fn parse_openai_null_content_and_missing_choices() {
        let resp = parse_openai(&json!({
            "choices": [{ "message": { "role": "assistant", "content": null,
                "tool_calls": [{ "id": "c", "function": { "name": "x", "arguments": "{}" } }] } }]
        }));
        assert_eq!(resp.text, "");
        assert_eq!(resp.calls.len(), 1);

        for val in [json!({}), json!({ "choices": [] }), json!({ "choices": [{}] })] {
            let resp = parse_openai(&val);
            assert!(resp.text.is_empty());
            assert!(resp.calls.is_empty());
        }
    }

    /// Ce que le modèle a émis doit se ré-émettre à l'identique au tour suivant (ids, noms,
    /// entrées) : sinon le fournisseur ne peut plus corréler les résultats d'outils.
    #[test]
    fn tool_calls_round_trip_through_the_normalized_format() {
        let anthropic_block = json!({
            "type": "tool_use", "id": "toolu_9", "name": "write_file",
            "input": { "path": "/a.txt", "content": "é\n\"x\"" }
        });
        let parsed = parse_anthropic(&json!({ "content": [anthropic_block.clone()] }));
        let body = client(Provider::Anthropic).build_anthropic(
            "sys",
            &[],
            &[Msg::Assistant {
                text: parsed.text,
                calls: parsed.calls,
            }],
        );
        assert_eq!(body["messages"][0]["content"], json!([anthropic_block]));

        let openai_call = json!({
            "id": "call_9", "type": "function",
            "function": {
                "name": "write_file",
                "arguments": "{\"content\":\"é\\n\\\"x\\\"\",\"path\":\"/a.txt\"}"
            }
        });
        let parsed = parse_openai(&json!({
            "choices": [{ "message": { "content": null, "tool_calls": [openai_call] } }]
        }));
        let body = client(Provider::OpenaiCompatible).build_openai(
            "sys",
            &[],
            &[Msg::Assistant {
                text: parsed.text,
                calls: parsed.calls,
            }],
        );
        let emitted = &body["messages"][1]["tool_calls"][0];
        assert_eq!(emitted["id"], "call_9");
        assert_eq!(emitted["function"]["name"], "write_file");
        let args: Value =
            serde_json::from_str(emitted["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args, json!({ "path": "/a.txt", "content": "é\n\"x\"" }));
    }

    // --- Réponses d'erreur ---------------------------------------------------------

    #[test]
    fn error_message_handles_each_provider_format() {
        // Anthropic.
        assert_eq!(
            error_message(&json!({
                "type": "error",
                "error": { "type": "overloaded_error", "message": "Overloaded" }
            })),
            "Overloaded"
        );
        // OpenAI.
        assert_eq!(
            error_message(&json!({
                "error": {
                    "message": "Incorrect API key provided",
                    "type": "invalid_request_error",
                    "param": null,
                    "code": "invalid_api_key"
                }
            })),
            "Incorrect API key provided"
        );
        // Ollama / serveurs compatibles : erreur en chaîne directe.
        assert_eq!(
            error_message(&json!({ "error": "model \"x\" not found" })),
            "model \"x\" not found"
        );
        // Formes inattendues → message générique, jamais de panique.
        assert_eq!(error_message(&json!({ "error": { "code": 400 } })), "erreur inconnue");
        assert_eq!(error_message(&json!({ "error": { "message": 42 } })), "erreur inconnue");
        assert_eq!(error_message(&json!({ "detail": "Not Found" })), "erreur inconnue");
        assert_eq!(error_message(&json!([])), "erreur inconnue");
    }
}
