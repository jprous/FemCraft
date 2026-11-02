//! Main window: menu, settings panel, status bar and the 3D viewport.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Stroke};
use eframe::egui_wgpu;
use glam::Vec3;

use femcraft_render::camera::{self, OrbitCamera, Projection, StandardView};
use femcraft_render::grid::{GridPlane, GridSettings};
use femcraft_render::viewport::{self, LineVertex, LinesCallback};

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
    lines_version: u64,
    built_from: Option<(GridSettings, bool)>,

    /// Snapped grid point under the mouse.
    cursor: Option<Vec3>,
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

        Self {
            camera,
            grid,
            show_axes: true,
            lines: Arc::new(Vec::new()),
            lines_version: 0,
            built_from: None,
            cursor: None,
        }
    }

    /// Rebuild the line geometry only when the grid or axes settings changed.
    fn refresh_lines(&mut self) {
        let key = (self.grid.clone(), self.show_axes);
        if self.built_from.as_ref() == Some(&key) {
            return;
        }

        let mut v = Vec::new();
        if self.grid.visible {
            self.grid.build_lines(&mut v);
        }
        if self.show_axes {
            let len = self.grid.half_size().max(self.grid.spacing * 5.0);
            for (dir, color) in [(Vec3::X, AXIS_X), (Vec3::Y, AXIS_Y), (Vec3::Z, AXIS_Z)] {
                v.push(LineVertex::new(Vec3::ZERO, color));
                v.push(LineVertex::new(dir * len, color));
            }
        }

        self.lines = Arc::new(v);
        self.lines_version += 1;
        self.built_from = Some(key);
    }

    fn zoom_to_grid(&mut self) {
        self.camera.fit(self.grid.origin(), self.grid.half_size());
    }

    fn set_view(&mut self, view: StandardView) {
        self.camera.set_view(view);
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
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
                ui.add_space(4.0);
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
                        if ui.selectable_label(g.spacing == s, format!("{s} m")).clicked() {
                            g.spacing = s;
                        }
                    }
                });
                if g.is_clamped() {
                    ui.colored_label(
                        Color32::YELLOW,
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
                    if ui.button("Zoom to grid").clicked() {
                        self.zoom_to_grid();
                    }
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.camera.projection, Projection::Perspective, "Perspective");
                    ui.radio_value(&mut self.camera.projection, Projection::Orthographic, "Orthographic");
                });
                ui.checkbox(&mut self.show_axes, "Show global axes");

                ui.separator();
                ui.weak("Right-drag: rotate\nMiddle-drag: pan\nWheel: zoom");
            });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                match self.cursor {
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
            });
        });
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());

        // Navigation
        let delta = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Secondary) {
            self.camera.orbit(delta.x, delta.y);
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            self.camera.pan(delta.x, delta.y, rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
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
            self.grid.intersect(origin, dir).map(|hit| self.grid.snap(hit))
        });
        if let Some(s) = self.cursor.and_then(to_screen) {
            let stroke = Stroke::new(1.5_f32,Color32::from_rgb(255, 200, 60));
            painter.line_segment([s - egui::vec2(6.0, 0.0), s + egui::vec2(6.0, 0.0)], stroke);
            painter.line_segment([s - egui::vec2(0.0, 6.0), s + egui::vec2(0.0, 6.0)], stroke);
        }

        // Axis labels at the tips of the global axes
        if self.show_axes {
            let len = self.grid.half_size().max(self.grid.spacing * 5.0);
            for (dir, name, color) in [(Vec3::X, "X", AXIS_X), (Vec3::Y, "Y", AXIS_Y), (Vec3::Z, "Z", AXIS_Z)] {
                if let Some(p) = to_screen(dir * len * 1.03) {
                    painter.text(p, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(14.0), rgba(color));
                }
            }
        }

        paint_axis_triad(&painter, rect, &self.camera);
    }
}

impl eframe::App for FemCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
    let mut axes: Vec<(Vec3, &str, [f32; 4])> = [(Vec3::X, "X", AXIS_X), (Vec3::Y, "Y", AXIS_Y), (Vec3::Z, "Z", AXIS_Z)]
        .into_iter()
        .map(|(d, n, c)| (view.transform_vector3(d), n, c))
        .collect();
    // Draw the axis pointing away from the viewer first.
    axes.sort_by(|a, b| a.0.z.total_cmp(&b.0.z));
    for (v, name, color) in axes {
        let tip = center + egui::vec2(v.x, -v.y) * 38.0;
        painter.line_segment([center, tip], Stroke::new(2.5_f32,rgba(color)));
        painter.text(
            center + egui::vec2(v.x, -v.y) * 48.0,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(13.0),
            rgba(color),
        );
    }
}
