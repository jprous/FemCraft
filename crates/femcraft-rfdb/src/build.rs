//! Building new RFEM models.
//!
//! RFEM needs far more than the object tables: analysis settings, layers, standards, design
//! configurations and so on. A new model therefore starts from a *template*: a real `.rf6`
//! with every model object purged (rows of the object tables plus everything they own) and
//! all settings kept. Objects are then created by cloning *prototype* row trees captured from
//! real files (main row, impl row, owned part rows, value lists) with fresh ids and GUIDs; the
//! field values are written afterwards through [`RfDocument`] like any edit.
//!
//! Row ownership as found in RFEM 6.16:
//! - `impl_id` / `impl_table` on a main row -> its impl row;
//! - `owner_id` / `owner_table` on part rows (`SpringConstants`, `SurfaceSpecificImplPart`, ...);
//! - list tables `<Owner>_<name>` with `id` = owner id and `container_order`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use femcraft_model::{ObjectId, ObjectKind, Value};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};

use crate::archive::Archive;
use crate::catalog::quote;
use crate::document::RfDocument;
use crate::{Error, mapping};

type Row = Vec<(String, Sql)>;

fn get<'a>(row: &'a Row, name: &str) -> Option<&'a Sql> {
    row.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

fn int(v: Option<&Sql>) -> Option<i64> {
    match v? {
        Sql::Integer(i) => Some(*i),
        _ => None,
    }
}

/// Object tables removed from a template besides the FemCraft kinds (they refer to model
/// objects). Materials and sections are kept: they come from RFEM's libraries.
pub const EXTRA_PURGE_TABLES: &[&str] = &[
    "LoadCombination",
    "ResultCombination",
    "MemberRepresentative",
    "ShrinkWrapping",
    // Kept: `ReinforcementDirection`, `ConcreteExposureCondition` - standalone objects that
    // concrete surfaces must refer to (RFEM crashed on surfaces without them).
    "MemberSet",
    "LineSet",
    "SurfaceSet",
    "SolidSet",
    "Opening",
    "MemberHinge",
    "LineHinge",
    "NodalRelease",
    "LineRelease",
    "SurfaceRelease",
    "MemberEccentricity",
    "MemberNonlinearity",
    "SurfaceReinforcement",
    "MemberSetLoad",
    "SurfaceSetLoad",
    "SolidSetLoad",
];

/// Table layout needed to follow ownership.
pub struct Schema {
    columns: HashMap<String, Vec<String>>,
    /// Columns that must be given a value (NOT NULL, no default, not the key), with their type.
    required: HashMap<String, Vec<(String, String)>>,
    /// Tables with `owner_id` and `owner_table`.
    owner_tables: Vec<String>,
    /// Owner table -> its list tables.
    lists: HashMap<String, Vec<String>>,
}

impl Schema {
    pub fn read(conn: &Connection) -> Result<Self, Error> {
        let mut columns: HashMap<String, Vec<String>> = HashMap::new();
        let mut required: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT m.name, p.name, p.[notnull] AND p.dflt_value IS NULL AND p.pk = 0, p.type \
             FROM sqlite_master m, pragma_table_info(m.name) p WHERE m.type = 'table'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let (t, c): (String, String) = (r.get(0)?, r.get(1)?);
            if r.get::<_, bool>(2)? {
                required
                    .entry(t.clone())
                    .or_default()
                    .push((c.clone(), r.get(3)?));
            }
            columns.entry(t).or_default().push(c);
        }
        let has = |cols: &[String], c: &str| cols.iter().any(|x| x == c);
        let owner_tables = columns
            .iter()
            .filter(|(_, c)| has(c, "owner_id") && has(c, "owner_table"))
            .map(|(t, _)| t.clone())
            .collect();
        // A list table belongs to the longest other table name it starts with (+ `_`).
        let mut lists: HashMap<String, Vec<String>> = HashMap::new();
        for (t, cols) in &columns {
            if !has(cols, "container_order") || !has(cols, "id") {
                continue;
            }
            let owner = columns
                .keys()
                .filter(|o| *o != t && t.starts_with(o.as_str()) && t.as_bytes()[o.len()] == b'_')
                .max_by_key(|o| o.len());
            if let Some(o) = owner {
                lists.entry(o.clone()).or_default().push(t.clone());
            }
        }
        Ok(Self {
            columns,
            required,
            owner_tables,
            lists,
        })
    }

    fn has_column(&self, table: &str, col: &str) -> bool {
        self.columns
            .get(table)
            .is_some_and(|c| c.iter().any(|x| x == col))
    }
}

