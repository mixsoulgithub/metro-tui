"""docs/hangzhou-subway.md -> docs/hangzhou-subway.dot

文档只给了"线路 + 站点顺序",没有坐标.坐标从 OSM 抓下来的数据里取:
  1) 站名直接去 OSM 命名站点节点里查(去掉括号/空格后比对),命中就落在对应的 stop 节点上;
  2) 一条线路的所有关系按 150 米并成一张 stop 图(节点=物理站点,边=关系里相邻的 stop);
     文档里相邻的两站在图上走最短路,路上经过的站补进来 —— 跨支线的接法(6 号线富阳支线
     接美院象山、3 号线两条支路)因此能按真实走向画出来;
  3) 没命中的站:在前后两个锚点之间沿折线按距离均分插值,两端则按锚点方向外推;
  4) 颜色抄线路关系上的 colour.

用法: python3 tools/md2dot.py [-o docs/hangzhou-subway.dot]
"""
import argparse, heapq, json, math, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from metro_osm import (M_PER_DEG_LAT, build_refs, dedup_edges, dist_m, load_pinyin,
                       m_per_deg_lon, named_stations, norm, pinyin_variants, write_dot)
MD = os.path.join(ROOT, "docs", "hangzhou-subway.md")
STATIONS = os.path.join(ROOT, "data", "osm_stations.json")
ROUTES = os.path.join(ROOT, "data", "osm_routes.json")
PINYIN = os.path.join(ROOT, "data", "pinyin_chars.txt")
OUT = os.path.join(ROOT, "docs", "hangzhou-subway.dot")
GAP_M = 3000.0        # 文档里相邻两站超过这个距离就记一笔
TRACK_M = 400.0       # 站点节点离轨道这么近才认它是这条线上的一站

HEADER = [
    "杭州地铁线路图:节点是站点(pos 单位米,x 向东 y 向北),",
    "边是同一线路上相邻的两站,color/line 就是线路色与线路名.",
    "站点坐标与线路色来自 OpenStreetMap(ODbL),拼音来自 pypinyin 的字典,",
    "由 tools/md2dot.py 从 docs/hangzhou-subway.md + data/*.json 生成.",
]

Pt = tuple  # (lat, lon)


def parse_md(path):
    """-> [(线路名(支线带后缀), [站名...])]"""
    segs = []
    cur = None
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        m = re.fullmatch(r"(\d+号线)", line)
        if m:
            cur = m.group(1)
            continue
        if "–" not in line:
            continue
        branch = "支线" in line[:12]
        body = re.sub(r"^[^：]*包括：", "", line)
        body = re.sub(r"^另有通往[^：]*支线：", "", body)
        names = [s.strip() for s in body.split("–") if s.strip()]
        segs.append((cur + "支线" if branch else cur, names))
    return segs


def along(rd, a, b, count):
    """锚点 a..b 之间沿折线按长度均分,给出 count 个点"""
    pts = [rd.coord[k] for k in rd.path(a, b)]
    lens = [dist_m(pts[i], pts[i + 1]) for i in range(len(pts) - 1)]
    total = sum(lens)
    out = []
    for n in range(1, count + 1):
        want = total * n / (count + 1)
        acc = 0.0
        for i, L in enumerate(lens):
            if L and acc + L >= want:
                f = (want - acc) / L
                out.append((pts[i][0] + (pts[i + 1][0] - pts[i][0]) * f,
                            pts[i][1] + (pts[i + 1][1] - pts[i][1]) * f))
                break
            acc += L
        else:
            out.append(pts[-1])
    return out


def lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def side_stops(rd, a, b, count, before):
    """在某一串 stop 里,按 a -> b 的方向取 a 这一侧的 count 个节点.

    端点上没对上名字的站(比如萧山国际机场)靠这个补,比按方向瞎外推准.
    """
    for seq in rd.seqs:
        if a not in seq or b not in seq:
            continue
        ia, ib = seq.index(a), seq.index(b)
        if ia == ib:
            continue
        step = 1 if ib > ia else -1
        if before:
            out = [seq[ia - step * k] for k in range(1, count + 1) if 0 <= ia - step * k < len(seq)]
        else:
            out = [seq[ib + step * k] for k in range(1, count + 1) if 0 <= ib + step * k < len(seq)]
        if len(out) == count:
            return out
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-o", "--out", default=OUT)
    ap.add_argument("--md", default=MD)
    args = ap.parse_args()

    segs = parse_md(args.md)
    nodes = json.load(open(STATIONS, encoding="utf-8"))["elements"]
    routes = json.load(open(ROUTES, encoding="utf-8"))["elements"]
    named = named_stations(nodes)
    refs = build_refs(routes, named, ["杭州地铁"])
    pinyin = load_pinyin(PINYIN)

    out_nodes = {}
    out_edges = []
    problems = []
    used_by_ref = {}

    def place(name, p):
        if name not in out_nodes:
            out_nodes[name] = p
        return name

    def chain(rd, label, a_name, a_k, b_name, b_k):
        """a 走到 b:图上最短路,路过的站一并补进来"""
        if a_k == b_k:
            return
        prev = place(a_name, rd.coord[a_k])
        for k in rd.path(a_k, b_k)[1:-1]:
            nm = rd.name[k]
            if not nm or nm == prev:
                continue
            out_edges.append((prev, place(nm, rd.coord[k]), rd.colour, label))
            prev = nm
        if b_name != prev:
            out_edges.append((prev, place(b_name, rd.coord[b_k]), rd.colour, label))

    for label, names in segs:
        ref = label.replace("号线", "").replace("支线", "")
        rd = refs.get(ref)
        if rd is None:
            problems.append("OSM 里没有 %s 的关系" % ref)
            continue
        idx = [None] * len(names)
        coord = [None] * len(names)
        for i, nm in enumerate(names):
            on_graph = None     # (到 stop 的距离, stop 序号, 坐标)
            off_graph = None    # (到整条线路的距离, 坐标)
            for lat, lon, _ in named.get(norm(nm), []):
                c = (lat, lon)
                k = rd.nearest(c)
                if k is not None:
                    d = dist_m(c, rd.coord[k])
                    if on_graph is None or d < on_graph[0]:
                        on_graph = (d, k, c)
                    continue
                # stop 里没有这一站(OSM 关系常漏端点站):只要站点节点贴着这条线的
                # 轨道就认它,免得把同名的别的站也算进来
                d = min(dist_m(c, tp) for tp in rd.track) if rd.track else None
                if d is not None and d < TRACK_M and (off_graph is None or d < off_graph[0]):
                    off_graph = (d, c)
            if on_graph:
                idx[i], coord[i] = on_graph[1], on_graph[2]
            elif off_graph:
                coord[i] = off_graph[1]
        hinted = list(coord)
        pins = [i for i, c in enumerate(coord) if c is not None]
        if not pins:
            problems.append("%s: 一站都没对上,跳过" % label)
            continue
        # 相邻两个已知点之间:有图上节点的沿线路均分,否则按序号直线均分
        for a, b in zip(pins, pins[1:]):
            if b == a + 1:
                continue
            if idx[a] is not None and idx[b] is not None:
                for i, p in zip(range(a + 1, b), along(rd, idx[a], idx[b], b - a - 1)):
                    coord[i] = p
            else:
                for i in range(a + 1, b):
                    coord[i] = lerp(coord[a], coord[b], (i - a) / (b - a))
        # 两端:先看关系的 stop 序列里有没有现成的端点站,不行再按方向外推
        head, tail = pins[0], pins[-1]
        if head > 0:
            got = (side_stops(rd, idx[head], idx[pins[1]], head, True)
                   if len(pins) > 1 and idx[head] is not None and idx[pins[1]] is not None else None)
            for i in range(head - 1, -1, -1):
                if got:
                    coord[i] = rd.coord[got[head - 1 - i]]
                elif len(pins) > 1:
                    coord[i] = lerp(coord[head], coord[pins[1]], -(head - i) / (pins[1] - head))
                else:
                    coord[i] = coord[head]
        if tail < len(names) - 1:
            got = (side_stops(rd, idx[tail], idx[pins[-2]], len(names) - 1 - tail, False)
                   if len(pins) > 1 and idx[tail] is not None and idx[pins[-2]] is not None else None)
            for i in range(tail + 1, len(names)):
                if got:
                    coord[i] = rd.coord[got[i - tail - 1]]
                elif len(pins) > 1:
                    coord[i] = lerp(coord[tail], coord[pins[-2]], -(i - tail) / (tail - pins[-2]))
                else:
                    coord[i] = coord[tail]
        # 同一条线路的下一段(6 号线支线):端点接回已经画好的站
        known = used_by_ref.get(ref)
        if known:
            best = None
            for end in (0, len(names) - 1):
                k = idx[end]
                if k is None:
                    continue
                for m, m_name in known.items():
                    d = dist_m(rd.coord[k], rd.coord[m])
                    if best is None or d < best[0]:
                        best = (d, end, k, m, m_name)
            if best and best[2] != best[3]:
                d, end, k, m, m_name = best
                if d > 1000:
                    problems.append("%s: 端点 %s 接到 %s(%.1f km)" % (label, names[end], m_name, d / 1000))
                chain(rd, label, names[end], k, m_name, m)
        # 相邻两站:能走图就走图,否则直连(插值出来的站没有节点)
        for i in range(1, len(names)):
            a, b = idx[i - 1], idx[i]
            if a is not None and b is not None and a != b:
                gap = dist_m(rd.coord[a], rd.coord[b])
                if gap > GAP_M:
                    problems.append("%s: %s - %s 直线 %.1f km" % (label, names[i - 1], names[i], gap / 1000))
                chain(rd, label, names[i - 1], a, names[i], b)
            else:
                out_edges.append((place(names[i - 1], coord[i - 1]),
                                  place(names[i], coord[i]), rd.colour, label))
        used_by_ref.setdefault(ref, {}).update(
            {k: names[i] for i, k in enumerate(idx) if k is not None})
        interp = [names[i] for i, c in enumerate(hinted) if c is None]
        if interp:
            problems.append("%s: 靠插值的站 %s" % (label, " ".join(interp)))
        hit = len(pins) / len(names)
        print("%-14s %2d 站  对上有坐标 %.0f%%" % (label, len(names), hit * 100), file=sys.stderr)

    def report():
        if problems:
            print("\n需要注意:", file=sys.stderr)
            for p in problems:
                print("  - " + p, file=sys.stderr)

    if not out_nodes:
        print("没有可用数据", file=sys.stderr)
        report()
        return 1
    edges = dedup_edges(out_edges)
    write_dot(args.out, HEADER, "hangzhou_metro", out_nodes, edges, pinyin)
    used = {n for e in edges for n in e[:2]}
    alone = [n for n in out_nodes if n not in used]
    if alone:
        problems.append("孤立站点: " + " ".join(alone))
    inter = sum(1 for nm in out_nodes if sum(1 for e in edges if nm in e[:2]) > 2)
    print("站点 %d,边 %d(换乘/分叉 %d 个)-> %s" % (len(out_nodes), len(edges), inter, args.out),
          file=sys.stderr)
    report()
    return 0


if __name__ == "__main__":
    sys.exit(main())
