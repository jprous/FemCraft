//! The library dialog: browse FemCraft's material or section library
//! ([`femcraft_rfdb::library`]) and pick an item. Opened by the "…" button in the Name column
//! of the Materials and Sections tables.
//!
//! Layout: a filter tree (region, category, family) on the left, the items in the middle (one
//! row each, sortable columns of key values), every RFEM value of the selected item on the
//! right; Select / Cancel at the bottom (double-click a row to select it).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use eframe::egui;
use egui_extras::{Column, Size, StripBuilder, TableBuilder};
use femcraft_model::ObjectId;
use femcraft_rfdb::{Library, LibraryFilter, LibraryItem, LibraryKind, LibraryValue};

/// A table column: library value key, heading, factor from SI to the shown unit.
struct Col {
    key: &'static str,
    label: &'static str,
    scale: f64,
}

const fn col(key: &'static str, label: &'static str, scale: f64) -> Col {
    Col { key, label, scale }
}

const SECTION_COLS: &[Col] = &[
    col("b", "b [mm]", 1e3),
    col("h", "h [mm]", 1e3),
    col("t_w", "t_w [mm]", 1e3),
    col("t", "t [mm]", 1e3),
    col("t_f", "t_f [mm]", 1e3),
    col("d", "d [mm]", 1e3),
    col("A", "A [cm²]", 1e4),
    col("I_y", "I_y [cm⁴]", 1e8),
    col("I_z", "I_z [cm⁴]", 1e8),
    col("W_y", "W_y [cm³]", 1e6),
];

const MATERIAL_COLS: &[Col] = &[
    col("E", "E [GPa]", 1e-9),
    col("f_ck", "f_ck [MPa]", 1e-6),
    col("f_yk_reinforcing", "f_yk [MPa]", 1e-6),
    col("f_y", "f_y [MPa]", 1e-6),
    col("f_u", "f_u [MPa]", 1e-6),
    col("gamma", "γ [kN/m³]", 1e-3),
];

fn cols(kind: LibraryKind) -> &'static [Col] {
    match kind {
        LibraryKind::Section => SECTION_COLS,
        LibraryKind::Material => MATERIAL_COLS,
    }
}

fn region_label(r: &str) -> &str {
    match r {
        "ZA" => "South Africa",
        "EU" => "Europe",
        "" => "Other",
        r => r,
    }
}

fn category_label(c: &str) -> &str {
    match c {
        "concrete" => "Concrete",
        "reinforcement" => "Reinforcement",
        "steel_hot_rolled" => "Steel, hot rolled",
        "steel_cold_formed" => "Steel, cold formed",
        "hot_rolled" => "Hot rolled",
        "cold_formed" => "Cold formed",
        "welded" => "Welded",
        "other" => "Other",
        c => c,
    }
}

/// One library, loaded once: its items and the table columns.
pub struct LibraryData {
    lib: Library,
    items: Vec<LibraryItem>,
    /// Column values per item `user_id`.
    values: HashMap<i64, Vec<Option<f64>>>,
    /// Item name per RFEM library id.
    names: HashMap<i64, String>,
}

impl LibraryData {
    fn load(kind: LibraryKind) -> Result<Self, String> {
        let dirs = Library::default_dirs();
        let lib = Library::find(&dirs).ok_or_else(|| {
            let list: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
            format!(
                "No FemCraft library found (materials.db / sections.db in {}).",
                list.join(", ")
            )
        })?;
        if kind == LibraryKind::Section {
            let _ = lib.migrate_concrete_names();
        }
        let items = lib
            .list(kind, &LibraryFilter::default())
            .map_err(|e| e.to_string())?;
        let keys: Vec<&str> = cols(kind).iter().map(|c| c.key).collect();
        let values = lib.columns(kind, &keys).map_err(|e| e.to_string())?;
        let names = items
            .iter()
            .filter_map(|i| Some((i.rfem_library_id?, i.name.clone())))
            .collect();
        Ok(Self {
            lib,
            items,
            values,
            names,
        })
    }
}

/// Libraries loaded so far (kept for the session).
#[derive(Default)]
pub struct LibraryCache {
    materials: Option<Result<Rc<LibraryData>, String>>,
    sections: Option<Result<Rc<LibraryData>, String>>,
}

impl LibraryCache {
    /// The library's name for RFEM library id `id` (RFEM stores no names for library items).
    pub fn name(&mut self, kind: LibraryKind, id: i64) -> Option<String> {
        self.get(kind).ok()?.names.get(&id).cloned()
    }

    /// For a library section RFEM knows under an id this library lacks (another manufacturer's
    /// copy of the same profile): the designation of the library section with the same
    /// A / I_y / I_z, when all such sections share one designation (e.g. `UB 254x146x37`).
    pub fn section_name_by_properties(&mut self, a: f64, iy: f64, iz: f64) -> Option<String> {
        let data = self.get(LibraryKind::Section).ok()?;
        let col = |k: &str| SECTION_COLS.iter().position(|c| c.key == k);
        let (ca, cy, cz) = (col("A")?, col("I_y")?, col("I_z")?);
        let close = |x: Option<f64>, y: f64| x.is_some_and(|x| (x - y).abs() <= 1e-2 * y.abs());
        let mut found: Option<String> = None;
        for item in &data.items {
            let Some(v) = data.values.get(&item.user_id) else {
                continue;
            };
            if close(v[ca], a) && close(v[cy], iy) && close(v[cz], iz) {
                let d = item.designation.split(" | ").next().unwrap_or("").to_owned();
                match &found {
                    Some(f) if *f != d => return None,
                    _ => found = Some(d),
                }
            }
        }
        found
    }

