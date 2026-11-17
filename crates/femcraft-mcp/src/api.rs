//! The methods, named and shaped like RFEM 6's API I (SOAP web service) so scripts and prompts
//! written for RFEM carry over: `set_node({value: {no, coordinate_1, ...}})`,
//! `get_node({no})`, `set_nodal_load({load_case_no, value})`, `get_all_object_numbers({type})`,
//! `new_model`, `open_model`, `save`.
//!
//! Field names are FemCraft's field keys ([`femcraft_model::kinds`]), which follow RFEM's API
//! names. Values are SI (m, N, Pa). Enumerations are RFEM's stored integer codes or texts, as
//! the model reads them from `.rf6` files.

use std::path::{Path, PathBuf};

use femcraft_model::{FieldType, ObjectId, ObjectKind as K, Value};
use femcraft_rfdb::RfDocument;
use serde_json::{Map, Value as Json, json};

/// One open model as listed by `get_model_list`.
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub active: bool,
    pub modified: bool,
}

/// What the methods act on: the GUI's open models, or a headless session.
pub trait Workspace {
    /// Starts an untitled model (from the blank model behind File > New) and makes it active.
    fn new_model(&mut self, name: Option<&str>) -> Result<String, String>;
    /// Opens an `.rf6` (or switches to it if open) and makes it active.
    fn open_model(&mut self, path: &Path) -> Result<String, String>;
    /// Saves the active model to `path`, or to its own file when `None`.
    fn save_model(&mut self, path: Option<&Path>) -> Result<PathBuf, String>;
    fn models(&self) -> Vec<ModelInfo>;
    fn active(&mut self) -> Option<&mut RfDocument>;
    /// Called after the active model was edited (the GUI redraws it).
    fn changed(&mut self) {}
}

/// Object types exposed so far, with RFEM's method suffix and `E_OBJECT_TYPE_*` name.
pub const KINDS: &[(K, &str, &str)] = &[
    (K::Material, "material", "E_OBJECT_TYPE_MATERIAL"),
    (K::Section, "section", "E_OBJECT_TYPE_SECTION"),
    (K::Thickness, "thickness", "E_OBJECT_TYPE_THICKNESS"),
    (K::Node, "node", "E_OBJECT_TYPE_NODE"),
    (K::Line, "line", "E_OBJECT_TYPE_LINE"),
    (K::Member, "member", "E_OBJECT_TYPE_MEMBER"),
    (K::Surface, "surface", "E_OBJECT_TYPE_SURFACE"),
    (
        K::NodalSupport,
        "nodal_support",
        "E_OBJECT_TYPE_NODAL_SUPPORT",
    ),
    (K::LineSupport, "line_support", "E_OBJECT_TYPE_LINE_SUPPORT"),
    (
        K::SurfaceSupport,
        "surface_support",
        "E_OBJECT_TYPE_SURFACE_SUPPORT",
    ),
    (K::LoadCase, "load_case", "E_OBJECT_TYPE_LOAD_CASE"),
    (K::NodalLoad, "nodal_load", "E_OBJECT_TYPE_NODAL_LOAD"),
    (K::LineLoad, "line_load", "E_OBJECT_TYPE_LINE_LOAD"),
    (K::SurfaceLoad, "surface_load", "E_OBJECT_TYPE_SURFACE_LOAD"),
];

fn kind_by_suffix(s: &str) -> Option<K> {
    KINDS.iter().find(|k| k.1 == s).map(|k| k.0)
}

fn kind_by_type(s: &str) -> Option<K> {
    KINDS
        .iter()
        .find(|k| k.2.eq_ignore_ascii_case(s) || k.1.eq_ignore_ascii_case(s))
        .map(|k| k.0)
}

/// A method: name, description and JSON schema of its arguments (for `tools/list`).
pub struct MethodInfo {
    pub name: String,
    pub description: String,
    pub schema: Json,
    pub changes_model: bool,
}

