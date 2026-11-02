//! Construction grid: metric spacing, placed on the XY, XZ or YZ plane.

use glam::Vec3;

use crate::viewport::LineVertex;

/// Upper limit on grid lines either side of the origin, to keep the GPU buffer sane.
pub const MAX_HALF_LINES: i32 = 1000;

const MINOR_COLOR: [f32; 4] = [0.42, 0.46, 0.52, 0.35];
const MAJOR_COLOR: [f32; 4] = [0.58, 0.62, 0.68, 0.75];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridPlane {
    XY,
    XZ,
    YZ,
}

impl GridPlane {
    pub const ALL: [GridPlane; 3] = [GridPlane::XY, GridPlane::XZ, GridPlane::YZ];

    pub fn label(self) -> &'static str {
        match self {
            GridPlane::XY => "XY (plan)",
            GridPlane::XZ => "XZ (elevation)",
            GridPlane::YZ => "YZ (elevation)",
        }
    }

    /// The two in-plane directions (u, v) and the plane normal.
    pub fn axes(self) -> (Vec3, Vec3, Vec3) {
        match self {
            GridPlane::XY => (Vec3::X, Vec3::Y, Vec3::Z),
            GridPlane::XZ => (Vec3::X, Vec3::Z, Vec3::Y),
            GridPlane::YZ => (Vec3::Y, Vec3::Z, Vec3::X),
        }
    }

    pub fn normal_name(self) -> &'static str {
        match self {
            GridPlane::XY => "Z",
            GridPlane::XZ => "Y",
            GridPlane::YZ => "X",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct GridSettings {
    pub visible: bool,
    pub plane: GridPlane,
    /// Distance between grid lines, metres.
    pub spacing: f32,
    /// Grid runs from -extent to +extent in both directions, metres.
    pub extent: f32,
    /// Every n-th line is drawn brighter.
    pub major_every: u32,
    /// Shift of the grid along the plane normal, metres (e.g. a storey level).
    pub offset: f32,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            plane: GridPlane::XY,
            spacing: 1.0,
            extent: 20.0,
            major_every: 5,
            offset: 0.0,
        }
    }
}

impl GridSettings {
    pub fn origin(&self) -> Vec3 {
        self.plane.axes().2 * self.offset
    }

    fn half_lines_unclamped(&self) -> i32 {
        (self.extent / self.spacing.max(1e-3)).floor().max(1.0) as i32
    }

    pub fn is_clamped(&self) -> bool {
        self.half_lines_unclamped() > MAX_HALF_LINES
    }

    pub fn half_lines(&self) -> i32 {
        self.half_lines_unclamped().min(MAX_HALF_LINES)
    }

    /// Size actually drawn either side of the origin, metres.
    pub fn half_size(&self) -> f32 {
        self.half_lines() as f32 * self.spacing
    }

    pub fn build_lines(&self, out: &mut Vec<LineVertex>) {
        let (u, v, _) = self.plane.axes();
        let origin = self.origin();
        let half = self.half_lines();
        let size = self.half_size();
        let major = self.major_every.max(1) as i32;

        out.reserve((half as usize * 2 + 1) * 4);
        for i in -half..=half {
            let t = i as f32 * self.spacing;
            let color = if i % major == 0 { MAJOR_COLOR } else { MINOR_COLOR };
            let mut line = |a: Vec3, b: Vec3| {
                out.push(LineVertex::new(a, color));
                out.push(LineVertex::new(b, color));
            };
            line(origin + u * t - v * size, origin + u * t + v * size);
            line(origin + v * t - u * size, origin + v * t + u * size);
        }
    }

    /// Where a ray hits the grid plane, if it does in front of the camera.
    pub fn intersect(&self, ray_origin: Vec3, ray_dir: Vec3) -> Option<Vec3> {
        let n = self.plane.axes().2;
        let denom = ray_dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.offset - ray_origin.dot(n)) / denom;
        (t > 0.0).then(|| ray_origin + ray_dir * t)
    }

    /// Round a point on the plane to the nearest grid intersection.
    pub fn snap(&self, p: Vec3) -> Vec3 {
        let (u, v, _) = self.plane.axes();
        let s = self.spacing.max(1e-3);
        let su = (p.dot(u) / s).round() * s;
        let sv = (p.dot(v) / s).round() * s;
        self.origin() + u * su + v * sv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_count_matches_spacing() {
        let g = GridSettings { spacing: 0.5, extent: 2.0, ..Default::default() };
        let mut v = Vec::new();
        g.build_lines(&mut v);
        // 9 positions (-2..=2 step 0.5), 2 lines each, 2 vertices per line
        assert_eq!(v.len(), 9 * 2 * 2);
    }

    #[test]
    fn snap_and_intersect_on_xz_plane() {
        let g = GridSettings { plane: GridPlane::XZ, offset: 3.0, ..Default::default() };
        let hit = g.intersect(Vec3::new(1.2, -10.0, 2.7), Vec3::Y).unwrap();
        assert_eq!(hit, Vec3::new(1.2, 3.0, 2.7));
        assert_eq!(g.snap(hit), Vec3::new(1.0, 3.0, 3.0));
    }
}
