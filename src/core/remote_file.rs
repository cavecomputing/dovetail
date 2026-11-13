//! A file on a host (`remote::Remote`): reading it whole, saving it so the host never holds half
//! a file or loses someone else's change, and the copy kept on this computer while text hasn't
//! reached the host.
//!
//! A save sends the text to a hidden temp file beside the target. The script on the host checks
//! that all of it arrived (a dropped connection ends the upload early, and looks no different
//! to `cat`), then that the target is still a file this window has seen, and only then renames
//! the temp file over it. So the host holds the old file or the new one, never a mix.
//!
//! Files are told apart by `cksum` of their contents. A window remembers the ones it has seen
//! in the data folder (`remote/<fingerprint>.json`): the file as last read or saved, and each
//! save sent since that the host never confirmed, as one of those may have arrived. Before a
//! save is sent, its text is also written beside that (`.txt`), and deleted once the host
//! confirms. Text that never arrived (the app was killed, the computer shut down) is therefore
//! still there, and comes back the next time the file is opened. Its undo steps are kept with it
//! (`.undo.json`), as scratch keeps its own, so Ctrl+Z still works on text that comes back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::lock::fingerprint;
use crate::core::opened::{NO_PERMISSION, NOT_UTF8};
use crate::core::remote::{FOLLOW_LINKS, Remote, Ssh, cksum};
use crate::core::save::atomic_write;
use crate::core::text_file::{LineEnding, decode, decode_lossy, encode};
use crate::core::text_model::Change;

/// The longest file opened from a host, in bytes: every save sends the whole file again.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// How many unconfirmed saves are remembered. One sent longer ago than that has long since
/// arrived or failed.
const MAX_SEEN: usize = 50;

/// Why a save was refused when the host's file isn't one this window has seen. The status bar
/// then offers Reload and Keep mine, as for a local file changed on disk.
pub const CHANGED: &str = "Save failed: changed on the host";

/// Answers for the file named on the input's first line: why it can't be read, or whether it
/// can be saved (`rw`), its checksum and length, and then the file itself. A file longer than
/// the second line allows is only measured (with `wc`, which asks the file's size: a checksum
/// would read all of a file that may be gigabytes long before saying it is too long).
const READ: [&str; 3] = [
    r#"
    IFS= read -r f; IFS= read -r max;
    if [ -d "$f" ]; then echo "@re@ folder"; exit 0; fi;
    if [ -f "$f" ]; then :; elif [ -e "$f" ]; then echo "@re@ special"; exit 0;
    else echo "@re@ missing"; exit 0; fi;
    if [ -r "$f" ]; then :; else echo "@re@ denied"; exit 0; fi;
    command -v cksum >/dev/null 2>&1 || { echo "@re@ nocksum"; exit 0; };
    "#,
    FOLLOW_LINKS,
    r#"
    w=ro; if [ -w "$f" ] && [ -w "$(dirname -- "$f")" ]; then w=rw; fi;
    set -- $(wc -c < "$f");
    if [ "$1" -gt "$max" ]; then echo "@re@ big $1"; exit 0; fi;
    set -- $(cksum < "$f");
    printf "@re@ file %s %s %s\n" "$w" "$1" "$2"; cat < "$f"
    "#,
];

/// Saves the rest of the input as the file named on its first line. The second line lists the
/// checksums the file there may have (`|` around each; `any` for whatever is there), the third
/// is the new text's, the fourth names the temp file. The text goes to the temp file, made as a
/// copy of the old file so it has its permissions; is checked to have arrived whole; and is
/// renamed over the file only if that is still one of those listed, or is gone. A file that
/// already holds the new text is left as it is.
const SAVE: [&str; 3] = [
    r#"
    IFS= read -r f; IFS= read -r seen; IFS= read -r new; IFS= read -r token;
    command -v cksum >/dev/null 2>&1 || { echo "@re@ nocksum"; exit 0; };
    "#,
    FOLLOW_LINKS,
    r#"
    if [ -e "$f" ] || [ -L "$f" ]; then
        if [ -f "$f" ]; then :; else echo "@re@ special"; exit 0; fi;
        if [ -w "$f" ]; then :; else echo "@re@ denied"; exit 0; fi;
    fi;
    t=$(dirname -- "$f")/.$token.tmp;
    cleanup() { rm -f -- "$t"; }; trap cleanup EXIT; trap "exit 1" HUP INT TERM PIPE;
    if [ -f "$f" ]; then cp -p -- "$f" "$t" || exit 1; fi;
    cat > "$t" || exit 1;
    set -- $(cksum < "$t");
    if [ "$1 $2" = "$new" ]; then :; else echo "@re@ cut"; exit 0; fi;
    sync "$t" 2>/dev/null;
    if [ -f "$f" ]; then set -- $(cksum < "$f"); cur="$1 $2"; else cur=absent; fi;
    if [ "$cur" = "$new" ]; then echo "@re@ saved"; exit 0; fi;
    case "|absent$seen" in *"|any|"*|*"|$cur|"*) ;; *) echo "@re@ changed"; exit 0;; esac;
    mv -f -- "$t" "$f" || exit 1;
    echo "@re@ saved"
    "#,
];

