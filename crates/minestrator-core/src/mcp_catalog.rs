//! Présentation du catalogue MCP local : **groupes d'outils** (toolsets), **annotations**
//! (`readOnlyHint`, `destructiveHint`…), **instructions** et réponse d'initialisation.
//!
//! Les listes de sécurité restent dans `mcp.rs` (`READ_TOOLS`, `WRITE_TOOLS`…) : les annotations en
//! dérivent (lecture seule = `READ_TOOLS`), complétées par les petites listes ci-dessous. Le
//! catalogue brut (`mcp::tool_list()`) n'est pas modifié : le Copilote continue de le lire tel quel.
//!
//! **Groupes.** Chaque outil appartient à un groupe ([`TOOL_GROUPS`]) ; les outils du MCP officiel
//! relayés par la passerelle forment le groupe `official`. Sélection, du plus prioritaire au moins
//! prioritaire : argument `--toolsets=a,b` (binaire `minestrator-mcp` ou `<app> --mcp`), variable
//! [`MCP_TOOLSETS_ENV`], réglage `McpConfig::toolsets` ; vide ou `all` = tout. `core` (identifiants
//! des serveurs) reste toujours actif. Un outil d'un groupe inactif n'est ni listé ni appelable.
//! Un serveur lancé par le Copilote (liste blanche d'env) ignore les groupes : sa liste prime.

use crate::mcp::{READ_TOOLS, WRITE_TOOLS};
use crate::mcp_transport::{negotiate, supported_versions};
use crate::Core;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Variable d'environnement : groupes d'outils exposés (`files,console`…).
pub(crate) const MCP_TOOLSETS_ENV: &str = "MINESTRATOR_MCP_TOOLSETS";

/// Groupe des outils du MCP officiel relayés par la passerelle.
pub(crate) const OFFICIAL_GROUP: &str = "official";
/// Groupe toujours actif : sans lui, aucun identifiant de serveur.
const ALWAYS_ON: &str = "core";

/// Groupes d'outils : nom et rôle.
pub(crate) const TOOLSETS: &[(&str, &str)] = &[
    (
        "core",
        "Découverte et état : liste des serveurs (identifiants), état temps réel.",
    ),
    (
        "console",
        "Contrôle et console : démarrer, arrêter, commandes, lecture de la console.",
    ),
    (
        "files",
        "Fichiers par SFTP : lire, chercher, écrire, éditer, archives, espace disque, transferts.",
    ),
    (
        "config",
        "Démarrage et réglages : commande Java, server.properties, secrets des agents.",
    ),
    (
        "content",
        "Mods, plugins, marketplace, mises à jour, dépendances, mods Satisfactory.",
    ),
    (
        "players",
        "Joueurs : déjà venus, modération, recherche sur tout le compte.",
    ),
    ("backups", "Sauvegardes automatiques et snapshots."),
    (
        "diagnostics",
        "Performance, journaux, incidents, disponibilité, monde, audit.",
    ),
    (
        "mybox",
        "Architecture des MyBox : capacité, placement, création, redimensionnement, suppression.",
    ),
    (
        "schedules",
        "Tâches planifiées du panel (exécutées par l'hébergeur).",
    ),
    (
        "fleet",
        "Plusieurs serveurs : copie, comparaison, commande groupée.",
    ),
    // Groupes prévus pour les autres lots : leurs outils y seront classés à l'intégration.
    (
        "agent",
        "Agir puis vérifier : commande avec sa sortie, attente d'un message, déploiement vérifié.",
    ),
    (
        "build",
        "Configurations structurées et manifestes de serveur (plan, application, clonage).",
    ),
    (
        "guard",
        "Garde des agents : journal des actions, annulation d'un lot de changements.",
    ),
    (
        OFFICIAL_GROUP,
        "Outils du MCP officiel MineStrator relayés par la passerelle.",
    ),
];

