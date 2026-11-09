//! 用大白话拼出"要起一个什么样的 daemon"，再把它交给 [`std::process::Command`]。
//!
//! 参数拼装单独成文件，是因为它有几条容易出错的规矩（壳自己管 `--listen`
//! 与 `--static-dir`，用户传进来的同名列必须丢掉；没有前端产物就不要传
//! `--static-dir` 让别人去猜），而这些规矩能在 Linux 上直接测。

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::supervisor::{Launcher, Sidecar};

/// 壳替用户定的参数，用户再给也不作数。
const RESERVED: [&str; 2] = ["--listen", "--static-dir"];

/// 起一个 daemon 需要知道的全部。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSpec {
    /// 可执行文件。
    pub program: PathBuf,
    /// 监听地址，壳选好端口后写死成 `127.0.0.1:端口`。
    pub address: String,
    /// 前端产物目录；找不到就不传 `--static-dir`，让 daemon 用自己的默认
    /// 并降级成"未构建前端"的提示（[13-deployment](../../../docs/13-deployment.md)）。
    pub ui_dir: Option<PathBuf>,
    /// 用户在桌面图标之外多给的参数，原样透传给 daemon（`--channel can0` 这类）。
    pub passthrough: Vec<String>,
    /// daemon 的 stdout/stderr 落到这里。
    pub log: PathBuf,
}

impl DaemonSpec {
    /// 参数向量（不含可执行文件名）。
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--listen".to_owned(), self.address.clone()];
        if let Some(ui) = &self.ui_dir {
            args.push("--static-dir".to_owned());
            args.push(ui.display().to_string());
        }
        args.extend(drop_reserved(&self.passthrough));
        args
    }
}

/// 去掉用户透传里属于壳的 `--listen` / `--static-dir`（连同它的值）。
///
/// 只认 `--flag 值` 与 `--flag=值` 两种写法；值一律跟在后一个位置，
/// 所以遇到保留项就跳过它和它的下一个位置。
fn drop_reserved(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        let name = arg.split('=').next().unwrap_or(arg);
        if RESERVED.contains(&name) {
            // `--listen=x` 自带值，不用再跳一个。
            skip_value = !arg.contains('=');
            continue;
        }
        kept.push(arg.clone());
    }
    kept
}

/// 真的起进程：日志追加到一个文件，Windows 上不要弹黑框。
pub struct DaemonLauncher {
    /// 要起的东西。
    pub spec: DaemonSpec,
}

impl Launcher for DaemonLauncher {
    fn spawn(&mut self) -> std::io::Result<Box<dyn Sidecar>> {
        if let Some(parent) = self.spec.log.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.spec.log)?;
        let mut command = Command::new(&self.spec.program);
        command
            .args(self.spec.args())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        // 双击启动时不该先蹦一个控制台窗口。daemon 不需要控制台：
        // 日志已经进文件，界面上有指标面板。
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(Box::new(command.spawn()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(ui: Option<&str>, passthrough: &[&str]) -> DaemonSpec {
        DaemonSpec {
            program: PathBuf::from("/opt/openape/openaped"),
            address: "127.0.0.1:47711".to_owned(),
            ui_dir: ui.map(PathBuf::from),
            passthrough: passthrough.iter().map(|arg| (*arg).to_owned()).collect(),
            log: PathBuf::from("/tmp/openape.log"),
        }
    }

    #[test]
    fn listen_and_ui_are_always_first() {
        let args = spec(Some("ui/dist"), &[]).args();
        assert_eq!(
            args,
            ["--listen", "127.0.0.1:47711", "--static-dir", "ui/dist"]
        );
    }

    #[test]
    fn a_missing_ui_dir_is_simply_not_passed() {
        let args = spec(None, &[]).args();
        assert_eq!(args, ["--listen", "127.0.0.1:47711"]);
        assert!(!args.iter().any(|arg| arg == "--static-dir"));
    }

    #[test]
    fn passthrough_reaches_the_daemon() {
        let args = spec(None, &["--channel", "can0", "--xcp-udp-headered"]).args();
        assert_eq!(
            args,
            [
                "--listen",
                "127.0.0.1:47711",
                "--channel",
                "can0",
                "--xcp-udp-headered"
            ]
        );
    }

    #[test]
    fn the_shell_keeps_its_own_flags() {
        let args = spec(
            Some("ui/dist"),
            &[
                "--listen",
                "0.0.0.0:8080",
                "--static-dir=elsewhere",
                "--channel",
                "can0",
            ],
        )
        .args();
        assert_eq!(
            args,
            [
                "--listen",
                "127.0.0.1:47711",
                "--static-dir",
                "ui/dist",
                "--channel",
                "can0"
            ],
            "用户给的 --listen/--static-dir 必须被丢掉，且不带值残留"
        );
    }

    #[test]
    fn a_spawn_of_a_missing_binary_is_an_error_not_a_panic() {
        let mut launcher = DaemonLauncher {
            spec: DaemonSpec {
                program: PathBuf::from("definitely-not-a-real-openaped"),
                ..spec(None, &[])
            },
        };
        assert!(launcher.spawn().is_err());
    }

    #[test]
    fn the_log_directory_is_created_before_spawning() {
        let dir = std::env::temp_dir().join(format!("openape-shell-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut launcher = DaemonLauncher {
            spec: DaemonSpec {
                program: PathBuf::from("definitely-not-a-real-openaped"),
                log: dir.join("nested").join("daemon.log"),
                ..spec(None, &[])
            },
        };
        assert!(launcher.spawn().is_err(), "二进制不存在");
        assert!(
            dir.join("nested").is_dir(),
            "目录要先建出来，否则日志写不了"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
