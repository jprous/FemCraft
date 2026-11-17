//! Builds two test models from scratch on an emptied template:
//!
//! ```text
//! rf6-build <out-dir> <template.rf6> [donor.rf6 ...]
//! ```
//!
//! - `template.rf6`: a small RFEM model; all its objects are removed except materials and
//!   sections (library data), its settings are kept.
//! - Prototypes for each object type are taken from the first file (template, then donors in
//!   order) that has a matching object.
//!
//! Writes `FemCraft portal frame.rf6` and `FemCraft pit.rf6` into `out-dir`.
//! RFEM axes: global Z points down, so "up" is negative Z.

use std::path::Path;

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;
use femcraft_rfdb::build::{Builder, Proto, find_proto, placeholder_previews, update_general_data};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

struct Protos {
    node: Proto,
    line: Proto,
    member: Proto,
    nodal_support: Proto,
    thickness: Proto,
    surface: Proto,
    surface_support: Proto,
    load_case: Proto,
    nodal_load: Proto,
    member_load: Proto,
    surface_load: Proto,
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `rf6-build --blank <out.rf6> <template.rf6>`: the blank model behind File > New.
    if let [flag, out, template] = &args[..]
        && flag == "--blank"
    {
        femcraft_rfdb::build::blank_model(Path::new(template), Path::new(out))?;
        println!("wrote {out}");
        return Ok(());
    }
    let [out, template, donors @ ..] = &args[..] else {
        return Err("usage: rf6-build <out-dir> <template.rf6> [donor.rf6 ...]".into());
    };
    let (template, out) = (Path::new(template), Path::new(out));
    let mut docs = vec![RfDocument::open(template)?];
    for d in donors {
        docs.push(RfDocument::open(Path::new(d))?);
    }
    let first = |kind: K, want: &[(&str, Value)]| -> Res<Proto> {
        docs.iter()
            .find_map(|d| find_proto(d, kind, want).ok())
            .ok_or_else(|| format!("no {} prototype matching {want:?}", kind.name()).into())
    };
    let force = || [("load_type", text("LOAD_TYPE_FORCE"))];
    let p = Protos {
        node: first(K::Node, &[("type", text("TYPE_STANDARD"))])?,
        line: first(K::Line, &[("type", text("TYPE_POLYLINE"))])?,
        member: first(K::Member, &[("type", text("TYPE_BEAM"))])?,
        nodal_support: first(K::NodalSupport, &[])?,
        thickness: first(K::Thickness, &[("type", text("TYPE_UNIFORM"))])?,
        surface: first(K::Surface, &[("geometry", text("GEOMETRY_PLANE"))])?,
        surface_support: first(K::SurfaceSupport, &[])?,
        load_case: first(K::LoadCase, &[])?,
        nodal_load: first(K::NodalLoad, &force())?,
        member_load: first(K::MemberLoad, &force())?,
        surface_load: first(K::SurfaceLoad, &force())?,
    };
    drop(docs);

    let portal = out.join("FemCraft portal frame.rf6");
    portal_frame(template, &p, &portal)?;
    println!("wrote {}", portal.display());
    for (stage, name) in [
        (1, "FemCraft pit 1 geometry.rf6"),
        (2, "FemCraft pit 2 support.rf6"),
        (3, "FemCraft pit 3 loads.rf6"),
    ] {
        let pit = out.join(name);
        pit_model(template, &p, &pit, stage)?;
        println!("wrote {}", pit.display());
    }
    Ok(())
}

/// Sets several fields of one object.
fn set(doc: &mut RfDocument, id: ObjectId, fields: &[(&str, Value)]) -> Res<()> {
    for (k, v) in fields {
        doc.model
            .set(id, k, v.clone())
            .map_err(|e| format!("{id:?}.{k}: {e}"))?;
    }
    Ok(())
}

fn nodes(b: &mut Builder, p: &Protos, n: usize) -> Res<()> {
    for no in 1..=n {
        b.add(&p.node, no as i64, None)?;
    }
    Ok(())
}

fn set_nodes(doc: &mut RfDocument, coords: &[[f64; 3]]) -> Res<()> {
    for (i, c) in coords.iter().enumerate() {
        let id = ObjectId::new(K::Node, i as u32 + 1);
        set(
            doc,
            id,
            &[
                ("coordinate_1", Value::Float(c[0])),
                ("coordinate_2", Value::Float(c[1])),
                ("coordinate_3", Value::Float(c[2])),
                ("comment", text("")),
            ],
        )?;
    }
    Ok(())
}

