//! Reading and writing RFEM 6 `.rf6` files (ZIP with `model.db` + sidecars).
//!
//! RFEM file support is reverse engineering for interoperability; table and column names are
//! collected in [`mapping`] and must be confirmed against real files.

pub mod archive;
pub mod build;
pub mod catalog;
pub mod document;
pub mod library;
pub mod mapping;
pub mod mesh;

pub use archive::{Archive, SourceFormat};
pub use catalog::Catalog;
pub use document::{KindReport, LoadReport, RfDocument};
pub use library::{Library, LibraryFilter, LibraryItem, LibraryKind, LibraryValue};
pub use mesh::FeMesh;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Zip(zip::result::ZipError),
    Format(String),
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Sqlite(e) => write!(f, "database error: {e}"),
            Error::Zip(e) => write!(f, "archive error: {e}"),
            Error::Format(s) => write!(f, "not a valid RFEM file: {s}"),
            Error::Unsupported(s) => write!(f, "not supported yet: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}
impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::Zip(e)
    }
}
