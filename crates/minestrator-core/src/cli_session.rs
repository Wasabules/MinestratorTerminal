//! Process agent **persistant** (Claude Code) : un seul `claude` reste vivant pour une session de
//! chat et consomme des messages successifs via `--input-format stream-json`, au lieu d'être
//! relancé à chaque tour. Élimine, par message, le bootstrap Node + la relecture `--resume` + le
//! respawn du serveur MCP. La voie one-shot de `copilot::chat_cli` reste le **repli** en cas
//! d'échec (spawn impossible, protocole inattendu, crash) → D est une accélération, jamais un
//! point de fragilité.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Un process `claude` maintenu vivant pour une conversation. `kill_on_drop` garantit qu'il est
/// tué quand la session est réinitialisée / l'onglet fermé (le `ChatSession` est alors droppé).
pub(crate) struct PersistentCli {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    /// Le system prompt a-t-il déjà été injecté ? Faux au démarrage à froid, vrai si lancé avec
    /// `--resume` (la session restaurée le contient déjà).
    pub(crate) primed: bool,
}

impl PersistentCli {
    /// Démarre le process. `resumed` = lancé avec `--resume` (session déjà amorcée).
    pub(crate) fn spawn(
        command: &str,
        args: &[String],
        cwd: Option<&std::path::Path>,
        env: &[(String, String)],
        resumed: bool,
    ) -> Result<Self, String> {
        let mut cmd = crate::cli::build_command(command, args, cwd, env);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("démarrage de l'agent persistant : {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "stdin de l'agent indisponible".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "stdout de l'agent indisponible".to_string())?;
        // On draine stderr en tâche de fond : sinon le pipe se remplit et fige le process. Drain en
        // OCTETS bruts jusqu'à EOF → robuste au non-UTF8 (une lecture ligne s'arrêterait sur un Err
        // et laisserait le tube se remplir).
        if let Some(mut se) = child.stderr.take() {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = se.read(&mut buf).await {
                    if n == 0 {
                        break; // EOF
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            primed: resumed,
        })
    }

    /// Le process est-il encore vivant (pas encore sorti) ?
    pub(crate) fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Envoie un message utilisateur et lit le flux jusqu'au `result` de CE tour. `on_line` reçoit
    /// chaque ligne brute (l'appelant y streame les deltas/phases et capture résultat + session).
    pub(crate) async fn run_turn<F: FnMut(&str)>(
        &mut self,
        user_text: &str,
        turn_timeout_s: u64,
        mut on_line: F,
    ) -> Result<(), String> {
        // Enveloppe stream-json d'un tour utilisateur (le JSON échappe le texte proprement).
        let line = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": user_text }] }
        })
        .to_string();
        self.write_line(&line).await?;

        loop {
            match tokio::time::timeout(
                Duration::from_secs(turn_timeout_s),
                self.stdout.next_line(),
            )
            .await
            {
                Ok(Ok(Some(line))) => {
                    let end = is_turn_end(&line);
                    on_line(&line);
                    if end {
                        return Ok(());
                    }
                }
                Ok(Ok(None)) => return Err("l'agent persistant s'est arrêté (fin de flux)".into()),
                Ok(Err(e)) => return Err(format!("lecture de l'agent : {e}")),
                Err(_) => {
                    return Err(format!("agent sans réponse dans le délai ({turn_timeout_s}s)"))
                }
            }
        }
    }

    async fn write_line(&mut self, line: &str) -> Result<(), String> {
        let map = |e: std::io::Error| format!("écriture vers l'agent : {e}");
        self.stdin.write_all(line.as_bytes()).await.map_err(map)?;
        self.stdin.write_all(b"\n").await.map_err(map)?;
        self.stdin.flush().await.map_err(map)
    }
}

/// Fin d'un tour = ligne `type:"result"` du stream-json Claude Code.
fn is_turn_end(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "result")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_end_only_on_result_line() {
        assert!(is_turn_end(r#"{"type":"result","result":"ok"}"#));
        assert!(!is_turn_end(r#"{"type":"assistant","message":{}}"#));
        assert!(!is_turn_end(r#"{"type":"stream_event","event":{}}"#));
        assert!(!is_turn_end("ligne non-json"));
    }
}
