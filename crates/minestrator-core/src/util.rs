//! Petits helpers texte partagés dans le crate.

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
