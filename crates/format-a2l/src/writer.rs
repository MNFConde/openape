//! IR 导出成 A2L 文本。
//!
//! 手写而不是用 `a2lfile` 的写接口：导出件要能被人读、被 diff，格式由我们
//! 说了算，且字段集合是闭的（MVP 只导出这几类块）。合法性由 `a2lfile` 严格
//! 模式在测试里兜底，不靠人肉检查。
//!
//! 两处反转与读入侧对齐：`MATRIX_DIM` 与 `AXIS_DESCR` 都是快速维在前。

use std::collections::BTreeSet;
use std::fmt::Write as _;

use openape_cal_core::{
    AxisBinding, ByteOrder, ElemType, Id, Role, Scale, ValueKind, Variable, VariableTree,
};

use crate::error::Error;

/// 没有量程时用的兜底上下限。A2L 的这两项必填，只能给一个极宽的范围。
const WIDE_LIMITS: f64 = 1.0e9;

/// 把变量树与轴绑定导出成 A2L 文本。
pub(crate) fn write(tree: &VariableTree, binding: &AxisBinding) -> Result<String, Error> {
    let mut out = String::new();
    let byte_order = tree
        .iter()
        .next()
        .map(Variable::byte_order)
        .unwrap_or_default();

    out.push_str("ASAP2_VERSION 1 71\n");
    out.push_str("/begin PROJECT OpenAPE \"\"\n");
    out.push_str("  /begin MODULE ECU \"\"\n");
    out.push_str("    /begin MOD_COMMON \"\"\n");
    writeln!(
        out,
        "      BYTE_ORDER {}",
        match byte_order {
            ByteOrder::Big => "MSB_LAST",
            ByteOrder::Little => "MSB_FIRST",
        }
    )
    .ok();
    out.push_str("    /end MOD_COMMON\n");

    write_record_layouts(&mut out, tree);
    write_compu_methods(&mut out, tree);

    for axis in binding.axes() {
        let conversion = conversion_of(tree, axis.symbol().as_str());
        write_axis_pts(
            &mut out,
            axis.id(),
            axis.symbol().as_str(),
            axis.address().get(),
            axis.elem(),
            axis.points(),
            &conversion,
            limits_of(tree, axis.symbol().as_str()),
            byte_order,
            axis.symbol().as_str() != axis.id().as_str(),
        );
    }
    for value in binding.values() {
        let conversion = conversion_of(tree, value.symbol().as_str());
        write_bound_value(&mut out, value, tree, binding, &conversion)?;
    }
    for variable in tree.iter() {
        if is_bound(variable, binding) {
            continue;
        }
        write_loose_variable(&mut out, variable, tree)?;
    }

    out.push_str("  /end MODULE\n");
    out.push_str("/end PROJECT\n");
    Ok(out)
}

fn is_bound(variable: &Variable, binding: &AxisBinding) -> bool {
    binding
        .values()
        .iter()
        .any(|value| value.symbol() == variable.symbol())
        || binding
            .axes()
            .iter()
            .any(|axis| axis.symbol() == variable.symbol())
}

/// 每种用到的元素类型一个布局。名字里带类型，读的人一眼能看出宽度。
fn write_record_layouts(out: &mut String, tree: &VariableTree) {
    let used: BTreeSet<ElemType> = tree.iter().map(Variable::elem).collect();
    for elem in used {
        writeln!(out, "    /begin RECORD_LAYOUT {}", layout_name(elem)).ok();
        writeln!(out, "      FNC_VALUES 1 {} ROW_DIR DIRECT", datatype(elem)).ok();
        out.push_str("    /end RECORD_LAYOUT\n");
    }
}

