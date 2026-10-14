"""按城市抓 OSM 地铁数据,给线路图渲染用.

每个城市出两份:
  data/osm/<slug>_routes.json    route=subway 关系(带成员几何),里面是各线路的 stop 顺序和 colour
  data/osm/<slug>_stations.json  station=subway 的命名站点节点(站名 -> 坐标)

线路和站点都是"整个 bbox 一次查"下来(按格查要几百次请求,慢几十倍):
站点用三种常见标法各查一次,哪种整城查不动,才把那种退回 0.06 度分格扫,
分格结果缓存在 .cache/<slug>_<i>_<j>.json,可反复重跑.
线路按 network 标签筛;没命中的 network 会连条数一起打印,方便发现漏掉的运营方.
Overpass 主站经常 504/SSL 掉线,所以备了几个镜像轮着重试.

用法: python3 tools/fetch_city.py --city shanghai [--city chengdu] [--cache .cache] [--refresh-routes]
      python3 tools/fetch_city.py --all
"""
import argparse, collections, concurrent.futures as cf
import json, math, os, sys, time, urllib.parse, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Overpass 主站有几个前端域名,限流各自独立;再加几个第三方镜像轮着用
EPS = [
    "https://overpass-api.de/api/interpreter",
    "https://z.overpass-api.de/api/interpreter",
    "https://lz4.overpass-api.de/api/interpreter",
    "https://overpass.osm.ch/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.osm.jp/api/interpreter",
]
UA = {"User-Agent": "metro-map-fetch-city/1.0 (data preparation)"}
STEP = 0.06                         # 站点分格边长(度)

from cities import CITIES, osm_dir


_MIRRORS = None
MIRROR_CACHE = ""          # main() 里设成 cache 目录,探活结果缓存一小时
MIRROR_TTL = 3600


def mirrors():
    """探一遍镜像(一个小查询),只留答得快的;都不通才退回原列表.

    有的镜像从某些网络里握手就超时(白等几十秒),留着只会拖慢每个城市.
    结果缓存一小时:每个城市一个新进程,不缓存就每个城市都白探一次.
    """
    global _MIRRORS
    path = os.path.join(MIRROR_CACHE, "mirrors.json") if MIRROR_CACHE else ""
    if _MIRRORS is None and path and os.path.exists(path):
        try:
            got = json.load(open(path, encoding="utf-8"))
            if time.time() - got.get("t", 0) < MIRROR_TTL and got.get("eps"):
                _MIRRORS = got["eps"]
        except Exception:
            pass
    if _MIRRORS is None:
        probe = urllib.parse.urlencode({"data": "[out:json][timeout:20];out count;"}).encode()
        alive = []
        for ep in EPS:
            t0 = time.time()
            try:
                urllib.request.urlopen(urllib.request.Request(ep, data=probe, headers=UA),
                                       timeout=12).read()
                alive.append((time.time() - t0, ep))
            except Exception:
                pass
        alive.sort()
        _MIRRORS = [ep for _, ep in alive] or list(EPS)
        if path:
            try:
                json.dump({"t": time.time(), "eps": _MIRRORS}, open(path, "w", encoding="utf-8"))
            except Exception:
                pass
        print("# Overpass 可用镜像: %s" % ", ".join(e.rsplit("/", 2)[-2] for e in _MIRRORS),
              file=sys.stderr)
    return _MIRRORS


def overpass(query, tries=6, timeout=120):
    """POST 查询;失败(504/SSL/超时)就换镜像 + 退避重试,全挂了才抛."""
    data = urllib.parse.urlencode({"data": query}).encode()
    last = ""
    eps = mirrors()
    for attempt in range(tries):
        try:
            ep = eps[attempt % len(eps)]
            req = urllib.request.Request(ep, data=data, headers=UA)
            with urllib.request.urlopen(req, timeout=timeout) as r:
                txt = r.read().decode("utf-8", "replace")
            if txt.lstrip().startswith("{"):
                return json.loads(txt)["elements"]
            last = txt[:60]
        except Exception as ex:
            last = "%s @ %s" % (str(ex)[:50], ep.rsplit("/", 2)[-2])
        time.sleep(5.0 + 10.0 * attempt)   # 镜像 504 多半是过载,退避给足
    raise RuntimeError(last)


