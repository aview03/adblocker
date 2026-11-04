//! VPN Gate(쓰쿠바 대학) 공개 서버 목록 + OpenVPN 연결.
//!
//! - 목록은 한 번에 전부 받아 디스크에 캐시하고, 마지막 요청으로부터 3일이 지나야 다시 받는다.
//! - 국가별로 묶고, 국가 안에서는 속도가 빠른 서버 순(속도를 모르면 무작위).
//! - config 는 자원봉사자가 만든 외부 입력이므로 허용 목록(allowlist)에 있는 지시어만 통과시킨다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const API_URL: &str = "https://www.vpngate.net/api/iphone/";
const MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// 서버 하나에 접속을 시도하는 최대 시간.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
/// 사전 탐색(probe): TCP 연결 / UDP 핸드셰이크 응답을 기다리는 시간, 전체 상한.
const PROBE_TCP_TIMEOUT: Duration = Duration::from_millis(2500);
const PROBE_UDP_WAIT: Duration = Duration::from_millis(1200);
const PROBE_TOTAL: Duration = Duration::from_secs(4);
/// 블랙리스트 기간: 무응답/시간 초과 6시간, 설정 거부 24시간(목록이 바뀌기 전엔 그대로), 인증 거부 1시간.
const TTL_DEAD: u64 = 6 * 60 * 60;
const TTL_REJECT: u64 = 24 * 60 * 60;
/// 인증 거부는 동시 접속 한도 초과 등 일시적인 경우가 많아 짧게 제외한다.
const TTL_AUTH: u64 = 60 * 60;
/// 모든 후보가 실패했는데 캐시가 이보다 오래됐다면, 3일 규칙의 예외로 목록을 한 번 새로 받는다.
const REFRESH_AFTER: Duration = Duration::from_secs(12 * 60 * 60);
/// 한 국가에서 실패해도 다음 서버로 넘어가며 시도하는 최대 개수.
const MAX_ATTEMPTS: usize = 6;

#[derive(Clone)]
pub struct Server {
    pub ip: String,
    /// bps. 0 이면 알 수 없음.
    pub speed: u64,
    pub ping: u32,
    config_b64: String,
}

pub struct Country {
    pub short: String,
    pub name: String,
    pub servers: Vec<Server>,
}

impl Country {
    /// 한국어 이름(모르는 국가는 영어 원문).
    pub fn label(&self) -> &str {
        ko_name(&self.short).unwrap_or(&self.name)
    }

    pub fn best_speed(&self) -> u64 {
        self.servers.iter().map(|s| s.speed).max().unwrap_or(0)
    }

    pub fn best_ping(&self) -> Option<u32> {
        self.servers.iter().filter(|s| s.speed > 0).min_by_key(|s| s.ping).map(|s| s.ping)
    }

    #[cfg(test)]
    fn attempt_order(&self) -> Vec<Server> {
        order(self.servers.clone()).into_iter().take(MAX_ATTEMPTS).collect()
    }
}

/// 접속을 시도할 순서: 속도를 아는 서버는 빠른 순, 모르는 서버는 무작위로 그 뒤에.
fn order(servers: Vec<Server>) -> Vec<Server> {
    let (mut known, mut unknown): (Vec<Server>, Vec<Server>) = servers.into_iter().partition(|s| s.speed > 0);
    known.sort_by(|a, b| b.speed.cmp(&a.speed));
    shuffle(&mut unknown);
    known.extend(unknown);
    known
}

/// 설정 파일에서 읽은 접속 지점(사전 탐색용).
struct Endpoint {
    host: String,
    port: u16,
    udp: bool,
}

impl Server {
    fn endpoint(&self) -> Option<Endpoint> {
        let text = String::from_utf8(b64_decode(&self.config_b64)?).ok()?;
        let (mut udp, mut remote) = (true, None);
        for l in text.lines() {
            let mut it = l.split_whitespace();
            match it.next() {
                Some("proto") => udp = it.next().map_or(true, |p| p.starts_with("udp")),
                Some("remote") if remote.is_none() => {
                    if let (Some(h), Some(Ok(p))) = (it.next(), it.next().map(str::parse::<u16>)) {
                        remote = Some((h.to_string(), p));
                    }
                }
                _ => {}
            }
        }
        let (host, port) = remote?;
        Some(Endpoint { host, port, udp })
    }
}

