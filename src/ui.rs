// 绘制:先把地理坐标投影到字符格,再把线路画成 ASCII 折线.
// 一条线路一种颜色,换乘站用 '#' 标出来,其余站点是线路色的小圆点 'o'.
use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::map::{Map, Scene};

/// 每行显示宽度(全角算两格)
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 看的是真实地理还是示意图
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Real,
    Schematic,
}

use crate::lang::Lang;

/// 一段平移动画:按**时长**缓动(和地图软件一个路子),不是"每帧走固定格数"
#[derive(Clone, Copy)]
pub struct Pan {
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub start: std::time::Instant,
    /// 走多少秒(远的多给一点,但有封顶)
    pub dur: f64,
}

pub struct View {
    pub mode: Mode,
    /// 正在输入的搜索词(None = 不在搜索)
    pub query: Option<String>,
    /// 搜索命中/高亮的站(会一直留着,方便按 n 一个个跳)
    pub hits: Vec<usize>,
    /// 聚焦的线路(只留这几条线的颜色,别的压暗)
    pub focus: Vec<usize>,
    /// 当前跳到第几个
    pub hit_at: usize,
    /// 放大倍数(1 = 铺满整个区域)
    pub zoom: f64,
    /// 视图中心(画布坐标),None = 用整幅图的中心
    pub center: Option<(f64, f64)>,
    /// 平移目标(见 main.rs 的 animate)
    pub target: Option<(f64, f64)>,
    /// 正在进行的平移
    pub pan: Option<Pan>,
    pub cursor: usize,
    /// 是否画出所有站名
    pub labels: bool,
    /// 光标站"呼吸"的相位:0 = 本站原色,1 = 纯红(主循环按时间算出来)
    pub pulse: f64,
    /// 跳过的站(搜到的、鼠标点的、半屏跳的),Ctrl-o / Ctrl-i 前后翻
    pub history: Vec<usize>,
    pub hist_at: usize,
    /// 是否显示键位速查
    pub help: bool,
    /// 绕行容忍强度:同线那一档允许比全局最优远几倍(见 step_cfg 的说明).
    /// None = 按现状(聚焦时 2.0,没聚焦时 1.0),用 :cycle 显式设过就按设的来
    pub cycle: Option<f64>,
    /// 侧偏权重:打分里"横向偏斜"乘几倍(默认 2.0,可用 :side 调).调小就更认远近
    pub side: f64,
    /// ":" 命令行的输入缓冲
    pub cmd: Option<String>,
    /// 底部提示(命令回显 / 方向选择的依据)
    pub note: Option<String>,
    /// 界面语言:只影响应用自己的提示文字(状态栏、速查、选城市、":" 回显);
    /// 命令行那套(-h 帮助与报错)固定英文,不跟着它走
    pub lang: Lang,
}

impl Default for View {
    fn default() -> Self {
        View {
            mode: Mode::Real,
            query: None,
            hits: Vec::new(),
            focus: Vec::new(),
            hit_at: 0,
            zoom: 1.0,
            center: None,
            target: None,
            pan: None,
            cursor: 0,
            labels: false,
            pulse: 0.0,
            history: Vec::new(),
            hist_at: 0,
            help: false,
            cycle: None,
            side: 2.0,
            cmd: None,
            note: None,
            lang: Lang::default(),
        }
    }
}

/// 画布坐标 -> 字符格的投影
#[derive(Clone, Copy)]
pub struct Proj {
    /// 每列多少画布单位
    pub scale: f64,
    cx: f64,
    cy: f64,
    /// 一行的物理高度相当于几列(真实地理 2,示意图 1)
    aspect: f64,
    flip_y: bool,
    x0: i32,
    y0: i32,
}

impl Proj {
    pub fn cell(&self, p: (f64, f64)) -> (i32, i32) {
        let dx = ((p.0 - self.cx) / self.scale).round() as i32;
        let dy = ((p.1 - self.cy) / (self.scale * self.aspect)).round() as i32;
        (self.x0 + dx, if self.flip_y { self.y0 - dy } else { self.y0 + dy })
    }
}

/// 把视图中心按**屏幕格**平移(dcols 列、drows 行,正数向右/向下),返回新的世界坐标中心.
/// 滚轮平移就靠它:先换算中心的格坐标,挪几格,再换回世界坐标
pub fn pan_center(proj: &Proj, from: (f64, f64), dcols: i32, drows: i32) -> (f64, f64) {
    let (x, y) = proj.cell(from);
    let (nx, ny) = (x + dcols, y + drows);
    let wx = proj.cx + (nx - proj.x0) as f64 * proj.scale;
    let wy = if proj.flip_y {
        proj.cy - (ny - proj.y0) as f64 * proj.scale * proj.aspect
    } else {
        proj.cy + (ny - proj.y0) as f64 * proj.scale * proj.aspect
    };
    (wx, wy)
}

/// 铺满整个区域时,每列对应多少画布单位
pub fn fit_scale(scene: &Scene, w: u16, h: u16) -> f64 {
    let (wd, hd) = scene.size();
    let cols = (w as f64 - 4.0).max(8.0);
    let rows = (h as f64 - 2.0).max(4.0) * scene.aspect;
    (wd / cols).max(hd / rows)
}

/// 图例占的宽度(放不下就是 0)
fn legend_width(map: &Map, area_w: u16, lang: Lang) -> u16 {
    let text = map
        .lines
        .iter()
        .map(|l| width(&l.label))
        .chain(std::iter::once(width(lang.pick("换乘站 #", "interchange #"))))
        .max()
        .unwrap_or(0) as u16;
    let need = text + 8;
    if area_w as u32 > need as u32 * 2 + 40 {
        need
    } else {
        0
    }
}

/// 格子上的方向位:一格会和上下左右四个方向连
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const UP: u8 = 4;
const DOWN: u8 = 8;


/// a 和 c 相对 b 在同一侧(都偏北/南/东/西)时,线在 b 这里应该是"竖着进"或"横着进";
/// 一进一出(正常穿过)就交给主方向自己定.返回 true 表示要求横着进站
fn arrive_hor(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> Option<bool> {
    let (da, dc) = (a.1 - b.1, c.1 - b.1);
    if da != 0 && dc != 0 && da.signum() == dc.signum() {
        return Some(false); // 两站都在上边或都在下边:竖着进
    }
    let (xa, xc) = (a.0 - b.0, c.0 - b.0);
    if xa != 0 && xc != 0 && xa.signum() == xc.signum() {
        return Some(true); // 都在左边或都在右边:横着进
    }
    None
}

/// 压暗用的灰
const DIM_COLOR: Color = Color::Rgb(66, 74, 96);

/// 键位速查:中文
pub const HELP_KEYS_ZH: &[(&str, &str)] = &[
    ("h j k l / 方向键", "往那个方向挑最近、最正的那一站(5l = 连挪 5 站)"),
    ("H J K L", "只走图上相邻的那一站(这条线的下一站)"),
    ("Ctrl-d / Ctrl-u", "半屏往下 / 往上跳一站(远距离旅行)"),
    ("Ctrl-o / Ctrl-i", "回到上一个 / 下一个跳过的站"),
    ("n / N", "下一个 / 上一个搜索命中"),
    ("/", "搜站名、拼音、首字母;纯数字 = 聚焦那条线"),
    ("V", "聚焦当前站的线路(再按一次取消)"),
    ("f", "把聚焦的线路(没聚焦就是当前站的线)铺满画面"),
    ("z / 空格", "把当前站挪到画面正中"),
    ("+ - / 0", "放大 / 缩小 / 复位"),
    ("b", "开关站名标注"),
    ("r", "示意图 / 真实地理"),
    ("鼠标左键", "点哪站就跳到哪站"),
    ("q / esc", "回选城市界面"),
    (":", "命令行(调试):cycle <倍数> 同线档位允许远多少,side <倍数> 横向偏斜算几倍距离,lang <zh|en> 界面语言"),
];

/// 键位速查:英文
pub const HELP_KEYS_EN: &[(&str, &str)] = &[
    ("h j k l / arrows", "nearest station on screen in that direction (5l = move 5 stations)"),
    ("H J K L", "one station along the line (previous / next)"),
    ("Ctrl-d / Ctrl-u", "jump half a screen down / up (long trips)"),
    ("Ctrl-o / Ctrl-i", "back to the previous / next station you jumped to"),
    ("n / N", "next / previous search hit"),
    ("/", "search name / pinyin / initials; a bare number focuses that line"),
    ("V", "focus the line(s) through this station (again to clear)"),
    ("f", "fit the focused line (or this station's line) to the screen"),
    ("z / space", "centre this station"),
    ("+ - / 0", "zoom in / out / reset"),
    ("b", "toggle station labels"),
    ("r", "schematic / geographic"),
    ("mouse left", "click a station to jump to it"),
    ("q / esc", "back to the city picker"),
    (":", "command line: cycle <x> same-line tolerance, side <x> sideways weight, lang <zh|en>"),
];

/// 当前语言的键位表
pub fn help_keys(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::Zh => HELP_KEYS_ZH,
        Lang::En => HELP_KEYS_EN,
    }
}

/// 当前那一站画成实心的,一眼就能认出"我在这儿"
const GLYPH_STATION_CURSOR: char = '●';
const GLYPH_INTERCHANGE_CURSOR: char = '◉';

/// 光标那一站:在平时那个亮白和红色之间按心跳节奏来回插值
const CURSOR_WHITE: (u8, u8, u8) = (246, 248, 252);
const BLINK_RED: (u8, u8, u8) = (255, 62, 56);

/// 闪的强度曲线:一个正弦,phase 是 0..1 的循环位置.
/// 0 -> 1 -> 0,两头平滑停住(正弦的斜率在端点本来就是 0),所以闪起来不生硬
pub fn pulse_at(phase: f64) -> f64 {
    0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()
}

/// 按强度把颜色往红色拉:t = 0 是原色,t = 1 是纯红
fn pulse_color(c: Color, t: f64) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c; // 具名色不动
    };
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    Color::Rgb(mix(r, BLINK_RED.0), mix(g, BLINK_RED.1), mix(b, BLINK_RED.2))
}

/// 站点字形:换乘站是靶心,普通站是空心圆
const GLYPH_INTERCHANGE: char = '◎';
const GLYPH_STATION: char = '○';

// 方向位的组合(模式匹配里要当作一个整体,不能现算)
const LR: u8 = LEFT | RIGHT;
const UD: u8 = UP | DOWN;
const LRD: u8 = LEFT | RIGHT | DOWN;
const LRU: u8 = LEFT | RIGHT | UP;
const UDR: u8 = UP | DOWN | RIGHT;
const UDL: u8 = UP | DOWN | LEFT;
const LRUD: u8 = LEFT | RIGHT | UP | DOWN;
const DR: u8 = DOWN | RIGHT;
const DL: u8 = DOWN | LEFT;
const UR: u8 = UP | RIGHT;
const UL: u8 = UP | LEFT;

/// 方向位 -> 制表符(lazygit 那套:横竖 + 圆角 + 三通四通)
fn box_char(mask: u8) -> char {
    match mask {
        0 => ' ',
        LRUD => '┼',
        LRD => '┬',
        LRU => '┴',
        UDR => '├',
        UDL => '┤',
        LR => '─',
        UD => '│',
        DR => '╭',
        DL => '╮',
        UR => '╰',
        UL => '╯',
        _ => '│',
    }
}

/// 画布:线路只记"这一格往哪几个方向连",最后按方向位合成制表符,
/// 这样拐弯处自然就是 ╭╮╯╰,交叉处自然就是 ┼,不用逐段猜字符
struct Canvas {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    mask: Vec<u8>,
    fg: Vec<(u8, u8, u8)>,
    /// 这一格的颜色是第几条线路占的(-1 = 还没人占)
    owner: Vec<i16>,
    /// 占住这一格的那条线在这一格的方向位(别的线从这儿过就"断一格")
    owner_mask: Vec<u8>,
    /// 这一格的颜色是不是"光标那条线"的
    mine: Vec<bool>,
    /// 站点:0 没有,1 普通站,2 换乘站
    station: Vec<u8>,
    /// 这一格上有哪几条线(位掩码,最多 64 条够用)
    lines_here: Vec<u64>,
    /// 作为站点,这一站属于哪几条线(穿过这一格的别的线不算)
    station_lines: Vec<u64>,
    /// 所有站点的格子:画线时先避开它们,别从站身上穿过去
    station_cells: std::collections::HashSet<(i32, i32)>,
    /// 聚焦的线路(位掩码;0 = 不聚焦).没聚焦时所有线都正常画
    focus: u64,
    /// 光标所在站的那几条线:颜色冲突时它们说了算
    priority: Vec<bool>,
}

