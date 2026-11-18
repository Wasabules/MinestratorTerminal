//! **Audit de configuration et de sécurité** d'un serveur (outil MCP `audit_server`) : des
//! constats triés par gravité, chacun avec sa recommandation. Les règles sont pures ([`evaluate`])
//! et ne voient que des données déjà lues ; la collecte (API, SFTP) est à part.
//!
//! Contrôles retenus, selon le jeu (`games.rs`) :
//! - Minecraft Java : `server.properties` (`online-mode` — acceptable derrière Velocity avec
//!   secret, ou BungeeCord avec BungeeGuard, d'après `spigot.yml` et `paper-global.yml` —, RCON et
//!   force de son mot de passe, liste blanche, `enable-query`, distances de vue et de simulation
//!   face à la RAM), nombre d'opérateurs (`ops.json`), `-Xmx` face à la RAM du conteneur, mises à
//!   jour disponibles (vérification sans effet de bord de `updates.rs`, garde-fou compris), version
//!   de Java face à celle de Minecraft ;
//! - Minecraft Bedrock : authentification Xbox (`online-mode`) et liste blanche (`allow-list`) ;
//! - autre jeu : rien n'est défini, et la sortie le dit.
//!
//! **Aucun secret n'est renvoyé** : le mot de passe RCON est qualifié (« vide », « faible »,
//! « correct »), le secret Velocity seulement dit défini ou non.

use crate::findings::{self, short_list, Finding, Severity};
use crate::games::GameCapabilities;
use crate::jar_meta::version_nums;
use crate::updates::{compare_numbers, ContentUpdates};
use crate::{sftp, yaml_lite, Core, Error, Result};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Message d'un outil réservé à Minecraft Java appelé sur un autre jeu.
pub(crate) fn unsupported_game(caps: &GameCapabilities, what: &str) -> String {
    if caps.is_minecraft_bedrock() {
        format!("Minecraft Bedrock : {what} n'est pas géré (réservé à Minecraft Java).")
    } else {
        format!("Jeu non géré ({}) : {what} n'est défini que pour Minecraft Java.", caps.family)
    }
}

/// Édition de Minecraft auditée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Edition {
    #[default]
    Java,
    Bedrock,
}

/// Réglages de proxy côté serveur (jamais le secret lui-même).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ProxySettings {
    /// `spigot.yml` : `settings.bungeecord`.
    pub bungeecord: Option<bool>,
    /// `paper-global.yml` : `proxies.velocity.enabled` (ou `paper.yml` avant 1.19).
    pub velocity: Option<bool>,
    /// Secret Velocity défini (non vide).
    pub velocity_secret: Option<bool>,
}

/// Lit les réglages de proxy (fichiers absents : `None`).
pub(crate) fn proxy_settings(spigot: Option<&str>, paper_global: Option<&str>, paper_legacy: Option<&str>) -> ProxySettings {
    let mut p = ProxySettings::default();
    if let Some(t) = spigot {
        p.bungeecord = yaml_lite::as_bool(yaml_lite::path(&yaml_lite::parse(t), &["settings", "bungeecord"]));
    }
    let velocity = |v: &serde_json::Value, keys: &[&str]| {
        let mut base: Vec<&str> = keys.to_vec();
        base.push("enabled");
        let enabled = yaml_lite::as_bool(yaml_lite::path(v, &base));
        base.pop();
        base.push("secret");
        let secret = yaml_lite::path(v, &base).map(|s| yaml_lite::as_text(Some(s)).is_some_and(|s| !s.trim().is_empty()));
        (enabled, secret)
    };
    if let Some(t) = paper_global {
        (p.velocity, p.velocity_secret) = velocity(&yaml_lite::parse(t), &["proxies", "velocity"]);
    } else if let Some(t) = paper_legacy {
        (p.velocity, p.velocity_secret) = velocity(&yaml_lite::parse(t), &["settings", "velocity-support"]);
    }
    p
}

/// Force d'un mot de passe (jamais affiché).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strength {
    Empty,
    Weak,
    Strong,
}

impl Strength {
    fn label(self) -> &'static str {
        match self {
            Strength::Empty => "vide",
            Strength::Weak => "faible",
            Strength::Strong => "correct",
        }
    }
}

/// Mots de passe courants (comparés sans casse).
const COMMON_PASSWORDS: &[&str] = &[
    "password", "motdepasse", "minecraft", "admin", "administrator", "rcon", "root", "changeme", "change_me",
    "123456", "12345678", "123456789", "1234567890", "azerty", "qwerty", "azertyuiop", "qwertyuiop", "letmein",
    "welcome", "server", "serveur", "default", "test", "secret", "minestrator",
];

/// Faible : moins de 12 caractères, mot courant (éventuellement suivi de chiffres), un seul
/// caractère répété, ou une seule classe de caractères en dessous de 16.
pub(crate) fn password_strength(pw: &str) -> Strength {
    let pw = pw.trim();
    if pw.is_empty() {
        return Strength::Empty;
    }
    let len = pw.chars().count();
    let lower = pw.to_lowercase();
    let stem = lower.trim_end_matches(|c: char| c.is_ascii_digit() || "!?.*_-".contains(c));
    let classes = [
        pw.chars().any(|c| c.is_lowercase()),
        pw.chars().any(|c| c.is_uppercase()),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let first = pw.chars().next();
    let repeated = pw.chars().all(|c| Some(c) == first);
    if len < 12 || COMMON_PASSWORDS.contains(&lower.as_str()) || COMMON_PASSWORDS.contains(&stem) || repeated || (classes < 2 && len < 16) {
        Strength::Weak
    } else {
        Strength::Strong
    }
}

/// Taille de tas Java lue dans la commande de démarrage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Heap {
    Mb(u64),
    /// Variable de l'hôte (`{{SERVER_MEMORY}}`) : toute la RAM du conteneur.
    Container,
}

