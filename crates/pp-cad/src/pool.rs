//! 常驻引擎进程的「槽位」:让建模几乎总是落在一个热的进程上。
//!
//! 起一个引擎进程约 4 秒(起解释器 + import build123d),之后每次建模只要几十毫秒。进程会在三种时候没掉:
//! 跑够了 `max_jobs` 个任务(OpenCascade 长跑会涨内存,主动换新)、脚本超时、用户中途取消(后两种是直接杀)。
//! 以前这 4 秒由**下一次建模**来付——正好是用户点了「停止」、改了一句话、再发出去的那一次。
//! 现在换新在后台线程里做:任务的结果先返回,新进程同时在起;这期间来的任务排在槽位的锁后面,
//! 等到的是热进程(最多等完剩下的那点 import 时间),而不是自己再冷启动一遍。
//!
//! 同一时间只有一个进程、一次只跑一个任务:锁在任务执行期间一直握着(建模本来就是一问一答,不需要并发)。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::engine::Engine;
use crate::run::{CadError, RunOptions, RunOutput};
use crate::worker::Worker;

/// 当前常驻进程的情况(给「引擎信息」面板和测试看)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerStatus {
    pub python_version: String,
    pub build123d_version: String,
    /// 这个进程已经跑过几个任务
    pub served: u32,
}

#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<Inner>,
}

struct Inner {
    slot: Mutex<Option<Worker>>,
    /// 每个进程在它下面有一个自己的沙箱目录
    sandbox_root: PathBuf,
    max_jobs: u32,
    ready_timeout: Duration,
    counter: AtomicU32,
    /// 置位期间不在后台起新进程:正在换引擎包——Windows 上正在运行的解释器删不掉
    paused: AtomicBool,
    rewarming: Mutex<Option<JoinHandle<()>>>,
}

/// `WorkerPool::pause` 的凭据:Drop 时恢复后台预热。
pub struct PauseGuard {
    inner: Arc<Inner>,
}

impl Drop for PauseGuard {
    fn drop(&mut self) {
        self.inner.paused.store(false, Ordering::SeqCst);
    }
}

impl Inner {
    fn slot(&self) -> MutexGuard<'_, Option<Worker>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sandbox(&self) -> PathBuf {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        self.sandbox_root.join(format!("worker-{}-{stamp}-{n}", std::process::id()))
    }

    /// 确保槽位里有一个可用的进程(没有就起一个,阻塞到它就绪)。起不来返回 `false`。
    fn ensure(&self, slot: &mut Option<Worker>, engine: &Engine) -> bool {
        if slot.as_ref().is_some_and(|w| w.served() >= self.max_jobs) {
            *slot = None;
        }
        if slot.is_none() {
            let started = Instant::now();
            match Worker::spawn(engine, &self.sandbox(), self.ready_timeout) {
                Ok(w) => {
                    log::info!("[cad] 常驻引擎就绪 · build123d {} · {}ms", w.build123d_version, started.elapsed().as_millis());
                    *slot = Some(w);
                }
                Err(e) => log::warn!("[cad] 常驻引擎起不来:{e}"),
            }
        }
        slot.is_some()
    }
}

