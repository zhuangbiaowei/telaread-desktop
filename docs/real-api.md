# 真实 API 接入（v1）

> 依据：`https://github.com/hutusi/tela/blob/main/docs/api/authorization.md`、
> `docs/api/v1.md` 与 `docs/api/openapi.json`（2026-10-28 实读）。
> 本地缓存副本在 `tests/fixtures/api/`（官方示例的响应体）与 `tmp/api/`（不提交）。

本文描述 **R1：真实 API 底座** 的落地结果：协议类型、OAuth 登录、同步/推送、正文与翻译、
CLI 与本地库。GUI 的实时后台同步、Discover/Search、划线/文件夹 UI 属于后续切片。

---

## 1. 已实现的能力

| 能力 | 接口 | 代码 |
|---|---|---|
| 能力发现 | `GET /meta` | `tela_net::ApiClient::meta` |
| OAuth 2.1 + PKCE（S256） | `/connect`、`/oauth/token` | `tela-net::oauth` / `callback` / `browser` |
| 刷新令牌持久化 | 系统凭据库 | `tela-net::token_store` |
| 访问令牌缓存与单飞刷新 | — | `tela-net::session` |
| 全量/增量同步 | `GET /v1/sync?lang=&cursor=&types=` | `tela-sync::SyncEngine::pull` |
| 离线变更推送 | `POST /v1/mutations` | `tela-sync::SyncEngine::push` |
| 已读规则（5 条优先级） | — | `tela-proto::read_rule` |
| 正文对象 | `GET /v1/bodies?keys=` | `tela-content::http::HttpContentApi` |
| 翻译请求/状态/对象 | `/v1/translations`、`/o/t/…` | 同上 |
| 接收方/读取方 | `/v1/me`、`/v1/usage` | `ApiClient::me/usage` |
| 本地库 v2 | — | `tela-store`（schema v2） |
| GUI 账号面板 | — | `tela-ui::account` + `ShellView`：登录/等待授权/复制链接/退出 |
| CLI | — | `tela-app login/sync/list/read/mark-read/mark-unread/like/status/meta/demo` |

尚未实现（有意留到后续）：

- GUI 的前台/后台同步循环与列表自动刷新；
- Discover / 站点 / 个人页 / 博客卷 / 搜索等公共浏览页；
- 划线、文件夹、关注、OPML 的 UI（协议与 mutation 类型已在 `tela-proto` 就绪）；
- 图片代理 `/img/…` 的下载与缓存；
- 应用打包、自动更新。

---

## 2. 注册应用

在 Tela 注册一个 public native client，登记**回环** redirect URI：

```
http://127.0.0.1/callback
```

不要带端口：运行时应用监听任意空闲端口（RFC 8252 §7.3）。`localhost` 不会被接受，
必须写 `127.0.0.1` 或 `[::1]`。注册完成后拿到 `tela_app_…` 形式的 client id。

scope 选择：

- `read`：只读；
- `write`：可写（包含读）；
- 需要邮箱时追加 `email`，即 `write email`。

成员在授权页可以把 `write` 降成 `read`，也可以不勾邮箱；最终权限以令牌响应的 `scope`
和 `/v1/me` 的 `scope` 为准。

---

## 3. 快速开始

```bash
export TELA_CLIENT_ID=tela_app_…
# 可选：export TELA_SCOPES="write email"

cargo run -p tela-app -- login          # 打开系统浏览器，回环端口接收 code
cargo run -p tela-app -- sync           # 先推离线队列，再拉数据
cargo run -p tela-app -- list           # 订阅 + 未读数
cargo run -p tela-app -- list --feed 1  # 某个订阅的文章
cargo run -p tela-app -- read 101       # 三视图阅读；打开即已读（--no-mark-read 关闭）
cargo run -p tela-app -- mark-read 101  # 已读（入队并推送）
cargo run -p tela-app -- mark-unread 101
cargo run -p tela-app -- like 101       # 点赞；--unlike 取消
cargo run -p tela-app -- status         # 游标、行数、待发队列、今日额度
cargo run -p tela-app -- logout         # 撤销该设备令牌 + 清本地账号数据
```

`login` 的两种调试开关：

