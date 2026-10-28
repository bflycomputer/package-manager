use std::cell::{Cell, RefCell};
use std::sync::RwLock;

use bumpalo::Bump;
use bumparaw_collections::RawMap;
use rustc_hash::FxBuildHasher;
use serde_json::value::RawValue;

use super::{KvReaderFieldId, KvWriterFieldId};
use crate::update::new::thread_local::{FullySend, MostlySend, ThreadLocal};
use crate::{FieldIdMapWithMetadata, GlobalFieldsIdsMap, Result, UserError};

/// A read-only view of the top-level fields in an inserted package document.
pub trait Document<'doc> {
    fn iter_top_level_fields(&self) -> impl Iterator<Item = Result<(&'doc str, &'doc RawValue)>>;
}

impl<'doc, D: ?Sized> Document<'doc> for &D
where
    D: Document<'doc>,
{
    fn iter_top_level_fields(&self) -> impl Iterator<Item = Result<(&'doc str, &'doc RawValue)>> {
        D::iter_top_level_fields(self)
    }
}

/// Serialize a package document into the field-id-keyed representation stored in LMDB.
pub fn write_to_obkv_without_vectors<'s, 'a, 'map, 'buffer>(
    document: &'s impl Document<'s>,
    fields_ids_map: &'a mut GlobalFieldsIdsMap<'map>,
    document_buffer: &'a mut bumpalo::collections::Vec<'buffer, u8>,
) -> Result<&'a KvReaderFieldId>
where
    's: 'a,
{
    document_buffer.clear();
    let mut unordered_field_buffer = Vec::new();
    let mut writer = KvWriterFieldId::new(&mut *document_buffer);

    for entry in document.iter_top_level_fields() {
        let (field_name, value) = entry?;
        let field_id =
            fields_ids_map.id_or_insert(field_name).ok_or(UserError::AttributeLimitReached)?;
        unordered_field_buffer.push((field_id, value));
    }

    unordered_field_buffer.sort_by_key(|(fid, _)| *fid);
    for (fid, value) in unordered_field_buffer {
        writer.insert(fid, value.get().as_bytes()).unwrap();
    }

    writer.finish().unwrap();
    Ok(KvReaderFieldId::from_slice(document_buffer))
}

pub struct DocumentContext<'doc, 'extractor: 'doc, 'fid: 'doc, T: MostlySend> {
    pub new_fields_ids_map: &'doc RefCell<GlobalFieldsIdsMap<'fid>>,
    pub doc_alloc: Bump,
    pub extractor_alloc: &'extractor Bump,
    pub doc_allocs: &'doc ThreadLocal<FullySend<Cell<Bump>>>,
    pub data: &'doc T,
}

impl<'doc, 'data: 'doc, 'extractor: 'doc, 'fid: 'doc, T: MostlySend>
    DocumentContext<'doc, 'extractor, 'fid, T>
{
    pub fn new<F>(
        new_fields_ids_map: &'fid RwLock<FieldIdMapWithMetadata>,
        extractor_allocs: &'extractor ThreadLocal<FullySend<Bump>>,
        doc_allocs: &'doc ThreadLocal<FullySend<Cell<Bump>>>,
        datastore: &'data ThreadLocal<T>,
        fields_ids_map_store: &'doc ThreadLocal<FullySend<RefCell<GlobalFieldsIdsMap<'fid>>>>,
        init_data: F,
    ) -> Result<Self>
    where
        F: FnOnce(&'extractor Bump) -> Result<T>,
    {
        let doc_alloc =
            doc_allocs.get_or(|| FullySend(Cell::new(Bump::with_capacity(1024 * 1024))));
        let doc_alloc = doc_alloc.0.take();
        let fields_ids_map = fields_ids_map_store
            .get_or(|| RefCell::new(GlobalFieldsIdsMap::new(new_fields_ids_map)).into());
        let extractor_alloc = extractor_allocs.get_or_default();
        let data = datastore.get_or_try(move || init_data(&extractor_alloc.0))?;

        Ok(Self {
            new_fields_ids_map: &fields_ids_map.0,
            doc_alloc,
            extractor_alloc: &extractor_alloc.0,
            doc_allocs,
            data,
        })
    }
}

impl<'doc> Document<'doc> for RawMap<'doc, FxBuildHasher> {
    fn iter_top_level_fields(&self) -> impl Iterator<Item = Result<(&'doc str, &'doc RawValue)>> {
        self.iter().map(|(name, value)| Ok((name, value)))
    }
}
