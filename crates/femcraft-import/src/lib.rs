//! File > Import: models from other analysis programs, built in FemCraft through the same
//! RFEM API I style methods the MCP server offers (`set_node`, `set_surface`, ...), so the
//! importer needs no knowledge of the `.rf6` tables.
//!
//! Plate meshes become RFEM surfaces again ([`surfaces`]): coplanar, edge-connected plates of
//! one thickness form a surface, its outline the outer boundary, inner boundaries openings.
//! Nodal supports on every node of such a surface become a surface support, plate pressures
//! and nodal forces of equal pressure per area become uniform surface loads, the rest stays
//! nodal (`merge_onto_surfaces`).

pub mod a03;
pub mod robot;
pub mod rtd;
pub mod staad;
pub mod surfaces;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Value as Json, json};

/// The formats File > Import offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    ProkonFrame,
    Staad,
    Robot,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::ProkonFrame, Format::Staad, Format::Robot];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Format::ProkonFrame => "Prokon Frame (.A03)",
            Format::Staad => "Staad.Pro (.std)",
            Format::Robot => "Robot (.str, .rtd)",
        }
    }

    /// File dialog filter: name and extensions.
    pub fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Format::ProkonFrame => ("Prokon Frame model", &["A03", "a03"]),
            Format::Staad => ("STAAD.Pro input file", &["std", "STD"]),
            Format::Robot => ("Robot model (.str text or .rtd binary)", &["str", "STR", "rtd", "RTD"]),
        }
    }

    pub fn supported(self) -> bool {
        true
    }
}

/// Calls one API method (`set_node`, `get_all_object_numbers`, ...) on the target model.
pub type Call<'a> = dyn FnMut(&str, &Json) -> Result<Json, String> + 'a;

/// What an import made, and what it could not.
#[derive(Debug, Default)]
pub struct Report {
    /// Objects created per kind (`node`, `surface`, ...).
    pub created: BTreeMap<&'static str, usize>,
    /// Things not (fully) converted.
    pub notes: Vec<String>,
    /// Calls that failed.
    pub errors: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let made: Vec<String> = self
            .created
            .iter()
            .map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" }))
            .collect();
        let mut s = format!("Imported {}.", made.join(", "));
        if !self.errors.is_empty() {
            s.push_str(&format!(
                " {} errors (first: {}).",
                self.errors.len(),
                self.errors[0]
            ));
        }
        s
    }

    fn set(&mut self, call: &mut Call, kind: &'static str, value: Json) -> bool {
        self.set_lc(call, kind, None, value)
    }

    fn set_lc(
        &mut self,
        call: &mut Call,
        kind: &'static str,
        lc: Option<u32>,
        value: Json,
    ) -> bool {
        let mut args = json!({ "value": value });
        if let Some(lc) = lc {
            args["load_case_no"] = json!(lc);
        }
        match call(&format!("set_{kind}"), &args) {
            Ok(r) => {
                if r["created"] == json!(true) {
                    *self.created.entry(kind).or_default() += 1;
                }
                true
            }
            Err(e) => {
                let no = args["value"]["no"].as_u64().unwrap_or(0);
                self.errors.push(format!("{kind} {no}: {e}"));
                false
            }
        }
    }
}

/// Object numbers of a kind in the target model.
fn numbers(call: &mut Call, ty: &str) -> Vec<u32> {
    call("get_all_object_numbers", &json!({ "type": ty }))
        .ok()
        .and_then(|v| {
            let arr = v.as_array().cloned().or_else(|| {
                v.as_object()
                    .and_then(|o| o.values().find_map(|x| x.as_array().cloned()))
            })?;
            Some(
                arr.iter()
                    .filter_map(|n| n.as_u64().map(|n| n as u32))
                    .collect(),
            )
        })
        .unwrap_or_default()
}

/// RFEM's axes from the file's: RFEM's Z points down.
fn to_rfem(p: [f64; 3], z_up: bool) -> [f64; 3] {
    if z_up {
        [p[0], -p[1], -p[2]]
    } else {
        // Y up: rotate Y onto Z, then flip to Z down.
        [p[0], p[2], -p[1]]
    }
}

/// Imports a STAAD.Pro `.std` file into the (new, empty) model behind `call`.
pub fn import_staad(text: &str, call: &mut Call) -> Result<Report, String> {
    import_model(staad::parse(text)?, call, &mut |_, _| {})
}

/// Imports a Prokon Frame `.A03` report into the (new, empty) model behind `call`.
pub fn import_a03(text: &str, call: &mut Call) -> Result<Report, String> {
    import_model(a03::parse(text)?, call, &mut |_, _| {})
}

/// Text of a file: UTF-16 (byte order mark; Robot writes its own `.str` that way) or UTF-8.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let u: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&u)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Parses a file of `format` (text formats are read as UTF-8, lossily).
fn read_model(format: Format, bytes: &[u8]) -> Result<staad::StaadModel, String> {
    let text = || decode_text(bytes);
    match format {
        Format::Staad => staad::parse(&text()),
        Format::ProkonFrame => a03::parse(&text()),
        Format::Robot if bytes.starts_with(&rtd::MAGIC) => rtd::parse(bytes),
        Format::Robot => robot::parse(&text()),
    }
}

/// Checks that `bytes` is a file of `format` (before a model is opened for it).
pub fn check(format: Format, bytes: &[u8]) -> Result<(), String> {
    read_model(format, bytes).map(|_| ())
}

/// Imports a file of `format` into the (new, empty) model behind `call`.
pub fn import(format: Format, bytes: &[u8], call: &mut Call) -> Result<Report, String> {
    import_with_progress(format, bytes, call, &mut |_, _| {})
}

/// Reports how far an import is: a fraction 0..1 and what it is doing.
pub type Progress<'a> = dyn FnMut(f32, &str) + 'a;

