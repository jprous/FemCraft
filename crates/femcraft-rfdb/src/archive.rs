//! `.rf6` container: a ZIP with `model.db` plus sidecar files kept byte-identical on save.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use crate::Error;

pub const MODEL_DB: &str = "model.db";
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    /// ZIP archive.
    Rf6,
    /// Bare SQLite database (assumed `.rfc` layout - still to confirm).
    SqliteFile,
    /// Unpacked folder containing `model.db` and sidecars.
    Folder,
}

/// The opened file: `model.db` extracted to a temporary file, everything else kept as it was.
pub struct Archive {
    pub format: SourceFormat,
    /// Original ZIP bytes, used to copy sidecar entries without recompressing them.
    zip_bytes: Option<Vec<u8>>,
    /// Sidecar names in their original order (`model.db` included, for ordering).
    pub entries: Vec<String>,
    folder: Option<PathBuf>,
    db_file: tempfile::NamedTempFile,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let mut db_file = tempfile::Builder::new().suffix(".db").tempfile()?;
        if path.is_dir() {
            let mut entries = Vec::new();
            for e in fs::read_dir(path)? {
                entries.push(e?.file_name().to_string_lossy().into_owned());
            }
            entries.sort();
            let db = path.join(MODEL_DB);
            if !db.is_file() {
                return Err(Error::Format(format!(
                    "no {MODEL_DB} in {}",
                    path.display()
                )));
            }
            fs::copy(&db, db_file.path())?;
            return Ok(Self {
                format: SourceFormat::Folder,
                zip_bytes: None,
                entries,
                folder: Some(path.to_owned()),
                db_file,
            });
        }

        let bytes = fs::read(path)?;
        if bytes.starts_with(SQLITE_MAGIC) {
            db_file.write_all(&bytes)?;
            return Ok(Self {
                format: SourceFormat::SqliteFile,
                zip_bytes: None,
                entries: vec![MODEL_DB.into()],
                folder: None,
                db_file,
            });
        }

        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes))?;
        let mut entries = Vec::with_capacity(zip.len());
        let mut found = false;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let name = f.name()?.into_owned();
            if name == MODEL_DB {
                let mut buf = Vec::with_capacity(f.size() as usize);
                f.read_to_end(&mut buf)?;
                db_file.write_all(&buf)?;
                found = true;
            }
            entries.push(name);
        }
        if !found {
            return Err(Error::Format(format!("no {MODEL_DB} in archive")));
        }
        db_file.flush()?;
        Ok(Self {
            format: SourceFormat::Rf6,
            zip_bytes: Some(bytes),
            entries,
            folder: None,
            db_file,
        })
    }

    /// Path of the working copy of `model.db`.
    pub fn db_path(&self) -> &Path {
        self.db_file.path()
    }

    /// Reads a sidecar file (for example `format.txt`).
    pub fn read_entry(&self, name: &str) -> Result<Vec<u8>, Error> {
        if let Some(bytes) = &self.zip_bytes {
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
            let mut f = zip.by_name(name)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            return Ok(buf);
        }
        if let Some(folder) = &self.folder {
            return Ok(fs::read(folder.join(name))?);
        }
        Err(Error::Format(format!(
            "{name} not available in a bare database"
        )))
    }

    /// Writes an `.rf6`: sidecars raw-copied from the source archive, `model.db` from the
    /// working copy. Only supported when the source was an `.rf6`.
    pub fn save_rf6(&self, path: &Path) -> Result<(), Error> {
        let src = self
            .zip_bytes
            .as_ref()
            .ok_or_else(|| Error::Unsupported("saving .rf6 from a non-.rf6 source".into()))?;
        let mut zip = zip::ZipArchive::new(Cursor::new(src))?;
        let db = fs::read(self.db_path())?;
        let tmp = path.with_extension("rf6.tmp");
        {
            let mut out = zip::ZipWriter::new(fs::File::create(&tmp)?);
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i)?;
                if f.name()? == MODEL_DB {
                    let opts = zip::write::SimpleFileOptions::default()
                        .compression_method(f.compression())
                        .last_modified_time(f.last_modified().unwrap_or_default());
                    drop(f);
                    out.start_file(MODEL_DB, opts)?;
                    out.write_all(&db)?;
                } else {
                    out.raw_copy_file(f)?;
                }
            }
            out.finish()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Writes just the database (for bare-SQLite sources).
    pub fn save_db(&self, path: &Path) -> Result<(), Error> {
        fs::copy(self.db_path(), path)?;
        Ok(())
    }
}
