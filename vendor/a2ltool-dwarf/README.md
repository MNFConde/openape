# a2ltool-dwarf（裁剪副本）

从 [`a2ltool`](https://github.com/DanielT/CanapeA2L) 3.4.2 裁剪而来，只保留 DWARF / PDB 提取层。
上游许可 `MIT OR Apache-2.0`，`LICENSE-MIT`、`LICENSE-APACHE` 原样保留。

## 保留范围

| 路径 | 说明 |
|---|---|
| `src/debuginfo/` | 上游 `src/debuginfo/` 原样（含 `dwarf/`、`pdb/`、`iter.rs`） |
| `src/symbol.rs` | 上游 `src/symbol.rs` 原样 |
| `src/lib.rs` | 新写，只做 `pub mod` 导出 |

上游的 A2L 读写、CLI、fuzz 入口（`creator/`、`insert.rs`、`remove.rs`、`update/`、`ifdata.rs`、`main.rs`、`lib.rs` 的 `core()`）全部未取用——本 crate 只回答问题"ELF 里的变量长什么样"，不碰 A2L。

依赖也随之收缩：上游的 `clap`、`argfile`、`glob`、`regex`、`fnv`、`num-traits`、`a2lfile` 都不再需要。

## 与上游的差异

只有可见性，无逻辑改动（M0-4 spike 已实测，见 `docs/decisions.md` D-02 的验证结论）：

- 模块 `debuginfo`、`symbol` 由 `mod` / `pub mod` 差异统一为 `pub mod`（上游本就 `pub mod debuginfo`，`symbol` 亦然）
- `debuginfo` 内部与 `symbol.rs` 中供跨模块使用、上游为 `pub(crate)` 的项改为 `pub`
- 删除 `debuginfo/dwarf/mod.rs` 与 `debuginfo/pdb/mod.rs` 末尾的 `mod test`：它们读 `fixtures/bin/` 下的 clang/gcc/CL 二进制（5.4 MB，未随本 crate 入库）。同等行为由 `crates/format-elf/tests/golden_elf.rs` 用本项目自己的夹具覆盖。其余模块的测试原样保留

## 升级方式

重跑 `script/vendor-a2ltool.sh`（把上游 tag 解包到 `tmp/`，复制上述三个路径，用 `git diff` 核对只看得到可见性改动）。
