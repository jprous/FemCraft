//! FemCraft's own material and section libraries.
//!
//! RFEM's library databases are encrypted. FemCraft's libraries are instead extracted from a
//! running RFEM by `tools/rfem_library_extract.py`: every item is created by name in a scratch
//! model, so `materials.db` / `sections.db` are RFEM `model.db` files (RFEM's own table layout,
//! one object per library item, `MaterialImpl.databaseMaterialId` /
//! `DatabaseSection.databaseSectionId` linking it to RFEM's library) plus two index tables:
//!
//! - `femcraft_library(kind, user_id, name, designation, standard, manufacturer, region,
//!   family, category, rfem_library_id, rfem_version)`: one row per item; `user_id` is the
//!   object number of the item in the library database;
//! - `femcraft_library_value(kind, user_id, key, idx, value_si, value_text, thickness_si)`:
//!   RFEM's property values (`SectionData_parameterValues`, `MaterialProperties_propertiesMap`),
//!   SI units. Thickness-dependent values (`f_y`, `f_u`) have one row per range, `idx` in
//!   order, `thickness_si` the upper bound of the range.
//!
//! The databases contain RFEM data and are kept outside the repository, in
//! `%APPDATA%\FemCraft\data\library` (or `FEMCRAFT_LIBRARY`, or `library` next to the
//! executable).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Material,
    Section,
}

impl LibraryKind {
    pub fn name(self) -> &'static str {
        match self {
            LibraryKind::Material => "material",
            LibraryKind::Section => "section",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            LibraryKind::Material => "materials.db",
            LibraryKind::Section => "sections.db",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().trim_end_matches('s') {
            "material" => Some(LibraryKind::Material),
            "section" | "cross_section" => Some(LibraryKind::Section),
            _ => None,
        }
    }
}

/// One library item.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    pub kind: LibraryKind,
    /// Object number in the library database.
    pub user_id: i64,
    /// RFEM's full name, e.g. `IPE 200 | EN 10365:2017 | ArcelorMittal (2018)`.
    pub name: String,
    pub designation: String,
    pub standard: String,
    pub manufacturer: String,
    /// `EU` or `ZA`.
    pub region: String,
    /// Leading letters of the designation (`IPE`, `UB`, `CHS`, `C`, `S`, ...).
    pub family: String,
    /// Materials: `concrete`, `reinforcement`, `steel_hot_rolled`, `steel_cold_formed`;
    /// sections: `hot_rolled`, `cold_formed`, `welded`, `other`.
    pub category: String,
    /// RFEM's library id (`databaseMaterialId` / `databaseSectionId`).
    pub rfem_library_id: Option<i64>,
    pub rfem_version: String,
}

/// One property value of an item.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryValue {
    pub key: String,
    pub value_si: Option<f64>,
    pub value_text: Option<String>,
    /// Upper bound of the thickness range a strength applies to.
    pub thickness_si: Option<f64>,
}

/// What to list. Empty strings match everything; `text` matches any part of the name.
#[derive(Debug, Clone, Default)]
pub struct LibraryFilter {
    pub text: String,
    pub region: String,
    pub family: String,
    pub category: String,
    /// 0 = no limit.
    pub limit: usize,
}

/// The library databases found in one folder.
pub struct Library {
    pub dir: PathBuf,
}

