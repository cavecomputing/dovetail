//! Word and token counts. The tokenizer (OpenAI's o200k) takes a moment to load, so it loads on a
//! background thread and token counts are `None` until it is ready.
//!
//! Counting is all it does, so it keeps only what that needs, each token's bytes and rank, and
//! takes about 14 MB. tiktoken-rs, which this replaced, also kept a table back from ranks to
//! bytes, a sorted copy of every token and 128 copies of the splitting pattern, for 47 MB.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::OnceLock;

use fancy_regex::Regex;
use rustc_hash::FxHashMap;

static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();

/// The vocabulary (see assets/tokenizer/README.md).
const VOCABULARY: &str = include_str!("../../assets/tokenizer/o200k_base.tiktoken");

/// How o200k splits text into pieces before merging each piece's bytes into tokens (tiktoken's
/// own pattern).
const PIECES: &str = concat!(
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"\p{N}{1,3}",
    "|",
    r" ?[^\s\p{L}\p{N}]+[\r\n/]*",
    "|",
    r"\s*[\r\n]+",
    "|",
    r"\s+(?!\S)",
    "|",
    r"\s+",
);

struct Tokenizer {
    /// Each token's bytes and its rank: the lower, the sooner its bytes merge.
    ranks: FxHashMap<&'static [u8], u32>,
    pieces: Regex,
}

impl Tokenizer {
    fn load() -> Tokenizer {
        let mut bytes = Vec::with_capacity(VOCABULARY.len());
        let mut tokens = Vec::new();
        for line in VOCABULARY.lines() {
            let (token, rank) = line
                .split_once(' ')
                .expect("each line is a token and its rank");
            let start = bytes.len();
            decode_base64(token, &mut bytes);
            let rank: u32 = rank.parse().expect("ranks are numbers");
            tokens.push((start..bytes.len(), rank));
        }
        // The tokens' bytes live as long as the app, so the table can point into them.
        let bytes: &'static [u8] = bytes.leak();
        Tokenizer {
            ranks: tokens
                .into_iter()
                .map(|(range, rank)| (&bytes[range], rank))
                .collect(),
            pieces: Regex::new(PIECES).expect("the pattern is valid"),
        }
    }

    fn count(&self, text: &str) -> usize {
        self.pieces
            .find_iter(text)
            .flatten()
            .map(|piece| self.piece_tokens(piece.as_str().as_bytes()))
            .sum()
    }

    /// How many tokens `piece` becomes: starting from its single bytes, the two neighbouring
    /// parts whose joined bytes rank lowest (the leftmost of equals) join, until no two neighbours
    /// join into a token.
    fn piece_tokens(&self, piece: &[u8]) -> usize {
        if self.ranks.contains_key(piece) {
            return 1;
        }
        // The parts, by the byte each starts at: where it ends (0 once it has joined the one
        // before), and where the one before it starts.
        let mut end: Vec<usize> = (1..=piece.len()).collect();
        let mut before: Vec<Option<usize>> = (0..piece.len()).map(|at| at.checked_sub(1)).collect();
        // Pairs that could join, lowest rank and then leftmost first. A pair goes stale when
        // either part joins another; its end then no longer matches.
        let mut pairs = BinaryHeap::new();
        let rank = |start: usize, pair_end: usize| self.ranks.get(&piece[start..pair_end]).copied();
        for start in 0..piece.len() - 1 {
            if let Some(rank) = rank(start, start + 2) {
                pairs.push(Reverse((rank, start, start + 2)));
            }
        }
        let mut parts = piece.len();
        while let Some(Reverse((_, start, pair_end))) = pairs.pop() {
            let next = end[start];
            if next == 0 || next >= piece.len() || end[next] != pair_end {
                continue;
            }
            end[start] = pair_end;
            end[next] = 0;
            parts -= 1;
            if pair_end < piece.len() {
                before[pair_end] = Some(start);
                if let Some(rank) = rank(start, end[pair_end]) {
                    pairs.push(Reverse((rank, start, end[pair_end])));
                }
            }
            if let Some(first) = before[start]
                && let Some(rank) = rank(first, pair_end)
            {
                pairs.push(Reverse((rank, first, pair_end)));
            }
        }
        parts
    }
}

