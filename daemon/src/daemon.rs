//! 守护循环(OpenWrt 校园网自动认证):
//! - 启动时立即做一轮「探测→(被劫持时)认证」
//! - 可选「掉线自动重试」: 每 retry_interval 秒探测一次, 发现掉线立即重试认证,
//!   连续失败 retry_count 次后暂停(标记 exhausted), 等待手动触发/重启服务重置
//! - 不做周期性发包保活(实测: 校园网只要还有流量就不会被空闲超时下线)
//! - 状态写 /tmp/portalkeeper.status(原子), 日志写 /tmp/portalkeeper.log(tmpfs), 均不落闪存
//! - 服务启停也进同一份日志: 本进程记「服务启动」与「守护进程退出」(捕获 SIGTERM),
//!   init 脚本记「服务停止」与「未启用/二进制缺失」, 两边合起来是完整生命周期

use std::fs;
use std::net::Ipv4Addr;
use std::process::exit;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::logging;
use crate::portal::{self, LineCfg};
use crate::uci;

pub const STATUS_PATH: &str = "/tmp/portalkeeper.status";
pub const TRIGGER_PATH: &str = "/tmp/portalkeeper.trigger";
const LOG_PATH: &str = "/tmp/portalkeeper.log";
const LOG_MAX_BYTES: u64 = 256 * 1024;
const TICK_SECS: u64 = 1;

/// 停止信号暂存。信号处理函数只写这个原子量(异步信号安全: 不做分配、不碰 IO),
/// 主循环每秒检查一次, 由主循环写日志后正常退出。
static STOP_SIG: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_stop_signal(sig: libc::c_int) {
    STOP_SIG.store(sig as i32, Ordering::SeqCst);
}

/// 捕获 SIGTERM/SIGINT(procd 停止或重启服务、Ctrl+C)。
/// 不捕获的话进程被默认动作直接杀死, 日志里就留不下退出的记录。
fn install_signal_handlers() {
    unsafe {
        let h = on_stop_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, h);
        libc::signal(libc::SIGINT, h);
    }
}

fn stop_pending() -> bool {
    STOP_SIG.load(Ordering::SeqCst) != 0
}

/// 可被信号打断的一拍等待。收到 SIGTERM 时 nanosleep 立刻返回 EINTR(信号处理函数
/// 已置位), 于是同一拍就能写退出日志 — 不用等满一秒, 退出记录也就不会排到
/// 重启后新进程的「服务启动」后面。
fn sleep_tick() {
    let ts = libc::timespec {
        tv_sec: TICK_SECS as _,
        tv_nsec: 0,
    };
    unsafe {
        libc::nanosleep(&ts, std::ptr::null_mut());
    }
}

/// 有停止请求时写一条退出日志并返回 true(调用方随即 exit(0):
/// 退出码 0 视为正常结束, procd 不会再按 respawn 规则把进程拉起来)
fn stop_requested(started: u64) -> bool {
    let sig = STOP_SIG.load(Ordering::SeqCst);
    if sig == 0 {
        return false;
    }
    let name = match sig {
        libc::SIGTERM => "SIGTERM(服务停止或重启)",
        libc::SIGINT => "SIGINT(Ctrl+C)",
        _ => "停止信号",
    };
    logging::log(&format!(
        "守护进程退出: 收到 {}, PID {}, 本次运行时长 {}",
        name,
        std::process::id(),
        human_duration(epoch_secs().saturating_sub(started))
    ));
    true
}

/// 秒数 → "1d 2h 3m 4s"(零值段省略), 仅用于日志里的运行时长
fn human_duration(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        parts.push(format!("{d}d"));
    }
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s}s"));
    }
    parts.join(" ")
}

