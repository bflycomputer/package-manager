//! Fixed index configuration for the package catalog.
//!
//! This replaces Meilisearch's general settings API in the stripped engine. The
//! catalog has one schema and one ranking policy, so runtime settings
//! mutation would only retain reindexing, vector, chat, and filter machinery
//! that this package manager never calls.

use crate::{Index, Result, UserError};

pub fn configure(index: &Index, wtxn: &mut heed::RwTxn<'_>) -> Result<()> {
    let mut fields_ids_map = index.fields_ids_map(wtxn)?;
    fields_ids_map
        .insert("id")
        .ok_or(UserError::AttributeLimitReached)?;
    index.put_fields_ids_map(wtxn, &fields_ids_map)?;
    index.put_primary_key(wtxn, "id")?;

    Ok(())
}
