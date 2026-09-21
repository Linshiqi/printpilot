//! 后端装配。顺序照搬 velo:single_instance(必须最前)→ 自定义 URI scheme → setup → 插件 → 命令表。

mod command;
mod config_manager;
mod credentials;
mod ctx;
mod engine_pack;
mod turns;

use std::sync::Arc;

use tauri::Manager;

pub use ctx::AppCtx;

/// 必须与 tauri.conf.json 的 identifier 一致(下面有单测守着)。
pub const APP_IDENTIFIER: &str = "ai.printpilot";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        // 本地文件进 WebView:pp-asset://<asset_id>(Windows 上是 http://pp-asset.localhost/<asset_id>)。
        // URL 里只有资产 ID,真实路径由后端查库得到——前端拿不到、也构造不出任意磁盘路径。
        .register_asynchronous_uri_scheme_protocol("pp-asset", |ctx, request, responder| {
            let id = request.uri().path().trim_start_matches('/').to_string();
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || responder.respond(command::asset::serve(&app, &id)));
        })
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let (cfg, cfg_state) = config_manager::load(&config_dir);
            let data_root = app.path().app_local_data_dir()?;
            let library_dir = config_manager::resolve_library_dir(&cfg, &data_root)?;
            let db_path = library_dir.join("printpilot.db");
            let db = pp_db::Db::open(&db_path).map_err(|e| format!("open {}: {e}", db_path.display()))?;
            log::info!(
                "[boot] 资料库 {} · schema v{} · 配置 {:?}",
                library_dir.display(),
                pp_db::SCHEMA_VERSION,
                cfg_state
            );
            command::cad::clean_scratch();
            app.manage(Arc::new(AppCtx::new(db, library_dir, data_root, config_dir, cfg, cfg_state)));
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("printpilot".into()),
                    }),
                ])
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .max_file_size(3_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .level(log::LevelFilter::Info)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            command::system::app_info,
            command::system::log_boot,
            command::system::log_client_error,
            command::system::reveal_library,
            command::system::set_demo_mode,
            command::system::cancel_turn,
            command::project::list_projects,
            command::project::create_project,
            command::project::get_project,
            command::project::update_project,
            command::project::move_project_stage,
            command::project::set_project_status,
            command::project::delete_project,
            command::project::list_stage_events,
            command::project::project_facts_all,
            command::project::project_overview,
            command::asset::list_assets,
            command::asset::delete_asset,
            command::asset::open_asset_external,
            command::mesh::lab_generate_sample,
            command::mesh::import_model,
            command::mesh::mesh_report,
            command::mesh::mesh_scale_to,
            command::mesh::mesh_flatten,
            command::mesh::mesh_mirror,
            command::mesh::export_mesh,
            command::provider::provider_status,
            command::provider::save_provider_key,
            command::provider::delete_provider_key,
            command::provider::test_provider,
            command::research::research_run,
            command::research::list_research_runs,
            command::research::get_research,
            command::research::adopt_opportunity,
            command::cad::cad_engine_info,
            command::cad::cad_engine_install,
            command::cad::cad_export,
            command::design::design_list,
            command::design::design_create,
            command::design::design_get,
            command::design::design_rename,
            command::design::design_delete,
            command::design::design_link_project,
            command::design::design_set_thumb,
            command::design::design_select_version,
            command::design::design_add_image,
            command::design::design_remove_reference,
            command::design::design_send,
            command::design::design_generate,
            command::design::design_set_param,
            command::design::design_run_code,
            command::design::design_review,
            command::imagery::image_provider_info,
            command::imagery::image_settings_get,
            command::imagery::image_settings_set,
            command::imagery::board_list,
            command::imagery::board_create,
            command::imagery::board_get,
            command::imagery::board_rename,
            command::imagery::board_delete,
            command::imagery::board_set_options,
            command::imagery::board_link_project,
            command::imagery::board_select,
            command::imagery::board_adopt,
            command::imagery::board_remove_image,
            command::imagery::board_add_image,
            command::imagery::board_send,
            command::imagery::board_to_design,
            command::imagery::board_export,
            command::pricing::printer_list,
            command::pricing::printer_save,
            command::pricing::printer_delete,
            command::pricing::material_list,
            command::pricing::material_save,
            command::pricing::material_delete,
            command::pricing::cost_defaults_get,
            command::pricing::cost_defaults_set,
            command::pricing::pricing_get,
            command::pricing::pricing_save,
            command::pricing::pricing_summary,
            command::pricing::print_run_add,
            command::pricing::print_run_delete,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PrintPilot");
}

