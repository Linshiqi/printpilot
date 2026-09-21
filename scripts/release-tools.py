# -*- coding: utf-8 -*-
"""发版流水线里和「在线升级」有关的两件小事(docs/adr/0010-online-update.md)。纯标准库。

  python scripts/release-tools.py engine-urls  <manifest.json> <platform> [--base URL ...]
      往引擎包的清单里写上「这个包可以从哪下载」。精简升级包不带引擎,应用要用的时候按这几个地址去下。

  python scripts/release-tools.py latest-json <version> <notes.md|-> <download-base> <sig-dir> <out.json>
      用各平台升级包的签名(*.sig)拼出更新清单 latest.json(tauri-plugin-updater 的静态 JSON 格式)。
      缺哪个平台的签名,就不写哪个平台——那个平台的用户查不到这次更新,但不会装到一个坏包。
"""
import datetime
import json
import os
import sys

REPO = "Linshiqi/printpilot"
# 升级包的文件名 → 更新清单里的平台键
UPDATE_ASSETS = {
    "windows-x86_64": "PrintPilot_{version}_windows_x64-update.exe",
    "darwin-aarch64": "PrintPilot_{version}_macos_apple-silicon.app.tar.gz",
    "darwin-x86_64": "PrintPilot_{version}_macos_intel.app.tar.gz",
}


def engine_asset(platform):
    return f"cad-engine_{platform}.tar.zst"


def engine_urls(manifest_path, platform, bases):
    with open(manifest_path, encoding="utf-8") as f:
        manifest = json.load(f)
    version = manifest["engine_version"]
    if not bases:
        bases = [
            f"https://dl.zotrus.com/printpilot/engine/{version}",
            f"https://github.com/{REPO}/releases/download/engine-{version}",
        ]
    manifest["download"] = [f"{b.rstrip('/')}/{engine_asset(platform)}" for b in bases]
    with open(manifest_path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=1)
    print(json.dumps(manifest["download"], indent=1))
    return version


def latest_json(version, notes_path, base, sig_dir, out_path):
    notes = ""
    if notes_path != "-" and os.path.isfile(notes_path):
        with open(notes_path, encoding="utf-8") as f:
            notes = f.read().strip()
    platforms = {}
    for key, pattern in UPDATE_ASSETS.items():
        name = pattern.format(version=version)
        sig = os.path.join(sig_dir, name + ".sig")
        if not os.path.isfile(sig):
            print(f"(no signature for {key}: {name}.sig - skipped)")
            continue
        with open(sig, encoding="utf-8") as f:
            platforms[key] = {"signature": f.read().strip(), "url": f"{base.rstrip('/')}/{name}"}
    if not platforms:
        print("no signatures at all: not writing latest.json")
        return False
    doc = {
        "version": version,
        "notes": notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1)
    print(f"{out_path}: {', '.join(platforms)}")
    return True


def main(argv):
    if len(argv) >= 4 and argv[1] == "engine-urls":
        bases = [argv[i + 1] for i, a in enumerate(argv) if a == "--base" and i + 1 < len(argv)]
        print(engine_urls(argv[2], argv[3], bases))
        return 0
    if len(argv) == 7 and argv[1] == "latest-json":
        return 0 if latest_json(*argv[2:7]) else 3
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
