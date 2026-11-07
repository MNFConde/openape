//! 从 a2ltool 裁剪出的 DWARF/PDB 提取层，只保留 `debuginfo` 与 `symbol`。
//! 上游：<https://github.com/DanielT/CanapeA2L>（`a2ltool` 3.4.2，MIT OR Apache-2.0）。
//! 本 crate 只做可见性调整与裁剪，无逻辑改动；细节见 `README.md`。

pub mod debuginfo;
pub mod symbol;
