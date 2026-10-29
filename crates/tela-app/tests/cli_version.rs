//! 切片 S1：`tela-app` 最小可运行入口的 CLI 契约测试。
//!
//! 契约来源：docs/development_plan.md §6 C12（`--version` 退出码 0）、
//! §7.2 退出码表（2 = CLI 参数错误）。
//!
//! 断言真实进程行为（退出码 + stdout 文本），不做"文件存在/不抛错"式断言。

use std::process::Command;

/// 运行 `tela-app` 可执行文件（由 cargo 提供 `CARGO_BIN_EXE_tela-app`）。
fn run_tela_app(args: &[&str]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_tela-app");
    Command::new(exe)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("无法启动 {exe}：{e}"))
}

/// `--version` 必须退出码 0，并在 stdout 打印 `tela-app <semver>`。
#[test]
fn version_exits_zero_and_prints_crate_name_and_semver() {
    let out = run_tela_app(&["--version"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert_eq!(
        out.status.code(),
        Some(0),
        "`tela-app --version` 退出码应为 0，实际 stdout={stdout:?} stderr={stderr:?}"
    );

    let expected = format!("tela-app {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        stdout, expected,
        "`tela-app --version` 输出应为 {expected:?}，实际 stdout={stdout:?}"
    );
}

/// 未知参数属 CLI 参数错误，按 §7.2 必须以退出码 2 结束（不是 0/1）。
#[test]
fn unknown_argument_exits_with_code_2() {
    let out = run_tela_app(&["--definitely-not-a-flag"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert_eq!(
        out.status.code(),
        Some(2),
        "未知参数退出码应为 2（§7.2），实际 stdout={stdout:?} stderr={stderr:?}"
    );
}
