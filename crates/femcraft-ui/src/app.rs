//! Main window: menu bar, ribbon, status bar and the dockable workspace holding the open models
//! and the panels. Several models can be open at once, each in its own tab; the panels, ribbon
//! and status bar act on the active model (the one last clicked or chosen in Windows).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Stroke};
use glam::Vec3;

use femcraft_model::ObjectKind;
use femcraft_render::camera::{Projection, StandardView};
use femcraft_render::grid::{GridPlane, GridSettings};
use femcraft_render::viewport;

use crate::dock::{self, Pane};
use crate::document::Document;
use crate::panels;
use crate::scene::{self, Visibility};

const SPACING_PRESETS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];

/// Recent files kept in File > Open Recent.
const MAX_RECENT: usize = 10;

/// Storage key of the saved UI state; bump the version if `Persisted` changes incompatibly
/// (an unreadable value is ignored and defaults are used).
const PERSIST_KEY: &str = "femcraft.ui.v2";

/// UI state kept from one session to the next (eframe storage, in the user's app data).
#[derive(serde::Serialize, serde::Deserialize)]
struct Persisted {
    tree: egui_tiles::Tree<Pane>,
    recent: Vec<PathBuf>,
    ribbon_minimized: bool,
    /// Display settings (absent in files from older versions).
    #[serde(default)]
    display: Option<SavedDisplay>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedDisplay {
    vis: Visibility,
    show_axes: bool,
    grid: SavedGrid,
}

/// `GridSettings` as saved (the render crate has no serde).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SavedGrid {
    visible: bool,
    /// "XY", "XZ" or "YZ".
    plane: String,
    spacing: f32,
    extent: f32,
    major_every: u32,
    offset: f32,
}

impl Default for SavedGrid {
    fn default() -> Self {
        SavedGrid::from(&GridSettings::default())
    }
}

impl From<&GridSettings> for SavedGrid {
    fn from(g: &GridSettings) -> Self {
        Self {
            visible: g.visible,
            plane: format!("{:?}", g.plane),
            spacing: g.spacing,
            extent: g.extent,
            major_every: g.major_every,
            offset: g.offset,
        }
    }
}

impl From<SavedGrid> for GridSettings {
    fn from(s: SavedGrid) -> Self {
        let d = GridSettings::default();
        let ok = |v: f32| v.is_finite() && v > 0.0;
        Self {
            visible: s.visible,
            plane: GridPlane::ALL
                .into_iter()
                .find(|p| format!("{p:?}") == s.plane)
                .unwrap_or(d.plane),
            spacing: if ok(s.spacing) { s.spacing } else { d.spacing },
            extent: if ok(s.extent) { s.extent } else { d.extent },
            major_every: s.major_every.max(1),
            offset: if s.offset.is_finite() {
                s.offset
            } else {
                d.offset
            },
        }
    }
}

pub struct FemCraftApp {
    grid: GridSettings,
    show_axes: bool,
    /// Display toggles remembered for newly opened models and the next session (follows the
    /// active model).
    display: Visibility,

    /// Open models, in opening order.
    docs: Vec<Document>,
    /// The model the panels, ribbon and status bar act on.
    active: Option<u64>,
    next_doc_id: u64,
    /// Models whose tab was closed this frame (removed after the layout is drawn).
    closed: Vec<u64>,
    /// True while the layout draws: it is taken out of `self.tree` then, so layout changes
    /// (opening a model from a pane, e.g. the Start page) are queued in `to_open`.
    drawing_layout: bool,
    to_open: Vec<PathBuf>,

    /// Recently opened models, most recent first (saved between sessions).
    recent: Vec<PathBuf>,
    /// Ribbon shows only its tab row (saved between sessions).
    ribbon_minimized: bool,
    /// Last error (open / save), shown in red.
    message: Option<String>,
    /// Last confirmation (e.g. saved), shown in the status bar.
    notice: Option<String>,

    /// Dockable layout of the model tabs and panels.
    tree: egui_tiles::Tree<Pane>,
    ribbon_tab: RibbonTab,
    tables: panels::TableState,
    render_state: Option<eframe::egui_wgpu::RenderState>,
}

