# M2 里程碑验收：同步引擎 + 订阅面（mock 驱动）

> **历史文档（2026-10-28）**：本文记录的是真实 API 就绪前的 mock 协议阶段，路径、类型与
> 命令（`/api/v1/sync`、`tela-mock-server`、`seq`、tombstone）已不再对应现有代码。
> 当前实现见 [`real-api.md`](./real-api.md)。保留本文仅用于追溯当时的验收口径。


- 里程碑：**M2 — 同步引擎（拉取编排 / HTTP 传输 / 同步状态）+ 订阅面（创建 / 取消 / 列表 / 状态）**
- 验收日期：**2026-10-26**（CST）
- 契约来源：`docs/development_plan.md` §6 C7、§7.2 错误与退出码、`docs/api-proposal.md` §6.1/§7.1；wire 类型唯一真相源为上游 `R2 = .reference/tela/packages/sync/src/rows.ts`（17 张行表，落地于 `crates/tela-proto/src/rows.rs`）
- 传输层现状：**服务器端 API 仍未就绪**，M2 全链路以「本地可启动的 mock HTTP 服务」驱动（`tela-mock-server`），真实 API 对接**未联调、未验证**
- 前置里程碑：`docs/m1_acceptance.md`（协议 + 存储 + 拉取在内存 mock 传输层下闭环）
- 机器可复现性：本文档每条验收项都给出命令原文与实际输出摘要；未实测项一律标注「未验证」，不作通过表述

## 0. 验收结论（结论先行）

| 验收门类 | 结论 | 判定依据（命令 → 结果摘要） |
|---|---|---|
| mock 服务可启动 + 协议对齐（M2-S1） | ✅ 通过 | `cargo test -p tela-net --locked --test mock_server_bin` → **2 passed / 0 failed**；`--test mock_http_server` → **6 passed / 0 failed** |
| HTTP 传输层 `HttpTransport`（M2-S2，补 M1 遗留项 2） | ✅ 通过（仅对 mock） | `cargo test -p tela-net --locked --test http_transport` → **5 passed / 0 failed** |
| 同步引擎 `pull_once` 编排（C7） | ✅ 通过 | `cargo test -p tela-sync --locked --test pull_e2e` → **8 passed / 0 failed** |
| 同步引擎 HTTP 全链路（编排 + 传输 + 落库） | ✅ 通过（仅对 mock） | `cargo test -p tela-sync --locked --test http_sync_e2e` → **3 passed / 0 failed** |
| 同步状态可查询（M2-S3：游标 / 进度 / 最近结果） | ✅ 通过 | `cargo test -p tela-sync --locked --test sync_status` → **2 passed / 0 failed** |
| 订阅面（M2-S4：创建 / 取消 / 列表 / 状态） | ✅ 通过 | `cargo test -p tela-sync --locked --test subscriptions` → **4 passed / 0 failed** |
| 全仓回归（不触网，不依赖外部服务） | ✅ 通过 | `cargo test --workspace --locked` → **exit 0，82 passed / 0 failed / 1 ignored** |
| 对接真实服务器 API | ❌ 未验证 | 服务端 API 未就绪；本文档 §5 只给替换点与改造清单 |
| 409（`upgrade` / `account_changed`）真实响应分支 | ❌ 未验证 | `http.rs` 有映射分支，但 `http_transport` 5 个用例未覆盖 409，mock 服务也不注入 409 |
| CLI 业务入口（`tela-app` 同步/订阅子命令、`--self-check`） | ❌ 未实现 | `crates/tela-app/src/main.rs` 仍只有 `clap` 的 `--version`，无任何子命令 |

**总结论：M2 的「同步引擎 + 订阅面」两块能力已在 mock HTTP 服务下闭环，且结果可复现（全仓 82 passed / 0 failed）。但真实服务端联调、409 分支、CLI 入口均未完成，M2 记为「mock 侧验收通过、联调与产品入口待补」，不宣告全绿。**

## 1. 验收范围

### 1.1 本里程碑覆盖（in scope）

| 切片 | 内容 | 落地位置（项目相对路径） |
|---|---|---|
| M2-S1 | 本地可启动的 mock 同步服务：`/healthz` 探活、`/api/v1/sync?cursor=N[&titles=lang]` 返回 M1 协议 wire JSON、未知路径 404、`fail_next_sync` 失败注入 | `crates/tela-net/src/mock_http.rs` |
| M2-S1b | 一条命令启动 mock 服务的可执行程序（`--addr` / `--fail-next-sync` / `-h`） | `crates/tela-net/src/bin/tela-mock-server.rs` |
| M2-S2 | 真实 HTTP/1.1 传输层，把 `SyncTransport` 落到 socket 上；对接参数集中到 `HttpTransportConfig` | `crates/tela-net/src/http.rs` |
| C7 | `SyncEngine::pull_once`：跨页编排、逐页提交、重试、断点续跑、页预算、幂等 | `crates/tela-sync/src/pull.rs` |
| M2-S3 | `SyncEngine::status()`：游标 / 逐表进度 / 最近结果 / 最近错误 | `crates/tela-sync/src/status.rs` |
| M2-S4 | `SubscriptionService`：`subscribe` / `unsubscribe` / `list` / `status_of` / `sync_all` | `crates/tela-sync/src/subscription.rs` |
| 导出面 | `tela-sync` 对外公开 `SyncEngine`、`PullOptions`、`PullReport`、`SyncStatus`、`Subscription*` | `crates/tela-sync/src/lib.rs` |
| 文档 | README「M2 mock 服务」启动说明；本验收文档 | `README.md`（第 24–42 行）、`docs/m2_acceptance.md` |

