use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use pp_db::Db;

use crate::config_manager::{self, AppConfig, LoadState};
use crate::credentials::{CredentialStore, KeyringStore};

/// 后端共享上下文:`app.manage(Arc<AppCtx>)`,命令里用 `State<'_, Arc<AppCtx>>` 取。
pub struct AppCtx {
    pub db: Arc<Db>,
    pub library_dir: PathBuf,
    /// 应用的本地数据目录(`%LOCALAPPDATA%i.printpilot`)。建模引擎包装在它下面的 `cad-engine/`——
    /// 跟着机器走、不跟着资料库走:资料库可以搬到别的盘或同步盘,几百 MB 的引擎不该跟着搬
    pub data_root: PathBuf,
    /// 接口密钥(系统凭据管理器)。走 trait 是为了测试时能换成内存实现
    pub credentials: Arc<dyn CredentialStore>,
    /// 常驻的建模引擎进程(省掉每次约 4 秒的冷启动)。用到时才起;被杀 / 跑满任务数之后在后台换新
    pub cad_pool: pp_cad::WorkerPool,
    /// 同一时间只许一个「解引擎包」在跑:界面重新挂载会让第二个安装请求紧跟着到,
    /// 两个并发的安装会互相清掉对方解到一半的目录
    pub engine_install: Mutex<()>,
    /// 正在跑的「一轮」(建模对话 / 出图 / 调研),用来中途取消
    pub turns: Arc<crate::turns::Turns>,
    /// 发布包的手机页(临时的局域网页面)。同一时间最多一个;换成 `None` 就停了
    pub share: Mutex<Option<crate::share::ShareHandle>>,
    config_dir: PathBuf,
    config: Mutex<AppConfig>,
    config_state: LoadState,
}

impl AppCtx {
    pub fn new(
        db: Db,
        library_dir: PathBuf,
        data_root: PathBuf,
        config_dir: PathBuf,
        config: AppConfig,
        config_state: LoadState,
    ) -> Self {
        Self {
            db: Arc::new(db),
            library_dir,
            data_root,
            credentials: Arc::new(KeyringStore),
            cad_pool: pp_cad::WorkerPool::new(
                crate::command::cad::scratch_root(),
                crate::command::cad::WORKER_MAX_JOBS,
                crate::command::cad::WORKER_READY_TIMEOUT,
            ),
            engine_install: Mutex::new(()),
            turns: Arc::new(crate::turns::Turns::default()),
            share: Mutex::new(None),
            config_dir,
            config: Mutex::new(config),
            config_state,
        }
    }

    /// 资产的相对路径 → 磁盘绝对路径。`rel_path` 入库时已校验(相对、正斜杠、无 `..`)。
    pub fn asset_path(&self, rel_path: &str) -> PathBuf {
        rel_path
            .split('/')
            .fold(self.library_dir.clone(), |dir, seg| dir.join(seg))
    }

    fn cfg(&self) -> MutexGuard<'_, AppConfig> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn config(&self) -> AppConfig {
        self.cfg().clone()
    }

    pub fn config_writable(&self) -> bool {
        self.config_state != LoadState::Unreadable
    }

    /// 改配置并落盘。落盘失败则内存里的值也不变——界面不会停在一个没保存住的假状态上。
    pub fn update_config(&self, change: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, String> {
        let mut guard = self.cfg();
        let mut next = guard.clone();
        change(&mut next);
        config_manager::save(&self.config_dir, &next, self.config_state)?;
        *guard = next.clone();
        Ok(next)
    }
}
