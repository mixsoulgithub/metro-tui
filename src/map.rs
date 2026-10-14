// 线路图模型:由 .dot 建图.站点是节点(带真实地理坐标),边是同一线路上相邻的两站.
use std::collections::HashMap;

use crate::dot;

#[derive(Clone, Debug)]
pub struct Line {
    pub label: String,
    pub color: (u8, u8, u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    pub line: usize,
}

pub struct Map {
    pub names: Vec<String>,
    /// 每站的可搜索拼音(音节用空格隔开):第一个是首选读音,后面是别的读法
    pub pinyin: Vec<Vec<String>>,
    /// 地理坐标:米(等距圆柱投影后的平面坐标),x 向东、y 向北
    pub pos: Vec<(f64, f64)>,
    pub lines: Vec<Line>,
    pub edges: Vec<Edge>,
    /// 每个站点经过的线路
    pub station_lines: Vec<Vec<usize>>,
    /// 每个站点的邻居:(站点, 线路)
    pub adj: Vec<Vec<(usize, usize)>>,
    /// 全域包围盒 (x0, y0, x1, y1)
    pub bbox: (f64, f64, f64, f64),
    /// 每条线走一遍的站点序列(解析时算好,见 line_paths)
    paths: Vec<(usize, Vec<usize>)>,
}

impl Map {
    pub fn from_dot(text: &str) -> Result<Map, String> {
        let d = dot::parse(text);
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut pos: Vec<(f64, f64)> = Vec::new();
        let mut pinyin: Vec<Vec<String>> = Vec::new();
        for n in &d.nodes {
            index.insert(n.name.as_str(), names.len());
            names.push(n.name.clone());
            pos.push(n.pos.ok_or_else(|| format!("节点 {} 缺少 pos", n.name))?);
            let mut py = Vec::new();
            // 音节之间留着空格:首字母缩写要靠它切音节
            for part in std::iter::once(n.pinyin.as_str()).chain(n.pinyin2.split('|')) {
                let s = part.trim();
                if !s.is_empty() && !py.iter().any(|p| p == s) {
                    py.push(s.to_string());
                }
            }
            pinyin.push(py);
        }
        let mut lines: Vec<Line> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut seen: HashMap<(usize, usize, usize), ()> = HashMap::new();
        for e in &d.edges {
            let a = *index
                .get(e.a.as_str())
                .ok_or_else(|| format!("边引用了未定义的站点 {}", e.a))?;
            let b = *index
                .get(e.b.as_str())
                .ok_or_else(|| format!("边引用了未定义的站点 {}", e.b))?;
            let color = e
                .color
                .ok_or_else(|| format!("边 {} -- {} 缺少 color", e.a, e.b))?;
            let li = match lines.iter().position(|l| l.label == e.line) {
                Some(i) => i,
                None => {
                    lines.push(Line {
                        label: e.line.clone(),
                        color,
                    });
                    lines.len() - 1
                }
            };
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            if seen.insert((lo, hi, li), ()).is_some() {
                continue;
            }
            edges.push(Edge { a, b, line: li });
        }
        if names.is_empty() || edges.is_empty() {
            return Err("图是空的".to_string());
        }
        let mut station_lines = vec![Vec::new(); names.len()];
        let mut adj = vec![Vec::new(); names.len()];
        for e in &edges {
            for (me, other) in [(e.a, e.b), (e.b, e.a)] {
                if !station_lines[me].contains(&e.line) {
                    station_lines[me].push(e.line);
                }
                adj[me].push((other, e.line));
            }
        }
        for v in station_lines.iter_mut() {
            v.sort_unstable();
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &pos {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
        let mut map = Map {
            names,
            pinyin,
            pos,
            lines,
            edges,
            station_lines,
            adj,
            bbox: (x0, y0, x1, y1),
            paths: Vec::new(),
        };
        map.paths = map.build_paths();
        Ok(map)
    }

    /// 换乘站:经过两条及以上线路
    pub fn is_interchange(&self, i: usize) -> bool {
        self.station_lines[i].len() > 1
    }

    /// 每条线的站点序列(有分叉就分成几条),带上线路号.
    /// 画线时按这个顺序走,好让同一条线在拐弯处接着上一段的方向走;
    /// `>` `<` 沿线走一站也用它
    /// 每条线走一遍的站点序列:(线路号, 站点序列).
    /// 解析时算一次就存下来:渲染每帧都要用,500 站的图重建一次不划算
    pub fn line_paths(&self) -> &[(usize, Vec<usize>)] {
        &self.paths
    }

    fn build_paths(&self) -> Vec<(usize, Vec<usize>)> {
        let mut out = Vec::new();
        for l in 0..self.lines.len() {
            let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
            for e in self.edges.iter().filter(|e| e.line == l) {
                adj.entry(e.a).or_default().push(e.b);
                adj.entry(e.b).or_default().push(e.a);
            }
            let key = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
            let mut visited: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
            let mut starts: Vec<usize> = adj.iter().filter(|(_, v)| v.len() != 2).map(|(k, _)| *k).collect();
            starts.sort_unstable();
            for &start in starts.iter().chain(adj.keys()) {
                for &nb in &adj[&start] {
                    if visited.contains(&key(start, nb)) {
                        continue;
                    }
                    let mut path = vec![start, nb];
                    visited.insert(key(start, nb));
                    let (mut prev, mut cur) = (start, nb);
                    while let Some(&next) = adj[&cur]
                        .iter()
                        .find(|&&n| n != prev && !visited.contains(&key(cur, n)))
                    {
                        visited.insert(key(cur, next));
                        path.push(next);
                        prev = cur;
                        cur = next;
                    }
                    if path.len() > 1 {
                        out.push((l, path));
                    }
                }
            }
        }
        out
    }

    pub fn line_color(&self, line: usize) -> (u8, u8, u8) {
        self.lines[line].color
    }
}

/// 松开示意图用的规格.留一点余量:200x50 的窗口下整幅图是"放大"铺满的,
/// 而不是被压着塞进去 —— 压着塞会四舍五入把两站并到相邻格
pub const NOMINAL: (f64, f64) = (184.0, 45.0);

/// 画布规格:按站数开方放大,让"每个站分到多少格子"和杭州基准差不多.
/// 站多了画布不放大,后处理(docs 里 layout.rs)在密集区就找不到空位,站会叠在一起
pub fn nominal_for(n: usize) -> (f64, f64) {
    let k = (n as f64 / 262.0).sqrt().max(1.0);
    (NOMINAL.0 * k, NOMINAL.1 * k)
}

/// 一套"画布坐标":同一张图可以按真实地理画,也可以按示意图画.
#[derive(Clone, Debug)]
pub struct Scene {
    pub pos: Vec<(f64, f64)>,
    pub bbox: (f64, f64, f64, f64),
    /// 一行的物理高度相当于几列:真实地理按字符格 2:1 压扁,示意图是 1:1
    pub aspect: f64,
    /// y 是向上的(地理坐标)还是向下的(示意图已经是屏幕方向)
    pub flip_y: bool,
}

impl Scene {
    fn from_pos(pos: Vec<(f64, f64)>, aspect: f64, flip_y: bool) -> Scene {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &pos {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
        Scene {
            pos,
            bbox: (x0, y0, x1, y1),
            aspect,
            flip_y,
        }
    }

    /// 真实经纬度(等距圆柱投影后的米)
    pub fn real(map: &Map) -> Scene {
        Scene {
            pos: map.pos.clone(),
            bbox: map.bbox,
            aspect: 2.0,
            flip_y: true,
        }
    }

    /// 示意图:站距均匀、走向贴到四种笔画上.
    /// 松开用的规格是固定的 —— 换窗口只是把同一张图放大缩小,
    /// 图本身不会跳;窗口大到站距拉开 4 格以上时,站名会自动标出来.
    pub fn schematic(map: &Map) -> Scene {
        let (w, h) = nominal_for(map.pos.len());
        Scene::from_pos(crate::layout::schematic(map, w, h), 1.0, false)
    }

    /// 测试用:直接拿一组坐标当画布坐标(不翻转、不压扁)
    #[cfg(test)]
    pub fn from_pos_for_test(pos: Vec<(f64, f64)>) -> Scene {
        Scene::from_pos(pos, 1.0, false)
    }

    pub fn center(&self) -> (f64, f64) {
        (
            (self.bbox.0 + self.bbox.2) / 2.0,
            (self.bbox.1 + self.bbox.3) / 2.0,
        )
    }

    pub fn size(&self) -> (f64, f64) {
        (
            (self.bbox.2 - self.bbox.0).max(1.0),
            (self.bbox.3 - self.bbox.1).max(1.0),
        )
    }
}