impl FemCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("FemCraft needs the wgpu renderer");
        viewport::install(&render_state);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let mut app = Self {
            grid: GridSettings::default(),
            show_axes: true,
            display: Visibility::default(),
            docs: Vec::new(),
            active: None,
            next_doc_id: 1,
            closed: Vec::new(),
            drawing_layout: false,
            to_open: Vec::new(),
            recent: Vec::new(),
            ribbon_minimized: false,
            message: None,
            notice: None,
            tree: dock::default_tree(),
            ribbon_tab: RibbonTab::Home,
            tables: panels::TableState::default(),
            render_state: Some(render_state),
        };
        // Layout, recent files and ribbon state from the previous session.
        if let Some(saved) = cc
            .storage
            .and_then(|s| eframe::get_value::<Persisted>(s, PERSIST_KEY))
        {
            if dock::is_complete(&saved.tree) {
                app.tree = saved.tree;
            }
            app.recent = saved.recent;
            app.ribbon_minimized = saved.ribbon_minimized;
            if let Some(d) = saved.display {
                app.display = d.vis;
                app.show_axes = d.show_axes;
                app.grid = d.grid.into();
            }
        }
        // `femcraft a.rf6 [b.rf6 ...] [--lc=N] [--local-axes]` opens the files directly.
        let args: Vec<String> = std::env::args().skip(1).collect();
        for path in args.iter().filter(|a| !a.starts_with("--")) {
            app.open(Path::new(path));
        }
        for a in &args {
            if let Some(d) = app.active_doc_mut() {
                if let Some(lc) = a.strip_prefix("--lc=").and_then(|n| n.parse().ok()) {
                    d.vis.load_case = lc;
                }
                if a == "--local-axes" {
                    d.vis.local_axes = true;
                }
            }
        }
        app
    }

    fn active_doc(&self) -> Option<&Document> {
        self.active
            .and_then(|id| self.docs.iter().find(|d| d.id == id))
    }

    fn active_doc_mut(&mut self) -> Option<&mut Document> {
        let id = self.active?;
        self.docs.iter_mut().find(|d| d.id == id)
    }

    /// Native "Open" dialog (modal), starting in the folder of the last opened model.
    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open RFEM model")
            .add_filter("RFEM 6 model", &["rf6"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self
            .recent
            .first()
            .and_then(|p| p.parent())
            .filter(|d| d.is_dir())
        {
            dialog = dialog.set_directory(dir);
        }
        if let Some(paths) = dialog.pick_files() {
            for path in paths {
                self.open(&path);
            }
        }
    }

    /// Opens an `.rf6` in a new tab, or switches to it if it is already open.
    fn open(&mut self, path: &Path) {
        if self.drawing_layout {
            self.to_open.push(path.to_owned());
            return;
        }
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if let Some(id) = self
            .docs
            .iter()
            .find(|d| std::path::absolute(&d.path).ok().as_ref() == Some(&abs))
            .map(|d| d.id)
        {
            self.activate(id);
            self.remember_recent(&abs);
            return;
        }
        match Document::open(self.next_doc_id, path) {
            Ok(mut doc) => {
                let id = doc.id;
                self.next_doc_id += 1;
                doc.vis.take_toggles(&self.display);
                doc.refresh(&self.grid, self.show_axes);
                doc.zoom_to_model(&self.grid);
                dock::add_document(&mut self.tree, id, self.active);
                self.docs.push(doc);
                self.active = Some(id);
                self.message = None;
                self.remember_recent(&abs);
            }
            Err(e) => self.message = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// Where the blank model behind File > New is kept (next to the saved UI state).
    fn blank_model_path() -> Option<PathBuf> {
        eframe::storage_dir("FemCraft").map(|d| d.join("new model.rf6"))
    }

    /// File > New: an untitled model from the blank model. The first time, the blank model is
    /// made from an RFEM file the user picks (settings, materials and sections are kept).
    fn new_model(&mut self) {
        let Some(blank) = Self::blank_model_path() else {
            self.message = Some("No folder for FemCraft's data was found".into());
            return;
        };
        if !blank.is_file() {
            let Some(template) = rfd::FileDialog::new()
                .set_title(
                    "New model: choose an RFEM 6 model to take settings, materials and sections from",
                )
                .add_filter("RFEM 6 model", &["rf6"])
                .pick_file()
            else {
                return;
            };
            if let Some(dir) = blank.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = femcraft_rfdb::build::blank_model(&template, &blank) {
                let _ = std::fs::remove_file(&blank);
                self.message = Some(format!(
                    "Could not make a blank model from {}: {e}",
                    template.display()
                ));
                return;
            }
        }
        let n = (1..)
            .find(|n| !self.docs.iter().any(|d| d.title() == format!("Model {n}")))
            .unwrap_or(1);
        match Document::new_untitled(self.next_doc_id, &blank, &format!("Model {n}")) {
            Ok(mut doc) => {
                let id = doc.id;
                self.next_doc_id += 1;
                doc.vis.take_toggles(&self.display);
                doc.refresh(&self.grid, self.show_axes);
                doc.zoom_to_grid(&self.grid);
                dock::add_document(&mut self.tree, id, self.active);
                self.docs.push(doc);
                self.active = Some(id);
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Could not start a new model: {e}")),
        }
    }

    /// File > Save: writes the active model back to its file (`.rf6`).
    fn save(&mut self) {
        let Some(path) = self.active_doc().map(|d| d.path.clone()) else {
            return;
        };
        let is_rf6 = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rf6"));
        if is_rf6 {
            self.save_to(&path);
        } else {
            self.save_as_dialog();
        }
    }

    /// File > Save As: asks for a new `.rf6` path and saves the active model there.
    fn save_as_dialog(&mut self) {
        let Some(d) = self.active_doc() else { return };
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save RFEM model as")
            .add_filter("RFEM 6 model", &["rf6"])
            .set_file_name(d.title());
        if let Some(dir) = d.path.parent().filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(mut path) = dialog.save_file() {
            if path.extension().is_none() {
                path.set_extension("rf6");
            }
            self.save_to(&path);
        }
    }

    fn save_to(&mut self, path: &Path) {
        let Some(d) = self.active_doc_mut() else {
            return;
        };
        match d.save_as(path) {
            Ok(()) => {
                self.message = None;
                self.notice = Some(format!("Saved {}", path.display()));
                self.remember_recent(path);
            }
            Err(e) => {
                self.notice = None;
                self.message = Some(format!("Could not save {}: {e}", path.display()));
            }
        }
    }

    /// Makes a model active and brings its tab to the front.
    fn activate(&mut self, id: u64) {
        self.active = Some(id);
        dock::focus(&mut self.tree, Pane::Document(id));
    }

    /// Closes a model (its tab, data and GPU buffers).
    fn close(&mut self, id: u64) {
        dock::remove_document(&mut self.tree, id);
        self.docs.retain(|d| d.id != id);
        if let Some(rs) = &self.render_state {
            viewport::forget_view(rs, id);
        }
        if self.active == Some(id) {
            self.active = self.docs.last().map(|d| d.id);
            if let Some(a) = self.active {
                dock::focus(&mut self.tree, Pane::Document(a));
            }
        }
    }

    /// Puts a successfully opened file at the top of the recent list.
    fn remember_recent(&mut self, path: &Path) {
        let path = path.to_owned();
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    fn zoom_to_model(&mut self) {
        let grid = self.grid.clone();
        if let Some(d) = self.active_doc_mut() {
            d.zoom_to_model(&grid);
        }
    }

    fn zoom_to_grid(&mut self) {
        let grid = self.grid.clone();
        if let Some(d) = self.active_doc_mut() {
            d.zoom_to_grid(&grid);
        }
    }

    fn set_view(&mut self, view: StandardView) {
        if let Some(d) = self.active_doc_mut() {
            d.camera.set_view(view);
        }
    }

    /// Projection radio buttons for the active model (disabled without one).
    fn projection_ui(&mut self, ui: &mut egui::Ui) {
        let enabled = self.active.is_some();
        let mut proj = self
            .active_doc()
            .map_or(Projection::Perspective, |d| d.camera.projection);
        ui.add_enabled_ui(enabled, |ui| {
            ui.radio_value(&mut proj, Projection::Perspective, "Perspective");
            ui.radio_value(&mut proj, Projection::Orthographic, "Orthographic");
        });
        if let Some(d) = self.active_doc_mut() {
            d.camera.projection = proj;
        }
    }

    /// Menu bar at the very top: File (with Open Recent), View, Windows, Help.
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui
                        .add(egui::Button::new("New").shortcut_text("Ctrl+N"))
                        .clicked()
                    {
                        ui.close();
                        self.new_model();
                    }
                    if ui.button("Open…").clicked() {
                        ui.close();
                        self.open_dialog();
                    }
                    ui.menu_button("Open Recent", |ui| self.recent_menu(ui));
                    ui.separator();
                    let has_doc = self.active.is_some();
                    if ui
                        .add_enabled(has_doc, egui::Button::new("Save").shortcut_text("Ctrl+S"))
                        .clicked()
                    {
                        ui.close();
                        self.save();
                    }
                    if ui
                        .add_enabled(
                            has_doc,
                            egui::Button::new("Save As…").shortcut_text("Ctrl+Shift+S"),
                        )
                        .clicked()
                    {
                        ui.close();
                        self.save_as_dialog();
                    }
                    ui.separator();
                    let active = self.active;
                    if ui
                        .add_enabled(active.is_some(), egui::Button::new("Close"))
                        .clicked()
                        && let Some(id) = active
                    {
                        self.close(id);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.label("Panels");
                    for pane in Pane::CLOSABLE {
                        let mut open = dock::is_open(&self.tree, pane);
                        if ui.checkbox(&mut open, pane.title()).changed() {
                            dock::set_open(&mut self.tree, pane, open);
                        }
                    }
                    if ui.button("Reset layout").clicked() {
                        self.reset_layout();
                        ui.close();
                    }
                    if ui
                        .button("Reset Display")
                        .on_hover_text("Default display toggles, grid and global axes")
                        .clicked()
                    {
                        self.reset_display();
                        ui.close();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.ribbon_minimized, "Minimise ribbon");
                });
                ui.menu_button("Windows", |ui| self.windows_menu(ui));
                ui.menu_button("Help", |ui| {
                    ui.label(format!("FemCraft {}", env!("CARGO_PKG_VERSION")));
                    ui.weak("Open / edit RFEM 6 models");
                });
            });
        });
    }

    /// Windows: the open models (click to switch), close one or all.
    fn windows_menu(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() {
            ui.weak("No models open");
            return;
        }
        let mut switch_to = None;
        for (i, d) in self.docs.iter().enumerate() {
            let star = if d.is_modified() { " *" } else { "" };
            let label = format!("{}  {}{star}", i + 1, d.title());
            let r = ui
                .selectable_label(self.active == Some(d.id), label)
                .on_hover_text(d.path.display().to_string());
            if r.clicked() {
                switch_to = Some(d.id);
                ui.close();
            }
        }
        ui.separator();
        let active = self.active;
        if ui
            .add_enabled(active.is_some(), egui::Button::new("Close active model"))
            .clicked()
            && let Some(id) = active
        {
            self.close(id);
            ui.close();
        }
        if ui.button("Close all").clicked() {
            for id in self.docs.iter().map(|d| d.id).collect::<Vec<_>>() {
                self.close(id);
            }
            ui.close();
        }
        if let Some(id) = switch_to {
            self.activate(id);
        }
    }

    /// Default layout, with the open models put back in the model space.
    /// View > Reset Display: default display toggles, grid and axes, for every open model.
    fn reset_display(&mut self) {
        self.display = Visibility::default();
        self.grid = GridSettings::default();
        self.show_axes = true;
        for d in &mut self.docs {
            d.vis.take_toggles(&self.display);
        }
    }

    fn reset_layout(&mut self) {
        self.tree = dock::default_tree();
        let mut prev = None;
        for d in &self.docs {
            dock::add_document(&mut self.tree, d.id, prev);
            prev = Some(d.id);
        }
        if let Some(a) = self.active {
            dock::focus(&mut self.tree, Pane::Document(a));
        }
    }

    /// File > Open Recent: the last opened models (missing files greyed out).
    fn recent_menu(&mut self, ui: &mut egui::Ui) {
        if self.recent.is_empty() {
            ui.weak("No recent files");
            return;
        }
        let mut chosen: Option<PathBuf> = None;
        for (i, path) in self.recent.iter().enumerate() {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let label = format!("{}  {name}", i + 1);
            let exists = path.is_file();
            let hover = if exists {
                path.display().to_string()
            } else {
                format!("{} (not found)", path.display())
            };
            let button = ui.add_enabled(exists, egui::Button::new(label));
            if button
                .on_hover_text(&hover)
                .on_disabled_hover_text(&hover)
                .clicked()
            {
                chosen = Some(path.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Clear list").clicked() {
            self.recent.clear();
            ui.close();
        }
        if let Some(path) = chosen {
            self.open(&path);
        }
    }

    /// Ribbon below the menu bar: tabs, and their tool groups unless minimised.
    /// The chevron button or a double-click on a tab minimises / expands it; clicking a tab
    /// while minimised expands it.
    fn ribbon(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("ribbon").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for tab in RibbonTab::ALL {
                    let r = ui.selectable_label(self.ribbon_tab == tab, tab.title());
                    if r.double_clicked() {
                        self.ribbon_minimized = !self.ribbon_minimized;
                    } else if r.clicked() {
                        self.ribbon_tab = tab;
                        self.ribbon_minimized = false;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tip = if self.ribbon_minimized {
                        "Expand the ribbon"
                    } else {
                        "Minimise the ribbon (double-click a tab also toggles)"
                    };
                    if chevron_button(ui, self.ribbon_minimized)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.ribbon_minimized = !self.ribbon_minimized;
                    }
                });
            });
            if self.ribbon_minimized {
                ui.add_space(2.0);
                return;
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.set_min_height(58.0);
                match self.ribbon_tab {
                    RibbonTab::Home => self.ribbon_home(ui),
                    RibbonTab::View => self.ribbon_view(ui),
                    RibbonTab::Model => ribbon_model(ui),
                    RibbonTab::Loads => self.ribbon_loads(ui),
                }
            });
            ui.add_space(2.0);
        });
    }

    fn ribbon_home(&mut self, ui: &mut egui::Ui) {
        ribbon_group(ui, "File", |ui| {
            if ui.button("Open…").clicked() {
                self.open_dialog();
            }
            coming_soon(ui, "Save");
        });
        ribbon_group(ui, "View", |ui| {
            if ui.button("Zoom extents").clicked() {
                self.zoom_to_model();
            }
            if ui.button("Iso").clicked() {
                self.set_view(StandardView::Iso);
            }
        });
    }

    fn ribbon_view(&mut self, ui: &mut egui::Ui) {
        ribbon_group(ui, "Views", |ui| {
            for (label, view) in [
                ("Iso", StandardView::Iso),
                ("Top", StandardView::Top),
                ("Front", StandardView::Front),
                ("Right", StandardView::Right),
            ] {
                if ui.button(label).clicked() {
                    self.set_view(view);
                }
            }
        });
        ribbon_group(ui, "Zoom", |ui| {
            if ui.button("Model").clicked() {
                self.zoom_to_model();
            }
            if ui.button("Grid").clicked() {
                self.zoom_to_grid();
            }
        });
        ribbon_group(ui, "Projection", |ui| self.projection_ui(ui));
        ribbon_group(ui, "Panels", |ui| {
            for pane in Pane::CLOSABLE {
                let mut open = dock::is_open(&self.tree, pane);
                if ui
                    .toggle_value(&mut open, pane.title())
                    .on_hover_text("Show or hide. Drag a panel's tab to move it anywhere.")
                    .changed()
                {
                    dock::set_open(&mut self.tree, pane, open);
                }
            }
            if ui.button("Reset layout").clicked() {
                self.reset_layout();
            }
        });
    }

    fn ribbon_loads(&mut self, ui: &mut egui::Ui) {
        ribbon_group(ui, "Load case", |ui| match self.active_doc_mut() {
            Some(d) => load_case_combo(ui, "ribbon_lc", d),
            None => {
                ui.weak("no model");
            }
        });
        ribbon_group(ui, "Insert", |ui| {
            for name in ["Nodal load", "Line load", "Member load", "Surface load"] {
                coming_soon(ui, name);
            }
        });
    }

    /// The dockable area below the ribbon: model tabs and panels as draggable tiles.
    fn docks(&mut self, ui: &mut egui::Ui) {
        // The tree is taken out while it draws, so panes can borrow the rest of the app.
        let mut tree = std::mem::replace(&mut self.tree, dock::default_tree());
        self.drawing_layout = true;
        tree.ui(&mut TileBehavior { app: self }, ui);
        self.drawing_layout = false;
        self.tree = tree;
        // Models opened from a pane (Start page) while the layout was drawing.
        for path in std::mem::take(&mut self.to_open) {
            self.open(&path);
        }
        // Model tabs closed with their ×: egui_tiles removed the tile, drop the model.
        for id in std::mem::take(&mut self.closed) {
            self.close(id);
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Document(id) => {
                let (grid, axes) = (self.grid.clone(), self.show_axes);
                if let Some(d) = self.docs.iter_mut().find(|d| d.id == id)
                    && d.viewport_ui(ui, &grid, axes)
                {
                    self.active = Some(id);
                }
            }
            Pane::Start => self.start_page(ui),
            Pane::Display => {
                egui::ScrollArea::vertical()
                    .id_salt("display_pane")
                    .show(ui, |ui| self.settings_contents(ui));
            }
            Pane::Navigator => match self.active_doc_mut() {
                Some(d) => {
                    panels::navigator(ui, Some(&d.rf.model), &d.load_cases, &mut d.selection)
                }
                None => panels::navigator(ui, None, &[], &mut Default::default()),
            },
            Pane::Tables => {
                let tables = &mut self.tables;
                let id = self.active;
                match id.and_then(|id| self.docs.iter_mut().find(|d| d.id == id)) {
                    Some(d) => {
                        let changed = panels::tables(
                            ui,
                            d.id,
                            Some(&mut d.rf.model),
                            tables,
                            &mut d.selection,
                        );
                        if changed {
                            d.revision += 1; // rebuild the 3D view
                        }
                    }
                    None => {
                        panels::tables(ui, 0, None, tables, &mut Default::default());
                    }
                }
            }
        }
    }

    /// Welcome page shown in the model space while no model is open.
    fn start_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading("FemCraft");
            ui.add_space(8.0);
            if ui.button("Open a model…").clicked() {
                self.open_dialog();
            }
            ui.weak("or drop .rf6 files on the window");
            if let Some(msg) = &self.message {
                ui.add_space(8.0);
                ui.colored_label(ui.visuals().error_fg_color, msg);
            }
            if !self.recent.is_empty() {
                ui.add_space(16.0);
                ui.label("Recent");
                let mut chosen = None;
                for path in &self.recent {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    let r = ui
                        .add_enabled(path.is_file(), egui::Button::new(name).frame(false))
                        .on_hover_text(path.display().to_string());
                    if r.clicked() {
                        chosen = Some(path.clone());
                    }
                }
                if let Some(p) = chosen {
                    self.open(&p);
                }
            }
        });
    }

    /// Model summary, display toggles and load case selector for the active model.
    fn model_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("Model");
        if let Some(msg) = &self.message {
            ui.colored_label(Color32::LIGHT_RED, msg);
        }
        let Some(d) = self.active_doc_mut() else {
            ui.weak("No model open. Use File > Open, drop .rf6 files on the window, or start\nfemcraft with file paths.");
            ui.separator();
            return;
        };
        ui.label(d.title())
            .on_hover_text(d.path.display().to_string());
        let count = |k| d.model().count(k);
        ui.weak(format!(
            "{} nodes · {} lines · {} members · {} surfaces · {} solids",
            count(ObjectKind::Node),
            count(ObjectKind::Line),
            count(ObjectKind::Member),
            count(ObjectKind::Surface),
            count(ObjectKind::Solid),
        ));

        let v = &mut d.vis;
        egui::Grid::new("display").num_columns(2).show(ui, |ui| {
            ui.checkbox(&mut v.nodes, "Nodes");
            ui.checkbox(&mut v.lines, "Lines");
            ui.end_row();
            ui.checkbox(&mut v.members, "Members");
            ui.add_enabled(
                v.members,
                egui::Checkbox::new(&mut v.member_profiles, "Profile"),
            )
            .on_hover_text("Draw members with their section; off: as a thick line");
            ui.end_row();
            ui.checkbox(&mut v.surfaces, "Surfaces");
            ui.checkbox(&mut v.supports, "Supports");
            ui.end_row();
            ui.checkbox(&mut v.loads, "Loads");
            ui.checkbox(&mut v.local_axes, "Local axes");
            ui.end_row();
            ui.add_enabled(
                v.loads,
                egui::Checkbox::new(&mut v.load_values, "Load values"),
            );
            ui.end_row();
        });
        let loads_on = d.vis.loads;
        ui.add_enabled_ui(loads_on, |ui| {
            ui.horizontal(|ui| {
                load_case_combo(ui, "display_lc", d);
                ui.label("Load case");
            });
        });
        let warn = ui.visuals().error_fg_color;
        for note in d.warnings.iter().chain(&d.scene.notes) {
            ui.colored_label(warn, note);
        }
        ui.separator();
    }

    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        self.model_section(ui);

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
        let has_model = self.active.is_some();
        ui.add_enabled_ui(has_model, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, view) in [
                    ("Iso", StandardView::Iso),
                    ("Top", StandardView::Top),
                    ("Front", StandardView::Front),
                    ("Right", StandardView::Right),
                ] {
                    if ui.button(label).clicked() {
                        self.set_view(view);
                    }
                }
                if ui.button("Zoom to model").clicked() {
                    self.zoom_to_model();
                }
                if ui.button("Zoom to grid").clicked() {
                    self.zoom_to_grid();
                }
            });
        });
        ui.horizontal(|ui| self.projection_ui(ui));
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

    fn status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let doc = self.active_doc();
                // `+ 0.0` turns -0.0 into 0.0 for display.
                match doc
                    .and_then(|d| d.cursor)
                    .map(|p| scene::to_model(p) + Vec3::ZERO)
                {
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
                if let Some(n) = &self.notice {
                    ui.separator();
                    ui.label(n);
                }
                if let Some(d) = doc {
                    ui.separator();
                    ui.label(d.title());
                    if !d.selection.is_empty() {
                        ui.separator();
                        let n = |k| d.selection.iter().filter(|id| id.kind == k).count();
                        ui.label(format!(
                            "Selected: {} nodes · {} lines · {} members · {} surfaces",
                            n(ObjectKind::Node),
                            n(ObjectKind::Line),
                            n(ObjectKind::Member),
                            n(ObjectKind::Surface),
                        ));
                    }
                }
            });
        });
    }
}

