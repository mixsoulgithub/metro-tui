English | [中文](README.md)

# metro-tui

Metro maps in the terminal. Drawn with `ratatui` + `crossterm`: **box-drawing characters**, one
24-bit colour per line. Station order and coordinates come from hand-checked tables and
OpenStreetMap; the same data draws either as a geographic projection (default) or as a relaxed
schematic.

![demo](assets/demo.gif)

*Hangzhou, geographic view: arrow-key hops, zoom, pinyin search, focus a line, walk its stops,
schematic toggle.*

## Features

- **Geographic by default**; `--schematic` (or `r`) switches to the schematic layout.
- **Stop order is the data**: `J`/`L` next stop, `H`/`K` previous stop — the line's own order,
  independent of screen direction and geography.
- **Arrow keys use screen geometry**: nearest station roughly in that direction.
- **Search**: Chinese, pinyin (`wulin`), initials (`wlgc`), or a line number (`/1`).
- **Mouse**: click a station, drag to pan, wheel to zoom, `Ctrl`+wheel to pan.
- **Multi-city**: a chooser at startup, `--city <slug>` to skip it; new cities need no rebuild.
- **Headless**: `--dump` prints one true-colour ANSI frame.
- **Tiny**: only `ratatui`, `crossterm`, `unicode-width`. No network or database at runtime.

## Install

Prebuilt binaries (x86_64 Linux, glibc and static musl) are on the [releases page](https://github.com/mixsoulgithub/metro-tui/releases/latest):

```bash
curl -L https://github.com/mixsoulgithub/metro-tui/releases/latest/download/metro-tui-x86_64-unknown-linux-musl.tar.gz | tar xz
install -m755 metro-tui ~/.local/bin/          # or anywhere on your PATH
metro-tui --city hangzhou
```

- **Homebrew** (Linux x86_64, static binary): `brew install mixsoulgithub/tap/metro-tui`
- **conda / micromamba**: recipe in `packaging/conda/meta.yaml` — `conda build` it and publish to
  your own channel, then `conda install -c <channel> metro-tui`

From source (needs Rust), no crates.io account required:

```bash
cargo install --git https://github.com/mixsoulgithub/metro-tui
cargo build --release                      # or clone and build
```

## Build & run

Rust (edition 2021) and a UTF-8 terminal with true colour.

```bash
cargo build --release
./target/release/metro-tui                # city chooser
./target/release/metro-tui --city chengdu # straight into a city
```

A whole city is about 97 km x 51.5 km, so ~200x50 uses the screen well.

Platforms: Linux x86_64 (dynamic glibc and static musl), macOS arm64/x86_64, and Windows
x86_64 (it compiles and is released, but has not been tested on Windows).

## Options

```
--city <slug>  open a city (hangzhou shanghai chengdu shenzhen ...)
--file <path>  read a map .dot
--dump         print one frame (ANSI) and exit; combine with --size
--size <WxH>   canvas size for --dump (default 200x50)
--labels       show all station labels at startup (same as b)
--schematic    schematic layout (default is geographic; r toggles)
--real         geographic layout (the default; kept for old commands)
--at <name>    cursor on that station at startup
--find <text>  search at startup (station / pinyin / initials / line)
```

## Keys

| Key | Action |
| --- | --- |
| `h j k l` / arrows | nearest station roughly in that direction (screen distance + 2x sideways offset, 45-degree cone); with a focused line, stations on it get a larger allowance (`:cycle`) |
| `H K` / `J L` | previous / next stop along the line — its own stop order, not geography. Several lines can move → a list pops up (digits, `j`/`k`, enter) |
| `+` `-` `0` | zoom in / out / reset to the whole map |
| `space` `z` | centre the view on the cursor station |
| `f` | fit the focused line (or the cursor station's line) to the screen |
| `b` | toggle station labels |
| `/` | search stations / pinyin / initials; a bare number highlights that line |
| `5l` `3j` | numeric prefix: move several stops |
| `Ctrl-d` `Ctrl-u` | half-screen leap down / up |
| `Ctrl-o` `Ctrl-i` / `Tab` | back / forward through the stations you jumped to |
| `n` `N` | next / previous search hit |
| `V` | focus the lines at the cursor station (again to clear) |
| `r` | geographic ↔ schematic |
| `?` | key cheat sheet |
| `:` | debug: `:cycle <x>` same-line allowance, `:side <x>` sideways weight, `:cycle auto` |
| mouse | click = jump, drag = pan, wheel = zoom, `Ctrl`+wheel = pan (tmux needs `-g mouse on`) |
| `q` / `esc` | back to the city chooser; press it there to quit |

The cursor station is solid (`●`, `◉` for an interchange) and pulses white↔red at ~75 bpm.

## Projections

**Geographic** (default): equirectangular straight from lat/lon, with a vertical correction of
roughly "a cell is twice as tall as wide". Positions are real; dense areas bunch up.

**Schematic** (`--schematic` / `r`): a deterministic relaxation from the real coordinates — springs
between neighbours, snapping to horizontal/vertical/45 degrees, straightening three-in-a-row per
line, repulsion to at least one cell apart, then cell snapping and a cleanup pass that keeps any two
stations at least one cell apart. Relative bearings survive; a 200x50 window gets an enlarged map,
not a squeezed one.

Drawing rules: `○` station, `◎` interchange, colours are the line's; orthogonal runs with
`─ │ ╭ ╮ ╯ ╰`; where two lines cross away from a station one passes under (a one-cell gap), so
`┼`-like glyphs only appear on stations; one colour per cell, and on conflict the **cursor
station's line wins**; terminus labels are always drawn, `b` forces all of them.

## Data

Station order lives in tables: `docs/*.md` (hand-checked) → `tools/csv2dot.py` → `docs/*.dot`
(Hangzhou's goes through `tools/md2dot.py`, which pulls coordinates from OSM and interpolates the
gaps). A `.dot` is either compiled in (`include_str!`) or just dropped in `docs/` or the user data
dir (`$XDG_DATA_HOME/metro/cities`, default `~/.local/share/metro/cities`) — both are scanned at
startup, so a new city needs no rebuild.

Any city can also be generated from OSM:

```
python3 tools/build_cities.py --all      # fetch + build, resumable
python3 tools/build_cities.py --city wuhan
```

Add a city by adding one entry to the table in `tools/cities.py` (slug, name, OSM network tags,
bbox). An empty `networks` list means "every `route=subway` relation in the bbox". The fetcher
probes Overpass mirrors at startup, rotates and backs off on failures, and prefers fetching station
nodes by id over whole-city queries. A proxy is picked up from the usual environment variables.

Sources: station coordinates and line colours from OpenStreetMap (`route=subway` relations,
`station=subway` nodes), (c) OpenStreetMap contributors, **ODbL**; the pinyin table is derived from
[pypinyin](https://github.com/mozillazg/python-pinyin) (MIT).

## Trade-offs

- A line never runs through another station and never closes a loop of its own; three small loops
  remain in the bundled Hangzhou map, closed by a final straight segment.
- Dense centres sit under one cell apart; zoom in or enlarge the window.
- Interchanges whose lines have separate OSM nodes are merged by name and proximity.
- `○ ◎ ─` have ambiguous East Asian width: a wide-character terminal with CJK fonts may draw them
  two cells wide and shift the map.

## License

Code: MIT ([LICENSE](LICENSE)). Map data: (c) OpenStreetMap contributors, ODbL.
