//! `write` / `page` / `dataset` 三条命令的实现。
//!
//! 三者都遵守 D-18 的纪律：一条命令一次自足事务、可直接连设备或经 daemon、
//! 输出双模、退出码按故障域分段。
//!
//! 写入报告一定包含三样：写前值、量程结论、写后值。少任何一样，用户都无法
//! 判断"这次写发生了什么"——写前与写后值相同也是一种信息（地址可能写错）。

use std::path::{Path, PathBuf};

use openape_cal_core::{
    Dataset, Difference, SourceMode, Symbol, Variable, VariableTree, diff, hex,
};
use openape_diag::{Domain, Fault, FaultCode, Severity};
use openape_session::{Session, WriteContext, read_snapshot};

use crate::commands::{connect_daemon, session_config};
use crate::output::render;
use crate::usage_fault;

/// 直连时的会话参数。
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// CAN 通道。
    pub channel: String,
    /// 命令帧标识符。
    pub tx_id: u32,
    /// 响应帧标识符。
    pub rx_id: u32,
    /// 单次等待响应毫秒数。
    pub timeout_ms: u64,
}

/// 一条 `write` 命令的参数。
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// 会话参数；经 daemon 时只用到通道以外的部分。
    pub session: SessionOptions,
    /// 符号名。没有符号时按地址写 raw。
    pub symbol: Option<String>,
    /// 地址。有符号时用作一致性校验。
    pub address: Option<u32>,
    /// 原样字节（十六进制）。
    pub data: Option<String>,
    /// 物理值，按变量的元素类型与换算编码。
    pub value: Option<f64>,
    /// ELF 描述文件。
    pub elf: Option<PathBuf>,
    /// A2L 描述文件。给了它才算带语义档。
    pub a2l: Option<PathBuf>,
    /// 审计与快照目录。
    pub state_dir: PathBuf,
    /// 谁写入的，进审计。
    pub actor: String,
}

/// 写入报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WriteReport {
    /// 通道名。
    pub channel: String,
    /// 写的哪个标定量。
    pub symbol: String,
    /// 落在哪个地址。
    pub address: String,
    /// 写前读到的字节，空格分隔的十六进制。
    pub before_hex: String,
    /// 写后读回的字节。
    pub after_hex: String,
    /// 写前快照文件路径。
    pub snapshot_path: String,
    /// 是否做过量程校验。`false` 时界面标 `[raw]`。
    pub range_checked: bool,
    /// 量程下限，未校验时为空。
    pub range_min: Option<f64>,
    /// 量程上限，未校验时为空。
    pub range_max: Option<f64>,
    /// 本次写入的物理值，未校验时为空。
    pub physical: Option<f64>,
    /// 审计时刻（ISO-8601 UTC）。
    pub audited_at: String,
}

impl WriteReport {
    /// 人类可读形态：写前值、量程结论、写后值、留痕位置，四行都不可省。
    pub fn to_human(&self) -> String {
        let range = if self.range_checked {
            format!(
                "量程 {}..={} 通过（物理值 {}）",
                self.range_min.unwrap_or_default(),
                self.range_max.unwrap_or_default(),
                self.physical.unwrap_or_default()
            )
        } else {
            "[raw] 无量程信息，未校验".to_owned()
        };
        format!(
            "写入 {} {}（{}）\n  写前 {}\n  {}\n  写后 {}\n  快照 {}\n  审计 {}",
            self.symbol,
            self.address,
            self.channel,
            self.before_hex,
            range,
            self.after_hex,
            self.snapshot_path,
            self.audited_at
        )
    }
}

/// 一条差异的对外形状。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DifferenceRow {
    /// 符号名。
    pub symbol: String,
    /// 地址。
    pub address: String,
    /// 左侧字节；空串表示左侧没有这条。
    pub left_hex: String,
    /// 右侧字节；空串表示右侧没有这条。
    pub right_hex: String,
}

impl DifferenceRow {
    fn of(difference: &Difference) -> Self {
        Self {
            symbol: difference.symbol().to_string(),
            address: difference.address().to_string(),
            left_hex: difference.left().map_or_else(String::new, hex_spaced),
            right_hex: difference.right().map_or_else(String::new, hex_spaced),
        }
    }
}

/// 描述文件里的变量树与输入档。两者同源，不分开解析。
fn descriptors(options: &WriteOptions) -> (Option<VariableTree>, SourceMode) {
    match openape_assembly::load_variables(options.elf.as_deref(), options.a2l.as_deref()) {
        Some(loaded) => (loaded.tree, loaded.mode),
        None => (None, SourceMode::ElfOnly),
    }
}

