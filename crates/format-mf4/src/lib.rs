//! MF4 写出端（M5-02）：把采集样本流落盘成 ASAM MDF 4.1，供离线重开读回。
//!
//! 分工与取舍：
//!
//! - **只写不读**。解析不在本 crate 的职责里——读回交给 `mf4-rs` 的解析器
//!   （验收工具）与 M5-05 的离线重开。写出端自己再实现一份解析器，两份代码
//!   共享的只有格式定义，互测不出对方的错。
//! - **块布局不手写**。MDF4 的块链接、对齐、补零交给 `mf4-rs` 的 writer；
//!   本 crate 管语义层：时间主通道、通道顺序、行边界校验、收尾落盘。
//!   "自己写自己读"不构成互证，第三方互测由 `script/verify-mf4.py` 用
//!   asammdf 读同一批文件承担。
//! - **物理值直存**。样本按 f64 落盘，不做换算与量程裁剪——换算是
//!   [`openape-cal-core`] 的职责，录制端改写数值就是把两处换算链埋进格式层。
//!
//! 时间主通道的物理单位按 MDF 约定是秒（`sync_type = 1`）；调用方把
//! 微秒时间戳除以 1e6 再进来。通道单位文本暂未落盘（`mf4-rs` 的 writer
//! 没有便捷路径），需要时写 `##TX` 并补 `unit_addr` 链接即可。

use std::path::Path;

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::mdf_writer::MdfWriter;
use thiserror::Error;

/// 一条待录通道：只带名字，值在行里按同一顺序给。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSpec {
    /// 通道名。读回方按这个名字取列（`mf4-rs`、asammdf 皆然）。
    pub name: String,
}

impl ChannelSpec {
    /// 一条通道。
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

/// 写出错误。
#[derive(Debug, Error)]
pub enum Error {
    /// 路径不是合法 UTF-8（`mf4-rs` 的 writer 按 `&str` 开文件）。
    #[error("路径不是合法 UTF-8：{0:?}")]
    BadPath(std::path::PathBuf),

    /// 底层 MDF 写出失败（开文件、写块、收尾）。
    #[error(transparent)]
    Mdf(#[from] MdfError),

    /// 行写进了已收尾的数据块。类型系统把 `finish` 后再用挡死（消费
    /// self），这条只给显式收尾前后的异常路径留闸。
    #[error("数据块已收尾，不能再写行")]
    AlreadyFinished,

    /// 一行的值数与建文件时的通道数不符。
    #[error("行值数与通道数不符：要 {expected} 个，来 {actual} 个")]
    ColumnMismatch {
        /// 建文件时的通道数。
        expected: usize,
        /// 本行实到的值数。
        actual: usize,
    },
}

/// 一份在写的 MF4 文件。
///
/// 用法：[`Writer::create`] 给通道名 → 逐行 [`Writer::write_row`]（时间在
/// 前、值按通道顺序在后）→ [`Writer::finish`] 消费收尾。收尾之后本类型
/// 就没了，写过的句柄不可能再被用。
pub struct Writer {
    inner: MdfWriter,
    cg_id: String,
    channels: usize,
    open: bool,
}

impl Writer {
    /// 建一份新文件，时间主通道 + 每通道一条 `f64` 列。
    ///
    /// 起始时间记 0（纳秒）：录制端没有墙钟锚点语义，离线重开关心的是
    /// 相对时间轴；带真实墙钟会让 golden 比对无谓地抖动。
    pub fn create(path: &Path, channels: &[ChannelSpec]) -> Result<Self, Error> {
        let as_str = path
            .to_str()
            .ok_or_else(|| Error::BadPath(path.to_path_buf()))?;
        let mut inner = MdfWriter::new(as_str)?;
        inner.init_mdf_file()?;
        inner.set_start_time(0, 0, 0, 0, 0)?;
        let cg_id = inner.add_channel_group(None, |_| {})?;

        // 通道顺序即记录布局顺序：时间主通道占第一个槽，随后各通道按
        // 传入顺序落列，读回按 `byte_offset` 切，与行内值的顺序一致。
        let time_spec = ChannelSpec::new("time");
        let mut last_cn: Option<String> = None;
        for (index, channel) in std::iter::once(&time_spec)
            .chain(channels.iter())
            .enumerate()
        {
            let cn_id = inner.add_channel(&cg_id, last_cn.as_deref(), |cn| {
                cn.name = Some(channel.name.clone());
                cn.data_type = DataType::FloatLE;
                cn.bit_count = 64;
                if index == 0 {
                    // 主通道的位型由 set_time_channel 落定，这里只声明
                    // 同步语义：物理单位是秒。
                    cn.sync_type = 1;
                }
            })?;
            if index == 0 {
                inner.set_time_channel(&cn_id)?;
            }
            last_cn = Some(cn_id);
        }
        // 记录布局到此落定，随即开数据块：后续 write_row 全部进这一个
        // DT 块，finish_data_block 收口并补长度。sorted 记录，无 record id。
        inner.start_data_block_for_cg(&cg_id, 0)?;

        Ok(Self {
            inner,
            cg_id,
            channels: channels.len(),
            open: true,
        })
    }

    /// 写一行样本：相对时间（秒）+ 各通道物理值，顺序同 [`Writer::create`]。
    pub fn write_row(&mut self, time_s: f64, values: &[f64]) -> Result<(), Error> {
        if values.len() != self.channels {
            return Err(Error::ColumnMismatch {
                expected: self.channels,
                actual: values.len(),
            });
        }
        if !self.open {
            // 类型系统已把"finish 后再写"挡住（finish 消费 self），这里
            // 只是给同一次借用里的显式收尾路径留个闸。正常流程到不了。
            return Err(Error::AlreadyFinished);
        }
        let mut record = Vec::with_capacity(self.channels + 1);
        record.push(DecodedValue::Float(time_s));
        record.extend(values.iter().map(|value| DecodedValue::Float(*value)));
        self.inner.write_record(&self.cg_id, &record)?;
        Ok(())
    }

    /// 收尾落盘：关数据块、补块大小与链接、刷缓冲。消费本类型。
    ///
    /// 不走 `finish` 直接丢弃（错误路径半途而废）会留下没有补全链接的
    /// 残件：`mf4-rs` 的 ID 块缺省就是"已收尾"标志，读方不会被明确拒绝，
    /// 只是读到坏数据。所以丢弃残件是调用方的责任——M5-03 的录制器在
    /// 写失败时删掉半成品重开，不在残件上续写。
    pub fn finish(mut self) -> Result<(), Error> {
        if self.open {
            self.inner.finish_data_block(&self.cg_id)?;
            self.open = false;
        }
        self.inner.finalize()?;
        Ok(())
    }
}
