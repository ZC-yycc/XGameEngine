//! 碰撞层索引：与物理过滤的 32 位成员掩码共用边界。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerIndex(u8);

impl LayerIndex {
    pub const MAX: u8 = u32::BITS as u8 - 1;

    pub const fn new(index: u8) -> Option<Self> {
        if index <= Self::MAX {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn raw(self) -> u8 {
        self.0
    }
}