fn json_type(ty: FieldType) -> Json {
    match ty {
        FieldType::Bool => json!({"type": "boolean"}),
        FieldType::Int | FieldType::Ref(_) => json!({"type": "integer"}),
        FieldType::Float("") => json!({"type": "number"}),
        FieldType::Float(u) => json!({"type": "number", "description": format!("[{u}]")}),
        FieldType::Text => json!({"type": "string"}),
        FieldType::Enum => json!({"type": ["integer", "string"],
            "description": "RFEM code (integer) or text"}),
        FieldType::RefList(_) => json!({"type": ["array", "string"], "items": {"type": "integer"},
            "description": "object numbers, e.g. [1,2,3] or \"1-3,7\""}),
    }
}

fn value_schema(kind: K) -> Json {
    let mut props = Map::new();
    props.insert(
        "no".into(),
        json!({"type": "integer", "description": "object number (default: next free)"}),
    );
    for f in kind.spec().fields {
        let mut t = json_type(f.ty);
        let desc = match t.get("description").and_then(Json::as_str) {
            Some(d) => format!("{} {d}", f.label),
            None => f.label.to_owned(),
        };
        t["description"] = desc.into();
        props.insert(f.key.into(), t);
    }
    json!({"type": "object", "properties": props, "additionalProperties": false})
}

const LC_NO: &str = "load_case_no";

/// All methods, for `tools/list`.
pub fn methods() -> Vec<MethodInfo> {
    let obj =
        |props: Json, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let mut out = vec![
        MethodInfo {
            name: "new_model".into(),
            description: "Starts a new, untitled model from FemCraft's blank model (File > New) \
                and makes it active. It keeps the template's materials, sections, thickness and \
                load case 1. Save it with save_as."
                .into(),
            schema: obj(json!({"name": {"type": "string"}}), &[]),
            changes_model: true,
        },
        MethodInfo {
            name: "open_model".into(),
            description: "Opens an RFEM 6 .rf6 file and makes it the active model.".into(),
            schema: obj(json!({"model_path": {"type": "string"}}), &["model_path"]),
            changes_model: false,
        },
        MethodInfo {
            name: "save".into(),
            description: "Saves the active model to its own .rf6 file (File > Save).".into(),
            schema: obj(json!({}), &[]),
            changes_model: false,
        },
        MethodInfo {
            name: "save_as".into(),
            description: "Saves the active model to a new .rf6 file (File > Save As); the model \
                then refers to that file."
                .into(),
            schema: obj(json!({"model_path": {"type": "string"}}), &["model_path"]),
            changes_model: false,
        },
        MethodInfo {
            name: "get_model_list".into(),
            description: "Open models: name, path, active, modified.".into(),
            schema: obj(json!({}), &[]),
            changes_model: false,
        },
    ];
    let types: Vec<&str> = KINDS.iter().map(|k| k.2).collect();
    for name in ["get_all_object_numbers", "get_first_free_number"] {
        out.push(MethodInfo {
            name: name.into(),
            description: format!(
                "{} of an object type in the active model. Loads need load_case_no.",
                if name.starts_with("get_all") {
                    "Object numbers"
                } else {
                    "Lowest free object number"
                }
            ),
            schema: obj(
                json!({"type": {"type": "string", "enum": types},
                       LC_NO: {"type": "integer"}}),
                &["type"],
            ),
            changes_model: false,
        });
    }
    for &(kind, suffix, _) in KINDS {
        let load = kind.spec().is_load();
        let lc = json!({"type": "integer", "description": "load case number"});
        let mut set_props = json!({"value": value_schema(kind)});
        let mut get_props = json!({"no": {"type": "integer"}});
        let (mut set_req, mut get_req) = (vec!["value"], vec!["no"]);
        if load {
            set_props[LC_NO] = lc.clone();
            get_props[LC_NO] = lc;
            set_req.push(LC_NO);
            get_req.push(LC_NO);
        }
        let label = kind.spec().label.to_lowercase();
        out.push(MethodInfo {
            name: format!("set_{suffix}"),
            description: format!(
                "Creates or changes one of the {label} of the active model. Only the fields \
                 given are set; a new object starts from a similar object when saved.{}",
                if load {
                    " Loads are numbered per load case."
                } else {
                    ""
                }
            ),
            schema: obj(set_props, &set_req),
            changes_model: true,
        });
        out.push(MethodInfo {
            name: format!("get_{suffix}"),
            description: format!("Field values of one of the {label} of the active model."),
            schema: obj(get_props, &get_req),
            changes_model: false,
        });
    }
    out
}

