# Tela Desktop 开发计划

> 版本：v1（2026-10-26）
> 作者：架构步骤产出，供后续 coding / deploy / write 步骤共同使用
> 状态：**计划**。凡标注「待确认」的条目均未经作者或团队确认，不得当作既定事实实现。
> 唯一交付路径：`docs/development_plan.md`（本文件）。实现代码路径契约见第 5 章。
>
> **执行状态（2026-10-27）**：M0–M3 已落地，M2/M3 的 GUI 缺口由 S0–S4 补课关闭
> （见 §9.2 与 [`gui_acceptance.md`](./gui_acceptance.md)）；M4/M5 未开始。
> 第 2 章“现状评估”是 2026-10-26 的计划基线快照，保留不动，不要当成当前状态。
>
> **执行状态（2026-10-28，R1）**：官方 API v1 文档（`docs/api/authorization.md`、`v1.md`、
> `openapi.json`）到位后，协议/同步/正文/认证已按真实 API **重写**，落地说明见
> [`real-api.md`](./real-api.md)，逐项剩余工作与估算见
> [`remaining-work.md`](./remaining-work.md)。本文件 §6/§8 里基于 mock 协议的契约（`seq`、
> tombstone、`x-tela-client`、`/api/v1/sync`）只作历史记录，**不再代表当前实现**；M4 的
> 收藏/文件夹/OPML/搜索将以 v1 mutation 与公共端点继续。

---

## 0. 这份计划解决什么问题

本仓库当前**只有设计文档，没有一行代码**（证据见 3.1）。所以本计划不是"在既有代码上加功能"，而是：

1. 把三份设计文档里已经写死的契约（协议常量、行字段、crate 划分、版本锁定、里程碑）**固化成可验收的切片顺序**；
2. 为每个跨模块接口定义**符号名 + 最小失败测试签名**，使后续 coding 步骤能按「先写失败测试（红）→ 再实现（绿）」推进，而不是先实现再猜测试；
3. 把文档之间的冲突、过期信息和缺口集中到第 12 章「待确认项」，避免 coding 步骤在歧义处自行发挥。

**本计划不新增产品需求。** 凡文档未写的需求，一律不进入范围，只以「待确认」形式记录。

---

## 1. 依据文档清单与出处

### 1.1 仓库内文档（逐份实读）

| # | 文件 | 规模 | 实读的关键章节 | 在本计划中的用途 |
|---|---|---|---|---|
| D1 | `README.md` | 2102 B / 39 行 | 全文；「文档」表、「三句话结论」、「调研基线」、「下一步」、「免责声明」 | 仓库当前状态（无代码）、调研基线版本、文档索引 |
| D2 | `docs/api-proposal.md` | 51168 B / 1226 行 | §0 TL;DR、§1 文档范围、§2 现状、§12「服务端改动清单（提议，按阶段）」含「阶段 0：不做任何改动也能跑（1 天）」、§13 需要和作者讨论的问题 | 客户端 API 现状、认证方案的阶段划分、与作者待议事项 |
| D3 | `docs/architecture.md` | 64906 B / 1570 行 | §1 定位与硬约束、§2.3 crate 划分、§3 技术选型表、§3.1 版本锁定（含 workspace `Cargo.toml` 片段）、§18 里程碑 M0–M4 | 技术选型、crate 边界、依赖版本、里程碑骨架 |
| D4 | `docs/product-design.md` | 35582 B / 734 行 | §5.1 P0、§5.2 P1、§5.3 P2、§5.4 明确不做、§13 路线图（v0.1–v1.0 + v1.x）、§14 与 Web 的功能差异、§15 待确认的产品问题（15 条）、§16 风险（8 条） | 范围（做/不做）、周次节奏、产品侧待确认清单 |

### 1.2 只读上游参考（非本仓库构建物）

`README.md` 明确：`.reference/` 仅用于查阅上游源码，已被 `.gitignore` 忽略，不参与本仓库构建。本计划引用其作为**外部协议事实的唯一来源**，不作为可修改代码。

| # | 路径 | 用途 |
|---|---|---|
| R1 | `.reference/tela/packages/sync/src/protocol.ts` | pull 协议常量与 `PullResponse` 形状（`HORIZON_DAYS`/`PAGE_ROWS`/`TITLE_IDS_MAX`/`MIN_CLIENT`/`NAMES_MEMBER_SINCE`/`CLIENT_HEADER`/`MEMBER_HEADER`） |
| R2 | `.reference/tela/packages/sync/src/rows.ts` | `SyncRows` 的 17 张表键名、各行类型字段、`Tombstone` 形状 |
| R3 | `.reference/tela/packages/sync/src/mutations.ts` | `mutationSchema`（`discriminatedUnion('type')`）、`MAX_MUTATIONS_PER_PUSH = 50`、`PREF_KEY`、`PREF_MAX_BYTES = 2048`、`RECOMMENDATION_NOTE_MAX = 500` |
| R4 | `.reference/tela/apps/reader/worker/edge.ts` | 写请求集合与跨源写拦截（`:122` `WRITES = new Set(['POST','PUT','PATCH','DELETE'])`；`:553-554` `cross-origin write refused` / 403） |
| R5 | `.reference/gpui-kit/crates/kit/Cargo.toml` | `name = "gpui-kit"`、`version = "0.8.0"`、`default = ["component","assets"]`、`test-support` feature 存在 |
| R6 | `.reference/gpui-kit/crates/kit/TESTING.md` | 上游 UI 测试约定（2026-10-27 已读；实际改用 GPUI 自带 test-support，见 Q-9） |

> 说明：本计划没有引用任何仓库外网络资料。版本时效判断以当前日期 2026-10-26 为准：`README.md` 的调研基线为 `hutusi/tela @ f57fa6f（2026-10-25）`、线上实测 2026-10-25，距今 1 天，可视为新鲜；`gpui-kit 0.8.0` 是否为当前最新版本**未核实**（见 Q-10）。

---

## 2. 现状评估

### 2.1 代码与测试现状：**不存在**

| 检查 | 结果 | 证据 |
|---|---|---|
| 根目录是否有 Rust 工程 | 否 | 根目录 `*.toml` glob 命中 **0** 个；`docs/architecture.md` §3.1 给出的 `Cargo.toml` 仍是"待创建"片段 |
| 是否有 `crates/` 目录与 7 个 crate | 否 | 根目录只有 `README.md`、`docs/`、`.reference/` |
| 是否有测试文件 / 可运行测试 | 否 | 无任何 `tests/`、`#[test]`、`#[gpui_kit::test]` |
| 文档是否自述"无代码" | 是 | `README.md`：「> 当前仓库只有设计文档，还没有代码。」 |

**结论：本计划无法做"文档描述 vs 现有代码"的差异比对，因为没有代码可比对。** 因此第 2.2 节改为做**文档 vs 上游参考源码**的一致性抽查，这是当前唯一可执行的"核对"，也是最容易在实现阶段踩坑的地方。

### 2.2 文档 ↔ 上游参考源码 抽查一致性（本步骤实做）

| 文档断言 | 文档出处 | 上游证据 | 结论 |
|---|---|---|---|
| 客户端协议版本下限为 3，服务端对更老客户端回 409 `upgrade` | 未在 D2/D3 直接引常量名 | `protocol.ts`：`export const MIN_CLIENT = 3`，注释写明 409 `upgrade` | **一致**（常量名与语义确认） |
| 请求头 `x-tela-client` | D2 提议 Cookie 兼容时伪造 `x-tela-client: 3`（§12 阶段 0） | `protocol.ts`：`CLIENT_HEADER = 'x-tela-client'` | **一致** |
| 每页每表 1000 行 | D3 同步章节 | `protocol.ts`：`PAGE_ROWS = 1000` | **一致** |
| 推送每批最多 50 条 | D3 同步章节 | `mutations.ts`：`MAX_MUTATIONS_PER_PUSH = 50` | **一致** |
| pull 返回 `cursor`/`more`/`reset`/`rows`/`tombstones` | D3 同步章节 | `protocol.ts`：`PullResponse` 同时含可选 `titlesLang` | **文档未提 `titlesLang`**，见 Q-3 |
| 跨源写请求被 403 拒绝 | D2 §12 阶段 0（需伪造 `Origin`） | `edge.ts:122`、`:553-554` `cross-origin write refused` / 403 | **一致**（Cookie 兼容方案的边界条件被证实存在） |
| `gpui-kit = "0.8.0"` | D3 §3 选型表 + §3.1 | `crates/kit/Cargo.toml`：`version = "0.8.0"` | **一致**（包名 `gpui-kit`，非 `gpui_kit` crate 名在 Cargo 中写 `gpui-kit`） |
| `gpui-kit` 把 `gpui-pre` 精确钉在 `=0.3.9` | D3 §3.1 注释 | 本次读取的 `crates/kit/Cargo.toml` 被截断，**未直接看到该依赖行** | **未验证**，见 Q-8 与 §2.4 验证命令 |
| `.reference/` 已被 `.gitignore` 忽略 | D1 调研基线段 | 本次未读取 `.gitignore`（工具未列隐藏文件） | **未验证**，见 Q-7 |
| 同步表的 17 个键名 | D3 SQLite schema 章节 | `rows.ts`：`profile, prefs, subscriptions, feeds, sites, articles, titles, states, recommendations, highlights, claims, translations, follows, folders, feedFolders, blogrolls, blogrollBlogs` | **键名可作为契约**（本计划 §8.1 直接采用；初稿记作 15 个，已按 R2 上游更正为 17，见 §8.1「M1 口径修正」） |

> 差异处理原则：以上"一致"项在实现中直接使用；"未验证/不一致"项一律进第 12 章，**不允许 coding 步骤按猜测实现**。

### 2.3 文档之间的一致性检查

