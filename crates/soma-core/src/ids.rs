//! Opaque string identifiers. Entities, anchors and systems use ULIDs; relation
//! kinds and documents use stable human-readable / content-hash ids.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

id_type!(
    /// A node or a relation.
    EntityId
);
id_type!(AnchorId);
id_type!(SystemId);
id_type!(
    /// e.g. `prerequisite-of`.
    KindId
);
id_type!(
    /// blake3 hex of the document bytes at capture time.
    DocumentId
);

pub fn new_ulid() -> String {
    ulid::Ulid::generate().to_string()
}

impl EntityId {
    pub fn generate() -> Self {
        Self(new_ulid())
    }
}

impl AnchorId {
    pub fn generate() -> Self {
        Self(new_ulid())
    }
}

impl SystemId {
    pub fn generate() -> Self {
        Self(new_ulid())
    }
}
