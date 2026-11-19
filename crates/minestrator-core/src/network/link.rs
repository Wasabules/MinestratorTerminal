//! Liaison d'un proxy à des serveurs de jeu (outil MCP `link_proxy`).
//!
//! Le plan est **pur** ([`build_plan`]) : à partir des fichiers lus, il calcule pour chaque
//! fichier les clés à écrire (comme `set_config`), avec un diff où les secrets sont masqués, et
//! les étapes à faire à la main (`manual`). L'application exécute ensuite chaque étape par
//! [`crate::agent_guard::guarded_write`] (`set_config`, `set_property`, `write_file`) : la garde
//! des agents capture chacune comme une étape annulable (`rollback_changeset`), l'outil composite
//! lui-même n'en crée pas (`Plan::Skip`).
//!
//! **Secret** : celui du proxy est réutilisé s'il existe, sinon un secret de 32 caractères est
//! tiré du générateur aléatoire du système (CSPRNG de `ring`, via rustls). Il n'apparaît que
//! dans les arguments internes des écritures : jamais dans la sortie, les diffs (masqués par
//! `config_edit`) ni le journal des agents (qui ne consigne que l'appel `link_proxy`).

use super::*;
use crate::config_edit::{edit_text, EditOptions, EditRequest};

/// Longueur d'un secret généré (Velocity en génère de 12 caractères).
const SECRET_LEN: usize = 32;
/// Serveurs de jeu au plus par appel.
const MAX_BACKENDS: usize = 30;

// --- Arguments --------------------------------------------------------------------------------------

/// Mode de transfert demandé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Transfert moderne de Velocity, signé par le secret partagé (Paper 1.13+).
    Modern,
    /// Transfert BungeeCord (`ip_forward`, `settings.bungeecord`) : sans signature.
    Legacy,
}

impl Mode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Modern => "modern",
            Mode::Legacy => "legacy",
        }
    }
}

/// Élément de `try` : nom d'entrée, ou identifiant d'un serveur de jeu de l'appel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TryItem {
    Name(String),
    Server(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinkArgs {
    pub proxy: i64,
    pub backends: Vec<i64>,
    pub names: HashMap<i64, String>,
    pub try_order: Option<Vec<TryItem>>,
    pub mode: Option<Mode>,
    pub dry_run: bool,
    pub snapshot: bool,
}

fn id_of(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Nom d'entrée accepté : 1 à 32 caractères parmi lettres, chiffres, `-` et `_` (ni `try`).
pub(crate) fn valid_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty()
        || n.len() > 32
        || !n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "nom « {n} » refusé : 1 à 32 caractères parmi lettres, chiffres, - et _"
        ));
    }
    if n.eq_ignore_ascii_case("try") {
        return Err("nom « try » refusé : réservé par Velocity (liste de repli)".into());
    }
    Ok(n.to_string())
}

pub(crate) fn parse_args(args: &Value) -> Result<LinkArgs, String> {
    let proxy = args
        .get("proxy_server_id")
        .and_then(id_of)
        .ok_or("paramètre `proxy_server_id` (entier) requis")?;
    let backends: Vec<i64> = match args.get("backend_server_ids") {
        Some(Value::Array(a)) if !a.is_empty() => a
            .iter()
            .map(|v| {
                id_of(v)
                    .ok_or_else(|| format!("`backend_server_ids` : identifiant illisible ({v})"))
            })
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(
                "paramètre `backend_server_ids` (liste d'entiers, au moins un) requis".into(),
            );
        }
    };
    if backends.len() > MAX_BACKENDS {
        return Err(format!(
            "{} serveurs de jeu : {MAX_BACKENDS} au plus par appel",
            backends.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for id in &backends {
        if *id == proxy {
            return Err(format!(
                "le proxy {proxy} ne peut pas être son propre serveur de jeu"
            ));
        }
        if !seen.insert(*id) {
            return Err(format!(
                "serveur {id} cité deux fois dans `backend_server_ids`"
            ));
        }
    }
    let mut names = HashMap::new();
    match args.get("names") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            if a.len() > backends.len() {
                return Err("`names` : plus de noms que de serveurs de jeu".into());
            }
            for (id, v) in backends.iter().zip(a) {
                match v {
                    Value::Null => {}
                    Value::String(s) if s.trim().is_empty() => {}
                    Value::String(s) => {
                        names.insert(*id, valid_name(s)?);
                    }
                    other => return Err(format!("`names` : texte attendu ({other})")),
                }
            }
        }
        Some(Value::Object(m)) => {
            for (k, v) in m {
                let id: i64 = k.trim().parse().map_err(|_| {
                    format!("`names` : clé « {k} » (identifiant de serveur) illisible")
                })?;
                if !backends.contains(&id) {
                    return Err(format!(
                        "`names` : le serveur {id} n'est pas dans `backend_server_ids`"
                    ));
                }
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("`names.{k}` : texte attendu"))?;
                names.insert(id, valid_name(s)?);
            }
        }
        Some(_) => {
            return Err(
                "`names` : liste de noms (dans l'ordre de backend_server_ids) ou objet { id: nom }"
                    .into(),
            );
        }
    }
    let try_order = match args.get("try") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => {
            Some(vec![TryItem::Name(s.trim().to_string())])
        }
        Some(Value::Array(a)) if !a.is_empty() => Some(
            a.iter()
                .map(|v| match v {
                    Value::Number(_) => id_of(v)
                        .map(TryItem::Server)
                        .ok_or_else(|| format!("`try` : identifiant illisible ({v})")),
                    Value::String(s) if !s.trim().is_empty() => {
                        Ok(TryItem::Name(s.trim().to_string()))
                    }
                    other => Err(format!(
                        "`try` : nom ou identifiant de serveur attendu ({other})"
                    )),
                })
                .collect::<Result<_, _>>()?,
        ),
        Some(_) => {
            return Err(
                "`try` : liste non vide de noms d'entrées ou d'identifiants de serveurs de jeu"
                    .into(),
            );
        }
    };
    let mode = match args
        .get("mode")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
    {
        None => None,
        Some(m) if m.is_empty() => None,
        Some(m) if m == "modern" => Some(Mode::Modern),
        Some(m) if m == "legacy" => Some(Mode::Legacy),
        Some(m) => {
            return Err(format!(
                "`mode` « {m} » : modern (Velocity, défaut) ou legacy (BungeeCord)"
            ));
        }
    };
    let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(true);
    Ok(LinkArgs {
        proxy,
        backends,
        names,
        try_order,
        mode,
        dry_run: flag("dry_run"),
        snapshot: flag("snapshot"),
    })
}