/// Like [`import`], telling `progress` where it is (for a progress dialog).
pub fn import_with_progress(
    format: Format,
    bytes: &[u8],
    call: &mut Call,
    progress: &mut Progress,
) -> Result<Report, String> {
    progress(0.0, "Reading the file");
    let m = read_model(format, bytes)?;
    import_model(m, call, progress)
}

fn import_model(
    m: staad::StaadModel,
    call: &mut Call,
    progress: &mut Progress,
) -> Result<Report, String> {
    progress(0.03, "Materials");
    let mut r = Report::default();
    let pos: BTreeMap<u32, [f64; 3]> = m
        .joints
        .iter()
        .map(|(n, p)| (*n, to_rfem(*p, m.z_up)))
        .collect();
    let tol = 1e-3;

    // Materials: the grade (C30/37, S355, ...) found in the template or the library.
    let mut mat_no: HashMap<String, u32> = HashMap::new();
    let existing: Vec<(u32, String)> = numbers(call, "E_OBJECT_TYPE_MATERIAL")
        .into_iter()
        .map(|no| {
            let name = call("get_material", &json!({ "no": no }))
                .ok()
                .and_then(|v| v["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
            (no, name)
        })
        .collect();
    let mut next_mat = existing.iter().map(|e| e.0).max().unwrap_or(0) + 1;
    let mut grades: BTreeMap<String, u32> = BTreeMap::new();
    // Library assignments, done together at the end: each one alone reloads the model.
    let mut library: Vec<Json> = Vec::new();
    for mat in &m.materials {
        let grade = grade_of(&mat.name);
        let no = match &grade {
            Some(g) if grades.contains_key(g) => grades[g],
            Some(g) => {
                let found = existing
                    .iter()
                    .find(|(_, n)| n.split(['|', ' ']).any(|w| w == g))
                    .map(|e| e.0);
                let no = found.or_else(|| {
                    // A grade is in several standards: take the library's first item.
                    let full = call(
                        "get_library_items",
                        &json!({ "kind": "material", "search": g, "limit": 0 }),
                    )
                    .ok()
                    .and_then(|v| {
                        v["items"].as_array()?.iter().find_map(|i| {
                            let n = i["name"].as_str()?;
                            n.starts_with(&format!("{g} |")).then(|| n.to_owned())
                        })
                    })
                    .unwrap_or_else(|| g.clone());
                    let ok = call(
                        "get_library_item",
                        &json!({ "kind": "material", "name": full }),
                    )
                    .is_ok()
                        && call("set_material", &json!({ "value": { "no": next_mat } })).is_ok();
                    ok.then(|| {
                        library.push(json!({ "type": "material", "no": next_mat, "name": full }));
                        *r.created.entry("material").or_default() += 1;
                        next_mat += 1;
                        next_mat - 1
                    })
                });
                match no {
                    Some(no) => {
                        grades.insert(g.clone(), no);
                        no
                    }
                    None => {
                        r.notes.push(format!(
                            "Material {}: grade {g} not found in the library, material 1 used",
                            mat.name
                        ));
                        1
                    }
                }
            }
            None => {
                r.notes.push(format!(
                    "Material {} (E = {:.3e} Pa): no grade recognised, material 1 used",
                    mat.name, mat.e
                ));
                1
            }
        };
        mat_no.insert(mat.name.clone(), no);
    }
    // Modified stiffness (cracked sections etc.) is not carried over.
    for mat in &m.materials {
        if let Some(base) = m.materials.iter().find(|b| {
            b.name != mat.name
                && grade_of(&b.name).is_some()
                && grade_of(&b.name) == grade_of(&mat.name)
                && b.e > mat.e * 1.01
        }) {
            r.notes.push(format!(
                "Material {}: E = {:.3e} Pa is {:.0} % of {} (a stiffness modifier); \
                 the library value is used",
                mat.name,
                mat.e,
                100.0 * mat.e / base.e,
                base.name
            ));
        }
    }
    let material_of = |el: u32| -> u32 {
        m.element_material
            .get(&el)
            .and_then(|n| mat_no.get(n))
            .copied()
            .unwrap_or(1)
    };

    progress(0.08, "Thicknesses");
    // Thicknesses: one per (thickness, material).
    let mut thick_no: BTreeMap<(i64, u32), u32> = BTreeMap::new();
    for (&pl, &t) in &m.plate_thickness {
        if m.plates.contains_key(&pl) {
            let key = ((t * 1e6).round() as i64, material_of(pl));
            let n = thick_no.len() as u32 + 1;
            thick_no.entry(key).or_insert(n);
        }
    }
    for (&(t, mat), &no) in &thick_no {
        r.set(
            call,
            "thickness",
            json!({ "no": no, "uniform_thickness": t as f64 / 1e6, "material": mat }),
        );
    }
    let missing_thickness = m
        .plates
        .keys()
        .filter(|p| !m.plate_thickness.contains_key(p))
        .count();
    if missing_thickness > 0 {
        r.notes.push(format!(
            "{missing_thickness} plates have no thickness: thickness 1 used"
        ));
    }

    progress(0.10, "Sections");
    // Sections: prismatic properties -> parametric concrete rectangles R_M1 b/h.
    let mut sec_no: BTreeMap<(String, u32), u32> = BTreeMap::new();
    let mut member_sec: BTreeMap<u32, u32> = BTreeMap::new();
    let mut odd = Vec::new();
    for (&mem, p) in &m.member_props {
        if !m.members.contains_key(&mem) {
            continue;
        }
        let (name, fitted) = rectangle_name(p);
        if !fitted {
            odd.push(mem);
        }
        let key = (name, material_of(mem));
        let n = sec_no.len() as u32 + 1;
        member_sec.insert(mem, *sec_no.entry(key).or_insert(n));
    }
    for ((name, mat), &no) in &sec_no {
        if r.set(call, "section", json!({ "no": no, "material": mat })) {
            library.push(json!({ "type": "section", "no": no, "name": name }));
        } else {
            r.notes
                .push(format!("Section {no} ({name}) could not be created"));
        }
    }
    if !odd.is_empty() {
        r.notes.push(format!(
            "{} members have properties that fit no rectangle (e.g. member {}): a square of \
             the same area was used",
            odd.len(),
            odd[0]
        ));
    }

    progress(0.30, "Rebuilding surfaces from the plates");
    // Surfaces from the plate mesh; key = thickness number.
    let plates: Vec<surfaces::Plate> = m
        .plates
        .iter()
        .map(|(&id, nodes)| {
            let key = m
                .plate_thickness
                .get(&id)
                .map(|&t| thick_no[&((t * 1e6).round() as i64, material_of(id))])
                .unwrap_or(1);
            surfaces::Plate {
                id,
                nodes: nodes.clone(),
                key,
            }
        })
        .collect();
    let mut plates = plates;
    let mut regions = surfaces::rebuild(&pos, &plates, tol);
    let zoned = split_by_load_zone(&m, &pos, &mut plates, &regions, tol);
    if zoned > 0 {
        regions = surfaces::rebuild(&pos, &plates, tol);
        r.notes.push(format!(
            "{zoned} surfaces split by load zone (plates with different pressures), so every              zone gets uniform surface loads: {} surfaces in all",
            regions.len()
        ));
    }
    let merged = merge_onto_surfaces(&m, &pos, &regions);
    let supports = &merged.supports;

    // Nodes to keep on outlines: corners, member ends, supports.
    let mut keep: BTreeSet<u32> = BTreeSet::new();
    for (a, b) in m.members.values() {
        keep.insert(*a);
        keep.insert(*b);
    }
    keep.extend(supports.keys());
    for reg in &regions {
        keep.extend(surfaces::corners(&reg.outer, &pos, tol));
        for h in &reg.holes {
            keep.extend(surfaces::corners(h, &pos, tol));
        }
    }
    let simplified: Vec<(Vec<u32>, Vec<Vec<u32>>)> = regions
        .iter()
        .map(|reg| {
            (
                surfaces::simplify(&reg.outer, &keep),
                reg.holes
                    .iter()
                    .map(|h| surfaces::simplify(h, &keep))
                    .collect(),
            )
        })
        .collect();

    // Nodes used by anything.
    let mut used: BTreeSet<u32> = keep
        .iter()
        .copied()
        .filter(|n| m.joints.contains_key(n))
        .collect();
    used.retain(|n| {
        m.members.values().any(|(a, b)| a == n || b == n)
            || supports.contains_key(n)
            || simplified
                .iter()
                .any(|(o, hs)| o.contains(n) || hs.iter().any(|h| h.contains(n)))
    });
    used.extend(merged.load_nodes.iter().copied());
    let node_count = used.len().max(1);
    for (i, &n) in used.iter().enumerate() {
        if i % 50 == 0 {
            progress(0.40 + 0.15 * i as f32 / node_count as f32, "Nodes");
        }
        let p = pos[&n];
        r.set(
            call,
            "node",
            json!({ "no": n, "coordinate_1": p[0], "coordinate_2": p[1], "coordinate_3": p[2] }),
        );
    }

    // Lines: one per member, shared with surface edges between the same two nodes.
    let mut lines: HashMap<(u32, u32), u32> = HashMap::new();
    let mut next_line = 1u32;
    let mut line_of = |r: &mut Report, call: &mut Call, a: u32, b: u32| -> u32 {
        let key = if a < b { (a, b) } else { (b, a) };
        if let Some(&no) = lines.get(&key) {
            return no;
        }
        let no = next_line;
        next_line += 1;
        lines.insert(key, no);
        r.set(
            call,
            "line",
            json!({ "no": no, "definition_nodes": [a, b] }),
        );
        no
    };
    progress(0.55, "Members");
    for (&mem, &(a, b)) in &m.members {
        if !(m.joints.contains_key(&a) && m.joints.contains_key(&b)) || a == b {
            r.notes.push(format!("Member {mem}: bad joints {a}, {b}"));
            continue;
        }
        let line = line_of(&mut r, call, a, b);
        let mut v = json!({ "no": mem, "line": line, "section_start": member_sec.get(&mem).copied().unwrap_or(1) });
        if let Some(beta) = m.beta.get(&mem) {
            v["rotation_angle"] = json!(beta.to_radians());
        }
        r.set(call, "member", v);
    }
    if !m.beta.is_empty() {
        r.notes.push(format!(
            "{} members have a STAAD beta angle, copied as RFEM's rotation β: check member \
             orientation (STAAD and RFEM define the default local axes differently)",
            m.beta.len()
        ));
    }

    let mut opening_no = 1u32;
    for (i, (reg, (outer, holes))) in regions.iter().zip(&simplified).enumerate() {
        let sno = i as u32 + 1;
        let loop_lines = |r: &mut Report,
                          call: &mut Call,
                          l: &[u32],
                          lf: &mut dyn FnMut(&mut Report, &mut Call, u32, u32) -> u32|
         -> Vec<u32> {
            (0..l.len())
                .map(|k| lf(r, call, l[k], l[(k + 1) % l.len()]))
                .collect()
        };
        let boundary = loop_lines(&mut r, call, outer, &mut line_of);
        r.set(
            call,
            "surface",
            json!({ "no": sno, "boundary_lines": boundary, "thickness": reg.key & 0xFFFF,
                    "comment": format!("{} plates", reg.plates.len()) }),
        );
        for h in holes {
            let bl = loop_lines(&mut r, call, h, &mut line_of);
            r.set(
                call,
                "opening",
                json!({ "no": opening_no, "boundary_lines": bl, "surfaces": [sno] }),
            );
            opening_no += 1;
        }
    }

    progress(0.70, "Surfaces and openings");
    progress(0.78, "Supports");
    // Nodal supports, grouped by restraint pattern (in RFEM's axes).
    // Per DOF: fixed (inf), a spring constant, or free (0); in RFEM's axes.
    let mut patterns: BTreeMap<[u64; 6], Vec<u32>> = BTreeMap::new();
    for (&n, d) in supports {
        let v = support_values(&m, n, d);
        // Group equal values (6 significant digits).
        let key = v.map(|x| {
            if x.is_finite() && x > 0.0 {
                let mag = 10f64.powi(x.log10().floor() as i32 - 5);
                ((x / mag).round() * mag).to_bits()
            } else {
                x.to_bits()
            }
        });
        patterns.entry(key).or_default().push(n);
    }
    if !m.inclined_supports.is_empty() {
        r.notes.push(format!(
            "{} inclined supports (INC) imported in global axes",
            m.inclined_supports.len()
        ));
    }
    const DOF_KEYS: [&str; 6] = [
        "spring_x",
        "spring_y",
        "spring_z",
        "rotational_restraint_x",
        "rotational_restraint_y",
        "rotational_restraint_z",
    ];
    let existing: Vec<(u32, Json)> = numbers(call, "E_OBJECT_TYPE_NODAL_SUPPORT")
        .into_iter()
        .filter_map(|no| Some((no, call("get_nodal_support", &json!({ "no": no })).ok()?)))
        .collect();
    let mut next_sup = existing.iter().map(|e| e.0).max().unwrap_or(0) + 1;
    for (key, nodes) in patterns {
        let vals = key.map(f64::from_bits);
        let same = |v: &Json, want: f64| {
            if want.is_infinite() {
                v.as_str() == Some("inf") || v.as_f64().is_some_and(|f| f > 1e20)
            } else if want == 0.0 {
                v.is_null() || v.as_f64() == Some(0.0)
            } else {
                v.as_f64().is_some_and(|f| (f - want).abs() <= 1e-6 * want)
            }
        };
        let found = existing.iter().find(|(_, s)| {
            DOF_KEYS.iter().zip(vals).all(|(k, w)| same(&s[k], w))
                && s["nodes"].as_array().is_none_or(|a| a.is_empty())
        });
        let mut v = json!({ "nodes": nodes });
        let no = match found {
            Some((no, _)) => *no,
            None => {
                for (k, w) in DOF_KEYS.iter().zip(vals) {
                    v[*k] = if w.is_infinite() {
                        json!("inf")
                    } else {
                        json!(w)
                    };
                }
                next_sup += 1;
                next_sup - 1
            }
        };
        v["no"] = json!(no);
        r.set(call, "nodal_support", v);
    }

    progress(0.85, "Load cases and loads");
    // Load cases (titles only for now).
    for (&no, name) in &m.load_cases {
        let name: String = name.chars().take(60).collect();
        let mut v = json!({ "no": no, "name": name });
        if let Some((_, f)) = m.self_weight.iter().find(|(lc, _)| *lc == no) {
            let f = to_rfem(*f, m.z_up);
            v["self_weight_active"] = json!(true);
            v["self_weight_factor_x"] = json!(f[0]);
            v["self_weight_factor_y"] = json!(f[1]);
            v["self_weight_factor_z"] = json!(f[2]);
        } else {
            // The template's load case 1 has self-weight on.
            v["self_weight_active"] = json!(false);
        }
        r.set(call, "load_case", v);
    }
    let used_lcs: BTreeSet<u32> = merged
        .surface_loads
        .iter()
        .map(|l| l.lc)
        .chain(merged.nodal_loads.iter().map(|l| l.lc))
        .collect();
    let used_lcs: BTreeSet<u32> = used_lcs
        .into_iter()
        .chain(m.member_loads.iter().map(|l| l.lc))
        .collect();
    for lc in used_lcs {
        if !m.load_cases.contains_key(&lc) {
            r.set(
                call,
                "load_case",
                json!({ "no": lc, "name": format!("Load case {lc}") }),
            );
        }
    }

    // Surface supports: one per distinct stiffness, listing its surfaces.
    let mut by_k: BTreeMap<[u64; 3], Vec<u32>> = BTreeMap::new();
    for s in &merged.surface_supports {
        by_k.entry(s.k.map(f64::to_bits))
            .or_default()
            .push(s.region as u32 + 1);
    }
    let mut next_ssup = numbers(call, "E_OBJECT_TYPE_SURFACE_SUPPORT")
        .into_iter()
        .max()
        .unwrap_or(0)
        + 1;
    for (k, surfs) in by_k {
        let mut v = json!({ "no": next_ssup, "surfaces": surfs, "shear_xz": 0.0,
                           "shear_yz": 0.0, "name": "STAAD supports" });
        for (key, w) in ["translation_x", "translation_y", "translation_z"]
            .iter()
            .zip(k.map(f64::from_bits))
        {
            v[*key] = if w.is_infinite() {
                json!("inf")
            } else {
                json!(w)
            };
        }
        if r.set(call, "surface_support", v) {
            next_ssup += 1;
        }
    }
    // Surface loads (uniform pressures) and the nodal loads left over.
    for l in &merged.surface_loads {
        r.set_lc(
            call,
            "surface_load",
            Some(l.lc),
            json!({ "surfaces": [l.region as u32 + 1], "uniform_magnitude": l.p,
                    "load_direction": 11 + l.axis as i64 }),
        );
    }
    for l in &merged.nodal_loads {
        r.set_lc(
            call,
            "nodal_load",
            Some(l.lc),
            json!({ "nodes": l.nodes, "magnitude": l.value,
                    "load_direction": 11 + l.axis as i64 }),
        );
    }
    r.notes.extend(merged.notes.iter().cloned());
    progress(0.95, "Member loads");
    // Uniform member loads: one per (case, direction, value), listing its members.
    let mut ml: BTreeMap<(u32, usize, i64, i64), (f64, Vec<u32>)> = BTreeMap::new();
    for l in &m.member_loads {
        if !m.members.contains_key(&l.member) {
            continue;
        }
        let (code, w) = match l.dir {
            staad::PressureDir::Global(a) => {
                let mut u = [0.0; 3];
                u[a] = 1.0;
                let u = to_rfem(u, m.z_up);
                let ax = (0..3).find(|i| u[*i] != 0.0).unwrap_or(0);
                (11 + ax as i64, l.w * u[ax])
            }
            staad::PressureDir::Local(_) => (2, l.w),
        };
        ml.entry((l.lc, code as usize, 0, (w * 1e3).round() as i64))
            .or_insert((w, Vec::new()))
            .1
            .push(l.member);
    }
    for ((lc, code, _, _), (w, members)) in ml {
        r.set_lc(
            call,
            "member_load",
            Some(lc),
            json!({ "members": members, "magnitude": w, "load_direction": code as i64 }),
        );
    }
    if m.combinations > 0 {
        r.notes.push(format!(
            "{} load combinations not imported: FemCraft has no combination objects yet",
            m.combinations
        ));
        let name = |lc: u32| {
            m.load_cases
                .get(&lc)
                .cloned()
                .unwrap_or_else(|| lc.to_string())
        };
        for (cname, items) in &m.combination_defs {
            let terms: Vec<String> = items
                .iter()
                .map(|(lc, uls, sls)| format!("{uls}/{sls} x {}", name(*lc)))
                .collect();
            r.notes.push(format!(
                "  Combination {cname} (ULS/SLS factor): {}",
                terms.join(" + ")
            ));
        }
    }
    if !library.is_empty() {
        progress(0.97, "Library materials and sections");
        // One call for all: the model is written and reloaded once.
        if let Err(e) = call("set_library_items", &json!({ "items": library })) {
            r.errors.push(format!("library materials / sections: {e}"));
        }
    }
    r.notes.extend(m.notes.iter().cloned());
    for (what, n) in m.skipped.iter().filter(|(_, n)| **n > 0) {
        r.notes.push(format!("Not imported: {what} ({n} records)"));
    }
    let (o, h): (usize, usize) = (regions.len(), simplified.iter().map(|s| s.1.len()).sum());
    r.notes.insert(
        0,
        format!(
            "{} plates rebuilt as {o} surfaces with {h} openings",
            m.plates.len()
        ),
    );
    Ok(r)
}

/// Result of folding per-node supports and loads onto the rebuilt surfaces.
struct Merged {
    /// Nodal supports left (not folded into a surface support).
    supports: BTreeMap<u32, [bool; 6]>,
    surface_supports: Vec<SurfaceSupport>,
    surface_loads: Vec<SurfaceLoad>,
    nodal_loads: Vec<NodalLoad>,
    /// Nodes of the nodal loads left.
    load_nodes: BTreeSet<u32>,
    notes: Vec<String>,
}

struct SurfaceSupport {
    region: usize,
    /// Spring per area (N/m³) along RFEM's X, Y, Z; `inf` = rigid.
    k: [f64; 3],
}

struct SurfaceLoad {
    lc: u32,
    region: usize,
    /// RFEM global axis 0..3.
    axis: usize,
    /// Pressure (Pa) along the axis; RFEM's Z points down.
    p: f64,
}

struct NodalLoad {
    lc: u32,
    axis: usize,
    value: f64,
    nodes: Vec<u32>,
}

type Corners = [[f64; 3]; 4];

/// Pressures per plate (per corner, vector in RFEM axes), summed per load case.
struct PlatePressures {
    /// Load case -> plate -> corners.
    map: BTreeMap<u32, BTreeMap<u32, Corners>>,
    skipped_local: usize,
    local_z: bool,
}

fn plate_pressures(m: &staad::StaadModel, pos: &BTreeMap<u32, [f64; 3]>) -> PlatePressures {
    let mut out = PlatePressures {
        map: BTreeMap::new(),
        skipped_local: 0,
        local_z: false,
    };
    for el in &m.element_loads {
        for &pl in &el.plates {
            let Some(nodes) = m
                .plates
                .get(&pl)
                .filter(|n| n.len() >= 3 && n.iter().all(|j| pos.contains_key(j)))
            else {
                continue;
            };
            let p: Vec<[f64; 3]> = nodes.iter().map(|n| pos[n]).collect();
            let axis_vec = match el.dir {
                staad::PressureDir::Global(a) => {
                    let mut u = [0.0; 3];
                    u[a] = 1.0;
                    to_rfem(u, m.z_up)
                }
                staad::PressureDir::Local(2) => {
                    out.local_z = true;
                    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                    let len = norm(n).max(1e-300);
                    [n[0] / len, n[1] / len, n[2] / len]
                }
                staad::PressureDir::Local(_) => {
                    out.skipped_local += 1;
                    continue;
                }
            };
            let vals = match el.value {
                staad::Pressure::Uniform(v) => [v; 4],
                staad::Pressure::Corners(c) => c,
            };
            let e = out.map.entry(el.lc).or_default().entry(pl).or_default();
            for (c, v) in e.iter_mut().zip(vals) {
                for i in 0..3 {
                    c[i] += v * axis_vec[i];
                }
            }
        }
    }
    out
}

/// Splits surfaces whose plates carry a few different pressure patterns (wind zones, a loaded
/// and an unloaded part) into one surface per pattern, so each gets uniform surface loads
/// instead of nodal ones. Only when a surface has 2..=128 patterns and falls into at most 20
/// pieces. Returns the plates with a zone in the upper 16 bits of their key.
fn split_by_load_zone(
    m: &staad::StaadModel,
    pos: &BTreeMap<u32, [f64; 3]>,
    plates: &mut [surfaces::Plate],
    regions: &[surfaces::Region],
    tol: f64,
) -> usize {
    type Sig = Vec<(u32, [[i64; 3]; 4])>;
    let pp = plate_pressures(m, pos);
    let sig_of = |pl: u32| -> Sig {
        pp.map
            .iter()
            .filter_map(|(lc, plates)| {
                plates
                    .get(&pl)
                    .map(|c| (*lc, c.map(|v| v.map(|x| (x * 1e3).round() as i64))))
            })
            .collect()
    };
    let index: HashMap<u32, usize> = plates.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let mut split = 0;
    for reg in regions {
        let mut sigs: BTreeMap<Sig, u32> = BTreeMap::new();
        for pl in &reg.plates {
            let n = sigs.len() as u32;
            sigs.entry(sig_of(*pl)).or_insert(n + 1);
        }
        if !(2..=128).contains(&sigs.len()) {
            continue;
        }
        let trial: Vec<surfaces::Plate> = reg
            .plates
            .iter()
            .map(|pl| {
                let src = &plates[index[pl]];
                surfaces::Plate {
                    id: src.id,
                    nodes: src.nodes.clone(),
                    key: src.key | (sigs[&sig_of(*pl)] << 16),
                }
            })
            .collect();
        if surfaces::rebuild(pos, &trial, tol).len() <= 20 {
            for t in trial {
                plates[index[&t.id]].key = t.key;
            }
            split += 1;
        }
    }
    split
}

/// Restraint of a joint per DOF in RFEM's axes: `inf` fixed, a spring constant, or 0 free.
fn support_values(m: &staad::StaadModel, n: u32, d: &[bool; 6]) -> [f64; 6] {
    let k = m.support_springs.get(&n).copied().unwrap_or([0.0; 6]);
    let v: Vec<f64> = (0..6)
        .map(|i| if d[i] { f64::INFINITY } else { k[i].abs() })
        .collect();
    if m.z_up {
        [v[0], v[1], v[2], v[3], v[4], v[5]]
    } else {
        [v[0], v[2], v[1], v[3], v[5], v[4]]
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Tributary area of a node over all surfaces.
fn area_all(trib: &[HashMap<u32, f64>], n: &u32) -> f64 {
    trib.iter().filter_map(|t| t.get(n)).sum()
}

/// Folds nodal supports and loads that sit on the nodes of a rebuilt surface into surface
/// supports and surface loads:
/// * a surface support when every node of the surface has a support and the springs per
///   tributary area agree (5 %);
/// * element pressures that are the same on all plates of a surface become one uniform
///   surface load; other element pressures are lumped to nodal loads;
/// * nodal forces whose pressures (force / tributary area) are equal over all nodes of a
///   surface (0.5 %) become a uniform surface load.
///
/// A linearly varying (triangular) load would need a linear surface load, which FemCraft
/// cannot write yet: those stay nodal loads.
fn merge_onto_surfaces(
    m: &staad::StaadModel,
    pos: &BTreeMap<u32, [f64; 3]>,
    regions: &[surfaces::Region],
) -> Merged {
    let rfem = |v: [f64; 3]| -> [f64; 3] {
        if m.z_up {
            [v[0], -v[1], -v[2]]
        } else {
            [v[0], v[2], -v[1]]
        }
    };
    // Nodes of each region, tributary area of each node.
    let mut region_nodes: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); regions.len()];
    let mut trib: Vec<HashMap<u32, f64>> = vec![HashMap::new(); regions.len()];
    let mut plate_area: HashMap<u32, f64> = HashMap::new();
    for (ri, reg) in regions.iter().enumerate() {
        for pl in &reg.plates {
            let Some(nodes) = m.plates.get(pl) else {
                continue;
            };
            let pts: Vec<[f64; 3]> = nodes.iter().filter_map(|n| pos.get(n).copied()).collect();
            if pts.len() != nodes.len() || pts.len() < 3 {
                continue;
            }
            let mut a = norm(cross(sub(pts[1], pts[0]), sub(pts[2], pts[0]))) / 2.0;
            if pts.len() == 4 {
                a += norm(cross(sub(pts[2], pts[0]), sub(pts[3], pts[0]))) / 2.0;
            }
            plate_area.insert(*pl, a);
            for n in nodes {
                region_nodes[ri].insert(*n);
                *trib[ri].entry(*n).or_default() += a / nodes.len() as f64;
            }
        }
    }
    let mut out = Merged {
        supports: m.supports.clone(),
        surface_supports: Vec::new(),
        surface_loads: Vec::new(),
        nodal_loads: Vec::new(),
        load_nodes: BTreeSet::new(),
        notes: Vec::new(),
    };

    // --- Supports ---
    // A surface whose every node has a support gets a surface support when the springs agree
    // within 5 % on all its elastic nodes: either the same spring per node (the surface spring
    // is their sum / the surface area) or the same spring per tributary area (spring / area of
    // the plates around the node). Rigid nodes (a fixed translation) among them keep their nodal
    // support. Surfaces with bands of different stiffness are left as nodal supports.
    let mut folded = 0usize;
    let mut banded = 0usize;
    let mut rigid_kept = 0usize;
    let mut rotations = false;
    for (ri, nodes) in region_nodes.iter().enumerate() {
        if nodes.is_empty() || !nodes.iter().all(|n| m.supports.contains_key(n)) {
            continue;
        }
        // (node, spring per tributary area, spring per node), both in RFEM's X, Y, Z.
        let kv: Vec<(u32, [f64; 3], [f64; 3])> = nodes
            .iter()
            .map(|n| {
                let v = support_values(m, *n, &m.supports[n]);
                (
                    *n,
                    std::array::from_fn(|ax| {
                        if v[ax].is_finite() && v[ax] > 0.0 {
                            v[ax] / trib[ri][n]
                        } else {
                            v[ax]
                        }
                    }),
                    [v[0], v[1], v[2]],
                )
            })
            .collect();
        let (rigid, elastic): (Vec<_>, Vec<_>) = kv
            .iter()
            .partition(|(_, k, _)| k.iter().any(|v| v.is_infinite()));
        let group = if elastic.is_empty() { &rigid } else { &elastic };
        let mut k = [0.0; 3];
        let mut ok = true;
        for (ax, kax) in k.iter_mut().enumerate() {
            let vals: Vec<f64> = group.iter().map(|(_, v, _)| v[ax]).collect();
            let per_node: Vec<f64> = group.iter().map(|(_, _, v)| v[ax]).collect();
            if vals.iter().all(|v| v.is_infinite() || *v == 0.0) {
                // All rigid or all free; a mix is not one stiffness.
                let inf = vals.iter().filter(|v| v.is_infinite()).count();
                if inf == 0 || inf == vals.len() {
                    *kax = if inf > 0 { f64::INFINITY } else { 0.0 };
                } else {
                    ok = false;
                }
            } else if vals.iter().any(|v| *v == 0.0 || v.is_infinite()) {
                ok = false;
            } else {
                let spread = |v: &[f64]| {
                    let mean = v.iter().sum::<f64>() / v.len() as f64;
                    let (lo, hi) = v
                        .iter()
                        .fold((f64::MAX, 0.0f64), |(l, h), v| (l.min(*v), h.max(*v)));
                    ((hi - lo) / mean, mean)
                };
                let (by_area, mean) = spread(&vals);
                let (by_node, _) = spread(&per_node);
                if by_area <= 0.05 {
                    *kax = mean;
                } else if by_node <= 0.05 {
                    let area: f64 = group.iter().map(|(n, _, _)| trib[ri][n]).sum();
                    *kax = per_node.iter().sum::<f64>() / area;
                } else {
                    ok = false;
                }
            }
        }
        if !ok {
            banded += 1;
            continue;
        }
        if k.iter().all(|v| *v == 0.0) {
            continue;
        }
        rotations |= group
            .iter()
            .any(|(n, _, _)| m.supports[n][3..].iter().any(|d| *d));
        out.surface_supports.push(SurfaceSupport { region: ri, k });
        for (n, _, _) in group {
            out.supports.remove(n);
        }
        if !elastic.is_empty() {
            rigid_kept += rigid.len();
        }
        folded += 1;
    }
    if folded > 0 {
        out.notes.push(format!(
            "{folded} surfaces had equal supports on all their nodes: combined into surface supports (spring per node / tributary area){}{}",
            if rigid_kept > 0 {
                format!("; {rigid_kept} rigid nodes kept their nodal support")
            } else {
                String::new()
            },
            if rotations {
                "; rotational restraints are not part of a surface support"
            } else {
                ""
            }
        ));
    }
    if banded > 0 {
        out.notes.push(format!(
            "{banded} surfaces have supports on every node but bands of different stiffness: kept as nodal supports"
        ));
    }

    // --- Loads ---
    let mut force: BTreeMap<(u32, u32), [f64; 3]> = BTreeMap::new();
    let mut moments = 0usize;
    for jl in &m.joint_loads {
        let f = rfem(jl.force);
        for &j in &jl.joints {
            if !pos.contains_key(&j) {
                continue;
            }
            let e = force.entry((jl.lc, j)).or_default();
            for i in 0..3 {
                e[i] += f[i];
            }
            moments += usize::from(jl.moment != [0.0; 3]);
        }
    }
    if moments > 0 {
        out.notes
            .push(format!("{moments} nodal moments not imported"));
    }
    let PlatePressures {
        map: plate_p,
        skipped_local,
        local_z,
    } = plate_pressures(m, pos);
    if skipped_local > 0 {
        out.notes.push(format!(
            "{skipped_local} plate pressures along local x / y not imported"
        ));
    }
    let same = |a: &[f64; 3], b: &[f64; 3]| {
        (0..3).all(|i| (a[i] - b[i]).abs() <= 1e-9 * (1.0 + a[i].abs()))
    };
    let (mut uniform_regions, mut lumped_plates) = (0usize, 0usize);
    for (&lc, plates) in &plate_p {
        for (ri, reg) in regions.iter().enumerate() {
            let loaded: Vec<(u32, &Corners)> = reg
                .plates
                .iter()
                .filter_map(|p| plates.get(p).map(|c| (*p, c)))
                .collect();
            if loaded.is_empty() {
                continue;
            }
            let first = loaded[0].1[0];
            let uniform = loaded.len() == reg.plates.len()
                && loaded.iter().all(|(p, c)| {
                    let nc = m.plates[p].len();
                    c[..nc].iter().all(|v| same(v, &first))
                });
            if uniform {
                for (ax, p) in first.iter().enumerate() {
                    if p.abs() > 1e-12 {
                        out.surface_loads.push(SurfaceLoad {
                            lc,
                            region: ri,
                            axis: ax,
                            p: *p,
                        });
                    }
                }
                uniform_regions += 1;
            } else {
                // Lump to the nodes: pressure x plate area / corners.
                for (p, c) in loaded {
                    let nodes = &m.plates[&p];
                    let w = plate_area[&p] / nodes.len() as f64;
                    for (n, v) in nodes.iter().zip(c.iter()) {
                        let e = force.entry((lc, *n)).or_default();
                        for i in 0..3 {
                            e[i] += v[i] * w;
                        }
                    }
                    lumped_plates += 1;
                }
            }
        }
    }
    if uniform_regions > 0 {
        out.notes.push(format!(
            "{uniform_regions} uniform plate pressures (per surface and load case) imported as \
             surface loads"
        ));
    }
    if lumped_plates > 0 {
        out.notes.push(format!(
            "{lumped_plates} plate pressures (varying, or on part of a surface) lumped to nodal \
             loads: a linear (triangular) surface load cannot be written yet"
        ));
    }
    if local_z {
        out.notes.push(
            "Pressures along a plate's local z (no direction given) follow the node order of \
             its plate (right-hand rule): check their sign"
                .into(),
        );
    }
    // Nodal forces -> uniform surface loads where the pressure is the same on every node.
    let lcs: BTreeSet<u32> = force.keys().map(|k| k.0).collect();
    let mut from_nodal = 0usize;
    for &lc in &lcs {
        for (ri, nodes) in region_nodes.iter().enumerate() {
            for ax in 0..3 {
                let fs: Vec<f64> = nodes
                    .iter()
                    .map(|n| force.get(&(lc, *n)).map_or(0.0, |v| v[ax]))
                    .collect();
                if fs.is_empty() || fs.iter().any(|f| f.abs() < 1e-9) {
                    continue;
                }
                let mut ps: Vec<f64> = nodes
                    .iter()
                    .zip(&fs)
                    .map(|(n, f)| f / trib[ri][n])
                    .collect();
                ps.sort_by(f64::total_cmp);
                let p = ps[ps.len() / 2];
                let fits = nodes.iter().zip(&fs).all(|(n, f)| {
                    let own = p * trib[ri][n];
                    let all = p * area_all(&trib, n);
                    (f - own).abs() <= 0.005 * own.abs() || (f - all).abs() <= 0.005 * all.abs()
                });
                if !fits {
                    continue;
                }
                out.surface_loads.push(SurfaceLoad {
                    lc,
                    region: ri,
                    axis: ax,
                    p,
                });
                for n in nodes {
                    if let Some(v) = force.get_mut(&(lc, *n)) {
                        v[ax] -= p * trib[ri][n];
                    }
                }
                from_nodal += 1;
            }
        }
    }
    if from_nodal > 0 {
        out.notes.push(format!(
            "{from_nodal} sets of equal nodal pressures (force / tributary area on every node of \
             a surface) combined into uniform surface loads"
        ));
    }
    // What is left stays nodal: one load per (case, axis, value).
    let mut groups: BTreeMap<(u32, usize, i64), (f64, Vec<u32>)> = BTreeMap::new();
    for (&(lc, n), v) in &force {
        for (ax, f) in v.iter().enumerate() {
            if f.abs() > 1e-6 {
                let key = (lc, ax, (f * 1e3).round() as i64);
                groups.entry(key).or_insert((*f, Vec::new())).1.push(n);
                out.load_nodes.insert(n);
            }
        }
    }
    for ((lc, axis, _), (value, nodes)) in groups {
        out.nodal_loads.push(NodalLoad {
            lc,
            axis,
            value,
            nodes,
        });
    }
    out
}

/// `C30/37`, `S355`, ... in a material name.
fn grade_of(name: &str) -> Option<String> {
    let up = name.to_ascii_uppercase();
    let b = up.as_bytes();
    for i in 0..b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if !boundary {
            continue;
        }
        let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        if b[i] == b'C' {
            let d1 = digits(i + 1);
            if d1 > 0 && b.get(i + 1 + d1) == Some(&b'/') {
                let d2 = digits(i + 2 + d1);
                if d2 > 0 {
                    return Some(up[i..i + 2 + d1 + d2].to_owned());
                }
            }
        }
        if b[i] == b'S' && digits(i + 1) == 3 {
            return Some(up[i..i + 4].to_owned());
        }
    }
    None
}

/// RFEM name of the rectangle matching STAAD prismatic properties: `R_M1 b/h` in mm, b along
/// local y and h along local z (STAAD's ZD and YD). Falls back to a square of the same area
/// (`false`) when the inertias fit no plausible rectangle.
fn rectangle_name(p: &staad::Prismatic) -> (String, bool) {
    let mm = |v: f64| ((v * 1000.0) / 5.0).round() * 5.0;
    let (b, h, ok) = if p.yd > 0.0 && p.zd > 0.0 {
        (p.zd, p.yd, true)
    } else if p.ax > 0.0 && p.iy > 0.0 && p.iz > 0.0 && (0.1..=10.0).contains(&(p.iz / p.iy).sqrt())
    {
        // Iz / Iy = (YD / ZD)^2 and A = YD * ZD.
        let ratio = (p.iz / p.iy).sqrt();
        let yd = (p.ax * ratio).sqrt();
        (p.ax / yd, yd, true)
    } else {
        let s = p.ax.max(1e-4).sqrt();
        (s, s, false)
    };
    let (b, h) = (mm(b).max(5.0), mm(h).max(5.0));
    let name = if b == h {
        format!("SQ_M1 {b}")
    } else {
        format!("R_M1 {b}/{h}")
    };
    (name, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_and_rectangles() {
        assert_eq!(grade_of("Concrete-C30/37-1").as_deref(), Some("C30/37"));
        assert_eq!(grade_of("Steel S355JR").as_deref(), Some("S355"));
        assert_eq!(grade_of("STEEL"), None);
        let p = staad::Prismatic {
            ax: 0.32,
            iy: 0.00341333,
            iz: 0.00085333,
            ..Default::default()
        };
        assert_eq!(rectangle_name(&p), ("R_M1 800/400".into(), true));
        let sq = staad::Prismatic {
            ax: 0.2025,
            iy: 6.8e-4,
            iz: 6.8e-4,
            ..Default::default()
        };
        assert_eq!(rectangle_name(&sq).0, "SQ_M1 450");
    }
}
