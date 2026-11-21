//! Schema catalogue: tables, columns and the RFEM object / impl / child pattern.

use rusqlite::Connection;
use rusqlite::types::ValueRef;

use crate::Error;

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub decl_type: String,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    /// Row count; -1 when not counted ([`Catalog::read`]; [`Catalog::read_with_counts`]
    /// counts).
    pub rows: i64,
}

impl TableInfo {
    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Main object table pointing to a detail table.
    pub fn has_impl(&self) -> bool {
        self.column("impl_id").is_some() && self.column("impl_table").is_some()
    }

    /// Child list table owned by a row of another table.
    /// Child list (`id` = owner + `container_order`) or part table (`owner_id`).
    pub fn is_child(&self) -> bool {
        self.column("owner_id").is_some() || self.column("container_order").is_some()
    }
}

pub struct Catalog {
    pub tables: Vec<TableInfo>,
}

impl Catalog {
    /// Every table with its columns, in one query (an RFEM model has ~5,900 tables: a
    /// `PRAGMA table_info` and a `COUNT(*)` per table took seconds on every open / reload).
    /// Rows are not counted (`rows` = -1).
    pub fn read(conn: &Connection) -> Result<Self, Error> {
        let mut tables: Vec<TableInfo> = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT m.name, p.name, p.type FROM sqlite_master m \
             JOIN pragma_table_info(m.name) p WHERE m.type = 'table' ORDER BY m.name, p.cid",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let table: String = r.get(0)?;
            let col = ColumnInfo {
                name: r.get(1)?,
                decl_type: r.get(2)?,
            };
            match tables.last_mut() {
                Some(t) if t.name == table => t.columns.push(col),
                _ => tables.push(TableInfo {
                    name: table,
                    columns: vec![col],
                    rows: -1,
                }),
            }
        }
        Ok(Self { tables })
    }

    /// [`Catalog::read`] with the row count of every table (slow on a full RFEM model).
    pub fn read_with_counts(conn: &Connection) -> Result<Self, Error> {
        let mut cat = Self::read(conn)?;
        for t in &mut cat.tables {
            t.rows =
                conn.query_row(&format!("SELECT COUNT(*) FROM {}", quote(&t.name)), [], |r| {
                    r.get(0)
                })?;
        }
        Ok(cat)
    }

    /// Case-insensitive table lookup.
    pub fn table(&self, name: &str) -> Option<&TableInfo> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Human-readable listing; `filter` keeps tables whose name contains any of the
    /// (case-insensitive) substrings, `non_empty` skips empty tables.
    pub fn describe(&self, filter: &[String], non_empty: bool) -> String {
        let mut out = String::new();
        for t in &self.tables {
            let lname = t.name.to_lowercase();
            if !filter.is_empty() && !filter.iter().any(|f| lname.contains(&f.to_lowercase())) {
                continue;
            }
            if non_empty && t.rows == 0 {
                continue;
            }
            let tag = match (t.has_impl(), t.is_child()) {
                (true, _) => " [object]",
                (_, true) => " [child]",
                _ => "",
            };
            out.push_str(&format!("{} ({} rows){tag}\n", t.name, t.rows));
            for c in &t.columns {
                out.push_str(&format!("    {} {}\n", c.name, c.decl_type));
            }
        }
        out
    }
}

/// Prints the first `limit` rows of a table as tab-separated text.
pub fn dump_rows(conn: &Connection, table: &str, limit: usize) -> Result<String, Error> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {} LIMIT {limit}", quote(table)))?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let mut out = format!("{}\n", names.join("\t"));
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let cells: Vec<String> = (0..names.len())
            .map(|i| match r.get_ref(i) {
                Ok(ValueRef::Null) => "NULL".into(),
                Ok(ValueRef::Integer(v)) => v.to_string(),
                Ok(ValueRef::Real(v)) => v.to_string(),
                Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).replace(['\t', '\n'], " "),
                Ok(ValueRef::Blob(b)) => format!("<blob {} bytes>", b.len()),
                Err(e) => format!("<{e}>"),
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    Ok(out)
}

pub fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
