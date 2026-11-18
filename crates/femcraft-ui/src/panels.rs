//! Model database views: the navigator tree and the object tables.
//!
//! Both are read-only views of the [`Model`] for now; clicking an object selects it
//! (Ctrl+click adds / removes), sharing the selection with the viewport.

use std::collections::HashSet;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use femcraft_model::{FieldType, Group, Model, Object, ObjectId, ObjectKind, Value};
use femcraft_rfdb::LibraryKind;

use crate::library_dialog::{LibraryCache, LibraryDialog, Outcome};

/// Objects listed per kind in the tree before "… more" (the tables show everything).
const MAX_TREE_ITEMS: usize = 500;

fn group_label(g: Group) -> &'static str {
    match g {
        Group::BasicObjects => "Basic Objects",
        Group::Supports => "Supports",
        Group::LoadCases => "Load Cases",
        Group::Loads => "Loads",
    }
}

/// Short text for one object: number plus name or type when it has one.
fn item_label(id: ObjectId, obj: &Object) -> String {
    let base = format!("{} {}", singular(id.kind), id.no);
    match (obj.get("name"), obj.get("type")) {
        (Value::Text(n), _) if !n.is_empty() => format!("{base} · {n}"),
        (_, Value::Text(t)) if !t.is_empty() => {
            let t = t
                .trim_start_matches("TYPE_")
                .replace('_', " ")
                .to_lowercase();
            format!("{base} · {t}")
        }
        _ => base,
    }
}

/// "Nodal Supports" -> "Nodal Support", "Thicknesses" -> "Thickness".
fn singular(kind: ObjectKind) -> String {
    let l = kind.spec().label;
    if let Some(s) = l.strip_suffix("sses") {
        format!("{s}ss")
    } else if let Some(s) = l.strip_suffix("ies") {
        format!("{s}y")
    } else {
        l.strip_suffix('s').unwrap_or(l).to_owned()
    }
}

/// Click handling shared by tree and tables: plain click selects only this object,
/// Ctrl+click toggles it.
fn click_select(ctrl: bool, selection: &mut HashSet<ObjectId>, id: ObjectId) {
    if ctrl {
        if !selection.remove(&id) {
            selection.insert(id);
        }
    } else {
        selection.clear();
        selection.insert(id);
    }
}

fn kind_items(
    ui: &mut egui::Ui,
    model: &Model,
    kind: ObjectKind,
    load_case: Option<u32>,
    selection: &mut HashSet<ObjectId>,
) {
    let items: Vec<(ObjectId, &Object)> = model
        .iter_kind(kind)
        .filter(|(id, _)| load_case.is_none_or(|lc| id.load_case == lc))
        .collect();
    if items.is_empty() {
        return;
    }
    let header = format!("{} ({})", kind.spec().label, items.len());
    egui::CollapsingHeader::new(header)
        .id_salt((kind, load_case))
        .show(ui, |ui| {
            for (id, obj) in items.iter().take(MAX_TREE_ITEMS) {
                let selected = selection.contains(id);
                if ui
                    .selectable_label(selected, item_label(*id, obj))
                    .clicked()
                {
                    click_select(ui.input(|i| i.modifiers.ctrl), selection, *id);
                }
            }
            if items.len() > MAX_TREE_ITEMS {
                ui.weak(format!(
                    "… {} more (see the table)",
                    items.len() - MAX_TREE_ITEMS
                ));
            }
        });
}