/// Load case selector for a model.
fn load_case_combo(ui: &mut egui::Ui, id: &str, d: &mut Document) {
    let current = d
        .load_cases
        .iter()
        .find(|(n, _)| *n == d.vis.load_case)
        .map_or_else(
            || format!("LC{}", d.vis.load_case),
            |(n, name)| format!("LC{n} {name}"),
        );
    egui::ComboBox::from_id_salt((id, d.id))
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (n, name) in &d.load_cases {
                ui.selectable_value(&mut d.vis.load_case, *n, format!("LC{n} {name}"));
            }
        });
}

impl eframe::App for FemCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        for path in dropped {
            self.open(&path);
        }
        // Ctrl+S saves, Ctrl+Shift+S saves as.
        let (save, save_as) = ui.input_mut(|i| {
            let save_as = i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::S,
            );
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                save_as,
            )
        });
        if save_as {
            self.save_as_dialog();
        } else if save {
            self.save();
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            self.new_model();
        }
        // Escape releases the selection of the active model (unless a menu / popup is open,
        // where Escape closes that instead).
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if escape
            && !egui::Popup::is_any_open(ui.ctx())
            && let Some(d) = self.active_doc_mut()
        {
            d.clear_selection();
        }
        self.menu_bar(ui);
        self.ribbon(ui);
        self.status_bar(ui);
        for d in &mut self.docs {
            d.refresh(&self.grid, self.show_axes);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.docks(ui));
        // The active model's display toggles are the ones remembered for the next model and
        // the next session.
        if let Some(v) = self.active_doc().map(|d| d.vis.clone()) {
            self.display.take_toggles(&v);
        }
    }

    /// Called by eframe on exit (and periodically): keeps layout, recent files and ribbon
    /// state. Open models are not reopened automatically, so their tabs are left out.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let state = Persisted {
            tree: dock::for_saving(&self.tree),
            recent: self.recent.clone(),
            ribbon_minimized: self.ribbon_minimized,
            display: Some(SavedDisplay {
                vis: self.display.clone(),
                show_axes: self.show_axes,
                grid: SavedGrid::from(&self.grid),
            }),
        };
        eframe::set_value(storage, PERSIST_KEY, &state);
    }
}

