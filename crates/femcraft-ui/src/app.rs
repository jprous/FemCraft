//! Main window: menu, settings panel, status bar and the 3D viewport.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Stroke};
use eframe::egui_wgpu;
use glam::Vec3;

use femcraft_model::{Model, ObjectId, ObjectKind};
use femcraft_render::camera::{self, OrbitCamera, Projection, StandardView};
use femcraft_render::grid::{GridPlane, GridSettings};
use femcraft_render::viewport::{self, LineVertex, LinesCallback};
use femcraft_rfdb::RfDocument;

use crate::scene::{self, Scene, SupportSymbol, Visibility};

/// An opened model file.
struct OpenModel {
    path: PathBuf,
    model: Model,
    load_cases: Vec<(u32, String)>,
    warnings: Vec<String>,
}

const AXIS_X: [f32; 4] = [0.92, 0.30, 0.30, 1.0];
const AXIS_Y: [f32; 4] = [0.40, 0.82, 0.35, 1.0];
const AXIS_Z: [f32; 4] = [0.35, 0.58, 0.98, 1.0];

const BG_TOP: Color32 = Color32::from_rgb(0x2e, 0x34, 0x3d);
const BG_BOTTOM: Color32 = Color32::from_rgb(0x16, 0x19, 0x1e);

const SPACING_PRESETS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];

pub struct FemCraftApp {
    camera: OrbitCamera,
    grid: GridSettings,
    show_axes: bool,

    /// Geometry last sent to the GPU, and what it was built from.
    lines: Arc<Vec<LineVertex>>,
    triangles: Arc<Vec<LineVertex>>,
    lines_version: u64,
    built_from: Option<(GridSettings, bool, Visibility, u64)>,

    /// Snapped grid point under the mouse.
    cursor: Option<Vec3>,

    model: Option<OpenModel>,
    /// Bumped when a model is opened.
    model_version: u64,
    vis: Visibility,
    scene: Scene,
    open_path: String,
    message: Option<String>,

    /// Selected objects (box selection).
    selection: HashSet<ObjectId>,
    /// Box drag in progress: button and start position.
    drag_box: Option<(egui::PointerButton, Pos2)>,
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

impl FemCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .expect("FemCraft needs the wgpu renderer");
        viewport::install(render_state);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let grid = GridSettings::default();
        let mut camera = OrbitCamera::default();
        camera.fit(grid.origin(), grid.half_size());

