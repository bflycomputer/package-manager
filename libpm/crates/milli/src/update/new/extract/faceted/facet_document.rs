use serde_json::Value;

use crate::attribute_patterns::PatternMatch;
use crate::fields_ids_map::metadata::Metadata;
use crate::update::new::document::Document;
use crate::update::new::extract::perm_json_p;
use crate::{FieldId, InternalError, Result};

#[allow(clippy::too_many_arguments)]
pub fn extract_document_facets<'doc>(
    document: impl Document<'doc>,
    // return the match result for the given field name.
    match_field: impl Fn(&str) -> PatternMatch,
    register_field: &mut impl FnMut(&str) -> Result<(FieldId, Metadata)>,
    facet_fn: &mut impl FnMut(FieldId, Metadata, perm_json_p::Depth, &Value) -> Result<()>,
) -> Result<()> {
    // extract the field if it is faceted (facet searchable, filterable, sortable)
    let mut extract_field = |name: &str, depth: perm_json_p::Depth, value: &Value| -> Result<()> {
        let (field_id, meta) = register_field(name)?;
        facet_fn(field_id, meta, depth, value)
    };

    for res in document.iter_top_level_fields() {
        let (field_name, value) = res?;
        let selection = match_field(field_name);

        // extract the field if it matches a pattern and if it is faceted (facet searchable, filterable, sortable)
        let mut match_and_extract = |name: &str, depth: perm_json_p::Depth, value: &Value| {
            let selection = match_field(name);
            if selection == PatternMatch::Match {
                extract_field(name, depth, value)?;
            }

            Ok(selection)
        };

        if selection != PatternMatch::NoMatch {
            // parse json.
            match serde_json::value::to_value(value).map_err(InternalError::SerdeJson)? {
                Value::Object(object) => {
                    perm_json_p::seek_leaf_values_in_object(
                        &object,
                        field_name,
                        perm_json_p::Depth::OnBaseKey,
                        &mut match_and_extract,
                    )?;

                    if selection == PatternMatch::Match {
                        extract_field(
                            field_name,
                            perm_json_p::Depth::OnBaseKey,
                            &Value::Object(object),
                        )?;
                    }
                }
                Value::Array(array) => {
                    perm_json_p::seek_leaf_values_in_array(
                        &array,
                        field_name,
                        perm_json_p::Depth::OnBaseKey,
                        &mut match_and_extract,
                    )?;

                    if selection == PatternMatch::Match {
                        extract_field(
                            field_name,
                            perm_json_p::Depth::OnBaseKey,
                            &Value::Array(array),
                        )?;
                    }
                }
                value => extract_field(field_name, perm_json_p::Depth::OnBaseKey, &value)?,
            }
        }
    }

    Ok(())
}
