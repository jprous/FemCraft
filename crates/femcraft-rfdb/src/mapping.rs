//! Where each FemCraft field lives in RFEM's `model.db`.
//!
//! Confirmed on RFEM 6.16 with `rf6-catalog --kinds` on a real model:
//! - Main tables use camelCase columns; the object number is `userID`, the db key is `id`.
//! - `impl_id` / `impl_table` point to the detail row, whose table name gives the type
//!   (`MemberImplBeam` -> `TYPE_BEAM`, `LineTypeLoadImplForce` -> `LOAD_TYPE_FORCE`).
//! - References are `<name>_id` (+ `<name>_table` when the target varies) holding db ids.
//! - Loads point to their load case via `parentModelObject_id` on the main row.
//! - List tables are `<ImplTable>_<field>`: `id` = owner impl id, `container_order`, and the
//!   value in `reference_id` (+ `reference_table`), `value_id` or `value`.
//! - "Part" tables hold one row per object: either `owner_id` / `owner_table` = impl row
//!   (`SpringConstants`), or `id` = impl id with `container_order` 0 (`..._magnitudes`).
//! - Key / value maps are `<Table>_<map>_keys` and `_values` paired by `id` + `container_order`.
//!
//! Entries marked "guess" are unconfirmed (no such objects in the test model yet).
//!
//! For each field the loader tries (case-insensitive, main table then impl table): the field key,
//! camelCase + `_id`, camelCase, then [`overrides`]; then [`part_column`], then child lists for
//! list fields, then [`derived`].

use femcraft_model::ObjectKind;

/// Columns holding the object number shown to the user.
pub const NUMBER_COLUMNS: &[&str] = &["userID", "no", "number"];
/// Primary-key column (falls back to SQLite `rowid`).
pub const ID_COLUMNS: &[&str] = &["id"];
/// Load-case reference on load tables.
pub const LOAD_CASE_COLUMNS: &[&str] = &["parentModelObject_id", "loadCase_id", "load_case"];
/// Columns that are bookkeeping, not data, in child list tables.
pub const CHILD_META_COLUMNS: &[&str] = &[
    "id",
    "owner_id",
    "owner_table",
    "container_order",
    "reference_table",
    "value_table",
    "row",
    "index",
    "order",
];
pub const CHILD_ORDER_COLUMNS: &[&str] = &["container_order", "row", "index", "order"];
/// Value column of a list table, in order of preference.
pub const CHILD_VALUE_COLUMNS: &[&str] = &["reference_id", "value_id", "value"];

/// Candidate table names for each kind.
pub fn tables(kind: ObjectKind) -> &'static [&'static str] {
    use ObjectKind::*;
    match kind {
        Material => &["Material"],
        Section => &["Section"],
        Thickness => &["Thickness"],
        Node => &["Node"],
        Line => &["Line"],
        Member => &["Member"],
        Surface => &["Surface"],
        Solid => &["Solid"],
        NodalSupport => &["NodalSupport"],
        LineSupport => &["LineSupport"],
        MemberSupport => &["MemberSupport"],
        SurfaceSupport => &["SurfaceSupport"],
        LoadCase => &["LoadCase"],
        NodalLoad => &["NodalLoad"],
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

/// Alternative column (or list-table suffix) names tried after the automatic ones.
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
        // Order confirmed by "Sliding in x and y" = 0, 0, inf, inf, inf.
        (SurfaceSupport, "translation_x") => &["springConstant_0"],
        (SurfaceSupport, "translation_y") => &["springConstant_1"],
        (SurfaceSupport, "translation_z") => &["springConstant_2"],
        // guess: which of the remaining two is xz / yz
        (SurfaceSupport, "shear_xz") => &["springConstant_3"],
        (SurfaceSupport, "shear_yz") => &["springConstant_4"],
        // Objects a load is applied to.
        (k, "nodes" | "lines" | "members" | "surfaces" | "solids") if k.spec().is_load() => {
            &["assignedTo"]
        }
        // guess
        (Surface, "boundary_lines") => &["boundaryLines", "lines"],
        (Solid, "boundary_surfaces") => &["boundarySurfaces", "surfaces"],
        (Thickness, "uniform_thickness") => &["thickness", "d"],
        _ => &[],
    }
}

