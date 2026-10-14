"""几号线,名称,经度,纬度 的 csv(上海 docs/shanghai-subway.md、成都 docs/chengdu.md、
深圳 docs/shenzhen.md)-> docs/<slug>-subway.dot

站序就按文件里的先后:一条线路一个块,块内顺序就是站序.同名站(换乘)在多块里重复,
合并成同一个节点.线路颜色能找到 data/osm/<slug>_routes.json(OSM 的 colour 标签)就用它,
找不到按序号取调色板.

块里遇到"相邻两站隔得离谱"的那种跳,分三种情况处理(都会打印出来):
  1. 前 8 站里有明显更近的站 —— 那是"主线走到枢纽 -> 岔去支线 -> 又接回主线"的写法
     (枢纽没有重复写),把这一跳接回岔口;
  2. 接不回岔口,但能把这一段和前面的段用"最近的一对站"接上,而且没有第二个差不多近的
     候选(否则是歧义,不敢猜)—— 就按那一对接上(文件把线路写成前后两截时会这样);
  3. 都对不上:丢掉这条边,让这段单成一块,并打印出来请人核对文件.

用法: python3 tools/csv2dot.py --city shanghai [--city ...|--all]
"""
import argparse, csv, io, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from metro_osm import dedup_edges, dist_m, load_pinyin, write_dot

DOCS = os.path.join(ROOT, "docs")
PINYIN = os.path.join(ROOT, "data", "pinyin_chars.txt")

from cities import CITIES, csv_path, dot_path

# 只处理"站序表是 csv 格式"的城市:杭州的表是老格式的 md,走 tools/md2dot.py
MANUAL = {s: c for s, c in CITIES.items() if c["csv"] and s != "hangzhou"}

PALETTE = [
    "#e4002b", "#0072bc", "#00a650", "#f6a800", "#8e44ad", "#00b5cc", "#d81b60",
    "#5d4037", "#7cb342", "#f4511e", "#3949ab", "#00897b", "#c0ca33", "#6d4c41",
    "#26a69a", "#ef6c00", "#5c6bc0", "#ad1457", "#283593", "#558b2f", "#00838f",
]

LONG = 1.6      # 超过中位间距这么多倍,当成"跳"
ABSURD = 12.0   # 跳得这么离谱又接不回去:多半是文件把两截线路写在一起
                # (机场快线那种十几公里没站的真长区间不够这个倍数,别误伤)
NEAR = 1.5      # "算得上近"的标准:不超过中位间距这么多倍
TIE = 1.05      # 两个候选差不多近时,取文件里更靠前的那个(岔口在支线第一站之前)
CLEAR = 1.3     # 跨段接续:最好的那对比第二好的近这么多倍才敢接


def read_blocks(path):
    """-> [(线路名, [(站名, 经度, 纬度)...])],保持文件顺序"""
    blocks = []
    where = {}
    with io.open(path, encoding="utf-8-sig", newline="") as f:
        rows = [r for r in csv.reader(f) if r and any(c.strip() for c in r)]
    head = rows[0]
    if len(head) < 4 or "经" not in head[2] + head[3]:
        raise SystemExit("%s: 第一行该是表头(几号线,名称,经度,纬度)" % path)
    for r in rows[1:]:
        if len(r) < 4:
            continue
        line, name = r[0].strip(), r[1].strip()
        if not line or not name:
            continue
        lon = r[2].strip()
        lat = r[3].strip()
        # 坐标可以空着:同一个站名在别的块里出现过的话,回头补上
        ll = (float(lon), float(lat)) if lon and lat else None
        if line not in where:
            where[line] = len(blocks)
            blocks.append((line, []))
        blocks[where[line]][1].append((name, ll))
    # 补坐标
    known = {}
    for _, stops in blocks:
        for nm, ll in stops:
            if ll is not None:
                known.setdefault(nm, ll)
    for label, stops in blocks:
        for k, (nm, ll) in enumerate(stops):
            if ll is None:
                if nm not in known:
                    raise SystemExit("%s: 站 %s 没有坐标,别的块里也没有同名的" % (label, nm))
                stops[k] = (nm, known[nm])
    return blocks


