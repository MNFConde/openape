//! sidecar 的生命周期：起、停、崩了重起、等它健康。
//!
//! 刻意不碰 Tauri：M5-13 的验收里"sidecar 生命周期与崩溃重启正确"是逻辑，
//! 放在这里就能在 Linux 上跑测试（CI 的 `cargo test --workspace` 就靠这个），
//! Windows 那支壳只剩"把窗口指向哪个地址"。

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// 子进程退出码。不直接用 [`std::process::ExitStatus`]——它的构造在各平台
/// 不一致，而这里只需要"退出了没有、退出码是几"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// 退出码；被信号杀掉时为 `None`。
    pub code: Option<i32>,
}

/// 子进程里我们真正用到的两件事。
pub trait Sidecar: Send {
    /// 没退出返回 `Ok(None)`。
    fn try_wait(&mut self) -> io::Result<Option<Exit>>;
    /// 结束它。已经退出的进程上调用不算错。
    fn kill(&mut self) -> io::Result<()>;
}

impl Sidecar for std::process::Child {
    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        std::process::Child::try_wait(self).map(|status| {
            status.map(|status| Exit {
                code: status.code(),
            })
        })
    }

    fn kill(&mut self) -> io::Result<()> {
        std::process::Child::kill(self)
    }
}

/// 怎么起一个 sidecar。抽成 trait 是为了让崩溃重起能在测试里按剧本演。
pub trait Launcher: Send {
    /// 起一个进程。失败照实返回，重试由 [`Supervisor`] 负责。
    fn spawn(&mut self) -> io::Result<Box<dyn Sidecar>>;
}

/// 崩了之后的重起预算。
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// 最多重起几次。用完就放弃，把错误交给用户看。
    pub max_restarts: u8,
    /// 两次重起之间至少隔多久。连着崩时给系统一点时间，别打转。
    pub backoff: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            backoff: Duration::from_millis(500),
        }
    }
}

/// 一次心跳的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    /// 子进程还在跑。
    Healthy,
    /// 崩了，退避期没过，暂时不起。
    Waiting,
    /// 刚重起过。
    Restarted,
    /// 重起预算用完，放弃。
    GaveUp,
    /// 已经喊停，不再管。
    Stopped,
}

/// sidecar 的看护者。每次心跳调一次 [`Supervisor::tick`]。
pub struct Supervisor<L: Launcher> {
    launcher: L,
    policy: Policy,
    child: Option<Box<dyn Sidecar>>,
    restarts: u8,
    next_start: Option<Instant>,
    stopping: bool,
}

impl<L: Launcher> Supervisor<L> {
    /// 建一个还没起进程的看护者。
    pub fn new(launcher: L, policy: Policy) -> Self {
        Self {
            launcher,
            policy,
            child: None,
            restarts: 0,
            next_start: None,
            stopping: false,
        }
    }

    /// 起第一个进程。失败也记一次"崩"：后面的 `tick` 会按预算重试，
    /// 调用方只需要把错误记进日志，不必自己写重试。
    pub fn start(&mut self) -> io::Result<()> {
        match self.launcher.spawn() {
            Ok(child) => {
                self.child = Some(child);
                Ok(())
            }
            Err(err) => {
                self.note_crash(Instant::now());
                Err(err)
            }
        }
    }

    /// 心跳一次。
    pub fn tick(&mut self, now: Instant) -> Beat {
        if self.stopping {
            return Beat::Stopped;
        }
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                // 还活着。
                Ok(None) => return Beat::Healthy,
                // 自己退了，或者问不出状态（管道断了这类）——都按崩处理。
                Ok(Some(_)) | Err(_) => self.child = None,
            }
            self.note_crash(now);
        }
        let Some(at) = self.next_start else {
            // 没有待起的进程：要么从没起过（调用方漏了 `start`），要么已放弃。
            return if self.restarts == 0 {
                Beat::Waiting
            } else {
                Beat::GaveUp
            };
        };
        if now < at {
            return Beat::Waiting;
        }
        match self.launcher.spawn() {
            Ok(child) => {
                self.child = Some(child);
                self.next_start = None;
                Beat::Restarted
            }
            Err(_) => {
                // 起不来跟崩了同样处理：继续按预算退避重试。
                self.note_crash(now);
                if self.next_start.is_none() {
                    Beat::GaveUp
                } else {
                    Beat::Waiting
                }
            }
        }
    }

    /// 停：杀掉子进程并从此不再重起。
    pub fn stop(&mut self) {
        self.stopping = true;
        self.next_start = None;
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        self.child = None;
    }

    /// 已经重起过几次。
    pub fn restarts(&self) -> u8 {
        self.restarts
    }

    /// 是否已喊停。
    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    fn note_crash(&mut self, now: Instant) {
        if self.restarts >= self.policy.max_restarts {
            self.next_start = None;
            return;
        }
        self.restarts += 1;
        self.next_start = Some(now + self.policy.backoff);
    }
}

