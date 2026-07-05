//! Docteur démarrage : pour un serveur qui ne démarre pas / crash-loop, rassemble en UN appel le
//! contexte utile (commande de démarrage + fin de `logs/latest.log` + dernier crash-report) et
//! **pré-scanne** les pannes connues (EULA, port, OOM, version Java, dépendance de mod, mixin,
//! monde corrompu, échec de chargement de mod). L'agent s'appuie sur ce rapport pour proposer un
//! correctif ciblé (avec un snapshot au préalable). Déterministe pour la collecte + les signatures ;
//! l'agent traite les cas hors-signature.

use crate::Core;
use std::fmt::Write as _;

/// Fin de log conservée (octets) — un `latest.log` d'échec de démarrage est court, un crash-report
/// tient largement dans cette fenêtre.
const TAIL_BYTES: usize = 12_000;

struct Finding {
    title: &'static str,
    hint: &'static str,
}

/// Rassemble + pré-scanne. Renvoie un rapport texte (anonymisé si activé), lisible tel quel.
pub(crate) async fn diagnose_startup(core: &Core, server_id: i64) -> String {
    let mut out = String::new();

    // 1) Commande de démarrage (Xmx, jar, flags).
    match core.get_startup(server_id).await {
        Ok(s) => {
            let _ = write!(out, "=== COMMANDE DE DÉMARRAGE ===\n{}\n\n", s.command.trim());
        }
        Err(e) => {
            let _ = write!(out, "=== COMMANDE DE DÉMARRAGE ===\n(indisponible : {e})\n\n");
        }
    }

    // 2) `logs/latest.log` (fin) = le plus fiable pour un échec de démarrage ; sinon console live.
    let latest = core
        .sftp_read_text(server_id, "/logs/latest.log")
        .await
        .ok()
        .map(|s| tail_bytes(&s, TAIL_BYTES))
        .filter(|s| !s.trim().is_empty());
    let log_blob = match latest {
        Some(l) => l,
        None => core
            .console_logs(server_id)
            .await
            .map(|l| tail_lines(&l.join("\n"), 80))
            .unwrap_or_default(),
    };

    // 3) Dernier crash-report.
    let crash = latest_crash_report(core, server_id).await;

    // 4) Pré-scan déterministe des signatures connues.
    let findings = scan_signatures(&format!("{log_blob}\n{}", crash.as_deref().unwrap_or("")));
    out.push_str("=== PANNES DÉTECTÉES (pré-scan) ===\n");
    if findings.is_empty() {
        out.push_str("Aucune signature connue — analyse les logs ci-dessous manuellement.\n\n");
    } else {
        for f in &findings {
            let _ = writeln!(out, "• {} → {}", f.title, f.hint);
        }
        out.push('\n');
    }

    // 5) Extraits de logs bruts.
    if !log_blob.trim().is_empty() {
        let _ = write!(out, "=== LOG (fin) ===\n{log_blob}\n\n");
    }
    if let Some(c) = crash {
        let _ = writeln!(out, "=== DERNIER CRASH-REPORT ===\n{c}");
    }

    core.redact_ai(&out)
}

/// Lit le crash-report le plus récent de `/crash-reports` (par date de modif), tronqué.
async fn latest_crash_report(core: &Core, id: i64) -> Option<String> {
    let mut files: Vec<crate::SftpEntry> = core
        .sftp_list(id, "/crash-reports")
        .await
        .ok()?
        .into_iter()
        .filter(|e| !e.is_dir && e.name.ends_with(".txt"))
        .collect();
    if files.is_empty() {
        return None;
    }
    // Plus récent d'abord : par mtime si dispo, sinon par nom (crash-AAAA-… = ordre chronologique).
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name)));
    let newest = &files[0];
    let content = core
        .sftp_read_text(id, &format!("/crash-reports/{}", newest.name))
        .await
        .ok()?;
    Some(format!("[{}]\n{}", newest.name, tail_bytes(&content, TAIL_BYTES)))
}