/// How `egui_tiles` draws and names FemCraft's panes.
struct TileBehavior<'a> {
    app: &'a mut FemCraftApp,
}

impl egui_tiles::Behavior<Pane> for TileBehavior<'_> {
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        if matches!(pane, Pane::Document(_)) {
            self.app.pane_ui(ui, *pane);
        } else {
            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.app.pane_ui(ui, *pane);
                });
        }
        egui_tiles::UiResponse::None
    }

    /// Model tabs show the file name; the active model's tab is marked.
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Document(id) => {
                let title = self.app.docs.iter().find(|d| d.id == *id).map_or_else(
                    || "(closed)".to_owned(),
                    |d| {
                        // "*" marks unsaved edits.
                        if d.is_modified() {
                            format!("{} *", d.title())
                        } else {
                            d.title()
                        }
                    },
                );
                if self.app.active == Some(*id) && self.app.docs.len() > 1 {
                    egui::RichText::new(title).strong().into()
                } else {
                    title.into()
                }
            }
            other => other.title().into(),
        }
    }

    /// Clicking a model tab makes it the active model.
    fn on_tab_button(
        &mut self,
        tiles: &mut egui_tiles::Tiles<Pane>,
        tile_id: egui_tiles::TileId,
        button_response: egui::Response,
    ) -> egui::Response {
        if button_response.clicked()
            && let Some(egui_tiles::Tile::Pane(Pane::Document(id))) = tiles.get(tile_id)
        {
            self.app.active = Some(*id);
        }
        if let Some(egui_tiles::Tile::Pane(Pane::Document(id))) = tiles.get(tile_id)
            && let Some(d) = self.app.docs.iter().find(|d| d.id == *id)
        {
            return button_response.on_hover_text(d.path.display().to_string());
        }
        button_response
    }

    fn is_tab_closable(
        &self,
        tiles: &egui_tiles::Tiles<Pane>,
        tile_id: egui_tiles::TileId,
    ) -> bool {
        !matches!(
            tiles.get(tile_id),
            Some(egui_tiles::Tile::Pane(Pane::Start))
        )
    }

    /// Closing a model tab closes the model; closing a panel hides it, so the ribbon / View
    /// menu can bring it back where it was.
    fn on_tab_close(
        &mut self,
        tiles: &mut egui_tiles::Tiles<Pane>,
        tile_id: egui_tiles::TileId,
    ) -> bool {
        if let Some(egui_tiles::Tile::Pane(Pane::Document(id))) = tiles.get(tile_id) {
            self.app.closed.push(*id);
            return false; // removed (with the Start pane restored if needed) after drawing
        }
        tiles.set_visible(tile_id, false);
        false
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            // Every pane keeps a tab bar, so every pane can be dragged.
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        3.0
    }
}

