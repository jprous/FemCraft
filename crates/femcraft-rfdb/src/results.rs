//! RFEM result data in a solved `.rf6`: which result cases were calculated, and how the
//! available results are organised for display.
//!
//! This is the *structural* interpreter. It answers two questions a Results navigator needs:
//!
//! 1. **Which cases can be shown?** ([`ResultSet`]) RFEM writes solver results under `!solver/`
//!    (per-case folders `!solver/RN_...`), and records every calculated analysis in the
//!    `solver_ids_history.xml` sidecar as `(solver_id, object_type, user_id)`. We read that list
//!    and name each case from the database, giving the dropdown of load cases, load combinations,
//!    design situations and result combinations that actually hold results.
//!
//! 2. **Which result quantities apply?** ([`result_tree`]) The categories (deformations, member
//!    internal forces, surface internal forces, support reactions, ...) depend on what the model
//!    contains. The tree mirrors RFEM's Results navigator: tick-box categories, each with a set of
//!    radio-selectable components.
//!
//! Decoding the numeric values out of the `!solver/RN_*` binary files comes later, component by
//! component; [`ResultSet::solver_folders`] keeps the folder list for that step.
//!
//! Confirmed on RFEM 6.16 / solver results version 18.00 (`Chamber 4x4x6.rf6`): 8 load cases,
//! 39 load combinations, 4 design situations; solver folders are `!solver/RN_<index>_<tag>` or
//! `!solver/RN_<uuid>` (the index/uuid to case mapping is not resolved yet).

use std::collections::BTreeMap;

use rusqlite::{Connection, OpenFlags};

use crate::{Archive, Error};

/// Sidecar listing every analysis RFEM calculated.
const HISTORY: &str = "solver_ids_history.xml";
/// Sidecar with the solver / results format version.
const VERSION_XML: &str = "!solver/solver_version.xml";
/// Per-case result folders live directly under this prefix: `!solver/RN_...` (nodal results of
/// surface / solid meshes) and `!solver/RO_...` (results of 1D member elements).
const RESULT_FOLDER_PREFIXES: [&str; 2] = ["!solver/RN", "!solver/RO"];

fn is_result_entry(e: &str) -> bool {
    RESULT_FOLDER_PREFIXES.iter().any(|p| e.starts_with(p))
}

/// The four kinds of case a result can belong to, in the order RFEM lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaseKind {
    LoadCase,
    LoadCombination,
    DesignSituation,
    ResultCombination,
}

impl CaseKind {
    /// Short tag RFEM shows before the number (`LC`, `CO`, `DS`, `RC`).
    pub fn tag(self) -> &'static str {
        match self {
            CaseKind::LoadCase => "LC",
            CaseKind::LoadCombination => "CO",
            CaseKind::DesignSituation => "DS",
            CaseKind::ResultCombination => "RC",
        }
    }

    /// Group heading for the dropdown.
    pub fn group_label(self) -> &'static str {
        match self {
            CaseKind::LoadCase => "Load Cases",
            CaseKind::LoadCombination => "Load Combinations",
            CaseKind::DesignSituation => "Design Situations",
            CaseKind::ResultCombination => "Result Combinations",
        }
    }

    /// Main database table holding this kind's rows.
    fn db_table(self) -> &'static str {
        match self {
            CaseKind::LoadCase => "LoadCase",
            CaseKind::LoadCombination => "LoadCombination",
            CaseKind::DesignSituation => "DesignSituation",
            CaseKind::ResultCombination => "ResultCombination",
        }
    }

    /// The `object_type` string used in `solver_ids_history.xml`.
    fn from_solver_type(s: &str) -> Option<Self> {
        match s {
            "Load_Case" => Some(CaseKind::LoadCase),
            "Load_Combination" => Some(CaseKind::LoadCombination),
            "Design_Situation" => Some(CaseKind::DesignSituation),
            "Result_Combination" => Some(CaseKind::ResultCombination),
            _ => None,
        }
    }

    /// All kinds, in display order.
    pub const ALL: [CaseKind; 4] = [
        CaseKind::LoadCase,
        CaseKind::LoadCombination,
        CaseKind::DesignSituation,
        CaseKind::ResultCombination,
    ];
}

/// Which extreme of an envelope (design situation / result combination) is plotted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Extremum {
    Max,
    Min,
    /// The full range: scale from the lowest minimum to the highest maximum; each point shows
    /// whichever of its max and min is larger in magnitude.
    #[default]
    Both,
}

impl Extremum {
    pub const ALL: [Extremum; 3] = [Extremum::Max, Extremum::Min, Extremum::Both];

    pub fn label(self) -> &'static str {
        match self {
            Extremum::Max => "Max",
            Extremum::Min => "Min",
            Extremum::Both => "Both",
        }
    }

    /// Reduces the per-member values at one point (`None` for no values).
    pub fn reduce(self, values: impl Iterator<Item = f64>) -> Option<f64> {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in values {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if lo > hi {
            return None;
        }
        Some(match self {
            Extremum::Max => hi,
            Extremum::Min => lo,
            Extremum::Both => {
                if hi.abs() >= lo.abs() {
                    hi
                } else {
                    lo
                }
            }
        })
    }
}

/// One case that has solver results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultCase {
    pub kind: CaseKind,
    /// RFEM object number (`userID`).
    pub no: u32,
    /// User name, empty when RFEM auto-names the case (combinations, design situations).
    pub name: String,
    /// Solver analysis id from the history file (0 when it could not be read).
    pub solver_id: u32,
    /// Solver ids of the load combinations a design situation or result combination envelopes
    /// (RFEM stores no results file of its own for them); empty for load cases / combinations.
    pub members: Vec<u32>,
}

impl ResultCase {
    /// True when the case is an envelope with a minimum and maximum (design situation or
    /// result combination).
    pub fn has_extremes(&self) -> bool {
        matches!(
            self.kind,
            CaseKind::DesignSituation | CaseKind::ResultCombination
        )
    }

    /// Solver ids whose result files make up this case: its own file for a load case or
    /// combination, the enveloped combinations for a design situation / result combination.
    pub fn result_ids(&self) -> Vec<u32> {
        if self.has_extremes() && !self.members.is_empty() {
            self.members.clone()
        } else {
            vec![self.solver_id]
        }
    }

