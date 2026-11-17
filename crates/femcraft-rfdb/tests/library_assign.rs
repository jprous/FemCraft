//! Assigning library items (`RfDocument::assign_library_item`) to real models: needs the
//! private `testdata/private/Prokon Model1 - FemCraft node2.rf6` and FemCraft's library
//! (materials.db, sections.db; see `femcraft_rfdb::library`). Skipped when either is missing.
//! `FEMCRAFT_KEEP=<path>` keeps the saved model (to open it in RFEM).

use std::path::{Path, PathBuf};

use femcraft_model::{ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::{Library, LibraryItem, LibraryKind, RfDocument};
use rusqlite::Connection;

fn private(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name)
}

fn setup(model: &str) -> Option<(Library, PathBuf)> {
    let src = private(model);
    let Some(lib) = Library::find(&Library::default_dirs()) else {
        eprintln!("skipped: no FemCraft library");
        return None;
    };
    if !src.is_file() {
        eprintln!("skipped: {} not found", src.display());
        return None;
    }
    Some((lib, src))
}

fn float(doc: &RfDocument, id: ObjectId, key: &str) -> f64 {
    doc.model
        .get(id)
        .unwrap()
        .get(key)
        .as_f64()
        .unwrap_or(f64::NAN)
}

fn item(lib: &Library, kind: LibraryKind, name: &str) -> LibraryItem {
    lib.get(kind, name)
        .unwrap()
        .unwrap_or_else(|| panic!("{name} in the library"))
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// What RFEM needs of a library section (RFEM reported files without these as corrupted):
/// its material characteristics and outlines lists.
fn assert_complete_section(conn: &Connection, user_id: u32) {
    let lib: i64 = conn
        .query_row(
            "SELECT i.librarySection_id FROM Section s JOIN SectionImplStandardizedSteel i \
             ON i.id = s.impl_id WHERE s.userID = ?1",
            [user_id],
            |r| r.get(0),
        )
        .unwrap();
    let smc = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM DatabaseSection_sectionMaterialCharacteristics WHERE id = {lib}"
        ),
    );
    let outlines = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM SectionTopology_outlines WHERE id = (SELECT topology_id FROM \
             SectionData WHERE id = (SELECT sectionData_id FROM DatabaseSection WHERE id = {lib}))"
        ),
    );
    assert!(
        smc > 0 && outlines > 0,
        "section {user_id}: smc {smc}, outlines {outlines}"
    );
}

#[test]
fn assign_section_and_material() {
    let Some((lib, src)) = setup("Prokon Model1 - FemCraft node2.rf6") else {
        return;
    };
    let mut doc = RfDocument::open(&src).unwrap();
    let section = ObjectId::new(K::Section, 1);
    let material = ObjectId::new(K::Material, 1);
    let members_before: Vec<(ObjectId, Value)> = doc
        .model
        .iter_kind(K::Member)
        .map(|(id, o)| (id, o.get("section_start").clone()))
        .collect();
    let mat_before = doc.model.get(section).unwrap().get("material").clone();

    // A pending edit survives the reload.
    let node = ObjectId::new(K::Node, 1);
    let x = float(&doc, node, "coordinate_1") + 0.125;
    doc.model
        .set(node, "coordinate_1", Value::Float(x))
        .unwrap();

    let ipe = item(
        &lib,
        LibraryKind::Section,
        "IPE AA 160 | DIN 1025:1965 | Macsteel",
    );
    let ub = item(
        &lib,
        LibraryKind::Section,
        "UB 203x133x25 | BS 4-1:1993 | Macsteel",
    );
    let a = |it: &LibraryItem| {
        lib.values(it)
            .unwrap()
            .into_iter()
            .find(|v| v.key == "A")
            .and_then(|v| v.value_si)
            .unwrap()
    };
    let db_sections = |doc: &RfDocument| {
        count(
            &Connection::open(doc.archive.db_path()).unwrap(),
            "SELECT COUNT(*) FROM DatabaseSection",
        )
    };
    let before = db_sections(&doc);
    // Twice: the first copy's rows are removed again (not shared).
    doc.assign_library_item(section, &lib, &ub).unwrap();
    doc.assign_library_item(section, &lib, &ipe).unwrap();
    assert!(doc.is_modified());
    assert!(
        (float(&doc, section, "A") - a(&ipe)).abs() < 1e-12,
        "section A is the library's"
    );
    assert_eq!(
        doc.model
            .get(section)
            .unwrap()
            .get("database_section_id")
            .as_i64(),
        ipe.rfem_library_id
    );
    assert_eq!(doc.model.get(section).unwrap().get("material"), &mat_before);
    assert!(
        (float(&doc, node, "coordinate_1") - x).abs() < 1e-12,
        "edit kept"
    );

    let s275 = item(&lib, LibraryKind::Material, "S275JR | SANS 50025-2");
    doc.assign_library_item(material, &lib, &s275).unwrap();
    // SANS 50025-2 steel: E = 200 GPa.
    assert!((float(&doc, material, "elasticity_modulus") - 200e9).abs() < 1.0);
    assert_eq!(
        doc.model
            .get(material)
            .unwrap()
            .get("database_material_id")
            .as_i64(),
        s275.rfem_library_id
    );
    // Wrong kind is refused.
    assert!(doc.assign_library_item(material, &lib, &ub).is_err());

    let dir = tempfile::tempdir().unwrap();
    let out = std::env::var_os("FEMCRAFT_KEEP")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.path().join("assigned.rf6"));
    doc.save(&out).unwrap();
    assert!(!doc.is_modified());

    let back = RfDocument::open(&out).unwrap();
    assert!((float(&back, section, "A") - a(&ipe)).abs() < 1e-12);
    assert!((float(&back, material, "elasticity_modulus") - 200e9).abs() < 1.0);
    assert!((float(&back, node, "coordinate_1") - x).abs() < 1e-12);
    for (id, sec) in members_before {
        assert_eq!(
            back.model.get(id).unwrap().get("section_start"),
            &sec,
            "{id:?}"
        );
    }
    // The replaced section's library rows went with it (section 1 shared none).
    assert_eq!(db_sections(&back), before);
    let conn = Connection::open(back.archive.db_path()).unwrap();
    assert_complete_section(&conn, 1);
    let fk = femcraft_rfdb::build::foreign_key_problems(&conn).unwrap();
    assert!(fk.is_empty(), "foreign key problems: {fk:?}");
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

/// `Prokon Model1.rf6` was saved with an incomplete library section 1 (before value_id lists
/// were copied); assigning it again repairs it.
#[test]
fn reassigning_repairs_incomplete_section() {
    let Some((lib, src)) = setup("Prokon Model1.rf6") else {
        return;
    };
    let mut doc = RfDocument::open(&src).unwrap();
    let section = ObjectId::new(K::Section, 1);
    let Some(id) = doc
        .model
        .get(section)
        .unwrap()
        .get("database_section_id")
        .as_i64()
    else {
        return;
    };
    let it = lib
        .list(LibraryKind::Section, &Default::default())
        .unwrap()
        .into_iter()
        .find(|i| i.rfem_library_id == Some(id))
        .expect("section 1 is a library section");
    doc.assign_library_item(section, &lib, &it).unwrap();
    let conn = Connection::open(doc.archive.db_path()).unwrap();
    assert_complete_section(&conn, 1);
    assert!(
        femcraft_rfdb::build::foreign_key_problems(&conn)
            .unwrap()
            .is_empty()
    );
}