impl WorkerPool {
    pub fn new(sandbox_root: PathBuf, max_jobs: u32, ready_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                slot: Mutex::new(None),
                sandbox_root,
                max_jobs: max_jobs.max(1),
                ready_timeout,
                counter: AtomicU32::new(0),
                paused: AtomicBool::new(false),
                rewarming: Mutex::new(None),
            }),
        }
    }

    /// 起好常驻进程(已经有就什么都不做),返回它的情况。起不来返回 `None`。
    pub fn warm(&self, engine: &Engine) -> Option<WorkerStatus> {
        let mut slot = self.inner.slot();
        if !self.inner.ensure(&mut slot, engine) {
            return None;
        }
        slot.as_ref().map(status_of)
    }

    /// 槽位里现在的进程(不会去起新的)。后台正在换新时,这里会等它换完。
    pub fn status(&self) -> Option<WorkerStatus> {
        self.inner.slot().as_ref().map(status_of)
    }

    /// 优先在常驻进程里执行(几十毫秒);常驻进程起不来就退回冷启动(约 4 秒)。
    pub fn run(&self, engine: &Engine, code: &str, work_dir: &Path, opts: &RunOptions) -> Result<RunOutput, CadError> {
        let mut slot = self.inner.slot();
        if !self.inner.ensure(&mut slot, engine) {
            drop(slot);
            return crate::run::run(engine, code, work_dir, opts);
        }
        let mut worker = slot.take().expect("ensure said there is one");
        let result = worker.run(code, work_dir, opts);
        // 脚本自己的错误不影响进程;超时 / 取消 / 协议错误之后进程已经被杀
        let alive = matches!(result, Ok(_) | Err(CadError::Script(_)));
        if alive && worker.served() < self.inner.max_jobs {
            *slot = Some(worker);
        } else {
            drop(slot);
            self.rewarm(engine.clone(), worker);
        }
        result
    }

    /// 在后台换一个新进程:先给旧的收尸(杀进程、删沙箱),再起新的。
    fn rewarm(&self, engine: Engine, old: Worker) {
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new().name("pp-cad-rewarm".into()).spawn(move || {
            drop(old);
            let mut slot = inner.slot();
            // 拿到锁之后再看:`pause()` 是先置位、再去拿锁清槽位的,所以这里要么看得到置位,要么起好的进程会被它清掉
            if inner.paused.load(Ordering::SeqCst) {
                return;
            }
            inner.ensure(&mut slot, &engine);
        });
        match spawned {
            Ok(handle) => *self.inner.rewarming.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle),
            Err(e) => log::warn!("[cad] 后台预热线程起不来(下次建模时再起进程):{e}"),
        }
    }

    /// 停掉常驻进程,并且在返回的凭据活着期间不再后台起新的(换引擎包时用)。
    pub fn pause(&self) -> PauseGuard {
        self.inner.paused.store(true, Ordering::SeqCst);
        *self.inner.slot() = None;
        PauseGuard { inner: self.inner.clone() }
    }

    /// 等后台的换新做完(测试用;应用里没有人需要等它)。
    pub fn wait_rewarm(&self) {
        let handle = self.inner.rewarming.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

fn status_of(w: &Worker) -> WorkerStatus {
    WorkerStatus {
        python_version: w.python_version.clone(),
        build123d_version: w.build123d_version.clone(),
        served: w.served(),
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
        let dir = std::env::temp_dir().join(format!("pp-cad-pool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    const BOX: &str = "from build123d import *\nresult = Box(30, 20, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))\n";
    const RUNAWAY: &str = "while True:\n    pass\n";

    /// 热进程里建一个盒子应该远小于一次冷启动(约 4 秒)。
    fn assert_warm(pool: &WorkerPool, engine: &Engine, dir: &Path) {
        let started = Instant::now();
        pool.run(engine, BOX, dir, &RunOptions::default()).expect("box builds");
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1500), "这一次应该落在热进程上,却花了 {took:?}");
    }

    #[test]
    fn a_killed_worker_is_replaced_in_the_background_so_the_next_job_is_warm() {
        let Some(engine) = engine() else { return };
        let root = scratch("killed");
        let pool = WorkerPool::new(root.join("sandboxes"), 40, Duration::from_secs(90));
        assert_eq!(pool.status(), None, "用到才起");
        pool.run(&engine, BOX, &root.join("job-1"), &RunOptions::default()).expect("first job");
        assert_eq!(pool.status().map(|s| s.served), Some(1));

        let opts = RunOptions {
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let started = Instant::now();
        assert_eq!(pool.run(&engine, RUNAWAY, &root.join("job-2"), &opts), Err(CadError::Timeout(Duration::from_secs(2))));
        assert!(started.elapsed() < Duration::from_millis(3500), "结果不该等新进程起好才返回:{:?}", started.elapsed());

        pool.wait_rewarm();
        assert_eq!(pool.status().map(|s| s.served), Some(0), "后台已经换上了一个新进程");
        assert_warm(&pool, &engine, &root.join("job-3"));

        drop(pool.pause());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_job_also_gets_a_fresh_worker_in_the_background() {
        let Some(engine) = engine() else { return };
        let root = scratch("cancelled");
        let pool = WorkerPool::new(root.join("sandboxes"), 40, Duration::from_secs(90));
        pool.warm(&engine).expect("worker starts");

        let opts = RunOptions::default();
        let flag = opts.cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.cancel();
        });
        assert_eq!(pool.run(&engine, RUNAWAY, &root.join("job-1"), &opts), Err(CadError::Cancelled));
        stopper.join().unwrap();

        pool.wait_rewarm();
        assert_eq!(pool.status().map(|s| s.served), Some(0));
        assert_warm(&pool, &engine, &root.join("job-2"));

        drop(pool.pause());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worker_that_served_its_share_is_recycled_after_the_job_not_before_the_next_one() {
        let Some(engine) = engine() else { return };
        let root = scratch("recycle");
        let pool = WorkerPool::new(root.join("sandboxes"), 2, Duration::from_secs(90));
        pool.run(&engine, BOX, &root.join("job-1"), &RunOptions::default()).expect("job 1");
        // 出错的任务也算数,但不会让进程提前被换掉
        let bad = pool.run(&engine, "from build123d import *\nresult = fillet(Box(5, 5, 5).edges(), radius=50)\n", &root.join("job-2"), &RunOptions::default());
        assert!(matches!(bad, Err(CadError::Script(_))), "{bad:?}");

        pool.wait_rewarm();
        assert_eq!(pool.status().map(|s| s.served), Some(0), "第 2 个任务跑完就在后台换了新进程");
        assert_warm(&pool, &engine, &root.join("job-3"));
        assert_eq!(pool.status().map(|s| s.served), Some(1));

        drop(pool.pause());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn while_paused_nothing_is_started_in_the_background() {
        let Some(engine) = engine() else { return };
        let root = scratch("paused");
        let pool = WorkerPool::new(root.join("sandboxes"), 1, Duration::from_secs(90));
        pool.warm(&engine).expect("worker starts");
        let sandboxes = || std::fs::read_dir(root.join("sandboxes")).map(|d| d.count()).unwrap_or(0);
        assert_eq!(sandboxes(), 1);

        let guard = pool.pause();
        assert_eq!(pool.status(), None, "暂停会停掉常驻进程");
        assert_eq!(sandboxes(), 0, "沙箱跟着进程一起清掉:引擎目录这时没有人占着");
        // 暂停期间前台照样能跑(和以前一样现起一个);它跑满 1 个任务要换新——但后台不许起
        pool.run(&engine, BOX, &root.join("job-1"), &RunOptions::default()).expect("job runs");
        pool.wait_rewarm();
        assert_eq!(pool.status(), None);
        assert_eq!(sandboxes(), 0);

        drop(guard);
        pool.run(&engine, BOX, &root.join("job-2"), &RunOptions::default()).expect("job runs");
        pool.wait_rewarm();
        assert_eq!(pool.status().map(|s| s.served), Some(0), "恢复之后照常后台换新");

        drop(pool.pause());
        let _ = std::fs::remove_dir_all(&root);
    }
}