    /// `LC1`, `CO20`, ... — the tag plus number.
    pub fn tag(&self) -> String {
        format!("{}{}", self.kind.tag(), self.no)
    }

    /// `CO20`, or `LC1 Self-weight` when the case has a name — what the dropdown shows.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.tag()
        } else {
            format!("{} {}", self.tag(), self.name)
        }
    }
}

/// What a solved file offers: the calculated cases and where their binary results live.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultSet {
    /// Calculated cases, grouped by kind then ordered by number.
    pub cases: Vec<ResultCase>,
    /// Per-case result folder names under `!solver/` (e.g. `RN_28_CO20 - SA2`, `RN_<uuid>`),
    /// kept for later numeric decoding; not yet mapped to [`cases`](Self::cases).
    pub solver_folders: Vec<String>,
    /// Solver results format version (`results_version` in `solver_version.xml`), informational.
    pub results_version: Option<String>,
}

impl ResultSet {
    /// Reads the result structure from an opened archive. `Ok(None)` when the file holds no
    /// solver results (an unsolved model).
    pub fn read(archive: &Archive) -> Result<Option<Self>, Error> {
        let has_results = archive
            .entries
            .iter()
            .any(|e| is_result_entry(e));
        if !has_results {
            return Ok(None);
        }

        let history = archive.read_entry(HISTORY).unwrap_or_default();
        let mut cases = parse_history(&history);

        // Name the cases from the database (load-case names, mostly; combinations are auto-named).
        if let Ok(conn) =
            Connection::open_with_flags(archive.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        {
            fill_case_names(&conn, &mut cases);
            fill_members(&conn, &mut cases);
        }

        cases.sort_by_key(|c| (c.kind, c.no));

        // Distinct result folder names, in archive order.
        let mut seen = std::collections::HashSet::new();
        let solver_folders: Vec<String> = archive
            .entries
            .iter()
            .filter(|e| is_result_entry(e))
            .filter_map(|e| e.strip_prefix("!solver/").and_then(|r| r.split('/').next()))
            .filter(|f| seen.insert(f.to_string()))
            .map(str::to_owned)
            .collect();
        drop_uncalculated(&mut cases, &solver_folders);

        // `<results_version value="18.00"/>`: the version is the element's `value` attribute.
        let results_version = archive.read_entry(VERSION_XML).ok().and_then(|b| {
            let text = String::from_utf8_lossy(&b);
            let at = text.find("results_version")?;
            attr(&text[at..], "value").map(str::to_owned)
        });

        Ok(Some(ResultSet {
            cases,
            solver_folders,
            results_version,
        }))
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// Cases of one kind, in number order.
    pub fn cases_of(&self, kind: CaseKind) -> impl Iterator<Item = &ResultCase> {
        self.cases.iter().filter(move |c| c.kind == kind)
    }
}

/// The solver id in a result folder name (`RN_28_CO20 - SA2` / `RO_4_LC2 - SA1` -> 28 / 4).
fn folder_solver_id(folder: &str) -> Option<u32> {
    let rest = folder.strip_prefix("RN_").or_else(|| folder.strip_prefix("RO_"))?;
    rest.split('_').next()?.parse().ok()
}

/// RFEM's history also lists cases that were never calculated (for example a combination that
/// was deleted since): leave out the cases without result folders. A design situation or result
/// combination stays when any combination it envelopes has results. Only done when every
/// folder is named by solver id (folders named by uuid cannot be matched to cases).
fn drop_uncalculated(cases: &mut Vec<ResultCase>, folders: &[String]) {
    if folders.is_empty() || folders.iter().any(|f| folder_solver_id(f).is_none()) {
        return;
    }
    let has = |id: u32| folders.iter().any(|f| folder_solver_id(f) == Some(id));
    cases.retain(|c| match c.kind {
        CaseKind::LoadCase | CaseKind::LoadCombination => has(c.solver_id),
        CaseKind::DesignSituation | CaseKind::ResultCombination => {
            c.members.iter().any(|&m| has(m))
        }
    });
}

/// Reads `(solver_id, user_id, object_type)` from the history sidecar into a deduplicated list
/// (one entry per case, the last analysis wins).
fn parse_history(bytes: &[u8]) -> Vec<ResultCase> {
    let text = String::from_utf8_lossy(bytes);
    // (kind, no) -> (name placeholder, solver_id); BTreeMap keeps a stable, sorted result.
    let mut map: BTreeMap<(CaseKind, u32), u32> = BTreeMap::new();
    let mut solver_id = 0u32;
    for piece in text.split('<') {
        let piece = piece.trim_start();
        if let Some(rest) = piece.strip_prefix("analysis ") {
            solver_id = attr(rest, "solver_id")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = piece.strip_prefix("case_object_index ") {
            let no = attr(rest, "user_id").and_then(|v| v.parse::<u32>().ok());
            let kind = attr(rest, "object_type").and_then(CaseKind::from_solver_type);
            if let (Some(no), Some(kind)) = (no, kind) {
                map.insert((kind, no), solver_id);
            }
        }
    }
    map.into_iter()
        .map(|((kind, no), solver_id)| ResultCase {
            kind,
            no,
            name: String::new(),
            solver_id,
            members: Vec::new(),
        })
        .collect()
}

/// Fills in each case's user name from its `*Impl*` row (`name` when `isNameEnabled`).
fn fill_case_names(conn: &Connection, cases: &mut [ResultCase]) {
    for kind in CaseKind::ALL {
        if !cases.iter().any(|c| c.kind == kind) {
            continue;
        }
        let names = case_names(conn, kind.db_table());
        for case in cases.iter_mut().filter(|c| c.kind == kind) {
            if let Some(name) = names.get(&case.no) {
                case.name = name.clone();
            }
        }
    }
}

/// Fills `members` of design situations (their generated action combinations' load
/// combinations) and result combinations (their generated sub-combinations), as solver ids.
fn fill_members(conn: &Connection, cases: &mut [ResultCase]) {
    let solver_of: BTreeMap<u32, u32> = cases
        .iter()
        .filter(|c| c.kind == CaseKind::LoadCombination)
        .map(|c| (c.no, c.solver_id))
        .collect();
    let ids = |sql: &str, key: i64| -> Vec<u32> {
        let Ok(mut stmt) = conn.prepare(sql) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([key], |r| r.get::<_, i64>(0)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for lc_id in rows.flatten() {
            let no: Option<i64> = conn
                .query_row("SELECT userID FROM LoadCombination WHERE id=?1", [lc_id], |r| {
                    r.get(0)
                })
                .ok();
            if let Some(sid) = no.and_then(|n| solver_of.get(&(n as u32))) {
                if !out.contains(sid) {
                    out.push(*sid);
                }
            }
        }
        out
    };
    for case in cases.iter_mut() {
        let (table, sql) = match case.kind {
            CaseKind::DesignSituation => (
                "DesignSituation",
                "SELECT g.reference_id FROM ActionCombinationImpl a                  JOIN ActionCombinationImpl_generatedCombinations g ON g.id = a.id                  WHERE a.designSituation_id = ?1 AND g.reference_table = 'LoadCombination'",
            ),
            CaseKind::ResultCombination => (
                "ResultCombination",
                "SELECT reference_id FROM ResultCombinationImpl_generatedSubCombinations                  WHERE id = ?1 AND reference_table = 'LoadCombination'",
            ),
            _ => continue,
        };
        let key: Option<i64> = conn
            .query_row(
                &format!("SELECT impl_id FROM {table} WHERE userID=?1"),
                [case.no],
                |r| r.get(0),
            )
            .ok();
        if let Some(key) = key {
            // A design situation's `impl_id` equals its row id in RFEM files; use the row id.
            let key = if case.kind == CaseKind::DesignSituation {
                conn.query_row(
                    &format!("SELECT id FROM {table} WHERE userID=?1"),
                    [case.no],
                    |r| r.get(0),
                )
                .unwrap_or(key)
            } else {
                key
            };
            case.members = ids(sql, key);
        }
        // Explicit load combinations (imported models) name their design situation themselves.
        if case.kind == CaseKind::DesignSituation
            && let Ok(ds_id) = conn.query_row(
                "SELECT id FROM DesignSituation WHERE userID=?1",
                [case.no],
                |r| r.get::<_, i64>(0),
            )
            && let Ok(mut stmt) = conn.prepare(
                "SELECT lc.userID FROM LoadCombination lc \
                 JOIN LoadCombinationImplStatic i ON i.id = lc.impl_id \
                 WHERE lc.impl_table = 'LoadCombinationImplStatic' AND i.designSituation_id = ?1 \
                 ORDER BY lc.userID",
            )
            && let Ok(rows) = stmt.query_map([ds_id], |r| r.get::<_, i64>(0))
        {
            for no in rows.flatten() {
                if let Some(sid) = solver_of.get(&(no as u32))
                    && !case.members.contains(sid)
                {
                    case.members.push(*sid);
                }
            }
        }
    }
}

/// `userID -> name` for one case table, reading the name from each row's impl table.
fn case_names(conn: &Connection, table: &str) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    let query = format!("SELECT userID, impl_id, impl_table FROM \"{table}\"");
    let Ok(mut stmt) = conn.prepare(&query) else {
        return out;
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)? as u32,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    });
    let Ok(rows) = rows else {
        return out;
    };
    for row in rows.flatten() {
        let (no, impl_id, impl_table) = row;
        if !is_safe_table(&impl_table) {
            continue;
        }
        // `name` is empty / absent unless the user set one; tolerate tables without the column.
        let name: Option<String> = conn
            .query_row(
                &format!("SELECT name FROM \"{impl_table}\" WHERE id=?1"),
                [impl_id],
                |r| r.get::<_, String>(0),
            )
            .ok();
        if let Some(name) = name {
            let name = name.trim();
            if !name.is_empty() {
                out.insert(no, name.to_owned());
            }
        }
    }
    out
}

/// A plain SQLite identifier (defends the formatted table name even though the file is our own).
fn is_safe_table(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reads an XML-style `name="value"` attribute from `s` (first occurrence).
fn attr<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = s.find(&key)? + key.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

// ---------------------------------------------------------------------------
// Result quantity catalogue (the navigator tree)
// ---------------------------------------------------------------------------

/// Which set of radio buttons a leaf belongs to. Only one leaf per group is active at a time:
/// one displayed result quantity ([`RadioGroup::Result`]), plus the orthogonal surface
/// [`RadioGroup::LayerSide`] modifier (top / middle / bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadioGroup {
    /// The single result quantity shown in the view.
    Result,
    /// Which surface layer (top / middle / bottom) surface results are read at.
    LayerSide,
}

/// A single selectable item in the navigator tree (a radio leaf).
///
/// These name *what* to show; the numeric decoding behind each one is added later, per component.
/// The set mirrors RFEM 6's Results navigator (confirmed against the Static Analysis tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResultComponent {
    // Global (nodal) deformations.
    GlobalU,
    GlobalUx,
    GlobalUy,
    GlobalUz,
    GlobalPhiX,
    GlobalPhiY,
    GlobalPhiZ,
    // Member local deformations.
    MemberUx,
    MemberUy,
    MemberUz,
    MemberPhiX,
    MemberPhiY,
    MemberPhiZ,
    // Member internal forces.
    MemberN,
    MemberVy,
    MemberVz,
    MemberMt,
    MemberMy,
    MemberMz,
    // Surface layer side (a modifier, not a result value).
    LayerTop,
    LayerMiddle,
    LayerBottom,
    // Surface local deformations.
    SurfaceUx,
    SurfaceUy,
    SurfaceUz,
    SurfacePhiX,
    SurfacePhiY,
    SurfacePhiZ,
    // Surface basic internal forces.
    SurfaceMx,
    SurfaceMy,
    SurfaceMxy,
    SurfaceVx,
    SurfaceVy,
    SurfaceNx,
    SurfaceNy,
    SurfaceNxy,
    // Surface principal internal forces.
    SurfaceM1,
    SurfaceM2,
    SurfaceAlphaB,
    SurfaceMtMaxB,
    SurfaceVMaxB,
    SurfaceBetaB,
    SurfaceN1,
    SurfaceN2,
    SurfaceAlphaM,
    SurfaceVMaxM,
    // Surface design internal forces.
    SurfaceMxDTop,
    SurfaceMyDTop,
    SurfaceMcDTop,
    SurfaceMxDBottom,
    SurfaceMyDBottom,
    SurfaceMcDBottom,
    SurfaceNxD,
    SurfaceNyD,
    SurfaceNcD,
    // Surface stresses (basic / principal). Sub-structure from RFEM's standard set, to confirm.
    SurfaceSigmaX,
    SurfaceSigmaY,
    SurfaceTauXy,
    SurfaceTauXz,
    SurfaceTauYz,
    SurfaceSigma1,
    SurfaceSigma2,
    SurfaceSigmaEqv,
    // Surface strains (basic). To confirm.
    SurfaceEpsilonX,
    SurfaceEpsilonY,
    SurfaceGammaXy,
    // Surface contact stresses.
    SurfaceContactSigmaZ,
    SurfaceContactTauYz,
    SurfaceContactTauXz,
    // Support reactions.
    NodalSupportReaction,
    LineSupportReaction,
    MemberSupportReaction,
    SurfaceSupportReaction,
}

