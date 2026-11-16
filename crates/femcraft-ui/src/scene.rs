//! Turns a model into drawable geometry: GPU lines / triangles plus 2D markers (nodes,
//! support symbols, load labels) that the viewport paints on top.
//!
//! Coordinates: RFEM's global Z points down. The viewport world has Z up, so a model point
//! `(x, y, z)` is drawn at `(x, -y, -z)` (a 180 degree turn about X, which keeps the axes
//! right-handed and shows the model the way RFEM does).

use femcraft_model::{Model, Object, ObjectId, ObjectKind as K, Value};
use femcraft_render::viewport::LineVertex;
use glam::Vec3;

pub const NODE: [f32; 4] = [0.95, 0.20, 0.20, 1.0];
const LINE: [f32; 4] = [0.62, 0.66, 0.72, 1.0];
const MEMBER: [f32; 4] = [0.45, 0.72, 1.0, 1.0];
const SURFACE_FILL: [f32; 4] = [0.30, 0.62, 0.70, 0.28];
const SUPPORTED_FILL: [f32; 4] = [0.62, 0.48, 0.28, 0.32];
pub const SUPPORT: [f32; 4] = [0.35, 0.85, 0.40, 1.0];
pub const NODAL_LOAD: [f32; 4] = [1.0, 0.35, 0.30, 1.0];
pub const LINE_LOAD: [f32; 4] = [1.0, 0.62, 0.20, 1.0];
pub const SURFACE_LOAD: [f32; 4] = [0.98, 0.86, 0.30, 1.0];
const SURFACE_LOAD_FILL: [f32; 4] = [0.98, 0.86, 0.30, 0.18];

/// Above this many load labels in one load case, values are not drawn.
const MAX_LOAD_LABELS: usize = 60;

/// Model point -> viewport world point.
pub fn to_world(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0] as f32, -p[1] as f32, -p[2] as f32)
}

/// Model direction -> viewport world direction.
fn dir_world(d: Vec3) -> Vec3 {
    Vec3::new(d.x, -d.y, -d.z)
}

/// Viewport world point -> model coordinates (for the status bar).
pub fn to_model(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.y, -p.z)
}

/// The model's global axes in viewport world coordinates.
pub const GLOBAL_AXES: [(Vec3, &str); 3] = [(Vec3::X, "X"), (Vec3::NEG_Y, "Y"), (Vec3::NEG_Z, "Z")];

#[derive(Clone, PartialEq, Debug)]
pub struct Visibility {
    pub nodes: bool,
    pub lines: bool,
    pub members: bool,
    pub surfaces: bool,
    pub supports: bool,
    pub loads: bool,
    /// Value labels next to loads.
    pub load_values: bool,
    /// Load case whose loads are shown.
    pub load_case: u32,
}

