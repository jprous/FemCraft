//! Result interpreter checks against a real solved RFEM model in `testdata/private/`
//! (never committed). Skipped when the file is absent, for example on CI.
//!
//! Set `FEMCRAFT_TEST_MODEL` to a solved `.rf6`; defaults to `testdata/private/test.rf6`.

use std::path::{Path, PathBuf};

use femcraft_rfdb::{CaseKind, ResultNode, RfDocument};

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

#[test]
fn reads_result_cases_and_tree() {
    let Some(src) = model_path() else { return };
    let doc = RfDocument::open(&src).unwrap();
    let Some(results) = doc.results().unwrap() else {
        eprintln!("skipped: {} holds no solver results", src.display());
        return;
    };

    assert!(!results.is_empty(), "a solved model lists result cases");
    assert!(
        !results.solver_folders.is_empty(),
        "result folders are recorded for later decoding"
    );

    // Cases are grouped by kind then ordered by number, and every case gets a tag/label.
    let mut last: Option<(CaseKind, u32)> = None;
    for case in &results.cases {
        assert!(case.no > 0, "case has a number");
        assert!(!case.label().is_empty());
        if let Some(prev) = last {
            assert!((prev.0, prev.1) <= (case.kind, case.no), "cases sorted");
        }
        last = Some((case.kind, case.no));
    }

    // The result tree reflects the model: a model with surfaces offers surface internal forces.
    let tree = femcraft_rfdb::results::result_tree(&doc.model);
    assert!(!tree.is_empty(), "a model offers at least deformations");
    let mut leaves = 0usize;
    count_leaves(&tree, &mut leaves);
    assert!(leaves > 0, "the tree has selectable quantities");

    eprintln!(
        "results: {} cases ({} folders), version {:?}",
        results.cases.len(),
        results.solver_folders.len(),
        results.results_version
    );
    for kind in CaseKind::ALL {
        let n = results.cases_of(kind).count();
        if n > 0 {
            eprintln!("  {}: {n}", kind.group_label());
        }
    }
    let top: Vec<&str> = tree
        .iter()
        .filter_map(|n| match n {
            ResultNode::Category { label, .. } => Some(*label),
            ResultNode::Leaf(_) => None,
        })
        .collect();
    eprintln!("top-level categories: {top:?} ({leaves} leaves total)");

    // Decode nodal displacements for the first calculated case.
    let fe_nodes = doc.fe_mesh().unwrap().map_or(0, |m| m.nodes.len());
    let first = &results.cases[0];
    let disp = doc
        .displacements(first.solver_id, fe_nodes)
        .unwrap()
        .expect("first case has a displacement file");
    assert_eq!(disp.nodal.len(), fe_nodes.max(disp.nodal.len()));
    let max_u = disp.max_translation();
    assert!(
        max_u > 0.0 && max_u < 1.0,
        "max translation {max_u} m is implausible"
    );
    eprintln!(
        "{} displacements: {} nodes, max |u| = {:.2} mm",
        first.tag(),
        disp.nodal.len(),
        max_u * 1000.0
    );
}

/// Counts the radio leaves anywhere in the tree.
fn count_leaves(nodes: &[ResultNode], n: &mut usize) {
    for node in nodes {
        match node {
            ResultNode::Leaf(_) => *n += 1,
            ResultNode::Category { children, .. } => count_leaves(children, n),
        }
    }
}

/// Design situations envelope their load combinations: members are resolved, and every member
/// has a displacement file.
#[test]
fn design_situations_list_member_combinations() {
    let Some(src) = model_path() else { return };
    let doc = RfDocument::open(&src).unwrap();
    let Some(results) = doc.results().unwrap() else {
        return;
    };
    for c in results.cases_of(CaseKind::DesignSituation) {
        assert!(c.has_extremes());
        assert!(!c.members.is_empty(), "{} has no member combinations", c.tag());
        let fe_nodes = doc.fe_mesh().unwrap().map_or(0, |m| m.nodes.len());
        for &id in &c.result_ids() {
            assert!(doc.displacements(id, fe_nodes).unwrap().is_some(), "{} id {id}", c.tag());
        }
    }
}

#[test]
fn extremum_reduces_values() {
    use femcraft_rfdb::Extremum;
    let v = [3.0, -5.0, 1.0];
    assert_eq!(Extremum::Max.reduce(v.into_iter()), Some(3.0));
    assert_eq!(Extremum::Min.reduce(v.into_iter()), Some(-5.0));
    assert_eq!(Extremum::Both.reduce(v.into_iter()), Some(-5.0));
    assert_eq!(Extremum::Max.reduce(std::iter::empty()), None);
}
