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
    let mut g = LOG.lock().unwrap();
    *g = Some(FileLog {
        path: PathBuf::from(path),
        max_bytes,
    });
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

/// 本地时间串 "YYYY-MM-DD HH:MM:SS"(跟随路由器系统时区/TZ, 与系统时间一致;
/// 取不到系统时间时回退为 UTC 计算)
pub fn timestamp() -> String {
    // 用系统 date 命令, 天然跟随 /etc/TZ 与 NTP 校准
    if let Ok(out) = std::process::Command::new("date").arg("+%F %T").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
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