pub struct ServerList {
    pub countries: Vec<Country>,
    /// 캐시를 받은 지 며칠이 지났는지.
    pub age_days: u64,
    /// 갱신에 실패해 오래된 캐시를 쓰는 중이면 true.
    pub stale: bool,
}

fn ko_name(short: &str) -> Option<&'static str> {
    Some(match short {
        "JP" => "일본",
        "KR" => "대한민국",
        "US" => "미국",
        "RU" => "러시아",
        "TH" => "태국",
        "CA" => "캐나다",
        "HR" => "크로아티아",
        "RO" => "루마니아",
        "PT" => "포르투갈",
        "GB" => "영국",
        "AT" => "오스트리아",
        "TW" => "대만",
        "LV" => "라트비아",
        "DE" => "독일",
        "FR" => "프랑스",
        "NL" => "네덜란드",
        "SG" => "싱가포르",
        "VN" => "베트남",
        "HK" => "홍콩",
        "IN" => "인도",
        "ID" => "인도네시아",
        "UA" => "우크라이나",
        "PL" => "폴란드",
        "CH" => "스위스",
        "SE" => "스웨덴",
        "ES" => "스페인",
        "IT" => "이탈리아",
        "BR" => "브라질",
        "AU" => "호주",
        "CN" => "중국",
        "MY" => "말레이시아",
        "PH" => "필리핀",
        "TR" => "튀르키예",
        _ => return None,
    })
}

// ---------- util ----------

fn rand_u64() -> u64 {
    use std::sync::atomic::AtomicU64;
    static CTR: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let mut x = t ^ CTR.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// OS 가 시드한 RandomState 에서 얻는 64비트 값 (비밀번호 용도로 시간 기반 난수보다 낫다).
fn secure_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(rand_u64());
    h.finish()
}

fn shuffle<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        v.swap(i, (rand_u64() % (i as u64 + 1)) as usize);
    }
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn data_dir() -> Option<PathBuf> {
    let d = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("AdBlocker-thinkgood"))?;
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

// ---------- 목록 ----------

fn parse(csv: &str) -> Vec<Country> {
    let mut countries: Vec<Country> = Vec::new();
    for line in csv.lines() {
        if line.starts_with('*') || line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.trim_end().split(',').collect();
        // HostName,IP,Score,Ping,Speed,CountryLong,CountryShort,...,OpenVPN_ConfigData_Base64(마지막)
        if f.len() < 15 {
            continue;
        }
        let (short, name) = (f[6].trim().to_uppercase(), f[5].trim().to_string());
        let config = f[f.len() - 1].trim();
        if short.len() != 2 || config.is_empty() {
            continue;
        }
        let server = Server {
            ip: f[1].trim().to_string(),
            speed: f[4].trim().parse().unwrap_or(0),
            ping: f[3].trim().parse().unwrap_or(0),
            config_b64: config.to_string(),
        };
        match countries.iter_mut().find(|c| c.short == short) {
            Some(c) => c.servers.push(server),
            None => countries.push(Country { short, name, servers: vec![server] }),
        }
    }
    // 일본 최상단, 나머지는 서버가 많은 순 → 이름순
    countries.sort_by(|a, b| {
        (b.short == "JP")
            .cmp(&(a.short == "JP"))
            .then(b.servers.len().cmp(&a.servers.len()))
            .then(a.name.cmp(&b.name))
    });
    countries
}

fn cache_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("vpngate-cache.csv"))
}

fn cache_age(path: &Path) -> Option<Duration> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(m).ok()
}