| 主题 | D4 产品设计 | D3 架构设计 | 判断 |
|---|---|---|---|
| 版本节奏 | §13：v0.1 第 1–2 周 … v1.0 第 15 周 | §18：M0 1–2 周 … M4 | **一致**（周次口径相同），映射见 §9 |
| 范围边界 | §5.4 明确不做：自抓 RSS / 评论 / 算法推荐 / 移动端 / 服务端全文搜索 / 全文再分发 | 未提相反内容 | **一致** |
| 首版登录方式 | §5.1 P0：设备授权登录（邀请制），"服务端未就绪时用 Cookie 兼容" | 未固定顺序 | **一致但节奏有歧义**，见 Q-1 |
| 高亮方案 | §15 内部问题 13：M1 可整段、M2 做选区 | §18 M2「高亮/笔记」 | **一致**，选区 vs 整段的最终选择仍是待确认（Q-4） |
| 翻译渲染方式 | 未指定实现手段 | §18 M2 提到「轻量 WebView 或重投影」 | **架构侧仍是候选方案**，见 Q-5 |
| 待作者确认清单 | §15 有 10 条"与作者"问题 | D2 §13 另有一份讨论清单 | **重复/重叠**，见 Q-6（必须合并后再对外提问，否则会向作者重复提问） |

### 2.4 未验证假设（本步骤**无法**执行命令，原因与验证方式一并给出）

当前执行环境本步骤只有文件读写工具，**没有 shell 执行工具**（工具清单中不存在 `box_bash`），因此以下假设不能在本步骤冒烟验证。全部标注「未验证假设」，并给出后续步骤必须执行的第一条命令与判定标准。

| # | 未验证假设 | 验证命令 | 通过判定 | 失败时修复 |
|---|---|---|---|---|
| A1 | 沙箱/目标机有 Rust 工具链且 ≥ `1.92` | `rustc --version && cargo --version` | 输出 `rustc 1.9x`，x≥2；退出码 0 | `rustup toolchain install 1.92 && rustup default 1.92` |
| A2 | edition 2024 可用 | `cargo new --help >/dev/null; grep -c 'edition = "2024"' Cargo.toml` | 命中 1 | 升级 toolchain；退回 2021 需**先**改 D3 §3.1 契约（Q-11） |
| A3 | `gpui-kit` 能在 Linux 上编译（需要 X11/Wayland、fontconfig 等系统库） | `cargo build -p tela-ui --features gpui-kit/test-support` | 退出码 0 | 按报错安装（Debian: `libxkbcommon-dev libwayland-dev libfontconfig1-dev libssl-dev`），并把安装命令写进 README |
| A4 | UI 测试可在无显示器环境运行 | `cargo test -p tela-ui -- --nocapture`（如有需要先 `xvfb-run -a cargo test -p tela-ui`） | 退出码 0，测试数 ≥1 通过 | 若必须显示器，则在 CI 用 `xvfb-run`；仍失败则 UI 层只做纯逻辑测试（Q-13） |
| A5 | 本机可访问 `https://telaread.com`（真联调） | `curl -sS -o /dev/null -w '%{http_code}\n' https://telaread.com/` | 非 5xx/000 | 无网络时全部联调改用 §8 的 golden fixture，联调项标为待办 |
| A6 | 有可用的邀请制账号（Cookie 兼容模式前提） | 手工：浏览器登录后导出 Cookie jar，`curl -b jar.txt https://telaread.com/api/v1/me` | 200 且含 member 字段 | 无账号则 M1 只做到 fixture 级，Q-2 列为阻塞项 |
| A7 | `rusqlite` bunded 编译通过 | `cargo build -p tela-store` | 退出码 0 | 装 `cc`/`pkg-config`；必要时用系统 sqlite（需改 D3 §3.1） |

> 规则：以上 A1–A7 中任一条在 M0 阶段失败，**不得**继续 M1（见 §9 每阶段"门禁"）。

---

## 3. 项目目标与范围

### 3.1 目标（v1.0）

做一个**本地优先**的 Tela 桌面阅读器（Rust + GPUI Kit）：复用 Tela 已有的同步协议与不可变对象作为客户端 API，桌面端只补齐"非浏览器客户端入口 + 桌面级阅读体验"。核心差异点（`D1` 三句话结论第 3 条、`D4` §16）：**本地优先 + 原生翻译对照 + 桌面阅读体验**。

### 3.2 范围内（P0，v0.1–v0.3，摘自 D4 §5.1）

账号（设备授权登录/Cookie 兼容、退出、切换）、订阅（URL 添加、OPML 导入导出、文件夹、取消）、阅读（三栏、Unread/Today/Liked、已读/未读、全部已读+撤销）、正文（块级渲染、图片、链接、代码、引用、表格）、翻译（自动/手动、side/trans/orig、流式分块、失败与额度提示）、收藏（点赞、推荐+笔记、高亮、笔记）、搜索（本地标题/摘要 + 服务端补全）、设置（阅读/翻译/账号/缓存）、同步（自动拉取/推送、离线队列、状态显示）、系统（快捷键、命令面板、深浅色、窗口记忆）。

### 3.3 范围外（D4 §5.4，硬边界，不进入任何里程碑）

自抓 RSS、评论/私信/群组、算法推荐信息流、广告与付费墙、移动端、服务端全文搜索、内容再分发/导出全文。

### 3.4 P1/P2 只登记不排期

P1（Discover 五 tab、关注与 Following、博客卷读写、通知、多窗口、导出 Markdown、打磨）与 P2（作者 dashboard、邀请管理、资料、分享卡片、TTS）按 D4 §5.2/§5.3 登记在 §9 的 M5 之后，**本计划不为其排具体周次**，避免与 D4 §13 的 v0.4–v1.x 冲突。

---

## 4. 目录与路径契约（后续所有步骤必须沿用）

| 用途 | 路径 | 依据 / 说明 |
|---|---|---|
| 文档 | `docs/` | 既有 3 份设计文档 + 本计划，均在 `docs/` |
| workspace 根 | `Cargo.toml`（`resolver = "2"`，`members = ["crates/*"]`） | D3 §3.1 |
| 7 个 crate | `crates/tela-proto`、`crates/tela-store`、`crates/tela-sync`、`crates/tela-net`、`crates/tela-content`、`crates/tela-ui`、`crates/tela-app` | D3 §2.3 + §3.1（`members = ["crates/*"]`）+ §18 M0「7 个 crate 骨架」 |
| 单元/集成测试 | 每个 crate 内 `crates/<crate>/tests/*.rs`（集成）与 `src/**` 内 `#[cfg(test)] mod tests`（单元） | 本计划新增约定（Q-12） |
| 共享 fixture | `tests/fixtures/{sync,mutations,content,opml}/` | 本计划新增约定（§8），与 crate 测试解耦 |
| UI golden 快照 | `crates/tela-ui/tests/snapshots/`（`insta`） | D3 §3 选型表「测试 | 内置 `#[gpui_kit::test]` + `insta` 快照」 |
| 生成产物（不提交） | `target/`、`tests/artifacts/` | 本计划新增约定 |

**计划冲突检查结论：未发现冲突。** 当前任务计划对本步骤只声明了 `docs/development_plan.md`（已满足路径），未声明任何与 `crates/*` 相悖的代码路径；D3 §3.1 的 `members = ["crates/*"]` 与本契约一致。若后续步骤的 expected_outputs 使用 `src/`、根 `tests/` 等其他布局，属**契约漂移**，必须先回写本文件第 4 章再实现，不得静默采用。

---

## 5. 模块划分

| crate | 职责（单一职责边界） | 依赖 | 不做什么 |
|---|---|---|---|
| `tela-proto` | wire 类型与常量：`PullResponse`、`SyncRows`（15 表）、`Tombstone`、`Mutation`、`PushResponse`、`MIN_CLIENT`/`CLIENT_HEADER`/`MEMBER_HEADER`/`PAGE_ROWS`/`HORIZON_DAYS`/`TITLE_IDS_MAX`/`MAX_MUTATIONS_PER_PUSH` | `serde`, `serde_json`, `thiserror`, `uuid` | 不发网络、不开库、不碰 UI |
| `tela-store` | SQLite schema 与迁移、行应用（`apply_pull`）、查询视图、本地待发队列（pending mutations）、FTS5 标题/摘要索引 | `rusqlite`, `tela-proto`, `im` | 不决定"何时拉取"、不渲染 |
| `tela-sync` | 同步编排：cursor 单线程推进、`reset` 清库重建、`more` 连续拉取、推送批量与幂等（mutation `mid`）、冲突回滚 | `tela-proto`, `tela-store`, `tela-net` | 不直接持有 UI 状态、不写 HTTP 细节 |
| `tela-net` | HTTP 客户端（`reqwest`）、认证（Cookie jar / 未来 Bearer）、头部注入（`x-tela-client`=3、`x-tela-member`）、超时重试、错误映射 | `reqwest`, `tokio`, `tela-proto`, `keyring` | 不解析业务语义、不碰 SQLite |
| `tela-content` | 正文对象：`c/<contentKey>.json` 取回后的块模型、HTML→块（`scraper`）、图片下载与解码调度（`image`） | `scraper`, `image`, `tela-net` | 不做排版/绘制 |
| `tela-ui` | 视图层：三栏列表、阅读视图、翻译对照、主题与设置；`gpui-kit` 组件 | `gpui-kit`, `tela-store`, `tela-content`, `tela-sync`, `tela-net`（仅系统浏览器） | 不直接发 HTTP、不写 SQL |
| `tela-app` | 入口：进程/线程拓扑、`gpui::Application` 启动、命令面板与快捷键注册、`--self-check` / `--version` 冒烟入口 | 全部 | 不含业务规则 |

依赖方向（单向，禁止反向 import）：`tela-app → {tela-ui, tela-sync} → {tela-store, tela-content, tela-net} → tela-proto`。

> 依据：D3 §2.3（crate 划分）、§3（选型表）、§3.1（版本锁定）。上述 crate 名与 D3 一致，未改名。

---

## 6. 跨模块接口契约表（含最小失败测试签名）

规则：
- 符号名唯一写法见 §14 命名表；测试文件路径按 §4 契约。
- **每个接口先写红测试，再写实现**（顺序见 §10）。
- 「首次运行预期失败点」= 实现尚不存在时的报错，用于确认测试真的在测该接口。

### C1 `tela-proto::PullResponse`

