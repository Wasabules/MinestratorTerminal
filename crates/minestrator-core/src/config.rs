//! Constantes de configuration du cœur métier.

pub const API_BASE_URL: &str = "https://mine.sttr.io";

/// Origin exigé par le daemon Wings sur le handshake WebSocket (sinon 403).
pub const WS_ORIGIN: &str = "https://minestrator.com";

pub const USER_AGENT: &str = concat!("MinestratorCore/", env!("CARGO_PKG_VERSION"));

pub const KEYRING_SERVICE: &str = "MinestratorTerminal";
pub const KEYRING_ACCOUNT: &str = "api-key";
/// Préfixe de compte trousseau pour les clés LLM du Copilote (une par fournisseur :
/// `llm-key-anthropic`, `llm-key-openai`, …). Permet de changer de fournisseur sans perdre les clés.
pub const KEYRING_ACCOUNT_LLM_PREFIX: &str = "llm-key-";

/// Version d'API Anthropic (en-tête `anthropic-version`).
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Variable d'env qui force l'activation du serveur MCP (bypass du réglage `enabled`).
/// Posée par le Copilote quand il lance notre MCP pour un agent CLI local, afin que
/// l'outillage fonctionne sans dépendre du toggle MCP de l'utilisateur.
pub const MCP_FORCE_ENABLED_ENV: &str = "MINESTRATOR_MCP_FORCE_ENABLED";
/// Restreint (côté serveur MCP) les outils exposés/appelables à une liste blanche CSV de noms
/// (ex. `read_file,list_files`). Posée par le Copilote dans l'env du sous-processus agent : garantit
/// qu'un agent CLI (quelle que soit sa propre gestion d'autorisation) ne peut PAS appeler un outil
/// hors liste — l'outil n'apparaît même pas. Absente ⇒ tous les outils (selon `allow_writes`).
pub const MCP_ALLOWED_TOOLS_ENV: &str = "MINESTRATOR_MCP_ALLOWED_TOOLS";
/// Modèle par défaut du Copilote (fournisseur Anthropic ; modifiable dans les réglages).
pub const COPILOT_DEFAULT_MODEL: &str = "claude-sonnet-5";
