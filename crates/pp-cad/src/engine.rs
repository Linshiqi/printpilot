//! 定位并探测 build123d 引擎(一个装好 build123d 的 Python 解释器)。
//!
//! 查找顺序:
//! 1. 环境变量 `PRINTPILOT_CAD_PYTHON`(开发者显式指定);
//! 2. `<应用数据>/cad-engine/python/python.exe` —— 给最终用户的**引擎包**(内嵌 CPython + build123d,首次使用时下载);
//! 3. `<应用数据>/cad-engine/venv/Scripts/python.exe` —— 开发期由 `scripts/setup-cad-engine.ps1` 建的虚拟环境。

use std::path::{Path, PathBuf};
use std::process::Command;

use pp_common::cad::CadEngineInfo;

use crate::run::{quiet, CadError};

pub const ENV_OVERRIDE: &str = "PRINTPILOT_CAD_PYTHON";

#[derive(Debug, Clone, PartialEq)]
pub struct Engine {
    pub python: PathBuf,
}

pub fn candidates(app_data_dir: &Path) -> Vec<PathBuf> {
    let root = app_data_dir.join("cad-engine");
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os(ENV_OVERRIDE).filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if cfg!(windows) {
        out.push(root.join("python").join("python.exe"));
        out.push(root.join("venv").join("Scripts").join("python.exe"));
    } else {
        out.push(root.join("python").join("bin").join("python3"));
        out.push(root.join("venv").join("bin").join("python"));
    }
    out
}

impl Engine {
    pub fn locate(app_data_dir: &Path) -> Option<Engine> {
        candidates(app_data_dir)
            .into_iter()
            .find(|p| p.is_file())
            .map(|python| Engine { python })
    }

    /// 真的起一次解释器,确认 build123d 能导入,并取版本号。冷启动要几秒(OpenCascade 很大)。
    pub fn probe(&self) -> Result<CadEngineInfo, CadError> {
        let mut cmd = Command::new(&self.python);
        cmd.args([
            "-I",
            "-c",
            "import sys, build123d; print(sys.version.split()[0]); print(build123d.__version__)",
        ]);
        let out = quiet(&mut cmd).output().map_err(|e| CadError::Spawn(e.to_string()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(CadError::Spawn(format!("engine does not start: {}", err.trim().chars().take(400).collect::<String>())));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim);
        Ok(CadEngineInfo {
            available: true,
            python: self.python.display().to_string(),
            python_version: lines.next().unwrap_or("").to_string(),
            build123d_version: lines.next().unwrap_or("").to_string(),
            ..Default::default()
        })
    }
}

/// 给设置页 / 建模面板用:有没有引擎、是什么版本、没有的话为什么。
pub fn engine_info(app_data_dir: &Path) -> CadEngineInfo {
    match Engine::locate(app_data_dir) {
        None => CadEngineInfo {
            problem: "not installed".into(),
            ..Default::default()
        },
        Some(engine) => engine.probe().unwrap_or_else(|e| CadEngineInfo {
            python: engine.python.display().to_string(),
            problem: e.to_string(),
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_pack_wins_over_the_dev_venv() {
        let dir = std::env::temp_dir().join(format!("pp-cad-engine-test-{}", std::process::id()));
        let list = candidates(&dir);
        let pack = list.iter().position(|p| p.components().any(|c| c.as_os_str() == "python"));
        let venv = list.iter().position(|p| p.components().any(|c| c.as_os_str() == "venv"));
        assert!(pack.unwrap() < venv.unwrap());
        assert!(list.iter().all(|p| p.starts_with(&dir) || std::env::var_os(ENV_OVERRIDE).is_some()));
    }

    #[test]
    fn a_missing_engine_is_reported_not_panicked() {
        let nowhere = std::env::temp_dir().join("pp-cad-no-such-dir-xyz");
        if std::env::var_os(ENV_OVERRIDE).is_none() {
            assert_eq!(Engine::locate(&nowhere), None);
            let info = engine_info(&nowhere);
            assert!(!info.available);
            assert_eq!(info.problem, "not installed");
        }
    }
}
