//! CDF 信息源：只读解析 ASAM CDF，并按名映射到我们的标定量。
//!
//! 范围由 [D-24] 定：**只读导入**，不做导出；内部 dataset 仍是 JSON（[D-08]）。
//! 于是这里没有 writer——写回 CDF 会把导入时丢掉的语义（版次、质量标记、
//! 未解析的结构体成员）悄悄抹平，用户从文件上分辨不出来。
//!
//! 两条口径必须记住：
//!
//! 1. **按 CDF 2.0.0 实现**。现行版本是 2.1.0，但官网下载只对 ASAM 会员开放；
//!    公开可得的是 2.0.0 的 XML schema。2.0.0 与 2.1.0 的差异**未验证**：解析
//!    器只认 2.0.0 的元素集，2.1.0 新增的元素会被拒并指到那一行。
//! 2. **schema 不随仓入库**。ASAM 的 XSD 头部声明版权，进不了本仓（MIT OR
//!    Apache-2.0）。"越出 schema 即拒绝"改由本 crate 的元素集校验承担：容器的
//!    允许子元素写成常量表（见 `parse`），**属性一律忽略**——2.0.0 的属性
//!    （`C`/`S`/`T`/`VIEW` 等）是给文档与工具用的一致性标记，对取值没有影响。
//!
//! 名字映射的口径：CDF 用短名定位标定量，嵌套 `SW-INSTANCE`（结构体成员）用
//! `.` 连接成路径。A2L 的标定量名与 ELF 符号名都是扁平标识符，所以**路径是
//! 一等公民**，映射按完整路径比，不做"取末段猜"的模糊匹配——猜错的代价是把
//! 值写到错的对象上。
//!
//! [D-08]: ../../../docs/decisions.md#d-08-数据集格式
//! [D-24]: ../../../docs/decisions.md#d-24-数据集对接格式改判-cdf

#![forbid(unsafe_code)]

mod assign;
mod error;
mod parse;

pub use assign::Assignment;
pub use error::Error;
pub use openape_cal_core::{Notice, NoticeSeverity as Severity};

/// 标定量的类别，取自 CDF 2.0.0 schema 里 `CATEGORY` 的枚举（共 16 个）。
///
/// 枚举取值一个不落地保留：轴类（`COM_AXIS` 等）与结构体类（`STRUCTURE` 等）
/// 我们不解析其内部，但**必须认得出名字**——认不出就会把合法文件判成越界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// 字符串标量。
    Ascii,
    /// 布尔标量。
    Boolean,
    /// 共用轴。
    ComAxis,
    /// 曲线（一维，带自己的轴）。
    Curve,
    /// 曲线数组。
    CurveArray,
    /// 曲线轴。
    CurveAxis,
    /// 从属于别的量的值。
    DependentValue,
    /// map（二维）。
    Map,
    /// map 数组。
    MapArray,
    /// 分辨率轴。
    ResAxis,
    /// 结构体。
    Structure,
    /// 结构体数组。
    StructureArray,
    /// 联合体。
    Union,
    /// 标量值。
    Value,
    /// 值数组。
    ValueArray,
    /// 值块（无轴的多值）。
    ValBlk,
}

/// `CATEGORY` 的全部合法写法，按 schema 顺序。错误信息与外层校验都用它。
pub const CATEGORIES: &[&str] = &[
    "ASCII",
    "BOOLEAN",
    "COM_AXIS",
    "CURVE",
    "CURVE_ARRAY",
    "CURVE_AXIS",
    "DEPENDENT_VALUE",
    "MAP",
    "MAP_ARRAY",
    "RES_AXIS",
    "STRUCTURE",
    "STRUCTURE_ARRAY",
    "UNION",
    "VALUE",
    "VALUE_ARRAY",
    "VAL_BLK",
];

impl Category {
    /// 按 schema 的写法解析；不在枚举里返回 `None`。
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        Some(match text {
            "ASCII" => Self::Ascii,
            "BOOLEAN" => Self::Boolean,
            "COM_AXIS" => Self::ComAxis,
            "CURVE" => Self::Curve,
            "CURVE_ARRAY" => Self::CurveArray,
            "CURVE_AXIS" => Self::CurveAxis,
            "DEPENDENT_VALUE" => Self::DependentValue,
            "MAP" => Self::Map,
            "MAP_ARRAY" => Self::MapArray,
            "RES_AXIS" => Self::ResAxis,
            "STRUCTURE" => Self::Structure,
            "STRUCTURE_ARRAY" => Self::StructureArray,
            "UNION" => Self::Union,
            "VALUE" => Self::Value,
            "VALUE_ARRAY" => Self::ValueArray,
            "VAL_BLK" => Self::ValBlk,
            _ => return None,
        })
    }

    /// schema 里的写法。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascii => "ASCII",
            Self::Boolean => "BOOLEAN",
            Self::ComAxis => "COM_AXIS",
            Self::Curve => "CURVE",
            Self::CurveArray => "CURVE_ARRAY",
            Self::CurveAxis => "CURVE_AXIS",
            Self::DependentValue => "DEPENDENT_VALUE",
            Self::Map => "MAP",
            Self::MapArray => "MAP_ARRAY",
            Self::ResAxis => "RES_AXIS",
            Self::Structure => "STRUCTURE",
            Self::StructureArray => "STRUCTURE_ARRAY",
            Self::Union => "UNION",
            Self::Value => "VALUE",
            Self::ValueArray => "VALUE_ARRAY",
            Self::ValBlk => "VAL_BLK",
        }
    }

    /// 中文名，供界面与日志显示。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ascii => "字符串",
            Self::Boolean => "布尔",
            Self::ComAxis => "共用轴",
            Self::Curve => "曲线",
            Self::CurveArray => "曲线数组",
            Self::CurveAxis => "曲线轴",
            Self::DependentValue => "从属值",
            Self::Map => "map",
            Self::MapArray => "map 数组",
            Self::ResAxis => "分辨率轴",
            Self::Structure => "结构体",
            Self::StructureArray => "结构体数组",
            Self::Union => "联合体",
            Self::Value => "标量",
            Self::ValueArray => "值数组",
            Self::ValBlk => "值块",
        }
    }
}

