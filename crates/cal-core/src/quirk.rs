//! 怪癖扩展点（D-06，M5-06）：类 ECU 的行为差异用声明式 TOML 填 trait，
//! 改配置不改代码。
//!
//! 分工：[`QuirkConfig`] 是配置形状（serde 反序列化 + 加载入口），[`Quirks`]
//! 是行为扩展点（trait，规范默认实现在方法里），[`ConfiguredQuirks`] 把
//! 配置填进 trait——master 与从站各自消费同一份差异声明，核心代码不内置
//! 任何具体 ECU 的怪癖。
//!
//! MVP 覆盖两个可从 vcan 复现的差异面（M5-07 注入验收）：
//!
//! - **DTO 填充**：实长帧（默认，标准允许）还是补齐到 max_cto，缺的字节
//!   补什么。
//! - **页回显**：GET_CAL_PAGE 响应按标准带保留位，还是只回页号的短响应。
//!
//! 坏配置要给可操作提示：字段名打错在反序列化时就报（`deny_unknown_fields`），
//! 错误文本带位置、原因与期望字段清单，不留"静默忽略拼错的字段"这条路。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// GET_CAL_PAGE 响应的回显格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageEcho {
    /// 标准：命令码后带两个保留字节再回页号（`[保留, 保留, 页号]`）。
    #[default]
    Standard,
    /// 短响应：命令码后只回页号。见过真实从站这么发，master 解析按
    /// `response[2]` 取页号会读不到。
    Short,
}

/// 一份怪癖配置。字段全带规范默认——只写与默认不同的差异，配置文件保持
/// 短。未知字段直接拒绝，见模块文档。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuirkConfig {
    /// DTO 补齐到 max_cto（CAN 经典帧 8 字节）。默认 false：按数据实长发帧。
    #[serde(default)]
    pub pad_dtos: bool,
    /// 补齐时填充的字节。默认 0x00；有的从站补 0xFF。只在 `pad_dtos` 为
    /// true 时生效。
    #[serde(default)]
    pub dto_padding: u8,
    /// GET_CAL_PAGE 响应格式。默认 [`PageEcho::Standard`]。
    #[serde(default)]
    pub page_echo: PageEcho,
}

/// 配置加载失败。toml 的错误已带行列号与原因，这里补一句可操作的指引。
#[derive(Debug, Error)]
#[error("怪癖配置解析失败：{source}\n提示：字段只有 pad_dtos / dto_padding / page_echo；page_echo 取 standard 或 short；完整示例见 openape-cal-core::QuirkConfig 文档")]
pub struct QuirkError {
    #[source]
    source: toml::de::Error,
}

impl QuirkConfig {
    /// 从 TOML 文本加载。拼错的字段名、类型不匹配都在这里报，配置错误
    /// 不留到运行时才暴露。
    pub fn from_toml(text: &str) -> Result<Self, QuirkError> {
        toml::from_str(text).map_err(|source| QuirkError { source })
    }

    /// 把配置填进扩展点。
    pub fn quirks(self) -> ConfiguredQuirks {
        ConfiguredQuirks(self)
    }
}

/// 怪癖扩展点：消费方按这里的回答决定行为差异，不写 if-ecu 分支。
///
/// 规范默认直接落在方法缺省实现上——没配怪的 ECU 拿到的全是标准行为，
/// 具体怪癖由 [`ConfiguredQuirks`] 用配置覆盖。新差异面 = 加一个带默认
/// 实现的方法 + 配置字段，消费方各自接线。
pub trait Quirks: Send + Sync {
    /// DTO 是否补齐到 max_cto。默认 false（实长帧）。
    fn pad_dtos(&self) -> bool {
        false
    }

    /// 补齐时用的填充字节。默认 0x00。
    fn dto_padding(&self) -> u8 {
        0x00
    }

    /// GET_CAL_PAGE 的回显格式。默认标准。
    fn page_echo(&self) -> PageEcho {
        PageEcho::Standard
    }
}

/// 配置填好的扩展点。构造只经 [`QuirkConfig::quirks`]，保证 trait 的回答
/// 与配置一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredQuirks(QuirkConfig);

impl Quirks for ConfiguredQuirks {
    fn pad_dtos(&self) -> bool {
        self.0.pad_dtos
    }

    fn dto_padding(&self) -> u8 {
        self.0.dto_padding
    }

    fn page_echo(&self) -> PageEcho {
        self.0.page_echo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验收线：类 ECU 差异改配置不改代码——两份 TOML 两种行为，路径里
    /// 没有任何新代码。
    #[test]
    fn config_alone_changes_the_behavior() {
        let standard = QuirkConfig::from_toml("").expect("空配置即全默认").quirks();
        let odd = QuirkConfig::from_toml(
            "pad_dtos = true\ndto_padding = 255\npage_echo = \"short\"\n",
        )
        .expect("示例配置必须能加载")
        .quirks();
        let odd: Box<dyn Quirks> = Box::new(odd);

        assert!(!standard.pad_dtos());
        assert_eq!(standard.dto_padding(), 0x00);
        assert_eq!(standard.page_echo(), PageEcho::Standard);

        assert!(odd.pad_dtos());
        assert_eq!(odd.dto_padding(), 0xFF);
        assert_eq!(odd.page_echo(), PageEcho::Short);
    }

    #[test]
    fn partial_config_keeps_defaults_for_the_rest() {
        let quirks = QuirkConfig::from_toml("page_echo = \"short\"\n")
            .expect("只写差异也要能加载")
            .quirks();
        assert!(!quirks.pad_dtos(), "没写的字段保持规范默认");
        assert_eq!(quirks.page_echo(), PageEcho::Short);
    }

    /// 坏配置给可操作提示：拼错的字段名当场报，并指认合法字段。
    #[test]
    fn unknown_field_is_rejected_with_the_field_list() {
        let err = QuirkConfig::from_toml("padding = 255\n").expect_err("拼错字段必须报");
        let text = err.to_string();
        assert!(text.contains("padding"), "错误要复述拼错的字段：{text}");
        assert!(text.contains("dto_padding"), "错误要给出合法字段：{text}");
        assert!(text.contains("提示"), "{text}");
    }

    #[test]
    fn wrong_value_type_reports_location() {
        let err = QuirkConfig::from_toml("dto_padding = \"fast\"\n").expect_err("类型错必须报");
        assert!(err.to_string().contains("dto_padding"), "{err}");
    }

    #[test]
    fn page_echo_round_trips_lowercase() {
        let text = toml::to_string(&QuirkConfig {
            page_echo: PageEcho::Short,
            ..QuirkConfig::default()
        })
        .expect("可序列化");
        let back = QuirkConfig::from_toml(&text).expect("往返后仍可加载");
        assert_eq!(
            back,
            QuirkConfig {
                page_echo: PageEcho::Short,
                ..QuirkConfig::default()
            }
        );
    }
}
