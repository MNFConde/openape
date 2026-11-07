//! 会话域的故障序号。集中在此，避免各处散落魔法数字。
//!
//! 序号是故障分类的载体：CLI 的退出码与界面面板都按（域, 序号）分辨"锁被
//! 占用"与"写入长度不符"，随手加一个新数字等于给上层加一个未登记类别。

/// 设备锁被占用或竞争失败。
pub const LOCK: u32 = 1;
/// 锁文件读写失败。
pub const LOCK_IO: u32 = 2;
/// 发帧失败。
pub const SEND: u32 = 3;
/// 本地执行（而非经 daemon）无法进行。
pub const LOCAL: u32 = 4;
/// 写入长度与目标变量不符。
pub const WRITE_LEN: u32 = 10;
/// 快照落盘失败。
pub const SNAPSHOT: u32 = 11;
/// 审计日志读写失败。
pub const AUDIT: u32 = 12;
/// 回滚失败。
pub const RESTORE: u32 = 13;
/// 标定页操作未生效。
pub const PAGE: u32 = 14;
/// 写入值超出 A2L 量程。
pub const RANGE: u32 = 15;
/// 触发录制未命中或未收齐。
pub const TRIGGER: u32 = 16;
