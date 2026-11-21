//! Builds a portal frame and an RC pit through the MCP JSON-RPC interface, saves and reopens
//! the `.rf6`. Uses local models in `testdata/private/` (never committed; skipped when absent).
//! `FEMCRAFT_KEEP_DIR=<dir>` keeps the saved file, e.g. to open it in RFEM.

use std::path::{Path, PathBuf};

use femcraft_mcp::headless::Headless;
use femcraft_model::ObjectKind as K;
use femcraft_rfdb::RfDocument;
use serde_json::{Value as Json, json};

fn private(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    p.is_file().then_some(p).or_else(|| {
        eprintln!("skipped: {name} not found");
        None
    })
}

struct Client {
    ws: Headless,
    id: u64,
}

impl Client {
    fn rpc(&mut self, method: &str, params: Json) -> Json {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let reply = femcraft_mcp::handle_text(&mut self.ws, &msg.to_string()).unwrap();
        let reply: Json = serde_json::from_str(&reply).unwrap();
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        reply["result"].clone()
    }

    /// Calls a tool; panics on a tool error.
    fn call(&mut self, name: &str, args: Json) -> Json {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        assert!(r.get("isError").is_none(), "{name}({args}): {r}");
        r["structuredContent"]["result"].clone()
    }

    fn call_err(&mut self, name: &str, args: Json) -> String {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        assert_eq!(r["isError"], json!(true), "{name} should fail");
        r["content"][0]["text"].as_str().unwrap().to_owned()
    }
}

#[test]
fn protocol_without_model() {
    let mut c = Client {
        ws: Headless::new(None, vec![]),
        id: 0,
    };
    let init = c.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["serverInfo"]["name"], "femcraft");
    let tools = c.rpc("tools/list", json!({}));
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "new_model",
        "open_model",
        "save",
        "save_as",
        "set_node",
        "set_surface_load",
        "get_all_object_numbers",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    assert!(
        c.call_err("set_node", json!({"value": {"no": 1}}))
            .contains("no model")
    );
    assert!(c.call_err("new_model", json!({})).contains("blank model"));
    // Notifications get no reply.
    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    assert!(femcraft_mcp::handle_text(&mut c.ws, &note.to_string()).is_none());
}

