use std::sync::atomic::AtomicBool;

use heed::RwTxn;

use super::super::channel::*;
use crate::documents::PrimaryKey;
use crate::error::handle_store_mdb_error;
use crate::fields_ids_map::metadata::FieldIdMapWithMetadata;
use crate::{Index, Result};

pub fn write_to_db(
    mut writer_receiver: WriterBbqueueReceiver<'_>,
    finished_extraction: &AtomicBool,
    index: &Index,
    wtxn: &mut RwTxn<'_>,
) -> Result<ChannelCongestion> {
    let span = tracing::trace_span!(target: "indexing::write_db", "all");
    let _entered = span.enter();
    let span = tracing::trace_span!(target: "indexing::write_db", "post_merge");
    let mut _entered_post_merge = None;
    while let Some(action) = writer_receiver.recv_action() {
        if _entered_post_merge.is_none()
            && finished_extraction.load(std::sync::atomic::Ordering::Relaxed)
        {
            _entered_post_merge = Some(span.enter());
        }

        match action {
            ReceiverAction::WakeUp => (),
            ReceiverAction::LargeEntry(LargeEntry { database, key, value }) => {
                let database_name = database.database_name();
                let database = database.database(index);
                if let Err(error) = database.put(wtxn, &key, &value) {
                    return Err(handle_store_mdb_error(
                        database_name,
                        &key,
                        value.len(),
                        error,
                    ));
                }
            }
        }

        // Every time there is a message in the channel we search
        // for new entries in the BBQueue buffers.
        write_from_bbqueue(&mut writer_receiver, index, wtxn)?;
    }

    write_from_bbqueue(&mut writer_receiver, index, wtxn)?;

    Ok(ChannelCongestion {
        attempts: writer_receiver.sent_messages_attempts(),
        blocking_attempts: writer_receiver.blocking_sent_messages_attempts(),
    })
}

/// Stats exposing the congestion of a channel.
#[derive(Debug, Copy, Clone)]
pub struct ChannelCongestion {
    /// Number of attempts to send a message into the bbqueue buffer.
    pub attempts: usize,
    /// Number of blocking attempts which require a retry.
    pub blocking_attempts: usize,
}

impl ChannelCongestion {
    pub fn congestion_ratio(&self) -> f32 {
        self.blocking_attempts as f32 / self.attempts as f32
    }

    pub fn merge(left: Option<Self>, right: Option<Self>) -> Option<Self> {
        match (left, right) {
            (None, None) => None,
            (None, Some(this)) | (Some(this), None) => Some(this),
            (
                Some(Self { attempts: left_attempts, blocking_attempts: left_blocking_attempts }),
                Some(Self { attempts: right_attempts, blocking_attempts: right_blocking_attempts }),
            ) => Some(Self {
                attempts: left_attempts + right_attempts,
                blocking_attempts: left_blocking_attempts + right_blocking_attempts,
            }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update_index(
    index: &Index,
    wtxn: &mut RwTxn<'_>,
    new_fields_ids_map: FieldIdMapWithMetadata,
    new_primary_key: Option<PrimaryKey<'_>>,
    document_ids: roaring::RoaringBitmap,
) -> Result<()> {
    index.put_fields_ids_map(wtxn, new_fields_ids_map.as_fields_ids_map())?;
    if let Some(new_primary_key) = new_primary_key {
        index.put_primary_key(wtxn, new_primary_key.name())?;
    }
    index.put_documents_ids(wtxn, &document_ids)?;
    Ok(())
}

/// A function dedicated to manage all the available BBQueue frames.
///
/// It reads all the available frames, do the corresponding database operations
/// and stops when no frame are available.
pub fn write_from_bbqueue(
    writer_receiver: &mut WriterBbqueueReceiver<'_>,
    index: &Index,
    wtxn: &mut RwTxn<'_>,
) -> crate::Result<()> {
    while let Some(frame_with_header) = writer_receiver.recv_frame() {
        match frame_with_header.header() {
            EntryHeader::DbOperation(operation) => {
                let database_name = operation.database.database_name();
                let database = operation.database.database(index);
                let frame = frame_with_header.frame();
                let (key, value) = operation.key_value(frame);
                if let Err(error) = database.put(wtxn, key, value) {
                    return Err(handle_store_mdb_error(
                        database_name,
                        key,
                        value.len(),
                        error,
                    ));
                }
            }
        }
    }

    Ok(())
}