测试文件（全部为项目相对路径）：

| 测试文件 | 覆盖对象 | 用例数（实测） |
|---|---|---|
| `crates/tela-net/tests/mock_http_server.rs` | mock 服务路由与假数据 | 6 |
| `crates/tela-net/tests/mock_server_bin.rs` | `tela-mock-server` 进程启动与响应 | 2 |
| `crates/tela-net/tests/http_transport.rs` | `HttpTransport` 头部、状态码映射、默认配置 | 5 |
| `crates/tela-net/tests/mock_pull.rs` | M1 内存脚本传输层（回归） | 4 |
| `crates/tela-sync/tests/pull_e2e.rs` | 分页 / 增量 / 重试 / 续跑 / 页预算 / 幂等（内存 mock） | 8 |
| `crates/tela-sync/tests/http_sync_e2e.rs` | 同一批语义在真 HTTP 上的端到端 | 3 |
| `crates/tela-sync/tests/sync_status.rs` | 状态快照（成功 / 失败两条路径） | 2 |
| `crates/tela-sync/tests/subscriptions.rs` | 订阅创建 / 列表 / 状态 / 取消 / 失败标记 | 4 |

### 1.2 明确不在本里程碑范围（out of scope）

- **真实服务端联调**：API 未就绪，M2 的「可用」口径仅限 mock（见 §6 第 1 项）。
- **push / mutation 上行**（C3 已定义类型，未接服务端）：`/api/v1/sync` 只做拉取。
- **`tela-content::parse_blocks`（C10）、`tela-ui` 列表与阅读视图（C11）、`tela-app` 业务入口（C12）、`tela-store` 待发队列（C6）**：均未开工。
- **多账号切换、鉴权令牌、限流与退避曲线**：`x-tela-member` 只做了「可配置则发送」，无账号切换逻辑。

### 1.3 验收环境（实测）

| 项 | 值 |
|---|---|
| 运行环境 | 本机 Linux x86_64，Cargo workspace（`--locked`，依赖锁定于 `Cargo.lock`） |
| 网络依赖 | 无。全部用例只访问 `127.0.0.1` 上由测试自己启动的 mock 服务（端口随机），不发外网请求 |
| 数据库 | 用例使用 `Store::open_in_memory()`，不写用户数据目录 |
| 验收执行日 | 2026-10-26（CST） |

## 2. mock 边界说明

### 2.1 如何启动（与 README 一致）

```bash
# 一条命令起 mock 同步服务（默认 127.0.0.1:8787；首行打印 "tela-mock-server listening on <base_url>"）
cargo run -p tela-net --bin tela-mock-server -- --addr 127.0.0.1:8787

# 另开终端：探活 + 拉首页（cursor=0）
curl -s http://127.0.0.1:8787/healthz
curl -s 'http://127.0.0.1:8787/api/v1/sync?cursor=0'
```

参数（实现见 `crates/tela-net/src/bin/tela-mock-server.rs`）：

| 参数 | 含义 | 默认 |
|---|---|---|
| `--addr <host:port>` | 监听地址；`127.0.0.1:0` 由系统分配空闲端口（测试用） | `127.0.0.1:8787` |
| `--fail-next-sync <n>` | 接下来 n 次 `/api/v1/sync` 返回 503（重试 / 续传演练） | `0` |
| `-h` / `--help` | 打印用法 | — |

### 2.2 启动与路由实测（本次执行）

执行命令与输出摘要（`--addr 127.0.0.1:0` 以避免端口占用；此命令一次完成「构建 → 启动 → 探活 → 拉三档游标 → 404 → 关闭」）：

```bash
cargo build -p tela-net --bin tela-mock-server --locked
./target/debug/tela-mock-server --addr 127.0.0.1:0 &
# 首行：tela-mock-server listening on http://127.0.0.1:46603
curl -s -i "$BASE/healthz"
curl -s "$BASE/api/v1/sync?cursor=0"
curl -s "$BASE/api/v1/sync?cursor=100"
curl -s "$BASE/api/v1/sync?cursor=120"
curl -s -o /dev/null -w '%{http_code}\n' "$BASE/api/v1/nope"
```