/// Classement de chaque outil local : (nom, groupe, titre affiché par les clients).
/// Un test exige que **chaque** outil de `READ_TOOLS` ∪ `WRITE_TOOLS` y figure une fois.
pub(crate) const TOOL_GROUPS: &[(&str, &str, &str)] = &[
    // core
    ("list_servers", "core", "Lister les serveurs"),
    ("server_status", "core", "État d'un serveur"),
    // console
    ("power_action", "console", "Démarrer, redémarrer ou arrêter"),
    ("send_command", "console", "Envoyer une commande console"),
    ("read_console", "console", "Lire la console"),
    // files
    ("list_files", "files", "Lister un dossier"),
    ("read_file", "files", "Lire un fichier"),
    ("read_gz", "files", "Lire un fichier gzippé"),
    ("list_archive", "files", "Contenu d'une archive"),
    ("read_archive_entry", "files", "Lire un fichier d'archive"),
    ("search_files", "files", "Chercher des fichiers"),
    ("disk_usage", "files", "Espace disque"),
    ("download_file", "files", "Télécharger un fichier en local"),
    ("file_stat", "files", "Taille et empreinte d'un fichier"),
    ("write_file", "files", "Écrire un fichier"),
    ("upload_file", "files", "Téléverser un fichier local"),
    ("edit_file", "files", "Modifier un fichier (remplacement)"),
    ("create_dir", "files", "Créer un dossier"),
    ("delete_path", "files", "Supprimer un fichier ou un dossier"),
    ("rename_path", "files", "Renommer ou déplacer"),
    ("extract_archive", "files", "Extraire une archive"),
    ("compress_files", "files", "Compresser en .zip"),
    // config
    ("read_startup", "config", "Commande de démarrage"),
    (
        "set_startup_params",
        "config",
        "Modifier la commande de démarrage",
    ),
    ("read_properties", "config", "Réglages du serveur"),
    ("set_property", "config", "Modifier un réglage du serveur"),
    // content
    ("list_installed_mods", "content", "Mods installés"),
    ("list_installed_plugins", "content", "Plugins installés"),
    ("market_search", "content", "Chercher sur le marketplace"),
    (
        "list_mod_versions",
        "content",
        "Versions d'un mod ou plugin",
    ),
    ("install_mod", "content", "Installer un mod ou plugin"),
    ("check_updates", "content", "Mises à jour disponibles"),
    ("update_mods", "content", "Mettre à jour mods et plugins"),
    (
        "check_plugin_deps",
        "content",
        "Dépendances des plugins et mods",
    ),
    ("ficsit_search", "content", "Chercher un mod Satisfactory"),
    (
        "ficsit_versions",
        "content",
        "Versions d'un mod Satisfactory",
    ),
    (
        "install_ficsit_mod",
        "content",
        "Installer un mod Satisfactory",
    ),
    // players
    ("list_players", "players", "Joueurs déjà venus"),
    ("player_action", "players", "Modérer un joueur"),
    ("find_player", "players", "Retrouver un joueur"),
    // backups
    ("list_backups", "backups", "Sauvegardes automatiques"),
    ("list_snapshots", "backups", "Snapshots"),
    ("create_snapshot", "backups", "Créer un snapshot"),
    // diagnostics
    (
        "server_metrics",
        "diagnostics",
        "Historique de consommation",
    ),
    (
        "analyze_performance",
        "diagnostics",
        "Analyse de performance (Spark)",
    ),
    ("parse_spark_report", "diagnostics", "Lire un rapport Spark"),
    ("diagnose_startup", "diagnostics", "Docteur démarrage"),
    (
        "inspect_region",
        "diagnostics",
        "Inspecter un fichier de région",
    ),
    ("world_info", "diagnostics", "Informations du monde"),
    (
        "audit_server",
        "diagnostics",
        "Audit de configuration et de sécurité",
    ),
    ("search_logs", "diagnostics", "Chercher dans les journaux"),
    ("error_digest", "diagnostics", "Résumé des erreurs"),
    ("uptime_report", "diagnostics", "Disponibilité"),
    ("list_incidents", "diagnostics", "Historique des incidents"),
    // mybox
    ("mybox_info", "mybox", "Détail d'une MyBox"),
    ("mybox_capacity", "mybox", "Capacité des MyBox"),
    ("server_catalog", "mybox", "Catalogue de création"),
    ("plan_placement", "mybox", "Calculer un placement"),
    ("mybox_smartscan", "mybox", "Analyse SmartScan"),
    ("mybox_create_server", "mybox", "Créer un serveur"),
    ("mybox_resize_server", "mybox", "Redimensionner un serveur"),
    ("rename_server", "mybox", "Renommer un serveur"),
    ("rename_mybox", "mybox", "Renommer une MyBox"),
    (
        "mybox_pause_server",
        "mybox",
        "Mettre en pause ou reprendre",
    ),
    ("mybox_reorder_servers", "mybox", "Réordonner les serveurs"),
    (
        "mybox_delete_server",
        "mybox",
        "Supprimer définitivement un serveur",
    ),
    // schedules
    ("list_schedules", "schedules", "Tâches planifiées"),
    ("create_schedule", "schedules", "Créer une tâche planifiée"),
    (
        "update_schedule",
        "schedules",
        "Modifier une tâche planifiée",
    ),
    (
        "delete_schedule",
        "schedules",
        "Supprimer une tâche planifiée",
    ),
    (
        "add_schedule_task",
        "schedules",
        "Ajouter une action planifiée",
    ),
    (
        "update_schedule_task",
        "schedules",
        "Modifier une action planifiée",
    ),
    (
        "delete_schedule_task",
        "schedules",
        "Retirer une action planifiée",
    ),
    (
        "reorder_schedule_tasks",
        "schedules",
        "Réordonner les actions planifiées",
    ),
    // fleet
    ("copy_between_servers", "fleet", "Copier entre serveurs"),
    ("diff_files", "fleet", "Comparer des fichiers ou dossiers"),
    (
        "broadcast_command",
        "fleet",
        "Commande sur plusieurs serveurs",
    ),
    // agent
    ("tail_console", "agent", "Suivre la console par curseur"),
    ("run_command", "agent", "Commande et sa réponse"),
    ("wait_for", "agent", "Attendre un état ou un message"),
    (
        "deploy_and_verify",
        "agent",
        "Déployer un plugin et vérifier",
    ),
    // build
    ("get_config", "build", "Lire une configuration structurée"),
    ("set_config", "build", "Modifier une configuration"),
    ("export_manifest", "build", "Exporter le manifeste d'un serveur"),
    ("plan_manifest", "build", "Comparer un serveur à un manifeste"),
    ("apply_manifest", "build", "Appliquer un manifeste"),
    ("clone_server", "build", "Cloner un serveur"),
    ("pregenerate_world", "build", "Prégénérer un monde (Chunky)"),
    (
        "connect_database",
        "build",
        "Brancher une base de données à un plugin",
    ),
    ("network_topology", "build", "Topologie d'un réseau proxy"),
    ("link_proxy", "build", "Relier des serveurs à un proxy"),
    // Préréglages de réseau (LuckPerms, Geyser) ; ping d'un serveur (diagnostic en lecture).
    ("network_preset", "build", "Préréglage de réseau (LuckPerms, Geyser)"),
    ("ping_server", "diagnostics", "Pinguer un serveur (Java, Bedrock)"),
    // guard
    (
        "list_agent_actions",
        "guard",
        "Journal des actions des agents",
    ),
    ("list_changesets", "guard", "Modifications annulables"),
    (
        "rollback_changeset",
        "guard",
        "Annuler les modifications d'un agent",
    ),
    // Depuis l'ordinateur (files) et sur plusieurs serveurs (fleet).
    ("sync_dir", "files", "Synchroniser un dossier local"),
    ("upload_world", "files", "Installer un monde depuis l'ordinateur"),
    ("apply_to_servers", "fleet", "Même outil sur plusieurs serveurs"),
    // Plateforme et version d'un serveur (platform.rs), suppression d'une base de la MyBox.
    (
        "set_server_platform",
        "build",
        "Choisir la plateforme et la version",
    ),
    ("delete_database", "mybox", "Supprimer une base de données"),
    // config : secrets des agents (agent_secrets.rs)
    ("list_secrets", "config", "Secrets des agents"),
    ("generate_secret", "config", "Générer un secret des agents"),
];

