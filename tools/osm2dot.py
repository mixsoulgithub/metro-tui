"""osm_dir(slug)/<slug>_routes.json + <slug>_stations.json -> 该城市的 <slug>-subway.dot

直接按 OSM 的 route=subway 关系生成:关系里 stop 成员的顺序就是站序,
stop 点按 150 米聚类成站体,站名取 300 米内最近的 station=subway 节点。
同一站名在不同线路上的站体会并成一个节点(换乘站),这是渲染端要的效果。

和 tools/md2dot.py 的区别:杭州那条是"手写站序文档 + OSM 补坐标",
这里是"完全靠 OSM",所以 OSM 里缺站点节点、或者 stop 顺序乱的线路会差一些。

用法: python3 tools/osm2dot.py --city shanghai [--city chengdu ...]
"""
import argparse, json, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from metro_osm import build_refs, dedup_edges, dist_m, load_pinyin, named_stations, write_dot

DOCS = os.path.join(ROOT, "docs")
PINYIN = os.path.join(ROOT, "data", "pinyin_chars.txt")

from cities import CITIES, dot_path, osm_dir

# 线路色缺失时按顺序取(够用就行,OSM 里基本都带 colour)
FALLBACK_COLOURS = [
    "#e4002b", "#0072bc", "#00a650", "#f6a800", "#8e44ad", "#00b5cc", "#d81b60",
    "#5d4037", "#7cb342", "#f4511e", "#3949ab", "#00897b", "#c0ca33", "#6d4c41",
]


def label_of(ref, cn=True):
    """线路名的规范写法:国内是 1 -> 1号线,6支线 -> 6号线支线;
    海外城市线路的 ref 就照原样(那边的 "1" 就是 1,不加"号线")"""
    ref = ref.strip()
    if not cn:
        return ref
    m = re.fullmatch(r"(\d+)\s*(支线|支)", ref)
    if m:
        return m.group(1) + "号线支线"
    if re.fullmatch(r"\d+", ref):
        return ref + "号线"
    return ref


def sort_key(label):
    """线路顺序:先按数字排,再按名字(1,2,...,9,10,...,支线)"""
    m = re.match(r"(\d+)", label)
    return (0, int(m.group(1)), "支线" in label, label) if m else (1, 0, False, label)


