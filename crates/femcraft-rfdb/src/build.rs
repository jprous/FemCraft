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
    "ReinforcementDirection",
    "ConcreteExposureCondition",
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
    /// Tables with `owner_id` and `owner_table`.
    owner_tables: Vec<String>,
    /// Owner table -> its list tables.
    lists: HashMap<String, Vec<String>>,
}

impl Schema {
    pub fn read(conn: &Connection) -> Result<Self, Error> {
        let mut columns: HashMap<String, Vec<String>> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p \
             WHERE m.type = 'table'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            columns.entry(r.get(0)?).or_default().push(r.get(1)?);
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
    archive: Archive,
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
            archive,
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
            self.insert(t, &out)?;
        }
        for (l, row) in &proto.lists {
            // Drop entries referring to other objects; FemCraft writes those lists itself.
            let refers = ["reference_id", "value_id"]
                .iter()
                .any(|c| matches!(get(row, c), Some(v) if !matches!(v, Sql::Null)));
            if refers {
                continue;
            }
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
            let out: Row = row
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
        if st == "SurfaceStiffnessStandard" {
            return Ok(());
        }
        self.conn
            .execute(&format!("DELETE FROM {} WHERE id = ?1", quote(&st)), [sid])?;
        let new = self.next_id("SurfaceStiffnessStandard")?;
        self.conn.execute(
            "INSERT INTO SurfaceStiffnessStandard (id, version, owner_id, owner_table, thickness_id) \
             VALUES (?1, 0, ?2, ?3, NULL)",
            rusqlite::params![new, iid, it],
        )?;
        self.conn.execute(
            "UPDATE SurfaceSpecificImplPart SET stiffness_table = 'SurfaceStiffnessStandard', \
             stiffness_id = ?1 WHERE id = ?2",
            rusqlite::params![new, part],
        )?;
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
            for c in cols {
                if c == "owner_id" {
                    // An orphaned part row: remove it.
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
        RfDocument::from_archive(self.archive)
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
        self.archive.db_path()
    }
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
