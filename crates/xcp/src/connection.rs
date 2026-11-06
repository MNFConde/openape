//! XCP 连接抽象。这是三道屏蔽墙的第二道：上层只认本 trait，
//! 传输介质（CAN 现在、以太网后补）差异收在实现里。

use async_trait::async_trait;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Severity};

/// 建链后从站报告的会话参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectInfo {
    /// 资源位。
    pub resource: u8,
    /// 通信模式基本字节（bit0 为字节序）。
    pub comm_mode_basic: u8,
    /// 从站能接收的单帧上限。
    pub max_cto: u16,
    /// 从站能发送的单帧上限。
    pub max_dto: u16,
    /// 协议版本。CAN 上传不进去时为 `None`，不臆造默认值。
    pub protocol_version: Option<(u8, u8)>,
    /// 传输层版本。同上。
    pub transport_version: Option<(u8, u8)>,
}

/// 会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStatus {
    /// 是否已建立会话。
    pub running: bool,
    /// 保护状态字节。
    pub protection: u8,
}

/// 一条 XCP 连接。
#[async_trait]
pub trait XcpConnection: Send + Sync {
    /// 建立会话。
    async fn connect(&mut self, mode: u8) -> Result<ConnectInfo, Fault>;

    /// 结束会话。
    async fn disconnect(&mut self) -> Result<(), Fault>;

    /// 查会话状态。
    async fn status(&mut self) -> Result<SessionStatus, Fault>;

    /// 读一段内存。按地址短读，不动传输地址。
    async fn read(&mut self, address: Address, len: ByteLen) -> Result<Vec<u8>, Fault>;

    /// 配置并启动 DAQ，返回把 DTO 切回信号的布局。
    ///
    /// 走完 FREE_DAQ → 分配 → 写条目 → 设模式 → 启停，成功后从站开始按事件
    /// 通道周期发帧。失败时从站可能停在半配置状态，调用方应重新配置而不是
    /// 接着用。
    async fn configure_daq(
        &mut self,
        lists: &[crate::daq::DaqListSpec],
    ) -> Result<crate::daq::DaqLayout, Fault>;
}

/// XCP 域的故障序号。集中在此，避免各处散落魔法数字。
pub mod seq {
    /// 等待从站响应超时。
    pub const TIMEOUT: u32 = 1;
    /// 响应与请求对不上。
    pub const PROTOCOL: u32 = 2;
    /// 从站返回错误码。
    pub const SLAVE_ERROR: u32 = 3;
    /// 帧编解码失败。
    pub const CODEC: u32 = 4;
    /// 会话尚未建立。
    pub const NOT_CONNECTED: u32 = 5;
    /// 底层总线出错。
    pub const BUS: u32 = 6;
    /// 读长度超出单帧能力。
    pub const TOO_LONG: u32 = 7;
    /// DAQ 配置被拒或从站不支持。
    pub const DAQ: u32 = 8;
}

/// 构造一条 XCP 域故障。序号固定在 [`seq`] 的常量里，构造不会失败。
pub(crate) fn fault(sequence: u32, severity: Severity, message: impl Into<String>) -> Fault {
    let code = FaultCode::new(Domain::Xcp, sequence).expect("序号常量在 1..=999 内");
    Fault::new(code, severity, message)
}
