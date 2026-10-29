# M0 验收记录（骨架 + 测试基建）

> 契约来源：`docs/development_plan.md` §4（workspace 契约）、§5（crate 职责）、§9「M0 — 骨架 + 测试基建」（交付物与 6 条验收方式）、§10.1（测试与 fixture 路径）、§15（立即可执行的第一步）；`docs/architecture.md` §2.3（crate 划分）、§3.1（版本锁定）、§18（里程碑 M0）。
>
> 本文件只记录**真实执行过的命令与其真实输出/退出码**。凡未执行、未验证或被降级的项，一律在 §7「未完成项与已知遗留」中显式标注，不以「文件存在」代替行为验证。
>
> 记录时间：2026-10-26（CST）。

---

## 1. 验收环境

### 1.1 探测命令与真实输出

```bash
command -v cargo rustc rustfmt python3 node
cargo --version; rustc --version; cargo clippy --version; rustfmt --version
uname -srm; echo "SHELL=$SHELL"; cat /etc/os-release | head -3
grep -nE "^(members|resolver)|^(edition|rust-version|version|license)\s*=|^\[workspace" Cargo.toml
```

实测结果：

| 项 | 实测值 |
| --- | --- |
| 操作系统 | `Linux 7.2.8-9-omarchy x86_64`（`Omarchy`，`ID=omarchy`） |
| Shell | `/usr/bin/bash`（后文所有命令均为 bash 语法） |
| `cargo` | `cargo 1.98.1 (797e8a9bc 2026-08-05)` |
| `rustc` | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| `cargo clippy` | `clippy 0.1.98 (48a229ceae 2026-09-01)` |
| `rustfmt` | `rustfmt 1.9.0-stable (48a229ceae 2026-09-01)` |
| 工具链来源 | `/home/<user>/.cargo/bin/{cargo,rustc}`（rustup 默认工具链，无版本切换） |

### 1.2 项目声明的工具链契约（根 `Cargo.toml`）

| 键 | 值（实测） |
| --- | --- |
| `[workspace] resolver` | `"2"` |
| `[workspace] members` | `["crates/*"]` |
| `[workspace.package] version` | `"0.1.0"` |
| `[workspace.package] edition` | `"2024"` |
| `[workspace.package] rust-version` | `"1.92"` |
| `[workspace.package] license` | `"MIT OR Apache-2.0"` |
| `[workspace.lints.rust]` | `unsafe_code = "forbid"` |
| `[workspace.lints.clippy]` | `dbg_macro = "warn"`、`todo = "warn"`、`unimplemented = "warn"` |

**版本一致性结论**：项目要求 `rust-version = 1.92`、`edition = 2024`；实测工具链 `rustc 1.98.1` ≥ 1.92，且 1.98.1 已支持 edition 2024，**无需切换/降级工具链**。`edition = "2024"` 与 `rust-version = "1.92"` 的降级风险（计划 Q-11）在本环境**未触发**，未做任何静默降级。

### 1.3 依赖安装形态

- 仓库内含 `Cargo.lock`（209 976 B），全部验收命令均可在 `--locked` 下通过（见 §3），说明**依赖解析结果与锁文件一致，无隐式版本漂移**。
- 沙箱内 `target/` 与 cargo registry 已具备，验收命令为**增量构建**；首次冷构建会额外下载 `clap/serde/serde_json/thiserror/uuid` 等 M0 实际启用的依赖。
- 外部依赖按里程碑启用（`Cargo.toml` `[workspace.dependencies]` 注释）：M0 实际编译链只含 `clap`（`tela-app`）+ `serde/serde_json/thiserror/uuid`（`tela-proto`）；`rusqlite/im/reqwest/tokio/keyring/scraper/image/tracing/anyhow` 仅锁定版本、无 crate 引用，因此**不参与 M0 构建**（见 §7 遗留项 4）。

---

## 2. 交付范围总览

