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

/// Client HTTP **partagé** (pool de connexions + TLS réutilisés) pour tous les appels LLM — évite
/// de reconstruire un `reqwest::Client` à chaque message de chat / diagnostic.
static LLM_HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Fournisseur LLM. Le transport `OpenaiCompatible` est volontairement générique
/// (URL de base configurable) pour couvrir tout service exposant l'API OpenAI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
            let msg = val
                .pointer("/error/message")
                .or_else(|| val.pointer("/error"))
                .and_then(|m| m.as_str())
                .unwrap_or("erreur inconnue");
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
                    msg.insert(
                        "content".into(),
                        if text.is_empty() { Value::Null } else { json!(text) },
                    );
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
