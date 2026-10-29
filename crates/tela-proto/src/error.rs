//! `TelaError` —— 全 workspace 的唯一错误类型。
//!
//! ## 三段式
//!
//! `Display` 恒为 `来源 → 阶段 → 原因+修复指引`，用 ` → ` 分隔。这是无人值守同步客户端的
//! 硬要求：一行日志要同时说清「哪里」「哪层」「怎么办」。
//!
//! ## 退出码（CLI）
//!
//! | 码 | 语义 | 典型场景 |
//! |---|---|---|
//! | 10 | 本机配置/输入不合法 | 没配 client_id、mutation 超限、fixture 坏了 |
//! | 11 | 本地库失败 | 打开/迁移/读写 |
//! | 12 | 服务端违约 | 响应形状不对、缺字段 |
//! | 13 | 正文/渲染失败 | 没有正文、译文对象坏了 |
//! | 20 | 网络或 API 失败 | 连接失败、5xx、404、invalid_request |
//! | 21 | 需要用户处理 | 未登录、令牌失效、应用被停用、订阅/推荐被拒 |
//! | 22 | 需要等待 | 429、每日额度、`apps_paused` |

use std::fmt;

/// 网络类错误的三段式上下文：第 1 段（URL）与第 2 段（端点/资源名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireContext {
    /// 请求 URL（含 query，便于直接复制复现）。
    pub url: String,
    /// 端点/资源名，例如 `sync`、`mutations`、`me`。
    pub object: String,
}

impl WireContext {
    /// 构造上下文。
    pub fn new(url: impl Into<String>, object: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            object: object.into(),
        }
    }
}

/// 本地库阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreStage {
    /// 打开库文件失败。
    Open,
    /// 迁移失败。
    Migrate,
    /// 行/游标/队列读写失败。
    Query,
}

impl StoreStage {
    /// 三段式第 2 段。
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Migrate => "migrate v2",
            Self::Query => "query",
        }
    }

    /// 三段式第 3 段。
    pub fn advice(self) -> &'static str {
        match self {
            Self::Open => "无法打开本地库；确认目录存在且可写、磁盘有余量",
            Self::Migrate => "迁移失败；旧库是 mock 协议格式，删除库文件后重新登录同步",
            Self::Query => "本地库读写失败；确认磁盘余量与目录权限后重试",
        }
    }
}

/// 协议违约的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolStage {
    /// 字段缺失或类型不符。
    ShapeMismatch,
    /// cursor 不是服务端给的那个。
    Cursor,
    /// 服务端返回了无法理解的结构。
    Unexpected,
}

impl ProtocolStage {
    /// 三段式第 2 段。
    pub fn label(self) -> &'static str {
        match self {
            Self::ShapeMismatch => "wire",
            Self::Cursor => "cursor",
            Self::Unexpected => "response",
        }
    }

    /// 三段式第 3 段。
    pub fn advice(self) -> &'static str {
        match self {
            Self::ShapeMismatch => "字段缺失或类型不符；对照 docs/api/openapi.json 核对响应",
            Self::Cursor => "游标不可用；清空本地游标后重新做一次快照同步",
            Self::Unexpected => "响应不符合 v1 契约；记录 URL 与服务端时间后重试",
        }
    }
}

