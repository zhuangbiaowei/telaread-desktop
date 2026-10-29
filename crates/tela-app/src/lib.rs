//! `tela-app` 的库入口：装配层（S1）。
//!
//! 二进制 `tela-app`（终端）与 `tela-gui`（GPUI）都只负责“解析参数 → 调库”；
//! `Store` / `ContentApi` / `SyncTransport` 的打开与组合集中在这里，
//! 避免两个宿主各写一套初始化逻辑。
//!
//! 边界：本 crate 负责装配，不实现视图；视图语义在 `tela-ui`，
//! 同步/存储语义分别在 `tela-sync` / `tela-store`。

pub mod services;

pub use services::{AppConfig, AppServices};
