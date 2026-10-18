//! Finding text for the Ctrl+F bar and Ctrl+D: every place a query appears.

use std::ops::Range;

/// Every match of `query` in `text`, as byte ranges in order. Case is ignored unless
/// `match_case`; with `whole_word`, a match can't run on into a word: no letter, digit or `_`
/// right before a query that starts with one, or right after a query that ends with one. Matches
/// don't overlap; an empty query finds nothing.
pub fn find_all(text: &str, query: &str, match_case: bool, whole_word: bool) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if query.is_empty() {
        return found;
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let (word_start, word_end) = (word(query.chars().next()), word(query.chars().next_back()));
    let mut from = 0;
    while let Some(next) = text[from..].chars().next() {
        match match_length(&text[from..], query, match_case) {
            Some(length)
                if !whole_word
                    || !(word_start && word(text[..from].chars().next_back())
                        || word_end && word(text[from + length..].chars().next())) =>
            {
                found.push(from..from + length);
                from += length;
            }
            _ => from += next.len_utf8(),
        }
    }
    found
}

/// How many bytes of the start of `text` match `query` (ignoring case unless `match_case`), if
/// it does.
fn match_length(text: &str, query: &str, match_case: bool) -> Option<usize> {
    let mut chars = text.char_indices();
    for wanted in query.chars() {
        let (_, found) = chars.next()?;
        let same = match match_case {
            true => found == wanted,
            false => found.to_lowercase().eq(wanted.to_lowercase()),
        };
        if !same {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(at, _)| at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_match_ignoring_case() {
        assert_eq!(
            find_all("Tag <tag> TAG", "tag", false, false),
            vec![0..3, 5..8, 10..13]
        );
    }

    #[test]
    fn matches_never_overlap() {
        assert_eq!(find_all("aaaa", "aa", false, false), vec![0..2, 2..4]);
    }

    #[test]
    fn empty_query_or_no_match_finds_nothing() {
        assert!(find_all("text", "", false, false).is_empty());
        assert!(find_all("text", "xyz", false, false).is_empty());
        assert!(find_all("", "a", false, false).is_empty());
    }

    #[test]
    fn ranges_are_bytes_of_the_text_around_wide_characters() {
        let text = "café Café";
        let found = find_all(text, "CAFÉ", false, false);
        assert_eq!(found, vec![0..5, 6..11]);
        assert_eq!(&text[found[1].clone()], "Café");
    }

    #[test]
    fn match_case_finds_only_the_same_letters() {
        assert_eq!(find_all("Tag <tag> TAG", "tag", true, false), vec![5..8]);
    }

    #[test]
    fn whole_word_skips_matches_inside_words() {
        let text = "cat concat cat_x (cat) caté";
        assert_eq!(find_all(text, "cat", false, true), vec![0..3, 18..21]);
        // Only an end with a word character needs a boundary, so "cat " may run into "cat".
        assert_eq!(find_all("cat cat", "cat ", false, true), vec![0..4]);
        assert_eq!(
            find_all("xcat cat", "cat ", false, true),
            Vec::<Range<usize>>::new()
        );
    }
}