/// Nom d'entrée tiré du nom d'un serveur : minuscules, accents retirés, `-` entre les mots.
pub(crate) fn slug(name: &str, id: i64) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut s = out.trim_end_matches('-').to_string();
    s.truncate(32);
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() || s == "try" {
        format!("serveur-{id}")
    } else {
        s
    }
}

/// Secret aléatoire (lettres et chiffres), tiré du générateur du système.
pub(crate) fn generate_secret() -> Result<String, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let rng = rustls::crypto::ring::default_provider().secure_random;
    let mut out = String::with_capacity(SECRET_LEN);
    let mut buf = [0u8; 64];
    while out.len() < SECRET_LEN {
        rng.fill(&mut buf)
            .map_err(|_| "générateur aléatoire du système indisponible".to_string())?;
        // Rejet au-delà du dernier multiple de 62 : tirage uniforme.
        for b in buf.iter().filter(|b| **b < 248) {
            out.push(char::from(ALPHABET[usize::from(*b % 62)]));
            if out.len() == SECRET_LEN {
                break;
            }
        }
    }
    Ok(out)
}

// --- Plan ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// Déjà conforme.
    Ok,
    /// À écrire.
    Change,
    /// À faire à la main (expliqué).
    Manual,
    /// Édition impossible (fichier invalide…) : rien ne sera écrit tant qu'elle reste.
    Refused,
}

impl Status {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Change => "change",
            Status::Manual => "manual",
            Status::Refused => "refused",
        }
    }
}

/// Écriture d'une étape. Contient le secret : **jamais rendue**.
#[derive(Clone, PartialEq)]
pub(crate) enum Action {
    Config {
        path: String,
        changes: Vec<(String, Value)>,
    },
    Property {
        key: String,
        value: String,
    },
    SecretFile {
        path: String,
    },
}

impl std::fmt::Debug for Action {
    // Sans les valeurs : un `{:?}` ne doit jamais montrer le secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::Config { path, changes } => {
                let keys: Vec<&str> = changes.iter().map(|(k, _)| k.as_str()).collect();
                write!(f, "Config({path}, {keys:?})")
            }
            Action::Property { key, .. } => write!(f, "Property({key})"),
            Action::SecretFile { path } => write!(f, "SecretFile({path})"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Step {
    pub id: String,
    pub server_id: i64,
    pub server: String,
    pub status: Status,
    pub summary: String,
    /// Diff unifié, secrets masqués.
    pub diff: Option<String>,
    /// Modifications clé par clé (`config_edit::ChangeReport`, secrets masqués).
    pub changes: Value,
    pub action: Option<Action>,
}

impl Step {
    fn new(
        id: String,
        server_id: i64,
        server: &str,
        status: Status,
        summary: impl Into<String>,
    ) -> Self {
        Step {
            id,
            server_id,
            server: server.to_string(),
            status,
            summary: summary.into(),
            diff: None,
            changes: Value::Null,
            action: None,
        }
    }
}

/// Entrée du proxy après le plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedEntry {
    pub name: String,
    pub address: String,
    pub server_id: i64,
    pub server: String,
    /// `ajoutée`, `mise à jour` ou `inchangée`.
    pub status: &'static str,
}

/// Provenance du secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretPlan {
    /// Transfert legacy : pas de secret.
    NotUsed,
    /// Secret du proxy réutilisé (d'où il vient).
    Reused(String),
    Generated,
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub mode: Mode,
    pub secret: SecretPlan,
    pub entries: Vec<PlannedEntry>,
    pub try_order: Vec<String>,
    pub try_changed: bool,
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
}

/// Proxy tel que lu.
pub(crate) struct ProxyInput<'a> {
    pub state: &'a ProxyState,
    pub config_text: &'a str,
}

/// Serveur de jeu tel que lu.
#[derive(Debug, Clone)]
pub(crate) struct BackendInput {
    pub server_id: i64,
    pub server: String,
    pub endpoint: Endpoint,
    pub state: Result<(BackendState, BackendFiles), String>,
}

