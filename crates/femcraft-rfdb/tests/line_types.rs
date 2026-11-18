//! Every RFEM line type in `testdata/private/Lines.rf6` (polyline, arc, circle, elliptical arc,
//! ellipse, parabola, spline) loads with its curve data, and each tessellated curve starts and
//! ends at its definition nodes. Skipped when the file is absent, for example on CI.

use std::path::{Path, PathBuf};

use femcraft_model::{ObjectId, ObjectKind as K};
use femcraft_rfdb::RfDocument;

fn model_path() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/private/Lines.rf6");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipped: {} not found", p.display());
        None
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn line_types_load_and_tessellate() {
    let Some(path) = model_path() else { return };
    let doc = RfDocument::open(&path).unwrap();
    let m = &doc.model;
    let line = |no| m.get(ObjectId::new(K::Line, no)).unwrap();
    let ty = |no| line(no).get("type").as_str().unwrap().to_owned();
    let num = |no, k: &str| line(no).get(k).as_f64().unwrap_or(f64::NAN);

    for (no, t) in [
        (1, "TYPE_POLYLINE"),
        (3, "TYPE_ARC"),
        (4, "TYPE_CIRCLE"),
        (5, "TYPE_ELLIPTICAL_ARC"),
        (6, "TYPE_ELLIPSE"),
        (7, "TYPE_PARABOLA"),
        (8, "TYPE_SPLINE"),
    ] {
        assert_eq!(ty(no), t, "line {no}");
    }
    // Control points come from ControlPoint -> ControlPointImpl.
    assert!((num(3, "arc_control_point_x") - 6.0).abs() < 1e-9);
    assert!((num(5, "elliptical_arc_perimeter_control_point_x") + 3.0).abs() < 1e-9);
    assert!((num(7, "parabola_control_point_y") - 4.0).abs() < 1e-9);
    assert!((num(4, "circle_radius") - 2.0).abs() < 1e-9);
    // Type-specific fields stay off other types (alpha is a column of two impl tables).
    assert!(line(7).get("elliptical_arc_alpha").as_f64().is_none());
    assert!(line(1).get("circle_radius").as_f64().is_none());

    for (id, _) in m.iter_kind(K::Line) {
        let pts = m.line_points(id.no);
        let nodes = m.line_nodes(id.no);
        let first = m.node_position(nodes[0]).unwrap();
        let last = m.node_position(*nodes.last().unwrap()).unwrap();
        assert!(pts.len() >= 2, "line {}", id.no);
        assert!(dist(pts[0], first) < 1e-6, "line {} start", id.no);
        if ty(id.no) == "TYPE_ELLIPSE" {
            // Closed: back to the first vertex, through the second one halfway.
            assert!(dist(*pts.last().unwrap(), first) < 1e-6);
            assert!(dist(pts[pts.len() / 2], last) < 1e-6);
            continue;
        }
        assert!(
            dist(*pts.last().unwrap(), last) < 1e-6,
            "line {} end {:?} vs {:?}",
            id.no,
            pts.last(),
            last
        );
    }
    // Curves are tessellated; circles close on their one node.
    for no in [3, 4, 5, 6, 7, 8] {
        assert!(m.line_points(no).len() > 4, "line {no}");
    }
    let c = m.line_points(4);
    assert!(c.iter().all(|p| (dist(*p, [3.0, 4.0, 0.0]) - 2.0).abs() < 1e-6));
}

#[test]
fn openings_load_with_boundary_and_surface() {
    let Some(path) = model_path() else { return };
    let doc = RfDocument::open(&path).unwrap();
    let m = &doc.model;

    // Two openings.
    assert_eq!(m.count(K::Opening), 2);
    let opn1 = m.get(ObjectId::new(K::Opening, 1)).unwrap();
    let opn2 = m.get(ObjectId::new(K::Opening, 2)).unwrap();

    // Opening 1: rectangular (4 polylines, nodes 28-31).
    assert_eq!(opn1.get("boundary_lines").as_ref_list().len(), 4);
    // Opening 2: circular (1 circle line 20).
    assert_eq!(opn2.get("boundary_lines").as_ref_list(), &[20]);

    // Both belong to surface 1.
    assert_eq!(opn1.get("surfaces").as_ref_list(), &[1]);
    assert_eq!(opn2.get("surfaces").as_ref_list(), &[1]);
}