| 项 | 内容 |
|---|---|
| 提供方 / 调用方 | `tela-proto` / `tela-sync`、`tela-store`（测试） |
| 符号 | `tela_proto::PullResponse { cursor: i64, more: bool, reset: bool, titles_lang: Option<String>, rows: SyncRows, tombstones: Vec<Tombstone> }`（serde 重命名 `titlesLang`） |
| 参数 / 返回 | `serde_json::from_str::<PullResponse>(&str) -> Result<PullResponse, serde_json::Error>` |
| 失败 | JSON 语法错误 → `serde_json::Error`；必填字段缺失 → 同上，须能在消息中定位字段路径 |
| 退出码 | 不适用（库）；经 `tela-app --self-check` 暴露为 10（见 §11） |
| 最小失败测试 | `crates/tela-proto/tests/pull_response.rs::rejects_missing_cursor`：输入 `{"rows":{},"tombstones":[],"more":false,"reset":true}` → 断言 `Err`，消息含 `missing field` 与 `cursor` |
| 首次预期失败点 | `error[E0432]: unresolved import tela_proto::PullResponse` |

### C2 `tela-proto::SyncRows` 缺表容错

| 项 | 内容 |
|---|---|
| 符号 | `tela_proto::SyncRows { profile, prefs, subscriptions, feeds, sites, articles, titles, states, recommendations, highlights, claims, translations, follows, folders, feedFolders, blogrolls, blogrollBlogs }`（17 张，取自 R2 `rows.ts`），全部 `#[serde(default)]` |
| 返回 | 缺失的表反序列化为空 `Vec`，**不报错** |
| 最小失败测试 | `crates/tela-proto/tests/sync_rows_default.rs::missing_tables_are_empty`：输入 `{"articles":[ <最小 ArticleRow> ]}` → 断言 `rows.subscriptions.is_empty()`、`rows.blogrolls.is_empty()`、`rows.articles.len()==1` |
| 首次预期失败点 | `SyncRows` 无 `Default` 实现 / 字段不存在 |

### C3 `tela-proto::Mutation` 与批量上限

| 项 | 内容 |
|---|---|
| 符号 | `tela_proto::Mutation`（`#[serde(tag = "type")]` 枚举，序列化形状对齐 `R3` 的 `discriminatedUnion('type')`）、`tela_proto::MAX_MUTATIONS_PER_PUSH: usize = 50` |
| 参数 / 返回 | `tela_proto::validate_push(&[Mutation]) -> Result<(), TelaError>` |
| 失败 | 超过 50 条 → `TelaError::TooManyMutations { count, max }` |
| 最小失败测试 | `crates/tela-proto/tests/push_batch.rs::rejects_51_mutations`：构造 51 条 → 断言 `matches!(err, TelaError::TooManyMutations { max: 50, .. })` |
| 首次预期失败点 | `TelaError` / `validate_push` 未定义 |

### C4 `tela-store::Store::open` 与迁移

| 项 | 内容 |
|---|---|
| 提供方 / 调用方 | `tela-store` / `tela-sync`、`tela-ui`、`tela-app` |
| 符号 | `tela_store::Store::open(path: &std::path::Path) -> Result<Store, TelaError>`、`Store::schema_version() -> i64` |
| 返回 / 失败 | 成功返回 `Store`；目录不可写 → `TelaError::Store { stage: StoreStage::Open }`；迁移失败 → `StoreStage::Migrate` 并保留旧库（`rusqlite` `backup` feature 先备份到 `<path>.bak`） |
| 最小失败测试 | `crates/tela-store/tests/migrate.rs::migrates_fresh_db_to_latest_and_is_idempotent`：`tempfile` 空目录 → `open` → 断言 `schema_version() == LATEST`；再次 `open` → 断言版本不变、无错误 |
| 失败路径测试 | 同文件 `::open_on_readonly_dir_reports_store_open`（`chmod 0o500` 的临时目录）→ 断言 `TelaError::Store{stage: Open}`，消息含目录路径（§11 三段式） |
| 首次预期失败点 | `unresolved import tela_store::Store` |

### C5 `tela-store::Store::apply_pull`（同步核心）

| 项 | 内容 |
|---|---|
| 符号 | `Store::apply_pull(&mut self, page: &PullResponse) -> Result<ApplyOutcome, TelaError>`；`ApplyOutcome { inserted: usize, deleted: usize, cursor: i64 }` |
| 语义 | `reset=true` → 清空 15 表后写入；`reset=false` → 仅 upsert 本页行与 `tombstones`；任一行的 `seq <= store.cursor` 视为协议回退 → 严格失败 |
| 失败 | `TelaError::Protocol { stage: ProtocolStage::SeqRegression, object: String }`，`object` 形如 `rows.articles[0].seq` |
| 最小失败测试 | `crates/tela-store/tests/apply_pull.rs::reapplying_older_seq_fails`：先 apply `cursor=17` 的页，再 apply 含 `seq=16` 行的页 → 断言 `Err(TelaError::Protocol{..})`，消息含 `seq` 与 `16` |
| 正向测试 | `::snapshot_reset_then_delta_updates_unread_count`：apply `F1` → 未读=1；apply `F2`（`states` 一行 `readAt` 非空）→ 未读=0，`articles` 行数仍为 1（增量不清库） |
| 首次预期失败点 | `apply_pull` 未定义 |

### C6 `tela-store` 待发队列

| 项 | 内容 |
|---|---|
| 符号 | `Store::enqueue(&mut self, m: Mutation) -> Result<uuid::Uuid, TelaError>`、`Store::pending(&self, limit: usize) -> Result<Vec<PendingMutation>, TelaError>`、`Store::ack(&mut self, mid: &[uuid::Uuid]) -> Result<(), TelaError>` |
| 语义 | `mid` 为 UUID v7，可排序；`ack` 幂等（删已删的不报错） |
| 最小失败测试 | `crates/tela-store/tests/pending.rs::ack_is_idempotent`：enqueue 1 条 → ack 两次 → 第二次断言 `Ok(())` 且 `pending(10).len() == 0` |
| 首次预期失败点 | 方法未定义 |

### C7 `tela-sync::SyncEngine`

| 项 | 内容 |
|---|---|
| 提供方 / 调用方 | `tela-sync` / `tela-app`、`tela-ui` |
| 符号 | `tela_sync::SyncEngine::new(store: Store, client: std::sync::Arc<dyn tela_net::SyncTransport>, clock: Box<dyn Fn() -> i64 + Send>)`、`SyncEngine::pull_once() -> Result<PullOutcome, TelaError>`、`SyncEngine::push_once() -> Result<PushOutcome, TelaError>` |
| 语义 | `pull_once`：循环直到 `more=false`（每次用返回的 `cursor` 续拉），并把每次 `apply_pull` 的结果累加；`push_once`：取 ≤50 条 pending 推送，成功后 `ack` |
| 失败 | 409 `upgrade` → `TelaError::UpgradeRequired { min_client: i64 }`；409 `account_changed` → `TelaError::AccountChanged` |
| 最小失败测试 | `crates/tela-sync/tests/pull_more.rs::follows_more_until_false`：用 `FakeTransport`（实现 `SyncTransport`）依次返回 `more=true`(cursor=17) → `more=false`(cursor=18) → 断言调用 2 次、`store.cursor()==18`、`PullOutcome.pages == 2` |
| 契约测试 | `crates/tela-sync/tests/upgrade.rs::maps_409_upgrade`：transport 返回 409 体 `{"error":"upgrade"}` → 断言 `TelaError::UpgradeRequired{min_client: 3}` |
| 首次预期失败点 | `SyncTransport` trait 与 `SyncEngine` 未定义 |
| 退出码 | 经 `--self-check` 暴露为 20 |

### C8 `tela-net::TelaClient` 与头部契约

| 项 | 内容 |
|---|---|
| 符号 | `tela_net::TelaClient::new(base: url::Url, auth: Box<dyn tela_net::AuthProvider>) -> Result<TelaClient, TelaError>`、`TelaClient::get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, TelaError>`、`TelaClient::post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, TelaError>` |
| 语义 | 每个 member 调用必须带 `x-tela-client: 3`、`x-tela-member: <member id>`；`/api/v1/me` **不带** `x-tela-member`（见 `R1` `MEMBER_HEADER` 注释） |
| 最小失败测试 | `crates/tela-net/tests/headers.rs::member_calls_send_both_headers`：用 `wiremock` 起本地服务，断言 `/api/v1/sync` 请求头含两值，`/api/v1/me` 请求头**不含** `x-tela-member` |
| 失败路径 | `::missing_member_header_on_sync_maps_409`：服务端回 409 `account_changed` → 断言 `TelaError::AccountChanged` |
| 首次预期失败点 | `tela_net::TelaClient` 未定义 / `wiremock` 未加入 dev-dependencies |

### C9 `tela-net::AuthProvider`

| 项 | 内容 |
|---|---|
| 符号 | `tela_net::AuthProvider`（trait）、`tela_net::CookieJarAuth::from_path(&Path) -> Result<CookieJarAuth, TelaError>`、`tela_net::StoredTokenAuth::load(service: &str, account: &str) -> Result<StoredTokenAuth, TelaError>`（`keyring`） |
| 返回 | `AuthProvider::headers(&self) -> Result<Vec<(String, String)>, TelaError>` |
| 最小失败测试 | `crates/tela-net/tests/auth.rs::cookie_jar_missing_file_reports_path`：`from_path("/nope/jar.txt")` → 断言 `Err(TelaError::Auth{..})`，消息含该路径（三段式第 1 段） |
| 首次预期失败点 | `CookieJarAuth` 未定义 |
| 备注 | Cookie 兼容仅为 D2 §12「阶段 0」的过渡方案；Bearer/设备流的最终形态是 Q-1 待确认项，本计划**不**假定其已可用 |

### C10 `tela-content::parse_blocks`

