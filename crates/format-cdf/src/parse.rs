//! XML 到模型：结构校验、取值提取。
//!
//! 元素集的常量表就是"schema 不入库"的替代物（见 crate 文档）。只校验**元素**，
//! 不校验属性：2.0.0 的属性是文档与工具用的一致性标记，取值不受它们影响。
//!
//! 取舍：**合法但未解析的容器给提示，不合法的结构直接拒**。前者如
//! `SW-CS-HISTORY`（版次历史）——文件能用，只是我们只读当前值；后者如
//! `SW-INSTANCE` 少了 `SHORT-NAME`——后面按名映射全靠它，缺了没法补救。

use openape_cal_core::Notice;
use roxmltree::{Document, Node, ParsingOptions};

use crate::{Category, CdfDocument, Entry, Error, Scalar};

/// `MSRSW` 允许的子元素。
const MSRSW_CHILDREN: &[&str] = &["SHORT-NAME", "CATEGORY", "SW-SYSTEMS", "SDGS", "LOCS"];
/// `SW-SYSTEMS` 允许的子元素。
const SW_SYSTEMS_CHILDREN: &[&str] = &["SW-SYSTEM"];
/// `SW-SYSTEM` 允许的子元素。
const SW_SYSTEM_CHILDREN: &[&str] = &["SHORT-NAME", "LONG-NAME", "CATEGORY", "SW-INSTANCE-SPEC"];
/// `SW-INSTANCE-SPEC` 允许的子元素。
const SW_INSTANCE_SPEC_CHILDREN: &[&str] = &["SW-INSTANCE-TREE"];
/// `SW-INSTANCE-TREE` 允许的子元素。
const SW_INSTANCE_TREE_CHILDREN: &[&str] = &[
    "SHORT-NAME",
    "LONG-NAME",
    "CATEGORY",
    "SW-INSTANCE-TREE-ORIGIN",
    "SW-CS-COLLECTIONS",
    "SW-INSTANCE",
];
/// `SW-INSTANCE` 允许的子元素。
const SW_INSTANCE_CHILDREN: &[&str] = &[
    "SHORT-NAME",
    "SW-ARRAY-INDEX",
    "LONG-NAME",
    "DISPLAY-NAME",
    "CATEGORY",
    "SW-FEATURE-REF",
    "SW-VALUE-CONT",
    "SW-AXIS-CONTS",
    "SW-CS-HISTORY",
    "SW-CS-FLAGS",
    "SW-INSTANCE-PROPS-VARIANTS",
    "SW-INSTANCE",
];
/// `SW-VALUE-CONT` 允许的子元素（schema 里是两个可选的 sequence，取并集）。
const SW_VALUE_CONT_CHILDREN: &[&str] = &["UNIT-DISPLAY-NAME", "SW-ARRAYSIZE", "SW-VALUES-PHYS"];
/// `SW-VALUES-PHYS` 允许的子元素。
const SW_VALUES_PHYS_CHILDREN: &[&str] = &["VT", "V", "VG"];
/// `SW-ARRAYSIZE` 允许的子元素。
const SW_ARRAYSIZE_CHILDREN: &[&str] = &["V"];
/// `VG`（值分组）允许的子元素。
const VG_CHILDREN: &[&str] = &["LABEL", "VT", "V", "VG"];
/// `SW-INSTANCE-PROPS-VARIANTS` 允许的子元素。
const SW_INSTANCE_VARIANTS_CHILDREN: &[&str] = &["SW-INSTANCE-PROPS-VARIANT"];
/// `SW-INSTANCE-PROPS-VARIANT` 允许的子元素。
const SW_INSTANCE_VARIANT_CHILDREN: &[&str] = &[
    "SW-VCD-CRITERION-VALUES",
    "SW-VALUE-CONT",
    "SW-AXIS-CONTS",
    "SW-CS-HISTORY",
    "SW-CS-FLAGS",
];
/// 认得但本次不解析的容器：给提示而不是拒。同一份文件里每种只提示一次，
/// 否则真实导出件里每条标定量都带 SW-FEATURE-REF，提示会把真正的问题淹掉。
const UNPARSED_CONTAINERS: &[(&str, &str)] = &[
    ("SW-AXIS-CONTS", "坐标系（曲线/map 的自有轴）"),
    ("SW-CS-HISTORY", "版次历史"),
    ("SW-CS-FLAGS", "版次标记"),
    ("SW-FEATURE-REF", "特征引用"),
];

