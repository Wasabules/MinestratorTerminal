//! Capacités par jeu — dérivées du type d'« egg » MineStrator (Pterodactyl) pour piloter quelles
//! vues/fonctions l'UI expose. Portable : le même mapping sert au futur daemon/CLI et au gating des
//! outils MCP. On mappe finement Minecraft (Java/Bedrock) et Satisfactory ; tout autre jeu retombe
//! sur un défaut sûr (Aperçu + Console + SFTP + Assistant + Backups).

use serde::Serialize;

/// Ce qu'un serveur d'un jeu donné sait faire, du point de vue de l'app. Source unique : tout
/// (vues, console, IA) dérive de cette structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Marketplace de mods : `"minecraft"` (Modrinth/CF/Spigot via MineStrator) | `"ficsit"` | `"none"`.
    pub mods: String,
    /// Sauvegardes/snapshots (true partout aujourd'hui ; champ prévu pour le futur).
    pub backups: bool,
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
        mods: mods.into(),
        backups: true,
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
}
