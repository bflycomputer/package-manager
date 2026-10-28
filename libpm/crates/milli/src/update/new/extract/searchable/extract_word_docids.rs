use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::ops::DerefMut as _;

use bumpalo::collections::vec::Vec as BumpVec;
use bumpalo::Bump;

use super::tokenize_document::{tokenizer_builder, DocumentTokenizer};
use crate::update::new::document::DocumentContext;
use crate::update::new::extract::cache::BalancedCaches;
use crate::update::new::indexer::document_changes::{
    extract, DocumentChanges, Extractor, IndexingContext,
};
use crate::update::new::ref_cell_ext::RefCellExt as _;
use crate::update::new::steps::IndexingStep;
use crate::update::new::thread_local::{FullySend, MostlySend, ThreadLocal};
use crate::update::new::DocumentChange;
use crate::{
    bucketed_position, DocumentId, FieldId, PatternMatch, Result, UserError, MAX_COUNTED_WORDS,
    MAX_POSITION_PER_ATTRIBUTE,
};

pub struct WordDocidsBalancedCaches<'extractor> {
    word_fid_docids: BalancedCaches<'extractor>,
    word_docids: BalancedCaches<'extractor>,
    word_position_docids: BalancedCaches<'extractor>,
    fid_word_count_docids: BalancedCaches<'extractor>,
    fid_word_count: HashMap<FieldId, usize>,
    current_docid: Option<DocumentId>,
}

unsafe impl MostlySend for WordDocidsBalancedCaches<'_> {}

impl<'extractor> WordDocidsBalancedCaches<'extractor> {
    pub fn new_in(buckets: usize, max_memory: Option<usize>, alloc: &'extractor Bump) -> Self {
        Self {
            word_fid_docids: BalancedCaches::new_in(buckets, max_memory, alloc),
            word_docids: BalancedCaches::new_in(buckets, max_memory, alloc),
            word_position_docids: BalancedCaches::new_in(buckets, max_memory, alloc),
            fid_word_count_docids: BalancedCaches::new_in(buckets, max_memory, alloc),
            fid_word_count: HashMap::new(),
            current_docid: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_add_u32(
        &mut self,
        field_id: FieldId,
        position: u16,
        word: &str,
        docid: u32,
        bump: &Bump,
    ) -> Result<()> {
        let word_bytes = word.as_bytes();
        self.word_docids.insert_add_u32(word_bytes, docid)?;

        let buffer_size = word_bytes.len() + 1 + size_of::<FieldId>();
        let mut buffer = BumpVec::with_capacity_in(buffer_size, bump);

        buffer.clear();
        buffer.extend_from_slice(word_bytes);
        buffer.push(0);
        buffer.extend_from_slice(&field_id.to_be_bytes());
        self.word_fid_docids.insert_add_u32(&buffer, docid)?;

        let position = bucketed_position(position);
        buffer.clear();
        buffer.extend_from_slice(word_bytes);
        buffer.push(0);
        buffer.extend_from_slice(&position.to_be_bytes());
        self.word_position_docids.insert_add_u32(&buffer, docid)?;

        if self.current_docid.is_some_and(|id| docid != id) {
            self.flush_fid_word_count(&mut buffer)?;
        }

        self.fid_word_count.entry(field_id).and_modify(|count| *count += 1).or_insert(1);

        self.current_docid = Some(docid);

        Ok(())
    }

    fn flush_fid_word_count(&mut self, buffer: &mut BumpVec<u8>) -> Result<()> {
        for (fid, count) in self.fid_word_count.drain() {
            if count <= MAX_COUNTED_WORDS {
                buffer.clear();
                buffer.extend_from_slice(&fid.to_be_bytes());
                buffer.push(count as u8);
                self.fid_word_count_docids.insert_add_u32(buffer, self.current_docid.unwrap())?;
            }
        }

        Ok(())
    }
}

pub struct WordDocidsCaches<'extractor> {
    pub word_docids: Vec<BalancedCaches<'extractor>>,
    pub word_fid_docids: Vec<BalancedCaches<'extractor>>,
    pub word_position_docids: Vec<BalancedCaches<'extractor>>,
    pub fid_word_count_docids: Vec<BalancedCaches<'extractor>>,
}

impl<'extractor> WordDocidsCaches<'extractor> {
    fn new() -> Self {
        Self {
            word_docids: Vec::new(),
            word_fid_docids: Vec::new(),
            word_position_docids: Vec::new(),
            fid_word_count_docids: Vec::new(),
        }
    }

