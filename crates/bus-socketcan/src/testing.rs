//! 测试夹具：临时 vcan 通道。
//!
//! 三个 crate 的集成测试都要"建一条用完即删的 vcan"，重复实现不仅啰嗦，
//! 还各自踩同一个并发坑：多个测试进程同时 `ip link add` 时 netlink 会短暂
//! 返回 EBUSY。这里统一重试，避免覆盖率统计因偶发失败而误报 0%。

use std::process::Command;
use std::time::Duration;

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
        let name = format!("v{}{}", std::process::id() % 10_000, tag);
        let mut last_error = String::new();

        // 并发建链路时 netlink 会短暂 EBUSY，且覆盖率工具并行跑多个 crate
        // 时竞争更激烈，退避到 500 ms 仍失败才算真跳过。
        for _ in 0..10 {
            let add = Command::new("ip")
                .args(["link", "add", &name, "type", "vcan"])
                .output()
                .ok()?;
            if add.status.success() {
                let up = Command::new("ip")
                    .args(["link", "set", &name, "up"])
                    .output()
                    .ok()?;
                if up.status.success() {
                    return Some(Self { name });
                }
                last_error = String::from_utf8_lossy(&up.stderr).trim().to_owned();
                let _ = Command::new("ip").args(["link", "del", &name]).output();
            } else {
                last_error = String::from_utf8_lossy(&add.stderr).trim().to_owned();
            }
            std::thread::sleep(Duration::from_millis(50));
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
