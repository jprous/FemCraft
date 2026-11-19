//! Turns a model into drawable geometry: GPU lines / triangles plus 2D markers (nodes,
//! support symbols, load labels) that the viewport paints on top.
//!
//! Coordinates: RFEM's global Z points down. The viewport world has Z up, so a model point
//! `(x, y, z)` is drawn at `(x, -y, -z)` (a 180 degree turn about X, which keeps the axes
//! right-handed and shows the model the way RFEM does).

use std::collections::{HashMap, HashSet};

use femcraft_model::{Model, Object, ObjectId, ObjectKind as K, Value};
use femcraft_render::viewport::LineVertex;
use glam::Vec3;

pub const NODE: [f32; 4] = [0.95, 0.20, 0.20, 1.0];
const LINE: [f32; 4] = [0.62, 0.66, 0.72, 1.0];
pub const MEMBER: [f32; 4] = [0.45, 0.72, 1.0, 1.0];
const SURFACE_FILL: [f32; 4] = [0.30, 0.62, 0.70, 0.28];
const SUPPORTED_FILL: [f32; 4] = [0.62, 0.48, 0.28, 0.32];
pub const SUPPORT: [f32; 4] = [0.35, 0.85, 0.40, 1.0];
pub const NODAL_LOAD: [f32; 4] = [1.0, 0.35, 0.30, 1.0];
pub const LINE_LOAD: [f32; 4] = [1.0, 0.62, 0.20, 1.0];
pub const SURFACE_LOAD: [f32; 4] = [0.98, 0.86, 0.30, 1.0];
const OPENING_EDGE: [f32; 4] = [0.90, 0.60, 0.20, 1.0];
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

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Visibility {
    pub nodes: bool,
    pub lines: bool,
    pub members: bool,
    /// Members drawn with their section profile; off: a line 1 px thicker than a line element.
    pub member_profiles: bool,
    pub surfaces: bool,
    pub supports: bool,
    pub loads: bool,
    /// Value labels next to loads.
    pub load_values: bool,
    /// Local axis triads on members and surfaces.
    pub local_axes: bool,
    /// FE mesh saved by RFEM, while the model is unedited.
    pub fe_mesh: bool,
    /// Result overlay (contour) for the selected result.
    pub results: bool,
    /// Results drawn in the RFEM colour bands; off: plain yellow lines (members: the line plot,
    /// surfaces: the displaced mesh net, no contour).
    pub result_colours: bool,
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
            member_profiles: true,
            surfaces: true,
            supports: true,
            loads: true,
            load_values: true,
            local_axes: false,
            fe_mesh: true,
            results: true,
            result_colours: true,
            load_case: 1,
            glyph_spacing: 1.0,
        }
    }
}

