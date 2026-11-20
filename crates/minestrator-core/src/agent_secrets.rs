//! Secrets des agents : valeurs nommées qu'un agent MCP écrit dans les configurations des serveurs
//! **sans jamais les voir**, par la référence `{{secret:NOM}}` (token d'un bot Discord, clé d'API
//! d'un plugin, mot de passe RCON…).
//!
//! **Stockage.** La valeur vit dans le stockage sécurisé natif (`secrets.rs` : trousseau de l'OS,
//! Keystore Android, fichier chiffré du service), une entrée par secret (`agent-secret-<nom>`,
//! [`crate::config::KEYRING_ACCOUNT_AGENT_SECRET_PREFIX`]). L'index **non secret** (nom,
//! description, date de création, valeur générée ou saisie) vit dans `agent_secrets.json`, relu à
//! chaque accès : l'app et les processus MCP le partagent. Nom : `[a-z0-9_-]`, 1 à 64 caractères.
//! Valeur : une seule ligne de 8 à 4096 caractères imprimables, sans référence. Jamais de valeur
//! dans un `PersistedConfig`, le `localStorage` ou un journal.
//!
//! **Références.** `{{secret:NOM}}` est remplacée par la valeur juste avant l'écriture, dans les
//! valeurs de `set_config`, le contenu de `write_file`, le remplacement (`replace`) d'`edit_file`,
//! la valeur de `set_property`, et les configs, réglages et fichiers d'un manifeste
//! (`plan_manifest`, `apply_manifest`). Règles :
//! - un nom inconnu ou une référence mal formée est une erreur **avant tout effet** ;
//! - le **Copilote** ne résout jamais une référence (refus explicite,
//!   [`crate::agent_guard::copilot_call`]) : il lit du contenu non fiable (chat des joueurs) et
//!   pourrait faire écrire un secret là où il le relirait ;
//! - toute valeur résolue, créée ou chargée est enregistrée dans `redact.rs`
//!   ([`crate::redact::register_secret`]) : sorties, diffs et journaux montrent `{{secret:NOM}}` à
//!   sa place, quelle que soit la clé qui la porte, même anonymisation IA désactivée
//!   (`Core::redact_ai`, et la sortie de chaque outil dans `mcp::dispatch`). Au premier usage dans
//!   un processus, puis à chaque changement de l'index, les valeurs de tous les secrets y sont
//!   chargées ([`Core::refresh_secret_masks`]) : une valeur écrite par un autre processus reste
//!   masquée ici.
//!
//! **Point d'accroche** pour un outil qui écrit du contenu fourni par un agent (par exemple
//! `sync_dir`, dossier local → serveur) : [`Core::resolve_secret_refs`] rend le texte résolu et
//! les noms utilisés, ou une erreur à renvoyer telle quelle (Copilote, nom inconnu, référence mal
//! formée). L'appeler avant tout effet, n'écrire que le texte résolu, ne montrer que le texte
//! d'origine (ou [`used_note`]). Plusieurs textes d'un même appel : [`Core::resolve_secret_texts`],
//! qui vérifie toutes les références avant la première substitution.
//!
//! **Outils MCP** : `list_secrets` (lecture : noms et descriptions) et `generate_secret` (secret
//! aléatoire, jamais rendu ; `INFRA_TOOLS`). Ajout d'une valeur saisie et suppression : réservés à
//! l'app (Réglages → MCP → Secrets des agents).

use crate::error::{Error, Result};
use crate::secrets::{self, SecretStore, SystemStore};
use crate::store::now_secs;
use crate::Core;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

/// Index non secret, dans le dossier de données.
const INDEX_FILE: &str = "agent_secrets.json";
const INDEX_TMP: &str = "agent_secrets.json.tmp";
/// Longueur maximale d'un nom.
pub(crate) const NAME_MAX_CHARS: usize = 64;
/// Longueur minimale d'une valeur : plus court, la chercher dans les sorties pour la masquer
/// couperait du texte ordinaire.
pub(crate) const VALUE_MIN_CHARS: usize = 8;
/// Longueur maximale d'une valeur.
pub(crate) const VALUE_MAX_CHARS: usize = 4096;
/// Longueur maximale d'une description.
pub(crate) const DESCRIPTION_MAX_CHARS: usize = 200;
/// Secrets au plus.
pub(crate) const MAX_SECRETS: usize = 100;
/// Longueur par défaut d'un secret généré.
pub(crate) const GENERATED_DEFAULT_CHARS: usize = 32;
/// Longueur maximale d'un secret généré.
pub(crate) const GENERATED_MAX_CHARS: usize = 256;
/// Début et fin d'une référence.
const REF_OPEN: &str = "{{secret:";
const REF_CLOSE: &str = "}}";

/// Refus opposé au Copilote.
const COPILOT_REFUSAL: &str = "Référence {{secret:…}} refusée : le Copilote ne peut pas utiliser les secrets des agents (il lit du contenu non fiable, comme le chat des joueurs). Rien n'a été écrit ; l'utilisateur peut faire cette modification depuis l'app ou un client MCP.";

// --- Types exposés -----------------------------------------------------------------------------

/// Secret des agents, tel que listé (jamais sa valeur).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentSecret {
    /// Nom, tel qu'écrit dans la référence `{{secret:NOM}}` (`[a-z0-9_-]`, 64 caractères au plus).
    pub name: String,
    /// À quoi sert le secret (texte libre, non secret, montré aux agents).
    #[serde(default)]
    pub description: String,
    /// Date de création (secondes Unix).
    pub created_at: i64,
    /// Valeur tirée au hasard (« Générer », `generate_secret`) plutôt que saisie.
    #[serde(default)]
    pub generated: bool,
}

/// Caractères d'un secret généré.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SecretCharset {
    /// Lettres et chiffres (défaut) : s'écrit tel quel dans tous les formats.
    #[default]
    Alphanumeric,
    /// Chiffres hexadécimaux minuscules.
    Hex,
    /// Lettres, chiffres, `-` et `_`.
    UrlSafe,
    /// Lettres, chiffres et ponctuation `!#%+-.=?@^_~` ; le premier caractère reste une lettre ou
    /// un chiffre.
    Symbols,
}

const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

