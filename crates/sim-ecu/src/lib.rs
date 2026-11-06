//! XCP 从站模拟器。
//!
//! 内核自研（[D-12](../../../docs/decisions.md#d-12-xcp-on-can-从站自研)）：上游
//! xcp-lite 没有 CAN 传输层。命令处理是纯函数，收发在外层循环里，测试因此
//! 不必碰总线。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use openape_bus::{Bus, CanFrame, CanId};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_xcp_proto::{Command, ErrorCode, PID_CMD_RES, Response, resource};

/// 虚拟内存大小（64 KiB），够放标定页与观测量。
pub const MEM_SIZE: usize = 0x10000;

/// 从站参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimEcuConfig {
    /// 命令帧标识符（从站监听）。
    pub command_id: CanId,
    /// 响应帧标识符（从站发送）。
    pub response_id: CanId,
}

impl SimEcuConfig {
    /// 与 M0-2 spike 一致的默认值。
    pub fn default_ids() -> Self {
        Self {
            command_id: CanId::standard(0x600).expect("0x600 在 11 位内"),
            response_id: CanId::standard(0x601).expect("0x601 在 11 位内"),
        }
    }
}

/// 一台虚拟 ECU。
#[derive(Debug)]
pub struct SimEcu {
    config: SimEcuConfig,
    memory: Vec<u8>,
    mta: u32,
    page: u16,
    running: bool,
}

impl SimEcu {
    /// 新建，内存清零。
    pub fn new(config: SimEcuConfig) -> Self {
        Self {
            config,
            memory: vec![0; MEM_SIZE],
            mta: 0,
            page: 0,
            running: false,
        }
    }

    /// 配置。
    pub fn config(&self) -> &SimEcuConfig {
        &self.config
    }

    /// 预置内存内容，供测试与夹具初始化。
    pub fn poke(&mut self, address: u32, data: &[u8]) {
        let start = address as usize;
        let end = start + data.len();
        if end <= self.memory.len() {
            self.memory[start..end].copy_from_slice(data);
        }
    }

    /// 读内存，测试断言用。
    pub fn peek(&self, address: u32, len: usize) -> &[u8] {
        let start = address as usize;
        &self.memory[start..start + len]
    }

    /// 处理一整帧（含 PID 与命令码），默认按标准回显命令码。
    pub fn handle(&mut self, frame: &[u8]) -> Response {
        match Command::decode(frame) {
            Err(_) => err(ErrorCode::CmdSyntax),
            Ok(command) => self.dispatch(command),
        }
    }