fn download(dest: &Path) -> Result<(), String> {
    let tmp = dest.with_extension("part");
    let out = Command::new("curl.exe")
        .args(["-fsS", "-m", "60", "-o"])
        .arg(&tmp)
        .arg(API_URL)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("curl 실행 실패: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("목록을 받지 못했습니다 ({})", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = std::fs::read_to_string(&tmp).unwrap_or_default();
    if !text.starts_with("*vpn_servers") {
        let _ = std::fs::remove_file(&tmp);
        return Err("서버 목록 형식이 올바르지 않습니다".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// 캐시가 3일 이내면 그대로, 아니면 새로 받는다. 받기에 실패하면 오래된 캐시라도 쓴다.
pub fn load_list() -> Result<ServerList, String> {
    let path = cache_path().ok_or("APPDATA 를 찾을 수 없습니다")?;
    let age = cache_age(&path);
    let mut stale = false;
    if age.map_or(true, |a| a > MAX_AGE) {
        if let Err(e) = download(&path) {
            if age.is_none() {
                return Err(e);
            }
            stale = true;
        }
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let countries = parse(&text);
    if countries.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Err("사용 가능한 서버가 없습니다".into());
    }
    let age_days = cache_age(&path).map_or(0, |a| a.as_secs() / 86_400);
    Ok(ServerList { countries, age_days, stale })
}

// ---------- config 검증 ----------

/// 값(인자)이 있는 지시어 중 허용하는 것. 스크립트/플러그인/프록시/관리 포트 등은 전부 거부된다.
const ALLOWED: &[&str] = &[
    "client", "dev", "dev-type", "proto", "remote", "cipher", "data-ciphers", "data-ciphers-fallback", "auth",
    "resolv-retry", "nobind", "persist-key", "persist-tun", "verb", "remote-cert-tls", "tls-client", "pull",
    "key-direction", "tun-mtu", "mssfix", "reneg-sec", "keepalive", "ping", "ping-restart", "connect-retry",
    "connect-timeout", "tls-version-min", "tls-cipher", "tls-ciphersuites", "ca", "cert", "key", "tls-auth",
    "tls-crypt", "auth-nocache", "float", "explicit-exit-notify",
];
const BLOCKS: &[&str] = &["ca", "cert", "key", "tls-auth", "tls-crypt"];

fn safe_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._:+/=".contains(&b))
}

/// 허용 목록에 없는 지시어가 하나라도 있으면 config 전체를 거부한다(조용히 지우지 않음).
fn sanitize(config: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(config).map_err(|_| "config 가 UTF-8 이 아닙니다")?;
    let mut out = String::with_capacity(text.len());
    let mut block: Option<&str> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(name) = block {
            // 인라인 블록 내부: PEM 본문만 허용
            if line == format!("</{name}>") {
                block = None;
            } else if !(line.is_empty()
                || (line.starts_with("-----") && line.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b' '))
                || line.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
                || line.bytes().all(|b| b.is_ascii_hexdigit() || b == b':'))
            {
                return Err("config 인라인 블록에 허용되지 않는 내용".into());
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(tag) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
            let b = BLOCKS.iter().find(|b| **b == tag).ok_or_else(|| format!("허용되지 않는 블록 <{tag}>"))?;
            block = Some(b);
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        if !ALLOWED.contains(&key) {
            return Err(format!("허용되지 않는 지시어 '{key}' — 접속하지 않습니다"));
        }
        if BLOCKS.contains(&key) && !args.is_empty() {
            return Err(format!("'{key}' 외부 파일 참조는 허용되지 않습니다"));
        }
        if !args.iter().all(|a| safe_token(a)) {
            return Err(format!("'{key}' 인자에 허용되지 않는 문자"));
        }
        if key == "proto" && !args.iter().all(|a| matches!(*a, "udp" | "tcp" | "udp4" | "tcp4" | "udp6" | "tcp6" | "tcp-client")) {
            return Err("지원하지 않는 proto".into());
        }
        if key == "dev" && !args.iter().all(|a| matches!(*a, "tun" | "tap")) {
            return Err("지원하지 않는 dev".into());
        }
        out.push_str(line);
        out.push('\n');
    }
    if block.is_some() {
        return Err("config 블록이 닫히지 않았습니다".into());
    }
    if !out.contains("\nremote ") && !out.starts_with("remote ") {
        return Err("remote 지시어가 없습니다".into());
    }
    Ok(out)
}

// ---------- 연결 ----------

pub fn find_openvpn() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(p) = std::env::var_os(var) {
            roots.push(PathBuf::from(p));
        }
    }
    roots.into_iter().map(|r| r.join("OpenVPN").join("bin").join("openvpn.exe")).find(|p| p.exists())
}

// ---------- 사전 탐색 (죽은 서버를 OpenVPN 을 띄우기 전에 걸러낸다) ----------

fn udp_probe(addr: std::net::SocketAddr) -> bool {
    let bind = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let Ok(sock) = UdpSocket::bind(bind) else { return true };
    if sock.connect(addr).is_err() {
        return true;
    }
    // OpenVPN P_CONTROL_HARD_RESET_CLIENT_V2: opcode(7<<3) + session id(8) + ack 개수(0) + packet id(4)
    let mut pkt = [0u8; 14];
    pkt[0] = 7 << 3;
    pkt[1..9].copy_from_slice(&rand_u64().to_le_bytes());
    let _ = sock.set_read_timeout(Some(PROBE_UDP_WAIT));
    for _ in 0..2 {
        if sock.send(&pkt).is_err() {
            return false;
        }
        let mut buf = [0u8; 64];
        // 서버는 P_CONTROL_HARD_RESET_SERVER_V2(8<<3) 로 답한다. (tls-auth/crypt 를 쓰지 않는 VPN Gate 설정 기준)
        if let Ok(n) = sock.recv(&mut buf) {
            if n > 0 && buf[0] >> 3 == 8 {
                return true;
            }
        }
    }
    false
}

fn probe(ep: &Endpoint) -> bool {
    let Some(addr) = (ep.host.as_str(), ep.port).to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return true; // 해석 못 하면 판단 보류(제외하지 않음)
    };
    if ep.udp {
        udp_probe(addr)
    } else {
        TcpStream::connect_timeout(&addr, PROBE_TCP_TIMEOUT).is_ok()
    }
}