fn set_lines(doc: &mut RfDocument, lines: &[[u32; 2]]) -> Res<()> {
    for (i, l) in lines.iter().enumerate() {
        let id = ObjectId::new(K::Line, i as u32 + 1);
        set(doc, id, &[("definition_nodes", Value::RefList(l.to_vec()))])?;
    }
    Ok(())
}

/// Two load cases: 1 self-weight, 2 imposed. Returns their database ids.
fn load_cases(b: &mut Builder, p: &Protos) -> Res<[i64; 2]> {
    Ok([b.add(&p.load_case, 1, None)?, b.add(&p.load_case, 2, None)?])
}

fn set_load_cases(doc: &mut RfDocument) -> Res<()> {
    set(
        doc,
        ObjectId::new(K::LoadCase, 1),
        &[
            ("name", text("Self-weight")),
            ("self_weight_active", Value::Bool(true)),
            ("self_weight_factor_z", Value::Float(1.0)),
        ],
    )?;
    set(
        doc,
        ObjectId::new(K::LoadCase, 2),
        &[
            ("name", text("Imposed")),
            ("self_weight_active", Value::Bool(false)),
        ],
    )
}

/// Clears references left dangling by prototypes / the purge and reports them.
fn report_fk(b: &Builder) -> Res<()> {
    for (col, n) in b.clear_dangling_references()? {
        eprintln!("  cleared {n} dangling reference(s) in {col}");
    }
    Ok(())
}

fn save(mut doc: RfDocument, path: &Path) -> Res<()> {
    update_general_data(&mut doc)?;
    placeholder_previews(&mut doc)?;
    doc.save(path)?;
    if !doc.report.warnings.is_empty() {
        eprintln!("{}", doc.report.warnings.join("\n"));
    }
    Ok(())
}

/// 6 m span, 4 m high, fixed column bases; 10 kN horizontal at the top of the left column and
/// 10 kN/m on the beam (load case 2).
fn portal_frame(template: &Path, p: &Protos, path: &Path) -> Res<()> {
    let mut b = Builder::from_template(template)?;
    nodes(&mut b, p, 4)?;
    for no in 1..=3 {
        b.add(&p.line, no, None)?;
        b.add(&p.member, no, None)?;
    }
    b.add(&p.nodal_support, 1, None)?;
    let [_, lc2] = load_cases(&mut b, p)?;
    b.add(&p.nodal_load, 1, Some(lc2))?;
    b.add(&p.member_load, 1, Some(lc2))?;
    report_fk(&b)?;
    let mut doc = b.finish()?;

    set_nodes(
        &mut doc,
        &[[0., 0., 0.], [0., 0., -4.], [6., 0., -4.], [6., 0., 0.]],
    )?;
    set_lines(&mut doc, &[[1, 2], [2, 3], [3, 4]])?;
    for no in 1..=3 {
        set(
            &mut doc,
            ObjectId::new(K::Member, no),
            &[
                ("line", Value::Ref(no)),
                ("section_start", Value::Ref(2)),
                ("rotation_angle", Value::Float(0.0)),
            ],
        )?;
    }
    set(
        &mut doc,
        ObjectId::new(K::NodalSupport, 1),
        &[
            ("name", text("Fixed")),
            ("nodes", Value::RefList(vec![1, 4])),
            ("spring_x", Value::Float(f64::INFINITY)),
            ("spring_y", Value::Float(f64::INFINITY)),
            ("spring_z", Value::Float(f64::INFINITY)),
            ("rotational_restraint_x", Value::Float(f64::INFINITY)),
            ("rotational_restraint_y", Value::Float(f64::INFINITY)),
            ("rotational_restraint_z", Value::Float(f64::INFINITY)),
        ],
    )?;
    set_load_cases(&mut doc)?;
    set(
        &mut doc,
        ObjectId::load(K::NodalLoad, 2, 1),
        &[
            ("nodes", Value::RefList(vec![2])),
            ("load_direction", Value::Int(0)), // global X
            ("magnitude", Value::Float(10_000.0)),
        ],
    )?;
    set(
        &mut doc,
        ObjectId::load(K::MemberLoad, 2, 1),
        &[
            ("members", Value::RefList(vec![2])),
            ("load_direction", Value::Int(13)),   // global Z
            ("load_distribution", Value::Int(0)), // uniform
            ("magnitude", Value::Float(10_000.0)),
        ],
    )?;
    save(doc, path)
}

