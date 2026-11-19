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

/// The colour-scale legend for the active result overlay (values stored in SI).
struct ResultLegend {
    label: &'static str,
    unit: &'static str,
    /// SI -> display-unit factor (e.g. 1000 for m -> mm).
    factor: f64,
    min: f32,
    max: f32,
}

/// Formats a legend value with a sensible number of digits for its size.
fn format_legend_value(v: f64) -> String {
    let a = v.abs();
    if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

/// Spring surface supports by surface number (finite `translation_z` only: a rigid support has
/// no spring, so no contact stress from displacements).
fn contact_supports(
    model: &femcraft_model::Model,
) -> std::collections::HashMap<u32, scene::ContactSupport> {
    let mut out = std::collections::HashMap::new();
    for (_, sup) in model.iter_kind(ObjectKind::SurfaceSupport) {
        let Some(cz) = sup.get("translation_z").as_f64().filter(|c| c.is_finite() && *c > 0.0)
        else {
            continue;
        };
        let tension_free = sup.get("nonlinearity").as_i64().is_some_and(|n| n != 0);
        for &n in sup.get("surfaces").as_ref_list() {
            out.insert(n, scene::ContactSupport { cz, tension_free });
        }
    }
    out
}

/// Diagonal of the FE mesh's bounding box (model units), used to scale the deformation factor.
fn mesh_span(mesh: &femcraft_rfdb::FeMesh) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for n in &mesh.nodes {
        for k in 0..3 {
            lo[k] = lo[k].min(n[k]);
            hi[k] = hi[k].max(n[k]);
        }
    }
    let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

pub struct Document {
    /// Unique per session; names the tab and the GPU buffers.
    pub id: u64,
    pub path: PathBuf,
    /// The file as loaded (model plus how it maps to the database), kept for saving.
    pub rf: RfDocument,
    /// FE mesh RFEM saved in the file (shown until the model is edited).
    pub fe_mesh: Option<femcraft_rfdb::FeMesh>,
    /// Solver results RFEM saved in the file (`None` for an unsolved model).
    pub results: Option<femcraft_rfdb::ResultSet>,
    /// The Results panel's current selection.
    pub results_view: crate::results_panel::ResultsView,
    /// The result case last mirrored into the Display panel's load case.
    synced_case: Option<(femcraft_rfdb::CaseKind, u32)>,
    /// The Display panel's load case as last seen, to tell when the user changed it.
    synced_lc: u32,
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

    /// Decoded nodal displacements for the selected result case (by solver id; inner `None` when
    /// the case has no displacement file, so it is not re-decoded every frame).
    result_disp: Option<(Vec<u32>, Option<Vec<femcraft_rfdb::Displacements>>)>,
    /// Colour-scale legend for the active result overlay, drawn over the viewport.
    result_legend: Option<ResultLegend>,

    lines: Arc<Vec<LineVertex>>,
    triangles: Arc<Vec<LineVertex>>,
    result_triangles: Arc<Vec<LineVertex>>,
    grid_lines: Arc<Vec<LineVertex>>,
    version: u64,
    built_from: Option<(GridSettings, bool, Visibility, u64, u64)>,
    /// Box drag in progress: button and start position.
    drag_box: Option<(egui::PointerButton, Pos2)>,
}

impl Document {
    /// Reads an `.rf6` into a new document.
    pub fn open(id: u64, path: &Path) -> Result<Self, String> {
        let doc = RfDocument::open(path).map_err(|e| e.to_string())?;
        let load_cases = scene::load_cases(&doc.model);
        let mut warnings = doc.report.warnings.clone();
        let fe_mesh = doc.fe_mesh().unwrap_or_else(|e| {
            warnings.push(format!("FE mesh not read: {e}"));
            None
        });
        let results = doc.results().unwrap_or_else(|e| {
            warnings.push(format!("Results not read: {e}"));
            None
        });
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
            warnings,
            rf: doc,
            fe_mesh,
            results,
            results_view: crate::results_panel::ResultsView::default(),
            synced_case: None,
            synced_lc: vis.load_case,
            result_disp: None,
            result_legend: None,
            revision: 0,
            vis,
            scene: Scene::default(),
            camera: OrbitCamera::default(),
            selection: HashSet::new(),
            cursor: None,
            lines: Arc::new(Vec::new()),
            triangles: Arc::new(Vec::new()),
            result_triangles: Arc::new(Vec::new()),
            grid_lines: Arc::new(Vec::new()),
            version: 0,
            built_from: None,
            drag_box: None,
        })
    }

    /// Reloads the model from `path` in place, as when RFEM has calculated it: the data,
    /// FE mesh and results are replaced while the camera, selection and display settings are
    /// kept, so the results just become available in the open tab.
    pub fn reload(&mut self, path: &Path, prototypes: Vec<PathBuf>) -> Result<(), String> {
        let fresh = Self::open(self.id, path)?;
        let Self {
            mut rf,
            fe_mesh,
            results,
            load_cases,
            warnings,
            vis,
            ..
        } = fresh;
        rf.prototype_files = prototypes;
        self.rf = rf;
        self.fe_mesh = fe_mesh;
        self.results = results;
        self.warnings = warnings;
        if !load_cases.iter().any(|(n, _)| *n == self.vis.load_case) {
            self.vis.load_case = vis.load_case;
        }
        self.load_cases = load_cases;
        self.selection
            .retain(|id| self.rf.model.get(*id).is_some());
        self.results_view = crate::results_panel::ResultsView::default();
        // Freshly calculated: show the first result quantity (displacements) of the first case.
        if let Some(results) = &self.results {
            let first = |nodes: &[femcraft_rfdb::ResultNode]| {
                fn find(nodes: &[femcraft_rfdb::ResultNode]) -> Option<femcraft_rfdb::ResultComponent> {
                    nodes.iter().find_map(|n| match n {
                        femcraft_rfdb::ResultNode::Leaf(l)
                            if l.group == femcraft_rfdb::RadioGroup::Result =>
                        {
                            Some(l.component)
                        }
                        femcraft_rfdb::ResultNode::Leaf(_) => None,
                        femcraft_rfdb::ResultNode::Category { children, .. } => find(children),
                    })
                }
                find(nodes)
            };
            let tree = femcraft_rfdb::results::result_tree(&self.rf.model);
            if let (Some(case), Some(comp)) = (results.cases.first(), first(&tree)) {
                self.results_view.selected = Some((case.kind, case.no));
                self.results_view.active = Some(comp);
                self.results_view.picked = true; // switches the Results display on
            }
        }
        self.synced_case = None;
        self.synced_lc = self.vis.load_case;
        self.result_disp = None;
        self.result_legend = None;
        self.path = path.to_owned();
        self.revision += 1;
        self.built_from = None;
        Ok(())
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
        self.rf.is_modified()
    }

    /// Writes the model (with its edits) to `path` as `.rf6`; the document then refers to it.
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        if self.rf.is_modified() {
            // Saving an edited model drops RFEM's mesh (see `RfDocument::save`).
            self.fe_mesh = None;
        }
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

    /// Rebuilds the drawable geometry when the grid, axes, display settings or result selection
    /// changed.
    pub fn refresh(&mut self, grid: &GridSettings, show_axes: bool) {
        // Support glyphs along lines / surface edges follow the grid spacing.
        self.vis.glyph_spacing = grid.spacing;
        // Picking a result quantity in the Results panel turns the Results display on.
        if std::mem::take(&mut self.results_view.picked) {
            self.vis.results = true;
        }
        // Choosing a load case in the Results panel shows that case's loads in the Display
        // panel too (combinations and design situations have no loads of their own to show).
        if self.results_view.selected != self.synced_case {
            self.synced_case = self.results_view.selected;
            if let Some((femcraft_rfdb::CaseKind::LoadCase, no)) = self.synced_case
                && self.load_cases.iter().any(|(n, _)| *n == no)
            {
                self.vis.load_case = no;
            }
        } else if self.vis.load_case != self.synced_lc
            && let Some(results) = &self.results
            && let Some(case) = results.cases.iter().find(|c| {
                c.kind == femcraft_rfdb::CaseKind::LoadCase && c.no == self.vis.load_case
            })
        {
            // The user picked another load case in the Display panel: show its results too.
            self.results_view.selected = Some((case.kind, case.no));
            self.synced_case = self.results_view.selected;
        }
        self.synced_lc = self.vis.load_case;
        // Load results for the selected case and pick an exaggeration factor before hashing.
        self.update_result_overlay();
        let key = (
            grid.clone(),
            show_axes,
            self.vis.clone(),
            self.revision,
            self.result_signature(),
        );
        if self.built_from.as_ref() == Some(&key) {
            return;
        }
        self.scene = scene::build(&self.rf.model, &self.vis);
        if let Some(mesh) = &self.fe_mesh {
            if self.vis.fe_mesh && !self.rf.is_modified() {
                scene::add_fe_mesh(&mut self.scene, mesh);
            }
        }
        // The result contour overlays the model view; only the displaced-mesh edge lines are left
        // off (add_result_surface draws no outlines, since without a depth buffer they would show
        // the far side of the surface through the near one).
        self.add_result_overlay();

        // The grid is drawn before the opaque result contour, so the contour hides it.
        let mut g = Vec::new();
        if grid.visible {
            grid.build_lines(&mut g);
        }
        self.grid_lines = Arc::new(g);
        let mut v = Vec::new();
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
        self.result_triangles = Arc::new(self.scene.result_triangles.clone());
        self.version += 1;
        self.built_from = Some(key);
    }

    /// The active result overlay: `(solver ids, component)` when a displacement or contact
    /// quantity is chosen for a selected case and the model is unedited (results belong to the
    /// saved geometry). A design situation / result combination lists the combinations it
    /// envelopes.
    fn active_overlay(&self) -> Option<(Vec<u32>, femcraft_rfdb::ResultComponent)> {
        if self.rf.is_modified() || !self.vis.results {
            return None;
        }
        let comp = self.results_view.active?;
        if !scene::is_displacement(comp) && !scene::is_contact(comp) {
            return None;
        }
        let (kind, no) = self.results_view.selected?;
        let case = self
            .results
            .as_ref()?
            .cases
            .iter()
            .find(|c| (c.kind, c.no) == (kind, no))?;
        Some((case.result_ids(), comp))
    }

    /// Loads the selected case's displacements (once per case) and, when the factor is unset,
    /// picks one so the peak displacement is a visible fraction of the model.
    fn update_result_overlay(&mut self) {
        let Some((ids, _)) = self.active_overlay() else {
            return;
        };
        let fe_nodes = self.fe_mesh.as_ref().map_or(0, |m| m.nodes.len());
        if self.result_disp.as_ref().map(|(s, _)| s) != Some(&ids) {
            let mut sets = Vec::new();
            for &id in &ids {
                match self.rf.displacements(id, fe_nodes) {
                    Ok(Some(d)) => sets.push(d),
                    Ok(None) => {}
                    Err(e) => self.warnings.push(format!("Results not read: {e}")),
                }
            }
            let decoded = (!sets.is_empty()).then_some(sets);
            self.result_disp = Some((ids, decoded));
        }
        if self.results_view.magnification <= 0.0
            && let (Some((_, Some(sets))), Some(mesh)) = (&self.result_disp, &self.fe_mesh)
        {
            let max_u = sets.iter().map(|d| d.max_translation()).fold(0.0, f64::max);
            if max_u > 0.0 {
                let factor = (0.05 * mesh_span(mesh) / max_u).clamp(1.0, 100_000.0);
                self.results_view.magnification = factor.round();
            }
        }
    }

    /// A hash of the result selection, so the scene rebuilds when it changes.
    fn result_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some((ids, comp)) = self.active_overlay() else {
            return 0;
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ids.hash(&mut h);
        comp.hash(&mut h);
        self.results_view.extremum.hash(&mut h);
        self.results_view.magnification.to_bits().hash(&mut h);
        h.finish()
    }

    /// Draws the active result overlay (displaced coloured surface, deformed member axes) and
    /// records its colour-scale range for the legend.
    fn add_result_overlay(&mut self) {
        self.result_legend = None;
        let Some((_, comp)) = self.active_overlay() else {
            return;
        };
        let (Some((_, Some(sets))), Some(mesh)) = (&self.result_disp, &self.fe_mesh) else {
            return;
        };
        let disps: Vec<&femcraft_rfdb::Displacements> = sets.iter().collect();
        let ext = self.results_view.extremum;
        let range = if scene::is_contact(comp) {
            let supports = contact_supports(&self.rf.model);
            scene::add_contact_surface(&mut self.scene, mesh, &disps, &supports, ext)
        } else {
            let mag = self.results_view.magnification.max(1.0);
            let coloured = self.vis.result_colours;
            scene::add_result_surface(&mut self.scene, mesh, &disps, comp, mag, ext, coloured)
        };
        let coloured = self.vis.result_colours;
        // Member axes coloured by the quantity (members are plotted as lines).
        let member_lines: HashSet<u32> = self
            .rf
            .model
            .iter_kind(ObjectKind::Member)
            .filter_map(|(_, m)| m.get("line").as_ref_no())
            .collect();
        let mut member_plotted = false;
        let range = if scene::is_contact(comp) || member_lines.is_empty() {
            range
        } else {
            let mag = self.results_view.magnification.max(1.0);
            let m = scene::add_result_members(
                &mut self.scene,
                mesh,
                &disps,
                &member_lines,
                comp,
                mag,
                ext,
                coloured,
            );
            member_plotted = m.is_some();
            match (range, m) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                (a, b) => a.or(b),
            }
        };
        // Plain yellow lines carry no colour scale. (The contact stress is always coloured.)
        let range = range.filter(|_| coloured || scene::is_contact(comp));
        if let Some((min, max)) = range {
            let (label, unit, factor) = comp.display();
            self.result_legend = Some(ResultLegend {
                label,
                unit,
                factor,
                min,
                max,
            });
        }
        // Deformed member axes only for a single case (an envelope has no one deformed shape).
        if scene::is_contact(comp) || disps.len() != 1 || member_plotted {
            return;
        }
        let mag = self.results_view.magnification.max(1.0);
        let member_lines: HashSet<u32> = self
            .rf
            .model
            .iter_kind(ObjectKind::Member)
            .filter_map(|(_, m)| m.get("line").as_ref_no())
            .collect();
        if !member_lines.is_empty() {
            scene::add_member_deformation(&mut self.scene, mesh, disps[0], &member_lines, mag);
        }
    }

    /// Draws the result colour-scale legend over the right of the viewport: solid RFEM colour
    /// bands (red/maximum at the top, blue/minimum at the bottom) with the quantity label and a
    /// value at every band boundary, max to min.
    fn paint_result_legend(&self, painter: &egui::Painter, rect: egui::Rect) {
        use femcraft_render::colorscale;
        let Some(leg) = &self.result_legend else {
            return;
        };
        let bands = colorscale::BAND_COUNT;
        let bar_w = 36.0;
        let margin = 16.0;
        let bar_x = rect.right() - margin - bar_w; // bar's left edge
        let bar_right = bar_x + bar_w;
        let bar_h = (rect.height() - 120.0).clamp(140.0, 320.0);
        let bar_top = rect.top() + 56.0;
        let text_col = Color32::from_gray(235);
        let font = egui::FontId::proportional(11.0);
        let boundary = |j: usize| bar_top + bar_h * (j as f32 / bands as f32);

        // Solid bands: top (j = 0) is the highest band (dark red), bottom the lowest (dark blue).
        for i in 0..bands {
            let c = colorscale::rfem_band(bands - 1 - i);
            let col = Color32::from_rgb(
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(bar_x, boundary(i)),
                    egui::pos2(bar_right, boundary(i + 1)),
                ),
                0.0,
                col,
            );
        }
        let bar = egui::Rect::from_min_size(egui::pos2(bar_x, bar_top), egui::vec2(bar_w, bar_h));
        painter.rect_stroke(
            bar,
            0.0,
            Stroke::new(1.0, Color32::from_gray(30)),
            egui::StrokeKind::Outside,
        );

        // Title above the bar, right-aligned so it stays on screen.
        let title = if leg.unit.is_empty() {
            leg.label.to_string()
        } else {
            format!("{} [{}]", leg.label, leg.unit)
        };
        painter.text(
            egui::pos2(bar_right, bar_top - 6.0),
            egui::Align2::RIGHT_BOTTOM,
            title,
            font.clone(),
            text_col,
        );

        // A value at every band boundary (max at the top, min at the bottom), labelled on the left.
        let span = (leg.max - leg.min) as f64;
        for j in 0..=bands {
            let y = boundary(j);
            let value = (leg.max as f64 - span * (j as f64 / bands as f64)) * leg.factor;
            painter.line_segment(
                [egui::pos2(bar_x - 4.0, y), egui::pos2(bar_x, y)],
                Stroke::new(1.0, Color32::from_gray(120)),
            );
            painter.text(
                egui::pos2(bar_x - 7.0, y),
                egui::Align2::RIGHT_CENTER,
                format_legend_value(value),
                font.clone(),
                text_col,
            );
        }
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
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            LinesCallback {
                view: self.id,
                view_proj,
                vertices: self.lines.clone(),
                triangles: self.triangles.clone(),
                result_triangles: self.result_triangles.clone(),
                grid_lines: self.grid_lines.clone(),
                version: self.version,
                pixels,
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
        self.paint_result_legend(&painter, rect);

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
