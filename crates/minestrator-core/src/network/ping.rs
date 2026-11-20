//! **Ping d'un serveur du compte** (outil `ping_server`, option `ping` de `network_topology`).
//!
//! - **Java** : Server List Ping du protocole moderne (1.7+) — handshake (`next_state` = 1), requête
//!   de statut, réponse JSON (version, protocole, MOTD, joueurs), puis ping/pong pour la latence ;
//!   repli sur le ping legacy `0xFE 0x01` (format de la 1.6, compris par les serveurs plus anciens)
//!   si la réponse moderne est illisible.
//! - **Bedrock** (`bedrock=true`, Geyser ou serveur Bedrock) : ping RakNet non connecté (`0x01`) sur
//!   un port UDP du serveur, réponse `0x1c` (`MCPE;motd;protocole;version;joueurs;max;…`).
//!
//! **Jamais un hôte arbitraire** : seul un `server_id` du compte est accepté ; l'adresse vient de
//! l'API (`GET /server/{id}` : `server.ip`, `server.port`, `ports[]`) et un port donné doit être
//! l'un de ceux du serveur. Une injection dans le chat ne peut donc pas en faire un scanner. Délais
//! courts (3 s par connexion), tout en Rust (tokio).

use super::*;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

/// Connexion TCP au plus.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Réponse (statut, pong) au plus.
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// Taille maximale d'un paquet de statut (favicon compris).
const MAX_PACKET: usize = 512 * 1024;
/// Attente d'une réponse Bedrock par port.
const BEDROCK_TIMEOUT: Duration = Duration::from_millis(1500);
/// Ports UDP essayés au plus sans `port`.
const MAX_BEDROCK_PORTS: usize = 6;
/// Pings concurrents (`network_topology` avec `ping`).
const PING_CONCURRENCY: usize = 8;
/// Version de protocole annoncée au handshake (« inconnue », convention du ping de statut).
const PROTOCOL_ANY: i32 = -1;
/// Marqueur des paquets RakNet hors connexion.
const RAKNET_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

// --- Encodage (pur) -------------------------------------------------------------------------------

pub(crate) fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut u = v as u32;
    loop {
        if u & !0x7f == 0 {
            out.push(u as u8);
            return;
        }
        out.push(((u & 0x7f) | 0x80) as u8);
        u >>= 7;
    }
}

/// VarInt lu dans `buf` à partir de `pos` (avancé).
pub(crate) fn read_varint(buf: &[u8], pos: &mut usize) -> Option<i32> {
    let mut out: u32 = 0;
    for i in 0..5 {
        let b = *buf.get(*pos)?;
        *pos += 1;
        out |= u32::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(out as i32);
        }
    }
    None
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as i32);
    out.extend_from_slice(s.as_bytes());
}

/// Paquet encadré (longueur VarInt puis identifiant et corps).
fn frame(id: i32, body: &[u8]) -> Vec<u8> {
    let mut inner = Vec::with_capacity(body.len() + 2);
    write_varint(&mut inner, id);
    inner.extend_from_slice(body);
    let mut out = Vec::with_capacity(inner.len() + 3);
    write_varint(&mut out, inner.len() as i32);
    out.extend(inner);
    out
}

/// Handshake vers l'état « statut », suivi de la requête de statut.
pub(crate) fn status_request(host: &str, port: u16) -> Vec<u8> {
    let mut body = Vec::new();
    write_varint(&mut body, PROTOCOL_ANY);
    write_string(&mut body, host);
    body.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut body, 1);
    let mut out = frame(0x00, &body);
    out.extend(frame(0x00, &[]));
    out
}

/// Ping legacy (1.6) : `FE 01 FA`, canal `MC|PingHost`, protocole 74, hôte et port.
pub(crate) fn legacy_request(host: &str, port: u16) -> Vec<u8> {
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    let channel = "MC|PingHost";
    let host16 = utf16(host);
    let mut data = vec![74u8];
    data.extend((host.encode_utf16().count() as u16).to_be_bytes());
    data.extend(&host16);
    data.extend(i32::from(port).to_be_bytes());
    let mut out = vec![0xfe, 0x01, 0xfa];
    out.extend((channel.encode_utf16().count() as u16).to_be_bytes());
    out.extend(utf16(channel));
    out.extend((data.len() as u16).to_be_bytes());
    out.extend(data);
    out
}

