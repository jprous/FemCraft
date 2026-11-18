//! Loading the model tables from `model.db` and writing edits back.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use femcraft_model::{FieldType, Model, Object, ObjectId, ObjectKind, Value, list};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::Error;
use crate::archive::{Archive, SourceFormat};
use crate::catalog::{Catalog, quote};
use crate::mapping::{self, CHILD_META_COLUMNS, CHILD_ORDER_COLUMNS, CHILD_VALUE_COLUMNS, Derived};

/// What the loader found for one object kind.
#[derive(Debug, Clone)]
pub struct KindReport {
    pub kind: ObjectKind,
    /// Table used, or `None` if no candidate table exists.
    pub table: Option<String>,
    pub objects: usize,
    /// Fields not found in any row.
    pub missing_fields: Vec<&'static str>,
    /// Columns of the main / impl tables not matched to any field (research aid).
    pub unmapped_columns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    pub kinds: Vec<KindReport>,
    /// Problems found while loading (for example duplicate object numbers).
    pub warnings: Vec<String>,
}

impl std::fmt::Display for LoadReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for k in &self.kinds {
            match &k.table {
                None => writeln!(f, "{:<26} no table", k.kind.name())?,
                Some(t) if k.objects == 0 => writeln!(
                    f,
                    "{:<26} {t}: 0 objects (mapping unverified)",
                    k.kind.name()
                )?,
                Some(t) => {
                    writeln!(f, "{:<26} {t}: {} objects", k.kind.name(), k.objects)?;
                    if !k.missing_fields.is_empty() {
                        writeln!(f, "    missing: {}", k.missing_fields.join(", "))?;
                    }
                    // `{:#}` also lists unmapped columns.
                    if f.alternate() && !k.unmapped_columns.is_empty() {
                        writeln!(f, "    unmapped: {}", k.unmapped_columns.join(", "))?;
                    }
                }
            }
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        Ok(())
    }
}

/// Where a field value was read from.
#[derive(Debug, Clone)]
enum Loc {
    Main(String),
    Impl(String),
    /// List held in a child table: rows `id = owner`, ordered by `container_order`.
    Child {
        table: String,
        owner: i64,
    },
    /// Object type derived from the impl table name (read-only).
    ImplType,
    /// Column of a one-row part table, addressed by SQLite rowid.
    Part {
        table: String,
        rowid: i64,
        column: String,
    },
    /// Looked up through key / value maps (read-only).
    Derived,
}

#[derive(Debug, Clone)]
struct Binding {
    table: String,
    key_col: &'static str,
    key: i64,
    impl_row: Option<(String, &'static str, i64)>,
    locs: HashMap<&'static str, Loc>,
    /// Values as loaded, so saving writes only changed fields.
    loaded: Object,
}

/// An opened RFEM model: the archive, the editable model and how it maps to the database.
pub struct RfDocument {
    pub archive: Archive,
    pub model: Model,
    pub report: LoadReport,
    bindings: HashMap<ObjectId, Binding>,
    /// Whether a reference field stores database ids (true) or object numbers (false).
    ref_by_id: HashMap<(ObjectKind, &'static str), bool>,
    /// Object number -> database id, per referenced kind.
    ids: HashMap<ObjectKind, HashMap<u32, i64>>,
    /// RFEM files to copy new objects from when this model has no object of that kind yet
    /// (see [`crate::build::insert_new_objects`]).
    pub prototype_files: Vec<std::path::PathBuf>,
    /// The working database was changed directly (library items assigned) since the last save.
    db_changed: bool,
}

type Row = Vec<(String, Sql)>;

fn get<'a>(row: &'a Row, name: &str) -> Option<&'a Sql> {
    row.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

fn find<'a, S: AsRef<str>>(row: &'a Row, cands: &[S]) -> Option<(&'a str, &'a Sql)> {
    cands.iter().find_map(|c| {
        row.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(c.as_ref()))
            .map(|(n, v)| (n.as_str(), v))
    })
}

fn int(v: &Sql) -> Option<i64> {
    match v {
        Sql::Integer(i) => Some(*i),
        Sql::Real(r) if r.fract() == 0.0 => Some(*r as i64),
        Sql::Text(t) => t.trim().parse().ok(),
        _ => None,
    }
}

/// Reads all rows of a table with the key column used to address them.
fn read_rows(conn: &Connection, table: &str) -> Result<(Vec<Row>, &'static str), Error> {
    let q = quote(table);
    let (sql, key) = match conn.prepare(&format!("SELECT * FROM {q} LIMIT 0")) {
        Ok(s) if s.column_names().any(|c| c.eq_ignore_ascii_case("id")) => {
            (format!("SELECT * FROM {q}"), "id")
        }
        _ => (format!("SELECT rowid AS rowid, * FROM {q}"), "rowid"),
    };
    let mut stmt = conn.prepare(&sql)?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let rows = stmt
        .query_map([], |r| {
            (0..names.len())
                .map(|i| Ok((names[i].clone(), r.get::<_, Sql>(i)?)))
                .collect()
        })?
        .collect::<Result<Vec<Row>, _>>()?;
    Ok((rows, key))
}

/// Row lookups by key, cached per table.
#[derive(Default)]
struct Cache {
    tables: HashMap<String, (HashMap<i64, Row>, &'static str)>,
    children: HashMap<String, HashMap<i64, Vec<Sql>>>,
}

impl Cache {
    fn row(
        &mut self,
        conn: &Connection,
        table: &str,
        key: i64,
    ) -> Result<Option<(Row, &'static str)>, Error> {
        if !self.tables.contains_key(table) {
            let (rows, kc) = read_rows(conn, table)?;
            let map = rows
                .into_iter()
                .filter_map(|r| Some((int(get(&r, kc)?)?, r)))
                .collect();
            self.tables.insert(table.to_owned(), (map, kc));
        }
        let (map, kc) = &self.tables[table];
        Ok(map.get(&key).map(|r| (r.clone(), *kc)))
    }