    fn get(&mut self, kind: LibraryKind) -> Result<Rc<LibraryData>, String> {
        let slot = match kind {
            LibraryKind::Material => &mut self.materials,
            LibraryKind::Section => &mut self.sections,
        };
        slot.get_or_insert_with(|| LibraryData::load(kind).map(Rc::new))
            .clone()
    }

    /// Forgets the loaded library of `kind`, so the next [`Self::get`] reloads it from disk.
    /// Called after a row is added, removed, or edited.
    fn invalidate(&mut self, kind: LibraryKind) {
        let slot = match kind {
            LibraryKind::Material => &mut self.materials,
            LibraryKind::Section => &mut self.sections,
        };
        *slot = None;
    }
}

/// What the dialog did this frame.
pub enum Outcome {
    Open,
    Cancelled,
    Picked(LibraryItem),
}

/// Filter tree selection: empty strings match everything.
#[derive(Default, Clone, PartialEq)]
struct TreePick {
    region: String,
    category: String,
    family: String,
}

/// Family codes of FemCraft's three parametric concrete section shapes (massive rectangle,
/// square, circle) - the only sections a user can add to / remove from the library. Steel
/// profiles come from a fixed manufacturer catalogue and cannot be made up.
const CONCRETE_FAMILIES: &[&str] = &["R", "SQ", "CIRCLE"];

/// A row being added or edited in the Section table: a parametric concrete shape whose name
/// and dimensions are typed in-place, write-through to the library on every change.
struct EditRow {
    user_id: i64,
    family: String,
    name: String,
    /// Dimensions, in mm (the table's display unit), as typed.
    b: String,
    h: String,
    d: String,
}

impl EditRow {
    fn new(user_id: i64, family: &str, name: String, b_mm: f64, h_mm: f64, d_mm: f64) -> Self {
        Self {
            user_id,
            family: family.to_owned(),
            name,
            b: number(b_mm),
            h: number(h_mm),
            d: number(d_mm),
        }
    }

    fn dims(&self) -> (f64, f64, f64) {
        let b = self.b.parse().unwrap_or(0.0);
        // A square has one true dimension (`b`, its only editable cell - see `table`'s
        // `dim_field`); `self.h` is only ever meaningful for a rectangle.
        let h = if self.family == "SQ" {
            b
        } else {
            self.h.parse().unwrap_or(0.0)
        };
        (b, h, self.d.parse().unwrap_or(0.0))
    }
}

/// A new item's auto-generated name for `family`, following the naming FemCraft's own library
/// RFEM itself uses for parametric concrete sections (mm, e.g. `R_M1 300/600`).
fn concrete_name(family: &str, b_mm: f64, h_mm: f64, d_mm: f64) -> String {
    match family {
        "R" => format!("R_M1 {}/{}", number(b_mm), number(h_mm)),
        "SQ" => format!("SQ_M1 {}", number(b_mm)),
        "CIRCLE" => format!("CIRCLE_M1 {}", number(d_mm)),
        _ => String::new(),
    }
}

/// Section properties of a parametric concrete shape, from its dimensions (mm), in SI units
/// and under the same key names as RFEM's own `SectionData_parameterValues` (so
/// [`Self::library_values`]'s result can overwrite them - see `set_concrete_values`). A
/// rectangle's formulas are used for a square too (`b_mm == h_mm`): RFEM treats them as
/// separate shapes, but a square's section properties are the rectangle's with `b = h`.
///
/// Every formula here was checked against FemCraft's own real, RFEM-computed catalogue
/// entries across several sizes per family (massive rectangle, square, circle), and matches
/// to at least 4 significant figures - except `tor_alpha` (and the `J_alpha` / `W_t` values
/// derived from it) for a non-square rectangle: RFEM's own `tor_alpha` is a torsional *stress*
/// coefficient that needs its own series solution to reproduce exactly (unlike every other
/// property here, which has a closed form), so it reuses the stiffness coefficient `tor_beta`
/// as a reasonable approximation. That only affects torsional-stress reporting; it doesn't
/// matter for a square or circle, whose `tor_alpha` is exact (1, resp. equal to `tor_beta`).
fn concrete_properties(family: &str, b_mm: f64, h_mm: f64, d_mm: f64) -> Vec<(&'static str, f64)> {
    match family {
        "R" | "SQ" => rect_properties(b_mm, h_mm),
        "CIRCLE" => circle_properties(d_mm),
        _ => Vec::new(),
    }
}

