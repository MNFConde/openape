//! 桌面外壳入口（仅 Windows，见 D-21）。
//!
//! 分工：`supervisor` 管 daemon 的起停与健康，`launch` 拼参数，`paths` 找
//! 文件，`page` 画报错页。这里只做把它们串起来与窗口那点事。

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    desktop::run();

    #[cfg(not(windows))]
    {
        eprintln!("桌面外壳只在 Windows 上构建（D-21）；其它平台用浏览器打开 daemon。");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod desktop {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use openape_ui_tauri::launch::{DaemonLauncher, DaemonSpec};
    use openape_ui_tauri::page::error_page;
    use openape_ui_tauri::paths::{daemon_file_name, find_daemon, find_ui_dir};
    use openape_ui_tauri::supervisor::{Beat, Policy, Supervisor, free_port, wait_healthy};
    use tauri::Manager;

    /// 和 `openaped` 的默认端口一致；被占就顺延。
    const PREFERRED_PORT: u16 = 47711;
    /// 顺延几个端口。够躲开"上次没退干净的 daemon"，不够躲开"整段端口被占"。
    const PORT_SPAN: u16 = 8;
    /// 一次启动最多等多久见到 `/healthz`。
    const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
    /// 心跳间隔：崩了最迟这么久被发现。
    const TICK: Duration = Duration::from_millis(300);
    /// 关窗时等 sidecar 收尸的上限。
    const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

    /// 关窗握手：壳喊停，看护线程确认已收尸，然后才让进程走。
    #[derive(Default)]
    struct Handshake {
        cancel: AtomicBool,
        done: Mutex<bool>,
        cond: Condvar,
    }

    impl Handshake {
        fn cancel(&self) {
            self.cancel.store(true, Ordering::SeqCst);
            let mut done = self.done.lock().expect("握手锁没被毒化");
            let deadline = Instant::now() + SHUTDOWN_WAIT;
            while !*done {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    log("关窗握手超时：sidecar 可能还没退干净");
                    return;
                }
                done = self
                    .cond
                    .wait_timeout(done, left)
                    .expect("握手锁没被毒化")
                    .0;
            }
        }

        fn finish(&self) {
            *self.done.lock().expect("握手锁没被毒化") = true;
            self.cond.notify_all();
        }

        fn stopping(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
    }

    pub fn run() {
        let handshake = Arc::new(Handshake::default());
        let worker_handshake = Arc::clone(&handshake);

        let app = tauri::Builder::default()
            .setup(move |app| {
                let window = app
                    .get_webview_window("main")
                    .ok_or("tauri.conf.json 里没有 label 为 main 的窗口")?;
                std::thread::spawn(move || supervise(window, worker_handshake));
                Ok(())
            })
            .build(tauri::generate_context!())
            .expect("桌面外壳建不起来");

        app.run(move |_app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                // 幂等：两个事件都会来，第二次直接看到 done 就返回。
                handshake.cancel();
            }
        });
    }

    /// 想用的端口：`OPENAPE_PORT` 能改，默认跟 `openaped` 一致。
    ///
    /// 有这个口子是因为"端口被别的软件占了"真的会发生，而且验桌面壳时
    /// 也需要让壳指向另一个端口上的 daemon。
    fn preferred_port() -> u16 {
        std::env::var("OPENAPE_PORT")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(PREFERRED_PORT)
    }

    /// 看护线程：起 daemon、等它健康、把窗口指向它、它崩了就重起。
    fn supervise(window: tauri::WebviewWindow, handshake: Arc<Handshake>) {
        // 先看有没有现成的。用户可能自己开着一个 daemon（比如在终端里跑采集），
        // 再起第二个只会撞设备锁，所以直接用它。
        let port = preferred_port();
        let preferred = format!("127.0.0.1:{port}");
        if wait_healthy(
            &preferred,
            Duration::from_millis(600),
            Duration::from_millis(100),
        )
        .is_ok()
        {
            log(&format!("{preferred} 上已经有 daemon，直接用它"));
            navigate(&window, &format!("http://{preferred}/"));
            handshake.finish();
            return;
        }

        let (program, ui_dir, log_path) = match resolve() {
            Ok(resolved) => resolved,
            Err(err) => {
                log(&err);
                show_error(&window, &err);
                handshake.finish();
                return;
            }
        };

        let Some(port) = free_port(port, PORT_SPAN) else {
            let err = format!("{port} 起连续 {PORT_SPAN} 个端口都被占");
            log(&err);
            show_error(&window, &err);
            handshake.finish();
            return;
        };
        let address = format!("127.0.0.1:{port}");
        log(&format!(
            "起 {} --listen {address}{}",
            program.display(),
            ui_dir
                .as_ref()
                .map(|dir| format!(" --static-dir {}", dir.display()))
                .unwrap_or_default()
        ));

        let spec = DaemonSpec {
            program,
            address: address.clone(),
            ui_dir,
            passthrough: std::env::args().skip(1).collect(),
            log: log_path.clone(),
        };
        let mut supervisor = Supervisor::new(DaemonLauncher { spec }, Policy::default());
        if let Err(err) = supervisor.start() {
            log(&format!("daemon 起不动：{err}（会按期重试）"));
        }

        let mut healthy = false;
        let mut last_error = String::from("daemon 还没起来");
        loop {
            if handshake.stopping() {
                supervisor.stop();
                log("按关窗请求收掉 sidecar");
                handshake.finish();
                return;
            }
            match supervisor.tick(Instant::now()) {
                Beat::Stopped => {
                    handshake.finish();
                    return;
                }
                Beat::Restarted => {
                    log(&format!(
                        "sidecar 崩了，第 {} 次重起",
                        supervisor.restarts()
                    ));
                    healthy = false;
                }
                Beat::GaveUp => {
                    let err = format!(
                        "daemon 连续崩了 {} 次，放弃：{last_error}",
                        supervisor.restarts()
                    );
                    log(&err);
                    show_error(&window, &err);
                    supervisor.stop();
                    handshake.finish();
                    return;
                }
                Beat::Healthy | Beat::Waiting => {}
            }
            if !healthy {
                let waiting = Instant::now();
                match wait_healthy(&address, HEALTH_TIMEOUT, Duration::from_millis(150)) {
                    Ok(()) => {
                        healthy = true;
                        log(&format!(
                            "{address} 健康（等了 {} ms），窗口指向它",
                            waiting.elapsed().as_millis()
                        ));
                        // 每次（重）起后都导航：daemon 换了进程，页面上的流已经断了，
                        // 留在旧页面上只会看到不动的曲线。
                        navigate(&window, &format!("http://{address}/"));
                    }
                    Err(err) => {
                        log(&format!("健康检查失败（{err}）"));
                        last_error = err;
                    }
                }
            }
            std::thread::sleep(TICK);
        }
    }

    /// 找 daemon 与前端产物；两者都没找到时给出能照做的错误。
    fn resolve() -> Result<(PathBuf, Option<PathBuf>, PathBuf), String> {
        let exe = std::env::current_exe().map_err(|err| format!("读自己的路径失败：{err}"))?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| String::from("可执行文件没有父目录"))?;
        let program = match std::env::var_os("OPENAPE_DAEMON") {
            Some(value) => PathBuf::from(value),
            None => find_daemon(exe_dir).ok_or_else(|| {
                format!(
                    "找不到 {}：把它放在 {} 旁边，或用 OPENAPE_DAEMON 指定",
                    daemon_file_name(),
                    exe_dir.display()
                )
            })?,
        };
        let ui_dir = std::env::var_os("OPENAPE_UI_DIR")
            .map(PathBuf::from)
            .or_else(|| find_ui_dir(exe_dir));
        if ui_dir.is_none() {
            log("没找到前端产物（ui/dist）：界面会显示 daemon 的“未构建前端”提示");
        }
        Ok((program, ui_dir, daemon_log_path()))
    }

    /// 壳自己的日志：侧车日志之外还得记"壳做了什么"。
    fn log_dir() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("openape")
            .join("logs")
    }

    fn shell_log_path() -> PathBuf {
        log_dir().join("shell.log")
    }

    /// daemon 的 stdout/stderr 落这里，报错页也指向它。
    fn daemon_log_path() -> PathBuf {
        log_dir().join("daemon.log")
    }

    fn navigate(window: &tauri::WebviewWindow, url: &str) {
        match tauri::Url::parse(url) {
            Ok(parsed) => match window.navigate(parsed) {
                Ok(()) => log(&format!("已导航到 {url}")),
                Err(err) => log(&format!("导航到 {url} 失败：{err}")),
            },
            Err(err) => log(&format!("{url} 不是合法地址：{err}")),
        }
    }

    fn show_error(window: &tauri::WebviewWindow, message: &str) {
        navigate(window, &error_page(message, &daemon_log_path()));
    }

    /// 追加一行带时间戳的日志。壳没有任何依赖，失败了也只能放弃。
    fn log(message: &str) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let path = shell_log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "[{stamp}] {message}");
        }
    }
}
