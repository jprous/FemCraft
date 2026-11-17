//! `set_section` / `set_material` with an RFEM library name, through the MCP JSON-RPC
//! interface. Needs `testdata/private/Prokon Model1 - FemCraft node2.rf6` and FemCraft's
//! library (materials.db, sections.db); skipped when either is missing.

use std::path::Path;

use femcraft_mcp::headless::Headless;
use femcraft_rfdb::{Library, RfDocument};
use serde_json::{Value as Json, json};

fn call(ws: &mut Headless, name: &str, args: Json) -> Result<Json, String> {
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": name, "arguments": args}});
    let reply: Json =
        serde_json::from_str(&femcraft_mcp::handle_text(ws, &msg.to_string()).unwrap()).unwrap();
    let r = &reply["result"];
    if r.get("isError") == Some(&json!(true)) {
        Err(r["content"][0]["text"].as_str().unwrap().to_owned())
    } else {
        Ok(r["structuredContent"]["result"].clone())
    }
}

#[test]
fn library_names_in_set_methods() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private/Prokon Model1 - FemCraft node2.rf6");
    if !src.is_file() || Library::find(&Library::default_dirs()).is_none() {
        eprintln!("skipped: model or library missing");
        return;
    }
    let mut ws = Headless::new(None, vec![]);
    call(&mut ws, "open_model", json!({"model_path": src})).unwrap();

    // An existing section and material.
    let ipe = "IPE AA 160 | DIN 1025:1965 | Macsteel";
    let r = call(
        &mut ws,
        "set_section",
        json!({"value": {"no": 1, "name": ipe}}),
    )
    .unwrap();
    assert_eq!(r["library_item"], ipe);
    let r = call(
        &mut ws,
        "set_material",
        json!({"value": {"no": 1, "name": "S275JR | SANS 50025-2"}}),
    )
    .unwrap();
    let mat_id = r["rfem_library_id"].clone();
    // A new section by designation, with a material.
    let r = call(
        &mut ws,
        "set_section",
        json!({"value": {"no": 20, "name": "UB 203x133x25", "material": 1}}),
    )
    .unwrap();
    assert_eq!(r["created"], true);
    assert_eq!(r["library_item"], "UB 203x133x25 | BS 4-1:1993 | Macsteel");

    let s20 = call(&mut ws, "get_section", json!({"no": 20})).unwrap();
    assert_eq!(s20["material"], 1);
    let m1 = call(&mut ws, "get_material", json!({"no": 1})).unwrap();
    assert_eq!(m1["database_material_id"], mat_id);
    assert!((m1["elasticity_modulus"].as_f64().unwrap() - 200e9).abs() < 1.0);

    // Unknown names and ambiguous designations are refused, nothing created.
    let e = call(
        &mut ws,
        "set_section",
        json!({"value": {"no": 21, "name": "IPE 999"}}),
    )
    .unwrap_err();
    assert!(e.contains("no library section"), "{e}");
    assert!(call(&mut ws, "get_section", json!({"no": 21})).is_err());

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("library names.rf6");
    call(&mut ws, "save_as", json!({"model_path": out})).unwrap();
    let back = RfDocument::open(&out).unwrap();
    let a20 = back
        .model
        .get(femcraft_model::ObjectId::new(
            femcraft_model::ObjectKind::Section,
            20,
        ))
        .unwrap()
        .get("A")
        .as_f64()
        .unwrap();
    let lib = Library::find(&Library::default_dirs()).unwrap();
    let ub = lib
        .get(femcraft_rfdb::LibraryKind::Section, "UB 203x133x25")
        .unwrap()
        .unwrap();
    let a = lib
        .values(&ub)
        .unwrap()
        .into_iter()
        .find(|v| v.key == "A")
        .and_then(|v| v.value_si)
        .unwrap();
    assert!((a20 - a).abs() < 1e-12, "library area {a}, got {a20}");
}
