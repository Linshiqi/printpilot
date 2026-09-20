//! 常驻的引擎进程(`runner.py --serve`)。
//!
//! 一次冷启动执行约 4 秒,几乎全是「起解释器 + import build123d」;同一个进程里连续建模只要几十毫秒。
//! 参数面板「改个数字就重建」要的是后者。隔离措施和冷启动完全一样(见 `run::sandboxed_command`),
//! 另外靠执行器自己保证任务之间不串味:每个任务全新的命名空间、AST 层禁止给属性赋值、`math` 每任务一份替身。
//!
//! 协议:stdin 每行一个任务(JSON,带 `seq`);任务跑完,子进程往 stdout 写一行含 `\x1ePP-DONE <seq>` 的文字,
//! 结果照旧在 `<out_dir>/result.json`。超时 → 直接杀掉这个进程(调用方下次再起一个)。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::engine::Engine;
use crate::run::{job_json, read_result, sandboxed_command, stderr_tail, write_runner, CadError, RunOptions, RunOutput};

/// 与 `runner.py` 里的 `SENTINEL` 一致。按「这一行里有没有它」来认协议行:
/// OpenCascade 偶尔会绕过 Python 直接往 stdout 写东西,可能和协议行挤在同一行里。
const SENTINEL: &str = "\u{1e}PP-";

pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// 这个进程自己的沙箱目录(工作目录、TEMP、HOME 都指向它);进程结束时删掉
    sandbox: PathBuf,
    seq: u64,
    served: u32,
    pub python_version: String,
    pub build123d_version: String,
}

impl Worker {
    /// 起一个常驻进程,等它报告就绪(import build123d 要几秒)。`sandbox` 必须是调用方新建的空目录。
    pub fn spawn(engine: &Engine, sandbox: &Path, ready_timeout: Duration) -> Result<Worker, CadError> {
        std::fs::create_dir_all(sandbox).map_err(|e| CadError::Io(e.to_string()))?;
        let runner = write_runner(sandbox)?;
        let mut cmd = sandboxed_command(engine, sandbox, &[runner.as_os_str(), std::ffi::OsStr::new("--serve")])?;
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| CadError::Spawn(e.to_string()))?;
        let stdin = child.stdin.take().ok_or_else(|| CadError::Spawn("no stdin pipe".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| CadError::Spawn("no stdout pipe".into()))?;

        // 读 stdout 的线程:阻塞读没法带超时,所以交给一个线程读、主线程在通道上带超时地等。
        // 子进程退出 → 读到 EOF → 线程结束 → 通道断开。
        let (tx, lines) = mpsc::channel();
        std::thread::Builder::new()
            .name("pp-cad-worker-stdout".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| CadError::Spawn(e.to_string()))?;

        let mut worker = Worker {
            child,
            stdin,
            lines,
            sandbox: sandbox.to_path_buf(),
            seq: 0,
            served: 0,
            python_version: String::new(),
            build123d_version: String::new(),
        };
        let ready = worker.wait_for("READY", ready_timeout).map_err(|e| match e {
            CadError::Timeout(_) => CadError::Spawn("engine did not become ready in time".into()),
            other => other,
        })?;
        let mut parts = ready.split_whitespace();
        worker.python_version = parts.next().unwrap_or("").to_string();
        worker.build123d_version = parts.next().unwrap_or("").to_string();
        Ok(worker)
    }

    /// 等一行含 `<SENTINEL><tag>` 的输出,返回标记后面的内容。超时 / 进程没了都会先把进程杀掉。
    fn wait_for(&mut self, tag: &str, timeout: Duration) -> Result<String, CadError> {
        let marker = format!("{SENTINEL}{tag}");
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(at) = line.find(&marker) {
                        return Ok(line[at + marker.len()..].trim().to_string());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.kill();
                    return Err(CadError::Timeout(timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.kill();
                    let tail = stderr_tail(&self.sandbox);
                    return Err(CadError::Protocol(if tail.is_empty() { "engine process exited".into() } else { tail }));
                }
            }
        }
    }