/// 3 x 3 x 3 m open pit: base slab and four walls, 200 mm concrete (material 1). Base on a
/// surface support; load case 2: 5 kN/m2 on the base, 10 kN/m2 inward on the walls.
/// `stage` 1 = geometry and load cases only, 2 = + surface support, 3 = + surface loads.
fn pit_model(template: &Path, p: &Protos, path: &Path, stage: u8) -> Res<()> {
    let mut b = Builder::from_template(template)?;
    nodes(&mut b, p, 8)?;
    for no in 1..=12 {
        b.add(&p.line, no, None)?;
    }
    b.add(&p.thickness, 1, None)?;
    for no in 1..=5 {
        let s = b.add(&p.surface, no, None)?;
        b.make_surface_standard(s)?;
    }
    if stage >= 2 {
        b.add(&p.surface_support, 1, None)?;
    }
    let [_, lc2] = load_cases(&mut b, p)?;
    if stage >= 3 {
        for no in 1..=5 {
            b.add(&p.surface_load, no, Some(lc2))?;
        }
    }
    report_fk(&b)?;
    let mut doc = b.finish()?;

    let (a, h) = (3.0, -3.0);
    set_nodes(
        &mut doc,
        &[
            [0., 0., 0.],
            [a, 0., 0.],
            [a, a, 0.],
            [0., a, 0.],
            [0., 0., h],
            [a, 0., h],
            [a, a, h],
            [0., a, h],
        ],
    )?;
    set_lines(
        &mut doc,
        &[
            [1, 2],
            [2, 3],
            [3, 4],
            [4, 1],
            [5, 6],
            [6, 7],
            [7, 8],
            [8, 5],
            [1, 5],
            [2, 6],
            [3, 7],
            [4, 8],
        ],
    )?;
    set(
        &mut doc,
        ObjectId::new(K::Thickness, 1),
        &[
            ("name", text("Concrete 200")),
            ("material", Value::Ref(1)),
            ("uniform_thickness", Value::Float(0.2)),
        ],
    )?;
    // Base, then walls y = 0, x = 3, y = 3, x = 0.
    let surfaces: [&[u32]; 5] = [
        &[1, 2, 3, 4],
        &[1, 10, 5, 9],
        &[2, 11, 6, 10],
        &[3, 12, 7, 11],
        &[4, 9, 8, 12],
    ];
    for (i, lines) in surfaces.iter().enumerate() {
        set(
            &mut doc,
            ObjectId::new(K::Surface, i as u32 + 1),
            &[
                ("boundary_lines", Value::RefList(lines.to_vec())),
                ("thickness", Value::Ref(1)),
            ],
        )?;
    }
    if stage >= 2 {
        set(
            &mut doc,
            ObjectId::new(K::SurfaceSupport, 1),
            &[
                ("name", text("Subgrade")),
                ("surfaces", Value::RefList(vec![1])),
                ("translation_x", Value::Float(f64::INFINITY)),
                ("translation_y", Value::Float(f64::INFINITY)),
                ("translation_z", Value::Float(50e6)), // 50 MN/m3
            ],
        )?;
    }
    set_load_cases(&mut doc)?;
    if stage < 3 {
        return save(doc, path);
    }
    // (surface, direction code, magnitude): 13 = global Z (down), 11 / 12 = global X / Y.
    let loads = [
        (1, 13, 5_000.0),
        (2, 12, 10_000.0),
        (3, 11, -10_000.0),
        (4, 12, -10_000.0),
        (5, 11, 10_000.0),
    ];
    for (i, (s, dir, mag)) in loads.iter().enumerate() {
        set(
            &mut doc,
            ObjectId::load(K::SurfaceLoad, 2, i as u32 + 1),
            &[
                ("surfaces", Value::RefList(vec![*s])),
                ("load_direction", Value::Int(*dir)),
                ("load_distribution", Value::Int(0)),
                ("uniform_magnitude", Value::Float(*mag)),
            ],
        )?;
    }
    save(doc, path)
}
