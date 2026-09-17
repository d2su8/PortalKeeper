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
    /// 手动指定的门户地址(留空 = 只用劫持自动发现);
    /// 可填基地址 http://10.10.0.1 或从浏览器抄来的完整登录页地址
    pub portal_url: String,
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
                // 跨主机/相对路径 => 被劫持; 同主机跳转(如 baidu http→https) => 无结论
                return match captive_target(host, loc) {
                    Some(url) => ProbeState::Captive(url),
                    None => ProbeState::Inconclusive,
                };
            }
            if resp.status == 200 || resp.status == 204 {
                return ProbeState::Online;
            }
            ProbeState::Inconclusive
        }
        Err(e) => ProbeState::Unreachable(e),
    }
}

/// 取 URL 里的主机名(不含 scheme/端口/路径)
pub fn url_host(url: &str) -> &str {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let hostport = &rest[..end];
    match hostport.find(':') {
        Some(i) => &hostport[..i],
        None => hostport,
    }
}

/// 取 URL 的基地址 scheme://host[:port](不含会话参数, 用于写回配置)
pub fn url_base(url: &str) -> String {
    let scheme = if url.starts_with("https://") { "https" } else { "http" };
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    format!("{scheme}://{}", &rest[..end])
}

/// 判断 302 Location 是否属于"被门户劫持", 并归一化成可直接请求的绝对地址。
/// 不依赖任何具体学校:
/// - 相对路径 `/xxx` → 劫持, 按请求主机补全
/// - 绝对地址: 主机与请求主机相同 → 正常跳转(如 http→https); 不同 → 劫持
pub fn captive_target(requested_host: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.is_empty() {
        return None;
    }
    if loc.starts_with('/') {
        return Some(format!("http://{requested_host}{loc}"));
    }
    if let Some(rest) = loc.strip_prefix("https://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        return if host.eq_ignore_ascii_case(requested_host) {
            None
        } else {
            Some(loc.to_string())
        };
    }
    let host = url_host(loc);
    if host.is_empty() || host.eq_ignore_ascii_case(requested_host) {
        return None;
    }
    Some(loc.to_string())
}

/// 该地址是否像门户登录页(含 password 输入框或门户特征词)
pub fn looks_like_portal_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    if lower.contains("type=\"password\"") || lower.contains("type='password'") {
        return true;
    }
    ["wlanuserip", "portal", "login.do", "self/index", "bossweb", "石斧"]
        .iter()
        .any(|k| lower.contains(k))
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

/// 登录流程用的探测地址表: 默认内置 3 个;
/// 环境变量 CAMPUS_AUTH_PROBE 可覆盖(逗号分隔 host[:port]/path), 供测试或自定义
/// (与桌面版同名同格式, 便于用同一套假门户做端到端验证)。
pub fn login_probe_targets() -> Vec<(String, String)> {
    if let Ok(v) = std::env::var("CAMPUS_AUTH_PROBE") {
        let list: Vec<(String, String)> = v
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                match s.find('/') {
                    Some(i) => Some((s[..i].to_string(), s[i..].to_string())),
                    None => Some((s.to_string(), "/".to_string())),
                }
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    PROBE_URLS
        .iter()
        .map(|(h, p)| (h.to_string(), p.to_string()))
        .collect()
}

/// 探测拿劫持 Location; 已在线返回标记。
/// 没有劫持响应时回退到配置里的门户地址(portal_url), 都没有则返回 None。
fn probe_for_login(client: &HttpClient, portal_url: &str, log: &(dyn Fn(&str) + Sync)) -> (Option<String>, bool) {
    for (host, path) in login_probe_targets() {
        match probe_one(client, &host, &path) {
            ProbeState::Online => return (None, true),
            ProbeState::Captive(loc) => return (Some(loc), false),
            _ => continue,
        }
    }
    if let Some(p) = parse_portal_url(portal_url) {
        (log)(&format!("探测不到劫持响应, 改用配置的门户地址: {p}"));
        return (Some(p), false);
    }
    (None, false)
}

/// 校验并归一化手动填写的门户地址: 允许 http://host[/path...] 或裸 host/path
pub fn parse_portal_url(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    let host = url_host(&with_scheme);
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return None;
    }
    Some(with_scheme)
}

