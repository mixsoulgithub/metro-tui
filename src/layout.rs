// 示意图布局:把地理坐标"松开"成一张地铁示意图.
//
// 做法是从真实位置出发做几轮弹簧放松,让每条线上的相邻站都保持差不多一样的间距,
// 同时把线的走向吸附到 - | / \ 四种笔画上(格坐标里就是横、竖、斜 45 度),
// 再用一个很弱的锚把整幅图拽在原位附近 —— 结果是相对方位跟真实地图一致,
// 但站与站之间疏密均匀,不会有市中心挤成一团的情况.
use std::collections::HashMap;

use crate::map::Map;

/// 相邻站的目标间距(格)
const STEP: f64 = 2.6;
/// 任意两站的最小间距(格)
const MIN_GAP: f64 = 2.6;
const ITERS: usize = 600;

/// 规模参数:站一多,画布、搜索半径、迭代轮数都要跟着放大 ——
/// 不然后处理在密集区找不到空位,站会叠在一起(杭州是基准,k = 1)
struct Cfg {
    k: f64,
}

impl Cfg {
    fn of(n: usize) -> Cfg {
        Cfg {
            k: (n as f64 / 262.0).sqrt().max(1.0),
        }
    }
    /// 同心环往外找空位的上限
    fn rings(&self) -> i32 {
        (64.0 * self.k) as i32
    }
    /// best_cell 的搜索半径(杭州 3,也就是 7x7)
    fn radius(&self) -> i32 {
        (3.0 * self.k).round() as i32
    }
    /// clean_cell 允许挪动的格数(杭州 2)
    fn nudge(&self) -> i32 {
        (2.0 * self.k).round() as i32
    }
    /// 迭代轮数:约束多了要更多轮才磨得平(循环里没人动就提前退出,k = 1 时行为不变)
    fn rounds(&self, base: usize) -> usize {
        (base as f64 * self.k).round() as usize
    }
}
/// 拉直力:让一条线连着三站尽量共线,画出来才有长长的直走廊
const K_STRAIGHT: f64 = 0.06;
/// 锚的力度:越小越不理会地理上的疏密,越往站距均匀那边靠
const ANCHOR_W: f64 = 0.030;

fn bbox(p: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for q in p {
        x0 = x0.min(q.0);
        y0 = y0.min(q.1);
        x1 = x1.max(q.0);
        y1 = y1.max(q.1);
    }
    (x0, y0, x1, y1)
}

/// 边的目标方向(格坐标下的单位向量):水平、竖直或 45 度
fn snap_dir(dx: f64, dy: f64) -> (f64, f64) {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ay < ax * 0.4 {
        (dx.signum(), 0.0)
    } else if ax < ay * 0.4 {
        (0.0, dy.signum())
    } else {
        // 45 度:格坐标里就是这个斜率,画出来正好一个直角拐弯
        let k = std::f64::consts::FRAC_1_SQRT_2;
        (dx.signum() * k, dy.signum() * k)
    }
}

/// 斥力:两站贴太近就推开(用网格找候选,免得 O(n^2))
fn repel(p: &[(f64, f64)], f: &mut [(f64, f64)], k: f64) {
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, q) in p.iter().enumerate() {
        let cell = ((q.0 / MIN_GAP).floor() as i32, (q.1 / MIN_GAP).floor() as i32);
        grid.entry(cell).or_default().push(i);
    }
    for (i, q) in p.iter().enumerate() {
        let cell = ((q.0 / MIN_GAP).floor() as i32, (q.1 / MIN_GAP).floor() as i32);
        for di in -1..=1 {
            for dj in -1..=1 {
                for &j in grid.get(&(cell.0 + di, cell.1 + dj)).map(Vec::as_slice).unwrap_or(&[]) {
                    if j <= i {
                        continue;
                    }
                    let (dx, dy) = (p[j].0 - q.0, p[j].1 - q.1);
                    let len = (dx * dx + dy * dy).sqrt();
                    if len >= MIN_GAP {
                        continue;
                    }
                    let (ux, uy) = if len < 1e-6 { (0.0, 1.0) } else { (dx / len, dy / len) };
                    let push = (MIN_GAP - len) * k;
                    f[i].0 -= ux * push;
                    f[i].1 -= uy * push;
                    f[j].0 += ux * push;
                    f[j].1 += uy * push;
                }
            }
        }
    }
}

