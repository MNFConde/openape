//! master 侧 DAQ：把"要采集哪些内存块"翻成 ODT 分组，并把回来的 DTO 切回
//! 各块的值。
//!
//! 这里只放纯逻辑。发命令的顺序在 [`crate::can`] 里——把"怎么分组"和
//! "怎么对话"分开，前者可以脱离总线测。
//!
//! 一个 DTO 的载荷上限是 `maxDto - 1`：byte0 是绝对 ODT 编号。经典 CAN 上
//! 就是 7 字节，所以两个 4 字节信号必须分两个 ODT，不能挤在一起。

use std::collections::BTreeMap;

use openape_cal_core::{Address, ByteLen};
use openape_diag::{Fault, Severity};

use crate::connection::{self, seq};

/// 一个要采集的内存块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaqSignal {
    /// 首地址。
    pub address: Address,
    /// 字节数。
    pub len: ByteLen,
}

/// 一条 DAQ list 的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaqListSpec {
    /// 事件通道号。同一通道上的 list 会被同一批 tick 驱动。
    pub event: u16,
    /// 采样预分频：每 N 个事件发一次。
    pub prescaler: u8,
    /// 采集哪些块，顺序即 DTO 内的字节顺序。
    pub signals: Vec<DaqSignal>,
    /// 要求 DTO 带从站时间戳（SET_DAQ_LIST_MODE 的 timestamp 位）。
    /// 对端能力里的强约束：XCPlite 的实现强制要这一位，不带就拒配。
    /// DTO 布局由对端说了算，master 的解码按它呈现的头适配。
    pub timestamp: bool,
    /// 条目的地址扩展（WRITE_DAQ 的 ext 字节）。list 内必须一致——
    /// 这是参考实现的硬约束。0 在多数真 ECU 上是绝对寻址；XCPlite
    /// 的默认配置里 0 是标定段相对，绝对寻址用 0x01。
    pub address_extension: u8,
}

impl DaqListSpec {
    /// 建一条 list 配置。信号不能为空——空 list 只会让从站空转。
    pub fn new(event: u16, prescaler: u8, signals: Vec<DaqSignal>) -> Option<Self> {
        if signals.is_empty() || prescaler == 0 {
            return None;
        }
        Some(Self {
            event,
            prescaler,
            signals,
            timestamp: false,
            address_extension: 0,
        })
    }

    /// 要求 DTO 带从站时间戳。
    pub fn with_timestamp(mut self) -> Self {
        self.timestamp = true;
        self
    }

    /// 声明条目的地址扩展。
    pub fn with_address_extension(mut self, ext: u8) -> Self {
        self.address_extension = ext;
        self
    }
}

/// 一个 ODT 里的一段字节：属于哪个信号、从 DTO 载荷的第几字节开始。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slice {
    offset: usize,
    len: ByteLen,
}

/// 配置成功后的布局：绝对 ODT 编号 → 该 ODT 的切片表。
///
/// 这张表是 master 自己写下去的条目的镜像，所以它比从站更早知道每个字节
/// 属于哪个信号。
///
/// 另存「每条 DAQ list 的首个绝对编号」（[`Self::insert_list`]）：私有头的
/// 从站（D-27 的 `PidKind::RelativeToDaqList`）发的是 list 内相对号，靠它
/// 才能还原成绝对号。标准从站的 DTO 自带绝对号，这张表不参与。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaqLayout {
    odts: BTreeMap<u8, Vec<Slice>>,
    list_first: BTreeMap<u16, u8>,
}

impl DaqLayout {
    /// 登记一个 ODT：按信号顺序给出各段字节数。重复编号会覆盖——编号由
    /// [`openape_xcp_proto::OdtIds`] 统一分配，重复说明调用方算错了。
    pub fn insert_odt(&mut self, pid: u8, lens: &[ByteLen]) {
        let mut offset = 0;
        let slices = lens
            .iter()
            .map(|len| {
                let slice = Slice { offset, len: *len };
                offset += usize::from(len.get());
                slice
            })
            .collect();
        self.odts.insert(pid, slices);
    }

    /// 登记一条 DAQ list 的首个绝对 ODT 编号。
    pub fn insert_list(&mut self, daq_list: u16, first_odt: u8) {
        self.list_first.insert(daq_list, first_odt);
    }

