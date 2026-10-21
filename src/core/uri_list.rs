//! The files in a `text/uri-list`, the form a file manager hands over when files are dragged onto
//! a window under Wayland (see `ui/wayland_drop.rs`).

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// The local files in `list`: one URI per line, `file:///home/me/notes.md` or
/// `file://localhost/...`, with `%20` and the like for bytes a URI can't hold. Lines starting
/// with `#` are comments, and anything other than a `file://` URI is left out.
pub fn files_in_uri_list(list: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for line in list.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("file://") else {
            continue;
        };
        // The host, usually empty or `localhost`, runs up to the path's first `/`.
        let Some(start) = rest.find('/') else {
            continue;
        };
        files.push(PathBuf::from(OsString::from_vec(percent_decoded(
            &rest[start..],
        ))));
    }
    files
}

/// `text` with each `%` and two hex digits turned back into the byte they stand for.
fn percent_decoded(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok());
        match (
            bytes[i],
            hex.and_then(|pair| u8::from_str_radix(pair, 16).ok()),
        ) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                i += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_file_and_skips_comments_and_other_uris() {
        let list = "# from the file manager\r\nfile:///home/me/notes.md\r\n\
                    https://example.com/a.txt\r\nfile://localhost/etc/hosts\r\n";
        assert_eq!(
            files_in_uri_list(list),
            [
                PathBuf::from("/home/me/notes.md"),
                PathBuf::from("/etc/hosts")
            ]
        );
    }

    #[test]
    fn decodes_escaped_bytes() {
        let list = "file:///home/me/My%20Notes/caf%C3%A9.txt\n";
        assert_eq!(
            files_in_uri_list(list),
            [PathBuf::from("/home/me/My Notes/café.txt")]
        );
        // A `%` without two hex digits after it is kept as it is.
        assert_eq!(
            files_in_uri_list("file:///a%2/b%zz"),
            [PathBuf::from("/a%2/b%zz")]
        );
    }
}
