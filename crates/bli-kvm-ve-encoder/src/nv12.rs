//! The picture as the driver takes it: NV12 of whole macroblocks, the
//! visible picture's last column and row repeated into the padding.

/// The bytes of a picture of `width` by `height` in whole macroblocks.
pub fn picture_bytes(width: usize, height: usize) -> usize {
    width.div_ceil(16) * 16 * height.div_ceil(16) * 16 * 3 / 2
}

/// Writes the picture of three planes (4:2:0, `width` and `height` even,
/// planes without padding) into `out` as NV12 of whole macroblocks.
pub fn from_i420(y: &[u8], u: &[u8], v: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    assert!(width >= 2 && height >= 2 && width % 2 == 0 && height % 2 == 0);
    assert!(
        y.len() >= width * height && u.len() >= width * height / 4 && v.len() >= width * height / 4
    );
    let (coded_width, coded_height) = (width.div_ceil(16) * 16, height.div_ceil(16) * 16);
    out.clear();
    out.resize(coded_width * coded_height * 3 / 2, 0);
    let (luma, chroma) = out.split_at_mut(coded_width * coded_height);
    for (row, line) in luma.chunks_exact_mut(coded_width).enumerate() {
        let source = row.min(height - 1) * width;
        line[..width].copy_from_slice(&y[source..source + width]);
        let last = line[width - 1];
        line[width..].fill(last);
    }
    for (row, line) in chroma.chunks_exact_mut(coded_width).enumerate() {
        let source = row.min(height / 2 - 1) * (width / 2);
        let (cb, cr) = (
            &u[source..source + width / 2],
            &v[source..source + width / 2],
        );
        for ((pair, cb), cr) in line.chunks_exact_mut(2).zip(cb).zip(cr) {
            pair[0] = *cb;
            pair[1] = *cr;
        }
        let (cb, cr) = (line[width - 2], line[width - 1]);
        for pair in line[width..].chunks_exact_mut(2) {
            pair[0] = cb;
            pair[1] = cr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_interleaved_and_padded_with_its_edge() {
        // 18 by 10: 32 by 16 in whole macroblocks.
        let (width, height) = (18, 10);
        let y: Vec<u8> = (0..width * height).map(|at| at as u8).collect();
        let u: Vec<u8> = (0..width * height / 4).map(|at| 100 + at as u8).collect();
        let v: Vec<u8> = (0..width * height / 4)
            .map(|at| 200u8.wrapping_add(at as u8))
            .collect();
        let mut out = vec![7; 3];
        from_i420(&y, &u, &v, width, height, &mut out);
        assert_eq!(out.len(), picture_bytes(width, height));
        assert_eq!(out.len(), 32 * 16 * 3 / 2);
        let luma = |x: usize, row: usize| out[row * 32 + x];
        assert_eq!(luma(0, 0), 0);
        assert_eq!(luma(17, 9), (9 * 18 + 17) as u8);
        // To the right of the picture its last column, below it its last row.
        assert_eq!(luma(31, 3), luma(17, 3));
        assert_eq!(luma(5, 15), luma(5, 9));
        assert_eq!(luma(31, 15), luma(17, 9));
        let chroma = |x: usize, row: usize| out[32 * 16 + row * 32 + x];
        assert_eq!((chroma(0, 0), chroma(1, 0)), (100, 200));
        assert_eq!((chroma(16, 4), chroma(17, 4)), (u[4 * 9 + 8], v[4 * 9 + 8]));
        assert_eq!(
            (chroma(30, 2), chroma(31, 2)),
            (chroma(16, 2), chroma(17, 2))
        );
        assert_eq!((chroma(4, 7), chroma(5, 7)), (chroma(4, 4), chroma(5, 4)));
    }

    #[test]
    fn a_picture_of_whole_macroblocks_is_not_padded() {
        let y = vec![9u8; 32 * 16];
        let (u, v) = (vec![1u8; 16 * 8], vec![2u8; 16 * 8]);
        let mut out = Vec::new();
        from_i420(&y, &u, &v, 32, 16, &mut out);
        assert_eq!(out.len(), 32 * 16 * 3 / 2);
        assert!(out[..32 * 16].iter().all(|sample| *sample == 9));
        assert!(out[32 * 16..].chunks_exact(2).all(|pair| pair == [1, 2]));
    }
}
