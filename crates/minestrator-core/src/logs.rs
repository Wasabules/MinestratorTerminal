//! Journaux **fichier** : rotation quotidienne dans `<dossier de données>/logs/`, 7 jours conservés,
//! et export **anonymisé** à joindre à un rapport de bug.
//!
//! Sans fichier, les journaux sont perdus là où ils servent le plus : l'app desktop release n'a pas
//! de console sous Windows (`windows_subsystem = "windows"`), et la sortie standard native n'est
//! pas reliée à logcat sur Android.
//!
//! Écriture **synchrone** (pas de `non_blocking`) : au niveau `info` le volume est faible, et une
//! écriture tamponnée perdrait les dernières lignes quand Tauri termine le processus sans détruire
//! le tampon — précisément celles qui expliquent un plantage.

use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const PREFIX: &str = "minestrator";
const SUFFIX: &str = "log";

/// Nombre de fichiers journaliers conservés.
pub const KEEP_FILES: usize = 7;

/// Taille par défaut d'un export (assez pour quelques jours d'usage normal, lisible d'un bloc).
pub const EXPORT_MAX_BYTES: usize = 512 * 1024;

/// Dossier des journaux (`<dossier de données>/logs`).
pub fn logs_dir() -> Option<PathBuf> {
    crate::util::data_dir().map(|d| d.join("logs"))
}

/// Installe le subscriber global : sortie standard + fichier tournant. `default_filter` sert quand
/// `RUST_LOG` est absent. Sans dossier de données utilisable, journalise sur la sortie standard
/// seulement. Sans effet si un subscriber est déjà installé.
pub fn init(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let file_layer = logs_dir().and_then(|d| file_appender(&d)).map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(appender)
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(file_layer)
        .try_init();
}

fn file_appender(dir: &Path) -> Option<RollingFileAppender> {
    std::fs::create_dir_all(dir).ok()?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(PREFIX)
        .filename_suffix(SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(dir)
        .ok()
}

/// Journaux récents, **anonymisés** (cf. [`crate::redact`]) : au plus `max_bytes`, en partant des
/// plus récents, restitués dans l'ordre chronologique. Chaîne vide s'il n'y a aucun journal.
pub fn export_recent(max_bytes: usize) -> String {
    logs_dir()
        .map(|d| export_from(&d, max_bytes))
        .unwrap_or_default()
}

fn export_from(dir: &Path, max_bytes: usize) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name.starts_with(PREFIX) && name.ends_with(SUFFIX);
            ours.then(|| (name, e.path()))
        })
        .collect();
    // « minestrator.AAAA-MM-JJ.log » : l'ordre lexical est l'ordre chronologique.
    files.sort();

    let mut chunks = Vec::new();
    let mut budget = max_bytes;
    for (name, path) in files.iter().rev() {
        if budget == 0 {
            break;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let part = tail_lines(&text, budget);
        let truncated = part.len() < text.len();
        budget -= part.len();
        chunks.push(format!("===== {name} =====\n{part}"));
        // Fichier tronqué = budget épuisé : ne pas y accoler des lignes plus anciennes.
        if truncated {
            break;
        }
    }
    chunks.reverse();
    crate::redact::redact(&chunks.join("\n"))
}

/// Au plus les `max` derniers octets de `s`, en commençant à une ligne entière (jamais au milieu
/// d'un caractère ni d'une ligne).
fn tail_lines(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    match s[start..].find('\n') {
        Some(i) => &s[start + i + 1..],
        None => &s[s.len()..],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minestrator-test-logs-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn export_is_chronological_bounded_and_redacted() {
        let dir = test_dir("export");
        std::fs::write(dir.join("minestrator.2026-11-16.log"), "veille 1\nveille 2\n").unwrap();
        std::fs::write(
            dir.join("minestrator.2026-11-17.log"),
            "jour 1 admin@example.net\njour 2 10.0.0.42\n",
        )
        .unwrap();
        std::fs::write(dir.join("autre.txt"), "à ignorer").unwrap();

        let all = export_from(&dir, 1024);
        let veille = all.find("veille 1").unwrap();
        let jour = all.find("jour 1").unwrap();
        assert!(veille < jour, "ordre chronologique");
        assert!(!all.contains("à ignorer"));
        assert!(!all.contains("admin@example.net"), "e-mail anonymisé");
        assert!(!all.contains("10.0.0.42"), "IPv4 anonymisée");

        // Budget serré : seule la fin du fichier le plus récent est gardée, en lignes entières,
        // sans y accoler la veille.
        let short = export_from(&dir, 20);
        assert!(short.contains("jour 2"));
        assert!(!short.contains("jour 1"));
        assert!(!short.contains("veille"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_dir_exports_nothing() {
        let dir = std::env::temp_dir().join("minestrator-test-logs-absent-dir");
        assert_eq!(export_from(&dir, 1024), "");
    }

    #[test]
    fn tail_lines_cuts_on_line_and_char_boundaries() {
        assert_eq!(tail_lines("a\nb\n", 10), "a\nb\n");
        assert_eq!(tail_lines("ligne é\nfin\n", 5), "fin\n");
        // Coupe tombant au milieu d'un caractère multi-octets : jamais de panique.
        assert_eq!(tail_lines("ééé\nok\n", 5), "ok\n");
        assert_eq!(tail_lines("sans saut de ligne", 3), "");
    }
}
