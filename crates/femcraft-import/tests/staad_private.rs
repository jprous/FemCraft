//! Imports `testdata/private/TSD1 3D.std` into a new model (headless MCP workspace on the
//! private template), saves it as `.rf6` and reopens it. Skipped when the files are absent.
//! `FEMCRAFT_KEEP=<path.rf6>` keeps the saved model (e.g. to open it in RFEM).

use std::path::{Path, PathBuf};

use femcraft_mcp::headless::Headless;
use femcraft_model::ObjectKind as K;
use femcraft_rfdb::RfDocument;
use serde_json::json;

fn private(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    p.is_file().then_some(p).or_else(|| {
        eprintln!("skipped: {name} not found");
        None
    })
}

#[test]
fn tsd_export() {
    let (Some(std), Some(template), Some(reservoir), Some(prokon)) = (
        private("TSD1 3D.std"),
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Al Qou_Rev6.rf6"),
        private("Prokon Model1.rf6"),
    ) else {
        return;
    };
    let text = std::fs::read_to_string(&std).unwrap();
    let mut ws = Headless::new(Some(template), vec![prokon, reservoir]);
    femcraft_mcp::api::call(&mut ws, "new_model", &json!({"name": "TSD1 3D"})).unwrap();
    let t0 = std::time::Instant::now();
    let report = femcraft_import::import_staad(&text, &mut |name, args| {
        femcraft_mcp::api::call(&mut ws, name, args)
    })
    .unwrap();
    println!("{} ({:?})", report.summary(), t0.elapsed());
    for n in &report.notes {
        println!("  note: {n}");
    }
    for e in &report.errors {
        println!("  error: {e}");
    }
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let dir = tempfile::tempdir().unwrap();
    let out = std::env::var_os("FEMCRAFT_KEEP")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.path().join("TSD1 3D.rf6"));
    femcraft_mcp::api::call(&mut ws, "save_as", &json!({"model_path": out})).unwrap();
    println!("saved after {:?}", t0.elapsed());
    let doc = RfDocument::open(&out).unwrap();
    let count = |k| doc.model.count(k);
    println!(
        "reopened: {} nodes, {} lines, {} members, {} surfaces, {} openings",
        count(K::Node),
        count(K::Line),
        count(K::Member),
        count(K::Surface),
        count(K::Opening)
    );
    assert_eq!(count(K::Member), 163);
    assert!(count(K::Surface) > 0);
}

/// The 120 wall plates (two walls 7.5 x 7 m, distorted mesh) become two rectangles.
#[test]
fn tsd_wall_outlines() {
    let Some(std) = private("TSD1 3D.std") else {
        return;
    };
    let m = femcraft_import::staad::parse(&std::fs::read_to_string(std).unwrap()).unwrap();
    let plates: Vec<_> = m
        .plates
        .iter()
        .map(|(&id, n)| femcraft_import::surfaces::Plate {
            id,
            nodes: n.clone(),
            key: 1,
        })
        .collect();
    let regions = femcraft_import::surfaces::rebuild(&m.joints, &plates, 1e-3);
    assert_eq!(regions.len(), 2);
    for r in &regions {
        assert!(r.holes.is_empty());
        let c = femcraft_import::surfaces::corners(&r.outer, &m.joints, 1e-3);
        assert_eq!(c.len(), 4, "{:?}", c);
    }
    assert_eq!(regions.iter().map(|r| r.plates.len()).sum::<usize>(), 120);
}

