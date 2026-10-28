use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use bumpalo::Bump;

use super::tokenize_document::{tokenizer_builder, DocumentTokenizer};
use crate::fields_ids_map::metadata::Metadata;
use crate::proximity::{index_proximity, MAX_DISTANCE};
use crate::update::new::document::{Document, DocumentContext};
use crate::update::new::extract::cache::BalancedCaches;
use crate::update::new::indexer::document_changes::{
    extract, DocumentChanges, Extractor, IndexingContext,
};
use crate::update::new::ref_cell_ext::RefCellExt as _;
use crate::update::new::steps::IndexingStep;
use crate::update::new::thread_local::{FullySend, ThreadLocal};
use crate::update::new::DocumentChange;
use crate::{FieldId, PatternMatch, Result, UserError, MAX_POSITION_PER_ATTRIBUTE};

pub struct WordPairProximityDocidsExtractorData<'a> {
    tokenizer: DocumentTokenizer<'a>,
    max_memory_by_thread: Option<usize>,
    buckets: usize,
}

impl<'extractor> Extractor<'extractor> for WordPairProximityDocidsExtractorData<'_> {
    type Data = RefCell<BalancedCaches<'extractor>>;

    fn init_data(&self, extractor_alloc: &'extractor Bump) -> Result<Self::Data> {
        Ok(RefCell::new(BalancedCaches::new_in(
            self.buckets,
            self.max_memory_by_thread,
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
            WordPairProximityDocidsExtractor::extract_document_change(
                context,
                &self.tokenizer,
                change,
            )?;
        }
        Ok(())
    }
}

pub struct WordPairProximityDocidsExtractor;

impl WordPairProximityDocidsExtractor {
    pub fn run_extraction<'pl, 'fid, 'indexer, 'index, 'extractor, DC: DocumentChanges<'pl>>(
        document_changes: &DC,
        indexing_context: IndexingContext<'fid, 'indexer, 'index>,
        extractor_allocs: &'extractor mut ThreadLocal<FullySend<Bump>>,
        step: IndexingStep,
    ) -> Result<Vec<BalancedCaches<'extractor>>> {
        // Warning: this is duplicated code from extract_word_docids.rs
        let mut builder = tokenizer_builder();
        let tokenizer = builder.build();
        let document_tokenizer = DocumentTokenizer {
            tokenizer: &tokenizer,
            max_positions_per_attributes: MAX_POSITION_PER_ATTRIBUTE,
        };
        let extractor_data = WordPairProximityDocidsExtractorData {
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

        Ok(datastore.into_iter().map(RefCell::into_inner).collect())
    }

    // This method is reimplemented to count the number of words in the document in each field
    // and to store the docids of the documents that have a number of words in a given field
    // equal to or under than MAX_COUNTED_WORDS.
    fn extract_document_change(
        context: &DocumentContext<RefCell<BalancedCaches<'_>>>,
        document_tokenizer: &DocumentTokenizer,
        document_change: DocumentChange,
    ) -> Result<()> {
        let doc_alloc = &context.doc_alloc;

        let mut key_buffer = bumpalo::collections::Vec::new_in(doc_alloc);
        let mut add_word_pair_proximity = bumpalo::collections::Vec::new_in(doc_alloc);

        let mut new_fields_ids_map = context.new_fields_ids_map.borrow_mut_or_yield();
        let new_fields_ids_map = &mut *new_fields_ids_map;

        let mut cached_sorter = context.data.borrow_mut_or_yield();
        let cached_sorter = &mut *cached_sorter;

        // is a vecdequeue, and will be smol, so can stay on the heap for now
        let mut word_positions: VecDeque<(Rc<str>, u16)> =
            VecDeque::with_capacity(MAX_DISTANCE as usize);

        let docid = document_change.docid();
        process_document_tokens(
            document_change.inserted(),
            document_tokenizer,
            &mut word_positions,
            &mut |field_name| {
                new_fields_ids_map
                    .id_with_metadata_or_insert(field_name)
                    .ok_or(UserError::AttributeLimitReached.into())
            },
            &mut |(w1, w2), prox| {
                add_word_pair_proximity.push(((w1, w2), prox));
            },
        )?;

        add_word_pair_proximity.sort_unstable();
        add_word_pair_proximity.dedup_by(|(k1, _), (k2, _)| k1 == k2);
        for ((w1, w2), prox) in add_word_pair_proximity.iter() {
            let key = build_key(*prox, w1, w2, &mut key_buffer);
            cached_sorter.insert_add_u32(key, docid)?;
        }
        Ok(())
    }
}

fn build_key<'a>(
    prox: u8,
    w1: &str,
    w2: &str,
    key_buffer: &'a mut bumpalo::collections::Vec<u8>,
) -> &'a [u8] {
    key_buffer.clear();
    key_buffer.push(prox);
    key_buffer.extend_from_slice(w1.as_bytes());
    key_buffer.push(0);
    key_buffer.extend_from_slice(w2.as_bytes());
    key_buffer.as_slice()
}

fn word_positions_into_word_pair_proximity(
    word_positions: &mut VecDeque<(Rc<str>, u16)>,
    word_pair_proximity: &mut impl FnMut((Rc<str>, Rc<str>), u8),
) {
    let (head_word, head_position) = word_positions.pop_front().unwrap();
    for (word, position) in word_positions.iter() {
        let prox = index_proximity(head_position as u32, *position as u32) as u8;
        if prox > 0 && prox < MAX_DISTANCE as u8 {
            word_pair_proximity((head_word.clone(), word.clone()), prox);
        }
    }
}

fn drain_word_positions(
    word_positions: &mut VecDeque<(Rc<str>, u16)>,
    word_pair_proximity: &mut impl FnMut((Rc<str>, Rc<str>), u8),
) {
    while !word_positions.is_empty() {
        word_positions_into_word_pair_proximity(word_positions, word_pair_proximity);
    }
}

fn process_document_tokens<'doc>(
    document: impl Document<'doc>,
    document_tokenizer: &DocumentTokenizer,
    word_positions: &mut VecDeque<(Rc<str>, u16)>,
    field_id_and_metadata: &mut impl FnMut(&str) -> Result<(FieldId, Metadata)>,
    word_pair_proximity: &mut impl FnMut((Rc<str>, Rc<str>), u8),
) -> Result<()> {
    let mut field_id = None;
    let mut token_fn = |_fname: &str, fid: FieldId, pos: u16, word: &str| {
        if field_id != Some(fid) {
            field_id = Some(fid);
            drain_word_positions(word_positions, word_pair_proximity);
        }
        // drain the proximity window until the head word is considered close to the word we are inserting.
        while word_positions
            .front()
            .is_some_and(|(_w, p)| index_proximity(*p as u32, pos as u32) >= MAX_DISTANCE)
        {
            word_positions_into_word_pair_proximity(word_positions, word_pair_proximity);
        }

        // insert the new word.
        word_positions.push_back((Rc::from(word), pos));
        Ok(())
    };

    let mut should_tokenize = |field_name: &str| {
        let (field_id, meta) = field_id_and_metadata(field_name)?;

        let pattern_match = if meta.is_searchable() == PatternMatch::Match {
            PatternMatch::Match
        } else {
            // TODO: should be a match on the field_name using `match_field_legacy` function,
            //       but for legacy reasons we iterate over all the fields to fill the field_id_map.
            PatternMatch::Parent
        };

        Ok((field_id, pattern_match))
    };

    document_tokenizer.tokenize_document(document, &mut should_tokenize, &mut token_fn)?;

    drain_word_positions(word_positions, word_pair_proximity);
    Ok(())
}