pub fn column_candidates(kind: ObjectKind, key: &'static str) -> Vec<String> {
    let camel = camel_case(key);
    let mut v = vec![key.to_owned(), format!("{camel}_id"), camel];
    v.extend(overrides(kind, key).iter().map(|s| s.to_string()));
    v
}

/// A field held in a part table owned by the impl row: `(table, column)`.
/// `{impl}` in the table name is replaced by the impl table name.
pub fn part_column(kind: ObjectKind, key: &str) -> Option<(&'static str, &'static str)> {
    use ObjectKind::*;
    const SPRINGS: &str = "SpringConstants";
    Some(match (kind, key) {
        (NodalSupport | LineSupport, "spring_x") => (SPRINGS, "springConstantAlongX"),
        (NodalSupport | LineSupport, "spring_y") => (SPRINGS, "springConstantAlongY"),
        (NodalSupport | LineSupport, "spring_z") => (SPRINGS, "springConstantAlongZ"),
        (NodalSupport | LineSupport, "rotational_restraint_x") => {
            (SPRINGS, "springConstantAroundX")
        }
        (NodalSupport | LineSupport, "rotational_restraint_y") => {
            (SPRINGS, "springConstantAroundY")
        }
        (NodalSupport | LineSupport, "rotational_restraint_z") => {
            (SPRINGS, "springConstantAroundZ")
        }
        // guess: member supports use the same spring table
        (MemberSupport, "spring_translation_x") => (SPRINGS, "springConstantAlongX"),
        (MemberSupport, "spring_translation_y") => (SPRINGS, "springConstantAlongY"),
        (MemberSupport, "spring_translation_z") => (SPRINGS, "springConstantAlongZ"),
        (MemberSupport, "spring_rotation") => (SPRINGS, "springConstantAroundX"),
        (MemberLoad | LineLoad, "magnitude") => {
            ("{impl}_magnitudes", "temperaturesOrMagnitudeFirstMagnitude")
        }
        (MemberLoad | LineLoad, "magnitude_2") => (
            "{impl}_magnitudes",
            "temperaturesOrMagnitudeSecondMagnitude",
        ),
        _ => return None,
    })
}

/// Read-only values found through key / value maps.
#[derive(Debug, Clone, Copy)]
pub enum Derived {
    /// Impl `librarySection_id` -> library row `sectionData_id` ->
    /// `SectionData_parameterValues[key].valueSI`.
    SectionParameter(&'static str),
    /// `MaterialImpl_materialDataVector[0]` -> `MaterialData.properties_id` ->
    /// `MaterialProperties_propertiesMap[key]` -> referenced row `valueSI`.
    MaterialProperty(&'static str),
}

pub fn derived(kind: ObjectKind, key: &'static str) -> Option<Derived> {
    use Derived::*;
    use ObjectKind::*;
    Some(match (kind, key) {
        (Section, "A" | "I_y" | "I_z" | "I_t") => SectionParameter(key),
        (Section, "A_y") => SectionParameter("A_v_y"),
        (Section, "A_z") => SectionParameter("A_v_z"),
        (Material, "elasticity_modulus") => MaterialProperty("E"),
        (Material, "shear_modulus") => MaterialProperty("G"),
        (Material, "poisson_ratio") => MaterialProperty("nu"),
        (Material, "specific_weight") => MaterialProperty("gamma"),
        (Material, "mass_density") => MaterialProperty("rho"),
        (Material, "thermal_expansion_coefficient") => MaterialProperty("alpha"),
        _ => return None,
    })
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