/// Calls a method by name with JSON arguments.
pub fn call(ws: &mut dyn Workspace, name: &str, args: &Json) -> Result<Json, String> {
    let empty = Map::new();
    let args = args.as_object().unwrap_or(&empty);
    let path = |key: &str| -> Result<PathBuf, String> {
        args.get(key)
            .and_then(Json::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} needs '{key}' (a file path)"))
    };
    match name {
        "new_model" => {
            let n = ws.new_model(args.get("name").and_then(Json::as_str))?;
            return Ok(json!({"name": n}));
        }
        "open_model" => {
            let n = ws.open_model(&path("model_path")?)?;
            return Ok(json!({"name": n}));
        }
        "save" => {
            let p = ws.save_model(None)?;
            return Ok(json!({"saved": p.display().to_string()}));
        }
        "save_as" => {
            let mut p = path("model_path")?;
            if p.extension().is_none() {
                p.set_extension("rf6");
            }
            let p = ws.save_model(Some(&p))?;
            return Ok(json!({"saved": p.display().to_string()}));
        }
        "get_model_list" => {
            let list: Vec<Json> = ws
                .models()
                .into_iter()
                .map(|m| {
                    json!({"name": m.name, "path": m.path.display().to_string(),
                           "active": m.active, "modified": m.modified})
                })
                .collect();
            return Ok(json!(list));
        }
        _ => {}
    }

    let lc = || -> Result<u32, String> {
        args.get(LC_NO)
            .and_then(Json::as_u64)
            .map(|n| n as u32)
            .ok_or_else(|| format!("{name} needs '{LC_NO}'"))
    };
    if name == "get_all_object_numbers" || name == "get_first_free_number" {
        let ty = args.get("type").and_then(Json::as_str).unwrap_or("");
        let kind = kind_by_type(ty).ok_or_else(|| format!("unknown object type '{ty}'"))?;
        let lc_no = if kind.spec().is_load() { lc()? } else { 0 };
        let doc = active(ws)?;
        if name == "get_first_free_number" {
            return Ok(json!(doc.model.next_no(kind, lc_no)));
        }
        let nos: Vec<u32> = doc
            .model
            .iter_kind(kind)
            .filter(|(id, _)| id.load_case == lc_no)
            .map(|(id, _)| id.no)
            .collect();
        return Ok(json!(nos));
    }

    let (verb, suffix) = name
        .split_once('_')
        .ok_or_else(|| format!("unknown method '{name}'"))?;
    let kind = kind_by_suffix(suffix).ok_or_else(|| format!("unknown method '{name}'"))?;
    let lc_no = if kind.spec().is_load() { lc()? } else { 0 };
    match verb {
        "get" => {
            let no = args
                .get("no")
                .and_then(Json::as_u64)
                .ok_or_else(|| format!("{name} needs 'no'"))? as u32;
            let doc = active(ws)?;
            let id = ObjectId::load(kind, lc_no, no);
            let obj = doc
                .model
                .get(id)
                .ok_or_else(|| format!("{} {no} does not exist", kind.name()))?;
            let mut out = Map::new();
            out.insert("no".into(), json!(no));
            for f in kind.spec().fields {
                let v = obj.get(f.key);
                if !v.is_null() {
                    out.insert(f.key.into(), to_json(v));
                }
            }
            Ok(Json::Object(out))
        }
        "set" => {
            let value = args
                .get("value")
                .and_then(Json::as_object)
                .ok_or_else(|| format!("{name} needs 'value' (an object of fields)"))?;
            // Check every field before changing anything.
            let mut fields = Vec::new();
            for (k, v) in value {
                if k == "no" {
                    continue;
                }
                let spec = kind.spec().field(k).ok_or_else(|| {
                    let keys: Vec<&str> = kind.spec().fields.iter().map(|f| f.key).collect();
                    format!(
                        "{} has no field '{k}'; fields: no, {}",
                        kind.name(),
                        keys.join(", ")
                    )
                })?;
                fields.push((
                    spec.key,
                    from_json(spec.ty, v).map_err(|e| format!("{k}: {e}"))?,
                ));
            }
            let doc = active(ws)?;
            let no = match value.get("no") {
                Some(n) => n
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("'no' must be a positive integer")? as u32,
                None => doc.model.next_no(kind, lc_no),
            };
            if kind.spec().is_load() && doc.model.get(ObjectId::new(K::LoadCase, lc_no)).is_none() {
                return Err(format!(
                    "load case {lc_no} does not exist; set_load_case first"
                ));
            }
            let id = ObjectId::load(kind, lc_no, no);
            let created = doc.model.get(id).is_none();
            if created {
                doc.model.add(id).map_err(|e| e.to_string())?;
            }
            for (k, v) in fields {
                doc.model.set(id, k, v).map_err(|e| e.to_string())?;
            }
            ws.changed();
            Ok(json!({"no": no, "created": created}))
        }
        _ => Err(format!("unknown method '{name}'")),
    }
}