/// Étape d'édition d'une config : diff masqué calculé sur le texte lu (comme `set_config`).
fn config_step(
    id: String,
    server_id: i64,
    server: &str,
    path: &str,
    text: &str,
    changes: Vec<(String, Value)>,
    redact: &dyn Fn(&str) -> String,
) -> Step {
    if changes.is_empty() {
        return Step::new(
            id,
            server_id,
            server,
            Status::Ok,
            format!("{path} : déjà conforme"),
        );
    }
    let Some(format) = Format::detect(path) else {
        return Step::new(
            id,
            server_id,
            server,
            Status::Refused,
            format!("{path} : format non pris en charge"),
        );
    };
    let reqs: Vec<EditRequest> = changes
        .iter()
        .map(|(k, v)| EditRequest {
            key: k.clone(),
            value: v.clone(),
        })
        .collect();
    match edit_text(format, path, text, &reqs, EditOptions::default(), redact) {
        Ok(o) if !o.changed => Step::new(
            id,
            server_id,
            server,
            Status::Ok,
            format!("{path} : déjà conforme"),
        ),
        Ok(o) => {
            let keys: Vec<&str> = o
                .changes
                .iter()
                .filter(|c| c.action != "inchangée")
                .map(|c| c.key.as_str())
                .collect();
            let mut s = Step::new(
                id,
                server_id,
                server,
                Status::Change,
                format!("{path} : {}", keys.join(", ")),
            );
            s.diff = Some(o.diff);
            s.changes = json!(o.changes);
            s.action = Some(Action::Config {
                path: path.to_string(),
                changes,
            });
            s
        }
        Err(e) => Step::new(
            id,
            server_id,
            server,
            Status::Refused,
            format!("{path} : {e}"),
        ),
    }
}

fn manual(server_id: i64, server: &str, tag: &str, why: impl Into<String>) -> Step {
    Step::new(
        format!("{server_id}.{tag}"),
        server_id,
        server,
        Status::Manual,
        why,
    )
}

/// Valeur de `player-info-forwarding-mode` dans la casse de l'existante (`NONE` → `MODERN`).
fn forwarding_value(raw: Option<&str>, wanted: &str) -> String {
    match raw {
        Some(r) if !r.is_empty() && r.chars().all(|c| !c.is_ascii_lowercase()) => {
            wanted.to_ascii_uppercase()
        }
        _ => wanted.to_string(),
    }
}

/// Chemin de clé `servers.<nom>` (nom entre guillemets s'il le faut).
fn entry_key(name: &str, tail: &[&str]) -> String {
    use crate::config_edit::Seg;
    let mut segs = vec![Seg::Key("servers".into()), Seg::Key(name.to_string())];
    segs.extend(tail.iter().map(|t| Seg::Key((*t).to_string())));
    crate::config_edit::render_key_path(&segs)
}

/// Entrées à déclarer : nom imposé, sinon celui d'une entrée qui vise déjà le serveur, sinon tiré
/// de son nom (unique).
fn plan_entries(
    conf: &ProxyConf,
    backends: &[BackendInput],
    names: &HashMap<i64, String>,
    warnings: &mut Vec<String>,
) -> Result<Vec<PlannedEntry>, String> {
    let eps: Vec<Endpoint> = backends.iter().map(|b| b.endpoint.clone()).collect();
    // Une entrée vise le serveur (son nom est repris) ; elle le vise **bien** si c'est son port
    // principal, par IP ou par nom (sinon — port secondaire, port seul, port oublié — son adresse
    // est réécrite).
    let points_to = |e: &Entry, id: i64| {
        match_entry(&Addr::parse(&e.address), &[], &eps).is_some_and(|m| m.server_id == id)
    };
    let targets = |e: &Entry, id: i64| {
        match_entry(&Addr::parse(&e.address), &[], &eps).is_some_and(|m| {
            m.server_id == id
                && m.port_ok
                && matches!(m.how, How::Primary | How::Dns | How::Resolved)
        })
    };
    let mut planned: Vec<PlannedEntry> = Vec::new();
    for b in backends {
        let address = b.endpoint.address();
        let name = match names.get(&b.server_id) {
            Some(n) => n.clone(),
            None => match conf
                .entries
                .iter()
                .find(|e| targets(e, b.server_id))
                .or_else(|| conf.entries.iter().find(|e| points_to(e, b.server_id)))
            {
                Some(e) => e.name.clone(),
                None => {
                    // Une entrée locale du même nom (exemple de la configuration par défaut,
                    // `lobby = "127.0.0.1:30066"`) ne mène nulle part : elle est reprise.
                    let base = slug(&b.server, b.server_id);
                    let taken = |n: &str| {
                        conf.entry(n).is_some_and(|e| {
                            classify(&Addr::parse(&e.address).host) != HostClass::Local
                        }) || planned.iter().any(|p| p.name.eq_ignore_ascii_case(n))
                    };
                    let mut n = base.clone();
                    let mut i = 2;
                    while taken(&n) {
                        n = format!("{base}-{i}");
                        i += 1;
                    }
                    n
                }
            },
        };
        if planned.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            return Err(format!("nom « {name} » donné à deux serveurs de jeu"));
        }
        let (status, address) = match conf.entry(&name) {
            Some(e) if e.address == address || targets(e, b.server_id) => {
                ("inchangée", e.address.clone())
            }
            Some(e) => {
                warnings.push(format!(
                    "l'entrée « {} » pointait vers {} : remplacée par « {} » ({address})",
                    e.name, e.address, b.server
                ));
                ("mise à jour", address)
            }
            None => ("ajoutée", address),
        };
        if let Some(other) = conf
            .entries
            .iter()
            .find(|e| !e.name.eq_ignore_ascii_case(&name) && targets(e, b.server_id))
        {
            warnings.push(format!(
                "l'entrée « {} » vise déjà « {} » : conservée en plus de « {name} »",
                other.name, b.server
            ));
        }
        let name = conf.entry(&name).map(|e| e.name.clone()).unwrap_or(name);
        planned.push(PlannedEntry {
            name,
            address,
            server_id: b.server_id,
            server: b.server.clone(),
            status,
        });
    }
    Ok(planned)
}

