use bumparaw_collections::RawMap;
use rustc_hash::FxBuildHasher;

use crate::DocumentId;

/// A document inserted during the one-shot package index build.
pub struct DocumentChange<'doc> {
    docid: DocumentId,
    external_document_id: &'doc str,
    document: RawMap<'doc, FxBuildHasher>,
}

impl<'doc> DocumentChange<'doc> {
    pub fn create(
        docid: DocumentId,
        external_document_id: &'doc str,
        document: RawMap<'doc, FxBuildHasher>,
    ) -> Self {
        Self { docid, external_document_id, document }
    }

    pub fn docid(&self) -> DocumentId {
        self.docid
    }

    pub fn external_docid(&self) -> &'doc str {
        self.external_document_id
    }

    pub fn inserted(&self) -> &RawMap<'doc, FxBuildHasher> {
        &self.document
    }
}
