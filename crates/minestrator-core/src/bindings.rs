//! Génération des types TypeScript des frontends depuis les structures Rust (feature `ts`).
//!
//! Tout type qui traverse l'IPC (réponse de commande, payload d'event, argument structuré) dérive
//! `ts_rs::TS` sous `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]`, à côté de ses attributs serde :
//! ts-rs les lit (`rename`, `rename_all`, `tag`, `default`…) pour reproduire la forme JSON exacte.
//! Ce module assemble ces déclarations en **un fichier par cible**, versionné :
//!
//! - `desktop/src/lib/bindings/core.ts` et `mobile/src/lib/bindings/core.ts` (contenu identique) ;
//! - `mobile/src/lib/bindings/mobile.ts`, pour les types propres à l'adaptation mobile, produit par
//!   le crate mobile avec le même [`Bindings`].
//!
//! Commandes (alias de `.cargo/config.toml`) :
//! - `cargo bindings` régénère les fichiers ;
//! - `cargo bindings-check` échoue si un fichier versionné ne correspond plus au Rust (CI).
//!
//! Choix de rendu :
//! - les entiers 64 bits sortent en `number` (ce que `JSON.parse` produit réellement), pas en
//!   `bigint`, via [`Config::with_large_int`] ;
//! - un seul fichier par cible, sans `import` interne : les `types.ts` des fronts n'ont qu'à
//!   ré-exporter (`export type * from './bindings/core'`) ;
//! - un champ par ligne, doc Rust rendue en JSDoc : le fichier se relit et ses diffs pointent le
//!   champ modifié.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ts_rs::{Config, TS};

/// Alias à lancer pour régénérer, rappelé dans l'en-tête et les messages d'échec.
pub const REGENERATE: &str = "cargo bindings";

/// Assembleur d'un fichier de bindings : déclarations dans l'ordre d'ajout, regroupées en sections.
pub struct Bindings {
    cfg: Config,
    source: String,
    imports: Vec<(String, Vec<String>)>,
    blocks: Vec<String>,
    declared: BTreeSet<String>,
    referenced: BTreeSet<String>,
}

impl Bindings {
    /// `source` décrit l'origine des types (rappelée dans l'en-tête du fichier).
    pub fn new(source: &str) -> Self {
        Self {
            cfg: Config::new().with_large_int("number"),
            source: source.to_string(),
            imports: Vec::new(),
            blocks: Vec::new(),
            declared: BTreeSet::new(),
            referenced: BTreeSet::new(),
        }
    }

    /// Ouvre une section (simple commentaire de regroupement dans le fichier généré).
    pub fn section(mut self, title: &str) -> Self {
        self.blocks.push(format!("// --- {title} ---"));
        self
    }

    /// Ajoute la déclaration de `T` (avec sa doc Rust, rendue en JSDoc).
    pub fn add<T: TS + ?Sized + 'static>(mut self) -> Self {
        let name = T::ident(&self.cfg);
        assert!(self.declared.insert(name.clone()), "type déclaré deux fois : {name}");
        for dep in T::dependencies(&self.cfg) {
            self.referenced.insert(dep.ts_name);
        }
        let decl = T::decl(&self.cfg);
        let pretty = pretty(&decl);
        // Garde-fou : la mise en forme ne touche qu'aux blancs.
        assert_eq!(squash(&pretty), squash(&decl), "mise en forme non neutre pour {name}");
        let mut block = T::docs().unwrap_or_default();
        block.push_str("export ");
        block.push_str(&pretty);
        self.blocks.push(block);
        self
    }

    /// Importe des types déclarés dans un autre fichier de bindings (`from` : chemin relatif TS).
    pub fn import(mut self, from: &str, names: &[&str]) -> Self {
        self.declared.extend(names.iter().map(|n| n.to_string()));
        self.imports
            .push((from.to_string(), names.iter().map(|n| n.to_string()).collect()));
        self
    }

    /// Types référencés par une déclaration mais ni déclarés ni importés ici. Doit être vide :
    /// sinon le fichier généré ne compilerait pas côté front.
    pub fn missing(&self) -> Vec<String> {
        self.referenced.difference(&self.declared).cloned().collect()
    }

    /// Contenu du fichier (fins de ligne LF, retour final).
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "// Fichier GÉNÉRÉ depuis les structures Rust ({}) par ts-rs.\n\
             // Ne pas modifier à la main : changer le type Rust, puis lancer `{REGENERATE}`.\n\
             // `cargo bindings-check` (CI) échoue si ce fichier ne correspond plus au Rust.\n",
            self.source
        ));
        for (from, names) in &self.imports {
            out.push_str(&format!("\nimport type {{ {} }} from '{from}';\n", names.join(", ")));
        }
        for block in &self.blocks {
            out.push('\n');
            out.push_str(block);
            out.push('\n');
        }
        out
    }
}