/// A file read from a host, ready to become the window's document.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    pub remote: Remote,
    pub text: String,
    pub line_ending: LineEnding,
    pub bom: bool,
    /// Why the text can't be edited; `None` means it can.
    pub read_only_reason: Option<String>,
    /// The host's file as read (its `cksum`).
    seen: String,
    /// Whether `text` is edits that never reached the host, kept on this computer since, in
    /// place of the host's file. The window saves them as soon as it shows them.
    pub unsent: bool,
    /// Unsent text's undo steps, oldest first.
    pub history: Vec<Vec<Change>>,
    /// Something to say once the file shows, such as where unsent text came from.
    pub notice: Option<String>,
}

/// Reads `remote` from its host, or says why it can't be opened. If text from an earlier window
/// never reached the host, that is what comes back (`Fetched::unsent`), even when the host
/// can't be reached now, so it is never out of reach.
pub fn fetch(ssh: &Ssh, data_dir: &Path, remote: &Remote) -> Result<Fetched, String> {
    let fetched = read(ssh, remote);
    let files = KeptFiles::of(data_dir, remote);
    let Some(unsent) = fs::read(&files.unsent).ok().and_then(|bytes| {
        let same = matches!(&fetched, Ok(fetched) if fetched.seen == cksum(&bytes));
        // Text the host already holds did arrive; only its answer was lost.
        (!same).then(|| decode(bytes).ok()).flatten()
    }) else {
        return fetched.map_err(|reason| format!("Couldn't open: {reason}"));
    };
    let host = &remote.host;
    let notice = match &fetched {
        Ok(_) => format!("These edits never reached {host}: saving them now"),
        Err(reason) => format!("Showing edits that never reached {host} ({reason})"),
    };
    Ok(Fetched {
        remote: remote.clone(),
        text: unsent.text,
        line_ending: unsent.line_ending,
        bom: unsent.bom,
        read_only_reason: None,
        seen: String::new(),
        unsent: true,
        history: fs::read_to_string(&files.undo)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        notice: Some(notice),
    })
}

/// Reads `remote` as its host holds it now, or says why it can't be read. What arrives is
/// checked against the checksum the host sent first, so a download cut short is refused, never
/// shown as the whole file.
pub fn read(ssh: &Ssh, remote: &Remote) -> Result<Fetched, String> {
    let host = &remote.host;
    let input = format!("{}\n{MAX_BYTES}\n", remote.path);
    let ran = ssh.run(host, &READ.concat(), input.into_bytes())?;
    let (words, bytes) = ran.answer(host)?;
    let megabytes = |bytes: usize| bytes.div_ceil(1024 * 1024);
    let (writable, seen) = match words.as_slice() {
        ["file", writable, crc, length] => (*writable == "rw", format!("{crc} {length}")),
        ["big", length] => {
            let size = megabytes(length.parse().unwrap_or(MAX_BYTES));
            let most = megabytes(MAX_BYTES);
            return Err(format!(
                "{size} MB is too big to open over ssh ({most} MB at most)"
            ));
        }
        ["folder"] => return Err("that is a folder".to_string()),
        ["special"] => return Err("not a regular file".to_string()),
        ["denied"] => return Err(format!("no permission to read it on {host}")),
        ["nocksum"] => return Err(format!("{host} has no cksum tool")),
        ["missing"] => return Err(format!("{host} has no such file")),
        _ => return Err(format!("{host} answered oddly")),
    };
    if cksum(bytes) != seen {
        return Err(format!("{remote} didn't arrive whole: try again"));
    }
    let mut fetched = Fetched {
        remote: remote.clone(),
        text: String::new(),
        line_ending: LineEnding::default(),
        bom: false,
        read_only_reason: (!writable).then(|| NO_PERMISSION.to_string()),
        seen,
        unsent: false,
        history: Vec::new(),
        notice: None,
    };
    // Not UTF-8: looked at, never saved, as for a file on this computer.
    if std::str::from_utf8(bytes).is_err() {
        fetched.text = decode_lossy(bytes).map_err(|error| error.to_string())?;
        fetched.read_only_reason = Some(NOT_UTF8.to_string());
        return Ok(fetched);
    }
    let file = decode(bytes.to_vec()).map_err(|error| error.to_string())?;
    fetched.text = file.text;
    fetched.line_ending = file.line_ending;
    fetched.bom = file.bom;
    Ok(fetched)
}

