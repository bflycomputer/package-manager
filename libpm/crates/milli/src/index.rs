use std::borrow::Cow;
use std::path::Path;

use heed::types::{SerdeJson, *};
use heed::{Database, RoTxn, RwTxn, Unspecified, WithoutTls};
use roaring::RoaringBitmap;

use crate::error::UserError;
use crate::fields_ids_map::FieldsIdsMap;
use crate::heed_codec::facet::{
    FacetGroupKeyCodec, FacetGroupValueCodec, FieldDocIdFacetF64Codec, FieldDocIdFacetStringCodec,
    OrderedF64Codec,
};
use crate::heed_codec::{StrBEU16Codec, StrRefCodec};
use crate::progress::Progress;
use crate::{
    CboRoaringBitmapCodec, DocumentId, FieldId, FieldIdWordCountCodec, ObkvCodec, Result,
    RoaringBitmapCodec, Search, U8StrStrCodec, BEU32,
};

pub mod main_key {
    pub const DOCUMENTS_IDS_KEY: &str = "documents-ids";
    pub const FIELDS_IDS_MAP_KEY: &str = "fields-ids-map";
    pub const PRIMARY_KEY_KEY: &str = "primary-key";
    pub const WORDS_FST_KEY: &str = "words-fst";
    pub const WORDS_PREFIXES_FST_KEY: &str = "words-prefixes-fst";
}

pub mod db_name {
    pub const MAIN: &str = "main";
    pub const WORD_DOCIDS: &str = "word-docids";
    pub const WORD_PREFIX_DOCIDS: &str = "word-prefix-docids";
    pub const EXTERNAL_DOCUMENTS_IDS: &str = "external-documents-ids";
    pub const DOCID_WORD_POSITIONS: &str = "docid-word-positions";
    pub const WORD_PAIR_PROXIMITY_DOCIDS: &str = "word-pair-proximity-docids";
    pub const WORD_POSITION_DOCIDS: &str = "word-position-docids";
    pub const WORD_FIELD_ID_DOCIDS: &str = "word-field-id-docids";
    pub const WORD_PREFIX_POSITION_DOCIDS: &str = "word-prefix-position-docids";
    pub const WORD_PREFIX_FIELD_ID_DOCIDS: &str = "word-prefix-field-id-docids";
    pub const FIELD_ID_WORD_COUNT_DOCIDS: &str = "field-id-word-count-docids";
    pub const FACET_ID_F64_DOCIDS: &str = "facet-id-f64-docids";
    pub const FACET_ID_STRING_DOCIDS: &str = "facet-id-string-docids";
    pub const FIELD_ID_DOCID_FACET_F64S: &str = "field-id-docid-facet-f64s";
    pub const FIELD_ID_DOCID_FACET_STRINGS: &str = "field-id-docid-facet-strings";
    pub const DOCUMENTS: &str = "documents";
}
const NUMBER_OF_DBS: u32 = 16;

#[derive(Clone)]
pub struct Index {
    /// The LMDB environment which this index is associated with.
    pub(crate) env: heed::Env<WithoutTls>,

    /// Contains many different types (e.g. the fields ids map).
    pub(crate) main: Database<Unspecified, Unspecified>,

    /// Maps the external documents ids with the internal document id.
    pub external_documents_ids: Database<Str, BEU32>,

    /// A word and all the documents ids containing the word.
    pub word_docids: Database<Str, CboRoaringBitmapCodec>,

    /// A prefix of word and all the documents ids containing this prefix.
    pub word_prefix_docids: Database<Str, CboRoaringBitmapCodec>,

    /// Maps the proximity between a pair of words with all the docids where this relation appears.
    pub word_pair_proximity_docids: Database<U8StrStrCodec, CboRoaringBitmapCodec>,

    /// Maps the word and the position with the docids that corresponds to it.
    pub word_position_docids: Database<StrBEU16Codec, CboRoaringBitmapCodec>,
    /// Maps the word and the field id with the docids that corresponds to it.
    pub word_fid_docids: Database<StrBEU16Codec, CboRoaringBitmapCodec>,

    /// Maps the field id and the word count with the docids that corresponds to it.
    pub field_id_word_count_docids: Database<FieldIdWordCountCodec, CboRoaringBitmapCodec>,
    /// Maps the word prefix and a position with all the docids where the prefix appears at the position.
    pub word_prefix_position_docids: Database<StrBEU16Codec, CboRoaringBitmapCodec>,
    /// Maps the word prefix and a field id with all the docids where the prefix appears inside the field
    pub word_prefix_fid_docids: Database<StrBEU16Codec, CboRoaringBitmapCodec>,

    /// Maps the facet field id and ranges of numbers with the docids that corresponds to them.
    pub facet_id_f64_docids: Database<FacetGroupKeyCodec<OrderedF64Codec>, FacetGroupValueCodec>,
    /// Maps the facet field id and ranges of strings with the docids that corresponds to them.
    pub facet_id_string_docids: Database<FacetGroupKeyCodec<StrRefCodec>, FacetGroupValueCodec>,
    /// Maps the document id, the facet field id and the numbers.
    pub field_id_docid_facet_f64s: Database<FieldDocIdFacetF64Codec, Unit>,
    /// Maps the document id, the facet field id and the strings.
    pub field_id_docid_facet_strings: Database<FieldDocIdFacetStringCodec, Str>,