    /// Values of a child list table for one owner, in row order.
    fn child(
        &mut self,
        conn: &Connection,
        cat: &Catalog,
        table: &str,
        owner: i64,
    ) -> Result<Vec<Sql>, Error> {
        if !self.children.contains_key(table) {
            let info = cat.table(table).expect("checked by caller");
            let value_col = CHILD_VALUE_COLUMNS
                .iter()
                .find_map(|c| info.column(c))
                .or_else(|| {
                    info.columns.iter().find(|c| {
                        !CHILD_META_COLUMNS
                            .iter()
                            .any(|m| c.name.eq_ignore_ascii_case(m))
                    })
                })
                .map(|c| c.name.clone());
            let owner = if info.column("owner_id").is_some() {
                "owner_id"
            } else {
                "id"
            };
            let order = CHILD_ORDER_COLUMNS
                .iter()
                .find_map(|o| info.column(o))
                .map_or("rowid".to_owned(), |c| quote(&c.name));
            let mut map: HashMap<i64, Vec<Sql>> = HashMap::new();
            if let Some(vc) = value_col {
                let sql = format!(
                    "SELECT {owner}, {} FROM {} ORDER BY {owner}, {order}",
                    quote(&vc),
                    quote(table)
                );
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    map.entry(r.get(0)?).or_default().push(r.get(1)?);
                }
            }
            self.children.insert(table.to_owned(), map);
        }
        Ok(self.children[table]
            .get(&owner)
            .cloned()
            .unwrap_or_default())
    }
}

/// `MemberImplBeam` -> `TYPE_BEAM`; `LineTypeLoadImplForce` with prefix `LOAD_TYPE` ->
/// `LOAD_TYPE_FORCE`. `None` when the impl table name carries no type (`MaterialImpl`).
fn type_from_impl(prefix: &str, impl_table: &str) -> Option<String> {
    upper_snake(prefix, &impl_table[impl_table.rfind("Impl")? + 4..])
}