impl SecretCharset {
    fn alphabet(self) -> &'static [u8] {
        match self {
            SecretCharset::Alphanumeric => ALNUM,
            SecretCharset::Hex => b"0123456789abcdef",
            SecretCharset::UrlSafe => {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            }
            SecretCharset::Symbols => {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#%+-.=?@^_~"
            }
        }
    }

    /// Nom d'argument MCP (`alphanumeric`, `hex`, `url_safe`, `symbols`).
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "alphanumeric" | "alnum" => Some(Self::Alphanumeric),
            "hex" => Some(Self::Hex),
            "url_safe" | "urlsafe" => Some(Self::UrlSafe),
            "symbols" => Some(Self::Symbols),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SecretCharset::Alphanumeric => "lettres et chiffres",
            SecretCharset::Hex => "hexadécimal",
            SecretCharset::UrlSafe => "lettres, chiffres, - et _",
            SecretCharset::Symbols => "lettres, chiffres et ponctuation",
        }
    }
}

// --- Validation --------------------------------------------------------------------------------

/// Nom valide : `[a-z0-9_-]`, 1 à 64 caractères.
pub(crate) fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("nom de secret vide".into());
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(format!("nom de secret trop long ({NAME_MAX_CHARS} caractères au plus)"));
    }
    if !name.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-')) {
        return Err(format!(
            "nom de secret « {} » invalide : lettres minuscules sans accent, chiffres, - et _ seulement",
            crate::util::truncate_on_boundary(name, NAME_MAX_CHARS, "…")
        ));
    }
    Ok(())
}

/// Valeur saisie valide : une ligne de caractères imprimables, 8 à 4096, sans référence.
fn validate_value(value: &str) -> std::result::Result<(), String> {
    let n = value.chars().count();
    if n < VALUE_MIN_CHARS {
        return Err(format!("valeur trop courte ({VALUE_MIN_CHARS} caractères au moins)"));
    }
    if n > VALUE_MAX_CHARS {
        return Err(format!("valeur trop longue ({VALUE_MAX_CHARS} caractères au plus)"));
    }
    if value.chars().any(char::is_control) {
        return Err("la valeur doit tenir sur une ligne, sans caractère de contrôle".into());
    }
    if value.contains(REF_OPEN) {
        return Err("une valeur ne peut pas contenir de référence {{secret:…}}".into());
    }
    Ok(())
}

/// Description nettoyée (espaces de bord retirés) et validée.
fn clean_description(description: &str) -> std::result::Result<String, String> {
    let d = description.trim();
    if d.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(format!("description trop longue ({DESCRIPTION_MAX_CHARS} caractères au plus)"));
    }
    if d.chars().any(char::is_control) {
        return Err("la description doit tenir sur une ligne".into());
    }
    Ok(d.to_string())
}

fn invalid(e: String) -> Error {
    Error::Invalid(e)
}

// --- Références --------------------------------------------------------------------------------

/// Le texte contient-il une référence, même mal formée ?
pub(crate) fn has_refs(text: &str) -> bool {
    text.contains(REF_OPEN)
}

/// Une valeur JSON contient-elle une référence (dans un texte, à n'importe quelle profondeur) ?
pub(crate) fn value_has_refs(v: &Value) -> bool {
    match v {
        Value::String(s) => has_refs(s),
        Value::Array(a) => a.iter().any(value_has_refs),
        Value::Object(m) => m.iter().any(|(k, x)| has_refs(k) || value_has_refs(x)),
        _ => false,
    }
}

/// Références d'un texte : plage (octets) et nom. Erreur sur une référence mal formée (non fermée,
/// nom invalide) : elle serait écrite telle quelle.
pub(crate) fn parse_refs(text: &str) -> std::result::Result<Vec<(Range<usize>, String)>, String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(REF_OPEN) {
        let start = from + i;
        let name_start = start + REF_OPEN.len();
        let snippet = || crate::util::truncate_on_boundary(&text[start..], 40, "…");
        let Some(len) = text[name_start..].find(REF_CLOSE) else {
            return Err(format!("référence non fermée : « {} » (forme attendue : {{{{secret:NOM}}}})", snippet()));
        };
        let name = &text[name_start..name_start + len];
        validate_name(name).map_err(|e| format!("référence « {} » : {e}", snippet()))?;
        let end = name_start + len + REF_CLOSE.len();
        out.push((start..end, name.to_string()));
        from = end;
    }
    Ok(out)
}

/// Texte où chaque référence est remplacée par sa valeur.
fn substitute(text: &str, refs: &[(Range<usize>, String)], values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (range, name) in refs {
        out.push_str(&text[pos..range.start]);
        out.push_str(values.get(name).map(String::as_str).unwrap_or_default());
        pos = range.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Textes d'une valeur JSON, en écriture.
fn strings_mut<'a>(v: &'a mut Value, out: &mut Vec<&'a mut String>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter_mut().for_each(|x| strings_mut(x, out)),
        Value::Object(m) => m.iter_mut().for_each(|(_, x)| strings_mut(x, out)),
        _ => {}
    }
}

/// Mention d'une sortie d'outil : secrets écrits par référence (vide s'il n'y en a pas).
pub(crate) fn used_note(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let refs: Vec<String> = names.iter().map(|n| format!("{REF_OPEN}{n}{REF_CLOSE}")).collect();
    format!(" Secret(s) écrit(s) par référence, valeur jamais affichée : {}.", refs.join(", "))
}

/// Refus d'une référence dans un champ qui ne la résout pas (clé de config, texte cherché).
pub(crate) fn refuse_refs(field: &str, text: &str) -> std::result::Result<(), String> {
    if has_refs(text) {
        return Err(format!(
            "{field} : les références {{{{secret:…}}}} ne sont pas résolues ici (la valeur d'un secret ne se cherche ni ne sert de clé) ; rien n'a été modifié"
        ));
    }
    Ok(())
}

/// [`refuse_refs`] sur les clés d'une valeur JSON, à toute profondeur (seuls les textes sont
/// résolus).
fn refuse_refs_in_keys(v: &Value) -> std::result::Result<(), String> {
    match v {
        Value::Array(a) => a.iter().try_for_each(refuse_refs_in_keys),
        Value::Object(m) => m.iter().try_for_each(|(k, x)| {
            refuse_refs(&format!("clé « {k} »"), k)?;
            refuse_refs_in_keys(x)
        }),
        _ => Ok(()),
    }
}