/// 一个取值。数字与文本分开存，不把非数字强行转成 `f64`。
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// 数值（物理值，单位见 [`Entry::unit`]）。
    Number(f64),
    /// 文本（`ASCII` 类标定量的内容，或 `VT` 元素）。
    Text(String),
}

impl Scalar {
    /// 当数值读；文本返回 `None`。
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(v) => Some(*v),
            Self::Text(_) => None,
        }
    }

    /// 当文本读；数值转成十进制字符串。
    pub fn as_text(&self) -> String {
        match self {
            Self::Number(v) => v.to_string(),
            Self::Text(t) => t.clone(),
        }
    }
}

/// 一条标定量。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    system: String,
    path: String,
    category: Option<Category>,
    unit: Option<String>,
    dims: Vec<usize>,
    values: Vec<Scalar>,
    line: usize,
}

impl Entry {
    /// 所属 `SW-SYSTEM` 的短名。
    pub fn system(&self) -> &str {
        &self.system
    }

    /// 标定量路径：顶层是短名，结构体成员用 `.` 连接。
    pub fn path(&self) -> &str {
        &self.path
    }

    /// 类别。schema 里 `CATEGORY` 可选，缺省时保持 `None` 而不猜。
    pub fn category(&self) -> Option<Category> {
        self.category
    }

    /// 单位（`UNIT-DISPLAY-NAME` 的文本）。
    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    /// 数组维度，空表示标量。
    pub fn dims(&self) -> &[usize] {
        &self.dims
    }

    /// 取值，按文件里的顺序。
    pub fn values(&self) -> &[Scalar] {
        &self.values
    }

    /// 该条目在文件里的行号（1 起），报错与界面定位都用它。
    pub fn line(&self) -> usize {
        self.line
    }

    /// 声明维度与取值个数是否对得上。schema 管不到这一层（没有 keyref），
    /// 于是它只是提示项，不是拒绝项。
    pub fn shape_matches(&self) -> bool {
        let expected: usize = self.dims.iter().product::<usize>().max(1);
        expected == self.values.len()
    }
}

/// 一次解析的结果。
#[derive(Debug, Clone)]
pub struct CdfDocument {
    entries: Vec<Entry>,
    notes: Vec<Notice>,
}

impl CdfDocument {
    /// 全部标定量，按文件里的顺序（不做重排：导入结果要能对着原文件核）。
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 读入期提示：合法但不完整、或做了降级处理的地方。
    pub fn notes(&self) -> &[Notice] {
        &self.notes
    }

    /// 按路径取一条。
    pub fn by_path(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// 与我们的变量名对齐。见 [`Assignment`]。
    pub fn assign<'a>(&'a self, names: impl IntoIterator<Item = &'a str>) -> Assignment<'a> {
        assign::assign(self, names)
    }

    pub(crate) fn new(entries: Vec<Entry>, notes: Vec<Notice>) -> Self {
        Self { entries, notes }
    }

    pub(crate) fn entry(system: String, path: String, category: Option<Category>) -> EntryBuilder {
        EntryBuilder {
            system,
            path,
            category,
        }
    }
}

/// 只给 `parse` 用的构造器：字段太多，逐个位置参数容易错位。
pub(crate) struct EntryBuilder {
    system: String,
    path: String,
    category: Option<Category>,
}

impl EntryBuilder {
    pub(crate) fn build(
        self,
        unit: Option<String>,
        dims: Vec<usize>,
        values: Vec<Scalar>,
        line: usize,
    ) -> Entry {
        Entry {
            system: self.system,
            path: self.path,
            category: self.category,
            unit,
            dims,
            values,
            line,
        }
    }
}

/// 解析一份 CDF 文本。
pub fn parse_str(text: &str) -> Result<CdfDocument, Error> {
    parse::document(text)
}

/// 解析一份 CDF 文件。
pub fn parse_file(path: impl AsRef<std::path::Path>) -> Result<CdfDocument, Error> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|err| Error::Io {
        path: path.display().to_string(),
        detail: err.to_string(),
    })?;
    parse::document(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_roundtrip_covers_schema_enum() {
        assert_eq!(CATEGORIES.len(), 16);
        for name in CATEGORIES {
            let parsed = Category::parse(name).expect("schema 枚举里的写法都该认得");
            assert_eq!(parsed.as_str(), *name);
        }
    }

    #[test]
    fn category_rejects_unknown_spelling() {
        assert!(Category::parse("VALUE_BLOCK").is_none());
        assert!(Category::parse("value").is_none());
    }

    #[test]
    fn scalar_keeps_text_as_text() {
        let text = Scalar::Text("1.0O".to_string());
        assert_eq!(text.as_number(), None);
        assert_eq!(text.as_text(), "1.0O");
    }
}
