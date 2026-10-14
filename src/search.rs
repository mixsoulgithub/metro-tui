// 搜索:站名(中文)和拼音都能匹配,纯数字当成线路号.
//
// 拼音来自 data/pinyin_chars.txt(由 pypinyin 的字典筛出来的),每个站可能有几种读法
// (多音字),都存在 map.pinyin 里;首字母缩写(比如 wlgc)也认.
use crate::map::Map;

/// 匹配结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// 输入是数字:要高亮的线路
    Lines(Vec<usize>),
    /// 输入是文字:匹配到的站(按匹配质量排好)
    Stations(Vec<usize>),
    /// 空的
    Empty,
}

/// 拼音/字母查询的归一化:只留字母数字,小写,ü 与 v 都当 u
pub fn latin(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() {
                Some(c)
            } else if c == 'ü' || c == 'v' {
                Some('u')
            } else {
                None
            }
        })
        .collect()
}

/// 拼音的首字母缩写:"wu lin guang chang" -> "wlgc"
pub fn initials(pinyin: &str) -> String {
    pinyin
        .split_whitespace()
        .filter_map(|s| s.chars().next())
        .collect()
}

/// 站名匹配得分:越小越好
fn score(map: &Map, i: usize, name_q: &str, name_lower: &str, latin_q: &str) -> Option<u8> {
    let name = &map.names[i];
    // 名字按小写比:海外站名是拉丁字母,输入不该区分大小写(中文名小写化是恒等的)
    let name_fold = name.to_lowercase();
    let mut best: Option<u8> = None;
    let mut put = |s: u8| {
        if best.map(|b| s < b).unwrap_or(true) {
            best = Some(s);
        }
    };
    if !name_q.is_empty() {
        if name_fold == name_lower {
            put(0);
        } else if name_fold.starts_with(name_lower) {
            put(1);
        } else if name_fold.contains(name_lower) {
            put(2);
        }
    }
    // 名字本身按字母比一遍:海外城市的站名就是英文/当地文字,输入不该区分大小写
    if !latin_q.is_empty() {
        let n = latin(name);
        if !n.is_empty() {
            if n == latin_q {
                put(4);
            } else if n.starts_with(latin_q) {
                put(5);
            } else if n.contains(latin_q) {
                put(6);
            }
        }
    }
    if !latin_q.is_empty() {
        for (k, py) in map.pinyin[i].iter().enumerate() {
            let p = latin(py);
            let ini = initials(py);
            // 首选读音比别的读法优先
            let base = if k == 0 { 3 } else { 7 };
            if p == latin_q {
                put(base);
            } else if p.starts_with(latin_q) {
                put(base + 1);
            } else if p.contains(latin_q) {
                put(base + 2);
            }
            if ini == latin_q {
                put(base + 3);
            } else if ini.starts_with(latin_q) {
                put(base + 4);
            } else if ini.contains(latin_q) {
                put(base + 5);
            }
        }
    }
    best
}

/// 匹配站名,按"越准越前"排
pub fn stations(map: &Map, text: &str) -> Vec<usize> {
    let name_q = text.trim();
    let latin_q = latin(name_q);
    if name_q.is_empty() && latin_q.is_empty() {
        return Vec::new();
    }
    let name_lower = name_q.to_lowercase();
    let mut hits: Vec<(u8, usize)> = Vec::new();
    for i in 0..map.names.len() {
        if let Some(s) = score(map, i, name_q, &name_lower, &latin_q) {
            hits.push((s, i));
        }
    }
    hits.sort_by_key(|&(s, i)| (s, i));
    hits.into_iter().map(|(_, i)| i).collect()
}

/// 数字 -> 线路(6 既中 6 号线也中 6 号线支线)
pub fn lines(map: &Map, digits: &str) -> Vec<usize> {
    let want = format!("{digits}号线");
    map.lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.label.starts_with(&want))
        .map(|(i, _)| i)
        .collect()
}

/// 解析输入:纯数字当线路,其余当站名/拼音
pub fn parse(map: &Map, text: &str) -> Found {
    let t = text.trim();
    if t.is_empty() {
        return Found::Empty;
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return Found::Lines(lines(map, t));
    }
    Found::Stations(stations(map, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Map {
        Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).expect("内嵌线路图")
    }

    fn names<'a>(m: &'a Map, hits: &[usize]) -> Vec<&'a str> {
        hits.iter().map(|&i| m.names[i].as_str()).collect()
    }

    #[test]
    fn 英文站名搜起来不分大小写() {
        let m = Map::from_dot(
            "graph g {\n  \"Kings Cross\" [pos=\"0,0\"];\n  \"Barking\" [pos=\"1000,0\"];\n  \"Kingsbury\" [pos=\"2000,0\"];\n  \"Kings Cross\" -- \"Barking\" [color=\"#000000\", line=\"Test\"];\n  \"Barking\" -- \"Kingsbury\" [color=\"#000000\", line=\"Test\"];\n}",
        )
        .expect("内联线路图");
        let hit = stations(&m, "king");
        let top = names(&m, &hit);
        println!("king -> {top:?}");
        assert_eq!(m.names[hit[0]], "Kings Cross", "前缀命中该排在子串命中前面");
        assert!(top.contains(&"Kingsbury"), "Kingsbury 也该中");
        assert_eq!(stations(&m, "KING")[0], hit[0], "大写输入结果一样");
    }

    #[test]
    fn 拼音和中英文首字母都能搜到() {
        let m = map();
        let hit = names(&m, &stations(&m, "wulin"));
        println!("wulin -> {hit:?}");
        assert!(hit.contains(&"武林广场"), "wulin 该搜到 武林广场");
        assert!(hit.contains(&"武林门"), "wulin 该搜到 武林门");

        // 首字母缩写
        let hit = stations(&m, "wlgc");
        assert_eq!(m.names[hit[0]], "武林广场", "wlgc 第一个该中 武林广场");

        // 中文
        let hit = names(&m, &stations(&m, "黄龙"));
        assert!(hit.contains(&"黄龙体育中心") && hit.contains(&"黄龙洞"));

        // 多音字:长河 的第一读音是 zhang,chang 排在备选里,照样要能搜到
        let hit = names(&m, &stations(&m, "changhe"));
        println!("changhe -> {hit:?}");
        assert!(hit.contains(&"长河"), "长河的另一个读音也该搜得到");
    }

    #[test]
    fn 纯数字当线路号() {
        let m = map();
        match parse(&m, "1") {
            Found::Lines(ls) => assert_eq!(m.lines[ls[0]].label, "1号线"),
            other => panic!("1 该当成线路号,却得到 {other:?}"),
        }
        match parse(&m, "6") {
            Found::Lines(ls) => {
                let labels: Vec<&str> = ls.iter().map(|&i| m.lines[i].label.as_str()).collect();
                assert!(labels.contains(&"6号线") && labels.contains(&"6号线支线"), "{labels:?}");
            }
            other => panic!("6 该当成线路号,却得到 {other:?}"),
        }
        match parse(&m, "19") {
            Found::Lines(ls) => assert_eq!(m.lines[ls[0]].label, "19号线"),
            other => panic!("19 该当成线路号,却得到 {other:?}"),
        }
        assert!(matches!(parse(&m, "  "), Found::Empty));
        assert!(matches!(parse(&m, "黄龙"), Found::Stations(_)));
    }
}
