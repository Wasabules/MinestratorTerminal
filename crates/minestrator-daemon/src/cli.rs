//! Ligne de commande du daemon (analyse maison : quelques sous-commandes, pas de dépendance).

use minestrator_core::service::is_valid_owner_id;

pub const USAGE: &str = "\
minestrator-daemon — service d'arrière-plan de Minestrator Terminal

Usage :
  minestrator-daemon [run] [--owner ID]...    au premier plan (VPS, conteneur, dev)
  minestrator-daemon service [--owner ID]...  lancé par le gestionnaire de services
  minestrator-daemon install [--owner ID]... [--no-start]
                                              installe/met à jour le service (admin)
  minestrator-daemon enable --owner ID        installe si besoin, autorise ID, démarre (admin)
  minestrator-daemon uninstall [--purge]      arrête et supprime le service + la clé machine
                                              (--purge : aussi l'historique et les journaux)
  minestrator-daemon start | stop | restart   pilote le service installé (admin)
  minestrator-daemon status [--json]          état du service (sans privilège)

ID : uid numérique (Linux/macOS) ou SID (Windows) d'un compte autorisé sur le canal IPC.
Documentation : docs/SERVICE.md et crates/minestrator-daemon/README.md.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Premier plan : variables d'environnement, IPC facultatif.
    Run { owners: Vec<String> },
    /// Lancé par systemd / launchd / le SCM.
    Service { owners: Vec<String> },
    Install {
        owners: Vec<String>,
        start: bool,
        /// Windows : enregistre le binaire courant tel quel, sans le recopier sous `Program Files`
        /// (dev uniquement : un binaire modifiable par l'utilisateur ne doit pas servir de service).
        allow_unsafe_path: bool,
    },
    Uninstall { purge: bool },
    Start,
    Stop,
    Restart,
    Status { json: bool },
    Help,
    Version,
}

/// Analyse les arguments (sans le nom du programme).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter().map(String::as_str).peekable();
    let sub = match it.peek() {
        Some(a) if !a.starts_with('-') => it.next().unwrap_or("run"),
        Some(&"-h") | Some(&"--help") => return Ok(Command::Help),
        Some(&"-V") | Some(&"--version") => return Ok(Command::Version),
        _ => "run",
    };

    let mut owners: Vec<String> = Vec::new();
    let mut no_start = false;
    let mut allow_unsafe_path = false;
    let mut purge = false;
    let mut json = false;
    while let Some(arg) = it.next() {
        match arg {
            "--owner" => {
                let id = it.next().ok_or("--owner attend un identifiant")?;
                if !is_valid_owner_id(id) {
                    return Err(format!("identifiant de propriétaire invalide : {id}"));
                }
                if !owners.iter().any(|o| o == id) {
                    owners.push(id.to_string());
                }
            }
            "--no-start" => no_start = true,
            "--allow-unsafe-path" => allow_unsafe_path = true,
            "--purge" => purge = true,
            "--json" => json = true,
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(format!("argument inconnu : {other}")),
        }
    }

    let only = |allowed: &[&str]| -> Result<(), String> {
        let used = [
            ("--owner", !owners.is_empty()),
            ("--no-start", no_start),
            ("--allow-unsafe-path", allow_unsafe_path),
            ("--purge", purge),
            ("--json", json),
        ];
        match used.iter().find(|(name, on)| *on && !allowed.contains(name)) {
            Some((name, _)) => Err(format!("{name} n'a pas de sens pour « {sub} »")),
            None => Ok(()),
        }
    };

    match sub {
        "run" => only(&["--owner"]).map(|_| Command::Run { owners }),
        "service" => only(&["--owner"]).map(|_| Command::Service { owners }),
        "install" => only(&["--owner", "--no-start", "--allow-unsafe-path"]).map(|_| Command::Install {
            owners,
            start: !no_start,
            allow_unsafe_path,
        }),
        "enable" => {
            only(&["--owner", "--allow-unsafe-path"])?;
            if owners.is_empty() {
                return Err("enable attend --owner ID".into());
            }
            Ok(Command::Install { owners, start: true, allow_unsafe_path })
        }
        "uninstall" => only(&["--purge"]).map(|_| Command::Uninstall { purge }),
        "start" => only(&[]).map(|_| Command::Start),
        "stop" => only(&[]).map(|_| Command::Stop),
        "restart" => only(&[]).map(|_| Command::Restart),
        "status" => only(&["--json"]).map(|_| Command::Status { json }),
        "help" => Ok(Command::Help),
        "version" => Ok(Command::Version),
        other => Err(format!("sous-commande inconnue : {other}")),
    }
}

/// Fusionne des listes de propriétaires (ordre conservé, sans doublon, identifiants valides).
pub fn merge_owners(existing: &[String], added: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in existing.iter().chain(added) {
        if is_valid_owner_id(id) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Propriétaires lus dans une liste d'arguments (`… --owner A --owner B`), filtrés par `valid`
/// (format uid pour systemd/launchd, SID pour Windows).
pub fn owners_from_args<'a>(
    args: impl IntoIterator<Item = &'a str>,
    valid: fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--owner" {
            if let Some(id) = it.next() {
                let id = id.trim_matches('"');
                if valid(id) && !out.iter().any(|o| o == id) {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// Un identifiant valide sur la plateforme qui exécute les tests.
    fn owner() -> &'static str {
        if cfg!(windows) {
            "S-1-5-21-1-2-3-1001"
        } else {
            "1000"
        }
    }

    #[test]
    fn sans_argument_premier_plan() {
        assert_eq!(p(&[]).unwrap(), Command::Run { owners: vec![] });
        assert_eq!(
            p(&["--owner", owner()]).unwrap(),
            Command::Run { owners: vec![owner().into()] }
        );
    }

    #[test]
    fn sous_commandes() {
        assert_eq!(p(&["service"]).unwrap(), Command::Service { owners: vec![] });
        assert_eq!(
            p(&["install", "--no-start"]).unwrap(),
            Command::Install { owners: vec![], start: false, allow_unsafe_path: false }
        );
        assert_eq!(
            p(&["enable", "--owner", owner()]).unwrap(),
            Command::Install { owners: vec![owner().into()], start: true, allow_unsafe_path: false }
        );
        assert_eq!(p(&["uninstall", "--purge"]).unwrap(), Command::Uninstall { purge: true });
        assert_eq!(p(&["status", "--json"]).unwrap(), Command::Status { json: true });
        assert_eq!(p(&["stop"]).unwrap(), Command::Stop);
        assert_eq!(p(&["--help"]).unwrap(), Command::Help);
        assert_eq!(p(&["-V"]).unwrap(), Command::Version);
    }

    #[test]
    fn erreurs() {
        assert!(p(&["enable"]).is_err());
        assert!(p(&["install", "--owner"]).is_err());
        assert!(p(&["install", "--owner", "pas un id"]).is_err());
        assert!(p(&["uninstall", "--json"]).is_err());
        assert!(p(&["status", "--purge"]).is_err());
        assert!(p(&["frobnicate"]).is_err());
        assert!(p(&["run", "--wat"]).is_err());
    }

    #[test]
    fn proprietaires_fusionnes_et_relus() {
        let a = vec![owner().to_string()];
        let merged = merge_owners(&a, &[owner().to_string(), "invalide".into()]);
        assert_eq!(merged, a);
        let args = ["service", "--owner", owner(), "--owner", owner(), "--owner", "x y"];
        assert_eq!(owners_from_args(args, is_valid_owner_id), a);
    }
}
