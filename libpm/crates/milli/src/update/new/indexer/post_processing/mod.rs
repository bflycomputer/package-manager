use std::collections::BTreeSet;

use facet_bulk::generate_facet_levels;
use fst::Streamer;
use heed::types::{Bytes, Str};
use heed::RwTxn;

use super::document_changes::IndexingContext;
use crate::facet::FacetType;
use crate::index::main_key::{WORDS_FST_KEY, WORDS_PREFIXES_FST_KEY};
use crate::progress::Progress;
use crate::update::new::indexer::{MiniString, WordDelta};
use crate::update::new::steps::{IndexingStep, PostProcessingFacets, PostProcessingWords};
use crate::update::new::word_fst_builder::{PrefixData, WordFstBuilder};
use crate::update::new::words_prefix_docids::{
    compute_word_prefix_docids, compute_word_prefix_fid_docids, compute_word_prefix_position_docids,
};
use crate::update::new::FacetFieldIdsDelta;
use crate::{GlobalFieldsIdsMap, Index, Result};

mod facet_bulk;

#[tracing::instrument(level = "trace", skip_all, target = "indexing::post_processing")]
pub(super) fn post_process(
    indexing_context: IndexingContext,
    wtxn: &mut RwTxn<'_>,
    mut global_fields_ids_map: GlobalFieldsIdsMap<'_>,
    word_delta: &WordDelta,
    facet_field_ids_delta: FacetFieldIdsDelta,
) -> Result<()> {
    let index = indexing_context.index;
    indexing_context.progress.update_progress(IndexingStep::PostProcessingFacets);
    compute_facet_level_database(
        index,
        wtxn,
        facet_field_ids_delta,
        &mut global_fields_ids_map,
        indexing_context.progress,
    )?;
    indexing_context.progress.update_progress(IndexingStep::PostProcessingWords);
    let prefix_data = compute_word_fst(index, wtxn, word_delta, indexing_context.progress)?;
    compute_prefix_database(index, wtxn, word_delta, &prefix_data, indexing_context.progress)?;

    Ok(())
}

#[tracing::instrument(
    level = "trace",
    skip_all,
    target = "indexing::post_processing",
    name = "prefix"
)]
fn compute_prefix_database(
    index: &Index,
    wtxn: &mut RwTxn,
    word_delta: &WordDelta,
    prefix_data: &PrefixData,
    progress: &Progress,
) -> Result<()> {
    progress.update_progress(PostProcessingWords::ComputePrefixes);
    let prefix_fst = fst::Set::new(&prefix_data.prefixes_fst_mmap[..])?;
    let modified = compute_prefixes(&prefix_fst, word_delta.words())?;
    compute_prefix_database_from_sources(index, wtxn, &modified, progress)
}

#[tracing::instrument(
    level = "trace",
    skip_all,
    target = "indexing::post_processing",
    name = "prefix_from_sources"
)]
pub(crate) fn compute_prefix_database_from_sources(
    index: &Index,
    wtxn: &mut RwTxn,
    modified: &BTreeSet<MiniString>,
    progress: &Progress,
) -> Result<()> {
    progress.update_progress(PostProcessingWords::WordPrefixDocids);
    compute_word_prefix_docids(wtxn, index, modified)?;

    progress.update_progress(PostProcessingWords::WordPrefixFieldIdDocids);
    compute_word_prefix_fid_docids(wtxn, index, modified)?;

    progress.update_progress(PostProcessingWords::WordPrefixPositionDocids);
    compute_word_prefix_position_docids(wtxn, index, modified)?;

    Ok(())
}

/// The words must be sorted.
fn compute_prefixes<'a, I>(prefix_fst: &fst::Set<&[u8]>, words: I) -> Result<BTreeSet<MiniString>>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut iter = words.into_iter();
    let mut prefix_stream = prefix_fst.stream();
    let mut current_prefix = match prefix_stream.next() {
        Some(current) => current,
        None => return Ok(BTreeSet::new()),
    };
    let mut current_word = match iter.next() {
        Some(current) => current,
        None => return Ok(BTreeSet::new()),
    };

    let mut output = BTreeSet::new();
    loop {
        // Current prefixes are only inserted once and each prefix is only inserted once.
        if current_word.as_bytes().starts_with(current_prefix) {
            let current_prefix = std::str::from_utf8(current_prefix)?;
            // safety: Prefixes are 3 bytes or less
            let current_prefix = MiniString::new(current_prefix).unwrap();
            output.insert(current_prefix);
        }

        if current_word.as_bytes() < current_prefix {
            current_word = match iter.next() {
                Some(current) => current,
                None => break,
            };
        } else {
            current_prefix = match prefix_stream.next() {
                Some(current) => current,
                None => break,
            };
        }
    }

    Ok(output)
}

#[tracing::instrument(level = "trace", skip_all, target = "indexing::post_processing")]
fn compute_word_fst(
    index: &Index,
    wtxn: &mut RwTxn,
    word_delta: &WordDelta,
    progress: &Progress,
) -> Result<PrefixData> {
    progress.update_progress(PostProcessingWords::WordFst);

    let mut word_fst_builder = WordFstBuilder::new()?;
    for word in word_delta.words() {
        word_fst_builder.register_word(word.as_bytes())?;
    }

    let (word_fst_mmap, prefix_data) = word_fst_builder.build()?;
    index.main.remap_types::<Str, Bytes>().put(wtxn, WORDS_FST_KEY, &word_fst_mmap)?;

    let PrefixData { prefixes_fst_mmap } = prefix_data;
    index.main.remap_types::<Str, Bytes>().put(wtxn, WORDS_PREFIXES_FST_KEY, &prefixes_fst_mmap)?;
    Ok(PrefixData { prefixes_fst_mmap })
}

#[tracing::instrument(
    level = "trace",
    skip_all,
    target = "indexing::post_processing",
    name = "facet_field_ids"
)]
fn compute_facet_level_database(
    index: &Index,
    wtxn: &mut RwTxn,
    mut facet_field_ids_delta: FacetFieldIdsDelta,
    global_fields_ids_map: &mut GlobalFieldsIdsMap,
    progress: &Progress,
) -> Result<()> {
    for fid in facet_field_ids_delta.consume_facet_string_delta() {
        // skip field ids that should not be facet leveled
        let Some(metadata) = global_fields_ids_map.metadata(fid) else {
            continue;
        };

        // Note in case of a settings change we will recompute the facet level database if the
        // user only enabled the facet search and the field is marked as comparable or sortable.
        if !metadata.require_facet_level_database() {
            continue;
        }

        let span =
            tracing::trace_span!(target: "indexing::post_processing::facet_field_ids", "string");
        let _entered = span.enter();
        progress.update_progress(PostProcessingFacets::StringsBulk);
        tracing::debug!(%fid, "bulk string facet processing in parallel");
        generate_facet_levels(index, wtxn, fid, FacetType::String)?;
    }

    for fid in facet_field_ids_delta.consume_facet_number_delta() {
        let span =
            tracing::trace_span!(target: "indexing::post_processing::facet_field_ids", "number");
        let _entered = span.enter();
        progress.update_progress(PostProcessingFacets::NumbersBulk);
        tracing::debug!(%fid, "bulk number facet processing");
        generate_facet_levels(index, wtxn, fid, FacetType::Number)?;
    }

    Ok(())
}
