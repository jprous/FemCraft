//! Where each FemCraft field lives in RFEM's `model.db`.
//!
//! Confirmed on RFEM 6.16 (`rf6-catalog` load report): main tables use camelCase columns, the
//! object number is `userID`, references are `<name>_id` (plus `<name>_table` when the target
//! table varies) holding database ids, and loads point to their load case via `parent_id`.
//! Names marked "guess" below are still unconfirmed.
//!
//! For each field the loader tries (case-insensitive, main table then impl table): the field key,
//! its camelCase form, camelCase + `_id`, then the overrides below.

use femcraft_model::ObjectKind;

/// Columns holding the object number shown to the user.
pub const NUMBER_COLUMNS: &[&str] = &["userID", "no", "number"];
/// Primary-key column (falls back to SQLite `rowid`).
pub const ID_COLUMNS: &[&str] = &["id"];
/// Load-case reference on load tables.
pub const LOAD_CASE_COLUMNS: &[&str] = &["parent_id", "loadCase_id", "load_case"];
/// Columns that are bookkeeping, not data, in child list tables.
pub const CHILD_META_COLUMNS: &[&str] = &["id", "owner_id", "row", "row_index", "index", "order"];
pub const CHILD_ORDER_COLUMNS: &[&str] = &["row", "row_index", "index", "order", "position"];

/// Candidate table names for each kind.
pub fn tables(kind: ObjectKind) -> &'static [&'static str] {
    use ObjectKind::*;
    match kind {
        Material => &["Material"],
        Section => &["Section", "CrossSection"],
        Thickness => &["Thickness"],
        Node => &["Node"],
        Line => &["Line"],
        Member => &["Member"],
        Surface => &["Surface"],
        Solid => &["Solid"],
        NodalSupport => &["NodalSupport", "NodeSupport"],
        LineSupport => &["LineSupport"],
        MemberSupport => &["MemberSupport"],
        SurfaceSupport => &["SurfaceSupport"],
        LoadCase => &["LoadCase"],
        NodalLoad => &["NodalLoad", "NodeLoad"],
        LineLoad => &["LineLoad"],
        MemberLoad => &["MemberLoad"],
        SurfaceLoad => &["SurfaceLoad"],
        SolidLoad => &["SolidLoad"],
        FreeConcentratedLoad => &["FreeConcentratedLoad"],
        FreeLineLoad => &["FreeLineLoad"],
        FreeRectangularLoad => &["FreeRectangularLoad"],
        FreePolygonLoad => &["FreePolygonLoad"],
        ImposedNodalDeformation => &["ImposedNodalDeformation"],
        ImposedLineDeformation => &["ImposedLineDeformation"],
    }
}

/// Alternative column names tried after the field key itself.
fn overrides(kind: ObjectKind, key: &str) -> &'static [&'static str] {
    use ObjectKind::*;
    match (kind, key) {
        (Node, "coordinate_1") => &["coordinates_x"],
        (Node, "coordinate_2") => &["coordinates_y"],
        (Node, "coordinate_3") => &["coordinates_z"],
        (Member, "rotation_angle") => &["angle"],
        (LoadCase, "action_category") => &["actionCategoryId"],
        (LoadCase, "self_weight_factor_x") => &["selfWeightFactors_x"],
        (LoadCase, "self_weight_factor_y") => &["selfWeightFactors_y"],
        (LoadCase, "self_weight_factor_z") => &["selfWeightFactors_z"],
        (MemberLoad | LineLoad, "distance_a") => &["distanceADefinition_distance"],
        (MemberLoad | LineLoad, "distance_a_is_relative") => &["distanceADefinition_isRelative"],
        (MemberLoad | LineLoad, "distance_b") => &["distanceBDefinition_distance"],
        (MemberLoad | LineLoad, "distance_b_is_relative") => &["distanceBDefinition_isRelative"],
        // guess: order of the five surface support constants
        (SurfaceSupport, "translation_x") => &["springConstant_0"],
        (SurfaceSupport, "translation_y") => &["springConstant_1"],
        (SurfaceSupport, "translation_z") => &["springConstant_2"],
        (SurfaceSupport, "shear_xz") => &["springConstant_3"],
        (SurfaceSupport, "shear_yz") => &["springConstant_4"],
        (Line, "definition_nodes") => &["nodes", "items", "node"],
        (Surface, "boundary_lines") => &["lines", "items", "line"],
        (Solid, "boundary_surfaces") => &["surfaces", "items", "surface"],
        (Material, "elasticity_modulus") => &["elasticity_modulus_global", "e", "e_modulus"],
        (Material, "shear_modulus") => &["shear_modulus_global", "g", "g_modulus"],
        (Material, "poisson_ratio") => &["poisson_ratio_global", "nu"],
        (Material, "thermal_expansion_coefficient") => {
            &["thermal_expansion_coefficient_global", "alpha"]
        }
        (Thickness, "uniform_thickness") => &["thickness", "d"],
        (Member, "section_end") => &["section"],
        (Member, "section_start") => &["section"],
        (_, "name") => &["user_defined_name", "description"],
        _ => &[],
    }
}

pub fn column_candidates(kind: ObjectKind, key: &'static str) -> Vec<String> {
    let camel = camel_case(key);
    let mut v = vec![key.to_owned(), format!("{camel}_id"), camel];
    v.extend(overrides(kind, key).iter().map(|s| s.to_string()));
    v
}

/// `self_weight_active` -> `selfWeightActive`; single-letter parts such as `I_y` stay as they are.
pub fn camel_case(key: &str) -> String {
    let mut parts = key.split('_');
    let mut out = parts.next().unwrap_or("").to_owned();
    for p in parts {
        let mut c = p.chars();
        if let Some(first) = c.next() {
            out.extend(first.to_uppercase());
            out.push_str(c.as_str());
        }
    }
    out
}
