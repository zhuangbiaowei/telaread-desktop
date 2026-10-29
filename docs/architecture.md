# Tela 桌面版架构设计

> **2026-10-28 更新（R1）**：官方 API v1 到位后，协议/同步/正文/认证已按真实接口重写
> （见 [`real-api.md`](./real-api.md)）。本文中引用 `/api/v1/sync`、`seq`、tombstone、
> `x-tela-client` 的段落是真实 API 就绪前的设计，**仅作历史参照**；当前 wire 契约以
> `docs/api/openapi.json` 与 `crates/tela-proto` 为准。

> 版本：0.1（设计稿）
> 技术栈：Rust 2024 + GPUI + gpui-kit 0.8.x
> 目标平台：macOS 15+、Windows 10+、Linux（Wayland / X11 + Vulkan）
> 配套文档：[`api-proposal.md`](./api-proposal.md)（协议）、[`product-design.md`](./product-design.md)（产品）

---

## 1. 定位与约束

### 1.1 这个应用是什么

一个**本地优先**的 Tela 桌面阅读器：把 Tela 的多语言独立博客阅读体验搬到桌面上，
在网络不稳、甚至完全离线时仍然可读；联网后自动把本地状态与服务端对齐。

它不是：

- 一个 RSS 阅读器的重新实现（不自己抓取 RSS，一切内容来自 Tela API）；
- 一个网页壳（不用 WebView 渲染正文）；
- 一个"下载器"（不做内容再分发）。

### 1.2 硬约束

| 约束 | 来源 | 影响 |
|---|---|---|
| 服务端协议是"单 `seq` 游标 + 幂等 mutation" | Tela ADR 0025 | 客户端必须实现 confirmed ⊕ pending 的视图模型 |
| 正文是**不可变内容寻址对象** | Tela ADR 0022 | 缓存可以永久有效；预取安全 |
| 正文对象是**按顶层块切分的 HTML** | `packages/content/src/object.ts` | 双语对照、翻译流式叠加都按块索引 |
| 高亮锚点是 `leafId + 字符偏移 + 上下文` | `packages/sync/src/rows.ts` | 必须能定位到叶子并做重锚 |
| 只有 4 种阅读语言 | `READING_LANGUAGES` | 翻译与字体回退可枚举 |
| GPUI 是 pinned 快照 | gpui-kit 0.8.0 → `gpui-pre 0.3.9` | 不能随便升；需要一层薄封装隔离 |
| Rust 1.92+ / edition 2024 | gpui-kit 安装文档 | CI 与开发环境固定版本 |
| Linux 需要 Vulkan + Wayland/X11 | gpui-kit 安装文档 | 发布说明要写清 |

### 1.3 质量目标

| 指标 | 目标 |
|---|---|
| 冷启动到可交互 | < 600 ms（SSD，2 万篇文章的库） |
| 打开已缓存文章 | < 50 ms |
| 列表滚动 | 稳定 120 fps（目标），不低于 60 fps |
| 内存占用（1 万篇文章 + 200 篇正文缓存） | < 300 MB |
| 同步一次增量（100 行） | < 300 ms（不含网络） |
| 崩溃率 | < 0.1% 会话 |
| 离线可用 | 已缓存的列表、正文、翻译、高亮、笔记全部可读可写 |

---

## 2. 总体架构

### 2.1 进程与线程

单进程：一条 UI 线程、一个网络 tokio runtime、一个数据库线程、一个图片线程池；
同步引擎是跑在 UI 执行器上的任务（只做编排，不做 I/O）。

```
┌──────────────────────────────────────────────────────────────┐
│ GPUI 主线程（UI）                                              │
│  · 所有 Entity / 渲染 / 输入 / 动画                            │
│  · 只读 Arc<Tables> 快照，不做 I/O、不做重计算                  │
└───────────────▲──────────────────────────┬───────────────────┘
                │ 事件（channel → cx.spawn）│ 命令（async）
┌───────────────┴──────────────┐  ┌────────▼───────────────────┐
│ Sync Engine（GPUI 任务）      │  │ Net（tokio runtime 线程）    │
│  · 拉取/推送状态机            │──▶│  · reqwest + rustls        │
│  · 退避、去抖、生命周期        │◀──│  · 对象/图片下载、翻译轮询   │
└───────────────┬──────────────┘  └────────────┬───────────────┘
                │ 读写                          │ 图片字节
┌───────────────▼──────────────┐  ┌────────────▼───────────────┐
│ Store（SQLite 专用线程）      │  │ Image pipeline（线程池）    │
│  · WAL，单写者                 │  │  · 解码、缩放、LRU          │
│  · 事务、迁移、FTS             │  │                            │
└──────────────────────────────┘  └────────────────────────────┘
```

**为什么网络要独立 tokio runtime**：

- GPUI 的异步执行器用于 UI 任务，不提供完整的 I/O reactor；
- `reqwest` 依赖 tokio 的 I/O driver，必须运行在 tokio runtime 上下文里；
- 把网络全部放进 `runtime.spawn(...)`，结果通过 channel 回到 GPUI 任务，边界清晰、不会阻塞渲染。

**为什么 SQLite 要专用线程**：

- `rusqlite` 是同步 API；在 GPUI 主线程执行会掉帧；
- SQLite 单写者模型天然适合"一个写线程 + 多个读连接（WAL）"；
- 事务边界与 reducer 的"一次 pull 一个事务"对齐。

### 2.2 数据流

```
                 ┌──────────────┐
   HTTP ─────────▶│  wire types  │  serde（tela-proto）
                 └──────┬───────┘
                        ▼
                 ┌──────────────┐
                 │  reducer     │  apply_pull / apply_mutation / view / settle
                 └──────┬───────┘
                        ▼
        ┌───────────────┴────────────────┐
        ▼                                ▼
┌──────────────┐                 ┌──────────────┐
│ SQLite       │                 │ Arc<Tables>  │  内存快照
│ confirmed    │                 │  view 快照    │
│ pending      │                 └──────┬───────┘
└──────────────┘                        ▼
                                 ┌──────────────┐
                                 │ GPUI views   │  纯渲染
                                 └──────────────┘
```

**关键规则**：UI 永远只读 `view`（confirmed ⊕ pending）。任何"本地先变、后台再同步"的交互
（标已读、点赞、高亮）都先写 pending，再立即用 `view` 重绘，UI 不等网络。

### 2.3 Workspace 与依赖方向

```
telaread-desktop/
├── Cargo.toml                 # workspace
├── crates/
│   ├── tela-proto/            # 线协议类型、常量、错误码（无 I/O）
│   ├── tela-store/            # SQLite schema、迁移、行映射、FTS
│   ├── tela-sync/             # reducer、拉取/推送引擎、调度
│   ├── tela-net/              # HTTP、认证、对象、图片、重试
│   ├── tela-content/          # 块模型、HTML 子集、锚点、翻译配对
│   ├── tela-ui/               # 视图、组件、主题、快捷键
│   └── tela-app/              # main、窗口、全局状态、生命周期、更新
└── docs/
```

依赖方向（严格单向）：

```
tela-app ──▶ tela-ui ──▶ tela-content ──▶ tela-proto
    │            │            │
    │            └──▶ tela-sync ──▶ tela-store
    │                     │
    └──────────────▶ tela-net ──▶ tela-proto
```

规则：

- `tela-proto` 不依赖任何内部 crate，也不做 I/O。
- `tela-store` 不知道 HTTP；`tela-net` 不知道 SQLite。
- `tela-sync` 是唯一同时依赖 store 与 net 的编排层。
- `tela-ui` 不直接发请求；它通过 `AppServices`（trait）调用命令。

---

## 3. 技术选型

| 领域 | 选择 | 理由 | 备选 |
|---|---|---|---|
| GUI | `gpui-kit = "0.8"` | 官方推荐单依赖；含 75+ 组件、虚拟列表、富文本、主题 | 直接用 `gpui-pre`（API 更底层，工作量大） |
| HTTP | `reqwest`（rustls, http2, stream） | 成熟、连接池、超时/重试可控 | GPUI 内置 `http_client`（能力较弱，cookie 不便） |
| 序列化 | `serde` + `serde_json` | 与 wire 格式一致 | `simd-json`（收益有限） |
| SQLite | `rusqlite`（bundled） | 同步 API 适合专用线程；可控制 pragma、FTS5 | `sqlx`（异步、编译期检查，但运行时与复杂度更高） |
| 时间 | `time`（UTC + 格式化） | 明确的时区语义 | `chrono` |
| ID | `uuid` v7 | mutation `mid` 可按时间排序 | 自增 |
| 日志 | `tracing` + `tracing-appender` | 结构化、可分层过滤 | `log` |
| 错误 | `thiserror`（库）+ `anyhow`（应用层） | 边界清晰 | — |
| i18n | `rust-i18n` | gpui-kit 官方配套 | `fluent`（更重） |
| 字体 | 系统字体 + 打包拉丁字体 | 控制体积与许可 | 打包 Noto CJK（~20MB/字重） |
| HTML 解析 | `scraper`（内部 html5ever + ego-tree） | 与 gpui-base 同源、容错好；能拿到元素/文本/属性，够渲染用 | `html5ever` + 自写 TreeSink（更底层） |
| 图片解码 | `image`（png/jpeg/webp） | 纯 Rust，够用 | `zune-image`（更快，生态小） |
| 本地搜索 | SQLite FTS5 | 零额外依赖 | `tantivy`（重） |
| 更新 | `self_update`（便携）/ 平台安装器 | gpui-kit 官方文档推荐 | 自建更新服务 |
| 测试 | 内置 `#[gpui_kit::test]` + `insta` 快照 | UI 集成测试 + golden 文件 | — |

