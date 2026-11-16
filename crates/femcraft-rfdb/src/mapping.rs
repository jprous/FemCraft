//! Where each FemCraft field lives in RFEM's `model.db`.
//!
//! **All names here are best guesses until confirmed against a real file** with the
//! `rf6-catalog` tool. The loader tries each candidate in order (case-insensitive), first on the
//! main table and then on the impl (detail) table, and reports fields it could not find.
//! A field's own key is always tried first, so only alternative names need listing.

use femcraft_model::ObjectKind;

/// Columns holding the object number shown to the user.
pub const NUMBER_COLUMNS: &[&str] = &["no", "number", "object_no", "nr"];
/// Primary-key column (falls back to SQLite `rowid`).
pub const ID_COLUMNS: &[&str] = &["id"];
/// Load-case reference on load tables.
pub const LOAD_CASE_COLUMNS: &[&str] = &["load_case", "load_case_id", "load_case_no"];
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
        (Node, "coordinate_1") => &["x", "coordinate_x", "global_coordinate_1"],
        (Node, "coordinate_2") => &["y", "coordinate_y", "global_coordinate_2"],
        (Node, "coordinate_3") => &["z", "coordinate_z", "global_coordinate_3"],
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

pub fn column_candidates(kind: ObjectKind, key: &'static str) -> Vec<&'static str> {
    let mut v = vec![key];
    v.extend_from_slice(overrides(kind, key));
    v
}
