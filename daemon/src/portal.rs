//! 认证引擎: 探测 → GET 登录页 → POST 表单 → 复核放行。
//!
//! 实测环境: 贺州学院校园网, 门户为 zaxsoft(石斧软件)的 bossWeb 门户
//! (Server: bossWebV1.0.1, 门户地址 10.255.2.252), AC 侧为 axe_bras/1.0。
//! 换成同类门户只需改下面的 PORTAL_HOST / SELF_SERVICE_URL 与 BASE_FORM_DEFAULTS。
//!
//! 门户协议要点(逐字段实机验证):
//! 1. 未认证时 AC 302 劫持任意 HTTP, query 即会话绑定参数, 必须原样带回
//! 2. 登录 = 向同一 URL POST B 组 9 字段 + userId/passwd(表单编码, 值百分号编码)
//! 3. 是否上线只看复核探测的响应头(无 Location = 已放行), 不解析中文
//! 4. 撞槽位绝不自动顶号, 只提示到自助管理后台手动下线

use std::net::Ipv4Addr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::http::HttpClient;

/// bossWeb 门户地址(贺州学院校园网实测值)
pub const PORTAL_HOST: &str = "10.255.2.252";
/// 自助管理后台(下线指引跳转目标)
pub const SELF_SERVICE_URL: &str = "http://10.255.2.252/self/index.html#/Login";

/// 探测地址 (host, path)
pub const PROBE_URLS: [(&str, &str); 3] = [
    ("www.msftconnecttest.com", "/connecttest.txt"),
    ("connect.rom.miui.com", "/generate_204"),
    ("www.baidu.com", "/"),
];

/// 电脑端 UA (占 PC 槽)
pub const UA_PC: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 手机端真实机型池(占手机槽), 每次认证随机选择, 降低 UA 指纹识别概率
const MOBILE_DEVICES: [&str; 6] = [
    "Pixel 7",
    "Pixel 7 Pro",
    "SM-S911B",
    "SM-S916B",
    "Pixel 8",
    "SM-A546B",
];

pub fn random_mobile_ua() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs() ^ (std::process::id() as u64) << 8)
        .unwrap_or(12345);
    let idx = (seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
        % MOBILE_DEVICES.len() as u64) as usize;
    format!(
        "Mozilla/5.0 (Linux; Android 13; {}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
        MOBILE_DEVICES[idx]
    )
}

/// B 组配置字段默认值(优先从登录页动态解析, 缺失时兜底)
const BASE_FORM_DEFAULTS: [(&str, &str); 9] = [
    ("scheme", "http"),
    ("serverIp", "tomcat_server:80"),
    ("hostIp", "http://127.0.0.1:8081/"),
    ("auth_type", "0"),
    ("isBindMac1", "0"),
    ("pageid", "-1"),
    ("templatetype", "1"),
    ("listbindmac", "0"),
    ("recordmac", "0"),
];

/// 槽位冲突关键词(命中 → 绝不自动顶号, 引导自助后台手动下线)
const CONFLICT_KEYWORDS: [&str; 6] = ["已在线", "重复", "超限", "终端", "绑定", "占用"];
/// 凭据错误关键词
const BADPASS_KEYWORDS: [&str; 1] = ["密码错误"];

/// 一条认证线路(单线模式=默认线路; 多线模式=每条物理线一个)
#[derive(Debug, Clone)]
pub struct LineCfg {
    pub name: String,
    /// 出口网卡名(如 eth1); None=按源 IP/默认路由
    pub device: Option<String>,
    /// 绑定源 IP; None=自动探测
    pub source: Option<Ipv4Addr>,
    pub username: String,
    pub password: String,
    /// pc | mobile
    pub ua: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    Online,
    Captive(String),
    Unreachable(String),
    Inconclusive,
}

/// 探测单个地址
fn probe_one(client: &HttpClient, host: &str, path: &str) -> ProbeState {
    match client.request("GET", &format!("http://{host}{path}"), &[("Accept", "*/*")], None) {
        Ok(resp) => {
            if let Some(loc) = resp.header("Location") {
                if is_portal_location(loc) {
                    return ProbeState::Captive(absolute_location(loc));
                }
                return ProbeState::Inconclusive; // 如 baidu http→https 的正常 302
            }
            if resp.status == 200 || resp.status == 204 {
                return ProbeState::Online;
            }
            ProbeState::Inconclusive
        }
        Err(e) => ProbeState::Unreachable(e),
    }
}

