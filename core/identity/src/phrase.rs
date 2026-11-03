//! The 12 recovery words: generate, parse, and hint at typos.

use std::fmt;

use bip39::{Language, Mnemonic};
use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, Zeroizing};

use crate::derive::Identity;

/// The only accepted number of words. 12 words = 128 bits of entropy plus a
/// 4-bit checksum.
pub const WORD_COUNT: usize = 12;

const ENTROPY_LEN: usize = 16;
const MAX_SUGGESTIONS: usize = 4;
const MAX_TYPO_DISTANCE: usize = 2;
/// BIP-39 English words are unique in their first 4 letters.
const UNIQUE_PREFIX: usize = 4;

/// The 2048 BIP-39 English words, in order.
pub fn word_list() -> &'static [&'static str; 2048] {
    Language::English.word_list()
}

/// Whether `word` (after the same clean-up as [`RecoveryPhrase::parse`]) is
/// in the word list.
pub fn is_word(word: &str) -> bool {
    let word = Zeroizing::new(normalize_word(word));
    Language::English.find_word(&word).is_some()
}

/// The closest words for one typed word, best first, at most 4. Empty when
/// the word is already in the list or nothing is close. Use it for live
/// hints on the restore screen; [`RecoveryPhrase::parse`] uses it too.
pub fn suggest(word: &str) -> Vec<&'static str> {
    let word = Zeroizing::new(normalize_word(word));
    suggest_normalized(&word)
}

/// `count` distinct random positions (0-based, sorted) to ask for in the
/// recovery-word check.
pub fn check_positions(count: usize) -> Vec<usize> {
    let count = count.min(WORD_COUNT);
    let mut positions: Vec<usize> = (0..WORD_COUNT).collect();
    // Partial Fisher-Yates.
    for i in 0..count {
        let j = i + (OsRng.next_u32() as usize) % (WORD_COUNT - i);
        positions.swap(i, j);
    }
    positions.truncate(count);
    positions.sort_unstable();
    positions
}

/// 12 recovery words. Zeroized on drop; `Debug` never shows the words.
#[derive(Clone)]
pub struct RecoveryPhrase(Mnemonic);

impl RecoveryPhrase {
    /// New random words from the OS random number generator.
    pub fn generate() -> RecoveryPhrase {
        let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
        OsRng.fill_bytes(&mut *entropy);
        RecoveryPhrase::from_entropy(&entropy)
    }

    /// The words for exactly these 128 bits.
    pub fn from_entropy(entropy: &[u8; ENTROPY_LEN]) -> RecoveryPhrase {
        let mnemonic = Mnemonic::from_entropy_in(Language::English, entropy)
            .expect("16 bytes is a valid BIP-39 entropy length");
        RecoveryPhrase(mnemonic)
    }

    /// Parses typed or pasted words. Accepts any case, spaces, commas, line
    /// breaks and numbering ("1. abandon 2. ability ..."), and words typed
    /// with a Hebrew keyboard layout by mistake ("שנשמגםמ" is "abandon").
    ///
    /// Errors, checked in this order: not exactly 12 words; words not in the
    /// list (all of them, each with suggestions); checksum mismatch.
    pub fn parse(input: &str) -> Result<RecoveryPhrase, PhraseError> {
        let words: Vec<Zeroizing<String>> = input
            .split(|c: char| c.is_whitespace() || c == ',')
            // Numbering such as "1." or "(2)" is not a word.
            .filter(|token| token.chars().any(char::is_alphabetic))
            .map(|token| Zeroizing::new(normalize_word(token)))
            .collect();
        if words.len() != WORD_COUNT {
            return Err(PhraseError::WordCount { found: words.len() });
        }

        let unknown: Vec<UnknownWord> = words
            .iter()
            .enumerate()
            .filter(|(_, word)| Language::English.find_word(word).is_none())
            .map(|(position, word)| UnknownWord { position, suggestions: suggest_normalized(word) })
            .collect();
        if !unknown.is_empty() {
            return Err(PhraseError::UnknownWords(unknown));
        }

        let mut joined = Zeroizing::new(String::with_capacity(WORD_COUNT * 9));
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                joined.push(' ');
            }
            joined.push_str(word);
        }
        match Mnemonic::parse_in_normalized(Language::English, &joined) {
            Ok(mnemonic) => Ok(RecoveryPhrase(mnemonic)),
            Err(bip39::Error::InvalidChecksum) => Err(PhraseError::Checksum),
            // Count and words were checked above.
            Err(_) => Err(PhraseError::Checksum),
        }
    }

    /// The words, in order, for showing to the user.
    pub fn words(&self) -> [&'static str; WORD_COUNT] {
        let mut words = [""; WORD_COUNT];
        for (slot, word) in words.iter_mut().zip(self.0.words()) {
            *slot = word;
        }
        words
    }

    /// The word at a 0-based position.
    pub fn word(&self, position: usize) -> Option<&'static str> {
        self.0.words().nth(position)
    }

    /// The 128 bits the words encode.
    pub fn entropy(&self) -> Zeroizing<[u8; ENTROPY_LEN]> {
        let (mut array, len) = self.0.to_entropy_array();
        debug_assert_eq!(len, ENTROPY_LEN);
        let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
        entropy.copy_from_slice(&array[..ENTROPY_LEN]);
        array.zeroize();
        entropy
    }

    /// Derives every key from the words. Deterministic.
    pub fn identity(&self) -> Identity {
        // BIP-39 seed with an empty passphrase (PBKDF2-HMAC-SHA512, 2048
        // rounds). The app lock passphrase is separate and never goes here.
        let seed = Zeroizing::new(self.0.to_seed_normalized(""));
        Identity::from_seed(&seed)
    }

    /// The recovery-word check: `answers` are (0-based position, typed word).
    /// True only if there is at least one answer and every one matches.
    pub fn check_words(&self, answers: &[(usize, &str)]) -> bool {
        let indices: Vec<usize> = self.0.word_indices().collect();
        let mut ok = !answers.is_empty();
        for (position, typed) in answers {
            let typed = Zeroizing::new(normalize_word(typed));
            let found = Language::English.find_word(&typed).map(usize::from);
            ok &= found.is_some() && indices.get(*position).copied() == found;
        }
        ok
    }
}

