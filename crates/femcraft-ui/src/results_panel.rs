//! Results navigator: a dropdown of calculated result cases and the tree of result quantities
//! available for the model, mirroring RFEM 6's Results navigator.
//!
//! Categories nest as collapsible headers; quantities are radio leaves. One result quantity is
//! displayed at a time (the [`RadioGroup::Result`] group), alongside the orthogonal surface
//! layer-side modifier ([`RadioGroup::LayerSide`]). The numeric display behind each quantity is
//! added later — this panel drives the selection; see [`femcraft_rfdb::results`].

use eframe::egui;
use femcraft_model::Model;
use femcraft_rfdb::results::result_tree;
use femcraft_rfdb::{
    CaseKind, Extremum, RadioGroup, ResultComponent, ResultLeaf, ResultNode, ResultSet,
};

/// What the user has picked in the Results panel (kept per document).
#[derive(Debug, Clone, Default)]
pub struct ResultsView {
    /// The selected case `(kind, number)`; defaults to the first case when a model is opened.
    pub selected: Option<(CaseKind, u32)>,
    /// The single result quantity chosen for display (the active [`RadioGroup::Result`] radio).
    pub active: Option<ResultComponent>,
    /// The surface layer side ([`RadioGroup::LayerSide`]); defaults to Top.
    pub layer: Option<ResultComponent>,
    /// Deformation exaggeration factor; `0.0` means "auto" (the document picks one on first use).
    pub magnification: f64,
    /// Max / Min / Both for design situations and result combinations; independent of the
    /// quantity radio buttons.
    pub extremum: Extremum,
    /// Set when the user picks a quantity; the document then switches the Results display on.
    pub picked: bool,
}

impl ResultsView {
    /// The currently selected component of a radio group.
    fn selection(&self, group: RadioGroup) -> Option<ResultComponent> {
        match group {
            RadioGroup::Result => self.active,
            RadioGroup::LayerSide => self.layer,
        }
    }

    /// Selects a component within its radio group (clears any other in that group).
    fn select(&mut self, leaf: &ResultLeaf) {
        match leaf.group {
            RadioGroup::Result => self.active = Some(leaf.component),
            RadioGroup::LayerSide => self.layer = Some(leaf.component),
        }
        self.picked = true;
    }
}

/// Draws the Results panel. `results` is `None` for an unsolved model.
pub fn results_panel(
    ui: &mut egui::Ui,
    results: Option<&ResultSet>,
    model: Option<&Model>,
    view: &mut ResultsView,
) {
    let Some(results) = results else {
        ui.add_space(8.0);
        ui.weak("No results in this model.");
        ui.weak("Solve the model in RFEM to produce results.");
        return;
    };
    if results.is_empty() {
        ui.add_space(8.0);
        ui.weak("No calculated result cases.");
        return;
    }

    // Keep the selection valid, defaulting to the first case.
    let valid = |s: (CaseKind, u32)| results.cases.iter().any(|c| (c.kind, c.no) == s);
    if !view.selected.is_some_and(valid) {
        view.selected = results.cases.first().map(|c| (c.kind, c.no));
    }
    // Surface results default to the top layer.
    view.layer.get_or_insert(ResultComponent::LayerTop);

    case_dropdown(ui, results, view);
    ui.separator();

    // Reserve space at the bottom for the deformation factor (shown for displacement results)
    // and the version line, so the scrolling tree does not cover them.
    let show_factor = model.is_some() && view.active.is_some_and(crate::scene::is_displacement);
    let has_extremes = view
        .selected
        .and_then(|(k, n)| results.cases.iter().find(|c| (c.kind, c.no) == (k, n)))
        .is_some_and(|c| c.has_extremes());
    let version = results.results_version.clone();
    let reserve = if show_factor { 36.0 } else { 0.0 } + if version.is_some() { 20.0 } else { 0.0 };
    let tree_height = (ui.available_height() - reserve).max(60.0);

    egui::ScrollArea::vertical()
        .max_height(tree_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if has_extremes {
                extremum_ui(ui, view);
            }
            result_nodes_ui(ui, model, view)
        });

    if show_factor {
        ui.separator();
        factor_row(ui, view);
    }
    if let Some(version) = version {
        ui.weak(format!("RFEM results v{version}"));
    }
}

