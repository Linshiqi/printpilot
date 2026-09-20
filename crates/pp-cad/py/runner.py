# -*- coding: utf-8 -*-
"""PrintPilot 的 build123d 执行器:在子进程里跑一段(模型写的)建模代码,导出文件并汇报指标。

用法:  python -I -B runner.py <job.json>      # 跑一个任务就退出
       python -I -B runner.py --serve         # 常驻:stdin 每行一个任务,见 serve()

job.json:
  { "code": "...", "out_dir": "...", "exports": ["step", "stl", "3mf"],
    "stl_tolerance": 0.01, "stl_angular_tolerance": 0.1 }

结果写到 <out_dir>/result.json(无论成败都会写;退出码 0 = 成功,1 = 失败):
  成功 { "ok": true,  "files": {...}, "metrics": {...}, "stdout": "...", "elapsed_ms": 123 }
  失败 { "ok": false, "stage": "validate|exec|result|export", "error_type": "...",
         "message": "...", "line": 12, "traceback": "...", "stdout": "..." }

安全(docs/adr/0003):这里执行的是不可信代码。
  1. 运行前做 AST 白名单检查:只许导入 build123d / math;禁危险内建;禁双下划线属性;
     禁 build123d 自带的文件读写(export_* / import_* / Mesher ...)。
  2. 执行命名空间只放「build123d 的公开名字 - 文件读写类名字」+ math + 少量安全内建。
  3. 导出由本执行器统一做,生成的代码自己碰不到文件系统。
  进程级的隔离(超时、工作目录、环境变量)由 Rust 侧的父进程负责。
"""
import ast
import contextlib
import io
import json
import math
import os
import sys
import time
import traceback
import types

ALLOWED_IMPORTS = {"build123d", "math"}

FORBIDDEN_NAMES = {
    "eval", "exec", "compile", "open", "__import__", "input", "breakpoint",
    "globals", "locals", "vars", "getattr", "setattr", "delattr", "dir",
    "help", "exit", "quit", "memoryview", "type", "object", "super", "classmethod",
    "staticmethod", "property", "__builtins__", "__loader__", "__spec__",
}

# build123d 里会读写文件 / 访问外部资源的名字:不进命名空间,也不许当属性调用
IO_NAME_PREFIXES = ("export", "import_", "read_", "write", "save", "load")
IO_NAMES = {"Mesher", "ExportSVG", "ExportDXF", "open", "system", "popen", "to_file", "from_file"}

SAFE_BUILTINS = [
    "abs", "all", "any", "bool", "dict", "divmod", "enumerate", "filter", "float", "int",
    "isinstance", "len", "list", "map", "max", "min", "pow", "print", "range", "repr",
    "reversed", "round", "set", "sorted", "str", "sum", "tuple", "zip",
    "True", "False", "None", "ValueError", "TypeError", "Exception", "RuntimeError",
    "ZeroDivisionError", "IndexError", "KeyError",
]

USER_FILE = "<model>"


class Rejected(Exception):
    def __init__(self, message, line=None):
        super().__init__(message)
        self.line = line


def is_io_name(name):
    return name in IO_NAMES or name.startswith(IO_NAME_PREFIXES)


def validate(tree):
    """AST 白名单。不合规就抛 Rejected(消息会原样回给模型,让它改)。"""
    for node in ast.walk(tree):
        line = getattr(node, "lineno", None)
        if isinstance(node, ast.Import):
            for alias in node.names:
                # 只认一模一样的 `import math`:`import math.os` 这种带点的写法也拒掉
                if alias.name != "math":
                    raise Rejected(
                        f"import {alias.name} is not allowed; use `from build123d import *` and `import math` only", line)
        elif isinstance(node, ast.ImportFrom):
            root = (node.module or "").split(".")[0]
            if node.level or root not in ALLOWED_IMPORTS:
                raise Rejected(f"from {node.module} import ... is not allowed", line)
            for alias in node.names:
                if alias.name != "*" and is_io_name(alias.name):
                    raise Rejected(f"{alias.name} does file I/O and is not allowed; exporting is done by the host", line)
        elif isinstance(node, ast.Name):
            if node.id in FORBIDDEN_NAMES or node.id.startswith("__"):
                raise Rejected(f"use of `{node.id}` is not allowed", line)
            if is_io_name(node.id):
                raise Rejected(f"`{node.id}` does file I/O and is not allowed; exporting is done by the host", line)
        elif isinstance(node, ast.Attribute):
            if node.attr.startswith("__"):
                raise Rejected(f"dunder attribute `{node.attr}` is not allowed", line)
            if is_io_name(node.attr):
                raise Rejected(f"`.{node.attr}` does file I/O and is not allowed; exporting is done by the host", line)
            # 常驻模式下同一个进程会跑很多段代码:`Box.foo = …` 这类对共享对象的改动会留到下一个任务里。
            # 建模代码用不着给属性赋值,整个禁掉。
            if isinstance(node.ctx, (ast.Store, ast.Del)):
                raise Rejected(f"assigning to attributes (`.{node.attr} = ...`) is not allowed; use local variables", line)
        elif isinstance(node, (ast.AsyncFunctionDef, ast.AsyncFor, ast.AsyncWith, ast.Await, ast.Global, ast.Nonlocal)):
            raise Rejected(f"{type(node).__name__} is not allowed", line)


