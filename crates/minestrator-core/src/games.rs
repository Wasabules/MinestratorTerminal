//! Capacités par jeu — dérivées du type d'« egg » MineStrator (Pterodactyl) pour piloter quelles
//! vues/fonctions l'UI expose. Portable : le même mapping sert au futur daemon/CLI et au gating des
//! outils MCP. On mappe finement Minecraft (Java/Bedrock) et Satisfactory ; tout autre jeu retombe
//! sur un défaut sûr (Aperçu + Console + SFTP + Assistant + Backups).

use serde::Serialize;

/// Ce qu'un serveur d'un jeu donné sait faire, du point de vue de l'app. Source unique : tout
/// (vues, console, IA) dérive de cette structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GameCapabilities {
    /// Famille de jeu : `"minecraft"` | `"satisfactory"` | `"generic"`.
    pub family: String,
    /// Autocomplétion console : `"minecraft_java"` | `"minecraft_bedrock"` | `"none"`.
    pub console_autocomplete: String,
    /// Format des logs pour la détection des niveaux (filtres + coloration console) :
    /// `"minecraft"` (`[…/ERROR]`, SEVERE) | `"unreal"` (Satisfactory : `LogX: Error/Warning/Display`)
    /// | `"generic"` (ERROR/WARN/INFO insensible à la casse).
    pub log_format: String,
    /// Vue Joueurs + carte joueurs de l'Aperçu + commandes joueur de la console.
    pub players: bool,
    /// Joueurs déjà venus (liste, temps de jeu, fiche, inventaire), lus par l'hébergeur dans le
    /// dossier `playerdata/` du monde : Minecraft **Java** seulement (Bedrock range ses joueurs
    /// dans la base du monde, que l'hébergeur ne lit pas).
    pub player_data: bool,
    /// Marketplace de mods : `"minecraft"` (Modrinth/CF/Spigot via MineStrator) | `"ficsit"` | `"none"`.
    pub mods: String,
    /// Sauvegardes/snapshots (true partout aujourd'hui ; champ prévu pour le futur).
    pub backups: bool,
    /// Vue Paramètres : réglages du serveur (`server.properties`) lus et modifiés via l'API, sans
    /// SFTP. Minecraft (Java et Bedrock) : le seul jeu dont l'hôte expose ce fichier.
    pub properties: bool,
    /// Commande console qui diffuse un message à tous les joueurs (`{message}` = le texte), pour
    /// les annonces avant un redémarrage programmé. `null` : le jeu n'en a pas, aucune annonce.
    pub broadcast: Option<String>,
}

/// Commande de diffusion d'un jeu (voir [`GameCapabilities::broadcast`]). Prudence : on n'en
/// déclare que pour les jeux dont la console l'accepte à coup sûr — une commande inconnue serait
/// au mieux ignorée, au pire interprétée autrement.
fn broadcast_for(family: &str) -> Option<String> {
    match family {
        // Java comme Bedrock : `say` diffuse dans le chat, préfixé de `[Server]`.
        "minecraft" => Some("say {message}".into()),
        // Console du serveur dédié : une ligne sans `/` est envoyée dans le chat.
        "factorio" => Some("{message}".into()),
        // Commande console native du serveur Rust (RCON), avec ou sans Oxide/uMod.
        "rust" => Some("say {message}".into()),
        _ => None,
    }
}

/// Mots-clés d'egg identifiant la famille Minecraft (loaders, forks, proxys). ⚠️ PAS de « vanilla »
/// (MineStrator suffixe beaucoup de jeux « (Vanilla) » : Valheim, Palworld, Terraria… → collision).
const MINECRAFT_KEYWORDS: &[&str] = &[
    "minecraft", "java", "bedrock", "paper", "spigot", "purpur", "folia", "bukkit", "fabric",
    "forge", "neoforge", "quilt", "mohist", "magma", "arclight", "sponge", "pocketmine", "geyser",
    "pufferfish", "nukkit",
];

/// Mots-clés d'egg identifiant Satisfactory.
const SATISFACTORY_KEYWORDS: &[&str] = &["satisfactory", "ficsit"];

/// Constructeur concis d'un profil de capacités.
fn caps(
    family: &str,
    console_autocomplete: &str,
    log_format: &str,
    players: bool,
    mods: &str,
) -> GameCapabilities {
    GameCapabilities {
        family: family.into(),
        console_autocomplete: console_autocomplete.into(),
        log_format: log_format.into(),
        players,
        player_data: players && console_autocomplete == "minecraft_java",
        mods: mods.into(),
        backups: true,
        properties: family == "minecraft",
        broadcast: broadcast_for(family),
    }
}

