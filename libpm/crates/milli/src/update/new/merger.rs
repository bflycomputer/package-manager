use std::ops::BitOr;

use hashbrown::HashSet;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use super::channel::*;
use super::extract::{merge_caches_sorted, transpose_and_freeze_caches, BalancedCaches, FacetKind};
use crate::{FieldId, InternalError, MustStopProcessing, Result};

#[tracing::instrument(level = "trace", skip_all, target = "indexing::merge")]
pub fn merge_and_send_docids<D>(
    caches: Vec<BalancedCaches<'_>>,
    docids_sender: WordDocidsSender<D>,
    must_stop_processing: &MustStopProcessing,
) -> Result<()>
where
    D: DatabaseType + Sync,
{
    merge_scan_and_send_docids(
        caches,
        docids_sender,
        // bool: BitOr + Default + Send + Sync
        |_: &mut bool, _| Ok(()),
        must_stop_processing,
    )
    .map(drop)
}

#[tracing::instrument(level = "trace", skip_all, target = "indexing::merge")]
pub fn merge_scan_and_send_docids<D, CP, St>(
    mut caches: Vec<BalancedCaches<'_>>,
    docids_sender: WordDocidsSender<D>,
    scan: CP,
    must_stop_processing: &MustStopProcessing,
) -> Result<St>
where
    D: DatabaseType + Sync,
    St: Default + BitOr<Output = St> + Sync + Send,
    CP: Fn(&mut St, &[u8]) -> Result<()> + Sync + Send,
{
    transpose_and_freeze_caches(&mut caches)?
        .into_par_iter()
        .map(|frozen| -> Result<_> {
            if must_stop_processing.get() {
                return Err(InternalError::AbortedIndexation.into());
            }

            let mut output = St::default();
            merge_caches_sorted(frozen, |key, bitmap| {
                scan(&mut output, key)?;
                docids_sender.write(key, &bitmap)
            })?;

            Ok(output)
        })
        .try_reduce(Default::default, |lhs, rhs| Ok(lhs | rhs))
}

#[tracing::instrument(level = "trace", skip_all, target = "indexing::merge")]
pub fn merge_and_send_facet_docids(
    mut caches: Vec<BalancedCaches<'_>>,
    docids_sender: FacetDocidsSender,
) -> Result<FacetFieldIdsDelta> {
    transpose_and_freeze_caches(&mut caches)?
        .into_par_iter()
        .map(|frozen| {
            let mut facet_field_ids_delta = FacetFieldIdsDelta::new();
            merge_caches_sorted(frozen, |key, bitmap| {
                facet_field_ids_delta.register_from_key(key);
                docids_sender.write(key, &bitmap)
            })?;
            Ok(facet_field_ids_delta)
        })
        .reduce(|| Ok(FacetFieldIdsDelta::new()), |lhs, rhs| Ok(lhs?.merge(rhs?)))
}

#[derive(Debug)]
pub struct FacetFieldIdsDelta {
    modified_facet_string_ids: HashSet<FieldId, rustc_hash::FxBuildHasher>,
    modified_facet_number_ids: HashSet<FieldId, rustc_hash::FxBuildHasher>,
}

impl FacetFieldIdsDelta {
    pub fn new() -> Self {
        Self {
            modified_facet_string_ids: Default::default(),
            modified_facet_number_ids: Default::default(),
        }
    }

    fn register_facet_string_id(&mut self, field_id: FieldId) {
        self.modified_facet_string_ids.insert(field_id);
    }

    fn register_facet_number_id(&mut self, field_id: FieldId) {
        self.modified_facet_number_ids.insert(field_id);
    }

    fn register_from_key(&mut self, key: &[u8]) {
        let (facet_kind, field_id, facet_value) = self.extract_key_data(key);
        match (facet_kind, facet_value) {
            (FacetKind::Number, Some(_)) => self.register_facet_number_id(field_id),
            (FacetKind::String, Some(_)) => self.register_facet_string_id(field_id),
            _ => (),
        }
    }

    fn extract_key_data<'key>(&self, key: &'key [u8]) -> (FacetKind, FieldId, Option<&'key [u8]>) {
        let facet_kind = FacetKind::from(key[0]);
        let field_id = FieldId::from_be_bytes([key[1], key[2]]);
        let facet_value = if key.len() >= 4 {
            // level is also stored in the key at [3] (always 0)
            Some(&key[4..])
        } else {
            None
        };

        (facet_kind, field_id, facet_value)
    }

    pub fn consume_facet_string_delta(&mut self) -> impl Iterator<Item = FieldId> + '_ {
        self.modified_facet_string_ids.drain()
    }

    pub fn consume_facet_number_delta(&mut self) -> impl Iterator<Item = FieldId> + '_ {
        self.modified_facet_number_ids.drain()
    }

    pub fn merge(mut self, rhs: Self) -> Self {
        let Self { modified_facet_number_ids, modified_facet_string_ids, .. } = rhs;
        self.modified_facet_number_ids.extend(modified_facet_number_ids);
        self.modified_facet_string_ids.extend(modified_facet_string_ids);
        self
    }
}
