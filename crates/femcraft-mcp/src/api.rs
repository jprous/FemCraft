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
use femcraft_rfdb::{Library, LibraryFilter, LibraryItem, LibraryKind, RfDocument};
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
    /// Calculates the active model with the RFEM server (Settings > RFEM server); returns the
    /// calculated copy, which is opened as a new model.
    fn solve(&mut self) -> Result<PathBuf, String> {
        Err("solving needs the FemCraft window (Settings > RFEM server)".into())
    }
    /// Generates only the FE mesh of the active model with the RFEM server (no results).
    fn mesh(&mut self) -> Result<PathBuf, String> {
        Err("meshing needs the FemCraft window (Settings > RFEM server)".into())
    }
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
    (K::Opening, "opening", "E_OBJECT_TYPE_OPENING"),
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
    (K::MemberLoad, "member_load", "E_OBJECT_TYPE_MEMBER_LOAD"),
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
            name: "generate_mesh".into(),
            description: "Generates only the FE mesh of the active model with the RFEM 6 server \
                (as RFEM's generate_mesh), without calculating. The model is saved first if it \
                has unsaved edits; the mesh is stored in the model's file and the open model \
                is reloaded (see get_mesh)."
                .into(),
            schema: obj(json!({}), &[]),
            changes_model: false,
        },
        MethodInfo {
            name: "calculate_all".into(),
            description: "Calculates the active model with the RFEM 6 server (as RFEM's \
                calculate_all). The model is saved first if it has unsaved edits; the calculated \
                copy (in FemCraft's working directory, folder 'solved') opens as a new model \
                and its path is returned."
                .into(),
            schema: obj(json!({}), &[]),
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
        MethodInfo {
            name: "get_mesh".into(),
            description: "FE mesh RFEM saved in the active model's file (after meshing or                 calculating in RFEM): node / element counts, elements per surface and the FE                 node chain along each line. full = true adds nodes ([x, y, z] in m, FE node n                 at index n - 1) and elements (4 FE node numbers, 4th = 3rd or 0 for a                 triangle, with their surface). stale = true once the model has been edited                 (saving then drops the mesh)."
                .into(),
            schema: obj(json!({"full": {"type": "boolean"}}), &[]),
            changes_model: false,
        },
        MethodInfo {
            name: "get_library_items".into(),
            description: "Lists FemCraft's material or section library (Eurocode and South \
                African items extracted from RFEM 6): RFEM name, standard, manufacturer, \
                region (EU / ZA), family (IPE, UB, CHS, C, S, ...), category (materials: \
                concrete, reinforcement, steel_hot_rolled, steel_cold_formed; sections: \
                hot_rolled, cold_formed) and RFEM library id."
                .into(),
            schema: obj(
                json!({"kind": {"type": "string", "enum": ["material", "section"]},
                       "search": {"type": "string", "description": "part of the name"},
                       "region": {"type": "string", "enum": ["EU", "ZA"]},
                       "family": {"type": "string"},
                       "category": {"type": "string"},
                       "limit": {"type": "integer", "description": "default 100, 0 = all"}}),
                &["kind"],
            ),
            changes_model: false,
        },
        MethodInfo {
            name: "set_library_items".into(),
            description: "FemCraft extension: makes several existing materials / sections \
                library items in one go (items: [{type: material | section, no, name}], name \
                as in set_material / set_section). Much faster than one set_* with name per \
                object: the model is reloaded once."
                .into(),
            schema: obj(
                json!({"items": {"type": "array", "items": {"type": "object",
                    "properties": {"type": {"type": "string", "enum": ["material", "section"]},
                                   "no": {"type": "integer"}, "name": {"type": "string"}},
                    "required": ["type", "no", "name"]}}}),
                &["items"],
            ),
            changes_model: true,
        },
        MethodInfo {
            name: "get_library_item".into(),
            description: "One library material or section with all its RFEM property values \
                (SI units; f_y / f_u per thickness range). name: RFEM's full name, or a \
                designation such as 'IPE 200' when only one item has it."
                .into(),
            schema: obj(
                json!({"kind": {"type": "string", "enum": ["material", "section"]},
                       "name": {"type": "string"}}),
                &["kind", "name"],
            ),
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
        if library_kind(kind).is_some() {
            set_props["value"]["properties"]["name"] = json!({"type": "string",
                "description": "RFEM library name, e.g. 'IPE 200 | DIN 1025:1965 | Macsteel' or \
                 'S355JR | SANS 50025-2' (get_library_items lists them); a designation such as \
                 'UB 203x133x25' when only one item has it"});
        }
        out.push(MethodInfo {
            name: format!("set_{suffix}"),
            description: format!(
                "Creates or changes one of the {label} of the active model. Only the fields \
                 given are set; a new object starts from a similar object when saved.{}{}",
                if load {
                    " Loads are numbered per load case."
                } else {
                    ""
                },
                match kind {
                    K::Section => {
                        " 'name' picks a library section (copied into the model with its \
                         properties; members keep referring to it; 'material' sets its material)."
                    }
                    K::Material => {
                        " 'name' picks a library material (copied into the model with its \
                         properties)."
                    }
                    _ => "",
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
        "generate_mesh" => {
            let p = ws.mesh()?;
            return Ok(json!({"meshed": p.display().to_string()}));
        }
        "calculate_all" => {
            let p = ws.solve()?;
            return Ok(json!({"solved": p.display().to_string()}));
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
        "get_library_items" | "get_library_item" => return library_call(name, args),
        "set_library_items" => return set_library_items(ws, args),
        "get_mesh" => {
            let doc = ws.active().ok_or("no model is open")?;
            let Some(mesh) = doc.fe_mesh().map_err(|e| e.to_string())? else {
                return Ok(json!({"mesh": false,
                    "note": "the file holds no FE mesh; mesh or calculate it in RFEM"}));
            };
            let mut per_surface = std::collections::BTreeMap::<u32, usize>::new();
            for &s in &mesh.element_surface {
                *per_surface.entry(s).or_default() += 1;
            }
            let mut out = json!({
                "mesh": true,
                "stale": doc.is_modified(),
                "node_count": mesh.nodes.len(),
                "element_count": mesh.elements.len(),
                "elements_per_surface": per_surface.iter()
                    .map(|(s, n)| json!({"surface": s, "elements": n})).collect::<Vec<_>>(),
                "lines": mesh.lines.iter()
                    .map(|(l, n)| json!({"line": l, "fe_nodes": n})).collect::<Vec<_>>(),
            });
            if args.get("full").and_then(Json::as_bool).unwrap_or(false) {
                out["nodes"] = json!(mesh.nodes);
                out["elements"] = mesh
                    .elements
                    .iter()
                    .zip(&mesh.element_surface)
                    .map(|(e, s)| json!({"nodes": e, "surface": s}))
                    .collect();
            }
            return Ok(out);
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
                let mut val = from_json(spec.ty, v).map_err(|e| format!("{k}: {e}"))?;
                // RFEM stores load directions as integer codes; accept its API names too.
                if spec.key == "load_direction"
                    && let Value::Text(t) = &val
                {
                    val = Value::Int(load_direction_code(t).ok_or_else(|| {
                        format!(
                            "{k}: unknown load direction '{t}'; use a code or \
                                 LOAD_DIRECTION_LOCAL_X/Y/Z, LOAD_DIRECTION_GLOBAL_X/Y/Z_OR_\
                                 USER_DEFINED_U/V/W_TRUE"
                        )
                    })?);
                }
                // Likewise load distributions and action categories: RFEM calls a file with
                // their API names in these integer columns corrupted.
                if let Value::Text(t) = &val
                    && let Some((codes, what)) = match spec.key {
                        "load_distribution" => Some((LOAD_DISTRIBUTIONS, "load distribution")),
                        "action_category" => Some((ACTION_CATEGORIES, "action category")),
                        _ => None,
                    }
                {
                    let name = t.trim().to_ascii_uppercase();
                    val = match codes.iter().find(|(n, _)| *n == name) {
                        Some((_, c)) => Value::Int(*c),
                        None => match name.parse::<i64>() {
                            Ok(c) => Value::Int(c),
                            Err(_) => {
                                let known: Vec<String> =
                                    codes.iter().map(|(n, c)| format!("{n} = {c}")).collect();
                                return Err(format!(
                                    "{k}: '{t}' is not a known {what} name; give RFEM's integer \
                                     code (known: {})",
                                    known.join(", ")
                                ));
                            }
                        },
                    };
                }
                fields.push((spec.key, val));
            }
            // Materials and sections: `name` is an RFEM library name (as RFEM's API takes it);
            // the library item is copied into the model after the other fields are set.
            let mut library = None;
            if let Some(lib_kind) = library_kind(kind)
                && let Some(i) = fields.iter().position(|(k, _)| *k == "name")
            {
                let (_, v) = fields.remove(i);
                let lib = Library::find(&Library::default_dirs())
                    .ok_or("no FemCraft library found (materials.db / sections.db)")?;
                let item = resolve_library(&lib, lib_kind, v.as_str().unwrap_or(""))?;
                library = Some((lib, item));
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
            let mut out = json!({"no": no, "created": created});
            if let Some((lib, item)) = library {
                doc.assign_library_item(id, &lib, &item)
                    .map_err(|e| format!("{} {no}: {e}", kind.name()))?;
                out["library_item"] = json!(item.name);
                out["rfem_library_id"] = json!(item.rfem_library_id);
            }
            ws.changed();
            Ok(out)
        }
        _ => Err(format!("unknown method '{name}'")),
    }
}

/// RFEM's stored code for a load direction given by its API I name: line / member / surface
/// loads use 0 / 1 / 2 = local x / y / z and 11 / 12 / 13 = global X / Y / Z (nodal loads
/// 0 / 1 / 2 = global X / Y / Z, and take codes only).
fn load_direction_code(name: &str) -> Option<i64> {
    let n = name.trim().to_ascii_uppercase();
    Some(match n.as_str() {
        "LOAD_DIRECTION_LOCAL_X" => 0,
        "LOAD_DIRECTION_LOCAL_Y" => 1,
        "LOAD_DIRECTION_LOCAL_Z" => 2,
        "LOAD_DIRECTION_GLOBAL_X_OR_USER_DEFINED_U_TRUE" => 11,
        "LOAD_DIRECTION_GLOBAL_Y_OR_USER_DEFINED_V_TRUE" => 12,
        "LOAD_DIRECTION_GLOBAL_Z_OR_USER_DEFINED_W_TRUE" => 13,
        _ => return None,
    })
}

/// Load distribution names with RFEM's stored code. Only uniform is confirmed (0 on every
/// uniform line / surface load in the verified models); others need a code.
const LOAD_DISTRIBUTIONS: &[(&str, i64)] = &[("LOAD_DISTRIBUTION_UNIFORM", 0)];

/// Action category names with RFEM's stored code (`LoadCaseImplStatic.actionCategoryId`).
/// Only permanent G is confirmed (11 on dead / self-weight cases in the verified models);
/// codes seen for other categories: 19 earth pressure, 36 / 42 imposed, 61 wind,
/// 71 temperature (names not confirmed).
const ACTION_CATEGORIES: &[(&str, i64)] = &[("ACTION_CATEGORY_PERMANENT_G", 11)];

/// The library item named `wanted` (RFEM's full name or a unique designation). A parametric
/// concrete size the library lacks (`R_M1 0.3048/0.6096`, metres or mm) is created from an
/// item of the same shape.
fn resolve_library(lib: &Library, kind: LibraryKind, wanted: &str) -> Result<LibraryItem, String> {
    let mut found = lib.get(kind, wanted).map_err(|e| e.to_string())?;
    if found.is_none() && kind == LibraryKind::Section {
        found =
            femcraft_rfdb::concrete::get_or_add_concrete(lib, wanted).map_err(|e| e.to_string())?;
    }
    found.ok_or_else(|| {
        format!(
            "no library {} named '{wanted}'; get_library_items lists them \
             (RFEM's full name, or a designation only one item has)",
            kind.name()
        )
    })
}

/// `set_library_items`: several existing materials / sections made library items at once
/// (one model reload instead of one per item, which costs seconds on a full template).
fn set_library_items(ws: &mut dyn Workspace, args: &Map<String, Json>) -> Result<Json, String> {
    let list = args
        .get("items")
        .and_then(Json::as_array)
        .ok_or("set_library_items needs 'items': [{type, no, name}]")?;
    let lib = Library::find(&Library::default_dirs())
        .ok_or("no FemCraft library found (materials.db / sections.db)")?;
    let mut items = Vec::new();
    let mut out = Vec::new();
    for it in list {
        let ty = it.get("type").and_then(Json::as_str).unwrap_or("");
        let kind = kind_by_type(ty).ok_or_else(|| format!("unknown type '{ty}'"))?;
        let lib_kind =
            library_kind(kind).ok_or_else(|| format!("{} has no library", kind.name()))?;
        let no = it
            .get("no")
            .and_then(Json::as_u64)
            .filter(|n| *n > 0)
            .ok_or("each item needs 'no'")? as u32;
        let name = it.get("name").and_then(Json::as_str).unwrap_or("");
        let item = resolve_library(&lib, lib_kind, name)
            .map_err(|e| format!("{} {no}: {e}", kind.name()))?;
        out.push(json!({"type": ty, "no": no, "library_item": item.name,
                        "rfem_library_id": item.rfem_library_id}));
        items.push((ObjectId::new(kind, no), item));
    }
    let doc = active(ws)?;
    if let Some((id, _)) = items.iter().find(|(id, _)| doc.model.get(*id).is_none()) {
        return Err(format!("{} {} does not exist", id.kind.name(), id.no));
    }
    doc.assign_library_items(&lib, &items)
        .map_err(|e| e.to_string())?;
    ws.changed();
    Ok(json!({"items": out}))
}

/// The library behind an object kind's `name` (materials, sections).
fn library_kind(kind: K) -> Option<LibraryKind> {
    match kind {
        K::Material => Some(LibraryKind::Material),
        K::Section => Some(LibraryKind::Section),
        _ => None,
    }
}

/// `get_library_items` / `get_library_item` (see [`femcraft_rfdb::library`]).
fn library_call(name: &str, args: &Map<String, Json>) -> Result<Json, String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let kind = LibraryKind::parse(&text("kind"))
        .ok_or_else(|| format!("{name} needs 'kind': \"material\" or \"section\""))?;
    let dirs = Library::default_dirs();
    let lib = Library::find(&dirs).ok_or_else(|| {
        let list: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        format!(
            "no FemCraft library found (materials.db / sections.db in {}); build it with \
             tools/rfem_library_extract.py",
            list.join(", ")
        )
    })?;
    let item_json = |i: &LibraryItem| {
        json!({"name": i.name, "designation": i.designation, "standard": i.standard,
               "manufacturer": i.manufacturer, "region": i.region, "family": i.family,
               "category": i.category, "rfem_library_id": i.rfem_library_id})
    };
    if name == "get_library_items" {
        let filter = LibraryFilter {
            text: text("search"),
            region: text("region"),
            family: text("family"),
            category: text("category"),
            limit: args.get("limit").and_then(Json::as_u64).unwrap_or(100) as usize,
        };
        let items = lib.list(kind, &filter).map_err(|e| e.to_string())?;
        return Ok(json!({"count": items.len(),
                         "items": items.iter().map(item_json).collect::<Vec<_>>()}));
    }
    let wanted = text("name");
    let item = lib
        .get(kind, &wanted)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no library {} named '{wanted}'", kind.name()))?;
    let mut values = Map::new();
    for v in lib.values(&item).map_err(|e| e.to_string())? {
        let val = match (v.value_si, &v.value_text) {
            (Some(x), _) => json!(x),
            (None, Some(t)) => json!(t),
            _ => Json::Null,
        };
        let val = match v.thickness_si {
            Some(t) => json!({"up_to_thickness": t, "value": val}),
            None => val,
        };
        match values.get_mut(&v.key) {
            // Thickness ranges: a list in RFEM's order.
            Some(Json::Array(a)) => a.push(val),
            Some(_) => {}
            None if v.thickness_si.is_some() => {
                values.insert(v.key, json!([val]));
            }
            None => {
                values.insert(v.key, val);
            }
        }
    }
    let mut out = item_json(&item);
    out["rfem_version"] = json!(item.rfem_version);
    out["values"] = Json::Object(values);
    Ok(out)
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
        // JSON has no infinity: "inf" = fixed / rigid, "-inf" = not set (RFEM's codes).
        (FieldType::Float(_), Json::String(s)) if s.trim_start_matches('-') == "inf" => {
            Value::Float(s.parse().map_err(|_| bad())?)
        }
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
