//! 从站侧 DAQ 状态机：配置的解析与周期出帧的计划。
//!
//! 纯状态机，不碰总线也不碰内存——出几帧、每帧是谁，由这里定；具体字节在
//! [`crate::SimEcu`] 里拼。这样"配置命令的语义"可以单测，不用起 vcan。
//!
//! 三条与真从站对齐的行为（M0-2 实测，见 `cairn/xcp-on-can.md`）：
//!
//! 1. **绝对 ODT 编号跨 list 连续**：`ALLOC_ODT` 时就把号占掉，
//!    `START_STOP_DAQ_LIST` 只把已占的号报回去。
//! 2. **`WRITE_DAQ` 写后指针自增**：master 连续写多条时只发一次
//!    `SET_DAQ_PTR`。
//! 3. DAQ 运行期间拒绝改配置，回 `DAQ_ACTIVE`——改到一半的数据比不采更糟。

use openape_xcp_proto::{ErrorCode, PID_DAQ_MAX};

/// 一条 ODT 条目：从 `address` 取 `size` 字节放进 DTO。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OdtEntry {
    /// 位偏移。MVP 只支持整字节，非零值在写入时被拒。
    pub bit_offset: u8,
    /// 字节数。
    pub size: u8,
    /// 地址扩展。
    pub address_extension: u8,
    /// 源地址。
    pub address: u32,
}

/// 一条 DAQ list。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaqList {
    odts: Vec<Vec<OdtEntry>>,
    first_pid: u8,
    event: u16,
    prescaler: u8,
    running: bool,
    ticks: u32,
}

impl DaqList {
    /// 首个绝对 ODT 编号。
    pub fn first_pid(&self) -> u8 {
        self.first_pid
    }

    /// ODT 个数。
    pub fn odt_count(&self) -> usize {
        self.odts.len()
    }

    /// 是否正在发送。
    pub fn is_running(&self) -> bool {
        self.running
    }
}

/// 一帧要发的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlan {
    /// 绝对 ODT 编号，即 DTO 的 byte0。
    pub pid: u8,
    /// 该 ODT 的条目，顺序即 DTO 内的字节顺序。
    pub entries: Vec<OdtEntry>,
}

/// DAQ 配置与运行状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaqState {
    lists: Vec<DaqList>,
    next_pid: u16,
    cursor: Option<(u16, u8, u8)>,
}

impl DaqState {
    /// 释放全部配置。
    pub fn free(&mut self) {
        *self = Self::default();
    }

    /// 分配 `count` 条 list。
    pub fn alloc_lists(&mut self, count: u16) -> Result<(), ErrorCode> {
        if self.any_running() {
            return Err(ErrorCode::DaqActive);
        }
        self.lists = vec![DaqList::default(); usize::from(count)];
        Ok(())
    }

    /// 给 list 分配 `odt_count` 个 ODT，并占掉它们的绝对编号。
    pub fn alloc_odts(&mut self, daq_list: u16, odt_count: u8) -> Result<(), ErrorCode> {
        if self.any_running() {
            return Err(ErrorCode::DaqActive);
        }
        let first = u8::try_from(self.next_pid)
            .ok()
            .filter(|pid| *pid <= PID_DAQ_MAX)
            .ok_or(ErrorCode::OutOfRange)?;
        let last = self.next_pid + u16::from(odt_count);
        if last > u16::from(PID_DAQ_MAX) + 1 {
            return Err(ErrorCode::OutOfRange);
        }
        self.next_pid = last;
        let list = self.list_mut(daq_list)?;
        list.first_pid = first;
        list.odts = vec![Vec::new(); usize::from(odt_count)];
        Ok(())
    }

    /// 给 ODT 预分配条目槽。
    pub fn alloc_entries(&mut self, daq_list: u16, odt: u8, entries: u8) -> Result<(), ErrorCode> {
        if self.any_running() {
            return Err(ErrorCode::DaqActive);
        }
        let slots = self.odt_mut(daq_list, odt)?;
        slots.clear();
        slots.reserve(usize::from(entries));
        Ok(())
    }

    /// 设置写游标。
    pub fn set_ptr(&mut self, daq_list: u16, odt: u8, entry: u8) -> Result<(), ErrorCode> {
        self.odt(daq_list, odt)?;
        self.cursor = Some((daq_list, odt, entry));
        Ok(())
    }