impl Canvas {
    fn new(area: Rect, priority: Vec<bool>, focus: u64) -> Canvas {
        let (w, h) = (area.width as i32, area.height as i32);
        Canvas {
            x0: area.x as i32,
            y0: area.y as i32,
            w,
            h,
            mask: vec![0; (w * h) as usize],
            fg: vec![(0, 0, 0); (w * h) as usize],
            owner: vec![-1; (w * h) as usize],
            owner_mask: vec![0; (w * h) as usize],
            mine: vec![false; (w * h) as usize],
            station: vec![0; (w * h) as usize],
            lines_here: vec![0; (w * h) as usize],
            station_lines: vec![0; (w * h) as usize],
            station_cells: std::collections::HashSet::new(),
            focus,
            priority,
        }
    }

    /// 一格只能有一种颜色,也只能有一种形状.
    /// 颜色谁说了算:光标所在站的那条线 > 先画的那条线 —— 于是两条线挤在同一格时,
    /// 颜色会跟着光标走,从当前站就能顺着颜色找到自己那条线.
    /// 形状只按"先画的说了算",**不跟光标走**:不然挪一下光标,谁在这里断一格就翻到
    /// 另一条线上,整段线的字形都跟着变,看着就是线在变形
    fn claim(&mut self, i: usize, line: usize, bit: u8, fg: (u8, u8, u8)) {
        let cur = self.owner[i];
        if cur == line as i16 {
            self.owner_mask[i] |= bit;
            if self.priority[line] {
                self.fg[i] = fg; // 这一格本来就是自己占的,颜色归自己
            }
            return;
        }
        if cur < 0 {
            self.owner[i] = line as i16;
            self.owner_mask[i] = bit;
            self.fg[i] = fg;
            self.mine[i] = self.priority[line];
        } else if self.priority[line] && !self.priority[cur as usize] {
            self.fg[i] = fg; // 只让颜色,形状留给先画的那条线
        }
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return None;
        }
        Some(((y - self.y0) * self.w + (x - self.x0)) as usize)
    }

    /// 这一格有没有东西(站名标注只往空位上写)
    fn at(&self, x: i32, y: i32) -> u8 {
        match self.idx(x, y) {
            Some(i) => {
                let mut k = self.station[i];
                if self.mask[i] != 0 {
                    k = k.max(1);
                }
                k
            }
            None => u8::MAX,
        }
    }

    /// 在 (x, y) 记上"这里和某个方向连着"
    fn link(&mut self, x: i32, y: i32, bit: u8, line: usize, fg: (u8, u8, u8)) {
        let Some(i) = self.idx(x, y) else { return };
        self.mask[i] |= bit;
        self.lines_here[i] |= 1 << line;
        self.claim(i, line, bit, fg);
    }

    /// 一段直线(同一行或同一列)
    fn run(&mut self, a: (i32, i32), b: (i32, i32), line: usize, fg: (u8, u8, u8)) {
        if a.1 == b.1 {
            let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
            for x in x0..x1 {
                self.link(x, a.1, RIGHT, line, fg);
                self.link(x + 1, a.1, LEFT, line, fg);
            }
        } else if a.0 == b.0 {
            let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
            for y in y0..y1 {
                self.link(a.0, y, DOWN, line, fg);
                self.link(a.0, y + 1, UP, line, fg);
            }
        }
    }

    /// 一段直线上的格子(含首尾)
    fn span(&self, a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        if a.1 == b.1 {
            for x in a.0.min(b.0)..=a.0.max(b.0) {
                out.push((x, a.1));
            }
        } else if a.0 == b.0 {
            for y in a.1.min(b.1)..=a.1.max(b.1) {
                out.push((a.0, y));
            }
        }
        out
    }

    /// 从 from 出发、只走这条线已经画过的格子,能不能走到 to
    fn joined(&self, line: usize, from: (i32, i32), to: (i32, i32)) -> bool {
        let bit = 1u64 << line;
        let has = |p: (i32, i32)| {
            self.idx(p.0, p.1)
                .map(|i| self.lines_here[i] & bit != 0)
                .unwrap_or(false)
        };
        if !has(from) || !has(to) {
            return false;
        }
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![from];
        seen.insert(from);
        while let Some(p) = stack.pop() {
            if p == to {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let q = (p.0 + dx, p.1 + dy);
                if has(q) && seen.insert(q) {
                    stack.push(q);
                }
            }
        }
        false
    }

    /// 这条折线会踩到多少**别的线**已经画过的格子(复用自己的走廊不算代价,
    /// 否则"下去再上来"会因为想省格子而绕到旁边去,围出一个方框)
    fn cost(&self, a: (i32, i32), corner: (i32, i32), b: (i32, i32), line: usize) -> i32 {
        let bit = 1u64 << line;
        let mut n = 0;
        // 这一笔里有没有"这条线还没占过"的格子
        let fresh = self
            .span(a, corner)
            .into_iter()
            .chain(self.span(corner, b))
            .any(|p| {
                self.idx(p.0, p.1)
                    .map(|i| self.lines_here[i] & bit == 0)
                    .unwrap_or(false)
            });
        // 两端在这条线已经画过的地方是通的,而这一笔又要踩新格子:
        // 那就等于把自己绕成一个环(图上一个封闭的圈),尽量别这么拐
        if fresh && (self.joined(line, a, corner) || self.joined(line, corner, b)) {
            n += 1000;
        }
        // 再把"封口"也算进去:这一笔落下之后,如果某一格的四条边都成了这条线的笔画,
        // 那一格就被自己围住了(视觉上就是个方框),这种拐法也要尽量躲开
        let path: std::collections::HashSet<(i32, i32)> =
            self.span(a, corner).into_iter().chain(self.span(corner, b)).collect();
        let mine = |p: (i32, i32)| {
            path.contains(&p)
                || self
                    .idx(p.0, p.1)
                    .map(|i| self.lines_here[i] & bit != 0)
                    .unwrap_or(false)
        };
        let mut encloses = false;
        'outer: for &(x, y) in &path {
            for (dx, dy) in [(-1i32, -1i32), (-1, 0), (0, -1), (0, 0)] {
                let c = (x + dx, y + dy);
                if mine(c) && mine((c.0 + 1, c.1)) && mine((c.0, c.1 + 1)) && mine((c.0 + 1, c.1 + 1)) {
                    encloses = true;
                    break 'outer;
                }
            }
        }
        if encloses {
            n += 1000;
        }
        for p in self.span(a, corner).into_iter().chain(self.span(corner, b)) {
            if p == a || p == b {
                continue; // 两个端点本来就是站
            }
            if let Some(i) = self.idx(p.0, p.1) {
                if self.lines_here[i] != 0 && self.lines_here[i] & bit == 0 {
                    n += 1;
                }
            }
            // 从站身上穿过去很难看:线会画进那个 ○ 里,还容易和别的段围出方框
            if self.station_cells.contains(&p) {
                n += 100;
            }
        }
        n
    }

    /// 一条边:同排或同列直接连,否则拐一个直角.
    ///
    /// 拐法有两种(先横后竖 / 先竖后横).`arrive_hor` 是这一站要求的"进站方向"
    /// (由前后两站的位置定,见 arrive_hor):要求竖着进,就先横着走再竖着进,
    /// 于是同一条线"下去再上来"会走同一列,不会绕着站点围出一个方框.
    /// 实在会多踩别人很多格子,才让位给"踩得少"的那种
    fn edge(
        &mut self,
        a: (i32, i32),
        b: (i32, i32),
        line: usize,
        fg: (u8, u8, u8),
        arrive_hor: Option<bool>,
    ) {
        if a == b || a.1 == b.1 && a.0 == b.0 {
            return;
        }
        if a.1 == b.1 || a.0 == b.0 {
            self.run(a, b, line, fg);
            return;
        }
        let hor_first = (b.0, a.1);
        let ver_first = (a.0, b.1);
        let (c1, c2) = (self.cost(a, hor_first, b, line), self.cost(a, ver_first, b, line));
        // 要求竖着进站(进站那一段是竖的)就先横后竖
        let prefer_hor = arrive_hor.map(|h| !h).unwrap_or((b.0 - a.0).abs() >= (b.1 - a.1).abs());
        let corner = if prefer_hor {
            if c1 <= c2 + 1 {
                hor_first
            } else {
                ver_first
            }
        } else if c2 <= c1 + 1 {
            ver_first
        } else {
            hor_first
        };
        self.run(a, corner, line, fg);
        self.run(corner, b, line, fg);
    }

    /// line_mask:这一站经过的所有线路;line:用哪个颜色(fg 也是它的)
    fn station_at(
        &mut self,
        x: i32,
        y: i32,
        interchange: bool,
        line_mask: u64,
        line: usize,
        fg: (u8, u8, u8),
    ) {
        let Some(i) = self.idx(x, y) else { return };
        self.station[i] = if interchange { 2 } else { 1 };
        self.lines_here[i] |= line_mask;
        self.station_lines[i] = line_mask;
        self.owner[i] = line as i16;
        self.owner_mask[i] = 0;
        self.fg[i] = fg;
        self.mine[i] = self.priority[line];
    }
}

