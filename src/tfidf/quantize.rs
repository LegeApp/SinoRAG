// 8-bit logarithmic quantization of TF-IDF weights.
//
// Per-term scale: caller supplies `w_max` (the largest weight observed for
// the term). Quantization is monotonic over [0, w_max] and idempotent
// across multiple round-trips at the same bucket. Weights above `w_max`
// saturate at 255.

const SCALE: f32 = 255.0;

#[inline]
fn log2_1p(x: f32) -> f32 {
    (1.0 + x).ln() / std::f32::consts::LN_2
}

/// Quantize a non-negative weight `w` against per-term scale `w_max`.
/// Returns a u8 in `[0, 255]`. Negative or NaN inputs map to 0.
#[inline]
pub fn quantize_log_u8(w: f32, w_max: f32) -> u8 {
    if !(w > 0.0) || !(w_max > 0.0) {
        return 0;
    }
    let denom = log2_1p(w_max);
    if denom <= 0.0 {
        return 0;
    }
    let ratio = (log2_1p(w) / denom).clamp(0.0, 1.0);
    (ratio * SCALE).round().clamp(0.0, SCALE) as u8
}

/// Dequantize back to an approximate weight using the same per-term scale.
#[inline]
pub fn dequantize_log_u8(q: u8, w_max: f32) -> f32 {
    if q == 0 || !(w_max > 0.0) {
        return 0.0;
    }
    let denom = log2_1p(w_max);
    let exponent = (q as f32 / SCALE) * denom;
    (exponent * std::f32::consts::LN_2).exp() - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_within_tolerance() {
        let w_max = 0.8f32;
        // log2(1+w) is near-linear at small w, so absolute bucket width is
        // roughly w_max/255 ≈ 0.003 and the relative error at small weights
        // can hit ~12%. Tolerate up to 15% across the range — this is fine
        // for cosine ranking, which is the consumer.
        for &w in &[0.01f32, 0.05, 0.1, 0.3, 0.5, 0.7, 0.8] {
            let q = quantize_log_u8(w, w_max);
            let back = dequantize_log_u8(q, w_max);
            let rel = ((back - w).abs() / w.max(1e-6)).min(1.0);
            assert!(rel < 0.15, "w={w} q={q} back={back} rel={rel}");
        }
    }

    #[test]
    fn monotonic_in_weight() {
        let w_max = 1.0f32;
        let mut prev = quantize_log_u8(0.0, w_max);
        for i in 1..=100 {
            let w = i as f32 / 100.0;
            let q = quantize_log_u8(w, w_max);
            assert!(q >= prev, "non-monotonic at w={w}: prev={prev} q={q}");
            prev = q;
        }
    }

    #[test]
    fn zero_and_saturation() {
        assert_eq!(quantize_log_u8(0.0, 1.0), 0);
        assert_eq!(quantize_log_u8(-1.0, 1.0), 0);
        assert_eq!(quantize_log_u8(1.0, 1.0), 255);
        // Above w_max saturates.
        assert_eq!(quantize_log_u8(2.0, 1.0), 255);
        assert_eq!(dequantize_log_u8(0, 1.0), 0.0);
    }

    #[test]
    fn w_max_zero_safe() {
        assert_eq!(quantize_log_u8(0.5, 0.0), 0);
        assert_eq!(dequantize_log_u8(128, 0.0), 0.0);
    }
}
