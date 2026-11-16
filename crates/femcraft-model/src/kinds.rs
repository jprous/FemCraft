//! Object kinds and their editable fields.
//!
//! Field descriptions drive table columns and edit dialogs in the UI, and are matched to RFEM
//! database columns in `femcraft-rfdb`. Field keys follow the names RFEM's own API uses where
//! known. All values are SI (m, N, Pa, rad, kg).

/// What kind of value a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Bool,
    Int,
    /// Floating-point value with its SI unit (`""` for dimensionless).
    Float(&'static str),
    Text,
    /// RFEM enumeration, stored as its text (for example `TYPE_STANDARD`).
    Enum,
    /// Reference to one object by number.
    Ref(ObjectKind),
    /// List of object numbers.
    RefList(ObjectKind),
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub ty: FieldType,
}

#[derive(Debug)]
pub struct KindSpec {
    pub kind: ObjectKind,
    /// Plural name as shown in RFEM's navigator / table tabs.
    pub label: &'static str,
    pub group: Group,
    pub fields: &'static [FieldSpec],
}

impl KindSpec {
    pub fn field(&self, key: &str) -> Option<&'static FieldSpec> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Loads are numbered per load case.
    pub fn is_load(&self) -> bool {
        self.group == Group::Loads
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    BasicObjects,
    Supports,
    LoadCases,
    Loads,
}

macro_rules! kinds {
    ($($k:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum ObjectKind { $($k),* }
        impl ObjectKind {
            pub const ALL: &'static [ObjectKind] = &[$(ObjectKind::$k),*];
            pub fn name(self) -> &'static str {
                match self { $(ObjectKind::$k => stringify!($k)),* }
            }
        }
    };
}

kinds!(
    Material,
    Section,
    Thickness,
    Node,
    Line,
    Member,
    Surface,
    Solid,
    NodalSupport,
    LineSupport,
    MemberSupport,
    SurfaceSupport,
    LoadCase,
    NodalLoad,
    LineLoad,
    MemberLoad,
    SurfaceLoad,
    SolidLoad,
    FreeConcentratedLoad,
    FreeLineLoad,
    FreeRectangularLoad,
    FreePolygonLoad,
    ImposedNodalDeformation,
    ImposedLineDeformation,
);

use FieldType::*;
use ObjectKind as K;

const fn f(key: &'static str, label: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec { key, label, ty }
}

const NAME: FieldSpec = f("name", "Name", Text);
const TYPE: FieldSpec = f("type", "Type", Enum);
const COMMENT: FieldSpec = f("comment", "Comment", Text);
const LOAD_TYPE: FieldSpec = f("load_type", "Load type", Enum);
const LOAD_DIST: FieldSpec = f("load_distribution", "Load distribution", Enum);
const LOAD_DIR: FieldSpec = f("load_direction", "Load direction", Enum);

const N_M: &str = "N/m";
const N_M2: &str = "N/m²";

static MATERIAL: &[FieldSpec] = &[
    NAME,
    TYPE,
    f("elasticity_modulus", "Modulus of elasticity E", Float("Pa")),
    f("shear_modulus", "Shear modulus G", Float("Pa")),
    f("poisson_ratio", "Poisson's ratio ν", Float("")),
    f("specific_weight", "Specific weight γ", Float("N/m³")),
    f("mass_density", "Mass density ρ", Float("kg/m³")),
    f(
        "thermal_expansion_coefficient",
        "Coefficient of thermal expansion α",
        Float("1/K"),
    ),
    COMMENT,
];

static SECTION: &[FieldSpec] = &[
    NAME,
    TYPE,
    f("material", "Material", Ref(K::Material)),
    f("A", "Area A", Float("m²")),
    f("I_y", "Moment of inertia Iy", Float("m⁴")),
    f("I_z", "Moment of inertia Iz", Float("m⁴")),
    f("I_t", "Torsional constant It", Float("m⁴")),
    f("A_y", "Shear area Ay", Float("m²")),
    f("A_z", "Shear area Az", Float("m²")),
    COMMENT,
];

static THICKNESS: &[FieldSpec] = &[
    NAME,
    TYPE,
    f("material", "Material", Ref(K::Material)),
    f("uniform_thickness", "Thickness d", Float("m")),
    COMMENT,
];

static NODE: &[FieldSpec] = &[
    TYPE,
    f("reference_node", "Reference node", Ref(K::Node)),
    f("coordinate_system", "Coordinate system", Int),
    f("coordinate_system_type", "Coordinate system type", Enum),
    f("coordinate_1", "X", Float("m")),
    f("coordinate_2", "Y", Float("m")),
    f("coordinate_3", "Z", Float("m")),
    COMMENT,
];

static LINE: &[FieldSpec] = &[
    TYPE,
    f("definition_nodes", "Nodes", RefList(K::Node)),
    f("rotation_angle", "Rotation β", Float("rad")),
    COMMENT,
];

static MEMBER: &[FieldSpec] = &[
    TYPE,
    f("line", "Line", Ref(K::Line)),
    f("section_start", "Section start", Ref(K::Section)),
    f("section_end", "Section end", Ref(K::Section)),
    f("rotation_angle", "Rotation β", Float("rad")),
    f("member_hinge_start", "Hinge start", Int),
    f("member_hinge_end", "Hinge end", Int),
    COMMENT,
];

/// Material comes from the thickness.
static SURFACE: &[FieldSpec] = &[
    f("type", "Stiffness type", Enum),
    f("geometry", "Geometry", Enum),
    f("boundary_lines", "Boundary lines", RefList(K::Line)),
    f("thickness", "Thickness", Ref(K::Thickness)),
    f("axes_rotation", "Local axes rotation", Float("rad")),
    f("axes_z_reversed", "Local z reversed", Bool),
    COMMENT,
];

static SOLID: &[FieldSpec] = &[
    TYPE,
    f(
        "boundary_surfaces",
        "Boundary surfaces",
        RefList(K::Surface),
    ),
    f("material", "Material", Ref(K::Material)),
    COMMENT,
];

static NODAL_SUPPORT: &[FieldSpec] = &[
    NAME,
    TYPE,
    f("nodes", "Nodes", RefList(K::Node)),
    f("spring_x", "Spring uX", Float(N_M)),
    f("spring_y", "Spring uY", Float(N_M)),
    f("spring_z", "Spring uZ", Float(N_M)),
    f("rotational_restraint_x", "Restraint φX", Float("N·m/rad")),
    f("rotational_restraint_y", "Restraint φY", Float("N·m/rad")),
    f("rotational_restraint_z", "Restraint φZ", Float("N·m/rad")),
    COMMENT,
];

static LINE_SUPPORT: &[FieldSpec] = &[
    NAME,
    TYPE,
    f("lines", "Lines", RefList(K::Line)),
    f("spring_x", "Spring uX", Float(N_M2)),
    f("spring_y", "Spring uY", Float(N_M2)),
    f("spring_z", "Spring uZ", Float(N_M2)),
    f("rotational_restraint_x", "Restraint φX", Float("N·m/rad/m")),
    f("rotational_restraint_y", "Restraint φY", Float("N·m/rad/m")),
    f("rotational_restraint_z", "Restraint φZ", Float("N·m/rad/m")),
    COMMENT,
];

static MEMBER_SUPPORT: &[FieldSpec] = &[
    NAME,
    f("members", "Members", RefList(K::Member)),
    f("spring_translation_x", "Spring Cu,x", Float(N_M2)),
    f("spring_translation_y", "Spring Cu,y", Float(N_M2)),
    f("spring_translation_z", "Spring Cu,z", Float(N_M2)),
    f("spring_rotation", "Spring Cφ,x", Float("N·m/rad/m")),
    f("spring_shear_x", "Shear Cs,x", Float(N_M2)),
    f("spring_shear_y", "Shear Cs,y", Float(N_M2)),
    f("spring_shear_z", "Shear Cs,z", Float(N_M2)),
    COMMENT,
];

static SURFACE_SUPPORT: &[FieldSpec] = &[
    NAME,
    f("surfaces", "Surfaces", RefList(K::Surface)),
    f("translation_x", "Spring uX", Float("N/m³")),
    f("translation_y", "Spring uY", Float("N/m³")),
    f("translation_z", "Spring uZ", Float("N/m³")),
    f("shear_xz", "Shear vxz", Float(N_M)),
    f("shear_yz", "Shear vyz", Float(N_M)),
    COMMENT,
];

static LOAD_CASE: &[FieldSpec] = &[
    NAME,
    f("action_category", "Action category", Enum),
    f("to_solve", "To solve", Bool),
    f("static_analysis_settings", "Static analysis settings", Int),
    f("self_weight_active", "Self-weight", Bool),
    f("self_weight_factor_x", "Self-weight factor X", Float("")),
    f("self_weight_factor_y", "Self-weight factor Y", Float("")),
    f("self_weight_factor_z", "Self-weight factor Z", Float("")),
    COMMENT,
];

static NODAL_LOAD: &[FieldSpec] = &[
    f("nodes", "Nodes", RefList(K::Node)),
    LOAD_TYPE,
    LOAD_DIR,
    f("magnitude", "Magnitude P / M", Float("N | N·m")),
    f("components_force_x", "Force PX", Float("N")),
    f("components_force_y", "Force PY", Float("N")),
    f("components_force_z", "Force PZ", Float("N")),
    f("components_moment_x", "Moment MX", Float("N·m")),
    f("components_moment_y", "Moment MY", Float("N·m")),
    f("components_moment_z", "Moment MZ", Float("N·m")),
    COMMENT,
];

/// Line and member loads share the same layout.
macro_rules! linear_load {
    ($objs:literal, $label:literal, $kind:expr) => {
        &[
            f($objs, $label, RefList($kind)),
            LOAD_TYPE,
            LOAD_DIST,
            LOAD_DIR,
            f("magnitude", "Magnitude p / p1", Float(N_M)),
            f("magnitude_2", "Magnitude p2", Float(N_M)),
            f(
                "distance_a",
                "Distance A (m, or fraction if relative)",
                Float(""),
            ),
            f("distance_a_is_relative", "A is relative", Bool),
            f(
                "distance_b",
                "Distance B (m, or fraction if relative)",
                Float(""),
            ),
            f("distance_b_is_relative", "B is relative", Bool),
            COMMENT,
        ]
    };
}

static LINE_LOAD: &[FieldSpec] = linear_load!("lines", "Lines", K::Line);
static MEMBER_LOAD: &[FieldSpec] = linear_load!("members", "Members", K::Member);

static SURFACE_LOAD: &[FieldSpec] = &[
    f("surfaces", "Surfaces", RefList(K::Surface)),
    LOAD_TYPE,
    LOAD_DIST,
    LOAD_DIR,
    f("uniform_magnitude", "Magnitude p", Float(N_M2)),
    f("magnitude_t_c", "Temperature Tc", Float("K")),
    f("magnitude_delta_t", "Temperature ΔT", Float("K")),
    COMMENT,
];

static SOLID_LOAD: &[FieldSpec] = &[
    f("solids", "Solids", RefList(K::Solid)),
    LOAD_TYPE,
    LOAD_DIST,
    LOAD_DIR,
    f("uniform_magnitude", "Magnitude p", Float("N/m³")),
    COMMENT,
];

const FREE_SURFACES: FieldSpec = f("surfaces", "Surfaces", RefList(K::Surface));
const FREE_PROJECTION: FieldSpec = f("load_projection", "Projection plane", Enum);

static FREE_CONCENTRATED_LOAD: &[FieldSpec] = &[
    FREE_SURFACES,
    LOAD_TYPE,
    FREE_PROJECTION,
    LOAD_DIR,
    f("magnitude", "Magnitude P", Float("N")),
    f("location_x", "Location X", Float("m")),
    f("location_y", "Location Y", Float("m")),
    COMMENT,
];

static FREE_LINE_LOAD: &[FieldSpec] = &[
    FREE_SURFACES,
    LOAD_DIST,
    FREE_PROJECTION,
    LOAD_DIR,
    f("magnitude_first", "Magnitude p1", Float(N_M)),
    f("magnitude_second", "Magnitude p2", Float(N_M)),
    f("location_first_x", "Point 1 X", Float("m")),
    f("location_first_y", "Point 1 Y", Float("m")),
    f("location_second_x", "Point 2 X", Float("m")),
    f("location_second_y", "Point 2 Y", Float("m")),
    COMMENT,
];

static FREE_RECTANGULAR_LOAD: &[FieldSpec] = &[
    FREE_SURFACES,
    LOAD_DIST,
    FREE_PROJECTION,
    LOAD_DIR,
    f("magnitude_first", "Magnitude p / p1", Float(N_M2)),
    f("magnitude_second", "Magnitude p2", Float(N_M2)),
    f("location_first_x", "Corner 1 X", Float("m")),
    f("location_first_y", "Corner 1 Y", Float("m")),
    f("location_second_x", "Corner 2 X", Float("m")),
    f("location_second_y", "Corner 2 Y", Float("m")),
    COMMENT,
];

/// Polygon corner points are a child list; not editable yet.
static FREE_POLYGON_LOAD: &[FieldSpec] = &[
    FREE_SURFACES,
    LOAD_DIST,
    FREE_PROJECTION,
    LOAD_DIR,
    f("magnitude", "Magnitude p", Float(N_M2)),
    COMMENT,
];

static IMPOSED_NODAL_DEFORMATION: &[FieldSpec] = &[
    f("nodes", "Nodes", RefList(K::Node)),
    f("imposed_displacement_x", "Displacement uX", Float("m")),
    f("imposed_displacement_y", "Displacement uY", Float("m")),
    f("imposed_displacement_z", "Displacement uZ", Float("m")),
    f("imposed_rotation_x", "Rotation φX", Float("rad")),
    f("imposed_rotation_y", "Rotation φY", Float("rad")),
    f("imposed_rotation_z", "Rotation φZ", Float("rad")),
    COMMENT,
];

static IMPOSED_LINE_DEFORMATION: &[FieldSpec] = &[
    f("lines", "Lines", RefList(K::Line)),
    f("imposed_displacement_line_start_x", "Start uX", Float("m")),
    f("imposed_displacement_line_start_y", "Start uY", Float("m")),
    f("imposed_displacement_line_start_z", "Start uZ", Float("m")),
    f("imposed_rotation_line_start", "Start φ", Float("rad")),
    f("imposed_displacement_line_end_x", "End uX", Float("m")),
    f("imposed_displacement_line_end_y", "End uY", Float("m")),
    f("imposed_displacement_line_end_z", "End uZ", Float("m")),
    f("imposed_rotation_line_end", "End φ", Float("rad")),
    COMMENT,
];

macro_rules! spec {
    ($k:ident, $label:literal, $group:ident, $fields:expr) => {
        KindSpec {
            kind: K::$k,
            label: $label,
            group: Group::$group,
            fields: $fields,
        }
    };
}

static SPECS: &[KindSpec] = &[
    spec!(Material, "Materials", BasicObjects, MATERIAL),
    spec!(Section, "Sections", BasicObjects, SECTION),
    spec!(Thickness, "Thicknesses", BasicObjects, THICKNESS),
    spec!(Node, "Nodes", BasicObjects, NODE),
    spec!(Line, "Lines", BasicObjects, LINE),
    spec!(Member, "Members", BasicObjects, MEMBER),
    spec!(Surface, "Surfaces", BasicObjects, SURFACE),
    spec!(Solid, "Solids", BasicObjects, SOLID),
    spec!(NodalSupport, "Nodal Supports", Supports, NODAL_SUPPORT),
    spec!(LineSupport, "Line Supports", Supports, LINE_SUPPORT),
    spec!(MemberSupport, "Member Supports", Supports, MEMBER_SUPPORT),
    spec!(
        SurfaceSupport,
        "Surface Supports",
        Supports,
        SURFACE_SUPPORT
    ),
    spec!(LoadCase, "Load Cases", LoadCases, LOAD_CASE),
    spec!(NodalLoad, "Nodal Loads", Loads, NODAL_LOAD),
    spec!(LineLoad, "Line Loads", Loads, LINE_LOAD),
    spec!(MemberLoad, "Member Loads", Loads, MEMBER_LOAD),
    spec!(SurfaceLoad, "Surface Loads", Loads, SURFACE_LOAD),
    spec!(SolidLoad, "Solid Loads", Loads, SOLID_LOAD),
    spec!(
        FreeConcentratedLoad,
        "Free Concentrated Loads",
        Loads,
        FREE_CONCENTRATED_LOAD
    ),
    spec!(FreeLineLoad, "Free Line Loads", Loads, FREE_LINE_LOAD),
    spec!(
        FreeRectangularLoad,
        "Free Rectangular Loads",
        Loads,
        FREE_RECTANGULAR_LOAD
    ),
    spec!(
        FreePolygonLoad,
        "Free Polygon Loads",
        Loads,
        FREE_POLYGON_LOAD
    ),
    spec!(
        ImposedNodalDeformation,
        "Imposed Nodal Deformations",
        Loads,
        IMPOSED_NODAL_DEFORMATION
    ),
    spec!(
        ImposedLineDeformation,
        "Imposed Line Deformations",
        Loads,
        IMPOSED_LINE_DEFORMATION
    ),
];

impl ObjectKind {
    pub fn spec(self) -> &'static KindSpec {
        let s = &SPECS[self as usize];
        debug_assert_eq!(s.kind, self);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_match_kinds() {
        assert_eq!(SPECS.len(), ObjectKind::ALL.len());
        for &k in ObjectKind::ALL {
            assert_eq!(k.spec().kind, k);
            let mut keys: Vec<_> = k.spec().fields.iter().map(|f| f.key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(
                keys.len(),
                k.spec().fields.len(),
                "duplicate field in {k:?}"
            );
        }
    }
}
