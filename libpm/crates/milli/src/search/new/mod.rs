mod bucket_sort;
mod db_cache;
mod graph_based_ranking_rule;
mod interner;
mod limits;
mod query_graph;
mod query_term;
mod ranking_rule_graph;
mod ranking_rules;
mod resolve_query_graph;
mod small_bitmap;

mod exact_attribute;
mod sort;

use bucket_sort::{bucket_sort, BucketSortOutput};
use charabia::TokenizerBuilder;
use db_cache::DatabaseCache;
use exact_attribute::ExactAttribute;
use graph_based_ranking_rule::{Exactness, Fid, Position, Proximity, Typo};
use heed::RoTxn;
use interner::{DedupInterner, Interner};
pub use query_graph::{QueryGraph, QueryNode};
use query_term::{located_query_terms_from_tokens, Phrase, QueryTerm};
pub use query_term::{ExtractedTokens, LocatedQueryTerm};
use ranking_rules::{
    BoxRankingRule, PlaceholderQuery, RankingRule, RankingRuleOutput, RankingRuleQueryTrait,
};
use resolve_query_graph::{compute_query_graph_docids, PhraseDocIdsCache};
use roaring::RoaringBitmap;
use sort::Sort;

use self::graph_based_ranking_rule::Words;
use self::interner::Interned;
use crate::progress::Progress;
use crate::search::steps::SearchStep;
use crate::{DocumentId, FieldsIdsMap, Index, Result};

/// A structure used throughout the execution of a search query.
pub struct SearchContext<'ctx> {
    pub index: &'ctx Index,
    pub txn: &'ctx RoTxn<'ctx>,
    pub fields_ids_map: &'ctx FieldsIdsMap,
    pub db_cache: DatabaseCache<'ctx>,
    pub word_interner: DedupInterner<String>,
    pub phrase_interner: DedupInterner<Phrase>,
    pub term_interner: Interner<QueryTerm>,
    pub phrase_docids: PhraseDocIdsCache,
}

impl<'ctx> SearchContext<'ctx> {
    pub fn new(
        index: &'ctx Index,
        txn: &'ctx RoTxn<'ctx>,
        fields_ids_map: &'ctx FieldsIdsMap,
    ) -> Result<Self> {
        Ok(Self {
            index,
            txn,
            fields_ids_map,
            db_cache: <_>::default(),
            word_interner: <_>::default(),
            phrase_interner: <_>::default(),
            term_interner: <_>::default(),
            phrase_docids: <_>::default(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq)]
pub enum Word {
    Original(Interned<String>),
    Derived(Interned<String>),
}

impl Word {
    pub fn interned(&self) -> Interned<String> {
        match self {
            Word::Original(word) => *word,
            Word::Derived(word) => *word,
        }
    }
}

fn resolve_maximally_reduced_query_graph(
    ctx: &mut SearchContext<'_>,
    universe: &RoaringBitmap,
    query_graph: &QueryGraph,
) -> Result<RoaringBitmap> {
    let mut graph = query_graph.clone();

    let nodes_to_remove: Vec<_> = query_graph
        .removal_order_for_terms_matching_strategy_last(ctx)
        .iter()
        .flat_map(|x| x.iter())
        .collect();
    graph.remove_nodes_keep_edges(&nodes_to_remove);

    let docids = compute_query_graph_docids(ctx, &graph, universe)?;

    Ok(docids)
}

#[tracing::instrument(level = "trace", skip_all, target = "search::universe")]
fn resolve_universe(
    ctx: &mut SearchContext<'_>,
    initial_universe: &RoaringBitmap,
    query_graph: &QueryGraph,
    progress: &Progress,
) -> Result<RoaringBitmap> {
    let _step = progress.update_progress_scoped(SearchStep::EvaluateQuery);
    resolve_maximally_reduced_query_graph(ctx, initial_universe, query_graph)
}

#[tracing::instrument(level = "trace", skip_all, target = "search::query")]
pub(in crate::search) fn resolve_negative_words(
    ctx: &mut SearchContext<'_>,
    universe: Option<&RoaringBitmap>,
    negative_words: &[Word],
) -> Result<RoaringBitmap> {
    let mut negative_bitmap = RoaringBitmap::new();
    for &word in negative_words {
        if let Some(bitmap) = ctx.word_docids(universe, word)? {
            negative_bitmap |= bitmap;
        }
    }
    Ok(negative_bitmap)
}

#[tracing::instrument(level = "trace", skip_all, target = "search::query")]
pub(in crate::search) fn resolve_negative_phrases(
    ctx: &mut SearchContext<'_>,
    negative_phrases: &[LocatedQueryTerm],
) -> Result<RoaringBitmap> {
    let mut negative_bitmap = RoaringBitmap::new();
    for term in negative_phrases {
        let query_term = ctx.term_interner.get(term.value);
        if let Some(phrase) = query_term.original_phrase() {
            negative_bitmap |= ctx.get_phrase_docids(phrase)?;
        }
    }
    Ok(negative_bitmap)
}

/// Return the list of initialised ranking rules to be used for a placeholder search.
fn get_ranking_rules_for_placeholder_search<'ctx>(
    ctx: &SearchContext<'ctx>,
) -> Result<Vec<BoxRankingRule<'ctx, PlaceholderQuery>>> {
    Ok(vec![
        Box::new(Sort::new(ctx.fields_ids_map, "traffic".to_owned(), false)?),
        Box::new(Sort::new(ctx.fields_ids_map, "id".to_owned(), true)?),
    ])
}

/// Return the list of initialised ranking rules to be used for a query graph search.
fn get_ranking_rules_for_query_graph_search<'ctx>(
    ctx: &SearchContext<'ctx>,
) -> Result<Vec<BoxRankingRule<'ctx, QueryGraph>>> {
    Ok(vec![
        Box::new(Words::new()),
        Box::new(Typo::new()),
        Box::new(Proximity::new()),
        Box::new(Fid::new()),
        Box::new(Sort::new(ctx.fields_ids_map, "traffic".to_owned(), false)?),
        Box::new(Position::new()),
        Box::new(ExactAttribute::new()),
        Box::new(Exactness::new()),
        Box::new(Sort::new(ctx.fields_ids_map, "id".to_owned(), true)?),
    ])
}