    /// 在游标处写一条条目，随后游标自增。
    pub fn write_entry(&mut self, entry: OdtEntry) -> Result<(), ErrorCode> {
        if entry.bit_offset != 0 {
            return Err(ErrorCode::CmdSyntax);
        }
        if entry.size == 0 {
            return Err(ErrorCode::OutOfRange);
        }
        let (daq_list, odt, index) = self.cursor.ok_or(ErrorCode::CmdSyntax)?;
        let slots = self.odt_mut(daq_list, odt)?;
        let index = usize::from(index);
        if index < slots.len() {
            slots[index] = entry;
        } else {
            slots.push(entry);
        }
        self.cursor = Some((daq_list, odt, index.saturating_add(1) as u8));
        Ok(())
    }

    /// 设置 list 的模式。`prescaler` 为 0 时按 1 处理。
    pub fn set_mode(&mut self, daq_list: u16, event: u16, prescaler: u8) -> Result<(), ErrorCode> {
        let list = self.list_mut(daq_list)?;
        list.event = event;
        list.prescaler = prescaler.max(1);
        Ok(())
    }

    /// 启停一条 list，返回它的首个绝对 ODT 编号。
    pub fn start_stop_list(&mut self, daq_list: u16, start: bool) -> Result<u8, ErrorCode> {
        let list = self.list_mut(daq_list)?;
        list.running = start;
        list.ticks = 0;
        Ok(list.first_pid)
    }

    /// 同步启停全部 list。
    pub fn start_stop_synch(&mut self, start: bool) {
        for list in &mut self.lists {
            list.running = start;
            list.ticks = 0;
        }
    }

    /// 清空一条 list 的条目。
    pub fn clear_list(&mut self, daq_list: u16) -> Result<(), ErrorCode> {
        let list = self.list_mut(daq_list)?;
        for odt in &mut list.odts {
            odt.clear();
        }
        list.running = false;
        Ok(())
    }

    /// 推进一个事件 tick，返回本 tick 该发的帧。
    pub fn tick(&mut self) -> Vec<FramePlan> {
        let mut plans = Vec::new();
        for list in &mut self.lists {
            if !list.running {
                continue;
            }
            list.ticks += 1;
            if list.ticks < u32::from(list.prescaler) {
                continue;
            }
            list.ticks = 0;
            for (index, entries) in list.odts.iter().enumerate() {
                if entries.is_empty() {
                    continue;
                }
                plans.push(FramePlan {
                    pid: list.first_pid + index as u8,
                    entries: entries.clone(),
                });
            }
        }
        plans
    }

    /// 是否有 list 正在发送。
    pub fn any_running(&self) -> bool {
        self.lists.iter().any(DaqList::is_running)
    }

    /// 已分配的 list 条数。
    pub fn list_count(&self) -> usize {
        self.lists.len()
    }

    /// 查一条 list。
    pub fn list(&self, daq_list: u16) -> Option<&DaqList> {
        self.lists.get(usize::from(daq_list))
    }

    fn list_mut(&mut self, daq_list: u16) -> Result<&mut DaqList, ErrorCode> {
        self.lists
            .get_mut(usize::from(daq_list))
            .ok_or(ErrorCode::OutOfRange)
    }

    fn odt(&self, daq_list: u16, odt: u8) -> Result<&Vec<OdtEntry>, ErrorCode> {
        self.list(daq_list)
            .and_then(|list| list.odts.get(usize::from(odt)))
            .ok_or(ErrorCode::OutOfRange)
    }

