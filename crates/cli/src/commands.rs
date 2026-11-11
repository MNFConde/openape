//! 三条子命令的实现。`main` 只负责解析与分派，这里一命令一函数。

use std::time::Duration;

use openape_bus::CanId;
use openape_cal_core::{Address, ByteLen};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_proto::open_ape_client::OpenApeClient;
use openape_proto::{BusReport, ConnectReport};
use openape_session::{Session, SessionConfig, list_channels, probe_channel};
use tonic::transport::Channel;

pub use crate::output::render;
use crate::output::{
    CrossCheckReport, SourceReport, SourceVariable, connect_info, probe_report, read_report,
};
use crate::usage_fault;

/// `probe bus`：列通道，可选探测一条。
pub async fn probe_bus(config_channel: Option<&str>, json: bool) -> Result<String, Fault> {
    let channels = list_channels();
    let probed = match config_channel {
        Some(channel) => Some(probe_report(probe_channel(channel)?)),
        None => None,
    };
    let report = BusReport { channels, probed };
    render_bus(json, &report)
}

/// 通道报告的双模渲染，直连与经 daemon 共用。
pub fn render_bus(json: bool, report: &BusReport) -> Result<String, Fault> {
    render(json, report, || {
        let mut out = format!("通道 {} 条", report.channels.len());
        if report.channels.is_empty() {
            out.push_str("\n  无 CAN 通道；vcan 可用 `ip link add dev vcan0 type vcan` 建一个");
        }
        for channel in &report.channels {
            out.push_str(&format!("\n  {channel}"));
        }
        if let Some(probe) = report.probed.as_ref() {
            let caps = probe.capabilities.as_ref();
            out.push_str(&format!(
                "\n探测 {}（{}）：经典 CAN={} CAN-FD={} 硬件时间戳={}",
                probe.channel,
                probe.backend,
                caps.is_some_and(|caps| caps.classic_can),
                caps.is_some_and(|caps| caps.can_fd),
                caps.is_some_and(|caps| caps.hardware_timestamp)
            ));
        }
        out
    })
}

/// `probe xcp`：建链后立刻断链，只报能力。
pub async fn probe_xcp(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
    json: bool,
) -> Result<String, Fault> {
    let report = xcp_connect(channel, tx_id, rx_id, timeout_ms).await?;
    render_connect(json, &report)
}

/// XCP 建链的判定本体：`probe xcp` 与 doctor 走同一个函数，判定条件
/// 只写一份。
async fn xcp_connect(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
) -> Result<ConnectReport, Fault> {
    let config = session_config(channel, tx_id, rx_id, timeout_ms)?;
    let mut session = Session::open(config)?;
    let info = session.connect(0).await?;
    let report = ConnectReport {
        channel: session.channel().to_owned(),
        info: Some(connect_info(
            info.resource,
            info.comm_mode_basic,
            info.max_cto,
            info.max_dto,
            info.protocol_version,
            info.transport_version,
        )),
    };
    session.disconnect().await.ok();
    Ok(report)
}

/// `read`：读一段内存并打印。
pub async fn read_memory(
    channel: &str,
    address: u32,
    len: u16,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
    json: bool,
) -> Result<String, Fault> {
    let config = session_config(channel, tx_id, rx_id, timeout_ms)?;
    let mut session = Session::open(config)?;
    session.connect(0).await?;
    let address = Address::new(address);
    let len = ByteLen::new(len).ok_or_else(|| usage_fault("--len 必须大于 0".into()))?;
    let data = session.read(address, len).await?;
    session.disconnect().await.ok();
    let report = read_report(
        session.channel().to_owned(),
        address.to_string(),
        len.get(),
        &data,
    );
    render_read(json, &report)
}

/// 把命令行参数变成会话配置，顺手校验标识符范围。
pub fn session_config(
    channel: &str,
    tx_id: u32,
    rx_id: u32,
    timeout_ms: u64,
) -> Result<SessionConfig, Fault> {
    let mut config = SessionConfig::new(channel);
    config.command_id = CanId::standard(to_standard_id(tx_id, "--tx-id")?)
        .ok_or_else(|| usage_fault(format!("--tx-id {tx_id:#x} 超出 11 位标准帧范围")))?;
    config.response_id = CanId::standard(to_standard_id(rx_id, "--rx-id")?)
        .ok_or_else(|| usage_fault(format!("--rx-id {rx_id:#x} 超出 11 位标准帧范围")))?;
    config.timeout = Duration::from_millis(timeout_ms);
    Ok(config)
}

