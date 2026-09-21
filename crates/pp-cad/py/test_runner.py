# -*- coding: utf-8 -*-
"""执行器的测试。用引擎自己的解释器跑:

    <engine>\\python.exe -m unittest discover -s crates/pp-cad/py -v

白名单那一组不需要 build123d;端到端那一组在没装 build123d 时自动跳过。
"""
import ast
import json
import os
import tempfile
import unittest

import runner

try:
    import build123d  # noqa: F401
    HAVE_B123D = True
except Exception:  # pragma: no cover
    HAVE_B123D = False


def rejected(code):
    try:
        runner.validate(ast.parse(code))
    except runner.Rejected as e:
        return str(e)
    return None


class AllowlistTests(unittest.TestCase):
    def test_ordinary_modeling_code_passes(self):
        code = """
from build123d import *
import math

# ---- PARAMS ----
width = 60.0  # mm | 总宽 | [20, 200]

with BuildPart() as p:
    Box(width, 40, 6)
    fillet(p.edges().filter_by(Axis.Z), radius=2)

class Helper:
    def area(self, r):
        return math.pi * r ** 2

result = p.part
"""
        self.assertIsNone(rejected(code))

    def test_other_imports_are_refused(self):
        for code in ["import os", "import subprocess as sp", "from pathlib import Path",
                     "import build123d", "from . import x", "import math.os"]:
            self.assertIsNotNone(rejected(code), code)
        self.assertIsNone(rejected("import math"))
        self.assertIsNone(rejected("from math import pi, sin"))

    def test_dangerous_builtins_are_refused(self):
        for code in ["open('x')", "eval('1')", "exec('x=1')", "__import__('os')",
                     "getattr(Box, 'x')", "globals()", "type(1)", "compile('1','','eval')"]:
            self.assertIsNotNone(rejected(code), code)

    def test_dunder_escapes_are_refused(self):
        for code in ["().__class__", "Box.__init__.__globals__", "x = [].__class__.__base__",
                     "__builtins__", "__name__"]:
            self.assertIsNotNone(rejected(code), code)

    def test_build123d_file_io_is_refused_in_every_spelling(self):
        for code in ["export_step(p, 'C:/x.step')", "export_stl(p, 'x.stl')", "import_step('x.step')",
                     "from build123d import export_stl", "Mesher().write('x.3mf')",
                     "p.export_brep('x')", "ExportSVG()", "p.part.save('x')"]:
            self.assertIsNotNone(rejected(code), code)

    def test_async_and_scope_tricks_are_refused(self):
        for code in ["async def f():\n    pass", "def f():\n    global x\n    x = 1"]:
            self.assertIsNotNone(rejected(code), code)

    def test_shared_objects_cannot_be_modified_so_nothing_leaks_into_the_next_job(self):
        # 常驻模式下同一个进程跑很多段代码:给共享对象的属性赋值会留到下一个任务里
        for code in ["from build123d import *\nBox.leak = 1", "import math\nmath.pi = 3",
                     "from build123d import *\ndel Box.leak", "import math\nmath.pi += 1"]:
            self.assertIn("assigning to attributes", rejected(code) or "", code)
        # 读属性、调方法、给局部变量赋值照常
        self.assertIsNone(rejected("from build123d import *\nb = Box(1, 1, 1)\nh = b.bounding_box().size.Z\nresult = b"))

    def test_rejection_reports_the_line(self):
        try:
            runner.validate(ast.parse("x = 1\ny = 2\nimport os\n"))
            self.fail("should have been rejected")
        except runner.Rejected as e:
            self.assertEqual(e.line, 3)