def strip_imports(tree):
    """白名单内的 import 已经由命名空间提供,执行前摘掉(这样 `from build123d import *` 带不进被禁的名字)。"""
    class Strip(ast.NodeTransformer):
        def visit_Import(self, node):
            return None

        def visit_ImportFrom(self, node):
            return None

    tree = Strip().visit(tree)
    ast.fix_missing_locations(tree)
    return tree


def build_namespace():
    import build123d

    names = getattr(build123d, "__all__", None) or [n for n in dir(build123d) if not n.startswith("_")]
    ns = {n: getattr(build123d, n) for n in names if not is_io_name(n) and hasattr(build123d, n)}
    # 每个任务一份 math 的替身:就算有办法改它,也带不到下一个任务里
    shim = types.ModuleType("math")
    shim.__dict__.update({k: v for k, v in vars(math).items() if not k.startswith("_")})
    ns["math"] = shim
    import builtins
    ns["__builtins__"] = {n: getattr(builtins, n) for n in SAFE_BUILTINS if hasattr(builtins, n)}
    # class 语句需要它;单独放行,不把整个 builtins 给出去
    ns["__builtins__"]["__build_class__"] = builtins.__build_class__
    ns["__name__"] = "__model__"
    return ns


def user_line(tb):
    """traceback 里最后一帧属于用户代码的行号。"""
    line = None
    for frame in traceback.extract_tb(tb):
        if frame.filename == USER_FILE:
            line = frame.lineno
    return line


def user_traceback(exc, source=""):
    """只保留用户代码的帧 + 异常本身:库内部的几十帧对修代码没有帮助,还浪费 token。
    用户代码不是磁盘上的文件,traceback 查不到源码行——从 `source` 里自己取:模型要看到出错的那一行才好修。"""
    src = source.splitlines()
    frames = [f for f in traceback.extract_tb(exc.__traceback__) if f.filename == USER_FILE]
    lines = []
    for f in frames:
        text = f.line or (src[f.lineno - 1].strip() if 0 < (f.lineno or 0) <= len(src) else "")
        lines.append(f"  line {f.lineno}: {text}".rstrip())
    lines.append(f"{type(exc).__name__}: {exc}")
    return "\n".join(lines)[-4000:]


def as_shape(result):
    """`result` 可以是 Part / Solid / Compound,也可以是 BuildPart 构建器(取它的 .part)。"""
    import build123d as bd

    if isinstance(result, bd.BuildPart):
        result = result.part
    if isinstance(result, (list, tuple)):
        result = bd.Compound(children=list(result))
    if not isinstance(result, bd.Shape):
        raise Rejected(f"`result` must be a build123d Part/Solid/Compound, got {type(result).__name__}")
    return result


def measure(shape):
    bb = shape.bounding_box()
    solids = shape.solids()
    com = None
    try:
        c = shape.center()
        com = [c.X, c.Y, c.Z]
    except Exception:
        pass
    return {
        "bbox_min": [bb.min.X, bb.min.Y, bb.min.Z],
        "bbox_max": [bb.max.X, bb.max.Y, bb.max.Z],
        "size": [bb.size.X, bb.size.Y, bb.size.Z],
        "volume_mm3": float(sum(s.volume for s in solids)),
        "area_mm2": float(shape.area),
        "solids": len(solids),
        "faces": len(shape.faces()),
        "edges": len(shape.edges()),
        "is_valid": bool(shape.is_valid()) if callable(getattr(shape, "is_valid", None)) else bool(shape.is_valid),
        "center": com,
    }


def export(shape, out_dir, kinds, tol, ang):
    import build123d as bd

    files = {}
    if "step" in kinds:
        bd.export_step(shape, os.path.join(out_dir, "model.step"))
        files["step"] = "model.step"
    if "stl" in kinds:
        bd.export_stl(shape, os.path.join(out_dir, "model.stl"), tolerance=tol, angular_tolerance=ang)
        files["stl"] = "model.stl"
    if "3mf" in kinds:
        mesher = bd.Mesher()
        mesher.add_shape(shape, linear_deflection=tol, angular_deflection=ang)
        mesher.write(os.path.join(out_dir, "model.3mf"))
        files["3mf"] = "model.3mf"
    return files