| 请求 | 实测结果（摘要） |
|---|---|
| `GET /healthz` | `HTTP/1.1 200 OK`，`Content-Type: application/json`，body `{"status":"ok","mock":true,"protocol":"m1"}` |
| `GET /api/v1/sync?cursor=0` | `{"cursor":100,"more":true,"reset":true,"rows":{"articles":[2 条]},"tombstones":[]}`（`articles` 内 `id` 1/2，`feedId` 1，`seq` 101/102） |
| `GET /api/v1/sync?cursor=100` | `{"cursor":120,"more":false,"reset":false,"rows":{"articles":[1 条]},"tombstones":[]}`（`id` 3，`seq` 120） |
| `GET /api/v1/sync?cursor=120` | `{"cursor":120,"more":false,"reset":false,"rows":{},"tombstones":[]}`（收敛空页） |
| `GET /api/v1/nope` | `404` |

### 2.3 假数据范围（实测口径）

| 维度 | mock 现状 |
|---|---|
| 页结构 | 三档：`cursor<100` → 快照页（`reset=true`、`cursor=100`、`more=true`）；`cursor<120` → 增量页（`reset=false`、`cursor=120`、`more=false`）；`cursor>=120` → 空页（游标不回退） |
| 行集合 | **仅 `articles` 一张表**（快照 2 行、增量 1 行，合计 3 行，均 `feedId=1`）；`tombstones` 恒为空 |
| `seq` 约束 | 每页 `seq` 严格大于请求游标（快照 101/102、增量 120），满足 M1 的 R5（`seq` 不得回退），可过 `PullResponse::validate_seq` 与 `Store::apply_pull` |
| `titles` 参数 | 被解析并记入请求日志（`MockRequest.titles_lang`），**不影响返回行集** |
| 失败注入 | `fail_next_sync(n)`：接下来 n 次 `/api/v1/sync` 返回 503；状态码在传输层归一为 `TelaError::Net`（可重试） |
| 并发 | 每次响应 `Connection: close`，无 keep-alive、无限流、无并发压测能力 |

### 2.4 与真实 API 的差异（mock 边界的全部含义）

| # | 差异 | 后果 |
|---|---|---|
| 1 | 只实现 HTTP/1.1 最小子集（标准库 `TcpListener`），无 TLS、无 HTTP/2、无压缩 | 若真实服务强制 HTTPS，需要在传输层加 TLS 栈（当前**不支持**） |
| 2 | 只有 `GET /healthz` 与 `GET /api/v1/sync`，**没有 push 端点** | 上行同步（C3 mutation）无法在 mock 上验证 |
| 3 | 假数据固定 3 行、单页 1–2 行，远小于 `PAGE_ROWS=1000` | 分页压力、大页解码性能未验证 |
| 4 | 不注入 409（`upgrade` / `account_changed`），只注入 503 | `http.rs` 的 409 分支**未被任何测试执行过**（未验证） |
| 5 | 无鉴权、无令牌刷新；`x-tela-member` 只是「配置了就发」 | 账号切换 / 会话失效路径未验证 |
| 6 | 假数据所有文章都属 `feedId=1`，且游标全局共享 | 「多订阅各自增量」的语义只在「游标收敛后都变空页」这一层面被验证（见 §6 第 6 项） |
| 7 | `README.md` 第 40–41 行称 `cursor=0` 返回 `feeds`/`articles`/`row_versions`/`feeds_rows`、增量页含「`row_versions` 删除标记」 | 与实测不符：实际只返回 `articles`，`tombstones` 恒空。**以代码与本文档实测为准**，README 该两行列入待修正（§6 第 5 项） |

## 3. 同步引擎验收

### 3.1 验收项与结果