/// 후보 전체를 병렬로 탐색. 제한 시간 안에 답이 없으면 죽은 것으로 본다.
fn probe_all(servers: &[Server]) -> Vec<bool> {
    let (tx, rx) = mpsc::channel();
    for (i, s) in servers.iter().enumerate() {
        let (tx, ep) = (tx.clone(), s.endpoint());
        std::thread::spawn(move || {
            let _ = tx.send((i, ep.map_or(true, |e| probe(&e))));
        });
    }
    drop(tx);
    let mut res = vec![false; servers.len()];
    let deadline = Instant::now() + PROBE_TOTAL;
    let mut got = 0;
    while got < servers.len() {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((i, alive)) => {
                res[i] = alive;
                got += 1;
            }
            Err(_) => break,
        }
    }
    res
}

// ---------- 블랙리스트 ----------

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn bl_parse(text: &str, now: u64) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let (ip, exp) = l.split_once('\t')?;
            let exp: u64 = exp.trim().parse().ok()?;
            (exp > now).then(|| (ip.to_string(), exp))
        })
        .collect()
}

fn bl_format(map: &HashMap<String, u64>) -> String {
    map.iter().map(|(ip, exp)| format!("{ip}\t{exp}\n")).collect()
}

fn bl_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("vpn-blacklist.txt"))
}

/// 만료되지 않은 항목만.
fn bl_load() -> HashMap<String, u64> {
    bl_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| bl_parse(&t, now_secs())).unwrap_or_default()
}

fn bl_update(f: impl FnOnce(&mut HashMap<String, u64>)) {
    if let Some(p) = bl_path() {
        let mut m = bl_load();
        f(&mut m);
        let _ = std::fs::write(p, bl_format(&m));
    }
}

fn bl_add(ip: &str, ttl: u64) {
    bl_update(|m| {
        m.insert(ip.to_string(), now_secs() + ttl);
    });
}

fn bl_remove(ip: &str) {
    bl_update(|m| {
        m.remove(ip);
    });
}

// ---------- 연결 ----------

/// 앱에 내장된 OpenVPN 공식 Windows 설치 패키지 (third_party/openvpn/README.md 참고).
static OPENVPN_MSI: &[u8] = include_bytes!("../third_party/openvpn/OpenVPN-2.7.8-I002-amd64.msi");

