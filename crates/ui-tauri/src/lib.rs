//! 桌面外壳的内部件（L5，不对外承诺兼容，见 D-20 / D-21）。
//!
//! 分两层：`supervisor` 只管 sidecar 的起停与健康探测，**不碰 Tauri**，所以在
//! Linux 上也能编译和测试；`main.rs` 里 Windows 那一支才是 WebView2 与窗口。

pub mod launch;
pub mod page;
pub mod paths;
pub mod supervisor;
