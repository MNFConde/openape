//! DBC 文本解析与信号取值。
//!
//! 只实现"总线帧 → 物理值"用得上的那部分语法：`BO_` 定义消息，`SG_`
//! 定义信号。其余行（`BA_`、`CM_`、`VAL_`、`SIG_VALTYPE_` 等）跳过并记
//! 一条提示——它们描述的多是显示与文档信息，不影响取值。

use std::collections::BTreeMap;

use crate::{ByteOrder, Error};

/// 一份 DBC。
#[derive(Debug, Clone, Default)]
pub struct Database {
    messages: BTreeMap<u32, Message>,
    skipped: Vec<String>,
}

impl Database {
    /// 按消息 id 查。
    pub fn message(&self, id: u32) -> Option<&Message> {
        self.messages.get(&id)
    }

    /// 全部消息，按 id 升序。
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.messages.values()
    }

    /// 解析时跳过的行（不认识的语句类别），去重后按出现顺序。
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }
}

/// 一条 CAN 消息。
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    id: u32,
    name: String,
    dlc: usize,
    signals: Vec<Signal>,
}

impl Message {
    /// 报文 id（11 位或 29 位，不含标志位）。
    pub fn id(&self) -> u32 {
        self.id
    }

    /// 报文名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 数据长度，字节。
    pub fn dlc(&self) -> usize {
        self.dlc
    }

    /// 信号清单，按定义顺序。
    pub fn signals(&self) -> &[Signal] {
        &self.signals
    }

    /// 按名字查信号。
    pub fn signal(&self, name: &str) -> Option<&Signal> {
        self.signals.iter().find(|signal| signal.name == name)
    }

    /// 解出一帧里全部信号的物理值。
    pub fn decode(&self, frame: &[u8]) -> Result<Vec<(&Signal, f64)>, Error> {
        if frame.len() < self.dlc {
            return Err(Error::FrameLength {
                expected: self.dlc,
                actual: frame.len(),
            });
        }
        Ok(self
            .signals
            .iter()
            .map(|signal| (signal, signal.decode(frame)))
            .collect())
    }
}

/// 一个信号。
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    name: String,
    start_bit: u32,
    len: u32,
    byte_order: ByteOrder,
    signed: bool,
    factor: f64,
    offset: f64,
    min: f64,
    max: f64,
    unit: String,
}

impl Signal {
    /// 信号名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 起始位。Intel 下是最低位，Motorola 下是最高位。
    pub fn start_bit(&self) -> u32 {
        self.start_bit
    }

    /// 位宽。
    pub fn len(&self) -> u32 {
        self.len
    }

    /// 位宽为零。DBC 里不该出现，但语法上允许。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 字节序。
    pub fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }

    /// 是否有符号。
    pub fn is_signed(&self) -> bool {
        self.signed
    }

    /// 比例系数。
    pub fn factor(&self) -> f64 {
        self.factor
    }

    /// 偏移。
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// 量程下限。
    pub fn min(&self) -> f64 {
        self.min
    }

    /// 量程上限。
    pub fn max(&self) -> f64 {
        self.max
    }

    /// 单位。空串表示没有单位。
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// 从一帧里解出物理值。
    pub fn decode(&self, frame: &[u8]) -> f64 {
        let raw = self.raw_bits(frame);
        let signed = self.sign_extend(raw);
        signed as f64 * self.factor + self.offset
    }

    /// 把物理值编成位。返回的错误只在不该发生的量程越界时出现。
    pub fn encode(&self, value: f64) -> Result<u64, Error> {
        if value < self.min || value > self.max {
            return Err(Error::OutOfRange {
                signal: self.name.clone(),
                value,
                min: self.min,
                max: self.max,
            });
        }
        let raw = ((value - self.offset) / self.factor).round();
        Ok(raw as i64 as u64)
    }

    /// 取原始位（未做符号扩展与换算）。
    pub fn raw_bits(&self, frame: &[u8]) -> u64 {
        match self.byte_order {
            ByteOrder::Intel => self.raw_intel(frame),
            ByteOrder::Motorola => self.raw_motorola(frame),
        }
    }

    fn raw_intel(&self, frame: &[u8]) -> u64 {
        let mut raw = 0u64;
        for offset in 0..self.len {
            let bit = self.start_bit + offset;
            let byte = (bit / 8) as usize;
            if byte >= frame.len() {
                break;
            }
            let value = (frame[byte] >> (bit % 8)) & 1;
            raw |= u64::from(value) << offset;
        }
        raw
    }

    fn raw_motorola(&self, frame: &[u8]) -> u64 {
        // Motorola 的位编号是锯齿序：起始位是最高位，往后按字节内的
        // 7→0 递减，跨到下一字节时回到 7。这半段的写法最容易与 Intel
        // 混，故单独成一个函数。
        let mut raw = 0u64;
        let mut byte = (self.start_bit / 8) as usize;
        let mut bit_in_byte = self.start_bit % 8;
        for _ in 0..self.len {
            if byte < frame.len() {
                let value = (frame[byte] >> bit_in_byte) & 1;
                raw = (raw << 1) | u64::from(value);
            } else {
                raw <<= 1;
            }
            if bit_in_byte == 0 {
                bit_in_byte = 7;
                byte += 1;
            } else {
                bit_in_byte -= 1;
            }
        }
        raw
    }

    fn sign_extend(&self, raw: u64) -> i64 {
        if !self.signed || self.len == 0 || self.len >= 64 {
            return raw as i64;
        }
        let sign_bit = 1u64 << (self.len - 1);
        if raw & sign_bit == 0 {
            return raw as i64;
        }
        (raw as i64) - (1i64 << self.len)
    }
}

