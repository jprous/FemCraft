//! Research tool for the RFEM file format.
//!
//! ```text
//! rf6-catalog <file.rf6>                       load report: tables / fields found per object kind
//! rf6-catalog <file.rf6> --tables [FILTER...]  tables and columns (non-empty only; --all for all)
//! rf6-catalog <file.rf6> --rows TABLE [N]      first N rows of a table (default 20)
//! ```

use std::path::Path;

use femcraft_rfdb::{Archive, Catalog, RfDocument, catalog::dump_rows};

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
            println!("entries: {}", doc.archive.entries.join(", "));
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
            print!("{}", Catalog::read(&conn)?.describe(&filter, !all));
        }
        Some("--rows") => {
            let table = args.get(2).ok_or("--rows needs a table name")?;
            let n = args.get(3).map_or(Ok(20), |s| s.parse())?;
            let archive = Archive::open(path)?;
            let conn = rusqlite::Connection::open(archive.db_path())?;
            print!("{}", dump_rows(&conn, table, n)?);
        }
        Some(other) => return Err(format!("unknown option {other}").into()),
    }
    Ok(())
}