/// Massive rectangle (or square, `b == h`) section properties; see [`concrete_properties`].
fn rect_properties(b_mm: f64, h_mm: f64) -> Vec<(&'static str, f64)> {
    let (b, h) = (b_mm / 1e3, h_mm / 1e3);
    let a = b * h;
    let (iy, iz) = (b * h.powi(3) / 12.0, h * b.powi(3) / 12.0);
    let (long, short) = (b.max(h), b.min(h));
    // Roark/Timoshenko's standard engineering approximation for a solid rectangle's torsion
    // constant (`long` = longer full side, `short` = shorter full side).
    let tor_beta = 1.0 / 3.0
        - 0.21 * (short / long) * (1.0 - short.powi(4) / (12.0 * long.powi(4)));
    let it = long * short.powi(3) * tor_beta;
    vec![
        ("b", b),
        ("h", h),
        ("A", a),
        ("A_m", 2.0 * (b + h)),
        ("A_m_V", 2.0 * (b + h) / a),
        ("A_u", 5.0 / 6.0 * a),
        ("A_v", 5.0 / 6.0 * a),
        ("A_y", 5.0 / 6.0 * a),
        ("A_z", 5.0 / 6.0 * a),
        ("DEPTH_FOR_TEMPERATURE_LOAD", h),
        ("WIDTH_FOR_TEMPERATURE_LOAD", b),
        ("I_p", iy + iz),
        ("I_p_SC", iy + iz),
        ("I_t", it),
        ("I_u", iy),
        ("I_v", iz),
        ("I_y", iy),
        ("I_yz", 0.0),
        ("I_z", iz),
        ("J_alpha", tor_beta * long * short.powi(3)),
        ("J_beta", it),
        ("SELF_WEIGHT_INTERNAL", 2500.0 * a),
        ("S_u_max", b * h * h / 8.0),
        ("S_v_max", h * b * b / 8.0),
        ("S_y_max", b * h * h / 8.0),
        ("S_z_max", h * b * b / 8.0),
        ("U_t", 2.0 * (b + h)),
        ("V", a),
        ("W_pl_u", b * h * h / 4.0),
        ("W_pl_v", h * b * b / 4.0),
        ("W_pl_y", b * h * h / 4.0),
        ("W_pl_z", h * b * b / 4.0),
        ("W_t", tor_beta * long * short.powi(2)),
        ("W_u", iy / (h / 2.0)),
        ("W_u_max", iy / (h / 2.0)),
        ("W_u_min", -iy / (h / 2.0)),
        ("W_v", iz / (b / 2.0)),
        ("W_v_max", iz / (b / 2.0)),
        ("W_v_min", -iz / (b / 2.0)),
        ("W_y", iy / (h / 2.0)),
        ("W_y_max", iy / (h / 2.0)),
        ("W_y_min", -iy / (h / 2.0)),
        ("W_z", iz / (b / 2.0)),
        ("W_z_max", iz / (b / 2.0)),
        ("W_z_min", -iz / (b / 2.0)),
        ("alpha", 0.0),
        ("alpha_pl_u", 1.5),
        ("alpha_pl_v", 1.5),
        ("alpha_pl_y", 1.5),
        ("alpha_pl_z", 1.5),
        ("e_y", b / 2.0),
        ("e_z", h / 2.0),
        ("j_p", ((iy + iz) / a).sqrt()),
        ("j_p_SC", ((iy + iz) / a).sqrt()),
        ("j_u", (iy / a).sqrt()),
        ("j_v", (iz / a).sqrt()),
        ("j_y", (iy / a).sqrt()),
        ("j_yz", 0.0),
        ("j_z", (iz / a).sqrt()),
        ("min_w", short),
        ("r_omega_SC", 0.0),
        ("r_u", 0.0),
        ("r_u_SC", 0.0),
        ("r_v", 0.0),
        ("r_v_SC", 0.0),
        ("r_y", 0.0),
        ("r_y_SC", 0.0),
        ("r_z", 0.0),
        ("r_z_SC", 0.0),
        ("t_M", 0.0),
        ("tor_alpha", tor_beta),
        ("tor_beta", tor_beta),
        ("u_SC", 0.0),
        ("u_pl", 0.0),
        ("v_SC", 0.0),
        ("v_pl", 0.0),
        ("y_SC", 0.0),
        ("y_pl", 0.0),
        ("z_SC", 0.0),
        ("z_pl", 0.0),
    ]
}