impl Visibility {
    /// Takes the display toggles of `other`, keeping this model's load case and glyph spacing.
    pub fn take_toggles(&mut self, other: &Visibility) {
        *self = Visibility {
            load_case: self.load_case,
            glyph_spacing: self.glyph_spacing,
            ..other.clone()
        };
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
    /// Opaque result-contour triangles, drawn through a depth-tested pipeline so the surface
    /// occludes itself correctly from any angle (kept apart from the translucent model fills).
    pub result_triangles: Vec<LineVertex>,
    pub nodes: Vec<Vec3>,
    /// Members without profile: segments painted in screen space, thicker than GPU lines.
    pub member_lines: Vec<(Vec3, Vec3)>,
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

/// FE element edges (medium grey) on top of the scene.
pub fn add_fe_mesh(s: &mut Scene, mesh: &femcraft_rfdb::FeMesh) {
    /// Thinner than model lines.
    const MESH_WIDTH: f32 = 0.6;
    const MESH: [f32; 4] = [0.55, 0.55, 0.58, 0.9];
    for (a, b) in mesh.edges() {
        if let (Some(a), Some(b)) = (mesh.node(a), mesh.node(b)) {
            s.lines
                .push(LineVertex::new(to_world(a), MESH).with_width(MESH_WIDTH));
            s.lines
                .push(LineVertex::new(to_world(b), MESH).with_width(MESH_WIDTH));
        }
    }
}

/// Bright orange for member deformation lines, like RFEM's deformed shape.
const DEFORM_LINE: [f32; 4] = [1.0, 0.55, 0.0, 1.0];

/// The scalar a displacement [`ResultComponent`] colours by (`None` for quantities not yet
/// decoded, e.g. forces and stresses). `u` is `[u_x, u_y, u_z, φ_x, φ_y, φ_z]` (m, rad).
fn displacement_scalar(c: femcraft_rfdb::ResultComponent, u: [f64; 6]) -> Option<f64> {
    use femcraft_rfdb::ResultComponent as C;
    Some(match c {
        C::GlobalU => (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt(),
        C::GlobalUx | C::SurfaceUx | C::MemberUx => u[0],
        C::GlobalUy | C::SurfaceUy | C::MemberUy => u[1],
        C::GlobalUz | C::SurfaceUz | C::MemberUz => u[2],
        C::GlobalPhiX | C::SurfacePhiX | C::MemberPhiX => u[3],
        C::GlobalPhiY | C::SurfacePhiY | C::MemberPhiY => u[4],
        C::GlobalPhiZ | C::SurfacePhiZ | C::MemberPhiZ => u[5],
        _ => return None,
    })
}

/// True for a component this build can display (a nodal displacement quantity).
pub fn is_displacement(c: femcraft_rfdb::ResultComponent) -> bool {
    displacement_scalar(c, [0.0; 6]).is_some()
}

/// Displaced world position of FE node `no`: model point plus `magnification × translation`.
fn displaced(
    mesh: &femcraft_rfdb::FeMesh,
    disp: &femcraft_rfdb::Displacements,
    no: u32,
    magnification: f64,
    mask: [f64; 3],
) -> Option<Vec3> {
    let p = mesh.node(no)?;
    let u = disp.node(no)?;
    Some(to_world([
        p[0] + u[0] * mask[0] * magnification,
        p[1] + u[1] * mask[1] * magnification,
        p[2] + u[2] * mask[2] * magnification,
    ]))
}

/// [`displaced`] with an explicit displacement record `u`.
fn displaced_by(
    mesh: &femcraft_rfdb::FeMesh,
    no: u32,
    u: [f64; 6],
    magnification: f64,
    mask: [f64; 3],
) -> Option<Vec3> {
    let p = mesh.node(no)?;
    Some(to_world([
        p[0] + u[0] * mask[0] * magnification,
        p[1] + u[1] * mask[1] * magnification,
        p[2] + u[2] * mask[2] * magnification,
    ]))
}

/// Which translations move the plotted geometry: a single translation component moves the
/// shape along that axis only; |u| and the rotations (which would not be visible otherwise)
/// show the full translation.
fn geometry_mask(c: femcraft_rfdb::ResultComponent) -> [f64; 3] {
    use femcraft_rfdb::ResultComponent as C;
    match c {
        C::GlobalUx | C::SurfaceUx | C::MemberUx => [1.0, 0.0, 0.0],
        C::GlobalUy | C::SurfaceUy | C::MemberUy => [0.0, 1.0, 0.0],
        C::GlobalUz | C::SurfaceUz | C::MemberUz => [0.0, 0.0, 1.0],
        _ => [1.0, 1.0, 1.0],
    }
}

/// Yellow of the plain (uncoloured) result lines.
const RESULT_YELLOW: [f32; 4] = [1.0, 0.9, 0.0, 1.0];
/// Width of a colour-banded result line.
const RESULT_WIDTH: f32 = 4.5;
/// Width of the plain yellow result lines (a little over a third of the coloured ones).
const RESULT_PLAIN_WIDTH: f32 = 1.6;
/// Width of the displaced mesh net of a surface result (thinner: there are many lines).
const RESULT_NET_WIDTH: f32 = 1.0;

/// Adds the contour of a displacement result as discrete RFEM colour bands: the FE surface mesh
/// displaced by `magnification`, every triangle split at the band boundaries so each value range
/// fills as one solid colour that matches the legend's blocks. `disps` holds one displacement set
/// per case enveloped (a single set for a load case / combination); with several, each node takes
/// the `extremum` of its values and the mesh is drawn undeformed. Returns the value range
/// `(min, max)`, or `None` when `component` is not a (decoded) displacement.
///
/// No element outlines are drawn: without a depth buffer a net of lines over the displaced surface
/// shows the far side through the near one and reads as inside-out. The bands alone are clearer.
pub fn add_result_surface(
    s: &mut Scene,
    mesh: &femcraft_rfdb::FeMesh,
    disps: &[&femcraft_rfdb::Displacements],
    component: femcraft_rfdb::ResultComponent,
    magnification: f64,
    extremum: femcraft_rfdb::Extremum,
    coloured: bool,
) -> Option<(f32, f32)> {
    let first = disps.first()?;
    let magnification = if disps.len() == 1 { magnification } else { 0.0 };
    let mask = geometry_mask(component);
    // Per FE node (1-based): displaced world position and the plotted scalar.
    let mut span = Span::default();
    let node: Vec<Option<(Vec3, f64)>> = (1..=mesh.nodes.len() as u32)
        .map(|no| {
            let values: Vec<f64> = disps
                .iter()
                .filter_map(|d| d.node(no).and_then(|u| displacement_scalar(component, u)))
                .collect();
            span.add(&values);
            Some((
                displaced(mesh, first, no, magnification, mask)?,
                extremum.reduce(values.into_iter())?,
            ))
        })
        .collect();
    if !coloured {
        // The displaced mesh net in plain yellow, no contour.
        for (a, b) in mesh.edges() {
            let at = |no: u32| node.get(no as usize - 1).copied().flatten();
            if let (Some(pa), Some(pb)) = (at(a), at(b)) {
                s.lines
                    .push(LineVertex::new(pa.0, RESULT_YELLOW).with_width(RESULT_NET_WIDTH));
                s.lines
                    .push(LineVertex::new(pb.0, RESULT_YELLOW).with_width(RESULT_NET_WIDTH));
            }
        }
        return span.0.map(|(lo, hi)| (lo as f32, hi as f32));
    }
    let at = |no: u32| node.get((no as usize).checked_sub(1)?).copied().flatten();
    let elems: Vec<Vec<(Vec3, f64)>> = mesh
        .elements
        .iter()
        .filter_map(|e| element_corners(e).iter().map(|&no| at(no)).collect())
        .collect();
    band_elements(s, &elems, span.range(extremum))
}

/// Lowest and highest value over every enveloped case and node. "Both" plots the full range
/// from the lowest minimum to the highest maximum.
#[derive(Default)]
struct Span(Option<(f64, f64)>);

impl Span {
    fn add(&mut self, values: &[f64]) {
        for &v in values {
            let (lo, hi) = self.0.unwrap_or((v, v));
            self.0 = Some((lo.min(v), hi.max(v)));
        }
    }

    /// The scale range to force for `Both`; other extremes use the plotted values' range.
    fn range(&self, extremum: femcraft_rfdb::Extremum) -> Option<(f64, f64)> {
        (extremum == femcraft_rfdb::Extremum::Both)
            .then_some(self.0)
            .flatten()
    }
}

/// Corner node numbers of an element: a triangle (`d == 0` or `d == c`) or a quad.
fn element_corners(e: &[u32; 4]) -> &[u32] {
    if e[3] != 0 && e[3] != e[2] {
        &e[..4]
    } else {
        &e[..3]
    }
}

/// Fills the elements (corner positions with values) in solid colour bands over their value range
/// and returns that range.
fn band_elements(
    s: &mut Scene,
    elems: &[Vec<(Vec3, f64)>],
    forced: Option<(f64, f64)>,
) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in elems.iter().flatten() {
        lo = lo.min(v.1);
        hi = hi.max(v.1);
    }
    if let Some((flo, fhi)) = forced {
        lo = lo.min(flo);
        hi = hi.max(fhi);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let bands = femcraft_render::colorscale::BAND_COUNT;
    let step = ((hi - lo) / bands as f64).max(f64::MIN_POSITIVE);
    for verts in elems {
        for k in 1..verts.len() - 1 {
            band_fill_triangle(s, [verts[0], verts[k], verts[k + 1]], lo, step, bands);
        }
    }
    Some((lo as f32, hi as f32))
}

/// Spring stiffness of a surface support that produces contact stress (SI: N/m³).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactSupport {
    /// Spring along the surface's local z (`translation_z`).
    pub cz: f64,
    /// Failure if the contact stress is negative (tension-free): negative σ_z shows as zero.
    pub tension_free: bool,
}

/// True for a quantity plotted from support springs and displacements (contact stress σ_z).
/// RFEM does not store contact stresses in the results; they are `c_z × u_z` (checked against
/// RFEM to ~0.5 % on `Chamber 4x4x6.rf6`, Dead Load).
pub fn is_contact(c: femcraft_rfdb::ResultComponent) -> bool {
    matches!(c, femcraft_rfdb::ResultComponent::SurfaceContactSigmaZ)
}

/// Unit normal of an element (from its first three corners), oriented towards +Z (model axes,
/// RFEM's z points down) when it is closer to vertical than horizontal.
fn element_normal(mesh: &femcraft_rfdb::FeMesh, e: &[u32; 4]) -> Option<[f64; 3]> {
    let (a, b, c) = (mesh.node(e[0])?, mesh.node(e[1])?, mesh.node(e[2])?);
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let mut n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-12 {
        return None;
    }
    n.iter_mut().for_each(|x| *x /= len);
    if n[2].abs() > 0.5 && n[2] < 0.0 {
        n.iter_mut().for_each(|x| *x = -*x);
    }
    Some(n)
}

/// Adds the contact-stress contour: only elements of surfaces that carry a spring support
/// (`supports`: surface no -> stiffness), drawn undeformed, coloured by sigma_z = c_z * u_n at each
/// FE node. `disps` holds one displacement set per case enveloped; each node takes the
/// `extremum` over them. Returns the value range `(min, max)` in Pa, or `None` when nothing is
/// supported.
pub fn add_contact_surface(
    s: &mut Scene,
    mesh: &femcraft_rfdb::FeMesh,
    disps: &[&femcraft_rfdb::Displacements],
    supports: &HashMap<u32, ContactSupport>,
    extremum: femcraft_rfdb::Extremum,
) -> Option<(f32, f32)> {
    let first = disps.first()?;
    let mut elems: Vec<Vec<(Vec3, f64)>> = Vec::new();
    let mut span = Span::default();
    for (i, e) in mesh.elements.iter().enumerate() {
        let Some(sup) = mesh.element_surface.get(i).and_then(|n| supports.get(n)) else {
            continue;
        };
        let Some(n) = element_normal(mesh, e) else {
            continue;
        };
        let verts: Option<Vec<(Vec3, f64)>> = element_corners(e)
            .iter()
            .map(|&no| {
                let sigma: Vec<f64> = disps
                    .iter()
                    .filter_map(|d| {
                        let u = d.node(no)?;
                        let sigma = sup.cz * (u[0] * n[0] + u[1] * n[1] + u[2] * n[2]);
                        Some(if sup.tension_free {
                            sigma.max(0.0)
                        } else {
                            sigma
                        })
                    })
                    .collect();
                span.add(&sigma);
                Some((
                    displaced(mesh, first, no, 0.0, [1.0; 3])?,
                    extremum.reduce(sigma.into_iter())?,
                ))
            })
            .collect();
        elems.extend(verts);
    }
    band_elements(s, &elems, span.range(extremum))
}

/// Splits one triangle (vertices carry `(displaced position, value)`) into the colour bands it
/// spans, filling each with its solid RFEM band colour.
fn band_fill_triangle(s: &mut Scene, tri: [(Vec3, f64); 3], lo: f64, step: f64, bands: usize) {
    let band_of = |v: f64| (((v - lo) / step).floor() as isize).clamp(0, bands as isize - 1);
    let (tmin, tmax) = tri
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v.1), b.max(v.1))
        });
    for k in band_of(tmin)..=band_of(tmax) {
        let lower = lo + k as f64 * step;
        let mut poly: Vec<(Vec3, f64)> = tri.to_vec();
        if k > 0 {
            poly = clip_band(&poly, lower, true);
        }
        if (k as usize) < bands - 1 {
            poly = clip_band(&poly, lower + step, false);
        }
        if poly.len() < 3 {
            continue;
        }
        let c = femcraft_render::colorscale::rfem_band(k as usize);
        for i in 1..poly.len() - 1 {
            s.result_triangles.push(LineVertex::new(poly[0].0, c));
            s.result_triangles.push(LineVertex::new(poly[i].0, c));
            s.result_triangles.push(LineVertex::new(poly[i + 1].0, c));
        }
    }
}

