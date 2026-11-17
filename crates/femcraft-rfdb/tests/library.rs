//! Library lookups (`femcraft_rfdb::library`) on a small index-only library.

use std::path::Path;

use femcraft_rfdb::library::{Library, LibraryFilter, LibraryKind};
use rusqlite::Connection;

/// A library with only the index tables (enough for lookups).
fn sample(dir: &Path) {
    let c = Connection::open(dir.join("sections.db")).unwrap();
    c.execute_batch(
        "CREATE TABLE femcraft_library (kind TEXT, user_id INTEGER, name TEXT, \
         designation TEXT, standard TEXT, manufacturer TEXT, region TEXT, family TEXT, \
         category TEXT, rfem_library_id INTEGER, rfem_version TEXT); \
         CREATE TABLE femcraft_library_value (kind TEXT, user_id INTEGER, key TEXT, \
         idx INTEGER, value_si REAL, value_text TEXT, thickness_si REAL); \
         INSERT INTO femcraft_library VALUES ('section', 1, \
         'IPE 200 | EN 10365:2017 | ArcelorMittal (2018)', 'IPE 200', 'EN 10365:2017', \
         'ArcelorMittal (2018)', 'EU', 'IPE', 'hot_rolled', 85940, '6.16.0005'), \
         ('section', 2, 'UB 203x133x25 | BS 4-1:1993 | Macsteel', 'UB 203x133x25', \
         'BS 4-1:1993', 'Macsteel', 'ZA', 'UB', 'hot_rolled', 61417, '6.16.0005'); \
         INSERT INTO femcraft_library_value VALUES ('section', 1, 'A', 0, 0.00285, NULL, \
         NULL), ('section', 1, 'I_y', 0, 1.943e-5, NULL, NULL);",
    )
    .unwrap();
}

#[test]
fn lookups() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let lib = Library::find(&[dir.path().to_owned()]).unwrap();
    let za = LibraryFilter {
        region: "za".into(),
        ..Default::default()
    };
    let found = lib.list(LibraryKind::Section, &za).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].manufacturer, "Macsteel");
    let ipe = lib.get(LibraryKind::Section, "IPE 200").unwrap().unwrap();
    assert_eq!(ipe.rfem_library_id, Some(85940));
    let v = lib.values(&ipe).unwrap();
    assert_eq!(v[0].key, "A");
    assert_eq!(v[1].value_si, Some(1.943e-5));
    assert_eq!(lib.count(LibraryKind::Section), 2);
    assert_eq!(lib.count(LibraryKind::Material), 0);
    assert!(lib.get(LibraryKind::Section, "IPE 999").unwrap().is_none());
}