@unittest.skipUnless(HAVE_B123D, "build123d is not installed in this interpreter")
class EndToEndTests(unittest.TestCase):
    def run_code(self, code, exports=("step", "stl")):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        return runner.run({"code": code, "out_dir": self.tmp.name, "exports": list(exports)})

    def test_a_box_with_a_hole_exports_and_measures_exactly(self):
        r = self.run_code("""
from build123d import *
w, d, h, hole = 60.0, 40.0, 10.0, 8.0
result = Box(w, d, h) - Cylinder(hole / 2, h)
""")
        self.assertTrue(r["ok"], r)
        m = r["metrics"]
        self.assertEqual([round(v, 6) for v in m["size"]], [60.0, 40.0, 10.0])
        import math
        self.assertAlmostEqual(m["volume_mm3"], 60 * 40 * 10 - math.pi * 4 ** 2 * 10, places=3)
        self.assertEqual(m["solids"], 1)
        self.assertTrue(m["is_valid"])
        for name in r["files"].values():
            self.assertGreater(os.path.getsize(os.path.join(self.tmp.name, name)), 100)

    def test_builder_mode_result_is_accepted(self):
        r = self.run_code("""
from build123d import *
with BuildPart() as p:
    Box(20, 20, 5)
    fillet(p.edges().filter_by(Axis.Z), radius=2)
result = p
""")
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["metrics"]["solids"], 1)

    def test_3mf_export(self):
        r = self.run_code("from build123d import *\nresult = Sphere(10)", exports=("3mf",))
        self.assertTrue(r["ok"], r)
        self.assertIn("3mf", r["files"])

    def test_analytic_faces_are_measured_exactly_and_curved_ones_to_mesh_accuracy(self):
        # 躺着的圆柱:最低点落在圆柱面上。解析面走精确算法——量出来正好贴床,不会被误判成「悬空」
        r = self.run_code("from build123d import *\nresult = Pos(0, 0, 10) * Rot(90, 0, 0) * Cylinder(10, 40)")
        self.assertTrue(r["ok"], r)
        self.assertEqual([round(v, 6) for v in r["metrics"]["size"]], [20.0, 40.0, 20.0])
        self.assertAlmostEqual(r["metrics"]["bbox_min"][2], 0.0, places=6)
        # 环面走三角网:只会偏小,且不超过弦差(每侧 0.02)
        r = self.run_code("from build123d import *\nresult = Torus(30, 6)")
        for got, want in zip(r["metrics"]["size"], (72.0, 72.0, 12.0)):
            self.assertLessEqual(got, want + 1e-9)
            self.assertGreater(got, want - 0.05)

    def test_fillets_do_not_inflate_the_size(self):
        # 圆角拐弯处是环面:build123d 那种「快但不精确」的包围盒会在这里偏大约 1 mm
        r = self.run_code("""
from build123d import *
L, W, H, wall, r = 120, 80, 30, 2.4, 6
outer = fillet(Box(L, W, H, align=(Align.CENTER, Align.CENTER, Align.MIN)).edges().filter_by(Axis.Z), r)
inner = fillet((Pos(0, 0, wall) * Box(L - 2 * wall, W - 2 * wall, H, align=(Align.CENTER, Align.CENTER, Align.MIN))).edges().filter_by(Axis.Z), r - wall)
result = fillet((outer - inner).edges().group_by(Axis.Z)[-1], 0.8)
""")
        self.assertTrue(r["ok"], r)
        self.assertEqual([round(v, 4) for v in r["metrics"]["size"]], [120.0, 80.0, 30.0])
        self.assertAlmostEqual(r["metrics"]["bbox_min"][2], 0.0, places=6)
        self.assertTrue(r["metrics"]["is_valid"])

    def test_stl_is_binary_and_meshed_with_an_absolute_chord_tolerance(self):
        # build123d 自带的 export_stl 用相对偏差:这只 60 mm 的放样花瓶只有约 3800 个三角面、弦差约 0.6 mm
        r = self.run_code(
            "from build123d import *\n"
            "result = loft([Pos(0, 0, 0) * Circle(20), Pos(0, 0, 30) * Circle(32), Pos(0, 0, 60) * Circle(14)])",
            exports=("stl",))
        self.assertTrue(r["ok"], r)
        path = os.path.join(self.tmp.name, r["files"]["stl"])
        with open(path, "rb") as f:
            triangles = int.from_bytes(f.read(84)[80:84], "little")
        self.assertEqual(os.path.getsize(path), 84 + 50 * triangles)  # 二进制 STL 的长度是定死的
        self.assertGreater(triangles, 8000)
        # 粗一档的公差 → 三角面明显变少:公差确实传到了三角化那一步
        coarse = runner.run({"code": "from build123d import *\nresult = loft([Pos(0, 0, 0) * Circle(20), Pos(0, 0, 30) * Circle(32), Pos(0, 0, 60) * Circle(14)])",
                             "out_dir": self.tmp.name, "exports": ["stl"], "stl_tolerance": 0.2, "stl_angular_tolerance": 0.5})
        with open(path, "rb") as f:
            self.assertLess(int.from_bytes(f.read(84)[80:84], "little"), triangles / 2)
        self.assertTrue(coarse["ok"], coarse)

    def test_printability_measures_bed_contact_and_flat_overhangs(self):
        # 一张桌子:四条腿着床,桌面底下是悬空的平面
        r = self.run_code("""
from build123d import *
top = Pos(0, 0, 20) * Box(60, 40, 4, align=(Align.CENTER, Align.CENTER, Align.MIN))
legs = [Pos(x, y, 0) * Box(6, 6, 20, align=(Align.CENTER, Align.CENTER, Align.MIN)) for x in (-27, 27) for y in (-17, 17)]
result = top + legs
""")
        self.assertTrue(r["ok"], r)
        self.assertAlmostEqual(r["metrics"]["bed_contact_mm2"], 4 * 36, places=3)
        self.assertAlmostEqual(r["metrics"]["overhang_mm2"], 60 * 40 - 4 * 36, places=3)
        # 实心的盒子:整个底面着床,没有悬空
        r = self.run_code("from build123d import *\nresult = Box(30, 20, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))")
        self.assertAlmostEqual(r["metrics"]["bed_contact_mm2"], 600, places=3)
        self.assertAlmostEqual(r["metrics"]["overhang_mm2"], 0, places=6)
        # 球:只有一个点着床
        r = self.run_code("from build123d import *\nresult = Pos(0, 0, 10) * Sphere(10)")
        self.assertAlmostEqual(r["metrics"]["bed_contact_mm2"], 0, places=6)

    def test_runtime_errors_point_at_the_users_line_without_library_frames(self):
        r = self.run_code("""
from build123d import *
box = Box(10, 10, 10)
result = fillet(box.edges(), radius=50)
""")
        self.assertFalse(r["ok"])
        self.assertEqual(r["stage"], "exec")
        self.assertEqual(r["line"], 4)
        self.assertNotIn("site-packages", r["traceback"])
        # 模型要看到出错的那一行源码才好修(用户代码不是磁盘文件,traceback 自己查不到)
        self.assertIn("line 4: result = fillet(box.edges(), radius=50)", r["traceback"])

    def test_missing_or_empty_result_is_explained(self):
        r = self.run_code("from build123d import *\npart = Box(1, 1, 1)")
        self.assertEqual((r["ok"], r["stage"]), (False, "result"))
        self.assertIn("result", r["message"])

        r = self.run_code("from build123d import *\nresult = Box(10, 10, 10) - Box(20, 20, 20)")
        self.assertEqual((r["ok"], r["stage"]), (False, "result"))

        r = self.run_code("result = 42")
        self.assertEqual((r["ok"], r["stage"]), (False, "result"))

    def test_refused_code_never_runs(self):
        marker = os.path.join(tempfile.gettempdir(), "pp_runner_should_not_exist.txt")
        if os.path.exists(marker):
            os.remove(marker)
        r = self.run_code(f"open({marker!r}, 'w').write('x')\nresult = None")
        self.assertEqual((r["ok"], r["stage"]), (False, "validate"))
        self.assertFalse(os.path.exists(marker))

    def test_each_job_gets_its_own_math_module(self):
        import math
        a, b = runner.build_namespace(), runner.build_namespace()
        self.assertIsNot(a["math"], math)
        self.assertIsNot(a["math"], b["math"])
        a["math"].pi = 3  # 就算绕过了白名单,也只改到这一个任务自己的替身
        self.assertEqual(b["math"].pi, math.pi)
        self.assertAlmostEqual(a["math"].sqrt(16), 4.0)

    def test_io_names_are_not_even_in_the_namespace(self):
        ns = runner.build_namespace()
        for name in ["export_step", "export_stl", "import_step", "Mesher", "open", "__import__"]:
            self.assertNotIn(name, ns)
            self.assertNotIn(name, ns["__builtins__"])
        self.assertIn("Box", ns)
        self.assertIn("fillet", ns)

    def test_print_output_is_captured(self):
        r = self.run_code("from build123d import *\nprint('wall =', 2.4)\nresult = Box(1, 1, 1)")
        self.assertTrue(r["ok"], r)
        self.assertIn("wall = 2.4", r["stdout"])

    def test_main_writes_result_json_and_sets_the_exit_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = os.path.join(tmp, "job.json")
            out = os.path.join(tmp, "out")
            with open(job, "w", encoding="utf-8") as f:
                json.dump({"code": "import os", "out_dir": out}, f)
            import sys
            argv, sys.argv = sys.argv, ["runner.py", job]
            try:
                self.assertEqual(runner.main(), 1)
            finally:
                sys.argv = argv
            with open(os.path.join(out, "result.json"), encoding="utf-8") as f:
                self.assertEqual(json.load(f)["stage"], "validate")