| # | M0 交付范围（用户确认） | 落盘位置 | 验收条目 | 结论 |
| --- | --- | --- | --- | --- |
| R1 | 项目骨架与依赖声明（按计划技术栈） | 根 `Cargo.toml`、`crates/*/Cargo.toml`、`crates/*/src/lib.rs`、`Cargo.lock` | A1–A6 | ✅ 通过 |
| R2 | 测试框架 + 一条可运行冒烟测试 | `crates/tela-proto/tests/smoke.rs` 等 4 个测试文件 + `tests/fixtures/sync/*.json` | B1–B5 | ✅ 通过（含红→绿证据） |
| R3 | Lint / 格式化 / CI 流水线配置 | `rustfmt.toml`、根 `Cargo.toml` `[workspace.lints]`、`.github/workflows/ci.yml` | C1–C4 | ✅ 本地通过；远端 CI 未实跑（见 §7 遗留项 1） |
| R4 | 最小可运行入口（CLI） | `crates/tela-app/src/main.rs`（`[[bin]] name = "tela-app"`） | D1–D4 | ✅ 通过 |
| R5 | M0 验收记录文档 | `docs/m0_acceptance.md`（本文件） | E1 | ✅ 通过 |

---

## 3. 验收项清单（命令 / 预期 / 实际 / 结论）

> 表格中"实际"列是从终端原样摘录的输出与退出码；命令全部为**失败即非零**形式，未使用 `|| true` / `|| echo` 掩盖失败。

### R1 项目骨架与依赖声明

| # | 验收项 | 命令 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- | --- | --- |
| A1 | workspace 可构建（计划 §9 验收 1） | `cargo build --workspace` | 退出码 0 | `exit=0`，stderr：`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 0.08s` | ✅ |
| A2 | 锁文件一致（CI Build 步骤原文） | `cargo build --workspace --locked` | 退出码 0 | `exit=0`，stderr：`Finished ... in 0.12s` | ✅ |
| A3 | 全部 target 可编译（计划 §15） | `cargo build --workspace --all-targets` | 退出码 0 | `all_targets_exit=0`，stderr：`Finished ... in 0.09s` | ✅ |
| A4 | 测试代码可编译（计划 §9 验收 2） | `cargo test --workspace --no-run` | 退出码 0 | `accept2_exit=0` | ✅ |
| A5 | 7 个 crate 骨架齐备 | `ls crates/*/Cargo.toml` | 7 个 crate manifest | `tela-app`、`tela-content`、`tela-net`、`tela-proto`、`tela-store`、`tela-sync`、`tela-ui` 共 7 个 | ✅ |
| A6 | 依赖声明可解析、无未解析依赖 | 由 A1/A2/A4 覆盖（cargo 负责解析 `[workspace.dependencies]` 与 `path` 内部依赖） | 无 `error:` 输出 | 三次构建/编译均无 `error:`，仅 `Finished` 行 | ✅ |

内部依赖方向（`Cargo.toml` 注释与各 crate manifest 一致，单向无环）：`tela-app → {tela-ui, tela-sync, tela-net, tela-content, tela-store, tela-proto} → tela-proto`。

### R2 测试框架 + 冒烟测试