/// 把用户给的值变成字节与目标变量。
fn resolve(
    tree: Option<&VariableTree>,
    options: &WriteOptions,
) -> Result<(Variable, Vec<u8>), Fault> {
    let named = match options.symbol.as_deref() {
        Some(name) => {
            let symbol = Symbol::new(name.trim())
                .ok_or_else(|| usage_fault(format!("符号 {name:?} 不合法")))?;
            let tree = tree
                .ok_or_else(|| usage_fault("按符号写需要描述文件：给 --elf 或 --a2l".to_owned()))?;
            Some(
                tree.get(&symbol)
                    .cloned()
                    .ok_or_else(|| usage_fault(format!("描述文件里没有 {symbol}")))?,
            )
        }
        None => None,
    };

    let (data, address) = match (&options.data, options.value) {
        (Some(text), None) => {
            let data = parse_hex_bytes(text)?;
            let address = options
                .address
                .or_else(|| named.as_ref().map(|variable| variable.address().get()))
                .ok_or_else(|| usage_fault("没有 --symbol 时必须给 --address".to_owned()))?;
            (data, address)
        }
        (None, Some(value)) => {
            let variable = named.as_ref().ok_or_else(|| {
                usage_fault("给 --value 需要 --symbol 与描述文件：按地址猜类型等于猜".to_owned())
            })?;
            let data = openape_cal_core::value::from_physical(
                value,
                variable.elem(),
                variable.byte_order(),
                variable.semantics(),
            )
            .ok_or_else(|| {
                usage_fault(format!(
                    "{value} 编不成 {} 的字节：超出类型范围或换算不可逆",
                    variable.symbol()
                ))
            })?;
            (data, variable.address().get())
        }
        (Some(_), Some(_)) => {
            return Err(usage_fault(
                "--data 与 --value 只能给一个：给两个无法判断以哪个为准".to_owned(),
            ));
        }
        (None, None) => {
            return Err(usage_fault(
                "要么给 --value（按符号编码），要么给 --data（原样字节）".to_owned(),
            ));
        }
    };
    if data.is_empty() {
        return Err(usage_fault(
            "写入数据为空：写 0 字节不会改变任何东西".to_owned(),
        ));
    }

    let target = match named {
        Some(variable) => {
            if let Some(address) = options.address
                && address != variable.address().get()
            {
                return Err(usage_fault(format!(
                    "{} 的首地址是 {}，与 --address 0x{address:08x} 不符",
                    variable.symbol(),
                    variable.address()
                )));
            }
            if variable.byte_len().map(|len| len.get() as usize) != Some(data.len()) {
                return Err(usage_fault(format!(
                    "{} 是 {:?} 字节，给了 {} 字节",
                    variable.symbol(),
                    variable.byte_len().map(|len| len.get()),
                    data.len()
                )));
            }
            variable
        }
        None => raw_variable(address, data.len())?,
    };
    Ok((target, data))
}

fn raw_variable(address: u32, len: usize) -> Result<Variable, Fault> {
    let symbol = Symbol::new("raw").ok_or_else(|| usage_fault("符号不合法".to_owned()))?;
    let dims = u32::try_from(len).map_err(|_| usage_fault("写入长度过大".to_owned()))?;
    Variable::new(
        symbol,
        openape_cal_core::Address::new(address),
        openape_cal_core::ElemType::U8,
    )
    .with_dims(vec![dims])
    .ok_or_else(|| usage_fault("写入长度不合法".to_owned()))
}

/// `write`：直连设备单点写。
pub async fn write_direct(options: &WriteOptions, json: bool) -> Result<String, Fault> {
    let (tree, mode) = descriptors(options);
    let (target, data) = resolve(tree.as_ref(), options)?;
    let config = session_config(
        &options.session.channel,
        options.session.tx_id,
        options.session.rx_id,
        options.session.timeout_ms,
    )?;
    let mut session = Session::open(config)?;
    session.connect(0).await?;
    let context = WriteContext::open(&options.state_dir, &options.actor, mode)?;
    let written = session.write_one(&target, &data, &context).await?;
    let channel = session.channel().to_owned();
    session.disconnect().await.ok();
    let (range_checked, min, max, physical) = match written.range {
        openape_session::RangeVerdict::Within { physical, min, max } => {
            (true, Some(min), Some(max), Some(physical))
        }
        openape_session::RangeVerdict::Unchecked => (false, None, None, None),
    };
    let report = WriteReport {
        channel,
        symbol: written.symbol.to_string(),
        address: written.address.to_string(),
        before_hex: hex_spaced(&written.before),
        after_hex: hex_spaced(&written.after),
        snapshot_path: written.snapshot.display().to_string(),
        range_checked,
        range_min: min,
        range_max: max,
        physical,
        audited_at: written.audit.at.to_string(),
    };
    let human = report.to_human();
    render(json, &report, || human)
}

