//! Analyse de performance via **Spark** : le cœur orchestre les commandes Spark
//! (health/tps/gc + profiler), lit la console, **télécharge et parse le rapport de profiler**
//! (protobuf `x-spark-sampler`), et assemble un contexte texte que le Copilote analysera.
//! Orchestration **côté code** (pas côté agent) : sûr et déterministe.
//!
//! Détection par **commande** (pas par le jar) : Spark peut être un **plugin** OU **intégré**
//! au serveur (Paper/Purpur/Folia récents), auquel cas il n'y a aucun jar dans `/plugins`.

use crate::events::{CopilotProgress, CoreEvent};
use crate::util::tail;
use crate::Core;
use prost::Message;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

const HEALTH_WAIT_S: u64 = 3;
const PROFILER_S: u64 = 30;
const CONSOLE_TAIL: usize = 90;
const PROFILER_TAIL: usize = 45;
/// Hôte de contenu Spark (bytebin) servant le rapport brut `application/x-spark-sampler`.
const SPARK_CONTENT_HOST: &str = "https://spark-usercontent.lucko.me";
const TOP_HOTSPOTS: usize = 15;

/// Client HTTP partagé pour récupérer les rapports Spark (pool de connexions + TLS réutilisés) :
/// un `reqwest::Client` est conçu pour être partagé, pas recréé à chaque appel.
static SPARK_HTTP: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(crate::http::client);

/// Collecte un rapport de performance Spark et le renvoie en texte pour le Copilote.
/// Émet des étapes de progression (`id` = celui de l'analyse).
pub(crate) async fn collect_spark(
    core: &Core,
    id: &str,
    server_id: i64,
    with_profiler: bool,
) -> String {
    progress(core, id, "Collecte des rapports Spark…");
    for cmd in ["spark health", "spark tps", "spark gc"] {
        let _ = core.send_command(server_id, cmd).await;
    }
    tokio::time::sleep(Duration::from_secs(HEALTH_WAIT_S)).await;
    let logs = core.console_logs(server_id).await.unwrap_or_default();
    let report = tail(&logs, CONSOLE_TAIL);

    if spark_unavailable(&report) {
        return format!(
            "=== PERFORMANCE ===\n\
             Spark ne répond pas sur ce serveur : la console renvoie « Unknown command » à `spark health`. \
             Sur Paper/Purpur/Folia récents, Spark est INTÉGRÉ (commande `spark`) — vérifie qu'il est activé ; \
             sinon installe le plugin (https://spark.lucko.me), puis relance l'analyse.\n\n\
             [extrait console]\n```\n{}\n```",
            tail(&logs, 25)
        );
    }

    let mut out = String::from("=== RAPPORT DE PERFORMANCE (Spark) ===\n");
    out.push_str("\n[health / tps / gc — extrait console]\n");
    out.push_str(&report);

    if with_profiler {
        progress(core, id, "Profiler Spark (30 s)…");
        let _ = core.send_command(server_id, "spark profiler start").await;
        tokio::time::sleep(Duration::from_secs(PROFILER_S)).await;
        let _ = core.send_command(server_id, "spark profiler stop").await;
        tokio::time::sleep(Duration::from_secs(3)).await;

        if let Ok(logs) = core.console_logs(server_id).await {
            match extract_spark_url(&logs) {
                Some(url) => {
                    let _ = writeln!(out, "\n\n[profiler {PROFILER_S}s] Rapport détaillé : {url}");
                    progress(core, id, "Analyse du profiler détaillé…");
                    if let Some(hot) = fetch_profiler_summary(&url).await {
                        out.push('\n');
                        out.push_str(&hot);
                    }
                }
                None => {
                    let _ = writeln!(
                        out,
                        "\n\n[profiler {PROFILER_S}s] (URL du rapport introuvable dans la console)"
                    );
                }
            }
            out.push_str("\n[console après profiler]\n");
            out.push_str(&tail(&logs, PROFILER_TAIL));
        }
    }

    out
}