    /// 把「list 内相对编号」还原成绝对 ODT 编号。
    ///
    /// list 没登记、或算出来的编号越过 [`openape_xcp_proto::PID_DAQ_MAX`]
    /// （0xFC 起被命令与响应占用）时返回 `None`——不猜、也不截断。
    pub fn absolute_pid(&self, daq_list: u16, relative: u8) -> Option<u8> {
        let first = *self.list_first.get(&daq_list)?;
        let absolute = u16::from(first) + u16::from(relative);
        let absolute = u8::try_from(absolute).ok()?;
        (absolute <= openape_xcp_proto::PID_DAQ_MAX).then_some(absolute)
    }

    /// 从信号表登记，等价于按长度调用 [`Self::insert_odt`]。
    pub(crate) fn insert(&mut self, pid: u8, signals: &[DaqSignal]) {
        let lens: Vec<ByteLen> = signals.iter().map(|signal| signal.len).collect();
        self.insert_odt(pid, &lens);
    }

    /// 按绝对 ODT 编号切一帧 DTO。编号不认识或载荷不足时返回 `None`。
    pub fn decode<'a>(&self, pid: u8, payload: &'a [u8]) -> Option<Vec<&'a [u8]>> {
        let slices = self.odts.get(&pid)?;
        slices
            .iter()
            .map(|slice| {
                let end = slice.offset + usize::from(slice.len.get());
                payload.get(slice.offset..end)
            })
            .collect::<Option<Vec<_>>>()
    }

    /// 已登记的 ODT 个数。
    pub fn len(&self) -> usize {
        self.odts.len()
    }

    /// 是否一个 ODT 都没登记。
    pub fn is_empty(&self) -> bool {
        self.odts.is_empty()
    }

    /// 某个 ODT 承载的信号数。
    pub fn signal_count(&self, pid: u8) -> Option<usize> {
        self.odts.get(&pid).map(Vec::len)
    }
}