/// Ping RakNet hors connexion : `0x01`, horodatage, marqueur, identifiant du client.
pub(crate) fn raknet_ping(time: i64, guid: i64) -> Vec<u8> {
    let mut out = vec![0x01];
    out.extend(time.to_be_bytes());
    out.extend(RAKNET_MAGIC);
    out.extend(guid.to_be_bytes());
    out
}

// --- Réponses (pur) --------------------------------------------------------------------------------

/// Statut d'un serveur Java.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct JavaStatus {
    pub version: Option<String>,
    pub protocol: Option<i64>,
    pub motd: String,
    pub online: Option<i64>,
    pub max: Option<i64>,
    pub latency_ms: Option<u64>,
    /// Réponse du ping legacy (0xFE).
    pub legacy: bool,
    /// Mods annoncés (Forge `forgeData` / `modinfo`).
    pub modded: Option<String>,
}

/// Texte brut d'un MOTD : composant de chat (texte, `extra`, `translate`) ou texte, codes `§x`
/// retirés, lignes et espaces normalisés.
pub(crate) fn motd_text(desc: &Value) -> String {
    fn walk(v: &Value, out: &mut String, depth: usize) {
        if depth > 32 {
            return;
        }
        match v {
            Value::String(s) => out.push_str(s),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out, depth + 1)),
            Value::Object(m) => {
                if let Some(t) = m.get("text") {
                    walk(t, out, depth + 1);
                } else if let Some(t) = m.get("translate").and_then(Value::as_str) {
                    out.push_str(t);
                }
                if let Some(e) = m.get("extra") {
                    walk(e, out, depth + 1);
                }
            }
            _ => {}
        }
    }
    let mut raw = String::new();
    walk(desc, &mut raw, 0);
    let stripped = strip_codes(&raw);
    stripped
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Retire les codes de mise en forme `§x` (et `§x§r…` des couleurs hexadécimales).
pub(crate) fn strip_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '§' {
            it.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Réponse JSON du ping moderne.
pub(crate) fn parse_status(v: &Value) -> JavaStatus {
    let modded = v
        .pointer("/forgeData/mods")
        .and_then(Value::as_array)
        .map(|m| format!("Forge ({} mods)", m.len()))
        .or_else(|| {
            v.pointer("/modinfo/type")
                .and_then(Value::as_str)
                .map(|t| format!("{t} (modinfo)"))
        });
    JavaStatus {
        version: v
            .pointer("/version/name")
            .and_then(Value::as_str)
            .map(|s| strip_codes(s).trim().to_string()),
        protocol: v.pointer("/version/protocol").and_then(Value::as_i64),
        motd: v.get("description").map(motd_text).unwrap_or_default(),
        online: v.pointer("/players/online").and_then(Value::as_i64),
        max: v.pointer("/players/max").and_then(Value::as_i64),
        latency_ms: None,
        legacy: false,
        modded,
    }
}

/// Réponse du ping legacy (texte UTF-16 décodé, sans l'en-tête `0xFF` et la longueur) : format
/// 1.6 (`§1\0protocole\0version\0motd\0joueurs\0max`) ou plus ancien (`motd§joueurs§max`).
pub(crate) fn parse_legacy(text: &str) -> Option<JavaStatus> {
    if let Some(rest) = text.strip_prefix("§1\0") {
        let p: Vec<&str> = rest.split('\0').collect();
        if p.len() < 5 {
            return None;
        }
        return Some(JavaStatus {
            protocol: p[0].trim().parse().ok(),
            version: Some(p[1].trim().to_string()).filter(|s| !s.is_empty()),
            motd: motd_text(&Value::String(p[2].to_string())),
            online: p[3].trim().parse().ok(),
            max: p[4].trim().parse().ok(),
            legacy: true,
            ..Default::default()
        });
    }
    let p: Vec<&str> = text.rsplitn(3, '§').collect();
    if p.len() != 3 {
        return None;
    }
    Some(JavaStatus {
        motd: motd_text(&Value::String(p[2].to_string())),
        online: p[1].trim().parse().ok(),
        max: p[0].trim().parse().ok(),
        legacy: true,
        ..Default::default()
    })
}

/// Statut d'un serveur Bedrock (ou de Geyser).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct BedrockStatus {
    pub edition: String,
    pub motd: String,
    pub protocol: Option<i64>,
    pub version: Option<String>,
    pub online: Option<i64>,
    pub max: Option<i64>,
    pub sub_motd: Option<String>,
    pub gamemode: Option<String>,
    pub latency_ms: Option<u64>,
}