/// The files this window has seen at one place on a host.
#[derive(Serialize, Deserialize, Default)]
struct Kept {
    /// `alias:/path`, to say which file's this is.
    file: String,
    /// The checksums a save may find on the host and replace: first the file as last read or
    /// saved (`any` after Keep mine), then each save sent since that the host never confirmed.
    seen: Vec<String>,
}

/// Where what's kept about one file on a host lives, in the data folder's `remote` folder, named
/// from a fingerprint of `alias:/path`.
struct KeptFiles {
    /// The checksums seen (`Kept`).
    seen: PathBuf,
    /// The text not yet on the host, as the host's file would hold it.
    unsent: PathBuf,
    /// That text's undo steps.
    undo: PathBuf,
}

impl KeptFiles {
    fn of(data_dir: &Path, remote: &Remote) -> KeptFiles {
        let name = fingerprint(remote.to_string().as_bytes());
        let file = |ending: &str| data_dir.join("remote").join(format!("{name:016x}{ending}"));
        KeptFiles {
            seen: file(".json"),
            unsent: file(".txt"),
            undo: file(".undo.json"),
        }
    }

    fn store(&self, kept: &Kept) -> io::Result<()> {
        let json = serde_json::to_string(kept).map_err(io::Error::other)?;
        atomic_write(&self.seen, json.as_bytes())
    }

    /// Keeps `bytes` and their undo steps as the text not yet on the host.
    fn keep_unsent(&self, bytes: &[u8], history: &[Vec<Change>]) -> io::Result<()> {
        let json = serde_json::to_string(history).map_err(io::Error::other)?;
        atomic_write(&self.unsent, bytes)?;
        atomic_write(&self.undo, json.as_bytes())
    }

    /// The host has the text: nothing is unsent.
    fn drop_unsent(&self) {
        let _ = fs::remove_file(&self.unsent);
        let _ = fs::remove_file(&self.undo);
    }
}

/// Starts from `fetched` as the file the window has seen, when it shows it (or reloads it): the
/// only one a save may replace. Unsent text keeps the files seen before it was typed.
pub fn begin(data_dir: &Path, fetched: &Fetched) {
    if fetched.unsent {
        return;
    }
    let files = KeptFiles::of(data_dir, &fetched.remote);
    let _ = files.store(&Kept {
        file: fetched.remote.to_string(),
        seen: vec![fetched.seen.clone()],
    });
    files.drop_unsent();
}

