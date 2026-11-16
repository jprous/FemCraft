//! Orbit camera for the 3D workspace. Global Z points up.

use glam::{Mat4, Vec3, Vec4};

const MAX_PITCH: f32 = 89.9 * std::f32::consts::PI / 180.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// Standard views, named the way structural/CAD tools usually do.
#[derive(Clone, Copy, Debug)]
pub enum StandardView {
    Iso,
    Top,
    Front,
    Right,
}

pub struct OrbitCamera {
    /// Point the camera orbits around.
    pub target: Vec3,
    /// Rotation around global Z, radians.
    pub yaw: f32,
    /// Elevation above the XY plane, radians.
    pub pitch: f32,
    /// Distance from eye to target, metres.
    pub distance: f32,
    /// Vertical field of view, radians (also sets the ortho zoom).
    pub fov_y: f32,
    pub projection: Projection,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let mut cam = Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 30.0,
            fov_y: 45f32.to_radians(),
            projection: Projection::Perspective,
        };
        cam.set_view(StandardView::Iso);
        cam
    }
}

impl OrbitCamera {
    pub fn set_view(&mut self, view: StandardView) {
        let (yaw, pitch) = match view {
            // Looking from (-X, -Y, +Z): X runs to the right, Y to the left, both away.
            StandardView::Iso => (-135f32, 35.264f32),
            StandardView::Top => (-90.0, 90.0),
            StandardView::Front => (-90.0, 0.0),
            StandardView::Right => (0.0, 0.0),
        };
        self.yaw = yaw.to_radians();
        self.pitch = pitch.to_radians().clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Unit vector from target towards the eye.
    fn eye_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * cy, cp * sy, sp)
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.eye_dir() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Z)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let far = self.distance * 200.0 + 1000.0;
        match self.projection {
            Projection::Perspective => {
                let near = (self.distance * 0.001).max(0.001);
                Mat4::perspective_rh(self.fov_y, aspect, near, far)
            }
            Projection::Orthographic => {
                let h = self.half_height();
                Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, -far, far)
            }
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    /// Half the visible height at the target distance, metres.
    fn half_height(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// Rotate by a mouse drag in pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Move the target so the model follows the mouse.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let forward = -self.eye_dir();
        let right = forward.cross(Vec3::Z).normalize_or_zero();
        let up = right.cross(forward);
        let metres_per_pixel = 2.0 * self.half_height() / viewport_height.max(1.0);
        self.target += (-dx * right + dy * up) * metres_per_pixel;
    }

    /// Zoom by a scroll amount (positive = closer).
    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.0015).exp()).clamp(0.05, 1.0e5);
    }

    /// Frame a region of the given radius around `center`.
    pub fn fit(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        self.distance = radius.max(0.1) / (self.fov_y * 0.5).tan() * 1.1;
    }

    /// World-space ray through a point given in normalised device coordinates.
    pub fn ray(&self, aspect: f32, ndc_x: f32, ndc_y: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        (near, (far - near).normalize_or_zero())
    }
}

/// Project a world point to normalised device coordinates. None if behind the camera.
pub fn to_ndc(view_proj: Mat4, p: Vec3) -> Option<Vec3> {
    let clip: Vec4 = view_proj * p.extend(1.0);
    (clip.w > 1e-6).then(|| clip.truncate() / clip.w)
}
