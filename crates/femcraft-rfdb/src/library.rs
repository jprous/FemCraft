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

/// Database files chosen in FemCraft's Settings (materials, sections); they win over the
/// folder search. Process-wide, so the GUI and its MCP server agree.
static DB_OVERRIDES: std::sync::Mutex<[Option<PathBuf>; 2]> = std::sync::Mutex::new([None, None]);

fn override_slot(kind: LibraryKind) -> usize {
    match kind {
        LibraryKind::Material => 0,
        LibraryKind::Section => 1,
    }
}

/// Uses `path` as the `kind` database instead of the one found in the library folders
/// (`None` = back to the search).
pub fn set_db_override(kind: LibraryKind, path: Option<PathBuf>) {
    if let Ok(mut o) = DB_OVERRIDES.lock() {
        o[override_slot(kind)] = path;
    }
}

/// The database file the library uses for `kind` when `dir` is searched.
pub fn db_override(kind: LibraryKind) -> Option<PathBuf> {
    DB_OVERRIDES.lock().ok()?[override_slot(kind)].clone()
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
        // A database chosen in Settings: its folder stands in for the search.
        for kind in [LibraryKind::Section, LibraryKind::Material] {
            if let Some(f) = db_override(kind).filter(|f| f.is_file())
                && let Some(dir) = f.parent()
            {
                return Some(Self { dir: dir.to_owned() });
            }
        }
        dirs.iter()
            .find(|d| {
                [LibraryKind::Material, LibraryKind::Section]
                    .iter()
                    .any(|k| d.join(k.file_name()).is_file())
            })
            .map(|d| Self { dir: d.clone() })
    }

    pub fn db_path(&self, kind: LibraryKind) -> PathBuf {
        if let Some(f) = db_override(kind).filter(|f| f.is_file()) {
            return f;
        }
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

    /// Like [`Self::open`], but read-write: for user-added / removed library items
    /// (currently just parametric concrete sections).
    fn open_rw(&self, kind: LibraryKind) -> Result<Connection, Error> {
        let path = self.db_path(kind);
        if !path.is_file() {
            return Err(Error::Format(format!(
                "no {} library ({} is missing)",
                kind.name(),
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Rows are added/removed by hand here, not in RFEM's own dependency order (as
        // `Builder` notes it must do the same for inserts).
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        Ok(conn)
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
        // API I names parametric concrete sections in metres (`R_M1 0.3/0.6`); the library
        // uses RFEM's own millimetres (`R_M1 300/600`). Accept both.
        if let Some(mm) = metre_concrete_name(name)
            && let Some(found) = self.get(kind, &mm)?
        {
            return Ok(Some(found));
        }
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

    /// A `user_id` free to use for a new item of `kind` (highest existing one, plus one).
    pub fn next_user_id(&self, kind: LibraryKind) -> Result<i64, Error> {
        let conn = self.open(kind)?;
        let max: i64 = conn.query_row(
            "SELECT COALESCE(MAX(user_id), 0) FROM femcraft_library WHERE kind = ?1",
            [kind.name()],
            |r| r.get(0),
        )?;
        Ok(max + 1)
    }

    /// Adds a new item (and its property values) to the library. Used for user-added
    /// parametric concrete sections; `item.user_id` should come from [`Self::next_user_id`].
    pub fn insert_item(&self, item: &LibraryItem, values: &[LibraryValue]) -> Result<(), Error> {
        let conn = self.open_rw(item.kind)?;
        conn.execute(
            "INSERT INTO femcraft_library (kind, user_id, name, designation, standard, \
             manufacturer, region, family, category, rfem_library_id, rfem_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                item.kind.name(),
                item.user_id,
                item.name,
                item.designation,
                item.standard,
                item.manufacturer,
                item.region,
                item.family,
                item.category,
                item.rfem_library_id,
                item.rfem_version,
            ],
        )?;
        insert_values(&conn, item.kind, item.user_id, values)?;
        Ok(())
    }

    /// Replaces an existing item's name and property values (its id, family and category are
    /// kept). Used when a user edits a parametric concrete section's dimensions.
    pub fn update_item(&self, item: &LibraryItem, values: &[LibraryValue]) -> Result<(), Error> {
        let conn = self.open_rw(item.kind)?;
        conn.execute(
            "UPDATE femcraft_library SET name = ?1, designation = ?2 \
             WHERE kind = ?3 AND user_id = ?4",
            params![item.name, item.designation, item.kind.name(), item.user_id],
        )?;
        conn.execute(
            "DELETE FROM femcraft_library_value WHERE kind = ?1 AND user_id = ?2",
            params![item.kind.name(), item.user_id],
        )?;
        insert_values(&conn, item.kind, item.user_id, values)?;
        Ok(())
    }

    /// Removes an item and its property values from the library.
    pub fn delete_item(&self, kind: LibraryKind, user_id: i64) -> Result<(), Error> {
        if kind == LibraryKind::Section {
            self.delete_concrete_rows(user_id)?;
        }
        let conn = self.open_rw(kind)?;
        conn.execute(
            "DELETE FROM femcraft_library_value WHERE kind = ?1 AND user_id = ?2",
            params![kind.name(), user_id],
        )?;
        conn.execute(
            "DELETE FROM femcraft_library WHERE kind = ?1 AND user_id = ?2",
            params![kind.name(), user_id],
        )?;
        Ok(())
    }

    /// Duplicates `template_user_id`'s full RFEM row tree - `Section`, its impl chain, its
    /// parameter value lists, its topology row - under a new `user_id`, with fresh ids and
    /// GUIDs. Unlike [`Self::insert_item`] (which only adds a [`LibraryItem`] to the lightweight
    /// index), the result is a real, usable RFEM section: `RfDocument::assign_library_item`
    /// deep-copies this same kind of row tree when assigning a library section to a model
    /// object, and needs it to exist. The template and the new item must be the same
    /// parametric concrete shape (rectangle, square or circle); callers set the new item's
    /// actual dimensions afterwards with [`Self::set_concrete_values`].
    pub fn duplicate_concrete_item(&self, template_user_id: i64, new_user_id: i64) -> Result<(), Error> {
        let path = self.db_path(LibraryKind::Section);
        let proto = {
            let conn = self.open(LibraryKind::Section)?;
            let schema = crate::build::Schema::cached(&conn)?;
            let lib_main: i64 = conn.query_row(
                "SELECT id FROM Section WHERE userID = ?1",
                [template_user_id],
                |r| r.get(0),
            )?;
            crate::build::capture_library_item(
                &conn,
                &schema,
                femcraft_model::ObjectKind::Section,
                "Section",
                lib_main,
            )?
        };
        let mut b = crate::build::Builder::on_db(&path)?;
        b.begin()?;
        b.add(&proto, new_user_id, None)?;
        b.commit()?;
        Ok(())
    }

    /// Overwrites a parametric concrete section's real RFEM property values (area, moments of
    /// inertia, torsion constant, ...) by key name - the ones `assign_library_item` actually
    /// copies into a model, as opposed to [`Self::update_item`]'s lightweight index copy (both
    /// need updating to keep the library's own display and an assigned model in sync).
    pub fn set_concrete_values(&self, user_id: i64, values: &[LibraryValue]) -> Result<(), Error> {
        let conn = self.open_rw(LibraryKind::Section)?;
        let (ps_id, data_id, topo_id): (i64, i64, i64) = conn.query_row(
            "SELECT ps.id, ps.sectionData_id, sd.topology_id FROM Section sec              JOIN SectionImplParametricMassive sm ON sm.id = sec.impl_id              JOIN ParametricSection ps ON ps.id = sm.librarySection_id              JOIN SectionData sd ON sd.id = ps.sectionData_id              WHERE sec.userID = ?1",
            [user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let si = |key: &str| values.iter().find(|v| v.key == key).and_then(|v| v.value_si);

        // 1. RFEM's master dimensions: `ParametricSection_parameterValues` (metres, in the
        //    shape's parameter order: b, h for a rectangle; one value for a square / circle).
        //    The section properties below and the outline are derived from these, and RFEM
        //    regenerates them from these when it opens the model - so without this step it
        //    shows the template's dimensions whatever else was written.
        let old: Vec<f64> = {
            let mut st = conn.prepare(
                "SELECT value FROM ParametricSection_parameterValues WHERE id = ?1                  ORDER BY container_order",
            )?;
            let rows = st.query_map([ps_id], |r| r.get::<_, f64>(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let new: Vec<f64> = match old.len() {
            2 => vec![si("b").unwrap_or(old[0]), si("h").unwrap_or(old[1])],
            1 => vec![si("d").or(si("b")).unwrap_or(old[0])],
            _ => Vec::new(),
        };
        for (i, v) in new.iter().enumerate() {
            conn.execute(
                "UPDATE ParametricSection_parameterValues SET value = ?1                  WHERE id = ?2 AND container_order = ?3",
                params![v, ps_id, i as i64],
            )?;
        }

        // 2. Outline / stress points: scaled from the old dimensions to the new ones.
        if !new.is_empty() && old.iter().all(|v| *v > 0.0) {
            let sx = new[0] / old[0];
            let sy = new.get(1).map_or(sx, |h| h / old[1]);
            conn.execute(
                "UPDATE SectionTopology_stressPoints SET location_x = location_x * ?1,                  location_y = location_y * ?2 WHERE id = ?3",
                params![sx, sy, topo_id],
            )?;
            conn.execute(
                "UPDATE SectionTopology_points SET location_x = location_x * ?1,                  location_y = location_y * ?2 WHERE id = ?3",
                params![sx, sy, topo_id],
            )?;
        }

        // 3. The section's property values (dimensions and everything derived): the SI value
        //    and, where the entry has an input unit (b / h / d in mm, e_y / e_z in cm), the
        //    value as shown in that unit.
        for v in values {
            let Some(si) = v.value_si else { continue };
            let unit: Option<String> = conn
                .query_row(
                    "SELECT inputUnit FROM SectionData_parameterValues_values WHERE id = ?1                      AND container_order = (SELECT container_order FROM                      SectionData_parameterValues_keys WHERE id = ?1 AND value = ?2)",
                    params![data_id, v.key],
                    |r| r.get(0),
                )
                .optional()?;
            let shown = match unit.as_deref() {
                Some("mm") => Some(si * 1e3),
                Some("cm") => Some(si * 1e2),
                Some("m") => Some(si),
                _ => None,
            };
            conn.execute(
                "UPDATE SectionData_parameterValues_values SET valueSI = ?1,                  valueInputUnit = COALESCE(?4, valueInputUnit) WHERE id = ?2                  AND container_order = (SELECT container_order FROM                  SectionData_parameterValues_keys WHERE id = ?2 AND value = ?3)",
                params![si, data_id, v.key, shown],
            )?;
        }
        Ok(())
    }

    /// The real RFEM row tree behind a parametric concrete section ([`Self::duplicate_concrete_item`]'s
    /// counterpart), removed along with its [`Self::delete_item`] index entry. A no-op for any
    /// other kind of section (no `SectionImplParametricMassive` row to find).
    fn delete_concrete_rows(&self, user_id: i64) -> Result<(), Error> {
        let conn = self.open_rw(LibraryKind::Section)?;
        let schema = crate::build::Schema::cached(&conn)?;
        let found: Option<i64> = conn
            .query_row(
                "SELECT id FROM Section WHERE userID = ?1",
                [user_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(sec_id) = found else {
            return Ok(());
        };
        let proto = crate::build::capture_library_item(
            &conn,
            &schema,
            femcraft_model::ObjectKind::Section,
            "Section",
            sec_id,
        )?;
        // Delete list entries first (foreign-key-safe order), then the rows themselves.
        for (table, id) in proto.row_keys() {
            let q = crate::catalog::quote(table);
            for l in schema.list_tables(table) {
                conn.execute(&format!("DELETE FROM {} WHERE id = ?1", crate::catalog::quote(l)), [id])?;
            }
            conn.execute(&format!("DELETE FROM {q} WHERE id = ?1"), [id])?;
        }
        Ok(())
    }

    /// Renames parametric concrete items still named in metres (`R_M1 0.3/0.6`, as API I
    /// names them) to RFEM's millimetres (`R_M1 300/600`). Idempotent; returns how many.
    pub fn migrate_concrete_names(&self) -> Result<usize, Error> {
        let conn = self.open_rw(LibraryKind::Section)?;
        let rows: Vec<(i64, String, String)> = {
            let mut st = conn.prepare(
                "SELECT user_id, name, designation FROM femcraft_library                  WHERE kind = 'section' AND category = 'concrete'",
            )?;
            let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            it.collect::<Result<_, _>>()?
        };
        let mut n = 0;
        for (uid, name, designation) in rows {
            let new_name = metre_concrete_name(&name);
            let new_des = metre_concrete_name(&designation);
            if new_name.is_none() && new_des.is_none() {
                continue;
            }
            conn.execute(
                "UPDATE femcraft_library SET name = ?1, designation = ?2                  WHERE kind = 'section' AND user_id = ?3",
                params![new_name.unwrap_or(name), new_des.unwrap_or(designation), uid],
            )?;
            n += 1;
        }
        Ok(n)
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

fn insert_values(
    conn: &Connection,
    kind: LibraryKind,
    user_id: i64,
    values: &[LibraryValue],
) -> Result<(), Error> {
    for v in values {
        conn.execute(
            "INSERT INTO femcraft_library_value (kind, user_id, key, idx, value_si, \
             value_text, thickness_si) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
            params![
                kind.name(),
                user_id,
                v.key,
                v.value_si,
                v.value_text,
                v.thickness_si
            ],
        )?;
    }
    Ok(())
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

/// `R_M1 0.3/0.6` / `SQ_M1 0.4` / `CIRCLE_M1 0.4` (metres) -> `R_M1 300/600` ... (mm).
fn metre_concrete_name(name: &str) -> Option<String> {
    let (shape, dims) = name.trim().split_once(' ')?;
    if !matches!(shape, "R_M1" | "SQ_M1" | "CIRCLE_M1") {
        return None;
    }
    let mm = |s: &str| s.parse::<f64>().ok().map(|v| format!("{}", (v * 1e4).round() / 10.0));
    let parts: Vec<String> = dims.split('/').map(mm).collect::<Option<_>>()?;
    // Already millimetres (a value >= 10 m is not a section): leave to the exact lookup.
    if dims.split('/').any(|d| d.parse::<f64>().is_ok_and(|v| v >= 10.0)) {
        return None;
    }
    Some(format!("{shape} {}", parts.join("/")))
}