/// Met une déclaration d'objet de ts-rs (`type X = { a: T, /** doc */ b: U, };`, rendue sur une
/// ligne) à raison d'un champ par ligne, JSDoc indentée : le fichier se relit et ses diffs pointent
/// le champ modifié. Les autres déclarations (unions…) restent telles quelles.
fn pretty(decl: &str) -> String {
    let Some(open) = decl.find(" = {") else {
        return decl.to_string();
    };
    let Some(body) = decl[open + 4..].strip_suffix("};") else {
        return decl.to_string();
    };
    let Some(fields) = split_fields(body) else {
        return decl.to_string();
    };
    let mut out = format!("{} = {{\n", &decl[..open]);
    for field in fields {
        for line in format!("{field},").lines() {
            out.push_str("  ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.push_str("};");
    out
}

/// Découpe le corps d'un objet aux virgules de premier niveau. Les commentaires `/** … */` et les
/// littéraux de chaîne sont opaques (ils peuvent contenir virgules et accolades). `None` si le
/// corps n'est pas un objet simple (accolade fermée trop tôt) : la déclaration reste alors brute.
fn split_fields(body: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        match c {
            '/' if chars.peek() == Some(&'*') => {
                // Commentaire : recopié tel quel jusqu'à `*/`.
                current.push(chars.next()?);
                let mut prev = '\0';
                for d in chars.by_ref() {
                    current.push(d);
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
            }
            '"' | '\'' | '`' => {
                for d in chars.by_ref() {
                    current.push(d);
                    if d == c {
                        break;
                    }
                }
            }
            '{' | '[' | '(' | '<' => depth += 1,
            '}' | ']' | ')' | '>' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                current.pop(); // le séparateur est remis par `pretty`
                let field = current.trim().to_string();
                current.clear();
                if !field.is_empty() {
                    fields.push(field);
                }
            }
            _ => {}
        }
    }
    let rest = current.trim();
    if !rest.is_empty() {
        fields.push(rest.to_string());
    }
    (depth == 0).then_some(fields)
}

/// Texte sans aucun blanc, pour vérifier qu'une mise en forme est neutre.
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Bindings des types du core exposés aux frontends (desktop et mobile).
pub fn core() -> Bindings {
    use crate::*;
    Bindings::new("crates/minestrator-core, feature `ts`")
        .section("Erreurs (error.rs)")
        .add::<error::ErrorPayload>()
        .add::<error::ErrorKind>()
        .section("Compte, serveurs, journal (models.rs, games.rs)")
        .add::<UserProfile>()
        .add::<UserNotification>()
        .add::<ServersOverview>()
        .add::<MyBoxSummary>()
        .add::<ServerListItem>()
        .add::<GameCapabilities>()
        .add::<LivePlayers>()
        .add::<Egg>()
        .add::<ActivityEntry>()
        .add::<ActivityPage>()
        .add::<ServerDetails>()
        .add::<LiveLight>()
        .add::<CpuLimits>()
        .add::<LimitMb>()
        .add::<Players>()
        .add::<ServerLive>()
        .add::<LiveStats>()
        .add::<LiveCpu>()
        .add::<LiveUsage>()
        .add::<LiveNetwork>()
        .add::<LiveUptime>()
        .add::<ServerProperties>()
        .add::<MyBoxDetails>()
        .add::<MyBoxInfo>()
        .add::<MyBoxOffer>()
        .add::<MyBoxServer>()
        .add::<MetricSample>()
        .section("Sauvegardes (models.rs)")
        .add::<Backup>()
        .add::<Snapshot>()
        .section("SFTP, archives, NBT, régions (models.rs, archive.rs, nbt.rs, lib.rs)")
        .add::<SftpEntry>()
        .add::<SftpHostKey>()
        .add::<ArchiveEntry>()
        .add::<NbtNode>()
        .add::<RegionChunk>()
        .section("Events temps réel (events.rs)")
        .add::<ConsoleOutput>()
        .add::<ConsoleStatus>()
        .add::<ConsoleConnection>()
        .add::<ConsoleStats>()
        .add::<SftpProgress>()
        .add::<ModInstallProgress>()
        .add::<Alert>()
        .add::<CopilotStarted>()
        .add::<CopilotProgress>()
        .add::<ChatDelta>()
        .add::<Diagnosis>()
        .add::<ProposedAction>()
        .add::<serde_json::Value>()
        .section("Réglages persistés (supervisor.rs, mcp.rs, redact.rs, mods.rs)")
        .add::<SupervisorConfig>()
        .add::<McpConfig>()
        .add::<PrivacyConfig>()
        .add::<GameSettings>()
        .add::<FactorioSettings>()
        .section("Copilote (copilot.rs, llm.rs, cli_agent.rs)")
        .add::<CopilotConfig>()
        .add::<Provider>()
        .add::<Autonomy>()
        .add::<Effort>()
        .add::<CliAgent>()
        .add::<CliStatus>()
        .add::<ChatReply>()
        .section("Marketplace Minecraft (models.rs)")
        .add::<MarketItem>()
        .add::<MarketPage>()
        .add::<MarketVersion>()
        .add::<InstalledItem>()
        .section("Mods Satisfactory (ficsit.rs)")
        .add::<FicsitMod>()
        .add::<FicsitModPage>()
        .add::<FicsitVersion>()
        .add::<FicsitTarget>()
        .add::<FicsitDep>()
        .add::<SmlVersion>()
        .add::<SmlTarget>()
        .add::<FicsitInstalledMod>()
        .add::<FicsitInstallItem>()
        .section("Mods multi-sources : Thunderstore, Factorio, uMod (mods.rs)")
        .add::<MarketMod>()
        .add::<MarketModPage>()
        .add::<MarketModVersion>()
        .add::<MarketInstalledMod>()
        .add::<ModInstallItem>()
        .section("Service d'arrière-plan (service.rs, supervisor.rs)")
        .add::<service::ServiceOverview>()
        .add::<service::ServiceStatus>()
        .add::<supervisor::SupervisorHeartbeat>()
        .section("Comptes MineStrator (accounts.rs)")
        .add::<Account>()
        .add::<AccountSwitch>()
}

/// Racine du dépôt, d'après le dossier d'un crate du workspace (`env!("CARGO_MANIFEST_DIR")`),
/// situé deux niveaux plus bas (`crates/<nom>`, `desktop/src-tauri`, `mobile/src-tauri`).
pub fn repo_root(manifest_dir: &str) -> PathBuf {
    let dir = Path::new(manifest_dir);
    dir.parent().and_then(Path::parent).unwrap_or(dir).to_path_buf()
}

/// Vérifie qu'un fichier versionné (`rel`, relatif à `root`) a exactement le contenu attendu, aux
/// fins de ligne près : une copie de travail en CRLF ne compte pas comme une dérive.
pub fn check(root: &Path, rel: &str, expected: &str) -> Result<(), String> {
    let actual = std::fs::read_to_string(root.join(rel))
        .map_err(|e| format!("{rel} illisible ({e}) : lancer `{REGENERATE}`."))?
        .replace("\r\n", "\n");
    if actual == expected {
        return Ok(());
    }
    let first_diff = actual
        .lines()
        .zip(expected.lines())
        .position(|(a, b)| a != b)
        .map(|i| format!("première différence ligne {}", i + 1))
        .unwrap_or_else(|| "longueurs différentes".to_string());
    Err(format!(
        "{rel} ne correspond plus aux types Rust ({first_diff}) : lancer `{REGENERATE}` et \
         commiter le résultat."
    ))
}

/// Écrit un fichier de bindings (`rel`, relatif à `root`) ; crée le dossier au besoin.
pub fn write(root: &Path, rel: &str, content: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fichiers alimentés par [`core`], relatifs à la racine du dépôt.
    const CORE_TARGETS: [&str; 2] = [
        "desktop/src/lib/bindings/core.ts",
        "mobile/src/lib/bindings/core.ts",
    ];

    #[test]
    fn bindings_are_self_contained() {
        assert_eq!(core().missing(), Vec::<String>::new(), "types référencés mais non déclarés");
    }

    #[test]
    fn bindings_are_up_to_date() {
        let root = repo_root(env!("CARGO_MANIFEST_DIR"));
        let content = core().render();
        let errors: Vec<String> = CORE_TARGETS
            .iter()
            .filter_map(|rel| check(&root, rel, &content).err())
            .collect();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    #[ignore = "réécrit les fichiers versionnés : lancé par `cargo bindings`"]
    fn regenerate_bindings() {
        let root = repo_root(env!("CARGO_MANIFEST_DIR"));
        let content = core().render();
        for rel in CORE_TARGETS {
            write(&root, rel, &content).expect(rel);
        }
    }

    #[test]
    fn pretty_met_un_champ_par_ligne() {
        let decl = "type X = { a: number, \n/**\n * Doc, avec { et \"guillemets\".\n */\nb: Array<string> | null, };";
        assert_eq!(
            pretty(decl),
            "type X = {\n  a: number,\n  /**\n   * Doc, avec { et \"guillemets\".\n   */\n  b: Array<string> | null,\n};"
        );
        // Une union n'est pas touchée.
        assert_eq!(pretty("type U = \"a\" | \"b\";"), "type U = \"a\" | \"b\";");
    }
}
