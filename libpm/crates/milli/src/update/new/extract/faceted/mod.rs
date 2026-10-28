mod extract_facets;
mod facet_document;

pub use extract_facets::FacetedDocidsExtractor;

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum FacetKind {
    Number = 0,
    String = 1,
}

impl From<u8> for FacetKind {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Number,
            1 => Self::String,
            _ => unreachable!(),
        }
    }
}

impl FacetKind {
    pub fn extract_from_key(key: &[u8]) -> (FacetKind, &[u8]) {
        (FacetKind::from(key[0]), &key[1..])
    }
}
