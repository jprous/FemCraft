//! One open model: its data, display settings, camera, selection and 3D view.
//!
//! Each document draws into its own tab; the renderer keeps separate GPU buffers per document
//! (keyed by its id), so several model tabs can be visible side by side.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Stroke};
use eframe::egui_wgpu;
use glam::Vec3;

use femcraft_model::{Model, ObjectId, ObjectKind};
use femcraft_render::camera::{self, OrbitCamera};
use femcraft_render::grid::GridSettings;
use femcraft_render::viewport::{LineVertex, LinesCallback};
use femcraft_rfdb::RfDocument;

use crate::scene::{self, Scene, SupportSymbol, Visibility};

pub const AXIS_X: [f32; 4] = [0.92, 0.30, 0.30, 1.0];
pub const AXIS_Y: [f32; 4] = [0.40, 0.82, 0.35, 1.0];
pub const AXIS_Z: [f32; 4] = [0.35, 0.58, 0.98, 1.0];

const BG_TOP: Color32 = Color32::from_rgb(0x2e, 0x34, 0x3d);
const BG_BOTTOM: Color32 = Color32::from_rgb(0x16, 0x19, 0x1e);

pub struct Document {
    /// Unique per session; names the tab and the GPU buffers.
    pub id: u64,
    pub path: PathBuf,
    /// The file as loaded (model plus how it maps to the database), kept for saving.
    pub rf: RfDocument,
    /// Bumped on every edit, so the 3D view is rebuilt.
    pub revision: u64,
    pub load_cases: Vec<(u32, String)>,
    /// Problems reported while loading.
    pub warnings: Vec<String>,
    pub vis: Visibility,
    pub scene: Scene,
    pub camera: OrbitCamera,
    pub selection: HashSet<ObjectId>,
    /// Snapped grid point under the mouse (world coordinates).
    pub cursor: Option<Vec3>,

    lines: Arc<Vec<LineVertex>>,
    triangles: Arc<Vec<LineVertex>>,
    version: u64,
    built_from: Option<(GridSettings, bool, Visibility, u64)>,
    /// Box drag in progress: button and start position.
    drag_box: Option<(egui::PointerButton, Pos2)>,
}

impl Document {
    /// Reads an `.rf6` into a new document.
    pub fn open(id: u64, path: &Path) -> Result<Self, String> {
        let doc = RfDocument::open(path).map_err(|e| e.to_string())?;
        let load_cases = scene::load_cases(&doc.model);
        let vis = Visibility {
            load_case: scene::first_loaded_case(&doc.model)
                .or(load_cases.first().map(|(n, _)| *n))
                .unwrap_or(1),
            ..Visibility::default()
        };
        Ok(Self {
            id,
            path: path.to_owned(),
            load_cases,
            warnings: doc.report.warnings.clone(),
            rf: doc,
            revision: 0,
            vis,
            scene: Scene::default(),
            camera: OrbitCamera::default(),
            selection: HashSet::new(),
            cursor: None,
            lines: Arc::new(Vec::new()),
            triangles: Arc::new(Vec::new()),
            version: 0,
            built_from: None,
            drag_box: None,
        })
    }

    /// A new, unsaved model from the blank model `template`, titled `name` (no extension, so
    /// File > Save asks where to save it).
    pub fn new_untitled(id: u64, template: &Path, name: &str) -> Result<Self, String> {
        let mut doc = Self::open(id, template)?;
        doc.path = PathBuf::from(name);
        Ok(doc)
    }

    pub fn model(&self) -> &Model {
        &self.rf.model
    }

    /// Unsaved edits?
    pub fn is_modified(&self) -> bool {
        self.rf.model.is_modified()
    }