fn is_portal_location(loc: &str) -> bool {
    if loc.starts_with('/') {
        return true;
    }
    if let Some(rest) = loc.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        return host.eq_ignore_ascii_case(PORTAL_HOST);
    }
    false
}

fn absolute_location(loc: &str) -> String {
    if loc.starts_with('/') {
        format!("http://{PORTAL_HOST}{loc}")
    } else {
        loc.to_string()
    }
}

/// 解析自定义探测地址: 允许 "http://host/path"、"host/path"、"host"(默认路径 /)。
/// 返回 None 表示留空或不合法, 调用方回退内置地址表。
pub fn parse_probe_url(s: &str) -> Option<(String, String)> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let rest = t.strip_prefix("http://").unwrap_or(t);
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    // 只允许主机名/域名/IPv4[:端口], 防止把奇怪内容拼进请求行
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

/// 本次探测要用的地址表: 配了自定义地址就只用它, 否则用内置的三个
pub fn probe_targets(custom: &str) -> Vec<(String, String)> {
    match parse_probe_url(custom) {
        Some(t) => vec![t],
        None => PROBE_URLS
            .iter()
            .map(|(h, p)| (h.to_string(), p.to_string()))
            .collect(),
    }
}

/// 并发探测地址表, 任一给出结论即返回 (Some(true)=在线 / Some(false)=被劫持 / None=无结论)。
/// custom_probe 为空 = 用内置地址; 非空且合法 = 只用用户配置的地址(判断是否联网用)
pub fn connectivity_test(client: &HttpClient, custom_probe: &str) -> (Option<bool>, String) {
    let targets = probe_targets(custom_probe);
    let (tx, rx) = mpsc::channel();
    thread::scope(|s| {
        for (host, path) in targets {
            let tx = tx.clone();
            s.spawn(move || {
                let st = probe_one(client, &host, &path);
                let _ = tx.send((host, path, st));
            });
        }
        drop(tx);
        let mut unreachable_msg = String::new();
        for (host, path, st) in rx.iter() {
            match st {
                ProbeState::Online => {
                    return (Some(true), format!("{host}{path}"));
                }
                ProbeState::Captive(loc) => {
                    return (Some(false), loc);
                }
                ProbeState::Unreachable(e) => unreachable_msg = e,
                ProbeState::Inconclusive => {}
            }
        }
        (None, unreachable_msg)
    })
}

/// 探测拿劫持 Location; 已在线返回标记
fn probe_for_login(client: &HttpClient) -> (Option<String>, bool) {
    match probe_one(client, PROBE_URLS[0].0, PROBE_URLS[0].1) {
        ProbeState::Online => return (None, true),
        ProbeState::Captive(loc) => return (Some(loc), false),
        _ => {}
    }
    for (host, path) in &PROBE_URLS[1..] {
        match probe_one(client, host, path) {
            ProbeState::Online => return (None, true),
            ProbeState::Captive(loc) => return (Some(loc), false),
            _ => continue,
        }
    }
    (None, false)
}

