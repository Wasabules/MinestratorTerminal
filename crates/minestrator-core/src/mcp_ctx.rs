//! Contexte d'un appel d'outil MCP : session, progression, annulation et confirmation demandée
//! directement à l'humain.
//!
//! Les outils s'en servent **sans changer de signature** : `mcp::dispatch(core, name, args)` reste
//! celle du Copilote. Le transport MCP pose le contexte autour de l'appel ; hors de ce contexte
//! (Copilote, tests, appel interne), chaque fonction est sans effet : `session_id()` et
//! `client_name()` valent `None`, `progress` ne fait rien, `cancelled()` vaut `false` et `confirm`
//! renvoie [`Confirmation::Unsupported`] — l'outil retombe alors sur sa garde habituelle
//! (`confirm_name`).
//!
//! Interface posée avant l'implémentation du transport : les signatures ci-dessous sont le
//! contrat entre les outils et le protocole.

/// Réponse de l'humain à une demande de confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// L'utilisateur a confirmé dans son client MCP.
    Accepted,
    /// L'utilisateur a refusé ou annulé : l'outil ne doit rien faire.
    Declined,
    /// Le client ne sait pas poser la question (ou appel hors MCP) : garde habituelle de l'outil.
    Unsupported,
}

/// Identifiant de la session MCP en cours (une connexion cliente), `None` hors transport MCP.
pub fn session_id() -> Option<String> {
    None
}

/// Nom du client MCP (`clientInfo.name` de l'initialisation), `None` hors transport MCP.
pub fn client_name() -> Option<String> {
    None
}

/// Signale l'avancement d'une opération longue au client (si sa requête porte un
/// `progressToken`). `total` absent = avancement inconnu. Sans effet hors transport MCP.
pub async fn progress(done: f64, total: Option<f64>, message: &str) {
    let _ = (done, total, message);
}

/// Le client a-t-il annulé l'appel en cours (`notifications/cancelled`) ? Un outil long le
/// consulte entre deux étapes pour s'arrêter proprement.
pub fn cancelled() -> bool {
    false
}

/// Demande à l'humain de confirmer une action (élicitation MCP). `expected` : texte qu'il doit
/// retaper (nom du serveur, par exemple), `None` pour un simple oui/non.
pub async fn confirm(message: &str, expected: Option<&str>) -> Confirmation {
    let _ = (message, expected);
    Confirmation::Unsupported
}