impl ResultComponent {
    /// Display label, unit and the SI → display-unit factor for a displayable quantity. The
    /// factor converts the stored SI value (m, rad) to the unit shown (mm, mrad). Returns empty
    /// strings and a factor of `1.0` for quantities this build cannot yet display.
    pub fn display(self) -> (&'static str, &'static str, f64) {
        use ResultComponent::*;
        match self {
            GlobalU => ("|u|", "mm", 1000.0),
            GlobalUx | SurfaceUx | MemberUx => ("u_x", "mm", 1000.0),
            GlobalUy | SurfaceUy | MemberUy => ("u_y", "mm", 1000.0),
            GlobalUz | SurfaceUz | MemberUz => ("u_z", "mm", 1000.0),
            GlobalPhiX | SurfacePhiX | MemberPhiX => ("φ_x", "mrad", 1000.0),
            GlobalPhiY | SurfacePhiY | MemberPhiY => ("φ_y", "mrad", 1000.0),
            GlobalPhiZ | SurfacePhiZ | MemberPhiZ => ("φ_z", "mrad", 1000.0),
            SurfaceContactSigmaZ => ("σ_z", "kN/m²", 1e-3),
            // Surface basic internal forces (SI N·m/m, N/m → kNm/m, kN/m).
            SurfaceMx => ("m_x", "kNm/m", 1e-3),
            SurfaceMy => ("m_y", "kNm/m", 1e-3),
            SurfaceMxy => ("m_xy", "kNm/m", 1e-3),
            SurfaceVx => ("v_x", "kN/m", 1e-3),
            SurfaceVy => ("v_y", "kN/m", 1e-3),
            SurfaceNx => ("n_x", "kN/m", 1e-3),
            SurfaceNy => ("n_y", "kN/m", 1e-3),
            SurfaceNxy => ("n_xy", "kN/m", 1e-3),
            _ => ("", "", 1.0),
        }
    }
}