/// Réponse RakNet `0x1c` (pong hors connexion).
pub(crate) fn parse_pong(buf: &[u8]) -> Option<BedrockStatus> {
    if buf.first() != Some(&0x1c) || buf.len() < 35 || buf[17..33] != RAKNET_MAGIC {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([buf[33], buf[34]]));
    let raw = buf.get(35..35 + len)?;
    let text = String::from_utf8_lossy(raw);
    let p: Vec<&str> = text.split(';').collect();
    if p.len() < 6 {
        return None;
    }
    let opt = |i: usize| {
        p.get(i)
            .map(|s| strip_codes(s).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Some(BedrockStatus {
        edition: p[0].to_string(),
        motd: strip_codes(p[1]).trim().to_string(),
        protocol: p[2].trim().parse().ok(),
        version: opt(3),
        online: p[4].trim().parse().ok(),
        max: p[5].trim().parse().ok(),
        sub_motd: opt(7),
        gamemode: opt(8),
        latency_ms: None,
    })
}

// --- Réseau ---------------------------------------------------------------------------------------------

/// Message clair d'une connexion impossible.
fn connect_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => {
            "connexion refusée : rien n'écoute sur ce port (serveur arrêté ou en démarrage ?)"
                .into()
        }
        std::io::ErrorKind::TimedOut => {
            "pas de réponse (délai dépassé) : serveur arrêté, port fermé ou filtré".into()
        }
        _ => format!("connexion impossible ({e})"),
    }
}

async fn timed<T>(
    d: Duration,
    what: &str,
    f: impl std::future::Future<Output = std::io::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(d, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{what} : {}", connect_error(&e))),
        Err(_) => Err(format!(
            "{what} : pas de réponse en {} s (serveur arrêté, port fermé ou filtré)",
            d.as_secs()
        )),
    }
}

/// Paquet encadré lu sur le flux.
async fn read_packet(s: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut len: u32 = 0;
    for i in 0..5 {
        let b = timed(IO_TIMEOUT, "lecture", s.read_u8()).await?;
        len |= u32::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            let n = len as usize;
            if n == 0 || n > MAX_PACKET {
                return Err(format!("paquet de {n} octets : réponse inattendue"));
            }
            let mut buf = vec![0u8; n];
            timed(IO_TIMEOUT, "lecture", s.read_exact(&mut buf)).await?;
            return Ok(buf);
        }
    }
    Err("longueur de paquet illisible : ce n'est pas un serveur Minecraft Java".into())
}

/// Ping moderne : statut puis latence (ping/pong).
async fn modern(ip: IpAddr, port: u16, host: &str) -> Result<JavaStatus, String> {
    let mut s = timed(CONNECT_TIMEOUT, "connexion", TcpStream::connect((ip, port))).await?;
    let _ = s.set_nodelay(true);
    let started = Instant::now();
    timed(
        IO_TIMEOUT,
        "envoi",
        s.write_all(&status_request(host, port)),
    )
    .await?;
    let pkt = read_packet(&mut s).await?;
    let first = started.elapsed();
    let mut pos = 0;
    if read_varint(&pkt, &mut pos) != Some(0x00) {
        return Err("réponse de statut inattendue".into());
    }
    let n = read_varint(&pkt, &mut pos).ok_or("réponse de statut illisible")? as usize;
    let json = pkt.get(pos..pos + n).ok_or("réponse de statut tronquée")?;
    let v: Value =
        serde_json::from_slice(json).map_err(|e| format!("statut JSON illisible ({e})"))?;
    let mut st = parse_status(&v);
    // Latence : aller-retour du ping (sinon celui de la requête de statut).
    let payload = 0x4d53_5452_4d49_4e41_i64;
    let pong = async {
        s.write_all(&frame(0x01, &payload.to_be_bytes())).await?;
        let t = Instant::now();
        let p = read_packet(&mut s).await.map_err(std::io::Error::other)?;
        Ok::<_, std::io::Error>((p, t.elapsed()))
    };
    st.latency_ms = Some(match tokio::time::timeout(IO_TIMEOUT, pong).await {
        Ok(Ok((p, rtt))) if p.len() == 9 && p[0] == 0x01 => rtt.as_millis() as u64,
        _ => first.as_millis() as u64,
    });
    Ok(st)
}