/// Liste de repli : imposée (`try`), sinon l'actuelle si elle reste valable (noms déclarés, au
/// moins un serveur joignable), sinon le premier serveur de jeu.
fn plan_try(
    conf: &ProxyConf,
    planned: &[PlannedEntry],
    explicit: Option<&[TryItem]>,
) -> Result<Vec<String>, String> {
    let address_of = |n: &str| -> Option<String> {
        planned
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(n))
            .map(|p| p.address.clone())
            .or_else(|| conf.entry(n).map(|e| e.address.clone()))
    };
    let canonical = |n: &str| -> Option<String> {
        planned
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(n))
            .map(|p| p.name.clone())
            .or_else(|| conf.entry(n).map(|e| e.name.clone()))
    };
    if let Some(items) = explicit {
        let mut out: Vec<String> = Vec::new();
        for it in items {
            // Un nom qui n'est pas déclaré mais qui est l'identifiant d'un serveur de l'appel le
            // désigne (le schéma annonce des textes).
            let by_id = |n: &str| {
                n.parse::<i64>()
                    .ok()
                    .and_then(|id| planned.iter().find(|p| p.server_id == id))
                    .map(|p| p.name.clone())
            };
            let n = match it {
                TryItem::Name(n) => canonical(n)
                    .or_else(|| by_id(n))
                    .ok_or_else(|| format!("`try` : « {n} » n'est ni déclaré sur le proxy ni un des serveurs de jeu"))?,
                TryItem::Server(id) => planned
                    .iter()
                    .find(|p| p.server_id == *id)
                    .map(|p| p.name.clone())
                    .ok_or_else(|| format!("`try` : le serveur {id} n'est pas dans `backend_server_ids` (donne plutôt le nom de son entrée)"))?,
            };
            if !out.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                out.push(n);
            }
        }
        return Ok(out);
    }
    let cur = &conf.try_order;
    let keep = !cur.is_empty()
        && cur.iter().all(|t| canonical(t).is_some())
        && cur.iter().any(|t| {
            address_of(t).is_some_and(|a| classify(&Addr::parse(&a).host) != HostClass::Local)
        });
    Ok(if keep {
        cur.clone()
    } else {
        planned
            .first()
            .map(|p| vec![p.name.clone()])
            .unwrap_or_default()
    })
}

/// Étapes d'un serveur de jeu.
fn plan_backend(
    b: &BackendInput,
    mode: Mode,
    proxy_online: bool,
    secret: Option<&str>,
    redact: &dyn Fn(&str) -> String,
) -> (Vec<Step>, Vec<String>) {
    let (id, who) = (b.server_id, b.server.as_str());
    let mut warnings = Vec::new();
    let (state, files) = match &b.state {
        Ok(x) => x,
        Err(e) => {
            return (
                vec![manual(
                    id,
                    who,
                    "unread",
                    format!(
                        "« {who} » illisible ({e}) : rien n'est modifié sur ce serveur ; à régler à la main ou à relancer quand il sera accessible."
                    ),
                )],
                warnings,
            );
        }
    };
    let mut steps = Vec::new();
    let online_step = |steps: &mut Vec<Step>| {
        let id_s = format!("{id}.online-mode");
        if state.online_mode == Some(false) {
            steps.push(Step::new(
                id_s,
                id,
                who,
                Status::Ok,
                "server.properties : online-mode=false (déjà)",
            ));
        } else {
            let cur = state.online_mode.map_or("?".to_string(), |v| v.to_string());
            let mut s = Step::new(
                id_s,
                id,
                who,
                Status::Change,
                format!(
                    "server.properties : online-mode {cur} → false (l'authentification se fait sur le proxy)"
                ),
            );
            s.diff = Some(format!("-online-mode={cur}\n+online-mode=false"));
            s.action = Some(Action::Property {
                key: "online-mode".into(),
                value: "false".into(),
            });
            steps.push(s);
        }
    };
    let ty = state.ty();
    match mode {
        Mode::Modern => match ty {
            ServerType::Paper => {
                if below_1_13(state.version.as_deref()) {
                    steps.push(manual(id, who, "version", format!("« {who} » est en Minecraft {} : le transfert moderne exige 1.13 ou plus ; mets-le à jour ou utilise mode=legacy avec BungeeGuard.", state.version.as_deref().unwrap_or("?"))));
                    return (steps, warnings);
                }
                let secret = secret.unwrap_or_default();
                let (path, text, prefix) = match state.paper {
                    PaperFile::Global => (PAPER_GLOBAL, files.paper_global.as_deref(), "proxies.velocity"),
                    PaperFile::Legacy => (PAPER_LEGACY, files.paper_legacy.as_deref(), "settings.velocity-support"),
                    PaperFile::Missing => {
                        steps.push(manual(id, who, "paper", format!("« {who} » : config/paper-global.yml absent (serveur jamais démarré ?). Démarre-le une fois pour que Paper génère sa configuration, arrête-le, puis relance link_proxy (idempotent). Rien n'est modifié sur ce serveur d'ici là.")));
                        return (steps, warnings);
                    }
                };
                steps.push(config_step(
                    format!("{id}.paper"),
                    id,
                    who,
                    path,
                    text.unwrap_or_default(),
                    vec![
                        (format!("{prefix}.enabled"), json!(true)),
                        (format!("{prefix}.online-mode"), json!(proxy_online)),
                        (format!("{prefix}.secret"), json!(secret)),
                    ],
                    redact,
                ));
                if state.bungeecord == Some(true) {
                    steps.push(config_step(
                        format!("{id}.spigot"),
                        id,
                        who,
                        SPIGOT_YML,
                        files.spigot.as_deref().unwrap_or_default(),
                        vec![("settings.bungeecord".into(), json!(false))],
                        redact,
                    ));
                }
                online_step(&mut steps);
            }
            ServerType::Spigot => steps.push(manual(id, who, "platform", format!("« {who} » est un serveur Spigot : il ne gère pas le transfert moderne de Velocity. Passe-le sur Paper (ou un fork) puis relance link_proxy, ou utilise mode=legacy avec BungeeGuard."))),
            ServerType::Fabric => steps.push(manual(id, who, "platform", format!("« {who} » (Fabric) : installe les mods FabricProxy-Lite et Fabric API, démarre une fois le serveur, puis renseigne `secret` dans config/FabricProxy-Lite.toml avec le contenu de forwarding.secret du proxy — à copier toi-même, l'agent ne voit jamais le secret — et online-mode=false. Non automatisé."))),
            ServerType::Forge => steps.push(manual(id, who, "platform", format!("« {who} » (Forge/NeoForge) : le transfert moderne demande un mod côté serveur (Proxy-Compatible-Forge, ou NeoForwarding) et parfois Ambassador sur Velocity ; à installer et régler à la main avec le secret du proxy. Non automatisé."))),
            ServerType::Vanilla => steps.push(manual(id, who, "platform", format!("« {who} » est un serveur vanilla : aucun transfert de proxy possible ; passe-le sur Paper (ou Fabric avec FabricProxy-Lite)."))),
            _ => steps.push(manual(id, who, "platform", format!("« {who} » : plateforme non reconnue ; règle le transfert à la main."))),
        },
        Mode::Legacy => match ty {
            ServerType::Paper | ServerType::Spigot => {
                if !state.spigot_file {
                    steps.push(manual(id, who, "spigot", format!("« {who} » : spigot.yml absent (serveur jamais démarré ?). Démarre-le une fois, arrête-le, puis relance link_proxy. Rien n'est modifié sur ce serveur d'ici là.")));
                    return (steps, warnings);
                }
                steps.push(config_step(
                    format!("{id}.spigot"),
                    id,
                    who,
                    SPIGOT_YML,
                    files.spigot.as_deref().unwrap_or_default(),
                    vec![("settings.bungeecord".into(), json!(true))],
                    redact,
                ));
                let paper = match state.paper {
                    PaperFile::Global => Some((PAPER_GLOBAL, files.paper_global.as_deref(), "proxies.velocity.enabled", "proxies.bungee-cord.online-mode")),
                    PaperFile::Legacy => Some((PAPER_LEGACY, files.paper_legacy.as_deref(), "settings.velocity-support.enabled", "settings.bungee-online-mode")),
                    PaperFile::Missing => None,
                };
                if let Some((path, text, velocity_key, online_key)) = paper {
                    let mut changes = Vec::new();
                    if state.velocity_enabled == Some(true) {
                        changes.push((velocity_key.to_string(), json!(false)));
                    }
                    if state.bungee_online_mode.is_some() || state.paper == PaperFile::Global {
                        changes.push((online_key.to_string(), json!(proxy_online)));
                    }
                    steps.push(config_step(format!("{id}.paper"), id, who, path, text.unwrap_or_default(), changes, redact));
                }
                online_step(&mut steps);
                if !state.bungeeguard {
                    steps.push(manual(id, who, "bungeeguard", format!("« {who} » : installe BungeeGuard (sur le proxy et sur ce serveur, même jeton). Sans lui, quiconque joint directement le serveur peut prendre n'importe quel pseudo, opérateurs compris.")));
                    warnings.push(format!("« {who} » : transfert legacy sans BungeeGuard — tant qu'il n'est pas installé, le serveur reste usurpable par une connexion directe (online-mode=false)"));
                }
            }
            ServerType::Fabric | ServerType::Forge => steps.push(manual(id, who, "platform", format!("« {who} » ({}) : le transfert legacy demande un mod côté serveur ; à installer et régler à la main.", ty.label()))),
            ServerType::Vanilla => steps.push(manual(id, who, "platform", format!("« {who} » est un serveur vanilla : aucun transfert de proxy possible ; passe-le sur Paper ou Spigot."))),
            _ => steps.push(manual(id, who, "platform", format!("« {who} » : plateforme non reconnue ; règle le transfert à la main."))),
        },
    }
    (steps, warnings)
}

