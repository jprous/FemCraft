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
pub const CHILD_VALUE_COLUMNS: &[&str] = &["reference_id", "object_id", "value_id", "value"];

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
        Opening => &["Opening"],
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
        (Member | Line, "rotation_angle") => &["angle"],
        // Confirmed on RFEM 6.16 (Lines.rf6): LineImplCircle / EllipticalArc / Parabola.
        (Line, "circle_center_coordinate_1") => &["center_x"],
        (Line, "circle_center_coordinate_2") => &["center_y"],
        (Line, "circle_center_coordinate_3") => &["center_z"],
        (Line, "circle_normal_coordinate_1") => &["normal_x"],
        (Line, "circle_normal_coordinate_2") => &["normal_y"],
        (Line, "circle_normal_coordinate_3") => &["normal_z"],
        (Line, "circle_radius") => &["radius"],
        (Line, "circle_rotation") => &["rotationAroundNormal"],
        (Line, "elliptical_arc_alpha" | "parabola_alpha") => &["alpha"],
        (Line, "elliptical_arc_beta") => &["beta"],
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
        // Member support constants: 0-2 translation x / y / z (confirmed by a support with
        // 2.5e6, 2.5e6, 5e6); guess: 3 rotation, 4-6 shear x / y / z, as in RFEM's API order.
        (MemberSupport, "spring_translation_x") => &["springConstant_0"],
        (MemberSupport, "spring_translation_y") => &["springConstant_1"],
        (MemberSupport, "spring_translation_z") => &["springConstant_2"],
        (MemberSupport, "spring_rotation") => &["springConstant_3"],
        (MemberSupport, "spring_shear_x") => &["springConstant_4"],
        (MemberSupport, "spring_shear_y") => &["springConstant_5"],
        (MemberSupport, "spring_shear_z") => &["springConstant_6"],
        (Material, "type") => &["materialType"],
        // From the table structure only (no objects seen yet).
        (FreeConcentratedLoad, "location_x") => &["locationFirst"],
        (FreeConcentratedLoad, "location_y") => &["locationSecond"],
        (ImposedNodalDeformation, "imposed_displacement_x") => &["imposedDisplacement_x"],
        (ImposedNodalDeformation, "imposed_displacement_y") => &["imposedDisplacement_y"],
        (ImposedNodalDeformation, "imposed_displacement_z") => &["imposedDisplacement_z"],
        (ImposedNodalDeformation, "imposed_rotation_x") => &["imposedRotation_x"],
        (ImposedNodalDeformation, "imposed_rotation_y") => &["imposedRotation_y"],
        (ImposedNodalDeformation, "imposed_rotation_z") => &["imposedRotation_z"],
        (ImposedLineDeformation, "imposed_displacement_line_start_x") => {
            &["imposedDisplacementAtLineStart_x"]
        }
        (ImposedLineDeformation, "imposed_displacement_line_start_y") => {
            &["imposedDisplacementAtLineStart_y"]
        }
        (ImposedLineDeformation, "imposed_displacement_line_start_z") => {
            &["imposedDisplacementAtLineStart_z"]
        }
        (ImposedLineDeformation, "imposed_displacement_line_end_x") => {
            &["imposedDisplacementAtLineEnd_x"]
        }
        (ImposedLineDeformation, "imposed_displacement_line_end_y") => {
            &["imposedDisplacementAtLineEnd_y"]
        }
        (ImposedLineDeformation, "imposed_displacement_line_end_z") => {
            &["imposedDisplacementAtLineEnd_z"]
        }
        (ImposedLineDeformation, "imposed_rotation_line_start") => &["imposedRotationLineStart"],
        (ImposedLineDeformation, "imposed_rotation_line_end") => &["imposedRotationAtLineEnd"],
        (NodalLoad, "load_direction") => &["loadDirectionAxis"],
        // NodalLoadImplComponents (RFEM 6.16 table structure; no object seen yet).
        (NodalLoad, "components_force_x") => &["force_x"],
        (NodalLoad, "components_force_y") => &["force_y"],
        (NodalLoad, "components_force_z") => &["force_z"],
        (NodalLoad, "components_moment_x") => &["moment_x"],
        (NodalLoad, "components_moment_y") => &["moment_y"],
        (NodalLoad, "components_moment_z") => &["moment_z"],
        (FreeRectangularLoad | FreeLineLoad, "load_distribution") => &["distribution"],
        // Objects a load is applied to.
        (k, "nodes" | "lines" | "members" | "surfaces" | "solids") if k.spec().is_load() => {
            &["assignedTo"]
        }
        // Confirmed on RFEM 6.16 (Lines.rf6): `OpeningImpl_boundaryLines` / `SurfaceImplPlane`.
        (Surface | Opening, "boundary_lines") => &["boundaryLines"],
        // Confirmed on RFEM 6.14 (MemberImplBeam; 0 = uniform).
        (Member, "section_distribution") => &["sectionDistributionType"],
        (Thickness, "uniform_thickness") => &["thickness"],
        // guess
        (Solid, "boundary_surfaces") => &["boundarySurfaces", "surfaces"],
        _ => &[],
    }
}

