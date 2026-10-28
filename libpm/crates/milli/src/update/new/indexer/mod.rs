use std::sync::atomic::AtomicBool;
use std::sync::{Once, RwLock};
use std::thread::{self, Builder};

use big_s::S;
use document_changes::{DocumentChanges, IndexingContext};
pub use document_operation::{IndexOperations, PayloadStats};
use heed::RwTxn;
pub use mini_string::MiniString;
pub use word_delta::WordDelta;
use write::write_to_db;
pub use write::{update_index, ChannelCongestion};

use super::channel::*;
use super::steps::IndexingStep;
use super::thread_local::ThreadLocal;
use crate::documents::PrimaryKey;
use crate::fields_ids_map::metadata::{FieldIdMapWithMetadata, MetadataBuilder};
use crate::progress::Progress;
use crate::update::GrenadParameters;
use crate::{
    FieldsIdsMap, GlobalFieldsIdsMap, Index, MustStopProcessing, Result, ThreadPoolNoAbort,
};

pub mod document_changes;
mod document_operation;
mod extract;
mod mini_string;
mod post_processing;
mod word_delta;
mod write;

static LOG_MEMORY_METRICS_ONCE: Once = Once::new();

/// This is the main function of this crate.
///
/// Give it the output of the [`Indexer::document_changes`] method and it will execute it in the [`rayon::ThreadPool`].
#[allow(clippy::too_many_arguments)] // clippy: 😝
pub fn index<'pl, 'indexer, 'index, DC>(
    wtxn: &mut RwTxn,
    index: &'index Index,
    pool: &ThreadPoolNoAbort,
    grenad_parameters: GrenadParameters,
    db_fields_ids_map: &'indexer FieldsIdsMap,
    new_fields_ids_map: FieldsIdsMap,
    new_primary_key: Option<PrimaryKey<'pl>>,
    document_changes: &DC,
    must_stop_processing: &'indexer MustStopProcessing,
    progress: &'indexer Progress,
) -> Result<ChannelCongestion>
where
    DC: DocumentChanges<'pl>,
{
    let mut bbbuffers = Vec::new();
    let finished_extraction = AtomicBool::new(false);

    let (grenad_parameters, total_bbbuffer_capacity) =
        indexer_memory_settings(pool.current_num_threads(), grenad_parameters);

    let (extractor_sender, writer_receiver) = pool
        .install(|| extractor_writer_bbqueue(&mut bbbuffers, total_bbbuffer_capacity, 1000))
        .unwrap();

    let metadata_builder = MetadataBuilder::from_index(index, wtxn)?;
    let new_fields_ids_map = FieldIdMapWithMetadata::new(new_fields_ids_map, metadata_builder);
    let new_fields_ids_map = RwLock::new(new_fields_ids_map);
    let fields_ids_map_store = ThreadLocal::with_capacity(rayon::current_num_threads());
    let mut extractor_allocs = ThreadLocal::with_capacity(rayon::current_num_threads());
    let doc_allocs = ThreadLocal::with_capacity(rayon::current_num_threads());

    let indexing_context = IndexingContext {
        index,
        db_fields_ids_map,
        new_fields_ids_map: &new_fields_ids_map,
        doc_allocs: &doc_allocs,
        fields_ids_map_store: &fields_ids_map_store,
        must_stop_processing,
        progress,
        grenad_parameters: &grenad_parameters,
    };

    let mut document_ids = index.documents_ids(wtxn)?;

    let congestion = thread::scope(|s| -> Result<ChannelCongestion> {
        let indexer_span = tracing::Span::current();
        let finished_extraction = &finished_extraction;
        let document_ids = &mut document_ids;
        let extractor_handle =
            Builder::new().name(S("indexer-extractors")).spawn_scoped(s, move || {
                pool.install(move || {
                    extract::extract_all(
                        document_changes,
                        indexing_context,
                        indexer_span,
                        extractor_sender,
                        &mut extractor_allocs,
                        finished_extraction,
                        document_ids,
                    )
                })
                .unwrap()
            })?;

        let global_fields_ids_map = GlobalFieldsIdsMap::new(&new_fields_ids_map);

        let congestion = write_to_db(writer_receiver, finished_extraction, index, wtxn)?;

        indexing_context.progress.update_progress(IndexingStep::WaitingForExtractors);

        let (facet_field_ids_delta, word_delta) = extractor_handle.join().unwrap()?;

        pool.install(|| {
            post_processing::post_process(
                indexing_context,
                wtxn,
                global_fields_ids_map,
                &word_delta,
                facet_field_ids_delta,
            )
        })
        .unwrap()?;

        indexing_context.progress.update_progress(IndexingStep::Finalizing);

        Ok(congestion) as Result<_>
    })?;

    // required to into_inner the new_fields_ids_map
    drop(fields_ids_map_store);

    let new_fields_ids_map = new_fields_ids_map.into_inner().unwrap();
    update_index(index, wtxn, new_fields_ids_map, new_primary_key, document_ids)?;

    Ok(congestion)
}

fn indexer_memory_settings(
    current_num_threads: usize,
    grenad_parameters: GrenadParameters,
) -> (GrenadParameters, usize) {
    let grenad_parameters = GrenadParameters {
        max_memory: grenad_parameters.max_memory.map(|memory| memory * 5 / 100),
        ..grenad_parameters
    };

    let minimum_total_bbbuffer_capacity = 50 * 1024 * 1024 * current_num_threads;
    let minimum_total_extractors_capacity = minimum_total_bbbuffer_capacity * 2;
    let (grenad_parameters, total_bbbuffer_capacity) = grenad_parameters.max_memory.map_or(
        (
            GrenadParameters {
                max_memory: Some(minimum_total_extractors_capacity),
                ..grenad_parameters
            },
            minimum_total_bbbuffer_capacity,
        ),
        |max_memory| {
            let total_bbbuffer_capacity = max_memory.max(minimum_total_bbbuffer_capacity);
            let grenad_parameters = GrenadParameters {
                max_memory: Some(max_memory.max(minimum_total_extractors_capacity)),
                ..grenad_parameters
            };
            (grenad_parameters, total_bbbuffer_capacity)
        },
    );

    LOG_MEMORY_METRICS_ONCE.call_once(|| {
        tracing::debug!(
            "Indexation allocated memory metrics - Total BBQueue size: \
             {total_bbbuffer_capacity}, Total extractor memory: {:?}",
            grenad_parameters.max_memory,
        );
    });

    (grenad_parameters, total_bbbuffer_capacity)
}
