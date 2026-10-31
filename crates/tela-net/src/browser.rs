//! 打开系统浏览器。
//!
//! 文档明确要求用成员自己的浏览器，**不能**用应用内 WebView：成员的 Tela 会话在浏览器里，
//! WebView 会让应用看到他们输入的内容（RFC 8252 §8.12）。

use std::process::Command;

use tela_proto::TelaError;

/// 用系统默认浏览器打开 URL。
pub fn open(url: &str) -> Result<(), TelaError> {
    match command_for(current_platform(), url).spawn() {
        Ok(_child) => Ok(()),
        Err(err) => Err(TelaError::Config {
            // 调用方各有上下文（授权页 / 原文 / 主页），这里只说明系统动作失败；
            // 授权 CLI 会把完整 URL 打印出来，GUI 会把链接放进提示或剪贴板。
            detail: format!("无法打开系统浏览器（{err}）"),
        }),
    }
}

/// 需要在哪个平台上打开浏览器（抽出来是为了让命令构造可以在任意平台上测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Macos,
    Windows,
    Other,
}

fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    }
}

fn command_for(platform: Platform, url: &str) -> Command {
    match platform {
        Platform::Macos => {
            let mut command = Command::new("open");
            command.arg(url);
            command
        }
        Platform::Windows => {
            // 不要用 `cmd /C start "" <url>`：cmd 会把 URL 里的 `&` 当成命令分隔符，
            // 浏览器只会收到 `...?response_type=code`，后面的 client_id、redirect_uri、
            // scope、state、PKCE 参数全部丢掉（v0.3.0 的「打开」与「复制链接」不一致）。
            // rundll32 直接调用系统的 FileProtocolHandler，URL 不经过任何 shell 解析。
            let mut command = Command::new("rundll32.exe");
            command.arg("url.dll,FileProtocolHandler").arg(url);
            command
        }
        Platform::Other => {
            let mut command = Command::new("xdg-open");
            command.arg(url);
            command
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_URL: &str = "https://telaread.com/connect?response_type=code\
&client_id=tela_app_example\
&redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback\
&scope=write\
&state=0123456789abcdef\
&code_challenge=abcdefghijklmnopqrstuvwxyz\
&code_challenge_method=S256";

    #[test]
    fn windows_does_not_route_the_url_through_cmd() {
        let command = command_for(Platform::Windows, AUTH_URL);
        assert_ne!(
            command.get_program().to_string_lossy(),
            "cmd",
            "URL 不能再经过 cmd：`&` 会被当成命令分隔符"
        );
        assert_eq!(command.get_program().to_string_lossy(), "rundll32.exe");
    }

    #[test]
    fn the_whole_url_reaches_the_browser_on_every_platform() {
        for platform in [Platform::Macos, Platform::Windows, Platform::Other] {
            let command = command_for(platform, AUTH_URL);
            let args: Vec<String> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(
                args.iter().any(|arg| arg == AUTH_URL),
                "{platform:?} 必须把完整 URL（含 & 之后的参数）作为单个参数传给浏览器，实际参数：{args:?}"
            );
        }
    }
}