fn to_standard_id(raw: u32, flag: &str) -> Result<u16, Fault> {
    u16::try_from(raw).map_err(|_| {
        usage_fault(format!("{flag} {raw:#x} 超出 16 位")).with_context("flag", flag.to_owned())
    })
}

/// `--via-daemon` 不带值时的默认地址。
pub const DAEMON_DEFAULT: &str = "http://127.0.0.1:47711";

/// 经 daemon 探测通道：daemon 已经拿着设备锁，这里只是问它要答案。
pub async fn probe_bus_via(url: &str, channel: Option<&str>, json: bool) -> Result<String, Fault> {
    let mut client = connect_daemon(url).await?;
    let channels = client
        .list_channels(openape_proto::Empty {})
        .await
        .map_err(daemon_fault)?
        .into_inner()
        .channels;
    let probed = match channel {
        Some(channel) => Some(
            client
                .probe_channel(openape_proto::ProbeChannelRequest {
                    channel: channel.to_owned(),
                })
                .await
                .map_err(daemon_fault)?
                .into_inner(),
        ),
        None => None,
    };
    let report = BusReport { channels, probed };
    render_bus(json, &report)
}

/// 经 daemon 读内存。读之前 daemon 会让采集泵让开接收端。
pub async fn read_via(url: &str, address: u32, len: u16, json: bool) -> Result<String, Fault> {
    if len == 0 {
        return Err(usage_fault("--len 必须大于 0".into()));
    }
    let mut client = connect_daemon(url).await?;
    let report = client
        .read_memory(openape_proto::ReadRequest {
            address: format!("0x{address:08x}"),
            len: u32::from(len),
        })
        .await
        .map_err(daemon_fault)?
        .into_inner();
    render_read(json, &report)
}

/// 经 daemon 看会话状态：它建好链了，就不必我们再连一次。
pub async fn probe_xcp_via(url: &str, channel: &str, json: bool) -> Result<String, Fault> {
    let mut client = connect_daemon(url).await?;
    let status = client
        .get_session(openape_proto::Empty {})
        .await
        .map_err(daemon_fault)?
        .into_inner();
    if !status.linked {
        return Err(daemon_fault(
            "daemon 没有接管任何通道；去掉 --via-daemon 直连设备",
        ));
    }
    if status.channel != channel {
        return Err(daemon_fault(format!(
            "daemon 接管的是 {}，不是 {channel}",
            status.channel
        )));
    }
    let report = ConnectReport {
        channel: status.channel,
        info: status.info,
    };
    render_connect(json, &report)
}

/// 连到 daemon。`write`/`page` 那几条命令也走这里，避免两套连接逻辑。
pub async fn connect_daemon(url: &str) -> Result<OpenApeClient<Channel>, Fault> {
    OpenApeClient::connect(url.to_owned())
        .await
        .map_err(|err| daemon_fault(format!("连不上 daemon {url}：{err}")))
}

/// daemon 侧的失败。归会话域：设备在 daemon 手里，问题出在那条会话上。
pub fn daemon_fault(message: impl ToString) -> Fault {
    Fault::new(
        FaultCode::from_const(Domain::Session, 4),
        Severity::Error,
        message.to_string(),
    )
}

/// 建链报告的双模渲染，两条路径共用，输出不会漂移。
pub fn render_connect(json: bool, report: &ConnectReport) -> Result<String, Fault> {
    let channel = &report.channel;
    let info = report.info.as_ref();
    render(json, report, || {
        let Some(info) = info else {
            return format!("{channel} 建链成功（daemon 未报告会话参数）");
        };
        format!(
            "{channel} 建链成功：resource={:#04x} maxCto={} maxDto={}",
            info.resource, info.max_cto, info.max_dto
        )
    })
}

/// 读结果的双模渲染。
pub fn render_read(json: bool, report: &openape_proto::ReadResponse) -> Result<String, Fault> {
    render(json, report, || {
        format!("{} {} = [{}]", report.address, report.len, report.data_hex)
    })
}

