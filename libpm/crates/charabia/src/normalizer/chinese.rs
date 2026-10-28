use super::CharNormalizer;
use crate::detection::{Language, Script};
use crate::normalizer::CharOrStr;
use crate::Token;

/// Normalize Chinese characters by:
/// converting Z, Simplified, Semantic, Old, and Wrong variants.
pub struct ChineseNormalizer;

impl CharNormalizer for ChineseNormalizer {
    fn normalize_char(&self, c: char) -> Option<CharOrStr> {
        // Normalize Z, Simplified, Semantic, Old, and Wrong variants
        let kvariant = match irg_kvariants::KVARIANTS.get(&c) {
            Some(kvariant) => kvariant.destination_ideograph,
            None => c,
        };

        Some(kvariant.into())
    }

    fn should_normalize(&self, token: &Token) -> bool {
        token.script == Script::Cj
            && matches!(token.language, None | Some(Language::Cmn) | Some(Language::Zho))
    }
}

#[cfg(test)]
mod test {
    use std::borrow::Cow::Owned;

    use crate::normalizer::test::test_normalizer;
    use crate::normalizer::{Normalizer, NormalizerOption};
    use crate::token::TokenKind;

    // base tokens to normalize.
    fn tokens() -> Vec<Token<'static>> {
        vec![
            Token {
                lemma: Owned("尊嚴".to_string()),
                char_end: 2,
                byte_end: 6,
                script: Script::Cj,
                language: Some(Language::Cmn),
                ..Default::default()
            },
            Token {
                lemma: Owned("生而自由".to_string()),
                char_end: 4,
                byte_end: 12,
                script: Script::Cj,
                language: Some(Language::Cmn),
                ..Default::default()
            },
            Token {
                lemma: Owned("澚䀾亚㮺刄杤".to_string()),
                char_end: 5,
                byte_end: 15,
                script: Script::Cj,
                language: Some(Language::Zho),
                ..Default::default()
            },
        ]
    }

    // expected result of the current Normalizer.
    fn normalizer_result() -> Vec<Token<'static>> {
        vec![
            Token {
                lemma: Owned("尊嚴".to_string()),
                char_end: 2,
                byte_end: 6,
                char_map: Some(vec![(3, 3), (3, 3)]),
                script: Script::Cj,
                language: Some(Language::Cmn),
                ..Default::default()
            },
            Token {
                lemma: Owned("生而自由".to_string()),
                char_end: 4,
                byte_end: 12,
                char_map: Some(vec![(3, 3), (3, 3), (3, 3), (3, 3)]),
                script: Script::Cj,
                language: Some(Language::Cmn),
                ..Default::default()
            },
            Token {
                lemma: Owned("澳䁈亞本刃𣜜".to_string()),
                char_end: 5,
                byte_end: 15,
                char_map: Some(vec![(3, 3), (3, 3), (3, 3), (3, 3), (3, 3), (3, 4)]),
                script: Script::Cj,
                language: Some(Language::Zho),
                ..Default::default()
            },
        ]
    }

    // expected result of the complete Normalizer pieline.
    fn normalized_tokens() -> Vec<Token<'static>> {
        vec![
            Token {
                kind: TokenKind::Word,
                lemma: Owned("尊嚴".to_string()),
                char_start: 0,
                char_end: 2,
                byte_start: 0,
                byte_end: 6,
                char_map: Some(vec![(3, 3), (3, 3)]),
                script: Script::Cj,
                language: Some(Language::Cmn),
            },
            Token {
                kind: TokenKind::Word,
                lemma: Owned("生而自由".to_string()),
                char_start: 0,
                char_end: 4,
                byte_start: 0,
                byte_end: 12,
                char_map: Some(vec![(3, 3), (3, 3), (3, 3), (3, 3)]),
                script: Script::Cj,
                language: Some(Language::Cmn),
            },
            Token {
                kind: TokenKind::Word,
                lemma: Owned("澳䁈亞本刃𣜜".to_string()),
                char_start: 0,
                char_end: 5,
                byte_start: 0,
                byte_end: 15,
                char_map: Some(vec![(3, 3), (3, 3), (3, 3), (3, 3), (3, 3), (3, 4)]),
                script: Script::Cj,
                language: Some(Language::Zho),
            },
        ]
    }

    test_normalizer!(ChineseNormalizer, tokens(), normalizer_result(), normalized_tokens());
}