/// 把信号按 DTO 载荷上限切进 ODT。
///
/// 顺序保持不变：从站按写入顺序拼字节，master 按同样顺序切回去，两边都靠
/// "顺序"对齐，不靠猜。
pub(crate) fn plan_odts(
    signals: &[DaqSignal],
    payload_limit: u8,
) -> Result<Vec<Vec<DaqSignal>>, Fault> {
    if payload_limit == 0 {
        return Err(connection::fault(
            seq::DAQ,
            Severity::Error,
            "从站报的 maxDto 小于 2，一帧装不下任何 DAQ 数据",
        ));
    }
    let mut odts: Vec<Vec<DaqSignal>> = Vec::new();
    let mut current: Vec<DaqSignal> = Vec::new();
    let mut used = 0usize;

    for signal in signals {
        let width = usize::from(signal.len.get());
        if width > usize::from(payload_limit) {
            return Err(connection::fault(
                seq::DAQ,
                Severity::Error,
                format!(
                    "{} 处的信号有 {width} 字节，超过单个 ODT 的 {payload_limit} 字节上限",
                    signal.address
                ),
            )
            .with_hint("拆成多次读取，或改用 CAN-FD 提高单帧容量"));
        }
        if used + width > usize::from(payload_limit) {
            odts.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(*signal);
        used += width;
    }
    if !current.is_empty() {
        odts.push(current);
    }
    Ok(odts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(address: u32, len: u16) -> DaqSignal {
        DaqSignal {
            address: Address::new(address),
            len: ByteLen::new(len).expect("测试长度非零"),
        }
    }

    #[test]
    fn signals_fill_an_odt_until_the_payload_limit() {
        // 上限 7：两个 2 字节信号只占 4，再加一个就 6；第四个会溢出开新 ODT
        let signals = vec![signal(0x1000, 2), signal(0x1002, 2), signal(0x1004, 2)];
        let odts = plan_odts(&signals, 7).expect("都放得下");
        assert_eq!(odts.len(), 1, "6 字节只用一个 ODT");

        let four = vec![
            signal(0x1000, 2),
            signal(0x1002, 2),
            signal(0x1004, 2),
            signal(0x1006, 2),
        ];
        let odts = plan_odts(&four, 7).expect("拆开也放得下");
        assert_eq!(odts.len(), 2, "8 字节要两个 ODT");
        assert_eq!(odts[0].len(), 3);
        assert_eq!(odts[1].len(), 1);
    }

    #[test]
    fn signals_that_exactly_fill_an_odt_stay_together() {
        // 4 + 3 = 7 正好装满，切换条件是「超过」而不是「达到」
        let odts = plan_odts(&[signal(0x1000, 4), signal(0x1004, 3)], 7).expect("放得下");
        assert_eq!(odts.len(), 1, "正好装满不该被切开");
    }

    #[test]
    fn the_new_odt_starts_empty_not_at_one() {
        // 4+3 装满第一组后，第二组从 0 开始算才有 4+3 又装满
        let signals = vec![
            signal(0x1000, 4),
            signal(0x1004, 3),
            signal(0x1008, 4),
            signal(0x100c, 3),
        ];
        let odts = plan_odts(&signals, 7).expect("放得下");
        assert_eq!(odts.len(), 2, "8 字节不该拆成三段");
        assert_eq!((odts[0].len(), odts[1].len()), (2, 2));
    }

    #[test]
    fn a_signal_wider_than_the_limit_is_rejected() {
        let err = plan_odts(&[signal(0x2000, 8)], 7).expect_err("8 字节装不进 7");
        let text = err.to_string();
        assert!(text.contains("0x00002000"), "{text}");
        assert!(text.contains("7"), "{text}");
    }

    #[test]
    fn degenerate_payload_limit_is_rejected() {
        assert!(plan_odts(&[signal(0x1000, 1)], 0).is_err());
    }

    #[test]
    fn layout_slices_each_signal_by_its_offset() {
        let signals = vec![signal(0x1000, 2), signal(0x1002, 4)];
        let mut layout = DaqLayout::default();
        layout.insert(0, &signals);
        // DTO 载荷：PID 之后是 counter(2) + sine(4)
        let payload = [0xA0, 0x00, 0xC8, 0x00, 0x01, 0x02];
        let parts = layout.decode(0, &payload).expect("编号 0 认识");
        assert_eq!(parts, vec![&payload[0..2], &payload[2..6]]);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout.signal_count(0), Some(2));
    }

    #[test]
    fn layout_reports_unknown_pid_and_short_payload() {
        let mut layout = DaqLayout::default();
        layout.insert(3, &[signal(0x1000, 4)]);
        assert!(layout.decode(0, &[0; 4]).is_none(), "没登记过的编号不猜");
        assert!(layout.decode(3, &[0; 3]).is_none(), "载荷不足要报，不补零");
        assert!(!layout.is_empty());
        assert!(DaqLayout::default().is_empty());
    }

    #[test]
    fn relative_pids_resolve_through_the_list_first_odt() {
        // 私有头的从站发 list 内相对号；绝对号 = 该 list 首号 + 相对号。
        let mut layout = DaqLayout::default();
        layout.insert_list(0, 4);
        layout.insert(4, &[signal(0x1000, 2)]);
        layout.insert(5, &[signal(0x1002, 2)]);
        assert_eq!(layout.absolute_pid(0, 0), Some(4));
        assert_eq!(layout.absolute_pid(0, 1), Some(5));
        assert_eq!(layout.absolute_pid(1, 0), None, "没登记的 list 不猜");
        assert_eq!(
            layout.absolute_pid(0, 0xF8),
            None,
            "4 + 0xF8 落到 0xFC，已经是命令区间"
        );
    }

    #[test]
    fn relative_pids_that_would_leave_the_daq_range_are_refused() {
        let mut layout = DaqLayout::default();
        layout.insert_list(0, 0xFB);
        assert_eq!(layout.absolute_pid(0, 0), Some(0xFB));
        assert_eq!(
            layout.absolute_pid(0, 1),
            None,
            "0xFC 起是命令与响应，不能当 DTO 编号用"
        );
    }

    #[test]
    fn list_spec_refuses_empty_or_zero_prescaler() {
        assert!(DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).is_some());
        assert!(
            DaqListSpec::new(0, 1, Vec::new()).is_none(),
            "空 list 只会空转"
        );
        assert!(
            DaqListSpec::new(0, 0, vec![signal(0x1000, 2)]).is_none(),
            "预分频 0 无意义"
        );
    }
}
