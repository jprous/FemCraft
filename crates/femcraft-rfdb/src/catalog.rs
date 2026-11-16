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
    pub fn is_child(&self) -> bool {
        self.column("owner_id").is_some()
    }
}

pub struct Catalog {
    pub tables: Vec<TableInfo>,
}

impl Catalog {
    pub fn read(conn: &Connection) -> Result<Self, Error> {
        let names: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut tables = Vec::with_capacity(names.len());
        for name in names {
            let q = quote(&name);
            let columns = conn
                .prepare(&format!("PRAGMA table_info({q})"))?
                .query_map([], |r| {
                    Ok(ColumnInfo {
                        name: r.get(1)?,
                        decl_type: r.get(2)?,
                    })
                })?
                .collect::<Result<_, _>>()?;
            let rows = conn.query_row(&format!("SELECT COUNT(*) FROM {q}"), [], |r| r.get(0))?;
            tables.push(TableInfo {
                name,
                columns,
                rows,
            });
        }
        Ok(Self { tables })
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

pub(crate) fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