fn active(ws: &mut dyn Workspace) -> Result<&mut RfDocument, String> {
    ws.active()
        .ok_or_else(|| "no model is open; call new_model or open_model first".to_owned())
}

fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::Float(f) if f.is_finite() => json!(f),
        Value::Float(_) => Json::Null,
        Value::Text(s) => json!(s),
        Value::Ref(n) => json!(n),
        Value::RefList(l) => json!(l),
    }
}

fn from_json(ty: FieldType, v: &Json) -> Result<Value, String> {
    let bad = || format!("expects {}", json_type(ty)["type"]);
    Ok(match (ty, v) {
        (_, Json::Null) => Value::Null,
        (FieldType::Bool, Json::Bool(b)) => Value::Bool(*b),
        (FieldType::Int, Json::Number(n)) => Value::Int(n.as_i64().ok_or_else(bad)?),
        (FieldType::Float(_), Json::Number(n)) => Value::Float(n.as_f64().ok_or_else(bad)?),
        (FieldType::Text, Json::String(s)) => Value::Text(s.clone()),
        (FieldType::Enum, Json::String(s)) => Value::Text(s.clone()),
        (FieldType::Enum, Json::Number(n)) => Value::Int(n.as_i64().ok_or_else(bad)?),
        (FieldType::Ref(_), Json::Number(n)) => {
            Value::Ref(n.as_u64().filter(|n| *n > 0).ok_or_else(bad)? as u32)
        }
        (FieldType::RefList(_), Json::Array(a)) => Value::RefList(
            a.iter()
                .map(|n| n.as_u64().filter(|n| *n > 0).map(|n| n as u32))
                .collect::<Option<_>>()
                .ok_or_else(bad)?,
        ),
        (FieldType::RefList(_), Json::String(s)) => {
            Value::RefList(femcraft_model::list::parse_list(s).ok_or_else(bad)?)
        }
        (FieldType::RefList(_), Json::Number(n)) => {
            Value::RefList(vec![n.as_u64().filter(|n| *n > 0).ok_or_else(bad)? as u32])
        }
        _ => return Err(bad()),
    })
}
