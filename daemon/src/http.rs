//! 极简 HTTP/1.1 客户端(门户为明文 HTTP, 无需 TLS):
//! - 支持绑定源 IP 与出口网卡(Linux SO_BINDTODEVICE, 多线多拨的关键)
//! - 支持 chunked 解码; UA 头只出现一次(门户按首个 UA 判定设备槽位)

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>, // name 全小写
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct HttpClient {
    pub source: Option<Ipv4Addr>,
    /// 出口网卡名(如 eth1), 仅 Linux: SO_BINDTODEVICE, 需 root(OpenWrt 默认 root)
    pub device: Option<String>,
    pub timeout: Duration,
}

impl HttpClient {
    pub fn new(source: Option<Ipv4Addr>, device: Option<String>, timeout: Duration) -> Self {
        HttpClient {
            source,
            device,
            timeout,
        }
    }

    /// GET/POST 一个 http:// URL
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<HttpResponse, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("仅支持 http URL: {url}"))?;
        let (hostport, path) = match rest.find('/') {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, "/"),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                (h, p.parse::<u16>().unwrap_or(80))
            }
            _ => (hostport, 80),
        };

        let target = (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("DNS 解析失败 {host}: {e}"))?
            .find(|a| a.is_ipv4())
            .ok_or_else(|| format!("DNS 未返回 IPv4 地址: {host}"))?;

        let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .map_err(|e| format!("创建 socket 失败: {e}"))?;
        #[cfg(target_os = "linux")]
        if let Some(dev) = &self.device {
            // SO_BINDTODEVICE 需 root; OpenWrt 守护进程默认 root
            let _ = sock.bind_device(Some(dev.as_bytes()));
        }
        if let Some(src) = self.source {
            sock.bind(&SockAddr::from(std::net::SocketAddr::from((src, 0))))
                .map_err(|e| format!("绑定源 IP {src} 失败: {e}"))?;
        }
        sock.connect_timeout(&target.into(), self.timeout)
            .map_err(|e| format!("连接 {target} 失败: {e}"))?;
        sock.set_read_timeout(Some(self.timeout))
            .map_err(|e| format!("set_read_timeout 失败: {e}"))?;
        sock.set_write_timeout(Some(self.timeout))
            .map_err(|e| format!("set_write_timeout 失败: {e}"))?;
        let mut stream: TcpStream = sock.into();

        let host_hdr = if port == 80 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host_hdr}\r\nConnection: close\r\n");
        let mut has_ua = false;
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("user-agent") {
                has_ua = true;
            }
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !has_ua {
            req.push_str("User-Agent: portalkeeper/1.0\r\n");
        }
        match body {
            Some(b) => {
                req.push_str(&format!("Content-Length: {}\r\n\r\n", b.len()));
                req.push_str(b);
            }
            None => req.push_str("\r\n"),
        }
        stream
            .write_all(req.as_bytes())
            .map_err(|e| format!("发送请求失败: {e}"))?;

        let mut raw = Vec::with_capacity(16 * 1024);
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => return Err(format!("读取响应失败: {e}")),
            }
        }

        parse_response(&raw)
    }
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "响应缺少头部分隔符".to_string())?;
    let head = String::from_utf8_lossy(&raw[..sep]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let _ver = parts.next().unwrap_or("HTTP/1.1");
    let status: u16 = parts
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| format!("无法解析状态行: {status_line}"))?;

    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut body = raw[sep + 4..].to_vec();
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        body = decode_chunked(&body);
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0usize;
    loop {
        let line_end = match data[i..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => i + p,
            None => break,
        };
        let size_str = String::from_utf8_lossy(&data[i..line_end]);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = match usize::from_str_radix(size_str, 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        i = line_end + 2;
        if size == 0 {
            break;
        }
        if i + size > data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2;
    }
    out
}
