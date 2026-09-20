//! 应用配置(`<app_config_dir>/config.json`)。安全性质照搬 velo 的 config_manager:
//! - **原子写**:先写 `.tmp` 再 `rename`,断电不会留下半个文件;
//! - **逐字段降级**:坏一个字段不丢整份配置;整个文件解析不了则备份成 `.corrupt` 再用默认值;
//! - **读失败时拒绝写入**:文件存在却读不出来(磁盘/权限问题)时绝不覆盖它。
//!
//! 密钥不在这里——接口密钥只进系统凭据管理器。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "config.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    /// 资料库位置;None = 默认(应用本地数据目录下的 library)
    pub library_dir: Option<String>,
    /// 演示模式:所有供应商适配器返回内置样例,不花钱
    pub demo_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    /// 还没有配置文件(首次启动)
    Missing,
    Ok,
    /// 文件坏了或有字段坏了,已尽量恢复
    Degraded,
    /// 文件存在但读不出来:此后禁止保存,免得用默认值盖掉用户的配置
    Unreadable,
}

pub fn load(config_dir: &Path) -> (AppConfig, LoadState) {
    let path = config_dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (AppConfig::default(), LoadState::Missing)
        }
        Err(e) => {
            log::error!("[config] 读取 {} 失败: {e};本次运行不会写入配置", path.display());
            return (AppConfig::default(), LoadState::Unreadable);
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            let backup = config_dir.join(format!("{FILE}.corrupt"));
            log::error!("[config] 配置文件无法解析({e}),已备份到 {}", backup.display());
            let _ = std::fs::copy(&path, &backup);
            return (AppConfig::default(), LoadState::Degraded);
        }
    };

    let mut state = LoadState::Ok;
    let mut field = |name: &str, ok: bool| {
        if !ok && !value.get(name).is_none_or(serde_json::Value::is_null) {
            log::warn!("[config] 字段 {name} 类型不对,使用默认值");
            state = LoadState::Degraded;
        }
    };
    let library_dir = value.get("library_dir").and_then(|v| v.as_str()).map(str::to_string);
    field("library_dir", library_dir.is_some());
    let demo_mode = value.get("demo_mode").and_then(|v| v.as_bool());
    field("demo_mode", demo_mode.is_some());

    (
        AppConfig {
            library_dir: library_dir.filter(|s| !s.trim().is_empty()),
            demo_mode: demo_mode.unwrap_or(false),
        },
        state,
    )
}

pub fn save(config_dir: &Path, cfg: &AppConfig, loaded_as: LoadState) -> Result<(), String> {
    if loaded_as == LoadState::Unreadable {
        return Err("config file was unreadable at startup; refusing to overwrite it".into());
    }
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let path = config_dir.join(FILE);
    let tmp = config_dir.join(format!("{FILE}.tmp"));
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// 资料库目录:配置里指定的,否则 `<app_local_data_dir>/library`。确保子目录存在。
pub fn resolve_library_dir(cfg: &AppConfig, default_root: &Path) -> std::io::Result<PathBuf> {
    let dir = match &cfg.library_dir {
        Some(custom) => PathBuf::from(custom),
        None => default_root.join("library"),
    };
    for sub in ["assets", "thumbs", "exports", "backups"] {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("pp-config-test-{}", pp_db::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn first_run_has_no_file_and_defaults() {
        let t = TempDir::new();
        assert_eq!(load(&t.0), (AppConfig::default(), LoadState::Missing));
    }

    #[test]
    fn save_then_load_roundtrips_and_leaves_no_tmp_file() {
        let t = TempDir::new();
        let cfg = AppConfig {
            library_dir: Some("D:\\打印资料库".into()),
            demo_mode: true,
        };
        save(&t.0, &cfg, LoadState::Missing).unwrap();
        assert_eq!(load(&t.0), (cfg, LoadState::Ok));
        assert!(!t.0.join("config.json.tmp").exists());
    }

    #[test]
    fn one_bad_field_does_not_lose_the_others() {
        let t = TempDir::new();
        std::fs::write(
            t.0.join(FILE),
            r#"{ "library_dir": "E:\\lib", "demo_mode": "yes please" }"#,
        )
        .unwrap();
        let (cfg, state) = load(&t.0);
        assert_eq!(cfg.library_dir.as_deref(), Some("E:\\lib"));
        assert!(!cfg.demo_mode);
        assert_eq!(state, LoadState::Degraded);
    }

    #[test]
    fn missing_or_null_fields_are_not_a_problem() {
        let t = TempDir::new();
        std::fs::write(t.0.join(FILE), r#"{ "library_dir": null }"#).unwrap();
        assert_eq!(load(&t.0), (AppConfig::default(), LoadState::Ok));
    }

    #[test]
    fn unparseable_file_is_backed_up_before_falling_back() {
        let t = TempDir::new();
        std::fs::write(t.0.join(FILE), "{ this is not json").unwrap();
        let (cfg, state) = load(&t.0);
        assert_eq!((cfg, state), (AppConfig::default(), LoadState::Degraded));
        assert_eq!(
            std::fs::read_to_string(t.0.join("config.json.corrupt")).unwrap(),
            "{ this is not json"
        );
    }

    #[test]
    fn never_overwrites_a_file_it_could_not_read() {
        let t = TempDir::new();
        std::fs::write(t.0.join(FILE), r#"{ "demo_mode": true }"#).unwrap();
        assert!(save(&t.0, &AppConfig::default(), LoadState::Unreadable).is_err());
        assert!(std::fs::read_to_string(t.0.join(FILE)).unwrap().contains("true"));
    }

    #[test]
    fn library_dir_defaults_under_app_data_and_gets_its_subfolders() {
        let t = TempDir::new();
        let dir = resolve_library_dir(&AppConfig::default(), &t.0).unwrap();
        assert_eq!(dir, t.0.join("library"));
        for sub in ["assets", "thumbs", "exports", "backups"] {
            assert!(dir.join(sub).is_dir());
        }
        let custom = t.0.join("elsewhere");
        let cfg = AppConfig {
            library_dir: Some(custom.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(resolve_library_dir(&cfg, &t.0).unwrap(), custom);
    }
}
