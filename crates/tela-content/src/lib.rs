//! `tela-content` —— 正文与译文的数据契约 + 统一内容 API 层。
//!
//! 职责边界：只做正文对象/译文的模型与取回，不做排版、不绘制、不碰 UI 状态机。
//!
//! 统一入口是 [`api::ContentApi`]：
//!
//! - 默认可装配 [`mock::MockContentApi`]（纯内存 fixture，无网络），用于离线开发与 GUI 演示；
//! - 真实服务用 [`http::HttpContentApi`]，由 `AppServices` 把已认证的
//!   `tela_net::ApiClient` 传进来（同一 trait、同一返回结构，调用方不变）。
//!
//! 真实接口：`GET /v1/bodies`、`GET/POST /v1/translations`、`GET /o/t/…`。

pub mod api;
pub mod http;
pub mod markdown;
pub mod mock;
pub mod model;
pub mod webpage;

pub use api::{
    ChapterRequest, ContentApi, ContentApiBackend, content_api_from_env, http_content_api,
    mock_content_api, resolve_backend,
};
pub use model::{
    BlockPage, BodiesResponse, Chapter, ContentBlock, ContentObject, ContentState, ContentStats,
    LeafInfo, RefusedBody, TranslationAnswer, TranslationAnswerStatus, TranslationObject,
    TranslationRow, TranslationState, TranslationStatusResponse, TranslationView,
};
