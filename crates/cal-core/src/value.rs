//! 原始字节与数值之间的换算。
//!
//! 三件事共用这一份：界面显示物理值、写入前做量程校验、CLI 把用户给的数值
//! 编成字节。三处各写一份迟早出现"界面说没越界、写入说越界"。
//!
//! 换算链固定为 **字节 → 原始值（按元素类型与字节序）→ 物理值（按 scale）**，
//! 反向走 **物理值 → 原始值（反解 scale）→ 字节**。没有 scale 时原始值就是
//! 物理值，即仅 ELF 档的 `[raw]` 语义：不编造单位与换算。

use crate::ir::{ByteOrder, ElemType, Semantics};

/// 把原始字节解成数值。长度必须正好等于元素宽度，多余或不足都返回 `None`。
pub fn decode(raw: &[u8], elem: ElemType, order: ByteOrder) -> Option<f64> {
    let width = elem.width().get() as usize;
    if raw.len() != width {
        return None;
    }
    let mut bytes = [0u8; 8];
    match order {
        ByteOrder::Big => bytes[8 - width..].copy_from_slice(raw),
        ByteOrder::Little => bytes[..width].copy_from_slice(raw),
    }
    let value = match (elem, order) {
        (ElemType::U8, _) => f64::from(bytes[7]),
        (ElemType::I8, _) => f64::from(bytes[7] as i8),
        (ElemType::U16, ByteOrder::Big) => f64::from(u16::from_be_bytes([bytes[6], bytes[7]])),
        (ElemType::U16, ByteOrder::Little) => f64::from(u16::from_le_bytes([bytes[0], bytes[1]])),
        (ElemType::I16, ByteOrder::Big) => f64::from(i16::from_be_bytes([bytes[6], bytes[7]])),
        (ElemType::I16, ByteOrder::Little) => f64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        (ElemType::U32, _) => f64::from(read_u32(&bytes, order)),
        (ElemType::I32, _) => f64::from(read_u32(&bytes, order) as i32),
        (ElemType::U64, _) => read_u64(&bytes, order) as f64,
        (ElemType::I64, _) => read_u64(&bytes, order) as i64 as f64,
        (ElemType::F32, _) => f64::from(f32::from_bits(read_u32(&bytes, order))),
        (ElemType::F64, _) => f64::from_bits(read_u64(&bytes, order)),
    };
    Some(value)
}

/// 把数值编成原始字节。整数类型超出范围即返回 `None`——静默截断会让写入
/// 落成另一个值，而写后读回又"一致"，最难查。
pub fn encode(value: f64, elem: ElemType, order: ByteOrder) -> Option<Vec<u8>> {
    if !value.is_finite() && !matches!(elem, ElemType::F32 | ElemType::F64) {
        return None;
    }
    let width = elem.width().get() as usize;
    let mut out = match elem {
        ElemType::U8 => vec![to_unsigned(value, f64::from(u8::MAX))? as u8],
        ElemType::I8 => vec![to_signed(value, f64::from(i8::MIN), f64::from(i8::MAX))? as i8 as u8],
        ElemType::U16 => (to_unsigned(value, f64::from(u16::MAX))? as u16)
            .to_be_bytes()
            .to_vec(),
        ElemType::I16 => (to_signed(value, f64::from(i16::MIN), f64::from(i16::MAX))? as i16)
            .to_be_bytes()
            .to_vec(),
        ElemType::U32 => (to_unsigned(value, f64::from(u32::MAX))? as u32)
            .to_be_bytes()
            .to_vec(),
        ElemType::I32 => (to_signed(value, f64::from(i32::MIN), f64::from(i32::MAX))? as i32)
            .to_be_bytes()
            .to_vec(),
        ElemType::U64 => {
            let rounded = value.round();
            if !rounded.is_finite() || rounded < 0.0 || rounded > u64::MAX as f64 {
                return None;
            }
            (rounded as u64).to_be_bytes().to_vec()
        }
        ElemType::I64 => {
            let rounded = value.round();
            if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
                return None;
            }
            (rounded as i64).to_be_bytes().to_vec()
        }
        ElemType::F32 => {
            let narrowed = value as f32;
            if !narrowed.is_finite() && value.is_finite() {
                return None;
            }
            narrowed.to_bits().to_be_bytes().to_vec()
        }
        ElemType::F64 => value.to_bits().to_be_bytes().to_vec(),
    };
    debug_assert_eq!(out.len(), width, "编码宽度必须等于元素宽度");
    if order == ByteOrder::Little {
        out.reverse();
    }
    Some(out)
}

/// 字节 → 物理值。没有 scale 时物理值等于原始值（仅 ELF 档的语义）。
pub fn to_physical(
    raw: &[u8],
    elem: ElemType,
    order: ByteOrder,
    semantics: &Semantics,
) -> Option<f64> {
    let raw_value = decode(raw, elem, order)?;
    Some(match semantics.scale() {
        Some(scale) => scale.apply(raw_value),
        None => raw_value,
    })
}

