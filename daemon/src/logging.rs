//! 日志: 默认只写 /tmp(tmpfs), 不落闪存避免 NAND 磨损; 超限滚动裁剪。
//! 同时输出到 stderr(procd 会转存到 syslog, 方便 logread 排查)。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

struct FileLog {
    path: PathBuf,
    max_bytes: u64,
}

static LOG: Mutex<Option<FileLog>> = Mutex::new(None);

/// 初始化全局日志; 失败(如 /tmp 不可写)时静默降级为仅 stderr
pub fn init(path: &str, max_bytes: u64) {
    adopt_system_timezone();
    let mut g = LOG.lock().unwrap();
    *g = Some(FileLog {
        path: PathBuf::from(path),
        max_bytes,
    });
}

// libc crate 未暴露 tzset(C 侧常是宏); 且 crate 的 time_t 在 32 位 musl 目标上
// 还是旧的 32 位定义(已废弃), 与 musl ≥1.2 的真实 ABI(64 位)不符 —— 这里按
// musl 1.2+ 自行声明(localtime_r 的 time_t 用 i64)。现代 OpenWrt
// (23.05/24.10/25.x, musl 1.2.3+)各架构 time_t 均为 64 位, 声明一致。
extern "C" {
    fn tzset();
    fn localtime_r(t: *const i64, tm: *mut libc::tm) -> *mut libc::tm;
}

/// 让 libc 时间函数跟随 OpenWrt 系统时区。
/// musl 的 localtime_r 只认 TZ 环境变量(不会像 busybox date 那样自己读 /etc/TZ),
/// 所以启动时把 /etc/TZ 导入环境并 tzset —— 之后日志时间戳与 init 脚本里
/// busybox date 的格式/时区一致(同一份日志文件, 两边时间不能差 8 小时)。
/// /etc/TZ 不存在则保持继承来的环境(默认 UTC)。
fn adopt_system_timezone() {
    let tz = match fs::read_to_string("/etc/TZ") {
        Ok(t) => t.trim().to_string(),
        Err(_) => return,
    };
    if tz.is_empty() {
        return;
    }
    unsafe {
        libc::setenv(c"TZ".as_ptr(), tz.as_ptr() as *const libc::c_char, 1);
        tzset();
    }
}

pub fn log(msg: &str) {
    let line = format!("[{}] {}", timestamp(), msg);
    println!("{line}");
    if let Ok(g) = LOG.lock() {
        if let Some(f) = g.as_ref() {
            rotate_if_needed(f);
            if let Ok(mut fh) = OpenOptions::new().create(true).append(true).open(&f.path) {
                let _ = writeln!(fh, "{line}");
            }
        }
    }
}

fn rotate_if_needed(f: &FileLog) {
    let len = fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0);
    if len < f.max_bytes {
        return;
    }
    // 保留后半段(按行边界), 避免无限增长占满 /tmp
    if let Ok(data) = fs::read(&f.path) {
        let cut = data.len() / 2;
        let start = data[cut..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| cut + p + 1)
            .unwrap_or(cut);
        let _ = fs::write(&f.path, &data[start..]);
    }
}

/// 本地时间串 "YYYY-MM-DD HH:MM:SS"(跟随 /etc/TZ 系统时区, 与 busybox date 一致;
/// 取不到时回退为 UTC 计算)。
/// 用 libc localtime_r 直接算, 不再每条日志 fork 一个 date 进程
/// (排障时一条认证几十条日志, fork 开销可观)。
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    unsafe {
        let t = secs as i64;
        let mut tm: libc::tm = std::mem::zeroed();
        if !localtime_r(&t, &mut tm).is_null() {
            let mut buf = [0u8; 32];
            let fmt = c"%Y-%m-%d %H:%M:%S";
            let n = libc::strftime(
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                fmt.as_ptr(),
                &tm,
            );
            if n > 0 && (n as usize) < buf.len() {
                return String::from_utf8_lossy(&buf[..n as usize]).into_owned();
            }
        }
    }
    utc_timestamp()
}

/// UTC 兜底: 手工 civil_from_days, 不依赖任何外部程序
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant civil_from_days: epoch 天数 → (年, 月, 日)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}