| # | 验收项 | 命令 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- | --- | --- |
| B1 | 冒烟测试真实运行（计划 §9 验收 3） | `cargo test -p tela-proto` | 退出码 0，输出含 `1 passed` | `accept3_exit=0`；`constants_are_pinned ... ok` + `test result: ok. 1 passed`；另有 `fixtures_sync` `2 passed`、`exports_contract` `1 passed`、lib unittest `0 passed` | ✅ |
| B2 | 完整测试套件（CI Test 步骤原文） | `cargo test --workspace --locked` | 退出码 0，0 failed | `exit=0`；所有测试目标 `test result: ok`（`cli_version` 2 passed、`exports_contract` 1 passed、`fixtures_sync` 2 passed、`smoke` 1 passed，其余 crate `0 passed; 0 failed`，含 6 个 doc-test 目标 `0 passed`） | ✅ |
| B3 | fixture 非空（计划 §9 验收 5） | `test -s tests/fixtures/sync/snapshot-minimal.json && test -s tests/fixtures/sync/invalid-missing-seq.json` | 退出码 0 | `accept5_exit=0`（`snapshot-minimal.json` 1 438 B、`invalid-missing-seq.json` 1 419 B） | ✅ |
| B4 | 公开符号契约（计划 §9 验收 6） | `cargo test -p tela-proto --test exports_contract` | 退出码 0，符号可导入 | `accept6_exit=0`；`protocol_constants_are_importable_and_invariant ... ok`，`1 passed` | ✅ |
| B5 | CLI 契约测试 | `cargo test -p tela-app --test cli_version` | 退出码 0，2 passed | `app_green_exit=0`；`version_exits_zero_and_prints_crate_name_and_semver ... ok`、`unknown_argument_exits_with_code_2 ... ok`，`2 passed` | ✅ |

测试框架说明：使用 **Rust 内建 `cargo test`（libtest）**，无额外测试框架依赖——这是 Rust 生态默认设施，`cargo test --workspace` 即测试基础设施入口。集成测试以 `crates/<crate>/tests/*.rs` 独立 binary 组织，fixture 走仓库内 `tests/fixtures/`（计划 §10.1 路径）。

### R3 Lint / 格式化 / CI

