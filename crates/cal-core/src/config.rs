//! 工程配置：从 TOML 读出轴与值的绑定声明。
//!
//! 配置只是**声明**，不是事实来源：解析出来的长度与元素类型一律回查变量
//! 树（见 [`crate::ir::axis::BindingSpec`]）。这里只管把文本变成声明，并把
//! 坏输入挡在门口——纯函数，不碰文件系统，IO 由外壳负责。
//!
//! 三条约定来自 [10-axis-binding-rfc] 第 4 节：带版本字段、容忍未知字段
//! （将来加节不炸旧程序）、`[[curve]]` 与 `[[map]]` 是同一模型的两个入口。
//!
//! [10-axis-binding-rfc]: ../../../../docs/10-axis-binding-rfc.md

use serde::Deserialize;

use crate::ir::Symbol;
use crate::ir::axis::{AxisKind, AxisSpec, BindingSpec, Id, ValueSpec};

/// 本软件支持的配置版本。
pub const SCHEMA_VERSION: u32 = 1;

/// 一份工程配置：版本加绑定声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    schema_version: u32,
    binding: BindingSpec,
}

impl ProjectConfig {
    /// 从 TOML 文本解析。
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let wire: ConfigWire = toml::from_str(text).map_err(|err| ConfigError::Syntax {
            line: err.span().map(|span| line_of(text, span.start)),
            message: err.message().to_owned(),
        })?;
        let schema_version = wire.schema_version.ok_or(ConfigError::MissingVersion)?;
        if schema_version > SCHEMA_VERSION {
            return Err(ConfigError::VersionTooNew {
                found: schema_version,
                supported: SCHEMA_VERSION,
            });
        }

