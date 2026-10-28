use std::fs::File;
use std::io::BufWriter;

use fst::SetBuilder;
use memmap2::Mmap;
use tempfile::tempfile;

use crate::{InternalError, Prefix, Result};

const PREFIX_COUNT_THRESHOLD: usize = 100;
const MAX_PREFIX_LENGTH: usize = 4;

pub struct WordFstBuilder {
    words: SetBuilder<BufWriter<File>>,
    prefixes: PrefixFstBuilder,
}

impl WordFstBuilder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            words: SetBuilder::new(BufWriter::new(tempfile()?))?,
            prefixes: PrefixFstBuilder::new(),
        })
    }

    pub fn register_word(&mut self, word: &[u8]) -> Result<()> {
        self.words.insert(word)?;
        self.prefixes.insert_word(word)
    }

    pub fn build(self) -> Result<(Mmap, PrefixData)> {
        let words_file =
            self.words.into_inner()?.into_inner().map_err(|_| {
                InternalError::IndexingMergingKeys { process: "building-words-fst" }
            })?;
        let words_fst_mmap = unsafe { Mmap::map(&words_file)? };
        let prefix_data = self.prefixes.build()?;
        Ok((words_fst_mmap, prefix_data))
    }
}

pub struct PrefixData {
    pub prefixes_fst_mmap: Mmap,
}

struct PrefixFstBuilder {
    prefix_fst_builders: Vec<SetBuilder<Vec<u8>>>,
    current_prefix: Vec<Prefix>,
    current_prefix_count: Vec<usize>,
}

impl PrefixFstBuilder {
    fn new() -> Self {
        Self {
            prefix_fst_builders: (0..MAX_PREFIX_LENGTH).map(|_| SetBuilder::memory()).collect(),
            current_prefix: vec![Prefix::new(); MAX_PREFIX_LENGTH],
            current_prefix_count: vec![0; MAX_PREFIX_LENGTH],
        }
    }

    fn insert_word(&mut self, bytes: &[u8]) -> Result<()> {
        for n in 0..MAX_PREFIX_LENGTH {
            let current_prefix = &mut self.current_prefix[n];
            let current_prefix_count = &mut self.current_prefix_count[n];
            let builder = &mut self.prefix_fst_builders[n];

            let word = std::str::from_utf8(bytes)?;
            let Some(prefix) = word.get(..=n) else {
                continue;
            };

            if *current_prefix_count == 0 || prefix != current_prefix.as_str() {
                *current_prefix = Prefix::from(prefix);
                *current_prefix_count = 0;
            }

            *current_prefix_count += 1;
            if *current_prefix_count == PREFIX_COUNT_THRESHOLD {
                builder.insert(prefix)?;
            }
        }
        Ok(())
    }

    fn build(self) -> Result<PrefixData> {
        let prefix_fsts: Vec<_> =
            self.prefix_fst_builders.into_iter().map(SetBuilder::into_set).collect();
        let op = fst::set::OpBuilder::from_iter(prefix_fsts.iter());
        let mut builder = SetBuilder::new(BufWriter::new(tempfile()?))?;
        builder.extend_stream(op.r#union())?;
        let prefix_fst_file = builder.into_inner()?.into_inner().map_err(|_| {
            InternalError::IndexingMergingKeys { process: "building-words-prefixes-fst" }
        })?;
        let prefix_fst_mmap = unsafe { Mmap::map(&prefix_fst_file)? };
        Ok(PrefixData { prefixes_fst_mmap: prefix_fst_mmap })
    }
}