/// 解析入口。
pub(crate) fn document(text: &str) -> Result<CdfDocument, Error> {
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, options).map_err(|err| Error::Xml {
        line: usize::try_from(err.pos().row).unwrap_or(1),
        detail: err.to_string(),
    })?;

    let root = doc.root_element();
    if root.tag_name().name() != "MSRSW" {
        return Err(Error::NotCdf {
            root: root.tag_name().name().to_string(),
        });
    }

    let mut walker = Walker {
        doc: &doc,
        entries: Vec::new(),
        notes: Vec::new(),
        noted_containers: Vec::new(),
    };
    walker.check_children(root, MSRSW_CHILDREN)?;

    let mut systems = 0usize;
    for systems_node in children(root, "SW-SYSTEMS") {
        walker.check_children(systems_node, SW_SYSTEMS_CHILDREN)?;
        for system in children(systems_node, "SW-SYSTEM") {
            systems += 1;
            walker.system(system)?;
        }
    }
    if systems == 0 {
        return Err(Error::Empty {
            detail: "没有 SW-SYSTEMS/SW-SYSTEM".to_string(),
        });
    }

    Ok(CdfDocument::new(walker.entries, walker.notes))
}

/// 遍历状态：收集条目与提示。
struct Walker<'d, 'i> {
    doc: &'d Document<'i>,
    entries: Vec<Entry>,
    notes: Vec<Notice>,
    /// 已经提示过的未解析容器名：同一份文件里每种只提示一次。
    noted_containers: Vec<&'static str>,
}

impl Walker<'_, '_> {
    fn line(&self, node: Node<'_, '_>) -> usize {
        usize::try_from(self.doc.text_pos_at(node.range().start).row).unwrap_or(1)
    }

    /// 子元素必须都在允许集里；越界即拒并指出那一行。
    fn check_children(&self, node: Node<'_, '_>, allowed: &[&str]) -> Result<(), Error> {
        let parent = node.tag_name().name();
        for child in node.children().filter(Node::is_element) {
            let name = child.tag_name().name();
            if !allowed.contains(&name) {
                return Err(Error::UnexpectedElement {
                    parent: parent.to_string(),
                    element: name.to_string(),
                    line: self.line(child),
                    allowed: allowed.join("、"),
                });
            }
        }
        Ok(())
    }

    /// 处理一个 `SW-SYSTEM`。
    fn system(&mut self, node: Node<'_, '_>) -> Result<(), Error> {
        self.check_children(node, SW_SYSTEM_CHILDREN)?;
        let name = self
            .text(node, "SHORT-NAME")
            .ok_or_else(|| Error::MissingElement {
                parent: "SW-SYSTEM".to_string(),
                element: "SHORT-NAME",
                line: self.line(node),
                why: "提示与映射都要按系统名定位",
            })?;

        for spec in children(node, "SW-INSTANCE-SPEC") {
            self.check_children(spec, SW_INSTANCE_SPEC_CHILDREN)?;
            for tree in children(spec, "SW-INSTANCE-TREE") {
                self.check_children(tree, SW_INSTANCE_TREE_CHILDREN)?;
                for instance in children(tree, "SW-INSTANCE") {
                    self.instance(instance, &name, "")?;
                }
            }
        }
        Ok(())
    }

