# Tela Desktop

基于 [Tela](https://telaread.com) 的桌面阅读器（Rust + [GPUI Kit](https://github.com/longbridge/gpui-kit)）。

> **当前进度**：**真实 API v1 底座已接通** —— OAuth 2.1 + PKCE 登录、刷新令牌进系统凭据库、
> 全量/增量同步、离线变更队列、正文对象与翻译、本地优先阅读、终端 CLI 与离线 GUI demo。
> 协议依据是官方 [authorization.md](https://github.com/hutusi/tela/blob/main/docs/api/authorization.md)
> 与 [v1.md](https://github.com/hutusi/tela/blob/main/docs/api/v1.md)（2026-10-28 实读）。
> 详细落地说明见 [`docs/real-api.md`](./docs/real-api.md)。

## 快速开始（真实 API）

前置：在 Tela 注册一个 public native client，redirect URI 登记为 `http://127.0.0.1/callback`
（不带端口，运行时用任意空闲端口）。

```bash
export TELA_CLIENT_ID=tela_app_…
# 需要邮箱时：export TELA_SCOPES="write email"

cargo run -p tela-app -- login          # 打开系统浏览器，回环端口接收授权码
cargo run -p tela-app -- sync           # 先推离线队列，再拉最新数据
cargo run -p tela-app -- list           # 订阅列表 + 未读数
cargo run -p tela-app -- list --feed 1  # 某个订阅下的文章
cargo run -p tela-app -- read 101       # 阅读（打开即已读；--no-mark-read 可关闭）
cargo run -p tela-app -- mark-read 101  # 标记已读（入离线队列并立即推送）
cargo run -p tela-app -- mark-unread 101
cargo run -p tela-app -- like 101       # 点赞；--unlike 取消
cargo run -p tela-app -- status         # 游标、行数、待发队列、今日额度
cargo run -p tela-app -- logout         # 撤销该设备令牌并清本地账号数据
```

不需要任何环境变量也能跑离线演示：

```bash
cargo run -p tela-app                    # 默认进入进程内 mock 的对照阅读页
cargo run -p tela-app -- demo --chapter mock-long
```

`tela-app --help` 列出全部子命令；`status --offline` 完全不访问网络。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `TELA_CLIENT_ID` | 构建期内置 | 发布包已内置 public client id；本地构建/调试可覆盖 |
| `TELA_SCOPES` | `write` | `read` / `write`，可附加 `email` |
| `TELA_API_BASE_URL` | `https://api.telaread.com` | API 基址 |
| `TELA_AUTH_BASE_URL` | `https://telaread.com` | 授权页站点 |
| `TELA_DB` | `%APPDATA%\tela\tela.db`（Windows）/ `$XDG_DATA_HOME/tela/tela.db` | SQLite 路径 |
| `TELA_LANG` | `zh-Hans` | 译文语言（`zh-Hans` / `zh-Hant` / `en` / `fr`） |
| `TELA_TOKEN_STORE` | `keyring` | `memory` 仅用于测试/临时会话 |
| `TELA_DEMO` | （未设置） | GUI 默认真实 API（登录模式）；`1`/`true` 进入 demo |
| `TELA_DEV_SEED` | （未设置） | demo 是否写演示数据；`0` 关闭 |

## 构建与测试

前置：Rust stable（本机验证 `rustc 1.98.1` / `cargo 1.98.1`；workspace 声明 `rust-version = "1.92"`）。
`reqwest` 使用 rustls（aws-lc-rs），首次构建需要 `cmake` 与 C 编译器；Linux 还需要 `pkg-config`。

```bash
cargo build --workspace --locked                          # 构建
cargo test --workspace --locked                           # 全部测试（含真实 API fixture 契约）
cargo run -p tela-app -- --version                        # tela-app 0.3.2
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

以上 5 条与 CI（[`.github/workflows/ci.yml`](./.github/workflows/ci.yml)）逐条对应。

真实 API 的在线冒烟（不需要令牌）：

```bash
TELA_TOKEN_STORE=memory TELA_DB=tmp/smoke.db cargo run -p tela-app -- meta
```

## GUI

GUI 依赖默认**不编译**：`tela-gui` 需要显式 `--features gui`，没有 Wayland/Vulkan 系统库的机器
仍可跑上面的默认检查。

```bash
# 默认：真实 API（登录模式）。未登录时侧栏显示「登录 Tela」，登录后自动同步
cargo run -p tela-app --features gui --bin tela-gui --locked

# 演示数据（不需要账号）：--demo，空库自动写入 2 个订阅 / 4 篇文章
TELA_DB=target/tela-gui.db \
  cargo run -p tela-app --features gui --bin tela-gui --locked -- --demo

# 本地调试真实 API：临时覆盖 client id / 库路径
source tmp/tela.env                 # 本地 client id / DB 路径（不提交）
cargo run -p tela-app --features gui --bin tela-gui --locked
scripts/tela-gui-real.sh --check    # 无窗口数据冒烟（读 tmp/tela.env + 真实装配）
```

发布包在构建时把 public client id 编进二进制（`.github/workflows/release.yml` 用仓库变量
`TELA_CLIENT_ID` 注入），所以用户解压后默认就是登录模式；`TELA_CLIENT_ID` 环境变量仍可覆盖。

- 左侧订阅、中间文章、右侧阅读面板；关闭最后一个窗口即退出。
- 侧栏底部是账号面板：未登录时显示「登录 Tela」，等待授权时可「打开授权页 / 复制链接 / 取消」，
  已登录显示 `@handle` 与 scope，并可「退出」；登录成功后自动同步并刷新列表。
- 选中文章或切换订阅（显示第一篇）会立即标为已读：本地先变、再后台推送；
  受 `reader.mark_on_open` 控制（CLI `read` 同理，可用 `--no-mark-read` 关闭）。
- 快捷键：`o` 原文 / `c` 对照 / `t` 译文 / `n` 下一页 / `p` 上一页 / `r` 重试。
- 演示数据只在 demo 装配且本地库为空时写入；`TELA_DEV_SEED=0` 可关闭。

```bash
# GUI 测试（无头；不需要 DISPLAY/WAYLAND_DISPLAY）
env -u DISPLAY -u WAYLAND_DISPLAY \
  cargo test -p tela-ui --features gui-test --locked

# 无窗口数据链路冒烟（CI 用）
TELA_DB=target/ci-gui.db TELA_DEV_SEED=1 \
  cargo run -p tela-app --features gui --bin tela-gui --locked -- --check
# → tela-gui check: feeds=2 articles=4 unread=3
```

![S3 三栏 + 对照阅读](docs/images/gui-s3-reader.png)

更多截图与逐条验收命令见 [`docs/gui_acceptance.md`](./docs/gui_acceptance.md)。

## 架构

依赖方向单向：`tela-app → {tela-ui, tela-sync} → {tela-store, tela-content, tela-net} → tela-proto`。

| 路径 | 职责 |
|---|---|
| `crates/tela-proto` | v1 wire 类型（14 张同步表、17 种 mutation、me/usage/meta）、错误、已读规则 |
| `crates/tela-store` | SQLite schema v2：行、不透明游标、离线队列、正文缓存 |
| `crates/tela-sync` | 拉取编排（reset/more/重试）、推送队列、状态快照 |
| `crates/tela-net` | OAuth PKCE、回环回调、系统凭据库、Bearer 客户端、错误映射 |
| `crates/tela-content` | 正文/译本模型、`/v1/bodies`、翻译状态与对象 |
| `crates/tela-ui` | 纯视图模型（`shell`/`reader/*`）+ GPUI 视图（`gui/*`） |
| `crates/tela-app` | 装配（`services.rs`）、终端 CLI、GPUI 入口 |
| `tests/fixtures/api/` | 官方 v1 示例响应体（同步/变更/me/usage/meta/翻译） |
| `tests/fixtures/content/` | 正文与译本对象 fixture |

### 关键语义

- **cursor 不透明**：与行同事务持久化；换语言/换行类型时客户端主动从快照重来。
- **离线队列**：`pending_mutations` 跨进程保存 `mid` 与 `at`；失败原样重发，有答复后整批丢弃。
- **已读**：按文档的 5 条优先级判定（`readAt` → `markedUnread` → 水位线 → 30 天 → 未读）。
- **令牌**：访问令牌只在内存；刷新令牌只进 Keychain / Credential Manager / Secret Service。
- **正文**：不可变对象，按 `contentKey` 缓存到 `bodies` 表，可离线重读。

更完整的接口映射、错误码与安全说明见 [`docs/real-api.md`](./docs/real-api.md)。

## 发布

发布由 [`release.yml`](./.github/workflows/release.yml) 自动完成：

```bash
# tag 去掉 v 前缀后必须等于 Cargo.toml 里 tela-app 的版本
# 仓库变量 TELA_CLIENT_ID 必须在构建前配好（发布包内置它）
git tag -a v0.3.2 -m "Tela Desktop v0.3.2"
git push origin v0.3.2
```

推送 tag 后：

1. 在 Linux 上校验 tag/版本并跑完整测试；
2. 在 `ubuntu-24.04` 与 `windows-latest` 原生构建 `tela-app` 与 `tela-gui`；
3. 生成 `tela-desktop-<tag>-x86_64-unknown-linux-gnu.tar.gz`、
   `tela-desktop-<tag>-x86_64-pc-windows-msvc.zip` 及对应 `.sha256`；
4. 创建 GitHub Release（自动生成 release notes）。

预发布用带连字符的 tag，会自动标记为 prerelease：

```bash
git tag -a v0.3.2-rc.1 -m "Tela Desktop v0.3.2-rc.1"
git push origin v0.3.2-rc.1
```

Linux GUI 依赖 Wayland/Vulkan/字体等系统库；Windows 包是原生 MSVC 构建。
当前不生成 macOS 产物。

### Windows 包是「解压即用」的

Windows 的 exe 静态链接 MSVC/C 运行库（见 [`.cargo/config.toml`](./.cargo/config.toml)
里的 `crt-static`），解压后直接双击 `tela-gui.exe` 即可，**不需要**另行安装
Visual C++ Redistributable。`release.yml` 在打包前会扫描两个 exe 的导入表，
一旦重新引入 `VCRUNTIME140.dll` / `MSVCP140.dll` 就让构建失败。

双击进入的是**登录模式**（发布包内置 public client id）：侧栏底部点「登录 Tela」，
浏览器完成授权后令牌进 Windows 凭据管理器，数据落在 `%APPDATA%\tela\tela.db`。
想离线看演示数据时用 `tela-gui.exe --demo`。

> 启动失败时没有控制台输出，原因会写到 `%APPDATA%\tela\tela-gui-error.log`。

> **v0.2.0 的 Windows 包没有静态 CRT**：双击会报
> `VCRUNTIME140.dll was not found`。请直接用 v0.2.1 及之后的包。

## 文档

| 文档 | 内容 |
|---|---|
| [真实 API 接入](./docs/real-api.md) | OAuth、同步/推送、正文/翻译、退出码、测试与下一步 |
| [剩余工作盘点](./docs/remaining-work.md) | 对照 M0–M5 与 P0/P1 的逐项状态、估算与下一批切片 |
| [API 方案（历史）](./docs/api-proposal.md) | 服务端公开 API 之前的调研与提议；现状以官方 v1 文档为准 |
| [架构设计](./docs/architecture.md) | 进程/线程、crate 划分、SQLite、同步/reducer、渲染管线 |
| [产品设计](./docs/product-design.md) | 目标用户、功能范围、关键界面、路线图 |
| [开发计划](./docs/development_plan.md) | 阶段/切片、接口契约、验收命令 |
| [GUI 验收](./docs/gui_acceptance.md) | S0–S4 的交付、命令实测、截图 |

## 调研基线

- `hutusi/tela` @ `f57fa6f`（2026-10-25）；API 文档实读于 2026-10-28
- `longbridge/gpui-kit` @ `7b14e03`（`gpui-kit 0.8.0`，GPUI snapshot `gpui-pre 0.3.9`）

## 下一步

1. GUI 的周期性同步与列表自动刷新（开窗前已同步一轮，`AppServices::sync_once` 可复用）；
2. Unread / Today / Liked 列表与「全部已读 + 撤销」；
3. 划线、文件夹、关注、OPML 的 UI；
4. Discover / 站点 / 个人页 / 搜索（公共只读，`lang` 必带）；
5. 图片代理与三平台打包、自动更新。

## 免责声明

本项目是个人/社区项目，与 Tela 官方无隶属关系。名称、图标、内容的使用需取得作者许可。


## 许可证

代码以 **MIT OR Apache-2.0** 双许可发布，见 `LICENSE-MIT` 与 `LICENSE-APACHE`。
Tela 的名称、图标与内容不在此许可范围内，使用需取得作者许可。