/// The Max / Min / Both radio group for envelope cases (design situations, result combinations).
fn extremum_ui(ui: &mut egui::Ui, view: &mut ResultsView) {
    egui::CollapsingHeader::new("Extreme values")
        .id_salt("extreme_values")
        .default_open(true)
        .show(ui, |ui| {
            for e in Extremum::ALL {
                if ui.radio(view.extremum == e, e.label()).clicked() {
                    view.extremum = e;
                    view.picked = true;
                }
            }
        });
}

/// The deformation exaggeration factor, pinned at the bottom: editable value with ± steppers and
/// an Auto reset (the document then picks a fitting factor).
fn factor_row(ui: &mut egui::Ui, view: &mut ResultsView) {
    ui.horizontal(|ui| {
        ui.label("Factor");
        if ui.small_button("−").clicked() {
            view.magnification = (view.magnification.max(1.0) / 1.25).max(1.0).round();
        }
        let mut value = view.magnification.max(1.0);
        if ui
            .add(
                egui::DragValue::new(&mut value)
                    .range(1.0..=1_000_000.0)
                    .speed(1.0),
            )
            .on_hover_text("Deformation exaggeration")
            .changed()
        {
            view.magnification = value;
        }
        if ui.small_button("+").clicked() {
            view.magnification = (view.magnification.max(1.0) * 1.25).round();
        }
        if ui
            .button("Auto")
            .on_hover_text("Fit the factor to the model")
            .clicked()
        {
            view.magnification = 0.0;
        }
    });
}

/// The case dropdown at the top: cases grouped by kind (Load Cases, Load Combinations, ...).
fn case_dropdown(ui: &mut egui::Ui, results: &ResultSet, view: &mut ResultsView) {
    let selected_text = view
        .selected
        .and_then(|(k, n)| results.cases.iter().find(|c| (c.kind, c.no) == (k, n)))
        .map_or_else(|| "Select a case".to_owned(), |c| c.label());

    ui.add_space(4.0);
    egui::ComboBox::from_id_salt("results_case")
        .selected_text(selected_text)
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            let mut first = true;
            for kind in CaseKind::ALL {
                let mut cases = results.cases_of(kind).peekable();
                if cases.peek().is_none() {
                    continue;
                }
                if !first {
                    ui.separator();
                }
                first = false;
                ui.weak(kind.group_label());
                for case in cases {
                    ui.selectable_value(
                        &mut view.selected,
                        Some((case.kind, case.no)),
                        case.label(),
                    );
                }
            }
        });
}

/// The result-quantity tree, rendered recursively (the caller provides the scroll area).
fn result_nodes_ui(ui: &mut egui::Ui, model: Option<&Model>, view: &mut ResultsView) {
    let Some(model) = model else {
        ui.weak("No model open.");
        return;
    };
    let tree = result_tree(model);
    if tree.is_empty() {
        ui.weak("No result quantities for this model.");
        return;
    }
    for node in &tree {
        render_node(ui, node, view, "");
    }
}

/// Draws one tree node: a tick-box category (recursing into its children) or a radio leaf.
fn render_node(ui: &mut egui::Ui, node: &ResultNode, view: &mut ResultsView, parent: &str) {
    match node {
        ResultNode::Category {
            label,
            default_open,
            children,
        } => {
            let path = format!("{parent}/{label}");
            egui::CollapsingHeader::new(*label)
                .id_salt(&path)
                .default_open(*default_open)
                .show(ui, |ui| {
                    for child in children {
                        render_node(ui, child, view, &path);
                    }
                });
        }
        ResultNode::Leaf(leaf) => {
            let selected = view.selection(leaf.group) == Some(leaf.component);
            let label = if leaf.unit.is_empty() {
                leaf.label.to_owned()
            } else {
                format!("{}  [{}]", leaf.label, leaf.unit)
            };
            if ui.radio(selected, label).clicked() {
                view.select(leaf);
            }
        }
    }
}
