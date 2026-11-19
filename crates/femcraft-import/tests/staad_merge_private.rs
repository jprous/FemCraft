//! Imports the private STAAD files through a recording mock (no RFEM model) to see how many
//! surface supports / loads and nodal objects result. Skipped when the files are absent.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

fn run(name: &str) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    let Ok(text) = std::fs::read_to_string(&p) else {
        eprintln!("skipped: {name} not found");
        return;
    };
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    let report = femcraft_import::import_staad(&text, &mut |m, args| {
        *calls.entry(m.to_owned()).or_default() += 1;
        if m == "set_surface_support" || m == "set_surface_load" {
            println!("{m} {args}");
        }
        Ok(match m {
            "get_all_object_numbers" => json!([]),
            _ => json!({ "created": true }),
        })
    })
    .unwrap();
    println!("== {name}\n{}", report.summary());
    for n in &report.notes {
        println!("  note: {n}");
    }
    println!("{calls:?}");
}

#[test]
fn merge_tsd() {
    run("TSD1 3D.std");
}

#[test]
fn merge_dc() {
    run("STAAD DC MODEL_04.25.2025.std");
}

#[test]
fn merge_tsd2() {
    run("TSD2 3D.std");
}

#[test]
fn a03_wall_with_shells() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private/PKW Wall_with shells.a03");
    let Ok(bytes) = std::fs::read(&p) else {
        eprintln!("skipped: A03 file not found");
        return;
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let m = femcraft_import::a03::parse(&text).unwrap();
    println!(
        "nodes {} shells {} supports {} loads {} lcs {:?} materials {}",
        m.joints.len(),
        m.plates.len(),
        m.supports.len(),
        m.element_loads.len(),
        m.load_cases,
        m.materials.len()
    );
    assert_eq!(m.plates.len(), 168);
    let report = femcraft_import::import_a03(&text, &mut |n, a| {
        if n.starts_with("set_surface") || n == "set_load_case" {
            println!("{n} {a}");
        }
        Ok(match n {
            "get_all_object_numbers" => json!([]),
            _ => json!({ "created": true }),
        })
    })
    .unwrap();
    println!("{}", report.summary());
    for n in &report.notes {
        println!("  note: {n}");
    }
}

/// The braced steel portal frame from the A03RF6STEEL skill (`C:\Temp\Model1.A03`).
#[test]
fn a03_frame_model1() {
    let Ok(bytes) = std::fs::read("C:/Temp/Model1.A03") else {
        eprintln!("skipped: C:/Temp/Model1.A03 not found");
        return;
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let m = femcraft_import::a03::parse(&text).unwrap();
    println!(
        "nodes {} members {} supports {} member loads {} combos {} lcs {:?} skipped {:?}",
        m.joints.len(),
        m.members.len(),
        m.supports.len(),
        m.member_loads.len(),
        m.combinations,
        m.load_cases,
        m.skipped
    );
    // The skill's reference counts: 407 nodes, 610 members, 14 supports, 653 beam loads.
    assert_eq!(m.members.len(), 610);
    assert_eq!(m.member_loads.len(), 653);
    // Applied totals sum |w| * L per load case (kN), from the skill's reference table.
    for (name, kn) in [("DEAD_L", 70.95), ("LIVE_L", 167.70), ("WIND_0", 410.25)] {
        let lc = *m
            .load_cases
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .unwrap()
            .0;
        let total: f64 = m
            .member_loads
            .iter()
            .filter(|l| l.lc == lc)
            .map(|l| {
                let (a, b) = m.members[&l.member];
                let (p, q) = (m.joints[&a], m.joints[&b]);
                let len =
                    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                l.w.abs() * len / 1e3
            })
            .sum();
        assert!((total - kn).abs() < 0.05 * kn, "{name}: {total} vs {kn}");
    }
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    let report = femcraft_import::import_a03(&text, &mut |n, _| {
        *calls.entry(n.to_owned()).or_default() += 1;
        Ok(match n {
            "get_all_object_numbers" => json!([]),
            _ => json!({ "created": true }),
        })
    })
    .unwrap();
    println!("{}\n{calls:?}", report.summary());
    for n in report.notes.iter().filter(|n| n.contains("ombination")) {
        println!("  note: {n}");
    }
}