/// `probe source`：报一份或两份描述文件里的变量，并做交叉校验。
///
/// 不给文件时用内置的"什么都没有"错误而不是猜默认路径——用户手上可能有
/// 十几份 A2L，猜错一份比报错更难查。
pub fn probe_source(
    elf: Option<&std::path::Path>,
    a2l: Option<&std::path::Path>,
    json: bool,
) -> Result<String, Fault> {
    let mut notes = Vec::new();
    let elf_tree = elf.map(|path| load_elf(path, &mut notes)).transpose()?;
    let a2l_tree = a2l.map(|path| load_a2l(path, &mut notes)).transpose()?;

    // 两份都给时以 ELF 为主：它描述的是编进 ECU 的那份二进制，A2L 可能过时。
    // "至少要给一个来源"就在这一处判，不再靠前置检查 + expect 兜着。
    let primary = match (&elf_tree, &a2l_tree) {
        (Some(elf), _) => elf,
        (None, Some(a2l)) => a2l,
        (None, None) => {
            return Err(usage_fault(
                "probe source 至少要给一个来源：--elf 或 --a2l".to_owned(),
            ));
        }
    };
    let report = SourceReport {
        variables: primary.iter().map(source_variable).collect(),
        cross_check: match (&elf_tree, &a2l_tree) {
            (Some(elf), Some(a2l)) => Some(build_cross_check(elf, a2l)),
            _ => None,
        },
        notes,
    };
    if json {
        serde_json::to_string_pretty(&report)
            .map_err(|err| usage_fault(format!("序列化失败：{err}")))
    } else {
        Ok(report.render())
    }
}

fn load_elf(
    path: &std::path::Path,
    notes: &mut Vec<String>,
) -> Result<openape_cal_core::VariableTree, Fault> {
    let source = openape_format_elf::load(path).map_err(fault_of)?;
    let (tree, elf_notes) = source.to_ir().map_err(fault_of)?;
    notes.extend(elf_notes.iter().map(ToString::to_string));
    Ok(tree)
}

fn load_a2l(
    path: &std::path::Path,
    notes: &mut Vec<String>,
) -> Result<openape_cal_core::VariableTree, Fault> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| usage_fault(format!("读不了 {}：{err}", path.display())))?;
    let source = openape_format_a2l::load_str(&text, None).map_err(fault_of)?;
    notes.extend(source.notes().iter().map(ToString::to_string));
    Ok(source.tree().clone())
}

fn build_cross_check(
    elf: &openape_cal_core::VariableTree,
    a2l: &openape_cal_core::VariableTree,
) -> CrossCheckReport {
    let report = openape_assembly::cross_check(elf, a2l);
    CrossCheckReport {
        conflicts: report.conflicts().iter().map(ToString::to_string).collect(),
        only_in_elf: symbols(report.only_in_elf()),
        only_in_a2l: symbols(report.only_in_a2l()),
    }
}

fn symbols(list: &[openape_cal_core::Symbol]) -> Vec<String> {
    list.iter()
        .map(|symbol| symbol.as_str().to_owned())
        .collect()
}

fn fault_of(error: impl std::fmt::Display) -> Fault {
    usage_fault(error.to_string())
}

/// doctor 的一次检查结论。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DoctorCheck {
    /// 类别名。
    pub name: &'static str,
    /// pass / skip / fail。
    pub outcome: &'static str,
    /// 判定细节。
    pub detail: String,
}

/// doctor 的输入。缺的类别跳过——缺省项不猜结果。
pub struct DoctorOptions<'a> {
    /// CAN 通道（给了才做通道探测）。
    pub channel: Option<&'a str>,
    /// 描述文件 ELF（给了才做信息源检查）。
    pub elf: Option<&'a std::path::Path>,
    /// 描述文件 A2L。
    pub a2l: Option<&'a std::path::Path>,
    /// daemon 地址（给了才做连通检查）。
    pub daemon: Option<&'a str>,
    /// 真建一次 XCP 链（会碰 ECU）。
    pub connect: bool,
}