def fetch_routes(bbox):
    """抓 bbox 内全部 route=subway 关系,不带 network 过滤,便于统计漏网网络."""
    q = ('[out:json][timeout:300];'
         'rel["route"="subway"](%f,%f,%f,%f);out geom;' % bbox)
    # 整个 bbox 的 out geom 是大查询,超时给宽一点,重试多几次
    return overpass(q, tries=6, timeout=150)


def split_routes(els, networks):
    """按 network 标签分成命中/未命中两份,未命中的按 network 计数."""
    if not networks:                # 没指定 network:bbox 内全部 route=subway 都要
        return list(els), collections.Counter((e.get("tags") or {}).get("network")
                                              for e in els)
    want = set(networks)
    kept = [e for e in els if (e.get("tags") or {}).get("network") in want]
    others = collections.Counter(
        (e.get("tags") or {}).get("network") for e in els
        if (e.get("tags") or {}).get("network") not in want)
    return kept, others


def cells(routes):
    seen = set()
    for e in routes:
        for m in e.get("members", []):
            if m["type"] == "node" and m.get("role") == "stop":
                seen.add((math.floor(m["lat"] / STEP), math.floor(m["lon"] / STEP)))
            for p in m.get("geometry") or []:
                seen.add((math.floor(p["lat"] / STEP), math.floor(p["lon"] / STEP)))
    return sorted(seen)


STATION_TAGS = [
    'node["station"="subway"]',
    'node["railway"="station"]["subway"="yes"]',
    'node["public_transport"="station"]["subway"="yes"]',
]


def fetch_nodes_by_id(ids, chunk=400):
    """按节点 id 取站点(小查询,走索引,比按 bbox 全城查稳得多).

    id 就是各条线 stop 成员的节点 —— 正是要对名字的那批节点.
    """
    out = {}
    ids = sorted({int(i) for i in ids})
    for i in range(0, len(ids), chunk):
        part = ids[i:i + chunk]
        q = ('[out:json][timeout:60];node(id:%s);out body;'
             % ",".join(str(x) for x in part))
        try:
            els = overpass(q, tries=3, timeout=60)
        except Exception as ex:
            print("按 id 取站点失败(%s),这一批跳过" % str(ex)[:40], file=sys.stderr)
            continue
        for nd in els:
            if nd.get("type") == "node":
                out[nd["id"]] = nd
    return out


def stop_ids(routes):
    """各条线 stop 成员里的节点 id"""
    out = []
    for r in routes:
        for m in r.get("members", []):
            # out geom 的成员用 ref 存节点 id(旧格式才是 id)
            nid = m.get("ref", m.get("id"))
            if m.get("type") == "node" and m.get("role") == "stop" and nid is not None:
                out.append(nid)
    return sorted(set(out))


def fetch_stations_fast(bbox, path, cache, slug, routes):
    """先整城一次性查站点节点(比按 0.06 度分格快几十倍),
    哪种标法整城查不动,就只把那种标法退回按格扫"""
    s, w, n, e = bbox
    nodes = {}
    fallback = []
    # 站点只是拿来对名字的,不该为它反复等:整城查一次、超时短一点,
    # 三种标法加起来最多 6 分钟,剩下的交给按格扫(而且只在一条都没查到时才扫)
    # 整城查只试一次、超时短一点:大 bbox 的整套查询在镜像上很容易超时,
    # 而且免费镜像按 IP 限流,串行更稳.失败就走按格扫(小查询反而可靠)
    #
    # 按标法依次试,第一种拿到站点就停:欧美城市 station=subway 覆盖得很全,
    # 多问那两种只是补充,不值得在 504 上多耗几分钟
    for tag in STATION_TAGS:
        if nodes:
            print("%s: 已经拿到 %d 个站点,剩下的标法不问" % (slug, len(nodes)),
                  file=sys.stderr)
            break
        try:
            els = overpass('[out:json][timeout=60];%s(%.4f,%.4f,%.4f,%.4f);out body;'
                           % ((tag, s, w, n, e)), tries=1, timeout=60)
        except Exception as ex:
            print("%s: 整城查 %s 失败(%s),这一种退回按格扫" % (slug, tag, str(ex)[:40]),
                  file=sys.stderr)
            fallback.append(tag)
            continue
        for nd in els:
            if nd.get("type") == "node":
                nodes[nd["id"]] = nd
    if fallback and nodes:
        print("%s: 整城查已拿到 %d 个站点,跳过按格扫(%d 种标法没整城查动)"
              % (slug, len(nodes), len(fallback)), file=sys.stderr)
    ncell = nfail = 0
    if fallback and not nodes:
        # 一个站点都没查到才值得按格扫(整城查成功时,那几种补充标法不值得花几分钟)
        cell_nodes, ncell, nfail = fetch_stations(routes, cache, slug, path, tags=fallback,
                                                  budget_s=180)
        nodes.update(cell_nodes)
    json.dump({"version": 0.6, "elements": list(nodes.values())},
              open(path, "w", encoding="utf-8"), ensure_ascii=False)
    return nodes, ncell, nfail

