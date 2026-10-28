use std::cell::RefCell;
use std::ops::DerefMut as _;

use bumpalo::collections::Vec as BVec;
use bumpalo::Bump;
use hashbrown::HashMap;
use serde_json::Value;

use super::super::cache::BalancedCaches;
use super::facet_document::extract_document_facets;
use super::FacetKind;
use crate::fields_ids_map::metadata::Metadata;
use crate::heed_codec::facet::OrderedF64Codec;
use crate::update::new::channel::FieldIdDocidFacetSender;
use crate::update::new::document::DocumentContext;
use crate::update::new::extract::perm_json_p;
use crate::update::new::indexer::document_changes::{
    extract, DocumentChanges, Extractor, IndexingContext,
};
use crate::update::new::ref_cell_ext::RefCellExt as _;
use crate::update::new::steps::IndexingStep;
use crate::update::new::thread_local::{FullySend, ThreadLocal};
use crate::update::new::DocumentChange;
use crate::update::GrenadParameters;
use crate::{DocumentId, FieldId, PatternMatch, Result, UserError, MAX_FACET_VALUE_LENGTH};

pub struct FacetedExtractorData<'a, 'b> {
    sender: &'a FieldIdDocidFacetSender<'a, 'b>,
    grenad_parameters: &'a GrenadParameters,
    buckets: usize,
}

impl<'extractor> Extractor<'extractor> for FacetedExtractorData<'_, '_> {
    type Data = RefCell<BalancedCaches<'extractor>>;

    fn init_data(&self, extractor_alloc: &'extractor Bump) -> Result<Self::Data> {
        Ok(RefCell::new(BalancedCaches::new_in(
            self.buckets,
            self.grenad_parameters.max_memory_by_thread(),
            extractor_alloc,
        )))
    }

    fn process<'doc>(
        &self,
        changes: impl Iterator<Item = Result<DocumentChange<'doc>>>,
        context: &DocumentContext<Self::Data>,
    ) -> Result<()> {
        for change in changes {
            let change = change?;
            FacetedDocidsExtractor::extract_document_change(
                context,
                change,
                self.sender,
            )?
        }
        Ok(())
    }
}

pub struct FacetedDocidsExtractor;