// --- Génération ----------------------------------------------------------------------------------

/// `count` caractères tirés uniformément dans `alphabet` (générateur aléatoire du système).
fn draw(alphabet: &[u8], count: usize, out: &mut String) -> std::result::Result<(), String> {
    let rng = rustls::crypto::ring::default_provider().secure_random;
    // Rejet au-delà du dernier multiple de la taille de l'alphabet : tirage uniforme.
    let limit = 256 - 256 % alphabet.len();
    let mut buf = [0u8; 64];
    let mut left = count;
    while left > 0 {
        rng.fill(&mut buf)
            .map_err(|_| "générateur aléatoire du système indisponible".to_string())?;
        for b in buf.iter().filter(|b| usize::from(**b) < limit) {
            out.push(char::from(alphabet[usize::from(*b) % alphabet.len()]));
            left -= 1;
            if left == 0 {
                break;
            }
        }
    }
    Ok(())
}

/// Valeur aléatoire de `length` caractères (8 à 256).
pub(crate) fn generate_value(length: usize, charset: SecretCharset) -> std::result::Result<String, String> {
    if !(VALUE_MIN_CHARS..=GENERATED_MAX_CHARS).contains(&length) {
        return Err(format!(
            "longueur {length} : entre {VALUE_MIN_CHARS} et {GENERATED_MAX_CHARS} caractères"
        ));
    }
    let mut out = String::with_capacity(length);
    // Premier caractère alphanumérique : `#` ou `!` en tête serait un commentaire dans un fichier
    // `.properties` écrit à la main, `-` une option sur une ligne de commande.
    let mut rest = length;
    if matches!(charset, SecretCharset::Symbols | SecretCharset::UrlSafe) {
        draw(ALNUM, 1, &mut out)?;
        rest -= 1;
    }
    draw(charset.alphabet(), rest, &mut out)?;
    Ok(out)
}

// --- Index et stockage -------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    secrets: Vec<AgentSecret>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Empreinte de l'index (date de modification, taille), `None` s'il n'existe pas.
type Stamp = Option<(Option<SystemTime>, u64)>;

/// Secrets des agents d'un dossier de données : index non secret et valeurs déjà masquées.
pub(crate) struct AgentSecrets {
    dir: PathBuf,
    /// Sérialise les modifications de l'index dans ce processus.
    write: Mutex<()>,
    /// Empreinte de l'index au dernier chargement des masques (`None` : jamais chargé).
    masks_seen: Mutex<Option<Stamp>>,
    /// Secrets dont la valeur est enregistrée dans `redact.rs` par ce processus.
    masked: Mutex<HashSet<String>>,
}