/// Valeur du DERNIER `-Xmx`/`-Xms` de la commande (c'est lui que la JVM retient).
pub(crate) fn parse_heap(cmd: &str, flag: &str) -> Option<Heap> {
    let tok = cmd.split_whitespace().rev().find(|t| t.starts_with(flag))?;
    let v = tok[flag.len()..].trim_matches(['"', '\'']);
    if v.contains("{{") || v.starts_with('$') {
        return Some(Heap::Container);
    }
    let split = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    let n: u64 = v[..split].parse().ok()?;
    let mb = match v[split..].to_ascii_lowercase().as_str() {
        "" => n / (1024 * 1024),
        "k" => n / 1024,
        "m" => n,
        "g" => n * 1024,
        "t" => n * 1024 * 1024,
        _ => return None,
    };
    Some(Heap::Mb(mb))
}

/// `-XX:MaxRAMPercentage=75.0` → 75.
pub(crate) fn max_ram_percentage(cmd: &str) -> Option<f64> {
    cmd.split_whitespace().rev().find_map(|t| t.strip_prefix("-XX:MaxRAMPercentage=")).and_then(|v| v.parse().ok())
}

/// Version de Java d'après l'image d'exécution (`reg.sttr.io/java/25_perf:latest`,
/// `ghcr.io/pterodactyl/yolks:java_21`, `eclipse-temurin:17-jre`, `openjdk:1.8`…).
pub(crate) fn java_from_image(image: &str) -> Option<u32> {
    let l = image.to_ascii_lowercase();
    for kw in ["java", "jdk", "jre", "temurin", "openjdk", "corretto", "zulu", "graalvm"] {
        let mut from = 0;
        while let Some(pos) = l[from..].find(kw) {
            let rest = &l[from + pos + kw.len()..];
            let rest = rest.trim_start_matches(['/', '_', ':', '-', '.']);
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                // `1.8` → Java 8.
                let n = if n == 1 {
                    rest[1..].strip_prefix('.').and_then(|r| r.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()).unwrap_or(1)
                } else {
                    n
                };
                if (7..=40).contains(&n) {
                    return Some(n);
                }
            }
            from += pos + kw.len();
        }
    }
    None
}

/// Java exigé par une version de Minecraft : `(minimum, maximum conseillé)`.
pub(crate) fn java_requirement(mc: &str) -> Option<(u32, Option<u32>)> {
    let v = version_nums(mc)?;
    let at_least = |min: &[u64]| compare_numbers(&v, min) != Ordering::Less;
    Some(if v[0] >= 2 {
        // Versions annuelles (26.1 et suivantes).
        (25, None)
    } else if at_least(&[1, 20, 5]) {
        (21, None)
    } else if at_least(&[1, 18]) {
        (17, None)
    } else if at_least(&[1, 17]) {
        (16, None)
    } else if at_least(&[1, 13]) {
        (8, Some(17))
    } else {
        (8, Some(8))
    })
}

/// Distances conseillées `(view, simulation)` pour une RAM de conteneur (Mo).
pub(crate) fn recommended_distances(ram_mb: i64) -> (i64, i64) {
    match ram_mb {
        r if r < 3072 => (8, 6),
        r if r < 6144 => (10, 10),
        r if r < 12288 => (14, 12),
        _ => (18, 14),
    }
}

/// Niveaux des opérateurs d'`ops.json` (les noms ne sont pas gardés).
pub(crate) fn parse_ops(json: &str) -> std::result::Result<Vec<i64>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| format!("ops.json invalide ({e})"))?;
    let list = v.as_array().ok_or("ops.json n'est pas une liste")?;
    Ok(list.iter().map(|o| o.get("level").and_then(serde_json::Value::as_i64).unwrap_or(4)).collect())
}

/// Plugins d'authentification (mode hors ligne protégé par mot de passe).
const AUTH_PLUGINS: &[&str] = &[
    "authme", "authmereloaded", "nlogin", "librelogin", "jpremium", "openlogin", "loginsecurity", "limboauth",
    "unifiedauth", "xauth", "crazylogin", "fastlogin",
];

/// Données de l'audit, déjà lues.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuditInput {
    pub edition: Edition,
    /// `server.properties` (valeurs brutes, secrets compris : jamais recopiées en sortie).
    pub properties: Option<BTreeMap<String, String>>,
    /// RAM du conteneur (Mo).
    pub ram_mb: Option<i64>,
    pub startup: Option<String>,
    pub java: Option<u32>,
    pub mc_version: Option<String>,
    /// Niveaux des opérateurs, ou raison de l'échec de lecture.
    pub ops: Option<std::result::Result<Vec<i64>, String>>,
    pub proxy: Option<ProxySettings>,
    /// Noms des plugins installés (hébergeur).
    pub plugins: Vec<String>,
    pub updates: Option<std::result::Result<ContentUpdates, String>>,
}

/// Résultat de l'audit.
#[derive(Debug, Default)]
pub(crate) struct AuditResult {
    pub findings: Vec<Finding>,
    /// Contrôles passés sans remarque.
    pub ok: Vec<String>,
    /// Contrôles impossibles (donnée manquante).
    pub unchecked: Vec<String>,
}

