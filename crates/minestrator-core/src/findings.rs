//! Constats triés par **gravité** (critique, important, info), chacun avec sa recommandation :
//! forme commune des rapports de `check_plugin_deps` (`plugin_deps.rs`) et `audit_server`
//! (`server_audit.rs`). Rendu texte compact, borné (un modpack peut produire des centaines de
//! constats), destiné à un agent IA comme à un humain.

use std::fmt::Write as _;

/// Gravité d'un constat, de la plus haute à la plus basse (ordre de tri).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Severity {
    /// Empêche le démarrage, ou expose le serveur (usurpation, prise de contrôle).
    Critical,
    /// Une fonction, un plugin ou un mod ne marchera pas, ou un risque réel à corriger.
    Important,
    /// À savoir ; rien d'urgent.
    Info,
}

impl Severity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Severity::Critical => "CRITIQUE",
            Severity::Important => "IMPORTANT",
            Severity::Info => "INFO",
        }
    }
}

/// Un constat : ce qui ne va pas, et quoi faire.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Finding {
    pub severity: Severity,
    pub message: String,
    /// Recommandation (vide : aucune).
    pub advice: String,
}

impl Finding {
    pub(crate) fn new(severity: Severity, message: impl Into<String>, advice: impl Into<String>) -> Self {
        Finding { severity, message: message.into(), advice: advice.into() }
    }
}

/// Nombre de constats affichés par gravité ; le reste est résumé.
const PER_SEVERITY: usize = 40;

/// Ajoute les constats à `out`, groupés par gravité (ordre d'arrivée conservé dans un groupe).
/// Sans constat, écrit `none`.
pub(crate) fn render(findings: &[Finding], none: &str, out: &mut String) {
    if findings.is_empty() {
        let _ = writeln!(out, "{none}");
        return;
    }
    for sev in [Severity::Critical, Severity::Important, Severity::Info] {
        let group: Vec<&Finding> = findings.iter().filter(|f| f.severity == sev).collect();
        if group.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{} ({})", sev.label(), group.len());
        for f in group.iter().take(PER_SEVERITY) {
            let _ = writeln!(out, "- {}", f.message);
            if !f.advice.is_empty() {
                let _ = writeln!(out, "  → {}", f.advice);
            }
        }
        if group.len() > PER_SEVERITY {
            let _ = writeln!(out, "… (+{} autre(s) constat(s) {})", group.len() - PER_SEVERITY, sev.label().to_lowercase());
        }
    }
}

/// Résumé d'une ligne : « 1 critique, 2 importants, 3 infos ».
pub(crate) fn tally(findings: &[Finding]) -> String {
    let n = |s: Severity| findings.iter().filter(|f| f.severity == s).count();
    format!("{} critique(s), {} important(s), {} info(s)", n(Severity::Critical), n(Severity::Important), n(Severity::Info))
}

/// Liste lisible bornée : `a, b, c (+2)`.
pub(crate) fn short_list<S: AsRef<str>>(items: &[S], max: usize) -> String {
    let mut out = items.iter().take(max).map(|s| s.as_ref()).collect::<Vec<_>>().join(", ");
    if items.len() > max {
        let _ = write!(out, " (+{})", items.len() - max);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_groups_by_severity_and_caps_them() {
        let mut findings = vec![
            Finding::new(Severity::Info, "info 1", ""),
            Finding::new(Severity::Critical, "grave", "corrige"),
        ];
        for i in 0..45 {
            findings.push(Finding::new(Severity::Important, format!("imp {i}"), ""));
        }
        let mut out = String::new();
        render(&findings, "rien", &mut out);
        let crit = out.find("CRITIQUE (1)").unwrap();
        let imp = out.find("IMPORTANT (45)").unwrap();
        let info = out.find("INFO (1)").unwrap();
        assert!(crit < imp && imp < info, "{out}");
        assert!(out.contains("- grave\n  → corrige"));
        assert!(out.contains("imp 39") && !out.contains("imp 40"));
        assert!(out.contains("(+5 autre(s) constat(s) important)"));
        assert_eq!(tally(&findings), "1 critique(s), 45 important(s), 1 info(s)");

        let mut empty = String::new();
        render(&[], "Aucun problème.", &mut empty);
        assert_eq!(empty, "Aucun problème.\n");
        assert_eq!(short_list(&["a", "b", "c"], 2), "a, b (+1)");
    }
}
