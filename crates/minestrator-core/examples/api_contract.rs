//! Contrat de l'API MineStrator : vérifie, avec une clé réelle et **uniquement des lectures**
//! (`GET`), que les réponses ont toujours la forme dont l'app dépend. Lancé chaque jour par
//! `.github/workflows/api-watch.yml`.
//!
//! Deux niveaux :
//! 1. **Désérialisation réelle** : les méthodes de lecture de `Core`, avec les modèles de l'app.
//! 2. **Champs bruts** : présence des champs que l'app lit avec tolérance (`#[serde(default)]`).
//!    Leur disparition ne casse pas le niveau 1 mais casse une fonctionnalité — c'est ainsi que le
//!    mot de passe SFTP a disparu sans qu'aucune erreur ne le signale.
//!
//! Le rapport peut finir dans une issue publique : il ne contient **aucune donnée du compte**
//! (ni pseudo, ni nom, ni identifiant, ni adresse) — seulement des gabarits d'endpoints, des
//! libellés anonymes (« serveur 1 ») et des messages d'erreur expurgés de leurs valeurs.
//!
//! Variables : `MINESTRATOR_API_KEY` (obligatoire), `API_CONTRACT_REPORT` (fichier Markdown du
//! rapport, facultatif), `API_CONTRACT_MAX_SERVERS` (serveurs examinés, 3 par défaut).
//!
//! `cargo run -p minestrator-core --example api_contract`

use minestrator_core::{Core, Error};
use serde_json::Value;
use std::fmt::Write as _;
use std::future::Future;

/// Champs bruts indispensables, par endpoint : chemin dans l'enveloppe `{ api: { data } }`.
/// `[]` désigne chaque élément d'un tableau (vérifié sur le premier).
const RAW_FIELDS: &[(&str, &[&str])] = &[
    ("/user", &["api.data.user.datas.id", "api.data.user.datas.pseudo"]),
    (
        "/user/{id}/servers",
        &[
            "api.data.servers[].id",
            "api.data.servers[].name",
            "api.data.servers[].id_mybox",
            "api.data.servers[].egg_name",
            "api.data.servers[].ip",
            "api.data.servers[].port",
        ],
    ),
    (
        "/server/{id}",
        &["api.data.sftp.host", "api.data.sftp.port", "api.data.sftp.user"],
    ),
    (
        "/server/{id}/websocket",
        &["api.data.websocket.url", "api.data.websocket.token"],
    ),
    // Non documenté (panel web) : jetons WebSocket de tous les serveurs en un appel.
    (
        "/user/{id}/servers/websocket",
        &["api.data.servers[].id", "api.data.servers[].socket", "api.data.servers[].token"],
    ),
    // Non documenté (panel web) : tâches planifiées du panel.
    ("/server/{id}/schedules", &["api.data.schedules"]),
    // Non documenté (panel web) : mises à jour des mods, plugins et datapacks (`updates.rs`),
    // vérifié sur les serveurs Minecraft seulement. Les listes peuvent être vides : seule leur
    // présence est exigée (les champs des éléments sont lus avec tolérance).
    ("/server/{id}/minecraft_version", &["api.data.version"]),
    ("/server/{id}/plugins", &["api.data.plugins"]),
    ("/server/{id}/mods", &["api.data.mods"]),
    ("/server/{id}/datapacks", &["api.data.level_name", "api.data.datapacks"]),
    // Non documenté (panel web) : joueurs déjà venus, fiche et inventaire (`players.rs`). Les
    // champs par joueur sont vérifiés à part (un serveur sans joueur n'a pas de `players[0]`).
    (
        "/server/{id}/players/offline",
        &["api.data.total_all", "api.data.players", "api.data.online.available"],
    ),
    (
        "/server/{id}/players/{uuid}/profile",
        &["api.data.stats", "api.data.times", "api.data.state", "api.data.flags"],
    ),
    ("/server/{id}/players/{uuid}/inventory", &["api.data.inventory", "api.data.equipment"]),
    // Non documenté (panel web) : statistiques à la minute de l'hébergeur (historique croisé et
    // disponibilité, `history.rs`). La grille est complétée de zéros : un point existe toujours.
    (
        "/server/{id}/stats/{début}/{fin}",
        &[
            "api.data.stats[].date",
            "api.data.stats[].cpu",
            "api.data.stats[].ram",
            "api.data.stats[].disk",
            "api.data.stats[].players",
        ],
    ),
    // Non documentés (panel web) : espace disque (vue « Espace disque », outil `disk_usage`).
    (
        "/server/{id}/disk/usage",
        &[
            "api.data.analyzed_path",
            "api.data.total_size",
            "api.data.items[].path",
            "api.data.items[].name",
            "api.data.items[].type",
            "api.data.items[].size",
        ],
    ),
    ("/server/{id}/disk/total", &["api.data.total_bytes", "api.data.raw_bytes"]),
    ("/mybox/{id}/disk", &["api.data.disk_servers", "api.data.disk_servers_unit"]),
    // Non documenté (panel web) : suivi des compressions faites par le serveur.
    ("/server/{id}/files/compress/jobs", &["api.data.jobs"]),
    // Non documenté (panel web) : seule source du mot de passe SFTP pour une clé API.
    (
        "/server/{id}/sftp",
        &[
            "api.data.sftp.host",
            "api.data.sftp.port",
            "api.data.sftp.user",
            "api.data.sftp.password",
        ],
    ),
];