impl Default for Visibility {
    fn default() -> Self {
        Self {
            nodes: true,
            lines: true,
            members: true,
            surfaces: true,
            supports: true,
            loads: true,
            load_values: true,
            load_case: 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SupportSymbol {
    /// All rotations restrained.
    Fixed,
    /// Translations restrained, rotations free (or partly).
    Hinged,
}

/// A text label anchored at a world point.
pub struct Label {
    pub pos: Vec3,
    pub text: String,
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct Scene {
    pub lines: Vec<LineVertex>,
    pub triangles: Vec<LineVertex>,
    pub nodes: Vec<Vec3>,
    pub supports: Vec<(Vec3, SupportSymbol)>,
    pub labels: Vec<Label>,
    /// World-space bounding box of the geometry.
    pub bounds: Option<(Vec3, Vec3)>,
    /// Problems found while building (missing nodes, unknown load directions, ...).
    pub notes: Vec<String>,
}

impl Scene {
    fn line(&mut self, a: Vec3, b: Vec3, c: [f32; 4]) {
        self.lines.push(LineVertex::new(a, c));
        self.lines.push(LineVertex::new(b, c));
    }

    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3, col: [f32; 4]) {
        for p in [a, b, c] {
            self.triangles.push(LineVertex::new(p, col));
        }
    }

    /// Arrow ending at `tip`, pointing along `dir` (unit), `len` long.
    fn arrow(&mut self, tip: Vec3, dir: Vec3, len: f32, c: [f32; 4]) {
        let tail = tip - dir * len;
        self.line(tail, tip, c);
        let (u, v) = perpendiculars(dir);
        let head = len * 0.22;
        for side in [u, -u, v, -v] {
            self.line(tip, tip - dir * head + side * head * 0.45, c);
        }
    }

    fn grow(&mut self, p: Vec3) {
        self.bounds = Some(match self.bounds {
            None => (p, p),
            Some((lo, hi)) => (lo.min(p), hi.max(p)),
        });
    }

    /// Centre and radius of the bounding box.
    pub fn extent(&self) -> Option<(Vec3, f32)> {
        self.bounds
            .map(|(lo, hi)| ((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(0.5)))
    }
}

fn perpendiculars(d: Vec3) -> (Vec3, Vec3) {
    let helper = if d.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = d.cross(helper).normalize_or_zero();
    (u, d.cross(u).normalize_or_zero())
}

fn f(o: &Object, key: &str) -> Option<f64> {
    o.get(key).as_f64()
}

fn is_fixed(v: Option<f64>) -> bool {
    v.is_some_and(|x| x.is_infinite() && x > 0.0)
}

/// Load value with SI -> kN units.
fn kn(v: f64, unit: &str) -> String {
    format!("{:.3} {unit}", v / 1000.0)
        .replace(".000 ", " ")
        .replace("00 ", " ")
}

pub fn build(model: &Model, vis: &Visibility) -> Scene {
    let mut s = Scene::default();
    let node = |no: u32| model.node_position(no).map(to_world);

    // Bounds and a size used for symbols and arrows.
    for (id, _) in model.iter_kind(K::Node) {
        if let Some(p) = node(id.no) {
            s.grow(p);
        }
    }
    let size = s.extent().map_or(10.0, |(_, r)| r * 2.0);
    let arrow_len = (size * 0.06).clamp(0.2, 5.0);

    // Surfaces first (translucent fill); remember which ones have a surface support.
    let mut supported: Vec<u32> = Vec::new();
    for (_, sup) in model.iter_kind(K::SurfaceSupport) {
        supported.extend_from_slice(sup.get("surfaces").as_ref_list());
    }
    let mut surface_polys: Vec<(u32, Vec<Vec3>)> = Vec::new();
    for (id, srf) in model.iter_kind(K::Surface) {
        match surface_polygon(model, srf.get("boundary_lines").as_ref_list()) {
            Some(poly) => surface_polys.push((id.no, poly)),
            None => s
                .notes
                .push(format!("Surface {}: boundary is not a closed loop", id.no)),
        }
    }
    if vis.surfaces {
        for (no, poly) in &surface_polys {
            let col = if supported.contains(no) && vis.supports {
                SUPPORTED_FILL
            } else {
                SURFACE_FILL
            };
            for [a, b, c] in triangulate(poly) {
                s.tri(a, b, c, col);
            }
        }
    }

    if vis.lines {
        for (id, _) in model.iter_kind(K::Line) {
            let pts: Vec<Vec3> = model
                .line_nodes(id.no)
                .iter()
                .filter_map(|n| node(*n))
                .collect();
            for w in pts.windows(2) {
                s.line(w[0], w[1], LINE);
            }
        }
    }

    if vis.members {
        for (id, m) in model.iter_kind(K::Member) {
            let Some(line) = m.get("line").as_ref_no() else {
                continue;
            };
            let pts: Vec<[f64; 3]> = model
                .line_nodes(line)
                .iter()
                .filter_map(|n| model.node_position(*n))
                .collect();
            let (b, h) = section_size(model, m.get("section_start").as_ref_no());
            let beta = f(m, "rotation_angle").unwrap_or(0.0) as f32;
            if pts.len() < 2 {
                s.notes
                    .push(format!("Member {}: line {line} has no nodes", id.no));
            }
            for w in pts.windows(2) {
                member_box(&mut s, w[0], w[1], b, h, beta);
            }
        }
    }

    if vis.nodes {
        for (id, _) in model.iter_kind(K::Node) {
            if let Some(p) = node(id.no) {
                s.nodes.push(p);
            }
        }
    }

    if vis.supports {
        for (_, sup) in model.iter_kind(K::NodalSupport) {
            let symbol = support_symbol(sup);
            for n in sup.get("nodes").as_ref_list() {
                if let Some(p) = node(*n) {
                    s.supports.push((p, symbol));
                }
            }
        }
        for (_, sup) in model.iter_kind(K::LineSupport) {
            let symbol = support_symbol(sup);
            for l in sup.get("lines").as_ref_list() {
                let pts: Vec<Vec3> = model
                    .line_nodes(*l)
                    .iter()
                    .filter_map(|n| node(*n))
                    .collect();
                for w in pts.windows(2) {
                    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                        s.supports.push((w[0].lerp(w[1], t), symbol));
                    }
                }
            }
        }
        // Surface supports: tinted fill (above) plus symbols at the boundary corners.
        for (_, sup) in model.iter_kind(K::SurfaceSupport) {
            for sn in sup.get("surfaces").as_ref_list() {
                if let Some((_, poly)) = surface_polys.iter().find(|(no, _)| no == sn) {
                    for p in poly {
                        s.supports.push((*p, SupportSymbol::Hinged));
                    }
                }
            }
        }
    }

    if vis.loads {
        let before = s.labels.len();
        loads(&mut s, model, vis.load_case, &surface_polys, arrow_len);
        // Labels become unreadable on dense models: drop them past a limit.
        let n = s.labels.len() - before;
        if !vis.load_values || n > MAX_LOAD_LABELS {
            s.labels.truncate(before);
            if vis.load_values {
                s.notes.push(format!(
                    "{n} load values hidden (more than {MAX_LOAD_LABELS})"
                ));
            }
        }
    }
    s
}

fn support_symbol(sup: &Object) -> SupportSymbol {
    let rot = [
        "rotational_restraint_x",
        "rotational_restraint_y",
        "rotational_restraint_z",
    ];
    if rot.iter().all(|k| is_fixed(f(sup, k))) {
        SupportSymbol::Fixed
    } else {
        SupportSymbol::Hinged
    }
}

/// Width / height of an equivalent rectangle with the section's A, Iy and Iz
/// (h = sqrt(12 Iy / A), b = sqrt(12 Iz / A)); 0.2 m square if unknown.
fn section_size(model: &Model, section: Option<u32>) -> (f32, f32) {
    let sec = section.and_then(|n| model.get(ObjectId::new(K::Section, n)));
    let (a, iy, iz) = match sec {
        Some(o) => (f(o, "A"), f(o, "I_y"), f(o, "I_z")),
        None => (None, None, None),
    };
    match (a, iy, iz) {
        (Some(a), Some(iy), Some(iz)) if a > 0.0 && iy > 0.0 && iz > 0.0 => {
            ((12.0 * iz / a).sqrt() as f32, (12.0 * iy / a).sqrt() as f32)
        }
        (Some(a), _, _) if a > 0.0 => {
            let d = a.sqrt() as f32;
            (d, d)
        }
        _ => (0.2, 0.2),
    }
}

/// Member local axes in model coordinates (RFEM convention approximated): x along the
/// member; for non-vertical members z lies in the vertical plane, pointing down (global +Z);
/// vertical members take y along global Y. Then rotated by beta about x.
fn member_axes(a: Vec3, b: Vec3, beta: f32) -> (Vec3, Vec3, Vec3) {
    let x = (b - a).normalize_or_zero();
    let (y, z) = if x.z.abs() > 0.999 {
        let y = Vec3::Y;
        (y, x.cross(y).normalize_or_zero())
    } else {
        let z = (Vec3::Z - x * x.z).normalize_or_zero();
        (z.cross(x).normalize_or_zero(), z)
    };
    let (sb, cb) = beta.sin_cos();
    (x, y * cb + z * sb, z * cb - y * sb)
}

/// Wire box around a member segment: b along local y, h along local z.
fn member_box(s: &mut Scene, a: [f64; 3], b: [f64; 3], width: f32, height: f32, beta: f32) {
    let am = Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32);
    let bm = Vec3::new(b[0] as f32, b[1] as f32, b[2] as f32);
    let (_, y, z) = member_axes(am, bm, beta);
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let off: Vec<Vec3> = corners
        .iter()
        .map(|(cy, cz)| y * (cy * width * 0.5) + z * (cz * height * 0.5))
        .collect();
    let (wa, wb) = (to_world(a), to_world(b));
    for i in 0..4 {
        let (o1, o2) = (dir_world(off[i]), dir_world(off[(i + 1) % 4]));
        s.line(wa + o1, wb + o1, MEMBER);
        s.line(wa + o1, wa + o2, MEMBER);
        s.line(wb + o1, wb + o2, MEMBER);
    }
}

/// Ordered boundary points of a surface from its boundary lines (chained end to end).
fn surface_polygon(model: &Model, lines: &[u32]) -> Option<Vec<Vec3>> {
    let mut segs: Vec<Vec<u32>> = lines
        .iter()
        .map(|l| model.line_nodes(*l).to_vec())
        .filter(|n| n.len() >= 2)
        .collect();
    if segs.is_empty() {
        return None;
    }
    let mut chain = segs.remove(0);
    while !segs.is_empty() {
        let end = *chain.last()?;
        let i = segs
            .iter()
            .position(|s| s[0] == end || *s.last().unwrap() == end)?;
        let mut next = segs.remove(i);
        if next[0] != end {
            next.reverse();
        }
        chain.extend_from_slice(&next[1..]);
    }
    if chain.first() == chain.last() {
        chain.pop();
    }
    let pts: Option<Vec<Vec3>> = chain
        .iter()
        .map(|n| model.node_position(*n).map(to_world))
        .collect();
    pts.filter(|p| p.len() >= 3)
}

/// Ear-clipping triangulation of a planar polygon (falls back to a fan if it gets stuck).
fn triangulate(poly: &[Vec3]) -> Vec<[Vec3; 3]> {
    let n = newell_normal(poly);
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let k = idx.len();
        let ear = (0..k).find(|&i| {
            let (a, b, c) = (
                poly[idx[(i + k - 1) % k]],
                poly[idx[i]],
                poly[idx[(i + 1) % k]],
            );
            if (b - a).cross(c - b).dot(n) <= 0.0 {
                return false;
            }
            !idx.iter().any(|&j| {
                let p = poly[j];
                p != a && p != b && p != c && in_triangle(p, a, b, c, n)
            })
        });
        match ear {
            Some(i) => {
                out.push([
                    poly[idx[(i + k - 1) % k]],
                    poly[idx[i]],
                    poly[idx[(i + 1) % k]],
                ]);
                idx.remove(i);
            }
            None => break,
        }
    }
    for i in 1..idx.len().saturating_sub(1) {
        out.push([poly[idx[0]], poly[idx[i]], poly[idx[i + 1]]]);
    }
    out
}

fn in_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3, n: Vec3) -> bool {
    [(a, b), (b, c), (c, a)]
        .iter()
        .all(|(u, v)| (*v - *u).cross(p - *u).dot(n) >= 0.0)
}