struct LineRun {
    /// uci 段名(主线路 main / 扩展线路 extra)
    name: String,
    /// 手动门户地址(取自主段 portal_url, 两条线路共用)
    portal_url: String,
    role: &'static str,
    device: Option<String>,
    source: Option<Ipv4Addr>,
    ua: String,
    username: String,
    password: String,
    state: String,
    detail: String,
    fail_count: u32,
    exhausted: bool,
}

pub fn run(rest: &[String]) {
    let cfg_path = rest
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| rest.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("/etc/config/portalkeeper");

    logging::init(LOG_PATH, LOG_MAX_BYTES);
    install_signal_handlers();
    let started = epoch_secs();

    let cfg = uci::Config::load(cfg_path);
    let main = cfg.first("portalkeeper");
    let auto_retry = main.and_then(|m| uci::get(m, "auto_retry")).unwrap_or("1") == "1";
    let retry_interval: u64 = main
        .and_then(|m| uci::get(m, "retry_interval"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(60)
        .clamp(15, 86_400);
    let retry_count: u32 = main
        .and_then(|m| uci::get(m, "retry_count"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .clamp(1, 100);
    let main_user = main.and_then(|m| uci::get(m, "username")).unwrap_or("");
    let main_pass = main.and_then(|m| uci::get(m, "password")).unwrap_or("");
    let main_ua = main.and_then(|m| uci::get(m, "ua")).unwrap_or("pc");
    // 自定义探测网站(空 = 内置地址表), 只影响「是否联网」的判定
    let probe_url = main
        .and_then(|m| uci::get(m, "probe_url"))
        .unwrap_or("")
        .to_string();
    // 手动填写的门户地址(空 = 只用 302 劫持自动发现); 换学校时不用改代码
    let portal_url = main
        .and_then(|m| uci::get(m, "portal_url"))
        .unwrap_or("")
        .to_string();

    // 线路 1: 主 WAN(基本设置), 段名 main
    let mut lines: Vec<LineRun> = vec![LineRun {
        name: "main".into(),
        portal_url: portal_url.clone(),
        role: "主 WAN",
        device: main
            .and_then(|m| uci::get(m, "device"))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        source: None,
        ua: main_ua.to_string(),
        username: main_user.to_string(),
        password: main_pass.to_string(),
        state: "init".into(),
        detail: "尚未检测".into(),
        fail_count: 0,
        exhausted: false,
    }];
    // 源 IP: 显式配置优先, 否则按网卡自动探测
    if let Some(src) = main
        .and_then(|m| uci::get(m, "source"))
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<Ipv4Addr>().ok())
    {
        lines[0].source = Some(src);
    } else {
        let dev = lines[0].device.clone();
        lines[0].source = portal::resolve_source(dev.as_deref());
    }

    // 线路 2(高级设置): 段名 extra, enabled=1 才生效; 账号密码独立
    if let Some(sec) = cfg
        .sections_of("line")
        .into_iter()
        .find(|s| s.name.as_deref() == Some("extra"))
    {
        if uci::get(sec, "enabled").unwrap_or("0") == "1" {
            let device = uci::get(sec, "device")
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let source = uci::get(sec, "source")
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<Ipv4Addr>().ok())
                .or_else(|| portal::resolve_source(device.as_deref()));
            lines.push(LineRun {
                name: "extra".into(),
                portal_url: portal_url.clone(),
                role: "扩展线路",
                device,
                source,
                ua: uci::get(sec, "ua")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "mobile".into()),
                // 留空 = 沿用主线路的账号密码(与 LuCI 里的提示一致)
                username: uci::get(sec, "username")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| lines[0].username.clone()),
                password: uci::get(sec, "password")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| lines[0].password.clone()),
                state: "init".into(),
                detail: "尚未检测".into(),
                fail_count: 0,
                exhausted: false,
            });
        }
    }

    // 启动前置检查: 主线路必须配好账号与密码。缺了就不启动(不拦保存、不报到错),
    // 把缺什么写进日志 —— 参数补齐后重启服务即可, 清空自己的设置也不会被表单挡住。
    let mut missing: Vec<&str> = Vec::new();
    if lines[0].username.trim().is_empty() {
        missing.push("账号(username)");
    }
    if lines[0].password.is_empty() {
        missing.push("密码(password)");
    }
    if !missing.is_empty() {
        logging::log(&format!(
            "服务未启动: 主线路缺少必需配置 {} —— 认证需要账号+密码, 其余参数都可留空(留空即自动检测/用默认值)。\
             配置方式: ① LuCI「概览 → 认证服务」填好后点「保存并应用」; \
             ② 或命令行 uci set portalkeeper.main.username='账号' / uci set portalkeeper.main.password='密码' \
             后 uci commit portalkeeper && /etc/init.d/portalkeeper restart。补齐前守护进程不会启动。",
            missing.join(" + ")
        ));
        exit(0);
    }

    logging::log(&format!(
        "服务启动: portalkeeperd {} (PID {}), 线路 {} 条, 掉线自动重试={}(间隔 {}s, 次数 {}){}; 不做周期发包保活",
        crate::VERSION,
        std::process::id(),
        lines.len(),
        if auto_retry { "开" } else { "关" },
        retry_interval,
        retry_count,
        if portal::parse_probe_url(&probe_url).is_some() {
            format!(", 自定义探测网站={}", probe_url.trim())
        } else {
            String::new()
        }
    ));

    // 启动即执行第一轮(开机自启场景下完成开机自动认证)
    monitor_all(&mut lines, retry_count, &probe_url);
    write_status(&lines, auto_retry, retry_interval, retry_count);

    if !auto_retry {
        // 自动重试关闭: 不做任何周期动作, 仅保留手动触发(立即检测并认证)
        logging::log("掉线自动重试已关闭, 守护进程转入待命(仅响应手动触发)");
        loop {
            sleep_tick();
            if stop_requested(started) {
                exit(0);
            }
            if trigger_fired() {
                reset_retry(&mut lines);
                monitor_all(&mut lines, retry_count, &probe_url);
                write_status(&lines, auto_retry, retry_interval, retry_count);
            }
        }
    }

    let mut next = epoch_secs() + retry_interval;
    loop {
        sleep_tick();
        if stop_requested(started) {
            exit(0);
        }
        let now = epoch_secs();
        if trigger_fired() {
            let _ = fs::remove_file(TRIGGER_PATH);
            reset_retry(&mut lines);
            monitor_all(&mut lines, retry_count, &probe_url);
            write_status(&lines, auto_retry, retry_interval, retry_count);
            next = epoch_secs() + retry_interval;
            continue;
        }
        if now >= next {
            monitor_all(&mut lines, retry_count, &probe_url);
            write_status(&lines, auto_retry, retry_interval, retry_count);
            next = epoch_secs() + retry_interval;
        }
    }
}

