use std::collections::BTreeSet;
use std::ops::BitOr;

#[derive(Default, Debug)]
pub struct WordDelta {
    words: BTreeSet<String>,
}

impl WordDelta {
    pub fn words(&self) -> impl Iterator<Item = &str> + '_ {
        self.words.iter().map(String::as_str)
    }

    pub fn insert(&mut self, word: String) {
        self.words.insert(word);
    }
}

impl BitOr for WordDelta {
    type Output = Self;

    fn bitor(mut self, rhs: Self) -> Self::Output {
        self.words.extend(rhs.words);
        self
    }
}
