//! 装配层：把多个信息源拼在一起。
//!
//! 这是分档表里唯一允许同时指名具体后端 crate 的位置（L4）。信息源前端
//! 各自只认自己的输入格式，互相不认识；"同一台 ECU 的两份描述文件说的
//! 是不是一回事"这个问题，只有在这里问得出来。
//!
//! 当前只做一件事：A2L 与 ELF 的交叉校验（[M3-07]）。两份文件描述同一批
//! 变量，地址、尺寸、元素类型必须一致——不一致意味着其中一份过时了，或者
//! 根本不是同一台 ECU 的。这种错在写入时不会报错，只会写到错的地方。
//!
//! [M3-07]: ../../todo.todo

#![forbid(unsafe_code)]

use openape_cal_core::{ElemType, Symbol, VariableTree};

/// 冲突落在哪个字段上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    /// 首地址不同。
    Address,
    /// 元素类型不同。
    Elem,
    /// 维度不同（元素个数或形状）。
    Dims,
}

impl Field {
    /// 字段名，用于报告。
    pub fn label(self) -> &'static str {
        match self {
            Self::Address => "地址",
            Self::Elem => "元素类型",
            Self::Dims => "维度",
        }
    }
}

/// 一条冲突：同一个符号，两边说的不一样。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    symbol: Symbol,
    field: Field,
    elf: String,
    a2l: String,
}

impl Conflict {
    /// 出问题的符号。
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// 出问题的字段。
    pub fn field(&self) -> Field {
        self.field
    }

    /// ELF 侧的值。
    pub fn elf(&self) -> &str {
        &self.elf
    }

    /// A2L 侧的值。
    pub fn a2l(&self) -> &str {
        &self.a2l
    }
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} 的{}不一致：ELF 说 {}，A2L 说 {}",
            self.symbol,
            self.field.label(),
            self.elf,
            self.a2l
        )
    }
}

/// 交叉校验结果。
#[derive(Debug, Clone, Default)]
pub struct Report {
    conflicts: Vec<Conflict>,
    only_in_elf: Vec<Symbol>,
    only_in_a2l: Vec<Symbol>,
}

impl Report {
    /// 字段不一致的清单，按符号名与字段排序。
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// 只在 ELF 里出现的符号。不算冲突——A2L 可以不描述全部变量。
    pub fn only_in_elf(&self) -> &[Symbol] {
        &self.only_in_elf
    }

    /// 只在 A2L 里出现的符号。常见于 A2L 记着 ELF 已被优化掉的变量。
    pub fn only_in_a2l(&self) -> &[Symbol] {
        &self.only_in_a2l
    }

    /// 有没有字段级冲突。单边缺失不算——那只是覆盖范围不同。
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// 能否放心使用这两份文件：没有字段级冲突。
    pub fn is_consistent(&self) -> bool {
        !self.has_conflicts()
    }
}

/// 比对两份变量树。
///
/// 只比两边都有的符号。单边缺失记进报告但不算冲突：A2L 常比 ELF 少（
/// 用户只关心部分变量），也常比 ELF 多（记着已经被优化掉的变量）——
/// 这两种都是正常状态，当成冲突会让报告全是噪音。
pub fn cross_check(elf: &VariableTree, a2l: &VariableTree) -> Report {
    let mut report = Report::default();
    for variable in elf.iter() {
        match a2l.get(variable.symbol()) {
            Some(other) => compare(variable, other, &mut report.conflicts),
            None => report.only_in_elf.push(variable.symbol().clone()),
        }
    }
    for variable in a2l.iter() {
        if elf.get(variable.symbol()).is_none() {
            report.only_in_a2l.push(variable.symbol().clone());
        }
    }
    report.only_in_elf.sort();
    report.only_in_a2l.sort();
    report
}

fn compare(
    elf: &openape_cal_core::Variable,
    a2l: &openape_cal_core::Variable,
    out: &mut Vec<Conflict>,
) {
    if elf.address() != a2l.address() {
        out.push(Conflict {
            symbol: elf.symbol().clone(),
            field: Field::Address,
            elf: format!("{:#010X}", elf.address().get()),
            a2l: format!("{:#010X}", a2l.address().get()),
        });
    }
    if elf.elem() != a2l.elem() {
        out.push(Conflict {
            symbol: elf.symbol().clone(),
            field: Field::Elem,
            elf: elem_name(elf.elem()),
            a2l: elem_name(a2l.elem()),
        });
    }
    if elf.dims() != a2l.dims() {
        out.push(Conflict {
            symbol: elf.symbol().clone(),
            field: Field::Dims,
            elf: dims_text(elf.dims()),
            a2l: dims_text(a2l.dims()),
        });
    }
}

fn elem_name(elem: ElemType) -> String {
    format!("{elem:?}")
}

fn dims_text(dims: &[u32]) -> String {
    if dims.is_empty() {
        return "标量".to_owned();
    }
    let parts: Vec<String> = dims.iter().map(u32::to_string).collect();
    format!("[{}]", parts.join("]["))
}
