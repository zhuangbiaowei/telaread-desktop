# M1 里程碑验收：协议 + 存储 + 拉取（mock 驱动）

> **历史文档（2026-10-28）**：本文记录的是真实 API 就绪前的 mock 协议阶段，路径、类型与
> 命令（`/api/v1/sync`、`tela-mock-server`、`seq`、tombstone）已不再对应现有代码。
> 当前实现见 [`real-api.md`](./real-api.md)。保留本文仅用于追溯当时的验收口径。


- 里程碑：**M1 — 协议 + 存储 + 拉取**（对应 `docs/development_plan.md` §9「M1」、§6 C1–C9/C13、§7 错误信息与退出码契约）
- 验收日期：**2026-10-26**（CST）
- 唯一真相源：上游 `R2 = .reference/tela/packages/sync/src/rows.ts` 与 `docs/api-proposal.md` §7.1，落地为 `crates/tela-proto`、`crates/tela-store`、`crates/tela-sync`、`crates/tela-net`
- 传输层现状：服务端 API 未就绪，M1 全链路以 **mock 传输层**驱动（`MockTransport`，内存脚本注入成功页与 401/409/传输错误，不建 socket、不发 HTTP）
- 机器核对：本文档全部「口径」小节由 `python3 scripts/check_m1_docs.py` 强制校验，文档与代码任一处不一致即 **非 0 退出**

## 0. 验收结论（结论先行）

| 验收门类 | 结论 | 判定依据 |
|---|---|---|
| 协议层（C1–C3、C13） | ✅ 通过 | `tela-proto` 34 个用例全绿：`pull_response` 8、`push_batch` 6、`sync_rows_default` 3、`error_codes` 4、`error_format` 7、`exports_contract` 2、`fixtures_sync` 2、`smoke` 1、`lib` 1 |
| 存储层（C4–C5） | ✅ 通过 | `tela-store` 10 个用例全绿：`migrate` 4、`rows` 4、`apply_pull` 2 |
| 拉取编排（C7） | ✅ 通过 | `tela-sync` `pull_e2e` 8 个用例全绿（分页 / 增量 / 重试 / 续跑 / 页预算 / 幂等） |
| 传输层与头部契约（C8/C9） | ⚠️ 部分通过 | mock 版已通（`tela-net` `mock_pull` 4 个用例）；`HttpTransport` 待服务端就绪后实现 |
| CLI 冒烟闭环（C12、§9 验收方式 2–4） | ❌ 未实现 | `crates/tela-app/src/main.rs` 仅支持 `--version`（24 行），无 `--self-check` |
| 文档 ⇄ 代码口径一致 | ✅ 通过 | `check_m1_docs.py` 输出 `OK 文档口径与代码一致：行表 17 张、常量 9 个、错误变体 10 个、拉取阶段 4 个`，`exit 0` |

**总结论：M1 的「协议 + 存储 + 拉取」三条主链路已在 mock 传输层下闭环，且结果可复现（`cargo test --workspace --locked` 58 passed / 0 failed）。由于 `tela-app --self-check` 未实现，M1 记为「主链路通过、CLI 冒烟待补」，不宣告全绿。**

## 1. 验收范围

### 1.1 本里程碑覆盖（in scope）

| 计划条款 | 内容 | 落地位置 |
|---|---|---|
| §6 C1 | `PullResponse` 解码与形状校验（未知字段、缺 cursor、未知表均报错） | `crates/tela-proto/src/pull.rs`、`tests/pull_response.rs` |
| §6 C2 | `SyncRows` 17 张行表与缺表容错（R1：缺表即空表） | `crates/tela-proto/src/rows.rs`、`tests/sync_rows_default.rs` |
| §6 C3 | `Mutation` 与批量上限（`MAX_MUTATIONS_PER_PUSH=50`） | `crates/tela-proto/src/mutation.rs`、`tests/push_batch.rs` |
| §6 C4 | `Store::open` 与迁移（`schema_version`、幂等初始化、禁止降级） | `crates/tela-store/src/{store.rs,schema.rs}`、`tests/migrate.rs` |
| §6 C5 | `Store::apply_pull`（增量 upsert、墓碑删除、游标提交） | `crates/tela-store/src/apply.rs`、`tests/apply_pull.rs` |
| §6 C7 | `SyncEngine`（分页、增量游标、重试、断点续跑、幂等） | `crates/tela-sync/src/pull.rs`、`tests/pull_e2e.rs` |
| §6 C8/C9 | `SyncTransport` trait + mock 传输层 + 头部契约常量 | `crates/tela-net/src/transport.rs`、`tests/mock_pull.rs` |
| §7 / §6 C13 | 错误三段式 `Display`、机器码、退出码 10/11/12/13/20 | `crates/tela-proto/src/error.rs`、`tests/{error_codes,error_format}.rs` |