/// A radio-selectable leaf: a component, its display label, SI unit and radio group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultLeaf {
    pub component: ResultComponent,
    /// Short label (`u_z`, `m_x`, `N`).
    pub label: &'static str,
    /// SI unit shown next to values (`mm`, `kNm/m`, `kN`); empty for a modifier like layer side.
    pub unit: &'static str,
    pub group: RadioGroup,
}

/// A node of the navigator tree: either a tick-box category (which may nest) or a radio leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultNode {
    Category {
        label: &'static str,
        /// Whether the category is expanded by default.
        default_open: bool,
        children: Vec<ResultNode>,
    },
    Leaf(ResultLeaf),
}

/// A result-quantity leaf (radio group [`RadioGroup::Result`]).
const fn leaf(component: ResultComponent, label: &'static str, unit: &'static str) -> ResultNode {
    ResultNode::Leaf(ResultLeaf {
        component,
        label,
        unit,
        group: RadioGroup::Result,
    })
}

/// A layer-side modifier leaf (radio group [`RadioGroup::LayerSide`]).
const fn layer(component: ResultComponent, label: &'static str) -> ResultNode {
    ResultNode::Leaf(ResultLeaf {
        component,
        label,
        unit: "",
        group: RadioGroup::LayerSide,
    })
}

fn category(label: &'static str, default_open: bool, children: Vec<ResultNode>) -> ResultNode {
    ResultNode::Category {
        label,
        default_open,
        children,
    }
}

