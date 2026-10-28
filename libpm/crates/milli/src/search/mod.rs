use levenshtein_automata::{LevenshteinAutomatonBuilder as LevBuilder, DFA};
use once_cell::sync::Lazy;
use roaring::bitmap::RoaringBitmap;

use self::new::PartialSearchResult;
use crate::progress::Progress;
use crate::search::new::{
    extract_tokens, resolve_negative_phrases, resolve_negative_words, ExtractedTokens, QueryGraph,
};
use crate::{
    execute_search, DocumentId, Error, FieldsIdsMap, Index, Result,
    SearchContext, SearchStep,
};

// Building these factories is not free.
static LEVDIST0: Lazy<LevBuilder> = Lazy::new(|| LevBuilder::new(0, true));
static LEVDIST1: Lazy<LevBuilder> = Lazy::new(|| LevBuilder::new(1, true));
static LEVDIST2: Lazy<LevBuilder> = Lazy::new(|| LevBuilder::new(2, true));

pub mod facet;
mod fst_utils;
pub mod new;
pub mod steps;

pub struct Search<'a> {
    query: Option<String>,
    limit: usize,
    rtxn: &'a heed::RoTxn<'a>,
    index: &'a Index,
    fields_ids_map: &'a FieldsIdsMap,
    progress: &'a Progress,
}

impl<'a> Search<'a> {
    pub fn new(
        rtxn: &'a heed::RoTxn<'a>,
        index: &'a Index,
        fields_ids_map: &'a FieldsIdsMap,
        progress: &'a Progress,
    ) -> Search<'a> {
        Search {
            query: None,
            limit: 20,
            rtxn,
            index,
            fields_ids_map,
            progress,
        }
    }

    pub fn query(&mut self, query: impl Into<String>) -> &mut Search<'a> {
        self.query = Some(query.into());
        self
    }

    pub fn limit(&mut self, limit: usize) -> &mut Search<'a> {
        self.limit = limit;
        self
    }

    pub fn execute(&self) -> Result<SearchResult> {
        let mut ctx = SearchContext::new(
            self.index,
            self.rtxn,
            self.fields_ids_map,
        )?;

        let mut universe = ctx.index.documents_ids(ctx.txn)?;

        let query_terms = self.build_located_query_terms(&mut ctx, &mut universe)?;

        let PartialSearchResult {
            candidates,
            documents_ids,
            ..
        } = execute_search(
            &mut ctx,
            query_terms,
            universe,
            self.limit,
            self.progress,
        )?;

        Ok(SearchResult {
            candidates,
            documents_ids,
        })
    }

    pub fn build_located_query_terms(
        &self,
        ctx: &mut SearchContext<'_>,
        universe: &mut RoaringBitmap,
    ) -> Result<Option<(QueryGraph, Vec<new::LocatedQueryTerm>)>, Error> {
        let mut ignored = RoaringBitmap::new();

        let query_graph_terms =
            if let Some(query) = self.query.as_deref().filter(|q| !q.trim().is_empty()) {
                let _step = self.progress.update_progress_scoped(SearchStep::TokenizeQuery);

                let ExtractedTokens { query_terms, graph, negative_words, negative_phrases } =
                    extract_tokens(ctx, query, Some(10))?;

                ignored |= resolve_negative_words(ctx, Some(&*universe), &negative_words)?;
                ignored |= resolve_negative_phrases(ctx, &negative_phrases)?;

                if query_terms.is_empty() {
                    // Do a placeholder search instead
                    None
                } else {
                    Some((graph, query_terms))
                }
            } else {
                None
            };

        *universe -= ignored;

        Ok(query_graph_terms)
    }
}

#[derive(Default, Debug)]
pub struct SearchResult {
    pub candidates: RoaringBitmap,
    pub documents_ids: Vec<DocumentId>,
}

fn get_first(s: &str) -> &str {
    match s.chars().next() {
        Some(c) => &s[..c.len_utf8()],
        None => panic!("unexpected empty query"),
    }
}

pub fn build_dfa(word: &str, typos: u8, is_prefix: bool) -> DFA {
    let lev = match typos {
        0 => &LEVDIST0,
        1 => &LEVDIST1,
        _ => &LEVDIST2,
    };

    if is_prefix {
        lev.build_prefix_dfa(word)
    } else {
        lev.build_dfa(word)
    }
}
