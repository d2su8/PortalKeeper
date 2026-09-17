//! 极简 UCI 配置解析(/etc/config/portalkeeper)。
//! 只覆盖本插件用到的子集: config/option/list(取最后一条)/单双引号值; # 注释行跳过。

use std::fs;

#[derive(Debug, Clone)]
pub struct Section {
    pub stype: String,
    pub name: Option<String>,
    pub options: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub sections: Vec<Section>,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2
        && ((v.starts_with('\'') && v.ends_with('\'')) || (v.starts_with('"') && v.ends_with('"')))
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

impl Config {
    pub fn load(path: &str) -> Config {
        let text = fs::read_to_string(path).unwrap_or_default();
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Config {
        let mut cfg = Config::default();
        let mut cur: Option<Section> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split_whitespace();
            let kw = it.next().unwrap_or("");
            match kw {
                "config" => {
                    if let Some(sec) = cur.take() {
                        cfg.sections.push(sec);
                    }
                    let stype = it.next().unwrap_or("").trim_matches(['\'', '"']).to_string();
                    let name = it.next().map(|s| s.trim_matches(['\'', '"']).to_string());
                    cur = Some(Section {
                        stype,
                        name,
                        options: Vec::new(),
                    });
                }
                "option" | "list" => {
                    // option <k> <v>  (list 取最后一条, 本插件无多值需求)
                    let k = it.next().unwrap_or("").trim_matches(['\'', '"']).to_string();
                    let rest = line.splitn(3, char::is_whitespace).nth(2).unwrap_or("");
                    let v = unquote(rest);
                    if let Some(sec) = cur.as_mut() {
                        sec.options.push((k, v));
                    }
                }
                _ => {}
            }
        }
        if let Some(sec) = cur.take() {
            cfg.sections.push(sec);
        }
        cfg
    }

    pub fn first(&self, stype: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.stype == stype)
    }

    pub fn sections_of(&self, stype: &str) -> Vec<&Section> {
        self.sections.iter().filter(|s| s.stype == stype).collect()
    }
}

pub fn get<'a>(sec: &'a Section, key: &str) -> Option<&'a str> {
    sec.get(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let cfg = Config::parse(
            "config portalkeeper 'main'\n\toption enabled '1'\n\toption interval 300\n\
             config line 'wan'\n\toption device \"eth1\"\n\t# comment\n\toption ua 'mobile'\n",
        );
        assert_eq!(cfg.sections.len(), 2);
        let main = cfg.first("portalkeeper").unwrap();
        assert_eq!(main.name.as_deref(), Some("main"));
        assert_eq!(main.get("enabled"), Some("1"));
        assert_eq!(main.get("interval"), Some("300"));
        let lines = cfg.sections_of("line");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].get("device"), Some("eth1"));
        assert_eq!(lines[0].get("ua"), Some("mobile"));
    }
}