/// `("TYPE", "WithoutThickness")` -> `TYPE_WITHOUT_THICKNESS`; `None` for an empty name.
fn upper_snake(prefix: &str, rest: &str) -> Option<String> {
    if rest.is_empty() {
        return None;
    }
    let mut out = String::from(prefix);
    for c in rest.chars() {
        if c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    Some(out)
}

/// The one row of a part table owned by an impl row: `(rowid, value of column)`.
fn part_value(
    conn: &Connection,
    cat: &Catalog,
    table: &str,
    column: &str,
    impl_table: &str,
    impl_id: i64,
) -> Result<Option<(String, i64, Sql)>, Error> {
    let table = table.replace("{impl}", impl_table);
    let Some(info) = cat.table(&table) else {
        return Ok(None);
    };
    let Some(col) = info.column(column) else {
        return Ok(None);
    };
    let filter = if info.column("owner_id").is_some() {
        if info.column("owner_table").is_some() {
            "owner_id = ?1 AND owner_table = ?2"
        } else {
            "owner_id = ?1"
        }
    } else if info.column("container_order").is_some() {
        "id = ?1 AND container_order = 0"
    } else {
        "id = ?1"
    };
    let n_params = if filter.contains("?2") { 2 } else { 1 };
    let sql = format!(
        "SELECT rowid, {} FROM {} WHERE {filter} LIMIT 1",
        quote(&col.name),
        quote(&info.name)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let row = if n_params == 2 {
        stmt.query_row(rusqlite::params![impl_id, impl_table], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
    } else {
        stmt.query_row(rusqlite::params![impl_id], |r| Ok((r.get(0)?, r.get(1)?)))
    }
    .optional()?;
    Ok(row.map(|(rowid, v)| (info.name.clone(), rowid, v)))
}

/// Value of `key` in the map `<table>_<map>_keys` / `_values` owned by `owner`.
/// Values holding `reference_id` / `reference_table` are followed to that row's `valueSI`.
fn map_value(
    conn: &Connection,
    cat: &Catalog,
    table: &str,
    map: &str,
    owner: i64,
    key: &str,
) -> Result<Option<Sql>, Error> {
    let keys = format!("{table}_{map}_keys");
    let values = format!("{table}_{map}_values");
    let Some(vinfo) = cat.table(&values) else {
        return Ok(None);
    };
    if cat.table(&keys).is_none() {
        return Ok(None);
    }
    let join = format!(
        "FROM {} k JOIN {} v ON v.id = k.id AND v.container_order = k.container_order \
         WHERE k.id = ?1 AND k.value = ?2",
        quote(&keys),
        quote(&values)
    );
    if vinfo.column("valueSI").is_some() {
        let sql = format!("SELECT v.valueSI {join}");
        return Ok(conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params![owner, key], |r| r.get(0))
            .optional()?);
    }
    if vinfo.column("reference_id").is_some() {
        let sql = format!("SELECT v.reference_table, v.reference_id {join}");
        let target: Option<(String, i64)> = conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params![owner, key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        if let Some((t, id)) = target
            && let Some(info) = cat.table(&t)
            && info.column("valueSI").is_some()
        {
            let sql = format!("SELECT valueSI FROM {} WHERE id = ?1", quote(&info.name));
            return Ok(conn
                .prepare_cached(&sql)?
                .query_row([id], |r| r.get(0))
                .optional()?);
        }
    }
    Ok(None)
}

/// SQL string literal.
fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Integer column of the row `id` in `table`.
fn lookup_int(conn: &Connection, table: &str, column: &str, id: i64) -> Result<Option<i64>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = ?1",
        quote(column),
        quote(table)
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([id], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

/// Read-only property values through RFEM's library / property maps.
fn derived_value(
    conn: &Connection,
    cat: &Catalog,
    d: Derived,
    impl_name: &str,
    impl_id: i64,
    impl_row: &Row,
) -> Result<Option<Sql>, Error> {
    match d {
        Derived::SectionParameter(keys) => {
            let (Some(Sql::Text(lib)), Some(lib_id)) = (
                get(impl_row, "librarySection_table"),
                get(impl_row, "librarySection_id").and_then(int),
            ) else {
                return Ok(None);
            };
            if cat.table(lib).is_none() {
                return Ok(None);
            }
            let Some(data) = lookup_int(conn, lib, "sectionData_id", lib_id)? else {
                return Ok(None);
            };
            for key in keys {
                if let Some(v) = map_value(conn, cat, "SectionData", "parameterValues", data, key)?
                {
                    return Ok(Some(v));
                }
            }
            Ok(None)
        }
        Derived::SectionLibraryId => {
            let (Some(Sql::Text(lib)), Some(lib_id)) = (
                get(impl_row, "librarySection_table"),
                get(impl_row, "librarySection_id").and_then(int),
            ) else {
                return Ok(None);
            };
            if lib != "DatabaseSection"
                || cat
                    .table(lib)
                    .and_then(|t| t.column("databaseSectionId"))
                    .is_none()
            {
                return Ok(None);
            }
            // Any stored type (an integer in RFEM's files); a number when it reads as one.
            let v: Option<Sql> = conn
                .prepare_cached(&format!(
                    "SELECT databaseSectionId FROM {} WHERE id = ?1",
                    quote(lib)
                ))?
                .query_row([lib_id], |r| r.get(0))
                .optional()?;
            Ok(v.and_then(|v| match v {
                Sql::Integer(i) => Some(Sql::Integer(i)),
                Sql::Text(t) => t.trim().parse().ok().map(Sql::Integer),
                _ => None,
            }))
        }
        Derived::SectionShape => {
            let (Some(Sql::Text(lib)), Some(lib_id)) = (
                get(impl_row, "librarySection_table"),
                get(impl_row, "librarySection_id").and_then(int),
            ) else {
                return Ok(None);
            };
            if lib != "ParametricSection" || cat.table(lib).is_none() {
                return Ok(None);
            }
            let Some(code) = lookup_int(conn, lib, "parametrizationShape", lib_id)? else {
                return Ok(None);
            };
            Ok(Some(match mapping::parametric_shape(code) {
                Some(name) => Sql::Text(name.to_owned()),
                None => Sql::Integer(code),
            }))
        }
        Derived::SectionParameters => {
            let (Some(Sql::Text(lib)), Some(lib_id)) = (
                get(impl_row, "librarySection_table"),
                get(impl_row, "librarySection_id").and_then(int),
            ) else {
                return Ok(None);
            };
            if lib != "ParametricSection" || cat.table("ParametricSection_parameterValues").is_none() {
                return Ok(None);
            }
            let mut st = conn.prepare_cached(
                "SELECT value FROM ParametricSection_parameterValues WHERE id = ?1                  ORDER BY container_order",
            )?;
            let vals: Vec<String> = st
                .query_map([lib_id], |r| r.get::<_, f64>(0))?
                .map(|v| v.map(|v| format!("{}", (v * 1e4).round() / 10.0)))
                .collect::<Result<_, _>>()?;
            Ok((!vals.is_empty()).then(|| Sql::Text(vals.join("/"))))
        }
        Derived::SurfaceLoadParameter(order, node) => {
            let table = format!("{impl_name}_NodeAndMagnitudeParameters_loadParameters");
            let (Some(info), Some(params)) =
                (cat.table(&table), get(impl_row, "loadParameters_id").and_then(int))
            else {
                return Ok(None);
            };
            if info.column("node_id").is_none()
                || info.column("magnitude").is_none()
                || cat.table("Node").is_none()
            {
                return Ok(None);
            }
            let sql = if node {
                format!(
                    "SELECT n.userID FROM {} p JOIN Node n ON n.id = p.node_id                      WHERE p.id = ?1 AND p.container_order = ?2",
                    quote(&table)
                )
            } else {
                format!(
                    "SELECT p.magnitude FROM {} p WHERE p.id = ?1 AND p.container_order = ?2                      AND p.node_id IS NOT NULL",
                    quote(&table)
                )
            };
            let mut st = conn.prepare_cached(&sql)?;
            let v = if node {
                st.query_row(rusqlite::params![params, order], |r| r.get::<_, i64>(0))
                    .optional()?
                    .map(Sql::Integer)
            } else {
                st.query_row(rusqlite::params![params, order], |r| r.get::<_, f64>(0))
                    .optional()?
                    .map(Sql::Real)
            };
            Ok(v)
        }
        Derived::SurfaceStiffness => {
            let Some((_, _, Sql::Text(t))) = part_value(
                conn,
                cat,
                "SurfaceSpecificImplPart",
                "stiffness_table",
                impl_name,
                impl_id,
            )?
            else {
                return Ok(None);
            };
            let rest = t.strip_prefix("SurfaceStiffness").unwrap_or(&t);
            Ok(upper_snake("TYPE", rest).map(Sql::Text))
        }
        Derived::ControlPoint(column, axis) => {
            let Some(cp) = get(impl_row, column).and_then(int) else {
                return Ok(None);
            };
            if cat.table("ControlPoint").is_none() || cat.table("ControlPointImpl").is_none() {
                return Ok(None);
            }
            let Some(cp_impl) = lookup_int(conn, "ControlPoint", "impl_id", cp)? else {
                return Ok(None);
            };
            let v: Option<Sql> = conn
                .prepare_cached(&format!(
                    "SELECT {} FROM ControlPointImpl WHERE id = ?1",
                    quote(&format!("coordinates_{axis}"))
                ))?
                .query_row([cp_impl], |r| r.get(0))
                .optional()?;
            Ok(v)
        }
        Derived::OpeningSurfaces => {
            // Reverse lookup: which surfaces contain this opening?
            // SurfaceSpecificImplPart_integratedOpenings.value_id = opening impl_id
            // → SurfaceSpecificImplPart.owner_id = surface impl_id → Surface.
            if cat.table("SurfaceSpecificImplPart_integratedOpenings").is_none()
                || cat.table("SurfaceSpecificImplPart").is_none()
            {
                return Ok(None);
            }
            let sql = "SELECT s.userID FROM SurfaceSpecificImplPart_integratedOpenings io \
                        JOIN SurfaceSpecificImplPart sp ON sp.id = io.id \
                        JOIN Surface s ON s.impl_id = sp.owner_id \
                        WHERE io.value_id = ?1 ORDER BY s.userID";
            let mut stmt = conn.prepare_cached(sql)?;
            let ids: Vec<u32> = stmt
                .query_map([impl_id], |r| r.get::<_, i64>(0))?
                .filter_map(|r| r.ok().and_then(|v| u32::try_from(v).ok()))
                .collect();
            if ids.is_empty() {
                return Ok(None);
            }
            let txt = ids
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            Ok(Some(Sql::Text(txt)))
        }
        Derived::MaterialProperty(key) => {
            let vector = format!("{impl_name}_materialDataVector");
            if cat.table(&vector).is_none() || cat.table("MaterialData").is_none() {
                return Ok(None);
            }
            let sql = format!(
                "SELECT reference_id FROM {} WHERE id = ?1 ORDER BY container_order LIMIT 1",
                quote(&vector)
            );
            let data: Option<i64> = conn
                .prepare_cached(&sql)?
                .query_row([impl_id], |r| r.get(0))
                .optional()?;
            let Some(data) = data else { return Ok(None) };
            let Some(props) = lookup_int(conn, "MaterialData", "properties_id", data)? else {
                return Ok(None);
            };
            map_value(conn, cat, "MaterialProperties", "propertiesMap", props, key)
        }
    }
}

/// Converts a stored value; references stay raw until resolved.
fn from_sql(ty: FieldType, v: &Sql) -> Value {
    match (ty, v) {
        (_, Sql::Null) => Value::Null,
        (FieldType::Bool, Sql::Text(t)) => Value::Bool(t.eq_ignore_ascii_case("true") || t == "1"),
        (FieldType::Bool, v) => int(v).map_or(Value::Null, |i| Value::Bool(i != 0)),
        (FieldType::Int, v) => int(v).map_or(Value::Null, Value::Int),
        (FieldType::Float(_), Sql::Real(r)) => Value::Float(*r),
        (FieldType::Float(_), Sql::Integer(i)) => Value::Float(*i as f64),
        (FieldType::Float(_), Sql::Text(t)) => t.trim().parse().map_or(Value::Null, Value::Float),
        (FieldType::Text | FieldType::Enum, Sql::Text(t)) => Value::Text(t.clone()),
        (FieldType::Text | FieldType::Enum, v) => int(v).map_or(Value::Null, Value::Int),
        (FieldType::Ref(_), v) => match int(v) {
            Some(i) if i > 0 => Value::Ref(i as u32),
            _ => Value::Null,
        },
        (FieldType::RefList(_), Sql::Text(t)) => {
            list::parse_list(t).map_or(Value::Null, Value::RefList)
        }
        (FieldType::RefList(_), v) => match int(v) {
            Some(i) if i > 0 => Value::RefList(vec![i as u32]),
            _ => Value::Null,
        },
        _ => Value::Null,
    }
}

const META_COLUMNS: &[&str] = &["id", "rowid", "impl_id", "impl_table"];

struct Raw {
    kind: ObjectKind,
    db_id: i64,
    no: u32,
    load_case: Option<u32>,
    obj: Object,
    binding: Binding,
}

impl RfDocument {
    /// Opens an `.rf6`, a bare database file or an unpacked folder.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::from_archive(Archive::open(path)?)
    }

    /// Main table and database key of a loaded object.
    pub fn db_key(&self, id: ObjectId) -> Option<(&str, i64)> {
        self.bindings.get(&id).map(|b| (b.table.as_str(), b.key))
    }

    /// Loads the model from an already opened archive (for example one prepared by
    /// [`crate::build`]).
    pub fn from_archive(archive: Archive) -> Result<Self, Error> {
        let (raws, report) = Self::read(&archive)?;
        let mut doc = Self {
            archive,
            model: Model::new(),
            report,
            bindings: HashMap::new(),
            ref_by_id: HashMap::new(),
            ids: HashMap::new(),
            prototype_files: Vec::new(),
            db_changed: false,
        };
        doc.resolve(raws);
        Ok(doc)
    }

    /// Whether there is anything to save: model edits or library items assigned.
    pub fn is_modified(&self) -> bool {
        self.model.is_modified() || self.db_changed
    }

    /// Makes material / section `id` the library item `item` ([`crate::library`]): the item's
    /// rows are copied from the library database and the object refers to them (members and
    /// surfaces keep referring to the object; a section keeps its material). Pending edits are
    /// written to the working database first and the model is reloaded, so the object shows
    /// the library values. Saved with the next [`RfDocument::save`].
    pub fn assign_library_item(
        &mut self,
        id: ObjectId,
        lib: &crate::Library,
        item: &crate::LibraryItem,
    ) -> Result<(), Error> {
        use crate::LibraryKind as L;
        let main = match (id.kind, item.kind) {
            (ObjectKind::Material, L::Material) => "Material",
            (ObjectKind::Section, L::Section) => "Section",
            _ => {
                return Err(Error::Unsupported(format!(
                    "a library {} for a {}",
                    item.kind.name(),
                    id.kind.name()
                )));
            }
        };
        // As saving does: new objects get rows, edits are written (the reload keeps them).
        self.insert_new_objects()?;
        self.apply_edits()?;
        let (table, key) = self
            .db_key(id)
            .map(|(t, k)| (t.to_owned(), k))
            .ok_or_else(|| {
                Error::Format(format!(
                    "{} {} is not in the database",
                    id.kind.name(),
                    id.no
                ))
            })?;
        let lib_conn =
            Connection::open_with_flags(lib.db_path(item.kind), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let lib_schema = crate::build::Schema::read(&lib_conn)?;
        let lib_main: i64 = lib_conn.query_row(
            &format!("SELECT id FROM {} WHERE userID = ?1", quote(main)),
            [item.user_id],
            |r| r.get(0),
        )?;
        let proto =
            crate::build::capture_library_item(&lib_conn, &lib_schema, id.kind, main, lib_main)?;
        let mut b = crate::build::Builder::on_db(self.archive.db_path())?;
        b.begin()?;
        b.replace_with(&proto, &table, key)?;
        b.commit()?;
        self.reload()?;
        self.db_changed = true;
        Ok(())
    }

    /// Loads the model again from the working database (after rows were inserted).
    fn reload(&mut self) -> Result<(), Error> {
        let (raws, report) = Self::read(&self.archive)?;
        self.model = Model::new();
        self.report = report;
        self.bindings.clear();
        self.ref_by_id.clear();
        self.ids.clear();
        self.resolve(raws);
        Ok(())
    }

    fn read(archive: &Archive) -> Result<(Vec<Raw>, LoadReport), Error> {
        let conn =
            Connection::open_with_flags(archive.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let cat = Catalog::read(&conn)?;
        let mut cache = Cache::default();
        let mut raws = Vec::new();
        let mut report = LoadReport::default();

        for &kind in ObjectKind::ALL {
            let spec = kind.spec();
            let Some(table) = mapping::tables(kind).iter().find_map(|n| cat.table(n)) else {
                report.kinds.push(KindReport {
                    kind,
                    table: None,
                    objects: 0,
                    missing_fields: vec![],
                    unmapped_columns: vec![],
                });
                continue;
            };
            let (rows, key_col) = read_rows(&conn, &table.name)?;
            let mut found: HashSet<&'static str> = HashSet::new();
            let mut used: HashSet<String> = META_COLUMNS.iter().map(|s| s.to_string()).collect();
            used.extend(mapping::NUMBER_COLUMNS.iter().map(|s| s.to_string()));
            if spec.is_load() {
                used.extend(mapping::LOAD_CASE_COLUMNS.iter().map(|s| s.to_string()));
            }
            let mut all_cols: BTreeSet<String> =
                table.columns.iter().map(|c| c.name.clone()).collect();

            for row in &rows {
                let Some(db_id) = get(row, key_col).and_then(int) else {
                    continue;
                };
                let no = find(row, mapping::NUMBER_COLUMNS)
                    .and_then(|(_, v)| int(v))
                    .unwrap_or(db_id);
                let impl_row = match (get(row, "impl_table"), get(row, "impl_id").and_then(int)) {
                    (Some(Sql::Text(t)), Some(iid)) if cat.table(t).is_some() => {
                        let name = cat.table(t).unwrap().name.clone();
                        cache
                            .row(&conn, &name, iid)?
                            .map(|(r, kc)| (name, kc, iid, r))
                    }
                    _ => None,
                };
                if let Some((name, ..)) = &impl_row {
                    all_cols.extend(
                        cat.table(name)
                            .unwrap()
                            .columns
                            .iter()
                            .map(|c| c.name.clone()),
                    );
                }

                let mut obj = Object::default();
                let mut locs = HashMap::new();
                for fs in spec.fields {
                    if let Some(t) = mapping::field_impl_table(kind, fs.key) {
                        if !impl_row
                            .as_ref()
                            .is_some_and(|(n, ..)| n.eq_ignore_ascii_case(t))
                        {
                            continue;
                        }
                    }
                    let cands = mapping::column_candidates(kind, fs.key);
                    let hit = find(row, &cands)
                        .map(|(c, v)| (Loc::Main(c.to_owned()), v.clone()))
                        .or_else(|| {
                            let (_, _, _, r) = impl_row.as_ref()?;
                            find(r, &cands).map(|(c, v)| (Loc::Impl(c.to_owned()), v.clone()))
                        });
                    let (loc, value) = match hit {
                        Some((loc, v)) => {
                            if let Loc::Main(c) | Loc::Impl(c) = &loc {
                                used.insert(c.to_lowercase());
                            }
                            (loc, from_sql(fs.ty, &v))
                        }
                        None => match (fs.ty, &impl_row) {
                            (_, Some((n, _, iid, _)))
                                if mapping::part_column(kind, fs.key).is_some() =>
                            {
                                let part = mapping::part_column(kind, fs.key).unwrap();
                                let owner = match part.via {
                                    None => Some(*iid),
                                    Some(c) => get(&impl_row.as_ref().unwrap().3, c).and_then(int),
                                };
                                let Some(owner) = owner else { continue };
                                let Some((table, rowid, v)) =
                                    part_value(&conn, &cat, part.table, part.column, n, owner)?
                                else {
                                    continue;
                                };
                                let loc = Loc::Part {
                                    table,
                                    rowid,
                                    column: part.column.to_owned(),
                                };
                                (loc, from_sql(fs.ty, &v))
                            }
                            (_, Some((n, _, iid, r)))
                                if mapping::derived(kind, fs.key).is_some() =>
                            {
                                let d = mapping::derived(kind, fs.key).unwrap();
                                let Some(v) = derived_value(&conn, &cat, d, n, *iid, r)? else {
                                    continue;
                                };
                                (Loc::Derived, from_sql(fs.ty, &v))
                            }
                            (FieldType::RefList(_), _) => {
                                let mut bases = vec![(table.name.clone(), db_id)];
                                if let Some((n, _, iid, _)) = &impl_row {
                                    bases.push((n.clone(), *iid));
                                }
                                let child = bases.iter().find_map(|(base, owner)| {
                                    cands.iter().find_map(|c| {
                                        let t = cat.table(&format!("{base}_{c}"))?;
                                        t.is_child().then(|| (t.name.clone(), *owner))
                                    })
                                });
                                let Some((ct, owner)) = child else { continue };
                                let items: Vec<u32> = cache
                                    .child(&conn, &cat, &ct, owner)?
                                    .iter()
                                    .filter_map(int)
                                    .filter_map(|i| u32::try_from(i).ok())
                                    .collect();
                                (Loc::Child { table: ct, owner }, Value::RefList(items))
                            }
                            (_, Some((n, ..)))
                                if matches!(fs.key, "type" | "load_type" | "geometry") =>
                            {
                                let prefix = match fs.key {
                                    "type" => "TYPE",
                                    "load_type" => "LOAD_TYPE",
                                    _ => "GEOMETRY",
                                };
                                let Some(t) = type_from_impl(prefix, n) else {
                                    continue;
                                };
                                (Loc::ImplType, Value::Text(t))
                            }
                            _ => continue,
                        },
                    };
                    found.insert(fs.key);
                    locs.insert(fs.key, loc);
                    obj.values.insert(fs.key, value);
                }

                let load_case = spec.is_load().then(|| {
                    find(row, mapping::LOAD_CASE_COLUMNS)
                        .or_else(|| find(&impl_row.as_ref()?.3, mapping::LOAD_CASE_COLUMNS))
                        .and_then(|(_, v)| int(v))
                        .map_or(0, |i| i as u32)
                });
                raws.push(Raw {
                    kind,
                    db_id,
                    no: no as u32,
                    load_case,
                    binding: Binding {
                        table: table.name.clone(),
                        key_col,
                        key: db_id,
                        impl_row: impl_row.map(|(n, kc, iid, _)| (n, kc, iid)),
                        locs,
                        loaded: Object::default(),
                    },
                    obj,
                });
            }

            report.kinds.push(KindReport {
                kind,
                table: Some(table.name.clone()),
                objects: rows.len(),
                missing_fields: spec
                    .fields
                    .iter()
                    .map(|f| f.key)
                    .filter(|k| !found.contains(k))
                    .collect(),
                unmapped_columns: all_cols
                    .into_iter()
                    .filter(|c| !used.contains(&c.to_lowercase()))
                    .collect(),
            });
        }

        Ok((raws, report))
    }

    /// Converts references to object numbers, deciding per field whether the database
    /// stores ids or numbers, then fills the model.
    fn resolve(&mut self, mut raws: Vec<Raw>) {
        let mut id_to_no: HashMap<ObjectKind, HashMap<i64, u32>> = HashMap::new();
        let mut nos: HashMap<ObjectKind, HashSet<u32>> = HashMap::new();
        for r in &raws {
            id_to_no.entry(r.kind).or_default().insert(r.db_id, r.no);
            nos.entry(r.kind).or_default().insert(r.no);
            self.ids.entry(r.kind).or_default().insert(r.no, r.db_id);
        }
        let empty_ids = HashMap::new();
        let empty_nos = HashSet::new();

        // Count for each reference field how many values match an id vs a number.
        let mut votes: HashMap<(ObjectKind, &'static str), (usize, usize)> = HashMap::new();
        let mut vote = |key, target: ObjectKind, vals: &[u32]| {
            let ids = id_to_no.get(&target).unwrap_or(&empty_ids);
            let ns = nos.get(&target).unwrap_or(&empty_nos);
            let e = votes.entry(key).or_insert((0, 0));
            e.0 += vals
                .iter()
                .filter(|v| ids.contains_key(&(**v as i64)))
                .count();
            e.1 += vals.iter().filter(|v| ns.contains(v)).count();
        };
        for r in &raws {
            for fs in r.kind.spec().fields {
                match (fs.ty, r.obj.get(fs.key)) {
                    (FieldType::Ref(t), Value::Ref(n)) => vote((r.kind, fs.key), t, &[*n]),
                    (FieldType::RefList(t), Value::RefList(l)) => vote((r.kind, fs.key), t, l),
                    _ => {}
                }
            }
            if let Some(lc) = r.load_case {
                vote((r.kind, "load_case"), ObjectKind::LoadCase, &[lc]);
            }
        }
        self.ref_by_id = votes
            .into_iter()
            // RFEM stores db ids; ties (ids equal to numbers) and fields with no values
            // (e.g. an empty list, filled in later by an edit) count as ids.
            .map(|(k, (by_id, by_no))| (k, by_id >= by_no))
            .collect();

        let map = |by_id: bool, target: ObjectKind, n: u32| -> u32 {
            if by_id {
                id_to_no
                    .get(&target)
                    .and_then(|m| m.get(&(n as i64)))
                    .copied()
                    .unwrap_or(n)
            } else {
                n
            }
        };
        for r in &mut raws {
            for fs in r.kind.spec().fields {
                let by_id = self
                    .ref_by_id
                    .get(&(r.kind, fs.key))
                    .copied()
                    .unwrap_or(true);
                match (fs.ty, r.obj.values.get_mut(fs.key)) {
                    (FieldType::Ref(t), Some(Value::Ref(n))) => *n = map(by_id, t, *n),
                    (FieldType::RefList(t), Some(Value::RefList(l))) => {
                        l.iter_mut().for_each(|n| *n = map(by_id, t, *n))
                    }
                    _ => {}
                }
            }
            let lc = r.load_case.map_or(0, |lc| {
                let by_id = self
                    .ref_by_id
                    .get(&(r.kind, "load_case"))
                    .copied()
                    .unwrap_or(true);
                map(by_id, ObjectKind::LoadCase, lc)
            });
            let id = ObjectId::load(r.kind, lc, r.no);
            if self.model.get(id).is_some() {
                // Keep the first; editing the wrong row would be worse than hiding one.
                self.report
                    .warnings
                    .push(format!("duplicate {id:?} (db id {}) skipped", r.db_id));
                continue;
            }
            r.binding.loaded = r.obj.clone();
            self.bindings.insert(id, r.binding.clone());
            self.model.insert_loaded(id, std::mem::take(&mut r.obj));
        }
    }

    /// Objects added in the model get database rows: each is a copy of a matching object (from
    /// this model, else from [`Self::prototype_files`]) with a new number. The model is then
    /// reloaded and the typed values set again, so [`Self::apply_edits`] writes them; fields left
    /// empty keep the copied values, except references, which are cleared.
    fn insert_new_objects(&mut self) -> Result<(), Error> {
        let mut new: Vec<ObjectId> = self
            .model
            .dirty()
            .iter()
            .filter(|id| !self.bindings.contains_key(id))
            .copied()
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        // Load cases first: loads need their database ids.
        new.sort_by_key(|id| (id.kind != ObjectKind::LoadCase, *id));
        // One transaction: nothing is inserted unless every new object can be.
        let mut b = crate::build::Builder::on_db(self.archive.db_path())?;
        let mut donors: Vec<Option<RfDocument>> = Vec::new();
        let files = self.prototype_files.clone();
        // A copy must suit this database (a file from another RFEM version may lack columns).
        let mut why: Vec<String> = Vec::new();
        let pick = |b: &crate::build::Builder,
                    donors: &mut Vec<Option<RfDocument>>,
                    why: &mut Vec<String>,
                    own: Option<&RfDocument>,
                    get: &dyn Fn(&RfDocument) -> Option<crate::build::Proto>|
         -> Option<crate::build::Proto> {
            let fits =
                |p: crate::build::Proto, from: &str, why: &mut Vec<String>| match b.misfit(&p) {
                    None => Some(p),
                    Some(m) => {
                        why.push(format!("{from}: {m}"));
                        None
                    }
                };
            // The model's own objects match its database; else the prototype with the fewest
            // missing columns (none: same RFEM version), missing ones getting defaults.
            if let Some(own) = own {
                match get(own) {
                    Some(p) => {
                        if let Some(p) = fits(p, "this model", why) {
                            return Some(p);
                        }
                    }
                    None => why.push("this model: none".into()),
                }
            }
            if files.is_empty() {
                why.push("no prototype files given".into());
            }
            let mut best: Option<(usize, crate::build::Proto)> = None;
            for (i, f) in files.iter().enumerate() {
                let name = f
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().into());
                if donors.len() <= i {
                    donors.push(match RfDocument::open(f) {
                        Ok(d) => Some(d),
                        Err(e) => {
                            why.push(format!("{name}: could not open ({e})"));
                            None
                        }
                    });
                }
                let Some(d) = donors[i].as_ref() else {
                    continue;
                };
                match get(d) {
                    Some(p) => {
                        if let Some(p) = fits(p, &name, why) {
                            let missing = b.missing_columns(&p);
                            if missing == 0 {
                                return Some(p);
                            }
                            if best.as_ref().is_none_or(|(m, _)| missing < *m) {
                                best = Some((missing, p));
                            }
                        }
                    }
                    None => why.push(format!("{name}: none")),
                }
            }
            best.map(|(_, p)| p)
        };
        // Keyed by kind and prototype filter (a nodal load with components needs another copy).
        let mut protos: HashMap<(ObjectKind, String), crate::build::Proto> = HashMap::new();
        let key_of = |id: &ObjectId, model: &Model| {
            let want = prototype_filter(id.kind, model.get(*id).expect("new objects exist"));
            ((id.kind, format!("{want:?}")), want)
        };
        for id in &new {
            let (key, want) = key_of(id, &self.model);
            if protos.contains_key(&key) {
                continue;
            }
            why.clear();
            let get = |d: &RfDocument| crate::build::find_proto(d, id.kind, &want).ok();
            let p = pick(&b, &mut donors, &mut why, Some(&*self), &get).ok_or_else(|| {
                Error::Unsupported(format!(
                    "adding {}: no usable example in this model or the prototype files{}",
                    id.kind.name(),
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", why.join("; "))
                    }
                ))
            })?;
            protos.insert(key, p);
        }
        // New surfaces need the objects RFEM makes with the first surface.
        let mut extra = Vec::new();
        if new.iter().any(|id| id.kind == ObjectKind::Surface) {
            for table in crate::build::SURFACE_SUPPORT_TABLES {
                if b.count_rows(table)? > 0 {
                    continue;
                }
                why.clear();
                let get = |d: &RfDocument| crate::build::first_of_table(d, table).ok().flatten();
                let p = pick(&b, &mut donors, &mut why, None, &get).ok_or_else(|| {
                    Error::Unsupported(format!(
                        "adding surfaces: no usable {table} in the prototype files{}",
                        if why.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", why.join("; "))
                        }
                    ))
                })?;
                extra.push(p);
            }
        }
        drop(donors);

        let mut lc_ids = self
            .ids
            .get(&ObjectKind::LoadCase)
            .cloned()
            .unwrap_or_default();
        {
            b.begin()?;
            for p in &extra {
                b.add(p, 1, None)?;
            }
            for id in &new {
                let lc = if id.kind.spec().is_load() {
                    Some(*lc_ids.get(&id.load_case).ok_or_else(|| {
                        Error::Unsupported(format!(
                            "{} {}: load case {} does not exist",
                            id.kind.name(),
                            id.no,
                            id.load_case
                        ))
                    })?)
                } else {
                    None
                };
                let main = b.add(&protos[&key_of(id, &self.model).0], id.no as i64, lc)?;
                match id.kind {
                    ObjectKind::Surface => b.make_surface_standard(main)?,
                    ObjectKind::LoadCase => {
                        lc_ids.insert(id.no, main);
                    }
                    _ => {}
                }
            }
            // References a copy brought from another file that point nowhere.
            b.clear_dangling_references()?;
            b.commit()?;
        }

        let old = std::mem::replace(&mut self.model, Model::new());
        self.reload()?;
        for id in old.dirty() {
            let (Some(o), Some(fresh)) = (old.get(*id), self.model.get(*id).cloned()) else {
                continue;
            };
            let is_new = new.contains(id);
            for f in id.kind.spec().fields {
                if !fresh.values.contains_key(f.key) {
                    continue;
                }
                let v = match (o.get(f.key), f.ty) {
                    (Value::Null, FieldType::Ref(_)) if is_new => Value::Null,
                    (Value::Null, FieldType::RefList(_)) if is_new => Value::RefList(Vec::new()),
                    (Value::Null, _) => continue,
                    (v, _) => v.clone(),
                };
                if fresh.get(f.key) != &v {
                    self.model
                        .set(*id, f.key, v)
                        .map_err(|e| Error::Format(format!("{id:?}.{}: {e}", f.key)))?;
                }
            }
        }
        Ok(())
    }

    /// Writes edited fields into the working database. All-or-nothing.
    fn apply_edits(&mut self) -> Result<(), Error> {
        if !self.model.is_modified() {
            return Ok(());
        }
        if let Some(id) = self.model.removed().iter().next() {
            return Err(Error::Unsupported(format!("deleting objects ({id:?})")));
        }
        let mut conn = Connection::open(self.archive.db_path())?;
        let tx = conn.transaction()?;
        for id in self.model.dirty() {
            let b = self
                .bindings
                .get(id)
                .ok_or_else(|| Error::Unsupported(format!("adding new objects ({id:?})")))?;
            let obj = self.model.get(*id).expect("dirty objects exist");
            for (key, val) in &obj.values {
                if b.loaded.get(key) == val {
                    continue;
                }
                let field = id.kind.spec().field(key).expect("model checks fields");
                let (table, key_col, key_val, col) = match b.locs.get(key) {
                    Some(Loc::Main(c)) => (&b.table, b.key_col, b.key, c),
                    Some(Loc::Impl(c)) => {
                        let (t, kc, k) = b.impl_row.as_ref().expect("impl loc has impl row");
                        (t, *kc, *k, c)
                    }
                    Some(Loc::Child { table, owner }) => {
                        let FieldType::RefList(target) = field.ty else {
                            return Err(Error::Unsupported(format!("list {key}")));
                        };
                        self.write_list(&tx, table, *owner, target, field.key, id.kind, val)?;
                        continue;
                    }
                    Some(Loc::ImplType) => {
                        return Err(Error::Unsupported(format!(
                            "changing {} type",
                            id.kind.name()
                        )));
                    }
                    Some(Loc::Part {
                        table,
                        rowid,
                        column,
                    }) => (table, "rowid", *rowid, column),
                    Some(Loc::Derived) => {
                        return Err(Error::Unsupported(format!(
                            "editing derived property {}.{key}",
                            id.kind.name()
                        )));
                    }
                    None => {
                        return Err(Error::Unsupported(format!(
                            "{}.{key} has no known database column",
                            id.kind.name()
                        )));
                    }
                };
                let sql_val = self.to_sql(id.kind, field.key, field.ty, val)?;
                tx.execute(
                    &format!(
                        "UPDATE {} SET {} = ?1 WHERE {key_col} = ?2",
                        quote(table),
                        quote(col)
                    ),
                    rusqlite::params![sql_val, key_val],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Replaces the rows of an RFEM list table (`id` = owner, `container_order`, value in
    /// `reference_id` (+ `reference_table`) or `value_id`).
    #[allow(clippy::too_many_arguments)]
    fn write_list(
        &self,
        conn: &Connection,
        table: &str,
        owner: i64,
        target: ObjectKind,
        key: &'static str,
        kind: ObjectKind,
        val: &Value,
    ) -> Result<(), Error> {
        let cols: Vec<String> = conn
            .prepare(&format!(
                "SELECT name FROM pragma_table_info({})",
                quote_str(table)
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let has = |c: &str| cols.iter().any(|x| x.eq_ignore_ascii_case(c));
        if !has("container_order") {
            return Err(Error::Unsupported(format!("writing list table {table}")));
        }
        let (value_col, table_col) = if has("reference_id") {
            ("reference_id", "reference_table")
        } else if has("object_id") {
            ("object_id", "object_table")
        } else if has("value_id") {
            ("value_id", "")
        } else {
            return Err(Error::Unsupported(format!(
                "list table {table} has no id column"
            )));
        };
        let items = val.as_ref_list();
        let ids = items
            .iter()
            .map(|n| self.db_id(kind, key, target, *n))
            .collect::<Result<Vec<_>, _>>()?;
        conn.execute(
            &format!("DELETE FROM {} WHERE id = ?1", quote(table)),
            [owner],
        )?;
        let target_table = mapping::tables(target)[0];
        for (i, db) in ids.iter().enumerate() {
            if !table_col.is_empty() && has(table_col) {
                conn.execute(
                    &format!(
                        "INSERT INTO {} (id, container_order, {value_col}, {table_col}) \
                         VALUES (?1, ?2, ?3, ?4)",
                        quote(table)
                    ),
                    rusqlite::params![owner, i as i64, db, target_table],
                )?;
            } else {
                conn.execute(
                    &format!(
                        "INSERT INTO {} (id, container_order, {value_col}) VALUES (?1, ?2, ?3)",
                        quote(table)
                    ),
                    rusqlite::params![owner, i as i64, db],
                )?;
            }
        }
        Ok(())
    }

    /// Database id stored for a reference from `kind.key` to object `n` of `target`.
    fn db_id(
        &self,
        kind: ObjectKind,
        key: &'static str,
        target: ObjectKind,
        n: u32,
    ) -> Result<i64, Error> {
        if !self.ref_by_id.get(&(kind, key)).copied().unwrap_or(true) {
            return Ok(n as i64);
        }
        self.ids
            .get(&target)
            .and_then(|m| m.get(&n))
            .copied()
            .ok_or_else(|| {
                Error::Unsupported(format!("{} {n} is not stored in the file", target.name()))
            })
    }

    fn to_sql(
        &self,
        kind: ObjectKind,
        key: &'static str,
        ty: FieldType,
        v: &Value,
    ) -> Result<Sql, Error> {
        let lookup = |target: ObjectKind, n: u32| self.db_id(kind, key, target, n);
        Ok(match (ty, v) {
            (_, Value::Null) => Sql::Null,
            (_, Value::Bool(b)) => Sql::Integer(*b as i64),
            (_, Value::Int(i)) => Sql::Integer(*i),
            (_, Value::Float(f)) => Sql::Real(*f),
            (_, Value::Text(s)) => Sql::Text(s.clone()),
            (FieldType::Ref(t), Value::Ref(n)) => Sql::Integer(lookup(t, *n)?),
            (FieldType::RefList(t), Value::RefList(l)) => {
                let ids = l
                    .iter()
                    .map(|n| lookup(t, *n).map(|i| i as u32))
                    .collect::<Result<Vec<_>, _>>()?;
                match ids.as_slice() {
                    [one] => Sql::Integer(*one as i64),
                    _ => Sql::Text(list::format_list(&ids)),
                }
            }
            (_, other) => Sql::Text(other.to_string()),
        })
    }

    /// FE mesh saved by RFEM (`!solver/M/`), if the file holds one. See [`crate::mesh`].
    pub fn fe_mesh(&self) -> Result<Option<crate::FeMesh>, Error> {
        crate::FeMesh::read(&self.archive)
    }

    /// Saves to `path`: `.rf6` sources are re-zipped with unchanged sidecars; bare databases are
    /// copied. Unchanged models give a byte-identical `model.db`.
    pub fn save(&mut self, path: &Path) -> Result<(), Error> {
        // Mesh and results under `!solver/` belong to the unedited model.
        if self.is_modified() && !self.archive.drop_prefixes.iter().any(|p| p == "!solver/") {
            self.archive.drop_prefixes.push("!solver/".into());
        }
        self.insert_new_objects()?;
        self.apply_edits()?;
        match self.archive.format {
            SourceFormat::Rf6 => self.archive.save_rf6(path)?,
            SourceFormat::SqliteFile => self.archive.save_db(path)?,
            SourceFormat::Folder => return Err(Error::Unsupported("saving to a folder".into())),
        }
        for id in self.model.dirty().clone() {
            if let (Some(b), Some(o)) = (self.bindings.get_mut(&id), self.model.get(id)) {
                b.loaded = o.clone();
            }
        }
        self.model.clear_changes();
        self.db_changed = false;
        Ok(())
    }
}

/// Which object a new object of `kind` is copied from (the simplest type of each kind).
fn prototype_filter(kind: ObjectKind, obj: &Object) -> Vec<(&'static str, Value)> {
    use ObjectKind as K;
    let t = |k: &'static str, v: &str| vec![(k, Value::Text(v.into()))];
    match kind {
        K::Node => t("type", "TYPE_STANDARD"),
        // A curve (arc, circle, ...) needs a copy of its own type: the type is not editable.
        K::Line => match obj.get("type") {
            Value::Text(ty) => t("type", ty),
            _ => t("type", "TYPE_POLYLINE"),
        },
        K::Member => t("type", "TYPE_BEAM"),
        K::Surface => t("geometry", "GEOMETRY_PLANE"),
        K::Thickness => t("type", "TYPE_UNIFORM"),
        // Nodal loads by components (PX ... MZ) are their own load type.
        K::NodalLoad
            if obj
                .values
                .iter()
                .any(|(k, v)| k.starts_with("components_") && !matches!(v, Value::Null)) =>
        {
            t("load_type", "LOAD_TYPE_COMPONENTS")
        }
        k if k.spec().is_load() && k != K::LoadCase => t("load_type", "LOAD_TYPE_FORCE"),
        _ => Vec::new(),
    }
}