/// Plan complet (pur). `generated` : secret à écrire si le proxy n'en a pas (transfert moderne).
pub(crate) fn build_plan(
    proxy: &ProxyInput,
    backends: &[BackendInput],
    args: &LinkArgs,
    generated: Option<&str>,
    redact: &dyn Fn(&str) -> String,
) -> Result<Plan, String> {
    let p = proxy.state;
    let c = &p.conf;
    let mode = args.mode.unwrap_or(match c.kind {
        ProxyKind::Velocity => Mode::Modern,
        ProxyKind::Bungee => Mode::Legacy,
    });
    if c.kind == ProxyKind::Bungee && mode == Mode::Modern {
        return Err("BungeeCord n'a pas de transfert moderne : mode legacy (avec BungeeGuard), ou un proxy Velocity".into());
    }
    let (secret, secret_plan) = match (mode, &p.secret) {
        (Mode::Legacy, _) => (None, SecretPlan::NotUsed),
        (Mode::Modern, Some(s)) => (Some(s.as_str()), SecretPlan::Reused(p.secret_note.clone())),
        (Mode::Modern, None) => (
            Some(generated.ok_or("aucun secret à écrire (génération impossible)")?),
            SecretPlan::Generated,
        ),
    };
    let mut warnings = Vec::new();
    let entries = plan_entries(c, backends, &args.names, &mut warnings)?;
    let try_order = plan_try(c, &entries, args.try_order.as_deref())?;
    let try_changed = try_order != c.try_order;
    let proxy_online = c.online_mode.unwrap_or(true);
    if c.online_mode == Some(false) {
        warnings.push(format!("le proxy « {} » est en online-mode=false : il n'authentifie pas les comptes (non modifié par link_proxy)", p.name));
    }
    let leftovers: Vec<String> = c
        .entries
        .iter()
        .filter(|e| classify(&Addr::parse(&e.address).host) == HostClass::Local)
        .filter(|e| !entries.iter().any(|x| x.name.eq_ignore_ascii_case(&e.name)))
        .map(|e| e.name.clone())
        .collect();
    if !leftovers.is_empty() {
        warnings.push(format!(
            "entrées locales conservées ({}) : elles ne mènent à aucun serveur (exemples de la configuration par défaut ?) ; sans effet hors de la liste de repli, à retirer à la main si tu veux",
            leftovers.join(", ")
        ));
    }

    let mut steps = Vec::new();
    let mut changes: Vec<(String, Value)> = Vec::new();
    let pid = p.server_id;
    match c.kind {
        ProxyKind::Velocity => {
            for e in entries.iter().filter(|e| e.status != "inchangée") {
                changes.push((entry_key(&e.name, &[]), json!(e.address)));
            }
            if try_changed {
                changes.push(("servers.try".into(), json!(try_order)));
            }
            let wanted = match mode {
                Mode::Modern => Forwarding::Modern,
                Mode::Legacy => Forwarding::Legacy,
            };
            if c.forwarding != wanted {
                changes.push((
                    "player-info-forwarding-mode".into(),
                    json!(forwarding_value(c.forwarding_raw.as_deref(), mode.label())),
                ));
            }
            if mode == Mode::Modern {
                if c.secret_file.is_none() && c.inline_secret.is_some() {
                    if let Some(s) = secret.filter(|s| c.inline_secret.as_deref() != Some(*s)) {
                        changes.push(("forwarding-secret".into(), json!(s)));
                    }
                } else if c.secret_file.is_none() {
                    changes.push(("forwarding-secret-file".into(), json!(DEFAULT_SECRET_FILE)));
                }
            }
        }
        ProxyKind::Bungee => {
            for e in entries.iter().filter(|e| e.status != "inchangée") {
                changes.push((entry_key(&e.name, &["address"]), json!(e.address)));
                if e.status == "ajoutée" {
                    changes.push((
                        entry_key(&e.name, &["motd"]),
                        json!(format!("&1{}", e.server)),
                    ));
                    changes.push((entry_key(&e.name, &["restricted"]), json!(false)));
                }
            }
            if c.forwarding != Forwarding::Legacy {
                changes.push(("ip_forward".into(), json!(true)));
            }
            if try_changed {
                if c.listeners == 0 {
                    steps.push(Step::new(
                        "proxy.priorities".into(),
                        pid,
                        &p.name,
                        Status::Refused,
                        "config.yml sans listeners : liste de repli impossible à écrire"
                            .to_string(),
                    ));
                } else {
                    changes.push(("listeners[0].priorities".into(), json!(try_order)));
                }
            }
        }
    }
    steps.insert(
        0,
        config_step(
            "proxy.config".into(),
            pid,
            &p.name,
            p.config_path,
            proxy.config_text,
            changes,
            redact,
        ),
    );
    if mode == Mode::Modern {
        if let Some(path) = c.secret_path() {
            if secret_plan == SecretPlan::Generated {
                let mut s = Step::new(
                    "proxy.secret".into(),
                    pid,
                    &p.name,
                    Status::Change,
                    format!(
                        "{path} : nouveau secret de transfert ({SECRET_LEN} caractères aléatoires), jamais affiché"
                    ),
                );
                s.action = Some(Action::SecretFile { path });
                steps.push(s);
            } else {
                steps.push(Step::new(
                    "proxy.secret".into(),
                    pid,
                    &p.name,
                    Status::Ok,
                    format!("{path} : secret existant réutilisé, jamais affiché"),
                ));
            }
        } else if c.secret_file.is_some() {
            steps.push(Step::new(
                "proxy.secret".into(),
                pid,
                &p.name,
                Status::Refused,
                "forwarding-secret-file : chemin refusé (hors du serveur)".to_string(),
            ));
        }
    }
    for b in backends {
        let (s, w) = plan_backend(b, mode, proxy_online, secret, redact);
        steps.extend(s);
        warnings.extend(w);
    }
    Ok(Plan {
        mode,
        secret: secret_plan,
        entries,
        try_order,
        try_changed,
        steps,
        warnings,
    })
}

