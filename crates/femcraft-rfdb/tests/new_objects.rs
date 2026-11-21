//! Objects added in FemCraft are saved: copied from an object of the model itself or from
//! prototype files. Uses local models in `testdata/private/` (never committed; skipped when
//! absent).

use std::path::{Path, PathBuf};

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;
use femcraft_rfdb::build::blank_model;

fn private(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    p.is_file().then_some(p).or_else(|| {
        eprintln!("skipped: {name} not found");
        None
    })
}

fn add(doc: &mut RfDocument, id: ObjectId, fields: &[(&str, Value)]) {
    doc.model.add(id).unwrap();
    for (k, v) in fields {
        doc.model.set(id, k, v.clone()).unwrap();
    }
}

fn node(doc: &mut RfDocument, no: u32, x: f64, y: f64, z: f64) {
    add(
        doc,
        ObjectId::new(K::Node, no),
        &[
            ("coordinate_1", Value::Float(x)),
            ("coordinate_2", Value::Float(y)),
            ("coordinate_3", Value::Float(z)),
        ],
    );
}

#[test]
fn new_model_with_frame_slab_and_loads() {
    let (Some(template), Some(portal), Some(pit)) = (
        private("Solids_check_RBeam.rf6"),
        private("FemCraft portal frame.rf6"),
        private("FemCraft pit 3 loads.rf6"),
    ) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let blank = dir.path().join("new model.rf6");
    blank_model(&template, &blank).unwrap();
    let mut doc = RfDocument::open(&blank).unwrap();
    doc.prototype_files = vec![portal, pit];

    // A cantilever column (member on line 1) and a 2 x 2 m slab (surface on lines 2-5).
    node(&mut doc, 1, 0.0, 0.0, 0.0);
    node(&mut doc, 2, 0.0, 0.0, -3.0);
    node(&mut doc, 3, 2.0, 0.0, -3.0);
    node(&mut doc, 4, 2.0, 2.0, -3.0);
    node(&mut doc, 5, 0.0, 2.0, -3.0);
    let lines: [&[u32]; 5] = [&[1, 2], &[2, 3], &[3, 4], &[4, 5], &[5, 2]];
    for (i, l) in lines.iter().enumerate() {
        add(
            &mut doc,
            ObjectId::new(K::Line, i as u32 + 1),
            &[("definition_nodes", Value::RefList(l.to_vec()))],
        );
    }
    add(
        &mut doc,
        ObjectId::new(K::Member, 1),
        &[("line", Value::Ref(1)), ("section_start", Value::Ref(2))],
    );
    add(
        &mut doc,
        ObjectId::new(K::Surface, 1),
        &[
            ("boundary_lines", Value::RefList(vec![2, 3, 4, 5])),
            ("thickness", Value::Ref(1)),
        ],
    );
    add(
        &mut doc,
        ObjectId::new(K::LoadCase, 2),
        &[("name", Value::Text("Imposed".into()))],
    );
    add(
        &mut doc,
        ObjectId::load(K::NodalLoad, 2, 1),
        &[
            ("nodes", Value::RefList(vec![3])),
            ("load_direction", Value::Int(1)), // global Y
            ("magnitude", Value::Float(5_000.0)),
        ],
    );
    add(
        &mut doc,
        ObjectId::load(K::SurfaceLoad, 2, 1),
        &[
            ("surfaces", Value::RefList(vec![1])),
            ("uniform_magnitude", Value::Float(2_000.0)),
        ],
    );
    let expected = doc.model.clone();

    let out = dir.path().join("Model 1.rf6");
    doc.save(&out).unwrap();
    assert!(!doc.model.is_modified());
    // FEMCRAFT_KEEP=<path>: keep the file, e.g. to open it in RFEM.
    if let Ok(keep) = std::env::var("FEMCRAFT_KEEP") {
        std::fs::copy(&out, keep).unwrap();
    }
    let back = RfDocument::open(&out).unwrap();
    for (kind, n) in [
        (K::Node, 5),
        (K::Line, 5),
        (K::Member, 1),
        (K::Surface, 1),
        (K::LoadCase, 2),
        (K::NodalLoad, 1),
        (K::SurfaceLoad, 1),
    ] {
        assert_eq!(back.model.count(kind), n, "{kind:?}");
    }
    // Every value typed in is read back.
    for &kind in K::ALL {
        for (id, o) in expected.iter_kind(kind) {
            let got = back.model.get(id).unwrap();
            for (k, v) in &o.values {
                assert_eq!(got.get(k), v, "{id:?}.{k}");
            }
        }
    }
    // Untyped references of new objects are empty (not left pointing at the copied object's).
    let m = back.model.get(ObjectId::new(K::Member, 1)).unwrap();
    assert!(m.get("section_end").as_ref_no().is_none_or(|s| s == 2));
}

#[test]
fn add_node_to_existing_model() {
    let Some(src) = private("FemCraft portal frame.rf6") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut doc = RfDocument::open(&src).unwrap();
    let no = doc.model.next_no(K::Node, 0);
    node(&mut doc, no, 3.0, 0.0, -6.0);
    let out = dir.path().join("added.rf6");
    doc.save(&out).unwrap();
    let back = RfDocument::open(&out).unwrap();
    assert_eq!(back.model.count(K::Node), no as usize);
    assert_eq!(back.model.node_position(no), Some([3.0, 0.0, -6.0]));
    assert_eq!(back.model.node_position(1), doc.model.node_position(1));
}