### 3.1 版本锁定

```toml
# Cargo.toml（workspace）
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.92"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
gpui-kit = "0.8.0"          # 它自己把 gpui-pre 钉在 =0.3.9
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.41", features = ["bundled", "backup", "functions"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls", "http2", "json", "stream", "gzip", "cookies"] }
tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros"] }
tracing = "0.1"
im = "15"                   # 持久化 Map，便于快照克隆
keyring = "4"               # 系统钥匙串
scraper = "0.29"           # html5ever + ego-tree，解析块 HTML
image = "0.25"
uuid = { version = "1", features = ["v7"] }
thiserror = "2"
anyhow = "1"
```

> 注意：`gpui-kit` 对 GPUI 的依赖是**精确版本**（`=0.3.9`）。不要手动 `cargo update` 到更新的
> `gpui-pre`，否则组件层可能编译不过。升级 gpui-kit 时整体升级并跑 UI 测试。

### 3.2 开发环境（Linux / Omarchy）

gpui-kit 官方脚本面向 Ubuntu 24.04；Arch 系（Omarchy）的近似依赖：

```bash
sudo pacman -S --needed base-devel clang cmake fontconfig \
  wayland libxkbcommon libxkbcommon-x11 libx11 libxcb \
  vulkan-icd-loader vulkan-radeon   # NVIDIA 用 vulkan-nvidia
rustup default stable                # 需要 >= 1.92
```

- 必须在 **Wayland 或 X11 图形会话**里运行；纯 SSH 会打不开窗口。
- 需要可用的 **Vulkan 驱动**（`vulkaninfo` 能跑通），只有 loader 不够。
- 可选：`libasound2`（语音功能）、`webkit2gtk-4.1`（只有用 `gpui-webview` 才需要）。

---

## 4. Crate 设计

### 4.1 `tela-proto`：线协议

职责：**只做类型与常量**，与 `packages/sync/src/*.ts` 一一对应。

```rust
// crates/tela-proto/src/lib.rs
pub const MIN_CLIENT: u32 = 3;
pub const CLIENT_VERSION: u32 = 3;
pub const HORIZON_DAYS: i64 = 30;
pub const PAGE_ROWS: usize = 1000;
pub const MAX_MUTATIONS_PER_PUSH: usize = 50;
pub const TITLE_IDS_MAX: usize = 1000;

pub const CLIENT_HEADER: &str = "x-tela-client";
pub const MEMBER_HEADER: &str = "x-tela-member";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRows {
    pub profile: Vec<ProfileRow>,
    pub prefs: Vec<PrefRow>,
    pub subscriptions: Vec<SubscriptionRow>,
    pub feeds: Vec<FeedRow>,
    pub sites: Vec<SiteRow>,
    pub articles: Vec<ArticleRow>,
    pub titles: Vec<TitleRow>,
    pub states: Vec<StateRow>,
    pub recommendations: Vec<RecommendationRow>,
    pub highlights: Vec<HighlightRow>,
    pub claims: Vec<ClaimRow>,
    pub translations: Vec<TranslationRow>,
    pub follows: Vec<FollowRow>,
    pub folders: Vec<FolderRow>,
    pub feed_folders: Vec<FeedFolderRow>,
    pub blogrolls: Vec<BlogrollRow>,
    pub blogroll_blogs: Vec<BlogrollBlogRow>,
}
```

设计要求：

1. **所有 `Option<T>` 对应"字段可能缺失"**，不能把 `null` 与缺失混为一谈。
   例如 `ProfileRow.avatar: Option<Option<String>>` 太丑；用一个 `Maybe<T>` 或
   `#[serde(default, skip_serializing_if = "Option::is_none")]` 的 `Option<T>` 加显式处理
   （见 7.1）。**推荐**：为 `profile` 单独定义 `ProfilePatch`，用 `Option<Option<T>>` 表达三方状态。
2. 未知枚举值不 panic：`status: String` + 访问器，而不是 `enum`（除非加 `#[serde(other)]`）。
3. `seq` 用 `i64`，`at` 用 `i64`（epoch ms）。
4. 所有结构体实现 `PartialEq`，便于测试与 diff。

### 4.2 `tela-store`：本地库

职责：schema、迁移、行 upsert/删除、游标、pending 队列、对象索引、FTS。

```rust
pub struct Store { /* 持有写连接 + 读连接池 */ }

impl Store {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn migrate(&mut self) -> Result<()>;

    pub fn load_snapshot(&self) -> Result<Snapshot>;          // 启动时读全表
    pub fn apply_pull(&mut self, pull: &PullResponse) -> Result<ApplyOutcome>;
    pub fn enqueue(&mut self, m: &Mutation) -> Result<()>;
    pub fn mark_acked(&mut self, mids: &[String], seq: i64) -> Result<()>;
    pub fn settle(&mut self, cursor: i64) -> Result<Vec<Mutation>>;
    pub fn set_cursor(&mut self, cursor: i64) -> Result<()>;

    pub fn put_object_meta(&mut self, key: &str, kind: ObjectKind, size: u64) -> Result<()>;
    pub fn touch_object(&mut self, key: &str, used_at: i64) -> Result<()>;
}
```

### 4.3 `tela-sync`：reducer 与引擎

```rust
pub struct Confirmed { pub cursor: i64, pub tables: Tables }
pub struct Pending { pub mutation: Mutation, pub acked_at: Option<i64> }

pub fn apply_pull(confirmed: &Confirmed, pull: &PullResponse) -> Confirmed;
pub fn apply_mutation(tables: &mut Tables, m: &Mutation);       // 预测
pub fn view(confirmed: &Confirmed, pending: &[Pending]) -> Tables;
pub fn settle(confirmed: &Confirmed, pending: Vec<Pending>) -> Vec<Pending>;
```

`Tables` 用 `im::HashMap`（或 `Arc<HashMap>`）以便廉价克隆：

```rust
pub struct Tables {
    pub profile: Option<ProfileRow>,
    pub prefs: im::HashMap<String, PrefRow>,
    pub subscriptions: im::HashMap<i64, SubscriptionRow>,
    pub feeds: im::HashMap<i64, FeedRow>,
    pub sites: im::HashMap<i64, SiteRow>,
    pub articles: im::HashMap<i64, ArticleRow>,
    pub titles: im::HashMap<(i64, Lang), TitleRow>,
    pub states: im::HashMap<i64, StateRow>,
    pub recommendations: im::HashMap<i64, RecommendationRow>,
    pub highlights: im::HashMap<HighlightId, HighlightRow>,
    pub translations: im::HashMap<(ContentKey, Lang), TranslationRow>,
    pub follows: im::HashMap<MemberId, FollowRow>,
    pub folders: im::HashMap<FolderId, FolderRow>,
    pub feed_folders: im::HashMap<i64, FeedFolderRow>,
    pub blogrolls: im::HashMap<BlogrollId, BlogrollRow>,
    pub blogroll_blogs: im::HashMap<(BlogrollId, i64), BlogrollBlogRow>,
}
```

引擎：

```rust
pub enum SyncEvent {
    Pulled { cursor: i64, rows: usize },
    Pushed { applied: usize, rejected: usize },
    Failed { kind: SyncFailure, retry_in: Duration },
    AccountChanged,
    UpgradeRequired,
    SignedOut,
}

pub struct SyncEngine { /* store, net, channel, config */ }

impl SyncEngine {
    pub fn start(&mut self, cx: &mut Context<Self>);
    pub fn request_pull(&mut self, reason: PullReason);
    pub fn enqueue(&mut self, m: Mutation);
    pub fn flush(&mut self);                 // 立即推送（窗口隐藏/关闭）
    pub fn set_active(&mut self, active: bool); // 前台/后台
}
```

### 4.4 `tela-net`：HTTP 与认证

