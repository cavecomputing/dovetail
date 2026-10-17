//! Word and token counts. The tokenizer (OpenAI's o200k) takes a moment to load, so it loads on a
//! background thread and token counts are `None` until it is ready.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

static TOKENIZER: OnceLock<&'static CoreBPE> = OnceLock::new();

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
    TOKENIZER.get_or_init(tiktoken_rs::o200k_base_singleton);
}

/// The o200k token count, or `None` while the tokenizer is still loading.
pub fn token_count(text: &str) -> Option<usize> {
    TOKENIZER
        .get()
        .map(|bpe| bpe.encode_with_special_tokens(text).len())
}

/// Where each o200k token of `text` starts, as byte offsets, or `None` while the tokenizer is
/// still loading. A token starts at its first non-space byte, so " She" goes with "She" rather
/// than the space before it; a token of only spaces or line breaks goes with the token before.
pub fn token_starts(text: &str) -> Option<Vec<usize>> {
    let bpe = TOKENIZER.get()?;
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
    for token in bpe.encode_with_special_tokens(text) {
        let bytes = bpe
            .decode_bytes(&[token])
            .expect("a token the tokenizer just made decodes");
        let start = match bytes.iter().position(|byte| !byte.is_ascii_whitespace()) {
            Some(spaces) => at + spaces,
            None => starts.last().copied().unwrap_or(at),
        };
        starts.push(start);
        at += bytes.len();
    }
    Some(starts)
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

/// How much `new` grew or shrank against `old`, in whole percent: by tokens once both are known,
/// by words until then. `None` when `old` is empty.
pub fn percent_change(old: Counts, new: Counts) -> Option<i64> {
    let (old, new) = match (old.tokens, new.tokens) {
        (Some(old), Some(new)) => (old, new),
        _ => (old.words, new.words),
    };
    if old == 0 {
        return None;
    }
    Some(((new as f64 - old as f64) / old as f64 * 100.0).round() as i64)
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
    fn token_starts_skip_leading_spaces_and_count_every_token() {
        load_tokenizer();
        let text = "Nyx is quiet. She keeps her distance.\n\nShe never explains herself.";
        let starts = token_starts(text).unwrap();
        assert_eq!(Some(starts.len()), token_count(text));
        assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
        let she = text.find("She").unwrap();
        assert!(starts.contains(&she), "\" She\" starts at the S");
        assert!(!starts.contains(&(she - 1)));
    }

    #[test]
    fn percent_change_prefers_tokens() {
        let old = Counts {
            words: 200,
            tokens: Some(286),
        };
        let new = Counts {
            words: 118,
            tokens: Some(157),
        };
        assert_eq!(percent_change(old, new), Some(-45));
        let loading = Counts {
            words: 300,
            tokens: None,
        };
        assert_eq!(percent_change(old, loading), Some(50));
        assert_eq!(percent_change(Counts::default(), new), None);
    }
}