#[test]
fn portal_frame_and_pit() {
    // File > New uses the template as it is: an RFEM model without geometry, with materials,
    // section 1, thickness 1, load cases and unassigned supports (e.g. no. 2 "Rigid").
    let (Some(template), Some(prokon), Some(portal), Some(pit), Some(reservoir)) = (
        private("FEMCRAFT - RFEM6 Template.rf6"),
        private("Prokon Model1.rf6"),
        private("FemCraft portal frame.rf6"),
        private("FemCraft pit 3 loads.rf6"),
        private("Al Qou_Rev6.rf6"),
    ) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client {
        ws: Headless::new(Some(template), vec![prokon, portal, pit, reservoir]),
        id: 0,
    };
    c.rpc("initialize", json!({}));
    assert_eq!(c.call("new_model", json!({}))["name"], "Model 1");

    // Bad input is refused with the valid fields listed.
    assert!(
        c.call_err("set_node", json!({"value": {"x": 1.0}}))
            .contains("coordinate_1")
    );

    // Portal frame in the XZ plane (RFEM's Z points down): columns 4 m, beam 6 m.
    for (no, x, z) in [(1, 0.0, 0.0), (2, 0.0, -4.0), (3, 6.0, -4.0), (4, 6.0, 0.0)] {
        c.call(
            "set_node",
            json!({"value": {"no": no, "coordinate_1": x, "coordinate_2": 0.0, "coordinate_3": z}}),
        );
    }
    for (no, nodes) in [(1, [1, 2]), (2, [2, 3]), (3, [3, 4])] {
        c.call(
            "set_line",
            json!({"value": {"no": no, "definition_nodes": nodes}}),
        );
        c.call(
            "set_member",
            json!({"value": {"no": no, "line": no, "section_start": 1}}),
        );
    }
    // Supports: assign objects to the template's supports, as in RFEM.
    let sup = c.call(
        "set_nodal_support",
        json!({"value": {"no": 2, "nodes": "1,4"}}),
    );
    assert_eq!(sup["created"], json!(false));

    // RC pit 3 x 2 m, 1.5 m deep, beside the frame: base slab + 4 walls on thickness 1.
    let (x0, y0, d) = (10.0, 0.0, 1.5);
    let corners = [(0.0, 0.0), (3.0, 0.0), (3.0, 2.0), (0.0, 2.0)];
    for (i, (x, y)) in corners.iter().enumerate() {
        for (level, z) in [(0, 0.0), (1, -d)] {
            c.call(
                "set_node",
                json!({"value": {"no": 11 + i + 4 * level, "coordinate_1": x0 + x,
                                 "coordinate_2": y0 + y, "coordinate_3": z}}),
            );
        }
    }
    // Base edges 11-14, top edges 15-18, verticals 19-22.
    for i in 0..4u32 {
        let j = (i + 1) % 4;
        c.call(
            "set_line",
            json!({"value": {"no": 11 + i, "definition_nodes": [11 + i, 11 + j]}}),
        );
        c.call(
            "set_line",
            json!({"value": {"no": 15 + i, "definition_nodes": [15 + i, 15 + j]}}),
        );
        c.call(
            "set_line",
            json!({"value": {"no": 19 + i, "definition_nodes": [11 + i, 15 + i]}}),
        );
    }
    c.call(
        "set_surface",
        json!({"value": {"no": 1, "boundary_lines": "11-14", "thickness": 1}}),
    );
    for i in 0..4u32 {
        let j = (i + 1) % 4;
        c.call(
            "set_surface",
            json!({"value": {"no": 2 + i,
                "boundary_lines": [11 + i, 19 + j, 15 + i, 19 + i], "thickness": 1}}),
        );
    }
    c.call(
        "set_surface_support",
        json!({"value": {"no": 1, "surfaces": [1]}}),
    );
    c.call(
        "set_line_support",
        json!({"value": {"no": 1, "lines": [15]}}),
    );

    // Loads go to existing load cases.
    assert!(
        c.call_err(
            "set_nodal_load",
            json!({"load_case_no": 9, "value": {"nodes": [2]}})
        )
        .contains("load case 9")
    );
    c.call(
        "set_load_case",
        json!({"value": {"no": 2, "name": "Imposed"}}),
    );
    c.call(
        "set_nodal_load",
        json!({"load_case_no": 2, "value": {"nodes": [2], "magnitude": 10_000.0}}),
    );
    c.call(
        "set_line_load",
        json!({"load_case_no": 2, "value": {"lines": [2], "magnitude": 5_000.0}}),
    );
    c.call(
        "set_surface_load",
        json!({"load_case_no": 2, "value": {"surfaces": [1], "uniform_magnitude": 2_000.0}}),
    );
    assert_eq!(
        c.call(
            "get_all_object_numbers",
            json!({"type": "E_OBJECT_TYPE_SURFACE"})
        ),
        json!([1, 2, 3, 4, 5])
    );
    let n = c.call("get_node", json!({"no": 3}));
    assert_eq!(
        (n["coordinate_1"].clone(), n["coordinate_3"].clone()),
        (json!(6.0), json!(-4.0))
    );

    assert!(c.call_err("save", json!({})).contains("untitled"));
    let out = dir.path().join("MCP portal and pit.rf6");
    c.call("save_as", json!({"model_path": out.to_str().unwrap()}));
    if let Ok(keep) = std::env::var("FEMCRAFT_KEEP_DIR") {
        std::fs::copy(&out, Path::new(&keep).join(out.file_name().unwrap())).unwrap();
    }
    let list = c.call("get_model_list", json!({}));
    assert_eq!(list[0]["modified"], json!(false));

    let back = RfDocument::open(&out).unwrap();
    for (kind, n) in [
        (K::Node, 12),
        (K::Line, 15),
        (K::Member, 3),
        (K::Surface, 5),
        (K::NodalLoad, 1),
        (K::LineLoad, 1),
        (K::SurfaceLoad, 1),
    ] {
        assert_eq!(back.model.count(kind), n, "{kind:?}");
    }
    assert_eq!(back.model.node_position(17), Some([13.0, 2.0, -1.5]));
    let ls = back
        .model
        .get(femcraft_model::ObjectId::new(K::LineSupport, 1))
        .unwrap();
    assert_eq!(ls.get("lines").as_ref_list(), &[15]);
}

/// API enum names RFEM stores as integer codes: known ones are converted, unknown ones are
/// refused, and saving never writes text into an integer column (RFEM calls such a file
/// corrupted).
#[test]
fn enum_names_become_codes() {
    let Some(template) = private("FEMCRAFT - RFEM6 Template.rf6") else {
        return;
    };
    let mut c = Client {
        ws: Headless::new(Some(template.clone()), vec![]),
        id: 0,
    };
    c.call("new_model", json!({}));
    c.call(
        "set_load_case",
        json!({"value": {"no": 2, "action_category": "ACTION_CATEGORY_PERMANENT_G"}}),
    );
    assert_eq!(
        c.call("get_load_case", json!({"no": 2}))["action_category"],
        json!(11)
    );
    let e = c.call_err(
        "set_load_case",
        json!({"value": {"no": 3, "action_category": "ACTION_CATEGORY_IMPOSED_LOADS_CATEGORY_H_ROOFS_QI_H"}}),
    );
    assert!(e.contains("integer code"), "{e}");

    // Bypassing the MCP: the save refuses the text and leaves the file unwritten.
    let dir = tempfile::tempdir().unwrap();
    let mut doc = RfDocument::open(&template).unwrap();
    let lc = femcraft_model::ObjectId::new(K::LoadCase, 2);
    doc.model
        .set(
            lc,
            "action_category",
            femcraft_model::Value::Text("ACTION_CATEGORY_PERMANENT_G".into()),
        )
        .unwrap();
    let out = dir.path().join("bad.rf6");
    let err = doc.save(&out).unwrap_err().to_string();
    assert!(err.contains("stores a number"), "{err}");
    // A number given as text is stored as a number.
    doc.model
        .set(lc, "action_category", femcraft_model::Value::Text("11".into()))
        .unwrap();
    doc.save(&out).unwrap();
}