    fn push(&mut self, other: WordDocidsBalancedCaches<'extractor>) -> Result<()> {
        let WordDocidsBalancedCaches {
            word_docids,
            word_fid_docids,
            word_position_docids,
            fid_word_count_docids,
            fid_word_count: _,
            current_docid: _,
        } = other;

        self.word_docids.push(word_docids);
        self.word_fid_docids.push(word_fid_docids);
        self.word_position_docids.push(word_position_docids);
        self.fid_word_count_docids.push(fid_word_count_docids);

        Ok(())
    }
}

pub struct WordDocidsExtractorData<'a> {
    tokenizer: DocumentTokenizer<'a>,
    max_memory_by_thread: Option<usize>,
    buckets: usize,
}

impl<'extractor> Extractor<'extractor> for WordDocidsExtractorData<'_> {
    type Data = RefCell<Option<WordDocidsBalancedCaches<'extractor>>>;

    fn init_data(&self, extractor_alloc: &'extractor Bump) -> Result<Self::Data> {
        Ok(RefCell::new(Some(WordDocidsBalancedCaches::new_in(
            self.buckets,
            self.max_memory_by_thread,
            extractor_alloc,
        ))))
    }

    fn process<'doc>(
        &self,
        changes: impl Iterator<Item = Result<DocumentChange<'doc>>>,
        context: &DocumentContext<Self::Data>,
    ) -> Result<()> {
        for change in changes {
            let change = change?;
            WordDocidsExtractors::extract_document_change(context, &self.tokenizer, change)?;
        }
        Ok(())
    }
}

pub struct WordDocidsExtractors;

impl WordDocidsExtractors {
    pub fn run_extraction<'pl, 'fid, 'indexer, 'index, 'extractor, DC: DocumentChanges<'pl>>(
        document_changes: &DC,
        indexing_context: IndexingContext<'fid, 'indexer, 'index>,
        extractor_allocs: &'extractor mut ThreadLocal<FullySend<Bump>>,
        step: IndexingStep,
    ) -> Result<WordDocidsCaches<'extractor>> {
        // Warning: this is duplicated code from extract_word_pair_proximity_docids.rs
        let mut builder = tokenizer_builder();
        let tokenizer = builder.build();
        let document_tokenizer = DocumentTokenizer {
            tokenizer: &tokenizer,
            max_positions_per_attributes: MAX_POSITION_PER_ATTRIBUTE,
        };
        let extractor_data = WordDocidsExtractorData {
            tokenizer: document_tokenizer,
            max_memory_by_thread: indexing_context.grenad_parameters.max_memory_by_thread(),
            buckets: rayon::current_num_threads(),
        };
        let datastore = ThreadLocal::new();
        {
            let span =
                tracing::trace_span!(target: "indexing::documents::extract", "docids_extraction");
            let _entered = span.enter();
            extract(
                document_changes,
                &extractor_data,
                indexing_context,
                extractor_allocs,
                &datastore,
                step,
            )?;
        }

        let mut merger = WordDocidsCaches::new();
        for cache in datastore.into_iter().flat_map(RefCell::into_inner) {
            merger.push(cache)?;
        }

        Ok(merger)
    }

    fn extract_document_change(
        context: &DocumentContext<RefCell<Option<WordDocidsBalancedCaches>>>,
        document_tokenizer: &DocumentTokenizer,
        document_change: DocumentChange,
    ) -> Result<()> {
        let mut cached_sorter_ref = context.data.borrow_mut_or_yield();
        let cached_sorter = cached_sorter_ref.as_mut().unwrap();
        let mut new_fields_ids_map = context.new_fields_ids_map.borrow_mut_or_yield();
        let new_fields_ids_map = new_fields_ids_map.deref_mut();
        let doc_alloc = &context.doc_alloc;

        let mut should_tokenize = |field_name: &str| {
            let Some((field_id, meta)) = new_fields_ids_map.id_with_metadata_or_insert(field_name)
            else {
                return Err(UserError::AttributeLimitReached.into());
            };

            let pattern_match = if meta.is_searchable() == PatternMatch::Match {
                PatternMatch::Match
            } else {
                // TODO: should be a match on the field_name using `match_field_legacy` function,
                //       but for legacy reasons we iterate over all the fields to fill the field_id_map.
                PatternMatch::Parent
            };

            Ok((field_id, pattern_match))
        };

        let mut token_fn = |_fname: &str, fid, pos, word: &str| {
            cached_sorter.insert_add_u32(fid, pos, word, document_change.docid(), doc_alloc)
        };
        document_tokenizer.tokenize_document(
            document_change.inserted(),
            &mut should_tokenize,
            &mut token_fn,
        )?;

        let buffer_size = size_of::<FieldId>();
        let mut buffer = BumpVec::with_capacity_in(buffer_size, &context.doc_alloc);
        cached_sorter.flush_fid_word_count(&mut buffer)
    }
}