    /// 按命令分派到各自的处理。圈复杂度等于从站支持的命令条数，与 master
    /// 侧的编解码同属刻意保留的协议分派，不再拆分。
    fn dispatch(&mut self, command: Command) -> Response {
        let code = command.code() as u8;
        match command {
            Command::Connect { .. } => {
                self.running = true;
                // CAN 上 maxCto = 8：PID + 命令码之后只剩 6 字节参数。
                // 版本字段放不下，master 侧因此按"缺席"处理，不臆造默认值。
                ok(
                    code,
                    vec![resource::CAL_PAG | resource::DAQ, 0x00, 8, 0x00, 8, 0x00],
                )
            }
            Command::Disconnect => {
                self.running = false;
                ok(code, vec![])
            }
            Command::GetStatus => {
                let session = u8::from(self.running);
                ok(code, vec![session, 0x00, 0x00, 0x00, 0x00, 0x00])
            }
            Command::Synch => ok(code, vec![]),
            Command::GetCommModeInfo => ok(code, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
            Command::SetMta {
                address_extension,
                address,
            } => {
                if address_extension != 0 {
                    return err(ErrorCode::OutOfRange);
                }
                self.mta = address;
                ok(code, vec![])
            }
            Command::Upload { len } => {
                let Some(data) = self.read_memory(self.mta, len) else {
                    return err(ErrorCode::OutOfRange);
                };
                self.mta = self.mta.saturating_add(len as u32);
                ok(code, data)
            }
            Command::ShortUpload {
                len,
                address_extension,
                address,
            } => {
                if address_extension != 0 {
                    return err(ErrorCode::OutOfRange);
                }
                match self.read_memory(address, len) {
                    Some(data) => ok(code, data),
                    None => err(ErrorCode::OutOfRange),
                }
            }
            Command::Download { len: _, data } => {
                let start = self.mta as usize;
                if start + data.len() > self.memory.len() {
                    return err(ErrorCode::OutOfRange);
                }
                self.memory[start..start + data.len()].copy_from_slice(&data);
                self.mta = self.mta.saturating_add(data.len() as u32);
                ok(code, vec![])
            }
            Command::GetCalPage { .. } => ok(code, self.page.to_le_bytes().to_vec()),
            Command::SetCalPage { page, segment, .. } => {
                if segment != 0 || page > 1 {
                    return err(ErrorCode::PageNotValid);
                }
                self.page = page;
                ok(code, vec![])
            }
        }
    }

    fn read_memory(&self, address: u32, len: u8) -> Option<Vec<u8>> {
        let start = address as usize;
        let end = start.checked_add(len as usize)?;
        (end <= self.memory.len()).then(|| self.memory[start..end].to_vec())
    }
}

fn ok(command: u8, payload: Vec<u8>) -> Response {
    Response::Ok { command, payload }
}

fn err(code: ErrorCode) -> Response {
    Response::Err { code: code as u8 }
}

/// 空闲时从站多久轮询一次。没有命令不是错误，继续等。
const IDLE_POLL: Duration = Duration::from_millis(200);

/// 在总线上跑从站，直到任务被取消。
pub async fn serve(mut ecu: SimEcu, bus: Arc<dyn Bus>) -> Result<(), Fault> {
    let config = ecu.config.clone();
    loop {
        let frame = match bus.recv(IDLE_POLL).await {
            Ok(frame) => frame,
            Err(openape_bus::BusError::Timeout { .. }) => continue,
            Err(err) => {
                let fault = bus_fault(&err);
                tracing::error!(target: "openape::sim", %fault, "从站收帧失败，退出");
                return Err(fault);
            }
        };
        let Some(outbound) = respond(&mut ecu, &config, &frame) else {
            continue;
        };
        if let Err(err) = bus.send(outbound).await {
            let fault = bus_fault(&err);
            tracing::error!(target: "openape::sim", %fault, "从站发响应失败，退出");
            return Err(fault);
        }
    }
}

/// 给定一帧，决定要不要回、回什么。不是命令帧就返回 `None`。
///
/// 从站不处理别人的流量，也不该把命令标识符上的 DAQ DTO 当命令——那样会
/// 回环自激。
fn respond(ecu: &mut SimEcu, config: &SimEcuConfig, frame: &CanFrame) -> Option<CanFrame> {
    if frame.id != config.command_id {
        return None;
    }
    if frame.data.first().copied() != Some(PID_CMD_RES) {
        return None;
    }
    let response = ecu.handle(&frame.data);
    CanFrame::new(config.response_id, response.encode())
}

fn bus_fault(err: &openape_bus::BusError) -> Fault {
    let code = FaultCode::new(Domain::Session, 2).expect("序号 2 合法");
    Fault::new(code, Severity::Error, format!("从站总线出错：{err}"))
        .with_hint("确认通道仍然存在（`probe bus`）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_xcp_proto::{CommandCode, MAX_CTO};

    fn ecu() -> SimEcu {
        SimEcu::new(SimEcuConfig::default_ids())
    }

    fn send(ecu: &mut SimEcu, command: Command) -> Response {
        let frame = command.encode().expect("测试命令都能编码");
        ecu.handle(&frame)
    }

    fn expect_ok(response: Response, command: CommandCode) -> Vec<u8> {
        match response {
            Response::Ok {
                command: echoed,
                payload,
            } => {
                assert_eq!(echoed, command as u8, "响应必须回显命令码");
                payload
            }
            other => panic!("期望正响应，实际 {other:?}"),
        }
    }

    #[test]
    fn connect_reports_resources_and_frame_limits() {
        let mut ecu = ecu();
        let payload = expect_ok(
            send(&mut ecu, Command::Connect { mode: 0 }),
            CommandCode::Connect,
        );
        assert_eq!(payload[0] & resource::DAQ, resource::DAQ);
        assert_eq!(payload[0] & resource::CAL_PAG, resource::CAL_PAG);
        assert_eq!(u16::from_le_bytes([payload[2], payload[3]]), 8, "maxCto");
        assert_eq!(u16::from_le_bytes([payload[4], payload[5]]), 8, "maxDto");
    }

    #[test]
    fn status_reflects_session_state() {
        let mut ecu = ecu();
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            0
        );
        let _ = send(&mut ecu, Command::Connect { mode: 0 });
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            1
        );
    }