    /// Maps the document id to the document as an obkv store.
    pub(crate) documents: Database<BEU32, ObkvCodec>,
}

impl Index {
    pub fn new<P: AsRef<Path>>(
        mut options: heed::EnvOpenOptions<WithoutTls>,
        path: P,
    ) -> Result<Index> {
        use db_name::*;

        options.max_dbs(NUMBER_OF_DBS);

        let env = unsafe { options.open(path) }?;
        let mut wtxn = env.write_txn()?;
        let main = env.database_options().name(MAIN).create(&mut wtxn)?;
        let word_docids = env.create_database(&mut wtxn, Some(WORD_DOCIDS))?;
        let external_documents_ids =
            env.create_database(&mut wtxn, Some(EXTERNAL_DOCUMENTS_IDS))?;
        let word_prefix_docids = env.create_database(&mut wtxn, Some(WORD_PREFIX_DOCIDS))?;
        let word_pair_proximity_docids =
            env.create_database(&mut wtxn, Some(WORD_PAIR_PROXIMITY_DOCIDS))?;
        let word_position_docids = env.create_database(&mut wtxn, Some(WORD_POSITION_DOCIDS))?;
        let word_fid_docids = env.create_database(&mut wtxn, Some(WORD_FIELD_ID_DOCIDS))?;
        let field_id_word_count_docids =
            env.create_database(&mut wtxn, Some(FIELD_ID_WORD_COUNT_DOCIDS))?;
        let word_prefix_position_docids =
            env.create_database(&mut wtxn, Some(WORD_PREFIX_POSITION_DOCIDS))?;
        let word_prefix_fid_docids =
            env.create_database(&mut wtxn, Some(WORD_PREFIX_FIELD_ID_DOCIDS))?;
        let facet_id_f64_docids = env.create_database(&mut wtxn, Some(FACET_ID_F64_DOCIDS))?;
        let facet_id_string_docids =
            env.create_database(&mut wtxn, Some(FACET_ID_STRING_DOCIDS))?;
        let field_id_docid_facet_f64s =
            env.create_database(&mut wtxn, Some(FIELD_ID_DOCID_FACET_F64S))?;
        let field_id_docid_facet_strings =
            env.create_database(&mut wtxn, Some(FIELD_ID_DOCID_FACET_STRINGS))?;
        let documents = env.create_database(&mut wtxn, Some(DOCUMENTS))?;

        let this = Index {
            env: env.clone(),
            main,
            external_documents_ids,
            word_docids,
            word_prefix_docids,
            word_pair_proximity_docids,
            word_position_docids,
            word_fid_docids,
            word_prefix_position_docids,
            word_prefix_fid_docids,
            field_id_word_count_docids,
            facet_id_f64_docids,
            facet_id_string_docids,
            field_id_docid_facet_f64s,
            field_id_docid_facet_strings,
            documents,
        };

        wtxn.commit()?;

        Ok(this)
    }