def run(job):
    started = time.time()
    out_dir = job["out_dir"]
    os.makedirs(out_dir, exist_ok=True)
    stdout = io.StringIO()

    def fail(stage, exc, line=None, tb=None):
        return {
            "ok": False, "stage": stage, "error_type": type(exc).__name__, "message": str(exc)[:2000],
            "line": line, "traceback": tb or "", "stdout": stdout.getvalue()[-4000:],
            "elapsed_ms": int((time.time() - started) * 1000),
        }

    try:
        tree = ast.parse(job["code"], filename=USER_FILE)
        validate(tree)
        code = compile(strip_imports(tree), USER_FILE, "exec")
    except SyntaxError as e:
        return fail("validate", e, line=e.lineno)
    except Rejected as e:
        return fail("validate", e, line=e.line)

    ns = build_namespace()
    try:
        with contextlib.redirect_stdout(stdout):
            exec(code, ns)  # noqa: S102 —— 已过白名单,命名空间受限
    except Exception as e:  # 用户代码里的任何异常
        return fail("exec", e, line=user_line(e.__traceback__), tb=user_traceback(e, job["code"]))

    try:
        if "result" not in ns:
            raise Rejected("the script must assign the final shape to a variable named `result`")
        shape = as_shape(ns["result"])
        metrics = measure(shape)
        if metrics["solids"] == 0 or metrics["volume_mm3"] <= 0:
            raise Rejected("`result` has no solid volume (did a boolean operation remove everything?)")
    except Exception as e:
        return fail("result", e)

    try:
        files = export(shape, out_dir, job.get("exports") or ["step", "stl"],
                       float(job.get("stl_tolerance", 0.01)), float(job.get("stl_angular_tolerance", 0.1)))
    except Exception as e:
        return fail("export", e, tb=traceback.format_exc()[-2000:])

    return {
        "ok": True, "files": files, "metrics": metrics, "stdout": stdout.getvalue()[-4000:],
        "elapsed_ms": int((time.time() - started) * 1000),
    }


def write_result(job, result):
    os.makedirs(job["out_dir"], exist_ok=True)
    with open(os.path.join(job["out_dir"], "result.json"), "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False)


# 常驻模式的协议行都带这个前缀。OpenCascade 偶尔会直接往 stdout 写东西(绕过 Python 的 sys.stdout),
# 所以父进程按「这一行里有没有这个标记」来认,而不是按整行匹配。
SENTINEL = "\x1ePP-"


def serve():
    """常驻模式:`python runner.py --serve`。

    一次执行约 4 秒,几乎全是「起解释器 + import build123d」;同一个进程里连续建模只要几十毫秒。
    所以让它常驻:stdin 每行一个 job(JSON,多一个 `seq` 和 `timeout_s`),每个 job 跑完往 stdout 写一行
    `<SENTINEL>DONE <seq>`,结果照旧写在 `<out_dir>/result.json`。

    - 每个 job 仍是全新的命名空间;AST 白名单禁了属性赋值,`math` 是每任务一份的替身——上一个任务留不下东西;
    - stdin 关闭(父进程退出)→ 循环结束 → 进程退出,不会留孤儿;
    - 自带看门狗:超时由父进程负责杀,但父进程要是先没了,卡死的任务得自己了断。
    """
    import gc
    import threading

    import build123d

    out = sys.stdout  # 先存下真正的 stdout:执行用户代码期间 sys.stdout 会被换成缓冲区
    out.write(f"{SENTINEL}READY {sys.version.split()[0]} {build123d.__version__}\n")
    out.flush()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        seq = -1
        try:
            job = json.loads(line)
            seq = job.get("seq", -1)
            watchdog = threading.Timer(float(job.get("timeout_s", 90)) + 15, os._exit, [3])
            watchdog.daemon = True
            watchdog.start()
            try:
                write_result(job, run(job))
            finally:
                watchdog.cancel()
        except Exception:  # 协议层面的问题(坏 JSON、写不了结果):父进程读不到 result.json,会按协议错误处理
            traceback.print_exc()
        gc.collect()
        out.write(f"{SENTINEL}DONE {seq}\n")
        out.flush()
    return 0


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--serve":
        return serve()
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    with open(sys.argv[1], encoding="utf-8") as f:
        job = json.load(f)
    result = run(job)
    write_result(job, result)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
