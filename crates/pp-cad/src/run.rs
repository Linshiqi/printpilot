//! 在子进程里执行一段建模代码(docs/adr/0003 的「进程隔离」一层)。
//!
//! - 解释器以 `-I -B -X utf8` 启动:隔离模式(不读 PYTHON* 环境变量、不加载用户 site)、不写 .pyc;
//! - 环境变量清空后只放回 Windows 起进程必需的几个——**接口密钥之类的东西不会漏进去**;
//! - 工作目录是调用方给的一个临时目录,执行器只往里面写;
//! - 超时即杀;
//! - 只认执行器写的 `result.json`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pp_common::cad::{CadMetrics, CadScriptError};
use serde::Deserialize;

use crate::engine::Engine;

/// 执行器脚本编进二进制里,每次执行写到工作目录——不存在「脚本版本和程序版本对不上」的问题。
const RUNNER_PY: &str = include_str!("../py/runner.py");

#[derive(Debug, Clone, PartialEq)]
pub enum CadError {
    /// 没找到引擎(没装引擎包 / 没跑 setup-cad-engine.ps1)
    EngineMissing,
    /// 解释器起不来
    Spawn(String),
    Timeout(Duration),
    /// 代码本身的问题:被白名单拒绝、运行时异常、没有 result、导出失败。可以喂回给模型修
    Script(CadScriptError),
    /// 执行器没按约定写结果(崩溃、被杀、磁盘满…)
    Protocol(String),
    Io(String),
    /// 用户取消了这一轮:正在跑的脚本进程已经被杀掉
    Cancelled,
}

impl std::fmt::Display for CadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CadError::EngineMissing => write!(f, "CAD engine is not installed"),
            CadError::Spawn(e) => write!(f, "CAD engine failed to start: {e}"),
            CadError::Timeout(d) => write!(f, "model script ran longer than {} s", d.as_secs()),
            CadError::Script(e) => write!(f, "{}", e.for_model()),
            CadError::Protocol(e) => write!(f, "CAD engine returned no result: {e}"),
            CadError::Io(e) => write!(f, "{e}"),
            CadError::Cancelled => write!(f, "cancelled by user"),
        }
    }
}

impl std::error::Error for CadError {}

impl CadError {
    pub fn code(&self) -> &'static str {
        match self {
            CadError::EngineMissing => "cad_engine_missing",
            CadError::Spawn(_) => "cad_engine_failed",
            CadError::Timeout(_) => "cad_timeout",
            CadError::Script(_) => "cad_script_error",
            CadError::Protocol(_) => "cad_engine_failed",
            CadError::Io(_) => "io_failed",
            CadError::Cancelled => "cancelled",
        }
    }
}

