//! portalkeeperd —— PortalKeeper 校园网自动认证 (OpenWrt 守护进程 + CLI)
//!
//! 子命令:
//!   daemon   守护循环: 周期探测/认证/保活, 状态写 /tmp/portalkeeper.status, 日志写 /tmp/portalkeeper.log
//!   check    联通测试: 仅探测认证状态, 不登录 (退出码 0=已在线 1=未认证/失败 2=无法判定)
//!   login    手动执行一次完整认证(探测→登录→复核)
//!   status   打印守护进程写入的线路状态
//!
//! 门户认证协议(逐字段实机验证, 与同系列桌面版同源):
//!   未认证时 AC 302 劫持任意 HTTP → 向带 query 的门户 URL POST B 组字段+账号密码
//!   → 复核探测无 Location 即上线。设备槽位由 User-Agent 决定(pc/mobile)。

mod daemon;
mod http;
mod logging;
mod portal;
mod uci;

use std::process::exit;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn usage() {
    println!(
        "portalkeeperd {} —— PortalKeeper 校园网自动认证 (OpenWrt)\n\
用法:\n\
  portalkeeperd daemon            启动守护进程(读 /etc/config/portalkeeper)\n\
  portalkeeperd check             联通测试: 仅探测, 不登录(0=已在线 1=未认证/失败 2=无法判定)\n\
  portalkeeperd login             手动执行一次完整认证\n\
  portalkeeperd status            查看守护进程线路状态(/tmp/portalkeeper.status)\n\
选项(check/login 可用):\n\
  --config PATH                   UCI 配置文件(默认 /etc/config/portalkeeper)\n\
  --source IP                     覆盖源 IP\n\
  --device NAME                   覆盖出口网卡(如 eth1)\n\
  --ua pc|mobile                  覆盖设备槽位\n\
  --timeout SECS                  单请求超时(默认 5)\n\
说明:\n\
  - 必需配置: uci 的 portalkeeper.main.username + password —— 缺任一项服务就不启动,\n\
    并把缺什么写进日志; 其余项都能留空(device 自动检测网卡, 间隔/次数用默认值, probe_url 用内置地址)\n\
  - 日志只写 /tmp(tmpfs), 不落闪存, 避免 NAND 磨损; 重启自动清空\n\
  - 判断「是否联网」的探测网站可用 uci 的 portalkeeper.main.probe_url 指定, 留空用内置地址\n\
  - 多线多拨在 /etc/config/portalkeeper 的 multi_line + line 段配置, 默认关闭",
        VERSION
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rest: &[String] = if args.is_empty() { &[] } else { &args[1..] };
    match cmd {
        "daemon" => daemon::run(rest),
        "check" => cli_check(rest),
        "login" => cli_login(rest),
        "status" => cli_status(),
        "version" | "--version" | "-v" => println!("portalkeeperd {}", VERSION),
        "help" | "--help" | "-h" => usage(),
        _ => {
            usage();
            exit(2);
        }
    }
}

/// 从参数+配置解析出一条线路(参数优先, 其次第一条启用的 line, 最后回退 main 段)。
/// 返回 (线路, 自定义探测网站) —— 后者空串表示用内置地址表
fn resolve_line(rest: &[String]) -> (portal::LineCfg, String) {
    let cfg = uci::Config::load(
        &arg_value(rest, "--config").unwrap_or_else(|| "/etc/config/portalkeeper".to_string()),
    );
    let main_sec = cfg.first("portalkeeper");

    // 参数覆盖
    let arg_source = arg_value(rest, "--source").and_then(|s| s.parse().ok());
    let arg_device = arg_value(rest, "--device");
    let arg_ua = arg_value(rest, "--ua").filter(|s| s == "pc" || s == "mobile");

    let line = cfg
        .sections_of("line")
        .into_iter()
        .find(|s| uci::get(s, "enabled").unwrap_or("1") != "0");

    let pick = |from_line: (Option<String>, Option<String>, Option<String>, Option<String>),
                from_main: (Option<String>, Option<String>, Option<String>, Option<String>)| {
        // (username, password, ua, device) 三级: 参数 > line > main
        let (lu, lp, lua, ldev) = from_line;
        let (mu, mp, mua, mdev) = from_main;
        (
            lu.or(mu).unwrap_or_default(),
            lp.or(mp).unwrap_or_default(),
            arg_ua.or(lua).or(mua).unwrap_or_else(|| "pc".into()),
            arg_device.or(ldev).or(mdev),
        )
    };

    let (username, password, ua, device) = match line {
        Some(sec) => pick(
            (
                uci::get(sec, "username").map(|s| s.to_string()).filter(|s| !s.is_empty()),
                uci::get(sec, "password").map(|s| s.to_string()).filter(|s| !s.is_empty()),
                uci::get(sec, "ua").map(|s| s.to_string()),
                uci::get(sec, "device").map(|s| s.to_string()).filter(|s| !s.is_empty()),
            ),
            (
                main_sec.and_then(|m| uci::get(m, "username")).map(|s| s.to_string()),
                main_sec.and_then(|m| uci::get(m, "password")).map(|s| s.to_string()),
                main_sec.and_then(|m| uci::get(m, "ua")).map(|s| s.to_string()),
                main_sec
                    .and_then(|m| uci::get(m, "device"))
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
            ),
        ),
        None => pick(
            (None, None, None, None),
            (
                main_sec.and_then(|m| uci::get(m, "username")).map(|s| s.to_string()),
                main_sec.and_then(|m| uci::get(m, "password")).map(|s| s.to_string()),
                main_sec.and_then(|m| uci::get(m, "ua")).map(|s| s.to_string()),
                main_sec
                    .and_then(|m| uci::get(m, "device"))
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
            ),
        ),
    };

    let source = match arg_source {
        Some(ip) => Some(ip),
        None => portal::resolve_source(device.as_deref()),
    };

    let probe_url = main_sec
        .and_then(|m| uci::get(m, "probe_url"))
        .unwrap_or("")
        .to_string();

    (
        portal::LineCfg {
            name: "cli".into(),
            device,
            source,
            username,
            password,
            ua,
        },
        probe_url,
    )
}

fn arg_value(rest: &[String], key: &str) -> Option<String> {
    rest.iter()
        .position(|a| a == key)
        .and_then(|i| rest.get(i + 1))
        .cloned()
}

fn timeout_of(rest: &[String]) -> u64 {
    arg_value(rest, "--timeout")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
        .clamp(2, 30)
}

fn cli_check(rest: &[String]) {
    let (line, probe_url) = resolve_line(rest);
    let client = http::HttpClient::new(
        line.source,
        line.device.clone(),
        std::time::Duration::from_secs(timeout_of(rest)),
    );
    println!(
        "联通测试开始: 设备={} 源IP={} 槽位={} 探测网站={}",
        line.device.as_deref().unwrap_or("自动"),
        line.source
            .map(|i| i.to_string())
            .unwrap_or_else(|| "自动检测".into()),
        line.ua,
        if portal::parse_probe_url(&probe_url).is_some() {
            probe_url.trim()
        } else {
            "内置(默认)"
        }
    );
    let (verdict, detail) = portal::connectivity_test(&client, &probe_url);
    match verdict {
        Some(true) => {
            println!("结论: 已联网(已认证), 无需登录 — {}", detail);
            exit(0);
        }
        Some(false) => {
            println!("结论: 未认证(被门户劫持), 可执行登录 — {}", detail);
            exit(1);
        }
        None => {
            println!("结论: 无法判定, 网络可能不通 — {}", detail);
            exit(2);
        }
    }
}

fn cli_login(rest: &[String]) {
    let (line, _probe_url) = resolve_line(rest);
    if line.username.is_empty() || line.password.is_empty() {
        eprintln!("!! 缺少账号或密码: 请在 LuCI 设置或 /etc/config/portalkeeper 中配置");
        exit(1);
    }
    let timeout = std::time::Duration::from_secs(timeout_of(rest));
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let out2 = out.clone();
    let log = move |s: &str| {
        println!("{}", s);
        out2.lock().unwrap().push(s.to_string());
    };
    let (ok, code, detail) = portal::authenticate(&line, &log, timeout);
    let _ = out;
    match (ok, code) {
        (true, "already") => {
            println!("结论: 本机已在线, 无需操作");
            exit(0);
        }
        (true, _) => {
            println!("结论: 认证成功");
            exit(0);
        }
        (false, "conflict") => {
            println!("结论: 槽位被占用, 请打开自助管理后台手动下线后重试");
            exit(3);
        }
        (false, "badpass") => {
            println!("结论: 密码错误");
            exit(1);
        }
        _ => {
            println!("结论: 认证失败 ({})", detail);
            exit(1);
        }
    }
}

fn cli_status() {
    match std::fs::read_to_string(daemon::STATUS_PATH) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                println!("更新时间: {}", v["updated_human"].as_str().unwrap_or("?"));
                if let Some(lines) = v["lines"].as_array() {
                    if lines.is_empty() {
                        println!("(无线路配置)");
                    }
                    for l in lines {
                        println!(
                            "[{}/{}] 设备={:?} 源IP={:?} 槽位={} 状态={} — {}",
                            l["role"].as_str().unwrap_or("?"),
                            l["name"].as_str().unwrap_or("?"),
                            l["device"].as_str(),
                            l["source"].as_str(),
                            l["ua"].as_str().unwrap_or("?"),
                            l["state"].as_str().unwrap_or("?"),
                            l["detail"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            Err(e) => {
                println!("状态文件损坏: {e}");
                exit(2);
            }
        },
        Err(_) => {
            println!("暂无状态文件({}) — 守护进程未运行?", daemon::STATUS_PATH);
            exit(2);
        }
    }
}