    /// 处理一个 `SW-INSTANCE`（可嵌套：结构体成员）。
    fn instance(&mut self, node: Node<'_, '_>, system: &str, parent: &str) -> Result<(), Error> {
        self.check_children(node, SW_INSTANCE_CHILDREN)?;
        let path = match self.instance_name(node)? {
            Name::Short(name) if parent.is_empty() => name,
            Name::Short(name) => format!("{parent}.{name}"),
            Name::Index(index) => format!("{parent}[{index}]"),
        };

        let category = match self.text(node, "CATEGORY") {
            Some(raw) => match Category::parse(&raw) {
                Some(known) => Some(known),
                None => {
                    // 真实导出件里确实出现过笔误（如 DEPEDNDENT_VALUE）。拒掉整个
                    // 文件会让用户拿不到其余几百条取值，所以只提示、不猜它本该是
                    // 哪个枚举值——猜错比留空更糟。
                    self.notes.push(Notice::warning(
                        &path,
                        format!(
                            "CATEGORY 值 {raw:?} 不在 CDF 2.0.0 的 16 个取值里（第 {} 行）；\
                             按未知类别保留，取值仍然导入",
                            self.line(node)
                        ),
                    ));
                    None
                }
            },
            None => None,
        };

        self.note_unparsed(node, &path);

        // 取值有两处落点：直属 SW-VALUE-CONT，或变体包裹里
        // SW-INSTANCE-PROPS-VARIANTS/SW-INSTANCE-PROPS-VARIANT/SW-VALUE-CONT
        // （真实导出件常见后者）。两处都没有才是"没有取值"。
        let direct = self.child(node, "SW-VALUE-CONT");
        let variant = if direct.is_some() {
            None
        } else {
            self.variant_value_cont(node, &path)?
        };

        if let Some(cont) = direct.or(variant) {
            let (unit, dims, values) = self.value_cont(cont, &path)?;
            let builder = CdfDocument::entry(system.to_string(), path.clone(), category);
            let entry = builder.build(unit, dims, values, self.line(node));
            if !entry.dims().is_empty() && !entry.shape_matches() {
                self.notes.push(Notice::warning(
                    &path,
                    format!(
                        "声明维度 {:?} 与取值个数 {} 对不上；按文件原样导入，写入前请人工核对",
                        entry.dims(),
                        entry.values().len()
                    ),
                ));
            }
            self.entries.push(entry);
        } else if children(node, "SW-INSTANCE").next().is_none() {
            self.notes.push(Notice::warning(
                &path,
                format!(
                    "既没有 SW-VALUE-CONT 也没有子实例，这一条没有可导入的取值（第 {} 行）",
                    self.line(node)
                ),
            ));
        }

        for nested in children(node, "SW-INSTANCE") {
            self.instance(nested, system, &path)?;
        }
        Ok(())
    }

    /// 名字要么是 `SHORT-NAME`，要么是数组下标 `SW-ARRAY-INDEX`（schema 二选一）。
    fn instance_name(&self, node: Node<'_, '_>) -> Result<Name, Error> {
        if let Some(name) = self.text(node, "SHORT-NAME") {
            return Ok(Name::Short(name));
        }
        if let Some(index) = self.text(node, "SW-ARRAY-INDEX") {
            return Ok(Name::Index(index));
        }
        Err(Error::MissingElement {
            parent: "SW-INSTANCE".to_string(),
            element: "SHORT-NAME",
            line: self.line(node),
            why: "按名映射到 A2L 标定量或 ELF 符号全靠它（数组元素用 SW-ARRAY-INDEX）",
        })
    }

    /// 未解析的合法容器：每种只提示一次——真实导出件里每条标定量都带
    /// `SW-FEATURE-REF`，逐条提示会把真正的问题淹掉。
    fn note_unparsed(&mut self, node: Node<'_, '_>, path: &str) {
        for extra in node.children().filter(Node::is_element) {
            let name = extra.tag_name().name();
            let Some((known, what)) = UNPARSED_CONTAINERS.iter().find(|(known, _)| *known == name)
            else {
                continue;
            };
            if self.noted_containers.contains(known) {
                continue;
            }
            self.noted_containers.push(known);
            self.notes.push(Notice::info(
                path,
                format!(
                    "有 {name}（{what}），本次只读当前取值，这部分未导入（第 {} 行；同一份文件里这类提示只出现一次）",
                    self.line(extra)
                ),
            ));
        }
    }