/// Champs dont la présence est seulement **signalée** (changement à connaître, pas une panne).
const INFO_FIELDS: &[(&str, &str, &str)] = &[(
    "/server/{id}",
    "api.data.sftp.password",
    "`GET /server/{id}` renvoie de nouveau le mot de passe SFTP (retiré fin 2026) : l'app le lit toujours via `/server/{id}/sftp`.",
)];

struct Report {
    checks: Vec<(String, Result<(), String>)>,
    infos: Vec<String>,
}

impl Report {
    fn check(&mut self, name: impl Into<String>, result: Result<(), String>) {
        let name = name.into();
        match &result {
            Ok(()) => eprintln!("ok      {name}"),
            Err(e) => eprintln!("ÉCART   {name} : {e}"),
        }
        self.checks.push((name, result));
    }

    async fn call<T, F: Future<Output = Result<T, Error>>>(&mut self, name: &str, f: F) -> Option<T> {
        match f.await {
            Ok(v) => {
                self.check(name, Ok(()));
                Some(v)
            }
            Err(e) => {
                self.check(name, Err(sanitize(&e.to_string())));
                None
            }
        }
    }

    fn failures(&self) -> usize {
        self.checks.iter().filter(|(_, r)| r.is_err()).count()
    }

    fn markdown(&self) -> String {
        let mut out = String::new();
        let failures = self.failures();
        let _ = writeln!(out, "## Contrat de l'API MineStrator\n");
        if failures == 0 {
            let _ = writeln!(out, "✅ Conforme : {} vérification(s).\n", self.checks.len());
        } else {
            let _ = writeln!(
                out,
                "❌ **{failures} écart(s)** sur {} vérification(s). Une fonctionnalité de l'app est probablement cassée.\n",
                self.checks.len()
            );
        }
        let _ = writeln!(out, "| Vérification | Résultat |\n|---|---|");
        for (name, result) in &self.checks {
            let cell = match result {
                Ok(()) => "✅".to_string(),
                Err(e) => format!("❌ {}", e.replace('|', "\\|")),
            };
            let _ = writeln!(out, "| `{name}` | {cell} |");
        }
        if !self.infos.is_empty() {
            let _ = writeln!(out, "\n### À savoir\n");
            for info in &self.infos {
                let _ = writeln!(out, "- {info}");
            }
        }
        out
    }
}

/// Retire d'un message d'erreur toute valeur entre guillemets (serde les cite parfois) et le
/// tronque : le rapport peut être publié.
fn sanitize(message: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in message.chars() {
        match c {
            '"' | '`' => {
                if !quoted {
                    out.push_str("«…»");
                }
                quoted = !quoted;
            }
            _ if !quoted => out.push(c),
            _ => {}
        }
    }
    out.chars().filter(|c| !c.is_ascii_digit()).take(200).collect()
}

