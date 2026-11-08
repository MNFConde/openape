//! master 侧的协议对话。用脚本化总线替代真实从站，专测"对不上时怎么报"。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use openape_bus::{Bus, BusCapabilities, BusError, CanFrame, CanId, FrameKinds};
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, FaultCode};
use openape_xcp::connection::seq;
use openape_xcp::{CanXcpMaster, DaqListSpec, DaqSignal, XcpConfig, XcpConnection};
use openape_xcp_proto::{ErrorCode, MAX_CTO, Response};

/// 按剧本回帧的总线：发送计数，接收按队列顺序给。
#[derive(Debug)]
struct ScriptedBus {
    inbound: Mutex<VecDeque<CanFrame>>,
    sent: Mutex<Vec<CanFrame>>,
    sends: AtomicUsize,
    capabilities: BusCapabilities,
}

impl ScriptedBus {
    fn new() -> Self {
        Self::with_capabilities(BusCapabilities::CLASSIC_SOFT_TIMESTAMP)
    }

    /// 能发 CAN-FD 帧的总线：短写只有在它上面才装得下数据。
    fn with_fd() -> Self {
        Self::with_capabilities(BusCapabilities {
            backends: FrameKinds::BOTH,
            hardware_timestamp: false,
            cro_padding: true,
        })
    }