/// Outils modifiants **non destructifs** (`destructiveHint: false`) : ils ajoutent sans rien
/// supprimer, écraser ni interrompre. En cas de doute, un outil modifiant reste destructif.
pub(crate) const NON_DESTRUCTIVE_TOOLS: &[&str] = &[
    "create_dir",
    "compress_files",
    "install_mod",
    "create_snapshot",
    "mybox_create_server",
    "rename_server",
    "rename_mybox",
    "mybox_reorder_servers",
    "create_schedule",
    "add_schedule_task",
    "reorder_schedule_tasks",
    "clone_server",
    // Crée un secret sous un nom libre (un nom pris est refusé, jamais écrasé).
    "generate_secret",
];

/// Outils modifiants **idempotents** (`idempotentHint: true`) : les rappeler avec les mêmes
/// arguments n'a pas d'effet de plus (ils fixent une valeur finale).
pub(crate) const IDEMPOTENT_TOOLS: &[&str] = &[
    "write_file",
    "upload_file",
    "create_dir",
    "set_property",
    "set_startup_params",
    "mybox_resize_server",
    "rename_server",
    "rename_mybox",
    "mybox_pause_server",
    "mybox_reorder_servers",
    "update_schedule",
    "update_schedule_task",
    "reorder_schedule_tasks",
    "set_config",
    "apply_manifest",
    // Fixe des valeurs finales ; le secret du proxy est réutilisé d'un appel à l'autre.
    "link_proxy",
    // Relancée sur un dossier déjà synchronisé, rien n'est envoyé ni supprimé.
    "sync_dir",
    // Fixe plateforme, version, Java et JAR : rappelé à l'identique, rien à changer.
    "set_server_platform",
    // Fixe des valeurs finales ; un plugin présent n'est pas réinstallé, une clé identique pas recopiée.
    "network_preset",
];

