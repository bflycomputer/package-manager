use std::cell::RefCell;

use bumpalo::Bump;

use crate::update::new::channel::DocumentsSender;
use crate::update::new::document::{write_to_obkv_without_vectors, DocumentContext};
use crate::update::new::indexer::document_changes::Extractor;
use crate::update::new::ref_cell_ext::RefCellExt as _;
use crate::update::new::thread_local::FullySend;
use crate::update::new::DocumentChange;
use crate::Result;
use roaring::RoaringBitmap;

pub struct DocumentsExtractor<'a, 'b> {
    document_sender: DocumentsSender<'a, 'b>,
}

impl<'a, 'b> DocumentsExtractor<'a, 'b> {
    pub fn new(document_sender: DocumentsSender<'a, 'b>) -> Self {
        Self { document_sender }
    }
}

#[derive(Default)]
pub struct DocumentExtractorData {
    pub document_ids: RoaringBitmap,
}

impl<'extractor> Extractor<'extractor> for DocumentsExtractor<'_, '_> {
    type Data = FullySend<RefCell<DocumentExtractorData>>;

    fn init_data(&self, _extractor_alloc: &'extractor Bump) -> Result<Self::Data> {
        Ok(FullySend(Default::default()))
    }

    fn process<'doc>(
        &self,
        changes: impl Iterator<Item = Result<DocumentChange<'doc>>>,
        context: &DocumentContext<Self::Data>,
    ) -> Result<()> {
        let mut document_buffer = bumpalo::collections::Vec::new_in(&context.doc_alloc);
        let mut document_extractor_data = context.data.0.borrow_mut_or_yield();
        for change in changes {
            let change = change?;
            // **WARNING**: the exclusive borrow on `new_fields_ids_map` needs to be taken **inside** of the `for change in changes` loop
            // Otherwise, `BorrowMutError` will occur for document changes that also need the new_fields_ids_map (e.g.: UpdateByFunction)
            let mut new_fields_ids_map = context.new_fields_ids_map.borrow_mut_or_yield();

            let external_docid = change.external_docid().to_owned();
            let docid = change.docid();
            let content = write_to_obkv_without_vectors(
                change.inserted(),
                &mut new_fields_ids_map,
                &mut document_buffer,
            )?;
            document_extractor_data.document_ids.insert(docid);
            self.document_sender.uncompressed(docid, external_docid, content).unwrap();
        }

        Ok(())
    }
}
