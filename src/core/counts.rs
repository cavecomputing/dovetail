//! Word and token counts. The token count is an estimate: a token is about four bytes of text
//! for the tokenizers the current models use (OpenAI's o200k, Claude's, Gemini's), in prose, code
//! and other scripts alike. An exact count would need a model's vocabulary (3.6 MB) and a merge
//! over every piece of the text, for a number the status bar only shows as a size.

/// Words are runs of non-whitespace.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// About how many tokens `text` is: one for every four bytes, rounded up.
pub fn token_count(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// A text's word and token counts.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counts {
    pub words: usize,
    pub tokens: usize,
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
    fn estimates_tokens_by_bytes() {
        assert_eq!(token_count(""), 0);
        assert_eq!(token_count("one"), 1);
        // 29 bytes: o200k makes 8 tokens of it.
        assert_eq!(token_count("Hello, world! This is a test."), 8);
        // Bytes, not characters: this is 24 bytes.
        assert_eq!(token_count("日本語のテキスト"), 6);
    }
}