| 项 | 内容 |
|---|---|
| 提供方 / 调用方 | `tela-content` / `tela-ui` |
| 符号 | `tela_content::parse_blocks(html: &str) -> Result<Vec<tela_content::Block>, TelaError>`；`Block` 枚举：`Heading{level,text}`、`Paragraph{inline}`、`Image{src,alt,width,height}`、`Code{lang,text}`、`Quote{blocks}`、`Table{head,rows}`、`List{ordered,items}` |
| 容错 | 未识别标签 → 降级为 `Paragraph` 收其文本；空 HTML → `Ok(vec![])`（不失败）；`src` 为空 → 保留块但标记 `src: None`（见 §8.3 R9） |
| 最小失败测试 | `crates/tela-content/tests/parse_blocks.rs::unknown_tag_degrades_to_paragraph`：输入 `<div><p>Hello</p><figure>cap</figure></div>` → 断言不报错且 `blocks.iter().any(|b| matches!(b, Block::Paragraph{..}))` |
| 正向测试 | `::parses_basic_fixture_into_five_blocks`：输入 `tests/fixtures/content/article-basic.html` → 断言 5 块、第 3 块 `Image.src` 保留原 URL、第 4 块 `Code.lang == Some("rust")`、纯文本含 `Hello`（断言性质见 §8.2 F4） |
| 首次预期失败点 | `unresolved import tela_content::parse_blocks` |

### C11 `tela-ui` 视图入口

| 项 | 内容 |
|---|---|
| 符号 | `tela_ui::reader::ReaderView::new(cx: &mut gpui::App, store: std::sync::Arc<std::sync::Mutex<tela_store::Store>>) -> gpui::Entity<ReaderView>`、`tela_ui::reader::ReaderView::select(&mut self, id: ArticleId, cx: &mut gpui::Context<Self>)` |
| 最小失败测试 | `crates/tela-ui/tests/reader_view.rs::selects_first_unread_on_mount`：打开含 1 条未读的 store → 断言 `selected_article_id == Some(42)` |
| 失败路径 | `::renders_empty_state_when_store_is_empty`：空库 → 断言渲染出空状态文案（D4 §5.2「空状态」属 P1，此处**仅**要求不 panic 的空态占位，文案待定 → Q-14） |
| 首次预期失败点 | `tela_ui::reader` 模块不存在；若 UI 测试无法在无显示器环境跑，见 A4 与 Q-13 |

> **实现偏差（2026-10-27，S2/S3）**：GUI 入口落在 `tela_ui::gui::{ShellView, ReaderView}`，
> 签名是 `ReaderView::new(data: Arc<dyn ShellData>, content: Arc<dyn ContentApi>, cx)`、
> `select(article_id: i64, title: String, cx)`。原因：UI 不应直接持有 `Store`，
> 也不应把 SQLite 锁带进渲染路径；数据访问收进 `tela-app::services::AppServices`，
> 由 `ShellData` 只读 trait 暴露。C11 的“不直接写 SQL”约束保持不变；
> 等价行为测试见 `crates/tela-ui/tests/{shell_view_model,gui_shell,gui_reader}.rs`。

### C12 `tela-app` 入口与 `--self-check`

| 项 | 内容 |
|---|---|
| 符号 | 二进制 `tela-app`：`cargo run -p tela-app -- --version`、`--self-check [--db <path>] [--fixture <path>]` |
| 行为 | `--version` 打印 `tela-app <semver>` 后退出码 0；`--self-check` 依次执行：fixture 解析 → store 迁移 → `apply_pull` → 报告结果，退出码见 §11 |
| 最小失败测试 | `crates/tela-app/tests/self_check.rs::fails_with_code_10_on_broken_fixture`：传入 `tests/fixtures/sync/invalid-missing-seq.json` → 断言进程退出码 **10**，stderr 含 `rows.articles[0].seq` |
| 契约测试 | `::version_prints_and_exits_zero` → 断言退出码 0 且 stdout 含 `tela-app` |
| 首次预期失败点 | 二进制不存在（`cargo run` 退出码 101） |

### C13 错误类型与三段式消息（跨全部 crate）

| 项 | 内容 |
|---|---|
| 符号 | `tela_proto::TelaError`（唯一错误类型，`thiserror`），变体：`Net{source}`、`Auth{detail}`、`Store{stage}`、`Protocol{stage,object}`、`Fixture{path,object,detail}`、`Content{article,content_key,detail}`、`UpgradeRequired{min_client}`、`AccountChanged`、`TooManyMutations{count,max}`、`Render{article,detail}` |
| 消息格式（强制） | `<来源> → <对象> → <原因与修复方向>`，三段用 ` → ` 分隔 |
| 最小失败测试 | `crates/tela-proto/tests/error_format.rs::every_variant_has_three_segments`：对每个变体构造样本 → 断言 `Display` 字符串按 `" → "` split 得到 **3** 段且每段非空 |
| 首次预期失败点 | `TelaError` 未定义 / `Display` 段数不等于 3 |

> 命名唯一性自检：错误类型只有 `TelaError` 一个写法（不是 `AppError`/`Error`）；`Store` 只在 `tela-store` 导出（不是 `Db`）；行表名用小写复数，与 `R2` 完全一致（`articles`、`states`、`subscriptions`…）。

---

## 7. 错误信息契约与退出码对照

### 7.1 三段式示例（实现必须逐字满足 split 后 3 段）

1. `tests/fixtures/sync/invalid-missing-seq.json → rows.articles[0].seq → 缺失或非数字；请按 .reference/tela/packages/sync/src/rows.ts 的 ArticleRow 补齐 seq`
2. `https://telaread.com/api/v1/sync?cursor=0 → documents → HTTP 409 upgrade：客户端 x-tela-client 低于 MIN_CLIENT(=3)；请提升协议版本或升级客户端`
3. `article 42 → c/abc123.json → 对象取回 404；确认该文章的 contentKey 是否已随 pull 下发，或服务端对象存储是否缺失`
4. `~/.local/share/tela/store.db → migrate v3 → 目标库为只读；检查目录权限，或删除 .bak 后重试`（迁移前已备份 `<path>.bak`）

### 7.2 退出码表（**本计划新增约定，待确认 Q-12**）

| 退出码 | 触发条件 | 错误信息形态 | 修复指引 |
|---|---|---|---|
| 0 | 命令成功 | — | — |
| 2 | CLI 参数错误（未知子命令/缺值） | clap 默认 usage 文本 | 看 usage；不要当作数据问题排查 |
| 10 | fixture 不合法：JSON 语法/必填字段缺失/`seq` 类型错 | 三段式，第 2 段为字段路径 | 先修 fixture（§8），**不要**改解析器 |
| 11 | store 打开或迁移失败 | 三段式，第 1 段为 db 路径 | 查权限/磁盘；用 `<path>.bak` 回滚 |
| 12 | pull 应用失败（`seq` 回退、未知表导致的不一致） | 三段式，第 2 段为 `rows.<表>[i].seq` | 先确认是否重放了旧页（cursor 状态），再查协议 |
| 13 | 正文对象渲染失败（contentKey 取回或解析） | 三段式，第 1 段 `article <id>`，第 2 段 `c/<key>.json` | 查对象是否存在 → 再查 `parse_blocks` |
| 20 | 网络/认证失败（401/403/409 `upgrade`/`account_changed`） | 三段式，第 2 段为 `documents`/`me` | `upgrade` → 提高 `x-tela-client`；`account_changed` → 重新登录/校验 `x-tela-member` |
| 101 | `cargo test`/`cargo run` 的 panic 或编译失败（cargo 默认） | Rust panic/编译诊断 | 属实现缺陷，按 traceback 首因修 |

**双向对齐要求**：§6 的 C12 测试必须断言 10；C7 的错误映射测试必须与 20 对应；`--self-check` 的任何新分支都必须在此表登记后再实现。

---

## 8. Golden fixture 规范（最小合法样本 + 可断言性质）

> 本项目的"格式"是 **Tela 同步 wire 形状**（不是 OFD/PDF）。但同样需要把 fixture 结构与断言写死，否则解析器与 fixture 会各按一套标准验收。

### 8.1 最小合法 pull 快照 `tests/fixtures/sync/snapshot-minimal.json`

**顶层必填**：`cursor`(number ≥ 0)、`more`(bool)、`reset`(bool)、`rows`(object)、`tombstones`(array)。`titlesLang` 可选。

**`rows` 必含 5 张表**（其余 12 张可缺省，缺省即空——对应容错 R1）：`subscriptions`、`feeds`、`sites`、`articles`、`states`。
**表键名全集（17，取自 `R2` 上游 `rows.ts`）**：`profile`、`prefs`、`subscriptions`、`feeds`、`sites`、`articles`、`titles`、`states`、`recommendations`、`highlights`、`claims`、`translations`、`follows`、`folders`、`feedFolders`、`blogrolls`、`blogrollBlogs`。

> **M1 口径修正（2026-10-26）**：本节初稿把全集记作 15 张，漏了 `feedFolders` 与 `blogrollBlogs`；`R2` 上游 `.reference/tela/packages/sync/src/rows.ts` 实为 17 张。M1 实现以 `R2` 为唯一真相，按 17 张落地（`crates/tela-proto/src/rows.rs::SyncRows::TABLE_NAMES`），本节数字与清单同步修正为 17，避免实现与文档再次漂移。

行字段以 `R2` 为唯一真相（本文件不复制字段全表，避免与上游漂移）。fixture 中用到的最低必填集，已按 `R2` 核对：

- `ArticleRow`：`id, feedId, url, title, author, publishedAt, fetchedAt, sortAt, sourceLang, excerpt, contentKey, wordCount, readingMinutes, extractState, likeCount, recommendCount, seq`
- `FeedRow`：`id, siteId, feedUrl, title, status, lastFetchedAt, lastError, seq`（`lastItemAt` 可选）
- `SiteRow`：`id, homeUrl, title, description, faviconKey, primaryLang, listing, owned, claimed, readerCount, translationOptOut, seq`
- `SubscriptionRow`：`feedId, watermarkId, createdAt, deletedAt, seq`
- `StateRow`：`articleId, readAt, likedAt, likedUpdatedAt, seq`（`readUpdatedAt` 可选）
- `Tombstone`：`{entity, key, seq}`；`PrefRow`：`{key, value, updatedAt, seq}`