fn blit(
    buf: &mut Buffer,
    cv: &Canvas,
    bright: Style,
    cursor: Option<(i32, i32)>,
    pulse: f64,
) {
    for y in cv.y0..cv.y0 + cv.h {
        for x in cv.x0..cv.x0 + cv.w {
            let Some(i) = cv.idx(x, y) else { continue };
            if cv.station[i] == 0 && cv.mask[i] == 0 {
                continue;
            }
            let (r, g, b) = cv.fg[i];
            let ch = match cv.station[i] {
                2 => GLYPH_INTERCHANGE,
                1 => GLYPH_STATION,
                // 非站点处:如果别的线也打这儿过,就按"占住这一格的那条线"自己的形状画,
                // 后来的那条断一格 —— 不然满屏 ┼ ├ ┬,看起来到处是换乘,其实那儿什么都没有
                _ if cv.mask[i].count_ones() >= 3 && cv.owner_mask[i] != cv.mask[i] => {
                    box_char(cv.owner_mask[i])
                }
                _ => box_char(cv.mask[i]),
            };
            let mut style = Style::default().fg(Color::Rgb(r, g, b));
            // 换乘站平时是亮白的,一眼能认出来;但如果这一站也在光标那条线上,
            // 就用那条线的颜色,这样顺着颜色能把整条线连起来看
            if cv.station[i] == 2 && !cv.mine[i] {
                style = bright;
            }
            // 聚焦某条线时,别的线(点也好笔画也好)全压暗,只留聚焦那几条线的颜色
            let dimmed = if cv.focus == 0 {
                false
            } else if cv.station[i] != 0 {
                // 站点格:看这一站自己在不在聚焦的线上(别的线只是从这儿路过)
                cv.station_lines[i] & cv.focus == 0
            } else {
                cv.lines_here[i] & cv.focus == 0
            };
            if dimmed {
                style = Style::default().fg(DIM_COLOR);
            }
            let mut ch = ch;
            if cursor == Some((x, y)) {
                // 当前站:实心字形 + 平时那个亮白 <-> 红色,按心跳节奏插值.
                // 背景高亮和加粗保持不变,这样闪到哪一档都看得见它在哪
                ch = match cv.station[i] {
                    2 => GLYPH_INTERCHANGE_CURSOR,
                    1 => GLYPH_STATION_CURSOR,
                    _ => ch,
                };
                let white = Color::Rgb(CURSOR_WHITE.0, CURSOR_WHITE.1, CURSOR_WHITE.2);
                style = Style::default()
                    .fg(pulse_color(white, pulse))
                    .bg(Color::Rgb(46, 56, 96))
                    .add_modifier(Modifier::BOLD);
            }
            let cell = &mut buf[(x as u16, y as u16)];
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
}

/// 光标朝 dir 方向走一站.
///
/// 小写 `h j k l` / 方向键:那个方向的 45 度锥形里,**屏幕上最近的一站**就是它.
/// 例外是"同一条线"那一档(见 step_cfg):线上的站容忍 cycle 倍距离优先,
/// 于是 /1 之后方向键基本就是沿着 1 号线走.
///
/// `step` 的老名字:等价于 cycle = 2.0 的那次调用(只给测试和老调用点用)
#[cfg(test)]
pub fn step(
    map: &Map,
    scene: &Scene,
    lay: &Layout,
    cursor: usize,
    dir: (i32, i32),
    focus: &[usize],
) -> usize {
    step_choice(map, scene, lay, cursor, dir, focus, 2.0).0
}

/// 固定侧偏权重(2.0)的 step_cfg,只给测试用
#[cfg(test)]
pub fn step_choice(
    map: &Map,
    scene: &Scene,
    lay: &Layout,
    cursor: usize,
    dir: (i32, i32),
    focus: &[usize],
    cycle: f64,
) -> (usize, Vec<(usize, f64, bool)>) {
    step_cfg(map, scene, lay, cursor, dir, focus, cycle, 2.0)
}

/// 方向键选站的完整过程:返回 (选中, 按分数排好的候选 [(站, 分数, 是否同线)]).
///
/// 打分 `距离 + side_w x 横向偏斜`.候选分两档:
///   0 档 = "同一条线"的站(聚焦了某几条线就按它们算;没聚焦就按**光标站自己的线**)
///   1 档 = 其它
/// 从 0 档往下看这一档最好的:只要不超过全局最优的 cycle 倍就走它 ——
/// cycle 调小就更认"屏幕上最近",调大就更认"同一条线";side_w 调小则纯看远近.
pub fn step_cfg(
    map: &Map,
    scene: &Scene,
    lay: &Layout,
    cursor: usize,
    dir: (i32, i32),
    focus: &[usize],
    cycle: f64,
    side_w: f64,
) -> (usize, Vec<(usize, f64, bool)>) {
    let cur = lay.proj.cell(scene.pos[cursor]);
    let (dc, dr) = (dir.0 as f64, dir.1 as f64);
    let mut cands: Vec<(u8, f64, usize, bool)> = Vec::new();
    for (i, p) in scene.pos.iter().enumerate() {
        if i == cursor {
            continue;
        }
        let c = lay.proj.cell(*p);
        let (dx, dy) = ((c.0 - cur.0) as f64, (c.1 - cur.1) as f64);
        let along = dx * dc + dy * dr;
        if along <= 0.0 {
            continue; // 不在这个方向上
        }
        let side = (dx * dr - dy * dc).abs();
        if side > along {
            continue; // 偏出去超过 45 度,不算"这个方向"
        }
        // "同一条线"这一档:聚焦了几条线就按它算;没聚焦就按光标站自己的线算,
        // 这样 :cycle 在平时按 j/k 也有效(不然只有按过 V 才看得出来)
        let mine: &[usize] = if focus.is_empty() {
            &map.station_lines[cursor]
        } else {
            focus
        };
        let focused = map.station_lines[i].iter().any(|l| mine.contains(l));
        let tier = if focused { 0 } else { 1 };
        // 分数:距离 + 2 倍的横向偏斜.偏斜权重要压得住"斜着挨着"的站 ——
        // 不然按 k 会挑右上/左上挨着的那一站,而不是正上方那一站
        let score = (dx * dx + dy * dy).sqrt() + side * side_w;
        cands.push((tier, score, i, focused));
    }
    cands.sort_by(|a, b| {
        (a.0, a.1)
            .partial_cmp(&(b.0, b.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let ranked: Vec<(usize, f64, bool)> = cands.iter().map(|&(_, s, i, f)| (i, s, f)).collect();
    let all_min = cands
        .iter()
        .map(|&(_, s, _, _)| s)
        .fold(f64::INFINITY, f64::min);
    // 0 档最好的那个只要不超过全局最优的 cycle 倍,就走它(cycle 设 0 就是不额外优先)
    if let Some(&(0, score, i, _)) = cands.first() {
        if score <= cycle * all_min {
            return (i, ranked);
        }
    }
    // 否则取全局分数最低的那个(全都够不着就原地不动)
    let best = cands
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|&(_, _, i, _)| i)
        .unwrap_or(cursor);
    (best, ranked)
}

/// 沿线走一站:返回候选 (站点, 线路).
///
/// `next` 为真取下一站、为假取上一站;顺序是**线路自己的站序**(路径里的先后),
/// 和真实地理位置、屏幕方向都无关.聚焦了某几条线时只算这几条;
/// 同一个站在多条线/多条支路上都出现时只留一个候选,并记住是哪条线带过去的.
pub fn line_step(map: &Map, cursor: usize, next: bool, focus: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (line, path) in map.line_paths().iter() {
        if !focus.is_empty() && !focus.contains(line) {
            continue;
        }
        let Some(i) = path.iter().position(|&s| s == cursor) else {
            continue;
        };
        let j = if next {
            i + 1
        } else if i == 0 {
            continue;
        } else {
            i - 1
        };
        if j >= path.len() {
            continue;
        }
        let st = path[j];
        if st == cursor || out.iter().any(|&(s, _)| s == st) {
            continue;
        }
        out.push((st, *line));
    }
    out
}

/// 上一次/下一站的候选列表浮层(H K 是上一站,J L 是下一站;有好几个就让用户挑)
pub fn render_choice(
    buf: &mut Buffer,
    area: Rect,
    map: &Map,
    cands: &[(usize, usize)],
    at: usize,
    next: bool,
    lang: Lang,
) {
    let title = if next {
        lang.pick("下一站走哪条线?", "which line to the next station?")
    } else {
        lang.pick("上一站走哪条线?", "which line to the previous station?")
    };
    let hint = lang.pick(
        "数字 / j k 选,回车走,esc 取消",
        "digits / j k to pick, Enter to go, Esc to cancel",
    );
    let texts: Vec<String> = cands
        .iter()
        .enumerate()
        .map(|(i, &(st, _line))| {
            let mark = if i == at { ">" } else { " " };
            format!("{mark} {} {}", i + 1, map.names[st])
        })
        .collect();
    let w = texts
        .iter()
        .map(|t| width(t))
        .chain([
            width(title),
            width(hint),
            cands
                .iter()
                .map(|&(_, l)| width(&map.lines[l].label) + 8)
                .max()
                .unwrap_or(0),
        ])
        .max()
        .unwrap_or(20) as u16;
    let h = texts.len() as u16 + 4;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    for yy in y..(y + h).min(area.y + area.height) {
        for xx in x..(x + w).min(area.x + area.width) {
            buf[(xx, yy)]
                .set_char(' ')
                .set_style(Style::default().bg(Color::Rgb(24, 28, 44)));
        }
    }
    let bright = Style::default()
        .fg(Color::Rgb(246, 248, 252))
        .bg(Color::Rgb(24, 28, 44))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default()
        .fg(Color::Rgb(170, 178, 200))
        .bg(Color::Rgb(24, 28, 44));
    buf.set_string(x + 2, y + 1, truncate_to(title, w as usize - 4), bright);
    for (i, t) in texts.iter().enumerate() {
        let yy = y + 2 + i as u16;
        let style = if i == at { bright } else { dim };
        buf.set_string(x + 2, yy, truncate_to(t, w as usize - 4), style);
        // 右边标一下是哪条线带过来的
        let label = &map.lines[cands[i].1].label;
        let lx = x + w - 2 - width(label) as u16;
        if lx > x + width(t) as u16 {
            buf.set_string(lx, yy, label, dim);
        }
    }
    buf.set_string(
        x + 2,
        y + h - 2,
        truncate_to(hint, w as usize - 4),
        Style::default()
            .fg(DIM_COLOR)
            .bg(Color::Rgb(24, 28, 44)),
    );
}

/// 计算布局(交互时移动光标也要用)
pub struct Layout {
    pub proj: Proj,
    pub area: Rect,
    pub legend_w: u16,
}

pub fn layout(map: &Map, scene: &Scene, area: Rect, view: &View) -> Layout {
    let status = 1.min(area.height);
    let body = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(status),
    };
    let legend_w = legend_width(map, body.width, view.lang);
    let map_area = Rect {
        width: body.width.saturating_sub(legend_w),
        ..body
    };
    let scale = fit_scale(scene, map_area.width, map_area.height) / view.zoom.max(0.2);
    let (cx, cy) = view.center.unwrap_or_else(|| scene.center());
    // 把中心对齐到字符格:一行的画布高度是 scale*aspect
    // 相机的小数位置只让整幅图按整数格整体平移(鼠标拖动也是这个路子),
    // 不然有的站先跳一格、有的站后跳,线在移动过程中一直变形
    let (cx, cy) = (
        (cx / scale).round() * scale,
        (cy / (scale * scene.aspect)).round() * scale * scene.aspect,
    );
    Layout {
        proj: Proj {
            scale,
            cx,
            cy,
            aspect: scene.aspect,
            flip_y: scene.flip_y,
            x0: map_area.x as i32 + map_area.width as i32 / 2,
            y0: map_area.y as i32 + map_area.height as i32 / 2,
        },
        area: map_area,
        legend_w,
    }
}

/// 把这几个站的包围盒铺满地图区:返回 (zoom, center).
/// 和 layout() 里那套缩放用同一份公式,所以结果直接塞回 view 就行
pub fn fit_to(
    scene: &Scene,
    area: Rect,
    map_area: Rect,
    pts: impl Iterator<Item = (f64, f64)>,
) -> Option<(f64, (f64, f64))> {
    let mut b: Option<(f64, f64, f64, f64)> = None;
    for p in pts {
        b = Some(match b {
            None => (p.0, p.1, p.0, p.1),
            Some((x0, y0, x1, y1)) => (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1)),
        });
    }
    let (x0, y0, x1, y1) = b?;
    let (bw, bh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let cols = (map_area.width as f64 - 4.0).max(8.0);
    let rows = (map_area.height as f64 - 2.0).max(4.0) * scene.aspect;
    let want = (bw / cols).max(bh / rows);
    let full = fit_scale(scene, map_area.width, map_area.height);
    let _ = area;
    Some((
        (full / want).clamp(0.2, 64.0),
        ((x0 + x1) / 2.0, (y0 + y1) / 2.0),
    ))
}

/// 键位速查浮层
pub fn render_help(buf: &mut Buffer, area: Rect, lines: &[(&str, &str)], lang: Lang) {
    let title = lang.pick("键位速查", "key reference");
    let w = lines
        .iter()
        .map(|(k, v)| width(k) + width(v) + 4)
        .chain([width(title) as usize + 6])
        .max()
        .unwrap_or(30)
        .min(area.width as usize) as u16;
    let h = (lines.len() as u16 + 4).min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    // 铺底:把这块刷成面板色,免得和地图糊在一起
    for yy in y..(y + h).min(area.y + area.height) {
        for xx in x..(x + w).min(area.x + area.width) {
            buf[(xx, yy)]
                .set_char(' ')
                .set_style(Style::default().bg(Color::Rgb(24, 28, 44)));
        }
    }
    let bright = Style::default()
        .fg(Color::Rgb(246, 248, 252))
        .bg(Color::Rgb(24, 28, 44))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default()
        .fg(Color::Rgb(170, 178, 200))
        .bg(Color::Rgb(24, 28, 44));
    let mut put_panel = |xx: u16, yy: u16, text: &str, style: Style| {
        if yy >= area.y + area.height || xx >= area.x + area.width {
            return;
        }
        let s = truncate_to(text, (area.x + area.width - xx) as usize);
        buf.set_string(xx, yy, &s, style);
    };
    put_panel(x + 2, y + 1, title, bright);
    for (i, (k, v)) in lines.iter().enumerate() {
        let yy = y + 2 + i as u16;
        put_panel(x + 2, yy, k, bright);
        put_panel(x + 2 + width(k).max(6) as u16 + 2, yy, v, dim);
    }
    put_panel(
        x + 2,
        y + h - 2,
        lang.pick("任意键关掉", "any key closes this"),
        dim,
    );
}

pub fn render(buf: &mut Buffer, area: Rect, map: &Map, scene: &Scene, view: &View) -> Layout {
    let lay = layout(map, scene, area, view);
    if view.help {
        render_help(buf, area, help_keys(view.lang), view.lang);
        return lay;
    }
    let map_area = lay.area;
    // 颜色冲突时谁说了算:聚焦了某条线就一直归它(光标挪到别的线上也不变),
    // 没聚焦时才是光标所在站的那几条线
    let mut priority = vec![false; map.lines.len()];
    let who = if view.focus.is_empty() {
        &map.station_lines[view.cursor]
    } else {
        &view.focus
    };
    for &l in who {
        if l < priority.len() {
            priority[l] = true;
        }
    }
    // 聚焦:搜索命中涉及的那几条线(数字输入就是那几条线)
    let focus: u64 = view
        .focus
        .iter()
        .filter(|l| **l < 64)
        .fold(0u64, |m, l| m | (1 << l));
    let mut cv = Canvas::new(map_area, priority, focus);
    // 先记下所有站点的格子:画线时要避开,免得从站身上穿过去
    for p in scene.pos.iter() {
        let c = lay.proj.cell(*p);
        if cv.idx(c.0, c.1).is_some() {
            cv.station_cells.insert(c);
        }
    }
    // 1. 连线:按线路一条条走,拐弯由方向位合成 ╭╮╯╰
    for (line, path) in map.line_paths().iter() {
        let cells: Vec<(i32, i32)> = path.iter().map(|&s| lay.proj.cell(scene.pos[s])).collect();
        for k in 0..cells.len() - 1 {
            // 下一站决定这一段该"竖着进"还是"横着进"
            let arrive = if k + 2 < cells.len() {
                arrive_hor(cells[k], cells[k + 1], cells[k + 2])
            } else {
                None
            };
            cv.edge(cells[k], cells[k + 1], *line, map.line_color(*line), arrive);
        }
    }
    // 2. 站点:一站也只有一种颜色,同样让光标那条线先挑
    for (i, p) in scene.pos.iter().enumerate() {
        let (x, y) = lay.proj.cell(*p);
        let line = map.station_lines[i]
            .iter()
            .copied()
            .find(|l| cv.priority[*l])
            .unwrap_or(map.station_lines[i][0]);
        let mask = map.station_lines[i]
            .iter()
            .filter(|l| **l < 64)
            .fold(0u64, |m, l| m | (1 << l));
        cv.station_at(x, y, map.is_interchange(i), mask, line, map.line_color(line));
    }
    let cursor_cell = lay.proj.cell(scene.pos[view.cursor]);
    let bright = Style::default()
        .fg(Color::Rgb(246, 248, 252))
        .add_modifier(Modifier::BOLD);
    let dark = Style::default().fg(Color::Rgb(70, 78, 100));
    blit(buf, &cv, bright, Some(cursor_cell), view.pulse);
    // 窗口够大(站距拉开到 4 格以上)就自动把站名标出来,不然图上一堆点看不出是哪站
    let labels = view.labels || median_edge_cells(map, scene, &lay.proj) >= 4.5;
    // 3. 站名标注(画在最上层,只写在空位上):端点站一直标,站距宽了或按了 b 就全标.
    //    顺序上先给"端点/换乘"找位置,剩下的站再捡空位
    let mut order: Vec<usize> = (0..map.names.len()).collect();
    order.sort_by_key(|&i| {
        let terminus = map.adj[i].len() == 1;
        !(map.is_interchange(i) || terminus)
    });
    let mut used: HashSet<(i32, i32)> = HashSet::new();
    for i in order {
        let terminus = map.adj[i].len() == 1;
        if !labels && !terminus {
            continue;
        }
        let (x, y) = lay.proj.cell(scene.pos[i]);
        let w = width(&map.names[i]) as i32;
        // 端点:先试"背着邻居"的那个方向,标签就像从线路末端伸出去
        let mut slots = vec![
            (x + 2, y),
            (x - w - 1, y),
            (x - w / 2, y - 1),
            (x - w / 2, y + 1),
        ];
        if terminus {
            let nbr = lay.proj.cell(scene.pos[map.adj[i][0].0]);
            let (dx, dy) = ((x - nbr.0).signum(), (y - nbr.1).signum());
            slots.sort_by_key(|(lx, ly)| {
                let (ox, oy) = ((lx - x).signum(), (ly - y).signum());
                // 与"背离邻居"方向一致的最靠前
                (ox != dx || oy != dy) as u8
            });
        }
        let Some(&(lx, ly)) = slots.iter().find(|&&(lx, ly)| {
            if lx < map_area.x as i32
                || ly < map_area.y as i32
                || lx + w > (map_area.x + map_area.width) as i32
                || ly >= (map_area.y + map_area.height) as i32
            {
                return false;
            }
            for k in 0..w {
                if cv.at(lx + k, ly) != 0 || used.contains(&(lx + k, ly)) {
                    return false;
                }
            }
            used.insert((lx, ly))
        }) else {
            continue;
        };
        for k in 0..w + 1 {
            used.insert((lx + k, ly));
            used.insert((lx + k, ly + 1));
            used.insert((lx + k, ly - 1));
        }
        let on_focus = focus == 0
            || map.station_lines[i]
                .iter()
                .any(|l| *l < 64 && focus & (1 << l) != 0);
        let style = if !on_focus {
            Style::default().fg(DIM_COLOR)
        } else if map.is_interchange(i) || terminus {
            bright
        } else {
            Style::default().fg(Color::Rgb(150, 158, 180))
        };
        buf.set_string(lx as u16, ly as u16, &map.names[i], style);
    }
    // 4. 图例
    if lay.legend_w > 0 {
        let lx = map_area.x + map_area.width + 2;
        let rows = map.lines.len() as u16 + 4;
        let ly = map_area.y + map_area.height.saturating_sub(rows) / 2;
        put(buf, lx, ly, view.lang.pick("线路", "lines"), dark);
        for (i, l) in map.lines.iter().enumerate() {
            let (r, g, b) = l.color;
            let lit = focus == 0 || (i < 64 && focus & (1 << i) != 0);
            let style = if !lit {
                Style::default().fg(DIM_COLOR)
            } else {
                Style::default().fg(Color::Rgb(r, g, b))
            };
            put(buf, lx, ly + 1 + i as u16, &format!("── {}", l.label), style);
        }
        let base = ly + 1 + map.lines.len() as u16 + 1;
        let dark = if focus != 0 {
            Style::default().fg(DIM_COLOR)
        } else {
            dark
        };
        put(buf, lx, base, view.lang.pick("○ 普通站", "○ station"), dark);
        put(
            buf,
            lx,
            base + 1,
            view.lang.pick("◎ 换乘站", "◎ interchange"),
            Style::default()
                .fg(Color::Rgb(246, 248, 252))
                .add_modifier(Modifier::BOLD),
        );
    }
    // 5. 状态行(搜索中就是输入行)
    let status_y = area.y + area.height - 1;
    if let Some(q) = &view.query {
        let prompt = format!("/{q}");
        put(buf, area.x, status_y, &prompt, Style::default().fg(Color::Rgb(238, 226, 104)));
        let x = area.x + width(&prompt) as u16;
        let l = view.lang;
        let what = if q.trim().is_empty() {
            l.pick(
                "站名 / 拼音 / 首字母(比如 wlgc),或线路号",
                "station name / pinyin / initials (e.g. wlgc), or a line number",
            )
            .to_string()
        } else if q.trim().chars().all(|c| c.is_ascii_digit()) {
            let hit_lines = crate::search::lines(map, q.trim());
            if hit_lines.is_empty() {
                l.pick("没有这条线", "no such line").to_string()
            } else {
                let labels = hit_lines
                    .iter()
                    .map(|x| map.lines[*x].label.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let n = view.hits.len();
                l.pick_owned(format!("聚焦 {labels}:{n} 站"), format!("focus {labels}: {n} stations"))
            }
        } else if view.hits.is_empty() {
            l.pick("没找到", "no match").to_string()
        } else {
            let cur = view.hits[view.hit_at.min(view.hits.len() - 1)];
            let more = if view.hits.len() > 1 { l.pick(",n 下一个", ", n for next") } else { "" };
            let (n, name) = (view.hits.len(), map.names[cur].as_str());
            l.pick_owned(
                format!("{n} 处匹配:{name}{more}"),
                format!("{n} match(es): {name}{more}"),
            )
        };
        put(buf, x + 2, status_y, &what, dark);
        let tip = l.pick("Enter 确定  Esc 取消", "Enter to confirm, Esc to cancel");
        let tw = width(tip) as u16;
        if area.width > tw + 4 {
            put(buf, area.x + area.width - tw, status_y, tip, dark);
        }
        return lay;
    }
    let name = &map.names[view.cursor];
    let mut x = area.x;
    x += draw(buf, x, status_y, view.lang.pick("站点 ", "station "), dark);
    x += draw(
        buf,
        x,
        status_y,
        name,
        Style::default()
            .fg(Color::Rgb(246, 248, 252))
            .add_modifier(Modifier::BOLD),
    );
    for line in &map.station_lines[view.cursor] {
        let (r, g, b) = map.line_color(*line);
        x += draw(buf, x, status_y, "  ", dark);
        x += draw(
            buf,
            x,
            status_y,
            &map.lines[*line].label,
            Style::default().fg(Color::Rgb(r, g, b)),
        );
    }
    // 聚焦状态在状态行上说一声
    let mut x = x;
    if !view.focus.is_empty() {
        let names = view
            .focus
            .iter()
            .map(|l| map.lines[*l].label.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        x += draw(
            buf,
            x,
            status_y,
            &view.lang.pick_owned(format!("  聚焦 {names}"), format!("  focus {names}")),
            dark,
        );
    }
    let hint = if let Some(buf) = &view.cmd {
        format!(":{buf}")
    } else if let Some(n) = &view.note {
        n.clone()
    } else {
        view.lang.pick_owned(
            format!(
                "x{:.1}  hjkl 移动  +/- 缩放  空格 居中  0 复位  / 搜索  b 站名  r 模式  q 回选城市",
                view.zoom
            ),
            format!(
                "x{:.1}  hjkl move  +/- zoom  space centre  0 reset  / search  b labels  r mode  q cities",
                view.zoom
            ),
        )
    };
    let hw = width(&hint) as u16;
    // 相邻站:把这条线上前后两站列出来(空间不够就少列几个)
    let mut nbrs: Vec<&str> = Vec::new();
    for (n, _) in &map.adj[view.cursor] {
        let name = map.names[*n].as_str();
        if !nbrs.contains(&name) {
            nbrs.push(name);
        }
    }
    let joined = nbrs.join(" ");
    let room =
        area.width.saturating_sub(hw + 4 + width(view.lang.pick("相邻 ", "nearby ")) as u16) as usize;
    if !nbrs.is_empty() && room > 6 {
        x += draw(buf, x, status_y, view.lang.pick("  相邻 ", "  nearby "), dark);
        draw(
            buf,
            x,
            status_y,
            &truncate_to(&joined, room),
            Style::default().fg(Color::Rgb(170, 178, 200)),
        );
    }
    if area.width > hw + 4 && x + 2 < area.x + area.width - hw {
        put(buf, area.x + area.width - hw, status_y, &hint, dark);
    }
    lay
}

/// 按显示宽度截断
fn truncate_to(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 相邻两站在屏幕上大概隔几格(用来决定要不要自动标站名)
fn median_edge_cells(map: &Map, scene: &Scene, proj: &Proj) -> f64 {
    let mut v: Vec<f64> = map
        .edges
        .iter()
        .map(|e| {
            let a = proj.cell(scene.pos[e.a]);
            let b = proj.cell(scene.pos[e.b]);
            (((a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)) as f64).sqrt()
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// 写一段文字,返回实际占用的列数
fn draw(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) -> u16 {
    if x >= buf.area.width || y >= buf.area.height {
        return 0;
    }
    let max = (buf.area.width - x) as usize;
    let s = truncate_to(text, max);
    let w = width(&s) as u16;
    buf.set_string(x, y, &s, style);
    w
}

/// 选城市界面:一屏放不下时,以选中那行为中心开一个窗口.
/// 返回 (第一行可见的行号, 可见行数)—— 画和"鼠标点第几行"都用它,免得两处各算一套
pub fn chooser_window(area: Rect, n: usize, sel: usize) -> (usize, usize) {
    let room = (area.height as usize).saturating_sub(6).max(1); // 标题 + 提示 + 上下各留一行
    let count = n.min(room);
    if count == 0 {
        return (0, 0);
    }
    let half = count / 2;
    let first = sel.saturating_sub(half).min(n.saturating_sub(count));
    (first, count)
}

fn chooser_texts(rows: &[(&str, &str)], sel: usize) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let mark = if i == sel { ">" } else { " " };
            format!("{mark} {name}  {desc}")
        })
        .collect()
}

/// 鼠标点到第几行(考虑滚动:屏幕上第 k 行对应 rows 里的 first + k)
pub fn chooser_row_at(
    area: Rect,
    rows: &[(&str, &str)],
    sel: usize,
    col: u16,
    row: u16,
    lang: Lang,
) -> Option<usize> {
    let (first, count) = chooser_window(area, rows.len(), sel);
    let rects = chooser_rects(area, rows, first, count, lang);
    rects
        .iter()
        .position(|r| col >= r.x && col < r.x + r.width && row == r.y)
        .map(|k| first + k)
}

/// 屏幕上那几行的矩形
fn chooser_rects(area: Rect, rows: &[(&str, &str)], _first: usize, count: usize, lang: Lang) -> Vec<Rect> {
    let texts = chooser_texts(rows, 0);
    let w = texts
        .iter()
        .map(|t| width(t))
        .chain([width(chooser_title(lang)), width(chooser_hint(lang))])
        .max()
        .unwrap_or(0) as u16;
    let h = count as u16 + 4;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    (0..count)
        .map(|k| Rect::new(x, y + 2 + k as u16, w, 1))
        .collect()
}

pub fn chooser_title(lang: Lang) -> &'static str {
    lang.pick("metro-tui - 选一个城市", "metro-tui - pick a city")
}

pub fn chooser_hint(lang: Lang) -> &'static str {
    lang.pick(
        "j k / 方向键 选择,回车打开,q 退出",
        "j k / arrows to pick, Enter to open, q to quit",
    )
}

/// 启动时的选城市界面:居中一列,选中的那行亮起来,底下是操作提示.
/// 城市多的时候以选中那行为中心滚动,上下各提示还有几个
pub fn render_chooser(buf: &mut Buffer, area: Rect, rows: &[(&str, &str)], sel: usize, lang: Lang) {
    let texts = chooser_texts(rows, sel);
    let (first, count) = chooser_window(area, rows.len(), sel);
    let rects = chooser_rects(area, rows, first, count, lang);
    let bright = Style::default()
        .fg(Color::Rgb(246, 248, 252))
        .add_modifier(Modifier::BOLD);
    let Some(top) = rects.first() else { return };
    let (x, y) = (top.x, top.y.saturating_sub(2));
    put(buf, x, y, chooser_title(lang), bright);
    for k in 0..count {
        let i = first + k;
        let style = if i == sel {
            bright
        } else {
            Style::default().fg(Color::Rgb(150, 158, 180))
        };
        put(buf, x, rects[k].y, &texts[i], style);
    }
    let dim = Style::default().fg(DIM_COLOR);
    if first > 0 {
        let more = lang.pick_owned(
            format!("  ↑ 上面还有 {first} 个"),
            format!("  ↑ {first} more above"),
        );
        put(buf, x, y + 1, &more, dim);
    }
    let last = rects.last().map(|r| r.y).unwrap_or(y);
    if first + count < rows.len() {
        put(
            buf,
            x,
            last + 1,
            &lang.pick_owned(
                format!("  ↓ 下面还有 {} 个", rows.len() - first - count),
                format!("  ↓ {} more below", rows.len() - first - count),
            ),
            dim,
        );
    }
    put(buf, x, last + 2, chooser_hint(lang), dim);
}

/// 启动时的选城市界面:居中一列,选中的那行亮起来,底下是操作提示
/// 鼠标点在哪个站上:地图区里取离点击点最近的那一站.
/// 不设"最大距离":点哪儿都会把光标挪过去,单击必定有反应(鼠标手感要的就是这个)
pub fn station_at(scene: &Scene, lay: &Layout, area: Rect, col: u16, row: u16) -> Option<usize> {
    let (c, r) = (col as i32, row as i32);
    if c < area.x as i32
        || c >= (area.x + area.width) as i32
        || r < area.y as i32
        || r >= (area.y + area.height) as i32
    {
        return None; // 图例 / 状态行上的点击不算
    }
    let mut best: Option<(i64, usize)> = None;
    for (i, p) in scene.pos.iter().enumerate() {
        let (x, y) = lay.proj.cell(*p);
        let d = ((x - c) as i64).pow(2) + ((y - r) as i64).pow(2);
        if best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    if x >= buf.area.width || y >= buf.area.height {
        return;
    }
    let max = (buf.area.width - x) as usize;
    let s = truncate_to(text, max);
    buf.set_string(x, y, &s, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Map, Scene};

    /// 甲 在中心,四个正方向各一站,外加一个东南方向的 己;
    /// 庚 在正东但更近,不过它和 甲 不共线(用来验"同线路优先")
    /// 甲 同时属于两条线,是个换乘站
    fn sample() -> Map {
        Map::from_dot(
            "graph g {\n\
             \"甲\" [pos=\"0,0\"];\n\
             \"乙\" [pos=\"1000,0\"];\n\
             \"丙\" [pos=\"0,1000\"];\n\
             \"丁\" [pos=\"-1000,0\"];\n\
             \"戊\" [pos=\"0,-1000\"];\n\
             \"己\" [pos=\"800,-800\"];\n\
             \"庚\" [pos=\"900,0\"];\n\
             \"甲\" -- \"乙\" [color=\"#df4661\", line=\"1号线\"];\n\
             \"乙\" -- \"丙\" [color=\"#df4661\", line=\"1号线\"];\n\
             \"甲\" -- \"戊\" [color=\"#df4661\", line=\"1号线\"];\n\
             \"丁\" -- \"甲\" [color=\"#00ff00\", line=\"2号线\"];\n\
             \"甲\" -- \"己\" [color=\"#00ff00\", line=\"2号线\"];\n\
             \"己\" -- \"庚\" [color=\"#0000ff\", line=\"3号线\"];\n\
             }\n",
        )
        .expect("样例图")
    }

    #[test]
    fn 方向位合成制表符() {
        assert_eq!(box_char(LEFT | RIGHT), '─');
        assert_eq!(box_char(UP | DOWN), '│');
        assert_eq!(box_char(DOWN | RIGHT), '╭');
        assert_eq!(box_char(DOWN | LEFT), '╮');
        assert_eq!(box_char(UP | RIGHT), '╰');
        assert_eq!(box_char(UP | LEFT), '╯');
        assert_eq!(box_char(UP | DOWN | RIGHT), '├');
        assert_eq!(box_char(LEFT | RIGHT | UP | DOWN), '┼');
    }

    #[test]
    fn 圆圈与制表符按一格算() {
        // 终端里这些字符的宽度是"不确定",unicode-width 默认按半角算;
        // 这里把它钉住:哪天依赖开了 cjk 特性,图例和站名的对位就会全歪
        assert_eq!(width("○"), 1);
        assert_eq!(width("◎"), 1);
        assert_eq!(width("─"), 1);
        assert_eq!(width("╭"), 1);
    }

    /// 1 号线横着、2 号线竖着,在(不是站点的)一个格子上交叉
    fn crossing() -> Map {
        Map::from_dot(
            "graph g {\n\
             \"A1\" [pos=\"0,0\"];\n\
             \"A2\" [pos=\"2000,0\"];\n\
             \"B1\" [pos=\"1000,1000\"];\n\
             \"B2\" [pos=\"1000,-1000\"];\n\
             \"A1\" -- \"A2\" [color=\"#ff0000\", line=\"1号线\"];\n\
             \"B1\" -- \"B2\" [color=\"#00ff00\", line=\"2号线\"];\n\
             }\n",
        )
        .expect("样例图")
    }

    /// 非站点处的交叉:交叉格照"先画的那条线"自己的形状画(直通的那一笔),
    /// 另一条在这里断一格 —— 不能画成 ┼,不然看起来像这里有换乘.
    /// 这个形状**不跟光标走**:挪光标只换颜色,不然整段线的字形会跟着光标变,像在变形.
    /// 颜色仍旧跟着光标那条线,从当前站顺着颜色就能找到自己那条线
    #[test]
    fn 交叉格形状固定只换颜色() {
        let m = crossing();
        let sc = Scene::real(&m);
        let area = Rect::new(0, 0, 60, 24);
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        let mut shapes = Vec::new();
        for (who, want) in [("A1", (255, 0, 0)), ("B1", (0, 255, 0))] {
            let mut view = View::default();
            view.cursor = at(who);
            let mut buf = Buffer::empty(area);
            let lay = render(&mut buf, area, &m, &sc, &view);
            let (x, _) = lay.proj.cell(sc.pos[at("B1")]);
            let (_, y) = lay.proj.cell(sc.pos[at("A1")]);
            let cell = &buf[(x as u16, y as u16)];
            // 1 号线横着,而且先画,所以这一格始终是 "─"
            assert_eq!(cell.symbol(), "─", "交叉格该是先画那条线的直笔画");
            assert_eq!(
                cell.fg,
                Color::Rgb(want.0, want.1, want.2),
                "光标在 {who} 时,交叉点要用它那条线的颜色"
            );
            shapes.push(cell.symbol().to_string());
        }
        assert_eq!(shapes[0], shapes[1], "挪光标不该改交叉格的字形");
    }

    #[test]
    fn 北边画在上面() {
        let m = sample();
        let sc = Scene::real(&m);
        let view = View::default();
        let area = Rect::new(0, 0, 40, 20);
        let lay = layout(&m, &sc, area, &view);
        let at = |name: &str| lay.proj.cell(sc.pos[m.names.iter().position(|n| n == name).unwrap()]);
        assert!(at("丙").1 < at("戊").1, "北边的站行号要更小");
        assert!(at("丁").0 < at("乙").0, "西边的站列号要更小");
    }

    #[test]
    fn 四个方向各自挑最近的站() {
        let m = sample();
        let sc = Scene::real(&m);
        let lay = layout(&m, &sc, Rect::new(0, 0, 60, 24), &View::default());
        let at = |name: &str| m.names.iter().position(|n| n == name).unwrap();
        let from = at("甲");
        // h/l/k/j 是 (列, 行):往西、往东、往北、往南(真实坐标里 y 向北,函数内部已经翻过)
        assert_eq!(step_choice(&m, &sc, &lay, from, (-1, 0), &[], 0.0).0, at("丁"), "h 该往西");
        assert_eq!(step_choice(&m, &sc, &lay, from, (0, -1), &[], 0.0).0, at("丙"), "k 该往北");
        assert_eq!(step_choice(&m, &sc, &lay, from, (0, 1), &[], 0.0).0, at("戊"), "j 该往南");
        // 往东:庚(900)比同线的 乙(1000)近,挑最近的那个
        assert_eq!(
            step_choice(&m, &sc, &lay, from, (1, 0), &[], 0.0).0,
            at("庚"),
            "l 该往东,挑那个方向上最近的一站"
        );
        // 东南那站(己)偏出去超过 45 度,不算"东",不该被选走
        assert_ne!(step_choice(&m, &sc, &lay, from, (1, 0), &[], 0.0).0, at("己"));
    }

    #[test]
    fn cycle_决定同线优先到什么程度() {
        let m = sample();
        let sc = Scene::real(&m);
        let lay = layout(&m, &sc, Rect::new(0, 0, 60, 24), &View::default());
        let at = |name: &str| m.names.iter().position(|n| n == name).unwrap();
        let from = at("甲");
        let l1 = m.lines.iter().position(|l| l.label.contains("1号线")).unwrap();
        // 默认强度(2.0):同一条线上远一点的那站也认(大连红旗西路按 j 走万嘉就是它)
        let (a, cands) = step_choice(&m, &sc, &lay, from, (1, 0), &[l1], 2.0);
        // 收紧到 0.5:回到"屏幕上最近的那站"
        let (b, _) = step_choice(&m, &sc, &lay, from, (1, 0), &[l1], 0.5);
        println!("cycle 2.0 -> {} | cycle 0.5 -> {} | 候选 {:?}", m.names[a], m.names[b],
                 cands.iter().map(|(i, sc, f)| (m.names[*i].as_str(), *sc, *f)).collect::<Vec<_>>());
        assert_eq!(m.names[a], "乙", "默认强度下认同线的乙");
        assert_eq!(m.names[b], "庚", "强度调小后认屏幕上更近的庚");
    }

    /// 选城市界面:城市都列出来,选中的那行带标记,底下有提示
    #[test]
    fn 选城市界面列出所有城市() {
        let rows = [("杭州", "13 条线,262 站"), ("上海", "20 条线,508 站")];
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        render_chooser(&mut buf, Rect::new(0, 0, 40, 12), &rows, 1, Lang::Zh);
        // 中日韩字占两格,后一格是空的,拼的时候要跨过去
        let joined = (0..12)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < 40 {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    // 宽字(中日韩)后面那一格是占位,跨过去
                    x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("杭州"), "该列出杭州");
        assert!(joined.contains("上海"), "该列出上海");
        assert!(joined.contains("> 上海"), "选中的那行要有标记");
        assert!(!joined.contains("> 杭州"), "没选中的不该有标记");
        assert!(joined.contains("回车打开"), "要有操作提示");
    }

    /// 鼠标点站:点在自己格子上就是它,离得远的格子/地图区外的格子都不认
    #[test]
    fn 鼠标点哪一站() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let view = View::default();
        let area = Rect::new(0, 0, 200, 50);
        let lay = layout(&m, &sc, area, &view);
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        for name in ["武林广场", "湘湖", "萧山国际机场", "火车东站"] {
            let i = at(name);
            let (x, y) = lay.proj.cell(sc.pos[i]);
            assert!(
                x >= lay.area.x as i32 && x < (lay.area.x + lay.area.width) as i32,
                "{name} 不在画面里,换个站试"
            );
            assert_eq!(
                station_at(&sc, &lay, lay.area, x as u16, y as u16),
                Some(i),
                "点在 {name} 自己的格子上该选它"
            );
        }
        // 空白角落:仍旧有反应,选的是离它最近的那一站
        let corner = (1u16, 1u16);
        let near = station_at(&sc, &lay, lay.area, corner.0, corner.1).expect("点哪儿都该有站");
        let (nx, ny) = lay.proj.cell(sc.pos[near]);
        let nearest = (0..sc.pos.len())
            .map(|i| {
                let (x, y) = lay.proj.cell(sc.pos[i]);
                (((x - 1) as i64).pow(2) + ((y - 1) as i64).pow(2), i)
            })
            .min()
            .unwrap()
            .1;
        assert_eq!(near, nearest, "该选离点击点最近的那一站,实际选了 ({nx},{ny})");
        // 地图区外面(比如状态行)一律不认
        assert_eq!(
            station_at(&sc, &lay, lay.area, 5, area.height - 1),
            None,
            "状态行上的点击该忽略"
        );
    }

    /// 选城市界面的行命中:行矩形和画出来的一致
    #[test]
    fn 选城市界面点行() {
        let rows = [("杭州", "13 条线,262 站"), ("上海", "21 条线,418 站")];
        let area = Rect::new(0, 0, 40, 12);
        // 一屏放得下时,屏幕上的第 k 行就是第 k 个城市
        let (first, count) = chooser_window(area, rows.len(), 0);
        assert_eq!((first, count), (0, 2));
        let rects = chooser_rects(area, &rows, first, count, Lang::Zh);
        for (i, r) in rects.iter().enumerate() {
            assert_eq!(chooser_row_at(area, &rows, 0, r.x + 2, r.y, Lang::Zh), Some(i));
        }
        assert_eq!(chooser_row_at(area, &rows, 0, 1, 1, Lang::Zh), None, "标题那行不算");
        let last_y = rects.last().unwrap().y;
        assert_eq!(
            chooser_row_at(area, &rows, 0, 2, last_y + 3, Lang::Zh),
            None,
            "提示那行不算"
        );
    }

    /// 城市多了要能以选中那行为中心滚动
    #[test]
    fn 选城市界面会滚动() {
        let rows: Vec<(String, String)> = (0..40)
            .map(|i| (format!("城{}", i), format!("{} 站", i * 10)))
            .collect();
        let refs: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let area = Rect::new(0, 0, 60, 20);
        let (first, count) = chooser_window(area, refs.len(), 0);
        assert_eq!(first, 0, "选中第一行时窗口贴顶");
        assert!(count < refs.len(), "放不下才需要滚动");
        let (first, _) = chooser_window(area, refs.len(), 25);
        assert!(first > 0 && first <= 25, "选中中间那行时窗口要跟着移:{first}");
        let (first, count) = chooser_window(area, refs.len(), 39);
        assert_eq!(first + count, 39 + 1, "选中末尾时窗口贴底");
        // 画出来的那几行里,选中行一定可见
        let at = 25usize;
        let (first, count) = chooser_window(area, refs.len(), at);
        assert!(at >= first && at < first + count);
    }

    /// 大写 H K / J L:沿线走一站,顺序是线路自己的站序,和方位无关
    #[test]
    fn 大写沿线走上一站下一站() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let l2 = m.lines.iter().position(|l| l.label == "2号线").unwrap();
        let cur = at("凤起路");
        // 1 号线上 龙翔桥 -> 凤起路 -> 武林广场
        let prev = line_step(&m, cur, false, &[l1]);
        let next = line_step(&m, cur, true, &[l1]);
        assert_eq!(prev.len(), 1);
        assert_eq!(next.len(), 1);
        assert_eq!(m.names[prev[0].0], "龙翔桥", "H/K 该是这条线的上一站");
        assert_eq!(m.names[next[0].0], "武林广场", "J/L 该是这条线的下一站");
        assert_eq!(prev[0].1, l1);
        // 聚焦 2 号线:站序是 中河北路 -> 凤起路 -> 武林门
        assert_eq!(m.names[line_step(&m, cur, false, &[l2])[0].0], "中河北路");
        assert_eq!(m.names[line_step(&m, cur, true, &[l2])[0].0], "武林门");
        // 没聚焦:两条线都能走 -> 两个候选,交给用户挑
        let all = line_step(&m, cur, false, &[]);
        let names: Vec<&str> = all.iter().map(|&(s, _)| m.names[s].as_str()).collect();
        println!("凤起路 没聚焦时的上一站候选: {names:?}");
        assert_eq!(all.len(), 2, "1/2 号线各给一个候选");
        assert!(names.contains(&"龙翔桥") && names.contains(&"中河北路"));
        // 端点:1 号线第一站没有上一站
        let first = at("湘湖");
        assert!(line_step(&m, first, false, &[l1]).is_empty());
        assert!(!line_step(&m, first, true, &[l1]).is_empty());
    }

    /// 候选列表浮层:把站名和它属于哪条线都画出来
    #[test]
    fn 选择浮层列出候选() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let cands = vec![(at("龙翔桥"), l1), (at("武林门"), m.lines.iter().position(|l| l.label == "2号线").unwrap())];
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        render_choice(&mut buf, area, &m, &cands, 1, false, Lang::Zh);
        let text: String = (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < area.width {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("上一站"), "标题该说明是上一站还是下一站");
        assert!(text.contains("龙翔桥") && text.contains("武林门"), "候选站名都要在");
        assert!(text.contains("1号线") && text.contains("2号线"), "候选来自哪条线也要标");
        assert!(text.contains("> 2 武林门"), "选中的那行要有标记");
    }

    /// ? 浮层:画的是键位速查,不是地图
    #[test]
    fn 帮助浮层() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = Rect::new(0, 0, 90, 30);
        let mut view = View::default();
        view.help = true;
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &m, &sc, &view);
        let text: String = (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < area.width {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("键位速查"), "该有标题");
        assert!(text.contains("半屏往下"), "该列出半屏跳");
        assert!(text.contains("任意键关掉"), "该提示怎么关");
        assert!(!text.contains("○○"), "浮层打开时不该还在画地图");
    }

    /// 当前站画成实心,并按正弦在亮白和红之间闪
    #[test]
    fn 当前站实心又按心跳闪红() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = Rect::new(0, 0, 200, 50);
        let cell_at = |name: &str, pulse: f64| {
            let want = m.names.iter().position(|n| n == name).unwrap();
            let mut view = View::default();
            view.cursor = want;
            view.pulse = pulse;
            let mut buf = Buffer::empty(area);
            let lay = render(&mut buf, area, &m, &sc, &view);
            let (x, y) = lay.proj.cell(sc.pos[want]);
            (buf[(x as u16, y as u16)].fg, buf[(x as u16, y as u16)].symbol().to_string())
        };
        // 实心:换乘站 ◉、普通站 ●(不闪的时候也照样是实心)
        let (_, ch) = cell_at("武林广场", 0.0);
        assert_eq!(ch, GLYPH_INTERCHANGE_CURSOR.to_string(), "换乘站该画成实心");
        let (_, ch2) = cell_at("湘湖", 0.0);
        assert_eq!(ch2, GLYPH_STATION_CURSOR.to_string(), "普通站该画成实心");
        // 颜色:两端 + 中间
        let (white, _) = cell_at("武林广场", 0.0);
        let (half, _) = cell_at("武林广场", 0.5);
        let (red, _) = cell_at("武林广场", 1.0);
        assert_eq!(white, Color::Rgb(CURSOR_WHITE.0, CURSOR_WHITE.1, CURSOR_WHITE.2));
        assert_eq!(red, Color::Rgb(BLINK_RED.0, BLINK_RED.1, BLINK_RED.2));
        let chan = |c: Color| match c {
            Color::Rgb(r, _, _) => r as i32,
            _ => -1,
        };
        assert!(chan(half) > chan(white) && chan(half) < chan(red), "中间该在两端之间");
    }

    /// 闪的曲线是正弦:0 起、1 顶、0 收,而且平滑(相邻取样不会跳变)
    #[test]
    fn 闪烁曲线是正弦() {
        let at = |p: f64| pulse_at(p);
        assert!((at(0.0) - 0.0).abs() < 1e-9, "从 0 起");
        assert!((at(0.25) - 0.5).abs() < 1e-9, "四分之一处到一半");
        assert!((at(0.5) - 1.0).abs() < 1e-9, "半周期到顶");
        assert!((at(0.75) - 0.5).abs() < 1e-9, "对称地回落");
        // 0..0.5 单调上升
        let up: Vec<f64> = (0..50).map(|i| at(i as f64 / 100.0)).collect();
        assert!(up.windows(2).all(|w| w[1] >= w[0]), "前半程该一直变红");
        // 平滑:取样够密时,相邻差值很小(不是方波/硬闪)
        let dense: Vec<f64> = (0..400).map(|i| at(i as f64 / 400.0)).collect();
        let jump = dense
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f64, f64::max);
        println!("最陡一档 Δ={jump:.4}(正弦该很小)");
        assert!(jump < 0.02, "该是平滑正弦,实际最大跳变 {jump}");
    }

    #[test]
    fn 整幅图能塞进画布() {
        let m = sample();
        let sc = Scene::real(&m);
        let view = View::default();
        let area = Rect::new(0, 0, 60, 24);
        let lay = layout(&m, &sc, area, &view);
        for p in &sc.pos {
            let (x, y) = lay.proj.cell(*p);
            assert!(x >= lay.area.x as i32 && x < (lay.area.x + lay.area.width) as i32);
            assert!(y >= lay.area.y as i32 && y < (lay.area.y + lay.area.height) as i32);
        }
    }

    #[test]
    fn 换乘站用井号表示() {
        let m = sample();
        let sc = Scene::real(&m);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 24));
        let view = View::default();
        render(&mut buf, Rect::new(0, 0, 60, 24), &m, &sc, &view);
        assert!(buf.content().iter().any(|c| c.symbol() == "◎"), "换乘站应画成 ◎");
        assert!(buf.content().iter().any(|c| c.symbol() == "○"), "普通站应画成 ○");
    }
}