fn trigger_fired() -> bool {
    let ok = fs::metadata(TRIGGER_PATH).is_ok();
    if ok {
        let _ = fs::remove_file(TRIGGER_PATH);
        logging::log("收到手动触发(/tmp/portalkeeper.trigger), 立即检测并尝试认证");
    }
    ok
}

fn reset_retry(lines: &mut [LineRun]) {
    for lr in lines.iter_mut() {
        lr.exhausted = false;
        lr.fail_count = 0;
    }
}

fn monitor_all(lines: &mut [LineRun], retry_count: u32, probe_url: &str) {
    for lr in lines.iter_mut() {
        // 已经收到停止信号就不再开启新的线路检测(下一拍主循环即退出)
        if stop_pending() {
            return;
        }
        monitor(lr, retry_count, probe_url);
    }
    write_status(lines, true, 0, retry_count);
}

/// 一轮监测: 探测 → 在线则不动(无保活发包) → 掉线则按重试策略认证
fn monitor(lr: &mut LineRun, retry_count: u32, probe_url: &str) {
    if lr.username.is_empty() || lr.password.is_empty() {
        // 只在刚进入该状态时记一条, 免得每分钟一条把启停记录冲掉
        if lr.state != "no-credentials" {
            logging::log(&format!("[{}] 跳过: 未配置账号密码", lr.role));
        }
        lr.state = "no-credentials".into();
        lr.detail = "未配置账号密码".into();
        return;
    }
    let client = crate::http::HttpClient::new(
        lr.source,
        lr.device.clone(),
        Duration::from_secs(5),
    );
    let (verdict, det) = portal::connectivity_test(&client, probe_url);
    match verdict {
        Some(true) => {
            if lr.state != "online" {
                logging::log(&format!("[{}] 已在线(探测通过)", lr.role));
            }
            lr.state = "online".into();
            lr.detail = "已认证在线".into();
            lr.fail_count = 0;
            lr.exhausted = false;
        }
        Some(false) => {
            if lr.exhausted {
                lr.state = "exhausted".into();
                lr.detail =
                    "自动重试次数已用完: 请排查后点「立即检测并认证」或重启服务重置".into();
                logging::log(&format!("[{}] 处于暂停重试状态, 跳过本轮", lr.role));
                return;
            }
            // 认证要连发几个请求, 停止信号在此期间到达就先不做, 保证退出记录及时落盘
            if stop_pending() {
                return;
            }
            logging::log(&format!("[{}] 检测到掉线(被门户劫持), 开始认证...", lr.role));
            let line = LineCfg {
                name: lr.name.clone(),
                portal_url: lr.portal_url.clone(),
                device: lr.device.clone(),
                source: lr.source,
                username: lr.username.clone(),
                password: lr.password.clone(),
                ua: lr.ua.clone(),
            };
            let (ok, code, detail) =
                portal::authenticate(&line, &logging::log, Duration::from_secs(5));
            if ok {
                lr.state = "online".into();
                lr.detail = if code == "already" {
                    "本就已在线".into()
                } else {
                    "认证成功".into()
                };
                lr.fail_count = 0;
                lr.exhausted = false;
            } else {
                lr.fail_count += 1;
                if lr.fail_count >= retry_count {
                    lr.exhausted = true;
                    lr.state = "exhausted".into();
                    lr.detail = format!(
                        "连续 {} 次认证失败, 已暂停自动重试 — 最后状态({}): {}。可到自助后台({})检查槽位",
                        retry_count, code, detail, portal::SELF_SERVICE_URL
                    );
                    logging::log(&format!("[{}] {}", lr.role, lr.detail));
                } else {
                    lr.state = "retrying".into();
                    lr.detail = format!(
                        "第 {}/{} 次认证失败({}): {}, 将在下次检测时重试",
                        lr.fail_count, retry_count, code, detail
                    );
                    logging::log(&format!("[{}] {}", lr.role, lr.detail));
                }
            }
        }
        None => {
            lr.state = "unreachable".into();
            lr.detail = format!("无法判定(线路可能未接好): {det}");
            logging::log(&format!("[{}] 探测无法判定: {}", lr.role, det));
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 原子写状态文件(tmp+rename), LuCI 经 rpcd 读取
fn write_status(lines: &[LineRun], auto_retry: bool, retry_interval: u64, retry_count: u32) {
    let arr: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| {
            json!({
                "name": l.name,
                "role": l.role,
                "device": l.device,
                "source": l.source.map(|i| i.to_string()),
                "ua": l.ua,
                "state": l.state,
                "detail": l.detail,
            })
        })
        .collect();
    let v = json!({
        "updated_human": logging::timestamp(),
        "updated": epoch_secs(),
        "auto_retry": auto_retry,
        "retry_interval": retry_interval,
        "retry_count": retry_count,
        "lines": arr,
    });
    let tmp = format!("{STATUS_PATH}.tmp");
    if fs::write(&tmp, serde_json::to_string(&v).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, STATUS_PATH);
    }
}