/// 解析 DBC 文本。
pub fn load_str(text: &str) -> Result<Database, Error> {
    let mut database = Database::default();
    let mut current: Option<Message> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(message) = parse_bo(trimmed) {
            if let Some(previous) = current.take() {
                database.messages.insert(previous.id, previous);
            }
            current = Some(message);
            continue;
        }
        if let Some(signal) = parse_sg(trimmed) {
            match current.as_mut() {
                Some(message) => message.signals.push(signal),
                None => database.skipped.push("SG_ 出现在 BO_ 之前".to_owned()),
            }
            continue;
        }
        note_skipped(trimmed, &mut database.skipped);
    }
    if let Some(message) = current {
        database.messages.insert(message.id, message);
    }
    if database.messages.is_empty() {
        return Err(Error::NoMessages);
    }
    Ok(database)
}

/// 只把"看起来是一条语句但没实现"的类别记下来，空行与注释不算。
fn note_skipped(line: &str, skipped: &mut Vec<String>) {
    if line.is_empty() || line.starts_with("//") {
        return;
    }
    let keyword = line.split_whitespace().next().unwrap_or("");
    // 文件头这几段只是元信息，跳过它们不影响取值，报出来只是噪音。
    const HEADER: [&str; 5] = ["VERSION", "NS_", "BS_:", "BS_", "BU_:"];
    if keyword.is_empty() || HEADER.contains(&keyword) {
        return;
    }
    if skipped.iter().any(|seen| seen == keyword) {
        return;
    }
    skipped.push(keyword.to_owned());
}

fn parse_bo(line: &str) -> Option<Message> {
    let rest = line.strip_prefix("BO_ ")?;
    let mut parts = rest.splitn(3, ' ');
    let id: u32 = parts.next()?.parse().ok()?;
    let name = parts.next()?.strip_suffix(':')?.to_owned();
    let dlc: usize = parts.next()?.split_whitespace().next()?.parse().ok()?;
    Some(Message {
        // DBC 里最高位是扩展帧标志，不属于 id 本身。
        id: id & 0x1FFF_FFFF,
        name,
        dlc,
        signals: Vec::new(),
    })
}

/// 把 `SG_ 名字 : 后面一长串` 拆成名字与剩余部分。
fn split_sg(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix("SG_ ")?;
    let (head, tail) = rest.split_once(" : ")?;
    head.split_whitespace()
        .next()
        .map(|name| (name.to_owned(), tail))
}

fn parse_sg(line: &str) -> Option<Signal> {
    let (name, tail) = split_sg(line)?;
    let mut fields = tail.split_whitespace();
    let (start_bit, len, byte_order, signed) = parse_position(fields.next()?)?;
    let (factor, offset) = parse_pair(fields.next()?, '(', ')')?;
    let (min, max) = parse_pair(fields.next()?, '[', ']')?;
    let unit = fields.next()?.trim_matches('"').to_owned();

    Some(Signal {
        name,
        start_bit,
        len,
        byte_order,
        signed,
        factor,
        offset,
        min,
        max,
        unit,
    })
}

