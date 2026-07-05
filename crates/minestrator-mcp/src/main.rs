//! Transport stdio pour le serveur MCP : lit des messages JSON-RPC (délimités par des
//! sauts de ligne) sur stdin, délègue à `minestrator_core::mcp`, écrit les réponses sur
//! stdout. Toute la logique vit dans le core (réutilisée aussi par le mode `--mcp` du GUI).
//!
//! **stdout = protocole** ; les logs vont sur stderr.

use minestrator_core::{mcp, Core};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let core = Core::new();
    tracing::info!("minestrator-mcp prêt (stdio)");
    mcp::serve_stdio(&core).await;
}
