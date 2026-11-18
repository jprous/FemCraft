//! Verifies that `Library::duplicate_concrete_item` creates an exact copy of a parametric
//! concrete section's RFEM row tree without duplicating list table entries.
//! Skipped when the FemCraft library is not found.

use femcraft_rfdb::Library;
use rusqlite::Connection;

fn count(conn: &Connection, sql: &str, id: i64) -> i64 {
    conn.query_row(sql, [id], |r| r.get(0)).unwrap()
}

#[test]
fn duplicate_concrete_no_list_doubling() {
    let Some(lib) = Library::find(&Library::default_dirs()) else {
        eprintln!("skipped: no FemCraft library");
        return;
    };

    // The first parametric concrete section is the rectangular template (userID 883).
    let template_uid: i64 = 883;
    let new_uid: i64 = 99999;

    // Clean up any previous test run.
    {
        let conn = Connection::open(lib.db_path(femcraft_rfdb::LibraryKind::Section)).unwrap();
        let _ = conn.execute(
            "DELETE FROM femcraft_library WHERE kind = 'section' AND user_id = ?1",
            [new_uid],
        );
        }
    // delete_concrete_rows is private; use delete_item which calls it internally.
    let _ = lib.delete_item(femcraft_rfdb::LibraryKind::Section, new_uid);

    // Duplicate the template.
    lib.duplicate_concrete_item(template_uid, new_uid)
        .expect("duplicate_concrete_item");

    // Open the library and compare list entry counts.
    let conn = Connection::open(lib.db_path(femcraft_rfdb::LibraryKind::Section)).unwrap();

    // Find the template's RFEM ids.
    let (t_sipm, t_ps, t_ship, t_sd, t_topo): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT sm.id, sm.librarySection_id, sm.sectionHybridImplPart_id, \
             ps.sectionData_id, sd.topology_id \
             FROM Section sec \
             JOIN SectionImplParametricMassive sm ON sm.id = sec.impl_id \
             JOIN ParametricSection ps ON ps.id = sm.librarySection_id \
             JOIN SectionData sd ON sd.id = ps.sectionData_id \
             WHERE sec.userID = ?1",
            [template_uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();

    // Find the duplicate's RFEM ids.
    let (d_sipm, d_ps, d_ship, d_sd, d_topo): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT sm.id, sm.librarySection_id, sm.sectionHybridImplPart_id, \
             ps.sectionData_id, sd.topology_id \
             FROM Section sec \
             JOIN SectionImplParametricMassive sm ON sm.id = sec.impl_id \
             JOIN ParametricSection ps ON ps.id = sm.librarySection_id \
             JOIN SectionData sd ON sd.id = ps.sectionData_id \
             WHERE sec.userID = ?1",
            [new_uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("duplicate Section not found");

    eprintln!("Template: SIPM={t_sipm} PS={t_ps} SHIP={t_ship} SD={t_sd} Topo={t_topo}");
    eprintln!("Duplicate: SIPM={d_sipm} PS={d_ps} SHIP={d_ship} SD={d_sd} Topo={d_topo}");

    // Compare counts for every list table.
    let checks: &[(&str, i64, i64)] = &[
        ("ParametricSection_parameterValues", t_ps, d_ps),
        (
            "ParametricSection_sectionMaterialCharacteristics",
            t_ps,
            d_ps,
        ),
        ("SectionHybridImplPart_materials", t_ship, d_ship),
        (
            "SectionImplParametricMassive_addonProperties_keys",
            t_sipm,
            d_sipm,
        ),
        (
            "SectionImplParametricMassive_addonProperties_values",
            t_sipm,
            d_sipm,
        ),
        (
            "SectionImplParametricMassive_costEstimationItems_keys",
            t_sipm,
            d_sipm,
        ),
        (
            "SectionImplParametricMassive_costEstimationItems_values",
            t_sipm,
            d_sipm,
        ),
        ("SectionData_bitmapDimensions", t_sd, d_sd),
        ("SectionData_dimensions", t_sd, d_sd),
        ("SectionData_parameterValues_keys", t_sd, d_sd),
        ("SectionData_parameterValues_values", t_sd, d_sd),
        ("SectionTopology_outlines", t_topo, d_topo),
        ("SectionTopology_stressPoints", t_topo, d_topo),
        ("SectionTopology_stressPointConnections", t_topo, d_topo),
    ];

    let mut failed = false;
    for (table, tid, did) in checks {
        let sql = format!("SELECT COUNT(*) FROM [{table}] WHERE id = ?1");
        let tc = count(&conn, &sql, *tid);
        let dc = count(&conn, &sql, *did);
        let status = if tc == dc {
            "OK"
        } else {
            failed = true;
            "DOUBLED"
        };
        eprintln!("{status:8} {table}: template={tc} duplicate={dc}");
    }

    // Clean up: delete the test duplicate.
    drop(conn);
    let _ = lib.delete_item(femcraft_rfdb::LibraryKind::Section, new_uid);

    assert!(!failed, "Some list tables have doubled entries in the duplicate");
}

/// RFEM reads a parametric section's dimensions from `ParametricSection_parameterValues`
/// (and the outline from the topology), so `set_concrete_values` must write those too.
#[test]
fn set_concrete_values_writes_master_dimensions() {
    use femcraft_rfdb::{LibraryKind, LibraryValue};
    let Some(lib) = Library::find(&Library::default_dirs()) else {
        eprintln!("skipped: no FemCraft library");
        return;
    };
    let (template_uid, new_uid) = (883_i64, 99998_i64);
    let _ = lib.delete_item(LibraryKind::Section, new_uid);
    lib.duplicate_concrete_item(template_uid, new_uid).unwrap();
    let v = |k: &str, x: f64| LibraryValue {
        key: k.into(),
        value_si: Some(x),
        value_text: None,
        thickness_si: None,
    };
    lib.set_concrete_values(new_uid, &[v("b", 0.35), v("h", 0.8), v("A", 0.28)])
        .unwrap();
    let conn = Connection::open(lib.db_path(LibraryKind::Section)).unwrap();
    let (ps, sd, topo): (i64, i64, i64) = conn
        .query_row(
            "SELECT ps.id, sd.id, sd.topology_id FROM Section s \
             JOIN SectionImplParametricMassive m ON m.id = s.impl_id \
             JOIN ParametricSection ps ON ps.id = m.librarySection_id \
             JOIN SectionData sd ON sd.id = ps.sectionData_id WHERE s.userID = ?1",
            [new_uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let dims: Vec<f64> = conn
        .prepare("SELECT value FROM ParametricSection_parameterValues WHERE id = ?1 ORDER BY container_order")
        .unwrap()
        .query_map([ps], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect();
    let (xmax, ymax): (f64, f64) = conn
        .query_row(
            "SELECT MAX(location_x), MAX(location_y) FROM SectionTopology_stressPoints WHERE id = ?1",
            [topo],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let b_in: f64 = conn
        .query_row(
            "SELECT valueInputUnit FROM SectionData_parameterValues_values WHERE id = ?1 AND \
             container_order = (SELECT container_order FROM SectionData_parameterValues_keys \
             WHERE id = ?1 AND value = 'b')",
            [sd],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    let _ = lib.delete_item(LibraryKind::Section, new_uid);
    assert_eq!(dims, vec![0.35, 0.8]);
    assert!((xmax - 0.175).abs() < 1e-9 && (ymax - 0.4).abs() < 1e-9, "{xmax} {ymax}");
    assert!((b_in - 350.0).abs() < 1e-9);
}