        let mut app = Self {
            camera,
            grid,
            show_axes: true,
            lines: Arc::new(Vec::new()),
            triangles: Arc::new(Vec::new()),
            lines_version: 0,
            built_from: None,
            cursor: None,
            model: None,
            model_version: 0,
            vis: Visibility::default(),
            scene: Scene::default(),
            open_path: String::new(),
            message: None,
            selection: HashSet::new(),
            drag_box: None,
        };
        // `femcraft model.rf6 [--lc=N] [--local-axes]` opens the file directly.
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(path) = args.iter().find(|a| !a.starts_with("--")) {
            app.open(Path::new(path));
        }
        for a in &args {
            if let Some(lc) = a.strip_prefix("--lc=").and_then(|n| n.parse().ok()) {
                app.vis.load_case = lc;
            }
            if a == "--local-axes" {
                app.vis.local_axes = true;
            }
        }
        app
    }

    /// Native "Open" dialog (modal), starting in the folder of the last opened model.
    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open RFEM model")
            .add_filter("RFEM 6 model", &["rf6"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = Path::new(&self.open_path).parent().filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open(&path);
        }
    }

    /// Opens an `.rf6`; on failure keeps the current model and shows the error.
    fn open(&mut self, path: &Path) {
        match RfDocument::open(path) {
            Ok(doc) => {
                let load_cases = scene::load_cases(&doc.model);
                self.vis.load_case = scene::first_loaded_case(&doc.model)
                    .or(load_cases.first().map(|(n, _)| *n))
                    .unwrap_or(1);
                self.model = Some(OpenModel {
                    path: path.to_owned(),
                    load_cases,
                    warnings: doc.report.warnings.clone(),
                    model: doc.model,
                });
                self.model_version += 1;
                self.selection.clear();
                self.message = None;
                self.open_path = path.display().to_string();
                self.refresh_lines();
                self.zoom_to_model();
            }
            Err(e) => self.message = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// Rebuild the GPU geometry only when the grid, axes, display settings or model changed.
    fn refresh_lines(&mut self) {
        // Support glyphs along lines / surface edges follow the grid spacing.
        self.vis.glyph_spacing = self.grid.spacing;
        let key = (
            self.grid.clone(),
            self.show_axes,
            self.vis.clone(),
            self.model_version,
        );
        if self.built_from.as_ref() == Some(&key) {
            return;
        }

        self.scene = match &self.model {
            Some(m) => scene::build(&m.model, &self.vis),
            None => Scene::default(),
        };

        let mut v = Vec::new();
        if self.grid.visible {
            self.grid.build_lines(&mut v);
        }
        if self.show_axes {
            let len = self.axis_length();
            for ((dir, _), color) in scene::GLOBAL_AXES.iter().zip([AXIS_X, AXIS_Y, AXIS_Z]) {
                v.push(LineVertex::new(Vec3::ZERO, color));
                v.push(LineVertex::new(*dir * len, color));
            }
        }
        v.extend_from_slice(&self.scene.lines);

        self.lines = Arc::new(v);
        self.triangles = Arc::new(self.scene.triangles.clone());
        self.lines_version += 1;
        self.built_from = Some(key);
    }

    fn axis_length(&self) -> f32 {
        match self.scene.extent() {
            Some((_, r)) => r * 0.4,
            None => self.grid.half_size().max(self.grid.spacing * 5.0),
        }
    }

    fn zoom_to_grid(&mut self) {
        self.camera.fit(self.grid.origin(), self.grid.half_size());
    }

    fn zoom_to_model(&mut self) {
        match self.scene.extent() {
            Some((c, r)) => self.camera.fit(c, r * 1.3),
            None => self.zoom_to_grid(),
        }
    }

    fn set_view(&mut self, view: StandardView) {
        self.camera.set_view(view);
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open…").clicked() {
                        ui.close();
                        self.open_dialog();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    for (label, view) in [
                        ("Isometric", StandardView::Iso),
                        ("Top (XY)", StandardView::Top),
                        ("Front (XZ)", StandardView::Front),
                        ("Right (YZ)", StandardView::Right),
                    ] {
                        if ui.button(label).clicked() {
                            self.set_view(view);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Zoom to model").clicked() {
                        self.zoom_to_model();
                        ui.close();
                    }
                    if ui.button("Zoom to grid").clicked() {
                        self.zoom_to_grid();
                        ui.close();
                    }
                });
            });
        });
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("settings")
            .resizable(true)
            .default_size(250.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.settings_contents(ui));
            });
    }

    /// Model summary, display toggles and load case selector.
    fn model_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("Model");
        if let Some(msg) = &self.message {
            ui.colored_label(Color32::LIGHT_RED, msg);
        }
        let Some(m) = &self.model else {
            ui.weak("No model open. Use File > Open, drop an .rf6 on the window, or start\nfemcraft with a file path.");
            ui.separator();
            return;
        };
        ui.label(
            m.path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        )
        .on_hover_text(m.path.display().to_string());
        let count = |k| m.model.count(k);
        ui.weak(format!(
            "{} nodes · {} lines · {} members · {} surfaces · {} solids",
            count(ObjectKind::Node),
            count(ObjectKind::Line),
            count(ObjectKind::Member),
            count(ObjectKind::Surface),
            count(ObjectKind::Solid),
        ));

        let v = &mut self.vis;
        egui::Grid::new("display").num_columns(2).show(ui, |ui| {
            ui.checkbox(&mut v.nodes, "Nodes");
            ui.checkbox(&mut v.lines, "Lines");
            ui.end_row();
            ui.checkbox(&mut v.members, "Members");
            ui.checkbox(&mut v.surfaces, "Surfaces");
            ui.end_row();
            ui.checkbox(&mut v.supports, "Supports");
            ui.checkbox(&mut v.loads, "Loads");
            ui.end_row();
            ui.checkbox(&mut v.local_axes, "Local axes");
            ui.add_enabled(
                v.loads,
                egui::Checkbox::new(&mut v.load_values, "Load values"),
            );
            ui.end_row();
        });
        let current = m
            .load_cases
            .iter()
            .find(|(n, _)| *n == v.load_case)
            .map_or_else(
                || format!("LC{}", v.load_case),
                |(n, name)| format!("LC{n} {name}"),
            );
        ui.add_enabled_ui(v.loads, |ui| {
            egui::ComboBox::from_label("Load case")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (n, name) in &m.load_cases {
                        ui.selectable_value(&mut v.load_case, *n, format!("LC{n} {name}"));
                    }
                });
        });
        let warn = ui.visuals().error_fg_color;
        for note in m.warnings.iter().chain(&self.scene.notes) {
            ui.colored_label(warn, note);
        }
        ui.separator();
    }

    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        self.model_section(ui);
        {
            {
                ui.heading("Grid");
                ui.checkbox(&mut self.grid.visible, "Show grid");

                let g = &mut self.grid;
                egui::Grid::new("grid_params")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Plane");
                        egui::ComboBox::from_id_salt("grid_plane")
                            .selected_text(g.plane.label())
                            .show_ui(ui, |ui| {
                                for p in GridPlane::ALL {
                                    ui.selectable_value(&mut g.plane, p, p.label());
                                }
                            });
                        ui.end_row();

                        ui.label("Spacing");
                        ui.add(
                            egui::DragValue::new(&mut g.spacing)
                                .speed(0.01)
                                .range(0.01..=100.0)
                                .max_decimals(3)
                                .suffix(" m"),
                        );
                        ui.end_row();

                        ui.label("Extent (±)");
                        ui.add(
                            egui::DragValue::new(&mut g.extent)
                                .speed(0.5)
                                .range(0.1..=5000.0)
                                .max_decimals(2)
                                .suffix(" m"),
                        );
                        ui.end_row();

                        ui.label("Major line every");
                        ui.add(egui::DragValue::new(&mut g.major_every).range(1..=100));
                        ui.end_row();

                        ui.label(format!("Level ({})", g.plane.normal_name()));
                        ui.add(
                            egui::DragValue::new(&mut g.offset)
                                .speed(0.05)
                                .max_decimals(3)
                                .suffix(" m"),
                        );
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.label("Spacing presets");
                ui.horizontal_wrapped(|ui| {
                    for s in SPACING_PRESETS {
                        if ui
                            .selectable_label(g.spacing == s, format!("{s} m"))
                            .clicked()
                        {
                            g.spacing = s;
                        }
                    }
                });
                if g.is_clamped() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "Too many lines for this spacing - grid is drawn smaller than the extent.",
                    );
                }

                ui.separator();
                ui.heading("View");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Iso").clicked() {
                        self.set_view(StandardView::Iso);
                    }
                    if ui.button("Top").clicked() {
                        self.set_view(StandardView::Top);
                    }
                    if ui.button("Front").clicked() {
                        self.set_view(StandardView::Front);
                    }
                    if ui.button("Right").clicked() {
                        self.set_view(StandardView::Right);
                    }
                    if ui.button("Zoom to model").clicked() {
                        self.zoom_to_model();
                    }
                    if ui.button("Zoom to grid").clicked() {
                        self.zoom_to_grid();
                    }
                });
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.camera.projection,
                        Projection::Perspective,
                        "Perspective",
                    );
                    ui.radio_value(
                        &mut self.camera.projection,
                        Projection::Orthographic,
                        "Orthographic",
                    );
                });
                ui.checkbox(&mut self.show_axes, "Show global axes");

                ui.separator();
                ui.weak(
                    "Wheel: zoom\n\
                     Middle-drag: pan\n\
                     Ctrl + middle-drag: rotate\n\
                     Double-click middle: zoom to extents\n\
                     Left-drag: select (→ window, ← crossing)\n\
                     Left-click empty: clear selection\n\
                     Right-drag: zoom to window",
                );
            }
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                // `+ 0.0` turns -0.0 into 0.0 for display.
                match self.cursor.map(|p| scene::to_model(p) + Vec3::ZERO) {
                    Some(p) => ui.monospace(format!(
                        "X {:>9.3} m   Y {:>9.3} m   Z {:>9.3} m",
                        p.x, p.y, p.z
                    )),
                    None => ui.monospace("X         -     Y         -     Z         -  "),
                };
                ui.separator();
                ui.label(format!(
                    "Grid {} · {} m",
                    self.grid.plane.label(),
                    self.grid.spacing
                ));
                if !self.selection.is_empty() {
                    ui.separator();
                    let n = |k| self.selection.iter().filter(|id| id.kind == k).count();
                    ui.label(format!(
                        "Selected: {} nodes · {} lines · {} members · {} surfaces",
                        n(ObjectKind::Node),
                        n(ObjectKind::Line),
                        n(ObjectKind::Member),
                        n(ObjectKind::Surface),
                    ));
                }
            });
        });
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
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
            self.zoom_to_model();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
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

        // GPU-drawn grid and axes
        let aspect = rect.width() / rect.height().max(1.0);
        let view_proj = self.camera.view_proj(aspect);
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            LinesCallback {
                view_proj,
                vertices: self.lines.clone(),
                triangles: self.triangles.clone(),
                version: self.lines_version,
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
            self.grid
                .intersect(origin, dir)
                .map(|hit| self.grid.snap(hit))
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
            let add = ctrl;
            if !add {
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
        if self.show_axes {
            let len = self.axis_length();
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
    }

    /// Nodes, support symbols and load labels, drawn in screen space over the GPU geometry.
    fn paint_model_marks(&self, painter: &egui::Painter, to_screen: &dyn Fn(Vec3) -> Option<Pos2>) {
        let node = rgba(scene::NODE);
        for p in self.scene.nodes.iter().filter_map(|p| to_screen(*p)) {
            painter.circle_filled(p, 2.5, node);
        }
        let green = rgba(scene::SUPPORT);
        let stroke = Stroke::new(1.5_f32, green);
        for g in &self.scene.supports {
            let Some(p) = to_screen(g.pos) else { continue };
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

impl eframe::App for FemCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped: Option<PathBuf> =
            ui.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_owned()));
        if let Some(path) = dropped {
            self.open(&path);
        }
        self.menu_bar(ui);
        self.settings_panel(ui);
        self.status_bar(ui);
        self.refresh_lines();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.viewport_ui(ui));
    }
}

fn rgba(c: [f32; 4]) -> Color32 {
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