#[cfg(test)]
mod real_map {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// 把整幅图渲染一遍,光标停在 cursor 这一站
    fn render_at(cursor: &str) -> (Map, Scene, Layout, Buffer) {
        render_with(cursor, &[])
    }

    fn render_with(cursor: &str, hits: &[usize]) -> (Map, Scene, Layout, Buffer) {
        render_focus(cursor, hits, Vec::new())
    }

    /// 渲染一帧,光标停在 cursor,可以带上聚焦状态
    fn render_focus(cursor: &str, hits: &[usize], focus: Vec<usize>) -> (Map, Scene, Layout, Buffer) {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).expect("内嵌线路图");
        let sc = Scene::schematic(&m);
        let mut view = View::default();
        view.hits = hits.to_vec();
        view.focus = focus;
        view.cursor = m.names.iter().position(|n| n == cursor).expect("站名");
        let mut term = Terminal::new(TestBackend::new(200, 50)).expect("测试终端");
        let mut lay = layout(&m, &sc, Rect::new(0, 0, 200, 50), &view);
        term.draw(|f| {
            let area = f.area();
            lay = render(f.buffer_mut(), area, &m, &sc, &view);
        })
        .expect("绘制");
        (m, sc, lay, term.backend().buffer().clone())
    }

    fn color_of(m: &Map, label: &str) -> Color {
        let i = m.lines.iter().position(|l| l.label == label).expect("线路名");
        let (r, g, b) = m.lines[i].color;
        Color::Rgb(r, g, b)
    }

    fn cell_color(m: &Map, sc: &Scene, lay: &Layout, buf: &Buffer, station: &str) -> Color {
        let i = m.names.iter().position(|n| n == station).expect("站名");
        let (x, y) = lay.proj.cell(sc.pos[i]);
        buf[(x as u16, y as u16)].fg
    }

    /// 换乘站的格子只有一个颜色:光标那条线经过它,就让那条线先挑;
    /// 光标那条线不过这儿,它就保持换乘站默认的亮白
    #[test]
    fn 换乘站的颜色跟着光标那条线() {
        // 西湖文化广场 是 1/3/19 三线换乘
        let (m3, sc3, lay3, buf3) = render_at("古荡"); // 3 号线
        assert_eq!(
            cell_color(&m3, &sc3, &lay3, &buf3, "西湖文化广场"),
            color_of(&m3, "3号线"),
            "光标在 3 号线上时,这一站该用 3 号线的颜色"
        );
        let (m1, sc1, lay1, buf1) = render_at("城站"); // 1 号线 / 5 号线
        assert_eq!(
            cell_color(&m1, &sc1, &lay1, &buf1, "西湖文化广场"),
            color_of(&m1, "1号线"),
            "光标在 1 号线上时,这一站该用 1 号线的颜色"
        );
        // 5/7 号线都不经过 西湖文化广场:不该动它,还是亮白的换乘站
        let (m5, sc5, lay5, buf5) = render_at("江城路");
        let bright = cell_color(&m5, &sc5, &lay5, &buf5, "西湖文化广场");
        assert_eq!(
            bright,
            Color::Rgb(246, 248, 252),
            "光标那条线不过这儿,换乘站该保持亮白"
        );
    }

    /// 10 号线在 北大桥 这一片曾经绕回来:按 j 会越过 翠柏路 直接跳到 文三路
    #[test]
    fn 北大桥往南该是下一站翠柏路() {
        let (m, sc, lay, _) = render_at("北大桥");
        let cur = m.names.iter().position(|n| n == "北大桥").unwrap();
        let pick = step(&m, &sc, &lay, cur, (0, 1), &[]);
        assert_eq!(m.names[pick], "翠柏路", "往南该是同线的下一站,不能跳过它");
    }

    /// 小写 h j k l:方向、距离都按**屏幕格子**算 —— 眼睛看到哪站在那个方向、哪站近,就走哪站.
    /// 终端大小会改变取整,所以答案也会跟着变(这正是这个键该有的手感)。
    #[test]
    fn 小写按屏幕上的距离挑() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let lay = layout(&m, &sc, Rect::new(0, 0, 200, 50), &View::default());
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        let pick = |cur: &str, dir| m.names[step(&m, &sc, &lay, at(cur), dir, &[])].clone();
        // 200x50 的示意图里,文海南路 被画在 文泽路 的右下方(竖的比横的还多),
        // 屏幕上 云水 反而更近 —— 小写就看这个;要按线路走下一站请用大写 L
        assert_eq!(pick("文泽路", (1, 0)), "云水");
        assert_eq!(pick("文泽路", (-1, 0)), "高沙路");
        assert_eq!(pick("文海南路", (1, 0)), "下沙江滨");
        // 换终端尺寸:取整变了,答案可能就不一样 —— 这就是"按屏幕距离"的含义
        let big = layout(&m, &sc, Rect::new(0, 0, 420, 100), &View::default());
        let mut changed = Vec::new();
        for cur in 0..m.pos.len() {
            for dir in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                let a = step(&m, &sc, &lay, cur, dir, &[]);
                let b = step(&m, &sc, &big, cur, dir, &[]);
                if a != b && changed.len() < 3 {
                    changed.push(format!(
                        "{} {:?}: 200x50 是 {},420x100 是 {}",
                        m.names[cur], dir, m.names[a], m.names[b]
                    ));
                }
            }
        }
        for c in &changed {
            println!("{c}");
        }
        assert!(!changed.is_empty(), "小写是看屏幕的,换尺寸该有差别");
    }

    /// 大写 H J K L:强制走图上相邻的那一站(这条线的下一站),不看屏幕也不看锥形

    /// 聚焦期间颜色优先级一直归聚焦那条线:光标挪到别的线上,画面也不变色
    #[test]
    fn 聚焦时颜色一直跟着聚焦那条线() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let (r, g, b) = m.lines[l1].color;
        // 数的时候跳过两个光标各自的格子(光标有自己的配色),只看地图区
        let count = |cursor: &str| {
            let (m, sc, lay, buf) = render_focus(cursor, &[], vec![l1]);
            let skip: Vec<(i32, i32)> = ["定安路", "龙安"]
                .iter()
                .map(|n| {
                    let i = m.names.iter().position(|x| x == n).unwrap();
                    lay.proj.cell(sc.pos[i])
                })
                .collect();
            buf.content()
                .iter()
                .enumerate()
                .filter(|(k, c)| {
                    let (x, y) = (k % 200, k / 200);
                    y < 49
                        && c.fg == Color::Rgb(r, g, b)
                        && !skip.contains(&(x as i32, y as i32))
                })
                .count()
        };
        let on_line = count("定安路"); // 1 号线上的站
        let off_line = count("龙安"); // 9 号线东端的站
        println!("光标在 1 号线上时 1 号线的格子 {on_line} 个;挪到 9 号线上 {off_line} 个");
        assert_eq!(on_line, off_line, "聚焦 1 号线时,颜色不该跟着光标换");
        assert!(on_line > 40, "1 号线的颜色总得有一大片");
    }