    /// 在这个常驻进程里执行 `code`。`work_dir` 是这个任务自己的目录,导出的文件落在 `work_dir/out/`。
    /// 返回 `Timeout` / `Protocol` / `Io` 之后这个 Worker 就不能再用了(进程已被杀),调用方应当丢掉它。
    pub fn run(&mut self, code: &str, work_dir: &Path, opts: &RunOptions) -> Result<RunOutput, CadError> {
        let out_dir = work_dir.join("out");
        std::fs::create_dir_all(&out_dir).map_err(|e| CadError::Io(e.to_string()))?;
        self.seq += 1;
        let mut job = job_json(code, &out_dir, opts);
        job["seq"] = serde_json::json!(self.seq);

        let sent = writeln!(self.stdin, "{job}").and_then(|()| self.stdin.flush());
        if let Err(e) = sent {
            self.kill();
            return Err(CadError::Protocol(format!("engine process is gone: {e}")));
        }
        let done = self.wait_for("DONE", opts.timeout)?;
        if done != self.seq.to_string() {
            self.kill();
            return Err(CadError::Protocol(format!("engine answered job {done}, expected {}", self.seq)));
        }
        self.served += 1;
        read_result(&out_dir, &self.sandbox)
    }

    /// 已经跑过几个任务。调用方用它决定什么时候换一个新进程(OpenCascade 长跑会涨内存)。
    pub fn served(&self) -> u32 {
        self.served
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill();
        // Windows 上进程刚结束的那一瞬间,它占着的目录(当前目录、刚写完的文件)可能还没放开:重试几次
        for attempt in 0..10 {
            match std::fs::remove_dir_all(&self.sandbox) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) if attempt == 9 => log::warn!("[cad] 沙箱目录 {} 没删掉:{e}", self.sandbox.display()),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 需要真引擎;没装时自动跳过。

    use super::*;

    fn engine() -> Option<Engine> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let found = Engine::locate(&Path::new(&local).join("ai.printpilot"));
        if found.is_none() {
            eprintln!("(skipped: no CAD engine installed — run scripts/setup-cad-engine.ps1)");
        }
        found
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pp-cad-worker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    const BOX: &str = "from build123d import *\nresult = Box(30, 20, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))\n";

    #[test]
    fn a_warm_worker_runs_jobs_back_to_back_far_faster_than_a_cold_start() {
        let Some(engine) = engine() else { return };
        let root = scratch("warm");
        let mut worker = Worker::spawn(&engine, &root.join("sandbox"), Duration::from_secs(90)).expect("worker starts");
        assert!(worker.build123d_version.starts_with("0."), "{:?}", worker.build123d_version);
        assert!(worker.python_version.starts_with("3."), "{:?}", worker.python_version);

        let opts = RunOptions::default();
        let first = worker.run(BOX, &root.join("job-1"), &opts).expect("first job");
        assert_eq!(first.metrics.size, [30.0, 20.0, 10.0]);
        assert!(first.files["stl"].is_file() && first.files["step"].is_file());

        // 出错的任务不影响后面的任务
        let bad = worker.run("from build123d import *\nresult = fillet(Box(5, 5, 5).edges(), radius=50)\n", &root.join("job-2"), &opts);
        assert!(matches!(bad, Err(CadError::Script(ref e)) if e.stage == "exec" && e.line == Some(2)), "{bad:?}");
        let refused = worker.run("import os\nresult = 1\n", &root.join("job-3"), &opts);
        assert!(matches!(refused, Err(CadError::Script(ref e)) if e.stage == "validate"), "{refused:?}");

        let started = Instant::now();
        let again = worker.run(&BOX.replace("30", "45"), &root.join("job-4"), &opts).expect("fourth job");
        let warm = started.elapsed();
        assert_eq!(again.metrics.size[0], 45.0);
        assert!(warm < Duration::from_millis(1500), "热进程里重建一个盒子花了 {warm:?}");
        assert_eq!(worker.served(), 4);

        let sandbox = root.join("sandbox");
        drop(worker);
        assert!(!sandbox.exists(), "进程结束后沙箱目录要清掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_runaway_job_is_killed_and_the_worker_reports_a_timeout() {
        let Some(engine) = engine() else { return };
        let root = scratch("timeout");
        let mut worker = Worker::spawn(&engine, &root.join("sandbox"), Duration::from_secs(90)).expect("worker starts");
        let opts = RunOptions {
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let res = worker.run("while True:\n    pass\n", &root.join("job-1"), &opts);
        assert_eq!(res, Err(CadError::Timeout(Duration::from_secs(2))));
        // 进程已经被杀:再用它只会得到协议错误,而不是卡住
        let after = worker.run(BOX, &root.join("job-2"), &RunOptions::default());
        assert!(matches!(after, Err(CadError::Protocol(_))), "{after:?}");
        drop(worker);
        let _ = std::fs::remove_dir_all(&root);
    }
}