/// The result-quantity tree available for `model`, mirroring RFEM 6's Results navigator.
/// Top-level categories whose objects the model lacks (no members, no surfaces, ...) are left out.
pub fn result_tree(model: &femcraft_model::Model) -> Vec<ResultNode> {
    use ResultComponent::*;
    use femcraft_model::ObjectKind as K;
    let mut tree = Vec::new();

    // Global deformations apply to any model with geometry.
    if model.count(K::Node) > 0 {
        tree.push(category(
            "Global Deformations",
            true,
            vec![
                leaf(GlobalU, "|u|", "mm"),
                leaf(GlobalUx, "u_x", "mm"),
                leaf(GlobalUy, "u_y", "mm"),
                leaf(GlobalUz, "u_z", "mm"),
                leaf(GlobalPhiX, "φ_x", "mrad"),
                leaf(GlobalPhiY, "φ_y", "mrad"),
                leaf(GlobalPhiZ, "φ_z", "mrad"),
            ],
        ));
    }

    if model.count(K::Member) > 0 {
        tree.push(category(
            "Members",
            false,
            vec![
                category(
                    "Local Deformations",
                    false,
                    vec![
                        leaf(MemberUx, "u_x", "mm"),
                        leaf(MemberUy, "u_y", "mm"),
                        leaf(MemberUz, "u_z", "mm"),
                        leaf(MemberPhiX, "φ_x", "mrad"),
                        leaf(MemberPhiY, "φ_y", "mrad"),
                        leaf(MemberPhiZ, "φ_z", "mrad"),
                    ],
                ),
                category(
                    "Internal Forces",
                    true,
                    vec![
                        leaf(MemberN, "N", "kN"),
                        leaf(MemberVy, "V_y", "kN"),
                        leaf(MemberVz, "V_z", "kN"),
                        leaf(MemberMt, "M_T", "kNm"),
                        leaf(MemberMy, "M_y", "kNm"),
                        leaf(MemberMz, "M_z", "kNm"),
                    ],
                ),
            ],
        ));
    }

    if model.count(K::Surface) > 0 {
        tree.push(category(
            "Surfaces",
            true,
            vec![
                category(
                    "Layer Side",
                    false,
                    vec![
                        layer(LayerTop, "Top"),
                        layer(LayerMiddle, "Middle"),
                        layer(LayerBottom, "Bottom"),
                    ],
                ),
                category(
                    "Local Deformations",
                    false,
                    vec![
                        leaf(SurfaceUx, "u_x", "mm"),
                        leaf(SurfaceUy, "u_y", "mm"),
                        leaf(SurfaceUz, "u_z", "mm"),
                        leaf(SurfacePhiX, "φ_x", "mrad"),
                        leaf(SurfacePhiY, "φ_y", "mrad"),
                        leaf(SurfacePhiZ, "φ_z", "mrad"),
                    ],
                ),
                category(
                    "Internal Forces",
                    true,
                    vec![
                        category(
                            "Basic Internal Forces",
                            true,
                            vec![
                                leaf(SurfaceMx, "m_x", "kNm/m"),
                                leaf(SurfaceMy, "m_y", "kNm/m"),
                                leaf(SurfaceMxy, "m_xy", "kNm/m"),
                                leaf(SurfaceVx, "v_x", "kN/m"),
                                leaf(SurfaceVy, "v_y", "kN/m"),
                                leaf(SurfaceNx, "n_x", "kN/m"),
                                leaf(SurfaceNy, "n_y", "kN/m"),
                                leaf(SurfaceNxy, "n_xy", "kN/m"),
                            ],
                        ),
                        category(
                            "Principal Internal Forces",
                            false,
                            vec![
                                leaf(SurfaceM1, "m_1", "kNm/m"),
                                leaf(SurfaceM2, "m_2", "kNm/m"),
                                leaf(SurfaceAlphaB, "α_b", "°"),
                                leaf(SurfaceMtMaxB, "m_T,max,b", "kNm/m"),
                                leaf(SurfaceVMaxB, "v_max,b", "kN/m"),
                                leaf(SurfaceBetaB, "β_b", "°"),
                                leaf(SurfaceN1, "n_1", "kN/m"),
                                leaf(SurfaceN2, "n_2", "kN/m"),
                                leaf(SurfaceAlphaM, "α_m", "°"),
                                leaf(SurfaceVMaxM, "v_max,m", "kN/m"),
                            ],
                        ),
                        category(
                            "Design Internal Forces",
                            false,
                            vec![
                                leaf(SurfaceMxDTop, "m_x,D+", "kNm/m"),
                                leaf(SurfaceMyDTop, "m_y,D+", "kNm/m"),
                                leaf(SurfaceMcDTop, "m_c,D+", "kNm/m"),
                                leaf(SurfaceMxDBottom, "m_x,D-", "kNm/m"),
                                leaf(SurfaceMyDBottom, "m_y,D-", "kNm/m"),
                                leaf(SurfaceMcDBottom, "m_c,D-", "kNm/m"),
                                leaf(SurfaceNxD, "n_x,D", "kN/m"),
                                leaf(SurfaceNyD, "n_y,D", "kN/m"),
                                leaf(SurfaceNcD, "n_c,D", "kN/m"),
                            ],
                        ),
                    ],
                ),
                category(
                    "Stresses",
                    false,
                    vec![
                        category(
                            "Basic Stresses",
                            false,
                            vec![
                                leaf(SurfaceSigmaX, "σ_x", "N/mm²"),
                                leaf(SurfaceSigmaY, "σ_y", "N/mm²"),
                                leaf(SurfaceTauXy, "τ_xy", "N/mm²"),
                                leaf(SurfaceTauXz, "τ_xz", "N/mm²"),
                                leaf(SurfaceTauYz, "τ_yz", "N/mm²"),
                            ],
                        ),
                        category(
                            "Principal Stresses",
                            false,
                            vec![
                                leaf(SurfaceSigma1, "σ_1", "N/mm²"),
                                leaf(SurfaceSigma2, "σ_2", "N/mm²"),
                                leaf(SurfaceSigmaEqv, "σ_eqv", "N/mm²"),
                            ],
                        ),
                    ],
                ),
                category(
                    "Strains",
                    false,
                    vec![category(
                        "Basic Strains",
                        false,
                        vec![
                            leaf(SurfaceEpsilonX, "ε_x", "‰"),
                            leaf(SurfaceEpsilonY, "ε_y", "‰"),
                            leaf(SurfaceGammaXy, "γ_xy", "‰"),
                        ],
                    )],
                ),
                category(
                    "Contact Stresses",
                    false,
                    vec![
                        leaf(SurfaceContactSigmaZ, "σ_z", "kN/m²"),
                        leaf(SurfaceContactTauYz, "τ_yz", "kN/m²"),
                        leaf(SurfaceContactTauXz, "τ_xz", "kN/m²"),
                    ],
                ),
            ],
        ));
    }

    // Support reactions: one category, a leaf per support kind present.
    let mut reactions = Vec::new();
    if model.count(K::NodalSupport) > 0 {
        reactions.push(leaf(NodalSupportReaction, "Nodal supports", "kN"));
    }
    if model.count(K::LineSupport) > 0 {
        reactions.push(leaf(LineSupportReaction, "Line supports", "kN/m"));
    }
    if model.count(K::MemberSupport) > 0 {
        reactions.push(leaf(MemberSupportReaction, "Member supports", "kN/m"));
    }
    if model.count(K::SurfaceSupport) > 0 {
        reactions.push(leaf(SurfaceSupportReaction, "Surface supports", "kN/m²"));
    }
    if !reactions.is_empty() {
        tree.push(category("Support Reactions", false, reactions));
    }

    tree
}

// ---------------------------------------------------------------------------
// Result value decoding (binary `!solver/RN_*` files)
// ---------------------------------------------------------------------------

