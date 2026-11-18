//! Line geometry: points along a line of any RFEM line type, in model coordinates.
//!
//! Conventions checked on RFEM 6.16 (`Lines.rf6`): arc = start node, end node and a control
//! point on the arc; circle = center, normal, radius, the one definition node on the circle;
//! elliptical arc = two main-axis points (center between them, parameter 0 towards the first),
//! a perimeter point (sets the minor radius and its side) and angles α (start) / β (sweep);
//! ellipse = two main-axis vertices (the definition nodes) and a perimeter control point.
//! Guesses: parabola = quadratic Bézier (control point = tangent intersection, α ignored);
//! spline = curve through the definition nodes (centripetal Catmull-Rom). NURBS falls back to
//! the definition nodes.

use crate::{Model, Object, ObjectId, ObjectKind};

pub type P3 = [f64; 3];

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: P3, s: f64) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: P3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: P3) -> Option<P3> {
    let l = len(a);
    (l > 1e-12).then(|| mul(a, 1.0 / l))
}

/// Segments for a sweep of `angle` radians (48 per full turn, at least 4).
fn steps(angle: f64) -> usize {
    ((angle.abs() / std::f64::consts::TAU * 48.0).ceil() as usize).max(4)
}

/// `c + cos t * u + sin t * v` for `t` from `t0` over `sweep`.
fn sweep(c: P3, u: P3, v: P3, t0: f64, sweep: f64) -> Vec<P3> {
    let n = steps(sweep);
    (0..=n)
        .map(|i| {
            let t = t0 + sweep * i as f64 / n as f64;
            add(c, add(mul(u, t.cos()), mul(v, t.sin())))
        })
        .collect()
}

fn vec3(o: &Object, x: &str, y: &str, z: &str) -> Option<P3> {
    let c = |k: &str| o.get(k).as_f64().filter(|v| v.is_finite());
    Some([c(x)?, c(y)?, c(z).unwrap_or(0.0)])
}

fn control_point(o: &Object, prefix: &str) -> Option<P3> {
    vec3(
        o,
        &format!("{prefix}_x"),
        &format!("{prefix}_y"),
        &format!("{prefix}_z"),
    )
}

/// Circular arc from `a` through `m` to `b`.
pub fn arc_through(a: P3, m: P3, b: P3) -> Option<Vec<P3>> {
    let (ab, am) = (sub(b, a), sub(m, a));
    let n = cross(ab, am);
    let n2 = dot(n, n);
    if n2 < 1e-18 {
        return None;
    }
    // Circumcenter of the triangle a, m, b.
    let c = add(
        a,
        mul(
            add(
                mul(cross(n, ab), dot(am, am)),
                mul(cross(am, n), dot(ab, ab)),
            ),
            0.5 / n2,
        ),
    );
    let r = len(sub(a, c));
    let u = unit(sub(a, c))?;
    // About -n (= am x ab) the order a -> m -> b is counter-clockwise.
    let v = unit(cross(mul(n, -1.0), u))?;
    let angle = |p: P3| {
        let d = sub(p, c);
        dot(d, v).atan2(dot(d, u)).rem_euclid(std::f64::consts::TAU)
    };
    let tb = angle(b);
    Some(sweep(c, mul(u, r), mul(v, r), 0.0, tb))
}

/// Full circle starting at `start` (or at the rotation angle from the in-plane X axis).
fn circle(o: &Object, start: Option<P3>) -> Option<Vec<P3>> {
    let c = vec3(
        o,
        "circle_center_coordinate_1",
        "circle_center_coordinate_2",
        "circle_center_coordinate_3",
    )?;
    let n = vec3(
        o,
        "circle_normal_coordinate_1",
        "circle_normal_coordinate_2",
        "circle_normal_coordinate_3",
    )
    .and_then(unit)
    .unwrap_or([0.0, 0.0, 1.0]);
    let r = o.get("circle_radius").as_f64().filter(|r| *r > 0.0)?;
    let (u, v) = match start.and_then(|s| unit(sub(s, c))) {
        Some(u) => (u, cross(n, u)),
        None => {
            let x = unit(sub([1.0, 0.0, 0.0], mul(n, n[0])))
                .or_else(|| unit(sub([0.0, 1.0, 0.0], mul(n, n[1]))))?;
            let y = cross(n, x);
            let rot = o.get("circle_rotation").as_f64().unwrap_or(0.0);
            let u = add(mul(x, rot.cos()), mul(y, rot.sin()));
            (u, cross(n, u))
        }
    };
    let mut pts = sweep(c, mul(u, r), mul(v, r), 0.0, std::f64::consts::TAU);
    if let (Some(s), Some(last)) = (start, pts.last_mut()) {
        *last = s;
        pts[0] = s;
    }
    Some(pts)
}

/// Center, major semi-axis vector and minor semi-axis vector from two main-axis points
/// (the first at parameter 0) and a perimeter point.
fn ellipse_axes(first: P3, second: P3, perimeter: P3) -> Option<(P3, P3, P3)> {
    let c = mul(add(first, second), 0.5);
    let big = sub(first, c);
    let a = len(big);
    let ua = unit(big)?;
    let d = sub(perimeter, c);
    let x = dot(d, ua);
    let w = sub(d, mul(ua, x));
    let s = 1.0 - (x / a).powi(2);
    if s <= 1e-12 {
        return None;
    }
    let b = len(w) / s.sqrt();
    Some((c, big, mul(unit(w)?, b)))
}