    fn with_capabilities(capabilities: BusCapabilities) -> Self {
        Self {
            inbound: Mutex::new(VecDeque::new()),
            sent: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
            capabilities,
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
        self.capabilities
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
    // 标准 CONNECT 响应布局：resource、commMode、maxCto(byte)、
    // maxDto(LE word)、protocol 版本、transport 版本。
    vec![0x05, 0x00, 0x08, 0x08, 0x00, 0x01, 0x01]
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
        info.protocol_version,
        Some((1, 0)),
        "标准布局带版本 major，minor 归零待 GET_VERSION 补"
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
    assert!(fault.message().contains("0x22"), "实际: {fault}");
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
    assert!(fault.message().contains("会话未建立"), "实际: {fault}");
    assert!(
        fault.hint().is_some_and(|hint| hint.contains("connect")),
        "提示要说清下一步：{fault}"
    );
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
    assert!(fault.message().contains("会话未建立"), "{fault}");
}

#[tokio::test]
async fn connect_response_with_zero_max_cto_is_rejected() {
    // 标准布局里 maxCto 是单字节；maxCto 为 0 是"响应不完整"的可靠判据。
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        // 长度够 7 字节，但 maxCto 与 maxDto 都是 0——从站不可能这么报。
        payload: vec![0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    });
    let mut master = master_with(&bus, 0);

    let fault = master.connect(0).await.expect_err("响应不完整");
    assert!(fault.message().contains("maxCto"), "实际: {fault}");
    assert!(fault.hint().is_some());
}

/// 把一组信号配成一条 list 所需的应答：CONNECT 之后每条命令一个空响应，
/// 最后 `START_STOP_DAQ_LIST` 回 firstPid。
fn queue_daq_dialog(bus: &ScriptedBus, command_count: usize, first_pid: u8) {
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    for _ in 1..command_count {
        bus.queue_response(Response::Ok { payload: vec![] });
    }
    bus.queue_response(Response::Ok {
        payload: vec![first_pid, 0x00],
    });
}

fn signal(address: u32, len: u16) -> DaqSignal {
    DaqSignal {
        address: Address::new(address),
        len: ByteLen::new(len).expect("测试长度非零"),
    }
}

#[tokio::test]
async fn configure_daq_emits_the_expected_command_sequence() {
    let bus = Arc::new(ScriptedBus::new());
    // 13 条 DAQ 命令：FREE、ALLOC_DAQ、ALLOC_ODT、ALLOC_ODT_ENTRY×2、
    // SET_DAQ_PTR×2、WRITE_DAQ×4、SET_DAQ_LIST_MODE、START_STOP_DAQ_LIST
    queue_daq_dialog(&bus, 13, 0);

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let list = DaqListSpec::new(
        0,
        1,
        vec![
            signal(0x1000, 2),
            signal(0x1002, 4),
            signal(0x1006, 2),
            signal(0x1008, 2),
        ],
    )
    .expect("配置合法");
    let layout = master.configure_daq(&[list]).await.expect("配置应成功");

    let codes: Vec<u8> = bus
        .sent_frames()
        .iter()
        .map(|frame| frame.data[0])
        .collect();
    assert_eq!(
        codes,
        vec![
            0xFF, // CONNECT
            0xD6, // FREE_DAQ
            0xD5, // ALLOC_DAQ
            0xD4, // ALLOC_ODT
            0xD3, // ALLOC_ODT_ENTRY（odt 0）
            0xD3, // ALLOC_ODT_ENTRY（odt 1）
            0xE2, // SET_DAQ_PTR（odt 0）
            0xE1, // WRITE_DAQ 0x1000
            0xE1, // WRITE_DAQ 0x1002
            0xE2, // SET_DAQ_PTR（odt 1）
            0xE1, // WRITE_DAQ 0x1006
            0xE1, // WRITE_DAQ 0x1008
            0xE0, // SET_DAQ_LIST_MODE
            0xDE, // START_STOP_DAQ_LIST
        ]
    );
    // 参数字节也要钉住：只对命令码的话，扩展地址、模式、优先级写错都发现不了
    let frames = bus.sent_frames();
    let set_ptr = &frames[6].data;
    assert_eq!(
        (set_ptr[0], set_ptr[5]),
        (0xE2, 0x00),
        "SET_DAQ_PTR 的 entry 为 0"
    );
    let write = &frames[7].data;
    assert_eq!(
        (write[0], write[1], write[3]),
        (0xE1, 0x00, 0x00),
        "WRITE_DAQ 的 bit_offset 与地址扩展为 0"
    );
    let mode = &frames[12].data;
    assert_eq!(mode[0], 0xE0);
    assert_eq!(mode[1], 0x00, "SET_DAQ_LIST_MODE 的 mode 为 0");
    assert_eq!(mode[7], 0x00, "priority 为 0");

    assert_eq!(layout.len(), 2, "8 字节切成两个 ODT");
    assert_eq!(
        layout.decode(0, &[1, 2, 3, 4, 5, 6]),
        Some(vec![&[1u8, 2][..], &[3, 4, 5, 6][..]])
    );
    assert_eq!(
        layout.decode(1, &[7, 8, 9, 10]),
        Some(vec![&[7u8, 8][..], &[9, 10][..]])
    );
}

#[tokio::test]
async fn status_reports_running_bit_and_protection() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    // GET_STATUS：byte0 的 bit0 是 running，byte2 是 protection
    bus.queue_response(Response::Ok {
        payload: vec![0x01, 0x00, 0x07],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let status = master.status().await.expect("要能读状态");
    assert!(status.running, "bit0 为 1");
    assert_eq!(status.protection, 0x07, "取第 3 字节");

    // 空载荷：两项都回落为 0，不臆造
    bus.queue_response(Response::Ok { payload: vec![] });
    let status = master.status().await.expect("空响应也要能读");
    assert!(!status.running);
    assert_eq!(status.protection, 0);
}

#[tokio::test]
async fn a_seven_byte_read_still_fits_one_short_upload() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        payload: vec![1, 2, 3, 4, 5, 6, 7],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let data = master
        .read(Address::new(0x1000), ByteLen::new(7).expect("7 合法"))
        .await
        .expect("7 字节正好一帧");
    assert_eq!(data.len(), 7);
    assert_eq!(
        bus.send_count(),
        2,
        "CONNECT 加一条 SHORT_UPLOAD：7 字节仍在单帧能力内"
    );
    assert_eq!(bus.sent_frames()[1].data[0], 0xF4);
}

#[tokio::test]
async fn long_reads_use_a_zero_address_extension() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] });
    bus.queue_response(Response::Ok {
        payload: vec![0; 7],
    });
    bus.queue_response(Response::Ok {
        payload: vec![0; 5],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master
        .read(Address::new(0x2000), ByteLen::new(12).expect("12 合法"))
        .await
        .expect("长读走 SET_MTA");

    let set_mta = &bus.sent_frames()[1].data;
    assert_eq!(set_mta[0], 0xF6);
    assert_eq!(set_mta[1], 0, "地址扩展字节必须是 0");
}

#[tokio::test]
async fn configure_daq_rejects_slave_pid_mismatch() {
    let bus = Arc::new(ScriptedBus::new());
    // 单 ODT、两个信号：FREE、ALLOC_DAQ、ALLOC_ODT、ALLOC_ODT_ENTRY、
    // SET_DAQ_PTR、WRITE_DAQ×2、SET_DAQ_LIST_MODE、START_STOP = 9 条
    queue_daq_dialog(&bus, 9, 5);

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let list = DaqListSpec::new(0, 1, vec![signal(0x1000, 2), signal(0x1002, 2)]).expect("合法");
    let fault = master
        .configure_daq(&[list])
        .await
        .expect_err("编号对不上要报");
    assert!(fault.message().contains("首个 ODT"), "{fault}");
}

#[tokio::test]
async fn configure_daq_refuses_slave_without_daq_resource() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        // resource = 0x01：只有标定页切换，没有 DAQ 位
        payload: vec![0x01, 0x00, 8, 0x00, 8, 0x00],
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let list = DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("合法");
    let fault = master
        .configure_daq(&[list])
        .await
        .expect_err("没有 DAQ 能力要报");
    assert!(fault.message().contains("DAQ"), "{fault}");
    assert_eq!(bus.send_count(), 1, "只在 CONNECT 上发过帧，不该动从站配置");
}

