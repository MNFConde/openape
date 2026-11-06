//! master 侧的协议对话。用脚本化总线替代真实从站，专测"对不上时怎么报"。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId};
use openape_cal_core::{Address, ByteLen};
use openape_diag::Domain;
use openape_xcp::{CanXcpMaster, XcpConfig, XcpConnection};
use openape_xcp_proto::{ErrorCode, MAX_CTO, Response};

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
        BusCapabilities::CLASSIC_SOFT_TIMESTAMP
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
        vec![0xFF, 0x00, 0, 0, 0, 0, 0, 0],
        "CRO 帧应以命令码开头、补零到 8，没有 PID 前缀"
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
async fn event_frames_are_skipped_until_the_answer_arrives() {
    // 真机上事件帧与响应混在同一标识符：不能因为来了数据就判命令失败，
    // 也不能拿事件帧当答复。
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_frame(CanId::Standard(0x601), vec![0xFD, 0x01]);
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    let mut master = master_with(&bus, 0);

    let info = master.connect(0).await.expect("跳过事件帧后应拿到答复");
    assert_eq!(info.resource, 0x05);
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
        payload: connect_ok_payload(),
    });

    let mut master = master_with(&bus, 0);
    let info = master.connect(0).await.expect("数据帧应被跳过");
    assert_eq!(info.max_cto, 8);
}

#[tokio::test]
async fn read_before_connect_is_refused_without_touching_the_bus() {
    let bus = Arc::new(ScriptedBus::new());
    let mut master = master_with(&bus, 0);

    let fault = master
        .read(Address::new(0x1000), ByteLen::new(2).expect("2 合法"))
        .await
        .expect_err("未连接");
    assert!(fault.message().contains("connect"), "实际: {fault}");
    assert_eq!(bus.send_count(), 0, "不该往总线上发任何东西");
}

#[tokio::test]
async fn read_returns_exactly_the_requested_bytes() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        payload: vec![0xAA, 0xBB, 0xCC, 0xDD],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let data = master
        .read(Address::new(0x1002), ByteLen::new(2).expect("2 合法"))
        .await
        .expect("读到数据");
    assert_eq!(data, vec![0xAA, 0xBB], "只取请求的字节数，尾部填充不要");

    let sent = bus.sent_frames();
    assert_eq!(sent[1].id, CanId::Standard(0x600));
    assert_eq!(
        sent[1].data,
        vec![0xF4, 0x02, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00],
        "SHORT_UPLOAD 帧：命令码 + len + ext + 地址（地址落在整帧 offset 4）"
    );
}

#[tokio::test]
async fn read_longer_than_one_frame_goes_through_set_mta_then_upload() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] });
    bus.queue_response(Response::Ok {
        payload: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
    });
    bus.queue_response(Response::Ok {
        payload: vec![0x08],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let data = master
        .read(Address::new(0x1000), ByteLen::new(8).expect("8 合法"))
        .await
        .expect("长读走 SET_MTA + UPLOAD");

    assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    let sent = bus.sent_frames();
    assert_eq!(sent[1].data[0], 0xF6, "先 SET_MTA");
    assert_eq!(sent[2].data[0], 0xF5, "再 UPLOAD");
    assert_eq!(sent[2].data[1], 7, "每帧最多搬 7 字节（整帧减去响应 PID）");
    assert_eq!(sent[3].data[1], 1, "最后一帧搬剩余 1 字节");
}

#[tokio::test]
async fn upload_shorter_than_requested_is_a_protocol_error() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        payload: vec![0xAA],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let fault = master
        .read(Address::new(0x1000), ByteLen::new(4).expect("4 合法"))
        .await
        .expect_err("只回了 1 字节");
    assert!(fault.message().contains("只回了"), "实际: {fault}");
}

#[tokio::test]
async fn disconnect_marks_session_closed() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master.disconnect().await.expect("断链成功");

    let fault = master
        .read(Address::new(0x1000), ByteLen::new(1).expect("1 合法"))
        .await
        .expect_err("断链后不能再读");
    assert!(fault.message().contains("connect"));
}

#[tokio::test]
async fn connect_response_with_zero_max_cto_is_rejected() {
    // 帧定长 8 字节，缺的参数会被补零；maxCto 为 0 是"响应不完整"的可靠判据。
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        // 长度够 6 字节，但 maxCto 与 maxDto 都是 0——从站不可能这么报。
        payload: vec![0x05, 0x00, 0x00, 0x00, 0x00, 0x00],
    });
    let mut master = master_with(&bus, 0);

    let fault = master.connect(0).await.expect_err("响应不完整");
    assert!(fault.message().contains("maxCto"), "实际: {fault}");
    assert!(fault.hint().is_some());
}

#[test]
fn max_short_upload_len_matches_frame_budget() {
    assert_eq!(openape_xcp::MAX_SHORT_UPLOAD_LEN as usize, MAX_CTO - 1);
}
