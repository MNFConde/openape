//! 构建脚本：Windows 上生成 Tauri 的资源与配置上下文。
fn main() {
    // 只有 Windows 才编 Tauri 的壳；其它平台的构建只为让 Linux CI 能跑
    // supervisor 的单测，不需要图标、配置与资源脚本。
    #[cfg(windows)]
    tauri_build::build();
}
