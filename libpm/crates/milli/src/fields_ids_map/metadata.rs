use std::collections::BTreeMap;

use heed::RoTxn;

use super::FieldsIdsMap;
use crate::attribute_patterns::PatternMatch;
use crate::{FieldId, Index, Result, Weight};

/// The field properties needed while building the fixed package index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metadata {
    searchable: (PatternMatch, Option<Weight>),
    asc_desc: PatternMatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldIdMapWithMetadata {
    fields_ids_map: FieldsIdsMap,
    builder: MetadataBuilder,
    metadata: BTreeMap<FieldId, Metadata>,
}

impl FieldIdMapWithMetadata {
    pub fn new(existing_fields_ids_map: FieldsIdsMap, builder: MetadataBuilder) -> Self {
        let metadata = existing_fields_ids_map
            .iter()
            .map(|(id, name)| (id, builder.metadata_for_field(name)))
            .collect();
        Self { fields_ids_map: existing_fields_ids_map, builder, metadata }
    }

    pub fn as_fields_ids_map(&self) -> &FieldsIdsMap {
        &self.fields_ids_map
    }

    pub fn insert(&mut self, name: &str) -> Option<FieldId> {
        let id = self.fields_ids_map.insert(name)?;
        self.metadata.insert(id, self.builder.metadata_for_field(name));
        Some(id)
    }

    pub fn id(&self, name: &str) -> Option<FieldId> {
        self.fields_ids_map.id(name)
    }

    pub fn id_with_metadata(&self, name: &str) -> Option<(FieldId, Metadata)> {
        let id = self.fields_ids_map.id(name)?;
        Some((id, self.metadata(id).unwrap()))
    }

    pub fn name(&self, id: FieldId) -> Option<&str> {
        self.fields_ids_map.name(id)
    }

    pub fn name_with_metadata(&self, id: FieldId) -> Option<(&str, Metadata)> {
        let name = self.fields_ids_map.name(id)?;
        Some((name, self.metadata(id).unwrap()))
    }

    pub fn metadata(&self, id: FieldId) -> Option<Metadata> {
        self.metadata.get(&id).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FieldId, &str, Metadata)> {
        self.fields_ids_map.iter().map(|(id, name)| (id, name, self.metadata(id).unwrap()))
    }

    pub fn iter_id_metadata(&self) -> impl Iterator<Item = (FieldId, Metadata)> + '_ {
        self.metadata.iter().map(|(id, metadata)| (*id, *metadata))
    }

}

impl Metadata {
    pub fn is_searchable(&self) -> PatternMatch {
        self.searchable.0
    }

    pub fn searchable_weight(&self) -> Option<Weight> {
        self.searchable.1
    }

    pub fn is_faceted(&self) -> PatternMatch {
        self.asc_desc
    }

    pub fn require_facet_level_database(&self) -> bool {
        self.asc_desc == PatternMatch::Match
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetadataBuilder;

impl MetadataBuilder {
    pub fn from_index(_index: &Index, _rtxn: &RoTxn) -> Result<Self> {
        Ok(Self)
    }

    pub fn metadata_for_field(&self, field: &str) -> Metadata {
        Metadata {
            searchable: self.is_searchable(field),
            asc_desc: match field {
                "traffic" | "id" => PatternMatch::Match,
                _ => PatternMatch::NoMatch,
            },
        }
    }

    fn is_searchable(&self, field: &str) -> (PatternMatch, Option<Weight>) {
        match field {
            "name" => (PatternMatch::Match, Some(0)),
            "description" => (PatternMatch::Match, Some(1)),
            _ => (PatternMatch::NoMatch, None),
        }
    }
}
