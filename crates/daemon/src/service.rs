//! `OpenApe` 服务的实现。
//!
//! 这里是唯一知道"daemon 有没有真的接管硬件"的地方：接管了就答自己知道的，
//! 没接管才去临时探测。探测会抢设备锁，所以不能无条件做。

use std::sync::Arc;

use openape_cal_core::{Address, ByteLen};
use openape_diag::{Fault, Metrics};
use openape_proto as proto;
use openape_session::{CaptureConfig, DaqCapture, DaqStream, Session, list_channels};
use openape_xcp::ConnectInfo;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

/// daemon 的运行状态。
#[derive(Debug)]
pub struct AppState {
    metrics: Arc<Metrics>,
    /// 未接管硬件时的空流：客户端照常订阅，只是永远等不到样本。
    idle: Arc<DaqStream>,
    /// 已接管的会话与采集。会话本身也要留着：丢出即释放设备锁，
    /// 采集泵虽然还在读总线，但锁已经没了。
    capture: Option<DaqCapture>,
    /// 会话本身也要留着：丢出即释放设备锁，采集泵虽然还在读总线，但通道
    /// 已经能被别人抢走。加锁是为了让代读命令与采集互斥。
    session: Option<tokio::sync::Mutex<Session>>,
    channel: Option<String>,
    info: Option<ConnectInfo>,
    /// 已加载的变量清单。未加载描述文件时为空，界面据此说明原因。
    variables: Option<Arc<proto::VariableList>>,
    /// 最近的故障。只存 `openape_diag::Fault` 的映射结果，不另造类型。
    faults: Arc<std::sync::Mutex<Vec<proto::FaultEvent>>>,
}