/// 单位与换算都挂在 COMPU_METHOD 上，所以只要两者有一个就得生成一个。
///
/// 遍历整棵树而不是只看绑定表——观测量也常带换算，漏了它导出件的引用就断了。
fn write_compu_methods(out: &mut String, tree: &VariableTree) {
    let mut written: BTreeSet<String> = BTreeSet::new();
    for variable in tree.iter() {
        if !needs_compu_method(variable) {
            continue;
        }
        let symbol = variable.symbol().as_str();
        let name = compu_name(symbol);
        if !written.insert(name.clone()) {
            continue;
        }
        let unit = variable
            .semantics()
            .unit()
            .map(|unit| unit.as_str())
            .unwrap_or("");
        match variable.semantics().scale() {
            Some(Scale::Linear { factor, offset }) => {
                writeln!(
                    out,
                    "    /begin COMPU_METHOD {name} \"\" LINEAR \"%8.3\" \"{unit}\""
                )
                .ok();
                writeln!(out, "      COEFFS_LINEAR {factor} {offset}").ok();
            }
            // 只有单位没有换算：IDENTICAL 把物理值原样传出去，单位随行
            None | Some(_) => {
                writeln!(
                    out,
                    "    /begin COMPU_METHOD {name} \"\" IDENTICAL \"%8.3\" \"{unit}\""
                )
                .ok();
            }
        }
        out.push_str("    /end COMPU_METHOD\n");
    }
}

fn find_variable<'a>(tree: &'a VariableTree, symbol: &str) -> Option<&'a Variable> {
    let wanted = openape_cal_core::Symbol::new(symbol)?;
    tree.get(&wanted)
}

#[expect(
    clippy::too_many_arguments,
    reason = "十个参数都是 A2L 轴描述必需的字段，打包成结构体会在调用点重复一遍拆装"
)]
fn write_axis_pts(
    out: &mut String,
    id: &Id,
    symbol: &str,
    address: u32,
    elem: ElemType,
    points: u32,
    conversion: &str,
    (lower, upper): (f64, f64),
    byte_order: ByteOrder,
    needs_symbol_link: bool,
) {
    writeln!(
        out,
        "    /begin AXIS_PTS {id} \"\" {address:#010X} NO_INPUT_QUANTITY {} 0 {conversion} {points} {lower} {upper}",
        layout_name(elem)
    )
    .ok();
    write_byte_order(out, byte_order);
    if needs_symbol_link {
        writeln!(out, "      SYMBOL_LINK \"{symbol}\" 0").ok();
    }
    out.push_str("    /end AXIS_PTS\n");
}

fn write_bound_value(
    out: &mut String,
    value: &openape_cal_core::BoundValue,
    tree: &VariableTree,
    binding: &AxisBinding,
    conversion: &str,
) -> Result<(), Error> {
    let kind = match value.kind() {
        ValueKind::Curve => "CURVE",
        ValueKind::Map => "MAP",
        ValueKind::Higher => {
            return Err(Error::NotExportable {
                symbol: value.symbol().to_string(),
                reason: "三维及以上的值不导出 A2L（MVP 范围）".to_owned(),
            });
        }
    };
    let (lower, upper) = limits_of(tree, value.symbol().as_str());
    writeln!(
        out,
        "    /begin CHARACTERISTIC {} \"\" {kind} {:#010X} {} 0 {conversion} {lower} {upper}",
        value.id(),
        value.address().get(),
        layout_name(value.elem())
    )
    .ok();
    if value.dims().len() > 1 {
        let quick_first: Vec<String> = value.dims().iter().rev().map(u32::to_string).collect();
        writeln!(out, "      MATRIX_DIM {}", quick_first.join(" ")).ok();
    }
    // AXIS_DESCR 快速维在前，与 dims 相反
    for axis_id in value.axes().iter().rev() {
        let Some(axis) = binding.axis(axis_id) else {
            return Err(Error::NotExportable {
                symbol: value.symbol().to_string(),
                reason: format!("引用的轴 {axis_id} 不在绑定表里"),
            });
        };
        let (axis_lower, axis_upper) = limits_of(tree, axis.symbol().as_str());
        let attribute = match axis.kind() {
            openape_cal_core::AxisKind::Shared => "COM_AXIS",
            openape_cal_core::AxisKind::Inline => "STD_AXIS",
        };
        writeln!(
            out,
            "      /begin AXIS_DESCR {attribute} NO_INPUT_QUANTITY {conversion} {} {axis_lower} {axis_upper}",
            axis.points()
        )
        .ok();
        if matches!(axis.kind(), openape_cal_core::AxisKind::Shared) {
            writeln!(out, "        AXIS_PTS_REF {}", axis.id()).ok();
        }
        out.push_str("      /end AXIS_DESCR\n");
    }
    out.push_str("    /end CHARACTERISTIC\n");
    Ok(())
}

