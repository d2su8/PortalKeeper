//! portalkeeperd —— PortalKeeper 校园网自动认证 (OpenWrt 守护进程 + CLI)
//!
//! 子命令:
//!   daemon   守护循环: 周期探测/认证/保活, 状态写 /tmp/portalkeeper.status, 日志写 /tmp/portalkeeper.log
//!   check    联通测试: 仅探测认证状态, 不登录 (退出码 0=已在线 1=未认证/失败 2=无法判定)
//!   login    手动执行一次完整认证(探测→登录→复核)
//!   status   打印守护进程写入的线路状态
//!   detect-portal  只探测门户地址(不登录, 排查用)
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

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

fn usage() {
    println!(
        "portalkeeperd {} —— PortalKeeper 校园网自动认证 (OpenWrt)\n\
用法:\n\
  portalkeeperd daemon            启动守护进程(读 /etc/config/portalkeeper)\n\
  portalkeeperd check             联通测试: 仅探测, 不登录(0=已在线 1=未认证/失败 2=无法判定)\n\
  portalkeeperd login             手动执行一次完整认证\n\
  portalkeeperd offline           强制下线本机(新版门户令牌 API: 拿令牌→查本机→下线)\n\
  portalkeeperd status            查看守护进程线路状态(/tmp/portalkeeper.status)\n\
  portalkeeperd detect-portal     只探测门户地址(不登录); 探测不到会给出手动获取办法\n\
选项(check/login 可用):\n\
  --config PATH                   UCI 配置文件(默认 /etc/config/portalkeeper)\n\
  --line main|extra               选择认证线路
\
  --source IP                     覆盖源 IP\n\
  --device NAME                   覆盖出口网卡(如 eth1)\n\
  --ua pc|mobile                  覆盖设备槽位\n\
  --portal URL                    手动指定门户地址(留空=靠 302 劫持自动发现)\n\
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
        "offline" => cli_offline(rest),
        "status" => cli_status(),
        "detect-portal" => cli_detect_portal(rest),
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
    let main = cfg.first("portalkeeper");
    let get_main = |key| {
        main.and_then(|section| uci::get(section, key))
            .unwrap_or("")
    };
    let line_arg = arg_value(rest, "--line");
    let extra = cfg.sections_of("line").into_iter().find(|section| {
        section.name.as_deref() == Some("extra") && uci::get(section, "enabled") == Some("1")
    });
    let line = match line_arg.as_deref() {
        Some("main") => None,
        Some("extra") if extra.is_some() => extra,
        Some(other) => {
            eprintln!("线路不可用: {other}");
            exit(2);
        }
        None => extra,
    };
    let get_line = |key| {
        line.and_then(|section| uci::get(section, key))
            .filter(|v| !v.is_empty())
    };
    let inherited = |key| get_line(key).unwrap_or_else(|| get_main(key));
    let device = arg_value(rest, "--device")
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let configured = if line.is_some() {
                get_line("device").unwrap_or("")
            } else {
                get_main("device")
            };
            (!configured.is_empty()).then(|| configured.to_string())
        });
    let source = arg_value(rest, "--source")
        .or_else(|| {
            if rest.iter().any(|a| a == "--device") {
                return None;
            }
            let configured = if line.is_some() {
                get_line("source").unwrap_or("")
            } else {
                get_main("source")
            };
            (!configured.is_empty()).then(|| configured.to_string())
        })
        .and_then(|ip| ip.parse().ok())
        .or_else(|| portal::resolve_source(device.as_deref()));
    let ua = arg_value(rest, "--ua")
        .filter(|v| v == "pc" || v == "mobile")
        .unwrap_or_else(|| {
            get_line("ua")
                .unwrap_or_else(|| {
                    if line.is_some() {
                        "mobile"
                    } else {
                        get_main("ua")
                    }
                })
                .to_string()
        });
    let trusted_portal =
        arg_value(rest, "--portal").unwrap_or_else(|| inherited("trusted_portal").to_string());
    let portal_url = if trusted_portal.is_empty() {
        get_main("portal_url").to_string()
    } else {
        trusted_portal.clone()
    };
    let probe_url = get_main("probe_url").to_string();
    (
        portal::LineCfg {
            name: if line.is_some() { "extra" } else { "main" }.into(),
            portal_url,
            trusted_portal,
            probe_url: probe_url.clone(),
            device,
            source,
            username: inherited("username").to_string(),
            password: inherited("password").to_string(),
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
        if portal::has_custom_probe(&probe_url) {
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
    let log = |s: &str| println!("{s}");
    let (ok, code, detail) = portal::authenticate(&line, &log, timeout);
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

/// 只探测门户地址(排查用): 有劫持就用劫持地址, 没有则验证手填的 portal_url; 不登录
fn cli_detect_portal(rest: &[String]) {
    let (line, _) = resolve_line(rest);
    let client = http::HttpClient::new(
        line.source,
        line.device.clone(),
        std::time::Duration::from_secs(timeout_of(rest)),
    );
    let log = |s: &str| println!("{s}");
    println!(
        "检测门户开始: 设备={} 源IP={} 手填门户={}",
        line.device.as_deref().unwrap_or("自动"),
        line.source
            .map(|i| i.to_string())
            .unwrap_or_else(|| "自动检测".into()),
        if line.portal_url.trim().is_empty() {
            "(未填)".to_string()
        } else {
            line.portal_url.clone()
        }
    );
    match portal::detect_portal(&client, &line.portal_url, &log) {
        Some(base) => println!(
            "发现门户: {base}\n(登录页 GET 正常; 可 uci set portalkeeper.main.portal_url='{base}' 固化)"
        ),
        None => {
            println!("未发现门户: 没有劫持响应, 手填的门户地址也验证不通过(本机可能已在线或不在校园网)");
            println!("手动获取: 浏览器打开 http://www.msftconnecttest.com/connecttest.txt ,");
            println!("地址栏跳转后的地址整条填进 LuCI「认证服务 → 门户地址」即可。");
            exit(2);
        }
    }
}

/// 强制下线本机(新版门户令牌 API): 拿令牌 → 查在线列表 → forceOffline
fn cli_offline(rest: &[String]) {
    let (line, _) = resolve_line(rest);
    if line.username.is_empty() || line.password.is_empty() {
        eprintln!("!! 缺少账号或密码: 请在 LuCI 设置或 /etc/config/portalkeeper 中配置");
        exit(1);
    }
    let log = |s: &str| println!("{s}");
    match portal::force_offline(
        &line,
        &log,
        std::time::Duration::from_secs(timeout_of(rest)),
    ) {
        Ok(msg) => {
            println!("结论: {msg}");
            exit(0);
        }
        Err(e) => {
            println!(
                "结论: {}",
                portal::redact_credentials(&e, &line.username, &line.password)
            );
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