/// Applique les règles (pur).
pub(crate) fn evaluate(input: &AuditInput) -> AuditResult {
    let mut r = AuditResult::default();
    let java = input.edition == Edition::Java;
    let Some(props) = &input.properties else {
        r.unchecked.push("server.properties (illisible)".into());
        if java {
            check_heap(input, &mut r);
            check_java(input, &mut r);
            check_ops(input, &mut r);
            check_updates(input, &mut r);
        }
        return r;
    };
    let prop = |k: &str| props.get(k).map(|v| v.trim().to_ascii_lowercase());
    let has_plugin = |list: &[&str]| -> Option<String> {
        input
            .plugins
            .iter()
            .find(|p| {
                let n: String = p.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                list.iter().any(|a| n == *a || n.starts_with(a))
            })
            .cloned()
    };
    let ops_count = input.ops.as_ref().and_then(|o| o.as_ref().ok()).map(Vec::len);

    // --- online-mode ---------------------------------------------------------------
    let proxy = input.proxy.clone().unwrap_or_default();
    match (prop("online-mode").as_deref(), input.edition) {
        (None, _) => r.unchecked.push("online-mode (absent de server.properties)".into()),
        (Some("false"), Edition::Bedrock) => r.findings.push(Finding::new(
            Severity::Critical,
            "online-mode=false : l'authentification Xbox Live est désactivée, n'importe qui peut prendre le pseudo d'un autre joueur.",
            "Remets online-mode=true (set_property).",
        )),
        (Some("false"), Edition::Java) => {
            let usurp = ops_count.filter(|n| *n > 0).map(|n| format!(" ({n} opérateur(s) usurpable(s))")).unwrap_or_default();
            if proxy.velocity == Some(true) {
                if proxy.velocity_secret == Some(true) {
                    r.findings.push(Finding::new(
                        Severity::Info,
                        "online-mode=false derrière Velocity (forwarding moderne, secret défini) : normal, seules les connexions signées par le proxy sont acceptées.",
                        "Garde le secret identique sur le proxy et ne le partage pas.",
                    ));
                } else {
                    r.findings.push(Finding::new(
                        Severity::Critical,
                        format!("online-mode=false et forwarding Velocity activé SANS secret dans paper-global.yml : le forwarding ne peut pas protéger le serveur{usurp}."),
                        "Renseigne proxies.velocity.secret (le même que forwarding.secret du proxy), ou remets online-mode=true.",
                    ));
                }
            } else if proxy.bungeecord == Some(true) {
                match has_plugin(&["bungeeguard"]) {
                    Some(name) => r.findings.push(Finding::new(
                        Severity::Info,
                        format!("online-mode=false derrière BungeeCord (bungeecord: true) protégé par {name} : correct si son jeton est configuré des deux côtés."),
                        "Vérifie le jeton de BungeeGuard sur le proxy et sur ce serveur.",
                    )),
                    None => r.findings.push(Finding::new(
                        Severity::Critical,
                        format!("online-mode=false avec le forwarding BungeeCord (bungeecord: true) sans BungeeGuard : quiconque joint directement le port du serveur peut se connecter sous n'importe quel pseudo{usurp}."),
                        "Installe BungeeGuard (proxy et serveur), ou passe à Velocity avec le forwarding moderne et un secret.",
                    )),
                }
            } else if let Some(auth) = has_plugin(AUTH_PLUGINS) {
                r.findings.push(Finding::new(
                    Severity::Important,
                    format!("online-mode=false (mode hors ligne) sans proxy, protégé par {auth} : chaque compte dépend de son mot de passe{usurp}."),
                    "Impose un mot de passe fort aux opérateurs, ou repasse en online-mode=true si tous les joueurs ont un compte Microsoft.",
                ));
            } else {
                let unread = if input.proxy.is_none() { " (réglages de proxy non lus)" } else { "" };
                r.findings.push(Finding::new(
                    Severity::Critical,
                    format!("online-mode=false sans proxy configuré{unread} : n'importe qui peut se connecter sous le pseudo de n'importe qui, opérateurs compris{usurp}."),
                    "Remets online-mode=true (set_property), ou place le serveur derrière Velocity (forwarding moderne + secret) / BungeeCord + BungeeGuard.",
                ));
            }
        }
        (Some(_), Edition::Java) if proxy.velocity == Some(true) || proxy.bungeecord == Some(true) => r.findings.push(Finding::new(
            Severity::Important,
            "online-mode=true alors que le forwarding de proxy est activé : les joueurs passant par le proxy seront refusés.",
            "Derrière un proxy, mets online-mode=false sur ce serveur (l'authentification se fait sur le proxy).",
        )),
        (Some(_), _) => r.ok.push("online-mode=true (comptes vérifiés)".into()),
    }

    // --- Liste blanche -------------------------------------------------------------
    let wl_key = if input.edition == Edition::Bedrock && props.contains_key("allow-list") { "allow-list" } else { "white-list" };
    match prop(wl_key).as_deref() {
        Some("true") => {
            r.ok.push(format!("{wl_key} activée"));
            if java && prop("enforce-whitelist").as_deref() == Some("false") {
                r.findings.push(Finding::new(
                    Severity::Info,
                    "enforce-whitelist=false : un joueur retiré de la liste blanche reste connecté jusqu'à sa déconnexion.",
                    "Mets enforce-whitelist=true pour l'expulser aussitôt.",
                ));
            }
        }
        Some(_) => r.findings.push(Finding::new(
            Severity::Info,
            format!("{wl_key} désactivée : serveur ouvert à tout compte (normal pour un serveur public)."),
            format!("Pour un serveur privé, active {wl_key} et ajoute les joueurs (player_action whitelist_add)."),
        )),
        None => r.unchecked.push(format!("{wl_key} (absent)")),
    }

    if !java {
        r.unchecked.push("RCON, query, distances, opérateurs, -Xmx, Java et mises à jour : non applicables à Bedrock".into());
        return r;
    }

    // --- RCON ------------------------------------------------------------------------
    match prop("enable-rcon").as_deref() {
        Some("true") => match props.get("rcon.password").map(|p| password_strength(p)) {
            Some(Strength::Empty) => r.findings.push(Finding::new(
                Severity::Important,
                "RCON activé avec un mot de passe vide : Minecraft refuse alors de démarrer RCON (réglage incohérent).",
                "Désactive enable-rcon, ou définis un mot de passe long et aléatoire (au moins 16 caractères).",
            )),
            Some(Strength::Weak) => r.findings.push(Finding::new(
                Severity::Critical,
                "RCON activé avec un mot de passe faible : la console complète du serveur est exposée sur le port RCON.",
                "Change le mot de passe RCON pour une valeur longue et aléatoire (au moins 16 caractères), ou désactive enable-rcon.",
            )),
            Some(s @ Strength::Strong) => r.findings.push(Finding::new(
                Severity::Info,
                format!("RCON activé (mot de passe {}) : il donne la console complète à qui le connaît.", s.label()),
                "Ne l'active que si un outil s'en sert ; sinon désactive enable-rcon.",
            )),
            None => r.unchecked.push("mot de passe RCON (réglage absent)".into()),
        },
        Some(_) => r.ok.push("RCON désactivé".into()),
        None => {}
    }

    // --- Query ---------------------------------------------------------------------
    if prop("enable-query").as_deref() == Some("true") {
        r.findings.push(Finding::new(
            Severity::Info,
            "enable-query=true : le port query (UDP) expose la liste des joueurs, la version et, sous Bukkit, les plugins installés.",
            "Désactive-le si aucun site de statistiques ne s'en sert (le panel de l'hébergeur peut l'utiliser pour les joueurs connectés : vérifie après coup).",
        ));
    } else if prop("enable-query").is_some() {
        r.ok.push("query désactivé".into());
    }

    // --- Distances --------------------------------------------------------------------
    match input.ram_mb {
        Some(ram) if ram > 0 => {
            let (rv, rs) = recommended_distances(ram);
            let view = props.get("view-distance").and_then(|v| v.trim().parse::<i64>().ok());
            let sim = props.get("simulation-distance").and_then(|v| v.trim().parse::<i64>().ok());
            let mut ok = true;
            for (key, value, rec) in [("view-distance", view, rv), ("simulation-distance", sim, rs)] {
                let Some(v) = value else { continue };
                if v > rec {
                    ok = false;
                    let sev = if v * 2 >= rec * 3 || v > 20 { Severity::Important } else { Severity::Info };
                    r.findings.push(Finding::new(
                        sev,
                        format!("{key}={v} élevée pour {ram} Mo de RAM (repère : {rec} au plus) : la charge croît avec le carré de la distance, par joueur."),
                        format!("Baisse {key} vers {rec} (set_property) ; sous Paper, les distances par monde se règlent aussi dans paper-world-defaults.yml."),
                    ));
                }
            }
            if let (Some(v), Some(s)) = (view, sim) {
                if s > v {
                    ok = false;
                    r.findings.push(Finding::new(
                        Severity::Info,
                        format!("simulation-distance ({s}) dépasse view-distance ({v}) : la simulation reste bornée par les chunks chargés."),
                        "Aligne simulation-distance sur view-distance ou en dessous.",
                    ));
                }
            }
            if ok && (view.is_some() || sim.is_some()) {
                r.ok.push(format!(
                    "distances (vue {}, simulation {}) adaptées à {ram} Mo",
                    view.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    sim.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
                ));
            }
        }
        _ => r.unchecked.push("distances face à la RAM (RAM du conteneur inconnue)".into()),
    }

    check_ops(input, &mut r);
    check_heap(input, &mut r);
    check_java(input, &mut r);
    check_updates(input, &mut r);
    r
}