/// A 2 x 2 plate floor: springs on every node, a uniform plate pressure, and joint loads whose
/// pressure is equal -> one surface support and surface loads, no nodal objects left.
#[test]
fn merged_support_and_loads() {
    let (Some(template), Some(reservoir), Some(prokon)) = (
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Al Qou_Rev6.rf6"),
        private("Prokon Model1.rf6"),
    ) else {
        return;
    };
    let mut text = String::from("STAAD SPACE\nSET Z UP\nUNIT METER NEWTON\nJOINT COORDINATES\n");
    for j in 0..3 {
        for i in 0..3 {
            text += &format!("{} {} {} 0\n", j * 3 + i + 1, i * 2, j * 2);
        }
    }
    text += "ELEMENT INCIDENCES\n1 1 2 5 4\n2 2 3 6 5\n3 4 5 8 7\n4 5 6 9 8\n\
             ELEMENT PROPERTY\n1 TO 4 THI 0.2\nSUPPORTS\n1 TO 9 FIXED BUT FX FY MX MY MZ KFZ 4000\n\
             LOADING 1 TITLE 1 Dead\nELEMENT LOAD\n1 TO 4 PR GZ -1000\n\
             LOADING 2 TITLE 2 Joint\nJOINT LOAD\n1 3 7 9 FZ -1000\n2 4 6 8 FZ -2000\n5 FZ -4000\nFINISH\n";
    let mut ws = Headless::new(Some(template), vec![prokon, reservoir]);
    femcraft_mcp::api::call(&mut ws, "new_model", &json!({"name": "merge"})).unwrap();
    let report = femcraft_import::import_staad(&text, &mut |name, args| {
        femcraft_mcp::api::call(&mut ws, name, args)
    })
    .unwrap();
    for n in &report.notes {
        println!("  note: {n}");
    }
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let out = std::env::var_os("FEMCRAFT_KEEP2")
        .map(PathBuf::from)
        .unwrap_or_else(|| tempfile::tempdir().unwrap().keep().join("merge.rf6"));
    femcraft_mcp::api::call(&mut ws, "save_as", &json!({"model_path": out})).unwrap();
    let doc = RfDocument::open(&out).unwrap();
    println!(
        "surfaces {} supports {} surface loads {} nodal loads {}",
        doc.model.count(K::Surface),
        doc.model.count(K::SurfaceSupport),
        doc.model.count(K::SurfaceLoad),
        doc.model.count(K::NodalLoad)
    );
    assert_eq!(doc.model.count(K::Surface), 1);
    assert_eq!(report.created.get("surface_support"), Some(&1));
    assert_eq!(report.created.get("surface_load"), Some(&2));
    assert_eq!(report.created.get("nodal_load"), None);
}

/// Prokon `.A03` wall with shells through the real template: surfaces, a surface support or
/// nodal ones, surface loads, self weight on the load case. `FEMCRAFT_KEEP3=<path.rf6>` keeps it.
#[test]
fn a03_wall_end_to_end() {
    let (Some(a03), Some(template), Some(reservoir), Some(prokon)) = (
        private("PKW Wall_with shells.a03"),
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Al Qou_Rev6.rf6"),
        private("Prokon Model1.rf6"),
    ) else {
        return;
    };
    let text = String::from_utf8_lossy(&std::fs::read(a03).unwrap()).into_owned();
    let mut ws = Headless::new(Some(template), vec![prokon, reservoir]);
    femcraft_mcp::api::call(&mut ws, "new_model", &json!({"name": "wall"})).unwrap();
    let report = femcraft_import::import_a03(&text, &mut |name, args| {
        femcraft_mcp::api::call(&mut ws, name, args)
    })
    .unwrap();
    println!("{}", report.summary());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let out = std::env::var_os("FEMCRAFT_KEEP3")
        .map(PathBuf::from)
        .unwrap_or_else(|| tempfile::tempdir().unwrap().keep().join("wall.rf6"));
    femcraft_mcp::api::call(&mut ws, "save_as", &json!({"model_path": out})).unwrap();
    let doc = RfDocument::open(&out).unwrap();
    println!(
        "surfaces {} surface loads {} nodal supports {} load cases {}",
        doc.model.count(K::Surface),
        doc.model.count(K::SurfaceLoad),
        doc.model.count(K::NodalSupport),
        doc.model.count(K::LoadCase)
    );
    assert_eq!(doc.model.count(K::Surface), 24);
    assert!(doc.model.count(K::LoadCase) >= 2);
}

/// The skill's braced portal frame (`C:\Temp\Model1.A03`) through the real template: member
/// loads are written and saved.
#[test]
fn a03_frame_end_to_end() {
    let Ok(bytes) = std::fs::read("C:/Temp/Model1.A03") else {
        return;
    };
    let (Some(template), Some(reservoir), Some(prokon)) = (
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Al Qou_Rev6.rf6"),
        private("Prokon Model1.rf6"),
    ) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut ws = Headless::new(Some(template), vec![prokon, reservoir]);
    femcraft_mcp::api::call(&mut ws, "new_model", &json!({"name": "frame"})).unwrap();
    let report = femcraft_import::import_a03(&text, &mut |name, args| {
        femcraft_mcp::api::call(&mut ws, name, args)
    })
    .unwrap();
    println!("{}", report.summary());
    for e in &report.errors {
        println!("  error: {e}");
    }
    let out = tempfile::tempdir().unwrap().keep().join("frame.rf6");
    femcraft_mcp::api::call(&mut ws, "save_as", &json!({"model_path": out})).unwrap();
    let doc = RfDocument::open(&out).unwrap();
    println!(
        "members {} member loads {}",
        doc.model.count(K::Member),
        doc.model.count(K::MemberLoad)
    );
    assert_eq!(doc.model.count(K::Member), 610);
    assert!(doc.model.count(K::MemberLoad) >= 17);
}