| # | 验收项 | 命令 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- | --- | --- |
| C1 | 格式化检查（CI Format check 原文） | `cargo fmt --all --check` | 退出码 0，无 diff | `fmt_exit=0`，无输出 | ✅ |
| C2 | 静态检查（CI Clippy 原文） | `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 0，0 warning | `clippy_exit=0`，stderr 仅 `Finished ... in 0.11s` | ✅ |
| C3 | lint 规则生效 | 根 `Cargo.toml` `[workspace.lints.*]` + 各 crate `[lints] workspace = true` | `unsafe_code` 被 forbid | 各 crate manifest 均含 `[lints] workspace = true`；C2 以 `-D warnings` 全绿 | ✅ |
| C4 | CI 配置与本地命令一致 | 对照 `.github/workflows/ci.yml` 步骤 | CI 每条 `run:` 均有本地等价执行 | 5 条 `run:` 全部对应到 C1/C2/A2/B2/D1（差异见下） | ✅（本地等价） |

`.github/workflows/ci.yml`（job：`fmt + clippy + build + test`）的 `run:` 步骤与本地复现映射：

| CI 步骤 | CI 命令原文 | 本地复现 | 结果 |
| --- | --- | --- | --- |
| Show toolchain | `cargo --version && rustc --version && cargo clippy --version && rustfmt --version` | §1.1 同名命令 | exit 0 |
| Format check | `cargo fmt --all --check` | C1 | exit 0 |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | C2 | exit 0 |
| Build | `cargo build --workspace --locked` | A2 | exit 0 |
| Test | `cargo test --workspace --locked` | B2 | exit 0 |
| CLI entry smoke (M0 验收第 4 条) | `cargo run -p tela-app --locked -- --version` | D1（**去掉 `--locked`**，按计划 §9 验收 4 原文） | exit 0 |

> 差异说明（非行为性）：CI 的 CLI smoke 多一个 `--locked`，只影响依赖解析阶段的锁文件校验，不改变二进制行为；`--locked` 形式已在 A2/B2 上验证通过。沙箱未执行 `--locked` 版 CLI smoke 的唯一原因是避免对同一行为重复执行等价命令。

### R4 最小可运行入口

| # | 验收项 | 命令 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- | --- | --- |
| D1 | `--version` 可用（计划 §9 验收 4） | `cargo run -p tela-app -- --version` | 退出码 0，stdout 含 `tela-app` | stdout：`tela-app 0.1.0`；`accept4_exit=0` | ✅ |
| D2 | 未知参数退出码 2（计划 §7.2 退出码表） | `cargo run -q -p tela-app -- --definitely-not-a-flag` | 退出码 **2**，stderr 含 clap 错误与 usage | `badarg_exit=2`；stderr 首行：`error: unexpected argument '--definitely-not-a-flag' found`（随后为 `Usage: tela-app`） | ✅ |
| D3 | 模块导入可用 | `cargo run -p tela-app -- --version` 成功 + `cargo test -p tela-proto --test exports_contract` | 入口链接到 6 个内部 crate 且公开符号可导入 | D1 exit 0（`tela-app` 二进制链接 `tela-ui/sync/net/content/store/proto`）；B4 1 passed | ✅ |
| D4 | 入口为真实进程行为断言（非仅文件存在） | `crates/tela-app/tests/cli_version.rs` 断言子进程 `status.code()` 与 `stdout` 字符串 | 退出码/stdout 逐字断言 | 见 §4 红→绿证据（变异后 2 条断言真实失败） | ✅ |

### R5 验收记录本身

| # | 验收项 | 命令 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- | --- | --- |
| E1 | 验收文档落盘且非空 | `test -s docs/m0_acceptance.md` | 退出码 0 | 退出码 0（文件由本步骤 `box_write_file` 写入，非空） | ✅ |
| E2 | 文档结论与真实执行一致 | 对照本文件 §3 表格与 §4 证据 | 每条结论可复现 | 所有"实际"列取自本次真实终端输出；失败/降级项集中在 §7 | ✅ |

---

## 4. 冒烟测试红 → 绿证据

### 4.1 首轮 TDD 红→绿（切片 S0 / S1，step-001 记录）

| 切片 | 命令 | 红（失败证据） | 绿（通过证据） |
| --- | --- | --- | --- |
| S0 协议常量冒烟测试 | `cargo test -p tela-proto` | 实现前 `tela_proto` 常量未定义，编译错误 `E0432`（`unresolved import tela_proto::{CLIENT_HEADER, ...}`），退出码 101 | `constants_are_pinned ... ok`，`1 passed`，退出码 0 |
| S1 CLI 入口契约 | `cargo test -p tela-app --test cli_version` | `2 failed`，`--version` 与退出码 2 两条断言均失败，退出码 101 | 2 条断言 `ok`，`2 passed`，退出码 0 |
| S2 格式化 | `cargo fmt --all --check` | 首轮存在 diff（检查非零退出） | 修复后 `fmt_exit=0`，无输出 |
| S3 Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | 首轮报 `assertions_on_constants` 等告警（`-D warnings` 下失败） | 修复后 `clippy_exit=0` |

### 4.2 本次验收的「变异校验」（证明测试仍在真实断言行为，非空转）

为确认测试套件此刻依然具备检测能力（而不是"永远为绿"的摆设），本次对两处实现做了**受控变异 → 观察红 → 逐字节还原 → 观察绿**，还原以 `sha256` 校验（`git status --porcelain` 输出为空，确认工作区无残留改动）。

**变异 1：`PAGE_ROWS` 1000 → 999（`crates/tela-proto/src/lib.rs`）**

```
$ cargo test -p tela-proto --test smoke
test constants_are_pinned ... FAILED
---- constants_are_pinned stdout ----
thread 'constants_are_pinned' panicked at crates/tela-proto/tests/smoke.rs:16:5:
assertion `left == right` failed: 分页每次最多 1000 行（R1）
  left: 999
 right: 1000
test result: FAILED. 0 passed; 1 failed; ...
red_smoke_exit=101
```

**变异 2：`tela-app --version` 输出版本改为 `0.0.0`（`crates/tela-app/src/main.rs`）**

```
$ cargo test -p tela-app --test cli_version
test version_exits_zero_and_prints_crate_name_and_semver ... FAILED
test unknown_argument_exits_with_code_2 ... ok
---- version_exits_zero_and_prints_crate_name_and_semver stdout ----
thread '...' panicked at crates/tela-app/tests/cli_version.rs:33:5:
assertion `left == right` failed: `tela-app --version` 输出应为 "tela-app 0.1.0\n"，实际 stdout="tela-app 0.0.0\n"
  left: "tela-app 0.0.0\n"
 right: "tela-app 0.1.0\n"
