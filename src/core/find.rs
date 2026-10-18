//! Finding text for the Ctrl+F bar: every place a query appears, ignoring case.

use std::ops::Range;

/// Every match of `query` in `text`, as byte ranges in order, ignoring case. Matches don't
/// overlap; an empty query finds nothing.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if query.is_empty() {
        return found;
    }
    let mut from = 0;
    while let Some(next) = text[from..].chars().next() {
        match match_length(&text[from..], query) {
            Some(length) => {
                found.push(from..from + length);
                from += length;
            }
            None => from += next.len_utf8(),
        }
    }
    found
}

/// How many bytes of the start of `text` match `query`, ignoring case, if it does.
fn match_length(text: &str, query: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for wanted in query.chars() {
        let (_, found) = chars.next()?;
        if !found.to_lowercase().eq(wanted.to_lowercase()) {
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
        assert_eq!(find_all("Tag <tag> TAG", "tag"), vec![0..3, 5..8, 10..13]);
    }

    #[test]
    fn matches_never_overlap() {
        assert_eq!(find_all("aaaa", "aa"), vec![0..2, 2..4]);
    }

    #[test]
    fn empty_query_or_no_match_finds_nothing() {
        assert!(find_all("text", "").is_empty());
        assert!(find_all("text", "xyz").is_empty());
        assert!(find_all("", "a").is_empty());
    }

    #[test]
    fn ranges_are_bytes_of_the_text_around_wide_characters() {
        let text = "café Café";
        let found = find_all(text, "CAFÉ");
        assert_eq!(found, vec![0..5, 6..11]);
        assert_eq!(&text[found[1].clone()], "Café");
    }
}
