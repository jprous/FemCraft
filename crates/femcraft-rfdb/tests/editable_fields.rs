//! Every field the UI tables let users edit must save and read back. Runs against local models
//! in `testdata/private/` (never committed); each model is skipped when absent.

use std::path::{Path, PathBuf};

use femcraft_model::{FieldType, ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;

/// Mirrors `femcraft_ui::panels::is_editable` (kept separate: rfdb does not depend on the UI).
fn editable(kind: K) -> Vec<&'static str> {
    let all = |k: K| k.spec().fields.iter().map(|f| f.key).collect::<Vec<_>>();
    let mut keys = match kind {
        K::Thickness => vec!["material", "uniform_thickness"],
        K::Node => vec!["coordinate_1", "coordinate_2", "coordinate_3"],
        K::Line => vec!["definition_nodes"],
        K::Member => vec!["line", "section_start", "section_end", "rotation_angle"],
        K::Surface => vec![
            "boundary_lines",
            "thickness",
            "axes_rotation",
            "axes_z_reversed",
        ],
        K::NodalSupport | K::LineSupport | K::MemberSupport | K::SurfaceSupport => all(kind),
        K::LoadCase => vec!["name", "self_weight_active", "to_solve"],
        K::NodalLoad | K::LineLoad | K::MemberLoad | K::SurfaceLoad => vec![
            "nodes",
            "lines",
            "members",
            "surfaces",
            "magnitude",
            "magnitude_2",
            "uniform_magnitude",
        ],
        _ => vec![],
    };
    // Every table: comments (mostly empty in the file).
    if !keys.contains(&"comment") && kind.spec().field("comment").is_some() {
        keys.push("comment");
    }
    keys
}

fn private(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/private")
        .join(name);
    p.is_file().then_some(p).or_else(|| {
        eprintln!("skipped: {name} not found");
        None
    })
}

/// A different but valid value for a field (references point at existing objects).
fn changed(doc: &RfDocument, ty: FieldType, current: &Value) -> Option<Value> {
    let numbers = |k: K| {
        doc.model
            .iter_kind(k)
            .map(|(id, _)| id.no)
            .collect::<Vec<_>>()
    };
    Some(match ty {
        FieldType::Float(_) => Value::Float(match current.as_f64() {
            Some(f) if f.is_finite() => f + 0.25,
            _ => 1234.5,
        }),
        FieldType::Bool => Value::Bool(!current.as_bool().unwrap_or(false)),
        FieldType::Text => Value::Text("FemCraft edit".into()),
        FieldType::Int | FieldType::Enum => return None, // codes: no safe generic change
        FieldType::Ref(k) => {
            let nos = numbers(k);
            let cur = current.as_ref_no();
            Value::Ref(*nos.iter().find(|n| Some(**n) != cur).or(nos.first())?)
        }
        FieldType::RefList(k) => {
            let mut l = current.as_ref_list().to_vec();
            if l.len() > 1 {
                l.reverse();
            } else {
                l = vec![*numbers(k).first()?];
            }
            Value::RefList(l)
        }
    })
}

fn check_model(name: &str) {
    let Some(src) = private(name) else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut doc = RfDocument::open(&src).unwrap();
    let mut expected: Vec<(ObjectId, &'static str, Value)> = Vec::new();
    for &kind in K::ALL {
        let Some((id, obj)) = doc.model.iter_kind(kind).next() else {
            continue;
        };
        let obj = obj.clone();
        // As in the UI: only fields the file stores for this object.
        for key in editable(kind)
            .into_iter()
            .filter(|k| obj.values.contains_key(k))
        {
            let ty = kind.spec().field(key).unwrap().ty;
            let Some(v) = changed(&doc, ty, obj.get(key)) else {
                continue;
            };
            doc.model.set(id, key, v.clone()).unwrap();
            expected.push((id, key, doc.model.get(id).unwrap().get(key).clone()));
        }
    }
    let out = dir.path().join("edited.rf6");
    doc.save(&out)
        .unwrap_or_else(|e| panic!("{name}: saving {} edits failed: {e}", expected.len()));
    let back = RfDocument::open(&out).unwrap();
    for (id, key, v) in &expected {
        assert_eq!(
            back.model.get(*id).unwrap().get(key),
            v,
            "{name}: {:?} {} {key}",
            id.kind,
            id.no
        );
    }
    eprintln!(
        "{name}: {} edited fields saved and read back",
        expected.len()
    );
}

#[test]
fn portal_frame() {
    check_model("FemCraft portal frame.rf6");
}

#[test]
fn pit() {
    check_model("FemCraft pit 3 loads.rf6");
}

#[test]
fn reservoir() {
    check_model("Al Qou_Rev6.rf6");
}

#[test]
fn solids() {
    check_model("Solids_check_RBeam.rf6");
}