/// Tree explorer: groups, object kinds and objects; loads grouped by load case.
pub fn navigator(
    ui: &mut egui::Ui,
    model: Option<&Model>,
    load_cases: &[(u32, String)],
    selection: &mut HashSet<ObjectId>,
) {
    let Some(model) = model else {
        ui.weak("No model open.");
        return;
    };
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in [Group::BasicObjects, Group::Supports, Group::LoadCases] {
                let kinds: Vec<ObjectKind> = ObjectKind::ALL
                    .iter()
                    .copied()
                    .filter(|k| k.spec().group == group && model.count(*k) > 0)
                    .collect();
                if kinds.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(group_label(group))
                    .default_open(group == Group::BasicObjects)
                    .show(ui, |ui| {
                        for kind in kinds {
                            kind_items(ui, model, kind, None, selection);
                        }
                    });
            }
            let load_kinds: Vec<ObjectKind> = ObjectKind::ALL
                .iter()
                .copied()
                .filter(|k| k.spec().is_load() && model.count(*k) > 0)
                .collect();
            if !load_kinds.is_empty() {
                egui::CollapsingHeader::new(group_label(Group::Loads)).show(ui, |ui| {
                    for (lc, name) in load_cases {
                        let n: usize = load_kinds
                            .iter()
                            .map(|k| {
                                model
                                    .iter_kind(*k)
                                    .filter(|(id, _)| id.load_case == *lc)
                                    .count()
                            })
                            .sum();
                        if n == 0 {
                            continue;
                        }
                        egui::CollapsingHeader::new(format!("LC{lc} {name} ({n})"))
                            .id_salt(("lc", *lc))
                            .show(ui, |ui| {
                                for kind in &load_kinds {
                                    kind_items(ui, model, *kind, Some(*lc), selection);
                                }
                            });
                    }
                });
            }
        });
}

/// Which table is shown, and the cell being edited.
pub struct TableState {
    pub kind: ObjectKind,
    /// Cell being edited: (document id, object, field key, text typed so far).
    editing: Option<(u64, ObjectId, &'static str, String)>,
    /// The editor was just opened (focus it once).
    focus_editor: bool,
    /// Why the last edit was refused.
    error: Option<String>,
    /// Information about the last action (not an error).
    notice: Option<String>,
    /// The library dialog, while open.
    library: Option<LibraryDialog>,
    /// Libraries loaded by the dialog (kept for the session).
    library_cache: LibraryCache,
    /// A library item picked for (document id, object), applied by the caller.
    pick: Option<(u64, ObjectId, femcraft_rfdb::LibraryItem)>,
}

impl TableState {
    /// The library item picked in the dialog, to assign to the object
    /// ([`femcraft_rfdb::RfDocument::assign_library_item`]).
    pub fn take_pick(&mut self) -> Option<(u64, ObjectId, femcraft_rfdb::LibraryItem)> {
        self.pick.take()
    }

    /// Reports how applying a pick went.
    pub fn report(&mut self, result: Result<String, String>) {
        match result {
            Ok(note) => {
                self.notice = Some(note);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                self.notice = None;
            }
        }
    }
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            kind: ObjectKind::Node,
            editing: None,
            focus_editor: false,
            error: None,
            notice: None,
            library: None,
            library_cache: LibraryCache::default(),
            pick: None,
        }
    }
}

/// Fields that can be edited in the tables (a first, temporary set). Every field of the
/// support kinds is editable.
pub fn is_editable(kind: ObjectKind, key: &str) -> bool {
    use ObjectKind as K;
    if key == "comment" {
        return true;
    }
    match kind {
        K::Thickness => matches!(key, "material" | "uniform_thickness"),
        K::Node => matches!(key, "coordinate_1" | "coordinate_2" | "coordinate_3"),
        K::Line => key == "definition_nodes",
        K::Member => matches!(
            key,
            "line" | "section_start" | "section_end" | "rotation_angle"
        ),
        K::Surface => matches!(
            key,
            "boundary_lines" | "thickness" | "axes_rotation" | "axes_z_reversed"
        ),
        K::NodalSupport | K::LineSupport | K::MemberSupport | K::SurfaceSupport => true,
        K::LoadCase => matches!(key, "name" | "self_weight_active" | "to_solve"),
        K::NodalLoad if key.starts_with("components_") || key == "load_direction" => true,
        K::NodalLoad | K::LineLoad | K::MemberLoad | K::SurfaceLoad => matches!(
            key,
            "nodes"
                | "lines"
                | "members"
                | "surfaces"
                | "magnitude"
                | "magnitude_2"
                | "uniform_magnitude"
        ),
        _ => false,
    }
}

