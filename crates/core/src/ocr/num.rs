//! 不使用 `as` 的数值转换。

/// `u32` 转 `f32`，拆成高低 16 位相加。
pub(super) fn u32_to_f32(value: u32) -> f32 {
    let high = u16::try_from(value >> 16).unwrap_or(u16::MAX);
    let low = u16::try_from(value & 0xFFFF).unwrap_or(u16::MAX);
    f32::from(high) * 65_536.0 + f32::from(low)
}

/// `usize` 转 `f32`，超出 `u32` 范围时饱和。
pub(super) fn usize_to_f32(value: usize) -> f32 {
    u32_to_f32(u32::try_from(value).unwrap_or(u32::MAX))
}

/// 向下取整转 `u32`；负数与 NaN 得 0，超大值饱和。
///
/// 直接拆解 IEEE 754 位模式，避免 `as`。
pub(super) fn floor_u32(value: f32) -> u32 {
    if value.is_nan() || value < 1.0 {
        return 0;
    }
    // 小于 2^32 的最大 f32
    if value >= 4_294_967_040.0 {
        return u32::MAX;
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 23) & 0xFF).saturating_sub(127);
    let mantissa = (bits & 0x007F_FFFF) | 0x0080_0000;
    if exponent >= 23 {
        mantissa.checked_shl(exponent - 23).unwrap_or(u32::MAX)
    } else {
        mantissa.checked_shr(23 - exponent).unwrap_or(0)
    }
}

/// 向上取整转 `u32`。
pub(super) fn ceil_u32(value: f32) -> u32 {
    floor_u32(value.ceil())
}

/// 四舍五入转 `u32`。
pub(super) fn round_u32(value: f32) -> u32 {
    floor_u32(value.round())
}

/// `u32` 转 `usize`，失败时饱和。
pub(super) fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_to_f32_is_exact_for_pixel_sizes() {
        assert_eq!(u32_to_f32(0), 0.0);
        assert_eq!(u32_to_f32(1), 1.0);
        assert_eq!(u32_to_f32(65_535), 65_535.0);
        assert_eq!(u32_to_f32(65_536), 65_536.0);
        assert_eq!(u32_to_f32(1_000_000), 1_000_000.0);
        assert_eq!(usize_to_f32(960), 960.0);
    }

    #[test]
    fn floor_handles_edges() {
        assert_eq!(floor_u32(-3.0), 0);
        assert_eq!(floor_u32(f32::NAN), 0);
        assert_eq!(floor_u32(0.99), 0);
        assert_eq!(floor_u32(1.0), 1);
        assert_eq!(floor_u32(1.99), 1);
        assert_eq!(floor_u32(31.5), 31);
        assert_eq!(floor_u32(8_388_608.0), 8_388_608);
        assert_eq!(floor_u32(1.0e12), u32::MAX);
        assert_eq!(floor_u32(f32::INFINITY), u32::MAX);
        assert_eq!(ceil_u32(1.01), 2);
        assert_eq!(ceil_u32(2.0), 2);
        assert_eq!(round_u32(2.5), 3);
        assert_eq!(round_u32(2.4), 2);
        assert_eq!(round_u32(-1.0), 0);
        assert_eq!(to_usize(7), 7);
    }
}