/// 完整认证流程. 返回 (成功?, code, detail)
/// code: already / ok / badpass / conflict / unreachable / fail
pub fn authenticate(
    line: &LineCfg,
    log: &(dyn Fn(&str) + Sync),
    timeout: Duration,
) -> (bool, &'static str, String) {
    let client = HttpClient::new(line.source, line.device.clone(), timeout);
    let ua = match line.ua.as_str() {
        "mobile" => random_mobile_ua(),
        _ => UA_PC.to_string(),
    };
    (log)(&format!(
        "=== 开始认证: 账号={} 设备={} 槽位={} 出口={:?} 源IP={:?} ===",
        line.username,
        line.name,
        line.ua,
        line.device,
        line.source
            .map(|i| i.to_string())
            .unwrap_or_else(|| "默认路由".into())
    ));
    (log)(&format!("本次 UA: {ua}"));

    // ---- 第 1 步: 探测, 拿劫持 Location ----
    let (loc, already) = probe_for_login(&client);
    if already {
        (log)("探测: 已放行, 本机已在线, 无需认证");
        return (true, "already", "已在线".into());
    }
    let Some(loc) = loc else {
        (log)("!! 探测不到门户劫持响应: 该线路可能未接入校园网");
        return (
            false,
            "unreachable",
            "探测无劫持响应, 无法取得会话参数".into(),
        );
    };
    (log)(&format!("第 1 步完成: 已取得会话绑定参数  Location = {loc}"));

    // ---- 第 2 步: GET 登录页, 种 Cookie + 动态解析字段 ----
    let resp = match client.request(
        "GET",
        &loc,
        &[
            ("User-Agent", ua.as_str()),
            ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
            ("Accept-Language", "zh-CN,zh;q=0.9"),
        ],
        None,
    ) {
        Ok(r) => r,
        Err(e) => {
            (log)(&format!("!! 获取登录页失败: {e}"));
            return (false, "unreachable", format!("门户连接失败: {e}"));
        }
    };
    if resp.status != 200 {
        (log)(&format!("!! 登录页返回 HTTP {}, 非预期", resp.status));
        return (false, "fail", format!("登录页 HTTP {}", resp.status));
    }
    let cookie = resp
        .header("Set-Cookie")
        .and_then(|c| c.split(';').next())
        .unwrap_or("")
        .trim()
        .to_string();
    (log)(&format!(
        "第 2 步完成: 会话 Cookie ({})",
        if cookie.is_empty() { "无" } else { cookie.split('=').next().unwrap_or("?") }
    ));

    let page_text = decode_body(&resp.body);
    let fields = parse_form_fields(&page_text);
    let get_field = |name: &str| -> Option<String> {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    (log)(&format!(
        "  登录页字段: hostIp={} 门户模板={}",
        get_field("hostIp").unwrap_or_default(),
        if page_text.contains("uploads/mobile/") {
            "手机"
        } else if page_text.contains("uploads/pc/") {
            "电脑"
        } else {
            "未知"
        }
    ));

    // ---- 第 3 步: POST 登录(字段名大小写敏感, 以页面原始键名为准) ----
    let mut form_pairs: Vec<(String, String)> = fields.clone();
    for (base_k, dv) in BASE_FORM_DEFAULTS {
        if !form_pairs.iter().any(|(k, _)| k.eq_ignore_ascii_case(base_k)) {
            form_pairs.push((base_k.to_string(), dv.to_string()));
        }
    }
    let tt = if line.ua == "mobile" { "2" } else { "1" };
    if let Some(pair) = form_pairs
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("templatetype"))
    {
        pair.1 = tt.to_string();
    }
    if let Some(pair) = form_pairs.iter_mut().find(|(k, _)| k == "userId") {
        pair.1 = line.username.clone();
    }
    if let Some(pair) = form_pairs.iter_mut().find(|(k, _)| k == "passwd") {
        pair.1 = line.password.clone();
    }
    let body = form_pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let keys_dbg = form_pairs
        .iter()
        .map(|(k, v)| if k.eq_ignore_ascii_case("passwd") { format!("{k}=***") } else { format!("{k}={v}") })
        .collect::<Vec<_>>()
        .join(", ");
    (log)(&format!("  表单明细: {keys_dbg}"));

    let headers = [
        ("User-Agent", ua.as_str()),
        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
        ("Accept-Language", "zh-CN,zh;q=0.9"),
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Referer", loc.as_str()),
        ("Cookie", cookie.as_str()),
    ];
    let resp = match client.request("POST", &loc, &headers, Some(&body)) {
        Ok(r) => r,
        Err(e) => {
            (log)(&format!("!! 提交认证请求失败: {e}"));
            return (false, "fail", format!("POST 失败: {e}"));
        }
    };
    (log)(&format!(
        "第 3 步完成: 认证请求已提交 (HTTP {}, {} 字节)",
        resp.status,
        resp.body.len()
    ));

    let resp_text = decode_body(&resp.body);
    let errmsg = extract_err_message(&resp_text);
    if !errmsg.is_empty() {
        (log)(&format!("  门户返回消息: {errmsg}"));
        if BADPASS_KEYWORDS.iter().any(|k| errmsg.contains(k)) {
            return (false, "badpass", errmsg);
        }
        if CONFLICT_KEYWORDS.iter().any(|k| errmsg.contains(k)) {
            (log)("!! 槽位冲突: 请到自助管理后台手动下线(本程序绝不自动顶号)");
            return (false, "conflict", errmsg);
        }
    }

    // ---- 第 4 步: 复核放行(唯一可靠判定) ----
    for attempt in 1..=2 {
        (log)(&format!("第 4 步: 等待 3 秒后复核放行 (第 {attempt}/2 次)..."));
        thread::sleep(Duration::from_secs(3));
        match probe_one(&client, PROBE_URLS[0].0, PROBE_URLS[0].1) {
            ProbeState::Online => {
                (log)("复核: 200/204 无重定向 -> 已放行");
                (log)("=== 认证成功, 已上线! ===");
                return (true, "ok", errmsg);
            }
            _ => (log)(&format!("复核第 {attempt} 次: 仍未放行, 继续等待...")),
        }
    }
    (log)("=== 认证失败: 复核仍未放行 ===");
    (
        false,
        "fail",
        if errmsg.is_empty() { "复核未放行".into() } else { errmsg },
    )
}