        let axes = wire
            .axis
            .into_iter()
            .map(AxisWire::into_spec)
            .collect::<Result<Vec<_>, _>>()?;
        let values = wire
            .curve
            .into_iter()
            .chain(wire.map)
            .map(ValueWire::into_spec)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            schema_version,
            binding: BindingSpec::new(axes, values),
        })
    }

    /// 配置版本。旧版配置原样保留版本号，便于回写时不被悄悄升级。
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// 绑定声明。
    pub fn binding(&self) -> &BindingSpec {
        &self.binding
    }

    /// 取出绑定声明（消费自身）。
    pub fn into_binding(self) -> BindingSpec {
        self.binding
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

/// TOML 的线格式。字段名与节名就是对外契约。
#[derive(Deserialize)]
struct ConfigWire {
    schema_version: Option<u32>,
    #[serde(default)]
    axis: Vec<AxisWire>,
    #[serde(default)]
    curve: Vec<ValueWire>,
    #[serde(default)]
    map: Vec<ValueWire>,
}

#[derive(Deserialize)]
struct AxisWire {
    id: String,
    symbol: String,
    #[serde(default)]
    kind: AxisKind,
}

impl AxisWire {
    fn into_spec(self) -> Result<AxisSpec, ConfigError> {
        let id = Id::new(&self.id).ok_or_else(|| ConfigError::BadName {
            field: "axis.id",
            value: self.id.clone(),
        })?;
        let symbol = Symbol::new(&self.symbol).ok_or_else(|| ConfigError::BadName {
            field: "axis.symbol",
            value: self.symbol.clone(),
        })?;
        Ok(AxisSpec::new(id, symbol).with_kind(self.kind))
    }
}

#[derive(Deserialize)]
struct ValueWire {
    id: String,
    symbol: String,
    #[serde(default)]
    axes: Vec<String>,
}

impl ValueWire {
    fn into_spec(self) -> Result<ValueSpec, ConfigError> {
        let id = Id::new(&self.id).ok_or_else(|| ConfigError::BadName {
            field: "curve.id / map.id",
            value: self.id.clone(),
        })?;
        let symbol = Symbol::new(&self.symbol).ok_or_else(|| ConfigError::BadName {
            field: "curve.symbol / map.symbol",
            value: self.symbol.clone(),
        })?;
        let axes = self
            .axes
            .into_iter()
            .map(|raw| {
                Id::new(&raw).ok_or_else(|| ConfigError::BadName {
                    field: "axes[]",
                    value: raw.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ValueSpec::new(id, symbol, axes))
    }
}

/// 配置读取失败。每条都带"怎么改"的提示——只说"解析失败"等于把问题
/// 丢回给用户。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// TOML 语法错误。
    #[error("配置第 {line} 行语法错误：{message}", line = line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_owned()))]
    Syntax {
        /// 出错行号；拿不到时为 `None`。
        line: Option<usize>,
        /// 解析器给的说明。
        message: String,
    },
    /// 缺少版本字段。
    #[error("配置缺少 schema_version，请在第一行写上 schema_version = {SCHEMA_VERSION}")]
    MissingVersion,
    /// 版本比本软件新。
    #[error(
        "配置版本 {found} 高于本软件支持的 {supported}，请升级软件；若配置来自更新版本，其新节会被旧软件静默忽略，不要强行读取"
    )]
    VersionTooNew {
        /// 配置里写的版本。
        found: u32,
        /// 本软件支持的版本。
        supported: u32,
    },
    /// id 或符号为空。
    #[error("字段 {field} 的 \"{value}\" 不合法：不能为空，且首尾不能有空白")]
    BadName {
        /// 出错的字段路径。
        field: &'static str,
        /// 原始文本。
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 第 8 节的配置原文。
    const GOLDEN: &str = r#"
schema_version = 1

[[axis]]
id = "x_speed"
symbol = "cal_axis_x"

[[axis]]
id = "y_temp"
symbol = "cal_axis_y"

[[map]]
id = "eff_map"
symbol = "cal_map"
axes = ["y_temp", "x_speed"]
"#;

    fn id(raw: &str) -> Id {
        Id::new(raw).expect("测试 id 合法")
    }

    fn sym(raw: &str) -> Symbol {
        Symbol::new(raw).expect("测试符号合法")
    }

    #[test]
    fn golden_config_parses_into_expected_binding() {
        let config = ProjectConfig::from_toml(GOLDEN).expect("golden 必读");
        assert_eq!(config.schema_version(), 1);
        let binding = config.binding();
        assert_eq!(binding.axes().len(), 2);
        assert_eq!(binding.values().len(), 1);

        let axis = &binding.axes()[0];
        assert_eq!(axis.id(), &id("x_speed"));
        assert_eq!(axis.symbol(), &sym("cal_axis_x"));
        assert_eq!(axis.kind(), AxisKind::Shared);

        let value = &binding.values()[0];
        assert_eq!(value.id(), &id("eff_map"));
        assert_eq!(value.axes(), &[id("y_temp"), id("x_speed")]);
    }

    #[test]
    fn curve_and_map_sections_feed_the_same_model() {
        let text = r#"
schema_version = 1

[[axis]]
id = "x"
symbol = "cal_axis_x"

[[curve]]
id = "limiter"
symbol = "cal_limiter"
axes = ["x"]

[[map]]
id = "m"
symbol = "cal_map"
axes = ["x", "x"]
"#;
        let config = ProjectConfig::from_toml(text).expect("两节都收");
        assert_eq!(config.binding().values().len(), 2);
        assert_eq!(config.binding().values()[0].id(), &id("limiter"));
        assert_eq!(config.binding().values()[1].id(), &id("m"));
    }

    #[test]
    fn unknown_fields_and_sections_are_ignored() {
        // 旧程序读新配置：新节、新字段一律不报错（RFC 第 4 节）
        let text = r#"
schema_version = 1
future_top_level = "x"

[[axis]]
id = "x_speed"
symbol = "cal_axis_x"
future_field = 42

[[whatever]]
id = "ignored"
"#;
        let config = ProjectConfig::from_toml(text).expect("未知字段必须容忍");
        assert_eq!(config.binding().axes().len(), 1);
        assert!(config.binding().values().is_empty());
    }

    #[test]
    fn missing_version_is_actionable() {
        let err = ProjectConfig::from_toml("[[axis]]\nid = \"a\"\nsymbol = \"b\"\n")
            .expect_err("没版本必须报");
        assert_eq!(err, ConfigError::MissingVersion);
        assert!(err.to_string().contains("schema_version = 1"), "{err}");
    }

    #[test]
    fn newer_version_is_rejected_with_explanation() {
        let err = ProjectConfig::from_toml("schema_version = 2\n").expect_err("高版本必须报");
        match err {
            ConfigError::VersionTooNew { found, supported } => {
                assert_eq!((found, supported), (2, 1));
            }
            other => panic!("期望 VersionTooNew，得到 {other:?}"),
        }
        assert!(err.to_string().contains("请升级软件"), "{err}");
    }

    #[test]
    fn syntax_error_reports_line_number() {
        let text = "schema_version = 1\n\n[[axis]\nid = \"a\"\n";
        let err = ProjectConfig::from_toml(text).expect_err("语法错必须报");
        match &err {
            ConfigError::Syntax { line, .. } => {
                assert_eq!(*line, Some(3), "行号要指向出错的第 3 行：{err}");
            }
            other => panic!("期望 Syntax，得到 {other:?}"),
        }
        assert!(err.to_string().contains("第 3 行"), "{err}");
    }

    #[test]
    fn empty_names_are_rejected_with_field_path() {
        let text = "schema_version = 1\n\n[[axis]]\nid = \"\"\nsymbol = \"cal_axis_x\"\n";
        let err = ProjectConfig::from_toml(text).expect_err("空 id 必须报");
        match &err {
            ConfigError::BadName { field, .. } => assert_eq!(*field, "axis.id"),
            other => panic!("期望 BadName，得到 {other:?}"),
        }
        assert!(err.to_string().contains("axis.id"), "{err}");
    }

    #[test]
    fn empty_axis_reference_is_rejected() {
        let text = r#"
schema_version = 1

[[curve]]
id = "c"
symbol = "cal_limiter"
axes = ["x", ""]
"#;
        let err = ProjectConfig::from_toml(text).expect_err("空轴引用必须报");
        assert!(
            matches!(
                err,
                ConfigError::BadName {
                    field: "axes[]",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn kind_defaults_to_shared_and_accepts_inline() {
        let text = r#"
schema_version = 1

[[axis]]
id = "a"
symbol = "s1"

[[axis]]
id = "b"
symbol = "s2"
kind = "inline"
"#;
        let config = ProjectConfig::from_toml(text).expect("kind 可选");
        assert_eq!(config.binding().axes()[0].kind(), AxisKind::Shared);
        assert_eq!(config.binding().axes()[1].kind(), AxisKind::Inline);
    }

    #[test]
    fn config_feeds_resolve_and_surfaces_rule_errors() {
        // 端到端：配置写反了顺序，报错要能指到 R1 并给出修法
        use crate::ir::{ElemType, Variable, VariableTree};
        use crate::types::Address;

        let tree = VariableTree::new(vec![
            Variable::new(sym("cal_map"), Address::new(0x2060), ElemType::F32)
                .with_dims(vec![4, 3])
                .expect("合法"),
            Variable::new(sym("cal_axis_x"), Address::new(0x2040), ElemType::F32)
                .with_dims(vec![3])
                .expect("合法"),
            Variable::new(sym("cal_axis_y"), Address::new(0x2030), ElemType::F32)
                .with_dims(vec![4])
                .expect("合法"),
        ])
        .expect("无重复");

        let swapped = GOLDEN.replace(
            "axes = [\"y_temp\", \"x_speed\"]",
            "axes = [\"x_speed\", \"y_temp\"]",
        );
        let config = ProjectConfig::from_toml(&swapped).expect("语法没问题");
        let err = crate::ir::AxisBinding::resolve(&tree, config.binding())
            .expect_err("顺序错必须被 R1 拦住");
        assert!(err.to_string().contains("R1"), "{err}");
        assert!(err.to_string().contains("确认 axes 顺序"), "{err}");
    }

    #[test]
    fn config_resolves_against_tree() {
        use crate::ir::{AxisBinding, ElemType, Variable, VariableTree};
        use crate::types::Address;

        let tree = VariableTree::new(vec![
            Variable::new(sym("cal_map"), Address::new(0x2060), ElemType::F32)
                .with_dims(vec![4, 3])
                .expect("合法"),
            Variable::new(sym("cal_axis_x"), Address::new(0x2040), ElemType::F32)
                .with_dims(vec![3])
                .expect("合法"),
            Variable::new(sym("cal_axis_y"), Address::new(0x2030), ElemType::F32)
                .with_dims(vec![4])
                .expect("合法"),
        ])
        .expect("无重复");
        let config = ProjectConfig::from_toml(GOLDEN).expect("golden 必读");
        let binding = AxisBinding::resolve(&tree, &config.into_binding()).expect("golden 必通过");
        assert_eq!(binding.values().len(), 1);
    }
}
