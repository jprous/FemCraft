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
