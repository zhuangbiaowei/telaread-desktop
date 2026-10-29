//! CLI 契约：demo 渲染、help、离线状态、未登录阅读的退出码。

use std::process::Command;

fn run(args: &[&str], db: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tela-app"))
        .args(args)
        .env("TELA_DB", db)
        .env("TELA_TOKEN_STORE", "memory")
        .output()
        .expect("run tela-app")
}

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");
    (dir, path)
}

#[test]
fn default_invocation_renders_the_offline_demo() {
    let (_dir, db) = temp_db();
    let out = run(&[], &db);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(stdout.contains("Hello Tela"), "stdout={stdout}");
    assert!(stdout.contains("你好，Tela"), "对照视图应含译文");
}

#[test]
fn demo_can_show_an_empty_chapter() {
    let (_dir, db) = temp_db();
    let out = run(&["demo", "--chapter", "mock-empty"], &db);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("本章没有可显示的正文内容"),
        "stdout={stdout}"
    );
}

#[test]
fn help_lists_the_real_api_workflow() {
    let (_dir, db) = temp_db();
    let out = run(&["--help"], &db);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    for command in [
        "login", "logout", "status", "sync", "list", "read", "meta", "demo",
    ] {
        assert!(stdout.contains(command), "help 缺少 {command}：{stdout}");
    }
}

#[test]
fn status_offline_reads_the_local_database_without_network() {
    let (_dir, db) = temp_db();
    let out = run(&["status", "--offline"], &db);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr={stderr}");
    assert!(stdout.contains("cursor"), "stdout={stdout}");
    assert!(stdout.contains("(未登录)"), "stdout={stdout}");
}

#[test]
fn reading_an_unknown_article_says_to_sync_first() {
    let (_dir, db) = temp_db();
    let out = run(&["read", "1"], &db);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(13), "stderr={stderr}");
    assert!(stderr.contains("本地库没有这篇文章"), "stderr={stderr}");
}

#[test]
fn requesting_a_translation_without_login_exits_with_the_login_code() {
    let (_dir, db) = temp_db();
    let out = run(&["read", "--translate", "1"], &db);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(21), "stderr={stderr}");
    assert!(stderr.contains("登录"), "stderr={stderr}");
}

#[test]
fn mutations_without_login_exit_with_the_login_code() {
    let (_dir, db) = temp_db();
    for args in [
        vec!["mark-read", "1"],
        vec!["mark-unread", "1"],
        vec!["like", "1"],
    ] {
        let out = run(&args, &db);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(21), "args={args:?} stderr={stderr}");
        assert!(stderr.contains("登录"), "args={args:?} stderr={stderr}");
    }
}

#[test]
fn sync_without_login_exits_with_the_login_code() {
    let (_dir, db) = temp_db();
    let out = run(&["sync"], &db);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(21), "stderr={stderr}");
}
