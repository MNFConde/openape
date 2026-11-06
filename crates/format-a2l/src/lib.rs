//! A2L 信息源：把 A2L 解析成 `cal-core` 的变量树与轴绑定。
//!
//! 这是"信息源墙"（[11-architecture-layers] 的三道屏蔽墙之一）的 A2L 实现：
//! 上层只认 `cal-core` 的 IR，不认 `a2lfile` 的任何类型。类型名、枚举与块
//! 结构全部在这里消化掉。
//!
//! 两档输入的差别也在这里落地：A2L 档给得出单位、量程与换算，于是 IR 的
//! 语义字段被填满；仅 ELF 档给不出，那些字段保持为空（[D-13]）。
//!
//! [11-architecture-layers]: ../../docs/11-architecture-layers.md
//! [D-13]: ../../docs/decisions.md

#![forbid(unsafe_code)]

mod convert;
mod error;
mod layout;
mod notice;

pub use error::Error;
pub use notice::{Notice, Severity};

use openape_cal_core::{AxisBinding, VariableTree};

use a2lfile::A2lObjectName;

/// 解析结果：变量树、轴绑定与读入期的提示。
#[derive(Debug, Clone)]
pub struct A2lSource {
    module: String,
    tree: VariableTree,
    binding: AxisBinding,
    notes: Vec<Notice>,
}

impl A2lSource {
    /// 命中的模块名。A2L 可能描述多个 ECU，这里只取一个。
    pub fn module(&self) -> &str {
        &self.module
    }

    /// 变量树，含标定量、观测量与被引用的轴。
    pub fn tree(&self) -> &VariableTree {
        &self.tree
    }

    /// 轴与值的绑定。
    pub fn binding(&self) -> &AxisBinding {
        &self.binding
    }

    /// 读入期提示：跳过的对象、降级的语义、`a2lfile` 的告警。
    pub fn notes(&self) -> &[Notice] {
        &self.notes
    }
}

/// 解析 A2L 文本。
///
/// `module` 为 `None` 时要求文件里只有一个模块；多个模块且未指名时列出来
/// 让调用方选，不猜。
pub fn load_str(text: &str, module: Option<&str>) -> Result<A2lSource, Error> {
    let (file, messages) = a2lfile::load_from_string(text, None, false)
        .map_err(|err| Error::Parse(err.to_string()))?;

    let mut notes: Vec<Notice> = messages
        .iter()
        .map(|message| Notice::warning("a2lfile", message.to_string()))
        .collect();

    let selected = select_module(&file, module)?;
    let (tree, binding) = convert::build(selected, &mut notes)?;
    Ok(A2lSource {
        module: selected.get_name().to_owned(),
        tree,
        binding,
        notes,
    })
}

fn select_module<'a>(
    file: &'a a2lfile::A2lFile,
    wanted: Option<&str>,
) -> Result<&'a a2lfile::Module, Error> {
    if let Some(name) = wanted {
        return file
            .project
            .module
            .iter()
            .find(|module| module.get_name() == name)
            .ok_or_else(|| Error::ModuleNotFound {
                wanted: name.to_owned(),
                available: available_modules(file),
            });
    }
    let mut modules = file.project.module.iter();
    match (modules.next(), modules.next()) {
        (Some(only), None) => Ok(only),
        _ => Err(Error::ModuleAmbiguous {
            available: available_modules(file),
        }),
    }
}