/// Outils qui sortent du compte MineStrator (`openWorldHint: true`) : marketplaces, ficsit.app,
/// spark.lucko.me. Les autres n'agissent que sur les serveurs et MyBox de l'utilisateur.
pub(crate) const OPEN_WORLD_TOOLS: &[&str] = &[
    "market_search",
    "list_mod_versions",
    "install_mod",
    "update_mods",
    "ficsit_search",
    "ficsit_versions",
    "install_ficsit_mod",
    "analyze_performance",
    "parse_spark_report",
    // Contenu résolu et installé depuis les marketplaces.
    "plan_manifest",
    "apply_manifest",
    // Chunky installé depuis Modrinth (`install_chunky`).
    "pregenerate_world",
    // Peut relayer install_mod / update_mods (marketplaces).
    "apply_to_servers",
    // JAR officiel téléchargé depuis PaperMC ou Purpur (voie JAR).
    "set_server_platform",
    // LuckPerms (Modrinth), Geyser et Floodgate (GeyserMC) installés au besoin.
    "network_preset",
];

/// Outils locaux dont la confirmation (`confirm_name`) est demandée à l'humain quand le client
/// sait le faire (élicitation) : l'argument devient alors facultatif dans leur schéma.
pub(crate) const HUMAN_CONFIRMED_TOOLS: &[&str] = &[
    "mybox_delete_server",
    // Changement de version (change_version) et suppression d'une base : nom exact retapé.
    "set_server_platform",
    "delete_database",
];

/// Instructions données au modèle à l'initialisation (enchaînements recommandés). Les outils des
/// autres groupes (`agent`, `build`, `guard`) sont cités même s'ils sont désactivés.
pub(crate) const INSTRUCTIONS: &str = "Serveur MCP de MineStrator Terminal : gestion de serveurs de jeu (surtout Minecraft) hébergés chez MineStrator.
Enchaînements recommandés :
- Commence par list_servers : il donne les server_id (et mybox_id) que prennent les autres outils.
- Erreurs ou crash : error_digest (erreurs regroupées, plugin probable) avant search_logs (recherche ciblée). Serveur qui ne démarre pas : diagnose_startup avant de proposer un correctif.
- Avant une intervention risquée (mods, configuration, monde) : create_snapshot.
- Après chaque action, vérifie son effet au lieu de le supposer : wait_for (message attendu dans la console), tail_console, run_command (commande et sa sortie), server_status.
- Configuration : get_config puis set_config avec dry_run pour prévisualiser ; plan_manifest avant apply_manifest.
- Réseau Velocity ou BungeeCord : network_topology, puis link_proxy (dry_run d'abord) ; ne manipule jamais le secret de transfert toi-même.
- Fichier qui contient des secrets : edit_file (remplacement ciblé) plutôt que read_file + write_file, car les secrets lus sont masqués en [SECRET].
- Token, clé d'API, mot de passe à écrire : jamais en clair ; list_secrets puis {{secret:NOM}} à la place de la valeur (set_config, write_file, edit_file, manifeste).
- Action destructive (suppression de serveur…) : seulement à la demande explicite de l'utilisateur ; la confirmation peut lui être demandée directement par ton client.
- MyBox : CPU en cœurs entiers, RAM en Go entiers.
Les sorties peuvent être anonymisées (IP, e-mails, mots de passe, pseudos remplacés par un alias stable).";

/// `{ name, title, version }` du serveur.
pub(crate) fn server_info() -> Value {
    json!({ "name": "minestrator-mcp", "title": "MineStrator Terminal", "version": env!("CARGO_PKG_VERSION") })
}

/// Capacités réellement implémentées : outils, prompts, ressources (lecture, sans abonnement).
/// Ni journalisation, ni complétion, ni notifications de changement de liste.
pub(crate) fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "prompts": { "listChanged": false },
        "resources": { "subscribe": false, "listChanged": false },
    })
}