/// Ribbon tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RibbonTab {
    Home,
    View,
    Model,
    Loads,
}

impl RibbonTab {
    const ALL: [RibbonTab; 4] = [
        RibbonTab::Home,
        RibbonTab::View,
        RibbonTab::Model,
        RibbonTab::Loads,
    ];

    fn title(self) -> &'static str {
        match self {
            RibbonTab::Home => "Home",
            RibbonTab::View => "View",
            RibbonTab::Model => "Model",
            RibbonTab::Loads => "Loads",
        }
    }
}

/// Small button with a painted chevron (the default font has no arrow glyphs):
/// pointing down when `down` (expand), up otherwise (minimise).
fn chevron_button(ui: &mut egui::Ui, down: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() {
        ui.painter().rect_filled(rect, 3.0, visuals.bg_fill);
    }
    let c = rect.center();
    let (w, h) = (5.0, 3.0);
    let points = if down {
        vec![
            c + egui::vec2(-w, -h),
            c + egui::vec2(w, -h),
            c + egui::vec2(0.0, h),
        ]
    } else {
        vec![
            c + egui::vec2(-w, h),
            c + egui::vec2(w, h),
            c + egui::vec2(0.0, -h),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        visuals.fg_stroke.color,
        Stroke::NONE,
    ));
    response
}