#[tracing::instrument(level = "trace", skip_all, target = "search::main")]
pub fn execute_search(
    ctx: &mut SearchContext<'_>,
    query_graph_terms: Option<(QueryGraph, Vec<LocatedQueryTerm>)>,
    mut universe: RoaringBitmap,
    length: usize,
    progress: &Progress,
) -> Result<PartialSearchResult> {
    let query_graph = query_graph_terms.map(|(query_graph, _)| query_graph);

    let bucket_sort_output = if let Some(query_graph) = query_graph {
        let ranking_rules = get_ranking_rules_for_query_graph_search(ctx)?;

        universe &= resolve_universe(
            ctx,
            &universe,
            &query_graph,
            progress,
        )?;

        let _step = progress.update_progress_scoped(SearchStep::KeywordRanking);
        bucket_sort(
            ctx,
            ranking_rules,
            &query_graph,
            &universe,
            length,
        )?
    } else {
        let ranking_rules = get_ranking_rules_for_placeholder_search(ctx)?;
        let _step = progress.update_progress_scoped(SearchStep::PlaceholderRanking);
        bucket_sort(
            ctx,
            ranking_rules,
            &PlaceholderQuery,
            &universe,
            length,
        )?
    };

    let BucketSortOutput { docids, all_candidates } = bucket_sort_output;

    Ok(PartialSearchResult {
        candidates: all_candidates,
        documents_ids: docids,
    })
}

pub fn extract_tokens(
    ctx: &mut SearchContext<'_>,
    query: &str,
    words_limit: Option<usize>,
) -> Result<ExtractedTokens> {
    let span = tracing::trace_span!(target: "search::tokens", "tokenizer_builder");
    let entered = span.enter();

    let mut tokbuilder: TokenizerBuilder<'_, &[u8]> = TokenizerBuilder::new();
    let tokenizer = tokbuilder.build();
    drop(entered);

    let span = tracing::trace_span!(target: "search::tokens", "tokenize");
    let entered = span.enter();
    let tokens = tokenizer.tokenize(query);
    drop(entered);

    located_query_terms_from_tokens(ctx, tokens, words_limit)
}

pub struct PartialSearchResult {
    pub candidates: RoaringBitmap,
    pub documents_ids: Vec<DocumentId>,
}
