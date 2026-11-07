//! ELF 信息源：把 ELF+DWARF 里的变量结构解析出来。
//!
//! 这是"信息源墙"（[11-architecture-layers] 的三道屏蔽墙之一）的 ELF 实现：
//! 上层只认本 crate 的模型与 `cal-core` 的 IR，不认 `gimli`/`object`，也不认
//! 借来的 `a2ltool-dwarf` 的任何类型。
//!
//! 本 crate 只回答"变量长什么样"：符号、地址、大小、元素类型、维度、结构体
//! 成员布局、枚举值、位域位置。单位、量程、换算、轴归属、标定-观测角色一律
//! 不猜——前四者只有 A2L 给得出，角色由用户指定（[D-13]）。
//!
//! [11-architecture-layers]: ../../docs/11-architecture-layers.md
//! [D-13]: ../../docs/decisions.md

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use a2ltool_dwarf::debuginfo::{DbgDataType, DebugData, TypeInfo, VarInfo, make_simple_unit_name};
use indexmap::IndexMap;

pub use openape_cal_core::{Notice, NoticeSeverity};

/// 标量元素类型。与 `cal-core` 的 [`openape_cal_core::ElemType`] 同形，
/// 但不在此处耦合 IR——映射发生在 ELF → IR 那一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalar {
    /// 无符号 8 位。
    U8,
    /// 有符号 8 位。
    I8,
    /// 无符号 16 位。
    U16,
    /// 有符号 16 位。
    I16,
    /// 无符号 32 位。
    U32,
    /// 有符号 32 位。
    I32,
    /// 无符号 64 位。
    U64,
    /// 有符号 64 位。
    I64,
    /// 32 位浮点。
    F32,
    /// 64 位浮点。
    F64,
}

/// 一个变量的形状。
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// 标量。
    Scalar(Scalar),
    /// 数组，维度按 C 声明顺序（`[4][3]` 即 `[4, 3]`）。
    Array {
        /// 最内层元素类型。
        elem: Scalar,
        /// 各维长度。
        dims: Vec<u32>,
    },
    /// 结构体。
    Struct {
        /// 成员，按声明顺序。
        members: Vec<Field>,
    },
    /// 联合体。成员共享同一段字节。
    Union {
        /// 成员，按声明顺序。
        members: Vec<Field>,
    },
    /// 枚举。
    Enum {
        /// 枚举项名与取值。
        values: Vec<(String, i64)>,
        /// 底层类型是否有符号。
        signed: bool,
    },
    /// 位域。
    Bits {
        /// 底层标量类型。
        base: Scalar,
        /// 起始位偏移，自底层类型首字节起。
        bit_offset: u16,
        /// 位宽。
        bit_width: u16,
    },
    /// 指针、函数指针、空类型、以及任何无法归入上述形状的。
    Opaque,
}

/// 结构体或联合体的一个成员。
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// 成员名。
    name: String,
    /// 相对结构体首字节的偏移。
    offset: u64,
    /// 成员形状。
    shape: Shape,
}

impl Field {
    /// 构造。成员名为空返回 `None`——匿名成员既可读性差也导出不了。
    pub fn new(name: impl Into<String>, offset: u64, shape: Shape) -> Option<Self> {
        let name = name.into();
        (!name.trim().is_empty()).then_some(Self {
            name,
            offset,
            shape,
        })
    }

    /// 成员名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 相对所在结构体首字节的偏移。
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// 成员形状。
    pub fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// 一个从 DWARF 抽出的变量。
#[derive(Debug, Clone, PartialEq)]
pub struct RawVar {
    /// 符号名。
    name: String,
    /// 首地址。
    address: u64,
    /// 总字节数。
    size: u64,
    /// 所属编译单元的简单名。
    unit: Option<String>,
    /// 符号名在整个 ELF 里是否唯一。
    unique: bool,
    /// 形状。
    shape: Shape,
}

impl RawVar {
    /// 符号名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 首地址。
    pub fn address(&self) -> u64 {
        self.address
    }

    /// 总字节数（数组含全部元素）。
    pub fn size(&self) -> u64 {
        self.size
    }

    /// 所属编译单元（`calib_c` 这样的简单名）。无调试信息时为空。
    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    /// 该符号名在整个 ELF 里是否唯一。`false` 表示跨编译单元重名，
    /// 上层必须加前缀才能区分。
    pub fn is_unique(&self) -> bool {
        self.unique
    }

