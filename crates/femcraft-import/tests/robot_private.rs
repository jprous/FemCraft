//! Robot `.str` / `.rtd` against the private files (skipped when absent). The `.str` is the
//! same TSD model as `TSD1 3D.std` (which is unmeshed).

use std::path::Path;

use femcraft_import::{Format, robot, rtd, staad};
use serde_json::json;

fn read(name: &str) -> Option<Vec<u8>> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    std::fs::read(p).ok()
}

fn mock(m: &str, _a: &serde_json::Value) -> Result<serde_json::Value, String> {
    Ok(match m {
        "get_all_object_numbers" => json!([]),
        _ => json!({ "created": true }),
    })
}

#[test]
fn str_matches_std() {
    let (Some(s), Some(d)) = (read("TSD1 3D.str"), read("TSD1 3D.std")) else {
        eprintln!("skipped: TSD1 files absent");
        return;
    };
    let r = robot::parse(&String::from_utf8_lossy(&s)).unwrap();
    let t = staad::parse(&String::from_utf8_lossy(&d)).unwrap();
    println!(
        "str: joints {} members {} plates {} supports {} lcs {} combos {} jl {} el {} ml {} skipped {:?}",
        r.joints.len(), r.members.len(), r.plates.len(), r.supports.len(),
        r.load_cases.len(), r.combination_defs.len(), r.joint_loads.len(),
        r.element_loads.len(), r.member_loads.len(), r.skipped
    );
    println!(
        "std: joints {} members {} plates {} supports {} lcs {} combos {} jl {} el {} ml {} skipped {:?}",
        t.joints.len(), t.members.len(), t.plates.len(), t.supports.len(),
        t.load_cases.len(), t.combination_defs.len(), t.joint_loads.len(),
        t.element_loads.len(), t.member_loads.len(), t.skipped
    );
    assert_eq!(r.joints.len(), 1333);
    assert_eq!(r.members.len(), 382);
    assert_eq!(r.plates.len(), 2478 - 382);
    // The .std is the unmeshed export of the same model: supports and cases agree.
    assert_eq!(r.supports.len(), t.supports.len());
    assert_eq!(r.load_cases.len(), t.load_cases.len());
    let report = femcraft_import::import(Format::Robot, &s, &mut mock).unwrap();
    println!("{}", report.summary());
    for n in report.notes.iter().take(12) {
        println!("  note: {n}");
    }
}

#[test]
fn sludge_tank_rtd() {
    let Some(b) = read("New Sludge Tank Rev 0.rtd") else {
        eprintln!("skipped: rtd absent");
        return;
    };
    let m = rtd::parse(&b).unwrap();
    println!("joints {} plates {} notes {:#?}", m.joints.len(), m.plates.len(), m.notes);
    assert!(m.plates.len() > 70);
    let report = femcraft_import::import(Format::Robot, &b, &mut mock).unwrap();
    println!("{}", report.summary());
}

#[test]
fn punching_str() {
    let Some(b) = read("Punching test model.str") else {
        eprintln!("skipped: punching .str absent");
        return;
    };
    let text = femcraft_import::decode_text(&b);
    let m = robot::parse(&text).unwrap();
    println!(
        "joints {} plates {} supports {} lcs {:?} sw {:?} jl {} el {} combos {:?} mat {:?} skipped {:?}",
        m.joints.len(), m.plates.len(), m.supports.len(), m.load_cases, m.self_weight,
        m.joint_loads.len(), m.element_loads.len(), m.combination_defs, m.materials, m.skipped
    );
    assert_eq!(m.joints.len(), 19363);
    assert_eq!(m.plates.len(), 18932);
    assert_eq!(m.supports.len(), 843);
    assert_eq!(m.supports[&21], [false, false, true, false, false, false]);
    assert_eq!(m.supports[&30], [false, false, false, true, true, false]);
    assert!(m.plate_thickness.values().all(|t| (*t - 0.225).abs() < 1e-12));
    assert_eq!(m.plate_thickness.len(), 18932);
    assert!((m.materials[0].e - 2.25e10).abs() < 1.0 && m.materials[0].name == "CONCRETE");
    assert_eq!(m.load_cases.len(), 3);
    assert_eq!(m.load_cases[&5], "DL5");
    assert_eq!(m.element_loads[0].value, femcraft_import::staad::Pressure::Uniform(-12000.0));
    assert_eq!(m.joint_loads[0].moment, [20000.0, 20000.0, 0.0]);
    assert_eq!(m.self_weight, vec![(5, [0.0, 0.0, -1.0])]);
    let report = femcraft_import::import(Format::Robot, &b, &mut mock).unwrap();
    println!("{}", report.summary());
}

/// The `.rtd` and the `.str` of the same model must agree on what the `.rtd` reader decodes.
#[test]
fn punching_rtd_matches_str() {
    let (Some(r), Some(s)) = (read("Punching test model.rtd"), read("Punching test model.str"))
    else {
        eprintln!("skipped: punching files absent");
        return;
    };
    let a = rtd::parse(&r).unwrap();
    let b = robot::parse(&femcraft_import::decode_text(&s)).unwrap();
    println!("rtd notes: {:#?}", a.notes);
    assert_eq!(a.plates.len(), b.plates.len());
    // Same connectivity, node positions, thickness and supports.
    for (no, nodes) in &b.plates {
        assert_eq!(a.plates.get(no), Some(nodes), "element {no}");
        assert_eq!(a.plate_thickness.get(no), b.plate_thickness.get(no), "thickness {no}");
        for n in nodes {
            let (p, q) = (a.joints[n], b.joints[n]);
            assert!((p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9, "node {n}");
        }
    }
    let sa: std::collections::BTreeSet<_> = a.supports.keys().collect();
    let sb: std::collections::BTreeSet<_> = b.supports.keys().collect();
    assert_eq!(sa, sb);
    assert_eq!(a.materials[0].name, b.materials[0].name);
    assert!((a.materials[0].e - b.materials[0].e).abs() < 1.0);
    assert!((a.materials[0].g - b.materials[0].g).abs() < 1.0);
    assert_eq!(a.load_cases, b.load_cases);
    // Loads: same case, targets, directions and values as in the .str.
    let key = |l: &femcraft_import::staad::ElementLoad| {
        (l.lc, format!("{:?}", l.dir), format!("{:?}", l.value), l.plates.iter().copied().collect::<std::collections::BTreeSet<_>>())
    };
    let ea: Vec<_> = a.element_loads.iter().map(key).collect();
    let eb: Vec<_> = b.element_loads.iter().map(key).collect();
    assert_eq!(ea.len(), 1);
    assert_eq!(ea[0].0, eb[0].0);
    assert_eq!(ea[0].1, eb[0].1);
    assert_eq!(ea[0].2, eb[0].2);
    assert_eq!(ea[0].3, eb[0].3);
    assert_eq!(a.joint_loads.len(), 1);
    assert_eq!(a.joint_loads[0].lc, b.joint_loads[0].lc);
    assert_eq!(a.joint_loads[0].joints, b.joint_loads[0].joints);
    assert_eq!(a.joint_loads[0].force, b.joint_loads[0].force);
    assert_eq!(a.joint_loads[0].moment, b.joint_loads[0].moment);
    assert_eq!(a.self_weight, b.self_weight);
    let report = femcraft_import::import(Format::Robot, &r, &mut mock).unwrap();
    println!("{}", report.summary());
}