/// Massive circle section properties; see [`concrete_properties`].
fn circle_properties(d_mm: f64) -> Vec<(&'static str, f64)> {
    let d = d_mm / 1e3;
    let pi = std::f64::consts::PI;
    let a = pi * d * d / 4.0;
    let i = pi * d.powi(4) / 64.0;
    let ip = pi * d.powi(4) / 32.0;
    vec![
        ("d", d),
        ("A", a),
        ("A_m", pi * d),
        ("A_m_V", pi * d / a),
        ("A_u", 0.84 * a),
        ("A_v", 0.84 * a),
        ("A_y", 0.84 * a),
        ("A_z", 0.84 * a),
        ("DEPTH_FOR_TEMPERATURE_LOAD", d),
        ("WIDTH_FOR_TEMPERATURE_LOAD", d),
        ("I_p", ip),
        ("I_p_SC", ip),
        ("I_t", ip),
        ("I_u", i),
        ("I_v", i),
        ("I_y", i),
        ("I_yz", 0.0),
        ("I_z", i),
        ("J_alpha", ip),
        ("J_beta", ip),
        ("SELF_WEIGHT_INTERNAL", 2500.0 * a),
        ("S_u_max", d.powi(3) / 12.0),
        ("S_v_max", d.powi(3) / 12.0),
        ("S_y_max", d.powi(3) / 12.0),
        ("S_z_max", d.powi(3) / 12.0),
        ("U_t", pi * d),
        ("V", a),
        ("W_pl_u", d.powi(3) / 6.0),
        ("W_pl_v", d.powi(3) / 6.0),
        ("W_pl_y", d.powi(3) / 6.0),
        ("W_pl_z", d.powi(3) / 6.0),
        ("W_t", ip / (d / 2.0)),
        ("W_u", i / (d / 2.0)),
        ("W_u_max", i / (d / 2.0)),
        ("W_u_min", -i / (d / 2.0)),
        ("W_v", i / (d / 2.0)),
        ("W_v_max", i / (d / 2.0)),
        ("W_v_min", -i / (d / 2.0)),
        ("W_y", i / (d / 2.0)),
        ("W_y_max", i / (d / 2.0)),
        ("W_y_min", -i / (d / 2.0)),
        ("W_z", i / (d / 2.0)),
        ("W_z_max", i / (d / 2.0)),
        ("W_z_min", -i / (d / 2.0)),
        ("alpha", 0.0),
        ("alpha_pl_u", 16.0 / (3.0 * pi)),
        ("alpha_pl_v", 16.0 / (3.0 * pi)),
        ("alpha_pl_y", 16.0 / (3.0 * pi)),
        ("alpha_pl_z", 16.0 / (3.0 * pi)),
        ("e_y", d / 2.0),
        ("e_z", d / 2.0),
        ("j_p", (ip / a).sqrt()),
        ("j_p_SC", (ip / a).sqrt()),
        ("j_u", (i / a).sqrt()),
        ("j_v", (i / a).sqrt()),
        ("j_y", (i / a).sqrt()),
        ("j_yz", 0.0),
        ("j_z", (i / a).sqrt()),
        ("min_w", d),
        ("r_omega_SC", 0.0),
        ("r_u", 0.0),
        ("r_u_SC", 0.0),
        ("r_v", 0.0),
        ("r_v_SC", 0.0),
        ("r_y", 0.0),
        ("r_y_SC", 0.0),
        ("r_z", 0.0),
        ("r_z_SC", 0.0),
        ("t_M", 0.0),
        ("tor_alpha", 1.0),
        ("tor_beta", 1.0),
        ("u_SC", 0.0),
        ("u_pl", 0.0),
        ("v_SC", 0.0),
        ("v_pl", 0.0),
        ("y_SC", 0.0),
        ("y_pl", 0.0),
        ("z_SC", 0.0),
        ("z_pl", 0.0),
    ]
}

/// [`concrete_properties`] as [`LibraryValue`]s, ready to store.
fn library_values(family: &str, b_mm: f64, h_mm: f64, d_mm: f64) -> Vec<LibraryValue> {
    concrete_properties(family, b_mm, h_mm, d_mm)
        .into_iter()
        .map(|(key, value_si)| LibraryValue {
            key: key.to_owned(),
            value_si: Some(value_si),
            value_text: None,
            thickness_si: None,
        })
        .collect()
}

pub struct LibraryDialog {
    pub kind: LibraryKind,
    /// The table row the pick is for: (document id, object).
    pub target: (u64, ObjectId),
    search: String,
    tree: TreePick,
    selected: Option<i64>,
    details: Option<(i64, Result<Vec<LibraryValue>, String>)>,
    /// Sort: column (0 = name, 1 = standard, 2 = manufacturer, 3.. = value columns), ascending.
    sort: Option<(usize, bool)>,
    /// Scroll the table to the selected row once (on opening).
    scroll_to_selected: bool,
    /// The cell's name, selected once the library is loaded.
    pending_name: Option<String>,
    /// The row currently added / edited in place (a parametric concrete section).
    editing: Option<EditRow>,
    /// Set after a row was added, removed or edited; the library is reloaded on the next
    /// frame to pick up the change.
    reload: bool,
}

impl LibraryDialog {
    /// `current`: the name in the cell, preselected when the library has it.
    pub fn new(kind: LibraryKind, target: (u64, ObjectId), current: &str) -> Self {
        Self {
            kind,
            target,
            search: String::new(),
            tree: TreePick::default(),
            selected: None,
            details: None,
            sort: None,
            scroll_to_selected: !current.is_empty(),
            pending_name: (!current.is_empty()).then(|| current.to_owned()),
            editing: None,
            reload: false,
        }
    }

