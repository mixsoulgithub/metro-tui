// metro:终端里的杭州地铁线路图.
// 本文件只负责命令行、终端进出与按键;图数据在 docs/hangzhou-subway.dot(可 --file 换),
// 解析在 dot.rs,模型在 map.rs,绘制在 ui.rs.
mod city;
mod dot;
mod layout;
mod map;
mod search;
mod ui;

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, TerminalOptions, Viewport};

use map::{Map, Scene};
use search::Found;
use ui::{Mode, View};

/// 搜索态下重新算一遍命中:数字=高亮整条线,文字=站名/拼音
fn refresh(map: &Map, view: &mut View) {
    let text = view.query.clone().unwrap_or_default();
    let found = search::parse(map, &text);
    view.hit_at = 0;
    view.hits = match found {
        // 数字:聚焦这几条线(它们的所有站都算命中,光标不动)
        Found::Lines(lines) => {
            view.focus = lines.clone();
            (0..map.names.len())
                .filter(|i| map.station_lines[*i].iter().any(|l| lines.contains(l)))
                .collect()
        }
        // 文字:聚焦命中那些站涉及的线,光标跳到第一个
        Found::Stations(hits) => {
            view.focus = hits
                .iter()
                .flat_map(|i| map.station_lines[*i].iter().copied())
                .fold(Vec::new(), |mut v, l| {
                    if !v.contains(&l) {
                        v.push(l);
                    }
                    v
                });
            if let Some(&first) = hits.first() {
                view.cursor = first;
            }
            hits
        }
        Found::Empty => {
            view.focus.clear();
            Vec::new()
        }
    };
}


const USAGE: &str = "\
metro - 终端里的地铁线路图(ASCII,一条线路一种颜色)

usage: metro [options]

不带参数时先出一个选城市的界面(杭州 / 上海 / 成都 / 深圳)。

options:
  --city <slug>  直接看某个城市:hangzhou / shanghai / chengdu / shenzhen
  --file <path>  读取线路图 .dot(比 --city 优先)
  --dump         渲染一帧到 stdout(真彩 ANSI)后退出,可配合 --size
  --size <WxH>   --dump 的画布大小(默认 200x50)
  --labels       一上来就把所有站名标出来(等于按 b)
  --real         按真实经纬度画(默认是间距均匀的示意图,按 r 切换)
  --at <站名>    开局把光标停在那一站
  --find <词>    开局就搜一遍(站名/拼音/首字母,或线路号)
  -h, --help     显示帮助

keys:
  h j k l / 方向键  朝那个方向挑屏幕上最近的一站(眼睛看到哪个近就哪个,
                    换终端尺寸/切视图都可能不一样);聚焦某条线时这条线上的站优先
  H K / J L        沿线走一站:**H K 是上一站,J L 是下一站** —— 只看线路自己的站序,
                    和真实方位、屏幕方向都无关(换乘站上按 H 就是这条线的上一站)
                    站上有好几条线都能走时弹列表,数字或 j k 选、回车走、esc 取消
  + -              放大 / 缩小
  0                复位成整幅图
  空格             把光标那一站挪到画面正中
                   (聚焦某条线时,方向键优先走那条线)
  5l / 3j           数字前缀:一次性挪好几站
  Ctrl-d / Ctrl-u   半屏往下 / 往上跳一站(远距离旅行)
  Ctrl-o / Ctrl-i   回到上一个 / 下一个跳过的站(Tab 就是 Ctrl-i)
  f                 把聚焦的线路(没聚焦就是当前站那条)整条铺满画面
  z                 和空格一样,把当前站挪到画面正中
  ?                 键位速查(任意键关掉)
  b                开关站名标注
  /                搜站名或拼音(首字母也行);输入纯数字则聚焦那条线
  n / N            下一个 / 上一个命中的站
  V                聚焦当前站所在的线路(换乘站会一次聚焦几条),再按一次取消;
                   效果和搜这条线的线路号一样,之后按 n 能一站站看过去
  r                在示意图与真实地理之间切换
  鼠标左键          单击即可:地图区里点哪儿就跳到离点击点最近的那一站
                    (tmux 里要 set -g mouse on 才能收到鼠标事件)
  q / esc          退出
";

struct Args {
    file: Option<String>,
    /// 直接指定城市(不弹选城市界面)
    city: Option<String>,
    dump: bool,
    size: Option<(u16, u16)>,
    labels: bool,
    /// 用真实经纬度画(默认是间距均匀的示意图)
    real: bool,
    /// 开局把光标停在哪一站
    at: Option<String>,
    /// 开局就搜一遍(等于按 / 输入再回车)
    find: Option<String>,
    help: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args {
        file: None,
        city: None,
        dump: false,
        size: None,
        labels: false,
        real: false,
        at: None,
        find: None,
        help: false,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--file" => {
                i += 1;
                args.file = Some(argv.get(i).ok_or("--file 缺少路径")?.clone());
            }
            "--city" => {
                i += 1;
                args.city = Some(argv.get(i).ok_or("--city 缺少城市名")?.clone());
            }
            "--dump" => args.dump = true,
            "--labels" => args.labels = true,
            "--real" => args.real = true,
            "--at" => {
                i += 1;
                args.at = Some(argv.get(i).ok_or("--at 缺少站名")?.clone());
            }
            "--find" => {
                i += 1;
                args.find = Some(argv.get(i).ok_or("--find 缺少内容")?.clone());
            }
            "--size" => {
                i += 1;
                let raw = argv.get(i).ok_or("--size 缺少尺寸")?;
                let (w, h) = raw.split_once(['x', 'X']).ok_or("--size 形如 200x50")?;
                args.size = Some((
                    w.parse().map_err(|_| "宽度不是数字")?,
                    h.parse().map_err(|_| "高度不是数字")?,
                ));
            }
            "-h" | "--help" => args.help = true,
            other => return Err(format!("未知参数 {other}")),
        }
        i += 1;
    }
    Ok(args)
}

/// vim 方向键:返回 (列方向, 行方向),h 往西、l 往东、k 往北、j 往南
/// 小写/方向键:朝那个方向在屏幕上挑最近的一站
fn dir_of(code: KeyCode) -> Option<(i32, i32)> {
    match code {
        KeyCode::Char('h') | KeyCode::Left => Some((-1, 0)),
        KeyCode::Char('l') | KeyCode::Right => Some((1, 0)),
        KeyCode::Char('k') | KeyCode::Up => Some((0, -1)),
        KeyCode::Char('j') | KeyCode::Down => Some((0, 1)),
        _ => None,
    }
}