#[tokio::test]
async fn configure_daq_rejects_oversized_signal_before_touching_the_slave() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    // maxDto = 8 → 一个 DTO 只能装 7 字节载荷，8 字节信号放不下
    let list = DaqListSpec::new(0, 1, vec![signal(0x1000, 8)]).expect("配置本身合法");
    let fault = master.configure_daq(&[list]).await.expect_err("要提前拒绝");
    assert!(fault.message().contains("超过单个 ODT"), "{fault}");
    assert_eq!(bus.send_count(), 1, "分组算不通就不要动从站");
}

#[test]
fn max_short_upload_len_matches_frame_budget() {
    assert_eq!(openape_xcp::MAX_SHORT_UPLOAD_LEN as usize, MAX_CTO - 1);
}

/// 经典 CAN 上的写对话：SET_MTA 一次，再按 6 字节一帧搬，最后读回。
#[tokio::test]
async fn write_splits_by_frame_capacity_and_reads_back() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] }); // SET_MTA
    bus.queue_response(Response::Ok { payload: vec![] }); // DOWNLOAD 6 字节
    bus.queue_response(Response::Ok { payload: vec![] }); // DOWNLOAD 2 字节
    bus.queue_response(Response::Ok { payload: vec![] }); // 读回：SET_MTA
    bus.queue_response(Response::Ok {
        payload: vec![1, 2, 3, 4, 5, 6, 7],
    }); // 读回：UPLOAD 7
    bus.queue_response(Response::Ok { payload: vec![8] }); // 读回：UPLOAD 1

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master
        .write(Address::new(0x2000), &[1, 2, 3, 4, 5, 6, 7, 8])
        .await
        .expect("写入并读回一致");

    let frames = bus.sent_frames();
    let codes: Vec<u8> = frames.iter().map(|frame| frame.data[0]).collect();
    assert_eq!(
        codes,
        vec![0xFF, 0xF6, 0xF0, 0xF0, 0xF6, 0xF5, 0xF5],
        "先 SET_MTA 定位，再逐帧 DOWNLOAD，最后读回校验"
    );
    assert_eq!(
        frames[2].data[1], 6,
        "每帧搬 6 字节（整帧减去命令码与 len）"
    );
    assert_eq!(frames[3].data[1], 2, "最后一帧搬剩余 2 字节");
    assert_eq!(frames[1].data[1], 0, "地址扩展字节为 0");
}

#[tokio::test]
async fn write_that_does_not_read_back_names_the_differing_byte() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] }); // SET_MTA
    bus.queue_response(Response::Ok { payload: vec![] }); // DOWNLOAD
    bus.queue_response(Response::Ok {
        payload: vec![0x00, 0xFF],
    }); // 读回值与写入值不同

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let fault = master
        .write(Address::new(0x2000), &[0x00, 0x01])
        .await
        .expect_err("读回不一致要失败");

    assert_eq!(
        fault.code(),
        FaultCode::new(Domain::Xcp, seq::VERIFY).expect("序号合法"),
        "读回不一致是独立类别，不与协议错混在一起"
    );
    assert!(fault.message().contains("第 1 字节"), "实际: {fault}");
    assert!(fault.hint().is_some(), "要给可操作建议");
}

#[tokio::test]
async fn write_of_zero_bytes_is_refused_before_touching_the_bus() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");

    let fault = master
        .write(Address::new(0x2000), &[])
        .await
        .expect_err("空写没有意义");
    assert_eq!(
        fault.code(),
        FaultCode::new(Domain::Xcp, seq::WRITE).expect("序号合法")
    );
    assert_eq!(bus.send_count(), 1, "只在 CONNECT 上发过帧");
}