/// Ping legacy (0xFE 0x01).
async fn legacy(ip: IpAddr, port: u16, host: &str) -> Result<JavaStatus, String> {
    let mut s = timed(CONNECT_TIMEOUT, "connexion", TcpStream::connect((ip, port))).await?;
    let started = Instant::now();
    timed(
        IO_TIMEOUT,
        "envoi",
        s.write_all(&legacy_request(host, port)),
    )
    .await?;
    let id = timed(IO_TIMEOUT, "lecture", s.read_u8()).await?;
    if id != 0xff {
        return Err("réponse legacy inattendue".into());
    }
    let chars = timed(IO_TIMEOUT, "lecture", s.read_u16()).await? as usize;
    let mut buf = vec![0u8; chars * 2];
    timed(IO_TIMEOUT, "lecture", s.read_exact(&mut buf)).await?;
    let units: Vec<u16> = (0..chars)
        .map(|i| u16::from_be_bytes([buf[2 * i], buf[2 * i + 1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    let mut st = parse_legacy(&text).ok_or("réponse legacy illisible")?;
    st.latency_ms = Some(started.elapsed().as_millis() as u64);
    Ok(st)
}

/// Ping Java : moderne, puis legacy si la réponse moderne est illisible (jamais si la connexion
/// elle-même échoue : le serveur ne répondrait pas mieux).
pub(crate) async fn ping_java(ip: IpAddr, port: u16, host: &str) -> Result<JavaStatus, String> {
    match modern(ip, port, host).await {
        Ok(s) => Ok(s),
        Err(e) if e.starts_with("connexion") => Err(e),
        Err(e) => legacy(ip, port, host)
            .await
            .map_err(|e2| format!("{e} ; ping legacy : {e2}")),
    }
}

/// Ping RakNet d'un port UDP.
pub(crate) async fn ping_bedrock(ip: IpAddr, port: u16) -> Result<BedrockStatus, String> {
    let bind = if ip.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let sock = UdpSocket::bind(bind)
        .await
        .map_err(|e| format!("socket UDP : {e}"))?;
    let guid = i64::from_be_bytes(*b"MSTerm\0\x01");
    let started = Instant::now();
    sock.send_to(&raknet_ping(0x5049_4e47, guid), (ip, port))
        .await
        .map_err(|e| format!("envoi UDP : {e}"))?;
    let mut buf = [0u8; 2048];
    let deadline = started + BEDROCK_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) if from.ip() == ip => {
                let mut st = parse_pong(&buf[..n]).ok_or("réponse UDP illisible : ce n'est pas un serveur Bedrock")?;
                st.latency_ms = Some(started.elapsed().as_millis() as u64);
                return Ok(st);
            }
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => return Err(format!("réception UDP : {e}")),
            Err(_) => {
                return Err(format!(
                    "pas de réponse Bedrock en {} ms sur le port UDP {port} (Geyser arrêté, port non ouvert en UDP, ou autre port)",
                    BEDROCK_TIMEOUT.as_millis()
                ))
            }
        }
    }
}

// --- Rendu ------------------------------------------------------------------------------------------------

pub(crate) fn render_java(st: &JavaStatus) -> Value {
    let mut v = json!({
        "online": true,
        "edition": "java",
        "version": st.version,
        "protocol": st.protocol,
        "motd": st.motd,
        "players": { "online": st.online, "max": st.max },
        "latency_ms": st.latency_ms,
        "method": if st.legacy { "ping legacy (0xFE, avant 1.7)" } else { "Server List Ping (1.7+)" },
    });
    if let Some(m) = &st.modded {
        v["modded"] = json!(m);
    }
    v
}

pub(crate) fn render_bedrock(st: &BedrockStatus, port: u16) -> Value {
    json!({
        "online": true,
        "edition": st.edition,
        "port_udp": port,
        "version": st.version,
        "protocol": st.protocol,
        "motd": st.motd,
        "sub_motd": st.sub_motd,
        "gamemode": st.gamemode,
        "players": { "online": st.online, "max": st.max },
        "latency_ms": st.latency_ms,
        "method": "ping RakNet hors connexion (UDP)",
    })
}

// --- Outil -------------------------------------------------------------------------------------------------

/// Adresses d'un serveur du compte (API), et son nom.
async fn endpoint_of(core: &Core, id: i64) -> Result<(String, Endpoint, Value), String> {
    let overview = core.list_servers().await.map_err(|e| e.to_string())?;
    let s = overview
        .servers
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("serveur {id} introuvable sur ce compte (list_servers) : seuls les serveurs du compte se pinguent"))?;
    let token = core.token().map_err(|e| e.to_string())?;
    let raw = core.api.server_value(&token, id).await.ok();
    let ep = raw
        .as_ref()
        .and_then(|v| Endpoint::from_server_value(id, v))
        .or_else(|| Endpoint::from_list_address(id, &s.address))
        .ok_or_else(|| format!("« {} » ({id}) : adresse ip:port introuvable", s.name))?;
    Ok((s.name.clone(), ep, raw.unwrap_or(Value::Null)))
}

