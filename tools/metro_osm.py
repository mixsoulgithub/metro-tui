"""OSM 地铁数据的公用零件:坐标换算、站名规范化、拼音表,以及按线路聚类的 stop 图.

tools/md2dot.py(杭州:站序来自手写文档,OSM 补坐标)和 tools/osm2dot.py(直接按 OSM 关系生成)
共用这些;.dot 的写出格式也只有这里一份,两个脚本出来的文件长得一样.
"""
import heapq, json, math, os, re


M_PER_DEG_LAT = 111320.0


CLUSTER_M = 150.0


NAME_M = 300.0


def m_per_deg_lon(lat=30.25):
    return M_PER_DEG_LAT * math.cos(math.radians(lat))


def dist_m(a, b):
    dy = (a[0] - b[0]) * M_PER_DEG_LAT
    dx = (a[1] - b[1]) * m_per_deg_lon()
    return math.hypot(dx, dy)


def local_pinyin_path():
    """本地(不进仓库)的拼音表:用户数据目录里那份,存在就用"""
    base = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return os.path.join(base, "metro", "pinyin_chars.txt")


def load_pinyin(path):
    """字 -> [读音...](去声调、小写).

    仓库里那份只覆盖 docs 下几座城市用到的字;本地城市的名字里还有别的字
    (比如"旗""栏"),所以再合并一份用户数据目录里的表(有就合,没有就只认仓库那份)
    """
    table = {}
    for p in (path, local_pinyin_path()):
        if not p or not os.path.exists(p):
            continue
        for line in open(p, encoding="utf-8"):
            if "\t" in line:
                ch, readings = line.rstrip("\n").split("\t", 1)
                table.setdefault(ch, [r for r in readings.split(",") if r])
    return table


def pinyin_variants(name, table, limit=12):
    """站名的拼音写法.第一个是首选读音(每个字取第一读音),后面是别的读法组合.

    多音字(行/长/大...)都列出来,搜索时命中哪个都算.
    """
    chars = [c for c in name if c in table]
    if not chars:
        return []
    combos = [[]]
    for ch in chars:
        combos = [pre + [r] for pre in combos for r in table[ch]][:limit * 4]
    out = []
    for combo in combos[:limit]:
        s = " ".join(combo)
        if s not in out:
            out.append(s)
    return out


def norm(name):
    """站名规范化.中文站名去掉空白和括号(表里常有 人民广场 (1号线) 这种);
    别的文字保留空格和撇号 —— 伦敦的 King's Cross 不该变成 KingsCross."""
    if any("\u4e00" <= c <= "\u9fff" for c in name):
        return re.sub(r"[\s（）()·、]", "", name)
    return re.sub(r"\s+", " ", name).strip()


class RefData:
    """一条线路(ref)的 stop 图"""

    def __init__(self, colour):
        self.colour = colour
        self.coord = []
        self.name = []
        self.adj = []
        self.seqs = []          # 每条关系的 stop 节点序列(用来补端点站、判断方向)
        self.track = []         # 轨道上的点(way 几何),用来判断某站是不是在这条线上
        self._grid = {}

    def _key(self, p):
        return (round(p[0] / 0.002), round(p[1] / 0.002))

    def node_of(self, p):
        best = None
        k0, k1 = self._key(p)
        for di in (-1, 0, 1):
            for dj in (-1, 0, 1):
                for k in self._grid.get((k0 + di, k1 + dj), []):
                    d = dist_m(p, self.coord[k])
                    if d < CLUSTER_M and (best is None or d < best[0]):
                        best = (d, k)
        if best is not None:
            return best[1]
        self.coord.append(p)
        self.name.append(None)
        self.adj.append([])
        k = len(self.coord) - 1
        self._grid.setdefault(self._key(p), []).append(k)
        return k

    def link(self, a, b):
        if a == b:
            return
        for n, _ in self.adj[a]:
            if n == b:
                return
        d = dist_m(self.coord[a], self.coord[b])
        self.adj[a].append((b, d))
        self.adj[b].append((a, d))

    def nearest(self, p, radius=NAME_M):
        best = None
        for k in range(len(self.coord)):
            d = dist_m(p, self.coord[k])
            if d < radius and (best is None or d < best[0]):
                best = (d, k)
        return best[1] if best else None

    def path(self, a, b):
        """最短路(按米),返回节点序号列表(含首尾)"""
        if a == b:
            return [a]
        dist = {a: 0.0}
        prev = {}
        pq = [(0.0, a)]
        while pq:
            d, u = heapq.heappop(pq)
            if u == b:
                break
            if d > dist.get(u, math.inf):
                continue
            for v, w in self.adj[u]:
                nd = d + w
                if nd < dist.get(v, math.inf):
                    dist[v] = nd
                    prev[v] = u
                    heapq.heappush(pq, (nd, v))
        if b not in dist:
            return [a, b]
        out = [b]
        while out[-1] != a:
            out.append(prev[out[-1]])
        return out[::-1]


