//! How close a decoded picture is to its source.

/// The peak signal-to-noise ratio of the luma plane, in decibels, over the
/// visible `width` by `height`. Equal planes give infinity.
pub fn luma(
    source: &[u8],
    source_stride: usize,
    decoded: &[u8],
    decoded_stride: usize,
    width: usize,
    height: usize,
) -> f64 {
    let mut squares = 0u64;
    for y in 0..height {
        let left = &source[y * source_stride..y * source_stride + width];
        let right = &decoded[y * decoded_stride..y * decoded_stride + width];
        for (a, b) in left.iter().zip(right) {
            let difference = *a as i64 - *b as i64;
            squares += (difference * difference) as u64;
        }
    }
    if squares == 0 {
        return f64::INFINITY;
    }
    let mean = squares as f64 / (width * height) as f64;
    10.0 * (255.0 * 255.0 / mean).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_planes_are_infinitely_close_and_noise_is_measured() {
        let source = vec![100u8; 64 * 48];
        assert_eq!(luma(&source, 64, &source, 64, 64, 48), f64::INFINITY);
        // Every sample off by 2: a mean square of 4, 42.11 dB.
        let decoded = vec![102u8; 64 * 48];
        let value = luma(&source, 64, &decoded, 64, 64, 48);
        assert!((value - 42.110).abs() < 0.001, "{value}");
        // What lies beyond the visible width does not count.
        let mut wide = vec![100u8; 80 * 48];
        for row in wide.chunks_exact_mut(80) {
            row[64..].fill(0);
        }
        assert_eq!(luma(&source, 64, &wide, 80, 64, 48), f64::INFINITY);
    }
}