| # | 验收项 | 可复现命令 | 实测结果 |
|---|---|---|---|
| S-1 | 首次全量拉取：跟 `more=true` 翻页直至收敛，行落库、游标推进 | `cargo test -p tela-sync --locked --test pull_e2e` | **8 passed / 0 failed**，含 `first_full_pull_lands_rows_and_advances_cursor`、`paginates_following_more_until_last_page` |
| S-2 | 增量拉取从「本地库游标」起跑，而非调用方传参 | 同上 | `incremental_pull_starts_from_local_cursor` ok |
| S-3 | 可重试错误（`TelaError::Net`）在同一游标重试 | 同上 | `retries_transient_transport_error_on_same_cursor` ok |
| S-4 | 第 N 页失败后从第 N-1 页游标续跑，已落库数据不丢 | 同上 | `resumes_from_last_committed_page_after_failure` ok |
| S-5 | 页预算 `max_pages` 生效（防 `more` 恒真的死循环） | 同上 | `stops_when_page_budget_exceeded` ok |
| S-6 | 重放同一页属协议违约，且**落库前**被拦 | 同上 | `replayed_page_terminates_before_touching_store`、`replayed_page_is_reported_as_protocol_stage` 均 ok |
| S-7 | 真 HTTP 端到端：首次同步落行且状态可查 | `cargo test -p tela-sync --locked --test http_sync_e2e` | **3 passed / 0 failed**，`first_sync_over_mock_http_lands_rows_and_status_is_queryable` ok |
| S-8 | 真 HTTP 端到端：重复同步幂等、游标不乱走 | 同上 | `repeated_sync_is_idempotent_and_keeps_cursor` ok |
| S-9 | 真 HTTP 端到端：503 注入被重试并从上次游标续传 | 同上 | `transient_errors_are_retried_and_sync_resumes_from_last_cursor` ok |
| S-10 | 传输层头部契约：`x-tela-client` 必发、`x-tela-member` 仅在配置时发 | `cargo test -p tela-net --locked --test http_transport` | **5 passed / 0 failed**，含 `pulls_snapshot_page_over_http_with_client_header`、`member_header_is_sent_only_when_configured` |
| S-11 | 传输层错误映射：连接失败 / 5xx → 可重试 `Net` | 同上 | `unreachable_endpoint_maps_to_net_error`、`server_error_maps_to_retryable_net_error` ok |
| S-12 | 默认配置指向文档记录的本地 mock | 同上 | `default_config_points_to_documented_local_mock` ok |
| S-13 | 同步状态：游标 / 逐表进度 / 总量 / 最近结果 | `cargo test -p tela-sync --locked --test sync_status` | **2 passed / 0 failed**，见 §3.3 |
| S-14 | 同步状态：失败后游标与进度不被清零，`last_error` 带阶段定位 | 同上 | `status_reports_last_error_and_keeps_persisted_progress` ok |
| S-15 | 409 `upgrade` / `account_changed` 映射 | — | **未验证**：无对应用例，mock 不注入 409 |
| S-16 | 真实服务端（HTTPS / 鉴权 / 大页 / push） | — | **未验证**：服务端 API 未就绪 |

### 3.2 编排合同（代码事实）

来源：`crates/tela-sync/src/pull.rs` 模块注释与实现。

- **起点游标来自本地库**（`Store::cursor()`）→ 进程重启、上次中途失败后再调用都自然续跑。
- **逐页提交**：每页先过 R5 的 `seq` 校验再 `Store::apply_pull`，一页一事务；第 N 页失败时第 1..N-1 页已在库里，游标停在最后一次成功页。
- **`more=true` 即续拉**，游标用本页返回的 `cursor`；连续页数超过 `PullOptions::max_pages` 视为分页未收敛。
- **重试按错误类别判定**：只有 `TelaError::Net`（5xx / 超时 / 连接失败）可重试，默认 `DEFAULT_MAX_ATTEMPTS = 3`（含首次）；鉴权、协议、版本、账号、本地库错误立即终止，不改写错误码。
- **默认页预算** `DEFAULT_MAX_PAGES = 1000`。
- **幂等**由 `Store::apply_pull` 的 `PRIMARY KEY (entity, row_key)` 保证，`PullReport.inserted` 不重复累加。
- **错误一律带阶段定位**：`PullError` 的 `Display` 含 `pull 第 N 页`、`cursor=`、尝试次数，并附底层 `TelaError` 三段式原因；`exit_code()` 与 `TelaError::exit_code()` 语义一致。
- `PullReport` 字段：`pages` / `inserted` / `deleted` / `cursor` / `attempts`（测试与状态查询均按此结构断言）。

### 3.3 同步状态实测断言（`sync_status` 2 个用例）

| 场景 | 断言（测试原文口径） | 结果 |
|---|---|---|
| 成功：两页脚本（游标 100 → 120，共 3 篇文章） | 同步前 `cursor=0`、`progress.total()=0`、`last_result=None`；同步后 `cursor=120`（且等于 `report.cursor`）、`progress.get("articles")=3`、`progress.total()=3`、`last_result=Some(report)`、`last_error=None` | ok |
| 失败：第 1 页成功、第 2 页连续两次 `Net`（`max_attempts=2`） | `cursor=100`（等于 `err.resume_from`）、`progress.get("articles")=1`、`progress.total()=1`、`last_result=None`、`last_error` 含 `pull 第 2 页` 与 `mock 503` | ok |

状态字段口径（`crates/tela-sync/src/status.rs`）：游标来自 `tela-store` 的 meta 表（跨进程持久）；进度按 `SyncRows::TABLE_NAMES` 的 R2 顺序逐表枚举，新增 wire 表无需改状态代码。

## 4. 订阅面验收

### 4.1 公开 API（代码事实）

来源：`crates/tela-sync/src/subscription.rs`（`pub fn` 实际清单）。

```text
SubscriptionService::new(&mut Store, &dyn SyncTransport)
SubscriptionService::with_options(self, PullOptions)
list(&self)    -> Result<Vec<Subscription>, SubscriptionError>       // 按 feed id 升序
status_of(&self, feed_id: i64) -> Result<Option<Subscription>, _>
subscribe(&mut self, feed_url: &str) -> Result<Subscription, _>
unsubscribe(&mut self, feed_id: i64) -> Result<bool, _>
sync_all(&mut self) -> Result<Vec<Subscription>, _>
SubscriptionStatus::{Synced, Pending, Error} + label() -> "synced"/"pending"/"error"
```

