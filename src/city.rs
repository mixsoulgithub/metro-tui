//! 内嵌的城市线路图.加新城市:先用 tools/fetch_city.py 抓数据、tools/osm2dot.py 生成
//! docs/<slug>-subway.dot,然后在这里加一行.

pub struct City {
    pub slug: &'static str,
    pub name: &'static str,
    /// 线路图 .dot 原文
    pub dot: &'static str,
}

pub const CITIES: &[City] = &[
    City {
        slug: "hangzhou",
        name: "杭州",
        dot: include_str!("../docs/hangzhou-subway.dot"),
    },
    City {
        slug: "shanghai",
        name: "上海",
        dot: include_str!("../docs/shanghai-subway.dot"),
    },
    City {
        slug: "chengdu",
        name: "成都",
        dot: include_str!("../docs/chengdu-subway.dot"),
    },
    City {
        slug: "shenzhen",
        name: "深圳",
        dot: include_str!("../docs/shenzhen-subway.dot"),
    },
];

/// 按 slug 找城市,找不到就把可选的列出来
pub fn by_slug(slug: &str) -> Result<&'static City, String> {
    CITIES
        .iter()
        .find(|c| c.slug == slug)
        .ok_or_else(|| format!("没有 {slug} 这个城市(可选:{})", slugs()))
}

pub fn slugs() -> String {
    CITIES
        .iter()
        .map(|c| format!("{}={}", c.slug, c.name))
        .collect::<Vec<_>>()
        .join(" ")
}

/// docs 目录里现成的线路图(工具建出来的,不用重新编译就能看).
/// 返回 (slug, 显示名, 线路数, 站点数, 路径);内嵌的那几座会跳过
pub fn external(dir: &std::path::Path) -> Vec<(String, String, usize, usize, std::path::PathBuf)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with("-subway.dot"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    for path in files {
        let Some(slug) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches("-subway.dot").to_string())
        else {
            continue;
        };
        if CITIES.iter().any(|c| c.slug == slug) {
            continue; // 内嵌的优先
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // 站数、边数用最省的字符串统计就够(这里只为了列表上显示)
        let stations = text.matches("[pos=").count();
        // 显示名从文件头的注释里取("南京地铁线路图:...")
        let name = text
            .lines()
            .next()
            .and_then(|l| l.trim_start_matches("// ").split("地铁").next())
            .filter(|n| !n.is_empty() && n.chars().count() <= 6)
            .map(|n| n.to_string())
            .unwrap_or_else(|| slug.clone());
        // 边数是重复计数(一条边两个方向都写),线路数按去重算
        let lines = {
            let mut set = std::collections::HashSet::new();
            for part in text.split("line=\"").skip(1) {
                if let Some(v) = part.split('"').next() {
                    set.insert(v.to_string());
                }
            }
            set.len()
        };
        out.push((slug, name, lines, stations, path));
    }
    out
}

/// 按 slug 找一个 dot:先看内嵌的,再看 docs 目录
pub fn resolve(slug: &str, dirs: &[std::path::PathBuf]) -> Result<String, String> {
    if let Ok(c) = by_slug(slug) {
        return Ok(c.dot.to_string());
    }
    for d in dirs {
        let path = d.join(format!("{slug}-subway.dot"));
        if let Ok(t) = std::fs::read_to_string(&path) {
            return Ok(t);
        }
    }
    Err(format!("没有 {slug} 这个城市(可选:{})", slugs()))
}