/// A titled group of ribbon tools.
fn ribbon_group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(add);
            ui.add_space(2.0);
            ui.weak(egui::RichText::new(title).small());
        });
    });
}

/// Placeholder ribbon button for a tool that is not implemented yet.
fn coming_soon(ui: &mut egui::Ui, label: &str) {
    ui.add_enabled(false, egui::Button::new(label))
        .on_disabled_hover_text("Coming later");
}

fn ribbon_model(ui: &mut egui::Ui) {
    ribbon_group(ui, "Insert", |ui| {
        for name in ["Node", "Line", "Member", "Surface", "Opening", "Solid"] {
            coming_soon(ui, name);
        }
    });
    ribbon_group(ui, "Basic objects", |ui| {
        for name in ["Material", "Section", "Thickness"] {
            coming_soon(ui, name);
        }
    });
    ribbon_group(ui, "Supports", |ui| {
        for name in ["Nodal", "Line", "Member", "Surface"] {
            coming_soon(ui, name);
        }
    });
    ribbon_group(ui, "Edit", |ui| {
        coming_soon(ui, "Delete");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_settings_read_back() {
        let grid = GridSettings {
            plane: GridPlane::XZ,
            spacing: 0.5,
            ..GridSettings::default()
        };
        let vis = Visibility {
            member_profiles: false,
            loads: false,
            ..Visibility::default()
        };
        let saved = SavedDisplay {
            vis: vis.clone(),
            show_axes: false,
            grid: SavedGrid::from(&grid),
        };
        let back: SavedDisplay = ron::from_str(&ron::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back.vis, vis);
        assert!(!back.show_axes);
        assert_eq!(GridSettings::from(back.grid), grid);
        // A toggle added in a later version falls back to its default.
        let old: Visibility = ron::from_str("(nodes: false)").unwrap();
        assert!(!old.nodes && old.member_profiles);
    }
}
