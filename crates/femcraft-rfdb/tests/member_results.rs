//! Member (1D element) displacements from `!solver/RO_*/Data.RO*`. Needs a model RFEM calculated
//! with members only, named by `FEMCRAFT_TEST_MEMBER_RESULTS` (e.g. the 5-span beam from
//! FemCraft's Solve); skipped otherwise.

use femcraft_rfdb::{CaseKind, RfDocument};

#[test]
fn member_displacements_are_read() {
    let Some(path) = std::env::var_os("FEMCRAFT_TEST_MEMBER_RESULTS") else {
        eprintln!("skipped: FEMCRAFT_TEST_MEMBER_RESULTS not set");
        return;
    };
    let doc = RfDocument::open(std::path::Path::new(&path)).unwrap();
    let results = doc.results().unwrap().expect("RO_ folders count as results");
    assert!(!results.is_empty(), "no result cases found");
    let mesh = doc.fe_mesh().unwrap().expect("FE mesh");
    let case = results
        .cases
        .iter()
        .find(|c| c.kind == CaseKind::LoadCase && c.no == 1)
        .expect("LC1");
    let d = doc
        .displacements(case.solver_id, mesh.nodes.len())
        .unwrap()
        .expect("member displacements");
    assert_eq!(d.nodal.len(), mesh.nodes.len());
    // Fixed at node 1, rollers at 2..6 (FE nodes 1..6 are the model nodes).
    for n in 1..=6u32 {
        assert!(d.node(n).unwrap()[2].abs() < 1e-9, "node {n} should not move down");
    }
    // Span 1 is loaded (downwards = +z): the peak deflection is a fraction of a millimetre.
    let peak = d.nodal.iter().map(|u| u[2]).fold(0.0, f64::max);
    assert!(peak > 1e-5 && peak < 1e-3, "peak uz {peak}");
}

/// Saving with the `!solver/` prefix dropped (Solve > Delete results) gives an unsolved model.
#[test]
fn dropping_solver_data_removes_results() {
    let Some(path) = std::env::var_os("FEMCRAFT_TEST_MEMBER_RESULTS") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("femcraft-del-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("unsolved.rf6");
    let mut doc = RfDocument::open(std::path::Path::new(&path)).unwrap();
    doc.archive.drop_prefixes.push("!solver/".into());
    doc.save(&out).unwrap();
    let back = RfDocument::open(&out).unwrap();
    assert!(back.results().unwrap().is_none(), "results survived");
    assert!(back.fe_mesh().unwrap().is_none(), "mesh survived");
    let _ = std::fs::remove_dir_all(&dir);
}
