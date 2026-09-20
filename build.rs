//! 让 Cargo 知道 `locales/*.json` 是这个 crate 的输入(velo 踩过的坑)。
//!
//! `leptos_i18n` 在**宏展开时**读翻译文件生成强类型 API,而稳定版 Rust 的过程宏无法向 Cargo
//! 声明文件依赖。于是 Cargo 只看 `.rs` 的时间戳:**只改 JSON、一行 Rust 都没动**时,它认为
//! 这个 crate 没变化,直接复用缓存的 WASM——界面上看到的还是旧文案。
//! 一行 `rerun-if-changed` 解决(按目录监听,新增语言文件也算数)。
fn main() {
    println!("cargo:rerun-if-changed=locales");
}
