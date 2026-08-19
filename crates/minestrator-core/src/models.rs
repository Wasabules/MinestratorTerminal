//! Structures (dé)sérialisées échangées avec l'API et exposées aux frontends.
//!
//! Enveloppe commune : `{ api: { code, endpoint, data, error } }` — capturée en
//! `serde_json::Value` puis désérialisée vers le type attendu (pas de bornes génériques).

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub api: ApiBody,
}

#[derive(Debug, Deserialize)]
pub struct ApiBody {
    #[allow(dead_code)]
    #[serde(default)]
    pub code: Option<i64>,
    #[allow(dead_code)]
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

// --- GET /user ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UserData {
    pub user: UserWrap,
}

#[derive(Debug, Deserialize)]
pub struct UserWrap {
    pub datas: UserDatas,
    /// Notifications de l'hébergeur (incidents, crédits, changements d'offre…). Elles arrivent
    /// avec le profil : aucun endpoint dédié n'est accessible avec une clé du panel.
    #[serde(default)]
    pub notifications: Vec<UserNotificationRaw>,
}
// `GET /user` porte aussi `global_message`/`global_alert` (bandeaux promo de l'hébergeur) :
// volontairement non parsés — DESIGN.md §7 rappelle que ce sont des campagnes temporaires et
// qu'il ne faut pas les câbler. À reprendre le jour où l'on décide d'afficher ces bandeaux.

/// Notification brute. L'API sérialise **tout en chaînes**, y compris `id`, `viewed` et `readed`
/// (`"0"`/`"1"`) : on passe par `Value` pour accepter aussi bien `"0"` que `0`.
#[derive(Debug, Deserialize)]
pub struct UserNotificationRaw {
    #[serde(default)]
    pub id: serde_json::Value,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub readed: serde_json::Value,
}

/// Notification exposée aux frontends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserNotification {
    pub id: String,
    /// Catégorie donnée par l'hébergeur (`Information`, `Erreur`, `StratorAI`…).
    pub kind: String,
    /// Sévérité au vocabulaire du panel : `error` | `warning` | `success` | `primary`.
    pub color: String,
    pub message: String,
    pub date: String,
    pub read: bool,
}

impl From<UserNotificationRaw> for UserNotification {
    fn from(n: UserNotificationRaw) -> Self {
        UserNotification {
            id: flat_string(&n.id),
            kind: n.kind,
            color: n.color,
            message: n.message,
            date: n.date,
            read: truthy(&n.readed),
        }
    }
}

/// Rend une valeur JSON scalaire en texte, qu'elle soit chaîne ou nombre.
fn flat_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Vrai pour `1`, `"1"`, `true` — l'API mélange les trois selon les champs.
fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        serde_json::Value::String(s) => s != "0" && !s.is_empty(),
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
pub struct UserDatas {
    pub id: i64,
    pub pseudo: String,
    pub mail: String,
    #[serde(default)]
    pub money: f64,
    #[serde(default)]
    pub active_mybox_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserProfile {
    pub id: i64,
    pub pseudo: String,
    pub mail: String,
    pub credits: f64,
    pub mybox_count: i64,
}

impl From<UserDatas> for UserProfile {
    fn from(d: UserDatas) -> Self {
        UserProfile {
            id: d.id,
            pseudo: d.pseudo,
            mail: d.mail,
            credits: d.money,
            mybox_count: d.active_mybox_count.unwrap_or(0),
        }
    }
}

// --- GET /user/{id}/servers ----------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ServersData {
    #[serde(default)]
    servers_groups: std::collections::HashMap<String, MyBoxGroupRaw>,
    #[serde(default)]
    servers: Vec<ServerRaw>,
}