impl AppState {
    /// 只提供健康与指标，不碰硬件。
    pub fn idle(metrics: Arc<Metrics>) -> Self {
        Self {
            metrics,
            idle: Arc::new(DaqStream::new(CaptureConfig::default().capacity)),
            capture: None,
            session: None,
            channel: None,
            info: None,
            variables: None,
            faults: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// 接管一条通道并开始采集。`capture` 由调用方配好 DAQ 后起。
    pub fn with_capture(
        metrics: Arc<Metrics>,
        channel: String,
        session: Session,
        capture: DaqCapture,
        info: ConnectInfo,
    ) -> Self {
        Self {
            metrics,
            idle: Arc::new(DaqStream::new(CaptureConfig::default().capacity)),
            capture: Some(capture),
            session: Some(tokio::sync::Mutex::new(session)),
            channel: Some(channel),
            info: Some(info),
            variables: None,
            faults: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// 指标出口。
    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// 已接管的通道名。
    pub fn channel(&self) -> Option<&str> {
        self.channel.as_deref()
    }

    /// 建链时从站报告的能力，供 CLI 与 UI 显示。
    pub fn info(&self) -> Option<&ConnectInfo> {
        self.info.as_ref()
    }

    /// 已接管的会话。没有接管时说明 daemon 只是健康检查壳子。
    pub fn session(&self) -> Option<&tokio::sync::Mutex<Session>> {
        self.session.as_ref()
    }

    /// 采集泵。`None` 表示没有在采集。
    pub fn capture(&self) -> Option<&DaqCapture> {
        self.capture.as_ref()
    }

    /// 已加载的变量清单。
    pub fn variables(&self) -> Option<&Arc<proto::VariableList>> {
        self.variables.as_ref()
    }

    /// 最近的故障，按发生顺序。
    pub fn faults(&self) -> Vec<proto::FaultEvent> {
        self.faults
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 记一条故障。只收 `openape_diag::Fault` 的映射结果——daemon 不自造
    /// 错误类型，否则界面上的故障与日志里的对不上。
    pub fn record_fault(&self, fault: &openape_diag::Fault) {
        const KEEP: usize = 64;
        if let Ok(mut guard) = self.faults.lock() {
            if guard.len() >= KEEP {
                guard.remove(0);
            }
            guard.push(fault_event(fault));
        }
    }

    /// 装上一份变量清单。
    pub fn with_variables(mut self, variables: proto::VariableList) -> Self {
        self.variables = Some(Arc::new(variables));
        self
    }

    fn stream(&self) -> &DaqStream {
        match self.capture.as_ref() {
            Some(capture) => capture.stream(),
            None => &self.idle,
        }
    }
}

/// 服务实现。
#[derive(Debug, Clone)]
pub struct OpenApeService {
    state: Arc<AppState>,
}

impl OpenApeService {
    /// 包一份状态。
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::open_ape_server::OpenApe for OpenApeService {
    async fn list_channels(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::ListChannelsResponse>, Status> {
        Ok(Response::new(proto::ListChannelsResponse {
            channels: list_channels(),
        }))
    }

    async fn probe_channel(
        &self,
        request: Request<proto::ProbeChannelRequest>,
    ) -> Result<Response<proto::ChannelProbe>, Status> {
        let channel = request.into_inner().channel;
        if self.state.channel() == Some(channel.as_str()) {
            // 已接管：答自己知道的，再去抢一次锁必然撞锁。
            if let Some(session) = self.state.session() {
                let guard = session.lock().await;
                let capabilities = guard.capabilities();
                return Ok(Response::new(proto::ChannelProbe {
                    channel,
                    backend: openape_bus_socketcan::BACKEND.to_owned(),
                    capabilities: Some(proto::BusCapabilities {
                        classic_can: capabilities.classic_can(),
                        can_fd: capabilities.can_fd(),
                        hardware_timestamp: capabilities.hardware_timestamp,
                    }),
                }));
            }
        }
        let probe = openape_session::probe_channel(&channel).map_err(to_status)?;
        Ok(Response::new(proto::ChannelProbe {
            channel: probe.channel,
            backend: probe.backend.to_owned(),
            capabilities: Some(proto::BusCapabilities {
                classic_can: probe.capabilities.classic_can(),
                can_fd: probe.capabilities.can_fd(),
                hardware_timestamp: probe.capabilities.hardware_timestamp,
            }),
        }))
    }

    async fn read_memory(
        &self,
        request: Request<proto::ReadRequest>,
    ) -> Result<Response<proto::ReadResponse>, Status> {
        let request = request.into_inner();
        let Some(session) = self.state.session() else {
            return Err(Status::failed_precondition(
                "daemon 没有接管通道，无法代读；直接连设备或用 CLI 不带 --via-daemon",
            ));
        };
        let address = parse_address(&request.address)?;
        let len = u16::try_from(request.len)
            .ok()
            .and_then(ByteLen::new)
            .ok_or_else(|| Status::invalid_argument(format!("长度 {} 不合法", request.len)))?;

        // 读之前让采集泵让开接收端，读完无论成败都要恢复。
        let capture = self.state.capture();
        if let Some(capture) = capture {
            capture.set_paused(true);
        }
        let mut guard = session.lock().await;
        let read = guard.read(address, len).await;
        drop(guard);
        if let Some(capture) = capture {
            capture.set_paused(false);
        }

        let data = read.map_err(to_status)?;
        let channel = self.state.channel().unwrap_or_default().to_owned();
        Ok(Response::new(proto::ReadResponse {
            channel,
            address: address.to_string(),
            len: u32::from(len.get()),
            data_hex: data
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
        }))
    }

    async fn get_session(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::SessionReport>, Status> {
        Ok(Response::new(proto::SessionReport {
            channel: self.state.channel().unwrap_or_default().to_owned(),
            linked: self.state.session().is_some(),
            info: self.state.info().map(to_connect_info),
            capturing: self.state.capture().is_some(),
        }))
    }

    async fn health(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::HealthStatus>, Status> {
        Ok(Response::new(proto::HealthStatus {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }))
    }

    async fn list_variables(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::VariableList>, Status> {
        let list = match self.state.variables() {
            Some(list) => (**list).clone(),
            None => proto::VariableList {
                variables: Vec::new(),
                notes: Vec::new(),
                loaded: false,
            },
        };
        Ok(Response::new(list))
    }

    async fn list_faults(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::FaultList>, Status> {
        Ok(Response::new(proto::FaultList {
            faults: self.state.faults(),
        }))
    }

    async fn get_metrics(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::MetricsSnapshot>, Status> {
        let snapshot = self.state.metrics().snapshot();
        Ok(Response::new(proto::MetricsSnapshot {
            daq_fps: snapshot.daq_fps,
            daq_frames_total: snapshot.daq_frames_total,
            dropped_frames_total: snapshot.dropped_frames_total,
            latency_p50_us: snapshot.latency_p50_us,
            latency_p99_us: snapshot.latency_p99_us,
            latency_samples: snapshot.latency_samples,
            queue_depth: snapshot.queue_depth,
            rss_bytes: snapshot.rss_bytes,
        }))
    }

    type StreamSamplesStream = ReceiverStream<Result<proto::DaqSample, Status>>;

    async fn stream_samples(
        &self,
        _request: Request<proto::StreamRequest>,
    ) -> Result<Response<Self::StreamSamplesStream>, Status> {
        let mut reader = self.state.stream().subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            while let Some(sample) = reader.recv().await {
                let message = proto::DaqSample {
                    pid: u32::from(sample.pid),
                    timestamp_us: sample.timestamp_us,
                    payload: sample.payload,
                };
                // 客户端断开就没必要继续搬：send 失败即退出。
                if tx.send(Ok(message)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

/// 会话层的建链信息转成契约类型。
fn to_connect_info(info: &ConnectInfo) -> proto::ConnectInfo {
    proto::ConnectInfo {
        resource: u32::from(info.resource),
        comm_mode_basic: u32::from(info.comm_mode_basic),
        max_cto: u32::from(info.max_cto),
        max_dto: u32::from(info.max_dto),
        protocol_version: info
            .protocol_version
            .map(|(major, minor)| proto::ProtocolVersion {
                major: u32::from(major),
                minor: u32::from(minor),
            }),
        transport_version: info
            .transport_version
            .map(|(major, minor)| proto::ProtocolVersion {
                major: u32::from(major),
                minor: u32::from(minor),
            }),
    }
}

/// 地址从字符串解析。十六进制带 `0x` 前缀，也接受纯十进制。
fn parse_address(raw: &str) -> Result<Address, Status> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"));
    let parsed = match digits {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => trimmed.parse::<u32>(),
    };
    parsed
        .map(Address::new)
        .map_err(|_| Status::invalid_argument(format!("地址 {raw} 不是 32 位整数")))
}

/// 把域故障翻成 gRPC 状态。消息带上原始提示，客户端不必再查表。
fn to_status(fault: Fault) -> Status {
    let mut message = fault.to_string();
    if let Some(hint) = fault.hint() {
        message.push('；');
        message.push_str(hint);
    }
    Status::failed_precondition(message)
}

/// 把 `openape_diag::Fault` 映射成契约里的形状。
///
/// 映射而不是另造：界面上的故障要能和日志里的同一条对上，字段偏离一点
/// 就查不下去了。
pub fn fault_event(fault: &openape_diag::Fault) -> proto::FaultEvent {
    proto::FaultEvent {
        code: fault.code().to_string(),
        severity: format!("{:?}", fault.severity()),
        message: fault.message().to_owned(),
        hint: fault.hint().unwrap_or_default().to_owned(),
        occurred_at_us: now_us(),
    }
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .unwrap_or(0)
}