impl AgentSecrets {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            write: Mutex::new(()),
            masks_seen: Mutex::new(None),
            masked: Mutex::new(HashSet::new()),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join(INDEX_FILE)
    }

    fn load(&self) -> Result<Vec<AgentSecret>> {
        match std::fs::read_to_string(self.index_path()) {
            Ok(text) => serde_json::from_str::<IndexFile>(&text)
                .map(|f| f.secrets)
                .map_err(|e| Error::Unexpected(format!("{INDEX_FILE} illisible : {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::Unexpected(format!("{INDEX_FILE} : {e}"))),
        }
    }

    /// Écriture atomique (fichier temporaire puis renommage).
    fn save(&self, secrets: &[AgentSecret]) -> Result<()> {
        let io = |e: std::io::Error| Error::Unexpected(format!("écriture de {INDEX_FILE} : {e}"));
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let json = serde_json::to_string_pretty(&IndexFile { secrets: secrets.to_vec() })
            .map_err(|e| Error::Unexpected(e.to_string()))?;
        let tmp = self.dir.join(INDEX_TMP);
        let _ = std::fs::remove_file(&tmp);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut f| {
                f.write_all(json.as_bytes())?;
                f.sync_all()
            })
            .map_err(io)?;
        std::fs::rename(&tmp, self.index_path()).map_err(io)
    }

    /// Secrets enregistrés, par nom.
    pub(crate) fn list(&self) -> Result<Vec<AgentSecret>> {
        let mut list = self.load()?;
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    fn remember_mask(&self, name: &str, value: &str) {
        crate::redact::register_secret(name, value);
        lock(&self.masked).insert(name.to_string());
    }

    /// Enregistre un nouveau secret : valeur dans le stockage (relue pour vérification), puis
    /// entrée d'index. Un nom déjà pris est refusé (jamais d'écrasement).
    fn insert(
        &self,
        store: &dyn SecretStore,
        name: &str,
        description: &str,
        value: &str,
        generated: bool,
        now: i64,
    ) -> Result<AgentSecret> {
        validate_name(name).map_err(invalid)?;
        let description = clean_description(description).map_err(invalid)?;
        validate_value(value).map_err(invalid)?;
        let _w = lock(&self.write);
        let mut list = self.load()?;
        if list.iter().any(|s| s.name == name) {
            return Err(invalid(format!(
                "un secret « {name} » existe déjà : choisis un autre nom (le remplacer demande de le supprimer d'abord dans l'app)"
            )));
        }
        if list.len() >= MAX_SECRETS {
            return Err(invalid(format!("{MAX_SECRETS} secrets au plus : supprime ceux qui ne servent plus")));
        }
        let account = secrets::agent_secret_account(name);
        store.write(&account, value)?;
        match store.read(&account) {
            Ok(Some(v)) if v == value => {}
            other => {
                let _ = store.delete(&account);
                return Err(match other {
                    Err(e) => e,
                    _ => Error::Keyring(format!("secret « {name} » non relu après écriture")),
                });
            }
        }
        let entry = AgentSecret { name: name.to_string(), description, created_at: now, generated };
        list.push(entry.clone());
        if let Err(e) = self.save(&list) {
            let _ = store.delete(&account);
            return Err(e);
        }
        self.remember_mask(name, value);
        Ok(entry)
    }

    /// Ajoute un secret saisi par l'utilisateur (espaces de bord retirés).
    pub(crate) fn add(
        &self,
        store: &dyn SecretStore,
        name: &str,
        description: &str,
        value: &str,
        now: i64,
    ) -> Result<AgentSecret> {
        self.insert(store, name, description, value.trim(), false, now)
    }

    /// Crée un secret aléatoire ; sa valeur ne sort jamais d'ici.
    pub(crate) fn generate(
        &self,
        store: &dyn SecretStore,
        name: &str,
        description: &str,
        length: usize,
        charset: SecretCharset,
        now: i64,
    ) -> Result<AgentSecret> {
        validate_name(name).map_err(invalid)?;
        let value = generate_value(length, charset).map_err(invalid)?;
        self.insert(store, name, description, &value, true, now)
    }

    /// Supprime un secret : valeur d'abord (un échec ne touche pas à l'index), puis entrée. La
    /// valeur reste masquée dans ce processus : les configs qui l'ont reçue la gardent.
    pub(crate) fn remove(&self, store: &dyn SecretStore, name: &str) -> Result<()> {
        let _w = lock(&self.write);
        let mut list = self.load()?;
        let Some(pos) = list.iter().position(|s| s.name == name) else {
            return Err(invalid(format!("secret « {name} » introuvable")));
        };
        store.delete(&secrets::agent_secret_account(name))?;
        list.remove(pos);
        self.save(&list)
    }

    /// Valeurs des secrets `names` (tous dans l'index et dans le stockage), sinon l'erreur à
    /// rendre à l'agent, qui cite tous les noms inconnus.
    fn lookup(&self, store: &dyn SecretStore, names: &[String]) -> std::result::Result<BTreeMap<String, String>, String> {
        let index = self.load().map_err(|e| e.to_string())?;
        let unknown: Vec<&str> = names
            .iter()
            .filter(|n| !index.iter().any(|s| s.name == **n))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "secret(s) inconnu(s) : {} — rien n'a été écrit. Noms disponibles : list_secrets ; un secret s'ajoute dans l'app (Réglages → MCP → Secrets des agents) ou se crée avec generate_secret.",
                unknown.join(", ")
            ));
        }
        let mut values = BTreeMap::new();
        for name in names {
            match store.read(&secrets::agent_secret_account(name)) {
                Ok(Some(v)) => {
                    values.insert(name.clone(), v);
                }
                Ok(None) => {
                    return Err(format!(
                        "le secret « {name} » n'a plus de valeur dans le stockage sécurisé : supprime-le puis recrée-le dans l'app ; rien n'a été écrit"
                    ))
                }
                Err(e) => return Err(format!("stockage sécurisé illisible ({e}) : rien n'a été écrit")),
            }
        }
        Ok(values)
    }

    /// Résout les références de plusieurs textes d'un même appel, en place. Toutes sont vérifiées
    /// (forme, droit, noms connus) avant la première substitution. `allowed` n'est consulté que
    /// s'il y a une référence. Rend les noms utilisés, dans l'ordre.
    pub(crate) fn resolve_in_place(
        &self,
        store: &dyn SecretStore,
        texts: Vec<&mut String>,
        allowed: impl FnOnce() -> bool,
    ) -> std::result::Result<Vec<String>, String> {
        if !texts.iter().any(|t| has_refs(t)) {
            return Ok(Vec::new());
        }
        if !allowed() {
            return Err(COPILOT_REFUSAL.into());
        }
        let mut wanted: Vec<String> = Vec::new();
        let mut parsed = Vec::with_capacity(texts.len());
        for t in &texts {
            let refs = parse_refs(t)?;
            for (_, n) in &refs {
                if !wanted.contains(n) {
                    wanted.push(n.clone());
                }
            }
            parsed.push(refs);
        }
        let values = self.lookup(store, &wanted)?;
        for (name, value) in &values {
            self.remember_mask(name, value);
        }
        for (t, refs) in texts.into_iter().zip(parsed) {
            if !refs.is_empty() {
                *t = substitute(t, &refs, &values);
            }
        }
        Ok(wanted)
    }

    /// Enregistre dans `redact.rs` les valeurs des secrets que ce processus ne masque pas encore :
    /// au premier appel, puis quand l'index change. Rien n'est lu dans le stockage sans secret
    /// enregistré.
    fn refresh_masks(&self, store: &dyn SecretStore) {
        let stamp: Stamp = std::fs::metadata(self.index_path())
            .ok()
            .map(|m| (m.modified().ok(), m.len()));
        {
            let mut seen = lock(&self.masks_seen);
            if seen.as_ref() == Some(&stamp) {
                return;
            }
            *seen = Some(stamp);
        }
        if stamp.is_none() {
            return;
        }
        let list = match self.load() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("secrets des agents : {e}");
                return;
            }
        };
        let known = lock(&self.masked).clone();
        for s in list.iter().filter(|s| !known.contains(&s.name)) {
            match store.read(&secrets::agent_secret_account(&s.name)) {
                Ok(Some(v)) => self.remember_mask(&s.name, &v),
                Ok(None) => {}
                Err(e) => tracing::warn!(secret = %s.name, "secret des agents illisible pour le masquage : {e}"),
            }
        }
    }
}

// --- Façade Core -------------------------------------------------------------------------------

impl Core {
    /// Secrets des agents enregistrés, par nom (jamais leurs valeurs).
    pub fn agent_secrets(&self) -> Result<Vec<AgentSecret>> {
        self.agent_secrets.list()
    }

    /// Ajoute un secret des agents saisi par l'utilisateur. La valeur va au stockage sécurisé et
    /// n'en ressort que pour être écrite par une référence `{{secret:NOM}}`.
    pub fn add_agent_secret(&self, name: &str, description: &str, value: &str) -> Result<AgentSecret> {
        self.agent_secrets.add(&SystemStore, name, description, value, now_secs())
    }

    /// Crée un secret des agents aléatoire (`length` : 8 à 256, défaut 32), jamais affiché.
    pub fn generate_agent_secret(
        &self,
        name: &str,
        description: &str,
        length: Option<u32>,
        charset: Option<SecretCharset>,
    ) -> Result<AgentSecret> {
        let length = length.map_or(GENERATED_DEFAULT_CHARS, |l| l as usize);
        self.agent_secrets
            .generate(&SystemStore, name, description, length, charset.unwrap_or_default(), now_secs())
    }