@unittest.skipUnless(HAVE_B123D, "build123d not installed")
class ServeModeTests(unittest.TestCase):
    """常驻模式:一个进程连续跑多个任务。"""

    def setUp(self):
        import subprocess
        import sys
        self.tmp = tempfile.TemporaryDirectory()
        here = os.path.dirname(os.path.abspath(runner.__file__))
        self.proc = subprocess.Popen(
            [sys.executable, "-I", "-B", "-X", "utf8", os.path.join(here, "runner.py"), "--serve"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8", cwd=self.tmp.name)
        ready = self.proc.stdout.readline()
        self.assertIn(runner.SENTINEL + "READY", ready)
        self.assertRegex(ready.strip(), r"READY \d+\.\d+\.\d+ \d+\.\d+")

    def tearDown(self):
        self.proc.stdin.close()
        self.assertEqual(self.proc.wait(timeout=20), 0, "stdin 一关,常驻进程就该自己退出")
        self.proc.stdout.close()
        self.tmp.cleanup()

    def job(self, seq, code):
        out = os.path.join(self.tmp.name, f"job-{seq}")
        self.proc.stdin.write(json.dumps({"seq": seq, "code": code, "out_dir": out, "exports": ["stl"], "timeout_s": 60}) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            self.assertTrue(line, "常驻进程意外退出")
            if runner.SENTINEL + f"DONE {seq}" in line:
                break
        with open(os.path.join(out, "result.json"), encoding="utf-8") as f:
            return json.load(f)

    def test_jobs_run_back_to_back_and_a_failure_does_not_kill_the_worker(self):
        first = self.job(1, "from build123d import *\nprint('hello')\nresult = Box(10, 20, 30)")
        self.assertTrue(first["ok"], first)
        self.assertEqual([round(v) for v in first["metrics"]["size"]], [10, 20, 30])
        self.assertIn("hello", first["stdout"], "用户的 print 进结果,不能混进协议行")

        broken = self.job(2, "from build123d import *\nresult = fillet(Box(10, 10, 10).edges(), radius=50)")
        self.assertFalse(broken["ok"])
        self.assertEqual(broken["stage"], "exec")

        refused = self.job(3, "import os\nresult = 1")
        self.assertEqual(refused["stage"], "validate")

        # 前面任务里的名字带不到后面:每个任务都是全新的命名空间
        leaked = self.job(4, "from build123d import *\nsecret = 42\nresult = Box(1, 1, 1)")
        self.assertTrue(leaked["ok"])
        after = self.job(5, "from build123d import *\nresult = Box(secret, 1, 1)")
        self.assertFalse(after["ok"])
        self.assertEqual(after["error_type"], "NameError")

        again = self.job(6, "from build123d import *\nresult = Cylinder(5, 10)")
        self.assertTrue(again["ok"], again)
        self.assertLess(again["elapsed_ms"], 2000, "热进程里建一个圆柱不该再花几秒")


if __name__ == "__main__":
    unittest.main()
