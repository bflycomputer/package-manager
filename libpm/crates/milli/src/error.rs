use std::convert::Infallible;
use std::{io, str};

use heed::{Error as HeedError, MdbError};
use serde_json::Value;
use thiserror::Error;

use crate::thread_pool_no_abort::CaughtPanic;
use crate::{DocumentId, Object};

#[derive(Error, Debug)]
pub enum Error {
    #[error("internal: {0}.")]
    InternalError(#[from] InternalError),
    #[error(transparent)]
    IoError(#[from] io::Error),
    #[error(transparent)]
    UserError(#[from] UserError),
}

#[derive(Error, Debug)]
pub enum InternalError {
    #[error(transparent)]
    Fst(#[from] fst::Error),
    #[error("invalid compression type has been specified to grenad")]
    GrenadInvalidCompressionType,
    #[error("invalid grenad file version")]
    GrenadInvalidFormatVersion,
    #[error("invalid merge while processing {process}")]
    IndexingMergingKeys { process: &'static str },
    #[error(transparent)]
    PanicInThreadPool(#[from] CaughtPanic),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error("decoding from storage failed")]
    Decoding,
    #[error("encoding into storage failed")]
    Encoding,
    #[error(transparent)]
    Store(#[from] MdbError),
    #[error(
        "Cannot insert {key:?} and value with length {value_length} into database {database_name}: {error}"
    )]
    StorePut {
        database_name: &'static str,
        key: Box<[u8]>,
        value_length: usize,
        error: heed::Error,
    },
    #[error(transparent)]
    Utf8(#[from] str::Utf8Error),
    #[error("An indexation process was explicitly aborted")]
    AbortedIndexation,
}

#[derive(Error, Debug)]
pub enum UserError {
    #[error("A document cannot contain more than 65,535 fields.")]
    AttributeLimitReached,
    #[error("Maximum number of documents reached.")]
    DocumentLimitReached,
    #[error(
        "Document identifier `{}` is invalid. A document identifier can be an integer or a string of at most 511 alphanumeric, hyphen, or underscore characters.",
        .document_id
    )]
    InvalidDocumentId { document_id: Value },
    #[error("An LMDB environment is already opened")]
    EnvAlreadyOpened,
    #[error("The database file is in an invalid state.")]
    InvalidStoreFile,
    #[error("Maximum database size has been reached.")]
    MaxDatabaseSizeReached,
    #[error(
        "Document doesn't have a `{}` attribute: `{}`.",
        .primary_key,
        serde_json::to_string(.document).unwrap()
    )]
    MissingDocumentId { primary_key: String, document: Object },
    #[error("The index has no primary key.")]
    NoPrimaryKeyCandidateFound,
    #[error(transparent)]
    SerdeJson(serde_json::Error),
    #[error("An unknown internal document id was used: `{document_id}`.")]
    UnknownInternalDocumentId { document_id: DocumentId },
}

macro_rules! error_from_internal {
    ($($source:ty),+ $(,)?) => {
        $(
            impl From<$source> for Error {
                fn from(error: $source) -> Self {
                    InternalError::from(error).into()
                }
            }
        )+
    };
}

error_from_internal!(fst::Error, str::Utf8Error, CaughtPanic);

impl<E> From<grenad::Error<E>> for Error
where
    Error: From<E>,
{
    fn from(error: grenad::Error<E>) -> Error {
        match error {
            grenad::Error::Io(error) => Error::IoError(error),
            grenad::Error::Merge(error) => Error::from(error),
            grenad::Error::InvalidCompressionType => {
                InternalError::GrenadInvalidCompressionType.into()
            }
            grenad::Error::InvalidFormatVersion => InternalError::GrenadInvalidFormatVersion.into(),
        }
    }
}

impl From<Infallible> for Error {
    fn from(_error: Infallible) -> Error {
        unreachable!()
    }
}

pub fn handle_store_mdb_error(
    database_name: &'static str,
    key: &[u8],
    value_length: usize,
    error: heed::Error,
) -> Error {
    match error {
        heed::Error::Mdb(MdbError::MapFull) => Error::from(error),
        heed::Error::Mdb(mdb_error) => InternalError::StorePut {
            database_name,
            key: key.into(),
            value_length,
            error: mdb_error.into(),
        }
        .into(),
        non_mdb_error => Error::from(non_mdb_error),
    }
}

impl From<HeedError> for Error {
    fn from(error: HeedError) -> Error {
        match error {
            HeedError::Io(error) => Error::from(error),
            HeedError::Mdb(MdbError::MapFull) => UserError::MaxDatabaseSizeReached.into(),
            HeedError::Mdb(MdbError::Invalid) => UserError::InvalidStoreFile.into(),
            HeedError::Mdb(error) => InternalError::Store(error).into(),
            HeedError::Encoding(_) => InternalError::Encoding.into(),
            HeedError::Decoding(_) => InternalError::Decoding.into(),
            HeedError::EnvAlreadyOpened => UserError::EnvAlreadyOpened.into(),
        }
    }
}