def fetch_stations(routes, cache, slug, path, tags=None, budget_s=None):
    """按格扫站点.整城查不动时才走这条路,所以给个时间预算:
    超了就不再往下扫(这些标法是补充,扫几十分钟不值)"""
    tags = tags or STATION_TAGS
    deadline = time.monotonic() + budget_s if budget_s else None

    def one(cell):
        ci, cj = cell
        key = os.path.join(cache, "%s_%d_%d.json" % (slug, ci, cj))
        if os.path.exists(key):
            return
        bbox = (ci * STEP, cj * STEP, (ci + 1) * STEP, (cj + 1) * STEP)
        # 车站的三种常见标法都要: 只标 station=subway 的、railway=station+subway=yes 的、
        # public_transport=station+subway=yes 的
        box = "%.4f,%.4f,%.4f,%.4f" % bbox
        body = "".join(t + "(%s);" % box for t in tags)
        els = overpass('[out:json][timeout:60];(' + body + ');out body;', timeout=60)
        json.dump(els, open(key, "w", encoding="utf-8"), ensure_ascii=False)

    todo = cells(routes)
    fails = []
    with cf.ThreadPoolExecutor(max_workers=2) as ex:
        futs = {ex.submit(one, c): c for c in todo}
        for i, f in enumerate(cf.as_completed(futs), 1):
            try:
                f.result()
            except Exception as ex2:
                fails.append(futs[f])
                print("失败 %s: %s" % (futs[f], ex2), file=sys.stderr)
            if i % 10 == 0:
                print("进度 %d/%d" % (i, len(todo)), file=sys.stderr, flush=True)
            if deadline and time.monotonic() > deadline:
                for f2 in futs:
                    f2.cancel()
                print("按格扫超时(已扫 %d/%d 格),剩下的跳过" % (i, len(todo)), file=sys.stderr)
                fails = []
                break
    # 首轮失败的格子补抓
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
        parts = f[:-5].split("_") if f.endswith(".json") else []
        # 只认本城市的格子文件 <slug>_<i>_<j>.json,别把杭州的 v2_i_j.json 或别的文件算进来
        if len(parts) != 3 or parts[0] != slug or not all(p.isdigit() for p in parts[1:]):
            continue
        for e in json.load(open(os.path.join(cache, f), encoding="utf-8")):
            if e.get("type") == "node":
                nodes[e["id"]] = e
    json.dump({"version": 0.6, "elements": list(nodes.values())},
              open(path, "w", encoding="utf-8"), ensure_ascii=False)
    return nodes, len(todo), len(fails)


def line_refs(routes):
    """各关系的线路编号(ref),空的不要,返回去重后的列表."""
    return sorted({(e.get("tags") or {}).get("ref") for e in routes
                   if (e.get("tags") or {}).get("ref")})


def no_stop(routes):
    """没有 role=stop 的 node 成员的关系,正常不该有."""
    out = []
    for e in routes:
        if not any(m["type"] == "node" and m.get("role") == "stop"
                   for m in e.get("members", [])):
            out.append((e.get("tags") or {}).get("ref") or e["id"])
    return out


