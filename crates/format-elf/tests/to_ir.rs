//! 验收（M3-03）：仅 ELF 档进 IR 时只填它知道的事实。
//!
//! 语义字段（单位、量程、换算）与角色一律留空——D-13 定了角色由用户指定，
//! 语义只有 A2L 给得出。这里断言的就是"没有多填"。

use openape_cal_core::{ByteOrder, ElemType, VariableTree};
use openape_format_elf::{ElfSource, load};

const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/elf/test.elf");

fn source() -> ElfSource {
    load(std::path::Path::new(ELF)).expect("golden ELF 必须能读")
}

fn tree() -> (VariableTree, Vec<openape_format_elf::Notice>) {
    source().to_ir().expect("golden ELF 必须能进 IR")
}

fn find<'a>(tree: &'a VariableTree, name: &str) -> &'a openape_cal_core::Variable {
    let symbol = openape_cal_core::Symbol::new(name).expect("合法符号");
    tree.get(&symbol)
        .unwrap_or_else(|| panic!("IR 里必须有 {name}"))
}

#[test]
fn scalars_keep_name_address_and_element_type() {
    let (tree, _) = tree();
    let kp = find(&tree, "cal_kp");
    assert_eq!(kp.elem(), ElemType::F32);
    assert!(kp.dims().is_empty(), "标量没有维度");
    assert_eq!(kp.elem_count(), 1);
}

#[test]
fn arrays_keep_every_dimension() {
    let (tree, _) = tree();
    let map = find(&tree, "cal_map");
    assert_eq!(map.elem(), ElemType::F32);
    assert_eq!(map.dims(), &[4, 3]);
    assert_eq!(map.elem_count(), 12);
}

#[test]
fn no_semantics_are_invented() {
    // 这是 M3-03 的核心断言：ELF 给不出单位、量程、换算，就不填。
    let (tree, _) = tree();
    for name in ["cal_kp", "cal_map", "cal_axis_x", "obs_speed"] {
        let var = find(&tree, name);
        assert!(
            var.semantics().is_empty(),
            "{name} 不该凭空长出单位、量程或换算"
        );
    }
}

#[test]
fn no_role_is_assigned() {
    // 角色由用户指定（D-13）。自动判定"const 就是标定量"在真实 ECU 上会错：
    // const 也可能只是观测量，volatile 也可能是标定量。
    let (tree, _) = tree();
    for var in tree.iter() {
        assert!(
            var.role().is_none(),
            "{} 的角色必须留给用户指定",
            var.symbol()
        );
    }
}

#[test]
fn byte_order_comes_from_the_elf_header() {
    // x86-64 是小端。IR 的默认值是 Big，不显式设置就会写反。
    let (tree, _) = tree();
    assert_eq!(find(&tree, "cal_kp").byte_order(), ByteOrder::Little);
}

#[test]
fn struct_members_become_flat_variables() {
    // A2L 的 MEASUREMENT/CHARACTERISTIC 都是扁平对象，没有"结构体"这一说；
    // 成员各有自己的地址，摊平才对得上。
    let (tree, _) = tree();
    let kp = find(&tree, "cal_gains.kp");
    let ki = find(&tree, "cal_gains.ki");
    assert_eq!(kp.elem(), ElemType::F32);
    assert_eq!(ki.elem(), ElemType::F32);
    assert_eq!(
        ki.address().get() - kp.address().get(),
        4,
        "成员地址差必须等于偏移"
    );
}

#[test]
fn enums_become_their_underlying_integer() {
    let (tree, _) = tree();
    let mode = find(&tree, "cal_mode");
    assert_eq!(mode.elem(), ElemType::U32, "mode_t 的底层是无符号 32 位");
}

#[test]
fn pointers_are_reported_as_skipped_not_silently_dropped() {
    // 指针与函数指针在 IR 里没有对应形状（它们既不是标量也不是数组）。
    // 跳过可以，静默跳过不行——用户会以为变量本来就不存在。
    let (tree, notes) = tree();
    let symbol = openape_cal_core::Symbol::new("obs_ptr").expect("合法符号");
    assert!(tree.get(&symbol).is_none(), "指针不该进树");
    assert!(
        notes.iter().any(|note| note.subject() == "obs_ptr"),
        "被跳过的变量必须留一条提示，实为 {notes:?}"
    );
    assert!(
        notes.iter().any(|note| note.subject() == "obs_callback"),
        "函数指针同样要报"
    );
}
