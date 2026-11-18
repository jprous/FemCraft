//! FemCraft domain model: object kinds, their fields and the editable model. No UI, no file I/O.

pub mod geometry;
pub mod kinds;
pub mod list;
pub mod model;
pub mod value;

pub use kinds::{FieldSpec, FieldType, Group, KindSpec, ObjectKind};
pub use model::{EditError, Model, Object, ObjectId};
pub use value::Value;