    /// Supprime un secret des agents (valeur et entrée). Les configs qui en ont reçu la valeur la
    /// gardent ; une référence à ce nom devient une erreur.
    pub fn delete_agent_secret(&self, name: &str) -> Result<()> {
        self.agent_secrets.remove(&SystemStore, name)
    }

    /// Masques des secrets des agents à jour (voir [`AgentSecrets::refresh_masks`]). Appelé avant
    /// toute sortie vers un agent (`Core::redact_ai`, `mcp::dispatch`).
    pub(crate) fn refresh_secret_masks(&self) {
        self.agent_secrets.refresh_masks(&SystemStore);
    }

    /// **Point d'accroche** : résout les références `{{secret:NOM}}` d'un texte fourni par un agent,
    /// juste avant de l'écrire. Rend le texte résolu (à écrire, jamais à montrer) et les noms
    /// utilisés, ou l'erreur à rendre telle quelle : appel du Copilote, nom inconnu ou référence
    /// mal formée. À appeler avant tout effet ; les valeurs résolues sont masquées ensuite dans
    /// toute sortie vers l'IA.
    pub(crate) fn resolve_secret_refs(&self, text: &str) -> std::result::Result<(String, Vec<String>), String> {
        let mut out = text.to_string();
        let used = self.resolve_secret_texts(vec![&mut out])?;
        Ok((out, used))
    }

    /// [`Core::resolve_secret_refs`] sur plusieurs textes d'un même appel, en place : toutes les
    /// références sont vérifiées avant la première substitution.
    pub(crate) fn resolve_secret_texts(&self, texts: Vec<&mut String>) -> std::result::Result<Vec<String>, String> {
        self.agent_secrets
            .resolve_in_place(&SystemStore, texts, || !crate::agent_guard::copilot_call())
    }
}

/// Point d'accroche de `sync_dir` (`local_sync::TEXT_HOOK`) : un fichier texte qui contient une
/// référence `{{secret:NOM}}` part résolu (comparé au fichier distant puis écrit, jamais affiché) ;
/// sans référence, il est envoyé tel quel. Mêmes règles que partout : nom inconnu ou Copilote =
/// refus, avant le premier envoi.
pub(crate) fn sync_text_hook(core: &Core, _server_id: i64, _rel: &str, text: &str) -> std::result::Result<Option<String>, String> {
    resolved_text(text, |t| core.resolve_secret_refs(t))
}

/// Logique du point d'accroche, sans `Core` (testable).
fn resolved_text(
    text: &str,
    resolve: impl FnOnce(&str) -> std::result::Result<(String, Vec<String>), String>,
) -> std::result::Result<Option<String>, String> {
    if !has_refs(text) {
        return Ok(None);
    }
    resolve(text).map(|(out, _)| Some(out))
}

/// Références des modifications de `set_config` : résolues dans les valeurs, refusées dans les clés.
pub(crate) fn resolve_edit_requests(
    core: &Core,
    requests: &mut [crate::config_edit::EditRequest],
) -> std::result::Result<Vec<String>, String> {
    core.resolve_secret_texts(edit_request_texts(requests)?)
}

/// Textes à résoudre des modifications de `set_config` (valeurs) ; une référence dans une clé est
/// refusée.
fn edit_request_texts(
    requests: &mut [crate::config_edit::EditRequest],
) -> std::result::Result<Vec<&mut String>, String> {
    let mut texts = Vec::new();
    if !requests.iter().any(|r| has_refs(&r.key) || value_has_refs(&r.value)) {
        return Ok(texts);
    }
    for r in requests.iter() {
        refuse_refs(&format!("clé « {} »", r.key), &r.key)?;
        refuse_refs_in_keys(&r.value)?;
    }
    for r in requests.iter_mut() {
        strings_mut(&mut r.value, &mut texts);
    }
    Ok(texts)
}

/// Références d'un manifeste : valeurs des configs et des réglages, contenu des fichiers.
pub(crate) fn resolve_manifest(core: &Core, m: &mut crate::manifest::Manifest) -> std::result::Result<Vec<String>, String> {
    core.resolve_secret_texts(manifest_texts(m)?)
}

/// Textes à résoudre d'un manifeste ; une référence dans une clé ou un chemin est refusée.
fn manifest_texts(m: &mut crate::manifest::Manifest) -> std::result::Result<Vec<&mut String>, String> {
    let mut texts: Vec<&mut String> = Vec::new();
    for c in m.configs.iter_mut() {
        for (k, v) in c.set.iter_mut() {
            refuse_refs(&format!("configs « {} », clé « {k} »", c.path), k)?;
            refuse_refs_in_keys(v)?;
            strings_mut(v, &mut texts);
        }
    }
    if let Some(props) = m.properties.as_mut() {
        for (k, v) in props.iter_mut() {
            refuse_refs(&format!("properties, clé « {k} »"), k)?;
            refuse_refs_in_keys(v)?;
            strings_mut(v, &mut texts);
        }
    }
    for f in m.files.iter_mut() {
        refuse_refs("files, chemin", &f.path)?;
        texts.push(&mut f.content);
    }
    Ok(texts)
}

// --- Outils MCP ----------------------------------------------------------------------------------

fn date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// Rendu de `list_secrets` (pur).
pub(crate) fn render_list(list: &[AgentSecret]) -> Value {
    let usage = "Écris {{secret:NOM}} à la place de la valeur : valeurs de set_config, contenu de write_file, `replace` d'edit_file, valeur de set_property, configs, properties et fichiers d'un manifeste. La valeur est substituée juste avant l'écriture ; sorties et diffs montrent la référence. Ne demande jamais la valeur à l'utilisateur dans la conversation.";
    let mut out = json!({
        "secrets": list
            .iter()
            .map(|s| json!({
                "name": s.name,
                "reference": format!("{REF_OPEN}{}{REF_CLOSE}", s.name),
                "description": s.description,
                "created": date(s.created_at),
                "generated": s.generated,
            }))
            .collect::<Vec<_>>(),
        "usage": usage,
    });
    if list.is_empty() {
        out["note"] = json!("Aucun secret enregistré : generate_secret en crée un aléatoire ; une valeur existante (token, clé d'API) s'ajoute dans l'app, Réglages → MCP → Secrets des agents.");
    }
    out
}