/// `起始位|位宽@字节序 符号` 这一段，形如 `7|4@0+`。
fn parse_position(raw: &str) -> Option<(u32, u32, ByteOrder, bool)> {
    let (bits, order_sign) = raw.split_once('@')?;
    let (start_bit, len) = parse_bits(bits)?;
    let (byte_order, signed) = parse_flags(order_sign)?;
    Some((start_bit, len, byte_order, signed))
}

fn parse_bits(raw: &str) -> Option<(u32, u32)> {
    let (start_bit, len) = raw.split_once('|')?;
    Some((start_bit.parse().ok()?, len.parse().ok()?))
}

fn parse_flags(raw: &str) -> Option<(ByteOrder, bool)> {
    let mut flags = raw.chars();
    let byte_order = match flags.next()? {
        '1' => ByteOrder::Intel,
        '0' => ByteOrder::Motorola,
        _ => return None,
    };
    let signed = match flags.next()? {
        '+' => false,
        '-' => true,
        _ => return None,
    };
    Some((byte_order, signed))
}

/// 圆括号里是逗号分隔的因子与偏移，方括号里是竖线分隔的量程。
fn parse_pair(raw: &str, open: char, close: char) -> Option<(f64, f64)> {
    let inner = raw.strip_prefix(open)?.strip_suffix(close)?;
    let separator = if close == ']' { '|' } else { ',' };
    let (left, right) = inner.split_once(separator)?;
    Some((left.trim().parse().ok()?, right.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
VERSION "test"
BO_ 256 EngineData: 8 ECU
 SG_ EngineSpeed : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TESTER
 SG_ CoolantTemp : 16|8@1- (1,-40) [-40|215] "degC" TESTER
"#;

    #[test]
    fn only_statements_we_did_not_implement_are_reported() {
        let database = load_str(TEXT).expect("能解析");
        assert_eq!(database.skipped(), Vec::<String>::new(), "VERSION 不算语句");
    }

    #[test]
    fn unknown_statement_kinds_are_listed_once() {
        let text = format!(
            "{TEXT}\nBA_DEF_ BO_ \"GenMsgCycleTime\" INT 0 1000;\nBA_ \"GenMsgCycleTime\" BO_ 256 10;"
        );
        let database = load_str(text.as_str()).expect("能解析");
        assert_eq!(database.skipped(), vec!["BA_DEF_", "BA_"]);
    }

    #[test]
    fn no_messages_is_an_error() {
        assert!(matches!(load_str("VERSION \"x\""), Err(Error::NoMessages)));
    }

    #[test]
    fn signed_signal_sign_extends() {
        let database = load_str(TEXT).expect("能解析");
        let message = database.message(256).expect("消息在");
        let mut frame = [0u8; 8];
        frame[2] = 0xF0; // CoolantTemp 的原始值是 -16
        let decoded = message.decode(&frame).expect("长度够");
        let temp = decoded
            .iter()
            .find(|(signal, _)| signal.name() == "CoolantTemp")
            .expect("信号在");
        assert!((temp.1 - (-56.0)).abs() < 1e-9, "实为 {}", temp.1);
    }

    #[test]
    fn motorola_bits_run_high_to_low_across_bytes() {
        let database = load_str(TEXT).expect("能解析");
        let message = database.message(256).expect("消息在");
        let signal = Signal {
            name: "m".to_owned(),
            start_bit: 7,
            len: 12,
            byte_order: ByteOrder::Motorola,
            signed: false,
            factor: 1.0,
            offset: 0.0,
            min: 0.0,
            max: 4095.0,
            unit: String::new(),
        };
        assert!(message.dlc() >= 2);
        // 0xAB 0xCD 的高 12 位是 0xABC。
        assert_eq!(signal.raw_bits(&[0xAB, 0xCD]), 0xABC);
    }

    #[test]
    fn encode_refuses_values_outside_the_declared_range() {
        let database = load_str(TEXT).expect("能解析");
        let signal = database
            .message(256)
            .and_then(|message| message.signal("CoolantTemp"))
            .expect("信号在");
        assert!(signal.encode(-100.0).is_err());
        assert_eq!(signal.encode(215.0).expect("上界含"), 255);
    }

    #[test]
    fn short_frames_are_refused_not_silently_padded() {
        let database = load_str(TEXT).expect("能解析");
        let message = database.message(256).expect("消息在");
        assert!(matches!(
            message.decode(&[0u8; 4]),
            Err(Error::FrameLength {
                expected: 8,
                actual: 4
            })
        ));
    }
}