/// Reconnaît les pannes de démarrage classiques. Ordre : du plus spécifique au plus générique.
fn scan_signatures(text: &str) -> Vec<Finding> {
    let lo = text.to_lowercase();
    let has = |n: &str| lo.contains(n);
    let mut f: Vec<Finding> = Vec::new();

    if has("you need to agree to the eula") || has("eula=false") {
        f.push(Finding { title: "EULA non acceptée", hint: "écris `/eula.txt` avec `eula=true` (accepte le CLUF Mojang) puis redémarre." });
    }
    if has("failed to bind to port")
        || has("address already in use")
        || has("perhaps a server is already running")
    {
        f.push(Finding { title: "Port déjà utilisé", hint: "un process fantôme occupe le port ; `power_action kill` puis redémarre." });
    }
    if has("outofmemoryerror") || has("gc overhead limit") || has("java heap space") {
        f.push(Finding { title: "Mémoire insuffisante (OOM)", hint: "Xmx trop bas pour la charge : réduis les mods/la view-distance, ou augmente le plan RAM (ne dépasse pas la RAM allouée)." });
    }
    if has("unsupportedclassversionerror") || has("compiled by a more recent version of the java") {
        f.push(Finding { title: "Version de Java incompatible", hint: "le jar exige une autre version de Java ; ajuste la version Java du serveur (image/paramètres)." });
    }
    if has("missing or unsupported mandatory dependencies")
        || (has("requires") && has("missing"))
        || has("incompatible mods")
    {
        f.push(Finding { title: "Dépendance de mod manquante / incompatible", hint: "un mod exige une dépendance absente ou incompatible ; installe-la ou retire le mod fautif (croise avec list_installed_mods)." });
    }
    if has("mixin apply failed") || has("mixinapplyerror") || (has("mixin") && has("failed to")) {
        f.push(Finding { title: "Conflit de mixin (mod)", hint: "un mod échoue à s'injecter ; repère-le dans la stacktrace et désactive-le (renomme le `.jar` en `.jar.disabled`)." });
    }
    if has("exception ticking world")
        || has("level.dat")
        || (has("chunk") && (has("corrupt") || has("failed to save")))
    {
        f.push(Finding { title: "Monde / chunk potentiellement corrompu", hint: "utilise inspect_region sur la région concernée (`/world/region/r.X.Z.mca` ; croise avec les coordonnées du crash), puis — snapshot d'abord — propose repair_region (clear_corrupt / delete)." });
    }
    // Générique en dernier (large) : échec de chargement d'un mod/plugin.
    if f.is_empty()
        && (has("failed to load")
            || has("caused by:")
            || has("could not execute entrypoint")
            || has("cpw.mods")
            || has("net.neoforged"))
    {
        f.push(Finding { title: "Échec de chargement d'un mod/plugin", hint: "cherche « Failed to load » / « Caused by: » pour le mod fautif, puis désactive-le (renomme le `.jar` en `.jar.disabled`)." });
    }
    f
}

fn tail_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(tronqué)\n{}", &s[start..])
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_signatures() {
        let eula = scan_signatures("Go to eula.txt for more info. eula=false");
        assert_eq!(eula.first().map(|f| f.title), Some("EULA non acceptée"));

        let oom = scan_signatures("java.lang.OutOfMemoryError: Java heap space");
        assert_eq!(oom.first().map(|f| f.title), Some("Mémoire insuffisante (OOM)"));

        // Cas générique (aucune signature spécifique) → échec de chargement.
        let generic = scan_signatures("Failed to load mod XYZ\nCaused by: java.lang.NoSuchMethodError");
        assert_eq!(generic.first().map(|f| f.title), Some("Échec de chargement d'un mod/plugin"));

        assert!(scan_signatures("démarrage nominal, tout va bien").is_empty());
    }

    #[test]
    fn tail_bytes_keeps_end_on_char_boundary() {
        let s = "héllo-world"; // « é » = 2 octets
        let t = tail_bytes(s, 5);
        assert!(t.ends_with("world"));
        assert!(t.starts_with("…"));
    }
}