/// 내장 설치 패키지로 OpenVPN(코어 + TAP-Windows6 드라이버)을 조용히 설치한다. (블로킹, 관리자 권한 필요)
pub fn install_openvpn() -> Result<PathBuf, String> {
    let dir = data_dir().ok_or("APPDATA 를 찾을 수 없습니다")?;
    let msi = dir.join("openvpn-setup.msi");
    std::fs::write(&msi, OPENVPN_MSI).map_err(|e| e.to_string())?;
    let out = Command::new("msiexec")
        .arg("/i")
        .arg(&msi)
        .args(["/qn", "/norestart", "ADDLOCAL=OpenVPN,Drivers,Drivers.TAPWindows6"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = std::fs::remove_file(&msi);
    let out = out.map_err(|e| format!("msiexec 실행 실패: {e}"))?;
    match out.status.code() {
        // 3010 = 성공(재부팅 권장)
        Some(0) | Some(3010) => find_openvpn().ok_or_else(|| "설치는 끝났지만 openvpn.exe 를 찾지 못했습니다".to_string()),
        Some(c) => Err(format!("OpenVPN 설치에 실패했습니다 (msiexec 코드 {c})")),
        None => Err("OpenVPN 설치가 중단되었습니다".to_string()),
    }
}

/// 실행 중인 OpenVPN 과 그 로컬 관리 인터페이스(정상 종료용).
pub struct Tunnel {
    child: Child,
    port: u16,
    pass: String,
}

pub type Handle = Arc<Mutex<Option<Tunnel>>>;

pub struct Connected {
    pub ip: String,
    /// 접속 버튼을 누른 뒤 연결될 때까지(탐색 포함) 걸린 시간.
    pub total_secs: f32,
}

pub struct Attempt {
    pub n: usize,
    pub ip: String,
    pub ok: bool,
    pub reason: &'static str,
    pub secs: f32,
}

pub enum Event {
    /// OpenVPN 이 없어서 내장 패키지를 설치하는 중.
    Installing,
    Probe { alive: usize, total: usize, secs: f32 },
    Attempt(Attempt),
    /// 후보가 모두 실패해서 목록을 일찍 새로 받았다.
    ListRefreshed,
}

enum TryErr {
    /// (사유, 블랙리스트 기간)
    Fail(&'static str, u64),
    Cancelled,
    Fatal(String),
}

fn read_country(short: &str) -> Option<Country> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    parse(&text).into_iter().find(|c| c.short == short)
}

/// 서버 하나에 접속을 시도한다. 성공하면 자식 프로세스를 `handle` 에 보관한다.
fn try_server(
    exe: &Path,
    server: &Server,
    cfg_path: &Path,
    auth_path: &Path,
    handle: &Handle,
    cancel: &Arc<AtomicBool>,
    on_end: &mut Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), TryErr> {
    let cfg = b64_decode(&server.config_b64)
        .ok_or(())
        .and_then(|b| sanitize(&b).map_err(|_| ()))
        .map_err(|_| TryErr::Fail("설정 거부", TTL_REJECT))?;
    std::fs::write(cfg_path, cfg).map_err(|e| TryErr::Fatal(e.to_string()))?;
    std::fs::write(auth_path, "vpn\nvpn\n").map_err(|e| TryErr::Fatal(e.to_string()))?;

    // 정상 종료(라우팅/DNS 정리)용 로컬 관리 인터페이스: 127.0.0.1 + 무작위 포트 + 무작위 비밀번호
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| TryErr::Fatal(format!("관리 포트를 열 수 없습니다: {e}")))?;
    let pass = format!("{:016x}{:016x}", secure_u64(), secure_u64());
    let mgmt_path = auth_path.with_file_name("vpn-session.mgmt");
    std::fs::write(&mgmt_path, format!("{pass}\n")).map_err(|e| TryErr::Fatal(e.to_string()))?;

    let spawned = Command::new(exe)
        .arg("--config")
        .arg(cfg_path)
        .arg("--auth-user-pass")
        .arg(auth_path)
        .args(["--management", "127.0.0.1", &port.to_string()])
        .arg(&mgmt_path)
        // OpenVPN 2.7 의 기본 드라이버는 DCO 이고 Wintun 지원은 제거됐다. VPN Gate 는 CBC 암호라 DCO 를
        // 쓸 수 없으므로(폴백되긴 하지만) 처음부터 TAP-Windows6 를 쓰도록 DCO 를 끈다.
        .arg("--disable-dco")
        .args(["--script-security", "0", "--verb", "3", "--connect-retry-max", "1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(cfg_path);
            let _ = std::fs::remove_file(auth_path);
            let _ = std::fs::remove_file(&mgmt_path);
            return Err(TryErr::Fatal(format!("OpenVPN 실행 실패: {e}")));
        }
    };
    let stdout = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut outcome: Result<(), TryErr> = Err(TryErr::Fail("시간 초과", TTL_DEAD));
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            outcome = Err(TryErr::Cancelled);
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(l) if l.contains("Initialization Sequence Completed") => {
                outcome = Ok(());
                break;
            }
            Ok(l) if l.contains("AUTH_FAILED") => {
                outcome = Err(TryErr::Fail("인증 거부", TTL_AUTH));
                break;
            }
            Ok(l) if l.contains("Exiting due to fatal error") => {
                outcome = Err(TryErr::Fail("접속 오류", TTL_DEAD));
                break;
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                outcome = Err(TryErr::Fail("종료됨", TTL_DEAD));
                break;
            }
        }
    }
    // 키/인증서가 든 임시 config 는 더 이상 필요 없다.
    let _ = std::fs::remove_file(cfg_path);
    let _ = std::fs::remove_file(auth_path);
    let _ = std::fs::remove_file(&mgmt_path); // 비밀번호는 시작 시 한 번만 읽는다

    if outcome.is_ok() {
        *handle.lock().unwrap() = Some(Tunnel { child, port, pass });
        if let Some(on_end) = on_end.take() {
            std::thread::spawn(move || {
                while rx.recv().is_ok() {}
                on_end();
            });
        }
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    outcome
}