```rust
pub struct Client {
    http: reqwest::Client,
    base: Url,
    auth: Auth,
    protocol: u32,
    member: Option<String>,
}

pub enum Auth {
    Bearer(SecretString),     // 推荐
    Cookie(CookieJar),        // 兼容现状
}

impl Client {
    pub async fn pull(&self, cursor: i64, titles: Lang) -> Result<PullResponse>;
    pub async fn push(&self, ms: &[Mutation]) -> Result<PushResponse>;
    pub async fn titles(&self, lang: Lang, ids: &[i64]) -> Result<Vec<TitleRow>>;
    pub async fn object(&self, key: &str) -> Result<Bytes>;
    pub async fn bundle(&self, keys: &[ContentKey]) -> Result<HashMap<ContentKey, ContentObject>>;
    pub async fn image(&self, key: &ContentKey, index: usize) -> Result<Bytes>;
    pub async fn translate(&self, article_id: i64, lang: Lang) -> Result<TranslationStatus>;
    pub async fn translations(&self, key: &str, lang: Lang) -> Result<TranslationRow>;
    pub async fn discover(&self, url: &str) -> Result<DiscoverResult>;
    pub async fn add_feed(&self, feed_url: &str) -> Result<AddFeedResult>;
}
```

统一错误：

```rust
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("unauthorized")] Unauthorized,
    #[error("upgrade required")] Upgrade,
    #[error("account changed")] AccountChanged,
    #[error("insufficient scope: {0}")] Scope(&'static str),
    #[error("rate limited, retry after {0:?}")] RateLimited(Option<Duration>),
    #[error("not found")] NotFound,
    #[error("http {status}")] Http { status: u16, body: String },
    #[error("network: {0}")] Network(#[from] reqwest::Error),
    #[error("decode: {0}")] Decode(#[from] serde_json::Error),
}
```

### 4.5 `tela-content`：内容与渲染模型

```rust
pub struct ContentObject { pub format: i32, pub norm: i32, pub key: ContentKey,
                           pub lang: Option<Lang>, pub blocks: Vec<Block>,
                           pub leaves: HashMap<LeafId, LeafInfo>,
                           pub images: Vec<String>, pub stats: Stats }
pub struct Block { pub tag: String, pub html: String, pub leaves: Vec<LeafId> }

pub struct ParsedBlock { pub tag: String, pub nodes: Vec<InlineNode>, pub leaves: Vec<LeafText> }
pub struct LeafText { pub id: LeafId, pub text: String, pub runs: Vec<Run> }

pub fn parse_block(html: &str, key: &ContentKey) -> ParsedBlock;
pub fn rewrite_images(block: &mut Block, base: &Url);
pub fn pair_blocks(original: &[Block], translated: &[Block]) -> Vec<BlockPair>;
pub fn resolve_anchor(anchor: &Anchor, leaves: &HashMap<LeafId, String>) -> Resolved;
pub fn anchor_in(leaf: &LeafId, text: &str, start: usize, end: usize) -> Anchor;
```

### 4.6 `tela-ui`：视图

```rust
pub struct Shell { sidebar: Entity<Sidebar>, list: Entity<ArticleList>, reader: Entity<Reader> }
pub struct ArticleList { /* ListDelegate + 过滤/搜索 */ }
pub struct Reader { article: Option<OpenArticle>, mode: ReadingMode, pane: Entity<ArticleBody> }
pub struct ArticleBody { blocks: Arc<Vec<BlockPair>>, highlights: Vec<ResolvedHighlight> }
```

### 4.7 `tela-app`：入口与生命周期

- `main`：初始化 GPUI、主题、i18n、日志、Store、Net、Sync；
- 窗口：主窗口 + 设置窗口 + 关于；
- 生命周期：`on_window_should_close` → flush；`cx.on_system_theme_change`；
- 快捷键：全局 action 注册；
- 更新：启动后延迟检查；
- 深链接：`tela://article/<id>`（可选）。

---

## 5. 数据层设计

### 5.1 原则

1. **SQLite 是唯一真相**：内存 `Tables` 是它的投影，可随时重建。
2. **一个事务一次 pull**：保证 cursor 与数据一起提交，断电不产生"半页"。
3. **pending 与 confirmed 同库不同表**：崩溃后重启，未发送的变更还在。
4. **对象在文件系统，元数据在库**：大 JSON 不塞进 SQLite。

### 5.2 Schema（草案）

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
); -- schema_version, member_id, cursor, titles_lang, protocol, last_pull_at

CREATE TABLE profile (
  id INTEGER PRIMARY KEY CHECK (id = 1),   -- 单行：当前账号的资料
  handle TEXT NOT NULL,
  display_name TEXT,
  bio TEXT,
  ui_locale TEXT,
  reading_lang TEXT,
  ui_locale_at INTEGER,
  reading_lang_at INTEGER,
  public_subscriptions INTEGER NOT NULL DEFAULT 0,
  public_likes INTEGER NOT NULL DEFAULT 0,
  public_subscriptions_version INTEGER NOT NULL DEFAULT 0,
  public_likes_version INTEGER NOT NULL DEFAULT 0,
  gravatar INTEGER NOT NULL DEFAULT 1,
  gravatar_found INTEGER,
  avatar_uploaded INTEGER NOT NULL DEFAULT 0,
  avatar TEXT,
  is_admin INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL
);

CREATE TABLE prefs (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at INTEGER NOT NULL, seq INTEGER NOT NULL);

