//! Surface internal forces rebuilt from a solved model's strain block
//! (`testdata/private/Chamber 4x4x6.rf6`, a 4×4×6 m chamber, LC1). Skipped when the file is
//! absent, for example on CI. Values checked against RFEM's own xlsx result export: the base
//! slab's peak |m_x| ≈ 34 kNm/m, the walls' ≈ 13 kNm/m.

use std::path::{Path, PathBuf};

use femcraft_rfdb::{surface_force_scalar, RfDocument, ResultComponent};

fn model_path() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private/Chamber 4x4x6.rf6");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipped: {} not found", p.display());
        None
    }
}

#[test]
fn rebuilds_surface_moments_from_the_block() {
    let Some(path) = model_path() else { return };
    let doc = RfDocument::open(&path).unwrap();
    let mesh = doc.fe_mesh().unwrap().expect("model has a mesh");

    // Per-surface plate stiffness is built from the model (C30/37, 0.4 m).
    let stiff = doc.surface_stiffness();
    assert!(!stiff.is_empty(), "surfaces have stiffness");

    // LC1 is the first calculated case; find its solver id.
    let results = doc.results().unwrap().expect("model is solved");
    let lc1 = results
        .cases
        .iter()
        .find(|c| c.kind == femcraft_rfdb::CaseKind::LoadCase && c.no == 1)
        .expect("LC1 has results");
    let id = *lc1.result_ids().first().unwrap();

    let forces = doc
        .surface_forces(id, &mesh)
        .unwrap()
        .expect("LC1 has a surface strain block");

    // Collect m_x over all nodes that carry a value (SI N·m/m).
    let mx: Vec<f64> = (1..=mesh.nodes.len() as u32)
        .filter_map(|n| forces.node(n))
        .filter_map(|f| surface_force_scalar(ResultComponent::SurfaceMx, &f))
        .collect();
    assert!(!mx.is_empty(), "some nodes have surface moments");

    // Peak |m_x| over the whole model is the base slab's ~34 kNm/m (34000 N·m/m); well within a
    // tolerance band it should land between 25 and 45 kNm/m, and never be absurd.
    let peak = mx.iter().fold(0.0f64, |a, &v| a.max(v.abs()));
    assert!(
        (25_000.0..45_000.0).contains(&peak),
        "peak |m_x| {peak} N·m/m out of the expected ~34 kNm/m range"
    );

    // A sanity check that the values are signed (hogging and sagging both occur in a chamber).
    assert!(mx.iter().any(|&v| v > 1_000.0), "some sagging moment");
    assert!(mx.iter().any(|&v| v < -1_000.0), "some hogging moment");
}