/// 跟随跳转抓取登录页, 返回 (最终 URL, HTML, Cookie)。最多 3 跳。
/// 相对 Location 按当前 URL 补全, 因此不依赖任何写死的门户地址。
fn fetch_login_page(
    client: &HttpClient,
    start_url: &str,
    ua: &str,
    log: &(dyn Fn(&str) + Sync),
) -> Result<(String, String, String), String> {
    let mut url = start_url.to_string();
    for hop in 0..3 {
        let resp = client.request(
            "GET",
            &url,
            &[
                ("User-Agent", ua),
                ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
                ("Accept-Language", "zh-CN,zh;q=0.9"),
            ],
            None,
        )?;
        if let Some(loc) = resp.header("Location") {
            let l = loc.trim();
            let next = if l.starts_with('/') {
                format!("{}{}", url_base(&url), l)
            } else if l.starts_with("http://") || l.starts_with("https://") {
                l.to_string()
            } else {
                format!("{}/{}", url.trim_end_matches('/'), l)
            };
            (log)(&format!("  跳转 {url} -> {next}"));
            url = next;
            if hop == 2 {
                return Err("跳转次数过多, 未拿到登录页".into());
            }
            continue;
        }
        if resp.status != 200 {
            return Err(format!("登录页 HTTP {}", resp.status));
        }
        let html = decode_body(&resp.body);
        let cookie = resp
            .header("Set-Cookie")
            .and_then(|c| c.split(';').next())
            .unwrap_or("")
            .trim()
            .to_string();
        return Ok((url, html, cookie));
    }
    Err("未拿到登录页".into())
}

