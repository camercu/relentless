use crate::compat::Duration;

/// The minimum allowed exponential base (values below this are clamped).
pub(super) const MIN_EXPONENTIAL_BASE: f64 = 1.0;

pub(super) fn clamp_exponential_base(base: f64) -> f64 {
    if !base.is_finite() {
        return MIN_EXPONENTIAL_BASE;
    }
    f64::max(base, MIN_EXPONENTIAL_BASE)
}

pub(super) fn saturating_duration_mul(dur: Duration, mul: u32) -> Duration {
    dur.checked_mul(mul).unwrap_or(Duration::MAX)
}

/// Multiplies a `Duration` by an `f64`, saturating on overflow.
///
/// Returns `Duration::ZERO` for non-positive or NaN multipliers.
/// Returns `Duration::MAX` when the result overflows.
pub(super) fn saturating_duration_mul_f64(dur: Duration, mul: f64) -> Duration {
    const NANOS_PER_SEC: u128 = 1_000_000_000;

    if !mul.is_finite() || mul <= 0.0 {
        if mul == f64::INFINITY {
            return Duration::MAX;
        }
        return Duration::ZERO;
    }
    let nanos = dur.as_nanos() as f64 * mul;
    if nanos >= Duration::MAX.as_nanos() as f64 {
        return Duration::MAX;
    }
    let total_nanos = nanos as u128;
    if total_nanos / NANOS_PER_SEC > u128::from(u64::MAX) {
        return Duration::MAX;
    }
    Duration::new(
        (total_nanos / NANOS_PER_SEC) as u64,
        (total_nanos % NANOS_PER_SEC) as u32,
    )
}

/// Raises `base` to an integer `exponent`, returning `f64::INFINITY` on overflow.
///
/// Bases below [`MIN_EXPONENTIAL_BASE`] are treated as `1.0`, so the
/// result is always `>= 1.0` for non-zero exponents.
pub(super) fn pow_nonnegative_f64(base: f64, exponent: u32) -> f64 {
    if exponent == 0 {
        return 1.0;
    }
    if base <= MIN_EXPONENTIAL_BASE {
        return 1.0;
    }

    let mut result = 1.0;
    let mut factor = base;
    let mut remaining = exponent;

    while remaining > 0 {
        if remaining & 1 == 1 {
            result *= factor;
            if !result.is_finite() {
                return f64::INFINITY;
            }
        }
        remaining >>= 1;
        if remaining > 0 {
            factor *= factor;
            if !factor.is_finite() {
                factor = f64::INFINITY;
            }
        }
    }

    result
}