    /// 变体包裹里的取值。
    ///
    /// 多个变体时只取第一个并明说：变体按 VCD 条件区分，猜哪一个是"当前"
    /// 等于替用户决定写哪一列，猜错的代价是标定数据错值。
    fn variant_value_cont<'a, 'b>(
        &mut self,
        node: Node<'a, 'b>,
        path: &str,
    ) -> Result<Option<Node<'a, 'b>>, Error> {
        let Some(wrapper) = self.child(node, "SW-INSTANCE-PROPS-VARIANTS") else {
            return Ok(None);
        };
        self.check_children(wrapper, SW_INSTANCE_VARIANTS_CHILDREN)?;

        let mut found = None;
        let mut total = 0usize;
        for variant in children(wrapper, "SW-INSTANCE-PROPS-VARIANT") {
            self.check_children(variant, SW_INSTANCE_VARIANT_CHILDREN)?;
            self.note_unparsed(variant, path);
            total += 1;
            if found.is_none() {
                found = self.child(variant, "SW-VALUE-CONT");
            }
        }
        if total > 1 {
            self.notes.push(Notice::warning(
                path,
                format!(
                    "有 {total} 个变体（按 VCD 条件区分），只导入第 1 个的取值，其余 {} 个未导入",
                    total - 1
                ),
            ));
        }
        Ok(found)
    }

    /// `SW-VALUE-CONT`：单位、维度与取值。
    #[expect(
        clippy::type_complexity,
        reason = "三段返回值就是 CDF 的 SW-VALUE-CONT 结构本身，抽结构体只是换个名字"
    )]
    fn value_cont(
        &mut self,
        node: Node<'_, '_>,
        path: &str,
    ) -> Result<(Option<String>, Vec<usize>, Vec<Scalar>), Error> {
        self.check_children(node, SW_VALUE_CONT_CHILDREN)?;

        let unit = self
            .text(node, "UNIT-DISPLAY-NAME")
            .filter(|text| !text.is_empty());

        let mut dims = Vec::new();
        if let Some(size) = self.child(node, "SW-ARRAYSIZE") {
            self.check_children(size, SW_ARRAYSIZE_CHILDREN)?;
            for value in children(size, "V") {
                let raw = self.node_text(value);
                let parsed = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Error::BadArraySize {
                        path: path.to_string(),
                        value: raw.clone(),
                        line: self.line(value),
                    })?;
                dims.push(parsed);
            }
        }

        let mut values = Vec::new();
        let mut labels = Vec::new();
        if let Some(phys) = self.child(node, "SW-VALUES-PHYS") {
            self.values(
                phys,
                path,
                SW_VALUES_PHYS_CHILDREN,
                &mut values,
                &mut labels,
            )?;
        }
        if !labels.is_empty() {
            let mut distinct: Vec<&str> = Vec::new();
            for label in &labels {
                if !distinct.contains(&label.as_str()) {
                    distinct.push(label);
                }
            }
            self.notes.push(Notice::info(
                path,
                format!("值分组标签 {distinct:?} 被摊平，分组的层级关系未保留"),
            ));
        }
        Ok((unit, dims, values))
    }

    /// 取值：`V` 数字、`VT` 文本、`VG` 分组（摊平，顺序保持不变）。
    fn values(
        &mut self,
        node: Node<'_, '_>,
        path: &str,
        allowed: &[&str],
        out: &mut Vec<Scalar>,
        labels: &mut Vec<String>,
    ) -> Result<(), Error> {
        self.check_children(node, allowed)?;
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "V" => {
                    let raw = self.node_text(child);
                    match raw.parse::<f64>() {
                        Ok(number) => out.push(Scalar::Number(number)),
                        Err(_) => {
                            self.notes.push(Notice::warning(
                                path,
                                format!(
                                    "取值 {raw:?} 不是数字，按文本保留（第 {} 行）",
                                    self.line(child)
                                ),
                            ));
                            out.push(Scalar::Text(raw));
                        }
                    }
                }
                // LABEL 只描述分组，取值与它无关：调用方已为它给过提示。
                "LABEL" => {}
                "VT" => out.push(Scalar::Text(self.node_text(child))),
                "VG" => {
                    // 空标签在真实文件里很常见（二维 map 的内层分组），不值得提示。
                    if let Some(label) = self.text(child, "LABEL").filter(|text| !text.is_empty()) {
                        labels.push(label);
                    }
                    self.values(child, path, VG_CHILDREN, out, labels)?;
                }
                other => {
                    return Err(Error::UnexpectedElement {
                        parent: node.tag_name().name().to_string(),
                        element: other.to_string(),
                        line: self.line(child),
                        allowed: allowed.join("、"),
                    });
                }
            }
        }
        Ok(())
    }

    fn child<'a, 'b>(&self, node: Node<'a, 'b>, name: &'static str) -> Option<Node<'a, 'b>> {
        children(node, name).next()
    }

    /// 取指定子元素的去空白文本；元素不存在返回 `None`。
    fn text(&self, node: Node<'_, '_>, name: &'static str) -> Option<String> {
        self.child(node, name).map(|child| self.node_text(child))
    }

    fn node_text(&self, node: Node<'_, '_>) -> String {
        node.text().unwrap_or_default().trim().to_string()
    }
}