/// Saves `text` as `remote` on its host (see the module's top), or says why it couldn't. Until
/// the host confirms, the text and `history`, its undo steps, are also kept on this computer.
pub fn save(
    ssh: &Ssh,
    data_dir: &Path,
    remote: &Remote,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    history: &[Vec<Change>],
) -> Result<(), String> {
    let host = &remote.host;
    let bytes = encode(text, line_ending, bom).into_bytes();
    let new = cksum(&bytes);
    let files = KeptFiles::of(data_dir, remote);
    let mut kept: Kept = fs::read_to_string(&files.seen)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    kept.file = remote.to_string();
    // Noted before it is sent: it may arrive without the host's answer getting back, and the
    // next save must then know the file it finds.
    kept.seen.retain(|seen| *seen != new);
    kept.seen.push(new.clone());
    if kept.seen.len() > MAX_SEEN {
        kept.seen.remove(1);
    }
    // If this computer's disk is full, the host may still take the save.
    let _ = files
        .keep_unsent(&bytes, history)
        .and_then(|()| files.store(&kept));

    // The temp file's name: this process and the time, so no two saves share one.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let token = format!(
        "save-{:x}-{:x}",
        std::process::id(),
        now.map_or(0, |since| since.as_nanos())
    );
    let seen = kept.seen.join("|");
    let mut input = format!("{}\n|{seen}|\n{new}\n{token}\n", remote.path).into_bytes();
    input.extend_from_slice(&bytes);
    let ran = ssh.run(host, &SAVE.concat(), input);
    let answer = ran.as_ref().map_err(String::clone).and_then(|ran| {
        let (words, _) = ran.answer(host)?;
        Ok(words.first().copied().unwrap_or_default())
    });
    let failed = match answer {
        Ok("saved") => {
            kept.seen = vec![new];
            let _ = files.store(&kept);
            // Unless newer text was kept meanwhile, as the app does when it ends mid-save.
            if fs::read(&files.unsent).is_ok_and(|unsent| unsent == bytes) {
                files.drop_unsent();
            }
            return Ok(());
        }
        Ok("changed") => return Err(CHANGED.to_string()),
        Ok("cut") => format!("lost the connection to {host}"),
        Ok("special") => "it isn't a file on the host any more".to_string(),
        Ok("denied") => format!("no permission to write it on {host}"),
        Ok("nocksum") => format!("{host} has no cksum tool"),
        Ok(_) => format!("{host} answered oddly"),
        Err(reason) => reason,
    };
    Err(format!("Save failed: {failed}"))
}

/// Keeps `text` and `history`, its undo steps, for `remote` on this computer alone, for when
/// the app ends with text that hasn't reached the host and there is no time to send it: `fetch`
/// brings both back.
pub fn keep_unsent(
    data_dir: &Path,
    remote: &Remote,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    history: &[Vec<Change>],
) -> io::Result<()> {
    let bytes = encode(text, line_ending, bom).into_bytes();
    KeptFiles::of(data_dir, remote).keep_unsent(&bytes, history)
}

/// Keep mine: the next save replaces whatever the host holds.
pub fn keep_mine(data_dir: &Path, remote: &Remote) {
    let _ = KeptFiles::of(data_dir, remote).store(&Kept {
        file: remote.to_string(),
        seen: vec!["any".to_string()],
    });
}

/// Lets go of `remote` when its window leaves it: nothing stays on this computer, unless text
/// still hasn't reached the host. `even_unsent` drops that too, for when the window has put
/// its text somewhere else (Save as).
pub fn forget(data_dir: &Path, remote: &Remote, even_unsent: bool) {
    let files = KeptFiles::of(data_dir, remote);
    if even_unsent {
        files.drop_unsent();
    }
    if !files.unsent.exists() {
        let _ = fs::remove_file(files.seen);
    }
}

/// The files that have text kept on this computer which never reached their host, as
/// `alias:/path`.
pub fn unsent(data_dir: &Path) -> Vec<Remote> {
    let Ok(dir) = fs::read_dir(data_dir.join("remote")) else {
        return Vec::new();
    };
    dir.flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .filter_map(|path| fs::read_to_string(path.with_extension("json")).ok())
        .filter_map(|json| serde_json::from_str::<Kept>(&json).ok())
        .filter_map(|kept| Remote::parse(&kept.file))
        .collect()
}

