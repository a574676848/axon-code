//! 领域指标比率计算。

/// 一个完整比率对应的 basis points。
pub const FULL_RATIO_BASIS_POINTS: u16 = 10_000;

/// 将分子/分母转换为 basis points，并将结果限制在 100%。
#[must_use]
pub fn ratio_basis_points(numerator: u64, denominator: u64) -> u16 {
    if denominator == 0 {
        return 0;
    }
    let scaled = u128::from(numerator)
        .saturating_mul(u128::from(FULL_RATIO_BASIS_POINTS))
        .checked_div(u128::from(denominator))
        .unwrap_or(0)
        .min(u128::from(FULL_RATIO_BASIS_POINTS));
    u16::try_from(scaled).unwrap_or(FULL_RATIO_BASIS_POINTS)
}

#[cfg(test)]
mod tests {
    use super::{FULL_RATIO_BASIS_POINTS, ratio_basis_points};

    #[test]
    fn ratio_uses_two_decimal_basis_points_and_caps_at_full_ratio() {
        assert_eq!(ratio_basis_points(2_700, 3_000), 9_000);
        assert_eq!(ratio_basis_points(1, 3), 3_333);
        assert_eq!(ratio_basis_points(2, 1), FULL_RATIO_BASIS_POINTS);
        assert_eq!(ratio_basis_points(1, 0), 0);
    }
}