    /// Writes the model (with its edits) to `path` as `.rf6`; the document then refers to it.
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        self.rf.save(path).map_err(|e| e.to_string())?;
        self.path = path.to_owned();
        Ok(())
    }

    /// Releases the selection and cancels a box selection in progress (Escape).
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.drag_box = None;
    }

    /// File name, for tab titles and menus.
    pub fn title(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// Rebuilds the drawable geometry when the grid, axes or display settings changed.
    pub fn refresh(&mut self, grid: &GridSettings, show_axes: bool) {
        // Support glyphs along lines / surface edges follow the grid spacing.
        self.vis.glyph_spacing = grid.spacing;
        let key = (grid.clone(), show_axes, self.vis.clone(), self.revision);
        if self.built_from.as_ref() == Some(&key) {
            return;
        }
        self.scene = scene::build(&self.rf.model, &self.vis);

        let mut v = Vec::new();
        if grid.visible {
            grid.build_lines(&mut v);
        }
        if show_axes {
            let len = self.axis_length(grid);
            for ((dir, _), color) in scene::GLOBAL_AXES.iter().zip([AXIS_X, AXIS_Y, AXIS_Z]) {
                v.push(LineVertex::new(Vec3::ZERO, color));
                v.push(LineVertex::new(*dir * len, color));
            }
        }
        v.extend_from_slice(&self.scene.lines);
        self.lines = Arc::new(v);
        self.triangles = Arc::new(self.scene.triangles.clone());
        self.version += 1;
        self.built_from = Some(key);
    }

    fn axis_length(&self, grid: &GridSettings) -> f32 {
        match self.scene.extent() {
            Some((_, r)) => r * 0.4,
            None => grid.half_size().max(grid.spacing * 5.0),
        }
    }

    pub fn zoom_to_model(&mut self, grid: &GridSettings) {
        match self.scene.extent() {
            Some((c, r)) => self.camera.fit(c, r * 1.3),
            None => self.zoom_to_grid(grid),
        }
    }

    pub fn zoom_to_grid(&mut self, grid: &GridSettings) {
        self.camera.fit(grid.origin(), grid.half_size());
    }

    /// Draws the model view and handles its mouse input. Returns true when the user
    /// interacted with it (so it becomes the active model).
    pub fn viewport_ui(&mut self, ui: &mut egui::Ui, grid: &GridSettings, show_axes: bool) -> bool {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());

        // Navigation, as in RFEM: middle-drag pans, Ctrl + middle-drag rotates, double-click
        // middle zooms to extents, wheel zooms; left-drag selects, right-drag zooms to a window.
        use egui::PointerButton::{Middle, Primary, Secondary};
        let delta = response.drag_delta();
        let (ctrl, pointer_pos) = ui.input(|i| (i.modifiers.ctrl, i.pointer.latest_pos()));
        if response.dragged_by(Middle) {
            if ctrl {
                self.camera.orbit(delta.x, delta.y);
            } else {
                self.camera.pan(delta.x, delta.y, rect.height());
            }
        }
        if response.double_clicked_by(Middle) {
            self.zoom_to_model(grid);
        }
        let mut scrolled = false;
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
                scrolled = true;
            }
        }
        for button in [Primary, Secondary] {
            if response.drag_started_by(button)
                && let Some(p) = response.interact_pointer_pos()
            {
                self.drag_box = Some((button, p));
            }
        }
        // A finished box: (button, rectangle, dragged left to right).
        let mut finished: Option<(egui::PointerButton, egui::Rect, bool)> = None;
        if let (Some((button, start)), Some(end)) = (self.drag_box, pointer_pos)
            && response.drag_stopped_by(button)
        {
            finished = Some((
                button,
                egui::Rect::from_two_pos(start, end),
                end.x >= start.x,
            ));
            self.drag_box = None;
        }
        if response.clicked_by(Primary) {
            self.selection.clear();
        }
        let aspect = rect.width() / rect.height().max(1.0);
        if let Some((Secondary, r, _)) = finished
            && r.width() > 4.0
            && r.height() > 4.0
        {
            let c = r.center();
            let ndc_x = (c.x - rect.left()) / rect.width() * 2.0 - 1.0;
            let ndc_y = 1.0 - (c.y - rect.top()) / rect.height() * 2.0;
            let fraction = (r.width() / rect.width()).max(r.height() / rect.height());
            self.camera.zoom_to_window(aspect, ndc_x, ndc_y, fraction);
        }

        let painter = ui.painter_at(rect);
        paint_background(&painter, rect);

        let view_proj = self.camera.view_proj(aspect);
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            LinesCallback {
                view: self.id,
                view_proj,
                vertices: self.lines.clone(),
                triangles: self.triangles.clone(),
                version: self.version,
            },
        ));

        let to_screen = |p: Vec3| {
            camera::to_ndc(view_proj, p).map(|n| {
                Pos2::new(
                    rect.left() + (n.x + 1.0) * 0.5 * rect.width(),
                    rect.top() + (1.0 - n.y) * 0.5 * rect.height(),
                )
            })
        };

        // Snapped cursor on the grid plane
        self.cursor = response.hover_pos().and_then(|pos| {
            let ndc_x = (pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
            let ndc_y = 1.0 - (pos.y - rect.top()) / rect.height() * 2.0;
            let (origin, dir) = self.camera.ray(aspect, ndc_x, ndc_y);
            grid.intersect(origin, dir).map(|hit| grid.snap(hit))
        });
        if let Some(s) = self.cursor.and_then(to_screen) {
            let stroke = Stroke::new(1.5_f32, Color32::from_rgb(255, 200, 60));
            painter.line_segment([s - egui::vec2(6.0, 0.0), s + egui::vec2(6.0, 0.0)], stroke);
            painter.line_segment([s - egui::vec2(0.0, 6.0), s + egui::vec2(0.0, 6.0)], stroke);
        }

        self.paint_model_marks(&painter, &to_screen);

        // Box selection: left to right = window (fully inside), right to left = crossing.
        if let Some((Primary, r, window)) = finished
            && r.width() > 3.0
            && r.height() > 3.0
        {
            if !ctrl {
                self.selection.clear();
            }
            for (id, pts) in &self.scene.pick {
                let screen: Option<Vec<Pos2>> = pts.iter().map(|p| to_screen(*p)).collect();
                let Some(screen) = screen else { continue };
                let hit = if window {
                    screen.iter().all(|p| r.contains(*p))
                } else if screen.len() == 1 {
                    r.contains(screen[0])
                } else {
                    let closed = id.kind == ObjectKind::Surface;
                    let n = screen.len();
                    let segs = if closed { n } else { n - 1 };
                    (0..segs).any(|i| segment_hits_rect(screen[i], screen[(i + 1) % n], r))
                };
                if hit {
                    self.selection.insert(*id);
                }
            }
        }

        // Selection highlight.
        let hl = Stroke::new(3.0_f32, Color32::from_rgb(255, 225, 40));
        for (id, pts) in &self.scene.pick {
            if !self.selection.contains(id) {
                continue;
            }
            let screen: Vec<Pos2> = pts.iter().filter_map(|p| to_screen(*p)).collect();
            match (id.kind, screen.as_slice()) {
                (_, [p]) => {
                    painter.circle_stroke(*p, 5.0, hl);
                }
                (ObjectKind::Surface, s) if s.len() > 2 => {
                    painter.add(egui::Shape::closed_line(
                        s.to_vec(),
                        Stroke::new(2.0_f32, hl.color),
                    ));
                }
                (_, s) if s.len() > 1 => {
                    painter.add(egui::Shape::line(s.to_vec(), hl));
                }
                _ => {}
            }
        }

        // Rectangle being dragged.
        if let (Some((button, start)), Some(now)) = (self.drag_box, pointer_pos) {
            let r = egui::Rect::from_two_pos(start, now);
            let (fill, stroke) = match button {
                Primary if now.x >= start.x => (
                    Color32::from_rgba_unmultiplied(80, 140, 255, 40),
                    Stroke::new(1.0_f32, Color32::from_rgb(110, 160, 255)),
                ),
                Primary => (
                    Color32::from_rgba_unmultiplied(80, 220, 120, 40),
                    Stroke::new(1.0_f32, Color32::from_rgb(110, 230, 140)),
                ),
                _ => (Color32::TRANSPARENT, Stroke::new(1.0_f32, Color32::WHITE)),
            };
            painter.rect_filled(r, 0.0, fill);
            painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
        }

        // Axis labels at the tips of the global axes (RFEM orientation: Z down)
        if show_axes {
            let len = self.axis_length(grid);
            for ((dir, name), color) in scene::GLOBAL_AXES.iter().zip([AXIS_X, AXIS_Y, AXIS_Z]) {
                if let Some(p) = to_screen(*dir * len * 1.03) {
                    painter.text(
                        p,
                        egui::Align2::CENTER_CENTER,
                        name,
                        egui::FontId::proportional(14.0),
                        rgba(color),
                    );
                }
            }
        }

        paint_axis_triad(&painter, rect, &self.camera);

        response.clicked() || response.dragged() || response.double_clicked() || scrolled
    }

    /// Nodes, support symbols and load labels, drawn in screen space over the GPU geometry.
    fn paint_model_marks(&self, painter: &egui::Painter, to_screen: &dyn Fn(Vec3) -> Option<Pos2>) {
        // Members without profile: 2 px, one more than a line element.
        let member = Stroke::new(2.0_f32, rgba(scene::MEMBER));
        for (a, b) in &self.scene.member_lines {
            if let (Some(a), Some(b)) = (to_screen(*a), to_screen(*b)) {
                painter.line_segment([a, b], member);
            }
        }
        let node = rgba(scene::NODE);
        for p in self.scene.nodes.iter().filter_map(|p| to_screen(*p)) {
            painter.circle_filled(p, 2.5, node);
        }
        for g in &self.scene.supports {
            let Some(p) = to_screen(g.pos) else { continue };
            // Baby blue when a gap / nonlinearity is defined.
            let green = rgba(if g.nonlinear {
                scene::NONLINEAR_SUPPORT
            } else {
                scene::SUPPORT
            });
            let stroke = Stroke::new(1.5_f32, green);
            let v = |x: f32, y: f32| p + egui::vec2(x, y);
            match g.symbol {
                SupportSymbol::Pinned => {
                    painter.add(egui::Shape::closed_line(
                        vec![p, v(-7.0, 12.0), v(7.0, 12.0)],
                        stroke,
                    ));
                }
                SupportSymbol::Roller => {
                    painter.add(egui::Shape::closed_line(
                        vec![p, v(-7.0, 11.0), v(7.0, 11.0)],
                        stroke,
                    ));
                    painter.line_segment([v(-9.0, 15.0), v(9.0, 15.0)], stroke);
                }
                SupportSymbol::Fixed => {
                    painter.rect_filled(
                        egui::Rect::from_min_max(v(-9.0, 1.0), v(9.0, 8.0)),
                        0.0,
                        green,
                    );
                }
                SupportSymbol::Spring => {
                    // Zig-zag along the restraint direction, hanging away from the structure.
                    let Some(q) = to_screen(g.pos + g.dir * 0.01) else {
                        continue;
                    };
                    let mut d = (q - p).normalized();
                    if !d.x.is_finite() || d.length() < 0.5 {
                        d = egui::vec2(0.0, 1.0);
                    }
                    if d.y < 0.0 {
                        d = -d;
                    }
                    let n = egui::vec2(-d.y, d.x);
                    let (len, amp, zigs) = (18.0, 4.0, 6);
                    let mut pts = vec![p];
                    for i in 1..zigs {
                        let side = if i % 2 == 0 { -amp } else { amp };
                        pts.push(p + d * (len * i as f32 / zigs as f32) + n * side);
                    }
                    let end = p + d * len;
                    pts.push(end);
                    painter.add(egui::Shape::line(pts, stroke));
                    painter.line_segment([end - n * 6.0, end + n * 6.0], stroke);
                }
            }
        }
        for l in &self.scene.labels {
            if let Some(p) = to_screen(l.pos) {
                painter.text(
                    p,
                    egui::Align2::CENTER_CENTER,
                    &l.text,
                    egui::FontId::proportional(12.0),
                    rgba(l.color),
                );
            }
        }
    }
}