/// 取整去重叠:一格一站,占了就按同心环往外找最近的空格
fn snap_to_grid(p: &[(f64, f64)], cfg: &Cfg) -> Vec<(f64, f64)> {
    let mut taken: Vec<(i32, i32)> = Vec::with_capacity(p.len());
    let mut out = vec![(0.0, 0.0); p.len()];
    let mut order: Vec<usize> = (0..p.len()).collect();
    order.sort_by(|&a, &b| {
        p[a].1
            .total_cmp(&p[b].1)
            .then(p[a].0.total_cmp(&p[b].0))
            .then(a.cmp(&b))
    });
    for i in order {
        let (x0, y0) = (p[i].0.round() as i32, p[i].1.round() as i32);
        let mut spot = (x0, y0);
        'grow: for r in 0..cfg.rings() {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    if !taken.contains(&(x0 + dx, y0 + dy)) {
                        spot = (x0 + dx, y0 + dy);
                        break 'grow;
                    }
                }
            }
        }
        taken.push(spot);
        out[i] = (spot.0 as f64, spot.1 as f64);
    }
    out
}

/// 八个方向:横竖与两条对角线
const DIRS8: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// 把边的走向整成横、竖或 45 度:让端点挪到同一个方向上最近的空格.
/// 这样画出来才是干净的一笔,而不是一格一格爬的阶梯.
fn octilinear(p: &mut [(f64, f64)], edges: &[(usize, usize)], cfg: &Cfg) {
    for _ in 0..cfg.rounds(24) {
        let mut taken: std::collections::HashSet<(i32, i32)> =
            p.iter().map(|q| (q.0 as i32, q.1 as i32)).collect();
        let mut moved = false;
        for &(a, b) in edges {
            let (ax, ay) = (p[a].0 as i32, p[a].1 as i32);
            let (bx, by) = (p[b].0 as i32, p[b].1 as i32);
            let (dx, dy) = (bx - ax, by - ay);
            if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
                continue;
            }
            // 先试挪 b;b 周围满了就把 a 挪到以 b 为中心的同方向格子上
            let spot = clean_cell((ax, ay), (bx, by), &taken, cfg.nudge())
                .map(|c| (b, c))
                .or_else(|| clean_cell((bx, by), (ax, ay), &taken, cfg.nudge()).map(|c| (a, c)));
            let Some((who, c)) = spot else { continue };
            let old = (p[who].0 as i32, p[who].1 as i32);
            taken.remove(&old);
            taken.insert(c);
            p[who] = (c.0 as f64, c.1 as f64);
            moved = true;
        }
        if !moved {
            break;
        }
    }
}

/// 以 base 为圆心、在 8 个方向上找一个空格,使它到 want 最近(用来把边掰直)
fn clean_cell(
    base: (i32, i32),
    want: (i32, i32),
    taken: &std::collections::HashSet<(i32, i32)>,
    nudge: i32,
) -> Option<(i32, i32)> {
    let l = (want.0 - base.0).abs().max((want.1 - base.1).abs());
    let mut best: Option<(i64, (i32, i32))> = None;
    for len in [l - 1, l, l + 1, l + 2] {
        if len < 1 {
            continue;
        }
        for (ux, uy) in DIRS8 {
            let c = (base.0 + ux * len, base.1 + uy * len);
            // 只许就近挪几格:不然节点会被越推越远,整幅图炸开
            if (c.0 - want.0).abs().max((c.1 - want.1).abs()) > nudge {
                continue;
            }
            if !room_at(c, taken, want) {
                continue;
            }
            let d = ((c.0 - want.0) as i64).pow(2) + ((c.1 - want.1) as i64).pow(2);
            if best.map(|(s, _)| d < s).unwrap_or(true) {
                best = Some((d, c));
            }
        }
    }
    best.map(|(_, c)| c)
}