impl Library {
    /// Folders searched by [`Library::find`]: `FEMCRAFT_LIBRARY`, FemCraft's data folder,
    /// `library` next to the executable.
    pub fn default_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("FEMCRAFT_LIBRARY") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(d) = std::env::var_os("APPDATA") {
            dirs.push(Path::new(&d).join("FemCraft").join("data").join("library"));
        }
        if let Some(d) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_owned))
        {
            dirs.push(d.join("library"));
        }
        dirs
    }

    /// The first folder holding a library database.
    pub fn find(dirs: &[PathBuf]) -> Option<Self> {
        dirs.iter()
            .find(|d| {
                [LibraryKind::Material, LibraryKind::Section]
                    .iter()
                    .any(|k| d.join(k.file_name()).is_file())
            })
            .map(|d| Self { dir: d.clone() })
    }

    pub fn db_path(&self, kind: LibraryKind) -> PathBuf {
        self.dir.join(kind.file_name())
    }

    fn open(&self, kind: LibraryKind) -> Result<Connection, Error> {
        let path = self.db_path(kind);
        if !path.is_file() {
            return Err(Error::Format(format!(
                "no {} library ({} is missing)",
                kind.name(),
                path.display()
            )));
        }
        Ok(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    }

    /// Items matching `filter`, in library order.
    pub fn list(
        &self,
        kind: LibraryKind,
        filter: &LibraryFilter,
    ) -> Result<Vec<LibraryItem>, Error> {
        let conn = self.open(kind)?;
        let like = |s: &str| format!("%{}%", s.replace('%', ""));
        let limit = if filter.limit == 0 {
            -1
        } else {
            filter.limit as i64
        };
        let mut stmt = conn.prepare(
            "SELECT user_id, name, designation, standard, manufacturer, region, family, \
             category, rfem_library_id, rfem_version FROM femcraft_library \
             WHERE kind = ?1 AND name LIKE ?2 AND (?3 = '' OR region = ?3) \
             AND (?4 = '' OR family = ?4 COLLATE NOCASE) AND (?5 = '' OR category = ?5) \
             ORDER BY user_id LIMIT ?6",
        )?;
        let rows = stmt.query_map(
            params![
                kind.name(),
                like(&filter.text),
                filter.region.to_ascii_uppercase(),
                filter.family,
                filter.category,
                limit
            ],
            |r| item(kind, r),
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The item named `name`: RFEM's full name, or a designation (`IPE 200`) when exactly
    /// one item has it.
    pub fn get(&self, kind: LibraryKind, name: &str) -> Result<Option<LibraryItem>, Error> {
        let conn = self.open(kind)?;
        let sql = "SELECT user_id, name, designation, standard, manufacturer, region, family, \
             category, rfem_library_id, rfem_version FROM femcraft_library \
             WHERE kind = ?1 AND ";
        let exact = conn
            .query_row(
                &format!("{sql}name = ?2"),
                params![kind.name(), name],
                |r| item(kind, r),
            )
            .optional()?;
        if exact.is_some() {
            return Ok(exact);
        }
        let mut stmt = conn.prepare(&format!("{sql}designation = ?2 LIMIT 2"))?;
        let found: Vec<LibraryItem> = stmt
            .query_map(params![kind.name(), name.trim()], |r| item(kind, r))?
            .collect::<Result<_, _>>()?;
        Ok((found.len() == 1).then(|| found.into_iter().next().unwrap()))
    }

    /// Property values of an item, in RFEM's order.
    pub fn values(&self, item: &LibraryItem) -> Result<Vec<LibraryValue>, Error> {
        let conn = self.open(item.kind)?;
        let mut stmt = conn.prepare(
            "SELECT key, value_si, value_text, thickness_si FROM femcraft_library_value \
             WHERE kind = ?1 AND user_id = ?2 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![item.kind.name(), item.user_id], |r| {
            Ok(LibraryValue {
                key: r.get(0)?,
                value_si: r.get(1)?,
                value_text: r.get(2)?,
                thickness_si: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The first value (`idx` 0) of each of `keys` for every item of `kind`, by `user_id`:
    /// table columns in one query.
    pub fn columns(
        &self,
        kind: LibraryKind,
        keys: &[&str],
    ) -> Result<std::collections::HashMap<i64, Vec<Option<f64>>>, Error> {
        let conn = self.open(kind)?;
        let marks: Vec<String> = (0..keys.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT user_id, key, value_si FROM femcraft_library_value \
             WHERE kind = ?1 AND idx = 0 AND key IN ({})",
            marks.join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let kind_name = kind.name();
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&kind_name];
        for k in keys {
            params.push(k);
        }
        let mut out: std::collections::HashMap<i64, Vec<Option<f64>>> = Default::default();
        let mut rows = stmt.query(params.as_slice())?;
        while let Some(r) = rows.next()? {
            let (id, key, v): (i64, String, Option<f64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
            if let Some(i) = keys.iter().position(|k| *k == key) {
                out.entry(id).or_insert_with(|| vec![None; keys.len()])[i] = v;
            }
        }
        Ok(out)
    }

    /// Number of items of each kind (0 when that database is missing).
    pub fn count(&self, kind: LibraryKind) -> usize {
        self.open(kind)
            .and_then(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM femcraft_library WHERE kind = ?1",
                    [kind.name()],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .map_or(0, |n| n as usize)
    }
}

fn item(kind: LibraryKind, r: &rusqlite::Row) -> rusqlite::Result<LibraryItem> {
    Ok(LibraryItem {
        kind,
        user_id: r.get(0)?,
        name: r.get(1)?,
        designation: r.get(2)?,
        standard: r.get(3)?,
        manufacturer: r.get(4)?,
        region: r.get(5)?,
        family: r.get(6)?,
        category: r.get(7)?,
        rfem_library_id: r.get(8)?,
        rfem_version: r.get(9)?,
    })
}