#[test]
    fn 聚焦的线路更优先() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let lay = layout(&m, &sc, Rect::new(0, 0, 200, 50), &View::default());
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let mut diff = Vec::new();
        for cur in 0..m.pos.len() {
            for dir in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                let plain = step_choice(&m, &sc, &lay, cur, dir, &[], 0.0).0;
                let focused = step_choice(&m, &sc, &lay, cur, dir, &[l1], 2.0).0;
                if plain != focused {
                    assert!(
                        m.station_lines[focused].contains(&l1),
                        "聚焦 1 号线时却走到了不含 1 号线的 {}",
                        m.names[focused]
                    );
                    if diff.len() < 5 {
                        diff.push(format!(
                            "{} --{:?}--> 不聚焦:{},聚焦 1 号线:{}",
                            m.names[cur], dir, m.names[plain], m.names[focused]
                        ));
                    }
                }
            }
        }
        for d in &diff {
            println!("{d}");
        }
        assert!(!diff.is_empty(), "聚焦该改变不少地方的选择");
    }

    /// 同线路优先不能太硬:独城生态公园 往南,同线的站在十几格开外,
    /// 同线路优先不能太硬:同线的站在屏幕上很远时要让位给近的那个,不能一路跨半个城
    #[test]
    fn 同线优先不能跨半个城() {
        let (m, sc, lay, _) = render_at("独城生态公园");
        let cur = m.names.iter().position(|n| n == "独城生态公园").unwrap();
        let here = lay.proj.cell(sc.pos[cur]);
        let dist = |i: usize| {
            let c = lay.proj.cell(sc.pos[i]);
            let (dx, dy) = ((c.0 - here.0) as f64, (c.1 - here.1) as f64);
            (dx, dy, (dx * dx + dy * dy).sqrt())
        };
        // 南边(行号变大)、且没偏出 45 度的候选里最近的那个
        let nearest = (0..sc.pos.len())
            .filter(|&i| i != cur)
            .map(|i| (i, dist(i)))
            .filter(|(_, (dx, dy, _))| *dy > 0.0 && dx.abs() <= *dy)
            .min_by(|a, b| a.1 .2.total_cmp(&b.1 .2))
            .expect("南边应该有人");
        let pick = step(&m, &sc, &lay, cur, (0, 1), &[]);
        let picked = dist(pick).2;
        println!(
            "往南: 选了 {} ({picked:.1} 格), 最近的候选 {} ({:.1} 格)",
            m.names[pick], m.names[nearest.0], nearest.1 .2
        );
        assert!(picked <= nearest.1 .2 * 1.7, "同线优先把距离压得太狠了");
        assert_ne!(m.names[pick], "水澄桥", "不该为了同线跨十几格");
    }

    /// 空格居中:光标那一站该落在画面正中
    #[test]
    fn 空格把当前站挪到画面中心() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let mut view = View::default();
        view.cursor = m.names.iter().position(|n| n == "武林广场").unwrap();
        let area = Rect::new(0, 0, 200, 50);
        // 没居中时它不在中间
        let c0 = layout(&m, &sc, area, &view).proj.cell(sc.pos[view.cursor]);
        // 居中(等于按空格)
        view.center = Some(sc.pos[view.cursor]);
        let lay = layout(&m, &sc, area, &view);
        let c = lay.proj.cell(sc.pos[view.cursor]);
        let cx = lay.area.x as i32 + lay.area.width as i32 / 2;
        let cy = lay.area.y as i32 + lay.area.height as i32 / 2;
        println!("居中前 {c0:?},居中后 {c:?},画面中心 ({cx}, {cy})");
        assert!((c.0 - cx).abs() <= 1 && (c.1 - cy).abs() <= 1, "该落在正中");
        assert!(c != c0, "换了个位置才对");
    }

    /// 界面语言:英文模式下,应用自己的提示文字(速查浮层、状态栏)里不该再留中文
    #[test]
    fn 英文界面不留中文() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::real(&m);
        let area = Rect::new(0, 0, 120, 30);
        let non_ascii = |buf: &Buffer| -> usize {
            buf.content()
                .iter()
                .filter(|c| c.symbol().chars().any(|ch| !ch.is_ascii()))
                .count()
        };
        // 速查浮层整块都是应用自己的文字,直接看里面还有没有非 ASCII
        for (lang, want) in [(Lang::Zh, true), (Lang::En, false)] {
            let mut view = View::default();
            view.lang = lang;
            view.help = true;
            let mut buf = Buffer::empty(area);
            render(&mut buf, area, &m, &sc, &view);
            let n = non_ascii(&buf);
            println!("{lang:?} 速查浮层里的非 ASCII 格数: {n}");
            assert_eq!(n > 0, want, "{lang:?} 的速查浮层语言不对");
        }
        // 状态栏:英文模式下最后一行该是英文提示(站名本身来自数据,不参与)
        let mut view = View::default();
        view.lang = Lang::En;
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &m, &sc, &view);
        let last: String = (0..area.width)
            .map(|x| buf[(x, area.height - 1)].symbol())
            .collect();
        println!("英文状态栏: {}", last.trim_end());
        assert!(last.contains("hjkl move"), "英文状态栏该是英文: {last}");
        assert!(!last.contains("移动"), "英文状态栏里不该有中文: {last}");
    }

    /// 平移必须是"整幅图一起走整数格":中心挪不到一格时,所有站要么原地不动,
    /// 要么一起挪同一个整数格.否则有的站先跳、有的站后跳,线在移动过程中一直变形
    /// (看起来就是"抖"),鼠标拖动之所以平滑,就是因为它本来就是整格地挪
    #[test]
    fn 平移时整幅图是刚性移动的() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = Rect::new(0, 0, 120, 40);
        let mut view = View::default();
        view.zoom = 4.0; // 中等大小,跟随光标时差不多就是这个量级
        let (cx, cy) = sc.center();
        let step = layout(&m, &sc, area, &view).proj.scale; // 一列多少画布单位
        view.center = Some((cx, cy));
        let cells = |v: &View| {
            let lay = layout(&m, &sc, area, v);
            (0..sc.pos.len())
                .map(|i| lay.proj.cell(sc.pos[i]))
                .collect::<Vec<_>>()
        };
        let base = cells(&view);
        for k in 1..10 {
            let f = k as f64 / 10.0;
            for (dx, dy) in [(step * f, 0.0), (0.0, step * sc.aspect * f)] {
                view.center = Some((cx + dx, cy + dy));
                let now = cells(&view);
                let deltas: std::collections::BTreeSet<(i32, i32)> = base
                    .iter()
                    .zip(&now)
                    .map(|(a, b)| (b.0 - a.0, b.1 - a.1))
                    .collect();
                println!("中心挪 ({dx:.2}, {dy:.2}) 时,各站的位移种类: {deltas:?}");
                assert_eq!(
                    deltas.len(),
                    1,
                    "挪不到一格时整幅图该一起走整数格,实际出现了 {} 种位移 {deltas:?}",
                    deltas.len()
                );
            }
        }
    }

    /// 相机停在同一格内,不管它的小数位置是多少,画出来必须一模一样:
    /// 画面只在跨过整格时整体挪一格(和鼠标拖动是一回事),不会随着小数位置一点点变形.
    /// 跟随光标移动时看着稳不稳,症结就在这里
    #[test]
    fn 相机在同一格内的小数位置画出来一样() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = Rect::new(0, 0, 150, 42);
        let mut view = View::default();
        view.zoom = 4.0; // 中等大小,跟随光标时差不多就是这个量级
        let step = layout(&m, &sc, area, &view).proj.scale;
        let row = step * sc.aspect;
        let (cx0, cy0) = sc.center();
        let snap = |v: &mut View| {
            let mut b = Buffer::empty(area);
            render(&mut b, area, &m, &sc, v);
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| b[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        // 格点(和 layout 里那套对齐一致)
        let (cx, cy) = ((cx0 / step).round() * step, (cy0 / row).round() * row);
        view.center = Some((cx, cy));
        let base = snap(&mut view);
        for (fx, fy) in [(-0.45, 0.0), (-0.2, 0.31), (0.0, -0.49), (0.3, 0.2), (0.45, -0.45)] {
            view.center = Some((cx + step * fx, cy + row * fy));
            let got = snap(&mut view);
            assert_eq!(
                got, base,
                "中心小数挪 ({fx}, {fy}) 格时画面变了,说明线在跟着小数位置变形"
            );
        }
    }

    /// 挪光标只该换颜色,不该换形状:把光标从一条线的站换到另一条线的站上,
    /// 整幅图的字形必须一模一样(只有光标那一格的站点字形变).
    /// 这条盯的就是"线跟着光标变形"—— 交叉格谁断一格曾经是跟着光标走的
    #[test]
    fn 挪光标不该改字形() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::real(&m);
        let area = Rect::new(0, 0, 120, 40);
        let at = |n: &str| m.names.iter().position(|x| x == n).unwrap();
        let snap = |cursor: usize| {
            let mut view = View::default();
            view.cursor = cursor;
            let mut buf = Buffer::empty(area);
            let lay = render(&mut buf, area, &m, &sc, &view);
            let grid: Vec<Vec<String>> = (0..area.height)
                .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            let (cx, cy) = lay.proj.cell(sc.pos[cursor]);
            (grid, (cx as u16, cy as u16))
        };
        let (a, ca) = snap(at("新港"));
        let (b, cb) = snap(at("杭州大会展中心"));
        // 最后一行是状态栏(站名、线路、相邻站),本来就该跟着光标变,不参与比较
        let diffs: Vec<(u16, u16, String, String)> = (0..area.height - 1)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| a[y as usize][x as usize] != b[y as usize][x as usize])
            .map(|(x, y)| {
                (
                    x,
                    y,
                    a[y as usize][x as usize].clone(),
                    b[y as usize][x as usize].clone(),
                )
            })
            .collect();
        for (x, y, old, new) in &diffs {
            println!("({x:3},{y:2}) {old:?} -> {new:?}");
        }
        assert_eq!(
            diffs.len(),
            2,
            "只有两处光标格该变,实际 {} 处: {diffs:?}",
            diffs.len()
        );
        assert!(
            diffs
                .iter()
                .all(|&(x, y, _, _)| (x, y) == ca || (x, y) == cb),
            "字形变了的地方不只是光标那两格: {diffs:?}"
        );
    }

    /// 聚焦一条线:这条线的点与笔画还是彩色的,别的全压暗
    #[test]
    fn 聚焦一条线时别的都压暗() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let hits: Vec<usize> = (0..m.names.len())
            .filter(|i| m.station_lines[*i].contains(&l1))
            .collect();
        // 光标放在 1 号线的站上(它自己另有一套光标配色,别拿来当对照)
        let (m, sc, lay, buf) = render_focus("湘湖", &hits, vec![l1]);
        let (r, g, b) = m.lines[l1].color;
        let fg = |name: &str| {
            let i = m.names.iter().position(|n| n == name).unwrap();
            let (x, y) = lay.proj.cell(sc.pos[i]);
            buf[(x as u16, y as u16)].fg
        };
        assert_eq!(fg("定安路"), Color::Rgb(r, g, b), "1 号线的站该留着自己的颜色");
        assert_eq!(fg("古荡"), DIM_COLOR, "别的线的站该压暗");
    }

    /// 两条线挤在同一格(非站点)时,颜色同样跟着光标那条线走
    #[test]
    fn 碰撞格的颜色跟着光标那条线() {
        let a = render_at("城站"); // 1 号线 / 5 号线
        let b = render_at("古荡"); // 3 号线 / 10 号线
        let diff = a
            .3
            .content()
            .iter()
            .zip(b.3.content().iter())
            .filter(|(x, y)| x.symbol() == y.symbol() && x.fg != y.fg)
            .count();
        println!("换一个站当光标,颜色不同的格子数: {diff}");
        assert!(diff > 10, "换光标应该让碰撞处换色,现在只变了 {diff} 格");
    }
}