/// Tables ending in an empty row: typing into it adds an object (saved as a copy of a similar
/// object, see `femcraft_rfdb`), loads going to the load case shown in the 3D view.
pub const NEW_ROW_KINDS: [ObjectKind; 9] = [
    ObjectKind::Node,
    ObjectKind::Line,
    ObjectKind::Member,
    ObjectKind::Surface,
    ObjectKind::LoadCase,
    ObjectKind::NodalLoad,
    ObjectKind::LineLoad,
    ObjectKind::MemberLoad,
    ObjectKind::SurfaceLoad,
];

/// Adds object `id` with starting values (node at the origin, load case without self-weight).
fn add_object(model: &mut Model, id: ObjectId) -> Result<(), String> {
    let obj = model.add(id).map_err(|e| e.to_string())?;
    // Every editable field present (empty), so all its cells stay editable.
    for f in id.kind.spec().fields {
        if is_editable(id.kind, f.key) {
            obj.values.insert(f.key, Value::Null);
        }
    }
    let defaults: &[(&str, Value)] = match id.kind {
        ObjectKind::Node => &[
            ("coordinate_1", Value::Float(0.0)),
            ("coordinate_2", Value::Float(0.0)),
            ("coordinate_3", Value::Float(0.0)),
        ],
        ObjectKind::LoadCase => &[("self_weight_active", Value::Bool(false))],
        ObjectKind::NodalLoad => &[("load_direction", Value::Int(2))],
        // As RFEM's default member (and the copy saved for it): a beam, uniform section.
        ObjectKind::Member => &[
            ("type", Value::Text("TYPE_BEAM".into())),
            ("section_distribution", Value::Int(0)),
        ],
        _ => &[],
    };
    for (k, v) in defaults {
        model.set(id, k, v.clone()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Text put in the editor for a value (full precision, lists in RFEM notation).
fn edit_text(v: &Value) -> String {
    match v {
        Value::Float(f) if f.is_infinite() => {
            if *f > 0.0 {
                "inf".into()
            } else {
                "-inf".into()
            }
        }
        Value::Float(f) => f.to_string(),
        other => other.to_string(),
    }
}

/// Reads typed text as a value of the field's type; references must name existing objects.
fn parse_cell(model: &Model, ty: FieldType, text: &str) -> Result<Value, String> {
    let t = text.trim();
    let exists = |kind: ObjectKind, no: u32| -> Result<(), String> {
        if model.get(ObjectId::new(kind, no)).is_some() {
            Ok(())
        } else {
            Err(format!("{} {no} does not exist", singular(kind)))
        }
    };
    Ok(match ty {
        FieldType::Float(_) => match t.to_ascii_lowercase().as_str() {
            "" => return Err("a number is required".into()),
            "inf" | "∞" | "+inf" => Value::Float(f64::INFINITY),
            "-inf" | "-" => Value::Float(f64::NEG_INFINITY),
            s => Value::Float(
                s.replace(',', ".")
                    .parse()
                    .map_err(|_| format!("'{t}' is not a number"))?,
            ),
        },
        FieldType::Int => Value::Int(
            t.parse()
                .map_err(|_| format!("'{t}' is not a whole number"))?,
        ),
        FieldType::Enum => match t.parse::<i64>() {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Text(t.to_owned()),
        },
        FieldType::Text => Value::Text(t.to_owned()),
        FieldType::Bool => Value::Bool(matches!(
            t.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "✔"
        )),
        FieldType::Ref(kind) if t.is_empty() => {
            let _ = kind;
            Value::Null
        }
        FieldType::Ref(kind) => {
            let no: u32 = t
                .parse()
                .map_err(|_| format!("'{t}' is not an object number"))?;
            exists(kind, no)?;
            Value::Ref(no)
        }
        FieldType::RefList(kind) => {
            let list = femcraft_model::list::parse_list(t)
                .ok_or_else(|| format!("'{t}' is not a list such as 1-4,7"))?;
            for no in &list {
                exists(kind, *no)?;
            }
            Value::RefList(list)
        }
    })
}

/// Fields edited with a dropdown: (stored code, name shown).
fn choices(kind: ObjectKind, key: &str) -> Option<&'static [(i64, &'static str)]> {
    match (kind, key) {
        // `loadDirectionAxis` in RFEM files: 0 / 1 / 2 = global X / Y / Z.
        (ObjectKind::NodalLoad, "load_direction") => Some(&[(0, "X"), (1, "Y"), (2, "Z")]),
        _ => None,
    }
}

/// Cell text, with RFEM's names for member type and cross-section distribution.
fn display_text(kind: ObjectKind, key: &str, v: &Value, ty: FieldType) -> String {
    if let (Some(options), Value::Int(i)) = (choices(kind, key), v)
        && let Some((_, name)) = options.iter().find(|(c, _)| c == i)
    {
        return (*name).to_owned();
    }
    match (kind, key, v) {
        (ObjectKind::Member, "type", Value::Text(t)) => {
            let name = t.strip_prefix("TYPE_").unwrap_or(t).replace('_', " ");
            let mut c = name.chars();
            c.next()
                .map(|f| f.to_string() + &c.as_str().to_lowercase())
                .unwrap_or_default()
        }
        (ObjectKind::Member, "section_distribution", Value::Int(0)) => "Uniform".into(),
        _ => cell_text(v, ty),
    }
}

fn cell_text(v: &Value, ty: FieldType) -> String {
    match (v, ty) {
        (Value::Float(f), _) if f.is_infinite() => {
            if *f > 0.0 {
                "∞".into()
            } else {
                "-".into()
            }
        }
        (Value::Float(f), _) => format!("{f:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned(),
        (Value::Bool(b), _) => {
            if *b {
                "✔".into()
            } else {
                String::new()
            }
        }
        _ => v.to_string(),
    }
}

/// Object tables: one tab per kind present in the model, columns from the field descriptions.
/// Editable cells (coloured headers) are edited with a double-click. `doc` identifies the
/// model. Returns true when the model was changed.
pub fn tables(
    ui: &mut egui::Ui,
    doc: u64,
    model: Option<&mut Model>,
    load_case: u32,
    state: &mut TableState,
    selection: &mut HashSet<ObjectId>,
) -> bool {
    let Some(model) = model else {
        ui.weak("No model open.");
        return false;
    };
    let kinds: Vec<ObjectKind> = ObjectKind::ALL
        .iter()
        .copied()
        .filter(|k| model.count(*k) > 0 || NEW_ROW_KINDS.contains(k))
        .collect();
    if kinds.is_empty() {
        ui.weak("The model has no objects.");
        return false;
    }
    if !kinds.contains(&state.kind) {
        state.kind = kinds[0];
    }
    // An editor left open on another model or table is dropped.
    if state
        .editing
        .as_ref()
        .is_some_and(|(d, id, ..)| *d != doc || id.kind != state.kind)
    {
        state.editing = None;
    }
    // Sheet tabs along the bottom (wrapping onto a second row instead of scrolling), the table
    // filling the space above them.
    egui::Panel::bottom("table_sheet_tabs")
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 4)))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for k in &kinds {
                    ui.selectable_value(&mut state.kind, *k, k.spec().label);
                }
            });
        });
    // The table fills the inner central panel: exactly the space above the sheet tabs.
    let mut edit: Option<(ObjectId, &'static str, Result<Value, String>)> = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            if let Some(err) = &state.error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            } else if let Some(note) = &state.notice {
                ui.label(note);
            }
            edit = object_table(ui, doc, model, load_case, state, selection);
        });
    library_dialog(ui.ctx(), doc, state);

    // Apply an edit after drawing (the rows borrow the model while they are drawn). An edit in
    // the empty row adds the object first.
    let Some((id, key, value)) = edit else {
        return false;
    };
    let result = value.and_then(|v| {
        if model.get(id).is_none() {
            add_object(model, id)?;
        }
        model.set(id, key, v).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            state.error = None;
            true
        }
        Err(e) => {
            state.error = Some(format!("{} {}: {e}", singular(id.kind), id.no));
            false
        }
    }
}