```bash
cargo run -p tela-app -- login --no-browser   # 只打印授权 URL，手动访问
cargo run -p tela-app -- login --ephemeral    # 刷新令牌只放内存（不落盘）
```

`--ephemeral` 只用于无桌面密钥环的环境或临时演示；正常安装**不要**使用。

---

## 4. 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `TELA_CLIENT_ID` | （空） | 注册得到的 `tela_app_…`；`login` 必需 |
| `TELA_SCOPES` | `write` | 空格分隔；需要邮箱时 `write email` |
| `TELA_API_BASE_URL` | `https://api.telaread.com` | API 基址（测试可指向本地） |
| `TELA_AUTH_BASE_URL` | `https://telaread.com` | 授权页站点（`/connect`） |
| `TELA_DB` | `$XDG_DATA_HOME/tela/tela.db` | SQLite 路径 |
| `TELA_LANG` | `zh-Hans` | 译文语言：`zh-Hans` / `zh-Hant` / `en` / `fr` |
| `TELA_TOKEN_STORE` | `keyring` | `memory` 只在测试/临时会话使用 |
| `TELA_DEMO` | （未设置） | GUI 默认 demo；`0` 表示真实 API |
| `TELA_DEV_SEED` | （未设置） | demo 装配是否写演示数据；`0` 关闭 |

注意：`TELA_CONTENT_API=http` 不再用于「凭空装配」真实后端——真实内容需要登录令牌，
由 `AppServices` 统一装配。`tela-app demo` 永远使用进程内 mock。

---

## 5. 令牌与安全

- **访问令牌**（`tela_at_…`）只放内存，并在 `Authorization: Bearer` 头里发送；绝不进 query。
- **刷新令牌**（`tela_rt_…`）只放系统凭据库：
  macOS Keychain / Windows Credential Manager / Linux Secret Service。
  服务名 `com.telaread.desktop`，账号名 `default`（多账号将改为按成员 handle 分键）。
- 每次刷新都会换新刷新令牌；**先落存储，再使用**。网络失败时保留旧令牌，两分钟内可原样重试。
- `invalid_grant` → 连接/设备已结束，只能重新 `login`；
  `unauthorized_client` → 应用被临时停用，**保留令牌**；
  `403 app_suspended` / `503 apps_paused` 同理。
- 401 `invalid_token` 由 `ApiClient` 自动刷新并**只重试一次**；仍失败就要求重新登录。

---

## 6. 同步语义

### 拉取

- `lang` 必填；`cursor` 不透明，与行数据同事务保存；
- `reset: true` 是快照：先清空该账号全部行，再写入本页；
- `more: true` 立刻用新 cursor 续拉；
- 语言或行类型变化时客户端**主动**从快照开始，不把旧 cursor 送出去；
- `removed.articles`：删文章，并级联删标题/状态/推荐/该正文的译本；私有划线保留；
- 退订（`subscriptions.deletedAt`）：删该 feed 的文章，但保留点赞、推荐、有划线的文章；
- `sites.translationAllowed == false`：删该站全部标题与正文译本，文章按原文显示。

### 推送

- 离线队列落在 `pending_mutations`，保留 `mid` 与 `at`，跨进程存活；
- 一次最多 50 条、1 MB；按入队顺序发送；
- 结果**按位置**匹配，`mid` 为 null 或与请求一致都接受；数量不符视为协议违约；
- 有答复后丢弃**所有**被答复的条目（applied 与 rejected 都算），随后拉取；
- 网络失败 / 5xx 时队列原样保留；绝不改写已发送的条目。

### 已读规则（顺序不能变）

1. `state.readAt` 非空 → 已读；
2. `state.markedUnread` → 未读（盖过水位线与地平线）；
3. 文章 id ≤ 订阅的 `watermarkId` → 已读；
4. `fetchedAt` 早于 `horizonDays`（30 天）→ 已读；
5. 其余 → 未读。

实现：`tela-proto::read_rule`，带命中规则枚举便于诊断与测试。

---

## 7. 本地库

schema v2：

