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
    /// Settings menu (paths).
    #[serde(default)]
    settings: AppSettings,
}

/// Settings menu: locations FemCraft works with. `None` = the default / automatic one.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AppSettings {
    section_db: Option<PathBuf>,
    material_db: Option<PathBuf>,
    /// `RFEM6Server.exe`.
    rfem_server: Option<PathBuf>,
    rfem_port: u16,
    /// Dlubal account e-mail and licence for the server (the password is only read from the
    /// environment, see `femcraft_rfem::PASSWORD_ENV`).
    rfem_email: String,
    rfem_license: String,
    /// Where dialogs start and calculated models are put (default: the folder FemCraft was
    /// started in).
    working_dir: Option<PathBuf>,
    /// File > Import runs in the background with a progress window (on) or blocks the window
    /// until it is done (off). Not in the menus: set `import_in_background` in `app.ron`.
    import_in_background: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            section_db: None,
            material_db: None,
            rfem_server: None,
            rfem_port: femcraft_rfem::DEFAULT_PORT,
            rfem_email: String::new(),
            rfem_license: String::new(),
            working_dir: None,
            import_in_background: true,
        }
    }
}

impl AppSettings {
    fn rfem(&self) -> femcraft_rfem::RfemConfig {
        femcraft_rfem::RfemConfig {
            server_exe: self
                .rfem_server
                .clone()
                .unwrap_or_else(|| PathBuf::from(femcraft_rfem::DEFAULT_SERVER)),
            port: self.rfem_port,
            email: Some(self.rfem_email.clone())
                .filter(|e| !e.is_empty())
                .or_else(|| femcraft_rfem::RfemConfig::default().email),
            license: Some(self.rfem_license.clone())
                .filter(|e| !e.is_empty())
                .or_else(|| femcraft_rfem::RfemConfig::default().license),
        }
    }

    /// Makes the choices take effect (library files, working directory).
    fn apply(&self) {
        use femcraft_rfdb::{LibraryKind, set_db_override};
        set_db_override(LibraryKind::Section, self.section_db.clone());
        set_db_override(LibraryKind::Material, self.material_db.clone());
        if let Some(d) = self.working_dir.as_ref().filter(|d| d.is_dir()) {
            let _ = std::env::set_current_dir(d);
        }
    }
}

/// The RFEM server dialog fields while it is open (applied with Save).
struct RfemDraft {
    server: String,
    port: u16,
    email: String,
    license: String,
    /// Typed password; empty keeps the saved one.
    password: String,
    error: Option<String>,
}

impl RfemDraft {
    fn new(s: &AppSettings) -> Self {
        Self {
            server: s.rfem().server_exe.display().to_string(),
            port: s.rfem_port,
            email: s.rfem_email.clone(),
            license: s.rfem_license.clone(),
            password: String::new(),
            error: None,
        }
    }
}

/// The library database in use for `kind`, for the Settings menu.
fn library_db_in_use(kind: femcraft_rfdb::LibraryKind) -> String {
    femcraft_rfdb::Library::find(&femcraft_rfdb::Library::default_dirs())
        .map(|l| l.db_path(kind))
        .filter(|p| p.is_file())
        .map_or("not found".to_owned(), |p| p.display().to_string())
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
    dots: bool,
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
            dots: g.dots,
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
            dots: s.dots,
        }
    }
}

/// A calculation / meshing run by the RFEM server on another thread.
struct SolveJob {
    /// The model tab it belongs to.
    doc: u64,
    job: femcraft_rfem::Job,
    rx: std::sync::mpsc::Receiver<Result<femcraft_rfem::Solved, String>>,
    /// What the server side is doing now (shown in the progress dialog).
    status: std::sync::Arc<std::sync::Mutex<String>>,
    started: std::time::Instant,
}

/// An import running on another thread. Its model calls (`set_node`, ...) are sent back and
/// answered here, on the UI thread, a few at a time per frame so the progress dialog stays
/// alive; the import is idle while it waits for each answer.
struct ImportJob {
    format: femcraft_import::Format,
    path: PathBuf,
    requests: std::sync::mpsc::Receiver<ImportCall>,
    done: std::sync::mpsc::Receiver<Result<femcraft_import::Report, String>>,
    /// Fraction and stage, set by the importer.
    progress: std::sync::Arc<std::sync::Mutex<(f32, String)>>,
    calls: usize,
    started: std::time::Instant,
    /// The model being built: its calls go there and its 3D view waits for the end.
    doc: u64,
}