fn progress(core: &Core, id: &str, phase: &str) {
    core.emit(CoreEvent::CopilotProgress(CopilotProgress {
        id: id.to_string(),
        phase: phase.to_string(),
    }));
}

/// Spark est-il indisponible ? Vrai seulement si « unknown command » ET aucun marqueur Spark.
fn spark_unavailable(console: &str) -> bool {
    let c = console.to_lowercase();
    let unknown = c.contains("unknown command") || c.contains("unknown or incomplete");
    let spark_ok = c.contains('⚡')
        || c.contains("mspt")
        || c.contains("spark v")
        || c.contains("tps from")
        || c.contains("tick durations")
        || c.contains("lucko.me");
    unknown && !spark_ok
}

fn extract_spark_url(logs: &[String]) -> Option<String> {
    const MARK: &str = "https://spark.lucko.me/";
    for line in logs.iter().rev() {
        if let Some(pos) = line.find(MARK) {
            let rest = &line[pos..];
            // Fin au premier blanc OU caractère de contrôle : les lignes Wings portent des codes
            // ANSI (`ESC[…m`) collés à l'URL, qui corrompraient la clé du rapport.
            let end = rest
                .find(|c: char| c.is_whitespace() || c.is_control())
                .unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

// --- Téléchargement + parsing du rapport de profiler (protobuf x-spark-sampler) -----------

/// Télécharge le rapport brut depuis bytebin et en extrait les points chauds (temps propre par
/// méthode). Renvoie `None` en cas d'échec (réseau, format inattendu) — dégradation gracieuse.
async fn fetch_profiler_summary(viewer_url: &str) -> Option<String> {
    let key = viewer_url.trim().trim_end_matches('/').rsplit('/').next()?;
    if key.is_empty() {
        return None;
    }
    let resp = SPARK_HTTP
        .get(format!("{SPARK_CONTENT_HOST}/{key}"))
        .header("User-Agent", crate::config::USER_AGENT)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let data = proto::SamplerData::decode(bytes).ok()?;
    summarize_profiler(&data)
}

/// Agrège le **temps propre** (self time) par méthode sur tous les threads, et renvoie le top N.
/// Les nœuds sont stockés à plat dans `ThreadNode.children` ; `children_refs` indexe ce tableau.
fn summarize_profiler(data: &proto::SamplerData) -> Option<String> {
    let mut agg: HashMap<(String, String), f64> = HashMap::new();
    let mut total = 0.0f64;

    for thread in &data.threads {
        let nodes = &thread.children;
        for node in nodes {
            let node_t: f64 = node.times.iter().sum();
            let child_t: f64 = node
                .children_refs
                .iter()
                .filter_map(|&r| usize::try_from(r).ok().and_then(|i| nodes.get(i)))
                .map(|c| c.times.iter().sum::<f64>())
                .sum();
            let self_t = (node_t - child_t).max(0.0);
            if self_t <= 0.0 || node.method_name.is_empty() {
                continue;
            }
            *agg.entry((node.class_name.clone(), node.method_name.clone()))
                .or_default() += self_t;
            total += self_t;
        }
    }

    if total <= 0.0 || agg.is_empty() {
        return None;
    }
    let mut ranked: Vec<((String, String), f64)> = agg.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out =
        String::from("[profiler — points chauds] Temps propre par méthode (% du total échantillonné) :\n");
    for ((class, method), t) in ranked.into_iter().take(TOP_HOTSPOTS) {
        let pct = t / total * 100.0;
        if pct < 0.5 {
            break;
        }
        let src = data
            .class_sources
            .get(&class)
            .map(|s| format!("  [{s}]"))
            .unwrap_or_default();
        let _ = writeln!(out, "- {pct:.1}%  {class}.{method}{src}");
    }
    Some(out)
}


/// Télécharge + parse un rapport Spark **existant** depuis son URL viewer (profiler OU
/// heapsummary), et renvoie un résumé texte. Sert d'outil à l'assistant (l'URL `spark.lucko.me`
/// est une app JS servant du protobuf binaire → illisible via un simple fetch web).
pub(crate) async fn parse_report_url(viewer_url: &str) -> Result<String, String> {
    let key = viewer_url
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "URL de rapport Spark invalide.".to_string())?;
    let resp = SPARK_HTTP
        .get(format!("{SPARK_CONTENT_HOST}/{key}"))
        .header("User-Agent", crate::config::USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("téléchargement du rapport Spark : {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("rapport Spark inaccessible (HTTP {}).", resp.status().as_u16()));
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.bytes().await.map_err(|e| format!("lecture du rapport : {e}"))?;

    // Le Content-Type distingue le type (`x-spark-sampler` vs `x-spark-heap`) ; à défaut on tente.
    if ctype.contains("heap") {
        proto::HeapData::decode(bytes)
            .ok()
            .and_then(|d| summarize_heap(&d))
            .ok_or_else(|| "Rapport heap illisible.".into())
    } else if ctype.contains("sampler") {
        proto::SamplerData::decode(bytes)
            .ok()
            .and_then(|d| summarize_profiler(&d))
            .ok_or_else(|| "Rapport profiler illisible.".into())
    } else {
        proto::SamplerData::decode(bytes.clone())
            .ok()
            .and_then(|d| summarize_profiler(&d))
            .or_else(|| proto::HeapData::decode(bytes).ok().and_then(|d| summarize_heap(&d)))
            .ok_or_else(|| {
                format!("Type de rapport Spark non reconnu (Content-Type: {ctype}).")
            })
    }
}

/// Résumé d'un heapsummary : les types qui consomment le plus de mémoire (taille retenue).
fn summarize_heap(data: &proto::HeapData) -> Option<String> {
    if data.entries.is_empty() {
        return None;
    }
    let mut entries: Vec<&proto::HeapEntry> =
        data.entries.iter().filter(|e| !e.r#type.is_empty()).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.size));
    let mut out =
        String::from("[heap — plus gros consommateurs mémoire] (taille retenue · instances) :\n");
    for e in entries.into_iter().take(TOP_HOTSPOTS) {
        let _ = writeln!(
            out,
            "- {}  ·  {} inst.  ·  {}",
            fmt_bytes(e.size),
            e.instances,
            e.r#type
        );
    }
    Some(out)
}

/// Formatage IEC (o/Kio/Mio/Gio).
fn fmt_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} Gio", n / (1024.0 * 1024.0 * 1024.0))
    } else if n >= 1024.0 * 1024.0 {
        format!("{:.0} Mio", n / (1024.0 * 1024.0))
    } else if n >= 1024.0 {
        format!("{:.0} Kio", n / 1024.0)
    } else {
        format!("{n:.0} o")
    }
}

/// Sous-ensemble du schéma `spark_sampler.proto` nécessaire aux points chauds (structs prost
/// à la main → pas de `protoc`). Les champs non déclarés sont ignorés au décodage.
mod proto {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SamplerData {
        #[prost(message, repeated, tag = "2")]
        pub threads: ::prost::alloc::vec::Vec<ThreadNode>,
        #[prost(map = "string, string", tag = "3")]
        pub class_sources: ::std::collections::HashMap<String, String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ThreadNode {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(message, repeated, tag = "3")]
        pub children: ::prost::alloc::vec::Vec<StackTraceNode>,
        #[prost(double, repeated, tag = "4")]
        pub times: ::prost::alloc::vec::Vec<f64>,
        #[prost(int32, repeated, tag = "5")]
        pub children_refs: ::prost::alloc::vec::Vec<i32>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct StackTraceNode {
        #[prost(string, tag = "3")]
        pub class_name: String,
        #[prost(string, tag = "4")]
        pub method_name: String,
        #[prost(double, repeated, tag = "8")]
        pub times: ::prost::alloc::vec::Vec<f64>,
        #[prost(int32, repeated, tag = "9")]
        pub children_refs: ::prost::alloc::vec::Vec<i32>,
    }

    /// Sous-ensemble de `spark_heap.proto` : entrées d'un heapsummary (type, instances, taille).
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct HeapData {
        #[prost(message, repeated, tag = "2")]
        pub entries: ::prost::alloc::vec::Vec<HeapEntry>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct HeapEntry {
        #[prost(int32, tag = "1")]
        pub order: i32,
        #[prost(int32, tag = "2")]
        pub instances: i32,
        #[prost(int64, tag = "3")]
        pub size: i64,
        #[prost(string, tag = "4")]
        pub r#type: String,
    }
}

#[cfg(test)]
mod tests {
    use super::proto::{HeapData, HeapEntry, SamplerData, StackTraceNode, ThreadNode};
    use super::*;

    fn node(class: &str, method: &str, times: &[f64], children: &[i32]) -> StackTraceNode {
        StackTraceNode {
            class_name: class.into(),
            method_name: method.into(),
            times: times.to_vec(),
            children_refs: children.to_vec(),
        }
    }

    fn thread(name: &str, nodes: Vec<StackTraceNode>) -> ThreadNode {
        ThreadNode {
            name: name.into(),
            children: nodes,
            times: Vec::new(),
            children_refs: vec![0],
        }
    }

    /// Arbre minimal : racine (100) → tickEntities (60, sur 2 fenêtres) + onTick (40) → sleep (10).
    /// Temps propres attendus : racine 0, tickEntities 60, onTick 30, sleep 10 (total 100).
    fn sample() -> SamplerData {
        SamplerData {
            threads: vec![thread(
                "Server thread",
                vec![
                    node("net.minecraft.server.MinecraftServer", "runServer", &[100.0], &[1, 2]),
                    node("net.minecraft.world.level.Level", "tickEntities", &[25.0, 35.0], &[]),
                    node("com.example.Plugin", "onTick", &[40.0], &[3]),
                    node("java.lang.Thread", "sleep", &[10.0], &[]),
                ],
            )],
            class_sources: HashMap::from([("com.example.Plugin".to_string(), "MonPlugin".to_string())]),
        }
    }

    fn hotspot_lines(out: &str) -> Vec<&str> {
        out.lines().filter(|l| l.starts_with("- ")).collect()
    }

    // --- Détection de Spark / URL du rapport ------------------------------------

    #[test]
    fn spark_unavailable_only_without_any_spark_marker() {
        assert!(spark_unavailable(
            "[12:00:00 INFO]: Unknown command. Type \"/help\" for help."
        ));
        assert!(spark_unavailable("Unknown or incomplete command, see below for error"));
        // Un marqueur Spark dans le même extrait l'emporte (ex. `spark gc` inconnu mais `tps` ok).
        assert!(!spark_unavailable(
            "Unknown command\n[⚡] TPS from last 5s, 10s, 1m, 5m, 15m: 20.0, 20.0, 20.0"
        ));
        assert!(!spark_unavailable("Unknown command\nTick durations (min/med/95%ile/max ms)"));
        assert!(!spark_unavailable("UNKNOWN COMMAND\nMSPT 12.5"));
        // Pas d'« unknown command » → Spark est présumé disponible.
        assert!(!spark_unavailable("[12:00:00 INFO]: Done (3.2s)!"));
        assert!(!spark_unavailable(""));
    }

    #[test]
    fn extract_spark_url_takes_the_most_recent_link() {
        let logs: Vec<String> = vec![
            "[⚡] https://spark.lucko.me/ANCIEN".into(),
            "[12:00:00 INFO]: autre ligne".into(),
            "[12:00:31 INFO]: [⚡] Profiler stopped & upload complete!".into(),
            "[12:00:31 INFO]: [⚡] https://spark.lucko.me/AbC123xyz (cliquer pour ouvrir)".into(),
            "[12:00:32 INFO]: ligne après".into(),
        ];
        assert_eq!(
            extract_spark_url(&logs).as_deref(),
            Some("https://spark.lucko.me/AbC123xyz")
        );
        // URL en fin de ligne (sans espace après).
        assert_eq!(
            extract_spark_url(&["voir https://spark.lucko.me/Zz9".to_string()]).as_deref(),
            Some("https://spark.lucko.me/Zz9")
        );
        assert_eq!(extract_spark_url(&[]), None);
        assert_eq!(
            extract_spark_url(&["https://spark-usercontent.lucko.me/x".to_string()]),
            None
        );
    }

    #[test]
    fn extract_spark_url_strips_trailing_ansi_codes() {
        let logs = vec!["\u{1b}[0;37m[⚡] https://spark.lucko.me/AbC123\u{1b}[m".to_string()];
        assert_eq!(extract_spark_url(&logs).as_deref(), Some("https://spark.lucko.me/AbC123"));
    }

    // --- Profiler : points chauds ---------------------------------------------------

    #[test]
    fn profiler_ranks_methods_by_self_time() {
        let out = summarize_profiler(&sample()).expect("résumé attendu");
        assert!(out.starts_with("[profiler — points chauds]"), "{out}");
        assert_eq!(
            hotspot_lines(&out),
            [
                // Les fenêtres de temps d'un nœud sont additionnées (25 + 35).
                "- 60.0%  net.minecraft.world.level.Level.tickEntities",
                // Temps propre = 40 − 10 (enfant) ; source du plugin annotée.
                "- 30.0%  com.example.Plugin.onTick  [MonPlugin]",
                "- 10.0%  java.lang.Thread.sleep",
            ]
        );
        // La racine (temps propre nul) n'apparaît pas.
        assert!(!out.contains("runServer"), "{out}");
    }

    #[test]
    fn profiler_aggregates_the_same_method_across_threads() {
        let mut data = sample();
        data.threads.push(thread(
            "Worker-1",
            vec![node("java.lang.Thread", "sleep", &[100.0], &[])],
        ));
        let out = summarize_profiler(&data).unwrap();
        // sleep = 10 + 100 sur un total de 200.
        assert_eq!(hotspot_lines(&out)[0], "- 55.0%  java.lang.Thread.sleep");
        assert_eq!(hotspot_lines(&out).len(), 3);
    }

    #[test]
    fn profiler_ignores_invalid_refs_unnamed_nodes_and_negative_self_time() {
        let data = SamplerData {
            threads: vec![thread(
                "t",
                vec![
                    // Références hors bornes / négatives ignorées → tout le temps est propre.
                    node("a.A", "run", &[50.0], &[-1, 99]),
                    // Méthode sans nom : écartée (et absente du total).
                    node("b.B", "", &[1000.0], &[]),
                    // Enfant plus coûteux que le parent (données incohérentes) → borné à 0, écarté.
                    node("c.C", "parent", &[10.0], &[3]),
                    node("d.D", "child", &[50.0], &[]),
                ],
            )],
            class_sources: HashMap::new(),
        };
        let out = summarize_profiler(&data).unwrap();
        // Ex æquo : l'ordre entre eux vient d'une HashMap (non déterministe) → comparaison triée.
        let mut lines = hotspot_lines(&out);
        lines.sort_unstable();
        assert_eq!(lines, ["- 50.0%  a.A.run", "- 50.0%  d.D.child"]);
    }

    #[test]
    fn profiler_stops_below_half_a_percent_and_caps_the_top() {
        let data = SamplerData {
            threads: vec![thread(
                "t",
                vec![node("big.Big", "hot", &[999.0], &[]), node("small.Small", "cold", &[1.0], &[])],
            )],
            class_sources: HashMap::new(),
        };
        let out = summarize_profiler(&data).unwrap();
        // 0,1 % < 0,5 % → coupé.
        assert_eq!(hotspot_lines(&out), ["- 99.9%  big.Big.hot"]);

        let many: Vec<StackTraceNode> = (0..20)
            .map(|i| node(&format!("p.C{i}"), "m", &[5.0], &[]))
            .collect();
        let data = SamplerData {
            threads: vec![thread("t", many)],
            class_sources: HashMap::new(),
        };
        let out = summarize_profiler(&data).unwrap();
        assert_eq!(hotspot_lines(&out).len(), TOP_HOTSPOTS);
    }

    #[test]
    fn profiler_without_samples_yields_nothing() {
        assert!(summarize_profiler(&SamplerData::default()).is_none());
        let only_zero = SamplerData {
            threads: vec![thread("t", vec![node("a.A", "idle", &[0.0], &[])])],
            class_sources: HashMap::new(),
        };
        assert!(summarize_profiler(&only_zero).is_none());
    }

    /// Le rapport arrive en protobuf binaire : on vérifie le décodage de bout en bout, y compris
    /// la tolérance aux champs que nos structs ne déclarent pas (métadonnées, etc.).
    #[test]
    fn decodes_sampler_protobuf_and_ignores_unknown_fields() {
        let bytes = sample().encode_to_vec();
        let decoded = SamplerData::decode(bytes.as_slice()).expect("décodage");
        assert_eq!(decoded, sample());
        assert_eq!(summarize_profiler(&decoded), summarize_profiler(&sample()));

        /// Même message avec, en plus, des champs inconnus de notre sous-schéma.
        #[derive(Clone, PartialEq, ::prost::Message)]
        struct SamplerDataComplet {
            #[prost(string, tag = "1")]
            metadata: String,
            #[prost(message, repeated, tag = "2")]
            threads: Vec<ThreadNode>,
            #[prost(map = "string, string", tag = "3")]
            class_sources: HashMap<String, String>,
            #[prost(uint64, tag = "7")]
            inconnu: u64,
        }
        let complet = SamplerDataComplet {
            metadata: "platform=Paper".into(),
            threads: sample().threads,
            class_sources: sample().class_sources,
            inconnu: 1234,
        };
        let decoded = SamplerData::decode(complet.encode_to_vec().as_slice()).expect("décodage");
        assert_eq!(decoded, sample());

        // Octets corrompus → erreur de décodage (pas de panique).
        assert!(SamplerData::decode(&[0xff, 0xff, 0xff][..]).is_err());
    }

    // --- Heap summary ---------------------------------------------------------------

    fn heap_entry(ty: &str, instances: i32, size: i64) -> HeapEntry {
        HeapEntry {
            order: 0,
            instances,
            size,
            r#type: ty.into(),
        }
    }

    #[test]
    fn heap_lists_biggest_types_first() {
        let data = HeapData {
            entries: vec![
                heap_entry("int[]", 5, 512),
                heap_entry("byte[]", 10, 1_610_612_736),
                heap_entry("", 1, 9_999_999_999),
                heap_entry("java.lang.String", 1000, 3 * 1024 * 1024),
            ],
        };
        // Aller-retour protobuf, puis résumé.
        let decoded = HeapData::decode(data.encode_to_vec().as_slice()).unwrap();
        let out = summarize_heap(&decoded).unwrap();
        assert!(out.starts_with("[heap — plus gros consommateurs mémoire]"), "{out}");
        assert_eq!(
            hotspot_lines(&out),
            [
                "- 1.5 Gio  ·  10 inst.  ·  byte[]",
                "- 3 Mio  ·  1000 inst.  ·  java.lang.String",
                "- 512 o  ·  5 inst.  ·  int[]",
            ]
        );
    }

    #[test]
    fn heap_caps_the_top_and_handles_empty_reports() {
        assert!(summarize_heap(&HeapData::default()).is_none());
        let data = HeapData {
            entries: (0..30).map(|i| heap_entry(&format!("T{i}"), 1, i)).collect(),
        };
        let out = summarize_heap(&data).unwrap();
        let lines = hotspot_lines(&out);
        assert_eq!(lines.len(), TOP_HOTSPOTS);
        assert!(lines[0].ends_with("T29"), "{}", lines[0]);
    }

    #[test]
    fn fmt_bytes_uses_iec_units() {
        assert_eq!(fmt_bytes(0), "0 o");
        assert_eq!(fmt_bytes(-5), "0 o");
        assert_eq!(fmt_bytes(1023), "1023 o");
        assert_eq!(fmt_bytes(1024), "1 Kio");
        assert_eq!(fmt_bytes(10 * 1024), "10 Kio");
        assert_eq!(fmt_bytes(1024 * 1024), "1 Mio");
        assert_eq!(fmt_bytes(700 * 1024 * 1024), "700 Mio");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.0 Gio");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "5.5 Gio");
    }
}
