//! Export de logs / fichiers texte vers un service de paste public (mclo.gs & instances
//! compatibles, pastes.dev). Le réseau est fait côté cœur (hors CSP du webview). L'anonymisation
//! du contenu (secrets/IP/e-mails) et le nettoyage ANSI sont appliqués dans `Core::paste_upload`
//! AVANT l'envoi — une publication publique ne doit jamais fuiter de donnée sensible.

use crate::error::{Error, Result};
use std::sync::LazyLock;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Plafond d'envoi (les services limitent ; on garde le DÉBUT — startup/premières erreurs y sont).
const MAX_PASTE: usize = 4 * 1024 * 1024;

/// Service de paste supporté.
#[derive(Clone, Copy)]
pub enum PasteService {
    /// Instance publique officielle mclo.gs.
    Mclogs,
    /// Instance mclo.gs auto-hébergée par MineStrator.
    Minestrator,
    /// pastes.dev (bytebin, LuckPerms).
    PastesDev,
}

impl PasteService {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "mclogs" => Some(Self::Mclogs),
            "minestrator" => Some(Self::Minestrator),
            "pastesdev" => Some(Self::PastesDev),
            _ => None,
        }
    }
}

/// Publie `content` (déjà anonymisé) et renvoie l'URL publique de la page.
pub async fn upload(service: PasteService, content: &str) -> Result<String> {
    let body = truncate(content, MAX_PASTE);
    match service {
        PasteService::Mclogs => mclogs("https://api.mclo.gs/1/log", body).await,
        PasteService::Minestrator => mclogs("https://mclogs.minestrator.com/1/log", body).await,
        PasteService::PastesDev => pastes_dev(body).await,
    }
}

/// API mclo.gs (et instances compatibles) : POST form `content`, réponse JSON `{success,url,error}`.
async fn mclogs(url: &str, content: &str) -> Result<String> {
    let resp = HTTP
        .post(url)
        .form(&[("content", content)])
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("envoi vers le service de paste : {e}")))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::Unexpected(format!("réponse du service de paste : {e}")))?;
    if json.get("success").and_then(|v| v.as_bool()) == Some(true) {
        json.get("url")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::Unexpected("réponse du service sans URL".into()))
    } else {
        let msg = json.get("error").and_then(|v| v.as_str()).unwrap_or("échec de l'envoi");
        Err(Error::Unexpected(format!("service de paste : {msg}")))
    }
}

/// API pastes.dev (bytebin) : POST corps brut, réponse JSON `{key}` → URL `pastes.dev/<key>`.
async fn pastes_dev(content: &str) -> Result<String> {
    let resp = HTTP
        .post("https://api.pastes.dev/post")
        .header(reqwest::header::CONTENT_TYPE, "text/log")
        .body(content.to_string())
        .send()
        .await
        .map_err(|e| Error::Unexpected(format!("envoi vers pastes.dev : {e}")))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::Unexpected(format!("réponse de pastes.dev : {e}")))?;
    json.get("key")
        .and_then(|v| v.as_str())
        .map(|k| format!("https://pastes.dev/{k}"))
        .ok_or_else(|| Error::Unexpected("réponse de pastes.dev sans clé".into()))
}

/// Tronque en gardant le DÉBUT (sur une frontière de caractère) si `content` dépasse `max`.
fn truncate(content: &str, max: usize) -> &str {
    if content.len() <= max {
        return content;
    }
    let mut end = max;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

/// Retire les séquences d'échappement ANSI (couleurs console) — le paste doit rester du texte pur.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Séquence CSI `ESC [ … lettre` → on saute jusqu'à la lettre finale incluse.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_ansi_color_codes() {
        assert_eq!(super::strip_ansi("\u{1b}[32mvert\u{1b}[0m ok"), "vert ok");
        assert_eq!(super::strip_ansi("pas d'ansi"), "pas d'ansi");
    }

    #[test]
    fn truncate_keeps_char_boundary() {
        let s = "héllo"; // 'é' = 2 octets
        assert_eq!(super::truncate(s, 2), "h"); // ne coupe pas au milieu de 'é'
        assert_eq!(super::truncate(s, 100), s);
    }

    #[test]
    fn service_ids_map() {
        assert!(super::PasteService::from_id("mclogs").is_some());
        assert!(super::PasteService::from_id("minestrator").is_some());
        assert!(super::PasteService::from_id("pastesdev").is_some());
        assert!(super::PasteService::from_id("nope").is_none());
    }
}
