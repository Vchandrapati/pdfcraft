//! The object vocabulary the structure inspector reads: a read-only copy of COS objects with the
//! accessors it uses (`get`, `as_dict`, `as_array`, …), returning `Result` so lookups chain with
//! `and_then`. Objects come from the lazy COS reader ([`crate::structure::LazyStructure`]).

use std::fmt;

/// An indirect object's number and generation.
pub(crate) type ObjectId = (u32, u16);

/// How a string was written: `(literal)` or `<hex>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StringFormat {
    Literal,
    Hexadecimal,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Object {
    Null,
    Boolean(bool),
    Integer(i64),
    Real(f32),
    Name(Vec<u8>),
    String(Vec<u8>, StringFormat),
    Array(Vec<Object>),
    Dictionary(Dictionary),
    Stream(Stream),
    Reference(ObjectId),
}

/// A stream's dictionary and its raw (still encoded) bytes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stream {
    pub dict: Dictionary,
    pub content: Vec<u8>,
}

/// A dictionary in the order its keys were read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dictionary(Vec<(Vec<u8>, Object)>);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Error {
    ObjectNotFound(ObjectId),
    ObjectType { expected: &'static str, found: &'static str },
    DictKey(String),
    ReferenceCycle(ObjectId),
    ReferenceLimit,
    RecursionLimit,
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ObjectNotFound((n, g)) => write!(f, "object {n} {g} R not found"),
            Error::ObjectType { expected, found } => write!(f, "expected {expected}, found {found}"),
            Error::DictKey(key) => write!(f, "missing dictionary key /{key}"),
            Error::ReferenceCycle((n, g)) => write!(f, "reference cycle at {n} {g} R"),
            Error::ReferenceLimit => f.write_str("too many nested references"),
            Error::RecursionLimit => f.write_str("objects nested too deeply"),
        }
    }
}

impl std::error::Error for Error {}

impl Object {
    fn variant(&self) -> &'static str {
        match self {
            Object::Null => "Null",
            Object::Boolean(_) => "Boolean",
            Object::Integer(_) => "Integer",
            Object::Real(_) => "Real",
            Object::Name(_) => "Name",
            Object::String(..) => "String",
            Object::Array(_) => "Array",
            Object::Dictionary(_) => "Dictionary",
            Object::Stream(_) => "Stream",
            Object::Reference(_) => "Reference",
        }
    }

    fn wrong(&self, expected: &'static str) -> Error {
        Error::ObjectType { expected, found: self.variant() }
    }

    pub(crate) fn as_bool(&self) -> Result<bool> {
        match self {
            Object::Boolean(v) => Ok(*v),
            _ => Err(self.wrong("Boolean")),
        }
    }

    pub(crate) fn as_i64(&self) -> Result<i64> {
        match self {
            Object::Integer(v) => Ok(*v),
            _ => Err(self.wrong("Integer")),
        }
    }

    /// An integer or a real, as `f32`.
    pub(crate) fn as_float(&self) -> Result<f32> {
        match self {
            Object::Integer(v) => Ok(*v as f32),
            Object::Real(v) => Ok(*v),
            _ => Err(self.wrong("Integer or Real")),
        }
    }

    pub(crate) fn as_name(&self) -> Result<&[u8]> {
        match self {
            Object::Name(v) => Ok(v),
            _ => Err(self.wrong("Name")),
        }
    }

    /// A string's bytes.
    pub(crate) fn as_str(&self) -> Result<&[u8]> {
        match self {
            Object::String(v, _) => Ok(v),
            _ => Err(self.wrong("String")),
        }
    }

    pub(crate) fn as_reference(&self) -> Result<ObjectId> {
        match self {
            Object::Reference(id) => Ok(*id),
            _ => Err(self.wrong("Reference")),
        }
    }

    pub(crate) fn as_array(&self) -> Result<&Vec<Object>> {
        match self {
            Object::Array(v) => Ok(v),
            _ => Err(self.wrong("Array")),
        }
    }

    pub(crate) fn as_dict(&self) -> Result<&Dictionary> {
        match self {
            Object::Dictionary(v) => Ok(v),
            _ => Err(self.wrong("Dictionary")),
        }
    }
}

impl Dictionary {
    pub(crate) fn has(&self, key: &[u8]) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    pub(crate) fn get(&self, key: &[u8]) -> Result<&Object> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v).ok_or_else(|| Error::DictKey(String::from_utf8_lossy(key).into_owned()))
    }

    /// The `/Type` name (or `Linearized` for a linearization dictionary).
    pub(crate) fn get_type(&self) -> Result<&[u8]> {
        self.get(b"Type").and_then(Object::as_name).or_else(|_| self.get(b"Linearized").map(|_| &b"Linearized"[..]))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Vec<u8>, &Object)> {
        self.0.iter().map(|(k, v)| (k, v))
    }
}

impl FromIterator<(Vec<u8>, Object)> for Dictionary {
    /// A repeated key keeps its last value, as a PDF reader does.
    fn from_iter<I: IntoIterator<Item = (Vec<u8>, Object)>>(iter: I) -> Self {
        let mut entries: Vec<(Vec<u8>, Object)> = Vec::new();
        for (key, value) in iter {
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }
        Dictionary(entries)
    }
}

/// Builders for test fixtures.
#[cfg(test)]
impl Dictionary {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Set `key`, replacing its value in place if it is already there.
    pub(crate) fn set(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Object>) {
        let (key, value) = (key.into(), value.into());
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

#[cfg(test)]
impl Object {
    pub(crate) fn as_stream(&self) -> Result<&Stream> {
        match self {
            Object::Stream(v) => Ok(v),
            _ => Err(self.wrong("Stream")),
        }
    }
}

#[cfg(test)]
impl From<i64> for Object {
    fn from(value: i64) -> Self {
        Object::Integer(value)
    }
}

#[cfg(test)]
impl From<Dictionary> for Object {
    fn from(value: Dictionary) -> Self {
        Object::Dictionary(value)
    }
}

#[cfg(test)]
impl<T: Into<Object>> From<Vec<T>> for Object {
    fn from(value: Vec<T>) -> Self {
        Object::Array(value.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_match_their_variants() {
        let d: Dictionary = [(b"Type".to_vec(), Object::Name(b"Page".to_vec())), (b"N".to_vec(), Object::Integer(3))].into_iter().collect();
        assert_eq!(d.get_type().unwrap(), b"Page");
        assert_eq!(d.get(b"N").and_then(Object::as_float).unwrap(), 3.0);
        assert!(matches!(d.get(b"N").and_then(Object::as_name), Err(Error::ObjectType { expected: "Name", found: "Integer" })));
        assert!(matches!(d.get(b"Missing"), Err(Error::DictKey(k)) if k == "Missing"));
        let lin: Dictionary = [(b"Linearized".to_vec(), Object::Integer(1))].into_iter().collect();
        assert_eq!(lin.get_type().unwrap(), b"Linearized");
    }

    #[test]
    fn a_repeated_key_keeps_its_last_value_in_its_first_place() {
        let d: Dictionary =
            [(b"A".to_vec(), Object::Integer(1)), (b"B".to_vec(), Object::Integer(2)), (b"A".to_vec(), Object::Integer(3))].into_iter().collect();
        assert_eq!(d.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap())).collect::<Vec<_>>(), [(b"A".to_vec(), 3), (b"B".to_vec(), 2)]);
    }
}
