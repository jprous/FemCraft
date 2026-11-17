//! Model database views: the navigator tree and the object tables.
//!
//! Both are read-only views of the [`Model`] for now; clicking an object selects it
//! (Ctrl+click adds / removes), sharing the selection with the viewport.

use std::collections::HashSet;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use femcraft_model::{FieldType, Group, Model, Object, ObjectId, ObjectKind, Value};

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

/// Which table is shown.
pub struct TableState {
    pub kind: ObjectKind,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            kind: ObjectKind::Node,
        }
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
pub fn tables(
    ui: &mut egui::Ui,
    model: Option<&Model>,
    state: &mut TableState,
    selection: &mut HashSet<ObjectId>,
) {
    let Some(model) = model else {
        ui.weak("No model open.");
        return;
    };
    let kinds: Vec<ObjectKind> = ObjectKind::ALL
        .iter()
        .copied()
        .filter(|k| model.count(*k) > 0)
        .collect();
    if kinds.is_empty() {
        ui.weak("The model has no objects.");
        return;
    }
    if !kinds.contains(&state.kind) {
        state.kind = kinds[0];
    }
    egui::ScrollArea::horizontal()
        .id_salt("table_tabs")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for k in &kinds {
                    ui.selectable_value(&mut state.kind, *k, k.spec().label);
                }
            });
        });
    ui.separator();

    let spec = state.kind.spec();
    let rows: Vec<(ObjectId, &Object)> = model.iter_kind(state.kind).collect();
    let is_load = spec.is_load();
    egui::ScrollArea::horizontal()
        .id_salt(("table", state.kind))
        .show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt(("table", state.kind))
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
                            ui.strong(text).on_hover_text(f.key);
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let (id, obj) = rows[row.index()];
                        row.set_selected(selection.contains(&id));
                        row.col(|ui| {
                            ui.label(id.no.to_string());
                        });
                        if is_load {
                            row.col(|ui| {
                                ui.label(id.load_case.to_string());
                            });
                        }
                        for f in spec.fields {
                            row.col(|ui| {
                                ui.label(cell_text(obj.get(f.key), f.ty));
                            });
                        }
                        let response = row.response();
                        if response.clicked() {
                            let ctrl = response.ctx.input(|i| i.modifiers.ctrl);
                            click_select(ctrl, selection, id);
                        }
                    });
                });
        });
}