/// One model call of an import and where to send its answer.
type ImportCall = (
    String,
    serde_json::Value,
    std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
);

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
    /// File > Import: summary and notes of the last import, shown in a window.
    import_report: Option<(String, Vec<String>)>,

    /// Dockable layout of the model tabs and panels.
    tree: egui_tiles::Tree<Pane>,
    ribbon_tab: RibbonTab,
    tables: panels::TableState,
    render_state: Option<eframe::egui_wgpu::RenderState>,
    /// MCP requests from the HTTP listener (see `femcraft_mcp::http`).
    mcp: Option<std::sync::mpsc::Receiver<femcraft_mcp::http::Request>>,
    settings: AppSettings,
    /// The RFEM server dialog (Settings > RFEM server…) while open.
    rfem_dialog: Option<RfemDraft>,
    /// Help > About FemCraft is open.
    show_about: bool,
    /// A calculation running on another thread (Solve menu).
    solve_job: Option<SolveJob>,
    import_job: Option<ImportJob>,
    /// Document an import is building (no 3D rebuild per call; see `Workspace::changed`).
    importing_doc: Option<u64>,
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
            import_report: None,
            tree: dock::default_tree(),
            ribbon_tab: RibbonTab::Home,
            tables: panels::TableState::default(),
            render_state: Some(render_state),
            mcp: None,
            settings: AppSettings::default(),
            rfem_dialog: None,
            show_about: false,
            solve_job: None,
            import_job: None,
            importing_doc: None,
        };
        // Built-in MCP server on 127.0.0.1 (FEMCRAFT_MCP_PORT, 0 = off). A second instance
        // finds the port taken and runs without it.
        let port = std::env::var("FEMCRAFT_MCP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(femcraft_mcp::DEFAULT_PORT);
        if port != 0 {
            let ctx = cc.egui_ctx.clone();
            app.mcp = femcraft_mcp::http::serve(port, move || ctx.request_repaint()).ok();
        }
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
            app.settings = saved.settings;
            if let Some(d) = saved.display {
                app.display = d.vis;
                app.show_axes = d.show_axes;
                app.grid = d.grid.into();
            }
        }
        app.settings.apply();
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
        } else {
            dialog = dialog.set_directory(self.working_dir());
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
        if let Err(e) = self.try_open(path) {
            self.message = Some(e);
        }
    }

    /// [`Self::open`] returning the error (MCP); the model's id when opened.
    fn try_open(&mut self, path: &Path) -> Result<u64, String> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if let Some(id) = self
            .docs
            .iter()
            .find(|d| std::path::absolute(&d.path).ok().as_ref() == Some(&abs))
            .map(|d| d.id)
        {
            self.activate(id);
            self.remember_recent(&abs);
            return Ok(id);
        }
        match Document::open(self.next_doc_id, path) {
            Ok(mut doc) => {
                let id = doc.id;
                self.next_doc_id += 1;
                doc.vis.take_toggles(&self.display);
                doc.rf.prototype_files = Self::prototype_files();
                doc.refresh(&self.grid, self.show_axes);
                doc.zoom_to_model(&self.grid);
                dock::add_document(&mut self.tree, id, self.active);
                self.docs.push(doc);
                self.active = Some(id);
                self.message = None;
                self.remember_recent(&abs);
                Ok(id)
            }
            Err(e) => Err(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// Where the blank model behind File > New is kept (next to the saved UI state).
    pub fn blank_model_path() -> Option<PathBuf> {
        let path = eframe::storage_dir("FemCraft").map(|d| d.join("new model.rf6"))?;
        // A portable install carries the template in `template` next to the executable: it is
        // copied to the data folder the first time, so File > New works without asking.
        if !path.is_file()
            && let Some(bundled) = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("template").join("new model.rf6")))
                .filter(|b| b.is_file())
        {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::copy(bundled, &path);
        }
        Some(path)
    }

    /// RFEM files new objects are copied from when a model has none of that kind yet: the
    /// `.rf6` files in `prototypes` next to the saved UI state.
    /// Also the `prototypes` folder next to the executable.
    pub fn prototype_files() -> Vec<PathBuf> {
        let dirs = [
            eframe::storage_dir("FemCraft").map(|d| d.join("prototypes")),
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("prototypes"))),
        ];
        let mut files: Vec<PathBuf> = Vec::new();
        for dir in dirs.into_iter().flatten() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rf6")))
                .collect();
            found.sort();
            files.extend(found);
        }
        files
    }

    /// File > New: an untitled model from the template model. The first time, the user picks
    /// the template: an RFEM model without geometry, copied as it is.
    fn new_model(&mut self) {
        let Some(blank) = Self::blank_model_path() else {
            self.message = Some("No folder for FemCraft's data was found".into());
            return;
        };
        if !blank.is_file() {
            let Some(template) = rfd::FileDialog::new()
                .set_title("New model: choose the template (an RFEM 6 model without geometry)")
                .add_filter("RFEM 6 model", &["rf6"])
                .pick_file()
            else {
                return;
            };
            if let Some(dir) = blank.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // Used as it is: a model saved by RFEM (no geometry, with the materials, sections,
            // supports and load cases new models should start with) opens in RFEM for sure.
            if let Err(e) = std::fs::copy(&template, &blank) {
                self.message = Some(format!("Could not copy {}: {e}", template.display()));
                return;
            }
        }
        if let Err(e) = self.new_from(&blank, None) {
            self.message = Some(e);
        }
    }

    /// An untitled model from the blank model `blank`, named `name` or "Model N".
    fn new_from(&mut self, blank: &Path, name: Option<&str>) -> Result<String, String> {
        let name = name.map_or_else(
            || {
                let n = (1..)
                    .find(|n| !self.docs.iter().any(|d| d.title() == format!("Model {n}")))
                    .unwrap_or(1);
                format!("Model {n}")
            },
            str::to_owned,
        );
        match Document::new_untitled(self.next_doc_id, blank, &name) {
            Ok(mut doc) => {
                let id = doc.id;
                self.next_doc_id += 1;
                doc.vis.take_toggles(&self.display);
                doc.rf.prototype_files = Self::prototype_files();
                doc.refresh(&self.grid, self.show_axes);
                doc.zoom_to_grid(&self.grid);
                dock::add_document(&mut self.tree, id, self.active);
                self.docs.push(doc);
                self.active = Some(id);
                self.message = None;
                Ok(name)
            }
            Err(e) => Err(format!("Could not start a new model: {e}")),
        }
    }

    /// File > Import > format: a file dialog filtered on the format's extension, then the
    /// file is built into a new untitled model (save it as `.rf6`).
    fn import_dialog(&mut self, format: femcraft_import::Format) {
        let (name, ext) = format.filter();
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Import {}", format.label()))
            .add_filter(name, ext)
            .add_filter("All files", &["*"])
            .set_directory(self.working_dir())
            .pick_file()
        else {
            return;
        };
        if let Err(e) = self.import(format, &path) {
            self.message = Some(e);
        }
    }

    fn import(&mut self, format: femcraft_import::Format, path: &Path) -> Result<(), String> {
        if !format.supported() {
            return Err(format!("Import of {} is not available yet", format.label()));
        }
        if self.import_job.is_some() {
            return Err("An import is already running".into());
        }
        let blank = Self::blank_model_path()
            .filter(|p| p.is_file())
            .ok_or("no blank model: use File > New once to choose the template")?;
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        // Check the file before opening a model for it.
        femcraft_import::check(format, &bytes)
            .map_err(|e| format!("Could not import {}: {e}", path.display()))?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported".into());
        self.new_from(&blank, Some(&stem))?;
        if !self.settings.import_in_background {
            // Blocking: the window is frozen until the model is built (Settings menu).
            self.importing_doc = self.active;
            let started = std::time::Instant::now();
            let result = femcraft_import::import_with_progress(
                format,
                &bytes,
                &mut |name, args| femcraft_mcp::api::call(self, name, args),
                &mut |_, _| {},
            );
            self.importing_doc = None;
            self.finish_import(path, result?);
            if let Some(n) = &mut self.notice {
                n.push_str(&format!(" ({:.1} s)", started.elapsed().as_secs_f64()));
            }
            return Ok(());
        }
        // The conversion runs on its own thread; its model calls come back here.
        let (req_tx, requests) = std::sync::mpsc::channel::<ImportCall>();
        let (done_tx, done) = std::sync::mpsc::channel();
        let progress = std::sync::Arc::new(std::sync::Mutex::new((0.0f32, String::new())));
        let shared = progress.clone();
        std::thread::Builder::new()
            .name("femcraft-import".into())
            .spawn(move || {
                let mut call = |name: &str, args: &serde_json::Value| {
                    let (tx, rx) = std::sync::mpsc::channel();
                    req_tx
                        .send((name.to_owned(), args.clone(), tx))
                        .map_err(|_| "the import was cancelled".to_string())?;
                    rx.recv()
                        .map_err(|_| "the import was cancelled".to_string())?
                };
                let mut report = |f: f32, what: &str| {
                    if let Ok(mut p) = shared.lock() {
                        *p = (f, what.to_owned());
                    }
                };
                let _ = done_tx.send(femcraft_import::import_with_progress(
                    format,
                    &bytes,
                    &mut call,
                    &mut report,
                ));
            })
            .map_err(|e| format!("Could not start the import: {e}"))?;
        self.import_job = Some(ImportJob {
            format,
            path: path.to_owned(),
            requests,
            done,
            progress,
            calls: 0,
            started: std::time::Instant::now(),
            doc: self.active.unwrap_or_default(),
        });
        self.importing_doc = self.active;
        Ok(())
    }

    /// Answers the running import's model calls (for up to ~80 ms per frame) and picks up its
    /// result.
    fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.import_job.take() else {
            return;
        };
        // The calls act on the imported model even if another tab is active meanwhile.
        let user_active = self.active;
        self.active = Some(job.doc);
        // Wait for each next call (the importer sends it right after our answer): with
        // `try_recv` only about one call per frame was answered.
        let budget = std::time::Duration::from_millis(80);
        let t0 = std::time::Instant::now();
        while let Some(left) = budget.checked_sub(t0.elapsed()) {
            match job.requests.recv_timeout(left) {
                Ok((name, args, reply)) => {
                    job.calls += 1;
                    let _ = reply.send(femcraft_mcp::api::call(self, &name, &args));
                }
                Err(_) => break,
            }
        }
        match job.done.try_recv() {
            Ok(result) => {
                // Calls queued just before the end.
                while let Ok((name, args, reply)) = job.requests.try_recv() {
                    let _ = reply.send(femcraft_mcp::api::call(self, &name, &args));
                }
                self.importing_doc = None;
                match result {
                    Ok(report) => {
                        self.finish_import(&job.path, report);
                        if let Some(n) = &mut self.notice {
                            n.push_str(&format!(" ({:.1} s)", job.started.elapsed().as_secs_f64()));
                        }
                    }
                    Err(e) => {
                        self.message =
                            Some(format!("Could not import {}: {e}", job.path.display()));
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.active = user_active;
                self.import_job = Some(job);
                ctx.request_repaint();
            }
            Err(_) => {
                self.importing_doc = None;
                self.message = Some("The import stopped unexpectedly".into());
            }
        }
    }

    fn finish_import(&mut self, path: &Path, report: femcraft_import::Report) {
        let (grid, axes) = (self.grid.clone(), self.show_axes);
        if let Some(d) = self.active_doc_mut() {
            // `changed` skipped every call of the import: rebuild the 3D view once now.
            d.revision += 1;
            d.load_cases = scene::load_cases(&d.rf.model);
            d.refresh(&grid, axes);
            d.zoom_to_model(&grid);
        }
        let mut notes = report.notes.clone();
        notes.extend(report.errors.iter().map(|e| format!("Error: {e}")));
        self.notice = Some(format!(
            "{} Save it as .rf6 (File > Save As).",
            report.summary()
        ));
        self.import_report = Some((format!("Imported {}", path.display()), notes));
    }

    /// Progress window shown while an import converts (the rest of the app stays usable).
    fn import_progress_dialog(&self, ctx: &egui::Context) {
        let Some(job) = &self.import_job else { return };
        let (fraction, stage) = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
        let secs = job.started.elapsed().as_secs();
        let name = job
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        egui::Window::new("Import")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(format!("Importing {}", job.format.label()));
            ui.add_space(8.0);
            ui.label(name);
            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                    .show_percentage()
                    .animate(true),
            );
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(stage);
            });
            ui.add_space(6.0);
            ui.weak(format!(
                "Elapsed: {}:{:02}   Model calls: {}",
                secs / 60,
                secs % 60,
                job.calls
            ));
            ui.weak("Please wait; the model opens when the conversion is done.");
        });
        ctx.request_repaint();
    }

    /// The notes of the last import (what was not converted).
    fn import_report_ui(&mut self, ctx: &egui::Context) {
        let Some((title, notes)) = &self.import_report else {
            return;
        };
        let mut open = true;
        egui::Window::new("Import report")
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(title);
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for n in notes {
                            ui.label(format!("• {n}"));
                        }
                    });
            });
        if !open {
            self.import_report = None;
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
            let _ = self.save_to(&path);
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
        if let Some(dir) = d
            .path
            .parent()
            .filter(|p| p.is_dir() && !p.as_os_str().is_empty())
        {
            dialog = dialog.set_directory(dir);
        } else {
            dialog = dialog.set_directory(self.working_dir());
        }
        if let Some(mut path) = dialog.save_file() {
            if path.extension().is_none() {
                path.set_extension("rf6");
            }
            let _ = self.save_to(&path);
        }
    }

    fn save_to(&mut self, path: &Path) -> Result<(), String> {
        let Some(d) = self.active_doc_mut() else {
            return Err("no model is open".into());
        };
        match d.save_as(path) {
            Ok(()) => {
                self.message = None;
                self.notice = Some(format!("Saved {}", path.display()));
                self.remember_recent(path);
                Ok(())
            }
            Err(e) => {
                let e = format!("Could not save {}: {e}", path.display());
                self.notice = None;
                self.message = Some(e.clone());
                Err(e)
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
                    ui.menu_button("Import", |ui| {
                        for format in femcraft_import::Format::ALL {
                            if ui.button(format.label()).clicked() {
                                ui.close();
                                self.import_dialog(format);
                            }
                        }
                    });
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
                ui.menu_button("Solve", |ui| self.solve_menu(ui));
                ui.menu_button("Settings", |ui| self.settings_menu(ui));
                ui.menu_button("Windows", |ui| self.windows_menu(ui));
                ui.menu_button("Help", |ui| {
                    if ui.button("About FemCraft…").clicked() {
                        ui.close();
                        self.show_about = true;
                    }
                });
            });
        });
    }

    /// The folder dialogs start in and calculated models go to.
    fn working_dir(&self) -> PathBuf {
        self.settings
            .working_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// One Settings item: a button showing the current location; returns the file or folder
    /// chosen in the dialog it opens.
    fn path_item(
        ui: &mut egui::Ui,
        label: &str,
        shown: &str,
        hint: &str,
        dialog: rfd::FileDialog,
        folder: bool,
    ) -> Option<PathBuf> {
        if !ui
            .button(format!("{label}\n{shown}"))
            .on_hover_text(hint)
            .clicked()
        {
            return None;
        }
        ui.close();
        if folder {
            dialog.pick_folder()
        } else {
            dialog.pick_file()
        }
    }

    /// Settings: click an item to choose its location.
    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        use femcraft_rfdb::LibraryKind;
        let auto = |p: &Option<PathBuf>, in_use: String| match p {
            Some(p) => p.display().to_string(),
            None => format!("{in_use}  (automatic)"),
        };
        let mut changed = false;
        ui.weak("Click an item to choose its location");
        ui.separator();

        let shown = auto(
            &self.settings.section_db,
            library_db_in_use(LibraryKind::Section),
        );
        let mut d = rfd::FileDialog::new()
            .set_title("Section database (sections.db)")
            .add_filter("SQLite database", &["db"]);
        if let Some(dir) = self.settings.section_db.as_ref().and_then(|p| p.parent()) {
            d = d.set_directory(dir);
        }
        if let Some(p) = Self::path_item(
            ui,
            "Section database…",
            &shown,
            "sections.db: FemCraft's section library",
            d,
            false,
        ) {
            self.settings.section_db = Some(p);
            changed = true;
        }

        let shown = auto(
            &self.settings.material_db,
            library_db_in_use(LibraryKind::Material),
        );
        let mut d = rfd::FileDialog::new()
            .set_title("Material database (materials.db)")
            .add_filter("SQLite database", &["db"]);
        if let Some(dir) = self.settings.material_db.as_ref().and_then(|p| p.parent()) {
            d = d.set_directory(dir);
        }
        if let Some(p) = Self::path_item(
            ui,
            "Material database…",
            &shown,
            "materials.db: FemCraft's material library",
            d,
            false,
        ) {
            self.settings.material_db = Some(p);
            changed = true;
        }

        if ui
            .button(format!(
                "RFEM server…\n{}",
                self.settings.rfem().server_exe.display()
            ))
            .on_hover_text("RFEM6Server.exe location, port and Dlubal login, used by Solve")
            .clicked()
        {
            ui.close();
            self.rfem_dialog = Some(RfemDraft::new(&self.settings));
        }

        let shown = auto(
            &self.settings.working_dir,
            std::env::current_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
        );
        let d = rfd::FileDialog::new()
            .set_title("FemCraft working directory")
            .set_directory(self.working_dir());
        if let Some(p) = Self::path_item(
            ui,
            "Working directory…",
            &shown,
            "Where dialogs start and calculated models (folder 'solved') go",
            d,
            true,
        ) {
            self.settings.working_dir = Some(p);
            changed = true;
        }
        ui.separator();
        if ui.button("Reset to defaults").clicked() {
            ui.close();
            self.settings = AppSettings::default();
            changed = true;
        }
        if changed {
            self.settings.apply();
        }
    }

    /// Settings > RFEM server…: location, port and Dlubal login of the RFEM server.
    fn rfem_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.rfem_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("RFEM server")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("rfem_server_grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("RFEM6Server.exe");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.server).desired_width(320.0));
                            if ui.button("Browse…").clicked() {
                                let mut f = rfd::FileDialog::new()
                                    .set_title("RFEM server (RFEM6Server.exe)")
                                    .add_filter("Program", &["exe"]);
                                if let Some(dir) =
                                    Path::new(&d.server).parent().filter(|p| p.is_dir())
                                {
                                    f = f.set_directory(dir);
                                }
                                if let Some(p) = f.pick_file() {
                                    d.server = p.display().to_string();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Web service port");
                        ui.add(egui::DragValue::new(&mut d.port).range(1..=65535));
                        ui.end_row();
                        ui.label("Dlubal e-mail");
                        ui.add(egui::TextEdit::singleline(&mut d.email).desired_width(320.0));
                        ui.end_row();
                        ui.label("Password");
                        ui.add(
                            egui::TextEdit::singleline(&mut d.password)
                                .password(true)
                                .desired_width(320.0),
                        );
                        ui.end_row();
                        ui.label("Licence (optional)");
                        ui.add(egui::TextEdit::singleline(&mut d.license).desired_width(320.0));
                        ui.end_row();
                    });
                let saved = !d.email.is_empty() && femcraft_rfem::has_saved_password(&d.email);
                ui.weak(if saved {
                    "A password is saved for this e-mail (Windows Credential Manager); \
                     leave the field empty to keep it."
                } else {
                    "The password is saved in the Windows Credential Manager, \
                     not in the settings file."
                });
                ui.weak("Solve logs the server in with this account. An open RFEM window is used instead.");
                if let Some(e) = &d.error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if saved && ui.button("Remove saved password").clicked() {
                        match femcraft_rfem::store_password(&d.email, "") {
                            Ok(()) => d.error = None,
                            Err(e) => d.error = Some(e),
                        }
                    }
                });
            });
        if save {
            let email = d.email.trim().to_owned();
            if !d.password.is_empty() {
                if email.is_empty() {
                    d.error = Some("Enter the e-mail the password belongs to".into());
                    self.rfem_dialog = Some(d);
                    return;
                }
                if let Err(e) = femcraft_rfem::store_password(&email, &d.password) {
                    d.error = Some(format!("Could not save the password: {e}"));
                    self.rfem_dialog = Some(d);
                    return;
                }
            }
            let default_exe = PathBuf::from(femcraft_rfem::DEFAULT_SERVER);
            self.settings.rfem_server = Some(PathBuf::from(d.server.trim()))
                .filter(|p| !p.as_os_str().is_empty() && *p != default_exe);
            self.settings.rfem_port = d.port;
            self.settings.rfem_email = email;
            self.settings.rfem_license = d.license.trim().to_owned();
            self.settings.apply();
            self.notice = Some("RFEM server settings saved".into());
        } else if open && !cancel {
            self.rfem_dialog = Some(d);
        }
    }

    /// Solve: calculate the active model with the RFEM server on another thread.
    fn solve_menu(&mut self, ui: &mut egui::Ui) {
        use femcraft_rfem::Job;
        let busy = self.solve_job.is_some();
        let label = if busy {
            "Calculating…"
        } else {
            "Calculate with RFEM server"
        };
        let mut start = None;
        if ui
            .add_enabled(self.active.is_some() && !busy, egui::Button::new(label))
            .on_hover_text("Saves the model, then runs calculate_all on the RFEM 6 server")
            .clicked()
        {
            start = Some(Job::Calculate);
        }
        if ui
            .add_enabled(
                self.active.is_some() && !busy,
                egui::Button::new("Mesh with RFEM"),
            )
            .on_hover_text("Saves the model, then generates only the FE mesh on the RFEM 6 server")
            .clicked()
        {
            start = Some(Job::Mesh);
        }
        if let Some(job) = start {
            ui.close();
            if let Err(e) = self.start_solve(job) {
                self.notice = None;
                self.message = Some(e);
            }
        }
        let has_results = self
            .active_doc()
            .is_some_and(|d| d.results.is_some() && !d.is_modified());
        if ui
            .add_enabled(has_results && !busy, egui::Button::new("Delete results"))
            .on_hover_text("Removes the calculated results (and FE mesh) from the model file")
            .clicked()
        {
            ui.close();
            if let Err(e) = self.delete_results("Results deleted") {
                self.notice = None;
                self.message = Some(e);
            }
        }
        let has_mesh = self
            .active_doc()
            .is_some_and(|d| d.fe_mesh.is_some() && !d.is_modified());
        if ui
            .add_enabled(has_mesh && !busy, egui::Button::new("Delete mesh"))
            .on_hover_text(
                "Removes the FE mesh from the model file. Results belong to the mesh, so any \
                 results are removed with it.",
            )
            .clicked()
        {
            ui.close();
            if let Err(e) = self.delete_results("Mesh deleted") {
                self.notice = None;
                self.message = Some(e);
            }
        }
        ui.weak(format!(
            "Server: {}",
            self.settings.rfem().server_exe.display()
        ));
    }

    /// Solve > Delete results / Delete mesh: writes the active model without its `!solver/`
    /// data (mesh and results, which cannot exist without it) and reloads it, so it is an
    /// unmeshed, unsolved model again. `done` is the status-bar message.
    fn delete_results(&mut self, done: &str) -> Result<(), String> {
        let path = self.saved_active_path()?;
        let protos = Self::prototype_files();
        let d = self.active_doc_mut().ok_or("no model is open")?;
        if !d.rf.archive.drop_prefixes.iter().any(|p| p == "!solver/") {
            d.rf.archive.drop_prefixes.push("!solver/".into());
        }
        d.save_as(&path)?;
        d.reload(&path, protos)?;
        self.message = None;
        self.notice = Some(done.to_owned());
        Ok(())
    }

    /// The active model's `.rf6` path, saved first when it has unsaved edits.
    fn saved_active_path(&mut self) -> Result<PathBuf, String> {
        let (path, modified) = self
            .active_doc()
            .map(|d| (d.path.clone(), d.is_modified()))
            .ok_or("no model is open")?;
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rf6"))
        {
            return Err("the model is untitled; save it first (File > Save As)".into());
        }
        if modified {
            self.save_to(&path)?;
        }
        Ok(path)
    }

    /// Saves the active model and starts calculating it.
    fn start_solve(&mut self, job: femcraft_rfem::Job) -> Result<(), String> {
        let path = self.saved_active_path()?;
        let id = self.active.ok_or("no model is open")?;
        let cfg = self.settings.rfem();
        let out = self.working_dir().join("solved");
        let (tx, rx) = std::sync::mpsc::channel();
        let what = match job {
            femcraft_rfem::Job::Calculate => "Calculating",
            femcraft_rfem::Job::Mesh => "Meshing",
        };
        self.notice = Some(format!("{what} {} with RFEM…", path.display()));
        self.message = None;
        let status = std::sync::Arc::new(std::sync::Mutex::new(String::from("Starting…")));
        self.solve_job = Some(SolveJob {
            doc: id,
            job,
            rx,
            status: status.clone(),
            started: std::time::Instant::now(),
        });
        std::thread::spawn(move || {
            let report = |s: &str| {
                if let Ok(mut st) = status.lock() {
                    *st = s.to_owned();
                }
            };
            let _ = tx.send(femcraft_rfem::run_job_with(&cfg, job, &path, &out, &report));
        });
        Ok(())
    }

    /// Makes the calculated copy `solved` the model's own file and reloads the open model from
    /// it in place, so the results simply appear in the same tab (view, selection and display
    /// settings are kept). The file it replaces is kept as `<name>.rf6bak`. The calculated copy
    /// is removed. A model edited meanwhile is not replaced: the calculated copy opens as a
    /// separate tab instead. Returns the model's path.
    fn adopt_solved(
        &mut self,
        id: u64,
        solved: &Path,
        job: femcraft_rfem::Job,
    ) -> Result<PathBuf, String> {
        let Some(pos) = self.docs.iter().position(|d| d.id == id) else {
            // The tab was closed meanwhile: show the calculated copy.
            self.try_open(solved)?;
            return Ok(solved.to_owned());
        };
        if self.docs[pos].is_modified() {
            self.try_open(solved)?;
            return Ok(solved.to_owned());
        }
        let target = self.docs[pos].path.clone();
        std::fs::copy(&target, target.with_extension("rf6bak"))
            .map_err(|e| format!("backup of {}: {e}", target.display()))?;
        std::fs::copy(solved, &target).map_err(|e| format!("replace {}: {e}", target.display()))?;
        // The scratch folder (`solved`) also holds RFEM's own backup and log files.
        match solved.parent() {
            Some(dir) if dir.file_name().is_some_and(|n| n == "solved") => {
                let _ = std::fs::remove_dir_all(dir);
            }
            _ => {
                let _ = std::fs::remove_file(solved);
            }
        }
        let protos = Self::prototype_files();
        self.docs[pos].reload(&target, protos)?;
        if job == femcraft_rfem::Job::Mesh {
            // The point of meshing is to see the mesh: switch Display > Mesh on.
            self.docs[pos].vis.fe_mesh = true;
        }
        Ok(target)
    }

    /// Runs `job` on the active model and waits for it (MCP); the open tab is updated in place.
    fn run_blocking(&mut self, job: femcraft_rfem::Job) -> Result<PathBuf, String> {
        if self.solve_job.is_some() {
            return Err("a calculation is already running".into());
        }
        let path = self.saved_active_path()?;
        let id = self.active.ok_or("no model is open")?;
        let done = femcraft_rfem::run_job(
            &self.settings.rfem(),
            job,
            &path,
            &self.working_dir().join("solved"),
        )?;
        self.adopt_solved(id, &done.path, job)
    }

    /// Help > About FemCraft: version and the change log.
    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut close = false;
        #[allow(deprecated)]
        let id = egui::Id::new("about_femcraft");
        let modal = egui::Modal::new(id).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(format!("FemCraft {}", env!("CARGO_PKG_VERSION")));
            ui.label("A 3D structural modelling workspace for FEA models.");
            ui.weak("Independent of, and not endorsed by, Dlubal Software. RFEM is a trademark of its owner.");
            ui.weak("Licensed under MIT or Apache-2.0.");
            ui.separator();
            ui.strong("Change log");
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for line in include_str!("../../../CHANGELOG.md").lines() {
                        if let Some(t) = line.strip_prefix("### ") {
                            ui.add_space(4.0);
                            ui.strong(t);
                        } else if let Some(t) = line.strip_prefix("## ") {
                            ui.add_space(6.0);
                            ui.heading(t);
                        } else if line.starts_with("# ") || line.trim().is_empty() {
                            // title / blank
                        } else {
                            ui.label(line.replacen("- ", "• ", 1));
                        }
                    }
                });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        // Not `modal.should_close()`: the click that chose Help > About reaches the backdrop in
        // the same frame and would close the dialog at once.
        let _ = modal;
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_about = false;
        }
    }

    /// The modal progress dialog shown while the RFEM server works: nothing else can be
    /// clicked until it is done.
    fn solve_dialog(&self, ctx: &egui::Context) {
        let Some(sj) = &self.solve_job else { return };
        let title = match sj.job {
            femcraft_rfem::Job::Calculate => "Calculating with RFEM",
            femcraft_rfem::Job::Mesh => "Meshing with RFEM",
        };
        let status = sj.status.lock().map(|s| s.clone()).unwrap_or_default();
        let secs = sj.started.elapsed().as_secs();
        #[allow(deprecated)]
        let id = egui::Id::new("solve_progress"); // a fixed id: the modal keeps its state
        egui::Modal::new(id).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(title);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(status);
            });
            ui.add_space(6.0);
            ui.weak(format!("Elapsed: {}:{:02}", secs / 60, secs % 60));
            ui.weak("Please wait; the model updates by itself when RFEM is done.");
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Picks up a finished calculation: the results appear in the open model.
    fn poll_solve(&mut self, ctx: &egui::Context) {
        let Some(sj) = &self.solve_job else {
            return;
        };
        let (id, job) = (sj.doc, sj.job);
        match sj.rx.try_recv() {
            Ok(Ok(done)) => {
                self.solve_job = None;
                match self.adopt_solved(id, &done.path, job) {
                    Ok(_) => {
                        self.message = None;
                        let what = match job {
                            femcraft_rfem::Job::Calculate => "Calculated",
                            femcraft_rfem::Job::Mesh => "Meshed",
                        };
                        self.notice = Some(format!("{what} in {:.0} s", done.seconds));
                    }
                    Err(e) => {
                        self.notice = None;
                        self.message = Some(e);
                    }
                }
            }
            Ok(Err(e)) => {
                self.solve_job = None;
                self.notice = None;
                self.message = Some(e);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            Err(_) => self.solve_job = None,
        }
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
            Pane::Results => match self.active_doc_mut() {
                Some(d) => crate::results_panel::results_panel(
                    ui,
                    d.results.as_ref(),
                    Some(&d.rf.model),
                    &mut d.results_view,
                ),
                None => {
                    crate::results_panel::results_panel(ui, None, None, &mut Default::default())
                }
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
                            d.vis.load_case,
                            tables,
                            &mut d.selection,
                        );
                        if changed {
                            d.revision += 1; // rebuild the 3D view
                            // A load case may have been added or renamed.
                            d.load_cases = scene::load_cases(&d.rf.model);
                        }
                        if let Some((doc, obj, item)) = tables.take_pick()
                            && doc == d.id
                        {
                            tables.report(assign_library_item(d, obj, &item));
                        }
                    }
                    None => {
                        panels::tables(ui, 0, None, 0, tables, &mut Default::default());
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
            let has_mesh = d.fe_mesh.is_some() && !d.rf.is_modified();
            ui.add_enabled(has_mesh, egui::Checkbox::new(&mut v.fe_mesh, "Mesh"))
                .on_hover_text("FE mesh saved by RFEM")
                .on_disabled_hover_text(if d.fe_mesh.is_some() {
                    "The mesh no longer matches the edited model; mesh it again in RFEM"
                } else {
                    "No mesh in this file; mesh or calculate the model in RFEM"
                });
            ui.checkbox(&mut v.results, "Results")
                .on_hover_text("Show the selected result (contour) on the model");
            ui.end_row();
            ui.add_enabled(
                v.loads,
                egui::Checkbox::new(&mut v.load_values, "Load values"),
            );
            ui.add_enabled(
                v.results,
                egui::Checkbox::new(&mut v.result_colours, "Colour results"),
            )
            .on_hover_text(
                "Off: results are plain thin yellow lines (members: the line plot; surfaces: \
                 the displaced mesh net, no contour)",
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

    fn status_bar(&mut self, ui: &mut egui::Ui) {
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
                // AutoCAD-style toggles at the right edge.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dots = self.grid.dots;
                    if status_toggle(ui, if dots { "DOTS" } else { "LINES" }, true)
                        .on_hover_text("Grid style: click to switch between dots and lines")
                        .clicked()
                    {
                        self.grid.dots = !dots;
                    }
                    let on = self.grid.visible;
                    if status_toggle(ui, "GRID", on)
                        .on_hover_text("Show / hide the grid")
                        .clicked()
                    {
                        self.grid.visible = !on;
                    }
                });
            });
        });
    }
}

