//! File > New: the blank model made from a local RFEM file (`testdata/private/`, never
//! committed; skipped when absent) opens, has no geometry, and saves edits.

use std::path::Path;

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;
use femcraft_rfdb::build::blank_model;

#[test]
fn blank_model_opens_and_saves() {
    let template =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/private/Solids_check_RBeam.rf6");
    if !template.is_file() {
        eprintln!("skipped: {} not found", template.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let blank = dir.path().join("new model.rf6");
    blank_model(&template, &blank).unwrap();

    let mut doc = RfDocument::open(&blank).unwrap();
    let m = &doc.model;
    for k in [
        K::Node,
        K::Line,
        K::Member,
        K::Surface,
        K::Solid,
        K::NodalSupport,
    ] {
        assert_eq!(m.count(k), 0, "{k:?}");
    }
    assert_eq!(m.count(K::LoadCase), 1);
    assert_eq!(m.count(K::Thickness), 1);
    assert!(m.count(K::Material) >= 1 && m.count(K::Section) >= 1);
    let lc = ObjectId::new(K::LoadCase, 1);
    assert_eq!(
        m.get(lc).unwrap().get("name"),
        &Value::Text("Self-weight".into())
    );

    // Edit and "Save As" elsewhere, as for an untitled model.
    let t1 = ObjectId::new(K::Thickness, 1);
    doc.model
        .set(t1, "uniform_thickness", Value::Float(0.25))
        .unwrap();
    doc.model
        .set(lc, "comment", Value::Text("FemCraft new".into()))
        .unwrap();
    let out = dir.path().join("Model 1.rf6");
    doc.save(&out).unwrap();
    let back = RfDocument::open(&out).unwrap();
    assert_eq!(
        back.model.get(t1).unwrap().get("uniform_thickness"),
        &Value::Float(0.25)
    );
    assert_eq!(
        back.model.get(lc).unwrap().get("comment"),
        &Value::Text("FemCraft new".into())
    );
}