// --- Rendu ------------------------------------------------------------------------------------------

fn render_step(s: &Step, outcome: Option<&(String, Option<String>)>) -> Value {
    let mut v = json!({
        "id": s.id,
        "server_id": s.server_id,
        "server": s.server,
        "status": s.status.label(),
        "summary": s.summary,
    });
    if !s.changes.is_null() {
        v["changes"] = s.changes.clone();
    }
    if let Some(d) = &s.diff {
        v["diff"] = json!(d);
    }
    if let Some((o, detail)) = outcome {
        v["outcome"] = json!(o);
        if let Some(d) = detail {
            v["detail"] = json!(d);
        }
    }
    v
}

pub(crate) fn render_plan(
    proxy: &ProxyState,
    plan: &Plan,
    dry_run: bool,
    outcomes: &HashMap<String, (String, Option<String>)>,
) -> Value {
    let count = |st: Status| plan.steps.iter().filter(|s| s.status == st).count();
    let manual: Vec<&str> = plan
        .steps
        .iter()
        .filter(|s| s.status == Status::Manual)
        .map(|s| s.summary.as_str())
        .collect();
    let mut v = json!({
        "proxy": { "server_id": proxy.server_id, "name": proxy.name, "type": proxy.conf.kind.label(), "config": proxy.config_path },
        "mode": plan.mode.label(),
        "dry_run": dry_run,
        "secret": match &plan.secret {
            SecretPlan::NotUsed => Value::Null,
            SecretPlan::Reused(from) => json!(format!("secret du proxy réutilisé ({from}) ; jamais affiché")),
            SecretPlan::Generated => json!(format!("secret généré ({SECRET_LEN} caractères, générateur aléatoire du système) ; {} ; jamais affiché", if dry_run { "écrit seulement à l'application" } else { "écrit sur le proxy et les serveurs de jeu" })),
        },
        "servers": plan.entries.iter().map(|e| json!({
            "name": e.name, "address": e.address, "server_id": e.server_id, "server": e.server, "entry": e.status,
        })).collect::<Vec<_>>(),
        "try": plan.try_order,
        "try_changed": plan.try_changed,
        "counts": { "change": count(Status::Change), "ok": count(Status::Ok), "manual": count(Status::Manual), "refused": count(Status::Refused) },
        "steps": plan.steps.iter().map(|s| render_step(s, outcomes.get(&s.id))).collect::<Vec<_>>(),
    });
    if !manual.is_empty() {
        v["manual"] = json!(manual);
    }
    if !plan.warnings.is_empty() {
        v["warnings"] = json!(plan.warnings);
    }
    v
}

