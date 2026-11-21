//! STAAD.Pro input files (`.std`): the command language STAAD and programs exporting to it
//! (Tekla Structural Designer, ...) write.
//!
//! Read: units, `SET Z UP`, joints, members, plate elements, member properties (prismatic
//! `PRI`), element thicknesses, isotropic materials, `CONSTANTS` (material / beta), supports
//! (`FIXED`, `PINNED`, `FIXED BUT ...`) and load case titles. Values may use a decimal comma
//! (`3,5`): exports from regional Windows settings do. Everything else is counted in
//! [`StaadModel::skipped`].

use std::collections::BTreeMap;

/// The parsed file, in the file's own axes but SI units (m, N, Pa).
#[derive(Debug, Default)]
pub struct StaadModel {
    /// `SET Z UP`: Z is vertical (else STAAD's default, Y up).
    pub z_up: bool,
    pub joints: BTreeMap<u32, [f64; 3]>,
    /// Member -> (start joint, end joint).
    pub members: BTreeMap<u32, (u32, u32)>,
    /// Plate element -> its 3 or 4 joints.
    pub plates: BTreeMap<u32, Vec<u32>>,
    pub member_props: BTreeMap<u32, Prismatic>,
    pub plate_thickness: BTreeMap<u32, f64>,
    pub materials: Vec<Material>,
    /// Member or plate -> material name (`CONSTANTS MATERIAL name MEMBER list`; STAAD numbers
    /// members and plates in one series).
    pub element_material: BTreeMap<u32, String>,
    /// Member -> beta angle (degrees).
    pub beta: BTreeMap<u32, f64>,
    /// Joint -> fixed degrees of freedom `[FX, FY, FZ, MX, MY, MZ]` in the file's axes.
    pub supports: BTreeMap<u32, [bool; 6]>,
    /// Joint -> spring constants of released degrees of freedom (`FIXED BUT FZ KFZ 7.5`), SI
    /// (N/m, N·m/rad), 0 = free.
    pub support_springs: BTreeMap<u32, [f64; 6]>,
    /// Joints with an inclined support (`INC dx dy dz`); imported in global axes.
    pub inclined_supports: BTreeMap<u32, [f64; 3]>,
    /// Load case number -> title.
    pub load_cases: BTreeMap<u32, String>,
    pub combinations: usize,
    /// Load case -> self-weight factors along the file's X, Y, Z (e.g. `[0, -1, 0]`).
    pub self_weight: Vec<(u32, [f64; 3])>,
    /// Uniform member loads (full length).
    pub member_loads: Vec<MemberLoad>,
    /// Load combinations: name and (load case, ULS factor, SLS factor) items.
    pub combination_defs: Vec<(String, Vec<(u32, f64, f64)>)>,
    /// `JOINT LOAD` records (forces N, moments N·m, file axes).
    pub joint_loads: Vec<JointLoad>,
    /// `ELEMENT LOAD` records on plates (pressures in Pa).
    pub element_loads: Vec<ElementLoad>,
    /// Commands not converted, with how many records each had.
    pub skipped: BTreeMap<String, usize>,
    /// Free-text notes for the import report (readers that decode only part of a file).
    pub notes: Vec<String>,
}

/// A uniform load along a whole member, N/m along a local z or a global axis (file axes).
#[derive(Debug, Clone)]
pub struct MemberLoad {
    pub lc: u32,
    pub member: u32,
    pub dir: PressureDir,
    pub w: f64,
}

/// A joint load: forces `[FX, FY, FZ]` (N) and moments `[MX, MY, MZ]` (N·m) on joints.
#[derive(Debug, Clone)]
pub struct JointLoad {
    pub lc: u32,
    pub joints: Vec<u32>,
    pub force: [f64; 3],
    pub moment: [f64; 3],
}

/// Direction of a plate pressure: local plate axis (0 = x, 1 = y, 2 = z) or global axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PressureDir {
    Local(usize),
    Global(usize),
}

/// Pressure value: one for the whole plate, or one per corner (`TRAP`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pressure {
    Uniform(f64),
    Corners([f64; 4]),
}