/// Valeur au bout d'un chemin `a.b[].c` (le premier élément pour `[]`).
fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for part in path.split('.') {
        let (key, array) = match part.strip_suffix("[]") {
            Some(k) => (k, true),
            None => (part, false),
        };
        current = current.get(key)?;
        if array {
            current = current.as_array()?.first()?;
        }
    }
    (!current.is_null()).then_some(current)
}

async fn raw_get(http: &reqwest::Client, key: &str, path: &str) -> Result<Value, String> {
    let resp = http
        .get(format!("{}{path}", minestrator_core::API_BASE_URL))
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| sanitize(&e.to_string()))?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| sanitize(&e.to_string()))?;
    if !status.is_success() {
        let code = body.pointer("/api/error").and_then(Value::as_str).unwrap_or("?");
        return Err(format!("HTTP {} ({code})", status.as_u16()));
    }
    Ok(body)
}

async fn raw_checks(
    report: &mut Report,
    http: &reqwest::Client,
    key: &str,
    template: &str,
    path: &str,
    label: &str,
) {
    let fields: Vec<&str> = RAW_FIELDS
        .iter()
        .filter(|(t, _)| *t == template)
        .flat_map(|(_, f)| f.iter().copied())
        .collect();
    let infos: Vec<_> = INFO_FIELDS.iter().filter(|(t, _, _)| *t == template).collect();
    match raw_get(http, key, path).await {
        Ok(body) => {
            for field in fields {
                let ok = lookup(&body, field).is_some();
                report.check(
                    format!("{template} → {field}{label}"),
                    if ok { Ok(()) } else { Err("champ absent".into()) },
                );
            }
            for (_, field, text) in infos {
                let present = lookup(&body, field)
                    .is_some_and(|v| v.as_str().is_none_or(|s| !s.is_empty()));
                if present && !report.infos.iter().any(|i| i == text) {
                    report.infos.push(text.to_string());
                }
            }
        }
        Err(e) => report.check(format!("{template} (brut){label}"), Err(e)),
    }
}