/// 没进绑定表的变量：观测量走 MEASUREMENT，标定量走 CHARACTERISTIC。
fn write_loose_variable(
    out: &mut String,
    variable: &Variable,
    tree: &VariableTree,
) -> Result<(), Error> {
    let conversion = conversion_of(tree, variable.symbol().as_str());
    let (lower, upper) = limits_of(tree, variable.symbol().as_str());
    let name = variable.symbol().as_str();
    let address = variable.address().get();

    match variable.role() {
        Some(Role::Measurement) => {
            writeln!(
                out,
                "    /begin MEASUREMENT {name} \"\" {} {conversion} 1 0.0 {lower} {upper}",
                datatype(variable.elem())
            )
            .ok();
            writeln!(out, "      ECU_ADDRESS {address:#010X}").ok();
            if let Some(&first) = variable.dims().first() {
                writeln!(out, "      ARRAY_SIZE {first}").ok();
            }
            out.push_str("    /end MEASUREMENT\n");
        }
        Some(Role::Characteristic) => {
            let kind = if variable.dims().is_empty() {
                "VALUE"
            } else {
                "VAL_BLK"
            };
            writeln!(
                out,
                "    /begin CHARACTERISTIC {name} \"\" {kind} {address:#010X} {} 0 {conversion} {lower} {upper}",
                layout_name(variable.elem())
            )
            .ok();
            // 一维也要写：不写 MATRIX_DIM 的 VAL_BLK 读回来是个"没有维度的
            // 矩阵"，会被加载器当成畸形对象跳过——导出件就此丢一条变量。
            if !variable.dims().is_empty() {
                let quick_first: Vec<String> =
                    variable.dims().iter().rev().map(u32::to_string).collect();
                writeln!(out, "      MATRIX_DIM {}", quick_first.join(" ")).ok();
            }
            out.push_str("    /end CHARACTERISTIC\n");
        }
        // 包括 Role 的非穷尽新变体：角色不明就不导，不猜
        None | Some(_) => {
            return Err(Error::NotExportable {
                symbol: variable.symbol().to_string(),
                reason: "没有角色或角色未支持；仅 ELF 档由用户指定角色后才能导出".to_owned(),
            });
        }
    }
    Ok(())
}

fn write_byte_order(out: &mut String, byte_order: ByteOrder) {
    let spelling = match byte_order {
        ByteOrder::Big => "MSB_LAST",
        ByteOrder::Little => "MSB_FIRST",
    };
    // 对象内的 BYTE_ORDER 是单行元素，不能包 /begin /end
    writeln!(out, "      BYTE_ORDER {spelling}").ok();
}

fn limits_of(tree: &VariableTree, symbol: &str) -> (f64, f64) {
    find_variable(tree, symbol)
        .and_then(|variable| variable.semantics().range())
        .map(|range| (range.min(), range.max()))
        .unwrap_or((-WIDE_LIMITS, WIDE_LIMITS))
}

/// 单位或换算任意一个存在，就得有 COMPU_METHOD——否则语义会丢掉。
fn needs_compu_method(variable: &Variable) -> bool {
    !variable.semantics().is_empty()
        && (variable.semantics().scale().is_some() || variable.semantics().unit().is_some())
}

