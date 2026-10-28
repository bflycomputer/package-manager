use bumpalo::collections::vec::Vec as BumpVec;
use bumpalo::Bump;
use bumparaw_collections::RawMap;
use heed::RoTxn;
use memmap2::Mmap;
use rayon::iter::IndexedParallelIterator;
use rayon::slice::ParallelSlice;
use rustc_hash::FxBuildHasher;
use serde_json::value::RawValue;
use serde_json::Deserializer;

use super::document_changes::DocumentChanges;
use crate::documents::PrimaryKey;
use crate::progress::Progress;
use crate::update::new::document::DocumentContext;
use crate::update::new::steps::IndexingStep;
use crate::update::new::thread_local::MostlySend;
use crate::update::new::DocumentChange;
use crate::{
    DocumentId, Error, FieldsIdsMap, Index, InternalError, MustStopProcessing, Result, UserError,
};

/// The package index's one-shot NDJSON insertion payload.
#[derive(Default)]
pub struct IndexOperations<'pl> {
    payload: Option<&'pl [u8]>,
}

impl<'pl> IndexOperations<'pl> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_documents(&mut self, payload: &'pl Mmap) -> Result<()> {
        #[cfg(unix)]
        payload.advise(memmap2::Advice::Sequential)?;
        self.payload = Some(&payload[..]);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(level = "trace", skip_all, target = "indexing::document_operation")]
    pub fn into_changes(
        self,
        indexer: &'pl Bump,
        index: &Index,
        rtxn: &'pl RoTxn<'pl>,
        _primary_key_from_op: Option<&'pl str>,
        new_fields_ids_map: &mut FieldsIdsMap,
        must_stop_processing: &MustStopProcessing,
        progress: Progress,
    ) -> Result<(DocumentOperationChanges<'pl>, Vec<PayloadStats>, Option<PrimaryKey<'pl>>)> {
        progress.update_progress(IndexingStep::PreparingPayloads);

        let primary_key_name =
            index.primary_key(rtxn)?.ok_or(UserError::NoPrimaryKeyCandidateFound)?;
        let primary_key = PrimaryKey::new_or_insert(primary_key_name, new_fields_ids_map)?;

        let Some(payload) = self.payload else {
            return Ok((
                DocumentOperationChanges { documents: &[] },
                Vec::new(),
                Some(primary_key),
            ));
        };

        progress.update_progress(IndexingStep::AssigningDocumentsIds);
        let mut documents = BumpVec::new_in(indexer);
        let mut iter = Deserializer::from_slice(payload).into_iter::<&RawValue>();
        while let Some(document) = iter.next().transpose().map_err(InternalError::SerdeJson)? {
            if must_stop_processing.get() {
                return Err(InternalError::AbortedIndexation.into());
            }

            let docid = DocumentId::try_from(documents.len())
                .map_err(|_| UserError::DocumentLimitReached)?;
            let external_document_id =
                match primary_key.extract_fields_and_docid(document, new_fields_ids_map, indexer) {
                    Ok(document_id) => document_id,
                    Err(Error::UserError(error)) => {
                        let stats = PayloadStats {
                            bytes: payload.len() as u64,
                            document_count: 0,
                            error: Some(error),
                        };
                        return Ok((
                            DocumentOperationChanges { documents: &[] },
                            vec![stats],
                            Some(primary_key),
                        ));
                    }
                    Err(error) => return Err(error),
                };

            documents.push((external_document_id, PayloadDocument { docid, document }));
        }

        progress.update_progress(IndexingStep::ReorderingPayloadOffsets);
        let document_count = documents.len() as u64;
        let documents = documents.into_bump_slice();
        let stats = PayloadStats { bytes: payload.len() as u64, document_count, error: None };

        Ok((DocumentOperationChanges { documents }, vec![stats], Some(primary_key)))
    }
}

#[derive(Clone, Copy)]
pub struct PayloadDocument<'pl> {
    docid: DocumentId,
    document: &'pl RawValue,
}

pub struct DocumentOperationChanges<'pl> {
    documents: &'pl [(&'pl str, PayloadDocument<'pl>)],
}

impl<'pl> DocumentChanges<'pl> for DocumentOperationChanges<'pl> {
    type Item = (&'pl str, PayloadDocument<'pl>);

    fn iter(
        &self,
        chunk_size: usize,
    ) -> impl IndexedParallelIterator<Item = impl AsRef<[Self::Item]>> {
        self.documents.par_chunks(chunk_size)
    }

    fn item_to_document_change<'doc, T: MostlySend + 'doc>(
        &'doc self,
        context: &'doc DocumentContext<T>,
        item: &'doc Self::Item,
    ) -> Result<Option<DocumentChange<'doc>>>
    where
        'pl: 'doc,
    {
        let (external_document_id, payload) = item;
        let document =
            RawMap::from_raw_value_and_hasher(payload.document, FxBuildHasher, &context.doc_alloc)
                .map_err(UserError::SerdeJson)?;

        Ok(Some(DocumentChange::create(payload.docid, external_document_id, document)))
    }

    fn len(&self) -> usize {
        self.documents.len()
    }
}

pub struct PayloadStats {
    pub bytes: u64,
    pub document_count: u64,
    pub error: Option<UserError>,
}