/// 任意两站都不能挨在一起(不只是线路上的相邻站):
/// 挨着画出来就是两个点糊在一块,市中心最容易这样
fn spread_all(p: &mut [(f64, f64)], cfg: &Cfg) {
    for _ in 0..cfg.rounds(40) {
        // 一格可能摞了不止一站(前面的收尾偶尔会撞上),所以这里用多重映射
        let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
        for (i, q) in p.iter().enumerate() {
            grid.entry((q.0 as i32, q.1 as i32)).or_default().push(i);
        }
        let mut taken: std::collections::HashSet<(i32, i32)> = grid.keys().copied().collect();
        let mut moved = false;
        for i in 0..p.len() {
            let (x, y) = (p[i].0 as i32, p[i].1 as i32);
            let mut close: Option<usize> = None;
            // 同一格里的另一站也算"贴在一起"
            if let Some(cell) = grid.get(&(x, y)) {
                if let Some(&j) = cell.iter().find(|&&j| j > i) {
                    close = Some(j);
                }
            }
            for dy in -1..=1i32 {
                for dx in -1..=1i32 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    if let Some(cell) = grid.get(&(x + dx, y + dy)) {
                        if let Some(&j) = cell.iter().find(|&&j| j > i) {
                            close = Some(j);
                        }
                    }
                }
            }
            let Some(j) = close else { continue };
            // 谁挪都行:先试 i,再试 j;每种先找"周围八格也空着"的位子,
            // 实在没有才退一步只要求这一格空
            let r = cfg.radius();
            let spot = best_cell(p[j], p[i], &taken, true, r)
                .map(|c| (i, c))
                .or_else(|| best_cell(p[i], p[j], &taken, true, r).map(|c| (j, c)))
                .or_else(|| best_cell(p[j], p[i], &taken, false, r).map(|c| (i, c)))
                .or_else(|| best_cell(p[i], p[j], &taken, false, r).map(|c| (j, c)));
            if let Some((who, c)) = spot {
                let old = (p[who].0 as i32, p[who].1 as i32);
                if let Some(cell) = grid.get_mut(&old) {
                    cell.retain(|&k| k != who);
                }
                grid.entry(c).or_default().push(who);
                // taken 也要跟着改:不然它还以为旧格占着、新格空着,
                // 后面就会把别的站搬进刚占上的这一格(格子少的时候必撞)
                taken.remove(&old);
                taken.insert(c);
                p[who] = (c.0 as f64, c.1 as f64);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

/// 相邻两站至少要隔一格:不然中间那截线会被两个站点字符吃掉,线看起来是断的
fn separate(p: &mut [(f64, f64)], edges: &[(usize, usize)], cfg: &Cfg) {
    for _ in 0..cfg.rounds(60) {
        let mut taken: std::collections::HashSet<(i32, i32)> =
            p.iter().map(|q| (q.0 as i32, q.1 as i32)).collect();
        let mut moved = false;
        for &(a, b) in edges {
            let (dx, dy) = (p[b].0 - p[a].0, p[b].1 - p[a].1);
            if dx.abs().max(dy.abs()) >= 2.0 {
                continue;
            }
            // 先挪 b:在附近找一个离 a 更远的空格,挪不动就反过来挪 a.
            // 先找"周围也空着"的位子,实在没有才退而求其次只要求这一格空
            let r = cfg.radius();
            let spot = best_cell(p[a], p[b], &taken, true, r)
                .map(|c| (b, c))
                .or_else(|| best_cell(p[b], p[a], &taken, true, r).map(|c| (a, c)))
                .or_else(|| best_cell(p[a], p[b], &taken, false, r).map(|c| (b, c)))
                .or_else(|| best_cell(p[b], p[a], &taken, false, r).map(|c| (a, c)));
            let Some((who, c)) = spot else { continue };
            taken.remove(&(p[who].0 as i32, p[who].1 as i32));
            taken.insert(c);
            p[who] = (c.0 as f64, c.1 as f64);
            moved = true;
        }
        if !moved {
            break;
        }
    }
}

/// 候选格能不能站:自己空着,周围 8 格也不能有别的站.
/// 只有"要挪走的那一站"自己现在占的那一格可以不算 —— 那个位子马上就空出来了.
/// (别的站占的格子一律算占着,不然会把两个站摞到同一格上)
fn room_at(c: (i32, i32), taken: &std::collections::HashSet<(i32, i32)>, own: (i32, i32)) -> bool {
    let blocked = |q: (i32, i32)| taken.contains(&q) && q != own;
    if blocked(c) {
        return false;
    }
    !DIRS8.iter().any(|(ux, uy)| blocked((c.0 + ux, c.1 + uy)))
}

/// 在 keep 周围 5x5 里挑一个空格:离 from 越远越好,同时别离 keep 太远
fn best_cell(
    from: (f64, f64),
    keep: (f64, f64),
    taken: &std::collections::HashSet<(i32, i32)>,
    strict: bool,
    radius: i32,
) -> Option<(i32, i32)> {
    let (kx, ky) = (keep.0 as i32, keep.1 as i32);
    let own = (kx, ky);
    let mut best: Option<(f64, (i32, i32))> = None;
    // 杭州是 7x7:挤成一坨的地方(换乘站扎堆)近处常常一个空位都没有,站多了还要更大
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            let c = (kx + dx, ky + dy);
            if strict && !room_at(c, taken, own) {
                continue;
            }
            if !strict && taken.contains(&c) {
                continue;
            }
            let d_from = ((c.0 as f64 - from.0).powi(2) + (c.1 as f64 - from.1).powi(2)).sqrt();
            let score = d_from * 3.0 - (dx * dx + dy * dy) as f64;
            if best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((score, c));
            }
        }
    }
    best.map(|(_, c)| c)
}

