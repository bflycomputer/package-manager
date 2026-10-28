use roaring::RoaringBitmap;

use super::ranking_rules::{BoxRankingRule, RankingRuleQueryTrait};
use super::SearchContext;
use crate::Result;

pub struct BucketSortOutput {
    pub docids: Vec<u32>,
    pub all_candidates: RoaringBitmap,
}

#[tracing::instrument(level = "trace", skip_all, target = "search::bucket_sort")]
pub fn bucket_sort<'ctx, Q: RankingRuleQueryTrait>(
    ctx: &mut SearchContext<'ctx>,
    mut ranking_rules: Vec<BoxRankingRule<'ctx, Q>>,
    query: &Q,
    universe: &RoaringBitmap,
    length: usize,
) -> Result<BucketSortOutput> {
    if ranking_rules.is_empty() {
        return Ok(BucketSortOutput {
            docids: universe.iter().take(length).collect(),
            all_candidates: universe.clone(),
        });
    }

    let ranking_rules_len = ranking_rules.len();

    ranking_rules[0].start_iteration(ctx, universe, query)?;

    let mut ranking_rule_universes =
        vec![RoaringBitmap::default(); ranking_rules_len];
    ranking_rule_universes[0].clone_from(universe);
    let mut cur_ranking_rule_index = 0;

    macro_rules! back {
        () => {
            ranking_rule_universes[cur_ranking_rule_index].clear();
            ranking_rules[cur_ranking_rule_index].end_iteration(ctx);
            if cur_ranking_rule_index == 0 {
                break;
            }
            cur_ranking_rule_index -= 1;
        };
    }

    let mut all_candidates = universe.clone();
    let mut valid_docids = vec![];

    macro_rules! maybe_add_to_results {
        ($candidates:expr) => {
            add_to_results(
                length,
                &mut valid_docids,
                &mut all_candidates,
                $candidates,
            );
        };
    }

    while valid_docids.len() < length {
        if ranking_rule_universes[cur_ranking_rule_index].is_empty()
            || ranking_rule_universes[cur_ranking_rule_index].len() == 1
        {
            let bucket = std::mem::take(&mut ranking_rule_universes[cur_ranking_rule_index]);
            maybe_add_to_results!(bucket);
            back!();
            continue;
        }

        let Some(next_bucket) = ranking_rules[cur_ranking_rule_index].next_bucket(
            ctx,
            &ranking_rule_universes[cur_ranking_rule_index],
        )?
        else {
            back!();
            continue;
        };

        debug_assert!(
            ranking_rule_universes[cur_ranking_rule_index].is_superset(&next_bucket.candidates)
        );

        ranking_rule_universes[cur_ranking_rule_index] -= &next_bucket.candidates;

        if cur_ranking_rule_index == ranking_rules_len - 1
            || next_bucket.candidates.len() <= 1
        {
            maybe_add_to_results!(next_bucket.candidates);
            continue;
        }

        cur_ranking_rule_index += 1;
        ranking_rule_universes[cur_ranking_rule_index].clone_from(&next_bucket.candidates);
        ranking_rules[cur_ranking_rule_index].start_iteration(
            ctx,
            &next_bucket.candidates,
            &next_bucket.query,
        )?;
    }

    Ok(BucketSortOutput { docids: valid_docids, all_candidates })
}

fn add_to_results(
    length: usize,
    valid_docids: &mut Vec<u32>,
    all_candidates: &mut RoaringBitmap,
    candidates: RoaringBitmap,
) {
    *all_candidates |= &candidates;

    if candidates.is_empty() {
        return;
    }

    let candidates = candidates.iter().take(length - valid_docids.len()).collect::<Vec<u32>>();
    valid_docids.extend_from_slice(&candidates);
}
