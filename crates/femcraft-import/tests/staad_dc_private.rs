//! Parses `testdata/private/STAAD DC MODEL_04.25.2025.std` (Y up, inches / kip, ~11,000
//! shells) and rebuilds its surfaces. Skipped when the file is absent.

use std::collections::BTreeMap;
use std::path::Path;

use femcraft_import::{staad, surfaces};

#[test]
fn dc_model_surfaces() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private/STAAD DC MODEL_04.25.2025.std");
    let Ok(text) = std::fs::read_to_string(&p) else {
        eprintln!("skipped: {} not found", p.display());
        return;
    };
    let t = std::time::Instant::now();
    let m = staad::parse(&text).unwrap();
    println!(
        "parsed in {:?}: z_up {}, {} joints, {} members, {} plates, {} with thickness, \
         {} member props, {} materials {:?}, {} supports, {} load cases",
        t.elapsed(),
        m.z_up,
        m.joints.len(),
        m.members.len(),
        m.plates.len(),
        m.plate_thickness.len(),
        m.member_props.len(),
        m.materials.len(),
        m.materials.iter().map(|x| &x.name).collect::<Vec<_>>(),
        m.supports.len(),
        m.load_cases.len()
    );
    let mut thick: BTreeMap<i64, usize> = BTreeMap::new();
    for t in m.plate_thickness.values() {
        *thick.entry((t * 1000.0).round() as i64).or_default() += 1;
    }
    println!("thicknesses (mm -> plates): {thick:?}");
    println!("skipped: {:?}", m.skipped);

    let plates: Vec<surfaces::Plate> = m
        .plates
        .iter()
        .map(|(&id, n)| surfaces::Plate {
            id,
            nodes: n.clone(),
            key: m
                .plate_thickness
                .get(&id)
                .map_or(0, |t| (t * 1000.0).round() as u32),
        })
        .collect();
    let t = std::time::Instant::now();
    let regions = surfaces::rebuild(&m.joints, &plates, 1e-3);
    println!("rebuilt in {:?}: {} surfaces", t.elapsed(), regions.len());
    let mut by_size: Vec<_> = regions.iter().collect();
    by_size.sort_by_key(|r| std::cmp::Reverse(r.plates.len()));
    let mut holes = 0;
    for r in &by_size {
        holes += r.holes.len();
    }
    println!("openings: {holes}");
    for r in by_size.iter().take(25) {
        let c = surfaces::corners(&r.outer, &m.joints, 1e-3);
        println!(
            "  t {} mm, {} plates, normal [{:.2}, {:.2}, {:.2}], outline {} nodes / {} corners, \
             {} openings (corners {:?})",
            r.key,
            r.plates.len(),
            r.normal[0],
            r.normal[1],
            r.normal[2],
            r.outer.len(),
            c.len(),
            r.holes.len(),
            r.holes
                .iter()
                .map(|h| surfaces::corners(h, &m.joints, 1e-3).len())
                .collect::<Vec<_>>()
        );
    }
    let single = regions.iter().filter(|r| r.plates.len() == 1).count();
    println!("surfaces of a single plate: {single}");
    assert!(!regions.is_empty());
    assert_eq!(
        regions.iter().map(|r| r.plates.len()).sum::<usize>(),
        m.plates.len(),
        "every plate in one surface"
    );
}

/// Full import into a new model (headless MCP workspace on the private template), timed per
/// API method. `FEMCRAFT_KEEP=<path.rf6>` saves the result.
#[test]
#[ignore = "slow: run with --ignored"]
fn dc_model_import() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/private");
    let std = dir.join("STAAD DC MODEL_04.25.2025.std");
    let template = dir.join("FEMCRAFT - RFEM6 Template.rf6");
    if !std.is_file() || !template.is_file() {
        return;
    }
    let text = std::fs::read_to_string(&std).unwrap();
    let mut ws = femcraft_mcp::headless::Headless::new(
        Some(template),
        vec![dir.join("Prokon Model1.rf6"), dir.join("Al Qou_Rev6.rf6")],
    );
    femcraft_mcp::api::call(&mut ws, "new_model", &serde_json::json!({"name": "DC"})).unwrap();
    let mut time: BTreeMap<String, (usize, std::time::Duration)> = BTreeMap::new();
    let t0 = std::time::Instant::now();
    let report = femcraft_import::import_staad(&text, &mut |name, args| {
        let t = std::time::Instant::now();
        let r = femcraft_mcp::api::call(&mut ws, name, args);
        let e = time.entry(name.to_owned()).or_default();
        e.0 += 1;
        e.1 += t.elapsed();
        r
    })
    .unwrap();
    println!("import {:?}: {}", t0.elapsed(), report.summary());
    for (k, (n, d)) in &time {
        println!("  {k}: {n} calls, {d:?}");
    }
    for n in &report.notes {
        println!("  note: {n}");
    }
    for e in report.errors.iter().take(10) {
        println!("  error: {e}");
    }
    if let Some(out) = std::env::var_os("FEMCRAFT_KEEP") {
        let t = std::time::Instant::now();
        femcraft_mcp::api::call(&mut ws, "save_as", &serde_json::json!({"model_path": out}))
            .unwrap();
        println!("saved in {:?}", t.elapsed());
    }
}