/// 探测某出口的源 IP: UDP connect 技巧(不发包), 配合 SO_BINDTODEVICE 可取任意网卡的地址
pub fn resolve_source(device: Option<&str>) -> Option<Ipv4Addr> {
    use socket2::SockAddr;
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .ok()?;
    #[cfg(target_os = "linux")]
    if let Some(d) = device {
        let _ = sock.bind_device(Some(d.as_bytes()));
    }
    let sa: std::net::SocketAddr = format!("{PORTAL_HOST}:80").parse().ok()?;
    sock.connect(&SockAddr::from(sa)).ok()?;
    let local = sock.local_addr().ok()?.as_socket()?;
    match local.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// body 解码: UTF-8 优先, 失败回退 GBK
pub fn decode_body(body: &[u8]) -> String {
    let (t, _, had_err) = encoding_rs::UTF_8.decode(body);
    if !had_err {
        return t.into_owned();
    }
    let (t2, _, _) = encoding_rs::GBK.decode(body);
    t2.into_owned()
}

/// 百分号编码(空格 → %20)
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 从登录页提取所有未 disabled 的 <input> 的 name -> value
/// 属性名匹配用小写副本, 属性值必须从原始文本切片(保留大小写, 门户 getParameter 大小写敏感)
pub fn parse_form_fields(html: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut i = 0usize;
    while let Some(pos) = lower[i..].find("<input") {
        let tag_start = i + pos;
        let tag_end = match lower[tag_start..].find('>') {
            Some(p) => tag_start + p,
            None => break,
        };
        let tag_raw = &html[tag_start..tag_end.min(html.len())];
        let tag_low = &lower[tag_start..tag_end.min(lower.len())];
        i = tag_end + 1;
        let attrs = split_attrs(tag_raw, tag_low);
        if attrs.iter().any(|(k, _)| *k == "disabled") {
            continue;
        }
        let mut name = None;
        let mut value = None;
        for (k, v) in &attrs {
            match *k {
                "name" => name = Some(v.to_string()),
                "value" => value = Some(v.to_string()),
                _ => {}
            }
        }
        if let Some(n) = name {
            let raw = value.unwrap_or_default();
            let decoded = raw
                .replace("&amp;", "&")
                .replace("&quot;", "\"")
                .replace("&#39;", "'");
            fields.push((n, decoded));
        }
    }
    fields
}

fn split_attrs<'a>(tag_raw: &'a str, tag_lower: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let b = tag_lower.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i] != b'=' && !(b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            if i > start {
                out.push((&tag_lower[start..i], ""));
            }
            continue;
        }
        let key = &tag_lower[start..i];
        i += 1;
        let vstart;
        if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            let quote = b[i];
            i += 1;
            vstart = i;
            while i < b.len() && b[i] != quote {
                i += 1;
            }
        } else {
            vstart = i;
            while i < b.len() && !(b[i] as char).is_whitespace() {
                i += 1;
            }
        }
        let val_end = i.min(tag_raw.len());
        let val = &tag_raw[vstart.min(tag_raw.len())..val_end];
        i += 1;
        out.push((key, val));
    }
    out
}

/// 从响应页提取隐藏域 errMessage 的 value
pub fn extract_err_message(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(p) = lower[search..].find("<input") {
        let tag_start = search + p;
        let tag_end = match lower[tag_start..].find('>') {
            Some(e) => tag_start + e,
            None => break,
        };
        let tag_lower = &lower[tag_start..=tag_end];
        let tag_raw = &html[tag_start..=tag_end];
        search = tag_end + 1;
        if tag_lower.contains("id=\"errmessage\"") || tag_lower.contains("id='errmessage'") {
            if let Some(vpos) = tag_lower.find("value=") {
                let rest = &tag_raw[vpos + 6..];
                let bytes = rest.as_bytes();
                if !bytes.is_empty() && (bytes[0] == b'"' || bytes[0] == b'\'') {
                    let q = bytes[0];
                    if let Some(end) = rest[1..].find(q as char) {
                        return rest[1..1 + end]
                            .replace("&amp;", "&")
                            .replace("&quot;", "\"");
                    }
                }
            }
            return String::new();
        }
    }
    String::new()
}