/// `SW-INSTANCE` 的两种命名方式。
enum Name {
    Short(String),
    Index(String),
}

fn children<'a, 'b>(node: Node<'a, 'b>, name: &'static str) -> impl Iterator<Item = Node<'a, 'b>> {
    node.children()
        .filter(move |child| child.is_element() && child.tag_name().name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str;

    /// 最小的合法 CDF：一个标量、一条曲线。
    fn minimal() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MSRSW CREATOR="hand-made">
  <SW-SYSTEMS>
    <SW-SYSTEM>
      <SHORT-NAME>Engine</SHORT-NAME>
      <SW-INSTANCE-SPEC>
        <SW-INSTANCE-TREE>
          <SHORT-NAME>Calibration</SHORT-NAME>
          <SW-INSTANCE>
            <SHORT-NAME>n_max</SHORT-NAME>
            <CATEGORY>VALUE</CATEGORY>
            <SW-VALUE-CONT>
              <UNIT-DISPLAY-NAME>rpm</UNIT-DISPLAY-NAME>
              <SW-VALUES-PHYS><V>6500</V></SW-VALUES-PHYS>
            </SW-VALUE-CONT>
          </SW-INSTANCE>
          <SW-INSTANCE>
            <SHORT-NAME>kf_tq</SHORT-NAME>
            <CATEGORY>CURVE</CATEGORY>
            <SW-VALUE-CONT>
              <SW-ARRAYSIZE><V>3</V></SW-ARRAYSIZE>
              <SW-VALUES-PHYS><V>1.0</V><V>1.5</V><V>2.0</V></SW-VALUES-PHYS>
            </SW-VALUE-CONT>
          </SW-INSTANCE>
        </SW-INSTANCE-TREE>
      </SW-INSTANCE-SPEC>
    </SW-SYSTEM>
  </SW-SYSTEMS>
</MSRSW>
"#
    }

    #[test]
    fn reads_scalar_and_curve() {
        let doc = parse_str(minimal()).expect("最小样本应能读");
        assert_eq!(doc.entries().len(), 2);

        let scalar = doc.by_path("n_max").expect("标量按短名可查");
        assert_eq!(scalar.system(), "Engine");
        assert_eq!(scalar.category(), Some(Category::Value));
        assert_eq!(scalar.unit(), Some("rpm"));
        assert_eq!(scalar.dims(), &[] as &[usize]);
        assert_eq!(scalar.values(), &[Scalar::Number(6500.0)]);

        let curve = doc.by_path("kf_tq").expect("曲线按短名可查");
        assert_eq!(curve.category(), Some(Category::Curve));
        assert_eq!(curve.dims(), &[3]);
        assert_eq!(curve.values().len(), 3);
        assert!(curve.shape_matches());
    }

    #[test]
    fn nested_instance_becomes_dotted_path() {
        let text = minimal().replace(
            "          <SW-INSTANCE>\n            <SHORT-NAME>n_max</SHORT-NAME>",
            "          <SW-INSTANCE>\n            <SHORT-NAME>curve_grp</SHORT-NAME>\n            <CATEGORY>STRUCTURE</CATEGORY>\n            <SW-INSTANCE>\n            <SHORT-NAME>n_max</SHORT-NAME>",
        );
        let text = text.replace(
            "          </SW-INSTANCE>\n          <SW-INSTANCE>\n            <SHORT-NAME>kf_tq",
            "          </SW-INSTANCE>\n          </SW-INSTANCE>\n          <SW-INSTANCE>\n            <SHORT-NAME>kf_tq",
        );
        let doc = parse_str(&text).expect("结构体成员应能读");
        assert!(
            doc.by_path("curve_grp.n_max").is_some(),
            "成员路径要带父名前缀"
        );
        assert_eq!(doc.entries().len(), 2, "结构体父本身没有取值，不产生条目");
    }

    #[test]
    fn root_must_be_msrsw() {
        let err = parse_str("<A2L/>").expect_err("别的根元素应被拒");
        assert!(matches!(err, Error::NotCdf { root } if root == "A2L"));
    }

    #[test]
    fn missing_short_name_is_rejected_with_line() {
        let text = minimal().replace("<SHORT-NAME>n_max</SHORT-NAME>", "");
        let err = parse_str(&text).expect_err("缺短名应被拒");
        match err {
            Error::MissingElement { element, line, .. } => {
                assert_eq!(element, "SHORT-NAME");
                assert!(line > 1, "报错要指到行：{line}");
            }
            other => panic!("错误类型不对：{other}"),
        }
    }

    #[test]
    fn unknown_element_is_rejected() {
        let text = minimal().replace(
            "<CATEGORY>VALUE</CATEGORY>",
            "<CATEGORY>VALUE</CATEGORY><SW-NEW-THING/>",
        );
        let err = parse_str(&text).expect_err("2.0.0 之外的元素应被拒");
        assert!(
            matches!(err, Error::UnexpectedElement { ref element, .. } if element == "SW-NEW-THING"),
            "错误类型不对：{err}"
        );
    }

    #[test]
    fn unknown_category_is_kept_as_unknown_with_warning() {
        // 真实导出件里出现过笔误（DEPEDNDENT_VALUE）。拒掉整份文件会让用户拿不到
        // 其余几百条取值，所以只提示、类别留空，不猜它原本该是哪个枚举值。
        let text = minimal().replace(
            "<CATEGORY>VALUE</CATEGORY>",
            "<CATEGORY>VALUE_BLOCK</CATEGORY>",
        );
        let doc = parse_str(&text).expect("枚举外的类别不该拒掉整份文件");
        assert_eq!(doc.by_path("n_max").expect("条目仍在").category(), None);
        assert!(
            doc.notes()
                .iter()
                .any(|n| n.message().contains("VALUE_BLOCK")),
            "要点名是哪个值：{:?}",
            doc.notes()
        );
    }

    #[test]
    fn values_inside_variant_wrapper_are_read() {
        // 真实导出件把取值放在变体包裹里：SW-INSTANCE 下没有直属 SW-VALUE-CONT。
        let text = minimal().replace(
            "<SW-VALUE-CONT>",
            "<SW-INSTANCE-PROPS-VARIANTS><SW-INSTANCE-PROPS-VARIANT><SW-VALUE-CONT>",
        );
        let text = text.replace(
            "</SW-VALUE-CONT>",
            "</SW-VALUE-CONT></SW-INSTANCE-PROPS-VARIANT></SW-INSTANCE-PROPS-VARIANTS>",
        );
        let doc = parse_str(&text).expect("变体写法应能读");
        assert_eq!(doc.entries().len(), 2, "两条都要从变体里取出来");
        assert_eq!(
            doc.by_path("n_max").expect("标量在").values()[0].as_number(),
            Some(6500.0)
        );
    }

    #[test]
    fn multiple_variants_import_the_first_and_say_so() {
        let text = minimal().replace(
            "<SW-VALUE-CONT>",
            "<SW-INSTANCE-PROPS-VARIANTS><SW-INSTANCE-PROPS-VARIANT><SW-VALUE-CONT>",
        );
        let text = text.replace(
            "</SW-VALUE-CONT>",
            "</SW-VALUE-CONT></SW-INSTANCE-PROPS-VARIANT>\
             <SW-INSTANCE-PROPS-VARIANT><SW-VALUE-CONT><SW-VALUES-PHYS><V>1</V></SW-VALUES-PHYS></SW-VALUE-CONT></SW-INSTANCE-PROPS-VARIANT>\
             </SW-INSTANCE-PROPS-VARIANTS>",
        );
        let doc = parse_str(&text).expect("多变体不该拒");
        assert!(
            doc.notes().iter().any(|n| n.message().contains("变体")),
            "要说明只导入了第一个：{:?}",
            doc.notes()
        );
    }

    #[test]
    fn undeclared_dims_do_not_warn() {
        // 真实文件常不写 SW-ARRAYSIZE：曲线只有 6 个值、没有维度声明。
        // 没有声明就无从"对不上"，警告只会变成噪声。
        let text = minimal().replace("<SW-ARRAYSIZE><V>3</V></SW-ARRAYSIZE>", "");
        let doc = parse_str(&text).expect("无维度声明应能读");
        assert!(
            !doc.notes().iter().any(|n| n.message().contains("对不上")),
            "不该报维度不符：{:?}",
            doc.notes()
        );
    }

    #[test]
    fn flat_unparsed_containers_are_noted_once_per_document() {
        // 真实导出件每条标定量都带 SW-FEATURE-REF；逐条提示会淹没真正的问题。
        let text = minimal().replace(
            "<CATEGORY>VALUE</CATEGORY>",
            "<CATEGORY>VALUE</CATEGORY><SW-FEATURE-REF>F</SW-FEATURE-REF>",
        );
        let text = text.replace(
            "<CATEGORY>CURVE</CATEGORY>",
            "<CATEGORY>CURVE</CATEGORY><SW-FEATURE-REF>F</SW-FEATURE-REF>",
        );
        let doc = parse_str(&text).expect("合法容器不该被拒");
        let notes = doc
            .notes()
            .iter()
            .filter(|n| n.message().contains("SW-FEATURE-REF"))
            .count();
        assert_eq!(notes, 1, "同一种容器只提示一次");
    }

    #[test]
    fn empty_value_group_label_is_silent() {
        let text = minimal().replace(
            "<SW-VALUES-PHYS><V>6500</V></SW-VALUES-PHYS>",
            "<SW-VALUES-PHYS><VG><LABEL/><V>6500</V></VG></SW-VALUES-PHYS>",
        );
        let doc = parse_str(&text).expect("空标签应能读");
        assert!(
            !doc.notes()
                .iter()
                .any(|n| n.message().contains("值分组标签")),
            "空标签不该提示：{:?}",
            doc.notes()
        );
    }

    #[test]
    fn bad_array_size_is_rejected() {
        let text = minimal().replace(
            "<SW-ARRAYSIZE><V>3</V></SW-ARRAYSIZE>",
            "<SW-ARRAYSIZE><V>0</V></SW-ARRAYSIZE>",
        );
        let err = parse_str(&text).expect_err("零维度应被拒");
        assert!(matches!(err, Error::BadArraySize { ref value, .. } if value == "0"));
    }

    #[test]
    fn text_value_kept_with_warning() {
        let text = minimal().replace("<V>6500</V>", "<V>6500 rpm</V>");
        let doc = parse_str(&text).expect("非数字取值不是越界，只是要提示");
        let scalar = doc.by_path("n_max").expect("条目仍在");
        assert_eq!(scalar.values(), &[Scalar::Text("6500 rpm".to_string())]);
        assert!(
            doc.notes().iter().any(|n| n.message().contains("不是数字")),
            "要给出提示：{:?}",
            doc.notes()
        );
    }

    #[test]
    fn array_size_mismatch_warns_but_imports() {
        let text = minimal().replace("<V>1.0</V><V>1.5</V><V>2.0</V>", "<V>1.0</V><V>1.5</V>");
        let doc = parse_str(&text).expect("个数对不上仍可导入");
        assert!(!doc.by_path("kf_tq").expect("条目仍在").shape_matches());
        assert!(doc.notes().iter().any(|n| n.message().contains("对不上")));
    }

    #[test]
    fn unparsed_container_gives_notice() {
        let text = minimal().replace(
            "<CATEGORY>VALUE</CATEGORY>",
            "<CATEGORY>VALUE</CATEGORY><SW-CS-HISTORY/>",
        );
        let doc = parse_str(&text).expect("合法容器不该被拒");
        assert!(doc.notes().iter().any(|n| n.message().contains("版次历史")));
    }

    #[test]
    fn empty_document_is_rejected() {
        let err = parse_str("<MSRSW><SW-SYSTEMS/></MSRSW>").expect_err("没有系统应被拒");
        assert!(matches!(err, Error::Empty { .. }));
    }

    #[test]
    fn malformed_xml_reports_line() {
        let err = parse_str("<MSRSW><SW-SYSTEMS>").expect_err("截断的文件应被拒");
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn doctype_is_tolerated() {
        let text = minimal().replace(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE MSRSW SYSTEM \"MSRSW.DTD\">",
        );
        assert!(parse_str(&text).is_ok(), "带 DOCTYPE 的真实文件不该被拒");
    }
}
