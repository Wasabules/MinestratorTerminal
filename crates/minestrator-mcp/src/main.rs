//! Transport stdio pour le serveur MCP : lit des messages JSON-RPC (délimités par des
//! sauts de ligne) sur stdin, délègue à `minestrator_core::mcp`, écrit les réponses sur
//! stdout. Toute la logique vit dans le core (réutilisée aussi par le mode `--mcp` du GUI).
//!
//! **stdout = protocole** ; les logs vont sur stderr. `--toolsets=a,b` restreint les groupes
//! d'outils exposés (lu par le core, voir `MCP.md`).

use minestrator_core::{mcp, Core};

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Pile élargie : les outils composites imbriquent des futurs volumineux (voir
    // `mcp::THREAD_STACK_BYTES`).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(mcp::THREAD_STACK_BYTES)
        .build()
        .expect("runtime tokio");
    runtime.block_on(async {
        let core = Core::new();
        tracing::info!("minestrator-mcp prêt (stdio)");
        mcp::serve_stdio(std::sync::Arc::new(core)).await;
    });
}