test result: FAILED. 1 passed; 1 failed; ...
red_cli_exit=101
```

> 同一文件内两条断言表现不同（`unknown_argument_exits_with_code_2` 仍 `ok`），说明两条测试各自独立断言真实行为，不互相遮蔽。

**还原校验与复绿**

| 文件 | 备份 sha256 | 还原后 sha256 | 是否一致 |
| --- | --- | --- | --- |
| `crates/tela-proto/src/lib.rs` | `c2c771b0a86ccd16a8f92f91c0b3c125b345a1c0f2cf4f4748d3866164cfe98c` | `c2c771b0a86ccd16a8f92f91c0b3c125b345a1c0f2cf4f4748d3866164cfe98c` | ✅ |
| `crates/tela-app/src/main.rs` | `8be29119f3f0bc5f53936965331c214404d9ae2ebe7b7256404340f74d68eff4` | `8be29119f3f0bc5f53936965331c214404d9ae2ebe7b7256404340f74d68eff4` | ✅ |

```
$ git status --porcelain        # 空输出：工作区无残留改动
$ cargo test -p tela-proto --test smoke --test fixtures_sync --test exports_contract
test protocol_constants_are_importable_and_invariant ... ok      (1 passed)
test snapshot_minimal_matches_section_8_1 ... ok                 (2 passed)
test invalid_missing_seq_only_lacks_article_seq ... ok
test constants_are_pinned ... ok                                 (1 passed)
proto_green_exit=0