/// Identifiants d'egg **exacts** (catalogue `/site/eggs`, relevé le 2026-08-19) des jeux que l'app
/// sait traiter spécifiquement. C'est la voie fiable : `id_egg` est stable côté hébergeur, là où
/// `egg_name` bouge au gré des renommages et piège sur les suffixes (« Terraria (Vanilla) »).
/// Tout id absent d'ici retombe sur l'heuristique de [`capabilities_for`].
const EGG_MINECRAFT_JAVA: i64 = 1;
const EGG_MINECRAFT_BEDROCK: i64 = 4362;
const EGG_SATISFACTORY: i64 = 12;
const EGG_VALHEIM: i64 = 6;
const EGG_V_RISING: i64 = 13;
const EGG_FACTORIO: i64 = 14;
const EGG_RUST: i64 = 33;
/// Eggs qui ne sont pas des jeux : bases de données proposées par l'hébergeur. Elles n'ont ni
/// joueurs, ni mods, ni autocomplétion console — les traiter comme un jeu générique afficherait
/// une vue Joueurs et une marketplace vides.
const EGG_DATABASES: &[i64] = &[15 /* Redis 7 */, 16 /* MongoDB 7 */, 30 /* MariaDB 11.6 */];

/// Déduit les capacités d'un serveur en privilégiant l'`id_egg` exact quand l'API le fournit,
/// avec repli sur l'heuristique par mots-clés ([`capabilities_for`]) pour les eggs inconnus —
/// un nouveau jeu ajouté par l'hébergeur reste ainsi utilisable sans mise à jour de l'app.
pub fn capabilities_for_egg(
    id_egg: Option<i64>,
    egg_name: &str,
    bedrock: bool,
) -> GameCapabilities {
    let Some(id) = id_egg else {
        return capabilities_for(egg_name, bedrock);
    };
    if EGG_DATABASES.contains(&id) {
        return caps("database", "none", "generic", false, "none");
    }
    match id {
        EGG_MINECRAFT_JAVA => caps("minecraft", "minecraft_java", "minecraft", true, "minecraft"),
        EGG_MINECRAFT_BEDROCK => caps("minecraft", "minecraft_bedrock", "minecraft", true, "none"),
        EGG_SATISFACTORY => caps("satisfactory", "none", "unreal", false, "ficsit"),
        EGG_VALHEIM => caps("valheim", "none", "generic", false, "thunderstore"),
        EGG_V_RISING => caps("v_rising", "none", "generic", false, "thunderstore"),
        EGG_FACTORIO => caps("factorio", "none", "generic", false, "factorio"),
        EGG_RUST => caps("rust", "none", "generic", false, "umod"),
        // Egg connu de l'hébergeur mais pas de nous (Palworld, ARK, DayZ…) : l'heuristique peut
        // encore reconnaître un fork Minecraft installé sous un autre egg.
        _ => capabilities_for(egg_name, bedrock),
    }
}

