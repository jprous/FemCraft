//! Round trip on a small synthetic database shaped like RFEM's (object / impl / child tables).
//! Ids deliberately differ from object numbers to exercise reference resolution.

use std::io::{Read, Write};
use std::path::Path;

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;

const SCHEMA: &str = "
CREATE TABLE Material (id INTEGER PRIMARY KEY, no INTEGER, name TEXT, elasticity_modulus REAL);
CREATE TABLE Section (id INTEGER PRIMARY KEY, no INTEGER, name TEXT, material INTEGER, A REAL);
CREATE TABLE Node (id INTEGER PRIMARY KEY, no INTEGER, impl_id INTEGER, impl_table TEXT, comment TEXT);
CREATE TABLE NodeImplStandard (id INTEGER PRIMARY KEY, coordinate_1 REAL, coordinate_2 REAL, coordinate_3 REAL);
CREATE TABLE Line (id INTEGER PRIMARY KEY, no INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE LineImplPolyline (id INTEGER PRIMARY KEY);
CREATE TABLE LineImplPolyline_definition_nodes (id INTEGER PRIMARY KEY, owner_id INTEGER, node INTEGER);
CREATE TABLE Member (id INTEGER PRIMARY KEY, no INTEGER, line INTEGER, section_start INTEGER, section_end INTEGER);
CREATE TABLE LoadCase (id INTEGER PRIMARY KEY, no INTEGER, name TEXT, self_weight_active INTEGER);
CREATE TABLE NodalLoad (id INTEGER PRIMARY KEY, no INTEGER, load_case INTEGER, nodes TEXT, force_magnitude REAL);
CREATE TABLE Unknown (id INTEGER PRIMARY KEY, blob BLOB);
INSERT INTO Material VALUES (500, 1, 'S235', 2.1e11);
INSERT INTO Section VALUES (600, 1, 'IPE 200', 500, 0.00285);
INSERT INTO NodeImplStandard VALUES (11, 0, 0, 0), (12, 5, 0, 0);
INSERT INTO Node VALUES (101, 1, 11, 'NodeImplStandard', NULL), (102, 2, 12, 'NodeImplStandard', 'top');
INSERT INTO LineImplPolyline VALUES (21);
INSERT INTO Line VALUES (201, 1, 21, 'LineImplPolyline');
INSERT INTO LineImplPolyline_definition_nodes VALUES (1, 21, 101), (2, 21, 102);
INSERT INTO Member VALUES (301, 1, 201, 600, 600);
INSERT INTO LoadCase VALUES (701, 1, 'Self-weight', 1);
INSERT INTO NodalLoad VALUES (801, 1, 701, '101', -10000);
INSERT INTO Unknown VALUES (1, x'0102');
";

fn make_rf6(dir: &Path) -> std::path::PathBuf {
    let db = dir.join("model.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(SCHEMA)
        .unwrap();
    let path = dir.join("test.rf6");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("format.txt", opts).unwrap();
    zip.write_all(b"RFEM 6.99 test").unwrap();
    zip.start_file("model.db", opts).unwrap();
    zip.write_all(&std::fs::read(&db).unwrap()).unwrap();
    zip.start_file("views.xml", opts).unwrap();
    zip.write_all(b"<views/>").unwrap();
    zip.finish().unwrap();
    path
}

fn entry(path: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut buf = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

#[test]
fn load_edit_save() {
    let dir = tempfile::tempdir().unwrap();
    let src = make_rf6(dir.path());
    let mut doc = RfDocument::open(&src).unwrap();
    let m = &doc.model;

    assert_eq!(m.node_position(2), Some([5.0, 0.0, 0.0]));
    let node = m.get(ObjectId::new(K::Node, 2)).unwrap();
    assert_eq!(node.get("type"), &Value::Text("TYPE_STANDARD".into()));
    assert_eq!(node.get("comment"), &Value::Text("top".into()));
    assert_eq!(
        m.line_nodes(1),
        &[1, 2],
        "child-table ids resolved to node numbers"
    );
    assert_eq!(m.member_nodes(1), Some((1, 2)));
    let member = m.get(ObjectId::new(K::Member, 1)).unwrap();
    assert_eq!(member.get("section_start"), &Value::Ref(1));
    let section = m.get(ObjectId::new(K::Section, 1)).unwrap();
    assert_eq!(section.get("material"), &Value::Ref(1));
    let load = m
        .get(ObjectId::load(K::NodalLoad, 1, 1))
        .expect("load keyed by load case number");
    assert_eq!(load.get("nodes"), &Value::RefList(vec![1]));
    let lc = m.get(ObjectId::new(K::LoadCase, 1)).unwrap();
    assert_eq!(lc.get("self_weight_active"), &Value::Bool(true));
    assert!(doc.report.to_string().contains("Solid"));

    // Unchanged save keeps every entry byte-identical.
    let same = dir.path().join("same.rf6");
    doc.save(&same).unwrap();
    for name in ["format.txt", "model.db", "views.xml"] {
        assert_eq!(entry(&src, name), entry(&same, name), "{name} changed");
    }

    // Edit scalar fields and a reference, save, reopen.
    doc.model
        .set(ObjectId::new(K::Node, 2), "coordinate_1", Value::Float(6.0))
        .unwrap();
    doc.model
        .set(ObjectId::new(K::Section, 1), "A", Value::Float(0.003))
        .unwrap();
    doc.model
        .set(
            ObjectId::load(K::NodalLoad, 1, 1),
            "nodes",
            Value::RefList(vec![2]),
        )
        .unwrap();
    let edited = dir.path().join("edited.rf6");
    doc.save(&edited).unwrap();
    assert!(!doc.model.is_modified());

    let doc2 = RfDocument::open(&edited).unwrap();
    assert_eq!(doc2.model.node_position(2), Some([6.0, 0.0, 0.0]));
    let a = doc2
        .model
        .get(ObjectId::new(K::Section, 1))
        .unwrap()
        .get("A")
        .as_f64();
    assert_eq!(a, Some(0.003));
    let load = doc2.model.get(ObjectId::load(K::NodalLoad, 1, 1)).unwrap();
    assert_eq!(load.get("nodes"), &Value::RefList(vec![2]));
    assert_eq!(entry(&src, "views.xml"), entry(&edited, "views.xml"));

    // List fields stored in child tables are read-only for now.
    let mut doc3 = RfDocument::open(&edited).unwrap();
    doc3.model
        .set(
            ObjectId::new(K::Line, 1),
            "definition_nodes",
            Value::RefList(vec![2, 1]),
        )
        .unwrap();
    assert!(doc3.save(&dir.path().join("x.rf6")).is_err());
}
