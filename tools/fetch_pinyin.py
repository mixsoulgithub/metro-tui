"""抓 pypinyin 的拼音字典,筛出站名用得到的字,写成 data/pinyin_chars.txt.

输出格式:每行 "字<TAB>读音1,读音2,..."(读音去声调、ü 记成 u、小写).
之所以自己筛一份:整个字典四万字,站名只用到几百个,摊进仓库没必要.

用法: python3 tools/fetch_pinyin.py
"""
import argparse, io, json, os, re, subprocess, sys, unicodedata, zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WHEEL_CACHE = os.path.join(ROOT, ".cache", "pypinyin")
OUT = os.path.join(ROOT, "data", "pinyin_chars.txt")


def tones_off(s: str) -> str:
    """去掉声调符号,v/ü 统一成 u"""
    s = s.replace("ü", "u").replace("ǖ", "u").replace("ǘ", "u").replace("ǚ", "u").replace("ǜ", "u")
    out = []
    for ch in unicodedata.normalize("NFD", s):
        if unicodedata.combining(ch):
            continue
        out.append(ch)
    return "".join(out).lower()


def get_dict_json() -> dict:
    os.makedirs(WHEEL_CACHE, exist_ok=True)
    whl = [f for f in os.listdir(WHEEL_CACHE) if f.endswith(".whl")]
    if not whl:
        subprocess.run(
            ["pip", "download", "pypinyin", "-d", WHEEL_CACHE],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        whl = [f for f in os.listdir(WHEEL_CACHE) if f.endswith(".whl")]
    with zipfile.ZipFile(os.path.join(WHEEL_CACHE, whl[0])) as z:
        with z.open("pypinyin/pinyin_dict.json") as f:
            raw = json.load(io.TextIOWrapper(f, encoding="utf-8"))
    # 键是码点的十进制字符串
    return {int(k): v for k, v in raw.items()}


def wanted_chars(dirs=None) -> set:
    """这些目录下所有 .md/.dot 里出现过的汉字(docs 是默认;本地城市用 --extra-dir 指)"""
    text = []
    for d in (dirs or [os.path.join(ROOT, "docs")]):
        if not os.path.isdir(d):
            continue
        for name in sorted(os.listdir(d)):
            if name.endswith((".md", ".dot")):
                text.append(open(os.path.join(d, name), encoding="utf-8").read())
    return {c for c in "".join(text) if "\u4e00" <= c <= "\u9fff"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-o", "--out", default=OUT)
    ap.add_argument("--extra-dir", action="append", default=[],
                    help="额外扫的目录(可重复,例如本地城市数据目录)")
    args = ap.parse_args()
    table = get_dict_json()
    chars = sorted(wanted_chars([os.path.join(ROOT, "docs")] + args.extra_dir))
    lines = []
    miss = []
    for ch in chars:
        raw = table.get(ord(ch))
        if not raw:
            miss.append(ch)
            continue
        readings = []
        for r in raw.split(","):
            r = tones_off(r.strip())
            if r and r not in readings:
                readings.append(r)
        lines.append("%s\t%s" % (ch, ",".join(readings)))
    open(args.out, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print("写出 %s:%d 个字" % (args.out, len(lines)))
    if miss:
        print("字典里没有的:%s" % " ".join(miss))


if __name__ == "__main__":
    main()
