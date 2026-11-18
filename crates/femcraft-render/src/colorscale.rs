//! RFEM's result colour scale: a rainbow ramp from dark blue (minimum) through cyan, green and
//! yellow to dark red (maximum), used for contour plots of surface and solid results.
//!
//! [`rfem_jet`] maps a normalised value `t` in `0.0..=1.0` to an RGBA colour, matching the band
//! colours RFEM shows in its result legend.

/// Colour stops, low (`t = 0`, dark blue) to high (`t = 1`, dark red). Sampled from RFEM's legend.
const STOPS: &[(f32, [f32; 3])] = &[
    (0.00, [0.00, 0.00, 0.55]), // dark blue
    (0.09, [0.00, 0.00, 1.00]), // blue
    (0.18, [0.00, 0.50, 1.00]), // azure
    (0.27, [0.00, 1.00, 1.00]), // cyan
    (0.36, [0.00, 1.00, 0.50]), // spring green
    (0.45, [0.00, 0.90, 0.00]), // green
    (0.55, [0.55, 1.00, 0.00]), // chartreuse
    (0.64, [1.00, 1.00, 0.00]), // yellow
    (0.73, [1.00, 0.75, 0.00]), // amber
    (0.82, [1.00, 0.50, 0.00]), // orange
    (0.91, [1.00, 0.00, 0.00]), // red
    (1.00, [0.55, 0.00, 0.00]), // dark red
];

/// Number of discrete colour bands RFEM shows in its result legend.
pub const BAND_COUNT: usize = STOPS.len();

/// The discrete band colour `i`, ordered low (`0`, dark blue) to high (`BAND_COUNT - 1`, dark
/// red) — RFEM fills its legend with solid blocks of these, not a blend.
pub fn rfem_band(i: usize) -> [f32; 4] {
    let c = STOPS[i.min(STOPS.len() - 1)].1;
    [c[0], c[1], c[2], 1.0]
}

/// Maps a normalised value `t` (clamped to `0.0..=1.0`) to an RGBA colour on RFEM's scale.
/// `t = 0` is the minimum (dark blue), `t = 1` the maximum (dark red).
pub fn rfem_jet(t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 1 < STOPS.len() && t > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, c0) = STOPS[i];
    let (t1, c1) = STOPS[(i + 1).min(STOPS.len() - 1)];
    let f = if (t1 - t0).abs() < f32::EPSILON {
        0.0
    } else {
        (t - t0) / (t1 - t0)
    };
    [
        c0[0] + (c1[0] - c0[0]) * f,
        c0[1] + (c1[1] - c0[1]) * f,
        c0[2] + (c1[2] - c0[2]) * f,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_and_clamps() {
        assert_eq!(rfem_jet(0.0), [0.00, 0.00, 0.55, 1.0]);
        assert_eq!(rfem_jet(1.0), [0.55, 0.00, 0.00, 1.0]);
        // Out-of-range clamps to the ends.
        assert_eq!(rfem_jet(-5.0), rfem_jet(0.0));
        assert_eq!(rfem_jet(5.0), rfem_jet(1.0));
    }

    #[test]
    fn midpoint_is_greenish() {
        let [r, g, b, _] = rfem_jet(0.5);
        assert!(g > r && g > b, "middle of the scale is green");
    }
}
