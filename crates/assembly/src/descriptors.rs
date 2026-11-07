//! 描述文件装配：把 ELF 与 A2L 读成一份变量树，并给出给界面的清单。
//!
//! 清单与树必须出自同一次解析：分开解析会让"界面显示的地址"与"实际写入的
//! 地址"有机会不一致——那是写坏 ECU 的经典路径。所以两者一起返回。
//!
//! 读不动的文件不阻断启动：记一条提示，让上层把"未加载"与"加载了但没变量"
//! 分开表达。

use std::path::Path;

use openape_cal_core::{SourceMode, VariableTree};
use openape_proto as proto;

/// 加载出来的描述信息。
#[derive(Debug, Clone)]
pub struct LoadedVariables {
    /// 给界面的清单。
    pub list: proto::VariableList,
    /// 变量树。写入按符号查长度、量程与元素类型都靠它。
    pub tree: Option<VariableTree>,
    /// 输入档。只有 A2L 才算带语义。
    pub mode: SourceMode,
}

/// 加载变量清单。两个文件都不给时返回 `None`——界面据此说明"未加载描述
/// 文件"，而不是显示一个空表格让人以为 ECU 没有变量。
pub fn load_variables(elf: Option<&Path>, a2l: Option<&Path>) -> Option<LoadedVariables> {
    if elf.is_none() && a2l.is_none() {
        return None;
    }
    let mut notes = Vec::new();
    let elf_tree = elf.and_then(|path| load_elf_tree(path, &mut notes));
    let a2l_tree = a2l.and_then(|path| load_a2l_tree(path, &mut notes));
    if let (Some(elf), Some(a2l)) = (&elf_tree, &a2l_tree) {
        notes.extend(
            crate::cross_check(elf, a2l)
                .conflicts()
                .iter()
                .map(ToString::to_string),
        );
    }
    let primary = elf_tree.as_ref().or(a2l_tree.as_ref());
    // 只有 A2L 带语义；单独的 ELF 一律按 raw 对待（D-13）。
    let mode = if a2l_tree.is_some() {
        SourceMode::WithA2l
    } else {
        SourceMode::ElfOnly
    };
    Some(LoadedVariables {
        list: proto::VariableList {
            variables: primary.map(entries).unwrap_or_default(),
            notes,
            loaded: primary.is_some(),
        },
        tree: primary.cloned(),
        mode,
    })
}

fn load_elf_tree(path: &Path, notes: &mut Vec<String>) -> Option<VariableTree> {
    let source = match openape_format_elf::load(path) {
        Ok(source) => source,
        Err(err) => {
            notes.push(format!("读 ELF 失败：{err}"));
            return None;
        }
    };
    let (tree, elf_notes) = match source.to_ir() {
        Ok(pair) => pair,
        Err(err) => {
            notes.push(format!("ELF 进 IR 失败：{err}"));
            return None;
        }
    };
    notes.extend(elf_notes.iter().map(ToString::to_string));
    Some(tree)
}

fn load_a2l_tree(path: &Path, notes: &mut Vec<String>) -> Option<VariableTree> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            notes.push(format!("读不了 {}：{err}", path.display()));
            return None;
        }
    };
    let source = match openape_format_a2l::load_str(&text, None) {
        Ok(source) => source,
        Err(err) => {
            notes.push(format!("读 A2L 失败：{err}"));
            return None;
        }
    };
    notes.extend(source.notes().iter().map(ToString::to_string));
    Some(source.tree().clone())
}

fn entries(tree: &VariableTree) -> Vec<proto::VariableEntry> {
    tree.iter()
        .map(|variable| proto::VariableEntry {
            symbol: variable.symbol().as_str().to_owned(),
            address: variable.address().get(),
            elem: format!("{:?}", variable.elem()),
            dims: variable.dims().to_vec(),
            role: variable
                .role()
                .map(|role| format!("{role:?}"))
                .unwrap_or_default(),
            raw: variable.semantics().is_empty(),
            range: variable.semantics().range().map(|range| proto::RangeCheck {
                min: range.min(),
                max: range.max(),
                physical: 0.0,
            }),
        })
        .collect()
}