/// Expected magic in the header of a `Data.RN*` nodal-results file.
const RN_MAGIC: i32 = 601;

/// Per-FE-node nodal displacements for one result case (SI units: metres, radians).
///
/// Decoded from `!solver/RN_*/Data.RN<solver_id>` (confirmed on RFEM 6.16, results v18.00):
/// a header of little-endian `i32`s where index 0 is the magic `601`, index 3 the number of
/// components (`6` = u_x, u_y, u_z, φ_x, φ_y, φ_z), index 4 the FE node count, and index 48 the
/// byte offset of the displacement block. The block is `count` records of six `f64`, in the same
/// FE-node order as [`crate::FeMesh`]. The file suffix is the case's solver id
/// (`Data.RN000028` = solver id 28), which [`ResultCase::solver_id`] carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Displacements {
    /// FE node `n` (1-based) -> `[u_x, u_y, u_z, φ_x, φ_y, φ_z]`; `nodal[n - 1]`.
    pub nodal: Vec<[f64; 6]>,
}

impl Displacements {
    /// Reads nodal displacements for the case whose solver id is `solver_id`. `fe_nodes` is the
    /// FE mesh node count to cross-check against (pass 0 to skip the check). `Ok(None)` when the
    /// file holds no results for that case or is not a recognised nodal-results block.
    pub fn read(archive: &Archive, solver_id: u32, fe_nodes: usize) -> Result<Option<Self>, Error> {
        let suffix = format!("Data.RN{solver_id:06}");
        let Some(entry) = archive
            .entries
            .iter()
            .find(|e| e.starts_with("!solver/RN") && e.ends_with(&suffix))
        else {
            return Self::read_members(archive, solver_id, fe_nodes);
        };
        let bytes = archive.read_entry(entry)?;
        if bytes.len() < 240 {
            return Err(Error::Format("results file too short".into()));
        }
        let i32_at = |i: usize| i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        if i32_at(0) != RN_MAGIC {
            return Ok(None);
        }
        // A mesh of 1D elements only (a frame): its results, in `RN_` or `RO_` folders, have
        // the element-end layout; the nodal block below holds something else there.
        if Self::is_member_mesh(archive, &bytes) {
            return Self::read_members_entry(archive, entry, fe_nodes);
        }
        let components = i32_at(3) as usize;
        let count = i32_at(4).max(0) as usize;
        let offset = i32_at(48).max(0) as usize;
        if components != 6 || count == 0 || offset == 0 || offset + count * 48 > bytes.len() {
            return Err(Error::Format("unexpected results layout".into()));
        }
        if fe_nodes != 0 && count != fe_nodes {
            return Err(Error::Format(format!(
                "results have {count} nodes, FE mesh has {fe_nodes}"
            )));
        }
        let f64_at = |o: usize| f64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
        let mut nodal = Vec::with_capacity(count);
        for i in 0..count {
            let base = offset + i * 48;
            let mut rec = [0.0f64; 6];
            for (k, v) in rec.iter_mut().enumerate() {
                let x = f64_at(base + k * 8);
                if !x.is_finite() || x.abs() > 1e9 {
                    return Err(Error::Format(
                        "implausible displacement value: not a nodal displacement block".into(),
                    ));
                }
                *v = x;
            }
            nodal.push(rec);
        }
        Ok(Some(Displacements { nodal }))
    }

    /// Displacements of 1D member elements: `!solver/RO_*/Data.RO<solver_id>` (RFEM 6.16,
    /// results v18.00). Same 601 header (`[3]` = 6 components, `[4]` = FE node count, `[5]` =
    /// element count). The block at the byte offset in `i32[155]` holds, per element, the
    /// displacements `[u_x, u_y, u_z, φ_x, φ_y, φ_z]` of its two end nodes (12 x f64), in
    /// `Data.E1D` element order (pairs of FE node numbers). Both ends of neighbouring elements
    /// agree, so the result is filled per FE node. Axes: member local, equal to global for
    /// members along X.
    fn read_members(
        archive: &Archive,
        solver_id: u32,
        fe_nodes: usize,
    ) -> Result<Option<Self>, Error> {
        let suffix = format!("Data.RO{solver_id:06}");
        let Some(entry) = archive
            .entries
            .iter()
            .find(|e| e.starts_with("!solver/RO") && e.ends_with(&suffix))
        else {
            return Ok(None);
        };
        Self::read_members_entry(archive, entry, fe_nodes)
    }

    /// True when the results file's own header says it holds the element-end block of 1D
    /// elements: a positive offset in `i32[155]` (-1 in a surface mesh's results) and a
    /// positive element count in `i32[5]` (0 there), with `Data.E1D` to place them. The
    /// header, not the folder name (`RN_` or `RO_`), tells the layouts apart.
    fn is_member_mesh(archive: &Archive, bytes: &[u8]) -> bool {
        if bytes.len() < 1024 {
            return false;
        }
        let i32_at = |i: usize| i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        i32_at(5) > 0
            && i32_at(155) > 0
            && archive.entries.iter().any(|e| e == "!solver/M/Data.E1D")
    }

