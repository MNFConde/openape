//! master 侧的协议对话。用脚本化总线替代真实从站，专测"对不上时怎么报"。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId, FrameKinds};
use openape_diag::Domain;
use openape_xcp::{CanXcpMaster, XcpConfig, XcpConnection};
use openape_xcp_proto::{CommandCode, ErrorCode, Response};

/// 按剧本回帧的总线：发送计数，接收按队列顺序给。
#[derive(Debug)]
struct ScriptedBus {
    inbound: Mutex<VecDeque<CanFrame>>,
    sent: Mutex<Vec<CanFrame>>,
    sends: AtomicUsize,
}

impl ScriptedBus {
    fn new() -> Self {
        Self {
            inbound: Mutex::new(VecDeque::new()),
            sent: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
        }
    }

    fn queue_response(&self, response: Response) {
        self.queue_frame(CanId::Standard(0x601), response.encode());
    }

    fn queue_frame(&self, id: CanId, data: Vec<u8>) {
        let frame = CanFrame::new(id, data).expect("不超过 64 字节");
        self.inbound.lock().expect("锁未中毒").push_back(frame);
    }

    fn sent_frames(&self) -> Vec<CanFrame> {
        self.sent.lock().expect("锁未中毒").clone()
    }

    fn send_count(&self) -> usize {
        self.sends.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Bus for ScriptedBus {
    fn backend(&self) -> &'static str {
        "scripted"
    }

    fn channel(&self) -> &str {
        "scripted0"
    }

    fn capabilities(&self) -> BusCapabilities {
        BusCapabilities {
            backends: FrameKinds::classic_only(),
            hardware_timestamp: false,
        }
    }

    async fn send(&self, frame: CanFrame) -> Result<(), BusError> {
        self.sends.fetch_add(1, Ordering::Relaxed);
        self.sent.lock().expect("锁未中毒").push(frame);
        Ok(())
    }

    async fn recv(&self, timeout: Duration) -> Result<CanFrame, BusError> {
        let next = self.inbound.lock().expect("锁未中毒").pop_front();
        match next {
            Some(frame) => Ok(frame),
            None => Err(BusError::Timeout {
                waited_ms: timeout.as_millis() as u64,
            }),
        }
    }
}

fn master_with(bus: &Arc<ScriptedBus>, retries: u8) -> CanXcpMaster {
    let mut config = XcpConfig::new(CanId::Standard(0x600), CanId::Standard(0x601));
    config.timeout = Duration::from_millis(5);
    config.retries = retries;
    CanXcpMaster::new(bus.clone(), config)
}

fn connect_ok_payload() -> Vec<u8> {
    // resource=DAQ|CAL_PAG, commMode=0, maxCto=8, maxDto=8。
    vec![0x05, 0x00, 0x08, 0x00, 0x08, 0x00]
}

#[tokio::test]
async fn connect_parses_slave_capabilities() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        command: CommandCode::Connect as u8,
        payload: connect_ok_payload(),
    });

    let mut master = master_with(&bus, 2);
    let info = master.connect(0).await.expect("从站正常应答");

    assert_eq!(info.resource, 0x05);
    assert_eq!(info.max_cto, 8);
    assert_eq!(info.max_dto, 8);
    assert_eq!(
        info.protocol_version, None,
        "CAN 上放不下版本字段，不许臆造"
    );
    assert_eq!(
        bus.sent_frames()[0].data,
        vec![0xFF, 0xFF, 0x00, 0, 0, 0, 0, 0],
        "CRO 帧应为 PID + 命令码 + 参数，补零到 8"
    );
}

#[tokio::test]
async fn missing_slave_times_out_and_reports_retry_count() {
    let bus = Arc::new(ScriptedBus::new());
    let mut master = master_with(&bus, 2);

    let fault = master.connect(0).await.expect_err("从站不存在");
    assert_eq!(fault.code().domain(), Domain::Xcp);
    assert_eq!(bus.send_count(), 3, "首发 1 次 + 重发 2 次");
    assert!(fault.hint().is_some(), "超时要给可操作建议");
}

#[tokio::test]
async fn response_echoing_wrong_command_is_a_protocol_error() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        command: CommandCode::GetStatus as u8,
        payload: connect_ok_payload(),
    });
    let mut master = master_with(&bus, 0);

    let fault = master.connect(0).await.expect_err("回显不符");
    assert!(fault.message().contains("回显"), "实际: {fault}");
}

#[tokio::test]
async fn slave_error_code_becomes_a_classified_fault() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Err {
        code: ErrorCode::OutOfRange as u8,
    });
    let mut master = master_with(&bus, 0);

    let fault = master.connect(0).await.expect_err("从站报错");
    assert!(fault.message().contains("0x12"), "实际: {fault}");
}

#[tokio::test]
async fn data_frames_from_other_ids_do_not_break_the_command() {
    let bus = Arc::new(ScriptedBus::new());
    // 先来一帧 DAQ 的 DTO，再来真正的响应。
    bus.queue_frame(
        CanId::Standard(0x601),
        vec![0x00, 0x11, 0x22, 0, 0, 0, 0, 0],
    );
    bus.queue_response(Response::Ok {
        command: CommandCode::Connect as u8,
        payload: connect_ok_payload(),
    });

    let mut master = master_with(&bus, 0);
    let info = master.connect(0).await.expect("数据帧应被跳过");
    assert_eq!(info.max_cto, 8);
}

#[tokio::test]
async fn disconnect_marks_session_closed() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        command: CommandCode::Connect as u8,
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        command: CommandCode::Disconnect as u8,
        payload: vec![],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master.disconnect().await.expect("断链成功");
    // 断链后再读会被拒，那条断言在 M1-09 加上 read 之后成立。
}

#[tokio::test]
async fn connect_response_with_zero_max_cto_is_rejected() {
    // 帧定长 8 字节，缺的参数会被补零；maxCto 为 0 是"响应不完整"的可靠判据。
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        command: CommandCode::Connect as u8,
        // 长度够 6 字节，但 maxCto 与 maxDto 都是 0——从站不可能这么报。
        payload: vec![0x05, 0x00, 0x00, 0x00, 0x00, 0x00],
    });
    let mut master = master_with(&bus, 0);

    let fault = master.connect(0).await.expect_err("响应不完整");
    assert!(fault.message().contains("maxCto"), "实际: {fault}");
    assert!(fault.hint().is_some());
}