    /// Create a write transaction to be able to write into the index.
    pub fn write_txn(&self) -> heed::Result<RwTxn<'_>> {
        self.env.write_txn()
    }

    /// Create a read transaction to be able to read the index.
    pub fn read_txn(&self) -> heed::Result<RoTxn<'_, WithoutTls>> {
        self.env.read_txn()
    }

    /// Returns the real size used by the index.
    pub fn on_disk_size(&self) -> Result<u64> {
        Ok(self.env.real_disk_size()?)
    }

    /* documents ids */

    /// Writes the documents ids that corresponds to the user-ids-documents-ids FST.
    pub(crate) fn put_documents_ids(
        &self,
        wtxn: &mut RwTxn<'_>,
        docids: &RoaringBitmap,
    ) -> heed::Result<()> {
        self.main.remap_types::<Str, RoaringBitmapCodec>().put(
            wtxn,
            main_key::DOCUMENTS_IDS_KEY,
            docids,
        )
    }

    /// Returns the internal documents ids.
    pub fn documents_ids(&self, rtxn: &RoTxn<'_>) -> heed::Result<RoaringBitmap> {
        Ok(self
            .main
            .remap_types::<Str, RoaringBitmapCodec>()
            .get(rtxn, main_key::DOCUMENTS_IDS_KEY)?
            .unwrap_or_default())
    }

    /* primary key */

    /// Writes the documents primary key, this is the field name that is used to store the id.
    pub(crate) fn put_primary_key(
        &self,
        wtxn: &mut RwTxn<'_>,
        primary_key: &str,
    ) -> heed::Result<()> {
        self.main.remap_types::<Str, Str>().put(wtxn, main_key::PRIMARY_KEY_KEY, primary_key)
    }

    /// Deletes the primary key of the documents, this can be done to reset indexes settings.
    /// Returns the documents primary key, `None` if it hasn't been defined.
    pub fn primary_key<'t>(&self, rtxn: &'t RoTxn<'_>) -> heed::Result<Option<&'t str>> {
        self.main.remap_types::<Str, Str>().get(rtxn, main_key::PRIMARY_KEY_KEY)
    }

    /* fields ids map */

    /// Writes the fields ids map which associate the documents keys with an internal field id
    /// (i.e. `u8`), this field id is used to identify fields in the obkv documents.
    pub(crate) fn put_fields_ids_map(
        &self,
        wtxn: &mut RwTxn<'_>,
        map: &FieldsIdsMap,
    ) -> heed::Result<()> {
        self.main.remap_types::<Str, SerdeJson<FieldsIdsMap>>().put(
            wtxn,
            main_key::FIELDS_IDS_MAP_KEY,
            map,
        )
    }

    /// Returns the fields ids map which associate the documents keys with an internal field id
    /// (i.e. `u8`), this field id is used to identify fields in the obkv documents.
    pub fn fields_ids_map(&self, rtxn: &RoTxn<'_>) -> heed::Result<FieldsIdsMap> {
        Ok(self
            .main
            .remap_types::<Str, SerdeJson<FieldsIdsMap>>()
            .get(rtxn, main_key::FIELDS_IDS_MAP_KEY)?
            .unwrap_or_default())
    }

    /// Identical to `searchable_fields`, but returns the ids instead.
    pub fn searchable_fields_ids(
        &self,
        _rtxn: &RoTxn<'_>,
        fields_ids_map: &FieldsIdsMap,
    ) -> Result<Vec<FieldId>> {
        Ok(["name", "description"]
            .into_iter()
            .filter_map(|field| fields_ids_map.id(field))
            .collect())
    }

    /* words fst */

    /// Writes the FST which is the words dictionary of the engine.
    /// Returns the FST which is the words dictionary of the engine.
    pub fn words_fst<'t>(&self, rtxn: &'t RoTxn<'_>) -> Result<fst::Set<Cow<'t, [u8]>>> {
        match self.main.remap_types::<Str, Bytes>().get(rtxn, main_key::WORDS_FST_KEY)? {
            Some(bytes) => Ok(fst::Set::new(bytes)?.map_data(Cow::Borrowed)?),
            None => Ok(fst::Set::default().map_data(Cow::Owned)?),
        }
    }

    /* words prefixes fst */

    /// Writes the FST which is the words prefixes dictionary of the engine.
    /// Returns the FST which is the words prefixes dictionary of the engine.
    pub fn words_prefixes_fst<'t>(&self, rtxn: &'t RoTxn<'t>) -> Result<fst::Set<Cow<'t, [u8]>>> {
        match self.main.remap_types::<Str, Bytes>().get(rtxn, main_key::WORDS_PREFIXES_FST_KEY)? {
            Some(bytes) => Ok(fst::Set::new(bytes)?.map_data(Cow::Borrowed)?),
            None => Ok(fst::Set::default().map_data(Cow::Owned)?),
        }
    }

    /* documents */

    /// Returns a document by using the document id.
    pub fn document<'t>(&self, rtxn: &'t RoTxn, id: DocumentId) -> Result<&'t obkv::KvReaderU16> {
        self.documents
            .get(rtxn, &id)?
            .ok_or(UserError::UnknownInternalDocumentId { document_id: id })
            .map_err(Into::into)
    }

    /// Returns an iterator over the requested documents. The next item will be an error if a document is missing.
    pub fn iter_documents<'a, 't: 'a>(
        &'a self,
        rtxn: &'t RoTxn<'t>,
        ids: impl IntoIterator<Item = DocumentId> + 'a,
    ) -> Result<impl Iterator<Item = Result<(DocumentId, &'t obkv::KvReaderU16)>> + 'a> {
        Ok(ids.into_iter().map(move |id| {
            let kv = self
                .documents
                .get(rtxn, &id)?
                .ok_or(UserError::UnknownInternalDocumentId { document_id: id })?;
            Ok((id, kv))
        }))
    }

    /// Returns a [`Vec`] of the requested documents. Returns an error if a document is missing.
    pub fn documents<'t>(
        &self,
        rtxn: &'t RoTxn<'t>,
        ids: impl IntoIterator<Item = DocumentId>,
    ) -> Result<Vec<(DocumentId, &'t obkv::KvReaderU16)>> {
        self.iter_documents(rtxn, ids)?.collect()
    }

    pub fn search<'a>(
        &'a self,
        rtxn: &'a RoTxn<'a>,
        fields_ids_map: &'a FieldsIdsMap,
        progress: &'a Progress,
    ) -> Search<'a> {
        Search::new(rtxn, self, fields_ids_map, progress)
    }

    /// Check if the word is indexed in the index.
    ///
    /// This function checks if the word is indexed in the index by looking at the word docids.
    ///
    /// # Arguments
    ///
    /// * `rtxn`: The read transaction.
    /// * `word`: The word to check.
    pub fn contains_word(&self, rtxn: &RoTxn<'_>, word: &str) -> Result<bool> {
        Ok(self.word_docids.remap_data_type::<DecodeIgnore>().get(rtxn, word)?.is_some())
    }

}