def colours_from_osm(slug):
    """data/osm/<slug>_routes.json 里的 ref -> colour(有就用真颜色)"""
    path = os.path.join(ROOT, "data", "osm", "%s_routes.json" % slug)
    if not os.path.exists(path):
        return {}
    import json
    try:
        els = json.load(open(path, encoding="utf-8"))["elements"]
    except Exception:
        return {}
    out = {}
    for e in els:
        t = e.get("tags") or {}
        ref = (t.get("ref") or "").strip()
        col = t.get("colour") or t.get("color")
        if ref and col:
            out.setdefault(ref, col)
            out.setdefault(ref + "号线", col)
    return out


def metres(a, b):
    """a, b 是 (经度, 纬度)"""
    return dist_m((a[1], a[0]), (b[1], b[0]))


def chain(label, pts, med, repairs, notes):
    """块内成边.pts 是 [(站名, 经度, 纬度)].

    默认就按文件顺序连;只有当相邻两站隔得离谱(超过中位间距 6 倍)又接不回岔口时,
    才把那里断开,再拿断口之后那一段去跟前面找最近的一对接上(接不上就单放着).
    """
    n = len(pts)
    if n < 2:
        return []
    gaps = [metres(pts[k][1:], pts[k + 1][1:]) for k in range(n - 1)]
    edges = []
    cuts = []          # 断开处:断口后那一段的站序
    prev = 0
    for k in range(1, n):
        d = gaps[k - 1]
        if d <= med * LONG:
            edges.append((pts[prev][0], pts[k][0]))
            prev = k
            continue
        # 前 8 站里有明显更近的 —— "主线走到枢纽 -> 岔去支线 -> 又接回主线"的写法
        near = [(j, metres(pts[j][1:], pts[k][1:])) for j in range(max(0, k - 8), k)]
        near = [(j, dj) for j, dj in near if dj <= med * NEAR]
        if near:
            dmin = min(dj for _, dj in near)
            cand = min(j for j, dj in near if dj <= dmin * TIE)
            if cand != k - 1:
                repairs.append("%s: %s -> %s 接回 %s(省掉 %.1f km 的直线)"
                               % (label, pts[k - 1][0], pts[k][0], pts[cand][0], d / 1000))
                edges.append((pts[cand][0], pts[k][0]))
                prev = k
                continue
        if d > med * ABSURD:
            # 离谱又接不回岔口:多半是文件把线路写成了前后两截
            notes.append("%s: %s 与 %s 之间 %.1f km,先断开" % (label, pts[k - 1][0], pts[k][0], d / 1000))
            cuts.append(k)
            prev = k
            continue
        # 真长区间(郊区那种),照连
        notes.append("%s: %s - %s 直线 %.1f km(真长区间,保留)" % (label, pts[k - 1][0], pts[k][0], d / 1000))
        edges.append((pts[prev][0], pts[k][0]))
        prev = k

    # 断开的地方:拿断口后那一段的站,跟前面已经连好的所有站逐对比,取最近的一对.
    # 第二好的那对如果跟它差不多近,就是歧义,不敢接
    for si, cut in enumerate(cuts):
        end = cuts[si + 1] if si + 1 < len(cuts) else n
        before = list(range(0, cut))
        cands = sorted((metres(pts[k][1:], pts[j][1:]), k, j)
                       for k in range(cut, end) for j in before)
        if not cands:
            continue
        d, k, j = cands[0]
        if d > med * NEAR:
            repairs.append("%s: %s 那一段接不回主线(最近的一对 %s %.1f km 也太远),先单放着"
                           % (label, pts[cut][0], pts[j][0], d / 1000))
            continue
        if len(cands) > 1 and d * CLEAR > cands[1][0]:
            repairs.append("%s: %s 那一段有两个差不多近的接口(%s %.0f m / %s %.0f m),不猜,先单放着"
                           % (label, pts[cut][0], pts[j][0], d, pts[cands[1][2]][0], cands[1][0]))
            continue
        repairs.append("%s: %s 那一段接回 %s(%.0f m)" % (label, pts[cut][0], pts[j][0], d))
        edges.append((pts[j][0], pts[k][0]))
    return edges