/// 唯一错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelaError {
    /// 本机配置不合法（缺 client_id、非法 base URL、未配置凭据库）。
    Config {
        /// 原因与修复方式。
        detail: String,
    },
    /// 网络失败（连接、超时、TLS）。
    Net {
        /// 请求上下文。
        ctx: WireContext,
        /// 失败原因。
        detail: String,
    },
    /// 没有可用的登录令牌，或令牌已经无法刷新。
    NotLoggedIn {
        /// 原因与下一步。
        detail: String,
    },
    /// OAuth 端点拒绝（`invalid_grant` / `unauthorized_client` / PKCE 校验失败等）。
    Auth {
        /// 请求上下文。
        ctx: WireContext,
        /// RFC 6749 的 `error` 与说明。
        detail: String,
    },
    /// API 返回了非 2xx 的 JSON 错误。
    Api {
        /// 请求上下文。
        ctx: WireContext,
        /// HTTP 状态码。
        status: u16,
        /// `{ "error": "<code>" }`。
        code: String,
        /// 服务端给的补充说明（可能为空）。
        detail: String,
        /// 429/503 的 `Retry-After`（秒）。
        retry_after: Option<u64>,
    },
    /// 每日额度或封禁类错误（`post_limit` / `feed_limit` / `rate_limited` 等）。
    Limit {
        /// 错误码。
        code: String,
        /// 上限值（服务端给时）。
        limit: Option<i64>,
        /// 额度重置时间（epoch 毫秒）。
        resets_at: Option<i64>,
        /// 说明。
        detail: String,
    },
    /// 本地库失败。
    Store {
        /// 阶段。
        stage: StoreStage,
        /// 库文件路径。
        path: String,
    },
    /// 服务端违约。
    Protocol {
        /// 类别。
        stage: ProtocolStage,
        /// 出问题的字段路径。
        object: String,
    },
    /// 离线 fixture 不合法。
    Fixture {
        /// 文件路径。
        path: String,
        /// 字段路径。
        object: String,
        /// 原因。
        detail: String,
    },
    /// 正文缺失、正文对象不可读或渲染失败。
    Content {
        /// 文章 id。
        article: i64,
        /// 正文对象 key。
        content_key: String,
        /// 原因。
        detail: String,
    },
    /// 单次 push 超过条数上限。
    TooManyMutations {
        /// 实际条数。
        count: usize,
        /// 上限。
        max: usize,
    },
    /// 单次 push 超过字节上限。
    PushTooLarge {
        /// 实际字节。
        bytes: usize,
        /// 上限。
        max: usize,
    },
    /// 一条 mutation 在客户端就不合法（发出去只会被拒）。
    InvalidMutation {
        /// 该 mutation 的 mid。
        mid: String,
        /// 原因。
        detail: String,
    },
    /// 渲染失败。
    Render {
        /// 文章 id。
        article: i64,
        /// 原因。
        detail: String,
    },
}

impl TelaError {
    /// 机器可读码（日志/断言用，禁止解析中文文案）。
    pub fn code(&self) -> &str {
        match self {
            Self::Config { .. } => "config",
            Self::Net { .. } => "net",
            Self::NotLoggedIn { .. } => "not_logged_in",
            Self::Auth { .. } => "auth",
            Self::Api { code, .. } => code,
            Self::Limit { code, .. } => code,
            Self::Store { .. } => "store",
            Self::Protocol { .. } => "protocol",
            Self::Fixture { .. } => "fixture",
            Self::Content { .. } => "content",
            Self::TooManyMutations { .. } => "too_many_mutations",
            Self::PushTooLarge { .. } => "push_too_large",
            Self::InvalidMutation { .. } => "invalid_mutation",
            Self::Render { .. } => "render",
        }
    }

    /// 线上对应的 HTTP 状态码；`0` 表示没有 HTTP 响应。
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Api { status, .. } => *status,
            Self::Auth { .. } => 400,
            _ => 0,
        }
    }

    /// 是否值得原样重试（网络失败与 5xx）。
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Net { .. } => true,
            // `apps_paused` 是「等运维恢复」，不是可以立刻重试的瞬时故障；
            // 其余 5xx（`unavailable`、无 body 的 500/502）才值得退避重试。
            Self::Api { status, code, .. } => *status >= 500 && code != "apps_paused",
            _ => false,
        }
    }

    /// 是否需要用户重新登录。
    pub fn needs_login(&self) -> bool {
        matches!(self, Self::NotLoggedIn { .. })
            || matches!(self, Self::Auth { detail, .. } if detail.contains("invalid_grant"))
            || matches!(self, Self::Api { status: 401, .. })
    }

    /// CLI 退出码。
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config { .. }
            | Self::Fixture { .. }
            | Self::TooManyMutations { .. }
            | Self::PushTooLarge { .. }
            | Self::InvalidMutation { .. } => 10,
            Self::Store { .. } => 11,
            Self::Protocol { .. } => 12,
            Self::Content { .. } | Self::Render { .. } => 13,
            Self::Net { .. } => 20,
            Self::NotLoggedIn { .. } | Self::Auth { .. } => 21,
            Self::Limit { .. } => 22,
            Self::Api { status, .. } => match status {
                401 | 403 => 21,
                429 | 503 => 22,
                _ => 20,
            },
        }
    }
}