/// Réponse à `initialize` (ère à poignée de main) : version négociée, capacités, identité,
/// instructions.
pub(crate) fn initialize_result(params: &Value) -> Value {
    let version = negotiate(params.get("protocolVersion").and_then(Value::as_str));
    let mut info = server_info();
    if version < "2025-06-18" {
        // `title` n'existe que depuis 2025-06-18.
        if let Some(i) = info.as_object_mut() {
            i.remove("title");
        }
    }
    json!({
        "protocolVersion": version,
        "capabilities": capabilities(),
        "serverInfo": info,
        "instructions": INSTRUCTIONS,
    })
}

/// Réponse à `server/discover` (ère sans état ; l'identité part dans `_meta`, posée par le
/// transport).
pub(crate) fn discover_result() -> Value {
    json!({
        "supportedVersions": supported_versions(),
        "capabilities": capabilities(),
        "instructions": INSTRUCTIONS,
    })
}

// --- Groupes d'outils --------------------------------------------------------------------------

static CLI_TOOLSETS: OnceLock<Option<Vec<String>>> = OnceLock::new();

/// Mémorise l'argument `--toolsets` du processus (une fois, au lancement du transport).
pub(crate) fn set_cli_toolsets(toolsets: Option<Vec<String>>) {
    let _ = CLI_TOOLSETS.set(toolsets);
}

fn split_csv(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// `--toolsets=a,b` ou `--toolsets a,b` dans la ligne de commande (la dernière occurrence prime).
/// Partagé par le binaire `minestrator-mcp` et `<app> --mcp`.
pub(crate) fn toolsets_from_args(args: impl IntoIterator<Item = String>) -> Option<Vec<String>> {
    let mut found = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--toolsets=") {
            found = Some(split_csv(value));
        } else if arg == "--toolsets" {
            found = args.next().map(|v| split_csv(&v));
        }
    }
    found
}

fn known_group(name: &str) -> bool {
    TOOLSETS.iter().any(|(g, _)| *g == name)
}

/// Groupe d'un outil local (`None` : non classé).
pub(crate) fn group_of(tool: &str) -> Option<&'static str> {
    TOOL_GROUPS
        .iter()
        .find(|(n, _, _)| *n == tool)
        .map(|(_, g, _)| *g)
}

fn title_of(tool: &str) -> Option<&'static str> {
    TOOL_GROUPS
        .iter()
        .find(|(n, _, _)| *n == tool)
        .map(|(_, _, t)| *t)
}

/// Groupes actifs d'après l'argument, la variable puis le réglage (le premier non vide l'emporte).
/// `None` = tous. Les noms inconnus sont ignorés (avertissement au journal) ; si aucun n'est
/// connu, tout reste exposé plutôt qu'un serveur vide.
pub(crate) fn resolve_toolsets(
    arg: Option<&[String]>,
    env: Option<&str>,
    setting: &[String],
) -> Option<BTreeSet<String>> {
    let chosen: Vec<String> = match (
        arg.filter(|a| !a.is_empty()),
        env.map(split_csv).filter(|e| !e.is_empty()),
    ) {
        (Some(a), _) => a.to_vec(),
        (None, Some(e)) => e,
        (None, None) => setting.to_vec(),
    };
    let names: Vec<String> = chosen
        .iter()
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() || names.iter().any(|n| n == "all") {
        return None;
    }
    let mut set = BTreeSet::new();
    for n in names {
        if known_group(&n) {
            set.insert(n);
        } else {
            tracing::warn!("groupe d'outils MCP inconnu ignoré : {n}");
        }
    }
    if set.is_empty() {
        tracing::warn!("aucun groupe d'outils MCP connu : tous les outils restent exposés");
        return None;
    }
    set.insert(ALWAYS_ON.to_string());
    Some(set)
}