/// Four files from the RFEM-saved template, each adding one step, to find what RFEM rejects:
/// 1 portal frame nodes + lines, 2 + members and a support, 3 + a slab with a surface support,
/// 4 + loads. `FEMCRAFT_KEEP_DIR=<dir>` keeps them.
#[test]
fn staged_models_from_template() {
    let (Some(template), Some(prokon), Some(portal), Some(pit)) = (
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Prokon Model1.rf6"),
        private("FemCraft portal frame.rf6"),
        private("FemCraft pit 3 loads.rf6"),
    ) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    for stage in 1..=4 {
        let mut doc = RfDocument::open(&template).unwrap();
        doc.prototype_files = vec![prokon.clone(), portal.clone(), pit.clone()];
        // Portal frame in the XZ plane (Z down) ...
        for (no, x, z) in [(1, 0.0, 0.0), (2, 0.0, -4.0), (3, 6.0, -4.0), (4, 6.0, 0.0)] {
            node(&mut doc, no, x, 0.0, z);
        }
        // ... and a 2 x 2 m slab beside it.
        for (no, x, y) in [
            (5, 10.0, 0.0),
            (6, 12.0, 0.0),
            (7, 12.0, 2.0),
            (8, 10.0, 2.0),
        ] {
            node(&mut doc, no, x, y, 0.0);
        }
        let lines: [&[u32]; 7] = [
            &[1, 2],
            &[2, 3],
            &[3, 4],
            &[5, 6],
            &[6, 7],
            &[7, 8],
            &[8, 5],
        ];
        for (i, l) in lines.iter().enumerate() {
            add(
                &mut doc,
                ObjectId::new(K::Line, i as u32 + 1),
                &[("definition_nodes", Value::RefList(l.to_vec()))],
            );
        }
        if stage >= 2 {
            for no in 1..=3 {
                add(
                    &mut doc,
                    ObjectId::new(K::Member, no),
                    &[("line", Value::Ref(no)), ("section_start", Value::Ref(1))],
                );
            }
            let fixed = ObjectId::new(K::NodalSupport, 2);
            doc.model
                .set(fixed, "nodes", Value::RefList(vec![1, 4]))
                .unwrap();
        }
        if stage >= 3 {
            add(
                &mut doc,
                ObjectId::new(K::Surface, 1),
                &[
                    ("boundary_lines", Value::RefList(vec![4, 5, 6, 7])),
                    ("thickness", Value::Ref(1)),
                ],
            );
            let rigid = ObjectId::new(K::SurfaceSupport, 1);
            doc.model
                .set(rigid, "surfaces", Value::RefList(vec![1]))
                .unwrap();
        }
        if stage >= 4 {
            add(
                &mut doc,
                ObjectId::load(K::NodalLoad, 2, 1),
                &[
                    ("nodes", Value::RefList(vec![2])),
                    ("magnitude", Value::Float(10_000.0)),
                ],
            );
            add(
                &mut doc,
                ObjectId::load(K::MemberLoad, 3, 1),
                &[
                    ("members", Value::RefList(vec![2])),
                    ("magnitude", Value::Float(5_000.0)),
                ],
            );
            add(
                &mut doc,
                ObjectId::load(K::SurfaceLoad, 3, 1),
                &[
                    ("surfaces", Value::RefList(vec![1])),
                    ("uniform_magnitude", Value::Float(2_000.0)),
                ],
            );
        }
        let expected = doc.model.clone();
        let out = dir.path().join(format!("FemCraft stage {stage}.rf6"));
        doc.save(&out).unwrap();
        if let Ok(keep) = std::env::var("FEMCRAFT_KEEP_DIR") {
            std::fs::copy(&out, Path::new(&keep).join(out.file_name().unwrap())).unwrap();
        }
        let back = RfDocument::open(&out).unwrap();
        for &kind in K::ALL {
            for (id, o) in expected.iter_kind(kind) {
                let got = back.model.get(id).unwrap();
                for (k, v) in &o.values {
                    assert_eq!(got.get(k), v, "stage {stage}: {id:?}.{k}");
                }
            }
        }
    }
}

/// Imposed-load cases get RFEM's standard-dependent options row (without it RFEM's server
/// crashes opening the file), and a new load case copied from the self-weight case does not
/// keep its foundation self-weight switches.
#[test]
fn imposed_load_cases_complete() {
    let Some(template) = private("FEMCRAFT - RFEM6 Template.rf6") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut doc = RfDocument::open(&template).unwrap();
    // An existing permanent case made imposed, and a new imposed case.
    doc.model
        .set(ObjectId::new(K::LoadCase, 2), "action_category", Value::Int(36))
        .unwrap();
    add(
        &mut doc,
        ObjectId::new(K::LoadCase, 8),
        &[
            ("name", Value::Text("Roof imposed".into())),
            ("action_category", Value::Int(42)),
        ],
    );
    let out = dir.path().join("imposed.rf6");
    doc.save(&out).unwrap();

    let back = RfDocument::open(&out).unwrap();
    let conn = rusqlite::Connection::open(back.archive.db_path()).unwrap();
    let rows: Vec<(i64, i64, Option<i64>, i64, i64)> = conn
        .prepare(
            "SELECT l.userID, i.actionCategoryId, o.id, i.activeSelfWeightOnFoundations, \
             i.selfWeightActive FROM LoadCase l JOIN LoadCaseImplStatic i ON i.id = l.impl_id \
             LEFT JOIN LoadCaseStandardDependentOptions o ON o.id = \
             i.loadCaseStandardDependentOptions_id AND o.owner_id = i.id ORDER BY l.userID",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (no, cat, options, foundations, self_weight) in &rows {
        let imposed = [36, 42].contains(cat);
        assert_eq!(options.is_some(), imposed, "load case {no} (category {cat})");
        if *no == 8 {
            assert_eq!(foundations, self_weight, "load case 8 foundation switch");
        }
    }
    assert!(rows.iter().any(|r| r.0 == 8), "load case 8 saved");
}