    fn odt_mut(&mut self, daq_list: u16, odt: u8) -> Result<&mut Vec<OdtEntry>, ErrorCode> {
        self.list_mut(daq_list)?
            .odts
            .get_mut(usize::from(odt))
            .ok_or(ErrorCode::OutOfRange)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(address: u32, size: u8) -> OdtEntry {
        OdtEntry {
            bit_offset: 0,
            size,
            address_extension: 0,
            address,
        }
    }

    /// 配一条 list：两个 ODT，各一个两字节条目。
    fn configured() -> DaqState {
        let mut daq = DaqState::default();
        daq.alloc_lists(1).expect("分配 list");
        daq.alloc_odts(0, 2).expect("分配 ODT");
        daq.alloc_entries(0, 0, 1).expect("分配条目槽");
        daq.alloc_entries(0, 1, 1).expect("分配条目槽");
        daq.set_ptr(0, 0, 0).expect("设游标");
        daq.write_entry(entry(0x1000, 2)).expect("写条目");
        daq.set_ptr(0, 1, 0).expect("设游标");
        daq.write_entry(entry(0x1002, 2)).expect("写条目");
        daq.set_mode(0, 0, 1).expect("设模式");
        daq
    }

    #[test]
    fn odt_numbers_run_continuously_across_lists() {
        let mut daq = DaqState::default();
        daq.alloc_lists(2).expect("两条 list");
        daq.alloc_odts(0, 2).expect("list 0 两个 ODT");
        assert_eq!(daq.list(0).map(DaqList::first_pid), Some(0));
        daq.alloc_odts(1, 1).expect("list 1 一个 ODT");
        assert_eq!(daq.list(1).map(DaqList::first_pid), Some(2), "接着编号");
        assert_eq!(daq.start_stop_list(1, true), Ok(2), "启动时把号报回去");
    }

    #[test]
    fn running_lists_refuse_reconfiguration() {
        let mut daq = configured();
        daq.start_stop_list(0, true).expect("启动");
        assert_eq!(daq.alloc_lists(1), Err(ErrorCode::DaqActive));
        assert_eq!(daq.alloc_odts(0, 1), Err(ErrorCode::DaqActive));
        assert_eq!(daq.alloc_entries(0, 0, 1), Err(ErrorCode::DaqActive));
    }

    #[test]
    fn write_after_set_ptr_advances_the_cursor() {
        // pyXCP 只发一次 SET_DAQ_PTR 连着写多条，指针不自增就会覆盖
        let mut daq = DaqState::default();
        daq.alloc_lists(1).expect("分配 list");
        daq.alloc_odts(0, 1).expect("分配 ODT");
        daq.set_ptr(0, 0, 0).expect("设游标");
        daq.write_entry(entry(0x1000, 2)).expect("第一条");
        daq.write_entry(entry(0x1002, 2))
            .expect("第二条，游标已自增");
        let plans = {
            daq.start_stop_list(0, true).expect("启动");
            daq.tick()
        };
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].entries,
            vec![entry(0x1000, 2), entry(0x1002, 2)],
            "两条都在，没有互相覆盖"
        );
    }

    #[test]
    fn tick_sends_every_odt_of_running_lists() {
        let mut daq = configured();
        assert!(daq.tick().is_empty(), "没启动就不发");
        daq.start_stop_list(0, true).expect("启动");
        let plans = daq.tick();
        assert_eq!(plans.len(), 2, "两个 ODT 各一帧");
        assert_eq!(plans[0].pid, 0);
        assert_eq!(plans[1].pid, 1);
    }

    #[test]
    fn prescaler_divides_the_event_rate() {
        let mut daq = configured();
        daq.set_mode(0, 0, 3).expect("预分频 3");
        daq.start_stop_list(0, true).expect("启动");
        let mut rounds = Vec::new();
        for tick in 1..=6 {
            if !daq.tick().is_empty() {
                rounds.push(tick);
            }
        }
        // 发帧时机是断言的一部分：只数总数的话，"发完把计数置 1"这类
        // 偏移错误照样能凑出相同的轮数
        assert_eq!(rounds, vec![3, 6], "每 3 个 tick 发一轮");
    }

    #[test]
    fn zero_prescaler_is_treated_as_one() {
        let mut daq = configured();
        daq.set_mode(0, 0, 0).expect("预分频 0");
        daq.start_stop_list(0, true).expect("启动");
        assert_eq!(daq.tick().len(), 2, "每个 tick 都发");
    }

    #[test]
    fn synch_stops_every_list() {
        let mut daq = configured();
        daq.start_stop_list(0, true).expect("启动");
        daq.start_stop_synch(false);
        assert!(daq.tick().is_empty());
        assert!(!daq.any_running());
        daq.start_stop_synch(true);
        assert_eq!(daq.tick().len(), 2);
    }

    #[test]
    fn free_resets_every_number() {
        let mut daq = configured();
        daq.start_stop_list(0, true).expect("启动");
        daq.free();
        assert_eq!(daq.list_count(), 0);
        assert!(!daq.any_running());
        daq.alloc_lists(1).expect("重新分配");
        daq.alloc_odts(0, 1).expect("重新分配 ODT");
        assert_eq!(
            daq.list(0).map(DaqList::first_pid),
            Some(0),
            "编号从 0 重来"
        );
    }

    #[test]
    fn unknown_list_or_odt_is_rejected() {
        let mut daq = DaqState::default();
        assert_eq!(
            daq.alloc_odts(0, 1),
            Err(ErrorCode::OutOfRange),
            "没分配 list"
        );
        daq.alloc_lists(1).expect("分配 list");
        assert_eq!(
            daq.set_ptr(0, 0, 0),
            Err(ErrorCode::OutOfRange),
            "list 建了但还没分配 ODT"
        );
        assert_eq!(daq.clear_list(3), Err(ErrorCode::OutOfRange));
        assert_eq!(daq.set_mode(9, 0, 1), Err(ErrorCode::OutOfRange));
    }

    #[test]
    fn write_without_cursor_is_a_syntax_error() {
        let mut daq = configured();
        daq.free();
        assert_eq!(daq.write_entry(entry(0x1000, 2)), Err(ErrorCode::CmdSyntax));
    }

    #[test]
    fn bit_offsets_other_than_zero_are_rejected() {
        let mut daq = configured();
        let mut odd = entry(0x1000, 1);
        odd.bit_offset = 3;
        assert_eq!(daq.write_entry(odd), Err(ErrorCode::CmdSyntax));
    }

    #[test]
    fn zero_sized_entry_is_rejected() {
        let mut daq = configured();
        assert_eq!(
            daq.write_entry(entry(0x1000, 0)),
            Err(ErrorCode::OutOfRange)
        );
    }

    #[test]
    fn clear_list_drops_entries_and_stops_it() {
        let mut daq = configured();
        daq.start_stop_list(0, true).expect("启动");
        assert!(daq.list(0).expect("有 list").is_running());
        daq.clear_list(0).expect("清空");
        assert!(!daq.any_running(), "清空要把运行位一起清掉");
        assert!(daq.tick().is_empty(), "清空后不再发");
        assert_eq!(
            daq.list(0).map(DaqList::odt_count),
            Some(2),
            "ODT 还在，条目没了"
        );
    }

    #[test]
    fn restarting_resets_the_prescaler_counter() {
        let mut daq = configured();
        daq.set_mode(0, 0, 3).expect("预分频 3");
        daq.start_stop_list(0, true).expect("启动");
        assert!(daq.tick().is_empty(), "第 1 个 tick 还在计数");
        assert!(daq.tick().is_empty(), "第 2 个 tick 还在计数");
        daq.start_stop_list(0, false).expect("停");
        daq.start_stop_list(0, true).expect("重启");
        // 计数必须归零：若从 1 起算，第 2 个 tick 就会发
        assert!(daq.tick().is_empty(), "重启后第 1 个 tick 仍在计数");
        assert!(daq.tick().is_empty(), "重启后第 2 个 tick 仍在计数");
        assert!(!daq.tick().is_empty(), "第 3 个 tick 才发");
    }

    #[test]
    fn synch_restart_also_resets_the_counter() {
        let mut daq = configured();
        daq.set_mode(0, 0, 2).expect("预分频 2");
        daq.start_stop_synch(true);
        assert!(daq.tick().is_empty(), "第 1 个 tick 只计数");
        daq.start_stop_synch(false);
        daq.start_stop_synch(true);
        assert!(daq.tick().is_empty(), "同步重启后第 1 个 tick 仍在计数");
        assert!(!daq.tick().is_empty(), "第 2 个 tick 才发");
    }

    #[test]
    fn odt_numbers_are_exhausted_at_fc() {
        let mut daq = DaqState::default();
        daq.alloc_lists(2).expect("两条 list");
        daq.alloc_odts(0, 252).expect("占满 0x00 到 0xFB");
        assert_eq!(
            daq.alloc_odts(1, 1),
            Err(ErrorCode::OutOfRange),
            "0xFC 起是命令与响应的 PID，不能再当 ODT 编号"
        );
    }
}