落库形态（写进同一张 `sync_rows` 表，按 wire 结构编码）：

- `subscriptions`：`row_key` = feed id 的十进制文本；
- `feeds`：`row_key` = feed id 的十进制文本，`status` 承载 `active`(=已同步) / `error` / `pending`，`last_error`、`last_fetched_at` 记录最近一次结果；
- 文章数按 `ArticleRow.feed_id` 统计（只读，不回写文章行）。

顺序契约：`SyncRows` 快照页（`reset=true`）会清空 `sync_rows`，而订阅行是客户端本地写入的同一张表，因此**必须先拉取、后写本地行**——`subscribe` / `sync_all` 两个入口都遵守该顺序。

### 4.2 验收项与结果

| # | 验收项 | 可复现命令 | 实测结果 |
|---|---|---|---|
| U-1 | 创建订阅即触发拉取，状态为 `Synced`，列表与 `status_of` 一致 | `cargo test -p tela-sync --locked --test subscriptions` | **4 passed / 0 failed**；`subscribe_pulls_mock_pages_and_list_reports_synced_status` ok |
| U-2 | 创建后本地事实：`articles=3`、`subscriptions` 1 行、游标 120、**mock 收到 2 次 `/api/v1/sync`**（快照页 + 增量页） | 同上 | 断言全部通过（同用例内） |
| U-3 | `unsubscribe` 删除订阅行与 feed 行，状态查询不再返回该订阅 | 同上 | `unsubscribe_removes_subscription_rows_and_status` ok |
| U-4 | `sync_all` 刷新在用订阅状态；**退订后不再发任何拉取请求** | 同上 | `sync_all_pulls_active_subscriptions_and_stops_after_unsubscribe` ok（断言 `after_second_sync == after_unsubscribe`） |
| U-5 | mock 返回错误时订阅状态如实记为 `Error` + `last_error`，列表仍可查询、进程不崩 | 同上 | `mock_error_marks_subscription_error_and_stays_queryable` ok（注入 `fail_next_sync(10)` 并耗尽默认 3 次重试） |
| U-6 | 多订阅各自独立增量（按 feed 隔离） | — | **未验证**：mock 假数据全部 `feedId=1`，且游标全局共享，无法区分「哪条订阅拉到了哪几行」 |
| U-7 | 订阅持久化跨进程（关掉再开仍能看到订阅） | — | **未验证**：订阅用例全部使用 `Store::open_in_memory()`；跨进程持久化仅在 `tela-store` 的 `migrate::cursor_and_rows_survive_reopen`（行 + 游标）层面被验证，未覆盖 `SubscriptionService` 视图重建 |

### 4.3 关键实测片段（订阅创建）

`subscribe_pulls_mock_pages_and_list_reports_synced_status` 的实际断言（测试文件原文口径）：

| 断言对象 | 期望值 | 结果 |
|---|---|---|
| `created.feed_url` | `https://mock.local/feed.xml` | ok |
| `created.status` | `SubscriptionStatus::Synced` | ok |
| `created.articles` | `3` | ok |
| `created.last_error` | `None` | ok |
| `list.len()` / `list[0].status` | `1` / `Synced` | ok |
| `status_of(created.feed_id)` | 与 `created` 完全相等 | ok |
| mock 收到的 `/api/v1/sync` 次数 | `2` | ok |
| `store.cursor()` | `120` | ok |
| `store.row_count("articles")` / `("subscriptions")` | `3` / `1` | ok |

## 5. 对接真实 API 的替换点与改造清单

### 5.1 替换点（改动半径）

| 层次 | 是否随真实 API 变化 | 位置 |
|---|---|---|
| wire 类型 / 错误码 / 退出码 | ❌ 不变 | `crates/tela-proto`（`PullResponse`、`SyncRows` 17 表、`TelaError`） |
| 落库与游标 | ❌ 不变 | `crates/tela-store`（`apply_pull`、`cursor` / `set_cursor`） |
| 拉取编排、重试、页预算、断点续跑 | ❌ 不变 | `crates/tela-sync/src/pull.rs` |
| 同步状态、订阅面 | ❌ 不变 | `crates/tela-sync/src/{status,subscription}.rs` |
| **服务地址 / 头部 / 超时** | ✅ 唯一配置点 | `crates/tela-net/src/http.rs::HttpTransportConfig`（`base_url`、`member_id`、`client_version`、`timeout`；常量 `DEFAULT_MOCK_BASE_URL = "http://127.0.0.1:8787"`） |
| **HTTP 细节（连接、读响应、状态码映射）** | ✅ 需按真实服务补齐 | `crates/tela-net/src/http.rs` 的 `SyncTransport for HttpTransport` 实现 |
| **mock 服务** | ✅ 直接替换为真实服务 | `crates/tela-net/src/mock_http.rs`、`bin/tela-mock-server.rs`（仅测试与本地联调使用） |