    /// The member displacements of the results file `entry` (see [`Self::read_members`]).
    fn read_members_entry(
        archive: &Archive,
        entry: &str,
        fe_nodes: usize,
    ) -> Result<Option<Self>, Error> {
        let bytes = archive.read_entry(entry)?;
        if bytes.len() < 1024 {
            return Err(Error::Format("member results file too short".into()));
        }
        let i32_at = |i: usize| i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        if i32_at(0) != RN_MAGIC || i32_at(3) != 6 {
            return Ok(None);
        }
        let count = i32_at(4).max(0) as usize;
        let elements = i32_at(5).max(0) as usize;
        let offset = i32_at(155).max(0) as usize;
        if count == 0 || elements == 0 || offset == 0 || offset + elements * 96 > bytes.len() {
            return Err(Error::Format("unexpected member results layout".into()));
        }
        if fe_nodes != 0 && count != fe_nodes {
            return Err(Error::Format(format!(
                "results have {count} nodes, FE mesh has {fe_nodes}"
            )));
        }
        // Mixed 1D + 2D models (RFEM 6.16): the nodal block at `i32[48]` holds `count` records
        // of six `f32` (u_x, u_y, u_z, φ_x, φ_y, φ_z, global axes, 24 bytes each) for every FE
        // node, members and surfaces alike; it ends where the block at `i32[160]` starts.
        // Checked against RFEM's API on a slab + frame model (max |u| 6.105 mm, node values
        // match); the member element-end block below agrees in magnitude but is member-local.
        if i32_at(6) > 0 {
            let off = i32_at(48).max(0) as usize;
            if off > 0 && off + count * 24 <= bytes.len() {
                let mut nodal = Vec::with_capacity(count);
                let mut ok = true;
                'rd: for i in 0..count {
                    let mut rec = [0.0f64; 6];
                    for (k, v) in rec.iter_mut().enumerate() {
                        let at = off + i * 24 + k * 4;
                        let x = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as f64;
                        if !x.is_finite() || x.abs() > 1e6 {
                            ok = false;
                            break 'rd;
                        }
                        *v = x;
                    }
                    nodal.push(rec);
                }
                if ok {
                    return Ok(Some(Displacements { nodal }));
                }
            }
        }
        // Element connectivity: the one beside the results, else the mesh's `Data.E1D`.
        let folder = &entry[..entry.rfind('/').unwrap_or(0)];
        let e1d_name = [format!("{folder}/Data.E1D"), "!solver/M/Data.E1D".to_owned()]
            .into_iter()
            .find(|n| archive.entries.iter().any(|e| e == n))
            .ok_or_else(|| Error::Format("no Data.E1D for the member results".into()))?;
        let e1d = archive.read_entry(&e1d_name)?;
        if e1d.len() < elements * 8 {
            return Err(Error::Format(
                "Data.E1D is shorter than the element count".into(),
            ));
        }
        let f64_at = |o: usize| f64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
        let mut nodal = vec![[0.0f64; 6]; count];
        // Track nodes set by member element-end data so we don't overwrite them with surface data.
        let mut member_node = vec![false; count];
        for el in 0..elements {
            for end in 0..2 {
                let at = el * 8 + end * 4;
                let n = i32::from_le_bytes(e1d[at..at + 4].try_into().unwrap());
                if n < 1 || n as usize > count {
                    return Err(Error::Format(format!("FE element node {n} out of range")));
                }
                let base = offset + el * 96 + end * 48;
                for k in 0..6 {
                    let x = f64_at(base + k * 8);
                    if !x.is_finite() {
                        return Err(Error::Format("non-finite displacement value".into()));
                    }
                    nodal[n as usize - 1][k] = x;
                }
                member_node[n as usize - 1] = true;
            }
        }
        Ok(Some(Displacements { nodal }))
    }

    /// Displacement of FE node `no` (1-based): `[u_x, u_y, u_z, φ_x, φ_y, φ_z]`.
    pub fn node(&self, no: u32) -> Option<[f64; 6]> {
        self.nodal.get((no as usize).checked_sub(1)?).copied()
    }

    /// Largest translational displacement magnitude over all nodes (metres).
    pub fn max_translation(&self) -> f64 {
        self.nodal
            .iter()
            .map(|u| (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt())
            .fold(0.0, f64::max)
    }
}

/// Per-FE-node surface internal forces for one result case, in each surface's local axes (SI:
/// moments N·m/m, shear / axial N/m), built from the stored generalised-strain block.
///
/// RFEM does not store the surface forces; it rebuilds them from the per-element-corner
/// generalised strains in the block at header `i32[163]` (see [`crate::surface_moments`]). For
/// each 2D element this reads the 8 strains at its 4 corners, applies the surface's isotropic
/// plate law ([`crate::surface_moments::basic_forces`]) and averages the per-element values at
/// each FE node of that surface (RFEM's "continuous within surfaces" smoothing). Confirmed on
/// `Chamber 4x4x6.rf6` (LC1): the non-planar walls to < 5 %, the base slab to ~10 % (sharper
/// moment gradients at the wall junctions).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceForces {
    /// FE node `n` (1-based) -> smoothed forces; `node[n - 1]`. `None` where no surface element
    /// touches the node (e.g. member-only nodes).
    pub node: Vec<Option<crate::surface_moments::BasicForces>>,
}

impl SurfaceForces {
    /// Reads the surface forces for the case whose solver id is `solver_id`. `mesh` supplies the
    /// 2D element connectivity and each element's surface (in `Data.E2D` order, matching the
    /// block); `stiffness` maps a surface number to its plate stiffness (elements whose surface is
    /// absent from the map are skipped). `Ok(None)` when the file holds no strain block for the
    /// case.
    pub fn read(
        archive: &Archive,
        solver_id: u32,
        mesh: &crate::FeMesh,
        stiffness: &BTreeMap<u32, crate::surface_moments::PlateStiffness>,
    ) -> Result<Option<Self>, Error> {
        use crate::surface_moments::{basic_forces, BasicForces, GeneralisedStrains};
        let suffix = format!("Data.RN{solver_id:06}");
        let Some(entry) = archive
            .entries
            .iter()
            .find(|e| e.starts_with("!solver/RN") && e.ends_with(&suffix))
        else {
            return Ok(None);
        };
        let bytes = archive.read_entry(entry)?;
        if bytes.len() < 656 {
            return Err(Error::Format("results file too short for surface forces".into()));
        }
        let i32_at = |i: usize| i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        if i32_at(0) != RN_MAGIC {
            return Ok(None);
        }
        let elements = mesh.elements.len();
        let offset = i32_at(163).max(0) as usize;
        // 32 f64 per element: 4 corners x 8 generalised strains.
        if offset == 0 || elements == 0 || offset + elements * 256 > bytes.len() {
            return Ok(None);
        }
        let f64_at = |o: usize| f64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
        let n_nodes = mesh.nodes.len();
        let mut sum = vec![[0.0f64; 8]; n_nodes];
        let mut cnt = vec![0u32; n_nodes];
        for (i, elem) in mesh.elements.iter().enumerate() {
            let surf = mesh.element_surface.get(i).copied().unwrap_or(0);
            let Some(k) = stiffness.get(&surf) else {
                continue;
            };
            for (corner, &no) in results_element_corners(elem).iter().enumerate() {
                if no < 1 || no as usize > n_nodes {
                    continue;
                }
                let base = offset + i * 256 + corner * 64;
                let g = GeneralisedStrains {
                    kappa_x: f64_at(base),
                    kappa_y: f64_at(base + 8),
                    kappa_xy: f64_at(base + 16),
                    gamma_xz: f64_at(base + 24),
                    gamma_yz: f64_at(base + 32),
                    epsilon_x: f64_at(base + 40),
                    epsilon_y: f64_at(base + 48),
                    epsilon_xy: f64_at(base + 56),
                };
                let f = basic_forces(&g, k);
                let acc = &mut sum[no as usize - 1];
                for (a, v) in acc.iter_mut().zip([
                    f.mx, f.my, f.mxy, f.vx, f.vy, f.nx, f.ny, f.nxy,
                ]) {
                    *a += v;
                }
                cnt[no as usize - 1] += 1;
            }
        }
        let node = (0..n_nodes)
            .map(|n| {
                (cnt[n] > 0).then(|| {
                    let s = sum[n];
                    let c = cnt[n] as f64;
                    BasicForces {
                        mx: s[0] / c,
                        my: s[1] / c,
                        mxy: s[2] / c,
                        vx: s[3] / c,
                        vy: s[4] / c,
                        nx: s[5] / c,
                        ny: s[6] / c,
                        nxy: s[7] / c,
                    }
                })
            })
            .collect();
        Ok(Some(SurfaceForces { node }))
    }