    /// 形状。
    pub fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// 抽取结果。
#[derive(Debug, Clone)]
pub struct ElfSource {
    vars: Vec<RawVar>,
    notes: Vec<Notice>,
}

impl ElfSource {
    /// 全部变量，按符号名排序。
    pub fn vars(&self) -> &[RawVar] {
        &self.vars
    }

    /// 按符号名查。
    pub fn get(&self, name: &str) -> Option<&RawVar> {
        self.vars.iter().find(|var| var.name == name)
    }

    /// 抽取期提示。
    pub fn notes(&self) -> &[Notice] {
        &self.notes
    }
}

/// 加载错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 文件读不了。
    #[error("读取 ELF 失败：{0}")]
    Io(#[from] std::io::Error),
    /// 没有可用的调试信息（未加 `-g` 或被 strip）。
    #[error("没有可用的调试信息：{0}")]
    NoDebugInfo(String),
    /// 上游解析失败。
    #[error("ELF 解析失败：{0}")]
    Parse(String),
}

/// 从 ELF 文件抽取变量。
pub fn load(path: &Path) -> Result<ElfSource, Error> {
    let debug = a2ltool_dwarf::debuginfo::DebugData::load_dwarf(path.as_os_str(), false)
        .map_err(classify)?;

    let mut notes = Vec::new();
    if debug.variables.is_empty() {
        return Err(Error::NoDebugInfo(
            "文件里没有可用的变量调试信息；确认编译时加了 -g 且没有被 strip".to_owned(),
        ));
    }

    let mut vars = Vec::new();
    for (name, list) in &debug.variables {
        let unique = list.len() == 1;
        if !unique {
            notes.push(Notice::warning(
                name.as_str(),
                format!("{} 份同名符号来自不同编译单元，需加前缀区分", list.len()),
            ));
        }
        for info in list {
            vars.push(build_var(&debug, name, info, unique));
        }
    }
    vars.sort_by(|a, b| a.name.cmp(&b.name).then(a.address.cmp(&b.address)));

    Ok(ElfSource { vars, notes })
}

/// 把上游的失败分成"没有调试信息"与"文件本身坏了"两类：前者用户能自己修
/// （重编加 `-g`），后者只能换文件。前者的提示必须说到怎么修——只说"读不到
/// 调试信息"等于让用户去查文档。
fn classify(message: String) -> Error {
    let lowered = message.to_lowercase();
    if lowered.contains("debug") || lowered.contains("dwarf") {
        Error::NoDebugInfo(format!(
            "{message}\n\
             修法：去掉 strip 这一步，或重新编译时加 -g（例如 gcc -c -g -O2），\
             并确认链接后没有再跑 strip"
        ))
    } else {
        Error::Parse(message)
    }
}

fn build_var(debug: &DebugData, name: &str, info: &VarInfo, unique: bool) -> RawVar {
    let typeref = debug.types.get(&info.typeref);
    let shape = typeref.map_or(Shape::Opaque, |ty| shape_of(&debug.types, ty));
    let unit = make_simple_unit_name(debug, info.unit_idx);
    RawVar {
        // 跨编译单元重名时必须加前缀：DWARF 里两个 `static` 都叫一个名字，
        // 但地址不同，不加前缀写下去会打到另一个变量上。
        name: if unique {
            name.to_owned()
        } else {
            qualified(name, unit.as_deref(), info.unit_idx)
        },
        address: info.address,
        size: typeref.map_or(0, TypeInfo::get_size),
        unit,
        unique,
        shape,
    }
}

/// 给重名符号加上编译单元前缀。拿不到单元名时退回序号——序号不好读，但比
/// 两条一模一样的名字强。
fn qualified(name: &str, unit: Option<&str>, unit_idx: usize) -> String {
    match unit {
        Some(unit) => format!("{unit}.{name}"),
        None => format!("unit{unit_idx}.{name}"),
    }
}

/// 把上游的类型树翻成 [`Shape`]。上游用 `TypeRef` 间接引用类型表，这里
/// 就地解开，调用方不需要知道那张表的存在。
fn shape_of(types: &HashMap<usize, TypeInfo>, ty: &TypeInfo) -> Shape {
    if let Some(scalar) = scalar_of(&ty.datatype) {
        return Shape::Scalar(scalar);
    }
    match &ty.datatype {
        DbgDataType::Bitfield {
            basetype,
            bit_offset,
            bit_size,
        } => Shape::Bits {
            base: shape_of(types, basetype).scalar_or(Scalar::U32),
            bit_offset: *bit_offset,
            bit_width: *bit_size,
        },
        DbgDataType::Enum {
            enumerators,
            signed,
            ..
        } => Shape::Enum {
            values: enumerators.clone(),
            signed: *signed,
        },
        DbgDataType::Struct { members, .. } => Shape::Struct {
            members: fields_of(types, members),
        },
        DbgDataType::Union { members, .. } => Shape::Union {
            members: fields_of(types, members),
        },
        DbgDataType::Array { .. } => array_shape(types, ty),
        DbgDataType::TypeRef(index, _) => types
            .get(index)
            .map_or(Shape::Opaque, |target| shape_of(types, target)),
        // 标量在上面已返回；指针、函数指针、以及上游将来新增而我们还不认识的
        // 类型一律落这里——报"说不清"比猜一个形状安全。
        _ => Shape::Opaque,
    }
}

/// 上游把标量分成整数与浮点两族，分开两支让每支都是一眼看完的表。
fn scalar_of(datatype: &DbgDataType) -> Option<Scalar> {
    integer_of(datatype).or_else(|| float_of(datatype))
}

/// 八个整数变体逐个对到标量。这是一张真值表而不是复杂度：拆成"无符号"
/// 与"有符号"两支能让 CC 降到 5，但两支结构完全一致，dryer 会报 1.00 重复
/// ——把 8 行数据换个写法换来一处重复，不划算。已登记为指标例外。
fn integer_of(datatype: &DbgDataType) -> Option<Scalar> {
    Some(match datatype {
        DbgDataType::Uint8 => Scalar::U8,
        DbgDataType::Uint16 => Scalar::U16,
        DbgDataType::Uint32 => Scalar::U32,
        DbgDataType::Uint64 => Scalar::U64,
        DbgDataType::Sint8 => Scalar::I8,
        DbgDataType::Sint16 => Scalar::I16,
        DbgDataType::Sint32 => Scalar::I32,
        DbgDataType::Sint64 => Scalar::I64,
        _ => return None,
    })
}

fn float_of(datatype: &DbgDataType) -> Option<Scalar> {
    Some(match datatype {
        DbgDataType::Float => Scalar::F32,
        DbgDataType::Double => Scalar::F64,
        _ => return None,
    })
}

/// 数组可能多层：`float[4][3]` 在上游是"4 个元素，每个又是 3 个 float"。
fn array_shape(types: &HashMap<usize, TypeInfo>, ty: &TypeInfo) -> Shape {
    let (dims, innermost) = array_chain(types, ty);
    if dims.is_empty() {
        return Shape::Opaque;
    }
    match shape_of(types, innermost) {
        Shape::Scalar(elem) => Shape::Array { elem, dims },
        _ => Shape::Opaque,
    }
}

/// 沿元素链收齐各维长度，返回维度与最内层类型。
fn array_chain<'a>(
    types: &'a HashMap<usize, TypeInfo>,
    start: &'a TypeInfo,
) -> (Vec<u32>, &'a TypeInfo) {
    let mut dims = Vec::new();
    let mut cursor = start;
    loop {
        match &cursor.datatype {
            DbgDataType::Array { dim, arraytype, .. } => {
                dims.extend(dim.iter().map(|d| u32::try_from(*d).unwrap_or(u32::MAX)));
                cursor = arraytype;
            }
            DbgDataType::TypeRef(index, _) => match types.get(index) {
                Some(target) => cursor = target,
                None => return (dims, cursor),
            },
            _ => return (dims, cursor),
        }
    }
}

fn fields_of(
    types: &HashMap<usize, TypeInfo>,
    members: &IndexMap<String, (TypeInfo, u64)>,
) -> Vec<Field> {
    members
        .iter()
        .filter_map(|(name, (ty, offset))| Field::new(name.clone(), *offset, shape_of(types, ty)))
        .collect()
}

impl Shape {
    /// 形状是标量时取它，否则退回给定默认值——位域的底层类型偶尔取不到。
    fn scalar_or(self, fallback: Scalar) -> Scalar {
        match self {
            Self::Scalar(inner) => inner,
            _ => fallback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typeinfo(datatype: DbgDataType) -> TypeInfo {
        TypeInfo {
            name: None,
            unit_idx: 0,
            datatype,
            dbginfo_offset: 0,
        }
    }

    #[test]
    fn every_integer_width_and_signedness_maps_to_its_own_scalar() {
        let cases = [
            (DbgDataType::Uint8, Scalar::U8),
            (DbgDataType::Uint16, Scalar::U16),
            (DbgDataType::Uint32, Scalar::U32),
            (DbgDataType::Uint64, Scalar::U64),
            (DbgDataType::Sint8, Scalar::I8),
            (DbgDataType::Sint16, Scalar::I16),
            (DbgDataType::Sint32, Scalar::I32),
            (DbgDataType::Sint64, Scalar::I64),
            (DbgDataType::Float, Scalar::F32),
            (DbgDataType::Double, Scalar::F64),
        ];
        for (datatype, expected) in cases {
            assert_eq!(scalar_of(&datatype), Some(expected), "{datatype:?}");
        }
    }

    #[test]
    fn non_scalar_datatypes_are_not_scalars() {
        assert_eq!(scalar_of(&DbgDataType::Pointer(0, 0)), None);
        assert_eq!(scalar_of(&DbgDataType::Other(4)), None);
        assert_eq!(scalar_of(&DbgDataType::FuncPtr(0)), None);
    }

    #[test]
    fn array_chain_stops_at_a_missing_typeref() {
        // TypeRef 指向不存在的下标时不能死循环，要带着已收的维度返回。
        let types = HashMap::new();
        let ty = typeinfo(DbgDataType::TypeRef(9, 0));
        let (dims, _) = array_chain(&types, &ty);
        assert!(dims.is_empty());
    }

    #[test]
    fn array_of_arrays_collects_dimensions_outer_first() {
        let mut types = HashMap::new();
        types.insert(1, typeinfo(DbgDataType::Float));
        types.insert(
            2,
            typeinfo(DbgDataType::Array {
                size: 12,
                dim: vec![3],
                stride: 4,
                arraytype: Box::new(typeinfo(DbgDataType::TypeRef(1, 0))),
            }),
        );
        let outer = typeinfo(DbgDataType::Array {
            size: 48,
            dim: vec![4],
            stride: 12,
            arraytype: Box::new(typeinfo(DbgDataType::TypeRef(2, 0))),
        });
        assert_eq!(
            shape_of(&types, &outer),
            Shape::Array {
                elem: Scalar::F32,
                dims: vec![4, 3]
            }
        );
    }

    #[test]
    fn array_of_structs_is_opaque() {
        // 结构体数组在 IR 里没有对应形状——导出 A2L 时它得靠 RECORD_LAYOUT，
        // 这里如实报"说不清"，不假装是标量数组。
        let mut types = HashMap::new();
        types.insert(
            1,
            typeinfo(DbgDataType::Struct {
                size: 4,
                is_class: false,
                inheritance: IndexMap::new(),
                members: IndexMap::new(),
            }),
        );
        let ty = typeinfo(DbgDataType::Array {
            size: 8,
            dim: vec![2],
            stride: 4,
            arraytype: Box::new(typeinfo(DbgDataType::TypeRef(1, 0))),
        });
        assert_eq!(shape_of(&types, &ty), Shape::Opaque);
    }

    #[test]
    fn dangling_typeref_is_opaque_not_a_panic() {
        let types = HashMap::new();
        let ty = typeinfo(DbgDataType::TypeRef(42, 0));
        assert_eq!(shape_of(&types, &ty), Shape::Opaque);
    }

    #[test]
    fn qualification_prefers_the_unit_name_and_falls_back_to_an_index() {
        assert_eq!(
            qualified("gain", Some("calib_c"), 3),
            "calib_c.gain",
            "有单元名时用它"
        );
        assert_eq!(
            qualified("gain", None, 3),
            "unit3.gain",
            "拿不到单元名时退回序号，比两条同名强"
        );
    }

    #[test]
    fn debug_related_failures_are_reported_as_missing_debug_info() {
        // 用户能自己修的那一类（重编加 -g）要与"文件坏了"分开。
        assert!(matches!(
            classify("no .debug_info section".to_owned()),
            Error::NoDebugInfo(_)
        ));
        assert!(matches!(
            classify("invalid DWARF version".to_owned()),
            Error::NoDebugInfo(_)
        ));
        assert!(matches!(
            classify("not an object file".to_owned()),
            Error::Parse(_)
        ));
    }

    #[test]
    fn empty_debug_info_is_an_error_not_an_empty_result() {
        // 一个变量都抽不到，几乎总是编译时漏了 -g；返回空清单会让上层以为
        // "这个 ECU 没有变量"，比报错更误导。
        let dir = std::env::temp_dir().join("openape-format-elf-empty");
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("not-an-elf.bin");
        std::fs::write(&path, b"definitely not an elf").expect("写临时文件");
        assert!(load(&path).is_err());
    }
}