    fn title(&self) -> &'static str {
        match self.kind {
            LibraryKind::Material => "Material Library",
            LibraryKind::Section => "Section Library",
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, cache: &mut LibraryCache) -> Outcome {
        let data = cache.get(self.kind);
        if let (Ok(data), Some(name)) = (&data, self.pending_name.take()) {
            self.selected = data
                .items
                .iter()
                .find(|i| i.name == name || i.designation == name)
                .map(|i| i.user_id);
        }
        let screen = ctx.content_rect();
        let size = egui::vec2(
            (screen.width() * 0.9).min(1250.0),
            (screen.height() * 0.85).min(720.0),
        );
        let mut outcome = Outcome::Open;
        let modal =
            egui::Modal::new(egui::Id::unique(("library_dialog", self.title()))).show(ctx, |ui| {
                ui.set_width(size.x);
                ui.set_height(size.y);
                ui.heading(self.title());
                match &data {
                    Err(e) => {
                        ui.add_space(8.0);
                        ui.colored_label(ui.visuals().error_fg_color, e);
                        ui.label(
                            "Build the library with tools/rfem_library_extract.py (see CLAUDE.md).",
                        );
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            outcome = Outcome::Cancelled;
                        }
                    }
                    Ok(data) => outcome = self.contents(ui, data),
                }
            });
        if modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        // Closing (or picking) while a row is still being typed: save it, and if it's the
        // row just picked, pick the freshly-typed version rather than the stale one.
        if !matches!(outcome, Outcome::Open)
            && let Ok(data) = &data
            && let Some((item, values)) = self.edited_item()
        {
            let _ = data.lib.update_item(&item, &values);
            let _ = data.lib.set_concrete_values(item.user_id, &values);
            if let Outcome::Picked(picked) = &mut outcome
                && picked.user_id == item.user_id
            {
                *picked = item;
            }
            self.editing = None;
            self.reload = true;
        }
        if self.reload {
            self.reload = false;
            cache.invalidate(self.kind);
        }
        outcome
    }

    fn contents(&mut self, ui: &mut egui::Ui, data: &LibraryData) -> Outcome {
        let mut outcome = Outcome::Open;
        let visible = self.visible(data);
        ui.horizontal(|ui| {
            ui.label("Search:");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("part of the name, e.g. 203x133 or S355")
                    .desired_width(260.0),
            );
            if r.changed() {
                self.scroll_to_selected = false;
            }
            if !self.search.is_empty() && ui.small_button("✖").clicked() {
                self.search.clear();
            }
        });
        ui.add_space(4.0);
        let footer = 30.0;
        StripBuilder::new(ui)
            .size(Size::remainder())
            .size(Size::exact(footer))
            .vertical(|mut strip| {
                strip.strip(|b| {
                    b.size(Size::exact(220.0))
                        .size(Size::remainder().at_least(300.0))
                        .size(Size::exact(300.0))
                        .horizontal(|mut strip| {
                            strip.cell(|ui| self.tree(ui, data));
                            strip.cell(|ui| {
                                if self.kind == LibraryKind::Section {
                                    self.add_remove_row(ui, data);
                                }
                                if let Some(item) = self.table(ui, data, &visible) {
                                    outcome = Outcome::Picked(item);
                                }
                            });
                            strip.cell(|ui| self.details(ui, data));
                        });
                });
                strip.cell(|ui| {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.weak(format!("{} of {} shown", visible.len(), data.items.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                outcome = Outcome::Cancelled;
                            }
                            let picked = self
                                .selected
                                .and_then(|id| data.items.iter().find(|i| i.user_id == id));
                            if ui
                                .add_enabled(picked.is_some(), egui::Button::new("Select"))
                                .clicked()
                            {
                                outcome = Outcome::Picked(picked.unwrap().clone());
                            }
                        });
                    });
                });
            });
        outcome
    }

    /// Add / Remove buttons for the Section table: enabled only for parametric concrete
    /// shapes (rectangular, square, circular), never for library-catalogue (e.g. steel)
    /// sections, whose sizes are fixed by a manufacturer and can't be made up.
    fn add_remove_row(&mut self, ui: &mut egui::Ui, data: &LibraryData) {
        ui.horizontal(|ui| {
            let add_family = CONCRETE_FAMILIES
                .iter()
                .find(|f| self.tree.category == "concrete" && self.tree.family == **f);
            if ui
                .add_enabled(add_family.is_some(), egui::Button::new("➕ Add"))
                .on_hover_text("Add a new size to this concrete section family")
                .on_disabled_hover_text(
                    "Only concrete sections (rectangular, square, circular) can be added; \
                     pick one of their families on the left",
                )
                .clicked()
                && let Some(family) = add_family
            {
                self.add_row(data, family);
            }
            let can_remove = self.selected.is_some_and(|id| {
                data.items
                    .iter()
                    .any(|i| i.user_id == id && i.category == "concrete")
            });
            if ui
                .add_enabled(can_remove, egui::Button::new("➖ Remove"))
                .on_hover_text("Remove the selected size")
                .on_disabled_hover_text(
                    "Only concrete sections (rectangular, square, circular) can be removed",
                )
                .clicked()
            {
                self.remove_row(data);
            }
        });
    }

    /// Adds a new, default-sized item of `family` to the library, selects it and switches it
    /// into edit mode so its name and dimensions can be typed right away.
    fn add_row(&mut self, data: &LibraryData, family: &str) {
        if self.editing.is_some() {
            self.commit_edit(data);
            self.editing = None;
        }
        // A real RFEM row tree to copy: picking the new item has to find one (`Section`
        // WHERE userID = ...), same as any catalogue item's, or assigning it to a model fails.
        let Some(template) = data
            .items
            .iter()
            .find(|i| i.family == family && i.category == "concrete")
        else {
            return;
        };
        let Ok(user_id) = data.lib.next_user_id(LibraryKind::Section) else {
            return;
        };
        if data
            .lib
            .duplicate_concrete_item(template.user_id, user_id)
            .is_err()
        {
            return;
        }
        let (b_mm, h_mm, d_mm) = (300.0, 300.0, 300.0);
        let name = concrete_name(family, b_mm, h_mm, d_mm);
        let item = LibraryItem {
            kind: LibraryKind::Section,
            user_id,
            name: name.clone(),
            designation: name.clone(),
            standard: String::new(),
            manufacturer: String::new(),
            region: self.tree.region.clone(),
            family: family.to_owned(),
            category: "concrete".to_owned(),
            rfem_library_id: None,
            rfem_version: String::new(),
        };
        let values = library_values(family, b_mm, h_mm, d_mm);
        let saved = data.lib.insert_item(&item, &values).is_ok()
            && data.lib.set_concrete_values(user_id, &values).is_ok();
        if saved {
            self.selected = Some(user_id);
            self.scroll_to_selected = true;
            self.editing = Some(EditRow::new(user_id, family, name, b_mm, h_mm, d_mm));
            self.reload = true;
        }
    }

    /// Removes the selected item from the library.
    fn remove_row(&mut self, data: &LibraryData) {
        let Some(id) = self.selected else { return };
        if self.editing.as_ref().is_some_and(|e| e.user_id != id) {
            self.commit_edit(data);
        }
        if data.lib.delete_item(LibraryKind::Section, id).is_ok() {
            self.editing = None;
            self.selected = None;
            self.details = None;
            self.reload = true;
        }
    }

    /// The item and property values implied by the edit currently in progress, if any.
    fn edited_item(&self) -> Option<(LibraryItem, Vec<LibraryValue>)> {
        let edit = self.editing.as_ref()?;
        let (b_mm, h_mm, d_mm) = edit.dims();
        let name = if edit.name.is_empty() {
            concrete_name(&edit.family, b_mm, h_mm, d_mm)
        } else {
            edit.name.clone()
        };
        let item = LibraryItem {
            kind: LibraryKind::Section,
            user_id: edit.user_id,
            name: name.clone(),
            designation: name,
            standard: String::new(),
            manufacturer: String::new(),
            region: self.tree.region.clone(),
            family: edit.family.clone(),
            category: "concrete".to_owned(),
            rfem_library_id: None,
            rfem_version: String::new(),
        };
        Some((item, library_values(&edit.family, b_mm, h_mm, d_mm)))
    }

    /// Writes the row being edited's current name and dimensions through to the library:
    /// both its lightweight index entry and its real RFEM property values (what a later pick
    /// actually assigns to a model object).
    fn commit_edit(&mut self, data: &LibraryData) {
        if let Some((item, values)) = self.edited_item() {
            let _ = data.lib.update_item(&item, &values);
            let _ = data.lib.set_concrete_values(item.user_id, &values);
        }
    }

    /// Items passing the tree and search filters, sorted.
    fn visible<'a>(&self, data: &'a LibraryData) -> Vec<&'a LibraryItem> {
        let words: Vec<String> = self
            .search
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let t = &self.tree;
        let mut out: Vec<&LibraryItem> = data
            .items
            .iter()
            .filter(|i| {
                (t.region.is_empty() || i.region == t.region)
                    && (t.category.is_empty() || i.category == t.category)
                    && (t.family.is_empty() || i.family == t.family)
            })
            .filter(|i| {
                let name = i.name.to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            })
            .collect();
        if let Some((c, asc)) = self.sort {
            let value = |i: &LibraryItem| {
                data.values
                    .get(&i.user_id)
                    .and_then(|v| v.get(c - 3).copied().flatten())
            };
            out.sort_by(|a, b| {
                let o = match c {
                    0 => natural(&a.designation, &b.designation),
                    1 => a.standard.cmp(&b.standard),
                    2 => a.manufacturer.cmp(&b.manufacturer),
                    _ => value(a)
                        .unwrap_or(f64::NAN)
                        .total_cmp(&value(b).unwrap_or(f64::NAN)),
                };
                if asc { o } else { o.reverse() }
            });
        }
        out
    }

    fn tree(&mut self, ui: &mut egui::Ui, data: &LibraryData) {
        // region -> category -> family -> count
        let mut tree: BTreeMap<&str, BTreeMap<&str, BTreeMap<&str, usize>>> = BTreeMap::new();
        for i in &data.items {
            *tree
                .entry(i.region.as_str())
                .or_default()
                .entry(i.category.as_str())
                .or_default()
                .entry(i.family.as_str())
                .or_default() += 1;
        }
        let mut pick = |ui: &mut egui::Ui, p: TreePick, text: String| {
            if ui.selectable_label(self.tree == p, text).clicked() {
                self.tree = p;
                self.scroll_to_selected = false;
            }
        };
        egui::ScrollArea::vertical()
            .id_salt("library_tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                pick(
                    ui,
                    TreePick::default(),
                    format!("All ({})", data.items.len()),
                );
                // South Africa first.
                let mut regions: Vec<_> = tree.iter().collect();
                regions.sort_by_key(|(r, _)| (**r != "ZA", **r));
                for (region, cats) in regions {
                    let n: usize = cats.values().flat_map(|f| f.values()).sum();
                    egui::CollapsingHeader::new(format!("{} ({n})", region_label(region)))
                        .id_salt(("library_region", *region))
                        .default_open(true)
                        .show(ui, |ui| {
                            let all = TreePick {
                                region: region.to_string(),
                                ..Default::default()
                            };
                            pick(ui, all, "All".into());
                            for (cat, fams) in cats {
                                let n: usize = fams.values().sum();
                                egui::CollapsingHeader::new(format!(
                                    "{} ({n})",
                                    category_label(cat)
                                ))
                                .id_salt(("library_category", *region, *cat))
                                .show(ui, |ui| {
                                    let all = TreePick {
                                        region: region.to_string(),
                                        category: cat.to_string(),
                                        family: String::new(),
                                    };
                                    pick(ui, all, "All".into());
                                    for (fam, n) in fams {
                                        let p = TreePick {
                                            region: region.to_string(),
                                            category: cat.to_string(),
                                            family: fam.to_string(),
                                        };
                                        pick(ui, p, format!("{fam} ({n})"));
                                    }
                                });
                            }
                        });
                }
            });
    }

    /// The item table; returns an item picked with a double-click.
    fn table(
        &mut self,
        ui: &mut egui::Ui,
        data: &LibraryData,
        visible: &[&LibraryItem],
    ) -> Option<LibraryItem> {
        let value_cols = cols(self.kind);
        // Value columns some visible item has.
        // A square has one parameter (b) and a circle one (d): when every visible item is of
        // those shapes, the columns for dimensions they lack are left out.
        let all_in = |fams: &[&str]| {
            !visible.is_empty() && visible.iter().all(|i| fams.contains(&i.family.as_str()))
        };
        let hide_h = all_in(&["SQ", "CIRCLE"]);
        let hide_b = all_in(&["CIRCLE"]);
        let shown: Vec<usize> = (0..value_cols.len())
            .filter(|c| !(hide_h && value_cols[*c].key == "h"))
            .filter(|c| !(hide_b && value_cols[*c].key == "b"))
            .filter(|c| {
                visible
                    .iter()
                    .any(|i| data.values.get(&i.user_id).is_some_and(|v| v[*c].is_some()))
            })
            .collect();
        let mut picked = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(("library_table", self.title()))
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .sense(egui::Sense::click())
            .column(Column::initial(230.0).at_least(120.0).clip(true))
            .column(Column::initial(120.0).at_least(60.0).clip(true))
            .column(Column::initial(110.0).at_least(60.0).clip(true))
            .columns(Column::initial(72.0).at_least(45.0), shown.len());
        if self.scroll_to_selected
            && let Some(row) = self
                .selected
                .and_then(|id| visible.iter().position(|i| i.user_id == id))
        {
            table = table.scroll_to_row(row, Some(egui::Align::Center));
            self.scroll_to_selected = false;
        }
        let mut sort = self.sort;
        table
            .header(22.0, |mut header| {
                let mut heading = |header: &mut egui_extras::TableRow, c: usize, text: &str| {
                    header.col(|ui| {
                        let arrow = match sort {
                            Some((s, true)) if s == c => " ⏶",
                            Some((s, false)) if s == c => " ⏷",
                            _ => "",
                        };
                        let r = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(format!("{text}{arrow}")).strong(),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Click to sort");
                        if r.clicked() {
                            sort = match sort {
                                Some((s, true)) if s == c => Some((c, false)),
                                Some((s, false)) if s == c => None,
                                _ => Some((c, true)),
                            };
                        }
                    });
                };
                heading(&mut header, 0, "Name");
                heading(&mut header, 1, "Standard");
                heading(&mut header, 2, "Manufacturer");
                for c in &shown {
                    heading(&mut header, c + 3, value_cols[*c].label);
                }
            })
            .body(|body| {
                body.rows(20.0, visible.len(), |mut row| {
                    let item = visible[row.index()];
                    row.set_selected(self.selected == Some(item.user_id));
                    let cell = |ui: &mut egui::Ui, text: &str| {
                        ui.add(egui::Label::new(text).selectable(false))
                    };
                    // This row's in-place edit, if any; `computed` is its live section
                    // properties (A, I_y, ...) from its (possibly just-typed) dimensions.
                    let mut editing = self.editing.as_mut().filter(|e| e.user_id == item.user_id);
                    let computed = editing.as_deref().map(|e| {
                        let (b, h, d) = e.dims();
                        concrete_properties(&e.family, b, h, d)
                    });
                    row.col(|ui| {
                        if let Some(edit) = editing.as_deref_mut() {
                            ui.text_edit_singleline(&mut edit.name);
                        } else {
                            cell(ui, &item.designation).on_hover_text(&item.name);
                        }
                    });
                    row.col(|ui| {
                        cell(ui, &item.standard);
                    });
                    row.col(|ui| {
                        cell(ui, &item.manufacturer);
                    });
                    let values = data.values.get(&item.user_id);
                    for c in &shown {
                        let key = value_cols[*c].key;
                        row.col(|ui| {
                            // Which dimension column is editable depends on the shape: both
                            // for a rectangle, only "b" for a square (its "h" stays a greyed
                            // read-only mirror) and only "d" for a circle (no b / h).
                            let dim_field = editing.as_deref_mut().and_then(|edit| {
                                match (edit.family.as_str(), key) {
                                    ("R", "b") => Some(&mut edit.b),
                                    ("R", "h") => Some(&mut edit.h),
                                    ("SQ", "b") => Some(&mut edit.b),
                                    ("CIRCLE", "d") => Some(&mut edit.d),
                                    _ => None,
                                }
                            });
                            if let Some(field) = dim_field {
                                ui.text_edit_singleline(field);
                                return;
                            }
                            // A circle has no b / h (mixed views leave the cells blank).
                            if item.family == "CIRCLE" && (key == "b" || key == "h") {
                                return;
                            }
                            let mirror = item.family == "SQ" && key == "h";
                            let v = computed
                                .as_ref()
                                .and_then(|p| p.iter().find(|(k, _)| *k == key))
                                .map(|(_, v)| *v)
                                .or_else(|| values.and_then(|v| v[*c]));
                            if let Some(v) = v {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let mut text = egui::RichText::new(number(
                                            v * value_cols[*c].scale,
                                        ));
                                        if mirror {
                                            text = text.weak();
                                        }
                                        ui.add(egui::Label::new(text).selectable(false))
                                    },
                                );
                            }
                        });
                    }
                    let r = row.response();
                    if r.clicked() {
                        if self.editing.as_ref().is_some_and(|e| e.user_id != item.user_id) {
                            self.commit_edit(data);
                            self.editing = None;
                            self.reload = true;
                        }
                        self.selected = Some(item.user_id);
                    }
                    if r.double_clicked() {
                        picked = Some(item.clone());
                    }
                });
            });
        self.sort = sort;
        picked
    }

    fn details(&mut self, ui: &mut egui::Ui, data: &LibraryData) {
        let Some(item) = self
            .selected
            .and_then(|id| data.items.iter().find(|i| i.user_id == id))
        else {
            ui.weak("Select an item to see its values.");
            return;
        };
        if self.details.as_ref().map(|d| d.0) != Some(item.user_id) {
            let values = data.lib.values(item).map_err(|e| e.to_string());
            self.details = Some((item.user_id, values));
        }
        ui.strong(&item.name);
        egui::ScrollArea::vertical()
            .id_salt("library_details")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("library_item")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        let mut row = |k: &str, v: String| {
                            ui.weak(k);
                            ui.label(v);
                            ui.end_row();
                        };
                        row("Designation", item.designation.clone());
                        row("Standard", item.standard.clone());
                        row("Manufacturer", item.manufacturer.clone());
                        row("Region", region_label(&item.region).to_owned());
                        row("Category", category_label(&item.category).to_owned());
                        row(
                            "RFEM library id",
                            item.rfem_library_id
                                .map_or_else(String::new, |i| i.to_string()),
                        );
                        row("From RFEM", item.rfem_version.clone());
                    });
                ui.add_space(6.0);
                ui.label(egui::RichText::new("RFEM values (SI units)").strong());
                match &self.details.as_ref().unwrap().1 {
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Ok(values) => {
                        egui::Grid::new("library_values")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for v in values {
                                    ui.monospace(&v.key);
                                    let text = match (v.value_si, &v.value_text) {
                                        (Some(x), _) => number(x),
                                        (None, Some(t)) => t.clone(),
                                        _ => String::new(),
                                    };
                                    match v.thickness_si {
                                        Some(t) => ui
                                            .label(format!("{text}  (t ≤ {} mm)", number(t * 1e3))),
                                        None => ui.label(text),
                                    };
                                    ui.end_row();
                                }
                            });
                    }
                }
            });
    }
}