fn quadratic_bezier(a: P3, c: P3, b: P3) -> Vec<P3> {
    let n = 24;
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let s = 1.0 - t;
            add(add(mul(a, s * s), mul(c, 2.0 * s * t)), mul(b, t * t))
        })
        .collect()
}

/// Centripetal Catmull-Rom through `pts` (ends extended by reflection).
pub fn spline_through(pts: &[P3]) -> Vec<P3> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let n = pts.len();
    let at = |i: isize| -> P3 {
        if i < 0 {
            sub(mul(pts[0], 2.0), pts[1])
        } else if i as usize >= n {
            sub(mul(pts[n - 1], 2.0), pts[n - 2])
        } else {
            pts[i as usize]
        }
    };
    let knot = |a: P3, b: P3| len(sub(b, a)).sqrt().max(1e-9);
    let mut out = vec![pts[0]];
    let per = 16;
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (
            at(i as isize - 1),
            at(i as isize),
            at(i as isize + 1),
            at(i as isize + 2),
        );
        let t1 = knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        let t3 = t2 + knot(p2, p3);
        let lerp = |a: P3, b: P3, ta: f64, tb: f64, t: f64| {
            add(mul(a, (tb - t) / (tb - ta)), mul(b, (t - ta) / (tb - ta)))
        };
        for k in 1..=per {
            let t = t1 + (t2 - t1) * k as f64 / per as f64;
            let a1 = lerp(p0, p1, 0.0, t1, t);
            let a2 = lerp(p1, p2, t1, t2, t);
            let a3 = lerp(p2, p3, t2, t3, t);
            let b1 = lerp(a1, a2, 0.0, t2, t);
            let b2 = lerp(a2, a3, t1, t3, t);
            out.push(lerp(b1, b2, t1, t2, t));
        }
        *out.last_mut().unwrap() = p2;
    }
    out
}

impl Model {
    /// Points along line `no` in model coordinates: the definition nodes for polylines, a
    /// tessellated curve for arcs, circles, ellipses, parabolas and splines. Falls back to the
    /// definition nodes when the curve data is missing.
    pub fn line_points(&self, no: u32) -> Vec<P3> {
        let Some(l) = self.get(ObjectId::new(ObjectKind::Line, no)) else {
            return Vec::new();
        };
        let nodes: Vec<P3> = self
            .line_nodes(no)
            .iter()
            .filter_map(|n| self.node_position(*n))
            .collect();
        let curve = match l.get("type").as_str().unwrap_or("TYPE_POLYLINE") {
            "TYPE_ARC" if nodes.len() >= 2 => control_point(l, "arc_control_point")
                .and_then(|m| arc_through(nodes[0], m, nodes[nodes.len() - 1])),
            "TYPE_CIRCLE" => circle(l, nodes.first().copied()),
            "TYPE_ELLIPSE" if nodes.len() >= 2 => control_point(l, "ellipse_control_point")
                .and_then(|p| ellipse_axes(nodes[0], nodes[1], p))
                .map(|(c, u, v)| sweep(c, u, v, 0.0, std::f64::consts::TAU)),
            "TYPE_ELLIPTICAL_ARC" => (|| {
                let (c, u, v) = ellipse_axes(
                    control_point(l, "elliptical_arc_first_control_point")?,
                    control_point(l, "elliptical_arc_second_control_point")?,
                    control_point(l, "elliptical_arc_perimeter_control_point")?,
                )?;
                let alpha = l.get("elliptical_arc_alpha").as_f64()?;
                let beta = l.get("elliptical_arc_beta").as_f64()?;
                Some(sweep(c, u, v, alpha, beta))
            })(),
            "TYPE_PARABOLA" if nodes.len() >= 2 => control_point(l, "parabola_control_point")
                .map(|c| quadratic_bezier(nodes[0], c, nodes[nodes.len() - 1])),
            "TYPE_SPLINE" => Some(spline_through(&nodes)),
            _ => None,
        };
        curve.filter(|c| c.len() >= 2).unwrap_or(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P3, b: P3) -> bool {
        len(sub(a, b)) < 1e-9
    }

    #[test]
    fn arc_passes_through_its_three_points() {
        let (a, m, b) = ([4.0, -2.0, 0.0], [6.0, 0.0, 0.0], [5.0, 2.0, 0.0]);
        let pts = arc_through(a, m, b).unwrap();
        assert!(close(pts[0], a) && close(*pts.last().unwrap(), b));
        let c = [23.0 / 6.0, 1.0 / 6.0, 0.0];
        let r = len(sub(a, c));
        assert!(pts.iter().all(|p| (len(sub(*p, c)) - r).abs() < 1e-9));
        // The arc bulges towards m (x up to 6), not the long way round (x down to ~1.7).
        assert!(pts.iter().all(|p| p[0] > 3.9));
    }

    #[test]
    fn spline_hits_its_nodes() {
        let n = [[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 1.0, 0.0], [4.0, 3.0, 0.0]];
        let pts = spline_through(&n);
        for p in n {
            assert!(pts.iter().any(|q| close(*q, p)));
        }
    }

    #[test]
    fn ellipse_axes_from_perimeter_point() {
        let (c, u, v) =
            ellipse_axes([-2.0, -2.0, 0.0], [-2.0, -6.0, 0.0], [-3.0, -4.0, 0.0]).unwrap();
        assert!(close(c, [-2.0, -4.0, 0.0]));
        assert!(close(u, [0.0, 2.0, 0.0]));
        assert!(close(v, [-1.0, 0.0, 0.0]));
    }
}
