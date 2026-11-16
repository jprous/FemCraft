use std::collections::{BTreeMap, BTreeSet};

use crate::kinds::{FieldType, ObjectKind};
use crate::value::Value;

/// Identifies an object. Loads are numbered per load case; other objects use `load_case = 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    pub kind: ObjectKind,
    pub load_case: u32,
    pub no: u32,
}

impl ObjectId {
    pub fn new(kind: ObjectKind, no: u32) -> Self {
        Self {
            kind,
            load_case: 0,
            no,
        }
    }

    pub fn load(kind: ObjectKind, load_case: u32, no: u32) -> Self {
        Self {
            kind,
            load_case,
            no,
        }
    }
}

/// One object: its field values keyed by [`crate::FieldSpec::key`].
/// Fields missing from the map are unknown / not stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    pub values: BTreeMap<&'static str, Value>,
}

impl Object {
    pub fn get(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.values.get(key).unwrap_or(&NULL)
    }
}

#[derive(Debug, PartialEq)]
pub enum EditError {
    NoSuchObject(ObjectId),
    NoSuchField(ObjectKind, String),
    WrongType {
        field: &'static str,
        expected: FieldType,
    },
    AlreadyExists(ObjectId),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NoSuchObject(id) => write!(f, "{id:?} does not exist"),
            EditError::NoSuchField(k, n) => write!(f, "{} has no field '{n}'", k.name()),
            EditError::WrongType { field, expected } => {
                write!(f, "field '{field}' expects {expected:?}")
            }
            EditError::AlreadyExists(id) => write!(f, "{id:?} already exists"),
        }
    }
}

impl std::error::Error for EditError {}

/// All objects of a model plus the set of objects changed since loading.
#[derive(Clone, Debug, Default)]
pub struct Model {
    objects: BTreeMap<ObjectId, Object>,
    dirty: BTreeSet<ObjectId>,
    removed: BTreeSet<ObjectId>,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts an object as loaded from a file (not marked dirty).
    pub fn insert_loaded(&mut self, id: ObjectId, obj: Object) {
        self.objects.insert(id, obj);
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id)
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Objects of one kind, ordered by load case then number.
    pub fn iter_kind(&self, kind: ObjectKind) -> impl Iterator<Item = (ObjectId, &Object)> {
        let lo = ObjectId {
            kind,
            load_case: 0,
            no: 0,
        };
        let hi = ObjectId {
            kind,
            load_case: u32::MAX,
            no: u32::MAX,
        };
        self.objects.range(lo..=hi).map(|(id, o)| (*id, o))
    }

    pub fn count(&self, kind: ObjectKind) -> usize {
        self.iter_kind(kind).count()
    }

    /// Lowest free number for a new object of `kind` (in `load_case` for loads).
    pub fn next_no(&self, kind: ObjectKind, load_case: u32) -> u32 {
        self.iter_kind(kind)
            .filter(|(id, _)| id.load_case == load_case)
            .map(|(id, _)| id.no)
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Sets one field after checking the field exists and the value fits its type.
    pub fn set(&mut self, id: ObjectId, key: &str, value: Value) -> Result<(), EditError> {
        let spec = id
            .kind
            .spec()
            .field(key)
            .ok_or_else(|| EditError::NoSuchField(id.kind, key.to_owned()))?;
        let value = coerce(spec.ty, value).ok_or(EditError::WrongType {
            field: spec.key,
            expected: spec.ty,
        })?;
        let obj = self
            .objects
            .get_mut(&id)
            .ok_or(EditError::NoSuchObject(id))?;
        if obj.get(key) != &value {
            obj.values.insert(spec.key, value);
            self.dirty.insert(id);
        }
        Ok(())
    }

    /// Adds a new, empty object.
    pub fn add(&mut self, id: ObjectId) -> Result<&mut Object, EditError> {
        if self.objects.contains_key(&id) {
            return Err(EditError::AlreadyExists(id));
        }
        self.removed.remove(&id);
        self.dirty.insert(id);
        Ok(self.objects.entry(id).or_default())
    }

    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let obj = self.objects.remove(&id)?;
        self.dirty.remove(&id);
        self.removed.insert(id);
        Some(obj)
    }