/// Joueurs déjà venus (Minecraft Java) : liste, puis fiche et inventaire du plus récent. Seuls les
/// gabarits d'endpoints apparaissent dans le rapport, jamais un UUID ou un pseudo.
async fn player_checks(report: &mut Report, core: &Core, http: &reqwest::Client, key: &str, id: i64, n: usize) {
    use minestrator_core::{PlayerFilter, PlayerSort, PlayersQuery};
    let tag = format!(" (serveur {n})");
    let label = |what: &str| format!("{what}{tag}");
    let offline = format!("/server/{id}/players/offline?limit=1");
    raw_checks(report, http, key, "/server/{id}/players/offline", &offline, &tag).await;
    let query = PlayersQuery {
        search: String::new(),
        sort: PlayerSort::LastSeen,
        filter: PlayerFilter::All,
        offset: 0,
        limit: 1,
    };
    let Some(page) = report.call(&label("GET /server/{id}/players/offline"), core.known_players(id, query, true)).await
    else {
        return;
    };
    let Some(player) = page.players.first() else { return };
    // Lus avec tolérance : leur disparition viderait les colonnes « temps de jeu » et « dernière venue ».
    report.check(
        label("/server/{id}/players/offline → players[].mtime, players[].play_time"),
        if player.last_seen.is_some() && player.play_time_secs.is_some() {
            Ok(())
        } else {
            Err("champ absent".into())
        },
    );
    let uuid = player.uuid.clone();
    report.call(&label("GET /server/{id}/players/{uuid}/profile"), core.player_profile(id, &uuid)).await;
    let path = format!("/server/{id}/players/{uuid}/profile");
    raw_checks(report, http, key, "/server/{id}/players/{uuid}/profile", &path, &tag).await;
    report.call(&label("GET /server/{id}/players/{uuid}/inventory"), core.player_inventory(id, &uuid)).await;
    let path = format!("/server/{id}/players/{uuid}/inventory");
    raw_checks(report, http, key, "/server/{id}/players/{uuid}/inventory", &path, &tag).await;
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let key = match std::env::var("MINESTRATOR_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            eprintln!("MINESTRATOR_API_KEY absente");
            std::process::exit(2);
        }
    };
    let max_servers: usize = std::env::var("API_CONTRACT_MAX_SERVERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    // Secrets dans un dossier temporaire : jamais dans le trousseau de la machine.
    let dir = std::env::temp_dir().join(format!("minestrator-api-contract-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dossier temporaire");
    std::env::set_var("MINESTRATOR_DATA_DIR", &dir);
    std::env::set_var("MINESTRATOR_SECRETS_FILE", "1");

    let core = Core::new();
    let http = minestrator_core::http::client();
    let mut report = Report { checks: Vec::new(), infos: Vec::new() };

    let profile = report.call("GET /user (authentification)", core.authenticate_and_store(&key)).await;
    raw_checks(&mut report, &http, &key, "/user", "/user", "").await;

    if let Some(profile) = profile {
        let servers_path = format!("/user/{}/servers", profile.id);
        raw_checks(&mut report, &http, &key, "/user/{id}/servers", &servers_path, "").await;
        let ws_path = format!("/user/{}/servers/websocket", profile.id);
        raw_checks(&mut report, &http, &key, "/user/{id}/servers/websocket", &ws_path, "").await;
    }

    if let Some(overview) = report.call("GET /user/{id}/servers", core.list_servers()).await {
        for (i, mybox) in overview.myboxes.iter().enumerate() {
            let label = format!("GET /mybox/{{id}} (MyBox {})", i + 1);
            report.call(&label, core.mybox_details(mybox.id)).await;
            let tag = format!(" (MyBox {})", i + 1);
            raw_checks(&mut report, &http, &key, "/mybox/{id}/disk", &format!("/mybox/{}/disk", mybox.id), &tag).await;
        }
        let mut examined = 0;
        for server in &overview.servers {
            if examined >= max_servers {
                break;
            }
            // Un serveur désactivé ou suspendu (signalé dans la liste), ou fermé à ce compte, répond
            // 403 à tout : ce n'est pas un écart de contrat.
            if server.unavailable_reason.is_some() {
                continue;
            }
            match core.server_details(server.id).await {
                Err(Error::Forbidden) => continue,
                Err(Error::Api { code }) if code.starts_with("API_SERVER_") => continue,
                other => {
                    examined += 1;
                    let n = examined;
                    let label = |what: &str| format!("{what} (serveur {n})");
                    report.check(
                        label("GET /server/{id}"),
                        other.map(|_| ()).map_err(|e| sanitize(&e.to_string())),
                    );
                    let tag = format!(" (serveur {n})");
                    raw_checks(&mut report, &http, &key, "/server/{id}", &format!("/server/{}", server.id), &tag).await;
                    raw_checks(
                        &mut report,
                        &http,
                        &key,
                        "/server/{id}/sftp",
                        &format!("/server/{}/sftp", server.id),
                        &tag,
                    )
                    .await;
                    raw_checks(
                        &mut report,
                        &http,
                        &key,
                        "/server/{id}/websocket",
                        &format!("/server/{}/websocket", server.id),
                        &tag,
                    )
                    .await;
                    report.call(&label("GET /server/{id}/live/light"), core.live_light(server.id)).await;
                    report.call(&label("GET /server/{id}/schedules"), core.panel_schedules(server.id)).await;
                    raw_checks(
                        &mut report,
                        &http,
                        &key,
                        "/server/{id}/schedules",
                        &format!("/server/{}/schedules", server.id),
                        &tag,
                    )
                    .await;
                    if server.capabilities.player_data {
                        player_checks(&mut report, &core, &http, &key, server.id, n).await;
                    }
                    // Bornes en heure de Paris (UTC+1 ou +2) : une fenêtre d'une heure qui finit
                    // une heure avant « maintenant UTC » est toujours dans le passé à Paris.
                    let end = chrono::Utc::now().naive_utc() - chrono::Duration::hours(1);
                    let start = end - chrono::Duration::hours(1);
                    let fmt = |d: chrono::NaiveDateTime| d.format("%Y-%m-%dT%H:%M:00").to_string();
                    raw_checks(
                        &mut report,
                        &http,
                        &key,
                        "/server/{id}/stats/{début}/{fin}",
                        &format!("/server/{}/stats/{}/{}", server.id, fmt(start), fmt(end)),
                        &tag,
                    )
                    .await;
                    report
                        .call(&label("GET /server/{id}/stats (historique croisé)"), async {
                            let h = core.server_history(server.id, 3600, Some(60)).await?;
                            if h.host_ok {
                                Ok(())
                            } else {
                                Err(Error::Unexpected("statistiques de l'hébergeur illisibles".into()))
                            }
                        })
                        .await;
                    report.call(&label("GET /server/{id}/disk/usage"), core.disk_usage(server.id, "/", 5)).await;
                    report.call(&label("GET /server/{id}/disk/total (espace)"), core.disk_space(server.id)).await;
                    report
                        .call(&label("GET /server/{id}/files/compress/jobs"), core.compress_jobs(server.id))
                        .await;
                    for (template, path) in [
                        ("/server/{id}/disk/usage", format!("/server/{}/disk/usage?limit=5", server.id)),
                        ("/server/{id}/disk/total", format!("/server/{}/disk/total", server.id)),
                        (
                            "/server/{id}/files/compress/jobs",
                            format!("/server/{}/files/compress/jobs?limit=5", server.id),
                        ),
                    ] {
                        raw_checks(&mut report, &http, &key, template, &path, &tag).await;
                    }
                    report.call(&label("GET /server/{id}/live"), core.server_live(server.id)).await;
                    report.call(&label("GET /server/{id}/activity"), core.server_activity(server.id, 5, 0)).await;
                    report.call(&label("GET /server/{id}/backups"), core.list_backups(server.id)).await;
                    report.call(&label("GET /server/{id}/console/logs"), core.console_logs(server.id)).await;
                    report.call(&label("GET /server/{id} (démarrage)"), core.get_startup(server.id)).await;
                    report.call(&label("GET /server/{id} (accès SFTP)"), core.sftp_host_key(server.id)).await;
                    if server.capabilities.properties {
                        report.call(&label("GET /server/{id}/properties"), core.server_properties(server.id)).await;
                    }
                    if server.capabilities.mods == "minecraft" {
                        for kind in ["minecraft_version", "plugins", "mods", "datapacks"] {
                            raw_checks(
                                &mut report,
                                &http,
                                &key,
                                &format!("/server/{{id}}/{kind}"),
                                &format!("/server/{}/{kind}", server.id),
                                &tag,
                            )
                            .await;
                        }
                        report.call(&label("GET /server/{id}/plugins"), core.installed_plugins(server.id)).await;
                        report.call(&label("GET /server/{id}/mods"), core.installed_mods(server.id)).await;
                        report.call(&label("GET /server/{id}/datapacks"), core.installed_datapacks(server.id)).await;
                    }
                }
            }
        }
        if examined == 0 {
            report.check("au moins un serveur actif à examiner", Err("aucun serveur actif sur ce compte".into()));
        }
    }
    report.call("GET /user/{id}/snapshots", core.list_snapshots()).await;
    report.call("GET /user (notifications)", core.user_notifications()).await;
    report.call("GET /site/eggs", core.list_eggs()).await;

    let markdown = report.markdown();
    if let Ok(path) = std::env::var("API_CONTRACT_REPORT") {
        std::fs::write(path, &markdown).expect("écriture du rapport");
    }
    println!("{markdown}");
    let _ = std::fs::remove_dir_all(&dir);
    if report.failures() > 0 {
        std::process::exit(1);
    }
}