最小样本（示意，`seq` 单调、时间用 UTC 秒）：

```json
{
  "cursor": 17,
  "more": false,
  "reset": true,
  "rows": {
    "sites": [{ "id": 7, "homeUrl": "https://example.com", "title": "Example", "description": null,
                "faviconKey": null, "primaryLang": "en", "listing": "public", "owned": false,
                "claimed": false, "readerCount": 0, "translationOptOut": false, "seq": 11 }],
    "feeds": [{ "id": 9, "siteId": 7, "feedUrl": "https://example.com/feed.xml", "title": "Example",
                "status": "active", "lastFetchedAt": 1761350400, "lastError": null, "seq": 12 }],
    "subscriptions": [{ "feedId": 9, "watermarkId": 0, "createdAt": 1761350400, "deletedAt": null, "seq": 13 }],
    "articles": [{ "id": 42, "feedId": 9, "url": "https://example.com/a", "title": "Hello", "author": "Ann",
                   "publishedAt": 1761350400, "fetchedAt": 1761350500, "sortAt": 1761350400, "sourceLang": "en",
                   "excerpt": "Hello world", "contentKey": "c/abc123.json", "wordCount": 12,
                   "readingMinutes": 1, "extractState": "ok", "likeCount": 0, "recommendCount": 0, "seq": 14 }],
    "states": []
  },
  "tombstones": []
}
```

**可断言性质（F1）**：

| 断言 | 期望 | 测试 |
|---|---|---|
| 反序列化 | 成功；`cursor==17`、`reset==true`、`more==false`、`titles_lang==None` | `crates/tela-proto/tests/pull_response.rs::parses_snapshot_minimal` |
| 表数量 | `rows.articles.len()==1`，其余 14 表均为空（未出现的表按空处理） | `crates/tela-proto/tests/sync_rows_default.rs::snapshot_missing_tables_are_empty` |
| 应用后 | `store.articles()`=1 行；未读数=1（`readAt` 为 null 且 `watermarkId=0` 且 `sortAt` 在 `HORIZON_DAYS=30` 内） | `crates/tela-store/tests/apply_pull.rs::snapshot_reset_then_delta_updates_unread_count` |
| 增量页 `delta-state-read.json`（`reset:false, cursor:18, rows.states=[{articleId:42, readAt:1761350600, likedAt:null, likedUpdatedAt:null, seq:18}]`） | apply 后**不清库**，`articles` 仍 1 行，未读数 0，`store.cursor()==18` | 同上 |

### 8.2 其余 fixture 与断言

| 编号 | 文件 | 最小内容 | 可断言性质 |
|---|---|---|---|
| F2 | `tests/fixtures/sync/pull-delta-more.json` | `more=true, cursor=17`，`rows.states` 1 行 | `pull_once` 调用 transport 2 次、`pages==2`、最终 `cursor==18`（C7） |
| F3 | `tests/fixtures/sync/invalid-missing-seq.json` | `articles[0]` 删掉 `seq` | `tela-app --self-check` 退出码 **10**，stderr 三段式且第 2 段含 `rows.articles[0].seq` |
| F4 | `tests/fixtures/content/article-basic.html` | `<h1>Hello</h1><p>World <a href="https://example.com">link</a></p><img src="https://example.com/i.png" alt="pic"><pre><code class="language-rust">fn main(){}</code></pre><blockquote><p>Q</p></blockquote>` | `parse_blocks` 返回 5 块；块类型序列 `[Heading(1), Paragraph, Image, Code, Quote]`；`Image.src=="https://example.com/i.png"`；`Code.lang==Some("rust")`；纯文本含 `Hello` |
| F5 | `tests/fixtures/content/article-unknown-tag.html` | 含 `<figure>` 等未知标签 | 不报错；未知标签降级 `Paragraph`，文本不丢 |
| F6 | `tests/fixtures/opml/subscriptions.opml` | 2 个 `<outline xmlUrl=... text=...>` | `import_opml` 返回 2 条订阅，`feedUrl` 与 `text` 逐字一致；重复导入不产生重复行 |
| F7 | `tests/fixtures/mutations/push-51.json` | 51 条 mutation | `validate_push` 返回 `TooManyMutations{count:51,max:50}`，消息三段式 |

> fixture 必须**手写提交**（不靠运行期生成），否则"解析器按一套、fixture 按另一套"的错位无法被发现。

### 8.3 关键字段容错策略（严格失败 vs 回退默认值）

| # | 场景 | 策略 | 回退值 / 失败形态 |
|---|---|---|---|
| R1 | 服务端未发送某表 key（旧 tela-api 回滚） | **回退** | 空 `Vec`，`tracing::warn!(table)` |
| R2 | `titlesLang` 缺失或为未知语言 | **回退** | 视为"全语言"，不报错（对齐 `R1` 注释：拒绝未知语言会中断整次同步） |
| R3 | `ArticleRow.contentKey == null` | **回退** | 列表正常显示，正文区降级为"内容未生成"占位，不阻塞整页应用 |
| R4 | 可选字段缺失（`lastItemAt`、`readUpdatedAt`、`titlesLang`） | **回退** | `None` |
| R5 | 行 `seq <= store.cursor`（重放/回退） | **严格失败** | `TelaError::Protocol{stage: SeqRegression}`，退出码 12 |
| R6 | `rows` 中出现未知表名 | **回退** | 忽略 + `warn`，不失败 |
| R7 | 未知 `status`/`extractState`/`listing` 字符串 | **回退** | 原样保存，UI 用默认样式 |
| R8 | 时间戳为 0 或负数 | **回退** | 展示为"未知时间"，不得 panic |
| R9 | `Image.src` 为空或非 http(s) | **回退** | 保留块、`src: None`，渲染占位图 |
| R10 | 单表行数 > `PAGE_ROWS(1000)` | **接受但告警** | 全量应用 + `warn`（服务端契约应为 1000） |
| R11 | 内容对象 HTML 为空字符串 | **回退** | `Ok(vec![])`，正文区空态 |
| R12 | 409 `upgrade` / `account_changed` | **严格失败（可恢复）** | `UpgradeRequired{min_client}` / `AccountChanged`，退出码 20 |

---

## 9. 分阶段里程碑

节奏对齐 D4 §13（v0.1 第 1–2 周 … v1.0 第 15 周）与 D3 §18（M0–M4）。每阶段**独立可验证、可回滚**（打 tag `m0`…`m5`）。

### M0 — 骨架 + 测试基建（第 1 周；对应 v0.1 前半）

- **目标**：workspace 与 7 个 crate 骨架可编译；测试基建可跑；`tela-app --version` 可用。
- **交付物**：根 `Cargo.toml`（§4 契约）、7 个 crate 的 `Cargo.toml` + 最小 `lib.rs`/`main.rs`、`crates/tela-proto/tests/smoke.rs`（1 条通过的冒烟测试）、`tests/fixtures/` 目录与 F1/F3 两个 fixture 文件、`README.md` 补「构建与测试」段。
- **依赖顺序**：无前置（A1–A7 为本阶段门禁）。
- **验收方式**（全部通过才进 M1）：
  1. `cargo build --workspace` → 退出码 0
  2. `cargo test --workspace --no-run` → 退出码 0（测试可编译）
  3. `cargo test -p tela-proto` → 退出码 0，且输出含 `1 passed`（冒烟测试真实运行）
  4. `cargo run -p tela-app -- --version` → 退出码 0，stdout 含 `tela-app`
  5. `test -s tests/fixtures/sync/snapshot-minimal.json && test -s tests/fixtures/sync/invalid-missing-seq.json`
  6. 契约测试 `cargo test -p tela-proto --test exports_contract`（断言 §6 列出的公开符号可导入）
- **回滚**：删 `crates/` 与根 `Cargo.toml`；无数据、无发布影响。

### M1 — 协议 + 存储 + 拉取（第 1–2 周；v0.1）

- **目标**：C1–C5、C8、C9、C12 全部转绿；用 fixture 完成"pull → apply → 未读数正确"闭环。
- **交付物**：`tela-proto` 全量 wire 类型与常量、`tela-store` schema+迁移+`apply_pull`、`tela-net` 头部契约、`tela-app --self-check`、§8.1 全部断言测试、F2/F7 测试。
- **依赖顺序**：M0 通过 → `tela-proto` → `tela-store` → `tela-net` → `tela-app`（单向）。
- **验收方式**：
  1. `cargo test -p tela-proto -p tela-store -p tela-net` → 退出码 0
  2. `cargo run -p tela-app -- --self-check --fixture tests/fixtures/sync/snapshot-minimal.json --db tests/artifacts/m1.db` → 退出码 **0**，stdout 含 `articles=1`、`unread=1`
  3. `cargo run -p tela-app -- --self-check --fixture tests/fixtures/sync/invalid-missing-seq.json --db tests/artifacts/m1.db` → 退出码 **10**，stderr 三段式
  4. `test -s tests/artifacts/m1.db`（真实生成 SQLite 文件，非空）
  5. 真联调（A5/A6 通过时）：`--self-check --remote https://telaread.com`（若未实现 remote 分支则记为 M2 待办，不得伪造通过）
- **回滚**：`git checkout m0`；db 为本地文件，删除即可。

### M2 — 同步引擎 + 订阅面（第 3–5 周；v0.2）

- **目标**：C6、C7 转绿；`more` 连续拉取、push 批量与幂等、离线队列可用；订阅/列表可见。
- **交付物**：`tela-sync`（`SyncEngine`、`SyncTransport` trait、`FakeTransport`）、`tela-store` pending 队列、三栏视图（`tela-ui` 列表 + 空态）、`tela-app` 进程内同步循环。
- **依赖顺序**：M1 → `tela-store::enqueue/pending/ack` → `tela-sync` → `tela-ui` 列表。
- **验收方式**：
  1. `cargo test -p tela-store -p tela-sync` → 0，含 `follows_more_until_false` 与 `ack_is_idempotent`
  2. `cargo test -p tela-ui` → 0（A4 不成立时改用 `xvfb-run -a cargo test -p tela-ui`）
  3. 快照测试：`cargo insta test -p tela-ui` → 0，`crates/tela-ui/tests/snapshots/*.snap` 存在且非空
  4. 三条 push 幂等验证：同一 mutation 连推两次，服务端/`FakeTransport` 侧只生效一次（断言 store 状态不变）
