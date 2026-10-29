//! `tela-gui --check` 的无显示器冒烟：装配服务 + 演示 seed + 列表链路。
//!
//! 不创建 GPUI 窗口，因此可以在 CI 的 GUI job 里跑；窗口本身的冒烟在
//! `tela-ui` 的 `gui_*` 测试里（同样无头）。

use std::process::Command;

#[test]
fn check_reports_demo_feeds_articles_and_unread() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("tela.db");

    let output = Command::new(env!("CARGO_BIN_EXE_tela-gui"))
        .arg("--check")
        .env("TELA_DB", &db)
        .env("TELA_DEV_SEED", "1")
        .env("TELA_CONTENT_API", "mock")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run tela-gui --check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code()
    );
    assert!(
        stdout.contains("feeds=2 articles=4 unread=3"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(db.exists(), "check 应真实打开/创建 SQLite 文件");
}
