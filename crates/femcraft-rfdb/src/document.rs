//! Loading the model tables from `model.db` and writing edits back.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use femcraft_model::{FieldType, Model, Object, ObjectId, ObjectKind, Value, list};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};

use crate::Error;
use crate::archive::{Archive, SourceFormat};
use crate::catalog::{Catalog, quote};
use crate::mapping::{self, CHILD_META_COLUMNS, CHILD_ORDER_COLUMNS};

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
}

impl std::fmt::Display for LoadReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for k in &self.kinds {
            match &k.table {
                None => writeln!(f, "{:<26} no table", k.kind.name())?,
                Some(t) => {
                    writeln!(f, "{:<26} {t}: {} objects", k.kind.name(), k.objects)?;
                    if !k.missing_fields.is_empty() {
                        writeln!(f, "    missing: {}", k.missing_fields.join(", "))?;
                    }
                    if !k.unmapped_columns.is_empty() {
                        writeln!(f, "    unmapped: {}", k.unmapped_columns.join(", "))?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Where a field value was read from.
#[derive(Debug, Clone)]
enum Loc {
    Main(String),
    Impl(String),
    /// List held in a child table (read-only for now).
    Child,
    /// Object type derived from the impl table name (read-only).
    ImplType,
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
            let value_col = info
                .columns
                .iter()
                .find(|c| {
                    !CHILD_META_COLUMNS
                        .iter()
                        .any(|m| c.name.eq_ignore_ascii_case(m))
                })
                .map(|c| c.name.clone());
            let order = CHILD_ORDER_COLUMNS
                .iter()
                .find_map(|o| info.column(o))
                .map_or("rowid".to_owned(), |c| quote(&c.name));
            let mut map: HashMap<i64, Vec<Sql>> = HashMap::new();
            if let Some(vc) = value_col {
                let sql = format!(
                    "SELECT owner_id, {} FROM {} ORDER BY owner_id, {order}",
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

/// `NodeImplStandard` on table `Node` -> `TYPE_STANDARD`.
fn type_from_impl(main: &str, impl_table: &str) -> String {
    let rest = impl_table.strip_prefix(main).unwrap_or(impl_table);
    let rest = rest.strip_prefix("Impl").unwrap_or(rest);
    let mut out = String::from("TYPE");
    for c in rest.chars() {
        if c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
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
        let archive = Archive::open(path)?;
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
                                (Loc::Child, Value::RefList(items))
                            }
                            (_, Some((n, ..))) if fs.key == "type" => {
                                (Loc::ImplType, Value::Text(type_from_impl(&table.name, n)))
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

        let mut doc = Self {
            archive,
            model: Model::new(),
            report,
            bindings: HashMap::new(),
            ref_by_id: HashMap::new(),
            ids: HashMap::new(),
        };
        doc.resolve(raws);
        Ok(doc)
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
            .map(|(k, (by_id, by_no))| (k, by_id > by_no))
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
                    .unwrap_or(false);
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
                    .unwrap_or(false);
                map(by_id, ObjectKind::LoadCase, lc)
            });
            let id = ObjectId::load(r.kind, lc, r.no);
            r.binding.loaded = r.obj.clone();
            self.bindings.insert(id, r.binding.clone());
            self.model.insert_loaded(id, std::mem::take(&mut r.obj));
        }
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
                    Some(Loc::Child) => {
                        return Err(Error::Unsupported(format!(
                            "editing list field {}.{key}",
                            id.kind.name()
                        )));
                    }
                    Some(Loc::ImplType) => {
                        return Err(Error::Unsupported(format!(
                            "changing {} type",
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

    fn to_sql(
        &self,
        kind: ObjectKind,
        key: &'static str,
        ty: FieldType,
        v: &Value,
    ) -> Result<Sql, Error> {
        let by_id = self.ref_by_id.get(&(kind, key)).copied().unwrap_or(false);
        let lookup = |target: ObjectKind, n: u32| -> Result<i64, Error> {
            if !by_id {
                return Ok(n as i64);
            }
            self.ids
                .get(&target)
                .and_then(|m| m.get(&n))
                .copied()
                .ok_or_else(|| {
                    Error::Unsupported(format!("{} {n} is not stored in the file", target.name()))
                })
        };
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

    /// Saves to `path`: `.rf6` sources are re-zipped with unchanged sidecars; bare databases are
    /// copied. Unchanged models give a byte-identical `model.db`.
    pub fn save(&mut self, path: &Path) -> Result<(), Error> {
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
        Ok(())
    }
}