fn check_ops(input: &AuditInput, r: &mut AuditResult) {
    match &input.ops {
        None => r.unchecked.push("opérateurs (ops.json non lu)".into()),
        Some(Err(e)) => r.unchecked.push(format!("opérateurs ({e})")),
        Some(Ok(levels)) => {
            let n = levels.len();
            let admins = levels.iter().filter(|l| **l >= 4).count();
            if n <= 3 {
                r.ok.push(format!("{n} opérateur(s)"));
            } else {
                r.findings.push(Finding::new(
                    if n > 10 { Severity::Important } else { Severity::Info },
                    format!("{n} opérateurs dans ops.json (dont {admins} de niveau 4) : chacun peut tout faire sur le serveur."),
                    "Réserve le statut d'opérateur à quelques administrateurs de confiance (player_action op_remove) et gère le reste avec un plugin de permissions.",
                ));
            }
        }
    }
}

fn check_heap(input: &AuditInput, r: &mut AuditResult) {
    let Some(cmd) = &input.startup else {
        r.unchecked.push("-Xmx (commande de démarrage non lue)".into());
        return;
    };
    let Some(ram) = input.ram_mb.filter(|r| *r > 0) else {
        r.unchecked.push("-Xmx face à la RAM (RAM du conteneur inconnue)".into());
        return;
    };
    let xmx = parse_heap(cmd, "-Xmx");
    let (heap_mb, label) = match (xmx, max_ram_percentage(cmd)) {
        (Some(Heap::Mb(m)), _) => (m as f64, format!("-Xmx {m} Mo")),
        (Some(Heap::Container), _) => (ram as f64, "-Xmx égal à toute la RAM du conteneur (variable de l'hôte)".to_string()),
        (None, Some(p)) => (ram as f64 * p / 100.0, format!("-XX:MaxRAMPercentage={p}")),
        (None, None) => {
            r.findings.push(Finding::new(
                Severity::Important,
                "Aucun -Xmx dans la commande de démarrage : la JVM se limite par défaut à 25 % de la RAM du conteneur.",
                format!("Ajoute -Xmx{}M (≈ 80 % de {ram} Mo) avec set_startup_params.", ram * 80 / 100),
            ));
            return;
        }
    };
    let pct = heap_mb / ram as f64 * 100.0;
    let rec = ram * 80 / 100;
    if pct > 100.0 {
        r.findings.push(Finding::new(
            Severity::Critical,
            format!("{label} dépasse la RAM du conteneur ({ram} Mo, {pct:.0} %) : le conteneur sera tué (OOM) dès que le tas grossira."),
            format!("Ramène -Xmx vers {rec}M (≈ 80 % de la RAM) avec set_startup_params."),
        ));
    } else if pct > 92.0 {
        r.findings.push(Finding::new(
            Severity::Important,
            format!("{label} = {pct:.0} % de la RAM du conteneur ({ram} Mo) : trop peu de marge hors tas (métaspace, threads, tampons réseau) → arrêts brutaux par manque de mémoire."),
            format!("Vise 75 à 85 % de la RAM, soit environ -Xmx{rec}M (set_startup_params)."),
        ));
    } else if pct < 50.0 {
        r.findings.push(Finding::new(
            Severity::Info,
            format!("{label} = {pct:.0} % seulement de la RAM du conteneur ({ram} Mo) : mémoire sous-exploitée."),
            format!("Monte -Xmx vers {rec}M (≈ 80 %) si le serveur manque de mémoire."),
        ));
    } else {
        r.ok.push(format!("{label} = {pct:.0} % de {ram} Mo"));
    }
    if let (Some(Heap::Mb(xms)), Some(Heap::Mb(xmx))) = (parse_heap(cmd, "-Xms"), xmx) {
        if xms > xmx {
            r.findings.push(Finding::new(
                Severity::Critical,
                format!("-Xms ({xms} Mo) dépasse -Xmx ({xmx} Mo) : la JVM refuse de démarrer."),
                "Mets -Xms au plus égal à -Xmx (set_startup_params).",
            ));
        }
    }
}