// The scripts need `sh`, which Windows doesn't have: there they are only ever sent to a host.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::core::remote::tests::fake_ssh;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// A "host" (a folder here, reached through the stand-in ssh) holding `notes.txt`.
    struct Host {
        dir: tempfile::TempDir,
        ssh: Ssh,
        remote: Remote,
        data: PathBuf,
    }

    impl Host {
        fn with(text: &str) -> Host {
            let dir = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(dir.path()).unwrap();
            fs::create_dir(root.join("host")).unwrap();
            fs::write(root.join("host/notes.txt"), text).unwrap();
            Host {
                ssh: fake_ssh(&root),
                remote: Remote::parse(&format!("web1:{}/host/notes.txt", root.display())).unwrap(),
                data: root.join("data"),
                dir,
            }
        }

        fn file(&self) -> PathBuf {
            PathBuf::from(&self.remote.path)
        }

        fn on_host(&self) -> String {
            fs::read_to_string(self.file()).unwrap()
        }

        /// How the connection fails from now on (see `fake_ssh`); empty for a good one.
        fn connection(&self, mode: &str) {
            fs::write(
                fs::canonicalize(self.dir.path()).unwrap().join("mode"),
                mode,
            )
            .unwrap();
        }

        /// Opens the file as a window would.
        fn open(&self) -> Fetched {
            let fetched = fetch(&self.ssh, &self.data, &self.remote).unwrap();
            begin(&self.data, &fetched);
            fetched
        }

        fn save(&self, text: &str) -> Result<(), String> {
            save(
                &self.ssh,
                &self.data,
                &self.remote,
                text,
                LineEnding::Lf,
                false,
                &[],
            )
        }

        /// The text kept on this computer because it hasn't reached the host.
        fn unsent(&self) -> Option<String> {
            fs::read_to_string(KeptFiles::of(&self.data, &self.remote).unsent).ok()
        }

        /// The names in the host's folder, to see that no temp file is left.
        fn host_files(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.file().parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    #[test]
    fn scripts_survive_any_login_shell() {
        for script in [READ.concat(), SAVE.concat()] {
            assert!(!script.contains('\''), "single quotes end the argument");
        }
    }

    #[test]
    fn a_file_is_read_and_saved_back() {
        let host = Host::with("one\r\ntwo\r\n");
        fs::set_permissions(host.file(), fs::Permissions::from_mode(0o640)).unwrap();
        let fetched = host.open();
        assert_eq!(fetched.text, "one\ntwo\n");
        assert_eq!(fetched.line_ending, LineEnding::CrLf);
        assert_eq!(fetched.read_only_reason, None);
        assert!(!fetched.unsent);
        save(
            &host.ssh,
            &host.data,
            &host.remote,
            "one\ntwo\nthree\n",
            fetched.line_ending,
            true,
            &[],
        )
        .unwrap();
        assert_eq!(host.on_host(), "\u{feff}one\r\ntwo\r\nthree\r\n");
        let mode = fs::metadata(host.file()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640, "the file keeps its permissions");
        assert_eq!(host.host_files(), ["notes.txt"], "no temp file is left");
        assert_eq!(host.unsent(), None, "nothing is kept once the host has it");
        // Saved again and again, each save knowing the one before.
        host.save("four").unwrap();
        host.save("four").unwrap();
        host.save("").unwrap();
        assert_eq!(host.on_host(), "");
        // Leaving the file leaves nothing behind on this computer.
        forget(&host.data, &host.remote, false);
        assert_eq!(fs::read_dir(host.data.join("remote")).unwrap().count(), 0);
    }

    #[test]
    fn a_save_never_replaces_a_change_made_on_the_host() {
        let host = Host::with("mine");
        host.open();
        fs::write(host.file(), "theirs").unwrap();
        assert_eq!(host.save("mine, edited").unwrap_err(), CHANGED);
        assert_eq!(host.on_host(), "theirs");
        assert_eq!(host.host_files(), ["notes.txt"]);
        assert_eq!(host.unsent().as_deref(), Some("mine, edited"));
        // A change that keeps the length is seen too, which a date and a size would miss.
        fs::write(host.file(), "mine").unwrap();
        host.save("mine, edited").unwrap();
        fs::write(host.file(), "mine, EDITED").unwrap();
        assert_eq!(host.save("more").unwrap_err(), CHANGED);
        // Reload takes the host's file as seen; Keep mine replaces whatever is there.
        let reloaded = read(&host.ssh, &host.remote).unwrap();
        begin(&host.data, &reloaded);
        assert_eq!(reloaded.text, "mine, EDITED");
        assert_eq!(host.unsent(), None);
        host.save("reloaded, then edited").unwrap();
        fs::write(host.file(), "theirs again").unwrap();
        assert_eq!(host.save("mine again").unwrap_err(), CHANGED);
        keep_mine(&host.data, &host.remote);
        host.save("mine again").unwrap();
        assert_eq!(host.on_host(), "mine again");
        // And the save after Keep mine checks again.
        fs::write(host.file(), "theirs once more").unwrap();
        assert_eq!(host.save("mine?").unwrap_err(), CHANGED);
    }

    #[test]
    fn a_connection_cut_mid_save_leaves_the_old_file_whole() {
        let host = Host::with("the old text");
        host.open();
        let long = "new text ".repeat(20_000);
        // The script reads its first four lines, then the connection drops partway into the
        // text: at every stage the host keeps the old file.
        for bytes in [0, 10, 300, 500, 5_000, 100_000] {
            host.connection(&format!("cut {bytes}"));
            let failed = host.save(&long).unwrap_err();
            assert_eq!(failed, "Save failed: lost the connection to web1");
            assert_eq!(host.on_host(), "the old text", "cut after {bytes} bytes");
            assert_eq!(host.host_files(), ["notes.txt"], "cut after {bytes} bytes");
            assert_eq!(host.unsent().as_deref(), Some(long.as_str()));
        }
        host.connection("");
        host.save(&long).unwrap();
        assert_eq!(host.on_host(), long);
        assert_eq!(host.unsent(), None);
    }

    #[test]
    fn a_save_whose_answer_was_lost_is_not_taken_for_a_stranger() {
        let host = Host::with("one");
        host.open();
        // The save arrives, but the connection drops before the host's answer gets back.
        host.connection("deaf");
        assert_eq!(
            host.save("two").unwrap_err(),
            "Save failed: lost the connection to web1"
        );
        assert_eq!(host.on_host(), "two");
        assert_eq!(host.save("three").unwrap_err().len(), 40);
        assert_eq!(host.on_host(), "three");
        // The next saves find text this window sent, not someone else's change.
        host.connection("");
        host.save("three").unwrap();
        assert_eq!(host.unsent(), None);
        host.connection("deaf");
        let _ = host.save("four");
        host.connection("");
        host.save("five").unwrap();
        assert_eq!(host.on_host(), "five");
    }

    #[test]
    fn text_that_never_reached_the_host_comes_back() {
        let host = Host::with("one");
        host.open();
        host.connection("down");
        assert_eq!(
            host.save("two").unwrap_err(),
            "Save failed: can't reach web1"
        );
        assert_eq!(host.on_host(), "one");
        assert_eq!(unsent(&host.data), vec![host.remote.clone()]);
        // The app ends; the window's last text is kept without reaching for the host.
        let typed = vec![vec![Change {
            at: 3,
            removed: String::new(),
            inserted: ", and more".to_string(),
        }]];
        let ending = LineEnding::CrLf;
        keep_unsent(
            &host.data,
            &host.remote,
            "two, and more",
            ending,
            false,
            &typed,
        )
        .unwrap();
        // Opened again while the host is still away: the text is there to read and save elsewhere.
        let offline = host.open();
        assert_eq!(offline.text, "two, and more");
        assert_eq!(offline.line_ending, LineEnding::Lf);
        assert!(offline.unsent);
        assert_eq!(offline.history, typed, "Ctrl+Z still works on it");
        assert_eq!(
            offline.notice.as_deref(),
            Some("Showing edits that never reached web1 (can't reach web1)")
        );
        // Opened again with the host back: the text shows, and saving it sends it.
        host.connection("");
        let back = host.open();
        assert!(back.unsent && back.text == "two, and more");
        host.save(&back.text).unwrap();
        assert_eq!(host.on_host(), "two, and more");
        assert!(unsent(&host.data).is_empty());
        assert!(!host.open().unsent);
        assert_eq!(fs::read_dir(host.data.join("remote")).unwrap().count(), 1);
    }

    #[test]
    fn unsent_text_does_not_replace_a_change_made_meanwhile() {
        let host = Host::with("one");
        host.open();
        host.connection("down");
        let _ = host.save("mine");
        host.connection("");
        fs::write(host.file(), "theirs").unwrap();
        let back = host.open();
        assert!(back.unsent && back.text == "mine");
        assert_eq!(host.save("mine").unwrap_err(), CHANGED);
        assert_eq!(host.on_host(), "theirs");
        // Text the host turns out to hold already is no longer unsent.
        fs::write(host.file(), "mine").unwrap();
        assert!(!host.open().unsent);
        assert_eq!(host.unsent(), None);
    }

    #[test]
    fn a_symlink_stays_a_link_and_a_deleted_file_is_written_again() {
        let host = Host::with("target");
        let folder = host.file().parent().unwrap().to_path_buf();
        fs::create_dir(folder.join("deep")).unwrap();
        fs::rename(host.file(), folder.join("deep/real.txt")).unwrap();
        symlink("hop.txt", host.file()).unwrap();
        symlink("deep/real.txt", folder.join("hop.txt")).unwrap();
        assert_eq!(host.open().text, "target");
        host.save("through the links").unwrap();
        assert!(fs::symlink_metadata(host.file()).unwrap().is_symlink());
        assert!(
            fs::symlink_metadata(folder.join("hop.txt"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(folder.join("deep/real.txt")).unwrap(),
            "through the links"
        );
        // Deleted on the host: written again, as a file on this computer is.
        fs::remove_file(folder.join("deep/real.txt")).unwrap();
        host.save("back again").unwrap();
        assert_eq!(host.on_host(), "back again");
        // Its folder gone, or a folder in its place: the save says so and writes nothing.
        fs::remove_dir_all(folder.join("deep")).unwrap();
        assert_eq!(
            host.save("nowhere to go").unwrap_err(),
            "Save failed: the folder is missing on the host"
        );
        fs::create_dir_all(folder.join("deep/real.txt")).unwrap();
        assert_eq!(
            host.save("a folder now").unwrap_err(),
            "Save failed: it isn't a file on the host any more"
        );
        assert_eq!(host.unsent().as_deref(), Some("a folder now"));
    }

    #[test]
    fn what_cannot_be_opened_says_why() {
        let host = Host::with("text");
        let folder = host.file().parent().unwrap().to_path_buf();
        let open = |name: &str| {
            let remote = Remote {
                path: format!("{}/{name}", folder.display()),
                ..host.remote.clone()
            };
            fetch(&host.ssh, &host.data, &remote)
        };
        assert_eq!(
            open("missing.txt").unwrap_err(),
            "Couldn't open: web1 has no such file"
        );
        fs::create_dir(folder.join("sub")).unwrap();
        assert_eq!(open("sub").unwrap_err(), "Couldn't open: that is a folder");
        fs::write(folder.join("program"), b"\x7fELF\0\xff\0").unwrap();
        assert_eq!(
            open("program").unwrap_err(),
            "Couldn't open: binary file, not text"
        );
        // Not UTF-8: shown, read-only.
        fs::write(folder.join("latin1.txt"), b"caf\xe9").unwrap();
        let latin1 = open("latin1.txt").unwrap();
        assert_eq!(latin1.text, "caf\u{fffd}");
        assert_eq!(latin1.read_only_reason.as_deref(), Some(NOT_UTF8));
        // Odd names reach the host as they are.
        let odd = "it's \"odd\" $HOME `id` \\n; rm -rf *.txt";
        fs::write(folder.join(odd), "still here").unwrap();
        assert_eq!(open(odd).unwrap().text, "still here");
        // A download cut short is refused, not shown as the whole file.
        // What the host's login files print first doesn't end up in the text.
        host.connection("noisy");
        assert_eq!(open("notes.txt").unwrap().text, "text");
        // A download cut short is refused, never shown as the whole file.
        fs::write(folder.join("long.txt"), "a long line of text\n".repeat(500)).unwrap();
        for bytes in [0, 10, 40, 5_000, 9_999] {
            host.connection(&format!("short {bytes}"));
            let cut = open("long.txt").unwrap_err();
            assert!(
                cut.ends_with("long.txt didn't arrive whole: try again")
                    || cut == "Couldn't open: lost the connection to web1",
                "{bytes}: {cut}"
            );
        }
        // Whatever shell the host logs in with, the scripts reach `sh` whole.
        for shell in ["bash", "zsh", "fish", "tcsh", "csh", "ksh"] {
            let found = std::process::Command::new(shell)
                .arg("-c")
                .arg("exit 0")
                .status();
            if found.is_ok_and(|status| status.success()) {
                host.connection(&format!("shell {shell}"));
                assert_eq!(open(odd).unwrap().text, "still here", "{shell}");
                assert_eq!(open("long.txt").unwrap().text.len(), 10_000, "{shell}");
            }
        }
        // Too long to open.
        host.connection("");
        let big = fs::File::create(folder.join("big.log")).unwrap();
        big.set_len(MAX_BYTES as u64 + 1).unwrap();
        assert_eq!(
            open("big.log").unwrap_err(),
            "Couldn't open: 33 MB is too big to open over ssh (32 MB at most)"
        );
        // A file the user can't write opens read-only (root can write anything, so not for it).
        fs::set_permissions(host.file(), fs::Permissions::from_mode(0o444)).unwrap();
        // SAFETY: geteuid only reads the process's user id.
        if unsafe { libc::geteuid() } != 0 {
            let read_only = open("notes.txt").unwrap();
            assert_eq!(read_only.read_only_reason.as_deref(), Some(NO_PERMISSION));
            // Nor is one made read-only since it was opened written over.
            let refused = host.save("mine").unwrap_err();
            assert_eq!(refused, "Save failed: no permission to write it on web1");
        }
    }
}