#[tokio::test]
async fn short_write_is_used_when_the_frame_is_wide_enough() {
    // 从站报 maxCto=64 且总线能发 CAN-FD：短写才装得下数据。
    let bus = Arc::new(ScriptedBus::with_fd());
    bus.queue_response(Response::Ok {
        payload: vec![0x05, 0x00, 64, 0x00, 64, 0x00],
    });
    bus.queue_response(Response::Ok { payload: vec![] }); // SHORT_DOWNLOAD
    bus.queue_response(Response::Ok { payload: vec![] }); // 读回：SET_MTA
    bus.queue_response(Response::Ok {
        payload: vec![0xAA; 7],
    }); // 读回：UPLOAD 7
    bus.queue_response(Response::Ok {
        payload: vec![0xAA; 3],
    }); // 读回：UPLOAD 3

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master
        .write(Address::new(0x2000), &[0xAA; 10])
        .await
        .expect("宽帧上的短写");

    let frames = bus.sent_frames();
    assert_eq!(frames[1].data[0], 0xED, "宽帧上走 SHORT_DOWNLOAD");
    assert_eq!(frames[1].data[1], 10, "len 等于写入字节数");
    assert_eq!(
        frames[1].data.len(),
        18,
        "命令头 8 字节加 10 字节数据，不补齐到 8 也不截断"
    );
    assert_eq!(frames[1].data[4], 0x00, "地址落在 byte4");
}

#[tokio::test]
async fn access_locked_write_carries_the_unlock_hint() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Err {
        code: ErrorCode::AccessLocked as u8,
    });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let fault = master
        .write(Address::new(0x2000), &[1])
        .await
        .expect_err("未解锁");
    let hint = fault.hint().unwrap_or_default().to_string();
    assert!(hint.contains("unlock"), "提示要指向解锁: {fault}");
}

#[tokio::test]
async fn set_cal_page_verifies_the_readback() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![] }); // SET_CAL_PAGE
    bus.queue_response(Response::Ok {
        payload: vec![0x00, 0x00, 0x00],
    }); // GET_CAL_PAGE：仍在页 0

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let fault = master.set_cal_page(1).await.expect_err("切换没生效");
    assert_eq!(
        fault.code(),
        FaultCode::new(Domain::Xcp, seq::PAGE).expect("序号合法")
    );
    assert!(fault.message().contains("读回是页 0"), "实际: {fault}");

    bus.queue_response(Response::Ok { payload: vec![] });
    bus.queue_response(Response::Ok {
        payload: vec![0x00, 0x00, 0x01],
    });
    master.set_cal_page(1).await.expect("读回一致就通过");
}

#[tokio::test]
async fn unlock_walks_seed_key_and_reports_the_resource_bit() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        payload: vec![4, 1, 2, 3, 4],
    }); // GET_SEED：4 字节种子
    bus.queue_response(Response::Ok {
        payload: vec![0x01],
    }); // UNLOCK：标定页资源位置位

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master.unlock().await.expect("示例算法应付得来");

    let frames = bus.sent_frames();
    assert_eq!(frames[1].data[..3], [0xF8, 0x00, 0x01], "GET_SEED 带资源位");
    assert_eq!(
        frames[2].data[..6],
        [0xF7, 0x04, 0xFE, 0xFD, 0xFC, 0xFB],
        "UNLOCK：长度加示例算法算出的密钥（种子取反）"
    );
}

#[tokio::test]
async fn unlock_failing_to_set_the_resource_bit_is_reported() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok {
        payload: vec![4, 1, 2, 3, 4],
    });
    bus.queue_response(Response::Ok {
        payload: vec![0x00],
    }); // 资源位没置起

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    let fault = master.unlock().await.expect_err("资源位没起来");
    assert_eq!(
        fault.code(),
        FaultCode::new(Domain::Xcp, seq::UNLOCK).expect("序号合法")
    );
    assert!(
        fault.hint().is_some_and(|hint| hint.contains("算法")),
        "{fault}"
    );
}

#[tokio::test]
async fn zero_length_seed_means_the_resource_is_already_open() {
    let bus = Arc::new(ScriptedBus::new());
    bus.queue_response(Response::Ok {
        payload: connect_ok_payload(),
    });
    bus.queue_response(Response::Ok { payload: vec![0] });

    let mut master = master_with(&bus, 0);
    master.connect(0).await.expect("建链成功");
    master.unlock().await.expect("已经解锁");
    assert_eq!(bus.send_count(), 2, "不必再发 UNLOCK");
}