fn check_java(input: &AuditInput, r: &mut AuditResult) {
    let (Some(java), Some(mc)) = (input.java, input.mc_version.as_deref()) else {
        r.unchecked.push("version de Java face à Minecraft (l'une des deux est inconnue)".into());
        return;
    };
    let Some((min, max)) = java_requirement(mc) else {
        r.unchecked.push(format!("version de Java (Minecraft « {mc} » non reconnu)"));
        return;
    };
    if java < min {
        r.findings.push(Finding::new(
            Severity::Critical,
            format!("Java {java} trop ancien pour Minecraft {mc} (Java {min} au minimum) : le serveur ne démarre pas (UnsupportedClassVersionError)."),
            format!("Choisis l'image Java {min} (ou plus récente) dans les paramètres de démarrage du panel."),
        ));
    } else if let Some(max) = max.filter(|m| java > *m) {
        let old = compare_numbers(&version_nums(mc).unwrap_or_default(), &[1, 13]) == Ordering::Less;
        r.findings.push(Finding::new(
            if old { Severity::Important } else { Severity::Info },
            format!("Java {java} plus récent que ce que Minecraft {mc} attend (Java {max} conseillé) : mods et plugins anciens peuvent planter."),
            format!("Choisis l'image Java {max} dans les paramètres de démarrage du panel."),
        ));
    } else {
        r.ok.push(format!("Java {java} adapté à Minecraft {mc}"));
    }
}

fn check_updates(input: &AuditInput, r: &mut AuditResult) {
    let u = match &input.updates {
        None => {
            r.unchecked.push("mises à jour (non vérifiées pour ce serveur)".into());
            return;
        }
        Some(Err(e)) => {
            r.unchecked.push(format!("mises à jour ({e})"));
            return;
        }
        Some(Ok(u)) => u,
    };
    if u.mc_version.is_none() {
        r.unchecked.push("mises à jour (version de Minecraft inconnue de l'hébergeur)".into());
        return;
    }
    for e in &u.errors {
        r.unchecked.push(format!("mises à jour des {} ({})", e.kind.as_str(), e.error.message));
    }
    if u.updates.is_empty() {
        r.ok.push(format!("aucune mise à jour en attente ({} élément(s) examiné(s))", u.examined));
    } else {
        let list: Vec<String> = u
            .updates
            .iter()
            .map(|x| {
                let from = if x.current_version.is_empty() { String::new() } else { format!("{} → ", x.current_version) };
                format!("{} {from}{}", x.name, x.new_version)
            })
            .collect();
        let mut msg = format!("{} mise(s) à jour disponible(s) : {}", list.len(), short_list(&list, 12));
        if !u.ignored.is_empty() {
            let _ = write!(
                msg,
                " ; {} proposition(s) de l'hébergeur écartée(s) par le garde-fou (autre plateforme ou version non plus récente), à ne pas installer",
                u.ignored.len()
            );
        }
        msg.push('.');
        r.findings.push(Finding::new(
            Severity::Info,
            msg,
            "Détail et clés : check_updates ; puis create_snapshot et update_mods, et redémarre le serveur.",
        ));
    }
}

/// Rapport texte (pur).
pub(crate) fn render(edition: Edition, header: &str, result: &AuditResult) -> String {
    let mut out = format!(
        "Audit de configuration et de sécurité — Minecraft {}{}{header}\n",
        if edition == Edition::Java { "Java" } else { "Bedrock" },
        if header.is_empty() { "" } else { " " }
    );
    let _ = writeln!(out, "Bilan : {}.", findings::tally(&result.findings));
    findings::render(&result.findings, "Aucun problème détecté.", &mut out);
    if !result.ok.is_empty() {
        let _ = writeln!(out, "\nSans remarque : {}.", result.ok.join(" ; "));
    }
    if !result.unchecked.is_empty() {
        let _ = writeln!(out, "Non vérifié : {}.", result.unchecked.join(" ; "));
    }
    out.push_str("Secrets jamais affichés : le mot de passe RCON est seulement qualifié (vide, faible ou correct).");
    out
}

// --- Collecte ----------------------------------------------------------------------

/// Fichiers lus par SFTP pour l'audit.
#[derive(Default)]
struct AuditFiles {
    ops: Option<std::result::Result<String, String>>,
    spigot: Option<String>,
    paper_global: Option<String>,
    paper_legacy: Option<String>,
}

/// Texte d'un fichier, `None` s'il est absent ou illisible ; `Err` si la session est morte.
async fn read_optional(conn: &sftp::SftpConn, path: &str) -> Result<Option<String>> {
    match sftp::read_text(conn, path).await {
        Ok(t) => Ok(Some(t)),
        Err(e) if sftp::is_session_dead(&e) => Err(e),
        Err(_) => Ok(None),
    }
}