// --- Lecture et application -------------------------------------------------------------------------

/// Proxy et serveurs de jeu lus pour l'appel.
struct Inputs {
    proxy: ProxyState,
    config_text: String,
    backends: Vec<BackendInput>,
}

async fn read_inputs(core: &Core, a: &LinkArgs) -> Result<Inputs, String> {
    let servers = mc_servers(core).await?;
    let find = |id: i64| -> Result<&McServer, String> {
        servers.iter().find(|s| s.id == id).ok_or_else(|| {
            format!("serveur {id} introuvable parmi les serveurs Minecraft Java du compte (list_servers ; Bedrock et autres jeux exclus)")
        })
    };
    let p = find(a.proxy)?;
    if !p.readable() {
        return Err(format!(
            "le proxy {} est {} : fichiers inaccessibles",
            p.label(),
            p.status_label()
        ));
    }
    if let Some(t) = p.ty.filter(|t| !t.is_proxy()) {
        return Err(format!(
            "{} n'est pas un proxy : plateforme {} (Velocity ou BungeeCord attendu)",
            p.label(),
            t.label()
        ));
    }
    let mut chosen = Vec::new();
    for id in &a.backends {
        let s = find(*id)?;
        if s.ty.is_some_and(ServerType::is_proxy) {
            return Err(format!(
                "{} est un proxy ({}), pas un serveur de jeu",
                s.label(),
                s.ty.map(ServerType::label).unwrap_or_default()
            ));
        }
        chosen.push(s.clone());
    }
    let (proxy, config_text) = read_proxy(core, p.id, &p.name, p.ty)
        .await?
        .ok_or_else(|| format!("{} n'est pas un proxy Velocity ou BungeeCord : ni velocity.toml ni config.yml de BungeeCord à sa racine", p.label()))?;
    let tasks: Vec<_> = chosen
        .into_iter()
        .map(|s| {
            let ep = s.endpoint.clone();
            async move {
                let ep =
                    ep.ok_or_else(|| format!("{} : adresse ip:port introuvable", s.label()))?;
                let state = if s.readable() {
                    read_backend(core, s.id, s.ty, s.version.clone()).await
                } else {
                    Err(format!("serveur {}", s.status_label()))
                };
                Ok::<BackendInput, String>(BackendInput {
                    server_id: s.id,
                    server: s.name.clone(),
                    endpoint: ep,
                    state,
                })
            }
        })
        .collect();
    // `buffered` garde l'ordre de l'appel (celui de `try` par défaut).
    let backends: Vec<BackendInput> = stream::iter(tasks)
        .buffered(SFTP_CONCURRENCY)
        .collect::<Vec<Result<BackendInput, String>>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
    Ok(Inputs {
        proxy,
        config_text,
        backends,
    })
}

/// Secrets lus ou générés : effacés de toute sortie.
fn secrets_of(inputs: &Inputs, generated: Option<&str>) -> Vec<String> {
    let mut v = inputs.proxy.secrets();
    v.extend(generated.map(str::to_string));
    for b in &inputs.backends {
        if let Ok((s, _)) = &b.state {
            v.extend(s.secrets());
        }
    }
    v
}

/// Exécute une étape par `mcp::dispatch` (garde des agents : une étape annulable par écriture).
async fn execute(core: &Core, s: &Step, secret: Option<&str>) -> Result<Option<String>, String> {
    let Some(action) = &s.action else {
        return Ok(None);
    };
    match action {
        Action::Config { path, changes } => {
            let changes: Vec<Value> = changes
                .iter()
                .map(|(k, v)| json!({ "key": k, "value": v }))
                .collect();
            crate::agent_guard::guarded_write(
                core,
                "set_config",
                json!({ "server_id": s.server_id, "path": path, "changes": changes, "create_missing": true, "keep_type": true }),
            )
            .await
        }
        Action::Property { key, value } => {
            crate::agent_guard::guarded_write(
                core,
                "set_property",
                json!({ "server_id": s.server_id, "property": key, "value": value }),
            )
            .await
        }
        Action::SecretFile { path } => {
            let content = secret.ok_or("secret absent")?;
            crate::agent_guard::guarded_write(
                core,
                "write_file",
                json!({ "server_id": s.server_id, "path": path, "content": content }),
            )
            .await
        }
    }
}