    /// Forces at FE node `no` (1-based).
    pub fn node(&self, no: u32) -> Option<crate::surface_moments::BasicForces> {
        self.node.get((no as usize).checked_sub(1)?).copied().flatten()
    }
}

/// The scalar a surface-force result component reads off [`BasicForces`] (SI), or `None` for a
/// component this build does not yet derive (principal / design / stresses are computed from
/// these by [`crate::surface_moments`] but not wired to the viewer yet).
pub fn surface_force_scalar(
    component: ResultComponent,
    f: &crate::surface_moments::BasicForces,
) -> Option<f64> {
    use ResultComponent as C;
    Some(match component {
        C::SurfaceMx => f.mx,
        C::SurfaceMy => f.my,
        C::SurfaceMxy => f.mxy,
        C::SurfaceVx => f.vx,
        C::SurfaceVy => f.vy,
        C::SurfaceNx => f.nx,
        C::SurfaceNy => f.ny,
        C::SurfaceNxy => f.nxy,
        _ => return None,
    })
}

/// Corner node numbers of a mesh element: a triangle (last index 0 or repeated) or a quad.
fn results_element_corners(e: &[u32; 4]) -> &[u32] {
    if e[3] != 0 && e[3] != e[2] {
        &e[..4]
    } else {
        &e[..3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_history_into_sorted_cases() {
        let xml = r#"<?xml version="1.0"?>
        <solver_analyses>
          <static_analysis>
            <analysis solver_id="9">
              <case_object_index user_id="20" object_type="Load_Combination"/>
            </analysis>
            <analysis solver_id="1">
              <case_object_index user_id="1" object_type="Load_Case"/>
            </analysis>
            <analysis solver_id="48">
              <case_object_index user_id="2" object_type="Design_Situation"/>
            </analysis>
            <analysis solver_id="2">
              <case_object_index user_id="1" object_type="Load_Case"/>
            </analysis>
          </static_analysis>
        </solver_analyses>"#;
        let cases = parse_history(xml.as_bytes());
        // Deduplicated (LC1 seen twice) and sorted by (kind, no).
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].kind, CaseKind::LoadCase);
        assert_eq!(cases[0].no, 1);
        assert_eq!(cases[0].solver_id, 2); // last analysis for LC1 wins
        assert_eq!(cases[1].kind, CaseKind::LoadCombination);
        assert_eq!(cases[1].no, 20);
        assert_eq!(cases[1].tag(), "CO20");
        assert_eq!(cases[2].kind, CaseKind::DesignSituation);
    }

    #[test]
    fn uncalculated_cases_are_left_out() {
        let case = |kind, no, solver_id, members: Vec<u32>| ResultCase {
            kind,
            no,
            name: String::new(),
            solver_id,
            members,
        };
        let mut cases = vec![
            case(CaseKind::LoadCase, 1, 1, vec![]),
            case(CaseKind::LoadCombination, 1, 2, vec![]),
            case(CaseKind::DesignSituation, 1, 3, vec![2]),
            case(CaseKind::DesignSituation, 2, 9, vec![1]),
        ];
        let folders = vec!["RO_1_LC1 - SA1".to_owned(), "RN_4_LC2 - SA1".to_owned()];
        assert_eq!(folder_solver_id("RN_28_CO20 - SA2"), Some(28));
        drop_uncalculated(&mut cases, &folders);
        let kept: Vec<_> = cases.iter().map(|c| (c.kind, c.no)).collect();
        assert_eq!(
            kept,
            vec![(CaseKind::LoadCase, 1), (CaseKind::DesignSituation, 2)]
        );
        // Folders named by uuid: nothing can be matched, nothing is dropped.
        let mut all = vec![case(CaseKind::LoadCase, 1, 1, vec![])];
        drop_uncalculated(&mut all, &["RN_0a1b2c".to_owned()]);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn reads_attribute() {
        assert_eq!(attr(r#"solver_id="42" x="1""#, "solver_id"), Some("42"));
        assert_eq!(
            attr(r#"<results_version value="18.00"/>"#, "value"),
            Some("18.00")
        );
        assert_eq!(attr("nothing here", "value"), None);
    }

    #[test]
    fn case_label_uses_name_when_present() {
        let named = ResultCase {
            kind: CaseKind::LoadCase,
            no: 1,
            name: "Self-weight".into(),
            solver_id: 1,
            members: Vec::new(),
        };
        assert_eq!(named.label(), "LC1 Self-weight");
        let auto = ResultCase {
            kind: CaseKind::LoadCombination,
            no: 20,
            name: String::new(),
            solver_id: 9,
            members: Vec::new(),
        };
        assert_eq!(auto.label(), "CO20");
    }
}