fn available_modules(file: &a2lfile::A2lFile) -> Vec<String> {
    file.project
        .module
        .iter()
        .map(|module| module.get_name().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn golden() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/a2l/axis-binding-golden.a2l"
        );
        std::fs::read_to_string(path).expect("golden 夹具必须存在")
    }

    use openape_cal_core as cal_core;

    #[test]
    fn golden_a2l_matches_expected_ir() {
        // 验收（M2-04）：M0-5 golden 的解析结果与 RFC 第 8 节期望一致
        let source = load_str(&golden(), None).expect("golden 必须能读");
        assert_eq!(source.module(), "ECU");

        let binding = source.binding();
        assert_eq!(binding.axes().len(), 2);
        assert_eq!(binding.values().len(), 1);

        let x = binding
            .axis(&cal_core::Id::new("x_speed").expect("合法"))
            .expect("有 x_speed");
        assert_eq!(x.points(), 3);
        assert_eq!(
            x.symbol().as_str(),
            "x_speed",
            "golden 没写 SYMBOL_LINK，符号即对象名"
        );
        assert_eq!(x.address(), cal_core::Address::new(0x2040));
        assert_eq!(x.elem(), cal_core::ElemType::F32);

        let y = binding
            .axis(&cal_core::Id::new("y_temp").expect("合法"))
            .expect("有 y_temp");
        assert_eq!(y.points(), 4);
        assert_eq!(y.address(), cal_core::Address::new(0x2030));

        let map = binding
            .value(&cal_core::Id::new("eff_map").expect("合法"))
            .expect("有 eff_map");
        assert_eq!(map.dims(), &[4, 3], "MATRIX_DIM 3 4 逆序成 dims [4, 3]");
        assert_eq!(
            map.axes(),
            &[
                cal_core::Id::new("y_temp").expect("合法"),
                cal_core::Id::new("x_speed").expect("合法")
            ],
            "AXIS_DESCR 是 x 在前，dims 顺序要反转"
        );
        assert_eq!(map.address(), cal_core::Address::new(0x2060));
        assert_eq!(map.kind(), cal_core::ValueKind::Map);
    }

    #[test]
    fn golden_variables_carry_semantics() {
        let source = load_str(&golden(), None).expect("golden 必须能读");
        let tree = source.tree();
        assert_eq!(tree.len(), 3, "两条轴加一个 Map");

        let map = tree
            .get(&cal_core::Symbol::new("eff_map").expect("合法"))
            .expect("树里有 eff_map");
        assert_eq!(map.dims(), &[4, 3]);
        assert_eq!(map.elem(), cal_core::ElemType::F32);
        assert_eq!(map.byte_len().map(cal_core::ByteLen::get), Some(48));
        assert_eq!(map.role(), Some(cal_core::Role::Characteristic));

        let range = map.semantics().range().expect("A2L 给了量程");
        assert_eq!(range.min(), -1.0e9);
        assert_eq!(range.max(), 1.0e9);
        assert!(
            map.semantics().unit().is_none(),
            "golden 里 NO_COMPU_METHOD 没有单位"
        );
    }

    #[test]
    fn golden_notes_are_empty() {
        let source = load_str(&golden(), None).expect("golden 必须能读");
        assert!(
            source.notes().is_empty(),
            "严格夹具不该有告警：{:?}",
            source.notes()
        );
    }

    #[test]
    fn binding_serializes_to_rfc_shape() {
        // RFC 第 8 节快照里的 symbol 是 ELF 的 C 符号（cal_axis_x），那份快照
        // 来自"ELF 加配置"路径；A2L 路径没有 SYMBOL_LINK 时符号就是对象名。
        // 这里锁的是形状与轴值关系，标志名差异不影响两者互相对应。
        let source = load_str(&golden(), None).expect("golden 必须能读");
        let json = serde_json::to_value(source.binding()).expect("可序列化");
        let expected = serde_json::json!({
            "axes": [
                {"id": "x_speed", "symbol": "x_speed", "len": 3, "kind": "shared", "elem": "f32"},
                {"id": "y_temp", "symbol": "y_temp", "len": 4, "kind": "shared", "elem": "f32"}
            ],
            "values": [
                {"id": "eff_map", "symbol": "eff_map", "kind": "map",
                 "dims": [4, 3], "axes": ["y_temp", "x_speed"], "elem": "f32"}
            ],
            "diagnostics": []
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn symbol_link_wins_over_object_name() {
        // 带 SYMBOL_LINK 时符号取 C 符号名，与 ELF 对得上
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE ECU ""
    /begin RECORD_LAYOUT RL
      FNC_VALUES 1 FLOAT32_IEEE ROW_DIR DIRECT
    /end RECORD_LAYOUT
    /begin AXIS_PTS x_obj "" 0x2040 NO_INPUT_QUANTITY RL 0 NO_COMPU_METHOD 3 -1.0E9 1.0E9
      SYMBOL_LINK "cal_axis_x" 0
    /end AXIS_PTS
    /begin CHARACTERISTIC c_obj "" CURVE 0x2100 RL 0 NO_COMPU_METHOD -1.0E9 1.0E9
      SYMBOL_LINK "cal_curve" 0
      /begin AXIS_DESCR COM_AXIS NO_INPUT_QUANTITY NO_COMPU_METHOD 3 -1.0E9 1.0E9
        AXIS_PTS_REF x_obj
      /end AXIS_DESCR
    /end CHARACTERISTIC
  /end MODULE
/end PROJECT
"#;
        let source = load_str(text, None).expect("要能读");
        let tree = source.tree();
        assert!(
            tree.get(&cal_core::Symbol::new("cal_axis_x").expect("合法"))
                .is_some(),
            "轴变量要按 C 符号名入树：{:?}",
            tree.iter()
                .map(|v| v.symbol().to_string())
                .collect::<Vec<_>>()
        );
        let curve = tree
            .get(&cal_core::Symbol::new("cal_curve").expect("合法"))
            .expect("曲线按 C 符号名入树");
        assert_eq!(curve.dims(), &[3]);
        let value = source
            .binding()
            .value(&cal_core::Id::new("c_obj").expect("合法"))
            .expect("值按对象名作 id");
        assert_eq!(value.symbol().as_str(), "cal_curve");
    }

    #[test]
    fn curve_with_com_axis_resolves_shared_axis() {
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE ECU ""
    /begin RECORD_LAYOUT RL
      FNC_VALUES 1 FLOAT32_IEEE ROW_DIR DIRECT
    /end RECORD_LAYOUT
    /begin AXIS_PTS x_speed "" 0x2040 NO_INPUT_QUANTITY RL 0 NO_COMPU_METHOD 3 -1.0E9 1.0E9
    /end AXIS_PTS
    /begin CHARACTERISTIC limiter "" CURVE 0x2200 RL 0 NO_COMPU_METHOD -1.0E9 1.0E9
      /begin AXIS_DESCR COM_AXIS NO_INPUT_QUANTITY NO_COMPU_METHOD 3 -1.0E9 1.0E9
        AXIS_PTS_REF x_speed
      /end AXIS_DESCR
    /end CHARACTERISTIC
  /end MODULE
/end PROJECT
"#;
        let source = load_str(text, None).expect("要能读");
        let value = source
            .binding()
            .value(&cal_core::Id::new("limiter").expect("合法"))
            .expect("有 limiter");
        assert_eq!(value.kind(), cal_core::ValueKind::Curve);
        assert_eq!(value.dims(), &[3]);
        assert_eq!(value.axes(), &[cal_core::Id::new("x_speed").expect("合法")]);
    }

    #[test]
    fn unsupported_characteristic_type_is_skipped_with_notice() {
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE ECU ""
    /begin RECORD_LAYOUT RL
      FNC_VALUES 1 FLOAT32_IEEE ROW_DIR DIRECT
    /end RECORD_LAYOUT
    /begin CHARACTERISTIC cube "" CUBOID 0x2300 RL 0 NO_COMPU_METHOD -1.0E9 1.0E9
      MATRIX_DIM 2 2 2
    /end CHARACTERISTIC
  /end MODULE
/end PROJECT
"#;
        let source = load_str(text, None).expect("跳过不是错误");
        assert!(source.binding().values().is_empty());
        assert!(
            source
                .notes()
                .iter()
                .any(|note| note.message().contains("Cuboid")),
            "要说清楚跳过了什么：{:?}",
            source.notes()
        );
    }

    #[test]
    fn duplicate_symbol_is_rejected() {
        let text = r#"
ASAP2_VERSION 1 71
/begin PROJECT P ""
  /begin MODULE ECU ""
    /begin MEASUREMENT same "" UWORD NO_COMPU_METHOD 0 0 0 0
      ECU_ADDRESS 0x1000
    /end MEASUREMENT
    /begin CHARACTERISTIC same "" VALUE 0x1004 RL 0 NO_COMPU_METHOD 0 10
    /end CHARACTERISTIC
  /end MODULE
/end PROJECT
"#;
        let err = load_str(text, None).expect_err("同名观测与标定必须报");
        assert!(matches!(err, Error::DuplicateSymbol { .. }), "{err:?}");
        assert!(err.to_string().contains("same"), "{err}");
    }

    #[test]
    fn missing_module_is_reported_with_candidates() {
        let err = load_str(&golden(), Some("NoSuchEcu")).expect_err("模块名不对要报");
        let text = err.to_string();
        assert!(text.contains("NoSuchEcu"), "{text}");
        assert!(text.contains("ECU"), "要列出可选模块：{text}");
    }

    #[test]
    fn broken_text_is_reported_as_parse_error() {
        let err = load_str("/begin PROJECT P", None).expect_err("残缺文件要报");
        assert!(matches!(err, Error::Parse(_)), "{err:?}");
    }
}
