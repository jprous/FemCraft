//! Research tool for the RFEM file format.
//!
//! ```text
//! rf6-catalog <file.rf6>                       load report: tables / fields found per object kind
//! rf6-catalog <file.rf6> --tables [FILTER...]  tables and columns (non-empty only; --all for all)
//! rf6-catalog <file.rf6> --rows TABLE [N]      first N rows of a table (default 20)
//! rf6-catalog <file.rf6> --kinds [KIND...]     per object kind: main, impl and related tables
//!                                              with 2 sample rows each (compact, for pasting)
//! rf6-catalog <file.rf6> --sql "SELECT ..."    read-only ad-hoc query, compact output
//! rf6-catalog <file.rf6> --fk                 foreign-key check (dangling references)
//! rf6-catalog <file.rf6> --objects KIND [N]    first N objects as FemCraft loads them
//! rf6-catalog <file.rf6> --edit OUT KIND NO[@LC] FIELD VALUE   change one field, save as OUT
//! ```

use std::path::Path;

use femcraft_model::ObjectKind;
use femcraft_rfdb::catalog::{dump_rows, quote};
use femcraft_rfdb::{Archive, Catalog, RfDocument, mapping};
use rusqlite::Connection;
use rusqlite::types::ValueRef;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!(
            "usage: rf6-catalog <file.rf6> [--tables [--all] [FILTER...] | --rows TABLE [N]]"
        );
        std::process::exit(2);
    };
    let path = Path::new(path);
    match args.get(1).map(String::as_str) {
        None => {
            let doc = RfDocument::open(path)?;
            if let Ok(f) = doc.archive.read_entry("format.txt") {
                println!("format.txt: {}", String::from_utf8_lossy(&f).trim());
            }
            let (solver, other): (Vec<_>, Vec<_>) = doc
                .archive
                .entries
                .iter()
                .partition(|e| e.starts_with("!solver/"));
            let other: Vec<&str> = other.iter().map(|s| s.as_str()).collect();
            println!(
                "entries: {} (+ {} in !solver/)",
                other.join(", "),
                solver.len()
            );
            print!("{}", doc.report);
        }
        Some("--tables") => {
            let archive = Archive::open(path)?;
            let conn = rusqlite::Connection::open(archive.db_path())?;
            let all = args.iter().any(|a| a == "--all");
            let filter: Vec<String> = args[2..]
                .iter()
                .filter(|a| *a != "--all")
                .cloned()
                .collect();
            print!("{}", Catalog::read_with_counts(&conn)?.describe(&filter, !all));
        }
        Some("--rows") => {
            let table = args.get(2).ok_or("--rows needs a table name")?;
            let n = args.get(3).map_or(Ok(20), |s| s.parse())?;
            let archive = Archive::open(path)?;
            let conn = rusqlite::Connection::open(archive.db_path())?;
            print!("{}", dump_rows(&conn, table, n)?);
        }
        Some("--kinds") => {
            let archive = Archive::open(path)?;
            let conn = Connection::open(archive.db_path())?;
            let cat = Catalog::read_with_counts(&conn)?;
            let wanted = &args[2..];
            for &kind in ObjectKind::ALL {
                if wanted.is_empty() || wanted.iter().any(|w| w.eq_ignore_ascii_case(kind.name())) {
                    print!("{}", describe_kind(&conn, &cat, kind, !wanted.is_empty())?);
                }
            }
        }
        Some("--edit") => {
            // --edit OUT.rf6 KIND NO[@LC] FIELD VALUE
            let [out, kind_name, no, field, value] = &args[2..] else {
                return Err("usage: --edit OUT.rf6 KIND NO[@LOADCASE] FIELD VALUE".into());
            };
            let kind = ObjectKind::ALL
                .iter()
                .copied()
                .find(|k| k.name().eq_ignore_ascii_case(kind_name))
                .ok_or("unknown kind")?;
            let (no, lc) = match no.split_once('@') {
                Some((n, lc)) => (n.parse()?, lc.parse()?),
                None => (no.parse()?, 0),
            };
            let id = femcraft_model::ObjectId::load(kind, lc, no);
            let value = match value.parse::<f64>() {
                Ok(v) => femcraft_model::Value::Float(v),
                Err(_) => femcraft_model::Value::Text(value.clone()),
            };
            let mut doc = RfDocument::open(path)?;
            println!(
                "before: {field} = {}",
                doc.model.get(id).ok_or("no such object")?.get(field)
            );
            doc.model.set(id, field, value)?;
            doc.save(Path::new(out))?;
            println!("saved {out}");
        }
        Some("--assign") => {
            // --assign OUT.rf6 material|section NO NAME [material|section NO NAME ...]
            let (out, rest) = args[2..].split_first().ok_or("--assign needs OUT")?;
            if rest.is_empty() || rest.len() % 3 != 0 {
                return Err("usage: --assign OUT.rf6 material|section NO NAME [...]".into());
            }
            let lib = femcraft_rfdb::Library::find(&femcraft_rfdb::Library::default_dirs())
                .ok_or("no FemCraft library found")?;
            let mut doc = RfDocument::open(path)?;
            for t in rest.chunks(3) {
                let kind = femcraft_rfdb::LibraryKind::parse(&t[0]).ok_or("material or section")?;
                let obj_kind = match kind {
                    femcraft_rfdb::LibraryKind::Material => ObjectKind::Material,
                    femcraft_rfdb::LibraryKind::Section => ObjectKind::Section,
                };
                let item = lib.get(kind, &t[2])?.ok_or("no such library item")?;
                let id = femcraft_model::ObjectId::new(obj_kind, t[1].parse()?);
                doc.assign_library_item(id, &lib, &item)?;
                println!("{} {} -> {}", obj_kind.name(), t[1], item.name);
            }
            doc.save(Path::new(out))?;
            println!("saved {out}");
        }
        Some("--objects") => {
            let kind_name = args.get(2).ok_or("--objects needs a kind")?;
            let kind = ObjectKind::ALL
                .iter()
                .copied()
                .find(|k| k.name().eq_ignore_ascii_case(kind_name))
                .ok_or("unknown kind")?;
            let n = args.get(3).map_or(Ok(5), |s| s.parse())?;
            let doc = RfDocument::open(path)?;
            for (id, obj) in doc.model.iter_kind(kind).take(n) {
                let fields: Vec<String> = obj
                    .values
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                let lc = if id.load_case > 0 {
                    format!(" (LC{})", id.load_case)
                } else {
                    String::new()
                };
                println!("{} {}{lc}: {}", kind.name(), id.no, fields.join(" "));
            }
        }
        Some("--proto") => {
            // --proto KIND NO[@LC]: the row tree an object is made of.
            let (Some(kind_name), Some(no)) = (args.get(2), args.get(3)) else {
                return Err("usage: --proto KIND NO[@LOADCASE]".into());
            };
            let kind = ObjectKind::ALL
                .iter()
                .copied()
                .find(|k| k.name().eq_ignore_ascii_case(kind_name))
                .ok_or("unknown kind")?;
            let (no, lc): (u32, u32) = match no.split_once('@') {
                Some((n, lc)) => (n.parse()?, lc.parse()?),
                None => (no.parse()?, 0),
            };
            let doc = RfDocument::open(path)?;
            let id = femcraft_model::ObjectId::load(kind, lc, no);
            let (table, key) = doc.db_key(id).ok_or("no such object")?;
            let conn = Connection::open_with_flags(
                doc.archive.db_path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let schema = femcraft_rfdb::build::Schema::read(&conn)?;
            let proto = femcraft_rfdb::build::capture(&conn, &schema, kind, table, key)?;
            print!("{}", proto.describe());
        }
        Some("--fk") => {
            let archive = Archive::open(path)?;
            let conn = Connection::open_with_flags(
                archive.db_path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let problems = femcraft_rfdb::build::foreign_key_problems(&conn)?;
            for (t, parent, n) in &problems {
                println!("{n} row(s) of {t} -> missing {parent}");
            }
            println!("{} foreign-key problem group(s)", problems.len());
        }
        Some("--sql") => {
            let sql = args.get(2).ok_or("--sql needs a query")?;
            let archive = Archive::open(path)?;
            let conn = Connection::open_with_flags(
                archive.db_path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            println!("{}", compact_rows(&conn, sql, &[])?.join("\n"));
        }
        Some(other) => return Err(format!("unknown option {other}").into()),
    }
    Ok(())
}

/// One row as `col=value` pairs, skipping NULL and empty text.
fn compact_rows(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut cells = Vec::new();
        for (i, n) in names.iter().enumerate() {
            let v = match r.get_ref(i)? {
                ValueRef::Null => continue,
                ValueRef::Integer(v) => v.to_string(),
                ValueRef::Real(v) => v.to_string(),
                ValueRef::Text(b"") => continue,
                ValueRef::Text(t) => {
                    let t = String::from_utf8_lossy(t);
                    if t.chars().count() > 40 {
                        format!("{}..", t.chars().take(40).collect::<String>())
                    } else {
                        t.into_owned()
                    }
                }
                ValueRef::Blob(b) => format!("<blob {}>", b.len()),
            };
            cells.push(format!("{n}={v}"));
        }
        out.push(format!("    {}", cells.join(" ")));
    }
    Ok(out)
}

fn describe_kind(
    conn: &Connection,
    cat: &Catalog,
    kind: ObjectKind,
    force: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let Some(main) = mapping::tables(kind).iter().find_map(|t| cat.table(t)) else {
        return Ok(String::new());
    };
    if main.rows == 0 && !force {
        return Ok(String::new());
    }
    let mut out = format!("## {} ({}, {} rows)\n", kind.name(), main.name, main.rows);
    let q = quote(&main.name);
    out += &compact_rows(conn, &format!("SELECT * FROM {q} LIMIT 2"), &[])?.join("\n");
    out.push('\n');
    let mut shown = vec![main.name.clone()];
    if main.has_impl() {
        let impls: Vec<(String, i64)> = conn
            .prepare(&format!(
                "SELECT impl_table, COUNT(*) FROM {q} GROUP BY impl_table"
            ))?
            .query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get(1)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (t, n) in impls {
            out += &format!("  impl {t} ({n} objects)\n");
            if cat.table(&t).is_some() {
                let sql = format!(
                    "SELECT * FROM {} WHERE id IN (SELECT impl_id FROM {q} WHERE impl_table = ?1) LIMIT 2",
                    quote(&t)
                );
                out += &compact_rows(conn, &sql, &[&t])?.join("\n");
                out.push('\n');
                shown.push(t);
            }
        }
    }
    // Non-empty tables named after the main or impl tables (child lists, property parts).
    let others: Vec<&str> = ObjectKind::ALL
        .iter()
        .filter(|k| **k != kind)
        .flat_map(|k| mapping::tables(*k).iter().copied())
        .collect();
    let prefixes = shown.clone();
    let mut related = 0;
    for t in &cat.tables {
        if t.rows == 0
            || shown.contains(&t.name)
            || others.iter().any(|o| t.name.eq_ignore_ascii_case(o))
        {
            continue;
        }
        if !prefixes.iter().any(|p| t.name.starts_with(p.as_str())) {
            continue;
        }
        related += 1;
        if related > 40 {
            out += &format!("  related {} ({} rows) [no sample]\n", t.name, t.rows);
            continue;
        }
        out += &format!("  related {} ({} rows)\n", t.name, t.rows);
        out += &compact_rows(
            conn,
            &format!("SELECT * FROM {} LIMIT 1", quote(&t.name)),
            &[],
        )?
        .join("\n");
        out.push('\n');
    }
    Ok(out)
}