fn newell_normal(poly: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        n += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    n.normalize_or_zero()
}

/// Global direction codes seen in RFEM 6.16 (`docs/rfem-tables.md`): nodal loads 0 / 1 / 2 =
/// global X / Y / Z; line, member and surface loads 11 / 12 / 13 = global X / Y / Z and
/// 0 / 1 / 2 = local x / y / z. Returns a model-space unit vector.
fn load_dir(code: i64, nodal: bool, local: Option<(Vec3, Vec3, Vec3)>) -> Option<Vec3> {
    match (code, nodal, local) {
        (0, true, _) => Some(Vec3::X),
        (1, true, _) => Some(Vec3::Y),
        (2, true, _) => Some(Vec3::Z),
        (11, false, _) => Some(Vec3::X),
        (12, false, _) => Some(Vec3::Y),
        (13, false, _) => Some(Vec3::Z),
        (0, false, Some((x, _, _))) => Some(x),
        (1, false, Some((_, y, _))) => Some(y),
        (2, false, Some((_, _, z))) => Some(z),
        _ => None,
    }
}

fn loads(s: &mut Scene, model: &Model, lc: u32, surfaces: &[(u32, Vec<Vec3>)], len: f32) {
    let in_case = |kind| {
        model
            .iter_kind(kind)
            .filter(move |(id, _)| id.load_case == lc)
    };
    let node = |no: u32| model.node_position(no);
    let mut unknown_dir = 0;

    for (_, l) in in_case(K::NodalLoad) {
        let mag = f(l, "magnitude").unwrap_or(0.0);
        let code = l.get("load_direction").as_i64().unwrap_or(2);
        let Some(d) = load_dir(code, true, None) else {
            unknown_dir += 1;
            continue;
        };
        let d = dir_world(d * mag.signum() as f32);
        for n in l.get("nodes").as_ref_list() {
            if let Some(p) = node(*n).map(to_world) {
                s.arrow(p, d, len * 1.5, NODAL_LOAD);
                s.labels.push(Label {
                    pos: p - d * len * 1.6,
                    text: kn(mag.abs(), "kN"),
                    color: NODAL_LOAD,
                });
            }
        }
    }

    // Line loads on lines, member loads on members' lines.
    let mut linear: Vec<(Vec<u32>, &Object)> = Vec::new();
    for (_, l) in in_case(K::LineLoad) {
        linear.push((l.get("lines").as_ref_list().to_vec(), l));
    }
    for (_, l) in in_case(K::MemberLoad) {
        let lines = l
            .get("members")
            .as_ref_list()
            .iter()
            .filter_map(|m| {
                model
                    .get(ObjectId::new(K::Member, *m))?
                    .get("line")
                    .as_ref_no()
            })
            .collect();
        linear.push((lines, l));
    }
    for (lines, l) in linear {
        let p1 = f(l, "magnitude").unwrap_or(0.0);
        let p2 = f(l, "magnitude_2").filter(|v| v.is_finite()).unwrap_or(p1);
        let code = l.get("load_direction").as_i64().unwrap_or(13);
        for line in lines {
            let pts: Vec<[f64; 3]> = model
                .line_nodes(line)
                .iter()
                .filter_map(|n| node(*n))
                .collect();
            let (Some(a), Some(b)) = (pts.first(), pts.last()) else {
                continue;
            };
            let am = Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32);
            let bm = Vec3::new(b[0] as f32, b[1] as f32, b[2] as f32);
            let Some(d) = load_dir(code, false, Some(member_axes(am, bm, 0.0))) else {
                unknown_dir += 1;
                continue;
            };
            let (wa, wb) = (to_world(*a), to_world(*b));
            let steps = 6;
            let mut tails = Vec::new();
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let m = p1 + (p2 - p1) * t as f64;
                if m == 0.0 {
                    continue;
                }
                let dw = dir_world(d * m.signum() as f32);
                let tip = wa.lerp(wb, t);
                s.arrow(tip, dw, len, LINE_LOAD);
                tails.push(tip - dw * len);
            }
            for w in tails.windows(2) {
                s.line(w[0], w[1], LINE_LOAD);
            }
            let mid = wa.lerp(wb, 0.5) - dir_world(d * p1.signum() as f32) * len * 1.25;
            let text = if (p2 - p1).abs() > 1e-9 {
                format!("{} .. {}", kn(p1, "kN/m"), kn(p2, "kN/m"))
            } else {
                kn(p1.abs(), "kN/m")
            };
            s.labels.push(Label {
                pos: mid,
                text,
                color: LINE_LOAD,
            });
        }
    }

    for (_, l) in in_case(K::SurfaceLoad) {
        let Some(mag) = f(l, "uniform_magnitude").filter(|v| *v != 0.0) else {
            continue;
        };
        let code = l.get("load_direction").as_i64().unwrap_or(13);
        for sn in l.get("surfaces").as_ref_list() {
            let Some((_, poly)) = surfaces.iter().find(|(no, _)| no == sn) else {
                continue;
            };
            let nw = newell_normal(poly);
            // Local z of the surface (model space), for local directions.
            let nm = dir_world(nw);
            let local = Some((Vec3::X, nm.cross(Vec3::X).normalize_or_zero(), nm));
            let Some(d) = load_dir(code, false, local) else {
                unknown_dir += 1;
                continue;
            };
            let dw = dir_world(d * mag.signum() as f32);
            let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
            surface_load_block(s, poly, dw, len);
            s.labels.push(Label {
                pos: c - dw * len * 1.25,
                text: kn(mag.abs(), "kN/m²"),
                color: SURFACE_LOAD,
            });
        }
    }
    if unknown_dir > 0 {
        s.notes.push(format!(
            "{unknown_dir} load(s) with an unknown direction code not drawn"
        ));
    }
}

