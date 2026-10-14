"""抓 OSM 上的杭州地铁数据,给 tools/md2dot.py 用.

两个东西:
  data/osm_routes.json    route=subway 关系(带成员几何),里面是各线路的 stop 顺序和 colour
  data/osm_stations.json  station=subway 的命名站点节点(站名 -> 坐标)

Overpass 对大结果会 504,所以站点按 0.06 度小格分开查,结果缓存在 .cache/ 里,可反复重跑.

用法: python3 tools/fetch_osm.py [--cache .cache]
"""
import argparse, concurrent.futures as cf
import json, math, os, sys, time, urllib.parse, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EP = "https://overpass-api.de/api/interpreter"
UA = {"User-Agent": "hangzhou-metro-map/1.0 (data preparation)"}
BBOX = (29.5, 119.5, 30.9, 121.0)   # s, w, n, e
STEP = 0.06
GRID = 0.002                        # 线路 stop 落格的粒度


def overpass(query, tries=4):
    data = urllib.parse.urlencode({"data": query}).encode()
    last = ""
    for attempt in range(tries):
        try:
            req = urllib.request.Request(EP, data=data, headers=UA)
            with urllib.request.urlopen(req, timeout=180) as r:
                txt = r.read().decode("utf-8", "replace")
            if txt.lstrip().startswith("{"):
                return json.loads(txt)["elements"]
            last = txt[:60]
        except Exception as ex:
            last = str(ex)[:60]
        time.sleep(1.5 + 2.0 * attempt)
    raise RuntimeError(last)


def fetch_routes(path):
    q = ('[out:json][timeout:300];'
         'rel["route"="subway"]["network"="杭州地铁"](%f,%f,%f,%f);out geom;' % BBOX)
    els = overpass(q)
    json.dump({"version": 0.6, "elements": els}, open(path, "w", encoding="utf-8"), ensure_ascii=False)
    print("%s: %d 条关系" % (path, len(els)), file=sys.stderr)
    return els


def cells(routes):
    seen = set()
    for e in routes:
        for m in e.get("members", []):
            if m["type"] == "node" and m.get("role") == "stop":
                seen.add((math.floor(m["lat"] / STEP), math.floor(m["lon"] / STEP)))
            for p in m.get("geometry") or []:
                seen.add((math.floor(p["lat"] / STEP), math.floor(p["lon"] / STEP)))
    return sorted(seen)


def fetch_stations(routes, cache, path):
    def one(cell):
        ci, cj = cell
        key = os.path.join(cache, "v2_%d_%d.json" % (ci, cj))
        if os.path.exists(key):
            return
        bbox = (ci * STEP, cj * STEP, (ci + 1) * STEP, (cj + 1) * STEP)
        # 车站的三种常见标法都要: 只标 station=subway 的、railway=station+subway=yes 的、
        # public_transport=station+subway=yes 的
        els = overpass(
            '[out:json][timeout:60];('
            'node["station"="subway"](%.4f,%.4f,%.4f,%.4f);'
            'node["railway"="station"]["subway"="yes"](%.4f,%.4f,%.4f,%.4f);'
            'node["public_transport"="station"]["subway"="yes"](%.4f,%.4f,%.4f,%.4f);'
            ');out body;' % (bbox * 3))
        json.dump(els, open(key, "w", encoding="utf-8"), ensure_ascii=False)

    todo = cells(routes)
    fails = []
    with cf.ThreadPoolExecutor(max_workers=3) as ex:
        futs = {ex.submit(one, c): c for c in todo}
        for i, f in enumerate(cf.as_completed(futs), 1):
            try:
                f.result()
            except Exception as ex2:
                fails.append(futs[f])
                print("失败 %s: %s" % (futs[f], ex2), file=sys.stderr)
            if i % 10 == 0:
                print("进度 %d/%d" % (i, len(todo)), file=sys.stderr, flush=True)
    for c in list(fails):
        for _ in range(6):
            try:
                one(c)
                fails.remove(c)
                break
            except Exception:
                time.sleep(5)
    nodes = {}
    for f in os.listdir(cache):
        for e in json.load(open(os.path.join(cache, f), encoding="utf-8")):
            if e.get("type") == "node":
                nodes[e["id"]] = e
    json.dump({"version": 0.6, "elements": list(nodes.values())},
              open(path, "w", encoding="utf-8"), ensure_ascii=False)
    print("%s: %d 个站点节点(失败格子 %d)" % (path, len(nodes), len(fails)), file=sys.stderr)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=os.path.join(ROOT, ".cache"))
    args = ap.parse_args()
    os.makedirs(args.cache, exist_ok=True)
    os.makedirs(os.path.join(ROOT, "data"), exist_ok=True)
    routes_path = os.path.join(ROOT, "data", "osm_routes.json")
    stations_path = os.path.join(ROOT, "data", "osm_stations.json")
    routes = (json.load(open(routes_path, encoding="utf-8"))["elements"]
              if os.path.exists(routes_path) else fetch_routes(routes_path))
    fetch_stations(routes, args.cache, stations_path)


if __name__ == "__main__":
    main()