### 1.2 明确不在本里程碑范围（out of scope）

- **真实 HTTP 传输与服务端联调**：服务端 API 未就绪，M1 的「联调」口径一律以 mock 传输层为准（见 §6 遗留项 2）。
- **`tela-app --self-check` 闭环**（§9 验收方式第 2–4 条）：属 C12 业务入口，当前未实现，不作为 M1 已达成项（见 §6 遗留项 1）。
- **C6 待发队列 / C10 `tela-content::parse_blocks` / C11 `tela-ui` 视图 / C12 `tela-app` 业务入口**：计划归入 M2 起，M1 不做。
- 内容渲染、正文解析、UI 表现：不在 M1 契约内。

### 1.3 验收环境（实测）

| 项 | 值 | 来源命令 |
|---|---|---|
| OS / 架构 | Linux x86_64 | `uname -sm` |
| Rust | rustc 1.98.1 (48a229cea 2026-09-01) | `rustc --version` |
| Cargo | cargo 1.98.1 (797e8a9bc 2026-08-05) | `cargo --version` |
| Python（文档校验脚本） | Python 3.14.8 | `python3 --version` |

## 2. 交付物清单

### 2.1 生产代码

| 路径 | 职责 | 关键事实 |
|---|---|---|
| `crates/tela-proto/src/rows.rs` | 17 张同步行表类型定义 | `SyncRows::TABLE_NAMES: [&'static str; 17]`，顺序逐字取自 R2 `rows.ts` |
| `crates/tela-proto/src/pull.rs` | `PullResponse` / `PullPage` 解码与形状校验 | 缺 cursor、未知表、seq 回归分别映射到对应错误变体 |
| `crates/tela-proto/src/mutation.rs` | `Mutation` 与 push 批量校验 | 超过 `MAX_MUTATIONS_PER_PUSH` 报 `TooManyMutations`（退出码 10） |
| `crates/tela-proto/src/error.rs` | `TelaError`：`code()` / `exit_code()` / `http_status()` / 三段式 `Display` | 401→`Auth`，409 `upgrade`→`UpgradeRequired`，409 `account_changed`→`AccountChanged` |
| `crates/tela-proto/src/lib.rs` | 协议级契约常量（7 个） | `MIN_CLIENT` / `CLIENT_HEADER` / `MEMBER_HEADER` / `PAGE_ROWS` / `HORIZON_DAYS` / `TITLE_IDS_MAX` / `MAX_MUTATIONS_PER_PUSH` |
| `crates/tela-store/src/store.rs` | SQLite 打开、迁移、行读写、游标 | `upsert_row` 用 `ON CONFLICT(entity, row_key) DO UPDATE`，幂等 |
| `crates/tela-store/src/schema.rs` | DDL | 全部 `CREATE TABLE/INDEX IF NOT EXISTS`，迁移单事务执行 |
| `crates/tela-store/src/apply.rs` | `apply_pull` 落库 | upsert + 墓碑删除 + 游标提交的原子语义 |
| `crates/tela-sync/src/pull.rs` | `SyncEngine` 编排 | 分页、`DEFAULT_MAX_ATTEMPTS=3` 同 cursor 重试、`DEFAULT_MAX_PAGES=1000` 页预算、`PullStage` 分类 |
| `crates/tela-net/src/transport.rs` | `SyncTransport` trait + `MockTransport` | 内存 `VecDeque` 脚本，可注入成功页与 401/409/传输错误 |
| `crates/tela-app/src/main.rs` | CLI 入口 | 仅 `--version`；`--self-check` 未实现（见 §6） |