/// `write`：经 daemon 代写。
pub async fn write_via_daemon(
    url: &str,
    options: &WriteOptions,
    json: bool,
) -> Result<String, Fault> {
    let (tree, _) = descriptors(options);
    let (target, data) = resolve(tree.as_ref(), options)?;
    let mut client = connect_daemon(url).await?;
    let response = client
        .write_memory(openape_proto::WriteRequest {
            address: target.address().to_string(),
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            data_hex: hex::encode(&data),
            symbol: options.symbol.clone().unwrap_or_default(),
            actor: options.actor.clone(),
        })
        .await
        .map_err(daemon_fault)?
        .into_inner();
    let range = response.range;
    let report = WriteReport {
        channel: response.channel,
        symbol: response.symbol,
        address: response.address,
        before_hex: response.before_hex,
        after_hex: response.after_hex,
        snapshot_path: response.snapshot_path,
        range_checked: response.range_checked,
        range_min: range.as_ref().map(|range| range.min),
        range_max: range.as_ref().map(|range| range.max),
        physical: range.map(|range| range.physical),
        audited_at: response.audited_at,
    };
    let human = report.to_human();
    render(json, &report, || human)
}

/// `page get`。
pub async fn page_get(
    options: &SessionOptions,
    url: Option<&str>,
    json: bool,
) -> Result<String, Fault> {
    let (channel, page) = match url {
        Some(url) => {
            let mut client = connect_daemon(url).await?;
            let response = client
                .get_cal_page(openape_proto::Empty {})
                .await
                .map_err(daemon_fault)?
                .into_inner();
            (
                response.channel,
                u8::try_from(response.page).unwrap_or(u8::MAX),
            )
        }
        None => {
            let mut session = open_direct(options).await?;
            let page = session.cal_page().await?;
            let channel = session.channel().to_owned();
            session.disconnect().await.ok();
            (channel, page)
        }
    };
    render_page(json, &channel, page)
}

/// `page set`。
pub async fn page_set(
    options: &SessionOptions,
    url: Option<&str>,
    page: u8,
    json: bool,
) -> Result<String, Fault> {
    let (channel, actual) = match url {
        Some(url) => {
            let mut client = connect_daemon(url).await?;
            let response = client
                .set_cal_page(openape_proto::SetCalPageRequest {
                    page: u32::from(page),
                })
                .await
                .map_err(daemon_fault)?
                .into_inner();
            (
                response.channel,
                u8::try_from(response.page).unwrap_or(u8::MAX),
            )
        }
        None => {
            let mut session = open_direct(options).await?;
            session.set_cal_page(page).await?;
            let actual = session.cal_page().await?;
            let channel = session.channel().to_owned();
            session.disconnect().await.ok();
            (channel, actual)
        }
    };
    render_page(json, &channel, actual)
}

/// `page diff`：只列有差异的地址。
pub async fn page_diff(
    options: &WriteOptions,
    url: Option<&str>,
    reference: u8,
    working: u8,
    json: bool,
) -> Result<String, Fault> {
    let (tree, mode) = descriptors(options);
    let differences: Vec<DifferenceRow> = match url {
        Some(url) => {
            let mut client = connect_daemon(url).await?;
            client
                .diff_pages(openape_proto::PageDiffRequest {
                    reference_page: u32::from(reference),
                    working_page: u32::from(working),
                })
                .await
                .map_err(daemon_fault)?
                .into_inner()
                .differences
                .iter()
                .map(|difference| DifferenceRow {
                    symbol: difference.symbol.clone(),
                    address: difference.address.clone(),
                    left_hex: difference.reference_hex.clone(),
                    right_hex: difference.working_hex.clone(),
                })
                .collect()
        }
        None => {
            let tree = tree
                .ok_or_else(|| usage_fault("页对比需要描述文件：给 --elf 或 --a2l".to_owned()))?;
            let mut session = open_direct(&options.session).await?;
            let differences = session.diff_pages(&tree, reference, working, mode).await?;
            session.disconnect().await.ok();
            differences.iter().map(DifferenceRow::of).collect()
        }
    };
    render_diff(
        json,
        &format!("页 {reference} 与页 {working}"),
        &format!("页 {reference}"),
        &format!("页 {working}"),
        &differences,
    )
}

