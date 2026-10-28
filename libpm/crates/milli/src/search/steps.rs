use crate::make_enum_progress;

make_enum_progress! {
    pub enum SearchStep {
        TokenizeQuery,
        EvaluateQuery,
        KeywordRanking,
        PlaceholderRanking,
    }
}