#[cfg(test)]
mod near {
    use super::*;
    #[test]
    fn 看看这一片的位置() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let lay = layout(&m, &sc, Rect::new(0, 0, 200, 50), &View::default());
        for n in ["和睦", "北大桥", "翠柏路", "学院路", "文三路", "花园岗", "渡驾桥", "大运河"] {
            let i = m.names.iter().position(|x| x == n).unwrap();
            let c = lay.proj.cell(sc.pos[i]);
            println!("{n:6} 布局 {:?} -> 格子 ({}, {})", sc.pos[i], c.0, c.1);
        }
    }
}



#[cfg(test)]
mod window {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// 数一数某块区域里有多少格是 3 号线的颜色
    fn yellow_near(cursor: &str) -> usize {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let mut view = View::default();
        view.cursor = m.names.iter().position(|n| n == cursor).unwrap();
        let mut term = Terminal::new(TestBackend::new(200, 50)).unwrap();
        let mut lay = layout(&m, &sc, Rect::new(0, 0, 200, 50), &view);
        term.draw(|f| {
            let a = f.area();
            lay = render(f.buffer_mut(), a, &m, &sc, &view);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let i = m.names.iter().position(|n| n == "黄龙洞").unwrap();
        let (cx, cy) = lay.proj.cell(sc.pos[i]);
        let l3 = m.lines.iter().position(|l| l.label == "3号线").unwrap();
        let (r, g, b) = m.lines[l3].color;
        let mut n = 0;
        for y in cy - 5..=cy + 5 {
            for x in cx - 10..=cx + 10 {
                if x < 0 || y < 0 || x >= 200 || y >= 50 {
                    continue;
                }
                if buf[(x as u16, y as u16)].fg == Color::Rgb(r, g, b) {
                    n += 1;
                }
            }
        }
        n
    }

    /// 光标那条线的颜色会盖过别人:停在 3 号线上,这一片就比停在 2 号线上黄
    #[test]
    fn 那块黄格子跟光标有关() {
        let a = yellow_near("黄龙洞");
        let b = yellow_near("沈塘桥");
        println!("光标在 黄龙洞(3 号线)时黄格 {a} 个;光标在 沈塘桥(2 号线)时 {b} 个");
        assert!(a > b, "光标那条线的颜色应该会让这一片更黄");
    }
}

#[cfg(test)]
mod spur {
    use super::*;

    /// "下去再上来"的两条边必须共用同一列:
    /// 黄龙体育中心(a) - 黄龙洞(b) - 武林门(c),a 与 c 都在 b 的北边,
    /// 于是进 b 与出 b 都走竖的,两条边落在同一列,不会围出一个方框
    #[test]
    fn 下去又上来要走同一列() {
        let mut cv = Canvas::new(Rect::new(0, 0, 60, 30), vec![false; 2], 0);
        let (a, b, c) = ((10, 5), (13, 9), (12, 5));
        let arrive = arrive_hor(a, b, c);
        assert_eq!(arrive, Some(false), "两站都在北边,该竖着进站");
        cv.edge(a, b, 0, (255, 0, 0), arrive);
        cv.edge(b, c, 0, (255, 0, 0), None);
        // b 所在那一列:从 a 的行到 b 的行,每一格都该有笔画
        for y in a.1..=b.1 {
            let i = cv.idx(b.0, y).expect("在画布内");
            assert_ne!(cv.mask[i], 0, "({}, {y}) 该有笔画", b.0);
        }
        // 同一个纵向区间里,别的列不该再出现笔画(那就是绕出来的方框)
        for x in a.0..b.0 {
            for y in a.1 + 1..b.1 {
                let i = cv.idx(x, y).expect("在画布内");
                assert_eq!(cv.mask[i], 0, "({x}, {y}) 不该有笔画:线绕出去了");
            }
        }
    }
}






#[cfg(test)]
mod drawn_cycles {
    use super::*;

    /// 一条线不该用自己的笔画围出空格(视觉上就是一个"环"/方框):
    /// 按格子算圈数 = 边 - 点 + 连通块,大于 0 就说明围住了至少一格
    #[test]
    fn 每条线自己画出来不该围成环() {
        for (name, dot) in [
            ("杭州", include_str!("../docs/hangzhou-subway.dot")),
            ("上海", include_str!("../docs/shanghai-subway.dot")),
            ("成都", include_str!("../docs/chengdu-subway.dot")),
            ("深圳", include_str!("../docs/shenzhen-subway.dot")),
        ] {
            let m = Map::from_dot(dot).unwrap();
            let sc = Scene::schematic(&m);
            let area = Rect::new(0, 0, 200, 50);
            let view = View::default();
            let mut buf = Buffer::empty(area);
            let lay = render(&mut buf, area, &m, &sc, &view);
            let mut cv = Canvas::new(lay.area, vec![false; m.lines.len()], 0);
            for p in sc.pos.iter() {
                let c = lay.proj.cell(*p);
                if cv.idx(c.0, c.1).is_some() {
                    cv.station_cells.insert(c);
                }
            }
            for (line, path) in m.line_paths().iter() {
                let cells: Vec<(i32, i32)> =
                    path.iter().map(|&s| lay.proj.cell(sc.pos[s])).collect();
                for k in 0..cells.len() - 1 {
                    let arrive = if k + 2 < cells.len() {
                        arrive_hor(cells[k], cells[k + 1], cells[k + 2])
                    } else {
                        None
                    };
                    cv.edge(cells[k], cells[k + 1], *line, m.line_color(*line), arrive);
                }
            }
            let mut worst = Vec::new();
            for l in 0..m.lines.len() {
                let bit = 1u64 << l;
                let cells: Vec<(i32, i32)> = (lay.area.y as i32
                    ..(lay.area.y + lay.area.height) as i32)
                    .flat_map(|y| {
                        (lay.area.x as i32..(lay.area.x + lay.area.width) as i32)
                            .map(move |x| (x, y))
                    })
                    .filter(|&(x, y)| {
                        cv.idx(x, y)
                            .map(|i| cv.lines_here[i] & bit != 0)
                            .unwrap_or(false)
                    })
                    .collect();
                let set: std::collections::HashSet<(i32, i32)> = cells.iter().copied().collect();
                let mut e = 0;
                for &(x, y) in &cells {
                    for (dx, dy) in [(1, 0), (0, 1)] {
                        if set.contains(&(x + dx, y + dy)) {
                            e += 1;
                        }
                    }
                }
                let mut comp = 0;
                let mut seen = std::collections::HashSet::new();
                for &c in &cells {
                    if !seen.insert(c) {
                        continue;
                    }
                    comp += 1;
                    let mut stack = vec![c];
                    while let Some(p) = stack.pop() {
                        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let q = (p.0 + dx, p.1 + dy);
                            if set.contains(&q) && seen.insert(q) {
                                stack.push(q);
                            }
                        }
                    }
                }
                let ring = e as i32 - cells.len() as i32 + comp as i32;
                if ring > 0 {
                    worst.push(format!("{} {} 围出 {ring} 格", m.lines[l].label, cells.len()));
                }
            }
            println!(
                "{name}: 围成环的线条 {}",
                if worst.is_empty() { "无".into() } else { worst.join(",") }
            );
            // 杭州 1 号线这一处是报过的 bug(龙翔桥/定安路 那一圈),它必须为 0;
            // 剩下的 2 号线 1 处、9 号线 2 处是被"直线段"封口的,单段路由躲不掉(见 README 已知取舍),
            // 所以这里守住"总数不再变多"
            if name == "杭州" {
                assert!(
                    worst.iter().all(|w| !w.starts_with("1号线 ")),
                    "1 号线不该围成环: {worst:?}"
                );
                assert!(worst.len() <= 2, "围成环的线条变多了: {worst:?}");
            }
        }
    }
}
