use std::fs::File;
use std::io::{BufReader, BufWriter, Seek as _};

use grenad::CompressionType;

use crate::Result;

pub fn create_writer<R: std::io::Write>(
    compression_type: CompressionType,
    compression_level: Option<u32>,
    file: R,
) -> grenad::Writer<BufWriter<R>> {
    let mut builder = grenad::Writer::builder();
    builder.compression_type(compression_type);
    if let Some(level) = compression_level {
        builder.compression_level(level);
    }
    builder.build(BufWriter::new(file))
}

pub fn writer_into_reader(
    writer: grenad::Writer<BufWriter<File>>,
) -> Result<grenad::Reader<BufReader<File>>> {
    let mut file = writer.into_inner()?.into_inner().map_err(|error| error.into_error())?;
    file.rewind()?;
    grenad::Reader::new(BufReader::new(file)).map_err(Into::into)
}

#[derive(Debug, Clone, Copy)]
pub struct GrenadParameters {
    pub chunk_compression_type: CompressionType,
    pub chunk_compression_level: Option<u32>,
    pub max_memory: Option<usize>,
    pub max_nb_chunks: Option<usize>,
}

impl Default for GrenadParameters {
    fn default() -> Self {
        Self {
            chunk_compression_type: CompressionType::None,
            chunk_compression_level: None,
            max_memory: None,
            max_nb_chunks: None,
        }
    }
}

impl GrenadParameters {
    pub fn max_memory_by_thread(&self) -> Option<usize> {
        self.max_memory.map(|memory| memory / rayon::current_num_threads())
    }
}