fn retry_hint(retry_after: Option<u64>) -> String {
    match retry_after {
        Some(seconds) => format!("等待 {seconds} 秒后重试"),
        None => "稍后重试".to_string(),
    }
}

impl fmt::Display for TelaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config { detail } => write!(f, "config → local → {detail}"),
            Self::Net { ctx, detail } => write!(
                f,
                "{} → {} → 请求失败：{}；检查网络、代理与系统时间后重试",
                ctx.url, ctx.object, detail
            ),
            Self::NotLoggedIn { detail } => {
                write!(f, "config → login → {detail}；先运行 `tela-app login`")
            }
            Self::Auth { ctx, detail } => write!(
                f,
                "{} → {} → 授权失败：{}；重新运行 `tela-app login`",
                ctx.url, ctx.object, detail
            ),
            Self::Api {
                ctx,
                status,
                code,
                detail,
                retry_after,
            } => {
                let retry = if *status == 429 || *status == 503 {
                    format!("；{}", retry_hint(*retry_after))
                } else {
                    String::new()
                };
                if detail.is_empty() {
                    write!(
                        f,
                        "{} → {} → HTTP {status} {code}{retry}",
                        ctx.url, ctx.object
                    )
                } else {
                    write!(
                        f,
                        "{} → {} → HTTP {status} {code}：{detail}{retry}",
                        ctx.url, ctx.object
                    )
                }
            }
            Self::Limit {
                code,
                limit,
                resets_at,
                detail,
            } => {
                let mut extra = Vec::new();
                if let Some(limit) = limit {
                    extra.push(format!("上限 {limit}"));
                }
                if let Some(resets_at) = resets_at {
                    extra.push(format!("{resets_at} 重置"));
                }
                let extra = if extra.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", extra.join("，"))
                };
                write!(f, "quota → {code} → {detail}{extra}")
            }
            Self::Store { stage, path } => {
                write!(f, "{} → {} → {}", path, stage.label(), stage.advice())
            }
            Self::Protocol { stage, object } => {
                write!(f, "{} → {} → {}", stage.label(), object, stage.advice())
            }
            Self::Fixture {
                path,
                object,
                detail,
            } => write!(f, "{} → {} → {}；修正 fixture 后重跑", path, object, detail),
            Self::Content {
                article,
                content_key,
                detail,
            } => write!(
                f,
                "article {article} → /v1/bodies/{content_key} → {detail}；\
                 确认该文章有正文（contentKey 非空）且今日额度未用尽"
            ),
            Self::TooManyMutations { count, max } => write!(
                f,
                "push → mutations → {count} 条超过单次上限 {max}；拆成多个批次"
            ),
            Self::PushTooLarge { bytes, max } => {
                write!(f, "push → body → {bytes} 字节超过上限 {max}；拆成多个批次")
            }
            Self::InvalidMutation { mid, detail } => {
                write!(f, "push → {mid} → {detail}；修正后再入队")
            }
            Self::Render { article, detail } => {
                write!(
                    f,
                    "article {article} → render → {detail}；检查块 HTML 是否可渲染"
                )
            }
        }
    }
}

impl std::error::Error for TelaError {}
