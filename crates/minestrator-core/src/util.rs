//! Petits helpers partagés dans le crate.

/// Dossier de données de l'app : `MINESTRATOR_DATA_DIR` s'il est défini (mobile : dossier privé
/// de l'app ; daemon), sinon le dossier de données standard de l'OS. `None` si l'OS n'en fournit
/// aucun (pas de `HOME`) — à l'appelant de choisir un repli, jamais pour des secrets.
pub fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("MINESTRATOR_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("com", "geoffreylecoq", "MinestratorTerminal")
                .map(|d| d.data_dir().to_path_buf())
        })
}

/// Les `n` dernières lignes, jointes par `\n`.
pub(crate) fn tail(lines: &[String], n: usize) -> String {
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Tronque `s` à `max` octets **sur une frontière de caractère**, en ajoutant `suffix` si une
/// coupe a eu lieu. Renvoie `s` inchangé s'il tient déjà.
pub(crate) fn truncate_on_boundary(s: &str, max: usize, suffix: &str) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &s[..end])
}
