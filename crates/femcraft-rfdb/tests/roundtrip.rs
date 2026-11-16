//! Round trip on a small synthetic database laid out like RFEM 6.16's `model.db`
//! (main / impl / list / part tables, key-value maps). Written for FemCraft; contains no RFEM
//! data. Database ids deliberately differ from object numbers (`userID`).

use std::io::{Read, Write};
use std::path::Path;

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;

const SCHEMA: &str = "
CREATE TABLE Material (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE MaterialImpl (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE MaterialImpl_materialDataVector (id INTEGER, container_order INTEGER, reference_id INTEGER, reference_table TEXT);
CREATE TABLE MaterialData (id INTEGER PRIMARY KEY, properties_id INTEGER);
CREATE TABLE MaterialProperties (id INTEGER PRIMARY KEY);
CREATE TABLE MaterialProperties_propertiesMap_keys (id INTEGER, container_order INTEGER, value TEXT);
CREATE TABLE MaterialProperties_propertiesMap_values (id INTEGER, container_order INTEGER, reference_id INTEGER, reference_table TEXT);
CREATE TABLE MaterialPropertyDouble (id INTEGER PRIMARY KEY, propertyKey TEXT, valueSI REAL);

CREATE TABLE Section (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE SectionImplStandardizedSteel (id INTEGER PRIMARY KEY, material_id INTEGER, librarySection_id INTEGER, librarySection_table TEXT);
CREATE TABLE DatabaseSection (id INTEGER PRIMARY KEY, sectionData_id INTEGER);
CREATE TABLE SectionData (id INTEGER PRIMARY KEY);
CREATE TABLE SectionData_parameterValues_keys (id INTEGER, container_order INTEGER, value TEXT);
CREATE TABLE SectionData_parameterValues_values (id INTEGER, container_order INTEGER, valueSI REAL);

CREATE TABLE Node (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT, comment TEXT);
CREATE TABLE NodeImplStandard (id INTEGER PRIMARY KEY, coordinates_x REAL, coordinates_y REAL, coordinates_z REAL);
CREATE TABLE Line (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE LineImplPolyline (id INTEGER PRIMARY KEY);
CREATE TABLE LineImplPolyline_definitionNodes (id INTEGER, container_order INTEGER, reference_id INTEGER, reference_table TEXT);
CREATE TABLE Member (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE MemberImplBeam (id INTEGER PRIMARY KEY, line_id INTEGER, sectionStart_id INTEGER, angle REAL);

CREATE TABLE NodalSupport (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE NodalSupportImpl (id INTEGER PRIMARY KEY, name TEXT, springConstants_id INTEGER);
CREATE TABLE NodalSupportImpl_nodes (id INTEGER, container_order INTEGER, value_id INTEGER);
CREATE TABLE SpringConstants (id INTEGER PRIMARY KEY, owner_id INTEGER, owner_table TEXT, springConstantAlongX REAL, springConstantAlongY REAL, springConstantAlongZ REAL, springConstantAroundX REAL, springConstantAroundY REAL, springConstantAroundZ REAL);

CREATE TABLE LoadCase (id INTEGER PRIMARY KEY, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE LoadCaseImplStatic (id INTEGER PRIMARY KEY, name TEXT, selfWeightActive INTEGER, selfWeightFactors_z REAL);
CREATE TABLE MemberLoad (id INTEGER PRIMARY KEY, parentModelObject_id INTEGER, parentModelObject_table TEXT, userID INTEGER, impl_id INTEGER, impl_table TEXT);
CREATE TABLE LineTypeLoadImplForce (id INTEGER PRIMARY KEY, parent_id INTEGER, loadDirection INTEGER, loadDistribution INTEGER);
CREATE TABLE LineTypeLoadImplForce_assignedTo (id INTEGER, container_order INTEGER, reference_id INTEGER, reference_table TEXT);
CREATE TABLE LineTypeLoadImplForce_magnitudes (id INTEGER, container_order INTEGER, temperaturesOrMagnitudeFirstMagnitude REAL, temperaturesOrMagnitudeSecondMagnitude REAL);
CREATE TABLE Unknown (id INTEGER PRIMARY KEY, blob BLOB);

INSERT INTO Material VALUES (500, 1, 50, 'MaterialImpl');
INSERT INTO MaterialImpl VALUES (50, NULL);
INSERT INTO MaterialImpl_materialDataVector VALUES (50, 0, 51, 'MaterialData');
INSERT INTO MaterialData VALUES (51, 52);
INSERT INTO MaterialProperties VALUES (52);
INSERT INTO MaterialProperties_propertiesMap_keys VALUES (52, 0, 'E'), (52, 1, 'nu');
INSERT INTO MaterialProperties_propertiesMap_values VALUES (52, 0, 53, 'MaterialPropertyDouble'), (52, 1, 54, 'MaterialPropertyDouble');
INSERT INTO MaterialPropertyDouble VALUES (53, 'E', 2.1e11), (54, 'nu', 0.3);

INSERT INTO Section VALUES (600, 1, 60, 'SectionImplStandardizedSteel');
INSERT INTO SectionImplStandardizedSteel VALUES (60, 500, 61, 'DatabaseSection');
INSERT INTO DatabaseSection VALUES (61, 62);
INSERT INTO SectionData VALUES (62);
INSERT INTO SectionData_parameterValues_keys VALUES (62, 0, 'A'), (62, 1, 'I_y'), (62, 2, 'A_v_z');
INSERT INTO SectionData_parameterValues_values VALUES (62, 0, 0.00285), (62, 1, 1.943e-5), (62, 2, 0.0014);

INSERT INTO NodeImplStandard VALUES (11, 0, 0, 0), (12, 5, 0, 0);
INSERT INTO Node VALUES (101, 1, 11, 'NodeImplStandard', NULL), (102, 2, 12, 'NodeImplStandard', 'top');
INSERT INTO LineImplPolyline VALUES (21);
INSERT INTO Line VALUES (201, 1, 21, 'LineImplPolyline');
INSERT INTO LineImplPolyline_definitionNodes VALUES (21, 0, 101, 'Node'), (21, 1, 102, 'Node');
INSERT INTO MemberImplBeam VALUES (31, 201, 600, 0);
INSERT INTO Member VALUES (301, 1, 31, 'MemberImplBeam');

INSERT INTO NodalSupport VALUES (401, 1, 41, 'NodalSupportImpl');
INSERT INTO NodalSupportImpl VALUES (41, 'Hinged', 42);
INSERT INTO NodalSupportImpl_nodes VALUES (41, 0, 101);
INSERT INTO SpringConstants VALUES (42, 41, 'NodalSupportImpl', 1e308 * 10, 1e308 * 10, 1e308 * 10, 0, 0, 0);

INSERT INTO LoadCase VALUES (701, 1, 71, 'LoadCaseImplStatic');
INSERT INTO LoadCaseImplStatic VALUES (71, 'Dead', 1, 1.0);
INSERT INTO MemberLoad VALUES (801, 701, 'LoadCase', 1, 81, 'LineTypeLoadImplForce');
INSERT INTO LineTypeLoadImplForce VALUES (81, 801, 2, 0);
INSERT INTO LineTypeLoadImplForce_assignedTo VALUES (81, 0, 301, 'Member');
INSERT INTO LineTypeLoadImplForce_magnitudes VALUES (81, 0, 330, -1e308 * 10);
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

fn f(doc: &RfDocument, id: ObjectId, key: &str) -> Value {
    doc.model.get(id).expect("object exists").get(key).clone()
}

#[test]
fn load_edit_save() {
    let dir = tempfile::tempdir().unwrap();
    let src = make_rf6(dir.path());
    let mut doc = RfDocument::open(&src).unwrap();
    let node2 = ObjectId::new(K::Node, 2);
    let section = ObjectId::new(K::Section, 1);
    let member = ObjectId::new(K::Member, 1);
    let support = ObjectId::new(K::NodalSupport, 1);
    let load = ObjectId::load(K::MemberLoad, 1, 1);

    // Plain, impl and reference columns.
    assert_eq!(doc.model.node_position(2), Some([5.0, 0.0, 0.0]));
    assert_eq!(f(&doc, node2, "type"), Value::Text("TYPE_STANDARD".into()));
    assert_eq!(f(&doc, node2, "comment"), Value::Text("top".into()));
    assert_eq!(f(&doc, member, "section_start"), Value::Ref(1));
    assert_eq!(f(&doc, section, "material"), Value::Ref(1));
    // List tables (ids resolved to numbers).
    assert_eq!(doc.model.line_nodes(1), &[1, 2]);
    assert_eq!(doc.model.member_nodes(1), Some((1, 2)));
    assert_eq!(f(&doc, support, "nodes"), Value::RefList(vec![1]));
    // Part tables.
    assert_eq!(f(&doc, support, "spring_x"), Value::Float(f64::INFINITY));
    assert_eq!(f(&doc, load, "magnitude"), Value::Float(330.0));
    // Key / value maps.
    assert_eq!(f(&doc, section, "A"), Value::Float(0.00285));
    assert_eq!(f(&doc, section, "A_z"), Value::Float(0.0014));
    let mat = ObjectId::new(K::Material, 1);
    assert_eq!(f(&doc, mat, "elasticity_modulus"), Value::Float(2.1e11));
    assert_eq!(f(&doc, mat, "poisson_ratio"), Value::Float(0.3));
    // Loads: type from impl table, load case from parentModelObject_id.
    assert_eq!(
        f(&doc, load, "load_type"),
        Value::Text("LOAD_TYPE_FORCE".into())
    );
    assert_eq!(f(&doc, load, "members"), Value::RefList(vec![1]));
    let lc = ObjectId::new(K::LoadCase, 1);
    assert_eq!(f(&doc, lc, "self_weight_active"), Value::Bool(true));
    assert!(doc.report.to_string().contains("Solid"));

    // Unchanged save keeps every entry byte-identical.
    let same = dir.path().join("same.rf6");
    doc.save(&same).unwrap();
    for name in ["format.txt", "model.db", "views.xml"] {
        assert_eq!(entry(&src, name), entry(&same, name), "{name} changed");
    }

    // Edit an impl column, a reference, a part-table column; save; reopen.
    doc.model
        .set(node2, "coordinate_1", Value::Float(6.0))
        .unwrap();
    doc.model
        .set(member, "rotation_angle", Value::Float(0.5))
        .unwrap();
    doc.model
        .set(load, "magnitude", Value::Float(450.0))
        .unwrap();
    doc.model
        .set(support, "spring_z", Value::Float(1e6))
        .unwrap();
    doc.model.set(lc, "name", Value::Text("G".into())).unwrap();
    let edited = dir.path().join("edited.rf6");
    doc.save(&edited).unwrap();
    assert!(!doc.model.is_modified());

    let doc2 = RfDocument::open(&edited).unwrap();
    assert_eq!(doc2.model.node_position(2), Some([6.0, 0.0, 0.0]));
    assert_eq!(f(&doc2, member, "rotation_angle"), Value::Float(0.5));
    assert_eq!(f(&doc2, load, "magnitude"), Value::Float(450.0));
    assert_eq!(f(&doc2, support, "spring_z"), Value::Float(1e6));
    assert_eq!(f(&doc2, support, "spring_x"), Value::Float(f64::INFINITY));
    assert_eq!(f(&doc2, lc, "name"), Value::Text("G".into()));
    assert_eq!(entry(&src, "views.xml"), entry(&edited, "views.xml"));

    // Library properties are read-only.
    let mut d = RfDocument::open(&edited).unwrap();
    d.model.set(section, "A", Value::Float(0.003)).unwrap();
    assert!(
        d.save(&dir.path().join("x.rf6")).is_err(),
        "A should be read-only"
    );

    // List fields: rewritten as list rows holding db ids, in order.
    let mut d = RfDocument::open(&edited).unwrap();
    let line = ObjectId::new(K::Line, 1);
    d.model
        .set(line, "definition_nodes", Value::RefList(vec![2, 1]))
        .unwrap();
    d.model
        .set(support, "nodes", Value::RefList(vec![1, 2]))
        .unwrap();
    let lists = dir.path().join("lists.rf6");
    d.save(&lists).unwrap();
    let d2 = RfDocument::open(&lists).unwrap();
    assert_eq!(d2.model.line_nodes(1), &[2, 1]);
    assert_eq!(f(&d2, support, "nodes"), Value::RefList(vec![1, 2]));
}