CREATE TABLE feeds (
  id INTEGER PRIMARY KEY, site_id INTEGER NOT NULL, feed_url TEXT NOT NULL,
  title TEXT, status TEXT NOT NULL, last_fetched_at INTEGER, last_error TEXT,
  last_item_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE sites (
  id INTEGER PRIMARY KEY, home_url TEXT NOT NULL, title TEXT, description TEXT,
  favicon_key TEXT, primary_lang TEXT, listing TEXT NOT NULL,
  owned INTEGER NOT NULL DEFAULT 0, claimed INTEGER NOT NULL DEFAULT 0,
  reader_count INTEGER NOT NULL DEFAULT 0, translation_opt_out INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL
);

CREATE TABLE subscriptions (
  feed_id INTEGER PRIMARY KEY, watermark_id INTEGER NOT NULL,
  created_at INTEGER NOT NULL, deleted_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE articles (
  id INTEGER PRIMARY KEY, feed_id INTEGER NOT NULL REFERENCES feeds(id),
  url TEXT, title TEXT NOT NULL, author TEXT,
  published_at INTEGER, fetched_at INTEGER NOT NULL, sort_at INTEGER NOT NULL,
  source_lang TEXT, excerpt TEXT, content_key TEXT,
  word_count INTEGER NOT NULL DEFAULT 0, reading_minutes INTEGER NOT NULL DEFAULT 0,
  extract_state TEXT NOT NULL, like_count INTEGER NOT NULL DEFAULT 0,
  recommend_count INTEGER NOT NULL DEFAULT 0, seq INTEGER NOT NULL
);
CREATE INDEX articles_feed_sort ON articles(feed_id, sort_at DESC, id DESC);
CREATE INDEX articles_sort ON articles(sort_at DESC, id DESC);

CREATE TABLE titles (
  article_id INTEGER NOT NULL, lang TEXT NOT NULL,
  title TEXT, excerpt TEXT, status TEXT NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY (article_id, lang)
);

CREATE TABLE states (
  article_id INTEGER PRIMARY KEY, read_at INTEGER, read_updated_at INTEGER,
  liked_at INTEGER, liked_updated_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE recommendations (
  article_id INTEGER PRIMARY KEY, note TEXT, created_at INTEGER NOT NULL,
  deleted_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE highlights (
  id TEXT PRIMARY KEY, article_id INTEGER NOT NULL, content_key TEXT NOT NULL,
  side TEXT NOT NULL, lang TEXT, leaf_id TEXT NOT NULL,
  start INTEGER NOT NULL, end INTEGER NOT NULL,
  quote TEXT NOT NULL, prefix TEXT NOT NULL, suffix TEXT NOT NULL,
  note TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  deleted_at INTEGER, seq INTEGER NOT NULL
);
CREATE INDEX highlights_article ON highlights(article_id);

CREATE TABLE translations (
  content_key TEXT NOT NULL, lang TEXT NOT NULL, state TEXT NOT NULL,
  chunk_keys_json TEXT NOT NULL DEFAULT '[]', object_key TEXT,
  failed_leaves_json TEXT NOT NULL DEFAULT '[]', seq INTEGER NOT NULL,
  PRIMARY KEY (content_key, lang)
);

CREATE TABLE follows (
  user_id TEXT PRIMARY KEY, handle TEXT, display_name TEXT, avatar TEXT,
  created_at INTEGER NOT NULL, deleted_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE folders (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL, deleted_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE feed_folders (
  feed_id INTEGER PRIMARY KEY, folder_id TEXT, updated_at INTEGER NOT NULL, seq INTEGER NOT NULL
);

CREATE TABLE blogrolls (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
  is_public INTEGER NOT NULL DEFAULT 0, public_version INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, deleted_at INTEGER, seq INTEGER NOT NULL
);

CREATE TABLE blogroll_blogs (
  blogroll_id TEXT NOT NULL, feed_id INTEGER NOT NULL,
  note TEXT, note_at INTEGER NOT NULL DEFAULT 0,
  added_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, removed_at INTEGER,
  seq INTEGER NOT NULL, PRIMARY KEY (blogroll_id, feed_id)
);

-- 本地表
CREATE TABLE pending_mutations (
  mid TEXT PRIMARY KEY, at INTEGER NOT NULL, kind TEXT NOT NULL,
  payload_json TEXT NOT NULL, acked_at INTEGER, created_at INTEGER NOT NULL
);
CREATE INDEX pending_order ON pending_mutations(created_at, mid);

CREATE TABLE objects (
  key TEXT PRIMARY KEY, kind TEXT NOT NULL,      -- c | t | tc | img | avatar
  size INTEGER NOT NULL, fetched_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX objects_lru ON objects(last_used_at);

CREATE TABLE tombstones (
  entity TEXT NOT NULL, key TEXT NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY (entity, key)
);

CREATE VIRTUAL TABLE articles_fts USING fts5(
  title, excerpt, content='articles', content_rowid='id', tokenize='unicode61'
);
-- external-content FTS 需要 INSERT/UPDATE/DELETE 触发器同步，或改为独立表在写入时填充

```

### 5.3 事务与提交

```rust
fn apply_pull(&mut self, pull: &PullResponse) -> Result<ApplyOutcome> {
    let tx = self.conn.transaction()?;
    if pull.reset { clear_all(&tx)?; }
    for row in &pull.rows.profile { upsert_profile(&tx, row)?; }
    // ... 17 张表
    for t in &pull.tombstones { delete_tombstoned(&tx, t)?; }
    prune(&tx)?;                           // 取消订阅后未被保留的文章
    set_cursor(&tx, pull.cursor)?;         // 与数据同一事务
    tx.commit()?;
    Ok(ApplyOutcome { cursor: pull.cursor })
}
```

**为什么 prune 放在同一个事务**：Web 端的 `prune()` 在 `applyPull` 里做，
取消订阅后文章要立即消失；如果分两次提交，UI 会短暂看到"幽灵文章"。

### 5.4 迁移

- `meta.schema_version` 单调递增；迁移脚本编译进二进制（`include_str!`）；
- 每次启动在一个事务里升级，失败则回滚并提示"需要更新版本"；
- **不做降级**：旧版本打开新库 → 明确报错，而不是损坏数据；
- 迁移前自动 `VACUUM INTO` 备份到 `backups/`（保留最近 2 份）。

### 5.5 多账号

```
<data_dir>/
├── tela.db                  # 单账号主库（MVP）
├── accounts/
│   ├── <memberId>/tela.db
│   └── <memberId>/objects/
└── objects/                 # 共享对象缓存（内容不可变）
```

- MVP：单账号 + 切换时保留旧库（不删），下次切回即用；
- 对象缓存跨账号共享（内容不含用户数据）；
- pending 队列**必须**按账号隔离，绝不能跨账号推送。

### 5.6 全文搜索

- 本地先搜：FTS5 对 `articles.title`、`articles.excerpt`、`titles.title`（当前语言）建索引；
- 正文全文不索引（体积大、收益低）；只索引标题与摘要，与 Web 的"device first"一致；
- FTS 查询失败时回退 `LIKE`；
- 服务端 `/api/v1/search` 补足 30 天视野之外的内容。

---

## 6. 协议层设计

### 6.1 启动序列

```
1. 打开 Store → 迁移 → 读 meta（cursor、member、titles_lang）
2. 读全表 → 构建 Arc<Tables>（confirmed）
3. 读 pending → 构建 view → 立即渲染（不等网络）
4. 若无 token → 登录页；否则：
5. 后台 pull(cursor, titles_lang)
6. 若有 pending → 后台 push
7. 若 cursor == 0 → 首次同步（快照），显示进度
```

**首屏渲染与网络完全解耦**：即使断网，也能在 600ms 内看到上次的内容。

### 6.2 拉取状态机

```
Idle ──request──▶ Pulling ──ok──▶ Applying ──▶ Idle
                    │                 │
                    │                 └─ more=true ──▶ Pulling（立刻）
                    ├─ 401 ──▶ SignedOut
                    ├─ 409 upgrade ──▶ UpgradeRequired
                    ├─ 409 account_changed ──▶ Resetting ──▶ SignedOut
                    ├─ 429/5xx/network ──▶ Backoff ──▶ Idle
                    └─ timeout(30s) ──▶ Backoff
```

触发点：

| 触发 | 说明 |
|---|---|
| 启动 | 一次 |
| 窗口获得焦点 | 从后台回来时 |
| 每 60 秒 | 仅当前台且可见 |
| 每次 push 之后 | 尽快确认自己的写入 |
| 快速拉取 | 打开的文章正文未到 / 新 feed 未抓取：4s 起，指数到 60s，最多 2 分钟 |
| 手动刷新 | 用户按 `⌘R` / `Ctrl+R` |

退避：1s → 2s → 4s … 上限 60s，±20% 抖动；恢复网络时重置。

### 6.3 推送状态机

```
本地变更 ──▶ Pending（落库）──▶ Debounce(250ms) ──▶ Pushing
                                                    ├─ ok → mark_acked
                                                    ├─ rejected → 丢弃 + 回滚乐观视图 + toast
                                                    └─ 失败 → Backoff（保留 Pending）
```

- 一批最多 50 条；超过则切分，**按创建顺序**发送；
- 窗口隐藏/关闭立即 flush；
- `rejected` 的处理：移除该 pending、记录日志、必要时显示一次 toast（例如 `invalid_name`）；
- 乐观视图：`view()` 已经应用了 pending，被 reject 后自动"弹回"。

### 6.4 合并与去重

- 同一 `mid` 不重复入队（幂等）；
- 同一实体的连续同类 mutation 可合并（可选优化）：
  - `setLiked` 同一 article 只保留最后一条；
  - `markRead` 同一 article 只保留第一条（时间戳更早）；
  - `setPref` 同一 key 只保留最后一条。
  合并会改变"撤销"语义，MVP 先不做，等有性能压力再说。

### 6.5 时钟

- 客户端 `at = now_ms`；服务端会夹到自己的时钟；
- 本地排序/显示用服务端返回值，不依赖本地时钟做跨设备判断；
- 检测到本机时钟与服务端偏差 > 5 分钟时，提示用户校准（可选）。

### 6.6 协议一致性测试

**目标**：Rust 的 reducer 与 TypeScript 的 reducer 行为逐字节一致。

做法：

1. 在 Tela 仓库加一个导出脚本（或在本仓库的 `scripts/` 里跑 Bun）：
   用 `packages/sync` 的纯函数，对一组构造的 pull/mutation 序列生成 `fixtures/*.json`：
   ```json
   { "name": "mark-read-over-unread", "confirmed": {...}, "pending": [...],
     "pull": {...}, "expect": { "tables": {...}, "pending": [...] } }
   ```
2. Rust 测试读取同一批 fixture，跑 `apply_pull` / `apply_mutation` / `view` / `settle`，逐表比较。
3. CI 在 Tela 仓库变更 `packages/sync` 时自动重新生成 fixtures，Rust 仓库通过 submodule 或
   定时任务拉取；不一致就红。

覆盖重点（来自 `packages/sync/src/client.test.ts`）：

- 快照 vs 增量、`more` 分页、`tombstones`
- 30 天视野与 watermark 的已读判定
- 手动未读 vs 后续已读的时钟竞争
- 隐私开关版本化（show 的 `base` 不匹配）
- 博客卷删除级联、笔记保留
- 订阅合并（`mergedInto`）与 `exact`
- 账号切换（`account_changed`）后的清库

---

## 7. 对象与缓存

### 7.1 对象种类

| 种类 | 键 | 大小 | 可变性 |
|---|---|---|---|
| 正文 | `c/<32hex>.json` | 5–100 KB | 不可变 |
| 完整翻译 | `t/<key>/<lang>/<sha>.json` | 5–120 KB | 不可变（内容寻址） |
| 分块翻译 | `tc/<key>/<lang>/<req>/<n>.json` | 1–20 KB | 不可变 |
| 图片 | `img/<key>/<i>` | ≤10 MB | 不可变 |
| 头像 | `avatar/<userId>?v=n` | ≤512 KB | 按版本 |

### 7.2 磁盘布局

```
<data_dir>/objects/
├── c/ab/cd/abcdef....json
├── t/ab/cd/abcd.../zh-Hans/<sha>.json
├── tc/ab/cd/.../<req>/0.json
├── img/ab/cd/abcd.../0
└── tmp/                        # 写入中转，启动时清理
```

- 路径用 key 的前 4 位分两级目录，避免单目录十万文件；
- 原子写：写 `tmp/<uuid>` → `fsync` → `rename`；
- 校验：正文对象写入前校验 `format`/`key`/`norm`；损坏则删除重下；
- 权限：目录 0700，文件 0600。

### 7.3 内存缓存

- L1：最近 32 个已解析 `Arc<ContentObject>`（约 2–4 MB）；
- L2：最近 200 个块的 `ParsedBlock`（渲染结果），LRU；
- 图片：解码后 `RenderImage` LRU，按字节数限制（默认 64 MB）；
- 所有缓存以 `Arc` 共享，避免渲染时克隆 HTML。

### 7.4 预取与淘汰

对齐 Web 策略（`apps/reader/src/store/objects.ts`）：

- **预取**：空闲时按列表顺序预取未读正文，每次 25 个（bundle），并发 2；
- **淘汰**：
  1. 7 天未打开且超过 50 MB → 删除正文；
  2. 仍超限 → 按 `last_used_at` 淘汰最旧；
  3. 正文删除时，同时删除它的翻译与分块（按 key 前缀）；
- **保留**：有高亮/笔记的文章正文**不自动删除**（否则笔记打不开）；
- **手动**：设置里显示缓存占用，可"清理未读缓存"。

### 7.5 图片

- 渲染时把 `/img/<key>/<i>` 重写为 `https://telaread.com/img/<key>/<i>`（带认证）；
- 下载 → 校验 content-type（png/jpeg/webp）→ 解码 → 缩放（按显示宽度，2x DPR）；
- 失败缓存 10 分钟（与 Web 一致），避免坏图反复请求；
- 失败时显示占位（`data-origin` 域名 + 图标），不破坏排版。

```rust
pub struct ImageLoader {
    client: Arc<Client>,
    disk: PathBuf,
    memory: Mutex<LruCache<ImageKey, Arc<RenderImage>>>,
    inflight: Mutex<HashMap<ImageKey, Shared<...>>>,
}
```

> GPUI 的 `ImageSource` 具体变体以锁定的 `gpui-pre 0.3.9` 为准：可以用解码后的内存图像，
> 或注册 `AssetSource` 从磁盘按路径加载。**不要**把远程 URL 直接交给 `img()`，否则会绕过
> 我们的认证与缓存。

---

## 8. 内容渲染

### 8.1 核心决策：自研块渲染器，而不是直接用 `TextView::html`

gpui-kit 的 `TextView::html` 能力很强（解析、链接、图片、选择、复制），但有两个硬限制
（源码为证）：

```rust
// crates/base/src/text/state.rs
pub fn set_range_highlights(&mut self, ...) -> Result<(), RangeHighlightError> {
    if self.format != TextViewFormat::Markdown { return Err(RangeHighlightError::Unsupported); }
    ...
}
pub fn selected_source_range(&self) -> Option<Range<usize>> {
    if self.format != TextViewFormat::Markdown { return None; }
    ...
}
// 注释原文：html5ever 不记录源码偏移
```

也就是说：

- HTML 视图**不能用**内置的范围高亮 API；
- HTML 视图**拿不到**选区对应的源偏移；
- 而 Tela 的高亮/笔记、翻译对齐、精确定位，恰恰需要"叶子 + 字符偏移"。

因此选择：

| 方案 | 高亮 | 选区偏移 | 工作量 | 结论 |
|---|---|---|---|---|
| A. `TextView::html` + 注入 `<mark>` | 只能预注入，重建成本高 | 只能靠 `selected_text` 搜索 | 低 | M1 过渡 |
| B. 自研 HTML→GPUI 渲染器 | 完全可控 | 完全可控 | 中高 | **M2 目标** |
| C. WebView 渲染 | 完全可控（DOM） | 完全可控 | 低但体验差 | 否决 |

**M1 过渡方案（可先上线）**：

- 每个顶层块一个 `Entity<TextViewState>`（`TextView::html`）；
- 已有高亮：渲染前在 HTML 里注入 `<mark data-hl="id">…</mark>`；
- 新建高亮：用 `selected_text()` 拿到纯文本，在该块的叶子文本里做"上下文匹配"定位；
- 已知限制：跨块选择不支持；重复文本需要上下文消歧；每次高亮变更要重建该块布局；
  `TextView` 的 HTML 解析通常不保留 `data-tb` 等自定义属性，因此无法从渲染结果反查叶子，
  只能按文本匹配（这也是 M2 要自研渲染器的原因之一）。

**M2 目标方案**：

- `tela-content` 用 `scraper`（html5ever）解析每个块的 HTML，产出 `ParsedBlock`：
  - 文本 run（带样式：粗体/斜体/代码/链接）；
  - 叶子节点（`data-tb`）与字符区间；
  - 图片、列表、引用、表格等块级结构；
- `tela-ui` 用 GPUI 的 `StyledText`/`Text` 组合渲染；
- 选区由我们的 `SelectionModel` 管理（块内 + 跨块），输出 `(leafId, start, end)`；
- 高亮作为"背景标记"绘制在文字 run 后面（不修改文本），支持重叠合并。

### 8.2 HTML 子集

Tela 的服务端已经做了严格白名单（`packages/content/src/sanitize.ts`），所以客户端只需要支持：

| 类别 | 标签 |
|---|---|
| 块级 | `p`、`div`、`h1`–`h6`、`blockquote`、`pre`、`ul`、`ol`、`li`、`table`、`thead`、`tbody`、`tr`、`th`、`td`、`figure`、`figcaption`、`hr` |
| 行内 | `a`、`strong`、`em`、`b`、`i`、`code`、`s`、`u`、`mark`、`sup`、`sub`、`br` |
| 媒体 | `img`（已改写成 `/img/...`）、`video`/`audio`（服务端已降级为链接） |

不支持的标签：按"透明容器"处理（渲染子节点）或降级为文本。

### 8.3 叶子与偏移

```rust
pub struct LeafText { pub id: LeafId, pub text: String, pub ranges: Vec<(Range<usize>, RunStyle)> }

pub fn leaves_of(parsed: &ParsedBlock) -> Vec<&LeafText>;
```

- `leafId` 直接取 `data-tb`；
- **协议里的偏移是 UTF-16 码元**（Tela 服务端与浏览器都用 JS 的 `String.slice` 语义）。
  一个 emoji 算 2，一个常用汉字算 1；
  服务端的校验 `m.quote.length !== m.end - m.start`（`invalid_range`）也证明它按 JS 的
  `length` 计，即 UTF-16 码元；
- Rust 内部建议用 **char 索引或字节偏移**，在网络边界做一次转换：

```rust
fn utf16_to_char(text: &str, utf16: usize) -> usize;
fn char_to_utf16(text: &str, chars: usize) -> usize;
```

> **这是最容易踩的坑**：直接用 Rust 的 `chars().nth(start)` 去套服务端的 `start`，遇到 emoji
> 或组合字符就会错位；高亮会"漂移"甚至越界 panic。所有来自 `/api/v1/sync` 的高亮锚点、
> 以及写回 `putHighlight` 的 `start`/`end`，都必须经过显式换算，并用中英日 emoji 混合文本做单测。

### 8.4 选区

```rust
pub struct Selection { pub anchor: Cursor, pub focus: Cursor }
pub struct Cursor { pub leaf: LeafId, pub offset: usize }  // UTF-16 还是 char 内部统一

impl SelectionModel {
    pub fn to_anchor(&self, leaves: &HashMap<LeafId, String>) -> Option<Anchor>;
    pub fn from_anchor(a: &Anchor) -> Self;
}
```

规则：

- 跨叶选择：起止各取所在叶，中间叶整段；
- 起止在同一叶：直接取区间；
- 空选区（单击）不产生高亮；
- 选区超出叶子边界：夹到叶子长度。

### 8.5 重锚

移植 `apps/reader/src/lib/anchor.ts`（149 行，纯函数）：

```rust
pub enum Resolved { Exact { leaf: LeafId, start: usize, end: usize },
                    Moved { leaf: LeafId, start: usize, end: usize },
                    Detached }

pub fn resolve_anchor(a: &Anchor, leaves: &IndexMap<LeafId, String>) -> Resolved;
```

评分规则（与 Web 完全一致）：

1. 同一叶、同偏移、同引文 → `Exact`；
2. 否则全叶搜索引文，按上下文匹配数打分，同叶 +8、距离近加成；
3. 最佳候选上下文太弱（< min(4, prefix+suffix 长度)）且有多个候选 → `Detached`；
4. 无候选 → `Detached`。

`Detached` 的高亮仍然显示（在"笔记"面板里），但正文中不高亮。

> 移植时同样要注意 UTF-16：`quote`/`prefix`/`suffix` 的截取、`quote` 的长度校验都要与
> 服务端一致（`quote.length === end - start`），否则 `putHighlight` 会被 `invalid_range` 拒绝。

### 8.6 双语对照

```
┌──────────────────────────┬──────────────────────────┐
│ 原文块 0                  │ 译文块 0                  │
│ 原文块 1                  │ 译文块 1                  │
│ 原文块 2（图）            │ 译文块 2（图，保留原图）    │
└──────────────────────────┴──────────────────────────┘
```

- 按**顶层块索引**配对（`pair_blocks`），不做句子级对齐；
- 块数与 tag 不匹配 → 退化为"整篇两栏"，避免错位；
- 原文栏背景用 `--color-source`（比纸色深一档），译文栏用纸色；
- 对齐：两栏共享同一行的垂直位置；块高不同时，行高 = 两者较大值（GPUI 用 `h_flex` + 同行
  两列，各自 `flex_1`）。

三种模式：

| 模式 | 布局 |
|---|---|
| `side` | 左右两栏 |
| `trans` | 只显示译文（译文缺失时显示原文 + "翻译中"） |
| `orig` | 只显示原文 |

### 8.7 流式翻译叠加

```
1. POST /api/v1/translations → state=requested
2. 轮询（400ms / 5s，然后 1.5s）
3. 每有新的 chunkKeys：GET /o/tc/... → { "blockIndex": "html" }
4. 用 chunk 替换对应块；未到的块显示原文 + 轻微"待翻译"标记
5. state=ready → GET /o/t/<key>/<lang>/<sha>.json → 整体替换
6. failedLeaves → 对应块保留原文，并标注"这段没翻出来"
```

UI 细节：

- 首块到达前，译文栏显示骨架屏（skeleton），不显示空白；
- 每块独立淡入（150ms），避免整页跳动；
- 翻译中切换文章 → 取消轮询（`Task` drop 即取消）。

### 8.8 排版

对齐 Web 的阅读排版 token：

| 项 | 值 |
|---|---|
| 正文字号 | `19.5px × scale`（`s=0.88 / m=1 / l=1.13 / xl=1.27`） |
| 行高 | 1.55 |
| 中文行高 | 1.8，字号 `18px × scale` |
| 行宽 | 560 / 640 / 760 px |
| 段间距 | 1em |
| 引用 | 左边框 2px `--color-line`，文字 `--color-ink-2`，斜体 |
| 代码 | 等宽，`--color-hover` 背景，圆角 4px |
| 图片 | 最大宽度 100%，圆角 4px，`figcaption` 12px `--color-muted` |
| 高亮 | `--color-highlight`，不透明度 0.55（暗色） |

字体：

```rust
// 拉丁：打包 EB Garamond / Figtree（OFL）
// 中文：优先系统字体，按脚本选择 SC/TC
const SERIF_SC: &[&str] = &["Songti SC", "Noto Serif CJK SC", "Source Han Serif SC", "SimSun"];
const SERIF_TC: &[&str] = &["Songti TC", "Noto Serif CJK TC", "Source Han Serif TC"];
const SANS_SC: &[&str] = &["PingFang SC", "Noto Sans CJK SC", "Microsoft YaHei"];
```

- GPUI 的 `Font::fallbacks` 用于缺字回退；
- `zh-Hant` 文本必须用 TC 字体栈（与 Web 一致，避免简体字形）；
- 字体加载失败不能让布局 panic（gpui 文档明确说"找不到字体会 fallback，全都找不到才 panic"）；
  因此启动时探测并选一个可用的默认字体。

---

## 9. UI 架构

### 9.1 窗口布局

```
┌─────────────────────────────────────────────────────────────────────┐
│ TitleBar（可选：macOS 透明标题栏 / Windows 自绘）                      │
├──────────────┬───────────────────────┬──────────────────────────────┤
│ 侧边栏 240px  │ 列表 380px             │ 阅读区 flex_1                 │
│ · 未读/今天/喜欢│ · 日期分组            │ · 标题 / 作者 / 时间           │
│ · 文件夹      │ · 未读点              │ · 工具条（Aa / 赞 / 推荐 / 高亮）│
│ · 订阅列表    │ · 标题 / 来源 / 时长    │ · 正文（单栏或双栏）           │
│ · 发现入口    │ · 虚拟列表            │ · 翻译条（按需）               │
│ · 设置        │                       │                              │
├──────────────┴───────────────────────┴──────────────────────────────┤
│ StatusBar：同步状态 · 离线/在线 · 今日翻译额度 · 缓存占用               │
└─────────────────────────────────────────────────────────────────────┘
```

- 三栏用 `h_resizable`（`ResizableState`），宽度持久化到 `layout.json`（设备本地，不同步）；
- 窄窗口（< 900px）折叠为两栏（列表/阅读）；
- 更窄（< 600px）单栏 + 返回按钮（对齐 Web 的手机布局）。

### 9.2 实体划分

```
AppServices（Global）
├── StoreHandle
├── NetHandle
├── SyncHandle
└── PrefsHandle

Shell（Entity）
├── Sidebar（Entity）
├── ArticleList（Entity<ArticleList>，含 ListState）
└── Reader（Entity）
    └── ArticleBody（Entity，按 articleId 复用）

Overlays（Entity）
├── CommandPalette
├── SettingsWindow
└── Toasts
```

规则：

- 每个 Entity 只持有**自己的**状态；共享数据通过 `Arc<Tables>` 快照传递；
- 订阅关系用 `cx.subscribe`，避免全局广播；
- 渲染函数里不做数据库查询、不发请求（gpui 的"render 必须是纯的"）。

### 9.3 列表虚拟化

- 文章列表：固定行高（72px / 88px），用 `v_virtual_list`；
- 正文块：不定高，用 `gpui::list` + `ListState`（`ListAlignment::Top`），行高由 GPUI 测量；
- 双语对照：一行 = 一个块对，行高 = max(原文高, 译文高)；
- 分组标题（"今天/昨天/更早"）作为特殊行插入。

### 9.4 状态更新路径

```rust
// Store 线程完成 apply_pull 后
fn on_pull_applied(&mut self, cx: &mut Context<Self>, snapshot: Arc<Tables>) {
    self.tables = snapshot;
    cx.notify();                    // 只重绘订阅了它的视图
}
```

- 拉取成功后重建一次 `view` 快照（confirmed ⊕ pending），O(n) 但 n 只有几千；
- 只读视图用 `Arc` 共享，避免每个组件 clone；
- 滚动时只重绘可见行；`ListState` 自己处理。

### 9.5 键盘与命令

全局 action（与 Web 对齐，并补桌面习惯）：

| 键 | 动作 |
|---|---|
| `j` / `k` | 下/上一条 |
| `Space` / `Shift+Space` | 翻页 |
| `Enter` | 打开 |
| `Esc` | 关闭阅读区 / 退出命令面板 |
| `m` | 标已读/未读 |
| `s` | 加星（点赞） |
| `h` | 高亮选区 |
| `o` | 在浏览器打开原文 |
| `l` | 复制链接 |
| `[` | 折叠侧边栏 |
| `f` | 折叠列表（专注模式） |
| `⌘K` / `Ctrl+K` | 命令面板 |
| `⌘R` / `Ctrl+R` | 同步 |
| `⌘F` / `Ctrl+F` | 搜索 |
| `⌘,` / `Ctrl+,` | 设置 |
| `g i` / `g a` / `g d` | 跳转 Inbox / All / Discover（可选） |

命令面板项：切换账号、切换语言、切换主题、字号、布局、同步、清缓存、打开设置、跳转文章。

### 9.6 主题

- 在启动时把 Tela 的 token 注册为 gpui-kit `Theme`（`Theme::update`）；
- 亮/暗两套，跟随系统（`cx.observe_system_theme`）；
- 自定义 token 放在 `TelaTheme` Global，UI 用 `cx.tela_theme()` 读取；
- 主题 pref 同步（`ui.theme` = `system|light|dark`），但设备本地也缓存一份避免首帧闪烁。

```rust
pub struct TelaTheme {
    pub paper: Hsla, pub surface: Hsla, pub ink: Hsla, pub body: Hsla,
    pub ink_2: Hsla, pub muted: Hsla, pub line: Hsla, pub hover: Hsla,
    pub accent: Hsla, pub highlight: Hsla, pub danger: Hsla,
}
```

### 9.7 无障碍

- 列表行有 `role=listitem`、`label`（标题 + 来源 + 未读数）；
- 阅读区按块暴露 heading/paragraph 语义；
- 所有快捷键都有菜单/命令面板入口；
- 焦点顺序：侧边栏 → 列表 → 阅读区 → 工具条；
- 用 `#[gpui_kit::test]` + `gpui_kit::test::{TestQueryExt, TestEventExt}` 的
  `find_by_label` / `find_all_by_role` 断言无障碍标签与角色（gpui-kit 的查询按**可访问名**
  匹配，不是渲染文本；因此要给稳定、可本地化的 label）。

---

## 10. 认证与会话

### 10.1 首选：Bearer Token（等待服务端支持）

```
keyring 存储 → 请求头 Authorization: Bearer <token>
```

- 存储：`keyring` crate（macOS Keychain / Windows Credential Manager / Linux Secret Service）；
- 启动：读 token → `GET /api/v1/me` 验证 → 拿 `memberId`；
- 401：清除 token，展示登录页，本地库保留（离线可读）；
- 403 insufficient_scope：提示重新授权。

### 10.2 兼容：Cookie

- `reqwest` + `cookie_store`（或自管 CookieJar）；
- 写请求带 `Origin: https://telaread.com`；
- token 持久化：只持久化 `tela.session_token`，不持久化其他 cookie；
- 在 UI 上明确标注"兼容模式"。

### 10.3 登录流程（设备流）

```
[登录页]
  ┌──────────────────────────────────────┐
  │ 在浏览器中打开 telaread.com/activate  │
  │ 输入代码：  K7QM-3XZP                 │
  │ [复制代码] [打开浏览器]                │
  │ 状态：等待确认…                        │
  └──────────────────────────────────────┘
```

- 复制到剪贴板 + 自动打开浏览器（用户可关）；
- 轮询 `interval` 秒；`slow_down` 自动加 5 秒；
- 成功后立即 pull 快照，显示"正在同步你的 320 个订阅…"。

### 10.4 退出

- 清除 keyring token；
- 询问："保留本地内容以便离线阅读？"（默认保留，提供"删除本地数据"）；
- 若删除：清空 accounts/<id> 与对象缓存中仅属于该账号的部分（保守起见可全清）。

---

## 11. 同步调度与生命周期

### 11.1 前台/后台

| 状态 | 行为 |
|---|---|
| 前台可见 | 每 60s pull；push 防抖 250ms |
| 前台不可见（最小化/遮挡） | 暂停定时 pull；push 仍 flush |
| 休眠/合盖 | 不活动；唤醒后立即 pull |
| 无网络 | 退避重试（1s→60s）；UI 显示"离线" |
| 系统省电模式 | 拉取间隔 × 2；暂停预取与图片预加载 |

### 11.2 关闭窗口

- macOS：关闭窗口不退出（Dock 图标仍在），后台继续低频同步（可选）；
- Windows/Linux：关闭即退出，退出前 `flush()`，最多等 2 秒；
- 退出时若仍有 pending：写入 `pending_mutations`，下次启动继续推送。

### 11.3 新文章通知（可选，P2）

- 每次 pull 后比较"新文章 id > 上次最大 id 且属于订阅"；
- 系统通知（gpui-kit `system-notification`），点击跳转文章；
- 设置项：关闭 / 仅未读 / 全部；
- 通知聚合：同一 feed 10 分钟内只通知一次。

---

## 12. 性能设计

### 12.1 预算

| 场景 | 预算 | 测量方式 |
|---|---|---|
| 冷启动到首帧 | 600 ms | 埋点 `app.start` → `first_frame` |
| 首屏列表可见 | +200 ms | 埋点 `list.visible` |
| 打开缓存文章 | 50 ms | 埋点 `article.open` |
| 打开未缓存文章（对象已预取） | 120 ms | 同上 |
| 滚动帧时间 | p95 < 8.3 ms（120fps） | `gpui-fps`（gpui-kit 内置） |
| 同步 100 行增量 | 300 ms | 埋点 |
| 内存（稳态） | < 300 MB | 进程 RSS |
| 数据库（5 万文章） | < 200 MB | 文件大小 |

### 12.2 关键手段

- **首屏不等网络**：先渲染本地快照；
- **对象预取**：空闲时 bundle 拉取；
- **图片懒加载**：进入视口前 1 屏才开始下载；
- **渲染隔离**：列表滚动时不触发阅读区重绘（`cx.notify` 精确到 Entity）；
- **避免 clone**：所有大对象用 `Arc`；
- **块级缓存**：`ParsedBlock` 按 `(contentKey, norm)` 缓存；
- **数据库索引**：`articles(sort_at, id)`、`articles(feed_id, sort_at, id)`；
- **批量写入**：一次 pull 一个事务，prepared statement 复用；
- **FTS 异步**：搜索在后台线程执行，结果通过 channel 回传。

### 12.3 内存控制

- `Arc<Tables>` 全量快照（约 5–30 MB，取决于订阅数）；
- 正文对象 L1 32 个；
- 图片 LRU 64 MB；
- 解析块缓存按字节限制（32 MB）；
- 超过阈值时主动触发一次淘汰并记录日志。

---

## 13. 安全与隐私

| 项 | 做法 |
|---|---|
| Token 存储 | OS keyring；绝不写日志/配置 |
| 传输 | TLS（rustls），HTTP/2；不固定证书（避免轮换失败） |
| 本地库 | 默认不加密；提供"加密本地数据"选项（SQLCipher，后续） |
| 对象缓存 | 目录 0700，文件 0600 |
| 日志 | 脱敏：token、cookie、邮箱、URL query 一律不落盘 |
| 崩溃上报 | 默认关闭；开启时只发堆栈 + 版本 + OS，不发内容 |
| 外部链接 | 用系统浏览器打开，不在应用内导航 |
| HTML | 只渲染服务端已 sanitize 的子集；不执行脚本 |
| 明文密码 | 客户端不保存密码，只用设备流/令牌 |
| 账号切换 | 隔离数据库；对象缓存可共享但无用户数据 |

**威胁模型（简版）**：

1. 本地磁盘被读取 → token 在 keyring，正文是公开内容，高亮/笔记可能泄露 → 提供加密选项；
2. 中间人 → TLS + 系统根证书；
3. 恶意博客内容 → 服务端 sanitize + 客户端白名单渲染；
4. 令牌泄露 → 可撤销、可限 scope、可设过期。

---

## 14. 可观测性与诊断

### 14.1 日志

```rust
tracing_subscriber::fmt()
    .with_env_filter("tela=info,gpui=warn,reqwest=warn")
    .with_writer(rolling::daily(log_dir, "tela.log"))
    .json()
    .init();
```

- 保留 7 天；单文件上限 10 MB；
- 关键事件：启动、登录、pull（cursor/rows/耗时）、push（applied/rejected）、对象命中/未命中、
  翻译状态、错误；
- 设置页提供"导出诊断包"（日志 + 版本 + 库统计，不含内容）。

### 14.2 指标（本地）

- 同步成功率、平均延迟；
- 对象缓存命中率；
- 首屏耗时、文章打开耗时；
- 崩溃次数（本地计数）。

这些指标默认只存本地，可选择性上报。

### 14.3 调试面板

`⌘⇧D` 打开：协议版本、cursor、pending 数、表行数、缓存占用、最近 20 条同步事件。
开发版专用。

---

## 15. 分发与更新

### 15.1 打包

| 平台 | 产物 | 签名 |
|---|---|---|
| macOS | `.app` + `.dmg` | Developer ID + 公证（notarytool） |
| Windows | `.exe`（Inno Setup）+ 便携 zip | 代码签名（OV/EV 或 Azure Trusted Signing） |
| Linux | `.deb` + `.tar.gz`（可选 AppImage/Flatpak） | 仓库签名 / 不上架则提供校验和 |

- 图标：跟随 Tela 品牌（需作者许可），提供 16–1024 多尺寸；
- `Info.plist` / 安装器版本号与 `Cargo.toml` 一致；
- 首次启动写入 `app identity`（通知用）。

### 15.2 更新策略

- **macOS**：整包替换（签名 bundle 不能只换二进制）；用 `.dmg` 或 Sparkle 风格；
- **Windows**：由 Inno Setup 安装器负责；
- **Linux**：交给包管理器；便携版用 `self_update`；
- 元数据：GitHub Releases + `SHA256SUMS`；**不要**把发布 token 打进二进制；
- 检查时机：启动后 30 秒，之后每 6 小时；失败静默；
- 手动"检查更新"始终可用；
- 灰度：先给 beta 通道，稳定后全量。

### 15.3 CI

```yaml
# .github/workflows/ci.yml（示意）
jobs:
  test:
    strategy: { matrix: { os: [macos-15, windows-2022, ubuntu-24.04] } }
    steps:
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --workspace
      - run: cargo deny check licenses
```

- UI 测试用 GPUI 的 `test-support` 无头运行（2026-10-27 实测：去掉 `DISPLAY`/`WAYLAND_DISPLAY` 仍通过）；
  真实窗口截图另用 `HeadlessAppContext` + WGPU，CI 默认跳过；
- 打包 job 只在 tag 上跑；
- 产物带 SHA256 与 SBOM（`cargo cyclonedx`）。

---

## 16. 测试策略

| 层级 | 内容 | 工具 |
|---|---|---|
| 单元 | reducer、锚点、UTF-16 偏移、排版计算 | `cargo test` |
| 协议一致性 | 与 TS reducer 的 golden fixtures | `insta` + 自定义 harness |
| 网络 | mock server（`wiremock`）覆盖 401/409/429/5xx/超时 | `wiremock` |
| 存储 | 迁移、事务回滚、断电模拟（kill -9 后重开） | `tempfile` |
| 渲染 | 块渲染的 golden 截图 | `#[gpui_kit::test]` + `insta` |
| UI 集成 | 键盘、列表选择、打开文章、高亮创建 | `#[gpui_kit::test]` + `TestAppContext` |
| 端到端 | 对着本地 Tela（`bun run dev`）跑真实同步 | 脚本 + 测试账号 |
| 性能 | 启动、滚动、内存 | `gpui-fps` + criterion |

### 16.1 UI 测试示例（示意，具体 API 以 `gpui-kit` 的 `test` 文档为准）

```rust
#[gpui_kit::test]
async fn opening_an_article_marks_it_read(cx: &mut TestAppContext) {
    let app = cx.update(|cx| boot_test_app(cx, Fixtures::one_unread_article()));
    let list = app.read_with(cx, |app, _| app.list.clone());
    cx.update(|cx| list.update(cx, |list, cx| list.select_next(cx)));
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.pending_count()) == 1);
}
```

### 16.2 端到端

- 用 Tela 仓库的 `bun run dev`（三个 Worker + 本地 D1 + 测试 feed server）；
- 测试账号通过 `ENV=test` 的 outbox 拿验证码；
- 场景：登录 → 订阅 → 等抓取 → 拉取 → 打开 → 标记 → 推送 → 另一设备拉取。

---

## 17. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Tela 不开放 Bearer/设备流 | 只能用 Cookie 兼容模式 | 先做 A 方案；把 `Auth` 做成 trait，切换成本低 |
| 协议变更（`MIN_CLIENT` 升级） | 客户端被 409 | `/client` 能力发现 + 启动时检查 + 自动更新 |
| GPUI API 变动 | 编译失败 | 锁定 `gpui-kit 0.8.0`；UI 层薄封装；不追新 |
| HTML 渲染保真度 | 排版错乱 | 白名单子集 + golden 截图 + 与 Web 对照 |
| 中文/emoji 偏移错误 | 高亮错位 | UTF-16 换算单测；跨语言 fixture |
| Linux 显卡/驱动问题 | 无法启动 | 文档写明 Vulkan 要求；提供 `--software` 诊断；错误页给出指引 |
| 字体缺失 | 排版 panic/乱码 | 启动探测 + fallback chain + 可选字体包 |
| 翻译配额耗尽 | 阅读体验中断 | UI 显示额度；缓存已翻译内容；提供"原文模式" |
| 对象缓存膨胀 | 磁盘占满 | 淘汰策略 + 设置页手动清理 + 配额提示 |
| 私有测试期无法注册 | 无法获取测试用户 | 与作者协调测试账号/邀请码 |
| 品牌与授权 | 法律风险 | 上线前取得作者书面许可；命名 "Tela Desktop (unofficial)" 直到授权 |

---

## 18. 里程碑

> 估算前提：1 名全职开发者 + AI 辅助，熟悉 Rust；不含与作者联调/等 API 的时间。
>
> **编号说明（2026-10-27）**：本节是 2026-10-25 的旧编号（M0–M4）。实际执行与验收以
> [`development_plan.md`](./development_plan.md) §9 的 M0–M5 为准；本节 M0 的“最小 UI”
> 已由开发计划的 M2/M3 承接，GUI 宿主在 2026-10-27 的 S0–S4 补课中完成
> （见 [`gui_acceptance.md`](./gui_acceptance.md)）。

### M0 — 骨架与协议（1–2 周）

- [ ] workspace + 7 个 crate 骨架
- [ ] `tela-proto` 全部类型 + serde 测试
- [ ] `tela-store` schema + 迁移 + 行映射
- [ ] `tela-net` Cookie 模式 + pull/push
- [ ] `tela-sync` reducer（apply_pull/apply_mutation/view/settle）+ fixture 测试
- [ ] 最小 UI：登录（Cookie）、列表、`TextView::html` 阅读、标已读
- **验收**：能登录、拉取快照、阅读、标记已读并推送成功

### M1 — 可用的阅读器（2–3 周）

- [ ] 对象缓存 + bundle 预取 + 图片代理
- [ ] 三栏布局 + 虚拟列表 + 键盘导航
- [ ] 翻译请求/轮询/分块叠加 + 三种阅读模式
- [ ] 偏好（字号/行宽/主题）+ 同步
- [ ] 文件夹、OPML 导入导出
- [ ] 离线模式与错误提示
- **验收**：断网可读已缓存内容；联网自动同步；翻译流式显示

### M2 — 高亮与打磨（2–3 周）

- [ ] 自研块渲染器（或 M1 的 `<mark>` 注入完善）
- [ ] 选区 → 锚点 → 高亮/笔记
- [ ] 重锚算法 + 笔记面板
- [ ] 搜索（本地 FTS + 服务端）
- [ ] 主题精修、动效、空状态
- **验收**：高亮跨版本可恢复；搜索 200ms 内出结果

### M3 — 发现与发布（2–3 周）

- [ ] Discover（This week / Articles / Blogs / Readers / Blogrolls）
- [ ] 资料页、关注、推荐
- [ ] 通知、Dock 徽标（可选）
- [ ] 打包、签名、自动更新
- [ ] 崩溃上报（可选）
- **验收**：三平台安装包可分发；自动更新可用

### M4 — 公测准备

- [ ] 性能达标（见 12.1）
- [ ] 无障碍检查
- [ ] 文档、隐私政策、关于页
- [ ] 与作者联调 Bearer/设备流
- **验收**：可对外发布（需作者授权）

---

## 19. 附录

### 19.1 入口骨架

```rust
// crates/tela-app/src/main.rs
use gpui_kit::*;

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            tela_ui::init_theme(cx);
            tela_ui::init_i18n();

            let services = AppServices::boot(cx);
            cx.set_global(services);

            gpui_kit::open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions { title: Some("Tela".into()), ..Default::default() }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(820.)), cx))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Shell::new(window, cx)),
            )
            .expect("open window");
        });
}
```

### 19.2 同步引擎骨架

```rust
pub struct SyncEngine {
    store: StoreHandle,
    net: NetHandle,                 // Clone 的句柄，内部持有 tokio runtime
    cursor: i64,                    // 引擎自己缓存，避免跨线程同步读库
    titles_lang: Lang,
    backoff: Duration,
    active: bool,
    _pull_task: Option<Task<()>>,
    _push_task: Option<Task<()>>,
}

impl SyncEngine {
    pub fn pull_now(&mut self, cx: &mut Context<Self>) {
        if self._pull_task.is_some() { return; }
        let (store, net) = (self.store.clone(), self.net.clone());
        let (cursor, lang) = (self.cursor, self.titles_lang);
        self._pull_task = Some(cx.spawn(async move |this, cx| {
            // NetHandle::pull 内部 runtime.spawn 发请求，这里只 await 一个 oneshot。
            // 不要在 GPUI 执行器上直接 poll reqwest 的 future（它需要 tokio 的 I/O driver）。
            let result = net.pull(cursor, lang).await;
            this.update(cx, |engine, cx| engine.on_pull(result, cx)).ok();
        }));
    }

    fn on_pull(&mut self, result: Result<PullResponse, NetError>, cx: &mut Context<Self>) {
        self._pull_task = None;
        match result {
            Ok(pull) => {
                let more = pull.more;
                self.cursor = pull.cursor;
                let snapshot = self.store.apply_pull(pull);
                self.backoff = Duration::from_secs(1);
                cx.emit(SyncEvent::Pulled { cursor: snapshot.cursor, rows: snapshot.rows });
                if more { self.pull_now(cx); }
                self.push_pending(cx);
            }
            Err(e) => self.on_error(e, cx),
        }
    }
}
```

### 19.3 块渲染骨架

```rust
impl ArticleBody {
    fn render_block(&self, pair: &BlockPair) -> impl IntoElement {
        h_flex()
            .items_start()
            .gap_8()
            .child(div().flex_1().min_w_0().child(self.render_html(&pair.original)))
            .when(self.mode == ReadingMode::Side, |row| {
                row.child(div().flex_1().min_w_0().child(self.render_html(&pair.translated)))
            })
    }

    fn render_html(&self, html: &str) -> impl IntoElement {
        // M2：交给 tela-content 解析为 ParsedBlock，再用 StyledText 渲染
        // M1：TextView::html(ElementId::Name(block_id), html)
        TextView::html(self.next_id(), html)
            .selectable(true)
            .image_source(|uri| self.images.resolve(uri))
    }
}
```

### 19.4 参考

- Tela：`docs/ARCHITECTURE.md`、ADR 0009/0019/0022/0023/0025/0026/0052
- gpui-kit：`website/docs/{installation,packaging,auto-update,fonts,i18n,text-system}.md`、
  `skills/gpui-kit/references/{coding-guides,gpui/async,gpui/entity}.md`
- 关键源码：
  - `packages/sync/src/{protocol,rows,mutations,client}.ts`
  - `apps/reader/src/{store,lib}/*`
  - `crates/base/src/text/{text_view,state}.rs`