impl FacetedDocidsExtractor {
    #[allow(clippy::too_many_arguments)]
    fn extract_document_change(
        context: &DocumentContext<RefCell<BalancedCaches>>,
        document_change: DocumentChange,
        sender: &FieldIdDocidFacetSender,
    ) -> Result<()> {
        let mut new_fields_ids_map = context.new_fields_ids_map.borrow_mut_or_yield();
        let mut cached_sorter = context.data.borrow_mut_or_yield();
        let mut facet_values = FacetValues::new(&context.doc_alloc);
        let docid = document_change.docid();

        // Using a macro avoid borrowing the parameters as mutable in both closures at
        // the same time by postponing their creation
        macro_rules! facet_fn {
            (add) => {
                |fid: FieldId, meta: Metadata, depth: perm_json_p::Depth, value: &Value| {
                    Self::facet_fn_with_options(
                        &context.doc_alloc,
                        cached_sorter.deref_mut(),
                        BalancedCaches::insert_add_u32,
                        &mut facet_values,
                        FacetValues::insert,
                        docid,
                        fid,
                        meta,
                        depth,
                        value,
                    )
                }
            };
        }

        let mut add = facet_fn!(add);
        extract_document_facets(
            document_change.inserted(),
            |field_name| match field_name {
                "traffic" | "id" => PatternMatch::Match,
                _ => PatternMatch::NoMatch,
            },
            &mut |name| {
                new_fields_ids_map
                    .id_with_metadata_or_insert(name)
                    .ok_or(UserError::AttributeLimitReached.into())
            },
            &mut add,
        )?;

        facet_values.send_data(docid, sender, &context.doc_alloc).unwrap();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn facet_fn_with_options<'extractor, 'doc>(
        doc_alloc: &'doc Bump,
        cached_sorter: &mut BalancedCaches<'extractor>,
        cache_fn: impl Fn(&mut BalancedCaches<'extractor>, &[u8], u32) -> Result<()>,
        facet_values: &mut FacetValues<'doc>,
        facet_fn: impl Fn(&mut FacetValues<'doc>, FieldId, BVec<'doc, u8>, FacetKind),
        docid: DocumentId,
        fid: FieldId,
        meta: Metadata,
        _depth: perm_json_p::Depth,
        value: &Value,
    ) -> Result<()> {
        // if the field is not faceted, do nothing
        if meta.is_faceted() != PatternMatch::Match {
            return Ok(());
        }

        let mut buffer = BVec::new_in(doc_alloc);
        match value {
            // Number
            // key: fid - level - orderedf64 - originalf64
            Value::Number(number) => {
                let mut ordered = [0u8; 16];
                if number
                    .as_f64()
                    .and_then(|n| OrderedF64Codec::serialize_into(n, &mut ordered).ok())
                    .is_some()
                {
                    let mut number = BVec::with_capacity_in(16, doc_alloc);
                    number.extend_from_slice(&ordered);
                    facet_fn(facet_values, fid, number, FacetKind::Number);

                    buffer.clear();
                    buffer.push(FacetKind::Number as u8);
                    buffer.extend_from_slice(&fid.to_be_bytes());
                    buffer.push(0); // level 0
                    buffer.extend_from_slice(&ordered);
                    cache_fn(cached_sorter, &buffer, docid)
                } else {
                    Ok(())
                }
            }
            // String
            // key: fid - level - truncated_string
            Value::String(s) if !s.is_empty() => {
                let mut string = BVec::new_in(doc_alloc);
                string.extend_from_slice(s.as_bytes());
                facet_fn(facet_values, fid, string, FacetKind::String);

                let normalized = crate::normalize_facet(s);
                let truncated = truncate_str(&normalized);
                buffer.clear();
                buffer.push(FacetKind::String as u8);
                buffer.extend_from_slice(&fid.to_be_bytes());
                buffer.push(0); // level 0
                buffer.extend_from_slice(truncated.as_bytes());
                cache_fn(cached_sorter, &buffer, docid)
            }
            // Bool is handled as a string
            Value::Bool(b) => {
                let b = if *b { "true" } else { "false" };
                let mut string = BVec::new_in(doc_alloc);
                string.extend_from_slice(b.as_bytes());
                facet_fn(facet_values, fid, string, FacetKind::String);

                buffer.clear();
                buffer.push(FacetKind::String as u8);
                buffer.extend_from_slice(&fid.to_be_bytes());
                buffer.push(0); // level 0
                buffer.extend_from_slice(b.as_bytes());
                cache_fn(cached_sorter, &buffer, docid)
            }
            // Otherwise, do nothing
            _ => Ok(()),
        }
    }
}

struct FacetValues<'doc> {
    strings:
        HashMap<(FieldId, &'doc str), BVec<'doc, u8>, hashbrown::DefaultHashBuilder, &'doc Bump>,
    f64s: HashMap<(FieldId, BVec<'doc, u8>), (), hashbrown::DefaultHashBuilder, &'doc Bump>,
    doc_alloc: &'doc Bump,
}

impl<'doc> FacetValues<'doc> {
    fn new(doc_alloc: &'doc Bump) -> Self {
        Self { strings: HashMap::new_in(doc_alloc), f64s: HashMap::new_in(doc_alloc), doc_alloc }
    }

    fn insert(&mut self, fid: FieldId, value: BVec<'doc, u8>, kind: FacetKind) {
        match kind {
            FacetKind::Number => {
                self.f64s.insert((fid, value), ());
            }
            FacetKind::String => {
                if let Ok(s) = std::str::from_utf8(&value) {
                    let normalized = crate::normalize_facet(s);
                    let truncated = self.doc_alloc.alloc_str(truncate_str(&normalized));
                    self.strings.insert((fid, truncated), value);
                }
            }
        }
    }

    fn send_data(
        self,
        docid: DocumentId,
        sender: &FieldIdDocidFacetSender,
        doc_alloc: &Bump,
    ) -> crate::Result<()> {
        let mut buffer = bumpalo::collections::Vec::new_in(doc_alloc);
        for ((fid, truncated), value) in self.strings {
            buffer.clear();
            buffer.extend_from_slice(&fid.to_be_bytes());
            buffer.extend_from_slice(&docid.to_be_bytes());
            buffer.extend_from_slice(truncated.as_bytes());
            sender.write_facet_string(&buffer, &value)?;
        }

        for ((fid, value), ()) in self.f64s {
            buffer.clear();
            buffer.extend_from_slice(&fid.to_be_bytes());
            buffer.extend_from_slice(&docid.to_be_bytes());
            buffer.extend_from_slice(&value);
            sender.write_facet_f64(&buffer)?;
        }

        Ok(())
    }
}

/// Truncates a string to the biggest valid LMDB key size.
fn truncate_str(s: &str) -> &str {
    let index = s
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(s.len()))
        .take_while(|idx| idx <= &MAX_FACET_VALUE_LENGTH)
        .last();

    &s[..index.unwrap_or(0)]
}

impl FacetedDocidsExtractor {
    #[tracing::instrument(level = "trace", skip_all, target = "indexing::extract::faceted")]
    pub fn run_extraction<'pl, 'fid, 'indexer, 'index, 'extractor, DC: DocumentChanges<'pl>>(
        document_changes: &DC,
        indexing_context: IndexingContext<'fid, 'indexer, 'index>,
        extractor_allocs: &'extractor mut ThreadLocal<FullySend<Bump>>,
        sender: &FieldIdDocidFacetSender,
        step: IndexingStep,
    ) -> Result<Vec<BalancedCaches<'extractor>>> {
        let datastore = ThreadLocal::new();

        {
            let span =
                tracing::trace_span!(target: "indexing::documents::extract", "docids_extraction");
            let _entered = span.enter();

            let extractor = FacetedExtractorData {
                grenad_parameters: indexing_context.grenad_parameters,
                buckets: rayon::current_num_threads(),
                sender,
            };
            extract(
                document_changes,
                &extractor,
                indexing_context,
                extractor_allocs,
                &datastore,
                step,
            )?;
        }

        Ok(datastore.into_iter().map(RefCell::into_inner).collect())
    }
}