/// A captured object: its rows and list rows, main row first.
#[derive(Debug, Clone)]
pub struct Proto {
    pub kind: ObjectKind,
    rows: Vec<(String, Row)>,
    lists: Vec<(String, Row)>,
}

impl Proto {
    /// One line per row: table and its non-null columns (research aid).
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for (t, row) in self.rows.iter().chain(&self.lists) {
            let cols: Vec<String> = row
                .iter()
                .filter(|(_, v)| !matches!(v, Sql::Null))
                .map(|(c, v)| match v {
                    Sql::Integer(i) => format!("{c}={i}"),
                    Sql::Real(r) => format!("{c}={r}"),
                    Sql::Text(s) => format!("{c}={}", s.chars().take(30).collect::<String>()),
                    _ => format!("{c}=<blob>"),
                })
                .collect();
            out.push_str(&format!("{t}: {}\n", cols.join(" ")));
        }
        out
    }
}

fn select_rows(conn: &Connection, sql: &str, id: i64) -> Result<Vec<Row>, Error> {
    let mut stmt = conn.prepare_cached(sql)?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let rows = stmt
        .query_map([id], |r| {
            (0..names.len())
                .map(|i| Ok((names[i].clone(), r.get::<_, Sql>(i)?)))
                .collect()
        })?
        .collect::<Result<Vec<Row>, _>>()?;
    Ok(rows)
}