def build(city_slug, routes_path, stations_path, out_path, min_stop_km=1000.0):
    name = CITIES[city_slug]["name"]
    nets = CITIES[city_slug]["networks"] or None   # 留空 = bbox 内所有 subway
    routes = json.load(open(routes_path, encoding="utf-8"))["elements"]
    stations = json.load(open(stations_path, encoding="utf-8"))["elements"]
    named = named_stations(stations)

    keep = [r for r in routes if nets is None
            or (r.get("tags") or {}).get("network") in nets]
    others = {}
    for r in routes:
        net = (r.get("tags") or {}).get("network", "(无)")
        if nets and net not in nets:
            others[net] = others.get(net, 0) + 1
    refs = build_refs(keep, named, nets)

    # ref -> 线路名 / 颜色;颜色缺失的按出现顺序补
    meta = {}
    for r in keep:
        t = r.get("tags") or {}
        ref = (t.get("ref") or t.get("name") or "").strip()
        if not ref:
            continue
        colour = t.get("colour") or t.get("color")
        slot = meta.setdefault(ref, {"colour": colour, "labels": set()})
        if colour:
            slot["colour"] = colour
        slot["labels"].add(label_of(ref, bool(nets)))
    idx = 0
    for ref, slot in sorted(meta.items()):
        if not slot["colour"]:
            slot["colour"] = FALLBACK_COLOURS[idx % len(FALLBACK_COLOURS)]
            idx += 1

    print("关系里的线路名:%s" % ", ".join(sorted({l for slot in meta.values() for l in slot["labels"]}
                                              , key=sort_key)))

    # 每个站名的坐标:同名(换乘)取平均,同一条线上重名的另说
    names_pos = {}
    claimed = {}
    raw_edges = []
    dropped = []
    problems = []
    for ref, rd in sorted(refs.items()):
        slot = meta.get(ref)
        if slot is None:
            continue
        label = sorted(slot["labels"])[0]
        used_here = set()
        for seq in rd.seqs:
            prev = None
            for k in seq:
                nm = rd.name[k]
                if not nm:
                    continue
                if nm in used_here:
                    # 同一条线上撞名:多半是两个同名站体被聚成一个名字,跳过以免自环
                    continue
                used_here.add(nm)
                p = rd.coord[k]
                old = names_pos.get(nm)
                cnt = claimed.get(nm, 0)
                if old is None:
                    names_pos[nm] = p
                else:
                    if dist_m(old, p) > 2000.0:
                        problems.append("%s: 同名站 %s 相隔 %.1f km" % (label, nm, dist_m(old, p) / 1000))
                    names_pos[nm] = ((old[0] * cnt + p[0]) / (cnt + 1), (old[1] * cnt + p[1]) / (cnt + 1))
                claimed[nm] = cnt + 1
                if prev and prev != nm:
                    raw_edges.append((prev, nm, slot["colour"], label))
                prev = nm
        unnamed = sum(1 for nm in rd.name if nm is None)
        if unnamed:
            dropped.append("%s: %d 个 stop 没名字(跳过)" % (label, unnamed))

    edges = dedup_edges(raw_edges)
    # 顺一下:节点按名字排序,边按线路 + 站名排序,输出才稳定
    used = {n for e in edges for n in e[:2]}
    nodes = {nm: names_pos[nm] for nm in sorted(names_pos) if nm in used}
    if not nodes:
        print("%s: 一个站点都没对上名字 —— 多半是站点表不是这个城市的" % city_slug, file=sys.stderr)
        return {"city": city_slug, "stations": 0, "edges": 0, "lines": 0,
                "interchanges": 0, "unnamed_stops": 0, "bytes": 0, "long_edges": 0}
    edges = [(a, b, c, l) for a, b, c, l in edges if a in nodes and b in nodes]
    edges.sort(key=lambda e: (sort_key(e[3]), e[0], e[1]))
    pinyin = load_pinyin(PINYIN)

    header = [
        "%s地铁线路图:节点是站点(pos 单位米,x 向东 y 向北)," % name,
        "边是同一线路上相邻的两站,color/line 就是线路色与线路名.",
        "站点坐标与线路名来自 OpenStreetMap(ODbL),拼音来自 pypinyin 的字典,",
        "由 tools/osm2dot.py 从 data/osm/%s_*.json 生成." % city_slug,
    ]
    write_dot(out_path, header, city_slug + "_metro", nodes, edges, pinyin)

    lines = {}
    for _, _, _, l in edges:
        lines[l] = lines.get(l, 0) + 1
    # 长边:多半是 OSM 缺了中间那站的名字(站被跳过),或者真有一条很长的区间
    long_edges = []
    for a, b, _, label in edges:
        d = dist_m(nodes[a], nodes[b])
        if d > min_stop_km:
            long_edges.append((d, label, a, b))
    long_edges.sort(reverse=True)

    stat = {
        "city": city_slug,
        "stations": len(nodes),
        "edges": len(edges),
        "lines": len(lines),
        "interchanges": sum(1 for nm in nodes if sum(1 for e in edges if nm in e[:2]) > 2),
        "unnamed_stops": sum(int(d.split(":")[1].split()[0]) for d in dropped),
        "bytes": os.path.getsize(out_path),
        "long_edges": len(long_edges),
    }
    print(" ".join("%s=%s" % kv for kv in stat.items()))
    if others:
        print("其它 network(没要):", ", ".join("%s x%d" % kv for kv in sorted(others.items())))
    for d in dropped[:12]:
        print("  略过:", d)
    for d, label, a, b in long_edges[:8]:
        print("  长边: %s %s - %s 直线 %.1f km" % (label, a, b, d / 1000.0))
    for p in problems[:12]:
        print("  注意:", p)
    print("线路:", ", ".join("%s(%d 段)" % kv for kv in sorted(lines.items(), key=lambda kv: sort_key(kv[0]))))
    return stat


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--city", action="append", default=[])
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--routes")
    ap.add_argument("--stations")
    ap.add_argument("-o", "--out")
    ap.add_argument("--min-stop-km", type=float, default=1000.0)
    args = ap.parse_args()
    slugs = sorted(CITIES) if args.all else args.city
    if not slugs:
        print("用法: python3 tools/osm2dot.py --city shanghai [--city ...|--all]")
        print("城市:", ", ".join("%s(%s)" % (s, CITIES[s]["name"]) for s in sorted(CITIES)))
        return 1
    for slug in slugs:
        if slug not in CITIES:
            print("没有这个城市:", slug, file=sys.stderr)
            return 1
        routes = args.routes or os.path.join(osm_dir(slug), "%s_routes.json" % slug)
        stations = args.stations or os.path.join(osm_dir(slug), "%s_stations.json" % slug)
        out = os.path.abspath(args.out or (dot_path(slug) if os.path.isabs(dot_path(slug)) else os.path.join(ROOT, dot_path(slug))))
        os.makedirs(os.path.dirname(out), exist_ok=True)
        if not os.path.exists(routes) or not os.path.exists(stations):
            print("%s: 缺数据文件(%s),先跑 tools/fetch_city.py --city %s" % (slug, routes, slug),
                  file=sys.stderr)
            return 1
        build(slug, routes, stations, out, args.min_stop_km)
    return 0


if __name__ == "__main__":
    sys.exit(main())
