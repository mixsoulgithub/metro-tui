"""城市表:加一座城市只改这里.

字段:
  name      中文名
  networks  OSM 里这条地铁线的 network 标签(可能有多个);留空 = bbox 内所有
            route=subway 关系都收(海外城市不一定按运营方打标签,这样更省事)
  bbox      OSM 查询范围 (s, w, n, e),给得比市区略宽没关系 —— 只按 network 标签筛关系
  csv       站序表文件名(写了的城市直接拿它出图),没有就留空
  dot       线路图文件名

路径由 home()/csv_path()/dot_path() 决定:线路图默认放 docs/,也可以另指一个目录.

"""
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

CURATED = ("hangzhou", "shanghai", "chengdu", "shenzhen")

# 被 tools/fetch_city.py(抓 OSM)、tools/osm2dot.py(OSM 关系 -> dot)、
# tools/csv2dot.py(站序 csv -> dot)共用.

CITIES = {
    "hangzhou": dict(name="杭州", networks=["杭州地铁"],
                     bbox=(29.50, 119.50, 30.90, 121.00),
                     csv="hangzhou-subway.md", dot="hangzhou-subway.dot", md2dot=True),
    "shanghai": dict(name="上海", networks=["上海地铁"],
                     bbox=(30.55, 120.75, 31.62, 122.05),
                     csv="shanghai-subway.md", dot="shanghai-subway.dot"),
    "chengdu": dict(name="成都", networks=["成都地铁"],
                    bbox=(30.20, 103.50, 31.10, 104.60),
                    csv="chengdu.md", dot="chengdu-subway.dot"),
    "shenzhen": dict(name="深圳", networks=["深圳地铁"],
                     bbox=(22.30, 113.65, 22.90, 114.75),
                     csv="shenzhen.md", dot="shenzhen-subway.dot"),
}

# 可选的城市表扩展:用户数据目录里再放一份 cities_local.py 也能并进来
EXTRA = os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "metro")
sys.path.insert(0, EXTRA)
try:
    from cities_local import LOCAL

    CITIES.update(LOCAL)
except ImportError:
    pass

for _slug, _c in CITIES.items():
    _c.setdefault("csv", None)
    _c.setdefault("dot", _slug + "-subway.dot")
    _c.setdefault("pinyin", _slug)


def home(slug):
    """这座城市的线路图、站序表放哪个目录"""
    return "docs" if slug in CURATED else os.path.join(EXTRA, "cities")


def osm_dir(slug):
    """这座城市的 OSM 原始数据(routes/stations json)放哪个目录"""
    if slug in CURATED:
        return os.path.join(ROOT, "data", "osm")
    return os.path.join(EXTRA, "osm")


def csv_path(slug):
    c = CITIES[slug]
    return os.path.join(home(slug), c["csv"] or (slug + ".md"))


def dot_path(slug):
    return os.path.join(home(slug), CITIES[slug]["dot"])

# 有站序表的(直接出图)和只能靠 OSM 的
WITH_CSV = [s for s, c in CITIES.items() if c["csv"]]
OSM_ONLY = [s for s, c in CITIES.items() if not c["csv"]]