/// 大写:沿线走一站,只认线路自己的站序(和真实方位、屏幕方向都无关).
/// H K 是上一站,J L 是下一站;上面那条线有好几条就弹列表让用户挑
fn topo_of(code: KeyCode) -> Option<bool> {
    match code {
        KeyCode::Char('H') | KeyCode::Char('K') => Some(false),
        KeyCode::Char('J') | KeyCode::Char('L') => Some(true),
        _ => None,
    }
}

fn dump(buf: &ratatui::buffer::Buffer) {
    let mut out = String::new();
    for y in 0..buf.area.height {
        // 全角字的第二格不用再输出(宽字本身已经占了两列)
        let mut skip_cells = 0usize;
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if skip_cells > 0 {
                skip_cells -= 1;
                continue;
            }
            skip_cells = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
            let (r, g, b) = match cell.fg {
                Color::Rgb(r, g, b) => (r, g, b),
                Color::Reset => (212, 218, 232),
                _ => (212, 218, 232),
            };
            let (br, bg_, bb) = match cell.bg {
                Color::Rgb(r, g, b) => (r, g, b),
                _ => (9, 10, 17),
            };
            let mut sgr = String::from("\x1b[");
            sgr.push_str(&format!("38;2;{r};{g};{b};48;2;{br};{bg_};{bb}"));
            if cell.modifier.contains(Modifier::BOLD) {
                sgr.push_str(";1");
            }
            if cell.modifier.contains(Modifier::DIM) {
                sgr.push_str(";2");
            }
            sgr.push_str("m");
            out.push_str(&sgr);
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    print!("{out}");
    let _ = io::stdout().flush();
}

/// 光标跑出视野就把视图平滑挪过去
fn follow_cursor(view: &mut View, map: &Map, sc: &Scene) {
    let Ok((w, h)) = crossterm::terminal::size() else {
        return;
    };
    let lay = ui::layout(map, sc, ratatui::layout::Rect::new(0, 0, w, h), view);
    let c = lay.proj.cell(sc.pos[view.cursor]);
    let inside = c.0 >= lay.area.x as i32
        && c.0 < (lay.area.x + lay.area.width) as i32
        && c.1 >= lay.area.y as i32
        && c.1 < (lay.area.y + lay.area.height) as i32;
    if !inside {
        let old = view.center.unwrap_or_else(|| sc.center());
        let p = sc.pos[view.cursor];
        let want = (old.0 + (p.0 - old.0) * 0.85, old.1 + (p.1 - old.1) * 0.85);
        pan_to(view, sc, want);
    }
}

/// 数字前缀:5l = 连挪 5 站.返回 true 表示这个键被当成计数吃了
fn count_digit(pending: &mut Option<u32>, c: char) -> bool {
    let Some(d) = c.to_digit(10) else { return false };
    if d == 0 && pending.is_none() {
        return false; // 没前缀时 0 还是"复位"
    }
    *pending = Some((pending.unwrap_or(0) * 10 + d).min(999));
    true
}

/// 把"现在这里"记进跳转历史(Ctrl-o / Ctrl-i 前后翻)
fn note_here(view: &mut View) {
    if view.history.last() == Some(&view.cursor) {
        view.hist_at = view.history.len() - 1;
        return;
    }
    view.history.truncate(view.hist_at + 1); // 从历史中间跳走,后面那段丢掉
    view.history.push(view.cursor);
    if view.history.len() > 64 {
        view.history.remove(0);
    }
    view.hist_at = view.history.len() - 1;
}

/// 回到历史里的上一个 / 下一个跳过的站.返回 false 表示到头了
fn jump_history(view: &mut View, back: bool) -> bool {
    if view.history.is_empty() {
        return false;
    }
    if back {
        if view.hist_at == 0 {
            return false;
        }
        view.hist_at -= 1;
    } else {
        if view.hist_at + 1 >= view.history.len() {
            return false;
        }
        view.hist_at += 1;
    }
    view.cursor = view.history[view.hist_at];
    true
}

/// 半屏跳:从当前站往 dir 方向挪半屏高,落到离那个点最近的一站.
/// 落点会夹在地图区里,再远也不会挑到图例或状态行上
fn leap(
    scene: &Scene,
    lay: &ui::Layout,
    cursor: usize,
    dir: (i32, i32),
    rows: i32,
) -> usize {
    let c = lay.proj.cell(scene.pos[cursor]);
    let dx = dir.0 * (lay.area.width as i32 / 2).max(1);
    let dy = dir.1 * rows.max(1);
    let (ax, ay) = (lay.area.x as i32, lay.area.y as i32);
    let x_max = ax + lay.area.width as i32 - 1;
    let y_max = ay + lay.area.height as i32 - 1;
    let x = (c.0 + dx).clamp(ax, x_max.max(ax));
    let y = (c.1 + dy).clamp(ay, y_max.max(ay));
    ui::station_at(scene, lay, lay.area, x as u16, y as u16).unwrap_or(cursor)
}

/// 把当前站所在的线路聚焦起来(换乘站会有好几条),再按一次取消.
/// 效果和"搜这几条线的线路号"一样:其余线全部压暗,而且 n 能在这几条线的站之间跳
fn focus_lines_of(map: &Map, view: &mut View) {
    let lines = map.station_lines[view.cursor].clone();
    // 这一站的线已经在聚焦里了(比如刚才就是按 V 聚焦的),那这一下就是取消
    if !lines.is_empty() && lines.iter().all(|l| view.focus.contains(l)) {
        view.focus.clear();
        view.hits.clear();
        view.hit_at = 0;
        return;
    }
    view.focus = lines;
    view.hits = (0..map.names.len())
        .filter(|&i| {
            map.station_lines[i]
                .iter()
                .any(|l| view.focus.contains(l))
        })
        .collect();
    view.hit_at = 0;
}

/// 用户数据目录:$XDG_DATA_HOME/metro(默认 ~/.local/share/metro)
fn user_data_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    Some(base.join("metro"))
}