$ cargo test -p tela-app --test cli_version
test version_exits_zero_and_prints_crate_name_and_semver ... ok
test unknown_argument_exits_with_code_2 ... ok
test result: ok. 2 passed; 0 failed
app_green_exit=0
```

**结论**：冒烟测试具备真实检测力（红：退出码 101 + 逐字断言差异；绿：退出码 0），当前实现为绿。

---

## 5. 格式化 / Lint / CI 一致性说明

- **格式化**：`rustfmt.toml`（33 B）为唯一格式化配置；CI 与本地均执行 `cargo fmt --all --check`，本次 exit 0。
- **Lint**：规则集中在根 `Cargo.toml` 的 `[workspace.lints.rust]` / `[workspace.lints.clippy]`，各 crate 用 `[lints] workspace = true` 继承（避免逐 crate 复制）。`unsafe_code = "forbid"` 意味着任何 `unsafe` 会直接编译失败；`dbg_macro/todo/unimplemented` 为 warn，在 `-D warnings` 下同样会阻断 CI。
- **CI**：`.github/workflows/ci.yml` 单 job `fmt + clippy + build + test`，步骤顺序 = 装工具链（读取仓库声明的 rust-version）→ 缓存 registry/target → 打印工具链版本（便于版本漂移归因）→ fmt → clippy → build `--locked` → test `--locked` → CLI smoke。本地已逐条等价复现（见 §3 C4 映射表），**唯一未做的是在 GitHub 托管 runner 上真实触发**，故只能声称"本地等价通过"，不能声称"远端 CI 绿色"（见 §7 遗留项 1）。

---

## 6. 目录结构说明

结构严格采用计划/架构文档的既有约定，**未新增或改名任何目录**：

| 路径 | 依据 | 说明 |
| --- | --- | --- |
| `crates/tela-{proto,store,sync,net,content,ui,app}/` | `architecture.md` §2.3、§3.1（`members = ["crates/*"]`）、计划 §4 | 7 个 crate，每个含 `Cargo.toml` + `src/lib.rs`（`tela-app` 为 `src/main.rs` 二进制） |
| `tests/fixtures/sync/{snapshot-minimal,invalid-missing-seq}.json` | 计划 §10.1（F1/F3 fixture） | M0 只落盘文件并校验非空；其内容断言在 `crates/tela-proto/tests/fixtures_sync.rs`（2 passed） |
| `crates/*/tests/*.rs` | Rust 集成测试惯例（计划 §10.1 测试清单） | `smoke.rs`（协议常量冒烟）、`exports_contract.rs`（公开符号契约）、`fixtures_sync.rs`（fixture 内容断言）、`cli_version.rs`（入口子进程断言） |
| `rustfmt.toml` | 计划 §4 lint/format 约定 | 格式化唯一配置源 |
| `.github/workflows/ci.yml` | 用户确认的 M0 范围（Lint/格式化/CI） | 本地已等价复现 |
| `Cargo.lock` | 可复现构建 | 209 976 B，`--locked` 全通过 |
| `README.md` | 计划 §9 交付物「README 补『构建与测试』段」 | 第 9 行为 `## 构建与测试`（3 895 B） |
| `tests/artifacts/` | 计划 §9 M1 用（`--self-check --db tests/artifacts/m1.db`） | M0 未产生，M1 再落盘 |

---

## 7. 未完成项与已知遗留（如实标注）

| # | 项 | 状态 | 影响 / 后续动作 |
| --- | --- | --- | --- |
| 1 | **远端 CI 未在 GitHub Actions runner 实跑** | ⚠️ 未验证 | 沙箱内无 GitHub runner 与 Actions 网络，`.github/workflows/ci.yml` 只做了本地等价复现。**不得声称"CI 流水线绿色"**；首次 push 后需看真实 runner 结果（尤其 `Swatinem/rust-cache`、toolchain 安装在托管环境的可用性）。 |
| 2 | **GUI（gpui-kit）未启用/未验证** | 🔻 降级（按里程碑） | `tela-ui` 将 `gpui-kit = { workspace = true, optional = true }`，通过 `gui` feature 门控，M0 默认不启用：GUI 依赖需要 `libxkbcommon`/`vulkan` 等系统库与图形会话，若在 M0 强制启用会让 `cargo build --workspace` 在无 GUI 系统库的机器（含 CI runner）不可复现。**这是对"计划 §4 技术栈"的一处有意降级**，M2 接入时必须改造 CI（安装系统库）并补齐 GUI 冒烟验证。 |
| 3 | **M1 业务能力未实现** | ⏳ 后续里程碑 | `tela-store`/`tela-sync`/`tela-net`/`tela-content`/`tela-ui` 目前仅骨架 `lib.rs`；`tela-proto` 只有 4 个协议常量，`PullResponse`/`SyncRows`/`Mutation`/`TelaError` 未实现；`tela-app --self-check`（计划 §9 M1 验收 2/3，期望退出码 0 / 10）不存在。M0 门禁不要求这些。 |
| 4 | **未使用的外部依赖未做兼容性验证** | ⚠️ 未验证 | `rusqlite/im/reqwest/tokio/keyring/scraper/image/tracing/anyhow` 仅在 `[workspace.dependencies]` 锁定版本，M0 无 crate 引用，因此**从未被编译**——其与 `rust-version = "1.92"`、`edition 2024` 的兼容性要到 M1 引入时才验证。M0 的"依赖声明完整"仅指声明齐备且不影响构建，不等于已验证可编译。 |
| 5 | **覆盖率 / benchmark / 依赖审计未配置** | ⚠️ 缺失 | 未接入 `cargo-llvm-cov`/`cargo-tarpaulin`、`criterion`、`cargo audit`/`cargo deny`。`Cargo.lock` 无安全扫描记录。 |
| 6 | **跨平台未验证** | ⚠️ 未验证 | 仅在 `Linux x86_64` 验证。macOS/Windows 的 GPUI 系统库、`keyring` 后端（计划 M1+）、窗口/图形会话行为均未测。 |
| 7 | **计划 Q-9（gpui-kit UI 测试 harness）未解决** | ⏳ M2 前置 | `.reference/gpui-kit/crates/kit/TESTING.md` 未逐字阅读，`#[gpui_kit::test]` 用法与 UI 测试约定未定，M0 未涉及 UI 测试。 |
| 8 | **计划 §9 验收 3 的"输出含 1 passed"口径** | 📝 口径说明 | `cargo test -p tela-proto` 现在一次运行 4 个测试目标（lib 0 / `smoke` 1 / `fixtures_sync` 2 / `exports_contract` 1），输出中既有 `1 passed` 也有 `2 passed`。冒烟测试确实真实运行并通过，未偏离计划意图，但**不能只靠"输出含 1 passed"一个字符串判断通过**，建议以退出码 0 + 具体测试名 `constants_are_pinned ... ok` 为准。 |
| 9 | **计划 §9 未把 fmt/clippy 列为 M0 门禁** | 📝 范围扩展（已确认） | 用户确认的 M0 范围包含 Lint/格式化/CI，故本记录将 C1–C4 纳入验收；计划 §9 原文 6 条验收仍全部单独通过（A1/A4/B1/D1/B3/B4）。 |
| 10 | **M0 未打 tag / 未做回滚演练** | ⏳ 未执行 | 计划 §9 提到按里程碑打 tag（`m0`…`m5`）与回滚方式（删 `crates/` 与根 `Cargo.toml`）；本步骤未创建 git tag，回滚路径仅文档化、未演练。 |

---

## 8. 复跑清单（可逐条复制执行）

单行 `&&` 链（任一条失败即整体非零退出）：

```bash
cargo build --workspace --locked && cargo test --workspace --no-run && cargo test -p tela-proto && cargo test -p tela-app --test cli_version && cargo run -p tela-app -- --version && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --locked && test -s tests/fixtures/sync/snapshot-minimal.json && test -s tests/fixtures/sync/invalid-missing-seq.json && test -s docs/m0_acceptance.md
```

期望：**退出码 0**，且输出含 `tela-app 0.1.0`、`constants_are_pinned ... ok`、`2 passed`（CLI 契约）、`test result: ok`。

计划 §9 六条验收原文（逐条独立执行）：

```bash
# 1
cargo build --workspace
# 2
cargo test --workspace --no-run
# 3
cargo test -p tela-proto
# 4
cargo run -p tela-app -- --version
# 5
test -s tests/fixtures/sync/snapshot-minimal.json && test -s tests/fixtures/sync/invalid-missing-seq.json
# 6
cargo test -p tela-proto --test exports_contract
```

全部实测结果：`exit 0 / 0 / 0 / 0（stdout=tela-app 0.1.0）/ 0 / 0`。

---

## 9. 结论与进入 M1 的建议

**M0 门禁判定：通过（本地环境）**。计划 §9 六条验收全部退出码 0；用户确认的五项 M0 交付范围均有真实行为验证记录（其中 R3 的远端 CI 执行属未验证项，见 §7-1）；冒烟测试具备真实检测力（红 exit 101 / 绿 exit 0，并用变异校验复证）。

进入 M1（协议 + 存储 + 拉取）的建议前置动作：

1. 首次 push 后确认真实 GitHub Actions 结果，把 runner 实测写回本文件 §3 C4。
2. 按计划 §6 的 C1–C5/C8/C9/C12 切片推进，先写失败测试（如 `PullResponse` 反序列化、`apply_pull` 未读数），再实现；`--self-check` 的退出码 0/10 契约必须做子进程断言。
3. 引入 `rusqlite`（`bundled`）时立即验证与 `rust-version = "1.92"` 的兼容性，并把 `--locked` 构建纳入复验（§7-4）。
4. M2 接入 gpui-kit 前先读 `.reference/gpui-kit/crates/kit/TESTING.md`（§7-7），并改造 CI 安装 GUI 系统库（§7-2）。