def build_refs(routes, named, networks=None):
    refs = {}
    for r in routes:
        t = r.get("tags") or {}
        if networks is not None and t.get("network") not in networks:
            continue
        stops = [(m["lat"], m["lon"]) for m in r["members"]
                 if m["type"] == "node" and m.get("role") == "stop"]
        if len(stops) < 2:
            continue
        rd = refs.get(t["ref"])
        if rd is None:
            rd = refs[t["ref"]] = RefData(t.get("colour", "#888888"))
        ids = [rd.node_of(p) for p in stops]
        rd.seqs.append(ids)
        for m in r["members"]:
            if m["type"] == "way":
                rd.track.extend((p["lat"], p["lon"]) for p in m.get("geometry") or [])
        for a, b in zip(ids, ids[1:]):
            rd.link(a, b)
    for rd in refs.values():
        for k in range(len(rd.coord)):
            best = None
            for name, plist in named.items():
                for lat, lon, _ in plist:
                    d = dist_m(rd.coord[k], (lat, lon))
                    if d < NAME_M and (best is None or d < best[0]):
                        best = (d, name)
            if best:
                rd.name[k] = best[1]
    return refs

def named_stations(nodes):
    """OSM 站点节点 -> {规范名: [(lat, lon, rank)...]}.

    同名节点可能有好几个(不同线路各一个站体),全都留着,匹配时取最近的;
    rank 用来优先认真正的车站节点.
    """
    named = {}
    for n in nodes:
        t = n.get("tags") or {}
        name = t.get("name") or t.get("name:zh")
        if not name:
            continue
        rank = 1 if t.get("railway") == "station" else 0
        named.setdefault(norm(name), []).append((n["lat"], n["lon"], rank))
    return named


def dedup_edges(raw):
    """[(a, b, 色, 线路名)] -> 去掉自环与重复(同一对站同一条线只留一条)"""
    out, seen = [], set()
    for a, b, colour, label in raw:
        if a == b:
            continue
        key = (min(a, b), max(a, b), label)
        if key in seen:
            continue
        seen.add(key)
        out.append((a, b, colour, label))
    return out


def write_dot(path, header, graph, nodes, edges, pinyin):
    """写出 .dot:nodes 是 {站名: (lat, lon)}(有序),edges 是 [(a, b, 色, 线路名)]"""
    x0 = min(c[1] for c in nodes.values())
    y0 = min(c[0] for c in nodes.values())
    with open(path, "w", encoding="utf-8") as f:
        for line in header:
            f.write("// " + line + "\n")
        f.write("graph " + graph + " {\n")
        for nm, (lat, lon) in nodes.items():
            attrs = 'pos="%.0f,%.0f"' % ((lon - x0) * m_per_deg_lon(), (lat - y0) * M_PER_DEG_LAT)
            py = pinyin_variants(nm, pinyin)
            if py:
                attrs += ', pinyin="%s"' % py[0]
                if len(py) > 1:
                    attrs += ', pinyin2="%s"' % "|".join(py[1:])
            f.write('  "%s" [%s];\n' % (nm, attrs))
        for a, b, colour, label in edges:
            f.write('  "%s" -- "%s" [color="%s", line="%s"];\n' % (a, b, colour, label))
        f.write("}\n")
