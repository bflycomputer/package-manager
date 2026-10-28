use heed::types::{Bytes, DecodeIgnore};
use heed::{BytesDecode, RoTxn};

use crate::heed_codec::facet::FacetGroupKeyCodec;
use crate::heed_codec::BytesRefCodec;

pub use facet_sort_ascending::ascending_facet_sort;
pub use facet_sort_descending::descending_facet_sort;

mod facet_sort_ascending;
mod facet_sort_descending;

pub(crate) fn get_first_facet_value<'t, BoundCodec, DC>(
    txn: &'t RoTxn<'t>,
    db: heed::Database<FacetGroupKeyCodec<BytesRefCodec>, DC>,
    field_id: u16,
) -> heed::Result<Option<BoundCodec::DItem>>
where
    BoundCodec: BytesDecode<'t>,
{
    let mut level0_prefix = Vec::from(field_id.to_be_bytes());
    level0_prefix.push(0);
    let mut values =
        db.remap_types::<Bytes, DecodeIgnore>().prefix_iter(txn, level0_prefix.as_slice())?;
    let Some(first) = values.next() else {
        return Ok(None);
    };
    let (key, _) = first?;
    let key = FacetGroupKeyCodec::<BoundCodec>::bytes_decode(key).map_err(heed::Error::Decoding)?;
    Ok(Some(key.left_bound))
}

pub(crate) fn get_last_facet_value<'t, BoundCodec, DC>(
    txn: &'t RoTxn<'t>,
    db: heed::Database<FacetGroupKeyCodec<BytesRefCodec>, DC>,
    field_id: u16,
) -> heed::Result<Option<BoundCodec::DItem>>
where
    BoundCodec: BytesDecode<'t>,
{
    let mut level0_prefix = Vec::from(field_id.to_be_bytes());
    level0_prefix.push(0);
    let mut values =
        db.remap_types::<Bytes, DecodeIgnore>().rev_prefix_iter(txn, level0_prefix.as_slice())?;
    let Some(last) = values.next() else {
        return Ok(None);
    };
    let (key, _) = last?;
    let key = FacetGroupKeyCodec::<BoundCodec>::bytes_decode(key).map_err(heed::Error::Decoding)?;
    Ok(Some(key.left_bound))
}

pub(crate) fn get_highest_level<'t, DC>(
    txn: &'t RoTxn<'t>,
    db: heed::Database<FacetGroupKeyCodec<BytesRefCodec>, DC>,
    field_id: u16,
) -> heed::Result<u8> {
    let field_id_prefix = &field_id.to_be_bytes();
    Ok(db
        .remap_types::<Bytes, DecodeIgnore>()
        .rev_prefix_iter(txn, field_id_prefix)?
        .next()
        .map(|entry| {
            let (key, _) = entry.unwrap();
            FacetGroupKeyCodec::<BytesRefCodec>::bytes_decode(key).unwrap().level
        })
        .unwrap_or(0))
}