def build(slug, in_path, out_path):
    name = CITIES[slug]["name"]
    blocks = read_blocks(in_path)
    colours = colours_from_osm(slug)
    pinyin = load_pinyin(PINYIN)

    pos = {}
    count = {}
    raw_edges = []
    repairs, notes = [], []
    for i, (label, stops) in enumerate(blocks):
        ref = label.split("/")[0].replace("号线", "").replace("支线", "")
        colour = colours.get(ref) or colours.get(label) or PALETTE[i % len(PALETTE)]
        pts = []
        for nm, (lon, lat) in stops:
            c = pos.get(nm)
            n = count.get(nm, 0)
            pos[nm] = ((lon, lat) if c is None
                       else ((c[0] * n + lon) / (n + 1), (c[1] * n + lat) / (n + 1)))
            count[nm] = n + 1
            pts.append((nm, lon, lat))
        gaps = [metres(pts[k][1:], pts[k + 1][1:]) for k in range(len(pts) - 1)]
        med = sorted(gaps)[len(gaps) // 2] if gaps else 0.0
        for a, b in chain(label, pts, med, repairs, notes):
            raw_edges.append((a, b, colour, label))

    ok = {n for e in raw_edges for n in e[:2]}
    nodes = {nm: (v[1], v[0]) for nm, v in pos.items() if nm in ok}   # write_dot 要 (lat, lon)
    edges = dedup_edges([e for e in raw_edges if e[0] in nodes and e[1] in nodes])
    header = [
        "%s地铁线路图:节点是站点(pos 单位米,x 向东 y 向北)," % name,
        "边是同一线路上相邻的两站,color/line 就是线路色与线路名.",
        "站点顺序与坐标来自 docs/%s,线路色来自 OpenStreetMap(ODbL)," % os.path.basename(in_path),
        "拼音来自 pypinyin 的字典,由 tools/csv2dot.py 生成.",
    ]
    write_dot(out_path, header, slug + "_metro", nodes, edges, pinyin)
    lines = {}
    for _, _, _, l in edges:
        lines[l] = lines.get(l, 0) + 1
    print("city=%s stations=%d edges=%d lines=%d interchanges=%d bytes=%d"
          % (slug, len(nodes), len(edges), len(lines),
             sum(1 for nm in nodes if sum(1 for e in edges if nm in e[:2]) > 2),
             os.path.getsize(out_path)))
    for r in repairs:
        print("  修正:", r)
    for t in notes:
        print("  断开:", t)
    return {"slug": slug, "stations": len(nodes), "edges": len(edges), "lines": len(lines)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--city", action="append", default=[])
    ap.add_argument("--all", action="store_true")
    args = ap.parse_args()
    if args.all:
        # 凡是有 csv 站序表的都能重建(杭州的表是老格式的 md,走 tools/md2dot.py,跳过)
        slugs = [s for s in sorted(CITIES)
                 if not CITIES[s].get("md2dot")
                 and os.path.exists(os.path.join(ROOT, csv_path(s)))]
    else:
        slugs = args.city
    if not slugs:
        print("用法: python3 tools/csv2dot.py --city shanghai [--city ...|--all]")
        print("城市:", ", ".join("%s(%s)" % (s, CITIES[s]["name"]) for s in sorted(MANUAL)))
        return 1
    bad = []
    for slug in slugs:
        if slug not in CITIES:
            print("没有这个城市:", slug, file=sys.stderr)
            bad.append(slug)
            continue
        src = (csv_path(slug) if os.path.isabs(csv_path(slug)) else os.path.join(ROOT, csv_path(slug)))
        if not os.path.exists(src):
            print("%s: 缺 %s" % (slug, src), file=sys.stderr)
            bad.append(slug)
            continue
        if not os.path.exists(src):
            print("%s: 缺 %s" % (slug, src), file=sys.stderr)
            return 1
        out = (dot_path(slug) if os.path.isabs(dot_path(slug)) else os.path.join(ROOT, dot_path(slug)))
        os.makedirs(os.path.dirname(out), exist_ok=True)
        try:
            build(slug, src, out)
        except SystemExit:
            bad.append(slug)
    if bad:
        print("没建成的:%s" % " ".join(bad), file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
