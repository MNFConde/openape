//! 资源保护（GET_SEED / UNLOCK）的算法挂点。
//!
//! 种子密钥算法与具体 ECU 绑定：没有通用算法，也不该由本软件发明一个。
//! 因此这里只定义 trait 与一个示例实现——master 侧用它算密钥，从站侧用
//! 同一个实现验密钥，两侧不会各写一套而悄悄错开。真实项目按 [D-06] 的
//! 怪癖配置层填自己的算法（M5）。
//!
//! [D-06]: ../../../docs/decisions.md#d-06-怪癖-配置层

/// 由种子算密钥。实现必须确定性：同一种子永远得出同一密钥，否则从站验不过。
pub trait SeedKey: std::fmt::Debug + Send + Sync {
    /// 算密钥。长度由算法决定，通常与种子等长。
    fn key(&self, seed: &[u8]) -> Vec<u8>;
}

/// 示例算法：逐字节取反。
///
/// 它只用于自研从站与测试，**不是任何真实 ECU 的算法**；对着真车用它会一直
/// 收到 `ERR_ACCESS_LOCKED`，这是预期行为而不是缺陷。
#[derive(Debug, Default, Clone, Copy)]
pub struct ExampleSeedKey;

impl ExampleSeedKey {
    /// 示例种子长度（字节）。4 字节够区分同一台 ECU 的两次解锁。
    pub const SEED_LEN: usize = 4;

    /// 由解锁计数推种子。从站每发一次 GET_SEED 就把计数加一，于是上一次会话
    /// 算出的密钥在下一次必然失效——固定种子的从站会让"重放旧密钥"看起来
    /// 像成功，测不出真正的解锁流程。
    pub fn seed(counter: u32) -> [u8; Self::SEED_LEN] {
        counter.to_le_bytes()
    }
}

impl SeedKey for ExampleSeedKey {
    fn key(&self, seed: &[u8]) -> Vec<u8> {
        seed.iter().map(|byte| !byte).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_key_is_the_bitwise_not_of_the_seed() {
        let seed = ExampleSeedKey::seed(1);
        assert_eq!(ExampleSeedKey.key(&seed), vec![0xFE, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn example_seed_changes_between_unlocks() {
        assert_ne!(
            ExampleSeedKey::seed(1),
            ExampleSeedKey::seed(2),
            "同一种子会让重放旧密钥看起来像成功"
        );
    }
}