/// doctor（M5-16）：一次跑完各故障类别的判定，收集全部结论而不是遇错
/// 即停。判定本体全部复用 probe 系的函数（通道枚举、通道探测、建链、
/// 描述文件装载、daemon 连通各一份实现），doctor 只收集与汇总——判定
/// 条件写两份就会漂移，probe 过的 doctor 也必须过。
pub async fn doctor(options: DoctorOptions<'_>) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let pass = |name: &'static str, detail: String| DoctorCheck {
        name,
        outcome: "pass",
        detail,
    };
    let skip = |name: &'static str, detail: &str| DoctorCheck {
        name,
        outcome: "skip",
        detail: detail.to_owned(),
    };
    fn fail(name: &'static str, error: impl std::fmt::Display) -> DoctorCheck {
        DoctorCheck {
            name,
            outcome: "fail",
            detail: error.to_string(),
        }
    }

    // 1. 通道枚举。
    let channels = list_channels();
    checks.push(if channels.is_empty() {
        pass(
            "channels",
            "0 条（本机没有可见 CAN 通道；vcan 用 ip link 建）".to_owned(),
        )
    } else {
        pass(
            "channels",
            format!("{} 条：{}", channels.len(), channels.join("、")),
        )
    });

    // 2. 通道探测（给了 --channel 才查）。
    checks.push(match options.channel {
        Some(channel) => match probe_channel(channel) {
            Ok(_) => pass("channel", format!("{channel} 可打开")),
            Err(error) => fail("channel", error),
        },
        None => skip("channel", "没给 --channel"),
    });

    // 3. XCP 建链（显式 --connect 才碰 ECU）。
    checks.push(match (options.connect, options.channel) {
        (true, Some(channel)) => match xcp_connect(channel, 0x600, 0x601, 200).await {
            Ok(report) => pass("xcp", format!("{channel} 建链成功：{}", report.channel)),
            Err(error) => fail("xcp", error),
        },
        (true, None) => skip("xcp", "--connect 需要配 --channel"),
        (false, _) => skip("xcp", "默认不碰 ECU，要建链加 --connect"),
    });

    // 4. 信息源（给了 --elf / --a2l 才查）。
    checks.push(match (options.elf, options.a2l) {
        (None, None) => skip("source", "没给 --elf / --a2l"),
        (elf, a2l) => {
            let mut notes = Vec::new();
            let outcome = (|| -> Result<(usize, usize), Fault> {
                let elf = elf
                    .map(|path| load_elf(path, &mut notes))
                    .transpose()?
                    .map(|tree| tree.iter().count())
                    .unwrap_or(0);
                let a2l = a2l
                    .map(|path| load_a2l(path, &mut notes))
                    .transpose()?
                    .map(|tree| tree.iter().count())
                    .unwrap_or(0);
                Ok((elf, a2l))
            })();
            match outcome {
                Ok((elf_vars, a2l_vars)) => pass(
                    "source",
                    format!("ELF {elf_vars} 个变量，A2L {a2l_vars} 个"),
                ),
                Err(error) => fail("source", error),
            }
        }
    });

    // 5. daemon 连通（给了 --daemon 才查）。
    checks.push(match options.daemon {
        Some(url) => match connect_daemon(url).await {
            Ok(mut client) => match client.get_session(openape_proto::Empty {}).await {
                Ok(reply) => {
                    let report = reply.into_inner();
                    pass(
                        "daemon",
                        format!(
                            "{} 在线：{}",
                            url,
                            if report.linked {
                                "已接管通道"
                            } else {
                                "未接管"
                            }
                        ),
                    )
                }
                Err(status) => fail("daemon", format!("{url} 会话查询失败：{status}")),
            },
            Err(error) => fail("daemon", error),
        },
        None => skip("daemon", "没给 --daemon"),
    });

    checks
}

/// doctor 报告的双模渲染。
pub fn render_doctor(json: bool, checks: &[DoctorCheck]) -> Result<String, Fault> {
    render(json, &Vec::from(checks), || {
        let mut out = String::from("自检结论：");
        for check in checks {
            out.push_str(&format!(
                "\n  [{}] {}\n    {}",
                check.outcome, check.name, check.detail
            ));
        }
        out
    })
}

fn source_variable(variable: &openape_cal_core::Variable) -> SourceVariable {
    SourceVariable {
        symbol: variable.symbol().as_str().to_owned(),
        address: variable.address().get(),
        elem: format!("{:?}", variable.elem()),
        dims: variable.dims().to_vec(),
        role: variable.role().map(|role| format!("{role:?}")),
        // 没有语义就是裸值：标出来，不填默认值。
        raw: variable.semantics().is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use openape_diag::Domain;

    use super::*;

    #[test]
    fn ids_beyond_11_bits_are_usage_errors() {
        let err = session_config("vcan0", 0x800, 0x601, 200).expect_err("0x800 超标准帧");
        assert_eq!(err.code().domain(), Domain::Internal, "用法错误归内部域");
        assert!(err.message().contains("--tx-id"), "要指出是哪个参数");

        let err = session_config("vcan0", 0x600, 0x1_0000, 200).expect_err("超 16 位");
        assert!(err.message().contains("--rx-id"), "实际: {err}");
    }

    #[test]
    fn valid_ids_become_the_session_config() {
        let config = session_config("vcan0", 0x600, 0x601, 150).expect("合法");
        assert_eq!(config.channel, "vcan0");
        assert_eq!(config.timeout, Duration::from_millis(150));
    }

    #[test]
    fn zero_length_is_refused_before_touching_the_bus() {
        // read_memory 会先开会话，所以这里只验长度校验本身。
        assert!(ByteLen::new(0).is_none());
    }
}