/// The first load case that has any loads (to show something useful on opening).
pub fn first_loaded_case(model: &Model) -> Option<u32> {
    K::ALL
        .iter()
        .filter(|k| k.spec().is_load())
        .flat_map(|k| model.iter_kind(*k).map(|(id, _)| id.load_case))
        .filter(|lc| *lc > 0)
        .min()
}

/// Uniform surface load drawn as RFEM does: a translucent block of height `h` standing on
/// the surface on the load side (`dir` points towards the surface), with outlined edges and
/// one arrow showing the direction.
fn surface_load_block(s: &mut Scene, poly: &[Vec3], dir: Vec3, h: f32) {
    let top: Vec<Vec3> = poly.iter().map(|p| *p - dir * h).collect();
    for [a, b, c] in triangulate(&top) {
        s.tri(a, b, c, SURFACE_LOAD_FILL);
    }
    let n = poly.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b, ta, tb) = (poly[i], poly[j], top[i], top[j]);
        s.tri(a, b, tb, SURFACE_LOAD_FILL);
        s.tri(a, tb, ta, SURFACE_LOAD_FILL);
        s.line(ta, tb, SURFACE_LOAD);
        s.line(a, ta, SURFACE_LOAD);
    }
    let c = poly.iter().copied().sum::<Vec3>() / n as f32;
    s.arrow(c, dir, h * 0.9, SURFACE_LOAD);
}

/// Load cases for the selector: (number, name).
pub fn load_cases(model: &Model) -> Vec<(u32, String)> {
    model
        .iter_kind(K::LoadCase)
        .map(|(id, o)| {
            let name = match o.get("name") {
                Value::Text(t) if !t.is_empty() => t.clone(),
                _ => String::new(),
            };
            (id.no, name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_triangulates_into_two() {
        let sq = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        assert_eq!(triangulate(&sq).len(), 2);
        // L-shape (concave) gives 4 triangles, none outside.
        let l = [
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        ];
        let tris = triangulate(&l);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|[a, b, c]| (*b - *a).cross(*c - *a).length() * 0.5)
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
    }

    #[test]
    fn kn_format() {
        assert_eq!(kn(10_000.0, "kN"), "10 kN");
        assert_eq!(kn(2_500.0, "kN/m"), "2.5 kN/m");
    }
}