- **回滚**：`git checkout m1`；pending 队列表可 DROP（无远端副作用，因为 push 未上线）。

### M3 — 正文 + 翻译对照（第 6–8 周；v0.2 后段→v0.3）

- **目标**：C10、C11 转绿；块级渲染 + 三模式翻译对照（side/trans/orig）可用。
- **交付物**：`tela-content`（`parse_blocks`、`Block`、图片下载调度）、`tela-ui` 阅读视图与翻译面板、F4/F5 fixture 与断言、`insta` 快照更新。
- **依赖顺序**：M2 → `tela-content` → `tela-ui::reader`。
- **验收方式（含实测结果，2026-10-27 复核）**：
  1. `cargo test -p tela-content -p tela-ui` → **0**（实测：全量 `cargo test --workspace --locked` 退出码 0，20 个测试目标全 `test result: ok`；其中 `tela-content` 11 项含 `content_api.rs`、`http_content_api.rs`，`tela-ui` 含 `reader_document` 3 项 / `reader_page` 6 项 / `reader_session` 5 项）
  2. `parse_blocks` 对 F4 的结构断言全部通过 → **是**（`crates/tela-content/tests/content_api.rs` 断言 5 块、类型序列、`Code.lang`、`Image.src` 等；`tests/fixtures/content/content-normal.json` + `translation-normal.zh-Hans.json` 为唯一数据源）
  3. `--self-check --render-only` → **未实现，记 Q-15 待办**（`crates/tela-app/src/main.rs` 只有 `--chapter/--page-size/--view` 等阅读参数与 M1 的 `--self-check`，无 `--render-only` 分支；本项**不**声称完成，替代证据见下条：`tmp/checks/verify_reader_cli.py` 用真实二进制逐段断言 stdout）
  4. 快照：阅读视图 golden 与翻译三模式各 ≥1 张 → **改为行为断言替代，未引入 insta**（`crates/tela-ui/Cargo.toml` 无 `insta` 依赖，`crates/tela-ui/tests/snapshots/` 不存在；等价证据：`reader_page.rs` 断言三种视图的渲染行/列与缺译文降级文案，`tmp/checks/verify_reader_cli.py` 断言 o/t/c 三视图真实终端输出）
  5. 端到端用户路径（真实二进制，mock 默认后端）：`python3 tmp/checks/verify_reader_cli.py` → `ALL_CHECKS_OK`，退出码 0。覆盖：默认启动（加载态 → 对照页 `共 5 段/第 1/1 页`，逐段原文+译文与 fixture 独立推导值一致）→ `o/c/t` 视图切换（原文仅原文列、译文仅译文列、对照两列且块序不变）→ 空章节（`--chapter mock-empty`，显式空态提示、无残留段落）→ `mock-partial` 译文空洞显式降级 → `mock-long` 120 段分 6 页（页码 1/6…6/6、每页 20 行、1..120 不重不漏，末页切视图后页码与行不变）→ 失败页按 `r` 重试重绘
  6. 翻译对照三模式：`--view original|compare|translation` 与交互键 `o/c/t`，均复用 `crates/tela-ui/src/reader/page.rs`（终端与 GUI 共用渲染逻辑）。「仅原文模式」（D4 §16 风险应对）**以 `--view original` 实现**，未提供 `--no-translate` feature flag

- **实际实现路径（mock 阶段已落地）**：
  - 取数抽象：`crates/tela-content/src/api.rs` 的 `ContentApi::load_chapter(&ChapterRequest) -> Result<Chapter, TelaError>`，装配入口 `content_api_from_env()` / `content_api_from_values()`；调用方只持有 `Arc<dyn ContentApi>`，切换后端不改调用方
  - 模型/对齐：`crates/tela-content/src/model.rs`（`Chapter`、`Block`/`BlockKind`、`TranslationView`、缺译文空洞）
  - mock 后端：`crates/tela-content/src/mock.rs`（`MOCK_CONTENT_KEY` 及 `mock-long`/`mock-empty`/`mock-partial` 别名、`with_failures` 一次性故障注入），数据来自 `tests/fixtures/content/*.json`
  - http 后端（占位，显式失败）：`crates/tela-content/src/http.rs`
  - 阅读/翻译视图：`crates/tela-ui/src/reader/{document,session,page}.rs`
  - 终端宿主与启动参数：`crates/tela-app/src/main.rs`
  - 测试与验收脚本：`crates/tela-content/tests/{content_api,http_content_api}.rs`、`crates/tela-ui/tests/{reader_document,reader_page,reader_session}.rs`、`crates/tela-app/tests/{reader_cli,reader_cli_http_backend}.rs`、`tmp/checks/verify_reader_cli.py`

- **mock 开关用法**：
  - 默认（不设任何环境变量）：mock 后端。`cargo run -p tela-app`（交互）或 `printf 'n\no\nc\nt\nq\n' | cargo run -q -p tela-app`（脚本/CI）
  - `TELA_CONTENT_API=mock`（或空串）：显式指定 mock；`TELA_CONTENT_API=http` + `TELA_API_BASE_URL=<base>`：切到真实 HTTP 后端；未知取值报错而非静默降级
  - 未就绪行为（本轮实测，`TELA_CONTENT_API=http TELA_API_BASE_URL=http://127.0.0.1:1` 运行真实二进制）：
    - 终端页显示 `加载失败：<完整 URL> → content → 请求失败：服务端内容接口尚未就绪…（base_url=…，译文语言=zh-Hans）；离线开发请设 TELA_CONTENT_API=mock…（按 r 重试）`，**不渲染任何段落**
    - stderr 同步输出可读日志前缀 `tela-app: 内容加载失败（http 后端）：<同一原因链>`
    - 错误类型映射为 `TelaError::Net`（退出码 20），**不会**返回 `Ok(空章节)` 冒充成功
    - 断言落点：`crates/tela-content/tests/http_content_api.rs::http_backend_reports_unready_endpoint_instead_of_empty_chapter`、同文件 `mock_backend_still_serves_chapter_while_http_endpoint_is_unready`（同一时刻 mock 仍返回 5 段正文+译文，证明切换隔离、失败不回落 mock）、`crates/tela-app/tests/reader_cli_http_backend.rs::http_backend_unready_renders_failure_page_and_logs_reason`（同时断言页面修复指引 + stderr 日志）

- **接入真实 API 时的替换点与契约**：
  1. 唯一替换点：`crates/tela-content/src/http.rs` 的 `HttpContentApi::load_chapter` 内部取数实现（把当前"返回显式 `TelaError::Net`"换为真实请求），公开签名与错误类型不变
  2. 按 `docs/api-proposal.md` §7.3/§7.4 请求 `GET <base>/o/c/<key>.json`（正文块）与 `GET <base>/t/<key>/<lang>/<sha>.json`（逐段译文），拼装为 `model.rs` 的 `Chapter`/`TranslationView`
  3. 复用 `crates/tela-net`（`transport.rs` 的 trait、`http.rs` 的真实传输、`mock_http.rs` 的离线夹具）与 `crates/tela-net/tests/http_transport.rs` 的既有断言模式，避免自造 HTTP 层
  4. 保留现有可观察契约：失败必须是显式错误（含目标 URL、base_url、失败原因、修复指引），`TelaError` 映射与退出码不变；`mock` 分支不得被真实实现顺带改写
  5. 真实实现落地后需新增超时/重试/`409 upgrade` 分支的断言，并把本条的"未就绪"测试改为可注入的失败传输用例

- **已知差异 / 未覆盖项（如实记录，未按已完成计）**：
  - 真实 HTTP 取数**未实现**：服务端内容接口未就绪，当前为显式失败占位（已按上节给出替换点）
  - `--self-check --render-only`（Q-15）未实现；M3 的自动断言以 `cargo test` + `tmp/checks/verify_reader_cli.py`（真实二进制 stdout）为准
  - 无 `insta` 快照：计划中的 golden 快照以文本渲染断言替代
  - 图片**下载调度未实现**：`Image` 段落只渲染占位文案（`verify_reader_cli.py` 已断言该文案），未做本地缓存/解码
  - GUI 宿主**已接入**（2026-10-27 补课）：`tela-ui` 的阅读视图同时服务终端与 GPUI 宿主，
    `tela-gui` 已能打开三栏窗口并切换 o/c/t、n/p、r；验收见 [`docs/gui_acceptance.md`](./gui_acceptance.md)
  - 真实服务端分页/流式分块未接入：当前分页为本地按 `--page-size` 切片
- **回滚**：`git checkout m2`；"仅原文模式"用 `--view original`（等价于 D4 §16 的"提供仅原文模式"风险应对，未实现 `--no-translate` flag）。

### M4 — 收藏 + 文件夹 + OPML + 搜索 + 离线（第 6–8 周并行；v0.3）

- **目标**：高亮/笔记（M2 方案：先整段）、文件夹、OPML 导入导出、本地 FTS5 搜索、离线队列完整化。
- **交付物**：`tela-store` 高亮/文件夹/搜索 API（FTS5）、F6 fixture 与断言、UI 对应入口、离线状态指示。
- **依赖顺序**：M2（队列）→ M4 各项互相独立，可按 F6 → 搜索 → 高亮排序。
- **验收方式**：
  1. `cargo test -p tela-store -p tela-ui` → 0
  2. OPML 往返测试：导出后重新导入，订阅集合等价（断言条数与 `feedUrl` 集合）
  3. 搜索测试：插入 3 篇文章，`search("hello")` 命中 `articles[0]`，`search("zzz")` 为空
  4. 离线测试：`FakeTransport` 全部失败 → 操作仍成功入队，恢复后一次 push 发出（断言 pending 归零）
- **回滚**：`git checkout m3`；FTS5 索引可重建（`REINDEX`）。

### M5 — 打包/签名/更新/无障碍/性能（第 9–15 周；v0.4→v1.0）