/// Captures the row tree of the object stored in `main_table` row `main_id`.
pub fn capture(
    conn: &Connection,
    schema: &Schema,
    kind: ObjectKind,
    main_table: &str,
    main_id: i64,
) -> Result<Proto, Error> {
    let mut proto = Proto {
        kind,
        rows: Vec::new(),
        lists: Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut queue = vec![(main_table.to_owned(), main_id)];
    while let Some((t, id)) = queue.pop() {
        if !seen.insert((t.clone(), id)) || !schema.columns.contains_key(&t) {
            continue;
        }
        let q = quote(&t);
        let Some(row) = select_rows(conn, &format!("SELECT * FROM {q} WHERE id = ?1"), id)?
            .into_iter()
            .next()
        else {
            continue;
        };
        if let (Some(Sql::Text(it)), Some(iid)) =
            (get(&row, "impl_table"), int(get(&row, "impl_id")))
        {
            queue.push((it.clone(), iid));
        }
        for o in &schema.owner_tables {
            let sql = format!(
                "SELECT id FROM {} WHERE owner_table = '{}' AND owner_id = ?1",
                quote(o),
                t.replace('\'', "''")
            );
            let ids: Vec<i64> = conn
                .prepare_cached(&sql)?
                .query_map([id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            queue.extend(ids.into_iter().map(|i| (o.clone(), i)));
        }
        for l in schema.lists.get(&t).into_iter().flatten() {
            let sql = format!(
                "SELECT * FROM {} WHERE id = ?1 ORDER BY container_order",
                quote(l)
            );
            for r in select_rows(conn, &sql, id)? {
                proto.lists.push((l.clone(), r));
            }
        }
        proto.rows.push((t, row));
    }
    Ok(proto)
}

/// Finds an object in an opened model matching `want` (field key, value) and captures it.
pub fn find_proto(
    doc: &RfDocument,
    kind: ObjectKind,
    want: &[(&str, Value)],
) -> Result<Proto, Error> {
    let id: ObjectId = doc
        .model
        .iter_kind(kind)
        // Saved objects only (new ones have no rows yet).
        .filter(|(id, _)| doc.db_key(*id).is_some())
        .find(|(_, o)| want.iter().all(|(k, v)| o.get(k) == v))
        .map(|(id, _)| id)
        .ok_or_else(|| {
            Error::Format(format!(
                "no {} matching {want:?} in the donor model",
                kind.name()
            ))
        })?;
    let (table, key) = doc.db_key(id).expect("loaded objects have a key");
    let conn =
        Connection::open_with_flags(doc.archive.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema = Schema::read(&conn)?;
    capture(&conn, &schema, kind, table, key)
}

/// Captures the first row tree of `table` in an opened model: standalone objects without an
/// [`ObjectKind`], such as `ConcreteExposureCondition`. None if the table is empty or missing.
pub fn first_of_table(doc: &RfDocument, table: &str) -> Result<Option<Proto>, Error> {
    let conn =
        Connection::open_with_flags(doc.archive.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema = Schema::read(&conn)?;
    if !schema.columns.contains_key(table) {
        return Ok(None);
    }
    let id: Option<i64> =
        conn.query_row(&format!("SELECT MIN(id) FROM {}", quote(table)), [], |r| {
            r.get(0)
        })?;
    // The kind is only a label here.
    id.map(|id| capture(&conn, &schema, ObjectKind::Surface, table, id))
        .transpose()
}

/// Objects RFEM expects concrete surfaces to refer to (it crashed on surfaces without them);
/// RFEM creates them with the first surface.
pub const SURFACE_SUPPORT_TABLES: [&str; 2] =
    ["ConcreteExposureCondition", "ReinforcementDirection"];

/// Random-enough GUID text (`xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx`) without extra crates.
fn new_guid(seq: &mut u64) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut x = t ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    *seq += 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        b >> 52,
        b & 0xffff_ffff_ffff
    )
}

/// A model under construction: a purged template plus cloned objects.
pub struct Builder {
    /// None when working on a document's own database ([`Builder::on_db`]).
    archive: Option<Archive>,
    db: std::path::PathBuf,
    conn: Connection,
    schema: Schema,
    next: HashMap<String, i64>,
    seq: u64,
}

impl Builder {
    /// Opens `donor` and removes every model object except materials and sections.
    pub fn from_template(donor: &Path) -> Result<Self, Error> {
        let mut archive = Archive::open(donor)?;
        archive.drop_prefixes = vec!["!solver/".into()];
        let conn = Connection::open(archive.db_path())?;
        // Rows are inserted in tree order, not dependency order; checked in `finish`.
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let schema = Schema::read(&conn)?;
        let mut b = Self {
            db: archive.db_path().to_owned(),
            archive: Some(archive),
            conn,
            schema,
            next: HashMap::new(),
            seq: 1,
        };
        let mut tables: Vec<&str> = ObjectKind::ALL
            .iter()
            .filter(|k| !matches!(k, ObjectKind::Material | ObjectKind::Section))
            .flat_map(|k| mapping::tables(*k).iter().copied())
            .collect();
        tables.extend_from_slice(EXTRA_PURGE_TABLES);
        b.purge(&tables)?;
        Ok(b)
    }

    /// Works on an open document's database (adding objects to an existing model); nothing
    /// is purged. [`Builder::finish`] is not available.
    pub(crate) fn on_db(db: &Path) -> Result<Self, Error> {
        let conn = Connection::open(db)?;
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let schema = Schema::read(&conn)?;
        Ok(Self {
            archive: None,
            db: db.to_owned(),
            conn,
            schema,
            next: HashMap::new(),
            seq: 1,
        })
    }

    /// Deletes all objects of `tables` with their impl, part and list rows, and list entries
    /// elsewhere that reference them.
    pub fn purge(&mut self, tables: &[&str]) -> Result<(), Error> {
        let c = &self.conn;
        c.execute_batch(
            "DROP TABLE IF EXISTS temp.fc_del; \
             CREATE TEMP TABLE fc_del (t TEXT NOT NULL, id INTEGER NOT NULL, PRIMARY KEY (t, id));",
        )?;
        for t in tables {
            if !self.schema.columns.contains_key(*t) {
                continue;
            }
            let q = quote(t);
            c.execute(
                &format!("INSERT OR IGNORE INTO temp.fc_del SELECT ?1, id FROM {q}"),
                [t],
            )?;
            if self.schema.has_column(t, "impl_table") {
                c.execute(
                    &format!(
                        "INSERT OR IGNORE INTO temp.fc_del SELECT impl_table, impl_id FROM {q} \
                         WHERE impl_table IS NOT NULL AND impl_id IS NOT NULL"
                    ),
                    [],
                )?;
            }
        }
        // Owned part rows, to any depth.
        loop {
            let mut added = 0;
            for o in &self.schema.owner_tables {
                added += c.execute(
                    &format!(
                        "INSERT OR IGNORE INTO temp.fc_del SELECT ?1, o.id FROM {} o \
                         JOIN temp.fc_del d ON d.t = o.owner_table AND d.id = o.owner_id",
                        quote(o)
                    ),
                    [o],
                )?;
            }
            if added == 0 {
                break;
            }
        }
        let doomed: Vec<String> = c
            .prepare("SELECT DISTINCT t FROM temp.fc_del")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for t in &doomed {
            if !self.schema.columns.contains_key(t) {
                continue;
            }
            let ids = "(SELECT id FROM temp.fc_del WHERE t = ?1)";
            for l in self.schema.lists.get(t).into_iter().flatten() {
                c.execute(&format!("DELETE FROM {} WHERE id IN {ids}", quote(l)), [t])?;
            }
            c.execute(&format!("DELETE FROM {} WHERE id IN {ids}", quote(t)), [t])?;
        }
        // List entries elsewhere that point at deleted rows.
        for (t, cols) in &self.schema.columns {
            if cols.iter().any(|x| x == "reference_table")
                && cols.iter().any(|x| x == "reference_id")
                && cols.iter().any(|x| x == "container_order")
            {
                c.execute(
                    &format!(
                        "DELETE FROM {} WHERE EXISTS (SELECT 1 FROM temp.fc_del d \
                         WHERE d.t = reference_table AND d.id = reference_id)",
                        quote(t)
                    ),
                    [],
                )?;
            }
        }
        c.execute_batch("DROP TABLE temp.fc_del;")?;
        Ok(())
    }

    fn next_id(&mut self, table: &str) -> Result<i64, Error> {
        if !self.next.contains_key(table) {
            let max: i64 = self.conn.query_row(
                &format!("SELECT COALESCE(MAX(id), 0) FROM {}", quote(table)),
                [],
                |r| r.get(0),
            )?;
            self.next.insert(table.to_owned(), max + 1);
        }
        let n = self.next.get_mut(table).unwrap();
        *n += 1;
        Ok(*n - 1)
    }

    /// Inserts a copy of `proto` with object number `user_id`. For loads, `load_case` is the
    /// database id of the load case. Returns the new main row id.
    pub fn add(
        &mut self,
        proto: &Proto,
        user_id: i64,
        load_case: Option<i64>,
    ) -> Result<i64, Error> {
        let mut map: HashMap<(String, i64), i64> = HashMap::new();
        for (t, row) in &proto.rows {
            let old = int(get(row, "id")).unwrap_or(0);
            let new = self.next_id(t)?;
            map.insert((t.clone(), old), new);
        }
        let main_table = proto.rows[0].0.clone();
        // Owned children of each row, to resolve `<name>_id` columns without a `_table`.
        let owner_of = |row: &Row| -> Option<(String, i64)> {
            match (get(row, "owner_table"), int(get(row, "owner_id"))) {
                (Some(Sql::Text(t)), Some(i)) => Some((t.clone(), i)),
                _ => None,
            }
        };
        for (t, row) in &proto.rows {
            let old = int(get(row, "id")).unwrap_or(0);
            let mut out: Row = Vec::new();
            for (c, v) in row {
                if !self.schema.has_column(t, c) {
                    continue;
                }
                let nv = if c == "id" {
                    Sql::Integer(map[&(t.clone(), old)])
                } else if c == "idForExportImport" && !matches!(v, Sql::Null) {
                    Sql::Text(new_guid(&mut self.seq))
                } else if c == "userID" && *t == main_table {
                    Sql::Integer(user_id)
                } else if let (Some(base), Sql::Integer(val)) = (c.strip_suffix("_id"), v) {
                    let tcol = format!("{base}_table");
                    let target = match get(row, &tcol) {
                        Some(Sql::Text(tt)) => Some(tt.clone()),
                        _ if c == "parent_id" && *t != main_table => Some(main_table.clone()),
                        _ => {
                            // Unique proto row whose table name contains the column base,
                            // else a unique owned child with that id.
                            let lb = base.to_lowercase();
                            let by_name: Vec<&String> = proto
                                .rows
                                .iter()
                                .map(|(pt, _)| pt)
                                .filter(|pt| {
                                    pt.to_lowercase().contains(&lb)
                                        && map.contains_key(&((*pt).clone(), *val))
                                })
                                .collect();
                            if by_name.len() == 1 {
                                Some(by_name[0].clone())
                            } else {
                                let kids: Vec<&String> = proto
                                    .rows
                                    .iter()
                                    .filter(|(_, r)| owner_of(r) == Some((t.clone(), old)))
                                    .filter(|(_, r)| int(get(r, "id")) == Some(*val))
                                    .map(|(pt, _)| pt)
                                    .collect();
                                (kids.len() == 1).then(|| kids[0].clone())
                            }
                        }
                    };
                    match target.and_then(|tt| map.get(&(tt, *val))) {
                        Some(n) => Sql::Integer(*n),
                        None => v.clone(),
                    }
                } else {
                    v.clone()
                };
                out.push((c.clone(), nv));
            }
            self.fill_required(t, &mut out);
            self.insert(t, &out)?;
        }
        // List entries: references to rows of this object's own tree are remapped (for
        // example `addonProperties_values` -> owned `AddonThicknessProperties`); references to
        // other objects are dropped (FemCraft writes those lists itself). When a map value is
        // dropped, its key (same `..._keys` table, owner and order) goes too.
        let mut kept: Vec<(String, Row)> = Vec::new();
        let mut dropped: HashSet<(String, i64, i64)> = HashSet::new();
        for (l, row) in &proto.lists {
            let mut out = row.clone();
            let mut keep = true;
            if let Some(Sql::Integer(rid)) = get(row, "reference_id") {
                match get(row, "reference_table") {
                    Some(Sql::Text(rt)) if map.contains_key(&(rt.clone(), *rid)) => {
                        let new = map[&(rt.clone(), *rid)];
                        for (c, v) in &mut out {
                            if c == "reference_id" {
                                *v = Sql::Integer(new);
                            }
                        }
                    }
                    _ => keep = false,
                }
            }
            if matches!(get(row, "value_id"), Some(Sql::Integer(_))) {
                keep = false;
            }
            if keep {
                kept.push((l.clone(), out));
            } else if let Some(base) = l.strip_suffix("_values") {
                let id = int(get(row, "id")).unwrap_or(0);
                let order = int(get(row, "container_order")).unwrap_or(0);
                dropped.insert((format!("{base}_keys"), id, order));
            }
        }
        kept.retain(|(l, row)| {
            let id = int(get(row, "id")).unwrap_or(0);
            let order = int(get(row, "container_order")).unwrap_or(0);
            !dropped.contains(&(l.clone(), id, order))
        });
        for (l, row) in &kept {
            let owner = self
                .schema
                .lists
                .iter()
                .find(|(_, ls)| ls.contains(l))
                .map(|(o, _)| o.clone());
            let (Some(owner), Some(old)) = (owner, int(get(row, "id"))) else {
                continue;
            };
            let Some(new) = map.get(&(owner, old)) else {
                continue;
            };
            let mut out: Row = row
                .iter()
                .map(|(c, v)| {
                    (
                        c.clone(),
                        if c == "id" {
                            Sql::Integer(*new)
                        } else {
                            v.clone()
                        },
                    )
                })
                .collect();
            self.fill_required(l, &mut out);
            self.insert(l, &out)?;
        }
        let main_old = int(get(&proto.rows[0].1, "id")).unwrap_or(0);
        let main_new = map[&(main_table.clone(), main_old)];
        if let Some(lc) = load_case {
            self.conn.execute(
                &format!(
                    "UPDATE {} SET parentModelObject_id = ?1, parentModelObject_table = 'LoadCase' \
                     WHERE id = ?2",
                    quote(&main_table)
                ),
                rusqlite::params![lc, main_new],
            )?;
        }
        Ok(main_new)
    }

    fn insert(&self, table: &str, row: &Row) -> Result<(), Error> {
        let cols: Vec<String> = row.iter().map(|(c, _)| quote(c)).collect();
        let marks: Vec<String> = (1..=row.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote(table),
            cols.join(", "),
            marks.join(", ")
        );
        let params: Vec<&Sql> = row.iter().map(|(_, v)| v).collect();
        self.conn
            .prepare_cached(&sql)?
            .execute(rusqlite::params_from_iter(params))?;
        Ok(())
    }

    /// Starts a transaction; without [`Builder::commit`] everything is rolled back when the
    /// builder is dropped.
    pub(crate) fn begin(&self) -> Result<(), Error> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    pub(crate) fn commit(self) -> Result<(), Error> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    /// Whether `proto` can be inserted here: every row's table exists. (Required columns the
    /// copy lacks, e.g. from an older RFEM version, get default values in [`Builder::add`].)
    /// Returns the first problem.
    pub fn misfit(&self, proto: &Proto) -> Option<String> {
        for (t, _) in proto.rows.iter().chain(&proto.lists) {
            if !self.schema.columns.contains_key(t) {
                return Some(format!("table {t} does not exist"));
            }
        }
        None
    }

    /// How many columns of this database the copy has no value for (0: it comes from the same
    /// RFEM version; such a copy is preferred).
    pub fn missing_columns(&self, proto: &Proto) -> usize {
        proto
            .rows
            .iter()
            .chain(&proto.lists)
            .map(|(t, row)| {
                self.schema.columns.get(t).map_or(0, |cols| {
                    cols.iter().filter(|c| get(row, c).is_none()).count()
                })
            })
            .sum()
    }

    /// Adds the required columns of `table` that `row` lacks, with RFEM's neutral defaults
    /// (0 / false, 0.0, empty text), as for a file written by an older RFEM version.
    fn fill_required(&self, table: &str, row: &mut Row) {
        for (c, ty) in self.schema.required.get(table).into_iter().flatten() {
            if get(row, c).is_some() {
                continue;
            }
            let ty = ty.to_ascii_uppercase();
            let v = if ty.contains("INT") || ty.contains("BOOL") {
                Sql::Integer(0)
            } else if ["REAL", "DOUB", "FLOA", "NUM", "DEC"]
                .iter()
                .any(|k| ty.contains(k))
            {
                Sql::Real(0.0)
            } else {
                Sql::Text(String::new())
            };
            row.push((c.clone(), v));
        }
    }

    /// Rows in `table` (0 if the table does not exist).
    pub(crate) fn count_rows(&self, table: &str) -> Result<i64, Error> {
        if !self.schema.columns.contains_key(table) {
            return Ok(0);
        }
        Ok(self
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", quote(table)), [], |r| {
                r.get(0)
            })?)
    }

    /// Gives a surface (main row id) the "standard" stiffness type, which carries a thickness.
    pub fn make_surface_standard(&mut self, surface_id: i64) -> Result<(), Error> {
        let (it, iid): (String, i64) = self.conn.query_row(
            "SELECT impl_table, impl_id FROM Surface WHERE id = ?1",
            [surface_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (part, st, sid): (i64, String, i64) = self.conn.query_row(
            "SELECT id, stiffness_table, stiffness_id FROM SurfaceSpecificImplPart \
             WHERE owner_table = ?1 AND owner_id = ?2",
            rusqlite::params![it, iid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if st != "SurfaceStiffnessStandard" {
            self.conn
                .execute(&format!("DELETE FROM {} WHERE id = ?1", quote(&st)), [sid])?;
            let new = self.next_id("SurfaceStiffnessStandard")?;
            self.conn.execute(
                "INSERT INTO SurfaceStiffnessStandard (id, version, owner_id, owner_table, \
                 thickness_id) VALUES (?1, 0, ?2, ?3, NULL)",
                rusqlite::params![new, iid, it],
            )?;
            self.conn.execute(
                "UPDATE SurfaceSpecificImplPart SET stiffness_table = 'SurfaceStiffnessStandard', \
                 stiffness_id = ?1 WHERE id = ?2",
                rusqlite::params![new, part],
            )?;
        }
        // (Also for surfaces that were standard already: cloning drops these list entries.)
        // Concrete surfaces refer to a durability and a reinforcement direction (one entry
        // per surface side), as in models saved by RFEM.
        for (list, target) in [
            (
                "SurfaceSpecificImplPart_concreteDurabilities",
                "ConcreteExposureCondition",
            ),
            (
                "SurfaceSpecificImplPart_reinforcementDirections",
                "ReinforcementDirection",
            ),
        ] {
            if !self.schema.columns.contains_key(list) || !self.schema.columns.contains_key(target)
            {
                continue;
            }
            let first: Option<i64> = self.conn.query_row(
                &format!("SELECT MIN(id) FROM {}", quote(target)),
                [],
                |r| r.get(0),
            )?;
            let Some(first) = first else { continue };
            let rows: i64 = self.conn.query_row(
                &format!("SELECT COUNT(*) FROM {} WHERE id = ?1", quote(list)),
                [part],
                |r| r.get(0),
            )?;
            if rows == 0 {
                for order in 0..2 {
                    self.conn.execute(
                        &format!(
                            "INSERT INTO {} (id, container_order, value_id) VALUES (?1, ?2, ?3)",
                            quote(list)
                        ),
                        rusqlite::params![part, order, first],
                    )?;
                }
            } else {
                self.conn.execute(
                    &format!(
                        "UPDATE {} SET value_id = ?1 WHERE id = ?2 AND value_id IS NULL",
                        quote(list)
                    ),
                    rusqlite::params![first, part],
                )?;
            }
        }
        Ok(())
    }

    /// Foreign-key violations as `(table, referenced table, count)`.
    pub fn foreign_key_problems(&self) -> Result<Vec<(String, String, i64)>, Error> {
        foreign_key_problems(&self.conn)
    }

    /// Sets every foreign key that points at a missing row to NULL (references left over from
    /// prototypes or purged objects). Returns `(table.column, count)` of what was cleared.
    pub fn clear_dangling_references(&self) -> Result<Vec<(String, i64)>, Error> {
        let mut found: Vec<(String, i64, i64)> = Vec::new();
        {
            let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                found.push((r.get(0)?, r.get(1)?, r.get(3)?));
            }
        }
        let mut cleared: HashMap<String, i64> = HashMap::new();
        for (table, rowid, fkid) in found {
            let cols: Vec<String> = self
                .conn
                .prepare_cached(&format!(
                    "SELECT \"from\" FROM pragma_foreign_key_list('{}') WHERE id = ?1",
                    table.replace('\'', "''")
                ))?
                .query_map([fkid], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let is_list = self.schema.has_column(&table, "container_order");
            for c in cols {
                if c == "owner_id" || is_list {
                    // An orphaned part row or a list entry to a missing object: remove it.
                    self.conn.execute(
                        &format!("DELETE FROM {} WHERE rowid = ?1", quote(&table)),
                        [rowid],
                    )?;
                    *cleared
                        .entry(format!("{table} (orphan rows deleted)"))
                        .or_default() += 1;
                    continue;
                }
                self.conn.execute(
                    &format!(
                        "UPDATE {} SET {} = NULL WHERE rowid = ?1",
                        quote(&table),
                        quote(&c)
                    ),
                    [rowid],
                )?;
                *cleared.entry(format!("{table}.{c}")).or_default() += 1;
            }
        }
        let mut v: Vec<_> = cleared.into_iter().collect();
        v.sort();
        Ok(v)
    }

    /// Finishes the database and loads it as a document, ready for setting field values.
    pub fn finish(self) -> Result<RfDocument, Error> {
        drop(self.conn);
        RfDocument::from_archive(self.archive.expect("finish needs a template builder"))
    }
}

/// Foreign-key violations in a database as `(table, referenced table, count)`.
pub fn foreign_key_problems(conn: &Connection) -> Result<Vec<(String, String, i64)>, Error> {
    let mut counts: HashMap<(String, String), i64> = HashMap::new();
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        *counts.entry((r.get(0)?, r.get(2)?)).or_default() += 1;
    }
    let mut v: Vec<_> = counts.into_iter().map(|((a, b), n)| (a, b, n)).collect();
    v.sort();
    Ok(v)
}

impl Builder {
    /// Path of the working database (for checks with another connection).
    pub fn db_path(&self) -> &Path {
        &self.db
    }
}

/// Writes a blank model to `out`: `template` emptied of all objects (its settings, materials
/// and sections kept), plus load case 1 "Self-weight" and, if the template has a uniform
/// thickness, thickness 1 (200 mm, material 1). The basis of File > New.
pub fn blank_model(template: &Path, out: &Path) -> Result<(), Error> {
    let src = RfDocument::open(template)?;
    let lc = find_proto(&src, ObjectKind::LoadCase, &[])?;
    let thickness = find_proto(
        &src,
        ObjectKind::Thickness,
        &[("type", Value::Text("TYPE_UNIFORM".into()))],
    )
    .ok();
    let first_material = src
        .model
        .iter_kind(ObjectKind::Material)
        .next()
        .map(|(id, _)| id.no);
    drop(src);

    let mut b = Builder::from_template(template)?;
    b.add(&lc, 1, None)?;
    if let Some(t) = &thickness {
        b.add(t, 1, None)?;
    }
    b.clear_dangling_references()?;
    let mut doc = b.finish()?;
    let set = |doc: &mut RfDocument, id: ObjectId, k: &str, v: Value| {
        doc.model
            .set(id, k, v)
            .map_err(|e| Error::Format(format!("{id:?}.{k}: {e}")))
    };
    let lc1 = ObjectId::new(ObjectKind::LoadCase, 1);
    set(&mut doc, lc1, "name", Value::Text("Self-weight".into()))?;
    set(&mut doc, lc1, "self_weight_active", Value::Bool(true))?;
    if thickness.is_some() {
        let t1 = ObjectId::new(ObjectKind::Thickness, 1);
        set(&mut doc, t1, "name", Value::Text("Slab 200".into()))?;
        set(&mut doc, t1, "uniform_thickness", Value::Float(0.2))?;
        if let Some(m) = first_material {
            set(&mut doc, t1, "material", Value::Ref(m))?;
        }
    }
    update_general_data(&mut doc)?;
    placeholder_previews(&mut doc)?;
    doc.save(out)
}

/// Updates `general_data.xml` for a newly built model: object counts, no results, new model id.
pub fn update_general_data(doc: &mut RfDocument) -> Result<(), Error> {
    let Ok(bytes) = doc.archive.read_entry("general_data.xml") else {
        return Ok(());
    };
    let mut xml = String::from_utf8_lossy(&bytes).into_owned();
    let m = &doc.model;
    let counts = [
        ("E_PROPERTY_NODE_COUNT", m.count(ObjectKind::Node)),
        ("E_PROPERTY_LINE_COUNT", m.count(ObjectKind::Line)),
        ("E_PROPERTY_SURFACE_COUNT", m.count(ObjectKind::Surface)),
        ("E_PROPERTY_MEMBER_COUNT", m.count(ObjectKind::Member)),
        ("E_PROPERTY_SOLID_COUNT", m.count(ObjectKind::Solid)),
        ("E_PROPERTY_LC_COUNT", m.count(ObjectKind::LoadCase)),
        ("E_PROPERTY_CO_COUNT", 0),
        ("E_PROPERTY_CS_COUNT", 0),
        ("E_PROPERTY_RC_COUNT", 0),
    ];
    for (key, n) in counts {
        set_xml_value(&mut xml, key, &n.to_string());
    }
    set_xml_value(&mut xml, "E_PROPERTY_HAS_RESULTS", "false");
    set_xml_value(&mut xml, "E_PROPERTY_HAS_ALL_RESULTS", "false");
    let mut seq = 7;
    let guid = format!("{{{}}}", new_guid(&mut seq));
    if let Some(p) = xml.find("value0=\"Model ID\"") {
        let tag_start = xml[..p].rfind('<').unwrap_or(0);
        let tag_end = p + xml[p..].find('>').unwrap_or(0);
        let tag = &xml[tag_start..tag_end];
        if let Some(v) = tag.find("value1=\"") {
            let s = tag_start + v + 8;
            if let Some(e) = xml[s..].find('"') {
                xml.replace_range(s..s + e, &guid);
            }
        }
    }
    doc.archive
        .replace
        .push(("general_data.xml".into(), xml.into_bytes()));
    Ok(())
}

/// Replaces the template's preview images (a picture of another model) with a question mark
/// and its 3D preview with an empty glTF scene.
pub fn placeholder_previews(doc: &mut RfDocument) -> Result<(), Error> {
    let (w, h) = (1200u16, 900u16);
    let (cx, cy) = (600.0f32, 330.0f32);
    let mut px = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let r = (dx * dx + dy * dy).sqrt();
            let ang = dy.atan2(dx).to_degrees(); // y down: 90 = straight down
            let hook = (125.0..175.0).contains(&r) && !(90.0..160.0).contains(&ang);
            let stem = (575.0..625.0).contains(&(x as f32)) && (455.0..600.0).contains(&(y as f32));
            let (ex, ey) = (x as f32 - 600.0, y as f32 - 690.0);
            let dot = ex * ex + ey * ey < 35.0 * 35.0;
            let v = if hook || stem || dot { 90u8 } else { 232u8 };
            px.extend_from_slice(&[v, v, v]);
        }
    }
    let mut jpg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpg, 85)
        .encode(&px, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| Error::Format(format!("preview image: {e}")))?;

    let mut json =
        br#"{"asset":{"version":"2.0","generator":"FemCraft"},"scene":0,"scenes":[{"nodes":[]}]}"#
            .to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() as u32;
    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&total.to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);

    for name in ["preview.jpg", "preview_dark.jpg"] {
        if doc.archive.entries.iter().any(|e| e == name) {
            doc.archive.replace.push((name.into(), jpg.clone()));
        }
    }
    if doc.archive.entries.iter().any(|e| e == "preview.glb") {
        doc.archive.replace.push(("preview.glb".into(), glb));
    }
    Ok(())
}

/// Sets `value="..."` of the tag carrying `key="KEY"`.
fn set_xml_value(xml: &mut String, key: &str, value: &str) {
    let Some(p) = xml.find(&format!("key=\"{key}\"")) else {
        return;
    };
    let tag_start = xml[..p].rfind('<').unwrap_or(0);
    let tag_end = p + xml[p..].find('>').unwrap_or(0);
    if let Some(v) = xml[tag_start..tag_end].find(" value=\"") {
        let s = tag_start + v + 8;
        if let Some(e) = xml[s..].find('"') {
            xml.replace_range(s..s + e, value);
        }
    }
}
