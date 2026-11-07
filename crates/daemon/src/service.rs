//! `OpenApe` 服务的实现。
//!
//! 这里是唯一知道"daemon 有没有真的接管硬件"的地方：接管了就答自己知道的，
//! 没接管才去临时探测。探测会抢设备锁，所以不能无条件做。

use std::sync::Arc;

use openape_diag::{Fault, Metrics};
use openape_proto as proto;
use openape_session::{CaptureConfig, DaqCapture, DaqStream, Session, list_channels};
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
    /// 只为持有而不读：会话被丢弃即释放设备锁，采集泵虽仍在读总线，
    /// 但通道已经可以被别人抢走。名字前的下划线是给 `dead_code` 看的。
    _session: Option<Session>,
    channel: Option<String>,
}

impl AppState {
    /// 只提供健康与指标，不碰硬件。
    pub fn idle(metrics: Arc<Metrics>) -> Self {
        Self {
            metrics,
            idle: Arc::new(DaqStream::new(CaptureConfig::default().capacity)),
            capture: None,
            _session: None,
            channel: None,
        }
    }

    /// 接管一条通道并开始采集。`capture` 由调用方配好 DAQ 后起。
    pub fn with_capture(
        metrics: Arc<Metrics>,
        channel: String,
        session: Session,
        capture: DaqCapture,
    ) -> Self {
        Self {
            metrics,
            idle: Arc::new(DaqStream::new(CaptureConfig::default().capacity)),
            capture: Some(capture),
            _session: Some(session),
            channel: Some(channel),
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
            // 已接管：答自己知道的，别再去抢一次锁。
            return Err(Status::failed_precondition(format!(
                "daemon 已独占 {channel}，探测请走 CLI 的 --via-daemon"
            )));
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

    async fn health(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::HealthStatus>, Status> {
        Ok(Response::new(proto::HealthStatus {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
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

/// 把域故障翻成 gRPC 状态。消息带上原始提示，客户端不必再查表。
fn to_status(fault: Fault) -> Status {
    let mut message = fault.to_string();
    if let Some(hint) = fault.hint() {
        message.push('；');
        message.push_str(hint);
    }
    Status::failed_precondition(message)
}