- **目标**：三平台可分发的包 + 自动更新 + 无障碍 + 性能达标；P1 功能（Discover/Following/博客卷/通知/多窗口/导出）按 D4 §13 分批进入。
- **交付物**：打包脚本与产物、更新通道配置、性能基线记录、`docs/` 下发布说明。
- **依赖顺序**：M1–M4 全部验收通过；依赖 Deploy Expert 给出部署/回滚方案（本计划不代替其结论）。
- **验收方式**（本计划只固定门禁形态，具体命令待 deploy 步骤补全 → Q-16）：
  1. 每平台产物存在且可启动：`test -s <artifact>` 且启动后进程存活 ≥5s
  2. 冷启动到列表可交互 ≤ 目标值（D3 §18 的性能目标，本计划不复述未核实的数字）
  3. 更新检查路径可回滚：旧版本能被新版本替换，失败时保留旧版本
- **回滚**：灰度通道回退到上一签名版本。

### 9.1 依赖顺序总览

```
M0(骨架+测试基建)
  └─> M1(proto/store/net/self-check)
        └─> M2(sync/队列/列表)
              ├─> M3(content/阅读/翻译)
              └─> M4(收藏/文件夹/OPML/搜索/离线)
                    └─> M5(打包/更新/性能)  ← 依赖 Deploy Expert 结论
```

### 9.2 GUI 补课（S0–S4，2026-10-27 完成）

M2/M3 的库层完成后，GUI 宿主、应用装配与三栏/阅读视图当时仍未落地。本轮按 S0→S4 补齐，
不改变上面的里程碑顺序（M4 仍未开始），详细验收见 [`docs/gui_acceptance.md`](./gui_acceptance.md)。

| 切片 | 内容 | 门禁 |
|---|---|---|
| S0 | `gpui-kit` 编译、`tela-gui` 入口、GPUI 窗口/测试冒烟 | `cargo test -p tela-ui --features gui-test --test gui_smoke` |
| S1 | `AppServices`：Store + ContentApi + SyncTransport、空库 seed、列表/正文请求 | `cargo test -p tela-app --test services`（8 passed） |
| S2 | `ShellViewModel` + GPUI 三栏 shell、订阅/文章列表、空态/错误态 | `shell_view_model` 5 + `gui_shell` 2 passed |
| S3 | `ReaderView`：后台取数、三视图、翻页、失败重试、快捷键 | `gui_reader` 4 passed |
| S4 | CI `gui` job、`--check` 数据冒烟、截图、文档回写 | `env -u DISPLAY cargo test -p tela-ui --features gui-test --locked`（26 passed / 1 ignored） |

关键结论：

1. **Q-13 已解**：GUI 测试在无 `DISPLAY`/`WAYLAND_DISPLAY` 下可运行，CI 不需要 Xvfb；
   截图用 `HeadlessAppContext` + WGPU（本地可用，CI 默认跳过）。
2. **`gpui-kit/test-support` 在 0.8.0 上不可用**：它会拉起 gpui-fast 0.1.10，与 gpui-pre 0.3.9
   的 `cocoa`/`unicode-properties` 精确版本冲突。GUI 测试改用 GPUI 自带 `test-support`
   （`#[gpui::test]` / `TestAppContext`），元素级查询由纯视图模型测试 + 截图补足。
3. **默认构建零 GUI 依赖**：`tela-ui/gui`、`tela-app/gui` 两把 feature 锁，`tela-gui` 是
   `required-features = ["gui"]` 的独立二进制；`cargo test --workspace` 仍是 128 passed。
4. **M2 的“进程内同步循环”仍只提供 `AppServices::sync_once()`**，尚未自动调度；
   等真实传输层（不是空脚本 MockTransport）就绪后在 M4 接入。

---

## 10. 测试契约与 TDD 切片顺序（强制）

### 10.1 切片 0：测试基建（必须最先，且必须真跑通一条测试）

1. 建 workspace + 7 crate（只含 `lib.rs`/`main.rs` 与 `Cargo.toml`）。
2. `tela-proto` 先加 `dev-dependencies = { serde_json, insta }`；写 **1 条**冒烟测试 `crates/tela-proto/tests/smoke.rs::constants_are_pinned`：断言 `MIN_CLIENT == 3 && CLIENT_HEADER == "x-tela-client" && MEMBER_HEADER == "x-tela-member" && PAGE_ROWS == 1000`（值取自 `R1`/`R3`）。
3. 命令：`cargo test -p tela-proto`。**先红**（常量未定义，`E0425`）→ 加常量 → **后绿**。
4. 写入顺序规定：**每个切片的测试文件必须先于对应实现文件落盘**（同一 commit 内的 `git diff --stat` 应体现测试先出现）。

### 10.2 切片序列（每片 = 1 个失败测试 + 1 份实现）

| 切片 | 顺序 | 测试文件（先） | 实现文件（后） | 红→绿判据 |
|---|---|---|---|---|
| S0 | 1 | `crates/tela-proto/tests/smoke.rs` | `crates/tela-proto/src/lib.rs`（常量） | `E0425` → 0 |
| S1 | 2 | `crates/tela-proto/tests/pull_response.rs`、`sync_rows_default.rs` | `src/wire.rs` | 反序列化测试由 `unresolved import` → 通过 |
| S2 | 3 | `crates/tela-proto/tests/push_batch.rs`、`error_format.rs` | `src/mutation.rs`、`src/error.rs` | `TooManyMutations` 未定义 → 通过 |
| S3 | 4 | `crates/tela-store/tests/migrate.rs` | `src/lib.rs`、`src/schema.rs` | `unresolved import Store` → 通过 |
| S4 | 5 | `crates/tela-store/tests/apply_pull.rs` | `src/apply.rs` | `apply_pull` 未定义 → 快照/增量断言通过 |
| S5 | 6 | `crates/tela-store/tests/pending.rs` | `src/pending.rs` | 幂等 ack 通过 |
| S6 | 7 | `crates/tela-net/tests/headers.rs`、`auth.rs` | `src/client.rs`、`src/auth.rs` | 头部断言通过（`wiremock` 加入 dev-deps） |
| S7 | 8 | `crates/tela-sync/tests/pull_more.rs`、`upgrade.rs` | `src/engine.rs`、`src/transport.rs` | `more` 循环断言通过 |
| S8 | 9 | `crates/tela-content/tests/parse_blocks.rs` | `src/blocks.rs` | F4/F5 断言通过 |
| S9 | 10 | `crates/tela-ui/tests/reader_view.rs` | `src/reader.rs` | 快照 + 选中态通过 |
| S10 | 11 | `crates/tela-app/tests/self_check.rs` | `src/main.rs` 的 `--self-check` 分支 | 退出码 0/10 双向断言通过 |
| S11 | 12 | `crates/tela-store/tests/opml.rs`、`search.rs` | `src/opml.rs`、`src/search.rs` | F6 往返与 FTS5 命中通过 |

### 10.3 契约一致性检查（必须进每个 coding 步骤的 checks）

- `cargo test -p tela-proto --test exports_contract`：断言业务 crate 引用的每个公开符号确实由提供方导出（`use tela_proto::{PullResponse, SyncRows, Tombstone, Mutation, TelaError, MIN_CLIENT, CLIENT_HEADER, MEMBER_HEADER, PAGE_ROWS, HORIZON_DAYS, TITLE_IDS_MAX, MAX_MUTATIONS_PER_PUSH};` 能编译）。
- `cargo build --workspace --all-targets`：退出码 0（防止"文件齐全但模块无法加载"）。
- 禁止通过删断言、`#[ignore]`、降低阈值来"变绿"。

---

## 11. 验收命令与失败排查顺序（统一口径）

### 11.1 标准冒烟三连（每个 coding 步骤都跑）

```bash
cargo build --workspace --all-targets          # 期望 0
cargo test --workspace                         # 期望 0
cargo run -p tela-app -- --version             # 期望 0，stdout 含 tela-app
```

### 11.2 数据链路冒烟

```bash
cargo run -p tela-app -- --self-check \
  --fixture tests/fixtures/sync/snapshot-minimal.json \
  --db tests/artifacts/smoke.db                # 期望 0，stdout 含 articles=1 unread=1
test -s tests/artifacts/smoke.db               # 期望 0（非空文件）
```

### 11.3 失败排查顺序（不得跳步）

1. **先验 fixture**：`python3 -m json.tool tests/fixtures/sync/*.json >/dev/null`（退出码 0 才算合法）→ 再看 F1–F7 的必填字段是否齐全。
2. **再查解析层**：`cargo test -p tela-proto`（只跑协议，排除网络与 UI）。
3. **再查存储层**：`cargo test -p tela-store`（排除网络）。
4. **再查网络层**：用 `wiremock`/`FakeTransport` 复现，不直连线上。
5. **最后查渲染**：`cargo test -p tela-content` → `cargo test -p tela-ui`。

> 任何一步失败时，先读 stderr 的**第一段**（三段式的"来源"），确认失败面后再改代码。

---

## 12. 待确认项（冲突 / 过期 / 缺口；均未获确认）

