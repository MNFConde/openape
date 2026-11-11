//! 测试夹具：临时 vcan 通道。
//!
//! 三个 crate 的集成测试都要"建一条用完即删的 vcan"，重复实现不仅啰嗦，
//! 还各自踩同一个并发坑：多个测试进程同时 `ip link add` 时 netlink 会短暂
//! 返回 EBUSY。这里统一重试，避免覆盖率统计因偶发失败而误报 0%。

use std::process::Command;
use std::time::Duration;

/// 经典 CAN 接口的 MTU，也是经典帧的身位上限。
pub const CAN_MTU: u32 = 16;
/// CAN-FD 接口的 MTU。内核在发送路径上按 `dev->mtu >= CANFD_MTU`
/// 允许 FD 帧，后端的能力探测用同一条判据。
pub const CANFD_MTU: u32 = crate::CANFD_MTU;

/// 重试退避：等内核 rtnl 把上一轮的链路操作结算完（并发时 netlink 先回 EBUSY）。
const RETRY_BACKOFF: Duration = Duration::from_millis(50);

/// `ip link show` 的有界确认窗口：add 报错时设备可能还在建。
const RTNL_CONFIRM: Duration = Duration::from_millis(300);

/// 给定窗口内 `ip link show <name>` 是否看到该链路。
///
/// 用它替代"失败后固定等一会儿再猜"：确认窗口到了仍看不到才算真失败。
fn link_exists(name: &str, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    loop {
        let shown = Command::new("ip").args(["link", "show", name]).output();
        if shown.is_ok_and(|out| out.status.success()) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(RETRY_BACKOFF);
    }
}

/// 一个用完即删的 vcan 通道。丢弃时自动 `ip link del`。
#[derive(Debug)]
pub struct VcanGuard {
    name: String,
}

impl VcanGuard {
    /// 建一条以标签区分的 vcan。需要 root 与 vcan 模块。
    ///
    /// 建不起来时返回 `None` **并打印原因**——跳过必须留痕，否则"全绿"会
    /// 掩盖没测。
    pub fn create(tag: &str) -> Option<Self> {
        Self::create_with_mtu(tag, None)
    }

    /// 建一条 MTU 指定为 72 的 vcan：内核据此允许收发 FD 帧。
    pub fn create_fd(tag: &str) -> Option<Self> {
        Self::create_with_mtu(tag, Some(CANFD_MTU))
    }

    /// 建一条 MTU 指定为 16 的 vcan：只有经典帧身位。
    pub fn create_classic_only(tag: &str) -> Option<Self> {
        Self::create_with_mtu(tag, Some(CAN_MTU))
    }

    fn create_with_mtu(tag: &str, mtu: Option<u32>) -> Option<Self> {
        let name = format!("v{}{}", std::process::id() % 10_000, tag);
        let mut last_error = String::new();

        // 并发建链路时 netlink 会短暂 EBUSY，且覆盖率工具并行跑多个 crate
        // 时竞争更激烈，退避到 500 ms 仍失败才算真跳过。
        for _ in 0..10 {
            let added = Command::new("ip")
                .args(["link", "add", &name, "type", "vcan"])
                .output()
                .ok()?;
            let exists = if added.status.success() {
                true
            } else {
                last_error = String::from_utf8_lossy(&added.stderr).trim().to_owned();
                // add 报了错、设备却可能正在建：有界确认一次再判失败，
                // 比固定等待更准（并发下 netlink 会先回 EBUSY）。
                link_exists(&name, RTNL_CONFIRM)
            };
            if exists {
                // MTU 必须在 up 之前设：链路已 up 时 vcan 的 change_mtu
                // 回 EBUSY，表现为"建链路失败"。
                if let Some(mtu) = mtu {
                    let set_mtu = Command::new("ip")
                        .args(["link", "set", &name, "mtu", &mtu.to_string()])
                        .output()
                        .ok()?;
                    if !set_mtu.status.success() {
                        last_error = String::from_utf8_lossy(&set_mtu.stderr).trim().to_owned();
                        let _ = Command::new("ip").args(["link", "del", &name]).output();
                        std::thread::sleep(RETRY_BACKOFF);
                        continue;
                    }
                }
                let up = Command::new("ip")
                    .args(["link", "set", &name, "up"])
                    .output()
                    .ok()?;
                if up.status.success() {
                    return Some(Self { name });
                }
                last_error = String::from_utf8_lossy(&up.stderr).trim().to_owned();
                let _ = Command::new("ip").args(["link", "del", &name]).output();
            }
            std::thread::sleep(RETRY_BACKOFF);
        }

        eprintln!("跳过：建 vcan {name} 失败（{last_error}）");
        None
    }

    /// 通道名。
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for VcanGuard {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.name])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_creates_and_removes_the_channel() {
        let Some(guard) = VcanGuard::create("u") else {
            return;
        };
        let name = guard.name().to_owned();
        assert!(
            crate::available_channels().contains(&name),
            "{name} 应在通道列表里"
        );
        drop(guard);
        assert!(
            !crate::available_channels().contains(&name),
            "丢弃后通道应被删除"
        );
    }
}