/// A pressure on plates (`PR`, `PRESSURE`, `TRAP ... JT`).
#[derive(Debug, Clone)]
pub struct ElementLoad {
    pub lc: u32,
    pub plates: Vec<u32>,
    pub dir: PressureDir,
    pub value: Pressure,
}

/// `PRI` member property (SI).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Prismatic {
    pub ax: f64,
    pub ix: f64,
    pub iy: f64,
    pub iz: f64,
    /// Depth along local y / width along local z when given (`YD` / `ZD`).
    pub yd: f64,
    pub zd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Material {
    pub name: String,
    pub e: f64,
    pub g: f64,
    pub poisson: f64,
    /// Weight density (N/m³).
    pub density: f64,
    pub alpha: f64,
}

/// Length and force factors to SI.
#[derive(Clone, Copy)]
struct Units {
    len: f64,
    force: f64,
}

fn length_unit(t: &str) -> Option<f64> {
    Some(match t {
        "METER" | "METERS" | "M" | "ME" => 1.0,
        "MMS" | "MM" | "MILLIMETER" | "MILLIMETERS" => 1e-3,
        "CM" | "CMS" | "CENTIMETER" | "CENTIMETERS" => 1e-2,
        "KM" => 1e3,
        "FEET" | "FT" | "FOOT" => 0.3048,
        "INCHES" | "INCH" | "IN" => 0.0254,
        _ => return None,
    })
}

fn force_unit(t: &str) -> Option<f64> {
    Some(match t {
        "NEWTON" | "NEWTONS" | "N" | "NEW" => 1.0,
        "KN" | "KNS" | "KILONEWTON" | "KILONEWTONS" => 1e3,
        "MN" | "MNS" | "MEGANEWTON" | "MEGANEWTONS" => 1e6,
        "KIP" | "KIPS" => 4448.221615,
        "POUND" | "POUNDS" | "LB" | "LBS" => 4.448221615,
        "KG" | "KGS" | "KILOGRAM" | "KILOGRAMS" => 9.80665,
        "MTON" | "MTONS" | "METRIC" => 9806.65,
        _ => return None,
    })
}

/// A number, accepting a decimal comma.
fn num(t: &str) -> Option<f64> {
    t.parse().ok().or_else(|| t.replace(',', ".").parse().ok())
}

fn int(t: &str) -> Option<u32> {
    t.parse().ok()
}

/// A STAAD list (`1 2 5 TO 9 12 TO 20 BY 2`); stops at the first token that is not part of it.
/// Returns the numbers and how many tokens were used.
fn parse_list(tokens: &[&str]) -> (Vec<u32>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(n) = int(tokens[i]) {
            out.push(n);
            i += 1;
        } else if tokens[i] == "TO" && i + 1 < tokens.len() {
            let (Some(&a), Some(b)) = (out.last(), int(tokens[i + 1])) else {
                break;
            };
            let mut step = 1;
            let mut used = 2;
            if tokens.get(i + 2) == Some(&"BY")
                && let Some(s) = tokens.get(i + 3).and_then(|t| int(t))
            {
                step = s.max(1);
                used = 4;
            }
            let mut k = a + step;
            while k <= b {
                out.push(k);
                k += step;
            }
            i += used;
        } else {
            break;
        }
    }
    (out, i)
}

/// Commands that start a block (first words, upper case).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Block {
    None,
    Joints,
    Members,
    Elements,
    MemberProperty,
    ElementProperty,
    DefineMaterial,
    Constants,
    Supports,
    Loading,
    JointLoad,
    ElementLoad,
    Combination,
    /// `START GROUP DEFINITION` / `START JOB INFORMATION` ... `END`.
    Group,
    /// Records skipped under this command name.
    Other,
}