/// 物理值 → 字节。反解 scale，再编码。
pub fn from_physical(
    value: f64,
    elem: ElemType,
    order: ByteOrder,
    semantics: &Semantics,
) -> Option<Vec<u8>> {
    let raw_value = match semantics.scale() {
        Some(crate::ir::Scale::Linear { factor, offset }) => {
            if factor == 0.0 || !factor.is_finite() {
                return None;
            }
            (value - offset) / factor
        }
        None => value,
    };
    encode(raw_value, elem, order)
}

fn read_u32(bytes: &[u8; 8], order: ByteOrder) -> u32 {
    let window = match order {
        ByteOrder::Big => [bytes[4], bytes[5], bytes[6], bytes[7]],
        ByteOrder::Little => [bytes[0], bytes[1], bytes[2], bytes[3]],
    };
    u32::from_be_bytes(window)
}

fn read_u64(bytes: &[u8; 8], order: ByteOrder) -> u64 {
    match order {
        ByteOrder::Big => u64::from_be_bytes(*bytes),
        ByteOrder::Little => {
            let mut window = [0u8; 8];
            window.copy_from_slice(&bytes[..8]);
            u64::from_le_bytes(window)
        }
    }
}

fn to_unsigned(value: f64, max: f64) -> Option<u64> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= 0.0 && rounded <= max).then_some(rounded as u64)
}

fn to_signed(value: f64, min: f64, max: f64) -> Option<i64> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= min && rounded <= max).then_some(rounded as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Range, Scale};

    #[test]
    fn u16_round_trips_in_both_byte_orders() {
        let bytes = encode(0x1234 as f64, ElemType::U16, ByteOrder::Big).expect("编得出");
        assert_eq!(bytes, vec![0x12, 0x34]);
        assert_eq!(
            decode(&bytes, ElemType::U16, ByteOrder::Big),
            Some(0x1234 as f64)
        );

        let little = encode(0x1234 as f64, ElemType::U16, ByteOrder::Little).expect("编得出");
        assert_eq!(little, vec![0x34, 0x12]);
        assert_eq!(
            decode(&little, ElemType::U16, ByteOrder::Little),
            Some(0x1234 as f64)
        );
    }

    #[test]
    fn signed_values_keep_their_sign() {
        let bytes = encode(-2.0, ElemType::I16, ByteOrder::Big).expect("编得出");
        assert_eq!(bytes, vec![0xFF, 0xFE]);
        assert_eq!(decode(&bytes, ElemType::I16, ByteOrder::Big), Some(-2.0));
        assert_eq!(decode(&[0xFF], ElemType::I8, ByteOrder::Big), Some(-1.0));
    }

    #[test]
    fn floats_round_trip() {
        let bytes = encode(1.5, ElemType::F32, ByteOrder::Big).expect("编得出");
        assert_eq!(decode(&bytes, ElemType::F32, ByteOrder::Big), Some(1.5));
        let bytes = encode(-0.25, ElemType::F64, ByteOrder::Little).expect("编得出");
        assert_eq!(
            decode(&bytes, ElemType::F64, ByteOrder::Little),
            Some(-0.25)
        );
    }

    #[test]
    fn integers_refuse_values_outside_their_range() {
        assert!(encode(70_000.0, ElemType::U16, ByteOrder::Big).is_none());
        assert!(encode(-1.0, ElemType::U8, ByteOrder::Big).is_none());
        assert!(encode(200.0, ElemType::I8, ByteOrder::Big).is_none());
        assert!(encode(f64::NAN, ElemType::U16, ByteOrder::Big).is_none());
    }

    #[test]
    fn fractional_input_rounds_to_the_nearest_integer() {
        let bytes = encode(2.6, ElemType::U8, ByteOrder::Big).expect("编得出");
        assert_eq!(bytes, vec![3]);
    }

    #[test]
    fn wrong_length_is_refused() {
        assert_eq!(decode(&[1], ElemType::U16, ByteOrder::Big), None);
        assert_eq!(decode(&[1, 2, 3], ElemType::U16, ByteOrder::Big), None);
    }

    #[test]
    fn scale_is_applied_and_undone() {
        let semantics = Semantics::none().with_scale(Scale::Linear {
            factor: 0.5,
            offset: -10.0,
        });
        let bytes = from_physical(-8.0, ElemType::U16, ByteOrder::Big, &semantics).expect("编得出");
        assert_eq!(bytes, vec![0x00, 0x04], "(−8 − (−10)) / 0.5 = 4");
        assert_eq!(
            to_physical(&bytes, ElemType::U16, ByteOrder::Big, &semantics),
            Some(-8.0)
        );
    }

    #[test]
    fn without_scale_the_physical_value_is_the_raw_value() {
        let semantics = Semantics::none();
        assert_eq!(
            to_physical(&[0x00, 0x64], ElemType::U16, ByteOrder::Big, &semantics),
            Some(100.0),
            "仅 ELF 档就是 raw：不编造换算"
        );
    }

    #[test]
    fn range_contains_both_ends() {
        let range = Range::new(1.0, 10.0).expect("合法量程");
        assert!(range.contains(1.0) && range.contains(10.0));
        assert!(!range.contains(0.9) && !range.contains(10.1));
    }
}