/// Does the segment `a`-`b` touch the rectangle?
fn segment_hits_rect(a: Pos2, b: Pos2, r: egui::Rect) -> bool {
    if r.contains(a) || r.contains(b) {
        return true;
    }
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ];
    (0..4).any(|i| segments_cross(a, b, corners[i], corners[(i + 1) % 4]))
}

fn segments_cross(p1: Pos2, p2: Pos2, q1: Pos2, q2: Pos2) -> bool {
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a - o).x * (b - o).y - (a - o).y * (b - o).x;
    let (d1, d2) = (cross(q1, q2, p1), cross(q1, q2, p2));
    let (d3, d4) = (cross(p1, p2, q1), cross(p1, p2, q2));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

pub fn rgba(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

/// Vertical gradient behind the 3D scene.
fn paint_background(painter: &egui::Painter, rect: egui::Rect) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), BG_TOP);
    mesh.colored_vertex(rect.right_top(), BG_TOP);
    mesh.colored_vertex(rect.right_bottom(), BG_BOTTOM);
    mesh.colored_vertex(rect.left_bottom(), BG_BOTTOM);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Small orientation triad in the bottom-left corner.
fn paint_axis_triad(painter: &egui::Painter, rect: egui::Rect, camera: &OrbitCamera) {
    let center = Pos2::new(rect.left() + 55.0, rect.bottom() - 55.0);
    let view = camera.view();
    let mut axes: Vec<(Vec3, &str, [f32; 4])> = scene::GLOBAL_AXES
        .iter()
        .zip([AXIS_X, AXIS_Y, AXIS_Z])
        .map(|((d, n), c)| (view.transform_vector3(*d), *n, c))
        .collect();
    // Draw the axis pointing away from the viewer first.
    axes.sort_by(|a, b| a.0.z.total_cmp(&b.0.z));
    for (v, name, color) in axes {
        let tip = center + egui::vec2(v.x, -v.y) * 38.0;
        painter.line_segment([center, tip], Stroke::new(2.5_f32, rgba(color)));
        painter.text(
            center + egui::vec2(v.x, -v.y) * 48.0,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(13.0),
            rgba(color),
        );
    }
}