/// 把地理坐标松开成示意图坐标(单位是字符格,x 向右、y 向下)
pub fn schematic(map: &Map, w: f64, h: f64) -> Vec<(f64, f64)> {
    let n = map.pos.len();
    let cfg = Cfg::of(n);
    let (x0, y0, x1, y1) = map.bbox;
    let (gw, gh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    // 起点:地理坐标铺进 w x h 的格子(y 翻转成屏幕方向)
    let mut p: Vec<(f64, f64)> = map
        .pos
        .iter()
        .map(|&(x, y)| {
            (
                (x - x0) / gw * (w - 6.0) + 3.0,
                (1.0 - (y - y0) / gh) * (h - 4.0) + 2.0,
            )
        })
        .collect();
    let anchor_raw = p.clone();
    let edges: Vec<(usize, usize)> = map.edges.iter().map(|e| (e.a, e.b)).collect();
    // 同一条线上前后夹着的中间站:(前一站, 中间站, 后一站).
    // 按线路算,不是按整图度数 —— 换乘站也是某条线上的中间站,同样要拉直
    let mut per_line: Vec<Vec<Vec<usize>>> = vec![vec![Vec::new(); n]; map.lines.len()];
    for e in &map.edges {
        per_line[e.line][e.a].push(e.b);
        per_line[e.line][e.b].push(e.a);
    }
    let mut bends: Vec<(usize, usize, usize)> = Vec::new();
    for line in &per_line {
        for i in 0..n {
            // 这条线上正好两个邻居才算中间站:线路端点、分叉口的方向本来就不定
            if line[i].len() == 2 {
                bends.push((line[i][0], i, line[i][1]));
            }
        }
    }
    // 锚点跟着当前布局一起缩放:只保留"相对方位",不让地理上稀疏的地方把整幅图撑大
    let raw_box = bbox(&anchor_raw);
    let raw_c = ((raw_box.0 + raw_box.2) / 2.0, (raw_box.1 + raw_box.3) / 2.0);
    let raw_size = ((raw_box.2 - raw_box.0).max(1.0), (raw_box.3 - raw_box.1).max(1.0));

    let mut anchor = anchor_raw.clone();
    for it in 0..ITERS {
        if it % 20 == 0 {
            let b = bbox(&p);
            let c = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
            let s = 0.5 * ((b.2 - b.0) / raw_size.0 + (b.3 - b.1) / raw_size.1);
            for i in 0..n {
                anchor[i] = (
                    c.0 + (anchor_raw[i].0 - raw_c.0) * s,
                    c.1 + (anchor_raw[i].1 - raw_c.1) * s,
                );
            }
        }
        let mut f = vec![(0.0f64, 0.0f64); n];
        for &(a, b) in &edges {
            let (dx, dy) = (p[b].0 - p[a].0, p[b].1 - p[a].1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (ux, uy) = (dx / len, dy / len);
            // 长度弹簧
            let fl = (len - STEP) * 0.30;
            f[a].0 += ux * fl;
            f[a].1 += uy * fl;
            f[b].0 -= ux * fl;
            f[b].1 -= uy * fl;
            // 方向吸附
            let (tx, ty) = snap_dir(dx, dy);
            let (px, py) = (-ty, tx);
            let c = (dx * px + dy * py) * 0.30;
            f[a].0 += px * c;
            f[a].1 += py * c;
            f[b].0 -= px * c;
            f[b].1 -= py * c;
        }
        for &(a, b, c) in &bends {
            // 中间站往两边连线的中点靠,一站一站的小折角就会被抹平
            f[b].0 += ((p[a].0 + p[c].0) / 2.0 - p[b].0) * K_STRAIGHT;
            f[b].1 += ((p[a].1 + p[c].1) / 2.0 - p[b].1) * K_STRAIGHT;
        }
        repel(&p, &mut f, 0.7);
        for i in 0..n {
            // 锚:整幅图别跑偏
            f[i].0 += (anchor[i].0 - p[i].0) * ANCHOR_W;
            f[i].1 += (anchor[i].1 - p[i].1) * ANCHOR_W;
            p[i].0 += f[i].0.clamp(-1.2, 1.2);
            p[i].1 += f[i].1.clamp(-1.2, 1.2);
        }
    }
    let mut out = snap_to_grid(&p, &cfg);
    // 拉直、拉开、分开,三件事互相会打断对方.挑位子时都尽量选"周围也空着"的格子,
    // 来回几轮磨到互相让步;最后再兜一次底,保证线路上的相邻站一定隔开一格
    for _ in 0..6 {
        separate(&mut out, &edges, &cfg);
        spread_all(&mut out, &cfg);
        octilinear(&mut out, &edges, &cfg);
    }
    separate(&mut out, &edges, &cfg);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Map;

    fn spread(p: &[(f64, f64)], map: &Map) -> (f64, f64, f64) {
        let mut v: Vec<f64> = map
            .edges
            .iter()
            .map(|e| {
                let (dx, dy) = (p[e.a].0 - p[e.b].0, p[e.a].1 - p[e.b].1);
                (dx * dx + dy * dy).sqrt()
            })
            .collect();
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (v[0] / mean, v[v.len() / 2] / mean, v[v.len() - 1] / mean * 1.0)
    }

    #[test]
    fn 示意图把站距拉齐() {
        let text = include_str!("../docs/hangzhou-subway.dot");
        let map = Map::from_dot(text).expect("内嵌线路图");
        let sch = schematic(&map, 170.0, 46.0);
        let (gmin, gmed, gmax) = spread(&map.pos, &map);
        let (smin, smed, smax) = spread(&sch, &map);
        println!("地理   min/med/max = {gmin:.2} {gmed:.2} {gmax:.2}");
        println!("示意图 min/med/max = {smin:.2} {smed:.2} {smax:.2}");
        let b = bbox(&sch);
        println!("示意图范围 {:.0} x {:.0} 格", b.2 - b.0, b.3 - b.1);
        let tight = map
            .edges
            .iter()
            .filter(|e| {
                let (dx, dy) = (sch[e.a].0 - sch[e.b].0, sch[e.a].1 - sch[e.b].1);
                dx.abs().max(dy.abs()) < 2.0
            })
            .count();
        println!("贴在一起的相邻站 {tight} 对");
        assert!(smax < gmax * 0.75, "示意图最长的边应该短不少");
        assert!(smin > gmin * 1.5, "示意图最短的边应该长一些");
        // 极密的市中心总有一两对挤不出空格,守住"几乎没有"就行
        assert!(tight <= 2, "相邻站贴在一起: {tight}");
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    use crate::map::Map;

    #[test]
    fn 边的方向大多是干净的() {
        let text = include_str!("../docs/hangzhou-subway.dot");
        let map = Map::from_dot(text).expect("内嵌线路图");
        let (nw, nh) = crate::map::nominal_for(map.pos.len());
        let p = schematic(&map, nw, nh);
        let mut dirty = 0;
        let mut long = 0;
        for e in &map.edges {
            let (dx, dy) = ((p[e.a].0 - p[e.b].0).abs() as i32, (p[e.a].1 - p[e.b].1).abs() as i32);
            if dx != 0 && dy != 0 && dx != dy {
                dirty += 1;
                if dx.max(dy) >= 3 {
                    long += 1;
                }
            }
        }
        println!("不是横竖 45 度的边: {dirty}/{} (其中长的一笔 {long})", map.edges.len());
        // 一条线上连着三站共线(同排或同列)的比例:越高画出来越像长走廊
        let mut nb: Vec<Vec<usize>> = vec![Vec::new(); map.names.len()];
        for e in &map.edges {
            nb[e.a].push(e.b);
            nb[e.b].push(e.a);
        }
        let mut straight = 0;
        let mut total = 0;
        for i in 0..map.names.len() {
            if nb[i].len() != 2 {
                continue;
            }
            let (a, b) = (nb[i][0], nb[i][1]);
            total += 1;
            let (pa, pb, pc) = (p[a], p[i], p[b]);
            if (pa.0 == pb.0 && pb.0 == pc.0) || (pa.1 == pb.1 && pb.1 == pc.1) {
                straight += 1;
            }
        }
        println!("三站共线 {straight}/{total}");
        // 10 号线 和睦-北大桥-翠柏路-学院路-文三路 这一串必须一路往南,
        // 不然按方向键就会"跳过"下一站(曾经 学院路 被挤到 翠柏路 北边,成了个回环)
        let row = |n: &str| {
            let i = map.names.iter().position(|x| x == n).unwrap();
            p[i].1
        };
        let chain = ["和睦", "北大桥", "翠柏路", "学院路", "文三路", "下宁桥", "古翠路", "沈塘桥"];
        println!(
            "坐标: {}",
            chain.iter().map(|n| { let i = map.names.iter().position(|x| x == n).unwrap(); format!("{n}=({:.0},{:.0})", p[i].0, p[i].1) }).collect::<Vec<_>>().join(" ")
        );
        println!(
            "这一片的行号: {}",
            chain.iter().map(|n| format!("{n}={:.0}", row(n))).collect::<Vec<_>>().join(" ")
        );
        for w in chain[..5].windows(2) {
            assert!(
                row(w[0]) <= row(w[1]),
                "{} 跑到 {} 南边去了,线在这里绕回来了",
                w[1],
                w[0]
            );
        }
        // 密密麻麻的地方总有掰不直的:短的两步台阶画出来也还像一笔,
        // 真正难看的是长台阶,所以盯住长的那一类
        assert!(long <= 30, "long staircase edges: {long}");
    }
}


#[cfg(test)]
mod density {
    use super::*;
    use crate::map::Map;
    use ratatui::layout::Rect;
    use crate::ui::{layout, View};

    /// 屏幕上每一站到最近邻的距离(用程序真正用的那套投影,别自己另算一遍)
    fn screen_nn(m: &Map, p: &[(f64, f64)]) -> Vec<f64> {
        let sc = crate::map::Scene::from_pos_for_test(p.to_vec());
        let lay = layout(m, &sc, Rect::new(0, 0, 200, 50), &View::default());
        let cells: Vec<(i32, i32)> = p.iter().map(|q| lay.proj.cell(*q)).collect();
        let mut out = Vec::new();
        for i in 0..cells.len() {
            let mut best = f64::MAX;
            for j in 0..cells.len() {
                if i == j {
                    continue;
                }
                let d = (cells[i].0 - cells[j].0).abs().max((cells[i].1 - cells[j].1).abs()) as f64;
                best = best.min(d);
            }
            out.push(best);
        }
        out.sort_by(f64::total_cmp);
        out
    }

    /// 站一多,画布/搜索半径/迭代轮数要跟着放大,不然后处理找不到空位,站会摞在一格.
    /// 这里用生成的 600 站大图当探针(上海那个量级)
    #[test]
    fn 大图也不会把站摞在一起() {
        let (cols, rows) = (30usize, 20usize);
        let mut dot = String::from("graph g {\n");
        for r in 0..rows {
            for c in 0..cols {
                dot += &format!(
                    "  \"s{}\" [pos=\"{},{}\"];\n",
                    r * cols + c,
                    c * 1000,
                    r * 1000
                );
            }
        }
        for r in 0..rows {
            for c in 0..cols {
                let i = r * cols + c;
                if c + 1 < cols {
                    dot += &format!(
                        "  \"s{i}\" -- \"s{}\" [color=\"#112233\", line=\"{}号线\"];\n",
                        i + 1,
                        r + 1
                    );
                }
                if r + 1 < rows {
                    dot += &format!(
                        "  \"s{i}\" -- \"s{}\" [color=\"#112233\", line=\"{}号线\"];\n",
                        i + cols,
                        r + 1
                    );
                }
            }
        }
        dot += "}\n";
        let m = Map::from_dot(&dot).expect("合成图");
        let (w, h) = crate::map::nominal_for(m.pos.len());
        let t0 = std::time::Instant::now();
        let p = schematic(&m, w, h);
        let mut cells: Vec<(i64, i64)> = p
            .iter()
            .map(|q| (q.0.round() as i64, q.1.round() as i64))
            .collect();
        cells.sort();
        let dup = cells.windows(2).filter(|w| w[0] == w[1]).count();
        println!(
            "{} 站:画布 {}x{},布局 {:?},摞在同一格的 {} 对",
            m.pos.len(),
            w as i64,
            h as i64,
            t0.elapsed(),
            dup
        );
        assert_eq!(dup, 0, "有站摞在同一格");
    }

    #[test]
    fn 屏幕上不该有挨着的站() {
        let text = include_str!("../docs/hangzhou-subway.dot");
        let map = Map::from_dot(text).unwrap();
        let mut counts = Vec::new();
        for (name, p) in [
            ("示意", {
                let (nw, nh) = crate::map::nominal_for(map.pos.len());
                schematic(&map, nw, nh)
            }),
            ("地理", map.pos.clone()),
        ] {
            let nn = screen_nn(&map, &p);
            let hist = |t: f64| nn.iter().filter(|d| **d <= t).count();
            println!(
                "{name}: 屏幕上最近邻 中位 {:.0} 格; <=1 格 {} 站, <=2 格 {} 站",
                nn[nn.len() / 2],
                hist(1.0),
                hist(2.0)
            );
            counts.push((hist(1.0), nn[nn.len() / 2]));
        }
        // 示意图在 200x50 的屏上不该有站贴着站
        assert_eq!(counts[0].0, 0, "示意图上还有挨着的站");
        assert!(counts[0].1 >= 2.0, "示意图最近邻太近");
        // 真实地理版本来就挤,只留个对照
        assert!(counts[1].0 > 50, "地理版反而变稀了?");
    }
}
