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
