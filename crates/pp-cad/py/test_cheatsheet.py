# -*- coding: utf-8 -*-
"""提示词里的 build123d 速查表(crates/pp-agent/prompts/cad_code.md)逐条过一遍真引擎。

速查表写错一个名字,模型就会照着错;build123d 升级改了 API,这里先红。
每条都走 runner.run(),所以同时验证了「沙箱命名空间里有这个名字」。

  <引擎 python> -m unittest test_cheatsheet -v
"""
import os
import tempfile
import unittest

import runner

HEAD = "from build123d import *\nimport math\n"


class CheatSheet(unittest.TestCase):
    def build(self, body, exports=()):
        with tempfile.TemporaryDirectory() as d:
            res = runner.run({"code": HEAD + body, "out_dir": d, "exports": list(exports) or ["none"]})
        self.assertTrue(res["ok"], msg=f'{res.get("stage")} line {res.get("line")}: {res.get("error_type")}: {res.get("message")}')
        return res["metrics"]

    def assertSize(self, m, expected, places=3):
        for got, want in zip(m["size"], expected):
            self.assertAlmostEqual(got, want, places=places)

    # ---- primitives ----
    def test_box_sits_on_the_plate_with_align_min(self):
        m = self.build("result = Box(60, 40, 6, align=(Align.CENTER, Align.CENTER, Align.MIN))")
        self.assertSize(m, (60, 40, 6))
        self.assertAlmostEqual(m["bbox_min"][2], 0, places=6)

    def test_cylinder_sphere_cone_torus(self):
        self.assertSize(self.build("result = Cylinder(5, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))"), (10, 10, 10))
        self.assertSize(self.build("result = Sphere(5)"), (10, 10, 10), places=2)
        self.assertSize(self.build("result = Cone(6, 2, 10)"), (12, 12, 10), places=2)
        self.assertSize(self.build("result = Torus(10, 2)"), (24, 24, 4), places=2)

    # ---- placement and booleans ----
    def test_cut_fuse_intersect(self):
        m = self.build(
            "body = Box(40, 20, 6, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "part = body - Pos(10, 0, -0.5) * Cylinder(3, 7, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "part = part + Pos(-10, 0, 6) * Cylinder(4, 5, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "result = part & Box(40, 20, 30, align=(Align.CENTER, Align.CENTER, Align.MIN))\n")
        self.assertEqual(m["solids"], 1)
        self.assertSize(m, (40, 20, 11))

    def test_rot_is_in_degrees_and_applies_before_pos(self):
        m = self.build("result = Pos(0, 0, 5) * Rot(0, 90, 0) * Cylinder(5, 30)")
        self.assertSize(m, (30, 10, 10), places=2)

    def test_lists_work_with_minus_and_plus(self):
        m = self.build(
            "body = Box(40, 20, 6, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "tool = Cylinder(2, 8)\n"
            "part = body - [Pos(x, 0, 3) * tool for x in (-10, 0, 10)]\n"
            "result = part + [Pos(x, 0, 6) * Box(2, 2, 2, align=(Align.CENTER, Align.CENTER, Align.MIN)) for x in (-15, 15)]\n")
        self.assertEqual(m["solids"], 1)
        self.assertSize(m, (40, 20, 8))

    def test_grid_and_polar_locations(self):
        m = self.build(
            "plate = Cylinder(30, 4, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "holes = [loc * Cylinder(2, 10) for loc in PolarLocations(20, 6)]\n"
            "grid = [loc * Cylinder(1.5, 10) for loc in GridLocations(10, 10, 2, 2)]\n"
            "result = plate - holes - grid\n")
        self.assertEqual(m["solids"], 1)
        # 6 + 4 个通孔:每个孔多一个圆柱面
        self.assertEqual(m["faces"], 3 + 10)

    # ---- 2D sketch -> solid ----
    def test_sketch_boolean_and_extrude(self):
        m = self.build(
            "profile = RectangleRounded(40, 20, radius=3) - Pos(10, 0) * Circle(3)\n"
            "result = extrude(profile, amount=5)\n")
        self.assertSize(m, (40, 20, 5))
        self.assertAlmostEqual(m["bbox_min"][2], 0, places=6)

    def test_extrude_from_an_offset_plane(self):
        m = self.build(
            "base = Box(20, 20, 6, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "boss = extrude(Plane.XY.offset(6) * Circle(4), amount=10)\n"
            "result = base + boss\n")
        self.assertEqual(m["solids"], 1)
        self.assertSize(m, (20, 20, 16))

    def test_extrude_both_is_symmetric_about_the_plane(self):
        m = self.build("result = extrude(Plane.XZ * Rectangle(30, 10), amount=8, both=True)")
        self.assertSize(m, (30, 16, 10))
        self.assertAlmostEqual(m["bbox_min"][1], -8, places=6)

    def test_revolve_a_profile_around_z(self):
        m = self.build("result = revolve(Plane.XZ * Pos(10, 0) * Rectangle(2, 5), axis=Axis.Z)")
        self.assertSize(m, (22, 22, 5), places=2)

    def test_other_2d_shapes(self):
        for sketch in ("Rectangle(10, 6)", "Circle(4)", "Ellipse(6, 3)", "RegularPolygon(6, 6)",
                       "SlotOverall(20, 6)", "Polygon((0, 0), (10, 0), (10, 5), (0, 8))"):
            with self.subTest(sketch=sketch):
                self.assertEqual(self.build(f"result = extrude({sketch}, amount=2)")["solids"], 1)

    def test_text_can_be_embossed(self):
        m = self.build(
            "plate = Box(40, 16, 2, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "label = extrude(Plane.XY.offset(2) * Text('PP', font_size=10), amount=0.8)\n"
            "result = plate + label\n")
        self.assertEqual(m["solids"], 1)
        self.assertAlmostEqual(m["size"][2], 2.8, places=3)

    # ---- selectors, fillet, chamfer, shell, mirror ----
    def test_fillet_vertical_edges_and_chamfer_the_bottom(self):
        m = self.build(
            "part = Box(40, 20, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "part = fillet(part.edges().filter_by(Axis.Z), radius=3)\n"
            "result = chamfer(part.edges().group_by(Axis.Z)[0], length=0.6)\n")
        self.assertEqual(m["solids"], 1)
        self.assertTrue(m["is_valid"])
        self.assertSize(m, (40, 20, 10))
        self.assertLess(m["volume_mm3"], 40 * 20 * 10)

    def test_top_and_bottom_faces_and_circular_edges(self):
        m = self.build(
            "part = Box(30, 30, 8, align=(Align.CENTER, Align.CENTER, Align.MIN)) - Cylinder(5, 30)\n"
            "top = part.faces().sort_by(Axis.Z)[-1]\n"
            "bottom = part.faces().sort_by(Axis.Z)[0]\n"
            "rims = part.edges().filter_by(GeomType.CIRCLE)\n"
            "print(round(top.center().Z, 3), round(bottom.center().Z, 3), len(rims))\n"
            "result = chamfer(rims, length=0.8)\n")
        self.assertEqual(m["solids"], 1)

    def test_hollow_box_open_at_the_top(self):
        m = self.build(
            "part = Box(40, 30, 20, align=(Align.CENTER, Align.CENTER, Align.MIN))\n"
            "result = offset(part, amount=-2, openings=part.faces().sort_by(Axis.Z)[-1])\n")
        self.assertEqual(m["solids"], 1)
        self.assertSize(m, (40, 30, 20))
        # 外体积 - 内腔(36 x 26 x 18)
        self.assertAlmostEqual(m["volume_mm3"], 40 * 30 * 20 - 36 * 26 * 18, delta=1.0)

    def test_mirror_about_a_plane(self):
        m = self.build(
            "half = Pos(5, 0, 0) * Box(10, 10, 4, align=(Align.MIN, Align.CENTER, Align.MIN))\n"
            "result = half + mirror(half, about=Plane.YZ)\n")
        # 两半不相接(中间隔 10 mm)→ 两个实体;这条只验证 mirror 的方向
        self.assertSize(m, (30, 10, 4))

    def test_the_complete_example_in_the_prompt_builds_exactly_as_promised(self):
        """提示词里的完整示例是模型照着学的范本:它必须真的能跑,而且满足我们对生成结果的全部要求。"""
        import re
        here = os.path.dirname(os.path.abspath(__file__))
        with open(os.path.join(here, "..", "..", "pp-agent", "prompts", "cad_code.md"), encoding="utf-8") as f:
            prompt = f.read()
        section = prompt.split("## Complete example", 1)[1]
        code = re.search(r"```python\n(.*?)```", section, re.S).group(1)

        with tempfile.TemporaryDirectory() as d:
            res = runner.run({"code": code, "out_dir": d, "exports": ["none"]})
        self.assertTrue(res["ok"], msg=f'{res.get("stage")} line {res.get("line")}: {res.get("message")}')
        m = res["metrics"]
        self.assertEqual(m["solids"], 1, "隔板必须和壳体并成一个实体")
        self.assertTrue(m["is_valid"])
        self.assertAlmostEqual(m["bbox_min"][2], 0, places=6)
        self.assertSize(m, (90, 60, 35))
        # 零隔板、多隔板两个边界也要成立(参数面板会让用户拖到这些值)
        for count in (0, 6):
            with self.subTest(divider_count=count):
                variant = code.replace("divider_count = 2 ", f"divider_count = {count} ")
                self.assertNotEqual(variant, code)
                with tempfile.TemporaryDirectory() as d:
                    r = runner.run({"code": variant, "out_dir": d, "exports": ["none"]})
                self.assertTrue(r["ok"], msg=r.get("message"))
                self.assertEqual(r["metrics"]["solids"], 1)

    def test_export_still_works_for_a_cheat_sheet_part(self):
        with tempfile.TemporaryDirectory() as d:
            res = runner.run({"code": HEAD + "result = fillet(Box(20, 20, 10).edges().filter_by(Axis.Z), radius=2)\n",
                              "out_dir": d, "exports": ["step", "stl"]})
            self.assertTrue(res["ok"], msg=res.get("message"))
            for name in res["files"].values():
                self.assertGreater(os.path.getsize(os.path.join(d, name)), 1000)


if __name__ == "__main__":
    unittest.main()
