//! 代码式 CAD(build123d)。设计见 docs/adr/0003-code-cad-build123d.md。
//!
//! - `engine`:找到装好 build123d 的 Python 解释器(引擎包 / 开发用的虚拟环境);
//! - `run`:在子进程里执行建模代码(冷启动,一次一个进程)——清空环境变量、超时即杀、只认执行器写的 result.json;
//! - `worker`:同样的隔离,但进程常驻——省掉每次约 4 秒的「起解释器 + import build123d」,改参数才能做到即改即见;
//!   执行器本身(`py/runner.py`)做 AST 白名单并统一导出 STEP / STL / 3MF;
//! - `params`:解析与改写 `# ---- PARAMS ----` 段——不经过模型的「局部修改」;
//! - `contract`:代码契约的分段与检查。

pub mod contract;
pub mod engine;
pub mod params;
pub mod run;
pub mod worker;

pub use engine::{engine_info, Engine};
pub use params::{parse_params, set_param, ParamError};
pub use run::{run, CadError, RunOptions, RunOutput};
pub use worker::Worker;

#[cfg(test)]
mod engine_tests {
    //! 需要真实引擎的端到端测试。没装引擎时自动跳过(打印一行说明),装了就真跑。
    //! 引擎由 `scripts/setup-cad-engine.ps1` 安装。

    use super::*;

    fn engine() -> Option<Engine> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let found = Engine::locate(&std::path::Path::new(&local).join("ai.printpilot"));
        if found.is_none() {
            eprintln!("(skipped: no CAD engine installed — run scripts/setup-cad-engine.ps1)");
        }
        found
    }

    fn work_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pp-cad-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    const BRACKET: &str = "from build123d import *\n\
\n\
# ---- PARAMS ----\n\
width = 60.0        # mm | 总宽 | [20, 200]\n\
depth = 40.0        # mm | 总深 | [20, 200]\n\
thick = 6.0         # mm | 板厚 | [2, 20]\n\
hole_d = 4.2        # mm | 安装孔直径 | [2, 10]\n\
\n\
# ---- FEATURE: plate ----\n\
plate = Box(width, depth, thick)\n\
plate = fillet(plate.edges().filter_by(Axis.Z), radius=5)\n\
\n\
# ---- FEATURE: mounting_holes ----\n\
holes = [Pos(x, y, 0) * Cylinder(hole_d / 2, thick) for x in (-width / 2 + 8, width / 2 - 8) for y in (-depth / 2 + 8, depth / 2 - 8)]\n\
\n\
# ---- RESULT ----\n\
result = plate - holes\n";

    #[test]
    fn real_engine_runs_a_parametric_part_and_a_param_edit_changes_exactly_that_dimension() {
        let Some(engine) = engine() else { return };
        let dir = work_dir("bracket");
        let out = run(&engine, BRACKET, &dir, &RunOptions::default()).unwrap_or_else(|e| panic!("bracket should build: {e:?}"));
        assert_eq!(out.metrics.solids, 1);
        assert!(out.metrics.is_valid);
        assert!((out.metrics.size[0] - 60.0).abs() < 1e-6 && (out.metrics.size[2] - 6.0).abs() < 1e-6);
        assert!(out.files["stl"].is_file() && out.files["step"].is_file());

        // 「局部修改」的零成本路径:改一个参数,只有那个尺寸变
        let wider = set_param(BRACKET, "width", 90.0).unwrap();
        let dir2 = work_dir("bracket-wide");
        let out2 = run(&engine, &wider, &dir2, &RunOptions::default()).unwrap();
        assert!((out2.metrics.size[0] - 90.0).abs() < 1e-6);
        assert!((out2.metrics.size[1] - out.metrics.size[1]).abs() < 1e-9);
        assert_eq!(out2.metrics.faces, out.metrics.faces, "拓扑不变,只是变宽了");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn real_engine_refuses_file_access_and_reports_runtime_errors_with_the_line() {
        let Some(engine) = engine() else { return };

        let dir = work_dir("refuse");
        let err = run(&engine, "import os\nresult = None\n", &dir, &RunOptions::default()).unwrap_err();
        let CadError::Script(e) = err else { panic!("expected a script error, got {err:?}") };
        assert_eq!((e.stage.as_str(), e.line), ("validate", Some(1)));
        let _ = std::fs::remove_dir_all(&dir);

        let dir = work_dir("fillet");
        let bad = "from build123d import *\nbox = Box(10, 10, 10)\nresult = fillet(box.edges(), radius=50)\n";
        let err = run(&engine, bad, &dir, &RunOptions::default()).unwrap_err();
        let CadError::Script(e) = err else { panic!("expected a script error, got {err:?}") };
        assert_eq!((e.stage.as_str(), e.line), ("exec", Some(3)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn real_engine_kills_runaway_scripts() {
        let Some(engine) = engine() else { return };
        let dir = work_dir("timeout");
        let opts = RunOptions {
            timeout: std::time::Duration::from_secs(8),
            ..Default::default()
        };
        let err = run(&engine, "while True:\n    pass\n", &dir, &opts).unwrap_err();
        assert!(matches!(err, CadError::Timeout(_)), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