/// 在 `[start, start + span)` 里找一个能绑住的回环端口。
///
/// daemon 自己也会绑（绑不上就退出），这里的探测是"尽量别撞"，两者之间
/// 留了个很窄的窗口——撞上了会表现成 daemon 立刻退出，走重起预算，
/// 不会静默。
pub fn free_port(start: u16, span: u16) -> Option<u16> {
    (start..start.saturating_add(span)).find(|port| {
        TcpListener::bind(("127.0.0.1", *port))
            .map(|listener| {
                drop(listener);
                true
            })
            .unwrap_or(false)
    })
}

/// 一次连接最多等多久。
///
/// 本机实测：往没人听的端口 connect 时，Windows 有时要两秒才回"连接被拒"，
/// 而 `TcpStream::connect` 没有超时参数——一次探测能把整段启动预算吃光，
/// 表现成"窗口停在启动页好几秒"。所以这里自己给它一个上限。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// 探一次 `GET /healthz`：2xx 就算健康。
fn healthz_once(addr: &str) -> io::Result<bool> {
    let target: SocketAddr = addr
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{addr} 不是 IP:端口")))?;
    let mut stream = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    write!(
        stream,
        "GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )?;
    let mut head = String::new();
    // 只读状态行，够判断了；body 多大都不关这里的事。
    let mut buf = [0u8; 64];
    let read = stream.read(&mut buf)?;
    head.push_str(&String::from_utf8_lossy(&buf[..read]));
    Ok(head.starts_with("HTTP/1.") && head.contains(" 2"))
}

/// 轮询到健康为止。超时返回最后一次失败的原因，调用方原样记日志。
pub fn wait_healthy(addr: &str, timeout: Duration, pause: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut last;
    loop {
        match healthz_once(addr) {
            Ok(true) => return Ok(()),
            Ok(false) => last = String::from("健康检查没有返回 2xx"),
            Err(err) => last = err.to_string(),
        }
        if Instant::now() >= deadline {
            return Err(format!("{addr} 在 {timeout:?} 内没起来：{last}"));
        }
        std::thread::sleep(pause);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::net::TcpListener as StdListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// 剧本里的子进程：`exit` 为 `Some` 表示下一跳就报退出。
    struct FakeChild {
        exit: Option<Exit>,
        killed: Arc<AtomicUsize>,
    }

    impl Sidecar for FakeChild {
        fn try_wait(&mut self) -> io::Result<Option<Exit>> {
            Ok(self.exit.take())
        }

        fn kill(&mut self) -> io::Result<()> {
            self.killed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// 按剧本依次产出子进程：`None` = 一直活着，`Some(code)` = 立刻退出。
    struct FakeLauncher {
        script: VecDeque<Option<i32>>,
        spawned: usize,
        fail: bool,
        killed: Arc<AtomicUsize>,
    }

    impl FakeLauncher {
        fn new(script: Vec<Option<i32>>) -> Self {
            Self {
                script: script.into(),
                spawned: 0,
                fail: false,
                killed: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn always_dying() -> Self {
            let mut this = Self::new(Vec::new());
            this.fail = true;
            this
        }
    }

    impl Launcher for FakeLauncher {
        fn spawn(&mut self) -> io::Result<Box<dyn Sidecar>> {
            if self.fail {
                return Err(io::Error::other("起不来"));
            }
            self.spawned += 1;
            let exit = self
                .script
                .pop_front()
                .flatten()
                .map(|code| Exit { code: Some(code) });
            Ok(Box::new(FakeChild {
                exit,
                killed: Arc::clone(&self.killed),
            }))
        }
    }

    fn policy() -> Policy {
        Policy {
            max_restarts: 2,
            backoff: Duration::from_millis(100),
        }
    }

    #[test]
    fn an_alive_sidecar_stays_healthy() {
        let mut sup = Supervisor::new(FakeLauncher::new(vec![None]), policy());
        sup.start().expect("能起");
        let now = Instant::now();
        assert_eq!(sup.tick(now), Beat::Healthy);
        assert_eq!(sup.tick(now), Beat::Healthy);
        assert_eq!(sup.restarts(), 0);
    }

    #[test]
    fn a_crash_waits_for_the_backoff_then_restarts() {
        let mut sup = Supervisor::new(FakeLauncher::new(vec![Some(1), None]), policy());
        sup.start().expect("能起");
        let now = Instant::now();

        assert_eq!(sup.tick(now), Beat::Waiting, "崩了先进退避");
        assert_eq!(sup.restarts(), 1);
        assert_eq!(
            sup.tick(now + Duration::from_millis(50)),
            Beat::Waiting,
            "退避没过不重起"
        );
        assert_eq!(
            sup.tick(now + Duration::from_millis(150)),
            Beat::Restarted,
            "过点就重起"
        );
        assert_eq!(sup.tick(now + Duration::from_millis(150)), Beat::Healthy);
    }

    #[test]
    fn repeated_crashes_give_up_after_the_budget() {
        let mut sup = Supervisor::new(FakeLauncher::new(vec![Some(1), Some(1), Some(1)]), policy());
        sup.start().expect("能起");
        let mut now = Instant::now();
        // 每一次"崩"要两跳：先发现退出进退避，过点再重起。
        assert_eq!(sup.tick(now), Beat::Waiting, "发现第一次崩");
        now += Duration::from_millis(150);
        assert_eq!(sup.tick(now), Beat::Restarted, "第一次重起");
        now += Duration::from_millis(150);
        assert_eq!(sup.tick(now), Beat::Waiting, "发现第二次崩");
        assert_eq!(sup.restarts(), 2);
        now += Duration::from_millis(150);
        assert_eq!(sup.tick(now), Beat::Restarted, "第二次重起");
        now += Duration::from_millis(150);
        assert_eq!(sup.tick(now), Beat::GaveUp, "预算用完就放弃");
        // 放弃之后不再重起。
        now += Duration::from_millis(150);
        assert_eq!(sup.tick(now), Beat::GaveUp);
    }

    #[test]
    fn stop_kills_and_never_respawns() {
        let mut sup = Supervisor::new(FakeLauncher::new(vec![None]), policy());
        sup.start().expect("能起");
        sup.stop();
        assert!(sup.is_stopping());
        let now = Instant::now();
        assert_eq!(sup.tick(now), Beat::Stopped);
        assert_eq!(sup.tick(now + Duration::from_secs(1)), Beat::Stopped);
    }

    #[test]
    fn a_failed_launch_is_retried_like_a_crash() {
        let mut sup = Supervisor::new(FakeLauncher::always_dying(), policy());
        assert!(sup.start().is_err(), "起不来要能把错误交出去");
        let now = Instant::now();
        assert_eq!(sup.tick(now), Beat::Waiting);
        assert_eq!(
            sup.tick(now + Duration::from_millis(150)),
            Beat::Waiting,
            "还是起不来就继续退避"
        );
        assert_eq!(sup.restarts(), 2, "两次都记进预算");
        assert_eq!(
            sup.tick(now + Duration::from_secs(1)),
            Beat::GaveUp,
            "预算用完放弃"
        );
    }

    /// 假的 daemon：一直应答到被喊停。
    ///
    /// 一开始写成"只接一次"，并行跑测试时撞了一次超时——探测第一次连接
    /// 被 accept 接住但没来得及写回，客户端已超时放弃，那个单次线程就此
    /// 退出，后面的重试全打空。假件也要像真件一样能接受重试。
    struct StubDaemon {
        addr: String,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl StubDaemon {
        fn start(status: &'static str) -> Self {
            let listener = StdListener::bind(("127.0.0.1", 0)).expect("能绑");
            let addr = listener.local_addr().expect("有地址").to_string();
            listener.set_nonblocking(true).expect("能设非阻塞");
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                let body = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut buf = [0u8; 512];
                            let _ = stream.read(&mut buf);
                            let _ = stream.write_all(body.as_bytes());
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
            Self {
                addr,
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for StubDaemon {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[test]
    fn free_port_skips_a_port_already_taken() {
        let held = StdListener::bind(("127.0.0.1", 0)).expect("能绑");
        let port = held.local_addr().expect("有地址").port();
        assert_ne!(free_port(port, 1), Some(port), "被占的端口不能给出去");
        drop(held);
        assert_eq!(free_port(port, 1), Some(port), "放开后同一个端口可用");
    }

    #[test]
    fn healthz_needs_a_2xx() {
        for (status, want) in [("200 OK", true), ("500 Internal Server Error", false)] {
            let daemon = StubDaemon::start(status);
            assert_eq!(healthz_once(&daemon.addr).ok(), Some(want), "{status}");
        }
    }

    #[test]
    fn wait_healthy_times_out_when_nobody_listens() {
        // 绑了再放开：端口上没人，连接会被拒。
        let addr = {
            let listener = StdListener::bind(("127.0.0.1", 0)).expect("能绑");
            listener.local_addr().expect("有地址")
        };
        let started = Instant::now();
        let err = wait_healthy(
            &addr.to_string(),
            Duration::from_millis(300),
            Duration::from_millis(50),
        )
        .expect_err("没人听就该超时");
        assert!(err.contains(&addr.to_string()), "错误里要带地址：{err}");
        // connect 自己不带超时，Windows 上"被拒"也可能拖两秒。单次探测必须有上限，
        // 否则窗口会停在启动页好几秒（本机实测踩过）。
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "单次探测耗时 {:?}，超时预算没被执行",
            started.elapsed()
        );
    }

    #[test]
    fn an_address_that_is_not_ip_and_port_is_rejected_at_once() {
        assert!(healthz_once("localhost").is_err(), "只认字面 IP:端口");
        assert!(healthz_once("127.0.0.1").is_err());
    }

    #[test]
    fn wait_healthy_returns_once_the_daemon_answers() {
        let daemon = StubDaemon::start("200 OK");
        wait_healthy(
            &daemon.addr,
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .expect("应当健康");
    }
}