fn conversion_of(tree: &VariableTree, symbol: &str) -> String {
    match find_variable(tree, symbol) {
        Some(variable) if needs_compu_method(variable) => compu_name(symbol),
        _ => "NO_COMPU_METHOD".to_owned(),
    }
}

fn compu_name(symbol: &str) -> String {
    format!("CM_{symbol}")
}

fn layout_name(elem: ElemType) -> String {
    format!("RL_{}", datatype(elem))
}

/// 与读入侧的映射互为逆表，圈复杂度同样等于数据类型数，刻意保留。
fn datatype(elem: ElemType) -> &'static str {
    match elem {
        ElemType::U8 => "UBYTE",
        ElemType::I8 => "SBYTE",
        ElemType::U16 => "UWORD",
        ElemType::I16 => "SWORD",
        ElemType::U32 => "ULONG",
        ElemType::I32 => "SLONG",
        ElemType::U64 => "A_UINT64",
        ElemType::I64 => "A_INT64",
        ElemType::F32 => "FLOAT32_IEEE",
        ElemType::F64 => "FLOAT64_IEEE",
        // 非穷尽枚举的新变体：宁可编不过，也不要静默写错类型
        other => unreachable!("未支持的元素类型 {other:?} 不该进入导出"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_cal_core::{Address, Role, Symbol, Variable, VariableTree};

    #[test]
    fn every_elem_type_has_an_a2l_spelling() {
        let cases = [
            (ElemType::U8, "UBYTE"),
            (ElemType::I8, "SBYTE"),
            (ElemType::U16, "UWORD"),
            (ElemType::I16, "SWORD"),
            (ElemType::U32, "ULONG"),
            (ElemType::I32, "SLONG"),
            (ElemType::U64, "A_UINT64"),
            (ElemType::I64, "A_INT64"),
            (ElemType::F32, "FLOAT32_IEEE"),
            (ElemType::F64, "FLOAT64_IEEE"),
        ];
        for (elem, spelling) in cases {
            assert_eq!(datatype(elem), spelling);
            assert_eq!(layout_name(elem), format!("RL_{spelling}"));
        }
    }

    #[test]
    fn bound_values_above_two_dimensions_are_refused() {
        // 绑了轴的三维值没有对应的 A2L 结构（CUBOID 不在 MVP 范围）
        let symbol = Symbol::new("wide").expect("合法");
        let variable = Variable::new(symbol.clone(), Address::new(0x1000), ElemType::F32)
            .with_dims(vec![2, 2, 2])
            .expect("三维")
            .with_role(Role::Characteristic);
        let tree = VariableTree::new(vec![variable]).expect("无重复");
        let axis_id = openape_cal_core::Id::new("a").expect("合法");
        let value = openape_cal_core::BoundValue::new(
            openape_cal_core::Id::new("wide").expect("合法"),
            symbol,
            Address::new(0x1000),
            ElemType::F32,
            vec![2, 2, 2],
            vec![axis_id.clone(), axis_id.clone(), axis_id],
        )
        .expect("三维值可构造");
        let binding = AxisBinding::from_parts(Vec::new(), vec![value]);
        let err = write(&tree, &binding).expect_err("三维不导出");
        assert!(err.to_string().contains("三维"), "{err}");
    }

    #[test]
    fn unbound_arrays_are_written_as_val_blk() {
        let variable = Variable::new(
            Symbol::new("table").expect("合法"),
            Address::new(0x1000),
            ElemType::U16,
        )
        .with_dims(vec![2, 4])
        .expect("二维")
        .with_role(Role::Characteristic);
        let tree = VariableTree::new(vec![variable]).expect("无重复");
        let text = write(&tree, &AxisBinding::default()).expect("要能导出");
        assert!(text.contains("VAL_BLK"), "{text}");
        assert!(
            text.contains("MATRIX_DIM 4 2"),
            "MATRIX_DIM 快速维在前：{text}"
        );
    }
}