impl fmt::Debug for RecoveryPhrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryPhrase([redacted])")
    }
}

/// Why typed words were rejected.
#[derive(Clone, PartialEq, Eq)]
pub enum PhraseError {
    /// Not exactly 12 words.
    WordCount { found: usize },
    /// Words not in the word list, in order.
    UnknownWords(Vec<UnknownWord>),
    /// Every word is in the list, but they do not form a valid phrase: one
    /// is wrong (or two are swapped).
    Checksum,
}

impl PhraseError {
    /// 0-based positions of the words to mark on the restore screen.
    pub fn positions(&self) -> Vec<usize> {
        match self {
            PhraseError::UnknownWords(words) => words.iter().map(|w| w.position).collect(),
            _ => Vec::new(),
        }
    }
}

/// A typed word that is not in the list. Holds the closest list words, not
/// the typed text.
#[derive(Clone, PartialEq, Eq)]
pub struct UnknownWord {
    /// 0-based.
    pub position: usize,
    /// Best first; may be empty.
    pub suggestions: Vec<&'static str>,
}

// Suggestions are close to a secret word, so `Debug` shows only counts.
impl fmt::Debug for UnknownWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnknownWord")
            .field("position", &self.position)
            .field("suggestions", &self.suggestions.len())
            .finish()
    }
}

impl fmt::Debug for PhraseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhraseError::WordCount { found } => {
                f.debug_struct("WordCount").field("found", found).finish()
            }
            PhraseError::UnknownWords(words) => f.debug_tuple("UnknownWords").field(words).finish(),
            PhraseError::Checksum => f.write_str("Checksum"),
        }
    }
}

impl fmt::Display for PhraseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhraseError::WordCount { found } => {
                write!(f, "expected {WORD_COUNT} recovery words, found {found}")
            }
            PhraseError::UnknownWords(words) => {
                f.write_str("not in the word list: word")?;
                for (i, word) in words.iter().enumerate() {
                    let sep = if i == 0 { " " } else { ", " };
                    write!(f, "{sep}{}", word.position + 1)?;
                }
                Ok(())
            }
            PhraseError::Checksum => f.write_str("the recovery words do not match each other"),
        }
    }
}

impl std::error::Error for PhraseError {}

fn suggest_normalized(word: &str) -> Vec<&'static str> {
    if word.is_empty() || Language::English.find_word(word).is_some() {
        return Vec::new();
    }
    let len = word.chars().count();
    let unique_prefix: Option<String> =
        (len >= UNIQUE_PREFIX).then(|| word.chars().take(UNIQUE_PREFIX).collect());
    let scored: Vec<(usize, &'static str)> = word_list()
        .iter()
        .filter_map(|&candidate| {
            let score = if candidate.starts_with(word)
                || unique_prefix.as_deref().is_some_and(|p| candidate.starts_with(p))
            {
                0
            } else {
                strsim::osa_distance(word, candidate)
            };
            (score <= MAX_TYPO_DISTANCE).then_some((score, candidate))
        })
        .collect();
    // Only the closest ones. Among equals, people rarely get the start of a
    // word wrong: prefer the same first letters.
    let Some(best) = scored.iter().map(|&(score, _)| score).min() else {
        return Vec::new();
    };
    let mut closest: Vec<(bool, usize, &'static str)> = scored
        .into_iter()
        .filter(|&(score, _)| score == best)
        .map(|(_, candidate)| {
            let common = word.chars().zip(candidate.chars()).take_while(|(a, b)| a == b).count();
            (common == 0, usize::MAX - common, candidate)
        })
        .collect();
    closest.sort_unstable();
    closest.into_iter().take(MAX_SUGGESTIONS).map(|(_, _, w)| w).collect()
}

/// Lower-case, Hebrew keyboard layout mapped to the Latin letter on the same
/// key, and anything that is not a letter dropped (so "1." disappears).
fn normalize_word(token: &str) -> String {
    let hebrew = token.chars().any(|c| ('\u{05D0}'..='\u{05EA}').contains(&c));
    token
        .chars()
        .filter_map(|c| {
            if hebrew {
                if let Some(latin) = hebrew_key(c) {
                    return Some(latin);
                }
            }
            c.is_ascii_alphabetic().then(|| c.to_ascii_lowercase())
        })
        .collect()
}

/// The Latin letter on the same key of the standard Israeli (SI-1452)
/// layout. On that layout Q and W type '/' and '\''.
fn hebrew_key(c: char) -> Option<char> {
    Some(match c {
        '/' => 'q',
        '\'' | '׳' => 'w',
        'ק' => 'e',
        'ר' => 'r',
        'א' => 't',
        'ט' => 'y',
        'ו' => 'u',
        'ן' => 'i',
        'ם' => 'o',
        'פ' => 'p',
        'ש' => 'a',
        'ד' => 's',
        'ג' => 'd',
        'כ' => 'f',
        'ע' => 'g',
        'י' => 'h',
        'ח' => 'j',
        'ל' => 'k',
        'ך' => 'l',
        'ז' => 'z',
        'ס' => 'x',
        'ב' => 'c',
        'ה' => 'v',
        'נ' => 'b',
        'מ' => 'n',
        'צ' => 'm',
        _ => return None,
    })
}