| 编号 | 类型 | 内容 | 影响 | 建议处置 |
|---|---|---|---|---|
| Q-1 | 文档冲突（节奏） | D4 §5.1 把"设备授权登录"列为 P0 主方案，"Cookie 兼容"作为过渡；D2 §12「阶段 0」明确 Cookie + 伪造 `Origin` + `x-tela-client: 3` 只是**内部验证**；D1「下一步」第 2 条要求先用 Cookie 跑通 M0。三者对"首版登录方式"的表述不一致 | 决定 M1 认证实现与是否阻塞 | 按 D1：M0/M1 用 Cookie 兼容（仅内部），设备流/Bearer 作为 M2 起的迁移目标；**最终以 Q-2 的作者答复为准** |
| Q-2 | 信息缺口（阻塞外部依赖） | 作者是否开放 API/授权？D1「免责声明」称与官方无隶属关系；D4 §15 问题 1–10、D2 §13 未答复 | 决定项目能否合法分发 | 对外发布前必须拿到书面答复；期间只做内部原型 |
| Q-3 | 文档缺失 | `PullResponse` 含可选 `titlesLang`（`R1`），且 `TITLE_IDS_MAX = 1000` + `POST /api/v1/sync/titles`（`R1` 注释）；D2/D3 均未描述标题语言过滤与标题接口 | 影响多语言标题与同步实现 | 建议纳入 M1 wire 类型（已写入 §6 C1/C2 的容错 R2），但**是否实现 titles 接口**待确认 |
| Q-4 | 待确认（产品） | 高亮首版"整段"还是"选区"（D4 §15 内部问题 13） | 影响 M4 数据模型与 UI | 按 D4 建议：M1/M4 整段，M2 后做选区；需产品侧确认 |
| Q-5 | 未决技术方案 | 翻译对照"M2 用轻量 WebView 还是原生重投影"（D3 §18 M2） | 影响 `tela-ui` 依赖与打包体积 | 属候选方案，**未验证**；建议 M3 前做一次最小验证（WebView 在目标平台可用性），判定标准：能在 macOS/Windows 显示同一 HTML 段落且滚动同步 |
| Q-6 | 文档重复 | D4 §15（10 条"与作者"）与 D2 §13（讨论清单）内容重叠 | 向作者重复提问、答复口径不一 | 合并去重后形成单一问题清单，再交作者；本计划不代为取舍 |
| Q-7 | 未验证 | `.reference/` 是否真被 `.gitignore` 忽略（D1 声称，未读文件确认） | 影响仓库体积与许可合规 | `cat .gitignore \| grep -c '.reference'` → 期望 ≥1；否则补写 |
| Q-8 | 未验证 | `gpui-kit` 是否把 `gpui-pre` 精确钉在 `=0.3.9`（D3 §3.1 注释；本次读取的 `crates/kit/Cargo.toml` 被截断未见该行） | 影响依赖可升级性（D3 明确警告勿 `cargo update`） | `grep -n 'gpui' .reference/gpui-kit/Cargo.toml \| head` → 期望见 `= 0.3.x`；结果回填本表 |
| Q-9 | 已解（2026-10-27） | gpui-kit 的 UI 测试 harness 已读；`gpui_kit/test-support` 因依赖解析缺陷不可用（见 §9.2） | 影响 S9 切片写法 | 改用 GPUI 自带 `#[gpui::test]` + `TestAppContext`；元素查询用纯视图模型测试补足 |
| Q-10 | 时效未核 | `gpui-kit 0.8.0` 是否为 2026-10-26 的最新版本；D3 选型基于 2026-10-25 的本地副本 | 若已发新版，`=0.3.9` 钉法与组件 API 可能变化 | 由 Research Expert 核对 crates.io 版本与 changelog 后再决定是否升级；**本计划不假定其最新** |
| Q-11 | 契约风险 | D3 §3.1 用 `edition = "2024"` / `rust-version = "1.92"`；若目标机工具链不足，降级到 2021 会改变大量语法契约 | 影响 M0 门禁 A1/A2 | 先按 D3 执行；确需降级时必须先修订 D3 §3.1 与本计划 §4，不得静默改 |
| Q-12 | 本计划新增约定（待确认） | §4 测试目录布局、§7.2 退出码表、`tela-app --self-check` / `--version` 均为本计划提出，D1–D4 未定义 | 后续 coding 的验收将依赖它们 | 需架构负责人确认后视为契约；未确认前 coding 步骤只按本文件执行，不得各自发明新退出码 |
| Q-13 | 已解（2026-10-27） | UI 测试的无显示器运行能力 | 影响 M2/M3 的自动化门禁 | 实测 `env -u DISPLAY -u WAYLAND_DISPLAY cargo test -p tela-ui --features gui-test` → 26 passed / 1 ignored；截图另用 `HeadlessAppContext`，见 §9.2 与 `docs/gui_acceptance.md` |
| Q-14 | 信息缺口 | 空状态文案、图标/字体的品牌素材授权（D4 §15 问题 1、D4 §16"设计不像 Tela"） | 影响 UI 文案与资产落库 | 文案先留占位常量并集中在一处，待作者许可后替换 |
| Q-15 | 信息缺口 | 本计划提出的 `--self-check --render-only` 分支 D1–D4 未定义 | 影响 M3 的可自动断言性 | 与 Q-12 一并确认；未确认前 M3 以 `cargo test -p tela-content` 为准 |
| Q-16 | 信息缺口 | 打包/签名/自动更新的具体命令与回滚策略（M5） | 影响 M5 验收 | 由 Deploy Expert 给出；本计划只固定门禁形态 |
| Q-17 | 缺口（许可） | 根目录未见 `LICENSE` 文件，而 D3 §3.1 声明 `license = "MIT OR Apache-2.0"`；D1「免责声明」要求取得作者许可 | 发布合规 | 先补 `LICENSE` 或明确"暂不发布"；命名/图标许可见 Q-2/Q-14 |
| Q-18 | 时效未核 | D2 的服务端能力描述基于 2026-10-25 线上实测与仓库只读副本；`MIN_CLIENT` 已到 3，若继续上升，客户端兼容策略需重定 | 影响 409 `upgrade` 处理与发布节奏 | 每次发布前核对 `R1` 的 `MIN_CLIENT`；把它作为发布检查项 |

---

## 13. 计划冲突 / 建议调整

1. **未发现路径冲突**：本步骤唯一产物为 `docs/development_plan.md`，已按唯一路径交付；代码路径 `crates/*` 与 D3 §3.1 的 `members = ["crates/*"]` 一致。若后续步骤声明 `src/` 或其它布局，属契约漂移，必须回写本文件第 4 章。
2. **建议调整（未生效，仅建议）**：D3 §18 把 M3 与 M4 都排在"第 6–8 周"，与 D4 §13 的 v0.3（第 6–8 周）内容量不匹配（v0.3 只列了高亮/笔记、文件夹、OPML、搜索、离线，**没列**三模式翻译对照的完成时点）。建议把"翻译三模式 + 流式分块"明确归入 v0.2 收尾或 v0.3，避免 M3/M4 同期超载。此建议需产品侧确认后才改 D4 §13。
3. **建议调整（未生效，仅建议）**：D2 §13 与 D4 §15 的问题清单建议合并为 `docs/open-questions.md`（新文件属**建议路径**，尚未创建，不得在后续步骤中当作已存在）。

---

## 14. 命名规范（唯一写法，禁止同义变体）

| 概念 | 唯一写法 | 禁止 |
|---|---|---|
| 错误类型 | `TelaError` | `AppError`、`Error`、`TelaErr` |
| 数据库句柄 | `Store` | `Db`、`Database`、`Repo` |
| 协议头常量 | `CLIENT_HEADER`、`MEMBER_HEADER` | `X_TELA_CLIENT`、`client_header` |
| 版本常量 | `MIN_CLIENT`、`PAGE_ROWS`、`HORIZON_DAYS`、`TITLE_IDS_MAX`、`MAX_MUTATIONS_PER_PUSH` | 小写或改词序 |
| 表名 | `articles`、`states`、`subscriptions`、`feeds`、`sites`、`titles`、`prefs`、`profile`、`recommendations`、`highlights`、`claims`、`translations`、`follows`、`folders`、`blogrolls`（与 `R2` 逐字一致） | 单数形式、驼峰 |
| 拉取入口 | `SyncEngine::pull_once` | `sync_once`、`fetch`、`pullAll` |
| 推送入口 | `SyncEngine::push_once` | `flush`、`upload` |
| 应用页 | `Store::apply_pull` | `ingest`、`merge` |
| 正文解析 | `tela_content::parse_blocks` | `html_to_blocks`、`parse_html` |
| 错误消息分隔 | `" → "`（U+2192，两侧各一空格） | `->`、`:` |

---

## 15. 立即可执行的第一步（给后续 coding 步骤）

```bash
# M0 / 切片 S0：先红
mkdir -p crates/tela-{proto,store,sync,net,content,ui,app}/src tests/fixtures/{sync,mutations,content,opml}
# 写 Cargo.toml（按 docs/architecture.md §3.1 的 workspace 片段）
# 先写 crates/tela-proto/tests/smoke.rs（C7 常量断言），此时 cargo test 必须失败（E0425）
cargo test -p tela-proto   # 期望：失败（红）
# 再实现 crates/tela-proto/src/lib.rs 中的常量
cargo test -p tela-proto   # 期望：0（绿）
cargo build --workspace --all-targets
cargo run -p tela-app -- --version
```

判定：以上 3 条命令全部退出码 0 且 `--version` 输出含 `tela-app`，M0 才算通过；任一条失败按 §11.3 排查，不得进入 M1。

---

## 附录 A：本计划的证据索引

| 结论 | 证据 |
|---|---|
| 仓库无代码 | `README.md`「当前仓库只有设计文档，还没有代码」；根目录 `*.toml` glob 命中 0；根目录仅 `README.md`/`docs/`/`.reference/` |
| 7 个 crate 与 workspace 形态 | `docs/architecture.md` §2.3、§3.1、§18 M0 |
| 依赖版本与 `edition`/`rust-version` | `docs/architecture.md` §3 表、§3.1 片段 |
| 里程碑 M0–M4 骨架 | `docs/architecture.md` §18 |
| 范围 P0/P1/P2 与"明确不做" | `docs/product-design.md` §5.1–§5.4 |
| 周次节奏 v0.1–v1.0 | `docs/product-design.md` §13 |
| 产品侧待确认 15 条、风险 8 条 | `docs/product-design.md` §15、§16 |
| Cookie 兼容阶段 0 的边界 | `docs/api-proposal.md` §12；`.reference/tela/apps/reader/worker/edge.ts:122,553-554` |
| 协议常量与 `PullResponse` | `.reference/tela/packages/sync/src/protocol.ts` |
| 15 表与行字段 | `.reference/tela/packages/sync/src/rows.ts` |
| mutation 上限与校验 | `.reference/tela/packages/sync/src/mutations.ts` |
| `gpui-kit 0.8.0` 与 `test-support` feature | `.reference/gpui-kit/crates/kit/Cargo.toml` |
