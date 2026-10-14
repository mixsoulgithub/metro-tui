"""一条命令建一批城市:抓 OSM -> 生成 dot -> 打印一行统计.

抓取是分格缓存 + 可重跑的,网络不稳时反复跑同一条命令就行(已抓的格子会跳过),
最后会列出哪些城市成功、哪些还缺数据.

用法:
  python3 tools/build_cities.py --city wuhan --city nanjing     指定几座
  python3 tools/build_cities.py --all                           表里所有没 dot 的城市
  python3 tools/build_cities.py --all --refresh                 连已抓的也重抓
"""
import argparse, io, os, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOOLS = os.path.join(ROOT, "tools")
sys.path.insert(0, TOOLS)
from cities import CITIES, dot_path


def run(cmd):
    return subprocess.run(cmd, cwd=ROOT).returncode


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--city", action="append", default=[])
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--refresh", action="store_true", help="已抓过的也重抓")
    ap.add_argument("--cache", default=os.path.join(ROOT, ".cache"))
    ap.add_argument("--done", default="", help="已建好的城市 slug 清单文件(一行一个),跳过它们")
    args = ap.parse_args()

    if args.all:
        # 默认只补还没有 dot 的(有人工站序表的那几座不在这儿,它们走 csv2dot)
        done = set(io.open(args.done, encoding="utf-8").read().split()) if args.done else set()
        todo = [s for s, c in sorted(CITIES.items())
                if not c["csv"] and s not in done
                and (args.refresh or not os.path.exists(dot_path(s)))]
    else:
        todo = args.city
    if not todo:
        print("没有要建的城市(都可能已经建好了,或者你没指定)")
        return 0

    ok, bad = [], []
    for slug in todo:
        if slug not in CITIES:
            print("没有这个城市:", slug, file=sys.stderr)
            bad.append(slug)
            continue
        print("== %s(%s)" % (slug, CITIES[slug]["name"]), flush=True)
        args_fetch = [sys.executable, os.path.join(TOOLS, "fetch_city.py"), "--city", slug,
                      "--cache", args.cache]
        if args.refresh:
            args_fetch.append("--refresh-routes")
        if run(args_fetch) != 0:
            print("   抓取失败(网络?),下次重跑会接着抓", file=sys.stderr)
            bad.append(slug)
            continue
        if run([sys.executable, os.path.join(TOOLS, "osm2dot.py"), "--city", slug]) != 0:
            bad.append(slug)
            continue
        ok.append(slug)

    print("\n成功:", " ".join(ok) if ok else "无")
    if bad:
        print("还缺:", " ".join(bad), "(重跑这条命令会接着抓)", file=sys.stderr)
    return 0 if not bad else 1


if __name__ == "__main__":
    sys.exit(main())
