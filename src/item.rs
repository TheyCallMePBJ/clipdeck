//! A row in the list: either a cliphist history entry or a pin.

use crate::entry::{Entry, Kind};
use crate::pins::Pin;

/// Identifies a row across reloads, for keeping the selection, caching
/// previews and confirming deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemId {
    /// A cliphist id.
    History(u64),
    /// A pin's content hash.
    Pin(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    History(Entry),
    Pinned(Pin),
}

impl Item {
    pub fn id(&self) -> ItemId {
        match self {
            Item::History(entry) => ItemId::History(entry.id),
            Item::Pinned(pin) => ItemId::Pin(pin.hash.clone()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Item::History(entry) => &entry.label,
            Item::Pinned(pin) => &pin.label,
        }
    }

    pub fn kind(&self) -> &Kind {
        match self {
            Item::History(entry) => &entry.kind,
            Item::Pinned(pin) => &pin.kind,
        }
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self, Item::Pinned(_))
    }
}
