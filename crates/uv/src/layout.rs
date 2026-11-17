//! A simple shelf layout for island rectangles: rows of islands, tallest first, all scaled by the
//! same factor so texel density stays even. The largest factor that fits the unit square (with
//! a margin around and between islands) is found by bisection.

use glam::DVec2;

fn fits(sizes: &[DVec2], order: &[usize], s: f64, margin: f64, out: Option<&mut Vec<DVec2>>) -> bool {
    let limit = 1.0 - margin;
    let (mut x, mut y, mut row) = (margin, margin, 0.0f64);
    let mut places = vec![DVec2::ZERO; sizes.len()];
    for &i in order {
        let Some(sz) = sizes.get(i) else { continue };
        let (w, h) = (sz.x * s, sz.y * s);
        if x + w > limit + 1e-12 && x > margin {
            x = margin;
            y += row + margin;
            row = 0.0;
        }
        if x + w > limit + 1e-12 || y + h > limit + 1e-12 {
            return false;
        }
        if let Some(p) = places.get_mut(i) {
            *p = DVec2::new(x, y);
        }
        x += w + margin;
        row = row.max(h);
    }
    if let Some(o) = out {
        *o = places;
    }
    true
}

/// Lower-left corners of the rectangles `sizes` and the scale factor applied to all of them.
pub fn shelf(sizes: &[DVec2], margin: f64) -> (Vec<DVec2>, f64) {
    if sizes.is_empty() {
        return (Vec::new(), 1.0);
    }
    let margin = if margin.is_finite() { margin.clamp(0.0, 0.1) } else { 0.0 };
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|a, b| sizes[*b].y.total_cmp(&sizes[*a].y).then(a.cmp(b)));
    let biggest = sizes.iter().map(|s| s.x.max(s.y)).fold(0.0, f64::max).max(1e-12);
    let (mut lo, mut hi) = (0.0, (1.0 - 2.0 * margin) / biggest);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if fits(sizes, &order, mid, margin, None) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut places = Vec::new();
    if !fits(sizes, &order, lo, margin, Some(&mut places)) {
        places = vec![DVec2::splat(margin); sizes.len()];
    }
    (places, lo)
}