/// Groupes actifs de ce processus. Le Copilote (liste blanche d'env) n'est jamais restreint par
/// les groupes : sa liste blanche prime.
pub(crate) fn active_toolsets(core: &Core) -> Option<BTreeSet<String>> {
    if crate::mcp::env_has_allowlist() {
        return None;
    }
    let cli = CLI_TOOLSETS.get().cloned().flatten();
    resolve_toolsets(
        cli.as_deref(),
        std::env::var(MCP_TOOLSETS_ENV).ok().as_deref(),
        &core.get_mcp_config().toolsets,
    )
}

/// Un outil est-il dans un groupe actif ? Un outil local non classé n'est exposé que sans
/// sélection ; un nom hors du catalogue local relève du groupe `official`.
pub(crate) fn tool_in_toolsets(selection: Option<&BTreeSet<String>>, tool: &str) -> bool {
    let Some(set) = selection else { return true };
    match group_of(tool) {
        Some(g) => set.contains(g),
        None if READ_TOOLS.contains(&tool) || WRITE_TOOLS.contains(&tool) => false,
        None => set.contains(OFFICIAL_GROUP),
    }
}

/// Message de refus d'un outil hors des groupes actifs.
pub(crate) fn inactive_message(tool: &str) -> String {
    let group = group_of(tool).unwrap_or(OFFICIAL_GROUP);
    format!(
        "Outil « {tool} » non exposé : son groupe « {group} » n'est pas activé (--toolsets, {MCP_TOOLSETS_ENV} ou réglage `toolsets`)."
    )
}

// --- Annotations et présentation de tools/list --------------------------------------------------

/// Annotations d'un outil local (`None` hors de `READ_TOOLS` ∪ `WRITE_TOOLS` : les outils officiels
/// relayés gardent les leurs).
pub(crate) fn annotations(tool: &str) -> Option<Value> {
    let read = READ_TOOLS.contains(&tool);
    if !read && !WRITE_TOOLS.contains(&tool) {
        return None;
    }
    let mut a = json!({ "readOnlyHint": read, "openWorldHint": OPEN_WORLD_TOOLS.contains(&tool) });
    if let Some(title) = title_of(tool) {
        a["title"] = json!(title);
    }
    if !read {
        a["destructiveHint"] = json!(!NON_DESTRUCTIVE_TOOLS.contains(&tool));
        a["idempotentHint"] = json!(IDEMPOTENT_TOOLS.contains(&tool));
    }
    Some(a)
}

/// Schéma d'un outil dont la confirmation est demandée à l'humain : `confirm_name` devient
/// facultatif, la description le dit. Partagé avec la passerelle (`official_relay.rs`).
pub(crate) fn relax_confirmation(def: &mut Value) {
    if let Some(req) = def
        .pointer_mut("/inputSchema/required")
        .and_then(Value::as_array_mut)
    {
        req.retain(|v| v != "confirm_name");
    }
    let desc = def
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    def["description"] = json!(format!(
        "Confirmation demandée DIRECTEMENT à l'utilisateur par ton client (il retape le nom exact) : `confirm_name` facultatif. {desc}"
    ));
}

fn tool_name(def: &Value) -> Option<&str> {
    def.get("name").and_then(Value::as_str)
}