/// 只做"找认证服务器": 探测劫持(或验证手填的门户地址), 返回门户基地址。
/// 用于 `detect-portal` 子命令, 不登录、不改任何状态。
pub fn detect_portal(
    client: &HttpClient,
    portal_url: &str,
    log: &(dyn Fn(&str) + Sync),
) -> Option<String> {
    for (host, path) in login_probe_targets() {
        if let ProbeState::Captive(url) = probe_one(client, &host, &path) {
            (log)(&format!("探测 {host}{path} -> 302 被劫持: {url}"));
            let base = url_base(&url);
            match fetch_login_page(client, &url, UA_PC, log) {
                Ok((final_url, html, _)) => (log)(&format!(
                    "  登录页{} (最终地址 {final_url})",
                    if looks_like_portal_page(&html) { "正常" } else { "内容不像门户登录页" }
                )),
                Err(e) => (log)(&format!("  登录页抓取失败: {e}")),
            }
            return Some(base);
        }
    }
    if let Some(p) = parse_portal_url(portal_url) {
        (log)(&format!("无劫持响应, 验证手填的门户地址: {p}"));
        if let Ok((final_url, html, _)) = fetch_login_page(client, &p, UA_PC, log) {
            (log)(&format!(
                "  可达, {} (最终地址 {final_url})",
                if looks_like_portal_page(&html) { "像门户登录页" } else { "内容不像门户登录页" }
            ));
            return Some(url_base(&final_url));
        }
    }
    None
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
    let (loc, already) = probe_for_login(&client, &line.portal_url, log);
    if already {
        (log)("探测: 已放行, 本机已在线, 无需认证");
        return (true, "already", "已在线".into());
    }
    let Some(loc) = loc else {
        (log)("!! 探测不到门户劫持响应, 且未配置门户地址(portal_url)");
        (log)("   手动获取办法: 浏览器打开任意 http 网站(如 http://www.msftconnecttest.com/connecttest.txt),");
        (log)("   地址栏会跳到校园网认证页 — 把那个地址整条填进 LuCI「认证服务 → 门户地址」或用 uci 设置 portal_url");
        return (
            false,
            "no-portal",
            "未探测到门户, 请手动填写门户地址(portal_url)".into(),
        );
    };
    (log)(&format!("第 1 步完成: 已取得会话绑定参数  Location = {loc}"));

    // ---- 第 2 步: GET 登录页(跟随跳转), 种 Cookie + 动态解析字段 ----
    let (loc, page_text, cookie) = match fetch_login_page(&client, &loc, &ua, log) {
        Ok(v) => v,
        Err(e) => {
            (log)(&format!("!! 获取登录页失败: {e}"));
            return (false, "unreachable", format!("门户连接失败: {e}"));
        }
    };
    (log)(&format!(
        "第 2 步完成: 会话 Cookie ({})",
        if cookie.is_empty() { "无" } else { cookie.split('=').next().unwrap_or("?") }
    ));
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
    // 账号/密码字段名按登录页推断(老板牌是 userId/passwd, 别家门户可能叫 account/pwd),
    // 推不出来才回退固定名 —— 换学校不用改代码
    let (uf, pf) = detect_credential_fields(&parse_form_inputs(&page_text));
    let user_field = uf.unwrap_or_else(|| "userId".to_string());
    let pass_field = pf.unwrap_or_else(|| "passwd".to_string());
    (log)(&format!("  表单身份字段: 账号={user_field} 密码={pass_field}"));
    if let Some(pair) = form_pairs.iter_mut().find(|(k, _)| *k == user_field) {
        pair.1 = line.username.clone();
    } else {
        form_pairs.push((user_field.clone(), line.username.clone()));
    }
    if let Some(pair) = form_pairs.iter_mut().find(|(k, _)| *k == pass_field) {
        pair.1 = line.password.clone();
    } else {
        form_pairs.push((pass_field.clone(), line.password.clone()));
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
        // 复核用与探测同一批地址(默认即内置第一个), 换过探测地址时行为一致
        let (rh, rp) = login_probe_targets()
            .into_iter()
            .next()
            .unwrap_or_else(|| (PROBE_URLS[0].0.to_string(), PROBE_URLS[0].1.to_string()));
        match probe_one(&client, &rh, &rp) {
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

/// 按文档顺序解析登录页 <input>, 返回 (name, type, value)。
/// 各家门户账号/密码字段名不同, 需要按 type=password 和可见文本输入的先后顺序推断。
pub fn parse_form_inputs(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
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
        let mut value = String::new();
        let mut typ = "text".to_string();
        for (k, v) in &attrs {
            match *k {
                "name" => name = Some(v.to_string()),
                "value" => {
                    value = v
                        .replace("&amp;", "&")
                        .replace("&quot;", "\"")
                        .replace("&#39;", "'")
                }
                "type" => typ = v.to_ascii_lowercase(),
                _ => {}
            }
        }
        if let Some(n) = name {
            if !n.is_empty() {
                out.push((n, typ, value));
            }
        }
    }
    out
}

/// 从表单输入推断 (账号字段名, 密码字段名):
/// - 密码: 第一个 type=password 的输入
/// - 账号: 密码框之前最近的一个可见文本输入; 找不到再按名称特征(user/account/login/name/id)找
pub fn detect_credential_fields(
    inputs: &[(String, String, String)],
) -> (Option<String>, Option<String>) {
    let visible_text = |t: &str| {
        !matches!(
            t,
            "hidden" | "submit" | "button" | "reset" | "image" | "checkbox" | "radio" | "file"
        )
    };
    let pass_idx = inputs.iter().position(|(_, t, _)| t == "password");
    let pass = pass_idx.map(|i| inputs[i].0.clone());
    let user = match pass_idx {
        Some(pi) => inputs[..pi]
            .iter()
            .rev()
            .find(|(_, t, _)| visible_text(t))
            .map(|(n, _, _)| n.clone()),
        None => None,
    };
    let user = user.or_else(|| {
        inputs
            .iter()
            .filter(|(_, t, _)| visible_text(t))
            .find(|(n, _, _)| {
                let n = n.to_ascii_lowercase();
                ["user", "account", "login", "name", "id"]
                    .iter()
                    .any(|k| n.contains(k))
            })
            .map(|(n, _, _)| n.clone())
    });
    (user, pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_host_redirect_is_not_captive() {
        assert_eq!(captive_target("www.baidu.com", "https://www.baidu.com/"), None);
        assert_eq!(captive_target("www.baidu.com", "http://www.baidu.com/index.html"), None);
    }

    #[test]
    fn other_school_portal_is_captive() {
        assert_eq!(
            captive_target("www.msftconnecttest.com", "http://10.10.0.1/portal/login?wlanuserip=1.2.3.4").as_deref(),
            Some("http://10.10.0.1/portal/login?wlanuserip=1.2.3.4")
        );
        assert!(captive_target("connect.rom.miui.com", "http://10.0.0.9:8080/portal").is_some());
        assert!(captive_target("a.example.com", "http://auth.school.edu.cn/wifi/login").is_some());
    }

    #[test]
    fn relative_location_resolved_against_requested_host() {
        assert_eq!(
            captive_target("www.msftconnecttest.com", "/portal/login?mac=aabbccddeeff").as_deref(),
            Some("http://www.msftconnecttest.com/portal/login?mac=aabbccddeeff")
        );
    }

    #[test]
    fn url_helpers() {
        assert_eq!(url_host("http://10.10.0.1:8080/portal/login?x=1"), "10.10.0.1");
        assert_eq!(url_base("http://10.10.0.1:8080/portal/login?x=1"), "http://10.10.0.1:8080");
        assert_eq!(url_base("http://10.255.2.252/x?y=1"), "http://10.255.2.252");
    }

    #[test]
    fn portal_url_parsing() {
        assert_eq!(parse_portal_url("  "), None);
        assert_eq!(parse_portal_url("http://10.255.2.252"), Some("http://10.255.2.252".into()));
        assert_eq!(
            parse_portal_url("10.10.0.1/portal/login?wlanuserip=1.2.3.4").as_deref(),
            Some("http://10.10.0.1/portal/login?wlanuserip=1.2.3.4")
        );
        assert_eq!(parse_portal_url("http:// bad url/"), None);
    }

    #[test]
    fn credential_field_detection() {
        // 老板牌: userId/passwd
        let h1 = r#"<input type="hidden" name="wlanuserip" value="1.2.3.4"><input type="text" name="userId"><input type="password" name="passwd">"#;
        assert_eq!(
            detect_credential_fields(&parse_form_inputs(h1)),
            (Some("userId".into()), Some("passwd".into()))
        );
        // 别家: account/pwd, 且账号框在密码框之前
        let h2 = r#"<input name="nasid" type="hidden"><input name="account" type="text"><input name="pwd" type="password">"#;
        assert_eq!(
            detect_credential_fields(&parse_form_inputs(h2)),
            (Some("account".into()), Some("pwd".into()))
        );
        // 没有 password 框时只按名称特征兜底
        let h3 = r#"<input name="username" type="text">"#;
        assert_eq!(
            detect_credential_fields(&parse_form_inputs(h3)),
            (Some("username".into()), None)
        );
    }

    #[test]
    fn portal_page_detection() {
        assert!(looks_like_portal_page(r#"<input type="password" name="pwd">"#));
        assert!(looks_like_portal_page("wlanuserip=1.2.3.4"));
        assert!(!looks_like_portal_page("<html><body>hello</body></html>"));
    }

    #[test]
    fn probe_targets_custom() {
        // uci 的 probe_url 是单个地址: 填了就用它, 留空/非法回退内置表
        assert_eq!(probe_targets("").len(), PROBE_URLS.len());
        assert_eq!(probe_targets("http:// bad url/").len(), PROBE_URLS.len());
        let t = probe_targets("10.0.0.1/portal");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0], ("10.0.0.1".to_string(), "/portal".to_string()));
    }
}
