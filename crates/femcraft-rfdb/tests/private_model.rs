//! Checks against a real RFEM model kept locally in `testdata/private/` (never committed).
//! Skipped when the file is absent, for example on CI.
//!
//! Set `FEMCRAFT_TEST_MODEL` to use another file than `testdata/private/test.rf6`.

use std::io::Read;
use std::path::{Path, PathBuf};

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;

fn model_path() -> Option<PathBuf> {
    let p = std::env::var_os("FEMCRAFT_TEST_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/private/test.rf6")
        });
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipped: {} not found", p.display());
        None
    }
}

fn entry(path: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut buf = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

#[test]
fn real_model_round_trip() {
    let Some(src) = model_path() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut doc = RfDocument::open(&src).unwrap();

    // Every line resolves to existing nodes, every member to a line.
    for (id, _) in doc.model.iter_kind(K::Line) {
        for n in doc.model.line_nodes(id.no) {
            assert!(
                doc.model.node_position(*n).is_some(),
                "line {} node {n}",
                id.no
            );
        }
    }
    for (id, _) in doc.model.iter_kind(K::Member) {
        assert!(doc.model.member_nodes(id.no).is_some(), "member {}", id.no);
    }

    // Unchanged save: model.db byte-identical.
    let same = dir.path().join("same.rf6");
    doc.save(&same).unwrap();
    assert!(
        entry(&src, "model.db") == entry(&same, "model.db"),
        "model.db changed"
    );

    // Edit the first node's Z and the first member load's magnitude, then reload.
    let (node, _) = doc.model.iter_kind(K::Node).next().unwrap();
    let z = doc.model.node_position(node.no).unwrap()[2] + 0.125;
    doc.model
        .set(node, "coordinate_3", Value::Float(z))
        .unwrap();
    let load: Option<ObjectId> = doc.model.iter_kind(K::MemberLoad).next().map(|(id, _)| id);
    if let Some(load) = load {
        doc.model
            .set(load, "magnitude", Value::Float(1234.5))
            .unwrap();
    }
    let edited = dir.path().join("edited.rf6");
    doc.save(&edited).unwrap();

    let doc2 = RfDocument::open(&edited).unwrap();
    assert_eq!(doc2.model.node_position(node.no).unwrap()[2], z);
    if let Some(load) = load {
        let m = doc2.model.get(load).unwrap().get("magnitude").as_f64();
        assert_eq!(m, Some(1234.5));
    }
    assert_eq!(doc2.model.len(), doc.model.len());
}