/// Déduit les capacités d'un serveur depuis son `egg_name` et le flag `bedrock` (Bedrock détecté
/// de façon fiable via le flag de l'API, pas par le nom). La valeur `mods` nomme la marketplace :
/// `minecraft` (Modrinth/CF/Spigot via MineStrator) · `ficsit` · `thunderstore` (Valheim/V Rising)
/// · `factorio` · `umod` (Rust) · `none`.
pub fn capabilities_for(egg_name: &str, bedrock: bool) -> GameCapabilities {
    let egg = egg_name.to_ascii_lowercase();
    let has = |kw: &[&str]| kw.iter().any(|k| egg.contains(k));

    if bedrock {
        // Le pipeline mods actuel (Modrinth/CF/Spigot via MineStrator) est Java ; pas d'équivalent Bedrock.
        caps("minecraft", "minecraft_bedrock", "minecraft", true, "none")
    } else if has(MINECRAFT_KEYWORDS) {
        caps("minecraft", "minecraft_java", "minecraft", true, "minecraft")
    } else if has(SATISFACTORY_KEYWORDS) {
        caps("satisfactory", "none", "unreal", false, "ficsit")
    } else if has(&["valheim"]) {
        caps("valheim", "none", "generic", false, "thunderstore")
    } else if has(&["vrising", "v rising", "v-rising"]) {
        caps("v_rising", "none", "generic", false, "thunderstore")
    } else if has(&["factorio"]) {
        caps("factorio", "none", "generic", false, "factorio")
    } else if has(&["rust"]) {
        caps("rust", "none", "generic", false, "umod")
    } else {
        // Défaut sûr : n'importe quel autre egg (Palworld, ARK, Terraria…).
        caps("generic", "none", "generic", false, "none")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_minecraft() {
        let c = capabilities_for("Paper 1.21", false);
        assert_eq!(c.family, "minecraft");
        assert_eq!(c.console_autocomplete, "minecraft_java");
        assert_eq!(c.log_format, "minecraft");
        assert!(c.players);
        assert_eq!(c.mods, "minecraft");
    }

    #[test]
    fn bedrock_minecraft() {
        // Détecté via le flag bedrock même si l'egg ne contient aucun mot-clé Java.
        let c = capabilities_for("Nukkit", true);
        assert_eq!(c.family, "minecraft");
        assert_eq!(c.console_autocomplete, "minecraft_bedrock");
        assert_eq!(c.log_format, "minecraft");
        assert!(c.players);
        assert_eq!(c.mods, "none");
    }

    #[test]
    fn satisfactory() {
        let c = capabilities_for("Satisfactory", false);
        assert_eq!(c.family, "satisfactory");
        assert_eq!(c.mods, "ficsit");
        assert!(!c.players);
        assert_eq!(c.console_autocomplete, "none");
        assert_eq!(c.log_format, "unreal");
    }

    #[test]
    fn unknown_game_falls_back_to_generic() {
        let c = capabilities_for("Palworld", false);
        assert_eq!(c.family, "generic");
        assert!(!c.players);
        assert_eq!(c.mods, "none");
        assert_eq!(c.console_autocomplete, "none");
        assert_eq!(c.log_format, "generic");
    }

    #[test]
    fn mod_marketplace_games_map_to_their_source() {
        assert_eq!(capabilities_for("Valheim (Vanilla)", false).mods, "thunderstore");
        assert_eq!(capabilities_for("Valheim (Vanilla)", false).family, "valheim");
        assert_eq!(capabilities_for("V Rising", false).mods, "thunderstore");
        assert_eq!(capabilities_for("V Rising", false).family, "v_rising");
        assert_eq!(capabilities_for("Factorio", false).mods, "factorio");
        assert_eq!(capabilities_for("Rust", false).mods, "umod");
        assert_eq!(capabilities_for("Rust", false).family, "rust");
    }

    #[test]
    fn egg_id_wins_over_the_name() {
        // Un serveur Minecraft nommé « Vanilla » : l'heuristique doit être court-circuitée par l'id.
        let c = capabilities_for_egg(Some(EGG_MINECRAFT_JAVA), "26.1.2 CanvasMC", false);
        assert_eq!(c.family, "minecraft");
        assert_eq!(c.mods, "minecraft");
        assert_eq!(capabilities_for_egg(Some(EGG_SATISFACTORY), "", false).mods, "ficsit");
        assert_eq!(capabilities_for_egg(Some(EGG_FACTORIO), "", false).mods, "factorio");
    }

    #[test]
    fn databases_are_not_games() {
        for id in EGG_DATABASES {
            let c = capabilities_for_egg(Some(*id), "MariaDB 11.6", false);
            assert_eq!(c.family, "database", "egg {id}");
            assert!(!c.players, "egg {id} ne doit pas exposer de joueurs");
            assert_eq!(c.mods, "none");
        }
    }

    #[test]
    fn broadcast_only_for_games_that_support_it() {
        assert_eq!(capabilities_for("Paper 1.21", false).broadcast.as_deref(), Some("say {message}"));
        assert_eq!(capabilities_for("Nukkit", true).broadcast.as_deref(), Some("say {message}"));
        assert_eq!(capabilities_for("Factorio", false).broadcast.as_deref(), Some("{message}"));
        assert_eq!(capabilities_for("Rust", false).broadcast.as_deref(), Some("say {message}"));
        // Pas de commande connue : aucune annonce plutôt qu'une commande au hasard.
        assert_eq!(capabilities_for("Satisfactory", false).broadcast, None);
        assert_eq!(capabilities_for("Valheim", false).broadcast, None);
        assert_eq!(capabilities_for("Palworld", false).broadcast, None);
        assert_eq!(capabilities_for_egg(Some(15), "Redis 7", false).broadcast, None);
    }

    #[test]
    fn unknown_egg_id_falls_back_to_the_heuristic() {
        // Egg absent de notre table : on garde la reconnaissance par nom (fork Minecraft ici).
        let c = capabilities_for_egg(Some(99_999), "Paper 1.21", false);
        assert_eq!(c.family, "minecraft");
        // Sans id du tout (ancienne réponse d'API), comportement historique inchangé.
        assert_eq!(capabilities_for_egg(None, "Satisfactory", false).mods, "ficsit");
    }

    #[test]
    fn player_data_is_java_only() {
        assert!(capabilities_for("Paper 1.21", false).player_data);
        assert!(capabilities_for_egg(Some(EGG_MINECRAFT_JAVA), "", false).player_data);
        // Bedrock a des joueurs, mais pas de `playerdata/` lisible par l'hébergeur.
        assert!(capabilities_for("Nukkit", true).players);
        assert!(!capabilities_for("Nukkit", true).player_data);
        assert!(!capabilities_for_egg(Some(EGG_MINECRAFT_BEDROCK), "", true).player_data);
        assert!(!capabilities_for("Satisfactory", false).player_data);
        assert!(!capabilities_for("Palworld", false).player_data);
    }
}