/// Outil MCP `list_secrets` : noms et descriptions, jamais de valeur.
pub(crate) fn list_secrets_tool(core: &Core) -> std::result::Result<String, String> {
    let list = core.agent_secrets().map_err(|e| e.to_string())?;
    // Descriptions saisies par l'utilisateur : anonymisées valeur par valeur, JSON valide.
    Ok(crate::config_edit::render_for_ai(core, render_list(&list)))
}

/// Arguments de `generate_secret` : nom, description, longueur, caractères.
pub(crate) fn generate_args(args: &Value) -> std::result::Result<(String, String, usize, SecretCharset), String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or("paramètre `name` (texte) requis")?
        .trim()
        .to_string();
    validate_name(&name)?;
    let description = args.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
    let length = match args.get("length") {
        None | Some(Value::Null) => GENERATED_DEFAULT_CHARS,
        Some(v) => v
            .as_u64()
            .ok_or("`length` : entier attendu")
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))?,
    };
    let charset = match args.get("charset").and_then(Value::as_str) {
        None => SecretCharset::default(),
        Some(s) => SecretCharset::parse(s)
            .ok_or_else(|| format!("`charset` « {s} » inconnu : alphanumeric, hex, url_safe ou symbols"))?,
    };
    Ok((name, description, length, charset))
}

/// Outil MCP `generate_secret` : crée un secret aléatoire, ne rend jamais sa valeur.
pub(crate) fn generate_secret_tool(core: &Core, args: &Value) -> std::result::Result<String, String> {
    // Défense en profondeur : l'outil est déjà hors de portée du Copilote (`INFRA_TOOLS`).
    if crate::agent_guard::copilot_call() {
        return Err("generate_secret est réservé à l'app et aux clients MCP : jamais exécuté par le Copilote.".into());
    }
    let (name, description, length, charset) = generate_args(args)?;
    core.agent_secrets
        .generate(&SystemStore, &name, &description, length, charset, now_secs())
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Secret « {name} » généré ({length} caractères, {}) et rangé dans le stockage sécurisé ; sa valeur n'est jamais affichée. Utilise-le par référence : {REF_OPEN}{name}{REF_CLOSE} (valeurs de set_config, contenu de write_file, `replace` d'edit_file, set_property, configs d'un manifeste). Suppression : dans l'app seulement.",
        charset.label()
    ))
}

