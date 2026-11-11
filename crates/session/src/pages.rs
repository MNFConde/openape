//! 标定页：页间取样、差异与按项复制。
//!
//! 页切换在协议层是 [`crate::engine::Session::set_cal_page`]。本模块管的是
//! 用户真正要的两件事：**看两页差在哪**、**把差异搬过去**。
//!
//! 复制没有"整页覆盖"形态：[`Session::copy_to_working_page`] 要求显式列出
//! 要复制的标定量，空列表直接拒绝。整页覆盖正是页机制要避免的事故——一
//! 次误操作就把工作页上所有手工调整抹掉。
//!
//! 每个复制项都走 [`crate::engine::Session::write_one`]，于是同样带写前读回、
//! 快照与审计，与单点写入没有两条路。

use openape_cal_core::{
    Dataset, Difference, Entry, Role, SourceMode, Symbol, Timestamp, Variable, VariableTree, diff,
};
use openape_diag::{Domain, Fault, FaultCode, Severity};

use crate::engine::Session;
use crate::seq;
use crate::write::{WriteContext, Written};

/// 该变量是否落在标定页上。
///
/// 角色明确是观测量的排除；角色未定（仅 ELF 档，D-13 不做角色判定）一律
/// 当作标定量——用户要的是"页上这些值差在哪"，宁多读也不少读。
fn on_calibration_page(variable: &Variable) -> bool {
    variable.role() != Some(Role::Measurement)
}

/// 从变量树里挑出页上要取样的变量。
pub fn calibration_variables(tree: &VariableTree) -> impl Iterator<Item = &Variable> {
    tree.iter().filter(|variable| on_calibration_page(variable))
}

impl Session {
    /// 切到 `page` 并把树里的标定量读成一份 dataset。
    ///
    /// 读完后会话停在该页上——两次取样之间不要夹其它操作，否则读到的会是
    /// 混合状态。
    pub async fn page_dataset(
        &mut self,
        tree: &VariableTree,
        page: u8,
        mode: SourceMode,
    ) -> Result<Dataset, Fault> {
        self.set_cal_page(page).await?;
        let mut entries = Vec::new();
        for variable in calibration_variables(tree) {
            let len = variable.byte_len().ok_or_else(|| {
                page_fault(format!(
                    "{} 的维度不合法，算不出页上取样长度",
                    variable.symbol()
                ))
            })?;
            let raw = self.read(variable.address(), len).await?;
            if let Some(entry) = Entry::new(variable.symbol().clone(), variable.address(), raw) {
                entries.push(entry);
            }
        }
        Dataset::new(mode, Timestamp::now(), entries)
            .map_err(|err| page_fault(format!("页 {page} 的取样组装失败：{err}")))
    }

    /// 比较两个页，只列差异地址。
    ///
    /// 两页的字节都在同一档输入下取样，所以这里不会遇到模式不一致；若遇到，
    /// 说明调用方混用了两档，按 dataset 的错误如实报出。
    pub async fn diff_pages(
        &mut self,
        tree: &VariableTree,
        reference: u8,
        working: u8,
        mode: SourceMode,
    ) -> Result<Vec<Difference>, Fault> {
        let left = self.page_dataset(tree, reference, mode).await?;
        let right = self.page_dataset(tree, working, mode).await?;
        diff(&left, &right).map_err(|err| page_fault(err.to_string()))
    }

    /// 把参考页的值复制到工作页，**只复制明确列出的项**。
    ///
    /// `approved` 为空即拒绝：复制必须是一次被确认的逐项决定，不是整页覆盖。
    pub async fn copy_to_working_page(
        &mut self,
        tree: &VariableTree,
        reference: &Dataset,
        working: u8,
        approved: &[Symbol],
        context: &WriteContext,
    ) -> Result<Vec<Written>, Fault> {
        if approved.is_empty() {
            return Err(
                page_fault("复制要明确列出目标标定量：没有整页覆盖入口".to_owned())
                    .with_hint("先跑页对比，按符号逐项确认后再复制"),
            );
        }
        self.set_cal_page(working).await?;
        let mut written = Vec::with_capacity(approved.len());
        for symbol in approved {
            let variable = tree
                .get(symbol)
                .ok_or_else(|| page_fault(format!("变量树里没有 {symbol}，无法复制")))?;
            let entry = reference
                .get(symbol)
                .ok_or_else(|| page_fault(format!("参考页取样里没有 {symbol}，不猜它的值")))?;
            let len = variable.byte_len();
            if len.map(|len| len.get() as usize) != Some(entry.raw().len()) {
                return Err(page_fault(format!(
                    "{} 的长度与参考页取样不一致，拒绝复制",
                    symbol
                )));
            }
            written.push(self.write_one(variable, entry.raw(), context).await?);
        }
        Ok(written)
    }
}

fn page_fault(message: String) -> Fault {
    let code = FaultCode::from_const(Domain::Session, seq::PAGE);
    Fault::new(code, Severity::Error, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openape_cal_core::{Address, ByteLen, ElemType};

    fn variable(name: &str, address: u32, role: Option<Role>) -> Variable {
        let built = Variable::new(
            Symbol::new(name).expect("测试符号合法"),
            Address::new(address),
            ElemType::U16,
        );
        match role {
            Some(role) => built.with_role(role),
            None => built,
        }
    }

    #[test]
    fn measurements_are_not_sampled_but_unknown_roles_are() {
        let tree = VariableTree::new(vec![
            variable("cal.kp", 0x1000, Some(Role::Characteristic)),
            variable("meas.rpm", 0x2000, Some(Role::Measurement)),
            variable("plain", 0x3000, None),
        ])
        .expect("无重复");
        let names: Vec<&str> = calibration_variables(&tree)
            .map(|variable| variable.symbol().as_str())
            .collect();
        assert_eq!(
            names,
            vec!["cal.kp", "plain"],
            "观测量不上页，角色未定要算上"
        );
    }

    #[test]
    fn byte_len_is_reported_in_bytes() {
        let tree = VariableTree::new(vec![variable("cal.kp", 0x1000, None)]).expect("无重复");
        let target = tree
            .get(&Symbol::new("cal.kp").expect("合法"))
            .expect("查得到");
        assert_eq!(target.byte_len(), ByteLen::new(2));
    }
}
