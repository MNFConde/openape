//! 存储布局：`RECORD_LAYOUT` 的数值类型与字节序。
//!
//! 长度与类型不在 A2L 对象里直接给，而是引一个 `RECORD_LAYOUT`，其
//! `FNC_VALUES` 的 `datatype` 才是元素类型。缺省情况下 A2L 规范按
//! `UBYTE` 处理——这一点据实照做，但要提示用户，因为"默认一字节"多数
//! 时候不是他想要的。

use a2lfile::{A2lObjectName, ByteOrderEnum, DataType, Module};
use openape_cal_core::{ByteOrder, ElemType};

use crate::notice::Notice;

/// 模块级字节序。缺省是 `MSB_LAST`，即大端。
pub(crate) fn byte_order(module: &Module) -> ByteOrder {
    let declared = module
        .mod_common
        .as_ref()
        .and_then(|common| common.byte_order.as_ref())
        .map(|order| order.byte_order);
    match declared {
        Some(ByteOrderEnum::MsbFirst | ByteOrderEnum::LittleEndian) => ByteOrder::Little,
        Some(ByteOrderEnum::MsbFirstMswLast | ByteOrderEnum::MsbLastMswFirst) => ByteOrder::Big,
        Some(ByteOrderEnum::BigEndian | ByteOrderEnum::MsbLast) | None => ByteOrder::Big,
    }
}

/// 单个对象上的字节序覆盖；没写就跟随模块。
pub(crate) fn object_byte_order(
    declared: Option<&a2lfile::ByteOrder>,
    fallback: ByteOrder,
) -> ByteOrder {
    match declared.map(|order| order.byte_order) {
        Some(ByteOrderEnum::MsbFirst | ByteOrderEnum::LittleEndian) => ByteOrder::Little,
        Some(_) => ByteOrder::Big,
        None => fallback,
    }
}

/// A2L 的 `datatype` 到 IR 元素类型。`FLOAT16_IEEE` 不在 MVP 范围内。
///
/// 圈复杂度等于 A2L 数据类型数：这是一张外部规范给的映射表，拆到底还是
/// 十一条臂，与 xcp 的命令分派同属刻意保留的例外。
pub(crate) fn elem_of_datatype(datatype: DataType) -> Option<ElemType> {
    match datatype {
        DataType::Ubyte => Some(ElemType::U8),
        DataType::Sbyte => Some(ElemType::I8),
        DataType::Uword => Some(ElemType::U16),
        DataType::Sword => Some(ElemType::I16),
        DataType::Ulong => Some(ElemType::U32),
        DataType::Slong => Some(ElemType::I32),
        DataType::AUint64 => Some(ElemType::U64),
        DataType::AInt64 => Some(ElemType::I64),
        DataType::Float32Ieee => Some(ElemType::F32),
        DataType::Float64Ieee => Some(ElemType::F64),
        DataType::Float16Ieee => None,
    }
}

/// 按 `deposit_record` 找元素类型。
///
/// 找不到布局或布局没有 `FNC_VALUES` 时按 A2L 缺省的 `UBYTE` 处理，并留
/// 一条提示——四种缺省叠在一起最容易让用户"读出来的值全不对"却找不到原因。
pub(crate) fn elem_of_deposit(
    module: &Module,
    deposit: &str,
    subject: &str,
    notes: &mut Vec<Notice>,
) -> Option<ElemType> {
    let Some(layout) = module
        .record_layout
        .iter()
        .find(|layout| layout.get_name() == deposit)
    else {
        notes.push(Notice::warning(
            subject,
            format!("找不到 RECORD_LAYOUT {deposit}，按 A2L 缺省的 UBYTE 处理"),
        ));
        return Some(ElemType::U8);
    };
    let Some(fnc_values) = layout.fnc_values.as_ref() else {
        notes.push(Notice::warning(
            subject,
            format!("RECORD_LAYOUT {deposit} 没有 FNC_VALUES，按 A2L 缺省的 UBYTE 处理"),
        ));
        return Some(ElemType::U8);
    };
    let elem = elem_of_datatype(fnc_values.datatype);
    if elem.is_none() {
        notes.push(Notice::warning(
            subject,
            format!(
                "FNC_VALUES 的 datatype {:?} 不在 MVP 支持范围内",
                fnc_values.datatype
            ),
        ));
    }
    elem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datatype_map_covers_every_variant() {
        let cases = [
            (DataType::Ubyte, Some(ElemType::U8)),
            (DataType::Sbyte, Some(ElemType::I8)),
            (DataType::Uword, Some(ElemType::U16)),
            (DataType::Sword, Some(ElemType::I16)),
            (DataType::Ulong, Some(ElemType::U32)),
            (DataType::Slong, Some(ElemType::I32)),
            (DataType::AUint64, Some(ElemType::U64)),
            (DataType::AInt64, Some(ElemType::I64)),
            (DataType::Float32Ieee, Some(ElemType::F32)),
            (DataType::Float64Ieee, Some(ElemType::F64)),
            (DataType::Float16Ieee, None),
        ];
        for (datatype, expected) in cases {
            assert_eq!(elem_of_datatype(datatype), expected, "{datatype:?}");
        }
    }

    #[test]
    fn missing_layout_falls_back_to_ubyte_with_a_warning() {
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE M ""
    /begin CHARACTERISTIC c "" VALUE 0x1000 NO_SUCH_LAYOUT 0 NO_COMPU_METHOD 0 10
    /end CHARACTERISTIC
  /end MODULE
/end PROJECT
"#;
        let (file, _) = a2lfile::load_from_string(text, None, false).expect("解析通过");
        let module = file.project.module.iter().next().expect("有一个模块");
        let mut notes = Vec::new();
        let elem = elem_of_deposit(module, "NO_SUCH_LAYOUT", "c", &mut notes);
        assert_eq!(elem, Some(ElemType::U8), "按 A2L 缺省处理");
        assert!(notes.iter().any(|note| note.message().contains("缺省")));
    }

    #[test]
    fn byte_order_enum_maps_both_spellings() {
        // MsbFirst 与 LittleEndian 是 A2L 里同一件事的两种写法
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE M ""
    /begin MOD_COMMON ""
      BYTE_ORDER MSB_FIRST
    /end MOD_COMMON
  /end MODULE
/end PROJECT
"#;
        let (file, _) = a2lfile::load_from_string(text, None, false).expect("解析通过");
        let module = file.project.module.iter().next().expect("有一个模块");
        assert_eq!(byte_order(module), ByteOrder::Little);
    }

    #[test]
    fn byte_order_defaults_to_big() {
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE M ""
  /end MODULE
/end PROJECT
"#;
        let (file, _) = a2lfile::load_from_string(text, None, false).expect("解析通过");
        let module = file.project.module.iter().next().expect("有一个模块");
        assert_eq!(byte_order(module), ByteOrder::Big);
    }
}