### 2.2 测试与文档

| 路径 | 用例数 | 锁定内容 |
|---|---|---|
| `crates/tela-proto/tests/pull_response.rs` | 8 | `PullResponse` 解码、形状校验、未知表、seq 回归 |
| `crates/tela-proto/tests/push_batch.rs` | 6 | `Mutation` 形状与 50 条上限 |
| `crates/tela-proto/tests/sync_rows_default.rs` | 3 | 缺表即空表（R1）、未知表报错 |
| `crates/tela-proto/tests/error_codes.rs` | 4 | 机器码与退出码映射 |
| `crates/tela-proto/tests/error_format.rs` | 7 | 三段式 `Display`、禁止解析中文文案 |
| `crates/tela-proto/tests/exports_contract.rs` | 2 | 常量公开导出与取值 |
| `crates/tela-proto/tests/fixtures_sync.rs` | 2 | 快照 fixture 可解码 |
| `crates/tela-proto/tests/smoke.rs` | 1 | 协议层冒烟 |
| `crates/tela-store/tests/migrate.rs` | 4 | 新库迁移、重复 open 幂等、游标与行跨重开存活 |
| `crates/tela-store/tests/rows.rs` | 4 | 增删改查、重复写幂等、删除只作用于命名行 |
| `crates/tela-store/tests/apply_pull.rs` | 2 | 增量 upsert 不清表、墓碑删除、游标提交 |
| `crates/tela-sync/tests/pull_e2e.rs` | 8 | 全量 / 增量 / 分页 / 重试 / 续跑 / 页预算 / 重放页 |
| `crates/tela-net/tests/mock_pull.rs` | 4 | mock 不触网、脚本化错误、耗尽标记、`titles.lang` 透传 |
| `crates/tela-app/tests/cli_version.rs` | 2 | `--version` 输出 |
| `scripts/check_m1_docs.py` | 4 类口径 | 17 表 / 9 常量 / 10 错误变体 / 4 拉取阶段 ⇄ 代码双向校验 |

## 3. 契约常量与错误码

### 3.1 同步行表：17 张（真相：`crates/tela-proto/src/rows.rs::TABLE_NAMES`）

<!-- check:table-keys -->
共 17 张：`profile`、`prefs`、`subscriptions`、`feeds`、`sites`、`articles`、`titles`、`states`、`recommendations`、`highlights`、`claims`、`translations`、`follows`、`folders`、`feedFolders`、`blogrolls`、`blogrollBlogs`
<!-- /check:table-keys -->

容错口径（§6 C2 / §8.3）：缺表即空表（`#[serde(default)]`），未知表名必须报 `TelaError::Protocol{UnknownTable}`，不得静默吞掉——`sync_rows_default.rs` 与 `pull_response.rs` 双向锁定这两条。

### 3.2 契约常量：9 个（7 个协议级 + 2 个拉取默认）

真相源：`crates/tela-proto/src/lib.rs`、`crates/tela-sync/src/pull.rs`。

<!-- check:constants -->
MIN_CLIENT=3
CLIENT_HEADER=x-tela-client
MEMBER_HEADER=x-tela-member
PAGE_ROWS=1000
HORIZON_DAYS=30
TITLE_IDS_MAX=1000
MAX_MUTATIONS_PER_PUSH=50
DEFAULT_MAX_ATTEMPTS=3
DEFAULT_MAX_PAGES=1000
<!-- /check:constants -->

- `MIN_CLIENT=3` ↔ `docs/api-proposal.md` §7.1 的 `x-tela-client` ≥ 3：低于该版本的客户端由服务端回 409 `upgrade`，本地映射为 `TelaError::UpgradeRequired`（退出码 20）。
- `PAGE_ROWS=1000` 是**服务端分页上限**，超限属服务端违约 → `Protocol`（退出码 12）；`MAX_MUTATIONS_PER_PUSH=50` 是**本地调用方上限** → `TooManyMutations`（退出码 10）。两者语义不同，不可合并。
- `DEFAULT_MAX_PAGES=1000` 是分页安全阀：超过即 `PullStage::PageBudget` 终止，防服务端 `more=true` 死循环。

### 3.3 错误码：10 个变体（机器码 + 退出码）

