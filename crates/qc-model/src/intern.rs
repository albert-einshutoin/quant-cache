//! String interning for compact trace representation.
//!
//! Deduplicates repeated strings (cache_key, object_id, content_type, etc.)
//! into `u32` IDs. Index 0 is reserved as the "None" sentinel.

use std::collections::HashMap;

/// A string interner that maps strings to unique `u32` IDs.
///
/// Index 0 is reserved for the empty/None sentinel.
/// Thread-unsafe (single-threaded replay).
#[derive(Debug, Clone)]
pub struct StringInterner {
    map: HashMap<String, u32>,
    strings: Vec<String>,
}

/// Sentinel ID representing None/empty for optional string fields.
pub const NONE_ID: u32 = 0;

impl StringInterner {
    /// Create a new interner with index 0 reserved for the empty sentinel.
    pub fn new() -> Self {
        let mut interner = Self {
            map: HashMap::new(),
            strings: Vec::new(),
        };
        // Reserve index 0 for None sentinel
        interner.strings.push(String::new());
        interner.map.insert(String::new(), 0);
        interner
    }

    /// Intern a string, returning its unique ID.
    /// Returns the existing ID if the string was previously interned.
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }

    /// Intern an optional string. Returns `NONE_ID` (0) for None.
    pub fn intern_option(&mut self, s: Option<&str>) -> u32 {
        match s {
            Some(s) => self.intern(s),
            None => NONE_ID,
        }
    }

    /// Resolve an ID back to its string. Panics on invalid ID.
    pub fn resolve(&self, id: u32) -> &str {
        &self.strings[id as usize]
    }

    /// Resolve an ID, returning None for the `NONE_ID` sentinel.
    pub fn resolve_option(&self, id: u32) -> Option<&str> {
        if id == NONE_ID {
            None
        } else {
            Some(&self.strings[id as usize])
        }
    }

    /// Number of interned strings (including the sentinel).
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the interner is empty (only sentinel).
    pub fn is_empty(&self) -> bool {
        self.strings.len() <= 1
    }
}

impl Default for StringInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_and_resolve() {
        let mut interner = StringInterner::new();
        let id1 = interner.intern("/img/logo.png");
        let id2 = interner.intern("/api/data");
        let id3 = interner.intern("/img/logo.png"); // duplicate

        assert_eq!(id1, id3); // same string → same ID
        assert_ne!(id1, id2);
        assert_eq!(interner.resolve(id1), "/img/logo.png");
        assert_eq!(interner.resolve(id2), "/api/data");
    }

    #[test]
    fn none_sentinel() {
        let mut interner = StringInterner::new();
        assert_eq!(interner.intern_option(None), NONE_ID);
        assert_eq!(interner.resolve_option(NONE_ID), None);

        let id = interner.intern_option(Some("hello"));
        assert_ne!(id, NONE_ID);
        assert_eq!(interner.resolve_option(id), Some("hello"));
    }

    #[test]
    fn empty_string_is_sentinel() {
        let interner = StringInterner::new();
        assert_eq!(interner.resolve(NONE_ID), "");
    }

    #[test]
    fn len_tracks_entries() {
        let mut interner = StringInterner::new();
        assert_eq!(interner.len(), 1); // sentinel only
        interner.intern("a");
        interner.intern("b");
        interner.intern("a"); // duplicate
        assert_eq!(interner.len(), 3); // sentinel + a + b
    }
}
