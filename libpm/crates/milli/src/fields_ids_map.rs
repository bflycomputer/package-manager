use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::FieldId;

mod global;
pub mod metadata;
pub use global::GlobalFieldsIdsMap;
pub use metadata::{FieldIdMapWithMetadata, MetadataBuilder};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldsIdsMap {
    names_ids: BTreeMap<String, FieldId>,
    ids_names: BTreeMap<FieldId, String>,
    next_id: Option<FieldId>,
}

impl FieldsIdsMap {
    pub fn new() -> FieldsIdsMap {
        FieldsIdsMap { names_ids: BTreeMap::new(), ids_names: BTreeMap::new(), next_id: Some(0) }
    }

    /// Returns the field id related to a field name, it will create a new field id if the
    /// name is not already known. Returns `None` if the maximum field id as been reached.
    pub fn insert(&mut self, name: &str) -> Option<FieldId> {
        match self.names_ids.get(name) {
            Some(id) => Some(*id),
            None => {
                let id = self.next_id?;
                self.next_id = id.checked_add(1);
                self.names_ids.insert(name.to_owned(), id);
                self.ids_names.insert(id, name.to_owned());
                Some(id)
            }
        }
    }

    /// Get the id of a field based on its name.
    pub fn id(&self, name: &str) -> Option<FieldId> {
        self.names_ids.get(name).copied()
    }

    /// Get the name of a field based on its id.
    pub fn name(&self, id: FieldId) -> Option<&str> {
        self.ids_names.get(&id).map(String::as_str)
    }

    /// Iterate over the ids and names in the ids order.
    pub fn iter(&self) -> impl Iterator<Item = (FieldId, &str)> {
        self.ids_names.iter().map(|(id, name)| (*id, name.as_str()))
    }

}

impl Default for FieldsIdsMap {
    fn default() -> FieldsIdsMap {
        FieldsIdsMap::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_ids_map() {
        let mut map = FieldsIdsMap::new();

        assert_eq!(map.insert("id"), Some(0));
        assert_eq!(map.insert("title"), Some(1));
        assert_eq!(map.insert("description"), Some(2));
        assert_eq!(map.insert("id"), Some(0));
        assert_eq!(map.insert("title"), Some(1));
        assert_eq!(map.insert("description"), Some(2));

        assert_eq!(map.id("id"), Some(0));
        assert_eq!(map.id("title"), Some(1));
        assert_eq!(map.id("description"), Some(2));
        assert_eq!(map.id("date"), None);

        assert_eq!(map.len(), 3);

        assert_eq!(map.name(0), Some("id"));
        assert_eq!(map.name(1), Some("title"));
        assert_eq!(map.name(2), Some("description"));
        assert_eq!(map.name(4), None);

        assert_eq!(map.remove("title"), Some(1));

        assert_eq!(map.id("title"), None);
        assert_eq!(map.insert("title"), Some(3));
        assert_eq!(map.len(), 3);

        let mut iter = map.iter();
        assert_eq!(iter.next(), Some((0, "id")));
        assert_eq!(iter.next(), Some((2, "description")));
        assert_eq!(iter.next(), Some((3, "title")));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn nested_fields() {
        let mut map = FieldsIdsMap::new();

        assert_eq!(map.insert("id"), Some(0));
        assert_eq!(map.insert("doggo"), Some(1));
        assert_eq!(map.insert("doggo.name"), Some(2));
        assert_eq!(map.insert("doggolution"), Some(3));
        assert_eq!(map.insert("doggo.breed.name"), Some(4));
        assert_eq!(map.insert("description"), Some(5));

        insta::assert_debug_snapshot!(map.nested_ids("doggo"), @r###"
        [
            1,
            4,
            2,
        ]
        "###);

        insta::assert_debug_snapshot!(map.nested_ids("doggo.breed"), @r###"
        [
            4,
        ]
        "###);

        insta::assert_debug_snapshot!(map.nested_ids("_vector"), @"[]");
    }
}