真相源：`crates/tela-proto/src/error.rs` 的 `TelaError::code()` / `exit_code()`；`http_status()` 负责线上映射。

<!-- check:error-codes -->
Net net 20
Auth unauthorized 20
Store store 11
Protocol protocol 12
Fixture fixture 10
Content content 13
UpgradeRequired upgrade 20
AccountChanged account_changed 20
TooManyMutations too_many_mutations 10
Render render 13
<!-- /check:error-codes -->

退出码语义（§7.2）：**10** = 输入/fixture 不合法（含超限 push）、**11** = 本地库打开/迁移失败、**12** = 协议形状/seq 回归/cursor 非法、**13** = 正文内容或渲染失败、**20** = 网络/鉴权/版本/账号。

三段式 `Display` 由 `error_format.rs` 逐变体锁定：split 后必须恰好 3 段且非空，且禁止解析中文文案做判断（断言只读 `code()`）。

### 3.4 拉取层失败阶段：4 个

真相源：`crates/tela-sync/src/pull.rs::PullStage`，决定 `PullError::exit_code()` 与「已落库 N 页，可续跑游标 X」文案。

<!-- check:pull-stages -->
`Transport` → `Protocol` → `Apply` → `PageBudget`
<!-- /check:pull-stages -->

| 阶段 | 触发条件 | 行为 |
|---|---|---|
| Transport | 连不上 / 401 / 5xx / 传输层错误 | `Net` 类按 `DEFAULT_MAX_ATTEMPTS=3` 在**同一 cursor** 重试；`Auth` 类**立即终止不重试** |
| Protocol | 服务端数据不可信（形状错、未知表、seq 回归、cursor 非法） | 立即终止且不落库该页，退出码 12 |
| Apply | 本地落库失败（`Store`） | 终止，退出码 11 |
| PageBudget | 翻页数超过 `DEFAULT_MAX_PAGES` | 终止，防死循环 |

## 4. Mock 拉取验证证据

### 4.1 mock 传输层形态

- `crates/tela-net/src/transport.rs` 定义 `trait SyncTransport`，`MockTransport` 以内存 `VecDeque` 保存脚本，`script_pull` / `script_pull_error` 注入成功页与 401/409/耗尽错误。
- 测试通过 `&dyn SyncTransport` 调用入口，**不创建 socket、不发 HTTP**；真实实现只需实现同一 trait 即可替换。
- `mock_pull.rs` 4 个用例分别验证：脚本化成功页、脚本化错误码、脚本耗尽标记、每次 pull 的 `titles.lang` 记录。

### 4.2 `pull_e2e.rs` 8 个用例的断言证据

| 用例 | 验证点 | 关键断言 |
|---|---|---|
| `first_full_pull_lands_rows_and_advances_cursor` | 首次全量拉取 | 行数与内容符合快照、游标前进到末页值 |
| `incremental_pull_starts_from_local_cursor` | 增量拉取 | 请求 cursor 起点为本地 500；旧行 seq 不被改写 |
| `paginates_following_more_until_last_page` | 多页翻页 | `pull_calls` 的 cursor 序列为 `[0,100,200]`，`more=false` 停止 |
| `retries_transient_transport_error_on_same_cursor` | 传输错误重试 | `Net` 错误后在同一 cursor 重发，`attempts==3` 后成功 |
| `resumes_from_last_committed_page_after_failure` | 断点续跑 | 401 立即终止，`pull_calls().len()==2`、`applied_pages==1`、`resume_from==100` |
| `stops_when_page_budget_exceeded` | 页预算熔断 | `PageBudget` 阶段终止并给出 max_pages 文案 |
| `replayed_page_terminates_before_touching_store` | 重放页不落库 | 重放页在写 store 之前终止，计数不变 |
| `replayed_page_is_reported_as_protocol_stage` | 重放页归类 | 阶段为 `Protocol{page:2,cursor:500,object:"rows.articles[0].seq"}`，`exit_code==12` |

### 4.3 幂等落库证据