/// Appends the bytes that standard base64 `text` stands for to `bytes`.
fn decode_base64(text: &str, bytes: &mut Vec<u8>) {
    let (mut bits, mut held) = (0u32, 0);
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        bits = bits << 6 | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
        }
    }
}

/// Words are runs of non-whitespace.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Starts loading the tokenizer on its own thread and calls `done` once it is ready, so the UI
/// can recount. Calling it again is harmless.
pub fn load_tokenizer_in_background(done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        load_tokenizer();
        done();
    });
}

/// Loads the tokenizer on the calling thread. Blocks until ready.
pub fn load_tokenizer() {
    TOKENIZER.get_or_init(Tokenizer::load);
}

/// The o200k token count, or `None` while the tokenizer is still loading.
pub fn token_count(text: &str) -> Option<usize> {
    TOKENIZER.get().map(|tokenizer| tokenizer.count(text))
}

/// A text's word and token counts; `tokens` is `None` while the tokenizer loads.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counts {
    pub words: usize,
    pub tokens: Option<usize>,
}

impl Counts {
    pub fn of(text: &str) -> Counts {
        Counts {
            words: word_count(text),
            tokens: token_count(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   \n\t "), 0);
        assert_eq!(word_count("one"), 1);
        assert_eq!(word_count("  two words\nand   three\t"), 4);
        assert_eq!(word_count("<tag attr=\"x\">{{char}}</tag>"), 2);
    }

    #[test]
    fn background_load_signals_and_counts_tokens() {
        let (sender, receiver) = std::sync::mpsc::channel();
        load_tokenizer_in_background(move || sender.send(()).unwrap());
        receiver.recv().unwrap();
        assert_eq!(token_count(""), Some(0));
        let count = token_count("Hello, world! This is a test.").unwrap();
        assert!((5..=12).contains(&count), "got {count}");
    }

    #[test]
    fn decodes_base64() {
        let mut bytes = Vec::new();
        for text in ["SGk=", "IQ==", "YWJj"] {
            decode_base64(text, &mut bytes);
        }
        assert_eq!(bytes, b"Hi!abc");
    }

    /// The same counts as tiktoken itself, over prose, code, other scripts, emoji, long runs
    /// (which take the most merging) and whitespace.
    #[test]
    fn counts_tokens_as_tiktoken_does() {
        load_tokenizer();
        let tiktoken = tiktoken_rs::o200k_base().unwrap();
        let samples = [
            "Hello, world! This is a test.",
            "fn main() {\n    println!(\"{}\", 1 + 2);\n}\n",
            "I'm sure they'll say it's what we'd've done. DON'T SHOUT.",
            "Ünïcödé, 日本語のテキスト, русский текст, عربي, 🎉🦀👍🏽",
            "    indented\n\n\n\ttabs\r\nand crlf  trailing   \n",
            "12345678901234567890 3.14159 -42 0x1F",
            "<tag attr=\"x\">{{char}}</tag> https://example.com/a/b?c=d#e",
        ];
        let long = [
            "a".repeat(5000),
            "=-".repeat(3000),
            "ab1".repeat(2000),
            " ".repeat(4000),
            "lorem ipsum dolor sit amet ".repeat(400),
            "dGhlIHF1aWNrIGJyb3du".repeat(300),
        ];
        for text in samples
            .iter()
            .copied()
            .chain(long.iter().map(String::as_str))
        {
            assert_eq!(
                token_count(text),
                Some(tiktoken.encode_ordinary(text).len()),
                "{:?}",
                &text[..text.len().min(40)]
            );
        }
    }
}
