//! 真实内容 API：`/v1/bodies`、`/v1/translations`、`/o/t/…`。
//!
//! 一次阅读最多三次请求：
//! 1. `GET /v1/bodies?keys=<key>` 取正文（不可变，调用方可缓存）；
//! 2. `GET /v1/translations/<key>/<lang>` 取译本状态；
//! 3. 若已有译本，`GET <translation.object>` 取译本对象。
//!
//! 正文被每日额度拒绝时返回 `TelaError::Limit`，界面据此展示「什么时候再能打开、
//! 可以订阅该博客」而不是笼统的网络错误。

use std::sync::Arc;

use tela_net::ApiClient;
use tela_proto::{TelaError, WireContext};

use crate::api::{ChapterRequest, ContentApi};
use crate::model::{
    BodiesResponse, Chapter, TranslationAnswer, TranslationObject, TranslationRow,
    TranslationStatusResponse,
};

/// 真实内容后端。
pub struct HttpContentApi {
    api: Arc<ApiClient>,
}

impl HttpContentApi {
    /// 用已认证客户端装配。
    pub fn new(api: Arc<ApiClient>) -> Self {
        Self { api }
    }

    /// 当前 API 基址。
    pub fn base_url(&self) -> &str {
        self.api.base_url()
    }
}

impl ContentApi for HttpContentApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        let key = normalize_content_key(&request.content_key);
        if key.is_empty() {
            return Err(TelaError::Content {
                article: 0,
                content_key: request.content_key.clone(),
                detail: "contentKey 为空".into(),
            });
        }

        let url = format!("/v1/bodies?keys={}", urlencoding::encode(&key));
        let bodies: BodiesResponse = self.api.get_json(&url)?;

        if let Some(refused) = bodies.refused.get(&key) {
            return Err(TelaError::Limit {
                code: refused.error.clone(),
                limit: Some(refused.limit),
                resets_at: Some(refused.resets_at),
                detail: format!(
                    "这篇来自未订阅的博客，今天的 {} 篇额度已用完；订阅该博客后不再受限",
                    refused.limit
                ),
            });
        }

        let content = bodies
            .objects
            .get(&key)
            .cloned()
            .ok_or_else(|| TelaError::Content {
                article: 0,
                content_key: key.clone(),
                detail: "Tela 没有这篇正文（contentKey 不存在或正文抽取失败）".into(),
            })?;

        let translation = self.translation_view(&content, &request.lang)?;
        Ok(Chapter::new(content, translation))
    }

    fn request_translation(
        &self,
        article_id: i64,
        lang: &str,
    ) -> Result<TranslationAnswer, TelaError> {
        self.api.post_json(
            "/v1/translations",
            &serde_json::json!({ "articleId": article_id, "lang": lang }),
        )
    }

    fn translation_status(
        &self,
        content_key: &str,
        lang: &str,
    ) -> Result<Option<TranslationRow>, TelaError> {
        let path = translation_status_path(content_key, lang);
        let response: TranslationStatusResponse = self.api.get_json(&path)?;
        Ok(response.translation)
    }

    fn load_translation_object(&self, path: &str) -> Result<TranslationObject, TelaError> {
        self.api.get_json(path)
    }

    fn load_image(&self, src: &str) -> Result<Vec<u8>, TelaError> {
        // `/img/…` 是相对路径，ApiClient 会拼到 API 基址上并带 Bearer；
        // 服务端也有直接把图片写成绝对 URL 的情况，get_bytes 原样使用。
        self.api.get_bytes(src)
    }
}

/// 正文 key 归一化：接受裸 key 或旧的 `c/<key>.json` 形状。
pub fn normalize_content_key(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("c/").unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix(".json").unwrap_or(trimmed);
    trimmed.to_string()
}

/// `GET /v1/translations/<key>/<lang>` 的路径。
pub fn translation_status_path(content_key: &str, lang: &str) -> String {
    format!(
        "/v1/translations/{}/{}",
        urlencoding::encode(content_key),
        urlencoding::encode(lang)
    )
}

/// 正文对象地址（`/o/c/<key>.json`，单独取一篇时用）。
pub fn content_object_path(content_key: &str) -> String {
    format!("/o/c/{}.json", urlencoding::encode(content_key))
}

/// 给错误补上正文上下文（测试/诊断用）。
pub fn content_context(content_key: &str) -> WireContext {
    WireContext::new(
        format!(
            "{}/v1/bodies?keys={content_key}",
            tela_proto::DEFAULT_API_BASE_URL
        ),
        "bodies",
    )
}