/// 국가 `short` 의 서버를 사전 탐색으로 걸러낸 뒤, 빠른 순으로 최대 MAX_ATTEMPTS 개를 시도한다.
/// (블로킹 — 스레드에서 호출) 진행 상황은 `on_event` 로 알린다.
/// 성공하면 자식 프로세스는 `handle` 에 보관되고, 이후 종료되면 `on_end` 가 불린다.
pub fn connect(
    exe: &Path,
    short: &str,
    handle: &Handle,
    cancel: &Arc<AtomicBool>,
    on_event: &dyn Fn(Event),
    on_end: impl FnOnce() + Send + 'static,
) -> Result<Connected, String> {
    let t0 = Instant::now();
    let dir = data_dir().ok_or("APPDATA 를 찾을 수 없습니다")?;
    let cfg_path = dir.join("vpn-session.ovpn");
    // VPN Gate 서버는 설정 파일에 계정이 없고 사용자명/비밀번호 "vpn"/"vpn" 을 요구한다.
    // 외부 입력이 아니라 우리가 직접 만드는 파일이며, 설정 파일 안의 auth-user-pass 는 허용하지 않는다.
    let auth_path = dir.join("vpn-session.auth");
    let mut on_end: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(on_end));
    let (mut n, mut refreshed) = (0usize, false);
    let mut last = String::from("응답하는 서버가 없습니다");

    loop {
        let country = read_country(short).ok_or("서버 목록을 읽을 수 없습니다")?;
        let bl = bl_load();
        let mut cands: Vec<Server> = country.servers.iter().filter(|s| !bl.contains_key(&s.ip)).cloned().collect();
        if cands.is_empty() {
            cands = country.servers.clone(); // 전부 제외돼 있으면 이번만 블랙리스트를 무시
        }

        let p0 = Instant::now();
        let flags = probe_all(&cands);
        let total = cands.len();
        let mut alive = Vec::new();
        for (s, ok) in cands.into_iter().zip(flags) {
            if ok {
                alive.push(s);
            } else {
                bl_add(&s.ip, TTL_DEAD);
            }
        }
        on_event(Event::Probe { alive: alive.len(), total, secs: p0.elapsed().as_secs_f32() });

        for server in order(alive).into_iter().take(MAX_ATTEMPTS) {
            if cancel.load(Ordering::Relaxed) {
                return Err("취소됨".into());
            }
            n += 1;
            let a0 = Instant::now();
            let res = try_server(exe, &server, &cfg_path, &auth_path, handle, cancel, &mut on_end);
            let secs = a0.elapsed().as_secs_f32();
            match res {
                Ok(()) => {
                    bl_remove(&server.ip);
                    on_event(Event::Attempt(Attempt { n, ip: server.ip.clone(), ok: true, reason: "연결", secs }));
                    return Ok(Connected { ip: server.ip, total_secs: t0.elapsed().as_secs_f32() });
                }
                Err(TryErr::Cancelled) => return Err("취소됨".into()),
                Err(TryErr::Fatal(e)) => return Err(e),
                Err(TryErr::Fail(reason, ttl)) => {
                    bl_add(&server.ip, ttl);
                    last = reason.to_string();
                    on_event(Event::Attempt(Attempt { n, ip: server.ip, ok: false, reason, secs }));
                }
            }
        }

        // 전부 실패: 캐시가 오래됐다면 목록을 일찍 새로 받아 한 번 더 시도한다.
        let old = cache_path().and_then(|p| cache_age(&p)).map_or(true, |a| a >= REFRESH_AFTER);
        if !refreshed && old {
            refreshed = true;
            if cache_path().is_some_and(|p| download(&p).is_ok()) {
                on_event(Event::ListRefreshed);
                continue;
            }
        }
        return Err(format!("접속하지 못했습니다 ({last})"));
    }
}