    /// Objects changed or added since loading (or since [`Model::clear_changes`]).
    pub fn dirty(&self) -> &BTreeSet<ObjectId> {
        &self.dirty
    }

    pub fn removed(&self) -> &BTreeSet<ObjectId> {
        &self.removed
    }

    pub fn is_modified(&self) -> bool {
        !self.dirty.is_empty() || !self.removed.is_empty()
    }

    pub fn clear_changes(&mut self) {
        self.dirty.clear();
        self.removed.clear();
    }

    // Geometry helpers for the viewport.

    pub fn node_position(&self, no: u32) -> Option<[f64; 3]> {
        let n = self.get(ObjectId::new(ObjectKind::Node, no))?;
        let c = |k| n.get(k).as_f64().unwrap_or(0.0);
        Some([c("coordinate_1"), c("coordinate_2"), c("coordinate_3")])
    }

    pub fn line_nodes(&self, no: u32) -> &[u32] {
        self.get(ObjectId::new(ObjectKind::Line, no))
            .map_or(&[], |l| l.get("definition_nodes").as_ref_list())
    }

    /// Start and end node of a member (RFEM takes them from the member's line).
    pub fn member_nodes(&self, no: u32) -> Option<(u32, u32)> {
        let m = self.get(ObjectId::new(ObjectKind::Member, no))?;
        let nodes = self.line_nodes(m.get("line").as_ref_no()?);
        Some((*nodes.first()?, *nodes.last()?))
    }
}

/// Converts `value` to the representation `ty` expects, or `None` if it cannot.
fn coerce(ty: FieldType, value: Value) -> Option<Value> {
    use FieldType as T;
    Some(match (ty, value) {
        (_, Value::Null) => Value::Null,
        (T::Bool, v) => Value::Bool(v.as_bool()?),
        (T::Int, v) => Value::Int(v.as_i64()?),
        (T::Float(_), v) => Value::Float(v.as_f64()?),
        (T::Text | T::Enum, Value::Text(s)) => Value::Text(s),
        // RFEM stores most enumerations as integer codes.
        (T::Enum, v) => Value::Int(v.as_i64()?),
        (T::Ref(_), v) => Value::Ref(v.as_ref_no()?),
        (T::RefList(_), Value::RefList(l)) => Value::RefList(l),
        (T::RefList(_), Value::Text(s)) => Value::RefList(crate::list::parse_list(&s)?),
        (T::RefList(_), v) => Value::RefList(vec![v.as_ref_no()?]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_and_geometry() {
        let mut m = Model::new();
        for (no, x) in [(1, 0.0), (2, 5.0)] {
            m.add(ObjectId::new(ObjectKind::Node, no)).unwrap();
            m.set(
                ObjectId::new(ObjectKind::Node, no),
                "coordinate_1",
                Value::Float(x),
            )
            .unwrap();
        }
        let line = ObjectId::new(ObjectKind::Line, 1);
        m.add(line).unwrap();
        m.set(line, "definition_nodes", Value::Text("1,2".into()))
            .unwrap();
        let mem = ObjectId::new(ObjectKind::Member, 1);
        m.add(mem).unwrap();
        m.set(mem, "line", Value::Int(1)).unwrap();

        assert_eq!(m.member_nodes(1), Some((1, 2)));
        assert_eq!(m.node_position(2), Some([5.0, 0.0, 0.0]));
        assert_eq!(m.next_no(ObjectKind::Node, 0), 3);
        assert!(m.set(mem, "line", Value::Text("x".into())).is_err());
        assert!(m.set(mem, "bogus", Value::Int(1)).is_err());

        m.clear_changes();
        m.set(
            ObjectId::new(ObjectKind::Node, 1),
            "coordinate_1",
            Value::Float(0.0),
        )
        .unwrap();
        assert!(!m.is_modified(), "setting the same value is not a change");
    }
}