/// Compact number text: 4 significant digits, scientific notation for very large / small.
fn number(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let a = x.abs();
    if !(1e-3..1e7).contains(&a) {
        return format!("{x:.4e}");
    }
    let decimals = (3 - a.log10().floor() as i32).clamp(0, 6) as usize;
    let s = format!("{x:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// Compares designations with their numbers as numbers (`UB 203x133x25` < `UB 1016x305x222`).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        let mut text = String::new();
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() || (c == '.' && !num.is_empty()) {
                num.push(c);
            } else {
                if !num.is_empty() {
                    out.push((std::mem::take(&mut text), num.parse().unwrap_or(0.0)));
                    num.clear();
                }
                text.push(c);
            }
        }
        out.push((text, num.parse().unwrap_or(-1.0)));
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(&pb) {
        let o = x.0.cmp(&y.0).then(x.1.total_cmp(&y.1));
        if o.is_ne() {
            return o;
        }
    }
    pa.len().cmp(&pb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number(203.2), "203.2");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(32.0), "32");
        assert_eq!(number(1.943e-5), "1.9430e-5");
        assert_eq!(number(2862.0), "2862");
    }

    #[test]
    fn natural_order() {
        use std::cmp::Ordering::Less;
        assert_eq!(natural("UB 203x133x25", "UB 1016x305x222"), Less);
        assert_eq!(natural("CHS 48.4x2.5", "CHS 101.6x3.5"), Less);
        assert_eq!(natural("IPE 80", "IPE 100"), Less);
    }
}
