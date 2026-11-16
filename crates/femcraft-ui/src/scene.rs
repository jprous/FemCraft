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
const SUPPORT_FILL: [f32; 4] = [0.35, 0.85, 0.40, 0.20];
const SUPPORT_EDGE: [f32; 4] = [0.35, 0.85, 0.40, 0.70];
/// Supports with a gap or other nonlinearity (baby blue).
pub const NONLINEAR_SUPPORT: [f32; 4] = [0.54, 0.81, 0.94, 1.0];
const NONLINEAR_FILL: [f32; 4] = [0.54, 0.81, 0.94, 0.22];
const NONLINEAR_EDGE: [f32; 4] = [0.54, 0.81, 0.94, 0.75];
const NL_KEYS: &[&str] = &[
    "nonlinearity_x",
    "nonlinearity_y",
    "nonlinearity_z",
    "nonlinearity_rx",
    "nonlinearity_ry",
    "nonlinearity_rz",
];
const AXIS_X: [f32; 4] = [0.95, 0.30, 0.30, 1.0];
const AXIS_Y: [f32; 4] = [0.35, 0.85, 0.35, 1.0];
const AXIS_Z: [f32; 4] = [0.35, 0.60, 1.0, 1.0];

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
    /// Local axis triads on members and surfaces.
    pub local_axes: bool,
    /// Load case whose loads are shown.
    pub load_case: u32,
    /// Spacing of support glyphs along lines and surface edges (the grid spacing), metres.
    pub glyph_spacing: f32,
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
            local_axes: false,
            load_case: 1,
            glyph_spacing: 1.0,
        }
    }
}

/// Support glyph type, from the support's stiffnesses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SupportSymbol {
    /// All translations and rotations (or surface shear) restrained: horizontal rectangle.
    Fixed,
    /// All translations restrained, rotations free: triangle.
    Pinned,
    /// Some translations free: triangle with a line below.
    Roller,
    /// A finite translational spring: zig-zag along the spring direction.
    Spring,
}

