//! 进程级设备锁。
//!
//! CAN 硬件通常不允许两个进程同时打开同一条通道，CLI 与 daemon 若各开一次
//! 就会互相打死（D-18）。锁文件按（后端, 通道）落在系统临时目录，占用者
//! 写自己的 PID；持有者崩溃留下的陈旧锁按 PID 是否存活判定并回收。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use openape_diag::{Domain, Fault, FaultCode, Severity};

/// 一把设备锁。丢弃即释放。
#[derive(Debug)]
pub struct DeviceLock {
    path: PathBuf,
}

impl DeviceLock {
    /// 占用（后端, 通道）。已被活进程占用时返回 [`Domain::Session`] 故障。
    pub fn acquire(backend: &str, channel: &str) -> Result<Self, Fault> {
        let dir = lock_dir();
        fs::create_dir_all(&dir).map_err(|err| io_fault(&dir, err))?;
        let path = dir.join(file_name(backend, channel));

        // 最多两轮：第一轮撞到陈旧锁就地清掉，第二轮再建。
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(holder) = live_holder(&path) {
                        return Err(Fault::new(
                            FaultCode::new(Domain::Session, 1).expect("序号 1 合法"),
                            Severity::Error,
                            format!("{channel} 已被进程 {holder} 占用"),
                        )
                        .with_context("channel", channel.to_owned())
                        .with_context("backend", backend.to_owned())
                        .with_hint("停掉占用者，或用 `--via-daemon` 复用它的会话"));
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(err) => return Err(io_fault(&path, err)),
            }
        }
        Err(Fault::new(
            FaultCode::new(Domain::Session, 1).expect("序号 1 合法"),
            Severity::Error,
            format!("{channel} 的锁竞争两轮仍未拿到"),
        )
        .with_hint("检查锁目录权限"))
    }

    /// 锁文件路径，便于日志与排障。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DeviceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_dir() -> PathBuf {
    std::env::temp_dir().join("openape-locks")
}

fn file_name(backend: &str, channel: &str) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    format!("{}-{}.lock", sanitize(backend), sanitize(channel))
}

/// 读出锁文件里的 PID，并判断该进程是否还活着。
fn live_holder(path: &Path) -> Option<u32> {
    let content = fs::read_to_string(path).ok()?;
    let pid: u32 = content.trim().parse().ok()?;
    Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
}

fn io_fault(path: &Path, err: std::io::Error) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Session, 2).expect("序号 2 合法"),
        Severity::Error,
        format!("锁文件 {} 操作失败：{err}", path.display()),
    )
    .with_hint("检查临时目录是否可写")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_channel(tag: &str) -> String {
        format!("t{}{}", std::process::id(), tag)
    }

    #[test]
    fn second_acquire_on_same_channel_is_refused() {
        let channel = unique_channel("a");
        let first = DeviceLock::acquire("socketcan", &channel).expect("首次能拿到");
        let err = DeviceLock::acquire("socketcan", &channel).expect_err("第二次应被拒");
        assert_eq!(err.code().domain(), Domain::Session);
        assert!(err.message().contains(&channel), "故障要说明是哪个通道");
        drop(first);
    }

    #[test]
    fn dropping_the_lock_releases_the_channel() {
        let channel = unique_channel("b");
        {
            let _lock = DeviceLock::acquire("socketcan", &channel).expect("首次能拿到");
        }
        DeviceLock::acquire("socketcan", &channel).expect("释放后能再拿");
    }

    #[test]
    fn stale_lock_from_dead_process_is_reclaimed() {
        let channel = unique_channel("c");
        let dir = lock_dir();
        fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join(file_name("socketcan", &channel));
        // PID 1 之外的极端值：/proc/<max> 不存在，视为崩溃残留。
        fs::write(&path, u32::MAX.to_string()).expect("写陈旧锁");
        let lock = DeviceLock::acquire("socketcan", &channel).expect("应接管陈旧锁");
        assert_eq!(lock.path(), path);
    }

    #[test]
    fn different_channels_do_not_collide() {
        let a = unique_channel("d");
        let b = unique_channel("e");
        let _first = DeviceLock::acquire("socketcan", &a).expect("能拿");
        DeviceLock::acquire("socketcan", &b).expect("不同通道互不影响");
    }
}