编排层只持有 `&dyn SyncTransport`（`SyncEngine::new(&mut store, &transport)`、`SubscriptionService::new(&mut store, &transport)`），因此换服务端**不动同步核心**。

### 5.2 改造清单（按序执行，每步都有验证方式）

| # | 改造项 | 涉及文件 | 具体改动 | 验证方式（就绪后执行） |
|---|---|---|---|---|
| 1 | 指向真实基址 | 调用方装配代码（当前无 CLI，需先有入口） | `HttpTransportConfig::new("https://<real-host>")`；或 `tela-mock-server --addr` 仅用于本地 | 启动后 `curl -s https://<real-host>/healthz` 返回 200 |
| 2 | TLS 支持 | `crates/tela-net/src/http.rs` | 当前**只支持 `http://`**（`split_base_url` 仅剥离 `http://` 前缀）；接 HTTPS 需引入 TLS 栈并扩 `split_base_url`/读写路径 | 新增 `https_transport` 用例，断言握手成功且响应可解码；失败路径归 `TelaError::Net` |
| 3 | 鉴权 / 账号 | `http.rs::HttpTransportConfig`、`tela-sync` 无需改 | 若真实服务需要 token/Cookie，新增头字段并在 `pull` 中发送；`member_id` → `x-tela-member` 已就绪 | 头部契约用例扩展（现有 `member_header_is_sent_only_when_configured` 为基线） |
| 4 | 409 分支实测 | `http.rs`（映射已实现，**未验证**） | 用真实或可注入 409 的 mock 跑通 `upgrade`（取 `minClient`）与 `account_changed` | 新增用例：409+`{"minClient":N}` → `UpgradeRequired{min_client:N}`；409+`account_changed` → `AccountChanged`；并断言编排层**不重试**（`is_retryable` 只认 `Net`） |
| 5 | 分页与过滤语义对齐 | 服务器端 + `tela-sync`（默认值不变） | 确认真实服务遵守 `cursor` 单调、`more` 语义、`PAGE_ROWS=1000`、`titles` 过滤；若服务端返回超过一页行数，`PullOptions::max_pages` 保留为保护 | 对真实服务跑一次 `pull_once`，断言 `report.pages >= 1`、`cursor` 单调不回退、逐表 `progress` 与响应行数一致 |
| 6 | 移除对 mock 的假设 | `crates/tela-sync/tests/*`（仅测试侧） | 把 `tests/subscriptions.rs`、`tests/http_sync_e2e.rs` 从「只跑 mock」扩展为「mock 为基座 + 真实服务可选套件（环境变量开关）」 | 两套都绿；真实套件默认 `ignored`，显式开关才跑 |
| 7 | push / mutation 上行（M3） | `tela-sync` 新增模块、服务端新增端点 | `PushBatch` 已定义（`MAX_MUTATIONS_PER_PUSH=50`），当前无发送路径；订阅行/feed 行本就是 wire 结构，可直接当 mutation 体制 | 同一 mutation 连推两次只生效一次（幂等断言，计划 M2 验收方式第 4 条） |
| 8 | CLI 入口（C12） | `crates/tela-app/src/main.rs` | 目前只有 `--version`；需加 `sync` / `subscribe` / `status` 子命令与 `--self-check` | 子命令退 0 且 stdout 含 `articles=<n>`；错误按 §7.2 退出码返回 |

## 6. 已知限制与后续事项（不伪造通过）

