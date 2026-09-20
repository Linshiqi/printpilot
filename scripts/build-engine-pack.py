#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Build the CAD engine pack that ships inside the installer.

    python scripts/build-engine-pack.py [--platform windows-x86_64|macos-aarch64|macos-x86_64] [--out src-tauri/resources/cad-engine]

What it produces (in --out):
    cad-engine.tar.zst   a relocatable CPython + build123d, one archive
    cad-engine.json      {engine_version, platform, sha256, bytes, unpacked_bytes, files, versions}

Why one archive instead of thousands of loose files in the installer: the installer stays fast
(one file instead of ~15k), and on macOS the Python binaries never sit inside the signed .app bundle —
the app unpacks them into its own data directory on first use (src-tauri/src/engine_pack.rs).

Everything is pinned so two builds give the same engine:
  - the interpreter: a python-build-standalone release, verified by SHA-256;
  - the packages: scripts/engine/constraints.txt (a freeze of the environment the test-suite passed on).

Needs: Python 3.9+ on the build machine and `pip install zstandard`. Nothing here runs on the user's machine.
(Comments are in English on purpose: this file is also read by CI logs on three operating systems.)
"""
import argparse
import hashlib
import io
import json
import os
import platform as host_platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

# Bump when the *contents* of the pack change (new build123d, new Python, different pruning).
# The app reinstalls the engine when the bundled version differs from the unpacked one.
ENGINE_VERSION = "b123d-0.12.0-py312-1"

PBS_TAG = "20260901"
PBS_PYTHON = "3.12.14"
PBS = {
    "windows-x86_64": ("x86_64-pc-windows-msvc", "7c45c9622400d578709a9b2cddbe8124cc21d382409d9f13406d706d28e31b14"),
    "macos-aarch64": ("aarch64-apple-darwin", "81a359f1cfadd4da11766534c5913791cea55f26e1bb902cacd2a531bb1e4b2b"),
    "macos-x86_64": ("x86_64-apple-darwin", "65b195c9cedc1fef6767f044f9822069adbd1bd9204d424ece4628776fdc04bb"),
}

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONSTRAINTS = os.path.join(ROOT, "scripts", "engine", "constraints.txt")
RUNNER = os.path.join(ROOT, "crates", "pp-cad", "py", "runner.py")

# Deliberately NOT pruned: the test-suites inside numpy / scipy / sklearn. It looks like free space, but
# `numpy.testing` imports `numpy._core.tests`, and scipy pulls `numpy.testing` in through `from numpy import *`
# (the first attempt at this script broke `import build123d` exactly that way). A lazy import that only fails
# on a user's machine is not worth ~15 MB.
# pip is not needed at run time; removing it also means there is no installer inside the engine.
PRUNE_PACKAGES = ("pip",)

SMOKE = """from build123d import *

# ---- PARAMS ----
width = 40.0  # mm | width | [10, 100]

# ---- FEATURE: body ----
body = Box(width, 20, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))
body = fillet(body.edges().filter_by(Axis.Z), radius=3)

# ---- FEATURE: hole ----
body = body - Cylinder(4, 30)

