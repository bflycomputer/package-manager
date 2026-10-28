use grenad::CompressionType;

use super::GrenadParameters;
use crate::thread_pool_no_abort::ThreadPoolNoAbort;
use crate::ThreadPoolNoAbortBuilder;

#[derive(Debug)]
pub struct IndexerConfig {
    pub max_memory: Option<usize>,
    pub thread_pool: ThreadPoolNoAbort,
}

impl IndexerConfig {
    pub fn grenad_parameters(&self) -> GrenadParameters {
        GrenadParameters {
            chunk_compression_type: CompressionType::None,
            chunk_compression_level: None,
            max_memory: self.max_memory,
            max_nb_chunks: None,
        }
    }
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            max_memory: None,
            thread_pool: ThreadPoolNoAbortBuilder::new_for_indexing()
                .build()
                .expect("failed to build indexing thread pool"),
        }
    }
}