#[derive(Debug, Deserialize)]
struct MyBoxGroupRaw {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    owner: i64,
    #[serde(default)]
    offer: String,
    #[serde(default)]
    resources: ResourcesRaw,
    #[serde(default)]
    tend_days: i64,
    #[serde(default)]
    is_expired: i64,
    #[serde(default)]
    is_suspended: i64,
    #[serde(default)]
    is_pro: i64,
    /// Offre d'essai (période limitée) — distincte de `is_free`.
    #[serde(default)]
    is_trial: i64,
    /// Ressources bonus offertes sur la MyBox (promo, parrainage…).
    #[serde(default)]
    has_bonus: i64,
    /// Date d'expiration (`YYYY-MM-DD HH:MM:SS`) — `tend_days` n'en donne que le nombre de jours.
    #[serde(default)]
    tend: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResourcesRaw {
    #[serde(default)]
    cpu: i64,
    #[serde(default)]
    ram: i64,
    #[serde(default)]
    disk: i64,
}

#[derive(Debug, Deserialize)]
struct ServerRaw {
    id: i64,
    #[serde(default)]
    name: String,
    id_mybox: i64,
    #[serde(default)]
    egg_name: String,
    #[serde(default)]
    egg_icon: Option<String>,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    port: i64,
    #[serde(default)]
    dns: Option<String>,
    #[serde(default)]
    hibernation: i64,
    #[serde(default)]
    is_disabled: i64,
    #[serde(default)]
    is_suspended: i64,
    #[serde(default)]
    is_expired: i64,
    #[serde(default)]
    is_bedrock: i64,
    #[serde(default)]
    owner: i64,
    /// Identifiant d'egg **exact** (catalogue `/site/eggs`) : bien plus fiable que `egg_name`
    /// pour reconnaître le jeu. Voir [`crate::games`].
    #[serde(default)]
    id_egg: Option<i64>,
    /// Malgré son nom, ce n'est pas une date mais l'**URL signée de téléchargement** du dernier
    /// backup (`https://flash.minestrator.com/get/<jeton chiffré>`, vérifié le 2026-08-19).
    /// Jeton porteur : ne pas l'afficher en clair ni le journaliser.
    #[serde(default)]
    last_backup: Option<String>,
    /// Avertissements configurés par l'hébergeur, à afficher AVANT l'action correspondante
    /// (chaîne vide = rien à afficher).
    #[serde(default)]
    poweraction_message_restart: Option<String>,
    #[serde(default)]
    poweraction_message_stop: Option<String>,
    /// Malgré son nom, ce champ ne porte pas une note libre mais une **clé de traduction du
    /// panel** expliquant l'indisponibilité (vérifié le 2026-08-19 : tous les serveurs suspendus
    /// du compte de test renvoient `PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`, les actifs
    /// `null` ou `""`). À traduire côté frontend, jamais à afficher brut.
    #[serde(default)]
    note: Option<String>,
    /// Fin d'abonnement du serveur (`YYYY-MM-DD HH:MM:SS`).
    #[serde(default)]
    tend: Option<String>,
    /// Référence courte à donner au support (ex. `P75E4`).
    #[serde(default)]
    hashsupport: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServersOverview {
    pub myboxes: Vec<MyBoxSummary>,
    pub servers: Vec<ServerListItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MyBoxSummary {
    pub id: i64,
    pub name: String,
    pub offer: String,
    pub owner: bool,
    pub cpu: i64,
    pub ram: i64,
    pub disk: i64,
    pub days_left: i64,
    pub expired: bool,
    pub suspended: bool,
    pub pro: bool,
    pub trial: bool,
    pub bonus: bool,
    /// Date d'expiration brute, pour un affichage exact (`days_left` reste l'arrondi).
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerListItem {
    pub id: i64,
    pub name: String,
    pub mybox_id: i64,
    pub egg_name: String,
    /// Nom de fichier brut renvoyé par l'API (`minecraft.webp`) — conservé pour le debug.
    pub egg_icon: Option<String>,
    /// URL CDN complète de la vignette du jeu, prête à poser dans un `<img>`.
    pub egg_icon_url: Option<String>,
    /// URL CDN de la bannière large du jeu (~200 Kio : ne la charger que si elle est affichée).
    pub egg_header_url: Option<String>,
    pub address: String,
    pub status: String,
    pub owner: bool,
    pub bedrock: bool,
    /// Identifiant d'egg exact (catalogue `/site/eggs`), quand l'API le fournit.
    pub egg_id: Option<i64>,
    /// URL signée de téléchargement du dernier backup automatique (`None` si aucun).
    pub last_backup_url: Option<String>,
    /// Avertissement à afficher avant un redémarrage (`None` si l'hébergeur n'en a pas défini).
    pub restart_warning: Option<String>,
    /// Avertissement à afficher avant un arrêt.
    pub stop_warning: Option<String>,
    /// Raison d'indisponibilité, sous forme de **clé i18n du panel**
    /// (`PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`). Vide pour un serveur sain.
    pub unavailable_reason: Option<String>,
    /// Fin d'abonnement du serveur.
    pub expires_at: Option<String>,
    /// Référence courte à donner au support.
    pub support_ref: Option<String>,
    /// Capacités dérivées du jeu (quelles vues/fonctions exposer). Voir [`crate::games`].
    pub capabilities: crate::games::GameCapabilities,
}

impl From<ServersData> for ServersOverview {
    fn from(raw: ServersData) -> Self {
        let mut myboxes: Vec<MyBoxSummary> = raw
            .servers_groups
            .into_values()
            .map(|m| MyBoxSummary {
                id: m.id,
                name: m.name,
                offer: m.offer,
                owner: m.owner != 0,
                cpu: m.resources.cpu,
                ram: m.resources.ram,
                disk: m.resources.disk,
                days_left: m.tend_days,
                expired: m.is_expired != 0,
                suspended: m.is_suspended != 0,
                pro: m.is_pro != 0,
                trial: m.is_trial != 0,
                bonus: m.has_bonus != 0,
                expires_at: m.tend,
            })
            .collect();
        myboxes.sort_by_cached_key(|m| m.name.to_lowercase());

        let mut servers: Vec<ServerListItem> = raw
            .servers
            .into_iter()
            .map(|s| ServerListItem {
                status: server_status(&s).to_string(),
                address: server_address(&s),
                capabilities: crate::games::capabilities_for_egg(
                    s.id_egg,
                    &s.egg_name,
                    s.is_bedrock != 0,
                ),
                egg_icon_url: egg_asset_url(s.egg_icon.as_deref(), crate::config::EGG_ICON_BASE),
                egg_header_url: egg_asset_url(
                    s.egg_icon.as_deref(),
                    crate::config::EGG_HEADER_BASE,
                ),
                id: s.id,
                name: s.name,
                mybox_id: s.id_mybox,
                egg_name: s.egg_name,
                egg_icon: s.egg_icon,
                owner: s.owner != 0,
                bedrock: s.is_bedrock != 0,
                egg_id: s.id_egg,
                last_backup_url: s.last_backup,
                restart_warning: non_empty(s.poweraction_message_restart),
                stop_warning: non_empty(s.poweraction_message_stop),
                unavailable_reason: non_empty(s.note),
                expires_at: s.tend,
                support_ref: non_empty(s.hashsupport),
            })
            .collect();
        servers.sort_by_cached_key(|s| s.name.to_lowercase());

        ServersOverview { myboxes, servers }
    }
}

fn server_status(s: &ServerRaw) -> &'static str {
    if s.is_expired != 0 {
        "expired"
    } else if s.is_suspended != 0 {
        "suspended"
    } else if s.is_disabled != 0 {
        "disabled"
    } else if s.hibernation != 0 {
        "hibernation"
    } else {
        "active"
    }
}

fn server_address(s: &ServerRaw) -> String {
    match &s.dns {
        Some(dns) if !dns.is_empty() => dns.clone(),
        _ => format!("{}:{}", s.ip, s.port),
    }
}

/// L'API renvoie beaucoup de chaînes vides là où « pas de valeur » serait plus juste
/// (notes, avertissements de power action) : on les ramène à `None`.
fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// Complète un nom de fichier d'egg (`minecraft.webp`) en URL CDN. Une valeur déjà absolue est
/// laissée telle quelle : si l'API se met un jour à renvoyer des URLs, rien ne casse.
fn egg_asset_url(icon: Option<&str>, base: &str) -> Option<String> {
    let icon = icon.map(str::trim).filter(|s| !s.is_empty())?;
    if icon.starts_with("http://") || icon.starts_with("https://") {
        return Some(icon.to_string());
    }
    Some(format!("{base}{icon}"))
}

// --- GET /server/{id}/websocket -------------------------------------------

/// Identifiants WebSocket seuls. `GET /server/{id}/websocket` renvoie ~1 Kio là où
/// `GET /server/{id}` en renvoie plusieurs (SFTP, historique, ports…) : à préférer pour le
/// rafraîchissement de token, qui arrive plusieurs fois par heure et par connexion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WsCredentials {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsCredentialsData {
    #[serde(default)]
    websocket: WsCredentials,
}

impl From<WsCredentialsData> for WsCredentials {
    fn from(d: WsCredentialsData) -> Self {
        d.websocket
    }
}

// --- POST /servers/live/light/bulk ----------------------------------------

/// Compte de joueurs de plusieurs serveurs en **un seul appel**. Attention : contrairement à
/// `GET /server/{id}/live/light`, la réponse ne porte QUE les joueurs — ni CPU, ni disque, ni
/// statut. C'est une vue de liste, pas un remplaçant du `live_light` complet.
#[derive(Debug, Clone, Serialize)]
pub struct LivePlayers {
    pub server_id: i64,
    pub current: i64,
    pub limit: i64,
}

#[derive(Debug, Deserialize)]
pub struct LiveBulkData {
    /// Valeur `null` observée pour un serveur que l'hôte ne sait pas interroger sur le moment
    /// (vérifié le 2026-08-19) : sans l'`Option`, toute la réponse échouerait à se désérialiser.
    #[serde(default)]
    pub servers: std::collections::HashMap<String, Option<LiveBulkEntry>>,
}

#[derive(Debug, Deserialize)]
pub struct LiveBulkEntry {
    #[serde(default)]
    pub players: Option<PlayersRaw>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlayersRaw {
    #[serde(default)]
    pub current: i64,
    #[serde(default)]
    pub limit: i64,
}

// --- GET /site/eggs -------------------------------------------------------

/// Entrée du catalogue d'eggs de l'hébergeur (37 au 2026-08-19) : la table de référence qui
/// relie un `id_egg` à un jeu, sans passer par des mots-clés dans un nom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Egg {
    pub id: i64,
    pub name: String,
    pub icon_url: Option<String>,
    pub header_url: Option<String>,
    /// Ressources minimales recommandées (cpu en cœurs, ram/disk en Go).
    pub cpu: String,
    pub ram: String,
    pub disk: String,
    pub official: bool,
}

#[derive(Debug, Deserialize)]
pub struct EggsData {
    #[serde(default)]
    pub eggs: Vec<EggRaw>,
}

#[derive(Debug, Deserialize)]
pub struct EggRaw {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub resources: EggResourcesRaw,
    #[serde(default)]
    pub is_official: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct EggResourcesRaw {
    #[serde(default)]
    pub cpu: String,
    #[serde(default)]
    pub ram: String,
    #[serde(default)]
    pub disk: String,
}

impl From<EggRaw> for Egg {
    fn from(e: EggRaw) -> Self {
        Egg {
            icon_url: egg_asset_url(e.icon.as_deref(), crate::config::EGG_ICON_BASE),
            header_url: egg_asset_url(e.icon.as_deref(), crate::config::EGG_HEADER_BASE),
            id: e.id,
            name: e.name,
            cpu: e.resources.cpu,
            ram: e.resources.ram,
            disk: e.resources.disk,
            official: e.is_official != 0,
        }
    }
}

// --- GET /server/{id} -----------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ServerFullData {
    server: ServerObjRaw,
    #[serde(default)]
    websocket: WsRaw,
    /// Journal d'actions tenu par l'hébergeur — la brique que le ROADMAP appelait « journal
    /// d'audit », déjà servie ici : elle couvre AUSSI les actions faites hors de l'app
    /// (panel web, staff), qu'un journal purement local ne verrait jamais.
    #[serde(default)]
    history: Vec<ServerActionRaw>,
}

#[derive(Debug, Deserialize)]
struct ServerActionRaw {
    #[serde(default)]
    date: String,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    is_staff: bool,
    #[serde(default)]
    action: String,
}

/// Entrée du journal d'actions d'un serveur.
#[derive(Debug, Clone, Serialize)]
pub struct ServerAction {
    /// `YYYY-MM-DD HH:MM:SS.mmm` tel que renvoyé par l'API.
    pub date: String,
    /// Adresse d'origine de l'action. Utile pour repérer une action qu'on n'a pas faite ;
    /// passe par [`crate::redact`] avant tout envoi à un LLM ou un service de paste.
    pub ip: String,
    /// Action déclenchée par le staff de l'hébergeur plutôt que par le titulaire du compte.
    pub staff: bool,
    /// Identifiant d'action (`power.restart`, `power.stop`…).
    pub action: String,
}

#[derive(Debug, Deserialize)]
struct ServerObjRaw {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    hibernation: i64,
}

#[derive(Debug, Default, Deserialize)]
struct WsRaw {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerDetails {
    pub id: i64,
    pub name: String,
    pub hibernation: bool,
    pub ws_url: Option<String>,
    pub ws_token: Option<String>,
    /// Journal d'actions, du plus récent au plus ancien.
    pub history: Vec<ServerAction>,
}

impl From<ServerFullData> for ServerDetails {
    fn from(r: ServerFullData) -> Self {
        ServerDetails {
            id: r.server.id,
            name: r.server.name,
            hibernation: r.server.hibernation != 0,
            ws_url: r.websocket.url.filter(|u| !u.is_empty()),
            ws_token: r.websocket.token.filter(|t| !t.is_empty()),
            history: r
                .history
                .into_iter()
                .map(|h| ServerAction {
                    date: h.date,
                    ip: h.ip,
                    staff: h.is_staff,
                    action: h.action,
                })
                .collect(),
        }
    }
}

// --- Démarrage / paramètres JVM (GET /server/{id} · settings ; PATCH .../startup/params) ---

#[derive(Debug, Deserialize)]
pub struct StartupData {
    #[serde(default)]
    pub settings: StartupSettings,
}

#[derive(Debug, Default, Deserialize)]
pub struct StartupSettings {
    #[serde(default)]
    pub startup: String,
    #[serde(default)]
    pub startup_file: String,
    #[serde(default)]
    pub java_memory: i64,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub id_java_version: i64,
}

/// Configuration de démarrage d'un serveur (commande Java + contexte).
#[derive(Debug, Clone, Serialize)]
pub struct Startup {
    /// Commande de lancement complète (contient `{{SERVER_JARFILE}}`).
    pub command: String,
    /// Fichier JAR de démarrage (résout `{{SERVER_JARFILE}}`).
    pub jar_file: String,
    /// Mémoire Java allouée (Go).
    pub java_memory: i64,
    /// Image Docker d'exécution.
    pub image: String,
    /// Identifiant de version Java.
    pub java_version: i64,
}

impl From<StartupData> for Startup {
    fn from(d: StartupData) -> Self {
        Startup {
            command: d.settings.startup,
            jar_file: d.settings.startup_file,
            java_memory: d.settings.java_memory,
            image: d.settings.image,
            java_version: d.settings.id_java_version,
        }
    }
}

// --- GET /server/{id}/live/light ------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveLight {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub cpu: CpuLimits,
    #[serde(default)]
    pub memory: LimitMb,
    #[serde(default)]
    pub disk: LimitMb,
    #[serde(default)]
    pub players: Option<Players>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub motd: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuLimits {
    #[serde(default)]
    pub dedicated: i64,
    #[serde(default)]
    pub flexcore: i64,
    #[serde(default)]
    pub limit: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LimitMb {
    #[serde(default)]
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Players {
    #[serde(default)]
    pub current: i64,
    #[serde(default)]
    pub limit: i64,
    #[serde(default)]
    pub list: Vec<String>,
}

// --- GET /server/{id}/console/logs ----------------------------------------

#[derive(Debug, Deserialize)]
pub struct ConsoleLogs {
    #[serde(default)]
    pub logs: Vec<String>,
}

// --- SFTP -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SftpData {
    #[serde(default)]
    pub sftp: Option<SftpCreds>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SftpCreds {
    pub host: String,
    #[serde(default = "default_sftp_port")]
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub password: String,
}

fn default_sftp_port() -> u16 {
    2022
}

#[derive(Debug, Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>,
}

// --- Marketplace (mods & plugins : Modrinth / CurseForge / SpigotMC) -------
//
// L'API expose des catalogues normalisés. Les items « mods » et « plugins » partagent
// presque le même schéma ; les rares divergences sont absorbées par des `alias` serde
// (`testedVersions` → `game_versions`) et des `default`. L'`id` est tantôt une chaîne
// (Modrinth) tantôt un nombre (CurseForge/SpigotMC) → capturé en `Value` puis normalisé.

/// Convertit un `id` JSON (chaîne **ou** nombre) en chaîne.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[derive(Debug, Deserialize)]
pub struct McVersionsData {
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct IconRaw {
    #[serde(default)]
    url: String,
    /// SpigotMC fournit l'icône en base64 (utilisable en `data:` — pratique hors-ligne).
    #[serde(default)]
    data: String,
}

/// Item de catalogue tel que renvoyé par l'API (clé `mods` OU `plugins`).
#[derive(Debug, Deserialize)]
pub struct MarketListRaw {
    #[serde(default, alias = "plugins")]
    pub mods: Vec<MarketItemRaw>,
    #[serde(default)]
    pub total_hits: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MarketItemRaw {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    downloads: i64,
    #[serde(default)]
    icon: IconRaw,
    #[serde(default, alias = "testedVersions")]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    source: String,
    #[serde(default)]
    premium: bool,
    /// SpigotMC : téléchargement externe (le jar n'est pas hébergé sur Spigot) → non installable.
    #[serde(default)]
    external: bool,
    #[serde(default)]
    curseforge_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketItem {
    /// Identifiant natif de la source (slug Modrinth via `slug`, id numérique CF/Spigot).
    pub id: String,
    pub slug: String,
    pub name: String,
    pub tag: String,
    pub downloads: i64,
    pub icon_url: Option<String>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub source: String,
    /// Payant/restreint → non installable via MineStrator (l'API renvoie 403).
    pub premium: bool,
    /// Téléchargement externe (SpigotMC) → non installable non plus.
    pub external: bool,
    pub external_url: Option<String>,
}

impl From<MarketItemRaw> for MarketItem {
    fn from(r: MarketItemRaw) -> Self {
        // Icône : URL http(s) directe si présente, sinon base64 SpigotMC en data URI.
        let icon_url = if r.icon.url.starts_with("http") {
            Some(r.icon.url)
        } else if !r.icon.data.is_empty() {
            Some(format!("data:image/png;base64,{}", r.icon.data))
        } else {
            None
        };
        MarketItem {
            id: value_to_string(&r.id),
            slug: r.slug,
            name: r.name,
            tag: r.tag,
            downloads: r.downloads,
            icon_url,
            game_versions: r.game_versions,
            loaders: r.loaders,
            source: r.source,
            premium: r.premium,
            external: r.external,
            external_url: r.curseforge_url,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketPage {
    pub items: Vec<MarketItem>,
    pub total_hits: Option<i64>,
}

impl From<MarketListRaw> for MarketPage {
    fn from(r: MarketListRaw) -> Self {
        MarketPage {
            items: r.mods.into_iter().map(MarketItem::from).collect(),
            total_hits: r.total_hits,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MarketVersionsRaw {
    #[serde(default)]
    pub versions: Vec<MarketVersionRaw>,
}

#[derive(Debug, Deserialize)]
pub struct MarketVersionRaw {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    release_type: String,
    #[serde(default)]
    downloads: Option<i64>,
    #[serde(default)]
    date_published: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketVersion {
    pub id: String,
    pub version_number: String,
    pub name: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub release_type: String,
    pub downloads: Option<i64>,
    pub date_published: String,
    pub filename: Option<String>,
    pub file_size: Option<i64>,
}

impl From<MarketVersionRaw> for MarketVersion {
    fn from(r: MarketVersionRaw) -> Self {
        MarketVersion {
            id: value_to_string(&r.id),
            version_number: r.version_number,
            name: r.name,
            game_versions: r.game_versions,
            loaders: r.loaders,
            release_type: r.release_type,
            downloads: r.downloads,
            date_published: r.date_published,
            filename: r.filename,
            file_size: r.file_size,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct InstalledRaw {
    #[serde(default, alias = "plugins")]
    pub mods: Vec<InstalledItemRaw>,
}

#[derive(Debug, Deserialize)]
pub struct InstalledItemRaw {
    #[serde(default)]
    name: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    version: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    loader: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledItem {
    pub name: String,
    pub filename: String,
    pub version: String,
    pub enabled: bool,
    pub loader: String,
}

impl From<InstalledItemRaw> for InstalledItem {
    fn from(r: InstalledItemRaw) -> Self {
        InstalledItem {
            name: r.name,
            filename: r.filename,
            version: r.version,
            enabled: r.enabled,
            loader: r.loader,
        }
    }
}

/// Un backup **quotidien automatique** d'un serveur (`GET /server/{id}/backups`, récents d'abord).
/// On ne peut que les lister et les restaurer (pas de création à la demande côté API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: i64,
    /// Taille en octets.
    pub size: i64,
    /// Horodatage « YYYY-MM-DD HH:MM:SS ».
    pub date: String,
}

#[derive(Debug, Deserialize)]
pub struct BackupsData {
    #[serde(default)]
    pub backups: Vec<Backup>,
}

/// Un **snapshot** créé à la demande (`GET /user/{id}/snapshots`). Point de sauvegarde rapide —
/// le bon filet AVANT une correction risquée (il est plus rapide à produire qu'un backup).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    /// Type d'univers (« Minecraft »). Champ `type` de l'API, renommé (mot réservé).
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub date: String,
    /// État brut renvoyé par l'API (1 = prêt d'après les données observées).
    #[serde(default)]
    pub status: i64,
    #[serde(default)]
    pub is_legacy: bool,
    /// Présents pour une création EN COURS de traitement (permet le suivi par polling) ; absents
    /// pour un snapshot déjà prêt.
    #[serde(default)]
    pub progress: Option<i64>,
    #[serde(default)]
    pub job_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotsData {
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
}