/// 연결 중인 OpenVPN 을 종료한다. 먼저 관리 인터페이스로 SIGTERM 을 보내 라우팅/DNS 를 정리하게 하고,
/// 4초 안에 끝나지 않으면 강제 종료한다.
pub fn disconnect(handle: &Handle) {
    let Some(mut t) = handle.lock().unwrap().take() else { return };
    if let Ok(mut sock) = TcpStream::connect_timeout(&([127, 0, 0, 1], t.port).into(), Duration::from_secs(1)) {
        let _ = sock.write_all(format!("{}\nsignal SIGTERM\n", t.pass).as_bytes());
        let _ = sock.flush();
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if matches!(t.child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = t.child.kill();
    let _ = t.child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "client\ndev tun\nproto udp\nremote 1.2.3.4 1194\ncipher AES-128-CBC\nnobind\n<ca>\n-----BEGIN CERTIFICATE-----\nMIIFazCC+/=\n-----END CERTIFICATE-----\n</ca>\n";

    #[test]
    fn accepts_normal_config() {
        assert!(sanitize(GOOD.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_script_directives() {
        for bad in ["up /tmp/x.sh", "script-security 2", "plugin foo.dll", "route-up cmd", "management 127.0.0.1 7505", "http-proxy 1.1.1.1 80", "tls-verify x", "ca C:/x.crt"] {
            let c = format!("{GOOD}{bad}\n");
            assert!(sanitize(c.as_bytes()).is_err(), "{bad} 가 통과됨");
        }
    }

    #[test]
    fn rejects_payload_inside_block() {
        let c = GOOD.replace("MIIFazCC+/=", "up evil.bat");
        assert!(sanitize(c.as_bytes()).is_err());
    }

    #[test]
    fn rejects_shell_metachars_in_args() {
        let c = GOOD.replace("remote 1.2.3.4 1194", "remote 1.2.3.4;calc 1194");
        assert!(sanitize(c.as_bytes()).is_err());
    }

    /// 실제 목록 파일로 검증: VG_CSV=<경로> cargo test real_list -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_list() {
        let path = std::env::var("VG_CSV").expect("VG_CSV");
        let list = parse(&std::fs::read_to_string(path).unwrap());
        let (mut ok, mut bad) = (0, 0);
        for c in &list {
            for sv in &c.servers {
                match b64_decode(&sv.config_b64).ok_or("b64".to_string()).and_then(|b| sanitize(&b)) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        bad += 1;
                        println!("거부 {} {}: {e}", c.short, sv.ip);
                    }
                }
            }
            println!("{} {} servers={} best={}Mbps order={:?}", c.short, c.label(), c.servers.len(), c.best_speed() / 1_000_000,
                c.attempt_order().iter().map(|s| s.speed / 1_000_000).collect::<Vec<_>>());
        }
        println!("통과 {ok} / 거부 {bad}");
        assert!(ok > 0);
    }

    #[test]
    fn probe_tcp_alive_and_dead() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(probe(&Endpoint { host: "127.0.0.1".into(), port, udp: false }));
        drop(l);
        assert!(!probe(&Endpoint { host: "127.0.0.1".into(), port, udp: false }));
    }

    #[test]
    fn probe_udp_alive_and_dead() {
        // 서버가 HARD_RESET_SERVER(8<<3) 로 답하면 alive
        let srv = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = srv.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let mut b = [0u8; 64];
            let (n, from) = srv.recv_from(&mut b).unwrap();
            assert_eq!((n, b[0] >> 3), (14, 7));
            srv.send_to(&[8 << 3, 0, 0], from).unwrap();
        });
        assert!(probe(&Endpoint { host: "127.0.0.1".into(), port, udp: true }));
        t.join().unwrap();
        // 아무도 듣지 않으면 dead
        let dead = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = dead.local_addr().unwrap().port();
        assert!(!probe(&Endpoint { host: "127.0.0.1".into(), port, udp: true }));
    }

    #[test]
    fn blacklist_expiry() {
        let mut m = HashMap::new();
        m.insert("1.1.1.1".to_string(), 2000u64);
        m.insert("2.2.2.2".to_string(), 500u64);
        let parsed = bl_parse(&bl_format(&m), 1000);
        assert!(parsed.contains_key("1.1.1.1"));
        assert!(!parsed.contains_key("2.2.2.2")); // 만료
    }

    #[test]
    fn endpoint_from_real_config() {
        let cfg = "client\ndev tun\nproto tcp\nremote 1.2.3.4 1475\n";
        let b64 = {
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let d = cfg.as_bytes();
            let mut o = String::new();
            for c in d.chunks(3) {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                for i in 0..4 {
                    if i <= c.len() { o.push(T[(n >> (18 - 6 * i) & 63) as usize] as char) } else { o.push('=') }
                }
            }
            o
        };
        let s = Server { ip: "1.2.3.4".into(), speed: 1, ping: 1, config_b64: b64 };
        let e = s.endpoint().unwrap();
        assert_eq!((e.host.as_str(), e.port, e.udp), ("1.2.3.4", 1475, false));
    }

    /// 실제 목록 탐색: VG_CSV=<경로> cargo test real_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_probe() {
        let list = parse(&std::fs::read_to_string(std::env::var("VG_CSV").unwrap()).unwrap());
        for c in list.iter().filter(|c| ["JP", "KR", "US"].contains(&c.short.as_str())) {
            let t = Instant::now();
            let flags = probe_all(&c.servers);
            let alive = flags.iter().filter(|a| **a).count();
            let udp = c.servers.iter().filter(|s| s.endpoint().is_some_and(|e| e.udp)).count();
            println!("{} 서버 {} (UDP {}) → 응답 {} · 탐색 {:.1}초", c.short, c.servers.len(), udp, alive, t.elapsed().as_secs_f32());
            let known = ["39.110.8.90", "124.214.92.150", "124.219.214.107"];
            for (s, ok) in c.servers.iter().zip(&flags).filter(|(s, _)| known.contains(&s.ip.as_str())) {
                let e = s.endpoint().unwrap();
                println!("   {}:{} {} speed={}Mbps → {}", e.host, e.port, if e.udp { "udp" } else { "tcp" }, s.speed / 1_000_000, if *ok { "alive" } else { "dead" });
            }
        }
    }

    #[test]
    fn b64() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
    }

    #[test]
    fn jp_first_then_by_count() {
        let row = |ip: &str, cc: &str, name: &str| format!("h,{ip},1,10,500,{name},{cc},1,1,1,1,x,y,z,QQ==\n");
        let csv = format!(
            "*vpn_servers\n#h\n{}{}{}{}*\n",
            row("1.1.1.1", "KR", "Korea"),
            row("1.1.1.2", "KR", "Korea"),
            row("1.1.1.3", "JP", "Japan"),
            row("1.1.1.4", "US", "United States"),
        );
        let c = parse(&csv);
        assert_eq!(c[0].short, "JP");
        assert_eq!(c[1].short, "KR");
        assert_eq!(c[2].short, "US");
    }
}