- `apply_pull.rs`：同页重复写入 `articles/44` 后 `row_count("articles")==1`、`inserted==2`；`states/42` 原地更新；`prefs/theme` 墓碑后 `get_row==None` 且 `cursor==18`。
- `rows.rs`：同一 `(entity,row_key)` 写两次仍 1 行、body 为最新值；不同 entity 的同名 `row_key` 互不干扰（`articles`/`feeds` 各 1 行）。
- 实现依据：`INSERT ... ON CONFLICT(entity, row_key) DO UPDATE SET seq=excluded.seq, body=excluded.body`，幂等与覆盖语义由主键约束保证。

### 4.4 切换到真实 API 的注入点

mock 仅替换传输层，编排逻辑不变：

1. `crates/tela-net/src/transport.rs`：新增 `HttpTransport` 实现 `SyncTransport`（头部拼装 `CLIENT_HEADER` / `MEMBER_HEADER`、状态码 → 错误变体映射、退避重试）。
2. `crates/tela-sync/src/pull.rs`：`SyncEngine::new` 的传输层注入点（当前接收 `&dyn SyncTransport`）。
3. `crates/tela-sync/src/pull.rs`：请求参数拼装与 `WireContext` 定位。

在此之前，M1 的一切拉取行为断言都只在 mock 语义下成立。

## 5. 测试结果

### 5.1 全局测试（mock 驱动，不触网）

命令：`cargo test --workspace --locked` → **exit 0**

| 测试目标 | 结果 |
|---|---|
| `tela-app` tests/cli_version.rs | 2 passed |
| `tela-net` tests/mock_pull.rs | 4 passed |
| `tela-proto` tests/error_codes.rs | 4 passed |
| `tela-proto` tests/error_format.rs | 7 passed |
| `tela-proto` tests/exports_contract.rs | 2 passed |
| `tela-proto` tests/fixtures_sync.rs | 2 passed |
| `tela-proto` tests/pull_response.rs | 8 passed |
| `tela-proto` tests/push_batch.rs | 6 passed |
| `tela-proto` tests/smoke.rs | 1 passed |
| `tela-proto` tests/sync_rows_default.rs | 3 passed |
| `tela-store` tests/apply_pull.rs | 2 passed |
| `tela-store` tests/migrate.rs | 4 passed |
| `tela-store` tests/rows.rs | 4 passed |
| `tela-sync` tests/pull_e2e.rs | 8 passed |
| `tela-sync` lib 单测 | 1 passed |
| **合计** | **58 passed / 0 failed**，另 1 个 doctest 标记 `#[ignore]` |

### 5.2 文档口径核对（红 → 绿全过程）

命令：`python3 scripts/check_m1_docs.py`

| 轮次 | 结果 | 说明 |
|---|---|---|
| 第 1 次 | `exit 1`，21 项不一致 | 本文档缺失导致 4 个 marker 块全缺；另 `development_plan.md` 残留旧口径「同步表的 15 个键名」，§6 C2 的 `SyncRows` 字段清单缺 `feedFolders` / `blogrollBlogs` |
| 第 2 次 | `exit 1`，剩 1 项 | `docs/m1_acceptance.md PullStage 不一致：doc=[] code=['Transport','Protocol','Apply','PageBudget']`——阶段块写成裸词而非反引号包裹，脚本取不到条目 |
| 第 3 次 | `exit 0` | 输出 `OK 文档口径与代码一致：行表 17 张、常量 9 个、错误变体 10 个、拉取阶段 4 个` |

该校验是**双向**的：既查文档漏写（少一张表即 FAIL），也查文档多写（常量块出现代码中不存在的名字同样 FAIL）。因此后续任何一方改动都必须同步另一方。

### 5.3 先红后绿的关键记录（防「测试假绿」）

| 行为 | 红证据 | 绿证据 |
|---|---|---|
| `apply_pull` 增量落库 | `delta_page` 曾报 `SeqRegression` 失败 | 修 fixture 后 `apply_pull` 2 passed |
| 重放页阶段归类 | 报 `left Apply{page:2,cursor:500} / right Protocol{...}` | 修 `pull.rs` 后同一命令 1 passed |
| 幂等写入 | `Store{Query}` 失败（重复写入应成功） | 改用 `ON CONFLICT DO UPDATE` 后 `rows` 4 passed |
| 游标持久化 | 「写游标应成功: Store{Query}」（当时为桩） | `migrate` 4 passed |