/// Logical records: comments dropped, `-` continuations joined, `;` separated.
fn records(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('*') {
            continue;
        }
        let (body, cont) = match line.strip_suffix('-') {
            // A trailing ` -` continues the record (a negative number never ends a line).
            Some(b) if b.is_empty() || b.ends_with(char::is_whitespace) => (b, true),
            _ => (line, false),
        };
        cur.push(' ');
        cur.push_str(body);
        if !cont {
            for part in cur.split(';') {
                let p = part.trim();
                if !p.is_empty() {
                    out.push(p.to_owned());
                }
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

pub fn parse(text: &str) -> Result<StaadModel, String> {
    let mut m = StaadModel::default();
    let mut units = Units {
        len: 1.0,
        force: 1.0,
    };
    let mut block = Block::None;
    let mut other = String::new();
    let mut started = false;
    let mut cur_lc = 0u32;
    for rec in records(text) {
        let upper = rec.to_ascii_uppercase();
        let tokens: Vec<&str> = upper.split_whitespace().collect();
        let raw_tokens: Vec<&str> = rec.split_whitespace().collect();
        let first = tokens[0];
        let starts_with_number = first.starts_with(|c: char| c.is_ascii_digit());
        // Unit changes can appear anywhere.
        if first == "UNIT" {
            for t in &tokens[1..] {
                if let Some(l) = length_unit(t) {
                    units.len = l;
                } else if let Some(f) = force_unit(t) {
                    units.force = f;
                }
            }
            continue;
        }
        if !started {
            if first == "STAAD" {
                started = true;
                continue;
            }
            return Err("not a STAAD.Pro input file (no STAAD command first)".into());
        }
        if !starts_with_number {
            let two = tokens.get(1).copied().unwrap_or("");
            let new_block = match (first, two) {
                ("SET", _) => {
                    if upper.contains("Z UP") {
                        m.z_up = true;
                    }
                    Some(Block::None)
                }
                ("JOINT", "COORDINATES") | ("JOINT", "COORD") => Some(Block::Joints),
                ("MEMBER", "INCIDENCES") | ("MEMBER", "INCIDENCE") => Some(Block::Members),
                ("ELEMENT", "INCIDENCES") | ("ELEMENT", "INCIDENCE") => Some(Block::Elements),
                ("MEMBER", "PROPERTY") | ("MEMBER", "PROPERTIES") => Some(Block::MemberProperty),
                ("ELEMENT", "PROPERTY") | ("ELEMENT", "PROPERTIES") => Some(Block::ElementProperty),
                ("DEFINE", "MATERIAL") => Some(Block::DefineMaterial),
                ("END", "MATERIAL") | ("END", "DEFINE") => Some(Block::None),
                // Named groups and job information: nothing to model.
                ("START", "GROUP") | ("START", "JOB") => {
                    other = format!("{two} block");
                    *m.skipped.entry(other.clone()).or_default() += 1;
                    Some(Block::Group)
                }
                ("END", "GROUP") | ("END", "JOB") => Some(Block::None),
                _ if block == Block::Group => None,
                ("CONSTANTS", _) | ("CONSTANT", _) => Some(Block::Constants),
                ("SUPPORTS", _) | ("SUPPORT", _) => Some(Block::Supports),
                // `LOADING n ...` or `LOAD n LOADTYPE Dead TITLE ...`.
                ("LOADING", _) | ("LOAD", _) if first == "LOADING" || int(two).is_some() => {
                    let load_case = tokens.get(1).and_then(|t| int(t)).unwrap_or(0);
                    // `LOADING 1 TITLE 1 Self weight` / `LOADING 1 Self weight`.
                    let mut rest: Vec<&str> = raw_tokens[2.min(raw_tokens.len())..].to_vec();
                    if rest
                        .first()
                        .is_some_and(|t| t.eq_ignore_ascii_case("LOADTYPE"))
                    {
                        rest.drain(..2.min(rest.len()));
                    }
                    if rest
                        .first()
                        .is_some_and(|t| t.eq_ignore_ascii_case("TITLE"))
                    {
                        rest.remove(0);
                    }
                    if rest.first().and_then(|t| int(t)) == Some(load_case) && rest.len() > 1 {
                        rest.remove(0);
                    }
                    cur_lc = load_case;
                    if load_case > 0 {
                        m.load_cases.insert(load_case, rest.join(" "));
                    }
                    Some(Block::Loading)
                }
                ("LOAD", "COMBINATION") | ("LOAD", "COMB") => {
                    m.combinations += 1;
                    Some(Block::Combination)
                }
                ("JOINT", "LOAD") | ("ELEMENT", "LOAD")
                    if matches!(
                        block,
                        Block::Loading | Block::Other | Block::JointLoad | Block::ElementLoad
                    ) =>
                {
                    if first == "JOINT" {
                        Some(Block::JointLoad)
                    } else {
                        Some(Block::ElementLoad)
                    }
                }
                ("MEMBER", "LOAD")
                    if matches!(
                        block,
                        Block::Loading | Block::Other | Block::JointLoad | Block::ElementLoad
                    ) =>
                {
                    other = format!("{first} {two} (loads)");
                    *m.skipped.entry(other.clone()).or_default() += 0;
                    Some(Block::Other)
                }
                ("PERFORM", _) | ("FINISH", _) | ("PRINT", _) => Some(Block::None),
                // Material / beta lines inside CONSTANTS, property lines inside DEFINE MATERIAL.
                _ if block == Block::Constants || block == Block::DefineMaterial => None,
                _ => {
                    other = if two.is_empty() || two.starts_with(|c: char| c.is_ascii_digit()) {
                        first.to_owned()
                    } else {
                        format!("{first} {two}")
                    };
                    *m.skipped.entry(other.clone()).or_default() += 1;
                    Some(Block::Other)
                }
            };
            if let Some(b) = new_block {
                block = b;
                continue;
            }
        }
        let l = units.len;
        let f = units.force;
        match block {
            Block::Joints => {
                if let [n, x, y, z, ..] = tokens[..]
                    && let (Some(n), Some(x), Some(y), Some(z)) = (int(n), num(x), num(y), num(z))
                {
                    m.joints.insert(n, [x * l, y * l, z * l]);
                }
            }
            Block::Members => {
                if let [n, a, b, ..] = tokens[..]
                    && let (Some(n), Some(a), Some(b)) = (int(n), int(a), int(b))
                {
                    m.members.insert(n, (a, b));
                }
            }
            Block::Elements => {
                let ids: Vec<u32> = tokens.iter().map_while(|t| int(t)).collect();
                if ids.len() >= 4 {
                    m.plates.insert(ids[0], ids[1..ids.len().min(5)].to_vec());
                }
            }
            Block::MemberProperty => {
                let (list, used) = parse_list(&tokens);
                let rest = &tokens[used..];
                if rest.first() == Some(&"PRI") || rest.first() == Some(&"PRIS") {
                    let mut p = Prismatic::default();
                    let mut i = 1;
                    while i + 1 < rest.len() {
                        if let Some(v) = num(rest[i + 1]) {
                            match rest[i] {
                                "AX" => p.ax = v * l * l,
                                "IX" => p.ix = v * l.powi(4),
                                "IY" => p.iy = v * l.powi(4),
                                "IZ" => p.iz = v * l.powi(4),
                                "YD" => p.yd = v * l,
                                "ZD" => p.zd = v * l,
                                _ => {}
                            }
                        }
                        i += 2;
                    }
                    for n in list {
                        m.member_props.insert(n, p);
                    }
                } else {
                    let key = format!("MEMBER PROPERTY {}", rest.first().unwrap_or(&"?"));
                    *m.skipped.entry(key).or_default() += list.len();
                }
            }
            Block::ElementProperty => {
                let (list, used) = parse_list(&tokens);
                let rest = &tokens[used..];
                if let ["THI" | "THICKNESS", t, ..] = rest
                    && let Some(t) = num(t)
                {
                    for n in list {
                        m.plate_thickness.insert(n, t * l);
                    }
                }
            }
            Block::DefineMaterial => match first {
                "ISOTROPIC" => m.materials.push(Material {
                    name: raw_tokens.get(1).copied().unwrap_or("").to_owned(),
                    ..Default::default()
                }),
                _ => {
                    if let (Some(mat), Some(v)) =
                        (m.materials.last_mut(), tokens.get(1).and_then(|t| num(t)))
                    {
                        match first {
                            "E" => mat.e = v * f / (l * l),
                            "G" => mat.g = v * f / (l * l),
                            "POI" | "POISSON" => mat.poisson = v,
                            "DEN" | "DENSITY" => mat.density = v * f / (l * l * l),
                            "ALP" | "ALPHA" => mat.alpha = v,
                            _ => {}
                        }
                    }
                }
            },
            Block::Constants => {
                // `MATERIAL name MEMBER list`, `BETA angle MEMBER list` (also `ALL`).
                let target = tokens.iter().position(|t| *t == "MEMBER" || *t == "MEMB");
                let list = target
                    .map(|i| parse_list(&tokens[i + 1..]).0)
                    .unwrap_or_default();
                match first {
                    "MATERIAL" => {
                        let name = raw_tokens.get(1).copied().unwrap_or("").to_owned();
                        for n in list {
                            m.element_material.insert(n, name.clone());
                        }
                    }
                    "BETA" => {
                        if let Some(a) = tokens.get(1).and_then(|t| num(t)) {
                            for n in list {
                                m.beta.insert(n, a);
                            }
                        }
                    }
                    _ => *m.skipped.entry(format!("CONSTANTS {first}")).or_default() += 1,
                }
            }
            Block::Supports => {
                let (list, used) = parse_list(&tokens);
                let mut rest = &tokens[used..];
                // Inclined support: `INC dx dy dz` (a reference direction) before the type.
                let mut inc = None;
                if rest.first() == Some(&"INC") && rest.len() >= 4 {
                    if let (Some(x), Some(y), Some(z)) = (num(rest[1]), num(rest[2]), num(rest[3]))
                    {
                        inc = Some([x * l, y * l, z * l]);
                    }
                    rest = &rest[4..];
                }
                const DOFS: [&str; 6] = ["FX", "FY", "FZ", "MX", "MY", "MZ"];
                let mut springs = [0.0; 6];
                let dofs = match rest.first() {
                    Some(&"FIXED") => {
                        let mut d = [true; 6];
                        if rest.get(1) == Some(&"BUT") {
                            let mut i = 2;
                            while i < rest.len() {
                                let t = rest[i];
                                if let Some(k) = DOFS.iter().position(|k| *k == t) {
                                    d[k] = false;
                                } else if let Some(k) = t
                                    .strip_prefix('K')
                                    .and_then(|s| DOFS.iter().position(|k| *k == s))
                                    && let Some(v) = rest.get(i + 1).and_then(|v| num(v))
                                {
                                    // KFx in force / length, KMx in force x length / rad.
                                    d[k] = false;
                                    springs[k] = if k < 3 { v * f / l } else { v * f * l };
                                    i += 1;
                                }
                                i += 1;
                            }
                        }
                        Some(d)
                    }
                    Some(&"PINNED") => Some([true, true, true, false, false, false]),
                    _ => None,
                };
                match dofs {
                    Some(d) => {
                        for n in list {
                            m.supports.insert(n, d);
                            if springs.iter().any(|s| *s != 0.0) {
                                m.support_springs.insert(n, springs);
                            }
                            if let Some(inc) = inc {
                                m.inclined_supports.insert(n, inc);
                            }
                        }
                    }
                    None => *m.skipped.entry("SUPPORTS (other)".into()).or_default() += 1,
                }
            }
            Block::JointLoad => {
                let (list, used) = parse_list(&tokens);
                let rest = &tokens[used..];
                let (mut force, mut moment) = ([0.0; 3], [0.0; 3]);
                let mut i = 0;
                while i + 1 < rest.len() {
                    let v = num(rest[i + 1]);
                    match (rest[i], v) {
                        ("FX", Some(v)) => force[0] += v * f,
                        ("FY", Some(v)) => force[1] += v * f,
                        ("FZ", Some(v)) => force[2] += v * f,
                        ("MX", Some(v)) => moment[0] += v * f * l,
                        ("MY", Some(v)) => moment[1] += v * f * l,
                        ("MZ", Some(v)) => moment[2] += v * f * l,
                        _ => {}
                    }
                    i += 2;
                }
                if list.is_empty() || rest.is_empty() {
                    *m.skipped.entry("JOINT LOAD (other)".into()).or_default() += 1;
                } else {
                    m.joint_loads.push(JointLoad {
                        lc: cur_lc,
                        joints: list,
                        force,
                        moment,
                    });
                }
            }
            Block::ElementLoad => {
                let (list, used) = parse_list(&tokens);
                let rest = &tokens[used..];
                let pa = f / (l * l);
                let dir_of = |t: &str| match t {
                    "X" => Some(PressureDir::Local(0)),
                    "Y" => Some(PressureDir::Local(1)),
                    "Z" => Some(PressureDir::Local(2)),
                    "GX" => Some(PressureDir::Global(0)),
                    "GY" => Some(PressureDir::Global(1)),
                    "GZ" => Some(PressureDir::Global(2)),
                    _ => None,
                };
                let parsed = match rest.first() {
                    Some(&"PR") | Some(&"PRESSURE") => {
                        let (dir, at) = match rest.get(1).and_then(|t| dir_of(t)) {
                            Some(d) => (d, 2),
                            None => (PressureDir::Local(2), 1),
                        };
                        rest.get(at)
                            .and_then(|t| num(t))
                            .map(|v| (dir, Pressure::Uniform(v * pa)))
                    }
                    Some(&"TRAP") => {
                        let mut at = 1;
                        let dir = match rest.get(at).and_then(|t| dir_of(t)) {
                            Some(d) => {
                                at += 1;
                                d
                            }
                            None => PressureDir::Local(2),
                        };
                        if rest.get(at) == Some(&"JT") {
                            at += 1;
                        }
                        let v: Vec<f64> = rest[at.min(rest.len())..]
                            .iter()
                            .map_while(|t| num(t))
                            .map(|v| v * pa)
                            .collect();
                        (v.len() == 4).then(|| (dir, Pressure::Corners([v[0], v[1], v[2], v[3]])))
                    }
                    _ => None,
                };
                match parsed {
                    Some((dir, value)) if !list.is_empty() => m.element_loads.push(ElementLoad {
                        lc: cur_lc,
                        plates: list,
                        dir,
                        value,
                    }),
                    _ => *m.skipped.entry("ELEMENT LOAD (other)".into()).or_default() += 1,
                }
            }
            Block::Loading | Block::Combination | Block::Group | Block::None => {}
            Block::Other => *m.skipped.entry(other.clone()).or_default() += 1,
        }
    }
    if m.joints.is_empty() {
        return Err("no JOINT COORDINATES found".into());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_numbers() {
        assert_eq!(
            parse_list(&["1", "3", "TO", "5", "PRI"]),
            (vec![1, 3, 4, 5], 4)
        );
        assert_eq!(parse_list(&["2", "TO", "8", "BY", "3"]), (vec![2, 5, 8], 5));
        assert_eq!(num("3,5"), Some(3.5));
        assert_eq!(num("1E-06"), Some(1e-6));
    }

    #[test]
    fn small_file() {
        let text = "STAAD SPACE\nSET Z UP\nUNIT METER KN\nJOINT COORDINATES\n1 0 0 0; 2 0 0 3,5\n\
                    MEMBER INCIDENCES\n1 1 2\nMEMBER PROPERTY\n1 PRI AX 0,32 -\nIY 0,1 IZ 0,4\n\
                    SUPPORTS\n1 FIXED BUT MY\nLOADING 3 TITLE 3 Dead\nJOINT LOAD\n2 FZ -1\nFINISH\n";
        let m = parse(text).unwrap();
        assert!(m.z_up);
        assert_eq!(m.joints[&2], [0.0, 0.0, 3.5]);
        assert_eq!(m.members[&1], (1, 2));
        assert_eq!(m.member_props[&1].iz, 0.4);
        assert_eq!(m.supports[&1], [true, true, true, true, false, true]);
        assert_eq!(m.load_cases[&3], "Dead");
        assert_eq!(m.joint_loads.len(), 1);
        assert_eq!(m.joint_loads[0].force, [0.0, 0.0, -1000.0]);
        assert_eq!(m.joint_loads[0].lc, 3);
    }
}