| 表 | 用途 |
|---|---|
| `meta` | `TEXT` 键值：schema 版本、游标、语言、类型、账号 handle |
| `sync_rows` | `(entity, row_key)` + wire JSON；无 `seq` |
| `pending_mutations` | 自增 `seq` + 唯一 `mid` + `at` + wire JSON |
| `bodies` | `content_key` → 不可变正文 JSON |

迁移：v1（mock 协议，行带 `seq`、游标是整数）与真实 API 不兼容，打开时丢弃 `sync_rows`
与 `meta`，保留库文件本身。真实数据从登录后的第一次快照开始。

---

## 8. 错误与退出码

| 码 | 语义 |
|---|---|
| 10 | 本机配置/输入不合法（缺 client id、mutation 超限、fixture 坏了） |
| 11 | 本地库打开/迁移/读写失败 |
| 12 | 服务端响应违约 |
| 13 | 正文/渲染失败 |
| 20 | 网络或 API 失败（5xx、404、`invalid_request`） |
| 21 | 需要用户处理（未登录、`invalid_grant`、`app_suspended`、`subscription_limit`） |
| 22 | 需要等待（429、`post_limit` / `feed_limit`、`apps_paused`） |

每日额度（`post_limit` / `feed_limit`）在正文层转成 `TelaError::Limit`，带 `limit` 与
`resetsAt`；界面应显示「什么时候再能打开、可以订阅该博客」，并在 `resetsAt` 前不再请求。

---

## 9. 测试

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
env -u DISPLAY -u WAYLAND_DISPLAY cargo test -p tela-ui --features gui-test --locked
TELA_DB=target/ci-gui.db TELA_DEV_SEED=1 \
  cargo run -p tela-app --features gui --bin tela-gui --locked -- --check
```

真实 API 的契约测试不依赖账号：

- `crates/tela-proto/tests/v1_fixtures.rs`：官方示例响应体的逐字段解码；
- `crates/tela-sync/tests/sync_engine.rs`：快照/续拉/重试/推送队列；
- `crates/tela-net/tests/oauth_flow.rs`：回环回调、`state`/`iss` 校验、令牌轮换；
- `crates/tela-net/tests/api_client.rs`：401 刷新、错误映射、cursor 百分号编码；
- `crates/tela-content/tests/content_api.rs`：`/v1/bodies`、额度拒绝、译本对象。

在线冒烟（不需要令牌）：

```bash
TELA_TOKEN_STORE=memory TELA_DB=tmp/smoke.db cargo run -p tela-app -- meta
```

---

## 10. 下一步

1. GUI 的周期性同步与列表自动刷新（当前已在开窗前同步一轮）；
2. Unread / Today / Liked 列表与「全部已读 + 撤销」；
3. 划线/文件夹/关注/OPML 的 UI（mutation 类型与推送队列已就绪）；
4. Discover/Search/站点页（公共只读，`lang` 必带，利用边缘缓存）；
5. 图片代理与三平台打包。

---

## 11. 2026-10-28 真实环境实测

在 `api.telaread.com` 上用真实账号（3 个订阅）跑通了完整链路：

| 项 | 结果 |
|---|---|
| OAuth 登录（read → 重新授权 write） | 成功；Keychain 持久化，跨进程刷新令牌轮换成功 |
| `GET /meta` | 200，v1 preview，限额 50/1MB/25/30 天 |
| 首次同步 | 1 页快照，454 行；3 个订阅、274 篇文章、129 条标题译文、34 条译本行 |
| 增量同步 | `reset=false`、0 行变更，cursor 前进 |
| 正文 + 译文 | `read 6775` 拿到 14 段对照，译本已就绪 |
| 正文缓存 | 二次阅读命中 `bodies`，离线状态可见 |
| 写链路 | `mark-read` → applied → 拉回 state，未读消失；`mark-unread` → applied → 未读恢复 |
| 服务端持久化 | `sync --full` 快照后仍未读状态保持，正文缓存保留 |
| 只读令牌拒绝 | 用 read 令牌请求翻译返回 403 `insufficient_scope`，阅读不受影响 |

> 注意：真实账号的 handle 与本地库不写入文档；`tmp/tela.env` 里只放 public client id，
> 刷新令牌始终只在系统凭据库。
