//! 把 CDF 的路径对齐到我们的变量名。
//!
//! 只按**完整路径**比，不做末段模糊匹配：CDF 的结构体成员路径是
//! `parent.member`，A2L 与 ELF 给的是扁平名，取末段会把同名成员对到错的对象上。
//! 对不上的两边都留着——用户要能看出"文件里多的是什么、我缺的是什么"。

use crate::{CdfDocument, Entry};

/// 一次对齐的结果。三份清单合起来才是全貌，所以只给读访问器，不给构造。
#[derive(Debug, Clone, Default)]
pub struct Assignment<'a> {
    matched: Vec<(&'a str, &'a Entry)>,
    cdf_only: Vec<&'a Entry>,
    name_only: Vec<&'a str>,
}

impl<'a> Assignment<'a> {
    /// 对上的：我们的名字与对应的 CDF 条目。
    pub fn matched(&self) -> &[(&'a str, &'a Entry)] {
        &self.matched
    }

    /// 只在 CDF 里出现的条目（我们的变量树里没有）。
    pub fn cdf_only(&self) -> &[&'a Entry] {
        &self.cdf_only
    }

    /// 只在我们这边出现的名字（CDF 里没给值）。
    pub fn name_only(&self) -> &[&'a str] {
        &self.name_only
    }

    /// 两边完全对齐。
    pub fn is_complete(&self) -> bool {
        self.cdf_only.is_empty() && self.name_only.is_empty()
    }
}

pub(crate) fn assign<'a>(
    doc: &'a CdfDocument,
    names: impl IntoIterator<Item = &'a str>,
) -> Assignment<'a> {
    let mut matched = Vec::new();
    let mut name_only = Vec::new();
    let mut taken: Vec<&str> = Vec::new();

    for name in names {
        match doc.by_path(name) {
            Some(entry) => {
                taken.push(entry.path());
                matched.push((name, entry));
            }
            None => name_only.push(name),
        }
    }

    let cdf_only = doc
        .entries()
        .iter()
        .filter(|entry| !taken.contains(&entry.path()))
        .collect();

    Assignment {
        matched,
        cdf_only,
        name_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> CdfDocument {
        let text = r#"<MSRSW><SW-SYSTEMS><SW-SYSTEM><SHORT-NAME>S</SHORT-NAME>
        <SW-INSTANCE-SPEC><SW-INSTANCE-TREE><SHORT-NAME>T</SHORT-NAME>
        <SW-INSTANCE><SHORT-NAME>kept</SHORT-NAME><SW-VALUE-CONT><SW-VALUES-PHYS><V>1</V></SW-VALUES-PHYS></SW-VALUE-CONT></SW-INSTANCE>
        <SW-INSTANCE><SHORT-NAME>only_in_cdf</SHORT-NAME><SW-VALUE-CONT><SW-VALUES-PHYS><V>2</V></SW-VALUES-PHYS></SW-VALUE-CONT></SW-INSTANCE>
        </SW-INSTANCE-TREE></SW-INSTANCE-SPEC></SW-SYSTEM></SW-SYSTEMS></MSRSW>"#;
        crate::parse_str(text).expect("样本合法")
    }

    #[test]
    fn splits_three_ways() {
        let doc = doc();
        let assigned = doc.assign(["kept", "only_here"]);
        assert_eq!(assigned.matched().len(), 1);
        assert_eq!(assigned.matched()[0].0, "kept");
        assert_eq!(assigned.matched()[0].1.path(), "kept");
        assert_eq!(assigned.cdf_only().len(), 1);
        assert_eq!(assigned.cdf_only()[0].path(), "only_in_cdf");
        assert_eq!(assigned.name_only(), &["only_here"]);
        assert!(!assigned.is_complete());
    }

    #[test]
    fn exact_match_only() {
        let doc = doc();
        let assigned = doc.assign(["kept.member"]);
        assert!(assigned.matched().is_empty(), "不做末段模糊匹配");
        assert_eq!(assigned.name_only(), &["kept.member"]);
    }

    #[test]
    fn full_overlap_is_complete() {
        let doc = doc();
        let assigned = doc.assign(["kept", "only_in_cdf"]);
        assert!(assigned.is_complete());
    }
}
