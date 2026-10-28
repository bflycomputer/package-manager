use crate::make_enum_progress;

make_enum_progress! {
    pub enum IndexingStep {
        PreparingPayloads,
        AssigningDocumentsIds,
        ReorderingPayloadOffsets,
        ExtractingDocuments,
        ExtractingFacets,
        ExtractingWords,
        ExtractingWordProximity,
        MergingFacetCaches,
        MergingWordCaches,
        MergingWordProximity,
        WaitingForDatabaseWrites,
        WaitingForExtractors,
        PostProcessingFacets,
        PostProcessingWords,
        Finalizing,
    }
}

make_enum_progress! {
    pub enum PostProcessingFacets {
        StringsBulk,
        NumbersBulk,
    }
}

make_enum_progress! {
    pub enum PostProcessingWords {
        WordFst,
        ComputePrefixes,
        WordPrefixDocids,
        WordPrefixFieldIdDocids,
        WordPrefixPositionDocids,
    }
}