/// `dataset diff`：比较两份 dataset 文件。
pub fn dataset_diff(left: &Path, right: &Path, json: bool) -> Result<String, Fault> {
    let left_dataset = load_dataset(left)?;
    let right_dataset = load_dataset(right)?;
    let differences = diff(&left_dataset, &right_dataset).map_err(|err| {
        usage_fault(err.to_string()).with_hint("两档 dataset（仅 ELF 与带 A2L）不可混比")
    })?;
    let rows: Vec<DifferenceRow> = differences.iter().map(DifferenceRow::of).collect();
    render_diff(
        json,
        &format!("{} 与 {}", left.display(), right.display()),
        &left.display().to_string(),
        &right.display().to_string(),
        &rows,
    )
}

/// `dataset restore`：把快照逐项写回 ECU，同样留痕。
pub async fn dataset_restore(
    snapshot: &Path,
    options: &WriteOptions,
    json: bool,
) -> Result<String, Fault> {
    let dataset = read_snapshot(snapshot)?;
    let (_, mode) = descriptors(options);
    if dataset.mode() != mode {
        return Err(usage_fault(format!(
            "快照是{}档，当前是{}档：两档数值不可混写",
            dataset.mode().label(),
            mode.label()
        )));
    }
    let config = session_config(
        &options.session.channel,
        options.session.tx_id,
        options.session.rx_id,
        options.session.timeout_ms,
    )?;
    let mut session = Session::open(config)?;
    session.connect(0).await?;
    let context = WriteContext::open(&options.state_dir, &options.actor, mode)?;
    let written = session.restore_snapshot(&dataset, &context).await?;
    let channel = session.channel().to_owned();
    session.disconnect().await.ok();

    #[derive(serde::Serialize)]
    struct RestoreReport {
        channel: String,
        snapshot: String,
        restored: usize,
        rows: Vec<String>,
    }
    let rows: Vec<String> = written
        .iter()
        .map(|entry| {
            format!(
                "  {} {} → {}",
                entry.address,
                hex_spaced(&entry.before),
                hex_spaced(&entry.after)
            )
        })
        .collect();
    let human = format!(
        "{channel} 已按 {} 回滚 {} 项\n{}",
        snapshot.display(),
        written.len(),
        rows.join("\n")
    );
    render(
        json,
        &RestoreReport {
            channel,
            snapshot: snapshot.display().to_string(),
            restored: written.len(),
            rows,
        },
        || human,
    )
}

/// 页号报告的双模渲染。
fn render_page(json: bool, channel: &str, page: u8) -> Result<String, Fault> {
    #[derive(serde::Serialize)]
    struct PageReport<'a> {
        channel: &'a str,
        page: u8,
    }
    render(json, &PageReport { channel, page }, || {
        format!("{channel} 当前标定页 {page}")
    })
}

/// 差异报告的双模渲染。空差异要说清"两侧一致"，不是"没查"。
fn render_diff(
    json: bool,
    subject: &str,
    left_label: &str,
    right_label: &str,
    rows: &[DifferenceRow],
) -> Result<String, Fault> {
    #[derive(serde::Serialize)]
    struct DiffReport<'a> {
        subject: &'a str,
        left: &'a str,
        right: &'a str,
        differences: &'a [DifferenceRow],
    }
    render(
        json,
        &DiffReport {
            subject,
            left: left_label,
            right: right_label,
            differences: rows,
        },
        || {
            if rows.is_empty() {
                return format!("{subject} 一致，无差异");
            }
            let mut out = format!("{subject} 差异 {} 处", rows.len());
            for row in rows {
                out.push_str(&format!(
                    "\n  {} {} 左={} 右={}",
                    row.address, row.symbol, row.left_hex, row.right_hex
                ));
            }
            out
        },
    )
}

fn open_session_from(options: &SessionOptions) -> Result<Session, Fault> {
    let config = session_config(
        &options.channel,
        options.tx_id,
        options.rx_id,
        options.timeout_ms,
    )?;
    Session::open(config)
}

async fn open_direct(options: &SessionOptions) -> Result<Session, Fault> {
    let mut session = open_session_from(options)?;
    session.connect(0).await?;
    Ok(session)
}

fn load_dataset(path: &Path) -> Result<Dataset, Fault> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| usage_fault(format!("读不了 {}：{err}", path.display())))?;
    Dataset::from_json(&text).map_err(|err| usage_fault(err.to_string()))
}

fn parse_hex_bytes(text: &str) -> Result<Vec<u8>, Fault> {
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    hex::decode(&compact).ok_or_else(|| usage_fault(format!("{text:?} 不是十六进制字节串")))
}

fn hex_spaced(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// daemon 侧的失败归会话域：设备在 daemon 手里。
fn daemon_fault(message: impl ToString) -> Fault {
    Fault::new(
        FaultCode::new(Domain::Session, 4).expect("序号 4 合法"),
        Severity::Error,
        message.to_string(),
    )
}
