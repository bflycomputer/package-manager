use bumpalo::Bump;
use bumparaw_collections::RawMap;
use rustc_hash::FxBuildHasher;
use serde_json::value::RawValue;
use serde_json::Value;

use crate::{FieldsIdsMap, Object, Result, UserError};

#[derive(Debug, Clone, Copy)]
pub struct PrimaryKey<'a> {
    name: &'a str,
}

impl<'a> PrimaryKey<'a> {
    pub fn new_or_insert(name: &'a str, fields: &mut FieldsIdsMap) -> Result<Self> {
        fields.insert(name).ok_or(UserError::AttributeLimitReached)?;
        Ok(Self { name })
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn extract_fields_and_docid<'payload, 'bump: 'payload>(
        &self,
        document: &'payload RawValue,
        fields: &mut FieldsIdsMap,
        bump: &'bump Bump,
    ) -> Result<&'bump str> {
        let document_map = RawMap::from_raw_value_and_hasher(document, FxBuildHasher, bump)
            .map_err(UserError::SerdeJson)?;

        for (name, _) in document_map.iter() {
            fields.insert(name).ok_or(UserError::AttributeLimitReached)?;
        }

        let Some(raw_id) = document_map.get(self.name) else {
            return Err(UserError::MissingDocumentId {
                primary_key: self.name.to_owned(),
                document: serde_json::from_str(document.get()).unwrap_or_else(|_| Object::new()),
            }
            .into());
        };
        let value: Value = serde_json::from_str(raw_id.get()).map_err(UserError::SerdeJson)?;
        let id = match value {
            Value::String(id) if valid_document_id(&id) => id,
            Value::Number(number) if !number.is_f64() => number.to_string(),
            document_id => return Err(UserError::InvalidDocumentId { document_id }.into()),
        };

        Ok(bump.alloc_str(&id))
    }
}

fn valid_document_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 512
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