| # | 项 | 现状（可核实） | 影响 | 后续动作 |
|---|---|---|---|---|
| 1 | 真实 API 未联调 | 全链路只跑过 `127.0.0.1` 的 mock 服务；无任何真实服务请求 | 「mock 绿」不代表「联调绿」，可能出现字段/分页/错误分布不一致 | 服务端就绪后执行 §5.2 第 1–6 项，并把实测结果追加到本文档 |
| 2 | `HttpTransport` 不支持 HTTPS | `http.rs::split_base_url` 只识别 `http://` 前缀 | 若真实服务强制 TLS，传输层直接不可用 | §5.2 第 2 项 |
| 3 | 409 分支未被执行 | `http.rs` 有 `upgrade` / `account_changed` 映射代码，`http_transport` 5 个用例未覆盖 | 版本升级 / 账号切换的真实行为未知（`min_client` 解析用字符串切分，鲁棒性未验证） | §5.2 第 4 项 |
| 4 | CLI 无同步/订阅入口 | `crates/tela-app/src/main.rs` 仅 `--version`；`SubscriptionService` 只有库 API | 用户无法从命令行触发同步、查看订阅；计划 §9 的 CLI 冒烟验收（含 M1 遗留的 `--self-check`）仍无法执行 | 实现 C12 子命令后补 CLI 级验收 |
| 5 | `README.md` 假数据描述漂移 | README 第 40–41 行写 `feeds`/`row_versions`/`feeds_rows` 与「删除标记」；实测 mock 只返回 `articles`、`tombstones` 恒空 | 读者按 README 预期会找不到 `feeds` 行，误导联调 | 修正 README 第 40–41 行为实测口径（或让 mock 补回这些表，二选一并同步文档） |
| 6 | 订阅与 feed 的对应关系不被服务端验证 | mock 假数据全部 `feedId=1`，且 `SyncEngine` 的游标全局共享 | 多订阅场景下「每条订阅各自增量」的语义未验证；`created.articles=3` 反映的是「本地库里 feedId=1 的文章数」，不是「服务端按该 URL 返回的文章数」 | 真实服务按 `feed_id` 分发后再验；必要时把 `subscribe` 的统计口径改成「该订阅 URL 对应 feed 的文章数」 |
| 7 | 订阅视图未做跨进程验证 | 订阅用例全用 `Store::open_in_memory()` | 重启后 `list()` / `status_of()` 能否重建未实测（底层行与游标持久化已有 `migrate` 用例覆盖） | 增加「文件库 → 关 → 重开 → list() 仍返回该订阅」用例 |
| 8 | 无并发 / 无限流 / 无大响应覆盖 | mock 每次 `Connection: close`；`MAX_RESPONSE_BYTES = 32 MiB` 无用例覆盖 | 并发同步、超大页、恶意长响应未验证 | 补「响应超 32 MiB 应报错而非 OOM」用例；并发留到 UI 接入后 |
| 9 | UI / 内容 / push 未开工 | `tela-ui`、`tela-content` 仅为占位 crate（`src/lib.rs` 无业务实现）；`tela-store` 无待发队列 | M2 交付只到「同步 + 订阅」这一层，产品不可用 | 按计划 §9 顺序推进 C6 / C10 / C11 / C12 |
| 10 | 文档口径无机器校验 | M1 有 `scripts/check_m1_docs.py`，M2 尚无对应脚本 | 本文档的数字（用例数、路径）靠人工维护，易漂移 | 仿 M1 增 `scripts/check_m2_docs.py`，断言用例数与文件路径存在 |

## 7. 复现命令（逐条可执行）

```bash
# 1) 全仓回归（不触网；mock 服务由测试自己起，端口随机）
cargo test --workspace --locked
# 实测：exit 0，82 passed / 0 failed / 1 ignored

# 2) 同步引擎：分页/增量/重试/续跑/页预算/幂等（内存 mock 传输层）
cargo test -p tela-sync --locked --test pull_e2e
# 实测：8 passed / 0 failed

# 3) 同步引擎：真 HTTP 端到端
cargo test -p tela-sync --locked --test http_sync_e2e
# 实测：3 passed / 0 failed

# 4) 同步状态
cargo test -p tela-sync --locked --test sync_status
# 实测：2 passed / 0 failed

# 5) 订阅面
cargo test -p tela-sync --locked --test subscriptions
# 实测：4 passed / 0 failed

# 6) HTTP 传输层与 mock 服务
cargo test -p tela-net --locked --test http_transport     # 5 passed
cargo test -p tela-net --locked --test mock_http_server   # 6 passed
cargo test -p tela-net --locked --test mock_server_bin    # 2 passed

# 7) 手工起 mock 服务并探活/拉页（默认端口）
cargo run -p tela-net --bin tela-mock-server -- --addr 127.0.0.1:8787
# 首行：tela-mock-server listening on http://127.0.0.1:8787
curl -s http://127.0.0.1:8787/healthz
# 实测：{"status":"ok","mock":true,"protocol":"m1"}
curl -s 'http://127.0.0.1:8787/api/v1/sync?cursor=0'
# 实测：{"cursor":100,"more":true,"reset":true,"rows":{"articles":[…2 条…]},"tombstones":[]}

# 8) 失败注入演练（接下来 3 次 /api/v1/sync 返回 503）
cargo run -p tela-net --bin tela-mock-server -- --addr 127.0.0.1:8787 --fail-next-sync 3
```

期望：命令 1 输出 `82 passed / 0 failed`（另有 1 个 `ignored`，为文档示例，不计入通过数）；命令 2–6 全部 `0 failed`。任一条红即说明代码或环境已变化，本文档数字需重新测量后更新。

