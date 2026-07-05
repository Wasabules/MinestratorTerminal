//! Exécution d'un **agent CLI local** (Claude Code, Gemini CLI…) pour le Copilote, **sans
//! clé API** : le CLI utilise l'authentification/abonnement déjà configuré sur la machine.
//!
//! Contrat minimal et robuste : prompt sur **stdin**, résultat sur **stdout**. Toutes les
//! issues sont couvertes et remontées en `Err(String)` lisible (affichée dans le Copilote) :
//! binaire introuvable, échec d'écriture, code de sortie non nul, **timeout** (le process est
//! alors tué et récupéré), ou **sortie vide**. stdout/stderr sont lus **en parallèle** (pas de
//! blocage si l'un des tubes se remplit).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Plafond de la sortie conservée (garde-fou contre une sortie pathologiquement longue). Large :
/// une session agentique en `stream-json` (signatures de *thinking* en base64, résultats d'outils)
/// dépasse vite quelques centaines de Ko ; trop bas, la troncature emporte la ligne `result` finale.
const MAX_OUTPUT: usize = 1024 * 1024;

/// Lance `command args…`, écrit `prompt` sur stdin, renvoie stdout (texte non vide).
pub async fn run(
    command: &str,
    args: &[String],
    prompt: &str,
    timeout_s: u64,
) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Aucune commande CLI configurée (Réglages → Copilote).".into());
    }

    let mut cmd = build_command(command, args, None, &[]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| launch_error(command, &e))?;

    // Écrit le prompt puis ferme stdin (signale la fin d'entrée à l'agent).
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(format!("écriture vers « {command} » : {e}"));
        }
        let _ = stdin.shutdown().await;
    }

    // Lit stdout/stderr en parallèle dans des tâches dédiées (évite tout interblocage de tube).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_task = tokio::spawn(read_all(stdout));
    let err_task = tokio::spawn(read_all(stderr));

    let status = match tokio::time::timeout(Duration::from_secs(timeout_s), child.wait()).await {
        Ok(res) => res.map_err(|e| format!("exécution de « {command} » : {e}"))?,
        Err(_) => {
            // Timeout : on interrompt, on récupère le process, et on remonte sa DERNIÈRE sortie
            // (stderr surtout) pour diagnostiquer (démarrage MCP, login manquant, flag inconnu…).
            let _ = child.start_kill();
            let _ = child.wait().await;
            let err = err_task.await.unwrap_or_default();
            let out = out_task.await.unwrap_or_default();
            return Err(format!(
                "« {command} » n'a pas répondu dans le délai ({timeout_s}s) — interrompu. Dernière sortie : {}",
                last_detail(&err, &out)
            ));
        }
    };

    let stdout_s = out_task.await.unwrap_or_default();
    let stderr_s = err_task.await.unwrap_or_default();

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        return Err(format!(
            "« {command} » a échoué (code {code}) : {}",
            detail(&stderr_s, &stdout_s)
        ));
    }

    if stdout_s.trim().is_empty() {
        let hint = first_line(&stderr_s);
        return Err(match hint {
            Some(h) => format!("« {command} » n'a rien renvoyé sur la sortie ({h})."),
            None => format!("« {command} » n'a rien renvoyé sur la sortie."),
        });
    }

    Ok(cap(stdout_s))
}

/// Comme [`run`], mais **streame stdout ligne par ligne** : `on_line` est appelé pour chaque
/// ligne reçue (ex. NDJSON de `--output-format stream-json`) — utile pour suivre en direct la
/// réflexion/les appels d'outils de l'agent. Renvoie la sortie complète accumulée.
#[allow(clippy::too_many_arguments)]
pub async fn run_streaming<F: FnMut(&str)>(
    command: &str,
    args: &[String],
    prompt: &str,
    timeout_s: u64,
    cwd: Option<&Path>,
    env: &[(String, String)],
    mut on_line: F,
) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Aucune commande CLI configurée (Réglages → Copilote).".into());
    }

    let mut cmd = build_command(command, args, cwd, env);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| launch_error(command, &e))?;

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(format!("écriture vers « {command} » : {e}"));
        }
        let _ = stdin.shutdown().await;
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let err_task = tokio::spawn(read_all(stderr));

    let mut acc = String::new();
    let status = match tokio::time::timeout(Duration::from_secs(timeout_s), async {
        if let Some(so) = stdout {
            let mut lines = BufReader::new(so).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&line);
                acc.push_str(&line);
                acc.push('\n');
            }
        }
        child.wait().await
    })
    .await
    {
        Ok(res) => res.map_err(|e| format!("exécution de « {command} » : {e}"))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let err = err_task.await.unwrap_or_default();
            return Err(format!(
                "« {command} » n'a pas répondu dans le délai ({timeout_s}s) — interrompu. Dernière sortie : {}",
                last_detail(&err, &acc)
            ));
        }
    };

    let stderr_s = err_task.await.unwrap_or_default();
    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        return Err(format!(
            "« {command} » a échoué (code {code}) : {}",
            detail(&stderr_s, &acc)
        ));
    }
    if acc.trim().is_empty() {
        return Err(format!("« {command} » n'a rien renvoyé sur la sortie."));
    }
    Ok(cap(acc))
}

/// Sur Windows, une commande « nue » (sans séparateur de chemin, ex. `claude`) est lancée via
/// `cmd /c` pour résoudre les shims `.cmd`/`.bat` (installation npm) et le PATH.
pub(crate) fn build_command(
    command: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Command {
    let is_bare = !command.contains('/') && !command.contains('\\');
    let mut cmd = if cfg!(windows) && is_bare {
        let mut cmd = Command::new("cmd");
        cmd.arg("/c").arg(command).args(args);
        cmd
    } else {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

async fn read_all<R: AsyncRead + Unpin>(reader: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut r) = reader {
        let _ = r.read_to_end(&mut buf).await;
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn launch_error(command: &str, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!(
            "Commande « {command} » introuvable — vérifie qu'elle est installée et dans le PATH \
             (ou indique le chemin complet dans Réglages → Copilote)."
        )
    } else {
        format!("Impossible de lancer « {command} » : {e}")
    }
}

/// Détail d'erreur : privilégie stderr, sinon stdout, sinon message générique (3 premières lignes).
fn detail(stderr: &str, stdout: &str) -> String {
    for src in [stderr, stdout] {
        let lines: Vec<&str> = src.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
        if !lines.is_empty() {
            return lines.join(" / ");
        }
    }
    "(aucun détail sur la sortie d'erreur)".into()
}

/// Comme [`detail`], mais les 3 DERNIÈRES lignes non vides (plus utile au timeout).
fn last_detail(stderr: &str, stdout: &str) -> String {
    for src in [stderr, stdout] {
        let lines: Vec<&str> = src.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if !lines.is_empty() {
            let start = lines.len().saturating_sub(3);
            return lines[start..].join(" / ");
        }
    }
    "(aucune sortie)".into()
}

fn first_line(s: &str) -> Option<String> {
    s.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

fn cap(s: String) -> String {
    crate::util::truncate_on_boundary(&s, MAX_OUTPUT, "\n…[sortie tronquée]")
}
