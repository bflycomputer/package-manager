use std::sync::atomic::{AtomicBool, Ordering};

use bumpalo::Bump;
use roaring::RoaringBitmap;
use tracing::Span;

use super::super::channel::*;
use super::super::extract::*;
use super::super::steps::IndexingStep;
use super::super::thread_local::{FullySend, ThreadLocal};
use super::super::FacetFieldIdsDelta;
use super::document_changes::{extract, DocumentChanges, IndexingContext};
use crate::progress::MergingWordCache;
use crate::update::new::indexer::WordDelta;
use crate::update::new::merger::merge_scan_and_send_docids;
use crate::update::new::{merge_and_send_docids, merge_and_send_facet_docids};
use crate::Result;

#[allow(clippy::too_many_arguments)]
pub(super) fn extract_all<'pl, 'extractor, DC>(
    document_changes: &DC,
    indexing_context: IndexingContext,
    indexer_span: Span,
    extractor_sender: ExtractorBbqueueSender,
    extractor_allocs: &'extractor mut ThreadLocal<FullySend<Bump>>,
    finished_extraction: &AtomicBool,
    document_ids: &mut RoaringBitmap,
) -> Result<(FacetFieldIdsDelta, WordDelta)>
where
    DC: DocumentChanges<'pl>,
{
    let span =
        tracing::trace_span!(target: "indexing::documents", parent: &indexer_span, "extract");
    let _entered = span.enter();

    // document but we need to create a function that collects and compresses documents.
    let document_sender = extractor_sender.documents();
    let document_extractor = DocumentsExtractor::new(document_sender);
    let datastore = ThreadLocal::with_capacity(rayon::current_num_threads());
    {
        let span = tracing::trace_span!(target: "indexing::documents::extract", parent: &indexer_span, "documents");
        let _entered = span.enter();
        extract(
            document_changes,
            &document_extractor,
            indexing_context,
            extractor_allocs,
            &datastore,
            IndexingStep::ExtractingDocuments,
        )?;
    }
    {
        let span = tracing::trace_span!(target: "indexing::documents::merge", parent: &indexer_span, "documents");
        let _entered = span.enter();
        for document_extractor_data in datastore {
            let document_extractor_data = document_extractor_data.0.into_inner();
            *document_ids |= document_extractor_data.document_ids;
        }
    }

    let facet_field_ids_delta;
    let word_delta;

    {
        let caches = {
            let span = tracing::trace_span!(target: "indexing::documents::extract", parent: &indexer_span, "faceted");
            let _entered = span.enter();

            FacetedDocidsExtractor::run_extraction(
                document_changes,
                indexing_context,
                extractor_allocs,
                &extractor_sender.field_id_docid_facet_sender(),
                IndexingStep::ExtractingFacets,
            )?
        };

        {
            let span = tracing::trace_span!(target: "indexing::documents::merge", parent: &indexer_span, "faceted");
            let _entered = span.enter();
            indexing_context.progress.update_progress(IndexingStep::MergingFacetCaches);

            facet_field_ids_delta =
                merge_and_send_facet_docids(caches, extractor_sender.facet_docids())?;
        }
    }

    {
        let WordDocidsCaches {
            word_docids,
            word_fid_docids,
            word_position_docids,
            fid_word_count_docids,
        } = {
            let span = tracing::trace_span!(target: "indexing::documents::extract", "word_docids");
            let _entered = span.enter();
            WordDocidsExtractors::run_extraction(
                document_changes,
                indexing_context,
                extractor_allocs,
                IndexingStep::ExtractingWords,
            )?
        };

        indexing_context.progress.update_progress(IndexingStep::MergingWordCaches);

        {
            let span = tracing::trace_span!(target: "indexing::documents::merge", "word_docids");
            let _entered = span.enter();
            indexing_context.progress.update_progress(MergingWordCache::WordDocids);

            word_delta = merge_scan_and_send_docids(
                word_docids,
                extractor_sender.docids::<WordDocids>(),
                |output: &mut WordDelta, key| {
                    let word = std::str::from_utf8(key)?.to_string();
                    output.insert(word);
                    Ok(())
                },
                indexing_context.must_stop_processing,
            )?;
        }

        {
            let span =
                tracing::trace_span!(target: "indexing::documents::merge", "word_fid_docids");
            let _entered = span.enter();
            indexing_context.progress.update_progress(MergingWordCache::WordFieldIdDocids);

            merge_and_send_docids(
                word_fid_docids,
                extractor_sender.docids::<WordFidDocids>(),
                indexing_context.must_stop_processing,
            )?;
        }

        {
            let span =
                tracing::trace_span!(target: "indexing::documents::merge", "word_position_docids");
            let _entered = span.enter();
            indexing_context.progress.update_progress(MergingWordCache::WordPositionDocids);

            merge_and_send_docids(
                word_position_docids,
                extractor_sender.docids::<WordPositionDocids>(),
                indexing_context.must_stop_processing,
            )?;
        }

        {
            let span =
                tracing::trace_span!(target: "indexing::documents::merge", "fid_word_count_docids");
            let _entered = span.enter();
            indexing_context.progress.update_progress(MergingWordCache::FieldIdWordCountDocids);

            merge_and_send_docids(
                fid_word_count_docids,
                extractor_sender.docids::<FidWordCountDocids>(),
                indexing_context.must_stop_processing,
            )?;
        }
    }

    let caches = {
        let span = tracing::trace_span!(target: "indexing::documents::extract", "word_pair_proximity_docids");
        let _entered = span.enter();

        WordPairProximityDocidsExtractor::run_extraction(
            document_changes,
            indexing_context,
            extractor_allocs,
            IndexingStep::ExtractingWordProximity,
        )?
    };

    {
        let span = tracing::trace_span!(target: "indexing::documents::merge", "word_pair_proximity_docids");
        let _entered = span.enter();
        indexing_context.progress.update_progress(IndexingStep::MergingWordProximity);

        merge_and_send_docids(
            caches,
            extractor_sender.docids::<WordPairProximityDocids>(),
            indexing_context.must_stop_processing,
        )?;
    }

    indexing_context.progress.update_progress(IndexingStep::WaitingForDatabaseWrites);
    finished_extraction.store(true, Ordering::Relaxed);

    Result::Ok((facet_field_ids_delta, word_delta))
}
