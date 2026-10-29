//! `tela-ui` — 视图层：三栏列表、阅读视图、翻译对照、主题与设置。
//!
//! 职责边界（docs/development_plan.md §5）：不直接发 HTTP、不写 SQL。
//!
//! M0（骨架）只提供 crate 边界与 `gui` feature 开关（`gpui-kit` 为可选依赖，
//! 需要 libxkbcommon/vulkan 等系统库与图形会话）。视图入口在 M2
//! 按 docs/development_plan.md §6 的 C11 切片实现。
//!
//! M3 新增 [`reader`]：阅读视图的纯逻辑层（段落映射 / 三视图切换 / 加载态 /
//! 长章节分页），只消费 `tela-content` 的 `Chapter`，不持有任何正文数据。
//! 无显示器环境下（含 CI）可完整跑行为测试，对应计划 A4 的 Q-13 降级方案。

pub mod account;
pub mod reader;
pub mod shell;

#[cfg(feature = "gui")]
pub mod gui;
