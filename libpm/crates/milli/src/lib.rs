#![allow(clippy::type_complexity)]
#![allow(clippy::result_large_err)]

pub mod documents;

mod attribute_patterns;
mod error;
pub mod facet;
mod fields_ids_map;
pub mod heed_codec;
pub mod index;
pub mod must_stop_processing;
pub mod fixed_config;
pub mod proximity;
mod search;
mod thread_pool_no_abort;
pub mod update;

pub mod progress;

use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::hash::BuildHasherDefault;

use charabia::normalizer::{CharNormalizer, CompatibilityDecompositionNormalizer};
use fxhash::{FxHasher32, FxHasher64};
pub use grenad::CompressionType;
pub use must_stop_processing::MustStopProcessing;
pub use search::new::{execute_search, SearchContext};
pub use thread_pool_no_abort::{CaughtPanic, ThreadPoolNoAbort, ThreadPoolNoAbortBuilder};
pub use {charabia as tokenizer, heed};

pub use self::attribute_patterns::PatternMatch;
pub use self::error::{Error, InternalError, UserError};
pub use self::fields_ids_map::metadata::Metadata;
pub use self::fields_ids_map::{
    FieldIdMapWithMetadata, FieldsIdsMap, GlobalFieldsIdsMap, MetadataBuilder,
};
pub use self::heed_codec::{
    CboRoaringBitmapCodec, CboRoaringBitmapLenCodec, FieldIdWordCountCodec, ObkvCodec,
    RoaringBitmapCodec, RoaringBitmapLenCodec, U8StrStrCodec,
};
pub use self::index::Index;
pub use self::search::steps::SearchStep;
pub use self::search::{Search, SearchResult};
pub use self::update::ChannelCongestion;

pub type Result<T, E = error::Error> = std::result::Result<T, E>;

pub type Attribute = u32;
pub type BEU16 = heed::types::U16<heed::byteorder::BE>;
pub type BEU32 = heed::types::U32<heed::byteorder::BE>;
pub type BEU64 = heed::types::U64<heed::byteorder::BE>;
pub type DocumentId = u32;
pub type FastMap4<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher32>>;
pub type FastMap8<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher64>>;
pub type FieldId = u16;
pub type Weight = u16;
pub type Object = serde_json::Map<String, serde_json::Value>;
pub type Position = u32;
pub type RelativePosition = u16;
pub type SmallString32 = smallstr::SmallString<[u8; 32]>;
pub type Prefix = smallstr::SmallString<[u8; 16]>;
pub type SmallVec16<T> = smallvec::SmallVec<[T; 16]>;
pub type SmallVec32<T> = smallvec::SmallVec<[T; 32]>;
pub type SmallVec8<T> = smallvec::SmallVec<[T; 8]>;


/// The maximum length a LMDB key can be.
///
/// Note that the actual allowed length is a little bit higher, but
/// we keep a margin of safety.
const MAX_LMDB_KEY_LENGTH: usize = 500;

/// The maximum length a field value can be when inserted in an LMDB key.
///
/// This number is determined by the keys of the different facet databases
/// and adding a margin of safety.
pub const MAX_FACET_VALUE_LENGTH: usize = MAX_LMDB_KEY_LENGTH - 32;

/// The maximum length a word can be
pub const MAX_WORD_LENGTH: usize = MAX_LMDB_KEY_LENGTH / 2;

pub const MAX_POSITION_PER_ATTRIBUTE: u32 = u16::MAX as u32 + 1;

/// The maximum amount of words counted inside of a field.
pub const MAX_COUNTED_WORDS: usize = 30;

// Convert an absolute word position into a relative position.
// Return the field id of the attribute related to the absolute position
// and the relative position in the attribute.
pub fn relative_from_absolute_position(absolute: Position) -> (FieldId, RelativePosition) {
    ((absolute >> 16) as u16, (absolute & 0xFFFF) as u16)
}

// Compute the absolute word position with the field id of the attribute and relative position in the attribute.
pub fn absolute_from_relative_position(field_id: FieldId, relative: RelativePosition) -> Position {
    ((field_id as u32) << 16) | (relative as u32)
}
// TODO: this is wrong, but will do for now
/// Compute the "bucketed" absolute position from the field id and relative position in the field.
///
/// In a bucketed position, the accuracy of the relative position is reduced exponentially as it gets larger.
pub fn bucketed_position(relative: u16) -> u16 {
    // The first few relative positions are kept intact.
    if relative < 16 {
        relative
    } else if relative < 24 {
        // Relative positions between 16 and 24 all become equal to 24
        24
    } else {
        // Then, groups of positions that have the same base-2 logarithm are reduced to
        // the same relative position: the smallest power of 2 that is greater than them
        (relative as f64).log2().ceil().exp2() as u16
    }
}

/// Divides one slice into two at an index, returns `None` if mid is out of bounds.
fn try_split_at<T>(slice: &[T], mid: usize) -> Option<(&[T], &[T])> {
    if mid <= slice.len() {
        Some(slice.split_at(mid))
    } else {
        None
    }
}

/// Divides one slice into an array and the tail at an index,
/// returns `None` if `N` is out of bounds.
fn try_split_array_at<T, const N: usize>(slice: &[T]) -> Option<([T; N], &[T])>
where
    [T; N]: for<'a> TryFrom<&'a [T]>,
{
    let (head, tail) = try_split_at(slice, N)?;
    let head = head.try_into().ok()?;
    Some((head, tail))
}

pub fn normalize_facet(original: &str) -> String {
    CompatibilityDecompositionNormalizer.normalize_str(original.trim()).to_lowercase()
}