def run_city(slug, cache, refresh_routes):
    city = CITIES[slug]
    routes_path = os.path.join(osm_dir(slug), "%s_routes.json" % slug)
    stations_path = os.path.join(osm_dir(slug), "%s_stations.json" % slug)
    if os.path.exists(routes_path) and not refresh_routes:
        routes = json.load(open(routes_path, encoding="utf-8"))["elements"]
        others = None
    else:
        els = fetch_routes(city["bbox"])
        routes, others = split_routes(els, city["networks"])
        json.dump({"version": 0.6, "elements": routes},
                  open(routes_path, "w", encoding="utf-8"), ensure_ascii=False)
    if others is not None:
        if city["networks"]:
            print("%s: bbox 内共 %d 条 route=subway 关系, 命中 %d 条"
                  % (slug, len(routes) + sum(others.values()), len(routes)), file=sys.stderr)
            if others:
                print("# 未命中的其它 network", file=sys.stderr)
                for net, n in others.most_common():
                    print("%s,%s,%d" % (slug, net if net else "(无 network)", n))
            else:
                print("# 未命中的其它 network: 无")
        else:
            # 没按 network 筛,others 就是全部关系的网络统计
            print("%s: bbox 内 %d 条 route=subway 关系(都收)" % (slug, len(routes)),
                  file=sys.stderr)
            print("# 涉及的 network", file=sys.stderr)
            for net, n in others.most_common():
                print("%s,%s,%d" % (slug, net if net else "(无 network)", n))
    else:
        print("# network 统计: 走的是缓存(%s),加 --refresh-routes 重抓" % routes_path)

    # 先按 stop 成员的 id 取站点:走的是一次几百个 id 的索引查询,又准又快,
    # 不用把整个 bbox 的 station=subway 节点都拉下来(那套在大城市经常超时)
    ids = stop_ids(routes)
    nodes = fetch_nodes_by_id(ids)
    unnamed = sum(1 for nd in nodes.values()
                  if not ((nd.get("tags") or {}).get("name") or "").strip())
    ncell = nfail = 0
    if ids and (len(nodes) < 0.5 * len(ids) or unnamed > 0.3 * max(1, len(nodes))):
        print("%s: 按 id 拿到 %d/%d 个站点,其中无名 %d,再用 bbox 查补一遍"
              % (slug, len(nodes), len(ids), unnamed), file=sys.stderr)
        more, ncell, nfail = fetch_stations_fast(city["bbox"], stations_path, cache, slug, routes)
        nodes.update(more)
    else:
        json.dump({"version": 0.6, "elements": list(nodes.values())},
                  open(stations_path, "w", encoding="utf-8"), ensure_ascii=False)
    refs = line_refs(routes)
    bad = no_stop(routes)
    print("slug,名称,关系条数,distinct_ref,站点节点数,格子数,失败格子数,缺stop成员的关系")
    print("%s,%s,%d,%d,%d,%d,%d,%d%s"
          % (slug, city["name"], len(routes), len(refs), len(nodes), ncell, nfail,
             len(bad), ("  " + "|".join(map(str, bad))) if bad else ""))
    print("%s routes %d B, stations %d B"
          % (slug, os.path.getsize(routes_path), os.path.getsize(stations_path)))
    return {"slug": slug, "routes": len(routes), "refs": len(refs),
            "nodes": len(nodes), "cells": ncell, "fails": nfail, "bad": bad}


def print_usage():
    print("用法: python3 tools/fetch_city.py --city <slug> [--city <slug> ...]")
    print("      python3 tools/fetch_city.py --all [--cache .cache] [--refresh-routes]")
    print("# 可选城市")
    print("slug,名称,networks,s,w,n,e")
    for slug, c in CITIES.items():
        print("%s,%s,%s,%s" % (slug, c["name"], "|".join(c["networks"]),
                               ",".join("%g" % v for v in c["bbox"])))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--city", action="append", default=[], help="城市 slug,可重复")
    ap.add_argument("--all", action="store_true", help="抓城市表里的所有城市")
    ap.add_argument("--cache", default=os.path.join(ROOT, ".cache"))
    ap.add_argument("--refresh-routes", action="store_true", help="忽略已抓的线路文件,重抓")
    args = ap.parse_args()
    slugs = list(dict.fromkeys(args.city + (list(CITIES) if args.all else [])))
    if not slugs:
        print_usage()
        return 0
    unknown = [s for s in slugs if s not in CITIES]
    if unknown:
        print("未知城市: %s" % ", ".join(unknown), file=sys.stderr)
        print_usage()
        return 1
    os.makedirs(args.cache, exist_ok=True)
    global MIRROR_CACHE
    MIRROR_CACHE = args.cache
    for slug in slugs:
        os.makedirs(osm_dir(slug), exist_ok=True)
        run_city(slug, args.cache, args.refresh_routes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
