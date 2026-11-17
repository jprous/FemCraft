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
    col("h", "h [mm]", 1e3),
    col("b", "b [mm]", 1e3),
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

    fn get(&mut self, kind: LibraryKind) -> Result<Rc<LibraryData>, String> {
        let slot = match kind {
            LibraryKind::Material => &mut self.materials,
            LibraryKind::Section => &mut self.sections,
        };
        slot.get_or_insert_with(|| LibraryData::load(kind).map(Rc::new))
            .clone()
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
        let shown: Vec<usize> = (0..value_cols.len())
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
                    row.col(|ui| {
                        cell(ui, &item.designation).on_hover_text(&item.name);
                    });
                    row.col(|ui| {
                        cell(ui, &item.standard);
                    });
                    row.col(|ui| {
                        cell(ui, &item.manufacturer);
                    });
                    let values = data.values.get(&item.user_id);
                    for c in &shown {
                        row.col(|ui| {
                            if let Some(v) = values.and_then(|v| v[*c]) {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(number(v * value_cols[*c].scale))
                                                .selectable(false),
                                        )
                                    },
                                );
                            }
                        });
                    }
                    let r = row.response();
                    if r.clicked() {
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