/// Ports UDP à essayer pour Bedrock : le port donné, sinon les ports secondaires décrits comme
/// Bedrock ou Geyser, les autres secondaires, puis le principal.
pub(crate) fn bedrock_ports(ep: &Endpoint, raw: &Value) -> Vec<u16> {
    let described = |p: u16| {
        raw.get("ports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|x| {
                x.get("port").and_then(Value::as_i64) == Some(i64::from(p))
                    && x.get("description")
                        .and_then(Value::as_str)
                        .is_some_and(|d| {
                            let d = d.to_ascii_lowercase();
                            d.contains("bedrock") || d.contains("geyser")
                        })
            })
    };
    let mut ports: Vec<u16> = ep.secondary.iter().map(|(_, p)| *p).collect();
    ports.sort_by_key(|p| !described(*p));
    ports.push(ep.port);
    ports.dedup();
    ports.truncate(MAX_BEDROCK_PORTS);
    ports
}

/// Outil MCP `ping_server`.
pub(crate) async fn ping_server_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = args
        .get("server_id")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .ok_or(
            "paramètre `server_id` (entier) requis : seuls les serveurs du compte se pinguent",
        )?;
    let bedrock = args
        .get("bedrock")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let port = match args.get("port") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .and_then(|p| u16::try_from(p).ok())
                .ok_or("`port` : entier 1-65535 attendu")?,
        ),
    };
    let (name, ep, raw) = endpoint_of(core, id).await?;
    let ip: IpAddr = ep
        .ip
        .parse()
        .map_err(|_| format!("« {name} » : adresse IP illisible"))?;
    let own: Vec<u16> = std::iter::once(ep.port)
        .chain(ep.secondary.iter().map(|(_, p)| *p))
        .collect();
    if let Some(p) = port.filter(|p| !own.contains(p)) {
        return Err(format!(
            "port {p} : ce n'est pas un port de « {name} » ({}) ; seuls ses ports se pinguent",
            own.iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let host = ep.dns.clone().unwrap_or_else(|| ep.ip.clone());
    let mut out = if bedrock {
        let ports = match port {
            Some(p) => vec![p],
            None => bedrock_ports(&ep, &raw),
        };
        let mut tried = Vec::new();
        let mut found = None;
        for p in &ports {
            match ping_bedrock(ip, *p).await {
                Ok(st) => {
                    found = Some(render_bedrock(&st, *p));
                    break;
                }
                Err(e) => tried.push(format!("{p} : {e}")),
            }
            if crate::mcp_ctx::cancelled() {
                break;
            }
        }
        match found {
            Some(v) => v,
            None => {
                return Err(format!(
                    "« {name} » ({id}) ne répond pas en Bedrock (UDP). Ports essayés — {}. Geyser doit être démarré et écouter sur un port du serveur (bedrock.port de sa config) ; un port ajouté avec manage_ports doit être celui-là.",
                    tried.join(" ; ")
                ))
            }
        }
    } else {
        let p = port.unwrap_or(ep.port);
        let st = ping_java(ip, p, &host).await.map_err(|e| {
            format!(
                "« {name} » ({id}) ne répond pas au ping Java sur {}:{p} : {e}",
                ep.ip
            )
        })?;
        let mut v = render_java(&st);
        v["port"] = json!(p);
        v
    };
    out["server_id"] = json!(id);
    out["name"] = json!(name);
    out["address"] = json!(ep.address());
    Ok(crate::config_edit::render_for_ai(core, out))
}

/// Pings de plusieurs serveurs du compte (adresses déjà connues), pour `network_topology`.
pub(crate) async fn ping_many(eps: Vec<Endpoint>) -> HashMap<i64, Value> {
    let tasks = eps.into_iter().map(|ep| async move {
        let v = match ep.ip.parse::<IpAddr>() {
            Ok(ip) => {
                let host = ep.dns.clone().unwrap_or_else(|| ep.ip.clone());
                match ping_java(ip, ep.port, &host).await {
                    Ok(st) => json!({
                        "online": true,
                        "version": st.version,
                        "players": { "online": st.online, "max": st.max },
                        "latency_ms": st.latency_ms,
                        "motd": st.motd,
                    }),
                    Err(e) => json!({ "online": false, "error": e }),
                }
            }
            Err(_) => json!({ "online": false, "error": "adresse IP illisible" }),
        };
        (ep.server_id, v)
    });
    stream::iter(tasks)
        .buffer_unordered(PING_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip() {
        for v in [0, 1, 127, 128, 255, 25565, 2_097_151, i32::MAX, -1] {
            let mut b = Vec::new();
            write_varint(&mut b, v);
            let mut pos = 0;
            assert_eq!(read_varint(&b, &mut pos), Some(v), "{v}");
            assert_eq!(pos, b.len());
        }
        let mut b = Vec::new();
        write_varint(&mut b, -1);
        assert_eq!(b, [0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(read_varint(&[0x80, 0x80], &mut 0), None, "tronqué");
    }

    #[test]
    fn the_status_request_is_a_handshake_then_a_status_query() {
        let b = status_request("mc.exemple.fr", 25565);
        let mut pos = 0;
        let len = read_varint(&b, &mut pos).unwrap() as usize;
        let hs = &b[pos..pos + len];
        let mut p = 0;
        assert_eq!(read_varint(hs, &mut p), Some(0x00), "paquet 0 : handshake");
        assert_eq!(read_varint(hs, &mut p), Some(-1), "protocole inconnu");
        let n = read_varint(hs, &mut p).unwrap() as usize;
        assert_eq!(&hs[p..p + n], b"mc.exemple.fr");
        p += n;
        assert_eq!(u16::from_be_bytes([hs[p], hs[p + 1]]), 25565);
        assert_eq!(
            read_varint(hs, &mut (p + 2)),
            Some(1),
            "état suivant : statut"
        );
        assert_eq!(&b[pos + len..], [0x01, 0x00], "requête de statut");
    }

    #[test]
    fn a_modern_status_is_read_as_plain_text() {
        let v = json!({
            "version": { "name": "Paper 1.21.4", "protocol": 769 },
            "players": { "max": 20, "online": 3, "sample": [{ "name": "Alex", "id": "x" }] },
            "description": { "text": "", "extra": [
                { "text": "Bienvenue ", "color": "gold", "bold": true },
                { "text": "§asur §lle\n  réseau" }
            ] },
            "favicon": "data:image/png;base64,AAAA",
            "forgeData": { "mods": [{}, {}] }
        });
        let st = parse_status(&v);
        assert_eq!(st.version.as_deref(), Some("Paper 1.21.4"));
        assert_eq!(st.protocol, Some(769));
        assert_eq!((st.online, st.max), (Some(3), Some(20)));
        assert_eq!(st.motd, "Bienvenue sur le\nréseau");
        assert_eq!(st.modded.as_deref(), Some("Forge (2 mods)"));
        // Velocity : description en texte, codes de couleur.
        let st = parse_status(
            &json!({ "version": { "name": "Velocity 3.4.0", "protocol": 769 }, "description": "§bA Velocity Server" }),
        );
        assert_eq!(st.motd, "A Velocity Server");
        assert_eq!(st.online, None);
        // Rien ne fuit du rendu : ni favicon ni pseudos.
        let out = render_java(&parse_status(&v)).to_string();
        assert!(!out.contains("Alex") && !out.contains("base64"), "{out}");
    }

    #[test]
    fn legacy_answers_are_read() {
        let text = ["§1", "127", "1.6.4", "Un serveur", "5", "20"].join("\u{0}");
        let st = parse_legacy(&text).unwrap();
        assert_eq!(st.protocol, Some(127));
        assert_eq!(st.version.as_deref(), Some("1.6.4"));
        assert_eq!(
            (st.motd.as_str(), st.online, st.max),
            ("Un serveur", Some(5), Some(20))
        );
        let old = parse_legacy("Très vieux serveur§2§10").unwrap();
        assert_eq!(
            (old.motd.as_str(), old.online, old.max),
            ("Très vieux serveur", Some(2), Some(10))
        );
        assert!(parse_legacy("n'importe quoi").is_none());
        // Requête : FE 01 FA, canal MC|PingHost en UTF-16.
        let r = legacy_request("a", 25565);
        assert_eq!(&r[..3], [0xfe, 0x01, 0xfa]);
        assert_eq!(u16::from_be_bytes([r[3], r[4]]), 11);
        assert_eq!(&r[r.len() - 4..], 25565i32.to_be_bytes());
    }

    #[test]
    fn a_raknet_pong_is_read() {
        let ping = raknet_ping(1, 2);
        assert_eq!(ping.len(), 33);
        assert_eq!(ping[0], 0x01);
        assert_eq!(&ping[9..25], RAKNET_MAGIC);
        let text =
            "MCPE;§eGeyser§r Réseau;800;1.21.80;2;100;123456;Sous-titre;Survival;1;19132;19133;";
        let mut pong = vec![0x1c];
        pong.extend(1i64.to_be_bytes());
        pong.extend(99i64.to_be_bytes());
        pong.extend(RAKNET_MAGIC);
        pong.extend((text.len() as u16).to_be_bytes());
        pong.extend(text.as_bytes());
        let st = parse_pong(&pong).unwrap();
        assert_eq!(st.edition, "MCPE");
        assert_eq!(st.motd, "Geyser Réseau");
        assert_eq!(
            (st.protocol, st.version.as_deref()),
            (Some(800), Some("1.21.80"))
        );
        assert_eq!((st.online, st.max), (Some(2), Some(100)));
        assert_eq!(st.gamemode.as_deref(), Some("Survival"));
        assert!(parse_pong(&pong[..20]).is_none());
        let mut bad = pong.clone();
        bad[20] ^= 0xff;
        assert!(parse_pong(&bad).is_none(), "marqueur faux");
    }

    #[test]
    fn bedrock_ports_are_the_servers_own_described_first() {
        let ep = Endpoint {
            server_id: 1,
            ip: "203.0.113.10".into(),
            port: 25565,
            dns: None,
            secondary: vec![
                ("203.0.113.10".into(), 40001),
                ("203.0.113.10".into(), 40002),
            ],
        };
        let raw = json!({ "ports": [
            { "port": 40001, "ip": "203.0.113.10", "description": "Votifier" },
            { "port": 40002, "ip": "203.0.113.10", "description": "Geyser (Bedrock)" }
        ] });
        assert_eq!(bedrock_ports(&ep, &raw), [40002, 40001, 25565]);
    }

    #[tokio::test]
    async fn a_closed_port_gives_a_clear_error() {
        // Port local fermé (socket liée puis relâchée) : refus immédiat, message explicite.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let e = ping_java("127.0.0.1".parse().unwrap(), port, "localhost")
            .await
            .unwrap_err();
        assert!(e.starts_with("connexion"), "{e}");
    }

    #[tokio::test]
    async fn a_local_fake_server_answers_the_ping() {
        // Faux serveur : répond au statut puis au ping, comme un serveur 1.7+.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let _ = s.read(&mut buf).await.unwrap();
            let json = r#"{"version":{"name":"Paper 1.21.4","protocol":769},"players":{"max":50,"online":7},"description":"Salut"}"#;
            let mut body = Vec::new();
            write_string(&mut body, json);
            s.write_all(&frame(0x00, &body)).await.unwrap();
            let n = s.read(&mut buf).await.unwrap();
            let mut pos = 0;
            let len = read_varint(&buf[..n], &mut pos).unwrap() as usize;
            s.write_all(&frame(0x01, &buf[pos + 1..pos + len]))
                .await
                .unwrap();
        });
        let st = ping_java("127.0.0.1".parse().unwrap(), port, "localhost")
            .await
            .unwrap();
        assert_eq!(st.version.as_deref(), Some("Paper 1.21.4"));
        assert_eq!((st.online, st.max), (Some(7), Some(50)));
        assert_eq!(st.motd, "Salut");
        assert!(st.latency_ms.is_some() && !st.legacy);
    }
}