/// Library behind the Name column's "…" button.
fn library_kind(kind: ObjectKind) -> Option<LibraryKind> {
    match kind {
        ObjectKind::Material => Some(LibraryKind::Material),
        ObjectKind::Section => Some(LibraryKind::Section),
        _ => None,
    }
}

/// Shows the open library dialog and handles its outcome.
fn library_dialog(ctx: &egui::Context, doc: u64, state: &mut TableState) {
    let Some(dialog) = &mut state.library else {
        return;
    };
    match dialog.show(ctx, &mut state.library_cache) {
        Outcome::Open => {}
        Outcome::Cancelled => state.library = None,
        Outcome::Picked(item) => {
            let (d, id) = dialog.target;
            if d == doc {
                state.pick = Some((d, id, item));
            }
            state.library = None;
        }
    }
}

/// The rows of one object kind; clicking a row selects the object, double-clicking an editable
/// cell edits it. Returns an edit to apply: (object, field, parsed value or error).
fn object_table(
    ui: &mut egui::Ui,
    doc: u64,
    model: &Model,
    load_case: u32,
    state: &mut TableState,
    selection: &mut HashSet<ObjectId>,
) -> Option<(ObjectId, &'static str, Result<Value, String>)> {
    let kind = state.kind;
    let spec = kind.spec();
    let is_load = spec.is_load() && kind != ObjectKind::LoadCase;
    let mut rows: Vec<(ObjectId, &Object)> = model.iter_kind(kind).collect();
    // The empty row: the next free number (in the shown load case for loads).
    let empty = Object::default();
    let new_id = NEW_ROW_KINDS.contains(&kind).then(|| {
        let lc = if is_load { load_case } else { 0 };
        let no = model.next_no(kind, lc);
        if is_load {
            ObjectId::load(kind, lc, no)
        } else {
            ObjectId::new(kind, no)
        }
    });
    if let Some(id) = new_id {
        rows.push((id, &empty));
    }
    let mut edit = None;
    let editable_color = ui.visuals().hyperlink_color;
    // The body scrolls within the space left above the sheet tabs (TableBuilder would
    // otherwise grow up to its default 800 px and run underneath them). Leave room for the
    // header row and the horizontal scroll bar.
    let body_height = (ui.available_height() - 22.0 - 14.0).max(40.0);
    egui::ScrollArea::horizontal()
        .id_salt(("table", kind))
        .show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt(("table", kind))
                .max_scroll_height(body_height)
                .min_scrolled_height(body_height)
                .auto_shrink([false, true])
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .column(Column::initial(55.0).at_least(40.0));
            if is_load {
                table = table.column(Column::initial(45.0).at_least(35.0));
            }
            table
                .columns(
                    Column::initial(110.0).at_least(40.0).clip(true),
                    spec.fields.len(),
                )
                .header(22.0, |mut header| {
                    header.col(|ui| {
                        ui.strong("No.");
                    });
                    if is_load {
                        header.col(|ui| {
                            ui.strong("LC");
                        });
                    }
                    for f in spec.fields {
                        header.col(|ui| {
                            let text = match f.ty {
                                FieldType::Float(u) if !u.is_empty() => {
                                    format!("{} [{u}]", f.label)
                                }
                                _ => f.label.to_owned(),
                            };
                            if is_editable(kind, f.key) {
                                ui.label(egui::RichText::new(text).strong().color(editable_color))
                                    .on_hover_text(format!(
                                        "{} - double-click a cell to edit (Enter to apply, Esc to cancel)",
                                        f.key
                                    ));
                            } else {
                                ui.strong(text).on_hover_text(f.key);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let (id, obj) = rows[row.index()];
                        let is_new = Some(id) == new_id;
                        row.set_selected(selection.contains(&id));
                        row.col(|ui| {
                            if is_new {
                                ui.weak(id.no.to_string())
                                    .on_hover_text("New row: type in a blue column to add");
                            } else {
                                ui.label(id.no.to_string());
                            }
                        });
                        if is_load {
                            row.col(|ui| {
                                ui.label(id.load_case.to_string());
                            });
                        }
                        for f in spec.fields {
                            row.col(|ui| {
                                let value = obj.get(f.key);
                                // Only fields the file stores for this object (e.g. a surface
                                // without thickness has no thickness to edit).
                                let editable = is_editable(kind, f.key)
                                    && (is_new || obj.values.contains_key(f.key));
                                // A fixed set of options: a dropdown, applied at once.
                                if editable && let Some(options) = choices(kind, f.key) {
                                    let current = value.as_i64();
                                    egui::ComboBox::from_id_salt(("choice", doc, id, f.key))
                                        .selected_text(display_text(kind, f.key, value, f.ty))
                                        .width(ui.available_width())
                                        .show_ui(ui, |ui| {
                                            for (code, name) in options {
                                                if ui
                                                    .selectable_label(current == Some(*code), *name)
                                                    .clicked()
                                                    && current != Some(*code)
                                                {
                                                    edit = Some((id, f.key, Ok(Value::Int(*code))));
                                                }
                                            }
                                        });
                                    return;
                                }
                                // True / false: a checkbox, applied at once.
                                if editable && f.ty == FieldType::Bool {
                                    let mut b = value.as_bool().unwrap_or(false);
                                    if ui.checkbox(&mut b, "").changed() {
                                        edit = Some((id, f.key, Ok(Value::Bool(b))));
                                    }
                                    return;
                                }
                                let this_cell = state
                                    .editing
                                    .as_ref()
                                    .is_some_and(|(_, e, k, _)| *e == id && *k == f.key);
                                if this_cell {
                                    let (enter, escape) = ui.input(|i| {
                                        (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
                                    });
                                    let text = &mut state.editing.as_mut().unwrap().3;
                                    let r = ui.add(
                                        egui::TextEdit::singleline(text)
                                            .desired_width(f32::INFINITY),
                                    );
                                    if state.focus_editor {
                                        r.request_focus();
                                        state.focus_editor = false;
                                    }
                                    if escape {
                                        state.editing = None;
                                    } else if r.lost_focus() || enter {
                                        let typed = state.editing.take().unwrap().3;
                                        edit = Some((id, f.key, parse_cell(model, f.ty, &typed)));
                                    }
                                    return;
                                }
                                // Library items: RFEM stores no name; show the library's.
                                let lib_name = (f.key == "name"
                                    && value.as_str().is_none_or(str::is_empty))
                                .then(|| {
                                    let lib = library_kind(kind)?;
                                    let key = match lib {
                                        LibraryKind::Material => "database_material_id",
                                        LibraryKind::Section => "database_section_id",
                                    };
                                    let rfem_id = obj.get(key).as_i64().filter(|i| *i > 0)?;
                                    state.library_cache.name(lib, rfem_id).or_else(|| {
                                        // Not in our library under that id: the same profile
                                        // from another manufacturer - match by properties.
                                        (lib == LibraryKind::Section)
                                            .then(|| {
                                                state.library_cache.section_name_by_properties(
                                                    obj.get("A").as_f64()?,
                                                    obj.get("I_y").as_f64()?,
                                                    obj.get("I_z").as_f64()?,
                                                )
                                            })
                                            .flatten()
                                    })
                                })
                                .flatten()
                                .or_else(|| {
                                    (f.key == "name" && value.as_str().is_none_or(str::is_empty))
                                        .then(|| parametric_section_name(kind, obj))
                                        .flatten()
                                });
                                let label = match &lib_name {
                                    Some(n) => ui
                                        .add(
                                            egui::Label::new(egui::RichText::new(n).italics())
                                                .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(
                                            "Library item: RFEM generates this name from its library",
                                        ),
                                    None => ui.add(
                                        egui::Label::new(display_text(kind, f.key, value, f.ty))
                                            .sense(egui::Sense::click()),
                                    ),
                                };
                                // The whole cell reacts, so empty cells can be edited too.
                                let cell = ui.interact(
                                    ui.max_rect(),
                                    ui.scope_id().with(("cell", f.key)),
                                    egui::Sense::click(),
                                );
                                let label = label.union(cell);
                                if editable && label.double_clicked() {
                                    state.editing = Some((doc, id, f.key, edit_text(value)));
                                    state.focus_editor = true;
                                } else if label.clicked() && !is_new {
                                    let ctrl = label.ctx.input(|i| i.modifiers.ctrl);
                                    click_select(ctrl, selection, id);
                                }
                                // Name of a material / section: "…" opens the library.
                                if f.key == "name"
                                    && let Some(lib) = library_kind(kind)
                                {
                                    let r = ui.max_rect();
                                    let button = egui::Rect::from_min_max(
                                        egui::pos2(r.right() - 20.0, r.top() + 1.0),
                                        egui::pos2(r.right() - 1.0, r.bottom() - 1.0),
                                    );
                                    if ui
                                        .put(button, egui::Button::new("…").small())
                                        .on_hover_text("Pick from the library")
                                        .clicked()
                                    {
                                        let current = lib_name
                                            .as_deref()
                                            .or(value.as_str())
                                            .unwrap_or("");
                                        state.library =
                                            Some(LibraryDialog::new(lib, (doc, id), current));
                                    }
                                }
                            });
                        }
                        let response = row.response();
                        if response.clicked() && !is_new {
                            let ctrl = response.ctx.input(|i| i.modifiers.ctrl);
                            click_select(ctrl, selection, id);
                        }
                    });
                });
        });
    edit
}

/// RFEM's generated name of a parametric concrete section (it stores none), e.g.
/// `R_M1 220/450`, `SQ_M1 500`, `CIRCLE_M1 500` (dimensions in mm).
fn parametric_section_name(kind: ObjectKind, obj: &femcraft_model::Object) -> Option<String> {
    if kind != ObjectKind::Section {
        return None;
    }
    let mm = |key: &str| obj.get(key).as_f64().map(|v| (v * 1e4).round() / 10.0);
    let n = |v: f64| format!("{v}");
    match obj.get("parametrization_type") {
        femcraft_model::Value::Text(shape) if shape.ends_with("__R_M1") => {
            Some(format!("R_M1 {}/{}", n(mm("b")?), n(mm("h")?)))
        }
        femcraft_model::Value::Text(shape) if shape.ends_with("__SQ_M1") => {
            Some(format!("SQ_M1 {}", n(mm("b")?)))
        }
        femcraft_model::Value::Text(shape) if shape.ends_with("__CIRCLE_M1") => {
            Some(format!("CIRCLE_M1 {}", n(mm("d")?)))
        }
        // Back-to-back double angle: name = shape + its parameters in mm (+ "/H", as RFEM
        // shows it on the two such sections seen so far).
        v if v.as_i64() == Some(168) => {
            let p = obj.get("parameters").as_str()?.to_owned();
            Some(format!("2LE(BB) {p}/H"))
        }
        _ => None,
    }
}