/// A support glyph at a world point. `dir` is the world direction of the restraint (for
/// springs: global Z for nodes / lines, the surface's local z for surface supports).
#[derive(Clone, Copy, Debug)]
pub struct SupportGlyph {
    pub pos: Vec3,
    pub symbol: SupportSymbol,
    pub dir: Vec3,
    /// A gap or other nonlinearity is defined: drawn in baby blue.
    pub nonlinear: bool,
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
    /// Selectable objects with their world points (node: 1 point, line / member: polyline,
    /// surface: boundary polygon).
    pub pick: Vec<(ObjectId, Vec<Vec3>)>,
    pub supports: Vec<SupportGlyph>,
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
            s.pick.push((ObjectId::new(K::Surface, *no), poly.clone()));
        }
    }

    if vis.lines {
        for (id, _) in model.iter_kind(K::Line) {
            let pts: Vec<Vec3> = model
                .line_nodes(id.no)
                .iter()
                .filter_map(|n| node(*n))
                .collect();
            if pts.len() >= 2 {
                s.pick.push((id, pts.clone()));
            }
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
            if pts.len() >= 2 {
                s.pick
                    .push((id, pts.iter().map(|p| to_world(*p)).collect()));
            }
        }
    }

    if vis.local_axes {
        let size = (arrow_len * 0.8).max(0.1);
        for (_, m) in model.iter_kind(K::Member) {
            let Some(line) = m.get("line").as_ref_no() else {
                continue;
            };
            let pts: Vec<Vec3> = model
                .line_nodes(line)
                .iter()
                .filter_map(|n| model.node_position(*n))
                .map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32))
                .collect();
            if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                let beta = f(m, "rotation_angle").unwrap_or(0.0) as f32;
                let mid = to_world([a.x as f64, a.y as f64, a.z as f64])
                    .lerp(to_world([b.x as f64, b.y as f64, b.z as f64]), 0.5);
                axes_triad(&mut s, mid, member_axes(*a, *b, beta), size);
            }
        }
        for (no, poly) in &surface_polys {
            let srf = model.get(ObjectId::new(K::Surface, *no));
            let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
            axes_triad(&mut s, c, surface_axes(poly, srf), size);
        }
    }

    if vis.nodes {
        for (id, _) in model.iter_kind(K::Node) {
            if let Some(p) = node(id.no) {
                s.nodes.push(p);
                s.pick.push((id, vec![p]));
            }
        }
    }

    if vis.supports {
        let spacing = vis.glyph_spacing.max(0.01);
        // Support volumes are half as deep as surface load blocks.
        let support_depth = arrow_len * 0.5;
        // Springs of nodal / line supports act along global Z (model), drawn in world.
        let global_z = dir_world(Vec3::Z);
        for (_, sup) in model.iter_kind(K::NodalSupport) {
            let symbol = nodal_or_line_symbol(sup);
            let nonlinear = is_nonlinear(sup, NL_KEYS);
            for n in sup.get("nodes").as_ref_list() {
                if let Some(pos) = node(*n) {
                    s.supports.push(SupportGlyph {
                        pos,
                        symbol,
                        dir: global_z,
                        nonlinear,
                    });
                }
            }
        }
        for (_, sup) in model.iter_kind(K::LineSupport) {
            let symbol = nodal_or_line_symbol(sup);
            let nonlinear = is_nonlinear(sup, NL_KEYS);
            let (fill, edge) = support_colors(nonlinear);
            for l in sup.get("lines").as_ref_list() {
                let pts: Vec<Vec3> = model
                    .line_nodes(*l)
                    .iter()
                    .filter_map(|n| node(*n))
                    .collect();
                for w in pts.windows(2) {
                    // Support side: global +Z (down) made perpendicular to the line; for
                    // vertical lines, global +X.
                    let along = (w[1] - w[0]).normalize_or_zero();
                    let mut side = global_z - along * global_z.dot(along);
                    if side.length_squared() < 1e-4 {
                        side = Vec3::X - along * along.x;
                    }
                    strip(
                        &mut s,
                        w[0],
                        w[1],
                        side.normalize_or_zero() * support_depth,
                        fill,
                        edge,
                    );
                    for pos in points_along(w[0], w[1], spacing, true) {
                        s.supports.push(SupportGlyph {
                            pos,
                            symbol,
                            dir: global_z,
                            nonlinear,
                        });
                    }
                }
            }
        }
        // Surface supports: tinted fill (above) plus glyphs along the boundary edges, one
        // spacing inside the surface.
        for (_, sup) in model.iter_kind(K::SurfaceSupport) {
            let symbol = surface_symbol(sup);
            let nonlinear = is_nonlinear(sup, &["nonlinearity"]);
            let (fill, edge) = support_colors(nonlinear);
            for sn in sup.get("surfaces").as_ref_list() {
                let Some((_, poly)) = surface_polys.iter().find(|(no, _)| no == sn) else {
                    continue;
                };
                let normal = newell_normal(poly);
                // Support side: the surface's positive local z (below a slab).
                let srf = model.get(ObjectId::new(K::Surface, *sn));
                let (_, _, z) = surface_axes(poly, srf);
                prism(&mut s, poly, dir_world(z) * support_depth, fill, edge);
                for pos in inset_edge_points(poly, normal, spacing) {
                    s.supports.push(SupportGlyph {
                        pos,
                        symbol,
                        dir: normal,
                        nonlinear,
                    });
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

/// Glyph from translational stiffnesses and the "rotation" ones (rotations for nodes / lines,
/// shear for surfaces): any finite spring -> spring; all translations fixed -> fixed when the
/// rest are fixed too, else pinned; some translations free -> roller.
fn classify(trans: [Option<f64>; 3], rot: &[Option<f64>]) -> SupportSymbol {
    let spring = |v: &Option<f64>| v.is_some_and(|x| x.is_finite() && x > 0.0);
    if trans.iter().any(spring) {
        SupportSymbol::Spring
    } else if trans.iter().all(|v| is_fixed(*v)) {
        if rot.iter().all(|v| is_fixed(*v)) {
            SupportSymbol::Fixed
        } else {
            SupportSymbol::Pinned
        }
    } else {
        SupportSymbol::Roller
    }
}

/// Any nonlinearity code other than 0 (linear) among keys.
fn is_nonlinear(sup: &Object, keys: &[&str]) -> bool {
    keys.iter()
        .any(|k| sup.get(k).as_i64().is_some_and(|v| v != 0))
}

/// Volume fill and edge colours for a support.
fn support_colors(nonlinear: bool) -> ([f32; 4], [f32; 4]) {
    if nonlinear {
        (NONLINEAR_FILL, NONLINEAR_EDGE)
    } else {
        (SUPPORT_FILL, SUPPORT_EDGE)
    }
}

fn nodal_or_line_symbol(sup: &Object) -> SupportSymbol {
    classify(
        [f(sup, "spring_x"), f(sup, "spring_y"), f(sup, "spring_z")],
        &[
            f(sup, "rotational_restraint_x"),
            f(sup, "rotational_restraint_y"),
            f(sup, "rotational_restraint_z"),
        ],
    )
}

fn surface_symbol(sup: &Object) -> SupportSymbol {
    classify(
        [
            f(sup, "translation_x"),
            f(sup, "translation_y"),
            f(sup, "translation_z"),
        ],
        &[f(sup, "shear_xz"), f(sup, "shear_yz")],
    )
}

/// Points along a segment every `spacing` (at least the midpoint); with `ends`, the end
/// points too. Capped so a fine grid cannot flood the view.
fn points_along(a: Vec3, b: Vec3, spacing: f32, ends: bool) -> Vec<Vec3> {
    let len = a.distance(b);
    let n = ((len / spacing).round() as usize).clamp(1, 400);
    let range = if ends { 0..=n } else { 1..=n.saturating_sub(1) };
    let mut v: Vec<Vec3> = range.map(|i| a.lerp(b, i as f32 / n as f32)).collect();
    if v.is_empty() {
        v.push(a.lerp(b, 0.5));
    }
    v
}

/// Points along each edge of a surface boundary, every `spacing`, moved one `spacing` into
/// the surface (in its plane). Skips the edge ends, which the inset would push past the
/// neighbouring edges; small surfaces get at least the edge midpoints, inset less.
fn inset_edge_points(poly: &[Vec3], normal: Vec3, spacing: f32) -> Vec<Vec3> {
    let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let edge = (b - a).normalize_or_zero();
        let mut inward = normal.cross(edge).normalize_or_zero();
        if inward.dot(c - a.lerp(b, 0.5)) < 0.0 {
            inward = -inward;
        }
        // Do not inset past the middle of the surface.
        let depth = spacing.min(a.lerp(b, 0.5).distance(c) * 0.5);
        for p in points_along(a, b, spacing, false) {
            out.push(p + inward * depth);
        }
    }
    out
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

/// Rotation of a line's local axes (lines carry a rotation like members).
fn line_beta(model: &Model, line: u32) -> f32 {
    model
        .get(ObjectId::new(K::Line, line))
        .and_then(|l| f(l, "rotation_angle"))
        .unwrap_or(0.0) as f32
}

/// Surface local axes in model coordinates from its boundary (world coordinates), following
/// RFEM's defaults as far as known:
/// - z normal to the surface; horizontal or inclined surfaces: z points down (global +Z);
///   vertical surfaces: z towards +X, or +Y when the surface is parallel to X (assumption);
///   then reversed if the surface says so;
/// - x along global X projected onto the surface (global Y when X is normal to it), then
///   rotated about z by the surface's axes rotation;
/// - y = z × x.
fn surface_axes(poly: &[Vec3], srf: Option<&Object>) -> (Vec3, Vec3, Vec3) {
    let model_poly: Vec<Vec3> = poly.iter().map(|p| to_model(*p)).collect();
    let mut z = newell_normal(&model_poly);
    let flip = if z.z.abs() > 1e-3 {
        z.z < 0.0
    } else if z.x.abs() > 1e-3 {
        z.x < 0.0
    } else {
        z.y < 0.0
    };
    if flip {
        z = -z;
    }
    if srf.is_some_and(|o| o.get("axes_z_reversed").as_bool() == Some(true)) {
        z = -z;
    }
    let along = |g: Vec3| (g - z * g.dot(z)).normalize_or_zero();
    let mut x = along(Vec3::X);
    if x.length_squared() < 0.5 {
        x = along(Vec3::Y);
    }
    let rot = srf.and_then(|o| f(o, "axes_rotation")).unwrap_or(0.0) as f32;
    if rot != 0.0 {
        x = glam::Quat::from_axis_angle(z, rot) * x;
    }
    (x, z.cross(x).normalize_or_zero(), z)
}

/// Small x / y / z triad (red / green / blue) at a world point for model-space axes.
fn axes_triad(s: &mut Scene, at: Vec3, (x, y, z): (Vec3, Vec3, Vec3), size: f32) {
    for (d, c) in [(x, AXIS_X), (y, AXIS_Y), (z, AXIS_Z)] {
        s.line(at, at + dir_world(d) * size, c);
    }
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

    // Line loads on lines (line axes, line rotation), member loads on members' lines
    // (member axes, member rotation beta). Each entry: (line, beta).
    let mut linear: Vec<(Vec<(u32, f32)>, &Object)> = Vec::new();
    for (_, l) in in_case(K::LineLoad) {
        let lines = l
            .get("lines")
            .as_ref_list()
            .iter()
            .map(|ln| (*ln, line_beta(model, *ln)))
            .collect();
        linear.push((lines, l));
    }
    for (_, l) in in_case(K::MemberLoad) {
        let lines = l
            .get("members")
            .as_ref_list()
            .iter()
            .filter_map(|m| {
                let mo = model.get(ObjectId::new(K::Member, *m))?;
                Some((
                    mo.get("line").as_ref_no()?,
                    f(mo, "rotation_angle").unwrap_or(0.0) as f32,
                ))
            })
            .collect();
        linear.push((lines, l));
    }
    for (lines, l) in linear {
        let p1 = f(l, "magnitude").unwrap_or(0.0);
        let p2 = f(l, "magnitude_2").filter(|v| v.is_finite()).unwrap_or(p1);
        let code = l.get("load_direction").as_i64().unwrap_or(13);
        for (line, beta) in lines {
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
            let Some(d) = load_dir(code, false, Some(member_axes(am, bm, beta))) else {
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
            let srf = model.get(ObjectId::new(K::Surface, *sn));
            let local = Some(surface_axes(poly, srf));
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
    prism(s, poly, -dir * h, SURFACE_LOAD_FILL, SURFACE_LOAD);
    let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
    s.arrow(c, dir, h * 0.9, SURFACE_LOAD);
}

/// Translucent block between a polygon and the same polygon moved by `offset` (world):
/// filled far face and sides, outlined far edges and side edges.
fn prism(s: &mut Scene, poly: &[Vec3], offset: Vec3, fill: [f32; 4], edge: [f32; 4]) {
    let top: Vec<Vec3> = poly.iter().map(|p| *p + offset).collect();
    for [a, b, c] in triangulate(&top) {
        s.tri(a, b, c, fill);
    }
    let n = poly.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b, ta, tb) = (poly[i], poly[j], top[i], top[j]);
        s.tri(a, b, tb, fill);
        s.tri(a, tb, ta, fill);
        s.line(ta, tb, edge);
        s.line(a, ta, edge);
    }
}

/// Translucent strip hanging from a line segment by `offset` (world), outlined.
fn strip(s: &mut Scene, a: Vec3, b: Vec3, offset: Vec3, fill: [f32; 4], edge: [f32; 4]) {
    let (ta, tb) = (a + offset, b + offset);
    s.tri(a, b, tb, fill);
    s.tri(a, tb, ta, fill);
    s.line(ta, tb, edge);
    s.line(a, ta, edge);
    s.line(b, tb, edge);
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

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-5
    }

    #[test]
    fn member_local_axes() {
        // Horizontal member along +X: y = +Y, z = +Z (down), right-handed.
        let (x, y, z) = member_axes(Vec3::ZERO, Vec3::X * 6.0, 0.0);
        assert!(close(x, Vec3::X) && close(y, Vec3::Y) && close(z, Vec3::Z));
        // beta = 90 degrees turns y onto z and z onto -y.
        let (_, y, z) = member_axes(Vec3::ZERO, Vec3::X * 6.0, std::f32::consts::FRAC_PI_2);
        assert!(close(y, Vec3::Z) && close(z, -Vec3::Y));
        // Inclined member (roof): z stays in the vertical plane, pointing down.
        let (x, y, z) = member_axes(Vec3::ZERO, Vec3::new(4.0, 0.0, -1.0), 0.0);
        assert!(z.z > 0.0 && z.dot(x).abs() < 1e-5 && close(y, Vec3::Y));
        // Column (bottom to top, i.e. towards -Z): y along global Y.
        let (_, y, _) = member_axes(Vec3::ZERO, Vec3::NEG_Z * 4.0, 0.0);
        assert!(close(y, Vec3::Y));
    }

    #[test]
    fn surface_local_axes() {
        // Horizontal slab (world coordinates in, model axes out): z down, x = X, y = Y.
        let slab: Vec<Vec3> = [[0., 0., 0.], [3., 0., 0.], [3., 3., 0.], [0., 3., 0.]]
            .iter()
            .map(|p| to_world(*p))
            .collect();
        let (x, y, z) = surface_axes(&slab, None);
        assert!(close(x, Vec3::X) && close(y, Vec3::Y) && close(z, Vec3::Z));
        // Wall in the XZ plane: z along +Y (assumed default), x along X.
        let wall: Vec<Vec3> = [[0., 0., 0.], [3., 0., 0.], [3., 0., -3.], [0., 0., -3.]]
            .iter()
            .map(|p| to_world(*p))
            .collect();
        let (x, _, z) = surface_axes(&wall, None);
        assert!(close(z, Vec3::Y) && close(x, Vec3::X));
    }

    #[test]
    fn support_classes() {
        let (inf, free) = (Some(f64::INFINITY), Some(0.0));
        use SupportSymbol::*;
        assert_eq!(classify([inf; 3], &[inf; 3]), Fixed);
        assert_eq!(classify([inf; 3], &[free, free, inf]), Pinned);
        assert_eq!(classify([free, free, inf], &[free; 3]), Roller);
        assert_eq!(classify([inf, inf, Some(5e7)], &[inf; 2]), Spring);
    }

    #[test]
    fn inset_points_on_a_3m_square() {
        let sq = [
            Vec3::ZERO,
            Vec3::X * 3.0,
            Vec3::new(3.0, 3.0, 0.0),
            Vec3::Y * 3.0,
        ];
        let pts = inset_edge_points(&sq, Vec3::Z, 1.0);
        // Two interior points per edge, all strictly inside the square.
        assert_eq!(pts.len(), 8);
        assert!(
            pts.iter()
                .all(|p| p.x > 0.1 && p.x < 2.9 && p.y > 0.1 && p.y < 2.9)
        );
    }

    #[test]
    fn kn_format() {
        assert_eq!(kn(10_000.0, "kN"), "10 kN");
        assert_eq!(kn(2_500.0, "kN/m"), "2.5 kN/m");
    }
}