#[cfg(test)]
mod config_consistency_tests {
    //! 配置一致性单测(velo 的做法):打包配置里的约束一旦被改坏,在 `cargo test` 就暴露,
    //! 而不是等到用户机器上才发现。

    fn conf() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json 可解析")
    }

    #[test]
    fn app_version_is_the_same_everywhere() {
        // 发版流程拿 tauri.conf.json 的版本去对标签;安装包的文件名、「关于」里显示的版本都来自这里
        assert_eq!(conf()["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
        let root = include_str!("../../Cargo.toml");
        assert!(root.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))), "根包(前端)的版本没跟上");
    }

    #[test]
    fn the_engine_pack_folder_is_bundled_and_never_empty() {
        // 引擎包随安装包走(单个安装包、用户零安装)。目录里必须始终有个占位文件:
        // 开发构建没有引擎包,空目录会让 tauri-build 报「资源不存在」
        assert_eq!(conf()["bundle"]["resources"]["resources/cad-engine/"].as_str(), Some("cad-engine/"));
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/cad-engine/README.txt").is_file());
        let conf = conf();
        let targets: Vec<&str> = conf["bundle"]["targets"].as_array().unwrap().iter().filter_map(|t| t.as_str()).collect();
        assert!(targets.contains(&"nsis") && targets.contains(&"dmg"), "{targets:?}");
    }

    #[test]
    fn identifier_matches_tauri_conf() {
        assert_eq!(conf()["identifier"].as_str(), Some(super::APP_IDENTIFIER));
    }

    #[test]
    fn release_build_uses_cargo_profile_not_trunk_release() {
        // `trunk build --release` 会跑 wasm-opt,binaryen 拒收 rustc 默认发的 bulk-memory 指令
        let cmd = conf()["build"]["beforeBuildCommand"].as_str().unwrap().to_string();
        assert!(cmd.contains("--cargo-profile release"), "{cmd}");
        assert!(!cmd.split_whitespace().any(|w| w == "--release"), "{cmd}");
    }

    #[test]
    fn dev_url_port_matches_trunk_toml() {
        let trunk = include_str!("../../Trunk.toml");
        let port = trunk
            .lines()
            .find_map(|l| l.trim().strip_prefix("port = "))
            .expect("Trunk.toml 里有 port");
        let dev_url = conf()["build"]["devUrl"].as_str().unwrap().to_string();
        assert!(dev_url.ends_with(&format!(":{port}")), "{dev_url} vs port {port}");
    }

    #[test]
    fn dev_port_stays_away_from_the_tauri_template_default() {
        // 本机有多个 Tauri 工程:模板默认的 1420 以及「顺手 +1 / +10」的那一带最容易撞车
        let dev_url = conf()["build"]["devUrl"].as_str().unwrap().to_string();
        let port: u16 = dev_url.rsplit(':').next().unwrap().parse().expect("devUrl 以端口结尾");
        assert!(!(1400..=1500).contains(&port), "devUrl 端口 {port} 离 Tauri 默认的 1420 太近");
        // 也别落进常见开发服务器的默认端口
        assert!(![3000, 4200, 5000, 5173, 8000, 8080, 8888].contains(&port), "{port} 是别的工具的默认端口");
        // Windows 的动态 / 保留端口段从 49152 开始,落进去可能绑定失败
        assert!(port < 49152, "{port} 在系统动态端口段里");
    }

    #[test]
    fn csp_allows_the_asset_scheme_for_images_and_fetch() {
        let csp = conf()["app"]["security"]["csp"].as_str().unwrap().to_string();
        for directive in ["connect-src", "img-src"] {
            let part = csp
                .split(';')
                .find(|d| d.trim_start().starts_with(directive))
                .unwrap_or_else(|| panic!("CSP 缺少 {directive}"));
            assert!(part.contains("http://pp-asset.localhost"), "{directive}: {part}");
        }
    }

    #[test]
    fn installer_needs_no_admin_and_bundles_webview_bootstrapper() {
        let win = &conf()["bundle"]["windows"];
        assert_eq!(win["nsis"]["installMode"].as_str(), Some("currentUser"));
        assert_eq!(win["webviewInstallMode"]["type"].as_str(), Some("embedBootstrapper"));
    }
}
