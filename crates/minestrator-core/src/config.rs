//! Constantes de configuration du cœur métier.

pub const API_BASE_URL: &str = "https://mine.sttr.io";

/// Serveur MCP OFFICIEL de MineStrator (transport Streamable HTTP, auth `Bearer <clé API>`, 107
/// outils en 10 toolsets). Le Copilote lui délègue la GESTION (lecture + une liste blanche
/// d'actions) et garde notre MCP local pour le SFTP + les outils exclusifs ; le serveur MCP local
/// peut aussi le relayer (passerelle). Politique : `official_relay.rs`.
pub const OFFICIAL_MCP_URL: &str = "https://mcp.sttr.io/minestrator";

/// Origin exigé par le daemon Wings sur le handshake WebSocket (sinon 403).
pub const WS_ORIGIN: &str = "https://minestrator.com";

/// CDN des visuels d'egg. L'API ne renvoie qu'un **nom de fichier** (`minecraft.webp`) dans
/// `egg_icon` : ces bases le complètent en URL affichable. `server_icon` = vignette carrée
/// (~1 Kio), `server_header` = bannière large (~200 Kio, à ne charger que si on l'affiche).
pub const EGG_ICON_BASE: &str = "https://cdn.minestrator.com/eggs/server_icon/";
pub const EGG_HEADER_BASE: &str = "https://cdn.minestrator.com/eggs/server_header/";

pub const USER_AGENT: &str = concat!("MinestratorCore/", env!("CARGO_PKG_VERSION"));

/// Service sous lequel tous les secrets sont rangés dans le stockage sécurisé de l'OS
/// (trousseau desktop, Keystore Android). Les comptes ci-dessous en sont les entrées.
pub const KEYRING_SERVICE: &str = "MinestratorTerminal";
/// Clé API du compte MineStrator **actif**. Le serveur MCP headless, les agents CLI du Copilote,
/// le superviseur et l'activation du service d'arrière-plan la lisent ici, sans rien savoir des
/// autres comptes enregistrés.
pub const KEYRING_ACCOUNT: &str = "api-key";
/// Préfixe de compte trousseau pour la clé API d'un compte MineStrator **inactif**
/// (`api-key-<idUtilisateur>`) : mise de côté au changement de compte, reprise au retour.
pub const KEYRING_ACCOUNT_KEY_PREFIX: &str = "api-key-";
/// Préfixe de compte trousseau pour les clés LLM du Copilote (une par fournisseur :
/// `llm-key-anthropic`, `llm-key-openai`, …). Permet de changer de fournisseur sans perdre les clés.
pub const KEYRING_ACCOUNT_LLM_PREFIX: &str = "llm-key-";
/// Préfixe de compte trousseau pour les secrets par jeu (ex. `game-secret-factorio` = token du
/// compte factorio.com pour télécharger des mods).
pub const KEYRING_ACCOUNT_GAME_PREFIX: &str = "game-secret-";
/// Préfixe de compte trousseau pour le mot de passe SFTP saisi par l'utilisateur pour un serveur
/// (`sftp-password-<id>`) : secours quand l'API n'en fournit pas, ou un qui ne fonctionne pas.
pub const KEYRING_ACCOUNT_SFTP_PREFIX: &str = "sftp-password-";
/// Préfixe de compte trousseau pour l'URL d'une destination d'alertes (`webhook-url-<id>`) : une
/// URL de webhook Discord suffit à poster dans le salon, c'est donc un secret à part entière.
pub const KEYRING_ACCOUNT_WEBHOOK_PREFIX: &str = "webhook-url-";
/// Préfixe de compte trousseau pour un **secret des agents** (`agent-secret-<nom>`) : valeur
/// qu'un agent MCP écrit dans une config par la référence `{{secret:<nom>}}` sans jamais la voir
/// (voir `agent_secrets.rs` ; noms et descriptions, non secrets, dans `agent_secrets.json`).
pub const KEYRING_ACCOUNT_AGENT_SECRET_PREFIX: &str = "agent-secret-";

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
/// Passerelle du serveur MCP local vers le MCP officiel : `1`/`true`/`on` la force, `0`/`false`/`off`
/// la coupe, quel que soit le réglage `official_gateway` de `mcp.json` (binaire headless, scripts).
/// Posée aussi par le Copilote dans l'env du serveur MCP de ses agents CLI (relais restreint).
pub const MCP_OFFICIAL_ENV: &str = "MINESTRATOR_MCP_OFFICIAL";
/// Serveur de la session du Copilote (`server_id`), posé par le Copilote dans l'env du serveur MCP
/// qu'il lance pour un agent CLI : tout appel visant un AUTRE serveur est refusé, et les outils
/// officiels relayés suivent la politique du Copilote (lecture + liste blanche), pas la passerelle.
pub const MCP_COPILOT_SERVER_ENV: &str = "MINESTRATOR_MCP_COPILOT_SERVER";