/// Impl table a field belongs to, when it only applies to one type (line curve fields share
/// column names such as `alpha` between impl tables).
pub fn field_impl_table(kind: ObjectKind, key: &str) -> Option<&'static str> {
    if kind != ObjectKind::Line {
        return None;
    }
    // Longest prefix first: `elliptical_arc_` before `ellipse_` / `arc_`.
    [
        ("elliptical_arc_", "LineImplEllipticalArc"),
        ("ellipse_", "LineImplEllipse"),
        ("arc_", "LineImplArc"),
        ("circle_", "LineImplCircle"),
        ("parabola_", "LineImplParabola"),
        ("nurbs_", "LineImplNurbs"),
    ]
    .into_iter()
    .find(|(p, _)| key.starts_with(p))
    .map(|(_, t)| t)
}

pub fn column_candidates(kind: ObjectKind, key: &'static str) -> Vec<String> {
    let camel = camel_case(key);
    let mut v = vec![key.to_owned(), format!("{camel}_id"), camel];
    v.extend(overrides(kind, key).iter().map(|s| s.to_string()));
    v
}

/// A field held in a one-row part table.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    /// Table name; `{impl}` is replaced by the impl table name.
    pub table: &'static str,
    pub column: &'static str,
    /// Impl-row column holding the owner key; `None` = the impl row id itself.
    pub via: Option<&'static str>,
}

/// Where a field held in a part table lives.
pub fn part_column(kind: ObjectKind, key: &str) -> Option<Part> {
    use ObjectKind::*;
    const fn p(table: &'static str, column: &'static str) -> Part {
        Part {
            table,
            column,
            via: None,
        }
    }
    const SPRINGS: &str = "SpringConstants";
    const SURFACE_LOAD_PARAMS: &str = "{impl}_NodeAndMagnitudeParameters_loadParameters";
    Some(match (kind, key) {
        (NodalSupport | LineSupport, "spring_x") => p(SPRINGS, "springConstantAlongX"),
        (NodalSupport | LineSupport, "spring_y") => p(SPRINGS, "springConstantAlongY"),
        (NodalSupport | LineSupport, "spring_z") => p(SPRINGS, "springConstantAlongZ"),
        (NodalSupport | LineSupport, "rotational_restraint_x") => {
            p(SPRINGS, "springConstantAroundX")
        }
        (NodalSupport | LineSupport, "rotational_restraint_y") => {
            p(SPRINGS, "springConstantAroundY")
        }
        (NodalSupport | LineSupport, "rotational_restraint_z") => {
            p(SPRINGS, "springConstantAroundZ")
        }
        (NodalSupport | LineSupport, "nonlinearity_x") => p(SPRINGS, "nonlinearityTypeAlongX"),
        (NodalSupport | LineSupport, "nonlinearity_y") => p(SPRINGS, "nonlinearityTypeAlongY"),
        (NodalSupport | LineSupport, "nonlinearity_z") => p(SPRINGS, "nonlinearityTypeAlongZ"),
        (NodalSupport | LineSupport, "nonlinearity_rx") => p(SPRINGS, "nonlinearityTypeAroundX"),
        (NodalSupport | LineSupport, "nonlinearity_ry") => p(SPRINGS, "nonlinearityTypeAroundY"),
        (NodalSupport | LineSupport, "nonlinearity_rz") => p(SPRINGS, "nonlinearityTypeAroundZ"),
        (MemberLoad | LineLoad, "magnitude") => {
            p("{impl}_magnitudes", "temperaturesOrMagnitudeFirstMagnitude")
        }
        (MemberLoad | LineLoad, "magnitude_2") => p(
            "{impl}_magnitudes",
            "temperaturesOrMagnitudeSecondMagnitude",
        ),
        // Surface loads: impl.loadParameters_id -> parameter list, first row.
        (SurfaceLoad, "uniform_magnitude" | "magnitude_t_c" | "magnitude_delta_t") => Part {
            table: SURFACE_LOAD_PARAMS,
            column: match key {
                "uniform_magnitude" => "magnitude",
                "magnitude_t_c" => "magnitude1",
                _ => "magnitude2",
            },
            via: Some("loadParameters_id"),
        },
        (FreePolygonLoad, "magnitude") => p("{impl}_loadMagnitudeParameters", "magnitude"),
        // Surface local axes (owner = surface impl).
        (Surface, "axes_rotation") => p("SurfaceAxes", "inputAxesAngularRotation"),
        (Surface, "axes_z_reversed") => p("SurfaceAxes", "reversedLocalAxisZ"),
        // Surface stiffness row (owner = surface impl) holds the thickness.
        (Surface, "thickness") => p("SurfaceStiffnessStandard", "thickness_id"),
        _ => return None,
    })
}

