//! 打开系统浏览器。
//!
//! 文档明确要求用成员自己的浏览器，**不能**用应用内 WebView：成员的 Tela 会话在浏览器里，
//! WebView 会让应用看到他们输入的内容（RFC 8252 §8.12）。

use std::process::Command;

use tela_proto::TelaError;

/// 用系统默认浏览器打开 URL。
pub fn open(url: &str) -> Result<(), TelaError> {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    match result {
        Ok(_child) => Ok(()),
        Err(err) => Err(TelaError::Config {
            detail: format!("无法打开系统浏览器（{err}）；请手动访问上面打印的授权地址"),
        }),
    }
}
