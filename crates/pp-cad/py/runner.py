# -*- coding: utf-8 -*-
"""PrintPilot 的 build123d 执行器:在子进程里跑一段(模型写的)建模代码,导出文件并汇报指标。

用法:  python -I -B runner.py <job.json>      # 跑一个任务就退出
       python -I -B runner.py --serve         # 常驻:stdin 每行一个任务,见 serve()

job.json:
  { "code": "...", "out_dir": "...", "exports": ["step", "stl", "3mf"],
    "stl_tolerance": 0.02, "stl_angular_tolerance": 0.2 }
  stl_tolerance 是**绝对**弦差(毫米),不是相对值——见 mesh()。

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


def mesh(shape, tol, ang):
    """把形体三角化一次;之后量包围盒(曲面部分)、导 STL 都用这一份网格。

    `tol` 是**绝对**弦差(毫米)。build123d 自带的 export_stl 用的是相对偏差(isRelative=True):
    小孔被细分得过头(165 个孔的板 8 万多面、三角化 1.8 秒),大曲面反而没有误差上限(60 mm 的放样花瓶
    实际弦差约 0.6 mm,打印出来看得见棱)。绝对偏差 0.02 mm / 0.2 rad 对 FDM 绰绰有余,常见零件还快 2~3 倍。
    """
    from OCP.BRepMesh import BRepMesh_IncrementalMesh

    BRepMesh_IncrementalMesh(shape.wrapped, tol, False, ang, True)


def bounds(shape, faces):
    """包围盒(要先 mesh())。解析面走精确算法,环面和自由曲面走三角网。

    build123d 默认的精确包围盒在带圆角的零件上要 150~190 毫秒——比建出这个零件还久。实测慢的只有两类面:
    环面(圆角拐弯处,每个面约 26 毫秒)和样条面(约 19 毫秒);平面 / 圆柱 / 圆锥 / 球有解析解,每个面 0.05 毫秒。
    不精确的那种(optimal=False)不能用:它在圆角 / 环面 / 样条面上偏大 1 ~ 50 毫米。
    所以:解析面照旧精确(22 mm 的圆柱量出来就是 22.00);环面和自由曲面用网格顶点——顶点都在曲面上,
    只会偏小、且不超过弦差(0.02 mm)。零件的最外沿几乎总是平面或圆柱,落在后一类上的情况很少。
    """
    import build123d as bd
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib

    analytic = (bd.GeomType.PLANE, bd.GeomType.CYLINDER, bd.GeomType.CONE, bd.GeomType.SPHERE)
    exact, meshed = Bnd_Box(), Bnd_Box()
    for face in faces:
        if face.geom_type in analytic:
            BRepBndLib.AddOptimal_s(face.wrapped, exact, False, False)  # 不用网格、不加形体容差
        else:
            BRepBndLib.Add_s(face.wrapped, meshed, True)  # useTriangulation
    lo, hi = [math.inf] * 3, [-math.inf] * 3
    for box in (exact, meshed):
        if box.IsVoid():
            continue
        gap = box.GetGap()  # OCC 会按网格偏差 / 容差把盒子撑大一圈;去掉它,得到真正的范围
        x0, y0, z0, x1, y1, z1 = box.Get()
        lo = [min(a, b + gap) for a, b in zip(lo, (x0, y0, z0))]
        hi = [max(a, b - gap) for a, b in zip(hi, (x1, y1, z1))]
    if math.isinf(lo[0]):  # 一个面都没有(不是实体):交给后面的「没有体积」去报
        bb = shape.bounding_box()
        return [bb.min.X, bb.min.Y, bb.min.Z], [bb.max.X, bb.max.Y, bb.max.Z]
    return lo, hi


def printability(faces, z_min):
    """两项 FDM 可打印性的量测,只看平面(精确、便宜;曲面的悬垂要采样,先不做):
    - bed_contact_mm2:贴在打印床上的平面面积。太小 = 零件只靠一条边 / 一个曲面着床,打不成;
    - overhang_mm2:离开床面、法线朝下的平面(天花板 / 悬臂 / 桥)的面积——这些地方要么搭桥、要么加支撑。
    """
    import build123d as bd

    contact = overhang = 0.0
    for face in faces:
        if face.geom_type != bd.GeomType.PLANE:
            continue
        n = face.normal_at()
        if n.Z > -0.999:  # 只要正朝下的平面;斜面(> 45° 的另算,先不做)
            continue
        if abs(face.center().Z - z_min) <= 0.02:
            contact += face.area
        else:
            overhang += face.area
    return float(contact), float(overhang)


def is_valid(shape):
    """OCC 的拓扑 / 几何自检,只跑一遍。

    build123d 的 `is_valid` 在不同版本里一会儿是方法、一会儿是属性;以前为了两头兼容写的
    `x.is_valid() if callable(getattr(x, "is_valid")) else x.is_valid` 会把属性求值两次——每次都是一整遍检查
    (165 个孔的板 2 × 90 毫秒)。直接调 OCC,和 build123d 内部做的是同一件事。
    """
    from OCP.BRepCheck import BRepCheck_Analyzer

    return bool(BRepCheck_Analyzer(shape.wrapped).IsValid())


def measure(shape):
    """要先 mesh()。"""
    faces = shape.faces()
    lo, hi = bounds(shape, faces)
    solids = shape.solids()
    com = None
    try:
        c = shape.center()
        com = [c.X, c.Y, c.Z]
    except Exception:
        pass
    try:
        contact, overhang = printability(faces, lo[2])
    except Exception:  # 量不出来不该让整次建模失败:这两项只是提示
        contact = overhang = None
    return {
        "bbox_min": lo,
        "bbox_max": hi,
        "size": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
        "volume_mm3": float(sum(s.volume for s in solids)),
        "area_mm2": float(shape.area),
        "solids": len(solids),
        "faces": len(faces),
        "edges": len(shape.edges()),
        "is_valid": is_valid(shape),
        "center": com,
        "bed_contact_mm2": contact,
        "overhang_mm2": overhang,
    }


def export(shape, out_dir, kinds, tol, ang):
    import build123d as bd

    files = {}
    if "step" in kinds:
        bd.export_step(shape, os.path.join(out_dir, "model.step"))
        files["step"] = "model.step"
    if "stl" in kinds:
        # 网格已经在 mesh() 里算好了:直接写,不让 build123d 的 export_stl 按它自己的(相对)参数再三角化一遍
        from OCP.StlAPI import StlAPI_Writer

        writer = StlAPI_Writer()
        writer.ASCIIMode = False
        if not writer.Write(shape.wrapped, os.path.join(out_dir, "model.stl")):
            raise RuntimeError("STL writer failed")
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

    tol, ang = float(job.get("stl_tolerance", 0.02)), float(job.get("stl_angular_tolerance", 0.2))
    try:
        if "result" not in ns:
            raise Rejected("the script must assign the final shape to a variable named `result`")
        shape = as_shape(ns["result"])
        mesh(shape, tol, ang)
        metrics = measure(shape)
        if metrics["solids"] == 0 or metrics["volume_mm3"] <= 0:
            raise Rejected("`result` has no solid volume (did a boolean operation remove everything?)")
    except Exception as e:
        return fail("result", e)

    try:
        files = export(shape, out_dir, job.get("exports") or ["step", "stl"], tol, ang)
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