async fn read_audit_files(conn: &sftp::SftpConn) -> Result<AuditFiles> {
    let ops = match sftp::read_text(conn, "/ops.json").await {
        Ok(t) => Some(Ok(t)),
        Err(e) if sftp::is_session_dead(&e) => return Err(e),
        Err(e) => Some(Err(e.to_string())),
    };
    Ok(AuditFiles {
        ops,
        spigot: read_optional(conn, "/spigot.yml").await?,
        paper_global: read_optional(conn, "/config/paper-global.yml").await?,
        paper_legacy: read_optional(conn, "/paper.yml").await?,
    })
}

impl Core {
    /// Capacités du jeu d'un serveur (`games.rs`), d'après la liste des serveurs du compte.
    pub(crate) async fn server_capabilities(&self, id: i64) -> Result<GameCapabilities> {
        self.list_servers()
            .await?
            .servers
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.capabilities)
            .ok_or_else(|| Error::Invalid(format!("Serveur {id} introuvable sur ce compte.")))
    }

    /// Audit de configuration et de sécurité (outil MCP `audit_server`). Lecture seule : la
    /// vérification des mises à jour est une pure comparaison côté hébergeur.
    pub(crate) async fn audit_server(&self, id: i64, force_updates: bool) -> Result<String> {
        let caps = self.server_capabilities(id).await?;
        let edition = if caps.is_minecraft_java() {
            Edition::Java
        } else if caps.is_minecraft_bedrock() {
            Edition::Bedrock
        } else {
            return Ok(format!(
                "Jeu non géré ({}) : aucun contrôle de configuration n'est défini pour ce jeu (seuls Minecraft Java et Bedrock le sont).",
                caps.family
            ));
        };
        let java = edition == Edition::Java;
        let token = self.token()?;
        let (props, light, startup, mc, plugins) = tokio::join!(
            self.server_properties(id),
            self.api.get_live_light(&token, id),
            async {
                if java {
                    self.get_startup(id).await.ok()
                } else {
                    None
                }
            },
            async {
                if java {
                    self.api.minecraft_version(&token, id).await.ok()
                } else {
                    None
                }
            },
            async {
                if java {
                    self.installed_plugins(id).await.unwrap_or_default()
                } else {
                    Vec::new()
                }
            },
        );
        let files = if java {
            match self.with_sftp(id, |conn| async move { read_audit_files(&conn).await }).await {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::debug!("audit : fichiers non lus ({e})");
                    None
                }
            }
        } else {
            None
        };
        let updates = if java && caps.mods == "minecraft" {
            Some(self.content_updates(id, force_updates).await.map_err(|e| e.to_string()))
        } else {
            None
        };
        let mc_version = mc.as_ref().map(|m| m.version.clone()).filter(|v| !v.is_empty());
        let input = AuditInput {
            edition,
            properties: props.ok().map(|p| p.properties),
            ram_mb: light.ok().map(|l| l.memory.limit).filter(|m| *m > 0),
            java: startup.as_ref().and_then(|s| java_from_image(&s.image)),
            startup: startup.map(|s| s.command),
            mc_version: mc_version.clone(),
            ops: files.as_ref().and_then(|f| f.ops.clone()).map(|o| o.and_then(|t| parse_ops(&t))),
            proxy: files.as_ref().map(|f| proxy_settings(f.spigot.as_deref(), f.paper_global.as_deref(), f.paper_legacy.as_deref())),
            plugins: plugins.into_iter().map(|p| p.name).collect(),
            updates,
        };
        let result = evaluate(&input);
        // En-tête : « 26.1.2 (paper) · Java 25 · RAM 4000 Mo ».
        let mut header = String::new();
        if let Some(v) = &mc_version {
            header.push_str(v);
            if let Some(src) = mc.as_ref().map(|m| m.source.as_str()).filter(|s| !s.is_empty()) {
                let _ = write!(header, " ({src})");
            }
        }
        if let Some(j) = input.java {
            let _ = write!(header, "{}Java {j}", if header.is_empty() { "· " } else { " · " });
        }
        if let Some(ram) = input.ram_mb {
            let _ = write!(header, "{}RAM {ram} Mo", if header.is_empty() { "· " } else { " · " });
        }
        Ok(self.redact_ai(&render(edition, &header, &result)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{ContentKind, ContentUpdate, IgnoreReason, IgnoredUpdate};

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// `server.properties` d'un serveur réel (relevé du 2026-11-18 : online-mode=false, query
    /// activé, RCON désactivé), 4000 Mo, `-Xmx3151M`, Java 25, Minecraft 26.1.2.
    fn real_input() -> AuditInput {
        AuditInput {
            edition: Edition::Java,
            properties: Some(props(&[
                ("online-mode", "false"),
                ("enable-rcon", "false"),
                ("rcon.password", ""),
                ("white-list", "false"),
                ("enforce-whitelist", "false"),
                ("enable-query", "true"),
                ("view-distance", "10"),
                ("simulation-distance", "10"),
            ])),
            ram_mb: Some(4000),
            startup: Some("java -Xms128M -Xmx3151M -Dterminal.jline=false -Dterminal.ansi=true -jar {{SERVER_JARFILE}}".into()),
            java: java_from_image("reg.sttr.io/java/25_perf:latest"),
            mc_version: Some("26.1.2".into()),
            ops: Some(parse_ops(r#"[{"uuid":"7f521663-1b2f-3e5b-9544-7f5250ff6b67","name":"VBProbe","level":4,"bypassesPlayerLimit":false}]"#)),
            proxy: Some(proxy_settings(
                Some("settings:\n  debug: false\n  bungeecord: false\n"),
                Some("proxies:\n  bungee-cord:\n    online-mode: true\n  velocity:\n    enabled: false\n    online-mode: true\n    secret: ''\n"),
                None,
            )),
            plugins: vec!["VoxelBench".into()],
            updates: None,
        }
    }

    fn by(r: &AuditResult, sev: Severity) -> Vec<String> {
        r.findings.iter().filter(|f| f.severity == sev).map(|f| f.message.clone()).collect()
    }

    #[test]
    fn audits_a_real_offline_server() {
        let r = evaluate(&real_input());
        let crit = by(&r, Severity::Critical);
        assert_eq!(crit.len(), 1, "{crit:?}");
        assert!(crit[0].contains("online-mode=false sans proxy") && crit[0].contains("1 opérateur(s) usurpable(s)"), "{crit:?}");
        let info = by(&r, Severity::Info);
        assert!(info.iter().any(|m| m.contains("enable-query=true")));
        assert!(info.iter().any(|m| m.contains("white-list désactivée")));
        assert!(by(&r, Severity::Important).is_empty(), "{:?}", r.findings);
        assert!(r.ok.iter().any(|o| o == "RCON désactivé"));
        assert!(r.ok.iter().any(|o| o.contains("-Xmx 3151 Mo = 79 % de 4000 Mo")), "{:?}", r.ok);
        assert!(r.ok.iter().any(|o| o == "Java 25 adapté à Minecraft 26.1.2"), "{:?}", r.ok);
        assert!(r.ok.iter().any(|o| o.contains("distances (vue 10, simulation 10)")), "{:?}", r.ok);
        assert!(r.unchecked.iter().any(|u| u.contains("mises à jour")));
    }

    #[test]
    fn online_mode_is_fine_behind_a_well_configured_proxy() {
        let mut input = real_input();
        input.proxy = Some(proxy_settings(None, Some("proxies:\n  velocity:\n    enabled: true\n    secret: 'abc'\n"), None));
        let r = evaluate(&input);
        assert!(by(&r, Severity::Critical).is_empty());
        assert!(by(&r, Severity::Info).iter().any(|m| m.contains("derrière Velocity")));

        input.proxy = Some(proxy_settings(None, Some("proxies:\n  velocity:\n    enabled: true\n    secret: ''\n"), None));
        assert!(by(&evaluate(&input), Severity::Critical)[0].contains("SANS secret"));

        input.proxy = Some(proxy_settings(Some("settings:\n  bungeecord: true\n"), None, None));
        assert!(by(&evaluate(&input), Severity::Critical)[0].contains("sans BungeeGuard"));
        input.plugins.push("BungeeGuard".into());
        assert!(by(&evaluate(&input), Severity::Critical).is_empty());

        // Ancien paper.yml (avant 1.19).
        let legacy = proxy_settings(None, None, Some("settings:\n  velocity-support:\n    enabled: true\n    secret: x\n"));
        assert_eq!((legacy.velocity, legacy.velocity_secret), (Some(true), Some(true)));

        // Mode hors ligne protégé par un plugin d'authentification : important, pas critique.
        let mut auth = real_input();
        auth.plugins = vec!["AuthMe".into()];
        let r = evaluate(&auth);
        assert!(by(&r, Severity::Critical).is_empty());
        assert!(by(&r, Severity::Important)[0].contains("AuthMe"));

        // online-mode=true alors qu'un proxy est configuré : les joueurs du proxy sont refusés.
        let mut both = real_input();
        both.properties.as_mut().unwrap().insert("online-mode".into(), "true".into());
        both.proxy = Some(proxy_settings(Some("settings:\n  bungeecord: true\n"), None, None));
        assert!(by(&evaluate(&both), Severity::Important).iter().any(|m| m.contains("online-mode=true alors que")));
    }

    #[test]
    fn rcon_password_is_qualified_never_shown() {
        let mut input = real_input();
        let p = input.properties.as_mut().unwrap();
        p.insert("enable-rcon".into(), "true".into());
        p.insert("rcon.password".into(), "minecraft123".into());
        let r = evaluate(&input);
        let crit = by(&r, Severity::Critical);
        assert!(crit.iter().any(|m| m.contains("RCON activé avec un mot de passe faible")), "{crit:?}");
        let text = render(Edition::Java, "x", &r);
        assert!(!text.contains("minecraft123"), "le secret ne doit jamais sortir");
        // Le texte survit au filtre d'anonymisation (aucun « clé: valeur » sensible).
        assert_eq!(crate::redact::redact(&text), text);

        assert_eq!(password_strength(""), Strength::Empty);
        assert_eq!(password_strength("password"), Strength::Weak);
        assert_eq!(password_strength("Minecraft2024!"), Strength::Weak, "mot courant suivi de chiffres");
        assert_eq!(password_strength("aaaaaaaaaaaaaaaaaaaa"), Strength::Weak);
        assert_eq!(password_strength("abcdefghijklm"), Strength::Weak, "une seule classe sous 16 caractères");
        assert_eq!(password_strength("k9#Vq2!mZ7rTx4Lp"), Strength::Strong);
    }

    #[test]
    fn heap_rules() {
        assert_eq!(parse_heap("java -Xmx4G -jar x", "-Xmx"), Some(Heap::Mb(4096)));
        assert_eq!(parse_heap("java -Xmx512m -Xmx2048M -jar x", "-Xmx"), Some(Heap::Mb(2048)), "le dernier gagne");
        assert_eq!(parse_heap("java -Xmx{{SERVER_MEMORY}}M -jar x", "-Xmx"), Some(Heap::Container));
        assert_eq!(parse_heap("java -jar x", "-Xmx"), None);
        assert_eq!(max_ram_percentage("java -XX:MaxRAMPercentage=75.0 -jar x"), Some(75.0));

        let mut input = real_input();
        input.startup = Some("java -Xmx5000M -jar x".into());
        assert!(by(&evaluate(&input), Severity::Critical).iter().any(|m| m.contains("dépasse la RAM du conteneur")));
        input.startup = Some("java -Xmx{{SERVER_MEMORY}}M -jar x".into());
        assert!(by(&evaluate(&input), Severity::Important).iter().any(|m| m.contains("100 %")));
        input.startup = Some("java -Xmx1G -jar x".into());
        assert!(by(&evaluate(&input), Severity::Info).iter().any(|m| m.contains("sous-exploitée")));
        input.startup = Some("java -jar x".into());
        assert!(by(&evaluate(&input), Severity::Important).iter().any(|m| m.contains("Aucun -Xmx")));
        input.startup = Some("java -Xms4G -Xmx3G -jar x".into());
        assert!(by(&evaluate(&input), Severity::Critical).iter().any(|m| m.contains("-Xms")));
    }

    #[test]
    fn java_versions() {
        assert_eq!(java_from_image("reg.sttr.io/java/25_perf:latest"), Some(25));
        assert_eq!(java_from_image("reg.sttr.io/java/8_perf:latest"), Some(8));
        assert_eq!(java_from_image("ghcr.io/pterodactyl/yolks:java_21"), Some(21));
        assert_eq!(java_from_image("eclipse-temurin:17-jre"), Some(17));
        assert_eq!(java_from_image("openjdk:1.8"), Some(8));
        assert_eq!(java_from_image("ghcr.io/parkervcp/yolks:nodejs_20"), None);
        assert_eq!(java_requirement("26.1.2"), Some((25, None)));
        assert_eq!(java_requirement("1.21.4"), Some((21, None)));
        assert_eq!(java_requirement("1.20.4"), Some((17, None)));
        assert_eq!(java_requirement("1.12.2"), Some((8, Some(8))));
        assert_eq!(java_requirement("24w14a"), None);

        let mut input = real_input();
        input.java = Some(21);
        assert!(by(&evaluate(&input), Severity::Critical).iter().any(|m| m.contains("Java 21 trop ancien pour Minecraft 26.1.2")));
        input.mc_version = Some("1.12.2".into());
        input.java = Some(17);
        assert!(by(&evaluate(&input), Severity::Important).iter().any(|m| m.contains("Java 8 conseillé")));
    }

    #[test]
    fn distances_ops_and_whitelist() {
        let mut input = real_input();
        let p = input.properties.as_mut().unwrap();
        p.insert("view-distance".into(), "24".into());
        p.insert("simulation-distance".into(), "16".into());
        p.insert("white-list".into(), "true".into());
        input.ops = Some(Ok(vec![4; 12]));
        let r = evaluate(&input);
        let imp = by(&r, Severity::Important);
        assert!(imp.iter().any(|m| m.contains("view-distance=24")), "{imp:?}");
        assert!(imp.iter().any(|m| m.contains("simulation-distance=16")), "{imp:?}");
        // Légèrement au-dessus du repère : simple information.
        let mut mild = real_input();
        mild.properties.as_mut().unwrap().insert("simulation-distance".into(), "12".into());
        mild.properties.as_mut().unwrap().insert("view-distance".into(), "12".into());
        let r2 = evaluate(&mild);
        assert!(by(&r2, Severity::Info).iter().any(|m| m.contains("simulation-distance=12")));
        assert!(by(&r2, Severity::Important).is_empty());
        assert!(imp.iter().any(|m| m.contains("12 opérateurs")), "{imp:?}");
        assert!(by(&r, Severity::Info).iter().any(|m| m.contains("enforce-whitelist=false")));
        assert_eq!(recommended_distances(2048), (8, 6));
        assert_eq!(recommended_distances(16_000), (18, 14));
        assert_eq!(parse_ops("[]").unwrap().len(), 0);
        assert!(parse_ops("{").is_err());
    }

    #[test]
    fn updates_respect_the_guard_and_bedrock_is_scoped() {
        let mut input = real_input();
        input.updates = Some(Ok(ContentUpdates {
            mc_version: Some("26.1.2".into()),
            platform: Some("paper".into()),
            checked_at: 0,
            examined: 3,
            untracked: 0,
            updates: vec![ContentUpdate {
                kind: ContentKind::Plugins,
                key: "Chunky-Paper-1.5.66.jar".into(),
                name: "Chunky".into(),
                enabled: true,
                current_version: "1.5.66".into(),
                new_version: "1.5.70".into(),
                new_filename: "Chunky-Paper-1.5.70.jar".into(),
                source: "modrinth".into(),
            }],
            ignored: vec![IgnoredUpdate {
                kind: ContentKind::Plugins,
                key: "LuckPerms-Bukkit-5.5.71.jar".into(),
                name: "LuckPerms".into(),
                current_version: "5.5.71".into(),
                new_version: "5.5.71".into(),
                new_filename: "LuckPerms-Velocity-5.5.71.jar".into(),
                reason: IgnoreReason::OtherPlatform,
            }],
            errors: vec![],
        }));
        let r = evaluate(&input);
        let up = by(&r, Severity::Info).into_iter().find(|m| m.contains("mise(s) à jour")).unwrap();
        assert!(up.contains("Chunky 1.5.66 → 1.5.70") && up.contains("1 proposition(s)") && up.contains("à ne pas installer"), "{up}");
        assert!(!up.contains("Velocity"), "une variante écartée n'est jamais proposée");

        let bedrock = AuditInput {
            edition: Edition::Bedrock,
            properties: Some(props(&[("online-mode", "false"), ("allow-list", "false")])),
            ..Default::default()
        };
        let r = evaluate(&bedrock);
        assert!(by(&r, Severity::Critical)[0].contains("Xbox"));
        assert!(by(&r, Severity::Info)[0].contains("allow-list désactivée"));
        assert!(r.unchecked.iter().any(|u| u.contains("non applicables à Bedrock")));
    }

    #[test]
    fn unsupported_games_say_so() {
        let sat = crate::games::capabilities_for("Satisfactory", false);
        assert!(unsupported_game(&sat, "X").contains("Jeu non géré (satisfactory)"));
        let bedrock = crate::games::capabilities_for("Nukkit", true);
        assert!(unsupported_game(&bedrock, "X").starts_with("Minecraft Bedrock"));
        let r = evaluate(&AuditInput { properties: None, ..Default::default() });
        assert!(r.unchecked.iter().any(|u| u.contains("server.properties")));
    }
}