/// 「取消」开关:调用方置为 true,正在执行的脚本进程会在约 0.1 秒内被杀掉,`run` 返回 `CadError::Cancelled`。
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 两个开关是不是同一个(`RunOptions` 要能比较;开关的「值」没有比较的意义)。
impl PartialEq for CancelFlag {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunOptions {
    /// `step` / `stl` / `3mf`
    pub exports: Vec<String>,
    /// STL / 3MF 的线性偏差(毫米)。0.01 对 FDM 已经远超打印精度
    pub stl_tolerance: f64,
    pub stl_angular_tolerance: f64,
    pub timeout: Duration,
    /// 用户点了「停止」时由调用方置位
    pub cancel: CancelFlag,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            exports: vec!["step".into(), "stl".into()],
            stl_tolerance: 0.01,
            stl_angular_tolerance: 0.1,
            timeout: Duration::from_secs(90),
            cancel: CancelFlag::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    /// 导出的文件:种类 → 绝对路径
    pub files: HashMap<String, PathBuf>,
    pub metrics: CadMetrics,
    pub stdout: String,
    pub elapsed_ms: u64,
}

#[derive(Deserialize)]
struct RawResult {
    ok: bool,
    #[serde(default)]
    files: HashMap<String, String>,
    #[serde(default)]
    metrics: Option<CadMetrics>,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    stage: String,
    #[serde(default)]
    error_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    traceback: String,
}

/// GUI 程序里起子进程:不要闪一个黑色控制台窗口。
pub(crate) fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 清空环境后放回的变量。Windows 上缺 SystemRoot 时 Python 连随机数源都初始化不了。
const KEPT_ENV: [&str; 5] = ["SystemRoot", "windir", "SystemDrive", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE"];

/// 解析执行器写的 result.json(纯函数,可单测)。
pub(crate) fn parse_result(json: &str, out_dir: &Path) -> Result<RunOutput, CadError> {
    let raw: RawResult = serde_json::from_str(json).map_err(|e| CadError::Protocol(format!("bad result.json: {e}")))?;
    if !raw.ok {
        return Err(CadError::Script(CadScriptError {
            stage: raw.stage,
            error_type: raw.error_type,
            message: raw.message,
            line: raw.line,
            traceback: raw.traceback,
        }));
    }
    let metrics = raw
        .metrics
        .ok_or_else(|| CadError::Protocol("result.json has no metrics".into()))?;
    let mut files = HashMap::new();
    for (kind, name) in raw.files {
        // 执行器只该给出工作目录里的文件名;带路径分隔符的一律不认
        if name.contains(['/', '\\']) || name.contains("..") {
            return Err(CadError::Protocol(format!("unexpected file name {name:?}")));
        }
        files.insert(kind, out_dir.join(name));
    }
    Ok(RunOutput {
        files,
        metrics,
        stdout: raw.stdout,
        elapsed_ms: raw.elapsed_ms,
    })
}

/// 起引擎子进程的公共部分(单次执行与常驻进程共用)——隔离措施都在这里:
/// 隔离模式的解释器、清空的环境变量、沙箱目录当工作目录和「主目录」、stderr 落文件、不弹控制台窗口。
pub(crate) fn sandboxed_command(engine: &Engine, sandbox: &Path, args: &[&std::ffi::OsStr]) -> Result<Command, CadError> {
    let io = |e: std::io::Error| CadError::Io(e.to_string());
    // stderr 写到文件而不是管道:我们靠轮询 / 读 stdout 等子进程,期间不读 stderr 管道,
    // OpenCascade 一旦往 stderr 刷几十 KB 的警告,管道写满,子进程就会一直卡到超时
    let stderr_file = std::fs::File::create(sandbox.join("stderr.txt")).map_err(io)?;
    let mut cmd = Command::new(&engine.python);
    cmd.args(["-I", "-B", "-X", "utf8"])
        .args(args)
        .current_dir(sandbox)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .env_clear();
    for key in KEPT_ENV {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    // build123d 的依赖链里有库在导入时就要「用户主目录」(parso 算缓存路径用 Path.home()),
    // 环境清空后会直接抛 "Could not determine home directory"。不给它真实的主目录——
    // 把这几个变量都指到沙箱目录:库能跑,它顺手写的缓存也只落在沙箱里。
    // (TMPDIR 是 macOS / Linux 上 Python 找临时目录用的;HOME 同时也是它们的「用户主目录」)
    for key in ["TEMP", "TMP", "TMPDIR", "USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"] {
        cmd.env(key, sandbox);
    }
    quiet(&mut cmd);
    Ok(cmd)
}

pub(crate) fn stderr_tail(sandbox: &Path) -> String {
    let text = std::fs::read(sandbox.join("stderr.txt"))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    text.trim().chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect()
}

pub(crate) fn write_runner(dir: &Path) -> Result<PathBuf, CadError> {
    let runner = dir.join("runner.py");
    std::fs::write(&runner, RUNNER_PY).map_err(|e| CadError::Io(e.to_string()))?;
    Ok(runner)
}

pub(crate) fn job_json(code: &str, out_dir: &Path, opts: &RunOptions) -> serde_json::Value {
    serde_json::json!({
        "code": code,
        "out_dir": out_dir,
        "exports": opts.exports,
        "stl_tolerance": opts.stl_tolerance,
        "stl_angular_tolerance": opts.stl_angular_tolerance,
        "timeout_s": opts.timeout.as_secs(),
    })
}

/// 读执行器写的结果。没有 result.json = 执行器没按约定收尾(崩溃、被杀、磁盘满…)。
pub(crate) fn read_result(out_dir: &Path, sandbox: &Path) -> Result<RunOutput, CadError> {
    match std::fs::read_to_string(out_dir.join("result.json")) {
        Ok(json) => parse_result(&json, out_dir),
        Err(_) => {
            let tail = stderr_tail(sandbox);
            Err(CadError::Protocol(if tail.is_empty() { "no result.json".into() } else { tail }))
        }
    }
}

/// 执行 `code`(冷启动:每次起一个新的解释器)。`work_dir` 必须是调用方新建的空目录;导出的文件落在 `work_dir/out/`。
pub fn run(engine: &Engine, code: &str, work_dir: &Path, opts: &RunOptions) -> Result<RunOutput, CadError> {
    let io = |e: std::io::Error| CadError::Io(e.to_string());
    let out_dir = work_dir.join("out");
    std::fs::create_dir_all(&out_dir).map_err(io)?;
    let runner = write_runner(work_dir)?;
    let job = work_dir.join("job.json");
    std::fs::write(&job, job_json(code, &out_dir, opts).to_string()).map_err(io)?;
    let mut cmd = sandboxed_command(engine, work_dir, &[runner.as_os_str(), job.as_os_str()])?;

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| CadError::Spawn(e.to_string()))?;
    loop {
        match child.try_wait().map_err(io)? {
            Some(_) => break,
            None if started.elapsed() >= opts.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CadError::Timeout(opts.timeout));
            }
            None if opts.cancel.is_cancelled() => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CadError::Cancelled);
            }
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    }
    read_result(&out_dir, work_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_result_is_parsed_and_paths_are_resolved() {
        let json = r#"{"ok":true,"files":{"stl":"model.stl","step":"model.step"},
            "metrics":{"bbox_min":[0,0,0],"bbox_max":[60,40,10],"size":[60,40,10],"volume_mm3":23497.3,
                       "area_mm2":6800.5,"solids":1,"faces":7,"edges":15,"is_valid":true,"center":[0,0,0]},
            "stdout":"hi","elapsed_ms":812}"#;
        let out = parse_result(json, Path::new("/work/out")).unwrap();
        assert_eq!(out.files["stl"], Path::new("/work/out").join("model.stl"));
        assert_eq!(out.metrics.size, [60.0, 40.0, 10.0]);
        assert_eq!((out.metrics.solids, out.metrics.is_valid), (1, true));
        assert_eq!((out.stdout.as_str(), out.elapsed_ms), ("hi", 812));
    }

    #[test]
    fn a_script_failure_becomes_a_structured_error() {
        let json = r#"{"ok":false,"stage":"exec","error_type":"ValueError","message":"Failed creating a fillet",
            "line":14,"traceback":"  line 14: x\nValueError: Failed","stdout":"","elapsed_ms":5}"#;
        let err = parse_result(json, Path::new("/w")).unwrap_err();
        let CadError::Script(e) = &err else { panic!("{err:?}") };
        assert_eq!((e.stage.as_str(), e.line), ("exec", Some(14)));
        assert_eq!(err.code(), "cad_script_error");
    }

    #[test]
    fn a_runner_that_names_files_outside_the_work_dir_is_not_trusted() {
        let json = r#"{"ok":true,"files":{"stl":"..\\..\\evil.stl"},"metrics":{"bbox_min":[0,0,0],"bbox_max":[1,1,1],
            "size":[1,1,1],"volume_mm3":1,"area_mm2":6,"solids":1,"faces":6,"edges":12,"is_valid":true}}"#;
        assert!(matches!(parse_result(json, Path::new("/w")), Err(CadError::Protocol(_))));
        assert!(matches!(parse_result("not json", Path::new("/w")), Err(CadError::Protocol(_))));
        assert!(matches!(parse_result(r#"{"ok":true}"#, Path::new("/w")), Err(CadError::Protocol(_))));
    }

    #[test]
    fn a_missing_interpreter_is_a_spawn_error_not_a_panic() {
        let engine = Engine {
            python: PathBuf::from("Z:/definitely/not/python.exe"),
        };
        let dir = std::env::temp_dir().join(format!("pp-cad-run-test-{}", std::process::id()));
        let err = run(&engine, "result = 1", &dir, &RunOptions::default()).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(err, CadError::Spawn(_)), "{err:?}");
    }
}