/// Flat status-bar toggle button in the style of AutoCAD's: highlighted while on.
fn status_toggle(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let label = egui::RichText::new(text).monospace().small();
    ui.add(
        egui::Button::new(label)
            .selected(on)
            .min_size(egui::vec2(52.0, 18.0)),
    )
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
        // Requests from MCP clients (the HTTP listener queues them for this thread).
        if let Some(rx) = self.mcp.take() {
            while let Ok(req) = rx.try_recv() {
                req.answer(self);
            }
            self.mcp = Some(rx);
        }
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
        self.poll_solve(&ui.ctx().clone());
        self.poll_import(&ui.ctx().clone());
        self.solve_dialog(&ui.ctx().clone());
        self.import_progress_dialog(&ui.ctx().clone());
        self.menu_bar(ui);
        self.rfem_dialog_ui(&ui.ctx().clone());
        self.about_dialog(&ui.ctx().clone());
        self.import_report_ui(&ui.ctx().clone());
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
            settings: self.settings.clone(),
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

/// MCP methods act on the open models like the menu commands do.
/// Makes material / section `obj` of `d` the picked library item; the text says what was done.
fn assign_library_item(
    d: &mut Document,
    obj: femcraft_model::ObjectId,
    item: &femcraft_rfdb::LibraryItem,
) -> Result<String, String> {
    use femcraft_model::{ObjectId, ObjectKind as K};
    let lib = femcraft_rfdb::Library::find(&femcraft_rfdb::Library::default_dirs())
        .ok_or("No FemCraft library found")?;
    d.rf.assign_library_item(obj, &lib, item)
        .map_err(|e| format!("{} {}: {e}", obj.kind.spec().label, obj.no))?;
    d.revision += 1; // rebuild the 3D view
    let mut note = format!(
        "{} {} is now {} (save to keep it).",
        obj.kind.name(),
        obj.no,
        item.name
    );
    // RFEM material type codes: 1 concrete, 10 steel.
    let material_type = |no: Option<u32>| {
        no.and_then(|n| d.rf.model.get(ObjectId::new(K::Material, n)))
            .and_then(|m| m.get("type").as_i64())
    };
    if obj.kind == K::Section {
        let sec = d.rf.model.get(obj);
        let mat = sec.and_then(|s| s.get("material").as_ref_no());
        let concrete_section = item.category == "concrete";
        match material_type(mat) {
            Some(1) if !concrete_section => note.push_str(&format!(
                " Note: its material {} is concrete.",
                mat.unwrap()
            )),
            Some(t) if t != 1 && concrete_section => note.push_str(&format!(
                " Note: its material {} is not concrete.",
                mat.unwrap()
            )),
            _ => {}
        }
    }
    Ok(note)
}

impl femcraft_mcp::Workspace for FemCraftApp {
    fn new_model(&mut self, name: Option<&str>) -> Result<String, String> {
        let blank = Self::blank_model_path()
            .filter(|p| p.is_file())
            .ok_or("no blank model: use File > New once in FemCraft to choose the template")?;
        self.new_from(&blank, name)
    }

    fn open_model(&mut self, path: &Path) -> Result<String, String> {
        let id = self.try_open(path)?;
        Ok(self
            .docs
            .iter()
            .find(|d| d.id == id)
            .map(Document::title)
            .unwrap_or_default())
    }

    fn save_model(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let own = self
            .active_doc()
            .map(|d| d.path.clone())
            .ok_or("no model is open")?;
        let path = match path {
            Some(p) => std::path::absolute(p).unwrap_or_else(|_| p.to_owned()),
            None if own
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("rf6")) =>
            {
                own
            }
            None => return Err("the model is untitled; use save_as".into()),
        };
        self.save_to(&path)?;
        Ok(path)
    }

    fn solve(&mut self) -> Result<PathBuf, String> {
        self.run_blocking(femcraft_rfem::Job::Calculate)
    }

    fn mesh(&mut self) -> Result<PathBuf, String> {
        self.run_blocking(femcraft_rfem::Job::Mesh)
    }

    fn models(&self) -> Vec<femcraft_mcp::ModelInfo> {
        self.docs
            .iter()
            .map(|d| femcraft_mcp::ModelInfo {
                name: d.title(),
                path: d.path.clone(),
                active: self.active == Some(d.id),
                modified: d.is_modified(),
            })
            .collect()
    }

    fn active(&mut self) -> Option<&mut femcraft_rfdb::RfDocument> {
        self.active_doc_mut().map(|d| &mut d.rf)
    }

    fn changed(&mut self) {
        // A model being imported is redrawn once, at the end (`finish_import`).
        if self.active.is_some() && self.active == self.importing_doc {
            return;
        }
        if let Some(d) = self.active_doc_mut() {
            d.revision += 1; // rebuild the 3D view
            d.load_cases = scene::load_cases(&d.rf.model);
        }
    }
}