    #[test]
    fn short_upload_reads_preseeded_memory() {
        let mut ecu = ecu();
        ecu.poke(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let payload = expect_ok(
            send(
                &mut ecu,
                Command::ShortUpload {
                    len: 4,
                    address_extension: 0,
                    address: 0x1000,
                },
            ),
            CommandCode::ShortUpload,
        );
        assert_eq!(payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn upload_advances_the_transfer_address() {
        let mut ecu = ecu();
        ecu.poke(0x1000, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let _ = send(
            &mut ecu,
            Command::SetMta {
                address_extension: 0,
                address: 0x1000,
            },
        );
        let first = expect_ok(
            send(&mut ecu, Command::Upload { len: 6 }),
            CommandCode::Upload,
        );
        let second = expect_ok(
            send(&mut ecu, Command::Upload { len: 2 }),
            CommandCode::Upload,
        );
        assert_eq!([first, second].concat(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn download_writes_memory_and_advances() {
        let mut ecu = ecu();
        let _ = send(
            &mut ecu,
            Command::SetMta {
                address_extension: 0,
                address: 0x2000,
            },
        );
        let _ = send(
            &mut ecu,
            Command::Download {
                len: 3,
                data: vec![0xAA, 0xBB, 0xCC],
            },
        );
        assert_eq!(ecu.peek(0x2000, 3), &[0xAA, 0xBB, 0xCC]);
        // 地址已自增到 0x2003。
        let _ = send(
            &mut ecu,
            Command::Download {
                len: 1,
                data: vec![0xDD],
            },
        );
        assert_eq!(ecu.peek(0x2003, 1), &[0xDD]);
    }

    #[test]
    fn out_of_range_read_is_refused_not_truncated() {
        let mut ecu = ecu();
        let response = send(
            &mut ecu,
            Command::ShortUpload {
                len: 4,
                address_extension: 0,
                address: (MEM_SIZE - 2) as u32,
            },
        );
        match response {
            Response::Err { code } => assert_eq!(code, ErrorCode::OutOfRange as u8),
            other => panic!("期望越界错误，实际 {other:?}"),
        }
    }

    #[test]
    fn calibration_page_switch_round_trips_and_rejects_bad_page() {
        let mut ecu = ecu();
        expect_ok(
            send(
                &mut ecu,
                Command::SetCalPage {
                    mode: 0,
                    segment: 0,
                    page: 1,
                },
            ),
            CommandCode::SetCalPage,
        );
        let payload = expect_ok(
            send(
                &mut ecu,
                Command::GetCalPage {
                    mode: 0,
                    segment: 0,
                },
            ),
            CommandCode::GetCalPage,
        );
        assert_eq!(u16::from_le_bytes([payload[0], payload[1]]), 1);

        let response = send(
            &mut ecu,
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 7,
            },
        );
        match response {
            Response::Err { code } => assert_eq!(code, ErrorCode::PageNotValid as u8),
            other => panic!("期望页号错误，实际 {other:?}"),
        }
    }

    #[test]
    fn every_response_fits_in_one_can_frame() {
        // 经典 CAN 一帧 8 字节；从站回超长响应会被总线层拒绝，
        // 现象是 master 一直等到超时。这里把每个命令的响应长度钉住。
        let commands = [
            Command::Connect { mode: 0 },
            Command::Disconnect,
            Command::GetStatus,
            Command::Synch,
            Command::GetCommModeInfo,
            Command::SetMta {
                address_extension: 0,
                address: 0x1000,
            },
            Command::ShortUpload {
                len: 6,
                address_extension: 0,
                address: 0x1000,
            },
            Command::Upload { len: 6 },
            Command::Download {
                len: 5,
                data: vec![0; 5],
            },
            Command::GetCalPage {
                mode: 0,
                segment: 0,
            },
            Command::SetCalPage {
                mode: 0,
                segment: 0,
                page: 1,
            },
        ];
        let mut ecu = ecu();
        ecu.poke(0x1000, &[0; 8]);
        for command in commands {
            let response = send(&mut ecu, command.clone());
            let frame = response.encode();
            assert!(
                frame.len() <= MAX_CTO,
                "{command:?} 的响应 {} 字节，超过一帧上限 {MAX_CTO}",
                frame.len()
            );
        }
    }

    #[test]
    fn unknown_command_byte_is_a_syntax_error() {
        let mut ecu = ecu();
        match ecu.handle(&[0x01, 0x02, 0x00]) {
            Response::Err { code } => assert_eq!(code, ErrorCode::CmdSyntax as u8),
            other => panic!("期望语法错误，实际 {other:?}"),
        }
    }

    #[test]
    fn disconnect_stops_the_session() {
        let mut ecu = ecu();
        let _ = send(&mut ecu, Command::Connect { mode: 0 });
        let _ = send(&mut ecu, Command::Disconnect);
        assert_eq!(
            expect_ok(send(&mut ecu, Command::GetStatus), CommandCode::GetStatus)[0],
            0
        );
    }
}