/// Outil MCP `link_proxy`.
pub(crate) async fn link_proxy_tool(core: &Core, args: &Value) -> Result<String, String> {
    let a = parse_args(args)?;
    crate::mcp_ctx::progress(0.0, None, "lecture du proxy et des serveurs de jeu").await;
    let inputs = Box::pin(read_inputs(core, &a)).await?;
    let needs_secret = inputs.proxy.conf.kind == ProxyKind::Velocity
        && a.mode != Some(Mode::Legacy)
        && inputs.proxy.secret.is_none();
    let generated = if needs_secret {
        Some(generate_secret()?)
    } else {
        None
    };
    let secrets = secrets_of(&inputs, generated.as_deref());
    let plan = {
        let redact = |s: &str| core.redact_ai(s);
        build_plan(
            &ProxyInput {
                state: &inputs.proxy,
                config_text: &inputs.config_text,
            },
            &inputs.backends,
            &a,
            generated.as_deref(),
            &redact,
        )
        .map_err(|e| scrub_text(&e, &secrets))?
    };
    let secret: Option<String> = match plan.secret {
        SecretPlan::NotUsed => None,
        SecretPlan::Reused(_) => inputs.proxy.secret.clone(),
        SecretPlan::Generated => generated.clone(),
    };
    let finish = |mut v: Value, status: &str, next: &str| {
        v["status"] = json!(status);
        v["next"] = json!(next);
        crate::config_edit::render_for_ai(core, scrub_value(v, &secrets))
    };
    let mut outcomes: HashMap<String, (String, Option<String>)> = HashMap::new();
    let todo: Vec<&Step> = plan
        .steps
        .iter()
        .filter(|s| s.status == Status::Change)
        .collect();
    let refused = plan.steps.iter().any(|s| s.status == Status::Refused);

    if a.dry_run {
        let next = if refused {
            "Des étapes sont refusées (fichier illisible ou chemin refusé) : corrige-les d'abord, rien ne sera écrit tant qu'elles restent."
        } else if todo.is_empty() {
            "Rien à écrire. Les étapes manual restent à faire à la main ; vérifie avec network_topology."
        } else {
            "Montre ce plan à l'utilisateur ; après son accord, relance avec dry_run=false (snapshot de chaque serveur modifié d'abord, sauf snapshot=false)."
        };
        return Ok(finish(
            render_plan(&inputs.proxy, &plan, true, &outcomes),
            "simulation : rien n'a été écrit",
            next,
        ));
    }
    if refused {
        return Err(finish(
            render_plan(&inputs.proxy, &plan, false, &outcomes),
            "refusé : rien n'a été écrit",
            "Corrige les étapes refused (fichier invalide : edit_file ; chemin du secret), puis relance.",
        ));
    }
    if todo.is_empty() {
        return Ok(finish(
            render_plan(&inputs.proxy, &plan, false, &outcomes),
            "déjà conforme : rien à écrire",
            "Les étapes manual restent à faire à la main ; vérifie avec network_topology.",
        ));
    }

    // Snapshots préalables, attendues : sans elles, rien n'est écrit.
    let mut snapshots = Vec::new();
    if a.snapshot {
        let mut ids: Vec<(i64, String)> = Vec::new();
        for s in &todo {
            if !ids.iter().any(|(id, _)| *id == s.server_id) {
                ids.push((s.server_id, s.server.clone()));
            }
        }
        let name = format!(
            "Avant link_proxy {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        crate::mcp_ctx::progress(0.0, Some(todo.len() as f64 + 1.0), "snapshots préalables").await;
        let mut jobs = Vec::new();
        for (id, server) in &ids {
            let job = core.create_snapshot(*id, &name).await.map_err(|e| {
                format!("snapshot préalable impossible sur « {server} » ({e}) : rien n'a été modifié ; relance avec snapshot=false pour passer outre")
            })?;
            jobs.push((*id, server.clone(), job));
        }
        for (id, server, job) in &jobs {
            Box::pin(crate::manifest::wait_snapshot(core, *job, &name)).await.map_err(|e| {
                format!("« {server} » : {e} ; rien n'a été modifié (relance plus tard, ou avec snapshot=false)")
            })?;
            snapshots
                .push(json!({ "server_id": id, "server": server, "name": name, "job_id": job }));
        }
    }

    // Écritures, proxy d'abord : un secret généré est posé sur le proxy avant les serveurs de jeu,
    // si bien qu'une relance après une coupure réutilise le même.
    let total = todo.len() as f64 + 1.0;
    let mut failed = false;
    let mut cancelled = false;
    let mut applied = 0usize;
    for (i, s) in todo.iter().enumerate() {
        if failed || cancelled || crate::mcp_ctx::cancelled() {
            let why = if failed {
                "arrêt après un échec"
            } else {
                "annulé"
            };
            cancelled = !failed;
            outcomes.insert(s.id.clone(), ("non exécutée".into(), Some(why.into())));
            continue;
        }
        crate::mcp_ctx::progress(i as f64 + 1.0, Some(total), &s.summary).await;
        match execute(core, s, secret.as_deref()).await {
            Ok(note) => {
                applied += 1;
                outcomes.insert(s.id.clone(), ("réussie".into(), note));
            }
            Err(e) => {
                failed = true;
                outcomes.insert(s.id.clone(), ("échouée".into(), Some(e)));
            }
        }
    }
    let mut v = render_plan(&inputs.proxy, &plan, false, &outcomes);
    v["applied"] = json!(applied);
    if !snapshots.is_empty() {
        v["snapshots"] = json!(snapshots);
    }
    let status = if failed {
        "terminé avec un échec : étapes suivantes non exécutées"
    } else if cancelled {
        "annulé : étapes suivantes non exécutées"
    } else {
        "terminé"
    };
    let next = if failed || cancelled {
        "Les étapes réussies sont annulables (list_changesets puis rollback_changeset) ; ou corrige la cause et relance link_proxy (idempotent : ce qui est conforme n'est pas refait)."
    } else {
        "Redémarre les serveurs de jeu puis le proxy (power_action restart) pour charger les réglages, puis vérifie : network_topology, tail_console du proxy (serveurs enregistrés, aucune erreur de transfert). Annulation : list_changesets puis rollback_changeset, ou les snapshots."
    };
    Ok(finish(v, status, next))
}
