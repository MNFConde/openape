//! 日志接入：常规运行日志 + 帧级记录的运行时开关。
//!
//! 黑匣子环形缓冲与五项指标不在此处，按 M2-06 随 DAQ 上流落地。

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_subscriber::EnvFilter;

/// 运行时帧级记录开关。默认关闭——帧级日志量级与 DAQ 帧率同阶，
/// 默认打开会淹掉常规日志。
static FRAME_TRACE: AtomicBool = AtomicBool::new(false);

/// 日志初始化只做一次；后续调用是幂等的空操作。
static INIT: Once = Once::new();

/// 输出详尽程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// 只报故障。
    Quiet,
    /// 默认：常规运行日志。
    Normal,
    /// 含 DEBUG 级细节。
    Verbose,
}

impl Verbosity {
    /// 转换成 `tracing` 的过滤指令。
    pub const fn filter(self) -> &'static str {
        match self {
            Self::Quiet => "error",
            Self::Normal => "info",
            Self::Verbose => "debug",
        }
    }
}

/// 初始化日志。多次调用只有第一次生效，返回是否由本次调用完成初始化。
pub fn init(verbosity: Verbosity) -> bool {
    let mut installed = false;
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(verbosity.filter()));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
        installed = true;
    });
    installed
}

/// 打开或关闭帧级记录。切换立即生效，不需要重启进程。
pub fn set_frame_trace(on: bool) {
    FRAME_TRACE.store(on, Ordering::Relaxed);
}

/// 当前是否记录帧级日志。
pub fn frame_trace() -> bool {
    FRAME_TRACE.load(Ordering::Relaxed)
}

/// 记录一帧。关闭时零成本返回。
pub fn trace_frame(direction: Direction, channel: &str, id: u32, data: &[u8]) {
    if !frame_trace() {
        return;
    }
    let hex = HexBytes(data);
    tracing::trace!(target: "openape::frame", channel, id = format_args!("{id:#05x}"), dir = direction.as_str(), data = %hex);
}

/// 帧方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 本机发出。
    Tx,
    /// 本机收到。
    Rx,
}

impl Direction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Tx => "tx",
            Self::Rx => "rx",
        }
    }
}

/// 把字节写成 `aa bb cc` 形式，避免日志里出现 `[170, 187]` 这种读不出帧格式的形态。
struct HexBytes<'a>(&'a [u8]);

impl std::fmt::Display for HexBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_trace_toggles_at_runtime() {
        let before = frame_trace();
        set_frame_trace(!before);
        assert_eq!(frame_trace(), !before, "切换应即时生效");
        set_frame_trace(false);
        assert!(!frame_trace(), "默认关闭");
    }

    #[test]
    fn trace_frame_is_inert_when_disabled() {
        set_frame_trace(false);
        // 关闭时不应 panic、不应分配；这里只验证调用路径可走通。
        trace_frame(Direction::Tx, "vcan0", 0x201, &[0xff, 0xff, 0x00]);
    }

    #[test]
    fn hex_bytes_render_as_frame_trace_column() {
        assert_eq!(HexBytes(&[0xaa, 0x00, 0xff]).to_string(), "aa 00 ff");
        assert_eq!(HexBytes(&[]).to_string(), "", "空载荷不该留多余空格");
        assert_eq!(HexBytes(&[0x05]).to_string(), "05", "单字节不补前导空格");
    }

    #[test]
    fn direction_labels_are_stable() {
        assert_eq!(Direction::Tx.as_str(), "tx");
        assert_eq!(Direction::Rx.as_str(), "rx");
    }

    #[test]
    fn verbosity_maps_to_filter_directives() {
        assert_eq!(Verbosity::Quiet.filter(), "error");
        assert_eq!(Verbosity::Normal.filter(), "info");
        assert_eq!(Verbosity::Verbose.filter(), "debug");
    }
}