## 6. 遗留风险与后续项（不伪造通过）

| # | 项 | 现状 | 风险 / 影响 | 后续动作 | 归属 |
|---|---|---|---|---|---|
| 1 | `tela-app --self-check` 未实现 | `crates/tela-app/src/main.rs` 仅 `--version`（24 行）；fixture 已就位（`tests/fixtures/sync/{snapshot-minimal,invalid-missing-seq}.json`） | 计划 §9 验收方式第 2–4 条无法执行，M1 不能宣告全绿 | 实现 `--self-check --fixture ... --db ...`：合法 fixture 返回 0 且 stdout 含 `articles=1`、`unread=1`；invalid fixture 返回 10；`m1.db` 非空 | C12（M1 收尾） |
| 2 | 真实 HTTP 传输未实现 | 仅有 `SyncTransport` trait 与 `MockTransport` | mock 不覆盖头部拼装、真实状态码、退避与连接复用；联调风险顺延到 M2 | 在 `crates/tela-net` 内新增 `HttpTransport`，注入点见 §4.4；补齐头部契约测试 | M2（服务端就绪后） |
| 3 | 文档口径漂移 | 已统一为 17 表 / 9 常量 / 10 错误码 / 4 阶段 | 人工维护易再次漂移 | 已新增 `scripts/check_m1_docs.py` 将口径变成可执行断言，纳入验收 check | 已缓解（持续生效） |
| 4 | M2 内容未开始 | `tela-content::parse_blocks`（C10）、`tela-ui` 视图（C11）、`tela-app` 业务入口（C12）、`tela-store` 待发队列（C6）均未开工 | 不影响 M1 判定，但影响整体节奏 | 按计划 §9 顺序推进 M2 | M2 |
| 5 | mock 语义与真实服务端可能不一致 | mock 脚本由本地构造，未对齐真实分页/错误码分布 | 可能出现「mock 绿、联调红」 | M2 引入真实 API 后先做契约对比轮，保留 mock 作为回归基座 | M2 |

## 7. 复现命令

```bash
# 1) 全局测试（mock 驱动，不触网）
cargo test --workspace --locked

# 2) 文档 ⇄ 代码口径核对（不一致即非 0）
python3 scripts/check_m1_docs.py
```

期望：命令 1 输出 `58 passed / 0 failed`（exit 0）；命令 2 输出 `OK 文档口径与代码一致：…`（exit 0）。

## 8. 来源摘要

| 事实类别 | 来源 | 获取日期 |
|---|---|---|
| 17 张行表键名与顺序 | `.reference/tela/packages/sync/src/rows.ts`（R2）、`crates/tela-proto/src/rows.rs` | 2026-10-26 |
| 契约常量取值 | `crates/tela-proto/src/lib.rs`、`crates/tela-sync/src/pull.rs` | 2026-10-26 |
| 错误变体、机器码、退出码、HTTP 映射 | `crates/tela-proto/src/error.rs`；契约语义见 `docs/api-proposal.md` §7.1、`docs/development_plan.md` §7 | 2026-10-26 |
| 拉取阶段与重试/预算语义 | `crates/tela-sync/src/pull.rs::PullStage` | 2026-10-26 |
| 测试用例数与通过数（58 passed / 0 failed） | 本机实测 `cargo test --workspace --locked`（exit 0） | 2026-10-26 |
| 文档口径校验结果（红→绿） | 本机实测 `python3 scripts/check_m1_docs.py`（3 轮，末轮 exit 0） | 2026-10-26 |
| 里程碑范围与验收方式 | `docs/development_plan.md` §6 C1–C13、§9「M1」 | 2026-10-26 |

口径与不确定性说明：

- 所有测试数字为 2026-10-26 在本机（Linux x86_64 / rustc 1.98.1）实际运行所得，不代表其他环境；环境变化需重新执行 §7 命令复验。
- 「mock 通过」不等于「服务端联调通过」：M1 未与真实服务端交互，`HttpTransport` 与头部细节未经验证（见 §6 第 2、5 项）。
- `tela-app --self-check` 未实现属**未达成**项，本文档不作通过表述，任一条款若后续被实现须补充实测证据后再更新本表。