/// Read-only values found through key / value maps.
#[derive(Debug, Clone, Copy)]
pub enum Derived {
    /// Impl `librarySection_id` -> library row `sectionData_id` ->
    /// `SectionData_parameterValues[key].valueSI`, first key present.
    SectionParameter(&'static [&'static str]),
    /// `MaterialImpl_materialDataVector[0]` -> `MaterialData.properties_id` ->
    /// `MaterialProperties_propertiesMap[key]` -> referenced row `valueSI`.
    MaterialProperty(&'static str),
    /// Impl `librarySection_id` -> `ParametricSection.parametrizationShape`, as RFEM's
    /// API enum (see [`parametric_shape`]); other codes stay integers.
    SectionShape,
    /// Impl `librarySection_id` -> `DatabaseSection.databaseSectionId`: RFEM's library id.
    SectionLibraryId,
    /// `SurfaceSpecificImplPart.stiffness_table` (owner = surface impl), e.g.
    /// `SurfaceStiffnessWithoutThickness` -> `TYPE_WITHOUT_THICKNESS`.
    SurfaceStiffness,
    /// Impl `<ref>` (e.g. `controlPoint_id`) -> `ControlPoint.impl_id` ->
    /// `ControlPointImpl.coordinates_<axis>` (global coordinates; confirmed on RFEM 6.16).
    ControlPoint(&'static str, &'static str),
    /// Opening → surfaces: reverse lookup through `SurfaceSpecificImplPart_integratedOpenings`
    /// (value_id = opening impl_id) → `SurfaceSpecificImplPart.owner_id` → `Surface.impl_id`.
    OpeningSurfaces,
}

pub fn derived(kind: ObjectKind, key: &'static str) -> Option<Derived> {
    use Derived::*;
    use ObjectKind::*;
    Some(match (kind, key) {
        (Section, "parametrization_type") => SectionShape,
        (Section, "database_section_id") => SectionLibraryId,
        (Section, "b") => SectionParameter(&["b"]),
        (Section, "h") => SectionParameter(&["h"]),
        (Section, "d") => SectionParameter(&["d", "D"]),
        (Section, "A") => SectionParameter(&["A"]),
        (Section, "I_y") => SectionParameter(&["I_y"]),
        (Section, "I_z") => SectionParameter(&["I_z"]),
        (Section, "I_t") => SectionParameter(&["I_t"]),
        // Library steel uses A_v_*, parametric sections A_y / A_z.
        (Section, "A_y") => SectionParameter(&["A_v_y", "A_y"]),
        (Section, "A_z") => SectionParameter(&["A_v_z", "A_z"]),
        (Surface, "type") => SurfaceStiffness,
        (Opening, "surfaces") => OpeningSurfaces,
        (Line, k) if k.ends_with("control_point_x")
            || k.ends_with("control_point_y")
            || k.ends_with("control_point_z") =>
        {
            let axis = &k[k.len() - 1..];
            let column = match k {
                _ if k.starts_with("elliptical_arc_first_") => "firstMainAxisControlPoint_id",
                _ if k.starts_with("elliptical_arc_second_") => "secondMainAxisControlPoint_id",
                _ if k.starts_with("elliptical_arc_perimeter_") => "perimeterControlPoint_id",
                _ => "controlPoint_id",
            };
            ControlPoint(column, axis)
        }
        (Material, "elasticity_modulus") => MaterialProperty("E"),
        (Material, "shear_modulus") => MaterialProperty("G"),
        (Material, "poisson_ratio") => MaterialProperty("nu"),
        (Material, "specific_weight") => MaterialProperty("gamma"),
        (Material, "mass_density") => MaterialProperty("rho"),
        (Material, "thermal_expansion_coefficient") => MaterialProperty("alpha"),
        _ => return None,
    })
}

/// `ParametricSection.parametrizationShape` codes confirmed on RFEM 6.16 (sections created by
/// API I name `R_M1 0.3/0.6`, `SQ_M1 0.4`, `CIRCLE_M1 0.4`; dimensions in m) -> API I's
/// `parametrization_type`. Massive (concrete) shapes; `seriesId` 1684 / 2006 / 1686.
pub fn parametric_shape(code: i64) -> Option<&'static str> {
    Some(match code {
        49 => "PARAMETRIC_MASSIVE_I__MASSIVE_RECTANGLE__R_M1",
        154 => "PARAMETRIC_MASSIVE_I__MASSIVE_SQUARE__SQ_M1",
        53 => "PARAMETRIC_MASSIVE_I__MASSIVE_CIRCLE__CIRCLE_M1",
        // Inferred: TYPE_PARAMETRIC_BARS with only `d` (rebar-like bar in the pit models).
        188 => "PARAMETRIC_BARS__ROUND_BAR__ROUND",
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
