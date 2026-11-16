/// A single field value of a model object.
///
/// References to other objects are stored by object number (`no`), never by database id.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Ref(u32),
    RefList(Vec<u32>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(v) => Some(v),
            Value::Int(v) => Some(v as f64),
            Value::Bool(b) => Some(b as i64 as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(v) => Some(v),
            Value::Ref(v) => Some(v as i64),
            Value::Bool(b) => Some(b as i64),
            Value::Float(v) if v.fract() == 0.0 => Some(v as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            Value::Int(v) => Some(v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_ref_no(&self) -> Option<u32> {
        match *self {
            Value::Ref(n) => Some(n),
            Value::Int(n) if n > 0 => u32::try_from(n).ok(),
            _ => None,
        }
    }

    pub fn as_ref_list(&self) -> &[u32] {
        match self {
            Value::RefList(v) => v,
            _ => &[],
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => Ok(()),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Text(s) => f.write_str(s),
            Value::Ref(n) => write!(f, "{n}"),
            Value::RefList(v) => f.write_str(&crate::list::format_list(v)),
        }
    }
}