## 8. 交付状态结论

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M1 — 协议 + 存储 + 拉取 | ✅ 主链路通过（遗留项部分回收） | 82 passed 中包含 M1 全部用例；M1 遗留项 2「真实 HTTP 传输未实现」已在 M2 落地为 `HttpTransport`（仅对 mock 验证）。M1 遗留项 1「`tela-app --self-check` 未实现」**仍未回收**；遗留项 5「mock 语义与真实服务端可能不一致」仍未消除 |
| M2 — 同步引擎 + 订阅面 | ⚠️ mock 侧通过，联调未开始 | 同步引擎（分页 / 增量 / 重试 / 续跑 / 幂等 / 状态）与订阅面（创建 / 取消 / 列表 / 状态 / 失败标记）在 mock HTTP 服务下全部实测通过；真实 API、HTTPS、409 分支、CLI 入口未完成，按 §6 清单顺延 |
| 产品可用性 | ❌ 不可交付用户 | 无 CLI 业务入口、无 UI、无 push；当前形态是可测试的同步内核 + 订阅服务层 |

**给下一阶段的明确交接：**（a）服务端就绪后，先按 §5.2 第 1–6 项做一轮契约对比，任何不一致回到 `crates/tela-proto` 定契约，不在 `tela-sync` 里打补丁；（b）`tela-app` 子命令与 `--self-check` 是实现「可交付」的下一道硬门槛；（c）订阅的 `articles` 统计口径（§6 第 6 项）需在真实多 feed 数据上重新定义，避免把「本地库同 feedId 文章数」当成「该订阅的文章数」对外承诺。

## 9. 来源摘要

| 事实类别 | 来源（项目相对路径） | 获取日期 |
|---|---|---|
| mock 服务实现、路由、假数据档位、失败注入 | `crates/tela-net/src/mock_http.rs` | 2026-10-26 |
| mock 服务启动参数与首行输出契约 | `crates/tela-net/src/bin/tela-mock-server.rs`、`README.md` 第 24–42 行 | 2026-10-26 |
| HTTP 传输层配置项、错误映射、`DEFAULT_MOCK_BASE_URL`、`pull_url` 拼装 | `crates/tela-net/src/http.rs` | 2026-10-26 |
| 拉取编排合同、默认常量（3 次尝试 / 1000 页）、`PullReport` 字段 | `crates/tela-sync/src/pull.rs` | 2026-10-26 |
| 同步状态字段与进度枚举口径 | `crates/tela-sync/src/status.rs`、`crates/tela-sync/tests/sync_status.rs` | 2026-10-26 |
| 订阅面 API、落库形态、顺序契约、异常路径 | `crates/tela-sync/src/subscription.rs`、`crates/tela-sync/tests/subscriptions.rs` | 2026-10-26 |
| 导出面 | `crates/tela-sync/src/lib.rs` | 2026-10-26 |
| 用例数与通过数（82 / 0 / 1 ignored；各套件 8/3/2/4/5/6/2） | 本机实测 `cargo test --workspace --locked` 与各 `--test <suite>` 命令（2026-10-26，exit 0） | 2026-10-26 |
| mock 服务实际响应（healthz、cursor=0/100/120、404、监听地址） | 本机实测 `tela-mock-server --addr 127.0.0.1:0` + `curl`（2026-10-26） | 2026-10-26 |
| M1 遗留项状态 | `docs/m1_acceptance.md` §6 | 2026-10-26 |
| 里程碑范围、C7/C12 与验收方式 | `docs/development_plan.md` §6、§7.2、§9 | 2026-10-26 |

口径与不确定性说明：

- 所有测试数字为 2026-10-26 在本机（Linux x86_64，`--locked`）实际运行所得，不代表其他环境；环境或代码变化后必须重跑 §7 命令再更新本文档。
- 「mock 通过」不等于「服务端联调通过」：M2 未与任何真实服务端交互，HTTPS、鉴权、409、push、大页分页均未验证（§3.1 S-15/S-16、§5.2、§6）。
- 全文未实测项均显式标注「未验证」，不计入通过；若某项后续被实现或验证，须补充实测输出后再修改本文档对应行。


## 10. 补记（2026-10-27）：GUI 切片补齐

本文件 §8 的“产品可用性 ❌ 无 UI”是 **2026-10-26 的快照**，保留作历史记录；此后已按
[`development_plan.md`](./development_plan.md) §9.2 完成 S0–S4：

- S1 应用装配：`crates/tela-app/src/services.rs`（Store + ContentApi + SyncTransport、空库 seed），
  `cargo test -p tela-app --test services` → 8 passed；
- S2 三栏 shell：`crates/tela-ui/src/shell.rs` + `gui/shell.rs`，纯模型 5 + GUI 2 passed；
- S3 阅读面板：`crates/tela-ui/src/gui/reader.rs`，`gui_reader` 4 passed（三视图/翻页/失败重试）；
- S0/S4：`gui_smoke` 1 passed、`gui` CI job、`tela-gui --check` → `feeds=2 articles=4 unread=3`。

GUI 验收的逐条命令、截图、限制与 Q-13 结论见 [`gui_acceptance.md`](./gui_acceptance.md)。
本文件 §8 的“CLI 业务入口未实现”仍成立；`tela-app` 的终端入口仍只有 `--version`/阅读参数，
同步循环尚未自动调度（`AppServices::sync_once()` 已提供）。