/// Sutherland–Hodgman clip of a value-carrying polygon by a scalar threshold: keeps the part with
/// value ≥ `t` (`keep_ge`) or ≤ `t`, inserting interpolated vertices where edges cross.
fn clip_band(poly: &[(Vec3, f64)], t: f64, keep_ge: bool) -> Vec<(Vec3, f64)> {
    let inside = |v: f64| if keep_ge { v >= t } else { v <= t };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let cur = poly[i];
        let nxt = poly[(i + 1) % poly.len()];
        if inside(cur.1) {
            out.push(cur);
        }
        if inside(cur.1) != inside(nxt.1) {
            let denom = nxt.1 - cur.1;
            if denom.abs() > f64::EPSILON {
                let f = ((t - cur.1) / denom).clamp(0.0, 1.0) as f32;
                out.push((cur.0 + (nxt.0 - cur.0) * f, t));
            }
        }
    }
    out
}

/// Adds deformed member axes as bright orange polylines: each listed line's FE node chain,
/// displaced by `magnification`. `member_lines` are the model lines that carry a member.
pub fn add_member_deformation(
    s: &mut Scene,
    mesh: &femcraft_rfdb::FeMesh,
    disp: &femcraft_rfdb::Displacements,
    member_lines: &HashSet<u32>,
    magnification: f64,
) {
    for (line_no, chain) in &mesh.lines {
        if !member_lines.contains(line_no) {
            continue;
        }
        for pair in chain.windows(2) {
            if let (Some(a), Some(b)) = (
                displaced(mesh, disp, pair[0], magnification, [1.0; 3]),
                displaced(mesh, disp, pair[1], magnification, [1.0; 3]),
            ) {
                s.line(a, b, DEFORM_LINE);
            }
        }
    }
}