/// 去哪儿找现成的线路图:当前目录、可执行文件旁边、用户数据目录
fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |d: std::path::PathBuf| {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    push(std::path::PathBuf::from("docs"));
    // 用户数据目录里的线路图(不在仓库里)
    if let Some(dir) = user_data_dir() {
        push(dir.join("cities"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push(dir.join("docs"));
            push(dir.join("..").join("docs"));
        }
    }
    dirs
}

/// 看哪个城市:--file > --city > 选城市界面 >(非交互时)第一个城市.
/// 返回 None 表示用户在选城市界面里退出
/// 这一轮看哪座城市:(城市名, 线路图原文).城市名只用来在选城市界面里定位光标光标
fn source_text(args: &Args) -> Result<Option<(String, String)>, String> {
    if let Some(path) = &args.file {
        return std::fs::read_to_string(path)
            .map(|t| Some((String::from("--file"), t)))
            .map_err(|e| format!("读不到 {path}: {e}"));
    }
    if let Some(slug) = &args.city {
        // 内嵌的没有就去 docs 里找(工具刚建出来的城市不用重新编译)
        return city::resolve(slug, &data_dirs()).map(|t| Some((slug.clone(), t)));
    }
    let interactive = !args.dump && io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive {
        return Ok(Some((String::from("--dump"), city::CITIES[0].dot.to_string())));
    }
    match choose_city(None) {
        Ok(Some((name, dot))) => Ok(Some((name, dot))),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 选城市界面里的一行:内嵌的(带原文)或 docs 里的(带路径)
struct Row {
    name: String,
    desc: String,
    dot: Option<&'static str>,
    path: Option<std::path::PathBuf>,
}

/// 选中一行:内嵌的直接用,docs 里的现场读出来
fn open_row(list: &[Row], i: usize) -> Option<(String, String)> {
    let r = list.get(i)?;
    if let Some(dot) = r.dot {
        return Some((r.name.clone(), dot.to_string()));
    }
    let path = r.path.as_ref()?;
    match std::fs::read_to_string(path) {
        Ok(t) => Some((r.name.clone(), t)),
        Err(e) => {
            eprintln!("metro: 读不到 {}: {e}", path.display());
            None
        }
    }
}

/// 启动界面:选城市(None 表示用户按 q 退出).
/// 除了内嵌的那几座,还列出 docs 目录里现成的 dot(工具刚建好的城市)
fn choose_city(preset: Option<&str>) -> io::Result<Option<(String, String)>> {
    let mut list: Vec<Row> = city::CITIES
        .iter()
        .map(|c| {
            let desc = match Map::from_dot(c.dot) {
                Ok(m) => format!("{} 条线,{} 站", m.lines.len(), m.names.len()),
                Err(e) => format!("读不了:{e}"),
            };
            Row {
                name: c.name.to_string(),
                desc,
                dot: Some(c.dot),
                path: None,
            }
        })
        .collect();
    for dir in data_dirs() {
        for (slug, name, lines, stations, path) in city::external(&dir) {
            if list.iter().any(|r| r.name == name) {
                continue;
            }
            list.push(Row {
                name: name.clone(),
                desc: format!("{lines} 条线,{stations} 站  ({slug})"),
                dot: None,
                path: Some(path),
            });
        }
    }
    let rows: Vec<(String, String)> = list
        .iter()
        .map(|r| (r.name.clone(), r.desc.clone()))
        .collect();
    let mut stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        EnableMouseCapture
    )?;
    let mut term = Terminal::with_options(
        CrosstermBackend::new(stdout),
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;
    // 从看图界面 q 回来时,光标停在刚才那座城市上
    let mut sel = preset
        .and_then(|name| list.iter().position(|r| r.name == name))
        .unwrap_or(0usize);
    let picked = loop {
        term.draw(|f| {
            let area = f.area();
            let refs: Vec<(&str, &str)> = rows.iter().map(|(n, d)| (n.as_str(), d.as_str())).collect();
            ui::render_chooser(f.buffer_mut(), area, &refs, sel);
        })?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let ev = event::read()?;
        if let Event::Mouse(me) = ev {
            // 点一行就是选它;点在已选中的那一行上就打开它
            if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                let refs: Vec<(&str, &str)> =
                    rows.iter().map(|(n, d)| (n.as_str(), d.as_str())).collect();
                if let Some(i) =
                    ui::chooser_row_at(term.size()?.into(), &refs, sel, me.column, me.row)
                {
                    if i == sel {
                        break open_row(&list, i);
                    }
                    sel = i;
                }
            }
            continue;
        }
        let Event::Key(KeyEvent { code, .. }) = ev else {
            continue;
        };
        match code {
            KeyCode::Char('q') | KeyCode::Esc => break None,
            KeyCode::Char('j') | KeyCode::Down => sel = (sel + 1).min(rows.len() - 1),
            KeyCode::Char('k') | KeyCode::Up => sel = sel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => break open_row(&list, sel),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let i = (c as u8 - b'1') as usize;
                if i < rows.len() {
                    break open_row(&list, i);
                }
            }
            _ => {}
        }
    };
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        term.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(picked)
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("metro: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    // 第一轮看哪座城市:--city/--file 直接用,否则弹选城市界面;之后都在选城市界面里挑
    // 看图界面按 q 回到选城市时,让界面停在刚才那座城市
    let mut last_name: Option<String>;
    let mut picked: Option<(String, String)> = match source_text(&args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("metro: {e}");
            return ExitCode::from(1);
        }
    };
    loop {
    // 记下这一轮的城市名,q 回去时好把光标停在它上面
    let text = match picked {
        Some((n, t)) => {
            last_name = Some(n);
            t
        }
        None => return ExitCode::SUCCESS, // 在选城市界面里按 q 退了
    };
    let map = match Map::from_dot(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("metro: 线路图有问题: {e}");
            return ExitCode::from(1);
        }
    };
    let mut view;
    let (w, h) = args
        .size
        .or_else(|| crossterm::terminal::size().ok())
        .unwrap_or((200, 50));
    let real = Scene::real(&map);
    let schematic = Scene::schematic(&map);
    // 每轮重建:换了城市,游标下标得对着新图
    view = View {
        labels: args.labels,
        mode: if args.real { Mode::Real } else { Mode::Schematic },
        ..View::default()
    };
    // 默认光标停在离画面中心最近的一站(按当前那套坐标算,别串了)
    let scene = scene_of(&view, &real, &schematic);
    let center = scene.center();
    let dist = |i: usize| {
        let p = scene.pos[i];
        (p.0 - center.0).powi(2) + (p.1 - center.1).powi(2)
    };
    view.cursor = match &args.at {
        Some(name) => match map.names.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                eprintln!("metro: 没有叫 {name} 的站");
                return ExitCode::from(1);
            }
        },
        None => (0..scene.pos.len())
            .min_by(|&a, &b| dist(a).partial_cmp(&dist(b)).unwrap())
            .unwrap_or(0),
    };
    if let Some(text) = &args.find {
        view.query = Some(text.clone());
        refresh(&map, &mut view);
        view.query = None;
    }
    if args.dump {
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("测试终端");
        term.draw(|f| {
            let area = f.area();
            ui::render(f.buffer_mut(), area, &map, scene_of(&view, &real, &schematic), &view);
        })
        .expect("绘制");
        dump(term.backend().buffer());
        return ExitCode::SUCCESS;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("metro: 需要真正的终端;只想要一帧可以用 --dump");
        return ExitCode::from(1);
    }
    match run(&mut view, &map, &real, &schematic) {
        Ok(Outcome::Quit) => return ExitCode::SUCCESS,
        Ok(Outcome::Back) => {}
        Err(e) => {
            eprintln!("metro: {e}");
            return ExitCode::from(1);
        }
    }
    // q 回选城市界面;在那里再按 q 才整个退出
    picked = match choose_city(last_name.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("metro: {e}");
            return ExitCode::from(1);
        }
    };
    }
}