#[cfg(test)]
mod tests {
    /// `sync_dir` : un fichier sans référence part tel quel, sans toucher au stockage ; avec une
    /// référence, le texte résolu remplace le fichier ; un refus arrête tout.
    #[test]
    fn sync_dir_hook_resolves_only_files_with_references() {
        assert!(crate::local_sync::TEXT_HOOK.is_some(), "point d'accroche branché");
        let untouched = super::resolved_text("motd=Bonjour
", |_| panic!("stockage lu sans référence"));
        assert_eq!(untouched, Ok(None));
        let resolved = super::resolved_text("token: {{secret:discord}}
", |t| {
            Ok((t.replace("{{secret:discord}}", "valeur"), vec!["discord".into()]))
        });
        assert_eq!(resolved, Ok(Some("token: valeur
".into())));
        let refused = super::resolved_text("token: {{secret:inconnu}}", |_| Err("secret inconnu".into()));
        assert_eq!(refused, Err("secret inconnu".into()));
    }

    use super::*;
    use crate::secrets::DirStore;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minestrator-test-agent-secrets-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Index et stockage de test, chacun dans son dossier temporaire (jamais le trousseau réel).
    fn vault(name: &str) -> (AgentSecrets, DirStore, PathBuf) {
        let dir = test_dir(name);
        let store = DirStore { dir: dir.join("store") };
        (AgentSecrets::new(&dir.join("data")), store, dir)
    }

    #[test]
    fn names_are_validated() {
        for ok in ["discord_token", "rcon-pass", "a", "x9", &"a".repeat(64)] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Discord", "mot passe", "é", "a.b", "a/b", "{x}", &"a".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn values_are_one_printable_line_of_bounded_length() {
        assert!(validate_value("12345678").is_ok());
        assert!(validate_value("1234567").is_err(), "trop court");
        assert!(validate_value(&"x".repeat(VALUE_MAX_CHARS + 1)).is_err());
        assert!(validate_value("ligne1\nligne2").is_err());
        assert!(validate_value("abc{{secret:x}}def").is_err());
        assert!(clean_description(&"d".repeat(DESCRIPTION_MAX_CHARS + 1)).is_err());
        assert_eq!(clean_description("  bot Discord ").unwrap(), "bot Discord");
    }

    #[test]
    fn references_are_parsed_and_malformed_ones_refused() {
        let refs = parse_refs("a={{secret:db_pass}} b={{secret:tok-1}}{{secret:db_pass}}").unwrap();
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["db_pass", "tok-1", "db_pass"]);
        assert_eq!(&"a={{secret:db_pass}}"[refs[0].0.clone()], "{{secret:db_pass}}");
        assert!(parse_refs("sans référence {{autre}}").unwrap().is_empty());
        assert!(parse_refs("x {{secret:non_fermee").is_err());
        assert!(parse_refs("x {{secret:Majuscule}}").is_err());
        assert!(parse_refs("x {{secret:}}").is_err());
        assert!(parse_refs("x {{secret: espace}}").is_err());
        assert!(has_refs("{{secret:") && !has_refs("{{ secret:x }}"));
        assert!(value_has_refs(&json!({ "a": [1, { "b": "x{{secret:y}}" }] })));
        assert!(!value_has_refs(&json!({ "a": [1, "{{SERVER_JARFILE}}"] })));
    }

    #[test]
    fn generated_values_follow_length_and_charset() {
        for charset in [SecretCharset::Alphanumeric, SecretCharset::Hex, SecretCharset::UrlSafe, SecretCharset::Symbols] {
            let v = generate_value(48, charset).unwrap();
            assert_eq!(v.chars().count(), 48);
            assert!(v.bytes().all(|b| charset.alphabet().contains(&b)), "{charset:?} {v}");
            assert!(v.chars().next().unwrap().is_ascii_alphanumeric());
            assert_ne!(v, generate_value(48, charset).unwrap(), "deux tirages identiques");
        }
        assert_eq!(generate_value(8, SecretCharset::Symbols).unwrap().len(), 8);
        assert!(generate_value(7, SecretCharset::Hex).is_err());
        assert!(generate_value(GENERATED_MAX_CHARS + 1, SecretCharset::Hex).is_err());
        assert_eq!(SecretCharset::parse("URL-safe"), Some(SecretCharset::UrlSafe));
        assert_eq!(SecretCharset::parse("base32"), None);
    }

    #[test]
    fn secrets_are_stored_apart_from_their_index() {
        let (v, store, dir) = vault("store");
        let s = v.add(&store, "discord_token", " bot du serveur ", "  zz-valeur-index-31  ", 1_700_000_000).unwrap();
        assert_eq!((s.name.as_str(), s.description.as_str(), s.generated), ("discord_token", "bot du serveur", false));
        // Valeur dans le stockage (espaces de bord retirés), jamais dans l'index.
        assert_eq!(store.read("agent-secret-discord_token").unwrap().as_deref(), Some("zz-valeur-index-31"));
        let index = std::fs::read_to_string(dir.join("data").join(INDEX_FILE)).unwrap();
        assert!(index.contains("discord_token") && !index.contains("zz-valeur-index-31"), "{index}");
        // Doublon refusé, sans toucher à la valeur.
        assert!(v.add(&store, "discord_token", "", "autre-valeur-123", 0).is_err());
        assert_eq!(store.read("agent-secret-discord_token").unwrap().as_deref(), Some("zz-valeur-index-31"));
        // Généré : valeur absente de l'index et de la réponse.
        let g = v.generate(&store, "rcon", "", 24, SecretCharset::Hex, 2).unwrap();
        assert!(g.generated);
        let value = store.read("agent-secret-rcon").unwrap().unwrap();
        assert_eq!(value.len(), 24);
        assert!(!serde_json::to_string(&g).unwrap().contains(&value));
        assert_eq!(v.list().unwrap().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["discord_token", "rcon"]);
        // Suppression : valeur et entrée.
        v.remove(&store, "rcon").unwrap();
        assert_eq!(store.read("agent-secret-rcon").unwrap(), None);
        assert_eq!(v.list().unwrap().len(), 1);
        assert!(v.remove(&store, "rcon").is_err(), "déjà supprimé");
        // Entrées invalides refusées avant toute écriture.
        assert!(v.add(&store, "Mauvais Nom", "", "valeur-valide-1", 0).is_err());
        assert!(v.add(&store, "court", "", "1234", 0).is_err());
        assert_eq!(store.read("agent-secret-court").unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn references_resolve_before_any_effect_and_are_masked_afterwards() {
        let (v, store, dir) = vault("resolve");
        v.add(&store, "bot", "", "Tq9-resolve-unique-a1", 0).unwrap();
        v.add(&store, "db", "", "Tq9-resolve-unique-b2", 0).unwrap();
        let mut a = "token: {{secret:bot}}\npassword: \"{{secret:db}}\"".to_string();
        let mut b = "aucune".to_string();
        let used = v.resolve_in_place(&store, vec![&mut a, &mut b], || true).unwrap();
        assert_eq!(used, ["bot", "db"]);
        assert_eq!(a, "token: Tq9-resolve-unique-a1\npassword: \"Tq9-resolve-unique-b2\"");
        assert_eq!(b, "aucune");
        // Toute sortie anonymisée montre la référence, même sous une clé non secrète.
        assert_eq!(crate::redact::redact("motd: Tq9-resolve-unique-a1"), "motd: {{secret:bot}}");
        assert_eq!(crate::redact::mask_known_secrets("x=Tq9-resolve-unique-b2"), "x={{secret:db}}");

        // Un nom inconnu parmi plusieurs textes : erreur qui les cite, aucun texte modifié.
        let mut c = "{{secret:bot}}".to_string();
        let mut d = "{{secret:inconnu}} {{secret:absent}}".to_string();
        let err = v.resolve_in_place(&store, vec![&mut c, &mut d], || true).unwrap_err();
        assert!(err.contains("inconnu, absent") && err.contains("rien n'a été écrit"), "{err}");
        assert_eq!((c.as_str(), d.as_str()), ("{{secret:bot}}", "{{secret:inconnu}} {{secret:absent}}"));
        // Référence mal formée : erreur, rien de résolu.
        let mut e = "{{secret:bot}} {{secret:Bot}}".to_string();
        assert!(v.resolve_in_place(&store, vec![&mut e], || true).is_err());
        assert_eq!(e, "{{secret:bot}} {{secret:Bot}}");
        // Index et stockage désaccordés : erreur explicite.
        store.delete("agent-secret-db").unwrap();
        let mut f = "{{secret:db}}".to_string();
        assert!(v.resolve_in_place(&store, vec![&mut f], || true).unwrap_err().contains("plus de valeur"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_copilot_never_resolves_a_reference() {
        let (v, store, dir) = vault("copilot");
        v.add(&store, "bot", "", "Tq9-copilot-unique-c3", 0).unwrap();
        let mut a = "token: {{secret:bot}}".to_string();
        let err = v.resolve_in_place(&store, vec![&mut a], || false).unwrap_err();
        assert!(err.contains("Copilote"), "{err}");
        assert_eq!(a, "token: {{secret:bot}}");
        // Même une référence mal formée ou inconnue : le refus du Copilote passe d'abord.
        let mut b = "{{secret:Inconnu".to_string();
        assert!(v.resolve_in_place(&store, vec![&mut b], || false).unwrap_err().contains("Copilote"));
        // Sans référence, le droit n'est même pas consulté.
        let mut c = "rien".to_string();
        assert!(v.resolve_in_place(&store, vec![&mut c], || panic!("consulté")).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn masks_are_loaded_from_the_index_written_by_another_process() {
        let (writer, store, dir) = vault("masks");
        writer.add(&store, "autre", "", "Tq9-masks-unique-d4", 0).unwrap();
        // Un autre processus : même dossier, rien en mémoire. Le chargement passe par l'index.
        let reader = AgentSecrets::new(&dir.join("data"));
        reader.refresh_masks(&store);
        assert!(lock(&reader.masked).contains("autre"));
        assert_eq!(crate::redact::mask_known_secrets("v=Tq9-masks-unique-d4"), "v={{secret:autre}}");
        // Index inchangé : rien n'est relu ; secret ajouté ensuite : chargé au passage suivant.
        writer.add(&store, "nouveau", "", "Tq9-masks-unique-e5", 0).unwrap();
        reader.refresh_masks(&store);
        assert!(lock(&reader.masked).contains("nouveau"));
        // Dossier sans index : aucune lecture du stockage.
        let empty = AgentSecrets::new(&dir.join("vide"));
        empty.refresh_masks(&store);
        assert!(lock(&empty.masked).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn set_config_output_shows_the_reference_never_the_value() {
        use crate::config_edit::{edit_text, render_outcome, EditOptions, EditRequest, Format};
        let (v, store, dir) = vault("set-config");
        v.add(&store, "bot", "", "Tq9-setconfig-unique-f6", 0).unwrap();
        // Chiffres seuls, écrits sur une clé numérique : la valeur devient un nombre.
        v.add(&store, "port", "", "48213377", 0).unwrap();
        let mut reqs = vec![
            // Clé non reconnue comme secrète : seule la référence protège la valeur.
            EditRequest { key: "bot.discord".into(), value: json!("{{secret:bot}}") },
            EditRequest { key: "bot.token".into(), value: json!("Bearer {{secret:bot}}") },
            EditRequest { key: "web.port".into(), value: json!("{{secret:port}}") },
        ];
        let used = v.resolve_in_place(&store, edit_request_texts(&mut reqs).unwrap(), || true).unwrap();
        assert_eq!(used, ["bot", "port"]);
        let old = "bot:\n  discord: x\n  token: y\nweb:\n  port: 8080\n";
        let o = edit_text(Format::Yaml, "/c.yml", old, &reqs, EditOptions::default(), &|s| s.to_string()).unwrap();
        // Le fichier reçoit les valeurs…
        assert!(o.new_text.contains("discord: Tq9-setconfig-unique-f6"), "{}", o.new_text);
        assert!(o.new_text.contains("port: 48213377"), "{}", o.new_text);
        // … la sortie n'en montre aucune : changements et diff portent la référence.
        let out = crate::redact::mask_known_secrets_value(render_outcome("/c.yml", &o, false, true));
        let text = out.to_string();
        assert!(!text.contains("Tq9-setconfig-unique-f6") && !text.contains("48213377"), "{text}");
        assert!(text.contains("{{secret:bot}}") && text.contains("{{secret:port}}"), "{text}");
        assert!(out["diff"].as_str().unwrap().contains("+  discord: {{secret:bot}}"), "{}", out["diff"]);
        // Une référence dans une clé est refusée avant toute résolution.
        let mut bad = vec![EditRequest { key: "{{secret:bot}}".into(), value: json!(1) }];
        assert!(edit_request_texts(&mut bad).is_err());
        let mut nested = vec![EditRequest { key: "a".into(), value: json!({ "b": { "{{secret:bot}}": 1 } }) }];
        assert!(edit_request_texts(&mut nested).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn manifest_references_are_resolved_in_configs_properties_and_files() {
        let (v, store, dir) = vault("manifest");
        v.add(&store, "rcon", "", "Tq9-manifest-unique-g7", 0).unwrap();
        let mut m = crate::manifest::Manifest::parse(&json!({
            "version": 1,
            "properties": { "rcon.password": "{{secret:rcon}}", "motd": "Salut" },
            "configs": [{ "path": "/plugins/X/config.yml", "set": { "a.b": ["x", "{{secret:rcon}}"] } }],
            "files": [{ "path": "/plugins/Y/token.txt", "content": "token={{secret:rcon}}\n" }],
        }))
        .unwrap();
        let used = v.resolve_in_place(&store, manifest_texts(&mut m).unwrap(), || true).unwrap();
        assert_eq!(used, ["rcon"]);
        let props = m.properties.as_ref().unwrap();
        assert_eq!(props["rcon.password"], "Tq9-manifest-unique-g7");
        assert_eq!(props["motd"], "Salut");
        assert_eq!(m.configs[0].set["a.b"], json!(["x", "Tq9-manifest-unique-g7"]));
        assert_eq!(m.files[0].content, "token=Tq9-manifest-unique-g7\n");
        // Clé ou chemin porteur d'une référence : refusé.
        let mut bad = crate::manifest::Manifest::parse(&json!({
            "version": 1,
            "configs": [{ "path": "/c.yml", "set": { "{{secret:rcon}}": 1 } }],
        }))
        .unwrap();
        assert!(manifest_texts(&mut bad).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn listing_shows_names_and_references_never_values() {
        let list = vec![AgentSecret { name: "bot".into(), description: "Bot Discord".into(), created_at: 0, generated: true }];
        let out = render_list(&list);
        assert_eq!(out["secrets"][0]["reference"], "{{secret:bot}}");
        assert_eq!(out["secrets"][0]["description"], "Bot Discord");
        assert!(out.get("note").is_none());
        assert!(render_list(&[])["note"].as_str().unwrap().contains("generate_secret"));
        assert!(used_note(&[]).is_empty());
        assert!(used_note(&["a".into(), "b".into()]).contains("{{secret:a}}, {{secret:b}}"));
        assert!(refuse_refs("find", "x{{secret:a}}").is_err() && refuse_refs("find", "x").is_ok());
    }

    #[test]
    fn generate_arguments_are_checked() {
        let (name, desc, len, cs) = generate_args(&json!({ "name": "rcon" })).unwrap();
        assert_eq!((name.as_str(), desc.as_str(), len, cs), ("rcon", "", GENERATED_DEFAULT_CHARS, SecretCharset::Alphanumeric));
        let (_, _, len, cs) = generate_args(&json!({ "name": "x", "length": 64, "charset": "symbols" })).unwrap();
        assert_eq!((len, cs), (64, SecretCharset::Symbols));
        assert!(generate_args(&json!({})).is_err());
        assert!(generate_args(&json!({ "name": "X Y" })).is_err());
        assert!(generate_args(&json!({ "name": "x", "length": "32" })).is_err());
        assert!(generate_args(&json!({ "name": "x", "charset": "base32" })).is_err());
    }
}
