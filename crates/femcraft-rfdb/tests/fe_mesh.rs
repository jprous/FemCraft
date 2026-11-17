//! FE mesh read from a model RFEM has meshed (`testdata/private/FemCraft pit 1 geometry-meshed.rf6`,
//! a 3 m pit of 5 surfaces, 0.25 m mesh). Skipped when the file is absent, for example on CI.

use std::path::{Path, PathBuf};

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;

fn model_path() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private/FemCraft pit 1 geometry-meshed.rf6");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipped: {} not found", p.display());
        None
    }
}

#[test]
fn reads_mesh_matching_the_model() {
    let Some(path) = model_path() else { return };
    let doc = RfDocument::open(&path).unwrap();
    let mesh = doc.fe_mesh().unwrap().expect("model has a mesh");
    assert_eq!(mesh.nodes.len(), 745);
    assert_eq!(mesh.elements.len(), 720);
    for s in 1..=5 {
        assert_eq!(
            mesh.element_surface.iter().filter(|&&x| x == s).count(),
            144
        );
    }
    // Each model line's FE chain runs between that line's end nodes.
    assert_eq!(mesh.lines.len(), 12);
    for (line, chain) in &mesh.lines {
        let ends = doc.model.line_nodes(*line);
        let (a, b) = (chain[0], *chain.last().unwrap());
        let pa = mesh.node(a).unwrap();
        let pb = mesh.node(b).unwrap();
        let qa = doc.model.node_position(ends[0]).unwrap();
        let qb = doc.model.node_position(*ends.last().unwrap()).unwrap();
        let d = |p: [f64; 3], q: [f64; 3]| (0..3).map(|i| (p[i] - q[i]).abs()).fold(0.0, f64::max);
        assert!(d(pa, qa) < 1e-6 && d(pb, qb) < 1e-6, "line {line}");
        // 3 m lines at a 0.25 m target: 12 divisions or more (RFEM refines some).
        assert!(chain.len() >= 13, "line {line}");
        assert!(chain.iter().all(|&n| mesh.node(n).is_some()));
    }
}

#[test]
fn edited_model_saves_without_stale_mesh() {
    let Some(path) = model_path() else { return };
    let dir = tempfile::tempdir().unwrap();

    let mut doc = RfDocument::open(&path).unwrap();
    let out = dir.path().join("unchanged.rf6");
    doc.save(&out).unwrap();
    assert!(RfDocument::open(&out).unwrap().fe_mesh().unwrap().is_some());

    let id = ObjectId::new(K::Node, 1);
    let x = doc.model.node_position(1).unwrap()[0];
    doc.model
        .set(id, "coordinate_1", Value::Float(x + 0.1))
        .unwrap();
    let out = dir.path().join("edited.rf6");
    doc.save(&out).unwrap();
    let saved = RfDocument::open(&out).unwrap();
    assert!(saved.fe_mesh().unwrap().is_none());
    assert!(
        !saved
            .archive
            .entries
            .iter()
            .any(|e| e.starts_with("!solver/"))
    );
}