use std::io::IsTerminal;

/// ":" 命令:目前只有 cycle(绕行容忍强度),调小更认"屏幕上最近",调大更认"同一条线"
fn cmd_apply(view: &mut View, line: &str) {
    let mut it = line.split_whitespace();
    let (cmd, arg) = (it.next().unwrap_or(""), it.next());
    match (cmd, arg) {
        ("cycle", Some(a)) if a == "auto" => {
            view.cycle = None;
            view.note = Some(String::from("cycle = auto(没聚焦 1.0,聚焦 2.0)"));
        }
        ("cycle", Some(a)) => match a.parse::<f64>() {
            Ok(v) => {
                view.cycle = Some(v.clamp(0.0, 50.0));
                view.note =
                    Some(format!("cycle = {:.2}(同线那一档最多允许远这么多倍)", v.clamp(0.0, 50.0)));
            }
            Err(_) => view.note = Some(format!("cycle: {a} 不是数字(或 auto)")),
        },
        ("cycle", None) => {
            view.note = Some(match view.cycle {
                Some(v) => format!("cycle = {v:.2}"),
                None => String::from("cycle = auto(没聚焦 0,聚焦 2)"),
            });
        }
        ("side", Some(a)) => match a.parse::<f64>() {
            Ok(v) => {
                view.side = v.clamp(0.0, 20.0);
                view.note = Some(format!("side = {:.2}(横向偏斜算几倍距离,调小更认远近)", view.side));
            }
            Err(_) => view.note = Some(format!("side: {a} 不是数字")),
        },
        ("side", None) => {
            view.note = Some(format!("side = {:.2}", view.side));
        }
        ("", None) => view.note = None,
        _ => view.note = Some(format!("未知命令:{line}(有 cycle <倍数> 和 side <倍数>)")),
    }
}

/// 方向键走了谁、为什么:把分数最高的几个候选打出来(调试用)
fn explain_dir(
    map: &Map,
    dir: (i32, i32),
    picked: usize,
    cands: &[(usize, f64, bool)],
    cycle: f64,
    side: f64,
) -> String {
    let key = match dir {
        (0, -1) => "k",
        (0, 1) => "j",
        (-1, 0) => "h",
        _ => "l",
    };
    let mut s = format!(
        "{key}(cycle {cycle:.2} side {side:.2}) 选中 {};",
        map.names[picked]
    );
    for (n, score, focused) in cands.iter().take(3) {
        s += &format!(
            " {} {score:.2}{}",
            map.names[*n],
            if *focused { " 同线" } else { " 异线" }
        );
    }
    s
}

/// 平滑移动:把视图中心朝目标挪一段(每帧走掉剩余距离的 22%,指数逼近),
/// 距目标不足半格就直接吸附过去.返回 true 表示还在动,主循环要接着按帧重画
fn animate(view: &mut View, scene: &Scene, cell: f64) -> bool {
    let Some(t) = view.target else {
        return false;
    };
    let cur = view.center.unwrap_or_else(|| scene.center());
    const K: f64 = 0.22;
    let next = (cur.0 + (t.0 - cur.0) * K, cur.1 + (t.1 - cur.1) * K);
    if (t.0 - next.0).abs() < cell * 0.5 && (t.1 - next.1).abs() < cell * 0.5 {
        view.center = Some(t);
        view.target = None;
        false
    } else {
        view.center = Some(next);
        true
    }
}

/// 让视图平滑地挪到某一点(画布坐标):已经在动的就从当前显示的位置接着走
fn pan_to(view: &mut View, scene: &Scene, p: (f64, f64)) {
    if view.center.is_none() {
        view.center = Some(scene.center());
    }
    view.target = Some(p);
}

/// 当前模式用的是哪套坐标
fn scene_of<'a>(view: &View, real: &'a Scene, schematic: &'a Scene) -> &'a Scene {
    match view.mode {
        Mode::Real => real,
        Mode::Schematic => schematic,
    }
}

/// 看图循环退出的两种去向:回选城市界面,还是整个退出
#[derive(PartialEq, Eq, Clone, Copy)]
enum Outcome {
    Back,
    Quit,
}

fn run(
    view: &mut View,
    map: &Map,
    real: &Scene,
    schematic: &Scene,
) -> io::Result<Outcome> {
    let mut stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        EnableMouseCapture
    )?;
    let mut term = Terminal::with_options(
        CrosstermBackend::new(stdout),
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;
    let result = event_loop(&mut term, view, map, real, schematic);
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        term.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    result
}

fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    view: &mut View,
    map: &Map,
    real: &Scene,
    schematic: &Scene,
) -> io::Result<Outcome> {
    let mut search_from = view.cursor;
    // 数字前缀(5l = 连挪 5 站)
    let mut pending: Option<u32> = None;
    // 左键拖动:起点格、按下时的视图中心、当时的投影
    let mut drag: Option<((u16, u16), (f64, f64), ui::Proj)> = None;
    // 上一站/下一站有多个候选时,挂着等用户挑:(候选, 当前选中, 是不是下一站)
    let mut choice: Option<(Vec<(usize, usize)>, usize, bool)> = None;
    note_here(view);
    // 当前站闪烁一个来回的时长:0.8 秒,差不多是心跳的频率(约 75 下/分)
    const BEAT: f64 = 0.8;
    let started = std::time::Instant::now();
    loop {
        // 按心跳的频率走一条正弦,渲染时拿这个强度在平时那个亮白和红色之间插值
        view.pulse = ui::pulse_at(started.elapsed().as_secs_f64() / BEAT);
        // 移动动画:还在动就按 ~60fps 重画,停了也要 ~30fps(闪烁要连贯)
        let moving = {
            let sc = scene_of(view, real, schematic);
            let lay = ui::layout(map, sc, term.size()?.into(), view);
            animate(view, sc, lay.proj.scale)
        };
        term.draw(|f| {
            let area = f.area();
            ui::render(f.buffer_mut(), area, map, scene_of(view, real, schematic), view);
            if let Some((cands, at, next)) = choice.as_ref() {
                ui::render_choice(f.buffer_mut(), area, map, cands, *at, *next);
            }
        })?;
        let wait = if moving {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(33)
        };
        if !event::poll(wait)? {
            continue;
        }
        let ev = event::read()?;
        if let Event::Mouse(me) = ev {
            match me.kind {
                // 滚轮:缩放;Ctrl 按住改成平移(和地图软件一致);Shift 平移左右
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let up = matches!(me.kind, MouseEventKind::ScrollUp);
                    let ctrl = me.modifiers.contains(KeyModifiers::CONTROL);
                    let shift = me.modifiers.contains(KeyModifiers::SHIFT);
                    if !ctrl && !shift {
                        let k = if up { 1.15 } else { 1.0 / 1.15 };
                        view.zoom = (view.zoom * k).clamp(0.2, 8.0);
                        view.note = Some(format!("zoom x{:.2}", view.zoom));
                    } else {
                        let sc = scene_of(view, real, schematic);
                        let lay = ui::layout(map, sc, term.size()?.into(), view);
                        let from = view.center.unwrap_or_else(|| sc.center());
                        let step = if up { -3 } else { 3 };
                        let (dc, dr) = if shift { (step, 0) } else { (0, step) };
                        view.center = Some(ui::pan_center(&lay.proj, from, dc, dr));
                        view.target = None;
                        view.note = Some(if shift {
                            format!("平移 {step} 列")
                        } else {
                            format!("平移 {step} 行")
                        });
                    }
                }
                // 左键按下:记住起点和当时的视图中心,准备拖动
                MouseEventKind::Down(MouseButton::Left) => {
                    let sc = scene_of(view, real, schematic);
                    let lay = ui::layout(map, sc, term.size()?.into(), view);
                    drag = Some((
                        (me.column, me.row),
                        view.center.unwrap_or_else(|| sc.center()),
                        lay.proj,
                    ));
                }
                // 拖动:地图跟着手走(往右拖,中心往左挪)
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(((c0, r0), center0, proj)) = drag.as_ref() {
                        let dc = me.column as i32 - *c0 as i32;
                        let dr = me.row as i32 - *r0 as i32;
                        if dc != 0 || dr != 0 {
                            view.center = Some(ui::pan_center(proj, *center0, -dc, -dr));
                            view.target = None;
                        }
                    }
                }
                // 松开:几乎没动才算"点了一下",那就跳站;拖过就不算点击
                MouseEventKind::Up(MouseButton::Left) => {
                    let moved = drag
                        .as_ref()
                        .map(|((c0, r0), _, _)| {
                            (me.column as i32 - *c0 as i32).abs()
                                + (me.row as i32 - *r0 as i32).abs()
                                > 1
                        })
                        .unwrap_or(false);
                    drag = None;
                    if !moved {
                        let sc = scene_of(view, real, schematic);
                        let lay = ui::layout(map, sc, term.size()?.into(), view);
                        if let Some(i) = ui::station_at(sc, &lay, lay.area, me.column, me.row) {
                            if i != view.cursor {
                                note_here(view);
                                view.cursor = i;
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = ev
        else {
            continue;
        };
        if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
            return Ok(Outcome::Quit);
        }
        // 速查浮层:任意键关掉
        if view.help {
            view.help = false;
            continue;
        }
        // 选上一站/下一站:数字直选、j k 挪、回车走、esc 取消
        if let Some((cands, at, next)) = choice.clone() {
            let mut go = None;
            match code {
                KeyCode::Esc | KeyCode::Char('q') => choice = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    choice = Some((cands.clone(), (at + 1) % cands.len(), next))
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    choice = Some((cands.clone(), (at + cands.len() - 1) % cands.len(), next))
                }
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => go = Some(at),
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    let i = (c as u8 - b'1') as usize;
                    if i < cands.len() {
                        go = Some(i);
                    }
                }
                _ => {}
            }
            if let Some(i) = go {
                note_here(view);
                view.cursor = cands[i].0;
                choice = None;
                let sc = scene_of(view, real, schematic);
                follow_cursor(view, map, sc);
            }
            continue;
        }
        let _ = &search_from;
        // ":" 命令行:所有按键都归输入行
        if view.cmd.is_some() {
            match code {
                KeyCode::Esc => view.cmd = None,
                KeyCode::Enter => {
                    let line = view.cmd.take().unwrap_or_default();
                    cmd_apply(view, &line);
                }
                KeyCode::Backspace => {
                    if let Some(b) = view.cmd.as_mut() {
                        b.pop();
                    }
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(b) = view.cmd.as_mut() {
                        b.push(c);
                    }
                }
                _ => {}
            }
            continue;
        }
        // 搜索态:所有按键都归输入行
        if view.query.is_some() {
            match code {
                KeyCode::Esc => {
                    view.query = None;
                    view.hits.clear();
                    view.focus.clear();
                    view.cursor = search_from;
                }
                KeyCode::Enter => {
                    view.query = None; // 命中留着,可以按 n 一个个跳
                }
                KeyCode::Backspace => {
                    if let Some(q) = view.query.as_mut() {
                        q.pop();
                    }
                    refresh(map, view);
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(q) = view.query.as_mut() {
                        q.push(c);
                    }
                    refresh(map, view);
                }
                _ => {}
            }
            continue;
        }
        let mut moved = None;
        let mut topo = None;
        let mut steps: u32 = 1;
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(Outcome::Back),
            // ?:键位速查;z:和空格一样把当前站居中
            KeyCode::Char('?') => view.help = true,
            KeyCode::Char('z') => {
                let sc = scene_of(view, real, schematic);
                let p = sc.pos[view.cursor.min(sc.pos.len() - 1)];
                pan_to(view, sc, p);
            }
            // f:把聚焦的线路(没聚焦就是当前站那条)整条铺满画面
            KeyCode::Char('f') => {
                let sc = scene_of(view, real, schematic);
                let area: ratatui::layout::Rect = term.size()?.into();
                let lay = ui::layout(map, sc, area, view);
                let lines = if view.focus.is_empty() {
                    map.station_lines[view.cursor].clone()
                } else {
                    view.focus.clone()
                };
                let pts: Vec<(f64, f64)> = (0..map.names.len())
                    .filter(|&i| map.station_lines[i].iter().any(|l| lines.contains(l)))
                    .map(|i| sc.pos[i])
                    .collect();
                if let Some((z, c)) = ui::fit_to(sc, area, lay.area, pts.into_iter()) {
                    view.zoom = z;
                    view.center = Some(c);
                    view.target = None;
                }
            }
            // 半屏跳:远距离旅行用
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                let sc = scene_of(view, real, schematic);
                let lay = ui::layout(map, sc, term.size()?.into(), view);
                let rows = lay.area.height as i32 / 2;
                let next = leap(sc, &lay, view.cursor, (0, 1), rows);
                if next != view.cursor {
                    note_here(view);
                    view.cursor = next;
                }
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                let sc = scene_of(view, real, schematic);
                let lay = ui::layout(map, sc, term.size()?.into(), view);
                let rows = lay.area.height as i32 / 2;
                let next = leap(sc, &lay, view.cursor, (0, -1), rows);
                if next != view.cursor {
                    note_here(view);
                    view.cursor = next;
                }
            }
            // 跳转历史:Tab 就是 Ctrl-i
            KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
                jump_history(view, true);
            }
            KeyCode::Tab | KeyCode::Char('i') if modifiers.contains(KeyModifiers::CONTROL) => {
                jump_history(view, false);
            }
            _ if dir_of(code).is_some() => {
                moved = dir_of(code);
                steps = pending.take().unwrap_or(1);
            }
            _ if topo_of(code).is_some() => {
                topo = topo_of(code);
            }
            KeyCode::Char('+') | KeyCode::Char('=') => view.zoom = (view.zoom * 1.4).min(64.0),
            KeyCode::Char('-') | KeyCode::Char('_') => view.zoom = (view.zoom / 1.4).max(1.0),
            // 空格:把光标那一站挪到画面正中(看局部的时候好使)
            KeyCode::Char(' ') => {
                let sc = scene_of(view, real, schematic);
                let p = sc.pos[view.cursor.min(sc.pos.len() - 1)];
                pan_to(view, sc, p);
            }
            KeyCode::Char('0') => {
                view.zoom = 1.0;
                view.center = None;
                view.target = None;
            }
            KeyCode::Char('b') => view.labels = !view.labels,
            // V:聚焦当前站所在的线路(换乘站是好几条),再按一次取消
            KeyCode::Char('V') => focus_lines_of(map, view),
            KeyCode::Char('/') => {
                search_from = view.cursor;
                view.query = Some(String::new());
                view.hits.clear();
                view.focus.clear();
            }
            // n:跳到下一个命中的站(搜完一串站就靠它挨个看)
            KeyCode::Char('n') if !view.hits.is_empty() => {
                note_here(view);
                view.hit_at = (view.hit_at + 1) % view.hits.len();
                view.cursor = view.hits[view.hit_at];
            }
            KeyCode::Char('N') if !view.hits.is_empty() => {
                note_here(view);
                view.hit_at = (view.hit_at + view.hits.len() - 1) % view.hits.len();
                view.cursor = view.hits[view.hit_at];
            }
            KeyCode::Char('r') => {
                view.mode = match view.mode {
                    Mode::Real => Mode::Schematic,
                    Mode::Schematic => Mode::Real,
                };
                view.zoom = 1.0;
                view.center = None;
                view.target = None;
            }
            // ":" 开命令行(调试/调参用,见 cmd_apply)
            KeyCode::Char(':') => {
                view.cmd = Some(String::new());
                view.note = None;
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                // 数字当计数前缀(5l);不是数字就把计数清掉
                if !count_digit(&mut pending, c) {
                    pending = None;
                }
            }
            _ => pending = None,
        }
        if let Some(dir) = moved {
            let area = term.size()?;
            let sc = scene_of(view, real, schematic);
            let lay = ui::layout(map, sc, area.into(), view);
            let mut why = String::new();
            for _ in 0..steps {
                let cyc = view.cycle.unwrap_or(if view.focus.is_empty() { 0.0 } else { 2.0 });
                let (next, cands) =
                    ui::step_cfg(map, sc, &lay, view.cursor, dir, &view.focus, cyc, view.side);
                if next == view.cursor {
                    break;
                }
                if why.is_empty() {
                    let cyc = view.cycle.unwrap_or(if view.focus.is_empty() { 0.0 } else { 2.0 });
                    why = explain_dir(map, dir, next, &cands, cyc, view.side);
                }
                view.cursor = next;
            }
            if !why.is_empty() {
                view.note = Some(why);
            }
            follow_cursor(view, map, sc);
        }
        if let Some(next) = topo {
            let cands = ui::line_step(map, view.cursor, next, &view.focus);
            match cands.len() {
                0 => {}
                1 => {
                    note_here(view);
                    view.cursor = cands[0].0;
                    view.note = Some(format!(
                        "{} 沿线{}:{}",
                        if next { "J/L" } else { "H/K" },
                        if next { "下一站" } else { "上一站" },
                        map.names[view.cursor]
                    ));
                    let sc = scene_of(view, real, schematic);
                    follow_cursor(view, map, sc);
                }
                // 好几条线都能走:弹个列表让用户挑
                _ => choice = Some((cands, 0, next)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数字前缀:连着按就是多位数,0 单独按还是复位
    #[test]
    fn 数字前缀() {
        let mut p: Option<u32> = None;
        assert!(count_digit(&mut p, '5'));
        assert_eq!(p, Some(5));
        assert!(count_digit(&mut p, '0'));
        assert_eq!(p, Some(50), "10l 里的 0 该接着前面");
        assert!(count_digit(&mut p, '0'));
        assert_eq!(p, Some(500), "还能继续接");
        let mut q: Option<u32> = None;
        assert!(!count_digit(&mut q, '0'), "没前缀时 0 不算计数(是复位)");
        assert_eq!(q, None);
        assert!(!count_digit(&mut q, 'x'), "不是数字就不算");
    }

    /// 跳转历史:Ctrl-o 回上一个跳过的站,Ctrl-i 再回去
    #[test]
    fn 跳转历史() {
        let mut view = View::default();
        view.cursor = 10;
        note_here(&mut view);
        assert_eq!(view.history, vec![10]);
        view.cursor = 20;
        note_here(&mut view);
        view.cursor = 30;
        note_here(&mut view);
        assert!(jump_history(&mut view, true));
        assert_eq!(view.cursor, 20);
        assert!(jump_history(&mut view, true));
        assert_eq!(view.cursor, 10);
        assert!(!jump_history(&mut view, true), "到头了就不动");
        assert!(jump_history(&mut view, false));
        assert_eq!(view.cursor, 20);
        // 从中间跳走:后面的历史丢掉
        view.cursor = 99;
        note_here(&mut view);
        assert_eq!(view.history, vec![10, 20, 99]);
    }

    /// 半屏跳:往下跳一站,该落在光标下方约半屏的地方,而且不会跳出地图区
    #[test]
    fn 半屏跳() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = ratatui::layout::Rect::new(0, 0, 200, 50);
        let lay = ui::layout(&m, &sc, area, &View::default());
        let cur = m.names.iter().position(|n| n == "武林广场").unwrap();
        let here = lay.proj.cell(sc.pos[cur]);
        let down = leap(&sc, &lay, cur, (0, 1), lay.area.height as i32 / 2);
        let dc = lay.proj.cell(sc.pos[down]);
        println!("半屏往下:{} -> {}  Δy={}", m.names[cur], m.names[down], dc.1 - here.1);
        assert_ne!(down, cur);
        assert!(dc.1 > here.1, "该往下走");
        assert!(dc.1 - here.1 >= lay.area.height as i32 / 4, "该跳得比一站远");
        assert!(dc.1 < lay.area.y as i32 + lay.area.height as i32, "别跳出地图区");
        let up = leap(&sc, &lay, cur, (0, -1), lay.area.height as i32 / 2);
        assert!(lay.proj.cell(sc.pos[up]).1 < here.1, "往上跳该在光标上方");
    }

    /// f:把一条线铺满画面 —— 这条线的站全部落在视图里
    #[test]
    fn 把线路铺满画面() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let area = ratatui::layout::Rect::new(0, 0, 200, 50);
        let mut view = View::default();
        let l1 = m.lines.iter().position(|l| l.label == "1号线").unwrap();
        let lay0 = ui::layout(&m, &sc, area, &view);
        let pts: Vec<(f64, f64)> = (0..m.names.len())
            .filter(|&i| m.station_lines[i].contains(&l1))
            .map(|i| sc.pos[i])
            .collect();
        let (z, c) = ui::fit_to(&sc, area, lay0.area, pts.clone().into_iter()).unwrap();
        view.zoom = z;
        view.center = Some(c);
        let lay = ui::layout(&m, &sc, area, &view);
        let mut inside = 0;
        for p in &pts {
            let (x, y) = lay.proj.cell(*p);
            if x >= lay.area.x as i32
                && x < (lay.area.x + lay.area.width) as i32
                && y >= lay.area.y as i32
                && y < (lay.area.y + lay.area.height) as i32
            {
                inside += 1;
            }
        }
        println!("1 号线 {} 站,铺满后落在画面里的 {}", pts.len(), inside);
        assert_eq!(inside, pts.len(), "整条线都该在画面里");
        assert!(z > 1.0, "该比整幅图更放大");
    }

    /// 平滑移动:一帧挪一段(指数逼近),不是一步到位;方向不变,最后吸附到目标
    #[test]
    fn 平滑移动是逐步逼近的() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let sc = Scene::schematic(&m);
        let mut view = View::default();
        let goal = (1000.0, 500.0);
        pan_to(&mut view, &sc, goal);
        assert!(view.center.is_some(), "开始动之前得先有起点");
        let mut dists = Vec::new();
        let mut frames = 0;
        while animate(&mut view, &sc, 1.0) {
            let cur = view.center.unwrap();
            dists.push((goal.0 - cur.0).hypot(goal.1 - cur.1));
            frames += 1;
            assert!(frames < 200, "动画没个头");
        }
        println!("动了 {frames} 帧,距离序列前几项 {:?}", &dists[..5.min(dists.len())]);
        assert!(frames > 5, "该是好幾帧的动画,不是一步到位(实际 {frames} 帧)");
        assert!(dists.windows(2).all(|w| w[1] < w[0]), "离目标该越来越近");
        assert_eq!(view.center, Some(goal), "最后要吸附到目标");
        assert!(view.target.is_none(), "到位后目标要清掉");
        // 停在原地之后 animate 该说"不动了"
        assert!(!animate(&mut view, &sc, 1.0));
    }

    /// 内嵌的每个城市都要能解析、能布局:数据坏了这里先红
    #[test]
    fn 内嵌城市都能解析() {
        let mut seen = Vec::new();
        for c in city::CITIES {
            let m = Map::from_dot(c.dot).unwrap_or_else(|e| panic!("{}: {e}", c.name));
            let stat = (m.lines.len(), m.names.len());
            println!("{}: {} 条线,{} 站", c.name, stat.0, stat.1);
            assert!(
                !seen.contains(&stat),
                "{} 的数据和别的城市一模一样({:?}),多半是把同一个 dot 复制过来了",
                c.name,
                stat
            );
            seen.push(stat);
            assert!(m.names.len() > 80, "{} 只有 {} 站", c.name, m.names.len());
            assert!(m.lines.len() >= 3, "{} 只有 {} 条线", c.name, m.lines.len());
            // 重名会让解析端把两个站合成一个,数据里不该有
            let mut names: Vec<&String> = m.names.iter().collect();
            names.sort();
            let dup = names.windows(2).find(|w| w[0] == w[1]);
            assert!(dup.is_none(), "{} 里有重名站: {:?}", c.name, dup);
            // 示意布局里不能有两站摞在同一格(站一多最容易出这个)
            let t0 = std::time::Instant::now();
            let sc = crate::map::Scene::schematic(&m);
            let (bw, bh) = crate::map::nominal_for(m.pos.len());
            let mut cells: Vec<(i64, i64)> = sc
                .pos
                .iter()
                .map(|p| (p.0.round() as i64, p.1.round() as i64))
                .collect();
            cells.sort();
            let dup = cells.windows(2).find(|w| w[0] == w[1]);
            assert!(
                dup.is_none(),
                "{} 的示意布局把两站摞在同一格: {:?}(画布 {}x{})",
                c.name,
                dup,
                bw as i64,
                bh as i64
            );
            // 布局空间里"最近的邻居有多远"的中位数:衡量站是不是挤在一起
            let mut nn: Vec<f64> = (0..sc.pos.len())
                .map(|i| {
                    (0..sc.pos.len())
                        .filter(|&j| j != i)
                        .map(|j| {
                            let (dx, dy) = (sc.pos[i].0 - sc.pos[j].0, sc.pos[i].1 - sc.pos[j].1);
                            (dx * dx + dy * dy).sqrt()
                        })
                        .fold(f64::INFINITY, f64::min)
                })
                .collect();
            nn.sort_by(f64::total_cmp);
            println!(
                "  {} 布局耗时 {:?},画布 {}x{},最近邻中位 {:.1} 格",
                c.name,
                t0.elapsed(),
                bw as i64,
                bh as i64,
                nn[nn.len() / 2]
            );
        }
    }

    /// V:聚焦当前站的线路,再按一次取消
    #[test]
    fn 聚焦当前站的线路() {
        let m = Map::from_dot(include_str!("../docs/hangzhou-subway.dot")).unwrap();
        let mut view = View::default();
        // 武林广场 是 1/3 号线换乘站
        view.cursor = m.names.iter().position(|n| n == "武林广场").unwrap();
        let want = m.station_lines[view.cursor].clone();
        assert!(want.len() >= 2, "挑个换乘站来测");
        focus_lines_of(&m, &mut view);
        assert_eq!(view.focus, want, "该聚焦这一站的每条线");
        assert!(!view.hits.is_empty(), "命中列表该填上,好让 n 用");
        assert!(
            view.hits.iter().all(|&i| m
                .station_lines[i]
                .iter()
                .any(|l| want.contains(l))),
            "命中列表里不该有别的线的站"
        );
        assert!(
            view.hits.contains(&view.cursor),
            "当前站自己当然在里面"
        );
        // 单线站:只聚焦那一条
        let mut v2 = View::default();
        v2.cursor = m.names.iter().position(|n| n == "湘湖").unwrap();
        focus_lines_of(&m, &mut v2);
        assert_eq!(v2.focus.len(), 1, "湘湖 只在 1 号线上");
        // 再按一次取消
        focus_lines_of(&m, &mut v2);
        assert!(v2.focus.is_empty(), "同一站再按该取消");
        assert!(v2.hits.is_empty());
        // 聚焦里已经包含这一站的线(比如搜了别的线之后挪过来)也算取消
        let mut v3 = View::default();
        v3.cursor = m.names.iter().position(|n| n == "湘湖").unwrap(); // 1 号线
        v3.focus = vec![
            m.lines.iter().position(|l| l.label == "1号线").unwrap(),
            m.lines.iter().position(|l| l.label == "3号线").unwrap(),
        ];
        focus_lines_of(&m, &mut v3);
        assert!(v3.focus.is_empty(), "1 号线已经在聚焦里,这一下该是取消");
    }

    /// docs 目录里现成的 dot 会被列出来(内嵌的城市要跳过),--city 也能直接打开
    #[test]
    fn docs_里的城市不用重新编译() {
        let dir = std::env::temp_dir().join(format!("metro-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dot = "// 测试城地铁线路图:节点是站点\n\
                   graph test_metro {\n\
                   \"甲\" [pos=\"0,0\", pinyin=\"jia\"];\n\
                   \"乙\" [pos=\"1000,0\", pinyin=\"yi\"];\n\
                   \"丙\" [pos=\"2000,0\", pinyin=\"bing\"];\n\
                   \"甲\" -- \"乙\" [color=\"#df4661\", line=\"1号线\"];\n\
                   \"乙\" -- \"丙\" [color=\"#00ff00\", line=\"2号线\"];\n\
                   }\n";
        std::fs::write(dir.join("testcity-subway.dot"), dot).unwrap();
        std::fs::write(dir.join("shanghai-subway.dot"), dot).unwrap(); // 内嵌的,该跳过
        let found = city::external(&dir);
        let names: Vec<&str> = found.iter().map(|(s, _, _, _, _)| s.as_str()).collect();
        println!("扫到: {names:?}");
        assert!(names.contains(&"testcity"), "该扫到 docs 里的城市");
        assert!(!names.contains(&"shanghai"), "内嵌的该跳过");
        let (_, name, lines, stations, _) = found.iter().find(|(s, _, _, _, _)| s == "testcity").unwrap();
        assert_eq!(name, "测试城");
        assert_eq!(*lines, 2);
        assert_eq!(*stations, 3);
        // --city 能从 docs 读到
        let text = city::resolve("testcity", &[dir.clone()]).expect("该能从 docs 读到");
        assert!(Map::from_dot(&text).is_ok());
        assert!(city::resolve("nosuchcity", &[dir.clone()]).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// --city 指定城市;不认识的 slug 要报清楚
    #[test]
    fn 指定城市() {
        let args = parse_args(&["--city".to_string(), "shanghai".to_string()]).unwrap();
        assert_eq!(args.city.as_deref(), Some("shanghai"));
        assert_eq!(city::by_slug("shanghai").unwrap().name, "上海");
        assert!(city::by_slug("beijing").is_err());
        assert_eq!(city::CITIES.len(), 4);
    }

    #[test]
    fn hjkl_方向对得上() {
        // (列方向, 行方向):h 西、l 东、k 北、j 南;方向键一样
        assert_eq!(dir_of(KeyCode::Char('h')), Some((-1, 0)));
        assert_eq!(dir_of(KeyCode::Char('l')), Some((1, 0)));
        assert_eq!(dir_of(KeyCode::Char('k')), Some((0, -1)));
        assert_eq!(dir_of(KeyCode::Char('j')), Some((0, 1)));
        assert_eq!(dir_of(KeyCode::Left), Some((-1, 0)));
        assert_eq!(dir_of(KeyCode::Right), Some((1, 0)));
        assert_eq!(dir_of(KeyCode::Up), Some((0, -1)));
        assert_eq!(dir_of(KeyCode::Down), Some((0, 1)));
        // 大写:沿线走一站,只认上下一条(和屏幕方位无关)
        assert_eq!(topo_of(KeyCode::Char('H')), Some(false));
        assert_eq!(topo_of(KeyCode::Char('K')), Some(false));
        assert_eq!(topo_of(KeyCode::Char('J')), Some(true));
        assert_eq!(topo_of(KeyCode::Char('L')), Some(true));
        assert_eq!(dir_of(KeyCode::Char('x')), None);
    }
}





#[cfg(test)]
mod audit_gaps {
    use super::*;
    #[test]
    fn 大图在小终端里的站距() {
        for c in city::CITIES {
            let m = Map::from_dot(c.dot).unwrap();
            let sc = crate::map::Scene::schematic(&m);
            for (w, h) in [(120u16, 40u16), (200, 50), (300, 70)] {
                let lay = ui::layout(&m, &sc, ratatui::layout::Rect::new(0, 0, w, h), &View::default());
                let cells: Vec<(i32, i32)> = sc.pos.iter().map(|p| lay.proj.cell(*p)).collect();
                let mut nn: Vec<f64> = (0..cells.len())
                    .map(|i| {
                        (0..cells.len())
                            .filter(|&j| j != i)
                            .map(|j| {
                                let (dx, dy) = (cells[i].0 - cells[j].0, cells[i].1 - cells[j].1);
                                ((dx * dx + dy * dy) as f64).sqrt()
                            })
                            .fold(f64::INFINITY, f64::min)
                    })
                    .collect();
                nn.sort_by(f64::total_cmp);
                let tight = nn.iter().filter(|d| **d <= 1.0).count();
                let overlap = {
                    let mut s = cells.clone();
                    s.sort();
                    s.windows(2).filter(|w| w[0] == w[1]).count()
                };
                println!("{:<4} {:>3}x{:<3} 站距中位 {:.1} 格,<=1 格的 {} 站,同格 {} 处",
                    c.name, w, h, nn[nn.len() / 2], tight, overlap);
                // 200x50 是常用尺寸:这里不许有站叠在同一格(小终端挤是必然的,只用来看趋势)
                if (w, h) == (200, 50) {
                    assert_eq!(overlap, 0, "{} 在 200x50 下有站叠在同一格", c.name);
                }
            }
        }
    }
}
