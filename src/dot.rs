// .dot 子集解析:本图只用到下面这几种写法,其余一律忽略.
//   graph 名字 { ... }
//   node [k=v, ...];  /  edge [k=v, ...];        默认属性
//   "站点" [pos="x,y", ...];                     节点
//   "甲" -- "乙" [color="#rrggbb", line="1号线"];  无向边
// 值是 utf-8 字符串(站点名是中文),不处理转义,不处理子图.

#[derive(Clone, Debug, Default)]
pub struct Attrs {
    kv: Vec<(String, String)>,
}

impl Attrs {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// 只补自己缺的键:节点/边的属性优先于默认属性
    fn fill_from(&mut self, other: &Attrs) {
        for (k, v) in &other.kv {
            if self.get(k).is_none() {
                self.kv.push((k.clone(), v.clone()));
            }
        }
    }

    fn set(&mut self, k: &str, v: &str) {
        match self.kv.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v.to_string(),
            None => self.kv.push((k.to_string(), v.to_string())),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub pos: Option<(f64, f64)>,
    /// 首选读音(拼音,音节用空格隔开)
    pub pinyin: String,
    /// 别的读法(多音字),用 '|' 隔开
    pub pinyin2: String,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub a: String,
    pub b: String,
    pub color: Option<(u8, u8, u8)>,
    pub line: String,
}

#[derive(Clone, Debug, Default)]
pub struct Dot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// 把一段文本切成"语句":去掉注释,按深度 0 的 ';' 切,花括号只当分隔符
fn statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for line in text.lines() {
        // 行内注释(值里没有 '//')
        let line = match line.find("//") {
            Some(i) if !quoted => &line[..i],
            _ => line,
        };
        for c in line.chars() {
            match c {
                '"' => {
                    quoted = !quoted;
                    cur.push(c);
                }
                ';' if !quoted => out.push(std::mem::take(&mut cur)),
                '{' | '}' if !quoted => {
                    if !cur.trim().is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            }
        }
        cur.push(' ');
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// 切词:引号字符串去掉引号,'=' '[' ']' ',' '--' '->' 各自成词
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut quoted = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                quoted = false;
                out.push(std::mem::take(&mut buf));
            } else {
                buf.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '=' | '[' | ']' | ',' => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
                if c != ',' {
                    out.push(c.to_string());
                }
            }
            '-' | '<' if chars.peek() == Some(&'-') || chars.peek() == Some(&'>') => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
                chars.next();
                out.push("--".to_string());
            }
            c if c.is_whitespace() => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            c => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// 从 '[' 开始解析属性表,返回 (属性, 下一个位置)
fn parse_attrs(tok: &[String], mut i: usize) -> (Attrs, usize) {
    let mut attrs = Attrs::default();
    if tok.get(i).map(String::as_str) != Some("[") {
        return (attrs, i);
    }
    i += 1;
    while i < tok.len() {
        match tok[i].as_str() {
            "]" => {
                i += 1;
                break;
            }
            key => {
                let mut value = "";
                if tok.get(i + 1).map(String::as_str) == Some("=") {
                    value = tok.get(i + 2).map(String::as_str).unwrap_or("");
                    i += 3;
                } else {
                    i += 1;
                }
                if !key.is_empty() {
                    attrs.set(key, value);
                }
            }
        }
    }
    (attrs, i)
}

fn color_of(attrs: &Attrs) -> Option<(u8, u8, u8)> {
    let s = attrs.get("color")?;
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((b(0)?, b(2)?, b(4)?))
}

pub fn parse(text: &str) -> Dot {
    let mut dot = Dot::default();
    let mut node_def = Attrs::default();
    let mut edge_def = Attrs::default();
    for stmt in statements(text) {
        let tok = tokens(&stmt);
        if tok.is_empty() {
            continue;
        }
        match tok[0].as_str() {
            // graph/node/edge 的默认属性
            kind @ ("graph" | "node" | "edge") => {
                let (attrs, _) = parse_attrs(&tok, 1);
                match kind {
                    "node" => node_def.fill_from(&attrs),
                    "edge" => edge_def.fill_from(&attrs),
                    _ => {}
                }
                continue;
            }
            _ => {}
        }
        if tok[0] == "--" || tok[0] == "=" {
            continue;
        }
        // 节点:"名" [属性]
        if tok.get(1).map(String::as_str) != Some("--") {
            let mut attrs = Attrs::default();
            let (given, _) = parse_attrs(&tok, 1);
            attrs.fill_from(&given);
            attrs.fill_from(&node_def);
            let pos = attrs.get("pos").and_then(|p| {
                let (x, y) = p.split_once(',')?;
                Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
            });
            dot.nodes.push(Node {
                name: tok[0].clone(),
                pos,
                pinyin: attrs.get("pinyin").unwrap_or("").to_string(),
                pinyin2: attrs.get("pinyin2").unwrap_or("").to_string(),
            });
            continue;
        }
        // 边:"甲" -- "乙" [属性]
        let (a, b) = (tok[0].clone(), tok[2].clone());
        let mut attrs = Attrs::default();
        let (given, _) = parse_attrs(&tok, 3);
        attrs.fill_from(&given);
        attrs.fill_from(&edge_def);
        dot.edges.push(Edge {
            a,
            b,
            color: color_of(&attrs),
            line: attrs.get("line").unwrap_or("").to_string(),
        });
    }
    dot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析站点与边() {
        let d = parse(
            "// 注释\n\
             graph g {\n\
             node [shape=point];\n\
             edge [color=\"#112233\", line=\"2号线\"];\n\
             \"甲\" [pos=\"10,20\"];\n\
             \"乙\" [pos=\"30.5,-40\"];\n\
             \"甲\" -- \"乙\" [color=\"#df4661\", line=\"1号线\"];\n\
             \"甲\" -- \"乙\";\n\
             }\n",
        );
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].name, "甲");
        assert_eq!(d.nodes[0].pos, Some((10.0, 20.0)));
        assert_eq!(d.nodes[1].pos, Some((30.5, -40.0)));
        assert_eq!(d.edges.len(), 2);
        // 边自己的属性盖过 edge 默认属性
        assert_eq!(d.edges[0].color, Some((0xdf, 0x46, 0x61)));
        assert_eq!(d.edges[0].line, "1号线");
        assert_eq!(d.edges[1].color, Some((0x11, 0x22, 0x33)));
        assert_eq!(d.edges[1].line, "2号线");
    }

    #[test]
    fn 没有坐标的节点是_none() {
        let d = parse("graph g {\n \"甲\";\n}");
        assert_eq!(d.nodes.len(), 1);
        assert_eq!(d.nodes[0].pos, None);
    }
}