/// Colours the member axes by a displacement quantity: every FE segment along a member line
/// takes the RFEM colour band of its mean value, drawn thick on the displaced shape. Returns the
/// value range `(min, max)` or `None` when `component` is not a displacement or no member line
/// has results.
pub fn add_result_members(
    s: &mut Scene,
    mesh: &femcraft_rfdb::FeMesh,
    disps: &[&femcraft_rfdb::Displacements],
    member_lines: &HashSet<u32>,
    component: femcraft_rfdb::ResultComponent,
    magnification: f64,
    extremum: femcraft_rfdb::Extremum,
    coloured: bool,
) -> Option<(f32, f32)> {
    use femcraft_rfdb::Extremum;
    disps.first()?;
    let mask = geometry_mask(component);
    // The shapes to draw. One case: its own displacements. An envelope: the shape that reaches
    // the maximum at every node, the one that reaches the minimum, or (Both) both in one view.
    #[derive(Clone, Copy, PartialEq)]
    enum Shape {
        Case,
        Max,
        Min,
    }
    let shapes: &[Shape] = if disps.len() == 1 {
        &[Shape::Case]
    } else {
        match extremum {
            Extremum::Max => &[Shape::Max],
            Extremum::Min => &[Shape::Min],
            Extremum::Both => &[Shape::Max, Shape::Min],
        }
    };
    // Displacement record and plotted value of a node on a shape.
    let at = |no: u32, shape: Shape| -> Option<([f64; 6], f64)> {
        let mut best: Option<([f64; 6], f64)> = None;
        for d in disps {
            let Some(u) = d.node(no) else { continue };
            let Some(v) = displacement_scalar(component, u) else {
                continue;
            };
            let better = match (shape, &best) {
                (_, None) => true,
                (Shape::Min, Some((_, bv))) => v < *bv,
                (_, Some((_, bv))) => v > *bv,
            };
            if better {
                best = Some((u, v));
            }
        }
        best
    };
    let mut segments = Vec::new();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &shape in shapes {
        for (line_no, chain) in &mesh.lines {
            if !member_lines.contains(line_no) {
                continue;
            }
            for pair in chain.windows(2) {
                let (Some((ua, va)), Some((ub, vb))) = (at(pair[0], shape), at(pair[1], shape))
                else {
                    continue;
                };
                let (Some(pa), Some(pb)) = (
                    displaced_by(mesh, pair[0], ua, magnification, mask),
                    displaced_by(mesh, pair[1], ub, magnification, mask),
                ) else {
                    continue;
                };
                lo = lo.min(va).min(vb);
                hi = hi.max(va).max(vb);
                segments.push((pa, pb, 0.5 * (va + vb)));
            }
        }
    }
    if segments.is_empty() {
        return None;
    }
    if !coloured {
        // Line plot only: a thin yellow line along each displaced shape.
        for (a, b, _) in segments {
            s.lines
                .push(LineVertex::new(a, RESULT_YELLOW).with_width(RESULT_PLAIN_WIDTH));
            s.lines
                .push(LineVertex::new(b, RESULT_YELLOW).with_width(RESULT_PLAIN_WIDTH));
        }
        return Some((lo as f32, hi as f32));
    }
    let bands = femcraft_render::colorscale::BAND_COUNT;
    let step = ((hi - lo) / bands as f64).max(f64::MIN_POSITIVE);
    for (a, b, v) in segments {
        let band = (((v - lo) / step).floor() as isize).clamp(0, bands as isize - 1) as usize;
        let c = femcraft_render::colorscale::rfem_band(band);
        s.lines.push(LineVertex::new(a, c).with_width(RESULT_WIDTH));
        s.lines.push(LineVertex::new(b, c).with_width(RESULT_WIDTH));
    }
    Some((lo as f32, hi as f32))
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
    // Collect openings per surface.
    let mut openings_per_surface: std::collections::HashMap<u32, Vec<(u32, Vec<Vec3>)>> =
        std::collections::HashMap::new();
    for (id, opn) in model.iter_kind(K::Opening) {
        if let Some(poly) = surface_polygon(model, opn.get("boundary_lines").as_ref_list()) {
            for &srf_no in opn.get("surfaces").as_ref_list() {
                openings_per_surface
                    .entry(srf_no)
                    .or_default()
                    .push((id.no, poly.clone()));
            }
        }
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
            let holes: Vec<&[Vec3]> = openings_per_surface
                .get(no)
                .map(|v| v.iter().map(|(_, h)| h.as_slice()).collect())
                .unwrap_or_default();
            if holes.is_empty() {
                for [a, b, c] in triangulate(poly) {
                    s.tri(a, b, c, col);
                }
            } else {
                for [a, b, c] in triangulate_with_holes(poly, &holes) {
                    s.tri(a, b, c, col);
                }
            }
            s.pick.push((ObjectId::new(K::Surface, *no), poly.clone()));
        }
        // Draw opening boundary lines.
        for openings in openings_per_surface.values() {
            for (opn_no, poly) in openings {
                for w in poly.windows(2) {
                    s.line(w[0], w[1], OPENING_EDGE);
                }
                if poly.len() >= 3 {
                    s.line(*poly.last().unwrap(), poly[0], OPENING_EDGE);
                }
                s.pick
                    .push((ObjectId::new(K::Opening, *opn_no), poly.clone()));
            }
        }
    }

    if vis.lines {
        for (id, _) in model.iter_kind(K::Line) {
            let pts: Vec<Vec3> = model.line_points(id.no).into_iter().map(to_world).collect();
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
            let pts: Vec<[f64; 3]> = model.line_points(line);
            let (b, h) = section_size(model, m.get("section_start").as_ref_no());
            let beta = f(m, "rotation_angle").unwrap_or(0.0) as f32;
            if pts.len() < 2 {
                s.notes
                    .push(format!("Member {}: line {line} has no nodes", id.no));
            }
            for w in pts.windows(2) {
                if vis.member_profiles {
                    member_box(&mut s, w[0], w[1], b, h, beta);
                } else {
                    s.member_lines.push((to_world(w[0]), to_world(w[1])));
                }
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
                let pts: Vec<Vec3> = model.line_points(*l).into_iter().map(to_world).collect();
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

/// Ordered boundary points of a surface from its boundary lines (chained end to end by
/// their end nodes; curved lines contribute their tessellated points).
fn surface_polygon(model: &Model, lines: &[u32]) -> Option<Vec<Vec3>> {
    // (start node, end node, points) per line.
    let mut segs: Vec<(u32, u32, Vec<[f64; 3]>)> = lines
        .iter()
        .filter_map(|l| {
            let nodes = model.line_nodes(*l);
            let pts = model.line_points(*l);
            if pts.len() < 2 {
                return None;
            }
            Some((*nodes.first()?, *nodes.last()?, pts))
        })
        .collect();
    if segs.is_empty() {
        return None;
    }
    let (first, mut end, mut chain) = segs.remove(0);
    while !segs.is_empty() {
        let i = segs.iter().position(|s| s.0 == end || s.1 == end)?;
        let (a, b, mut pts) = segs.remove(i);
        if a != end {
            pts.reverse();
            end = a;
        } else {
            end = b;
        }
        chain.extend_from_slice(&pts[1..]);
    }
    if end == first && chain.len() > 1 {
        chain.pop();
    }
    let pts: Vec<Vec3> = chain.into_iter().map(to_world).collect();
    (pts.len() >= 3).then_some(pts)
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

/// Triangulate a polygon with holes using bridge-edge insertion: each hole is connected to the
/// outer boundary (or a previously merged hole) by a pair of coincident edges, producing a
/// single simple polygon that the ear-clipper handles. The bridge is chosen at the rightmost
/// vertex of the hole (projected onto the polygon plane) for simplicity.
fn triangulate_with_holes(outer: &[Vec3], holes: &[&[Vec3]]) -> Vec<[Vec3; 3]> {
    if holes.is_empty() {
        return triangulate(outer);
    }
    let n = newell_normal(outer);
    // Project onto a 2D plane for bridge computation.
    let (u_ax, v_ax) = {
        let t = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let u = t.cross(n).normalize_or_zero();
        (u, n.cross(u).normalize_or_zero())
    };
    let proj = |p: Vec3| -> (f32, f32) { (p.dot(u_ax), p.dot(v_ax)) };

    let mut merged: Vec<Vec3> = outer.to_vec();
    // Ensure the outer boundary is counter-clockwise in the projected plane relative to n.
    if signed_area_2d(&merged, &proj) < 0.0 {
        merged.reverse();
    }

    // Sort holes by their rightmost x so we always bridge to the already-merged boundary.
    let mut hole_order: Vec<usize> = (0..holes.len()).collect();
    hole_order.sort_by(|&a, &b| {
        let ma = holes[a].iter().map(|p| ordered(proj(*p).0)).max();
        let mb = holes[b].iter().map(|p| ordered(proj(*p).0)).max();
        mb.cmp(&ma) // descending: rightmost first
    });

    for &hi in &hole_order {
        let hole = holes[hi];
        if hole.is_empty() {
            continue;
        }
        // Ensure the hole is clockwise (opposite winding to the outer).
        let mut h: Vec<Vec3> = hole.to_vec();
        if signed_area_2d(&h, &proj) > 0.0 {
            h.reverse();
        }
        // Find the rightmost vertex of the hole.
        let (hr_idx, _) = h
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| ordered(proj(**a).0).cmp(&ordered(proj(**b).0)))
            .unwrap();
        let hr = proj(h[hr_idx]);
        // Find the nearest visible vertex on the merged boundary to bridge to.
        // Simple: pick the closest merged vertex that is to the right of or at hr.x.
        let bridge_idx = (0..merged.len())
            .filter(|&i| {
                let (mx, _) = proj(merged[i]);
                mx >= hr.0 - 1e-6
            })
            .min_by(|&a, &b| {
                let da = dist2d(hr, proj(merged[a]));
                let db = dist2d(hr, proj(merged[b]));
                ordered(da).cmp(&ordered(db))
            })
            .unwrap_or_else(|| {
                // Fallback: nearest vertex overall.
                (0..merged.len())
                    .min_by(|&a, &b| {
                        let da = dist2d(hr, proj(merged[a]));
                        let db = dist2d(hr, proj(merged[b]));
                        ordered(da).cmp(&ordered(db))
                    })
                    .unwrap_or(0)
            });

        // Insert the hole into merged at bridge_idx: outer[..=bridge] + hole rotated to hr + outer[bridge..]
        let rotated_hole: Vec<Vec3> = (0..h.len()).map(|i| h[(hr_idx + i) % h.len()]).collect();
        let mut new_merged = Vec::with_capacity(merged.len() + h.len() + 2);
        new_merged.extend_from_slice(&merged[..=bridge_idx]);
        new_merged.extend_from_slice(&rotated_hole);
        new_merged.push(rotated_hole[0]); // back to hole start
        new_merged.extend_from_slice(&merged[bridge_idx..]); // duplicate bridge vertex closes it
        merged = new_merged;
    }

    triangulate(&merged)
}

fn signed_area_2d(poly: &[Vec3], proj: &dyn Fn(Vec3) -> (f32, f32)) -> f32 {
    let mut area = 0.0f32;
    for i in 0..poly.len() {
        let (ax, ay) = proj(poly[i]);
        let (bx, by) = proj(poly[(i + 1) % poly.len()]);
        area += (bx - ax) * (by + ay);
    }
    area
}

fn dist2d(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn ordered(f: f32) -> OrderedF32 {
    OrderedF32(f)
}

#[derive(PartialEq)]
struct OrderedF32(f32);
impl Eq for OrderedF32 {}
impl PartialOrd for OrderedF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrderedF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
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
        // Projected global directions (Xp, Yp, Zp): drawn like the plain ones for now.
        (14, false, _) => Some(Vec3::X),
        (15, false, _) => Some(Vec3::Y),
        (16, false, _) => Some(Vec3::Z),
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
            let pts: Vec<[f64; 3]> = model.line_points(line);
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

    // Uniform surface load blocks are scaled slightly by magnitude: the largest load in the
    // case keeps the full height, the smallest ones are half as deep.
    let peak_uniform = in_case(K::SurfaceLoad)
        .filter_map(|(_, l)| f(l, "uniform_magnitude"))
        .filter(|v| v.is_finite())
        .fold(0.0f64, |a, v| a.max(v.abs()));
    for (_, l) in in_case(K::SurfaceLoad) {
        let world =
            |no: u32| node(no).map(|p| dir_world(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)));
        // Varying between two nodes (e.g. soil / water pressure on a wall): p1 at node 1
        // linearly to p2 at node 2, constant across the direction normal to that line.
        let varying = (|| {
            let n1 = world(l.get("varying_node_1").as_i64()? as u32)?;
            let n2 = world(l.get("varying_node_2").as_i64()? as u32)?;
            let (m1, m2) = (f(l, "varying_magnitude_1")?, f(l, "varying_magnitude_2")?);
            ((m1 != 0.0 || m2 != 0.0) && (n2 - n1).length() > 1e-6).then_some((n1, m1, n2, m2))
        })();
        if let Some((n1, m1, n2, m2)) = varying {
            let code = l.get("load_direction").as_i64().unwrap_or(13);
            let axis = n2 - n1;
            let (u, span) = (axis.normalize(), axis.length());
            let peak = m1.abs().max(m2.abs()) as f32;
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
                let dw = dir_world(d);
                let mag_at = |p: Vec3| m1 as f32 + (m2 - m1) as f32 * (p - n1).dot(u) / span;
                let offsets: Vec<Vec3> = poly
                    .iter()
                    .map(|p| -dw * (mag_at(*p) / peak * len))
                    .collect();
                prism_varying(s, poly, &offsets, SURFACE_LOAD_FILL, SURFACE_LOAD);
                let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
                let mc = mag_at(c);
                if mc.abs() > 1e-6 * peak {
                    let toward = dw * mc.signum();
                    s.arrow(
                        c,
                        toward,
                        len * 0.9 * (mc.abs() / peak).max(0.25),
                        SURFACE_LOAD,
                    );
                }
                s.labels.push(Label {
                    pos: c - dw * len * 1.25 * mc.signum(),
                    text: format!(
                        "{} - {}",
                        kn(m1.abs(), "kN/m²").trim_end_matches(" kN/m²"),
                        kn(m2.abs(), "kN/m²")
                    ),
                    color: SURFACE_LOAD,
                });
            }
            continue;
        }
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
            let h = len * (0.5 + 0.5 * (mag.abs() / peak_uniform) as f32).clamp(0.5, 1.0);
            surface_load_block(s, poly, dw, h);
            s.labels.push(Label {
                pos: c - dw * h * 1.25,
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

/// Like [`prism`], with a separate offset (world) per polygon vertex: a wedge for a load
/// varying across the surface. Vertices with a zero offset stay on the surface.
fn prism_varying(s: &mut Scene, poly: &[Vec3], offsets: &[Vec3], fill: [f32; 4], edge: [f32; 4]) {
    let top: Vec<Vec3> = poly.iter().zip(offsets).map(|(p, o)| *p + *o).collect();
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
    fn contact_stress_is_spring_times_normal_displacement() {
        use femcraft_rfdb::{Displacements, FeMesh};
        let mesh = FeMesh {
            nodes: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            elements: vec![[1, 2, 3, 4]],
            element_surface: vec![1],
            lines: vec![],
        };
        let disp = Displacements {
            nodal: vec![
                [0.0, 0.0, 0.010, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.020, 0.0, 0.0, 0.0],
                [0.0, 0.0, -0.005, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.010, 0.0, 0.0, 0.0],
            ],
        };
        let mut sup = HashMap::new();
        sup.insert(
            1,
            ContactSupport {
                cz: 6.0e6,
                tension_free: true,
            },
        );
        let mut s = Scene::default();
        let (lo, hi) =
            add_contact_surface(&mut s, &mesh, &[&disp], &sup, femcraft_rfdb::Extremum::Max)
                .unwrap();
        // Uplift (-5 mm) is cut off to zero; the peak is 6e6 * 0.02 = 120 kN/m².
        assert!(lo.abs() < 1e-3 && (hi - 1.2e5).abs() < 1.0, "{lo}..{hi}");
        assert!(!s.result_triangles.is_empty());
        // A surface without a spring support paints nothing.
        let mut s2 = Scene::default();
        assert!(
            add_contact_surface(
                &mut s2,
                &mesh,
                &[&disp],
                &HashMap::new(),
                femcraft_rfdb::Extremum::Max
            )
            .is_none()
        );
    }

    /// Member results plot as one connected polyline per member line (no segment returns to the
    /// start). Needs a solved members-only model in `FEMCRAFT_TEST_MEMBER_RESULTS`.
    #[test]
    fn member_results_form_a_polyline() {
        use femcraft_rfdb::{CaseKind, Extremum, ResultComponent, RfDocument};
        let Some(path) = std::env::var_os("FEMCRAFT_TEST_MEMBER_RESULTS") else {
            return;
        };
        let doc = RfDocument::open(std::path::Path::new(&path)).unwrap();
        let mesh = doc.fe_mesh().unwrap().unwrap();
        let id = doc
            .results()
            .unwrap()
            .unwrap()
            .cases
            .iter()
            .find(|c| c.kind == CaseKind::LoadCase && c.no == 1)
            .unwrap()
            .solver_id;
        let disp = doc.displacements(id, mesh.nodes.len()).unwrap().unwrap();
        let members: HashSet<u32> = mesh.lines.iter().map(|l| l.0).collect();
        let mut s = Scene::default();
        add_result_members(
            &mut s,
            &mesh,
            &[&disp],
            &members,
            ResultComponent::GlobalUz,
            1000.0,
            Extremum::Both,
            true,
        )
        .unwrap();
        assert!(s.lines.len() >= 100 && s.lines.len() % 2 == 0);
        // Each segment starts where the previous one ended, except at the start of a new member.
        let mut breaks = 0;
        for w in s.lines.chunks(2).collect::<Vec<_>>().windows(2) {
            let (end, next) = (Vec3::from(w[0][1].pos), Vec3::from(w[1][0].pos));
            if end.distance(next) > 1e-4 {
                breaks += 1;
            }
            // Segments are short (one FE element), never a long chord.
            let len = Vec3::from(w[1][0].pos).distance(Vec3::from(w[1][1].pos));
            assert!(len < 1.0, "segment of length {len}");
        }
        assert!(breaks <= members.len(), "{breaks} breaks");
    }

    #[test]
    fn result_surface_colours_and_displaces() {
        use femcraft_rfdb::{Displacements, Extremum, FeMesh, ResultComponent};
        // One quad: 4 FE nodes, element [1,2,3,4].
        let mesh = FeMesh {
            nodes: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            elements: vec![[1, 2, 3, 4]],
            element_surface: vec![1],
            lines: vec![(1, vec![1, 2])],
        };
        let disp = Displacements {
            nodal: vec![
                [0.0, 0.0, 0.000, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.010, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.020, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.010, 0.0, 0.0, 0.0],
            ],
        };
        let mut s = Scene::default();
        let range = add_result_surface(
            &mut s,
            &mesh,
            &[&disp],
            ResultComponent::GlobalUz,
            100.0,
            Extremum::Both,
            true,
        );
        // Discrete bands split the quad into many solid-colour triangles spanning the u_z values.
        let tris = &s.result_triangles;
        assert!(tris.len() >= 6 && tris.len() % 3 == 0);
        let (lo, hi) = range.expect("displacement range");
        assert!(
            (lo - 0.0).abs() < 1e-6 && (hi - 0.02).abs() < 1e-6,
            "{lo}..{hi}"
        );
        // Node 3 (max u_z) is displaced far along world Z (model z is negated, magnified ×100).
        assert!(tris.iter().any(|v| v.pos[2].abs() > 1.0));
        // The largest value is red, the smallest blue (RFEM's scale).
        assert!(
            tris.iter().any(|v| v.color[0] > 0.4 && v.color[2] < 0.2),
            "max value painted red"
        );
        assert!(
            tris.iter().any(|v| v.color[2] > 0.4 && v.color[0] < 0.2),
            "min value painted blue"
        );
        // A non-displacement component paints nothing.
        let mut s2 = Scene::default();
        assert!(
            add_result_surface(
                &mut s2,
                &mesh,
                &[&disp],
                ResultComponent::SurfaceMx,
                1.0,
                Extremum::Both,
                true
            )
            .is_none()
        );

        // Member deformation: line 1 carries a member -> one orange segment.
        let mut s3 = Scene::default();
        let members: HashSet<u32> = [1].into_iter().collect();
        add_member_deformation(&mut s3, &mesh, &disp, &members, 1.0);
        assert_eq!(s3.lines.len(), 2, "one segment = two line vertices");
    }

    #[test]
    fn plain_results_are_thin_yellow_lines_and_follow_the_component() {
        use femcraft_rfdb::{Displacements, Extremum, FeMesh, ResultComponent};
        let mesh = FeMesh {
            nodes: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            elements: vec![[1, 2, 3, 4]],
            element_surface: vec![1],
            lines: vec![(1, vec![1, 2])],
        };
        let disp = Displacements {
            nodal: vec![
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.5, 0.0, 0.010, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.020, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.010, 0.0, 0.0, 0.0],
            ],
        };
        // Surface, colours off: the displaced mesh net in yellow, no contour triangles.
        let mut s = Scene::default();
        add_result_surface(
            &mut s,
            &mesh,
            &[&disp],
            ResultComponent::GlobalUz,
            10.0,
            Extremum::Both,
            false,
        )
        .unwrap();
        assert!(s.result_triangles.is_empty());
        assert_eq!(s.lines.len(), 8, "four edges");
        assert!(
            s.lines
                .iter()
                .all(|v| v.color == RESULT_YELLOW && v.width == RESULT_NET_WIDTH)
        );
        // u_z only moves the shape along z: node 2 keeps its x (world x = model x).
        assert!(s.lines.iter().any(|v| (v.pos[0] - 1.0).abs() < 1e-6));
        // u_x only moves it along x: node 2 is now at x = 1.5.
        let mut sx = Scene::default();
        add_result_surface(
            &mut sx,
            &mesh,
            &[&disp],
            ResultComponent::GlobalUx,
            1.0,
            Extremum::Both,
            false,
        )
        .unwrap();
        assert!(sx.lines.iter().any(|v| (v.pos[0] - 1.5).abs() < 1e-6));
        assert!(sx.lines.iter().all(|v| v.pos[2].abs() < 1e-6));
        // A rotation plots the full translation shape.
        let mut sp = Scene::default();
        add_result_surface(
            &mut sp,
            &mesh,
            &[&disp],
            ResultComponent::GlobalPhiY,
            10.0,
            Extremum::Both,
            false,
        )
        .unwrap();
        assert!(sp.lines.iter().any(|v| v.pos[2].abs() > 0.05));
        // Members, colours off: a plain yellow line plot.
        let members: HashSet<u32> = [1].into_iter().collect();
        let mut sm = Scene::default();
        add_result_members(
            &mut sm,
            &mesh,
            &[&disp],
            &members,
            ResultComponent::GlobalUz,
            10.0,
            Extremum::Both,
            false,
        )
        .unwrap();
        assert_eq!(sm.lines.len(), 2);
        assert!(
            sm.lines
                .iter()
                .all(|v| v.color == RESULT_YELLOW && v.width == RESULT_PLAIN_WIDTH)
        );
    }

    #[test]
    fn envelope_members_draw_the_max_and_min_shapes() {
        use femcraft_rfdb::{Displacements, Extremum, FeMesh, ResultComponent};
        let mesh = FeMesh {
            nodes: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            elements: vec![],
            element_surface: vec![],
            lines: vec![(1, vec![1, 2])],
        };
        let up = Displacements {
            nodal: vec![[0.0; 6], [0.0, 0.0, 0.010, 0.0, 0.0, 0.0]],
        };
        let down = Displacements {
            nodal: vec![[0.0; 6], [0.0, 0.0, -0.004, 0.0, 0.0, 0.0]],
        };
        let members: HashSet<u32> = [1].into_iter().collect();
        let tip_z = |ext| {
            let mut s = Scene::default();
            add_result_members(
                &mut s,
                &mesh,
                &[&up, &down],
                &members,
                ResultComponent::GlobalUz,
                10.0,
                ext,
                true,
            )
            .unwrap();
            // The tip of each drawn segment (second vertex); world z = -model z.
            s.lines.chunks(2).map(|c| c[1].pos[2]).collect::<Vec<_>>()
        };
        let both = tip_z(Extremum::Both);
        assert_eq!(both.len(), 2, "two shapes in one view");
        assert!(
            both.iter().any(|z| (z + 0.10).abs() < 1e-5),
            "max shape: {both:?}"
        );
        assert!(
            both.iter().any(|z| (z - 0.04).abs() < 1e-5),
            "min shape: {both:?}"
        );
        assert_eq!(tip_z(Extremum::Max).len(), 1);
        assert!((tip_z(Extremum::Max)[0] + 0.10).abs() < 1e-5);
        assert!((tip_z(Extremum::Min)[0] - 0.04).abs() < 1e-5);
    }

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
    fn members_with_and_without_profile() {
        let mut m = Model::new();
        for (no, x) in [(1, 0.0), (2, 6.0)] {
            let id = ObjectId::new(K::Node, no);
            m.add(id).unwrap();
            m.set(id, "coordinate_1", Value::Float(x)).unwrap();
            m.set(id, "coordinate_2", Value::Float(0.0)).unwrap();
            m.set(id, "coordinate_3", Value::Float(0.0)).unwrap();
        }
        let line = ObjectId::new(K::Line, 1);
        m.add(line).unwrap();
        m.set(line, "definition_nodes", Value::RefList(vec![1, 2]))
            .unwrap();
        let member = ObjectId::new(K::Member, 1);
        m.add(member).unwrap();
        m.set(member, "line", Value::Ref(1)).unwrap();

        let vis = Visibility {
            lines: false,
            ..Visibility::default()
        };
        let profile = build(&m, &vis);
        assert!(!profile.lines.is_empty() && profile.member_lines.is_empty());

        let plain = build(
            &m,
            &Visibility {
                member_profiles: false,
                ..vis.clone()
            },
        );
        assert_eq!(plain.member_lines.len(), 1);
        assert!(plain.lines.is_empty() && plain.triangles.is_empty());

        let hidden = build(
            &m,
            &Visibility {
                members: false,
                ..vis
            },
        );
        assert!(hidden.member_lines.is_empty() && hidden.lines.is_empty());
    }

    /// Soil / water pressure on the Chamber model's walls (linear between two nodes) is drawn.
    #[test]
    fn varying_wall_loads_are_drawn() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/private/Chamber 4x4x6.rf6");
        if !path.is_file() {
            return;
        }
        let doc = femcraft_rfdb::RfDocument::open(&path).unwrap();
        let vis = Visibility::default();
        let mut with = vis.clone();
        with.load_case = 5;
        let none = {
            let mut v = vis.clone();
            v.load_case = 1;
            build(&doc.model, &v)
        };
        for lc in [6, 8] {
            let mut v = vis.clone();
            v.load_case = lc;
            let sc = build(&doc.model, &v);
            assert!(sc.notes.is_empty(), "LC{lc}: {:?}", sc.notes);
        }
        let sc = build(&doc.model, &with);
        assert!(sc.triangles.len() > none.triangles.len(), "wedges drawn");
        assert!(sc.notes.is_empty(), "{:?}", sc.notes);
        assert!(
            sc.labels.iter().any(|l| l.text == "0 - 31 kN/m²"),
            "{:?}",
            sc.labels.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn kn_format() {
        assert_eq!(kn(10_000.0, "kN"), "10 kN");
        assert_eq!(kn(2_500.0, "kN/m"), "2.5 kN/m");
    }
}