/// `tools/list` des outils locaux (déjà filtrés par la liste blanche d'env) : groupes inactifs
/// retirés, annotations ajoutées, confirmation par l'humain annoncée si le client la gère.
pub(crate) fn present_tools(core: &Core, tools: &mut Value) {
    let selection = active_toolsets(core);
    let human = crate::mcp_ctx::elicitation_supported();
    let Some(list) = tools.as_array_mut() else {
        return;
    };
    list.retain(|t| tool_name(t).is_some_and(|n| tool_in_toolsets(selection.as_ref(), n)));
    for def in list.iter_mut() {
        let Some(name) = tool_name(def).map(str::to_string) else {
            continue;
        };
        if let Some(a) = annotations(&name) {
            def["annotations"] = a;
        }
        if human && HUMAN_CONFIRMED_TOOLS.contains(&name.as_str()) {
            relax_confirmation(def);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exposed() -> Vec<&'static str> {
        READ_TOOLS.iter().chain(WRITE_TOOLS).copied().collect()
    }

    /// Le réglage des groupes (Réglages → MCP du desktop) liste ses groupes en dur : il doit suivre
    /// `TOOLSETS`, sans en oublier ni en inventer.
    #[test]
    fn desktop_settings_list_every_toolset() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../desktop/src/lib/components/settings/McpToolsetsSection.svelte"
        );
        let src = std::fs::read_to_string(path).expect("McpToolsetsSection.svelte");
        let start = src.find("const GROUPS = [").expect("GROUPS");
        let end = start + src[start..].find("] as const").expect("fin de GROUPS");
        let listed: BTreeSet<&str> = src[start..end]
            .split('\'')
            .skip(1)
            .step_by(2)
            .collect();
        let expected: BTreeSet<&str> = TOOLSETS.iter().map(|(g, _)| *g).collect();
        assert_eq!(listed, expected, "groupes du réglage desktop ≠ TOOLSETS");
    }

    #[test]
    fn every_exposed_tool_has_exactly_one_group() {
        // Échoue tant qu'un outil de READ_TOOLS ∪ WRITE_TOOLS n'est pas classé : à l'ajout d'un
        // outil, le ranger dans TOOL_GROUPS (groupe existant, ou nouveau groupe dans TOOLSETS).
        for t in exposed() {
            let n = TOOL_GROUPS.iter().filter(|(name, _, _)| *name == t).count();
            assert_eq!(n, 1, "{t} : doit figurer une fois dans TOOL_GROUPS");
        }
        for (name, group, title) in TOOL_GROUPS {
            assert!(exposed().contains(name), "{name} classé mais pas exposé");
            assert!(
                known_group(group) && *group != OFFICIAL_GROUP,
                "{name} : groupe {group}"
            );
            assert!(!title.is_empty());
        }
    }

    #[test]
    fn annotations_are_consistent_with_the_security_lists() {
        for t in exposed() {
            let a = annotations(t).unwrap_or_else(|| panic!("{t} sans annotations"));
            let read = READ_TOOLS.contains(&t);
            assert_eq!(a["readOnlyHint"], read, "{t}");
            assert!(a["openWorldHint"].is_boolean(), "{t}");
            if read {
                assert!(
                    a.get("destructiveHint").is_none(),
                    "{t} : sans objet pour une lecture"
                );
            } else {
                assert!(
                    a["destructiveHint"].is_boolean() && a["idempotentHint"].is_boolean(),
                    "{t}"
                );
            }
        }
        for list in [
            NON_DESTRUCTIVE_TOOLS,
            IDEMPOTENT_TOOLS,
            HUMAN_CONFIRMED_TOOLS,
        ] {
            for t in list {
                assert!(
                    WRITE_TOOLS.contains(t),
                    "{t} : liste réservée aux outils modifiants"
                );
            }
        }
        for t in OPEN_WORLD_TOOLS {
            assert!(exposed().contains(t), "{t}");
        }
        // Ce qui supprime, écrase ou interrompt reste destructif.
        for t in [
            "mybox_delete_server",
            "delete_path",
            "write_file",
            "power_action",
            "delete_schedule",
            "broadcast_command",
            "copy_between_servers",
        ] {
            assert_eq!(annotations(t).unwrap()["destructiveHint"], true, "{t}");
        }
        for t in ["create_dir", "create_snapshot", "compress_files"] {
            assert_eq!(annotations(t).unwrap()["destructiveHint"], false, "{t}");
        }
        for t in ["set_property", "write_file"] {
            assert_eq!(annotations(t).unwrap()["idempotentHint"], true, "{t}");
        }
        // Un outil relayé (hors catalogue local) garde ses propres annotations.
        assert!(annotations("reinstall_server").is_none());
    }

    #[test]
    fn toolsets_follow_argument_then_env_then_setting() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let set = |v: &[&str]| Some(v.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>());
        assert_eq!(resolve_toolsets(None, None, &[]), None, "rien = tout");
        assert_eq!(
            resolve_toolsets(None, None, &s(&["files"])),
            set(&["core", "files"]),
            "réglage ; core toujours actif"
        );
        assert_eq!(
            resolve_toolsets(None, Some("console, players"), &s(&["files"])),
            set(&["console", "core", "players"]),
            "env > réglage"
        );
        assert_eq!(
            resolve_toolsets(Some(&s(&["Diagnostics"])), Some("console"), &s(&["files"])),
            set(&["core", "diagnostics"]),
            "argument > env"
        );
        assert_eq!(
            resolve_toolsets(Some(&[]), Some("console"), &[]),
            set(&["console", "core"]),
            "argument vide : ignoré"
        );
        assert_eq!(resolve_toolsets(None, Some("all"), &s(&["files"])), None);
        assert_eq!(
            resolve_toolsets(None, Some("fichiers"), &[]),
            None,
            "aucun groupe connu : tout"
        );
        assert_eq!(
            resolve_toolsets(None, Some("fichiers,files"), &[]),
            set(&["core", "files"]),
            "inconnu ignoré"
        );
    }

    #[test]
    fn toolsets_argument_is_read_from_the_command_line() {
        let args = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(toolsets_from_args(args(&["app", "--mcp"])), None);
        assert_eq!(
            toolsets_from_args(args(&["app", "--mcp", "--toolsets=files,console"])),
            Some(args(&["files", "console"]))
        );
        assert_eq!(
            toolsets_from_args(args(&["bin", "--toolsets", "files"])),
            Some(args(&["files"]))
        );
        assert_eq!(
            toolsets_from_args(args(&["bin", "--toolsets=a", "--toolsets=b"])),
            Some(args(&["b"])),
            "la dernière prime"
        );
    }

    #[test]
    fn inactive_groups_hide_local_and_official_tools() {
        let sel = resolve_toolsets(None, Some("files"), &[]);
        assert!(tool_in_toolsets(sel.as_ref(), "read_file"));
        assert!(
            tool_in_toolsets(sel.as_ref(), "list_servers"),
            "core toujours actif"
        );
        assert!(!tool_in_toolsets(sel.as_ref(), "power_action"));
        assert!(
            !tool_in_toolsets(sel.as_ref(), "reinstall_server"),
            "officiel inactif"
        );
        let with_official = resolve_toolsets(None, Some("files,official"), &[]);
        assert!(tool_in_toolsets(with_official.as_ref(), "reinstall_server"));
        assert!(
            tool_in_toolsets(None, "power_action") && tool_in_toolsets(None, "reinstall_server")
        );
        assert!(inactive_message("power_action").contains("« console »"));
    }

    #[test]
    fn confirmation_can_be_relaxed_when_the_human_confirms() {
        let mut def = crate::mcp::tool_list()
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "mybox_delete_server")
            .unwrap()
            .clone();
        relax_confirmation(&mut def);
        let req = def["inputSchema"]["required"].as_array().unwrap();
        assert!(!req.contains(&json!("confirm_name")) && req.contains(&json!("server_id")));
        assert!(def["description"]
            .as_str()
            .unwrap()
            .starts_with("Confirmation demandée DIRECTEMENT"));
        // Le catalogue brut (Copilote, clients sans élicitation) n'est pas touché.
        let raw = crate::mcp::tool_list()
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "mybox_delete_server")
            .unwrap();
        assert!(raw["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("confirm_name")));
    }

    #[test]
    fn initialization_negotiates_and_describes_the_server() {
        let r = initialize_result(&json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(r["protocolVersion"], "2024-11-05");
        assert!(
            r["serverInfo"].get("title").is_none(),
            "pas de title avant 2025-06-18"
        );
        assert_eq!(r["capabilities"]["resources"]["subscribe"], false);
        assert!(r["capabilities"].get("logging").is_none());
        let r = initialize_result(&json!({ "protocolVersion": "2031-01-01" }));
        assert_eq!(r["protocolVersion"], "2025-11-25");
        assert_eq!(r["serverInfo"]["title"], "MineStrator Terminal");
        assert_eq!(r["instructions"], INSTRUCTIONS);
        let d = discover_result();
        assert_eq!(d["supportedVersions"][0], "2026-07-28");
    }

    #[test]
    fn instructions_are_short_and_cite_the_recommended_chains() {
        assert!(
            INSTRUCTIONS.chars().count() < 2000,
            "{}",
            INSTRUCTIONS.chars().count()
        );
        for t in [
            "list_servers",
            "error_digest",
            "search_logs",
            "diagnose_startup",
            "create_snapshot",
            "wait_for",
            "tail_console",
            "run_command",
            "plan_manifest",
            "apply_manifest",
            "set_config",
            "dry_run",
            "edit_file",
        ] {
            assert!(INSTRUCTIONS.contains(t), "{t}");
        }
    }
}
