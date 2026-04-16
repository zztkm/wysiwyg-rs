use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Attribute value — covers the practical range of editor node/mark attributes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    String(Arc<str>),
    Int(i64),
    Bool(bool),
    Null,
}

/// Immutable attribute map shared via `Arc` for cheap cloning.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Attrs(pub Arc<BTreeMap<Arc<str>, AttrValue>>);

impl Attrs {
    pub fn empty() -> Self {
        Attrs(Arc::new(BTreeMap::new()))
    }

    pub fn get(&self, key: &str) -> Option<&AttrValue> {
        self.0.get(key)
    }

    /// Create a new `Attrs` with the given key-value pair added or replaced.
    pub fn with(self, key: impl Into<Arc<str>>, value: AttrValue) -> Self {
        let mut map = (*self.0).clone();
        map.insert(key.into(), value);
        Attrs(Arc::new(map))
    }
}

impl From<BTreeMap<Arc<str>, AttrValue>> for Attrs {
    fn from(map: BTreeMap<Arc<str>, AttrValue>) -> Self {
        Attrs(Arc::new(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_attrs() {
        let a = Attrs::empty();
        assert!(a.get("key").is_none());
    }

    #[test]
    fn with_adds_key() {
        let a = Attrs::empty()
            .with("level", AttrValue::Int(2))
            .with("lang", AttrValue::String("rust".into()));
        assert_eq!(a.get("level"), Some(&AttrValue::Int(2)));
        assert_eq!(a.get("lang"), Some(&AttrValue::String("rust".into())));
    }

    #[test]
    fn with_is_nondestructive() {
        let a = Attrs::empty().with("x", AttrValue::Bool(true));
        let b = a.clone().with("y", AttrValue::Int(1));
        // a is unchanged
        assert!(a.get("y").is_none());
        assert_eq!(b.get("x"), Some(&AttrValue::Bool(true)));
    }
}