# ---- RESULT ----
result = body
"""


def log(msg):
    print(f"[engine-pack] {msg}", flush=True)


def detect_platform():
    system, machine = host_platform.system(), host_platform.machine().lower()
    if system == "Windows":
        return "windows-x86_64"
    if system == "Darwin":
        return "macos-aarch64" if machine in ("arm64", "aarch64") else "macos-x86_64"
    raise SystemExit(f"unsupported build host: {system} {machine}")


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url, dest):
    log(f"download {url}")
    for attempt in range(1, 4):
        try:
            with urllib.request.urlopen(url, timeout=120) as r, open(dest, "wb") as f:
                shutil.copyfileobj(r, f, 1 << 20)
            return
        except Exception as e:  # noqa: BLE001 - CI networks are flaky; retry a couple of times
            log(f"  attempt {attempt} failed: {e}")
            time.sleep(5 * attempt)
    raise SystemExit(f"could not download {url}")


def python_exe(py_root, plat):
    return os.path.join(py_root, "python.exe") if plat.startswith("windows") else os.path.join(py_root, "bin", "python3")


def run(cmd, **kw):
    log("$ " + " ".join(str(c) for c in cmd))
    subprocess.run(cmd, check=True, **kw)


def fetch_interpreter(plat, work):
    triple, want = PBS[plat]
    name = f"cpython-{PBS_PYTHON}+{PBS_TAG}-{triple}-install_only_stripped.tar.gz"
    url = f"https://github.com/astral-sh/python-build-standalone/releases/download/{PBS_TAG}/{name.replace('+', '%2B')}"
    archive = os.path.join(work, name)
    download(url, archive)
    got = sha256_of(archive)
    if got != want:
        raise SystemExit(f"checksum mismatch for {name}: {got} != {want}")
    with tarfile.open(archive, "r:gz") as tar:
        tar.extractall(work)  # noqa: S202 - pinned, checksum-verified archive
    os.remove(archive)
    return os.path.join(work, "python")  # the archive's top-level directory


def install_packages(py, plat):
    env = dict(os.environ, PIP_DISABLE_PIP_VERSION_CHECK="1", PIP_NO_INPUT="1", PYTHONNOUSERSITE="1")
    run([py, "-I", "-m", "pip", "install", "--no-cache-dir", "--prefer-binary", "--no-warn-script-location",
         "-c", CONSTRAINTS, "build123d==0.12.0"], env=env)


def site_packages(py_root, plat):
    if plat.startswith("windows"):
        return os.path.join(py_root, "Lib", "site-packages")
    return os.path.join(py_root, "lib", f"python{PBS_PYTHON.rsplit('.', 1)[0]}", "site-packages")


def prune(py_root, plat):
    sp = site_packages(py_root, plat)
    for name in os.listdir(sp):
        low = name.lower()
        if any(low == p or low.startswith(p + "-") for p in PRUNE_PACKAGES):
            shutil.rmtree(os.path.join(sp, name), ignore_errors=True)
    # Console-script launchers (pip.exe, ipython, f2py ...) embed the absolute path of the build machine.
    scripts = os.path.join(py_root, "Scripts") if plat.startswith("windows") else None
    if scripts and os.path.isdir(scripts):
        shutil.rmtree(scripts, ignore_errors=True)
    if not plat.startswith("windows"):
        bindir = os.path.join(py_root, "bin")
        for name in os.listdir(bindir):
            if not name.startswith("python"):
                os.remove(os.path.join(bindir, name))
    for dirpath, dirnames, _ in os.walk(py_root):
        for d in list(dirnames):
            if d == "__pycache__":
                shutil.rmtree(os.path.join(dirpath, d), ignore_errors=True)
                dirnames.remove(d)
    log("pruned pip, launcher scripts, __pycache__")


def precompile(py, py_root):
    # The app starts the engine with -B (never writes .pyc). Shipping bytecode that stays valid no matter
    # what mtimes the files get on the user's disk ("unchecked-hash") saves seconds on every cold start.
    subprocess.run([py, "-I", "-m", "compileall", "-q", "-j", "0", "--invalidation-mode", "unchecked-hash", py_root],
                   check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def smoke_test(py, work):
    """Run the real executor exactly the way the app does: isolated mode, no bytecode writes, empty environment."""
    job_dir = os.path.join(work, "smoke")
    os.makedirs(job_dir)
    job = os.path.join(job_dir, "job.json")
    out = os.path.join(job_dir, "out")
    with open(job, "w", encoding="utf-8") as f:
        json.dump({"code": SMOKE, "out_dir": out, "exports": ["step", "stl", "3mf"]}, f)
    env = {k: os.environ[k] for k in ("SystemRoot", "windir", "SystemDrive") if k in os.environ}
    for k in ("TEMP", "TMP", "TMPDIR", "USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"):
        env[k] = job_dir
    started = time.time()
    subprocess.run([py, "-I", "-B", "-X", "utf8", RUNNER, job], check=False, cwd=job_dir, env=env)
    with open(os.path.join(out, "result.json"), encoding="utf-8") as f:
        result = json.load(f)
    if not result.get("ok"):
        raise SystemExit(f"smoke test failed: {json.dumps(result, ensure_ascii=False)[:2000]}")
    m = result["metrics"]
    size = [round(v, 3) for v in m["size"]]
    if size != [40.0, 20.0, 10.0] or m["solids"] != 1 or not m["is_valid"]:
        raise SystemExit(f"smoke test produced the wrong part: {m}")
    for kind, name in result["files"].items():
        if os.path.getsize(os.path.join(out, name)) < 500:
            raise SystemExit(f"smoke test: {kind} export is empty")
    log(f"smoke test ok in {time.time() - started:.1f}s: {size} mm, {m['faces']} faces, exports {sorted(result['files'])}")
    shutil.rmtree(job_dir, ignore_errors=True)

    # The real gate: the executor's own test-suite and every API the code-generation prompt teaches,
    # run by the interpreter we are about to ship (crates/pp-cad/py, ~40 tests, a few seconds).
    tests_dir = os.path.dirname(RUNNER)
    run([py, "-I", "-B", "-X", "utf8", "-m", "unittest", "discover", "-s", tests_dir, "-t", tests_dir], cwd=tests_dir)


def versions(py):
    code = ("import sys, json, build123d, OCP, numpy, scipy; "
            "print(json.dumps({'python': sys.version.split()[0], 'build123d': build123d.__version__, "
            "'ocp': getattr(OCP, '__version__', ''), 'numpy': numpy.__version__, 'scipy': scipy.__version__}))")
    out = subprocess.run([py, "-I", "-c", code], check=True, capture_output=True, text=True).stdout
    return json.loads(out.strip().splitlines()[-1])


def pack(py_root, out_dir, plat, vers):
    import zstandard

    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(py_root, "engine.json"), "w", encoding="utf-8") as f:
        json.dump({"engine_version": ENGINE_VERSION, "platform": plat, "versions": vers}, f, indent=1)

    archive = os.path.join(out_dir, "cad-engine.tar.zst")
    files, unpacked = 0, 0
    cctx = zstandard.ZstdCompressor(level=19, threads=-1)
    with open(archive, "wb") as raw, cctx.stream_writer(raw) as z, tarfile.open(fileobj=z, mode="w|", format=tarfile.PAX_FORMAT) as tar:
        for dirpath, dirnames, filenames in os.walk(py_root):
            dirnames.sort()
            # os.walk does not descend into symlinked directories; record them as links or they vanish
            linked_dirs = [d for d in dirnames if os.path.islink(os.path.join(dirpath, d))]
            for name in sorted(filenames + linked_dirs):
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, os.path.dirname(py_root)).replace(os.sep, "/")  # python/...
                info = tar.gettarinfo(full, arcname=rel)
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = 1_700_000_000  # fixed: same input -> same archive
                if info.islnk():
                    # store hard links as plain files: simpler to validate on the unpacking side
                    info.type = tarfile.REGTYPE
                    info.linkname = ""
                    info.size = os.path.getsize(full)
                if info.issym():
                    tar.addfile(info)
                else:
                    with open(full, "rb") as f:
                        tar.addfile(info, f)
                    unpacked += info.size
                files += 1
    manifest = {
        "engine_version": ENGINE_VERSION,
        "platform": plat,
        "sha256": sha256_of(archive),
        "bytes": os.path.getsize(archive),
        "unpacked_bytes": unpacked,
        "files": files,
        "versions": vers,
    }
    with open(os.path.join(out_dir, "cad-engine.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=1)
    log(f"packed {files} files, {unpacked / 1e6:.0f} MB -> {manifest['bytes'] / 1e6:.0f} MB  ({archive})")
    return manifest


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--platform", choices=sorted(PBS), default=None)
    ap.add_argument("--out", default=os.path.join(ROOT, "src-tauri", "resources", "cad-engine"))
    ap.add_argument("--keep", action="store_true", help="keep the work directory (debugging)")
    args = ap.parse_args()
    plat = args.platform or detect_platform()
    try:
        import zstandard  # noqa: F401
    except ImportError:
        raise SystemExit("this script needs the `zstandard` package:  python -m pip install zstandard")

    work = tempfile.mkdtemp(prefix="pp-engine-")
    log(f"platform {plat} · engine {ENGINE_VERSION} · work dir {work}")
    try:
        py_root = fetch_interpreter(plat, work)
        py = python_exe(py_root, plat)
        install_packages(py, plat)
        vers = versions(py)
        log(f"versions {vers}")
        prune(py_root, plat)
        smoke_test(py, work)       # after pruning: proves we did not delete anything the engine needs
        precompile(py, py_root)
        manifest = pack(py_root, os.path.abspath(args.out), plat, vers)
        print(json.dumps(manifest, indent=1))
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    # Windows consoles default to a legacy code page; pip's progress output can contain anything.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()
