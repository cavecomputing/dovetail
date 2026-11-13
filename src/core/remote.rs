//! Other machines, reached with the system's own `ssh`, so `~/.ssh/config`, keys, the agent and
//! jump hosts all work as they do in a terminal. Here: how a file on a host is named
//! (`alias:/path`), what the Ctrl+O picker's box means when it names a host, running a script on
//! a host, why that failed in plain words, and what's in a folder there.
//!
//! Every request is one `ssh` run, which starts `sh` on the host with one of the scripts below
//! and talks to it through its input and output. `ssh` runs in batch mode: there is nowhere to
//! type a password, so a host that asks for one fails with the reason instead of hanging. The
//! host needs a POSIX `sh` and, for files, `cksum`: any Linux, macOS or BSD.

use std::ffi::OsString;
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::core::folder::Entry;
use crate::core::paths::Paths;

/// A file on another machine: the host as `ssh` knows it (an alias from `~/.ssh/config`, or
/// `user@host`) and the file's whole path there.
#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub host: String,
    pub path: String,
}

impl Remote {
    /// The file `text` names as `alias:/path`, as scp writes one, or `None` when it is anything
    /// else, such as a path on this computer.
    pub fn parse(text: &str) -> Option<Remote> {
        let (host, path) = text.split_once(':')?;
        let named = path.starts_with('/') && !path.ends_with('/') && !path.contains('\n');
        (is_host(host) && named).then(|| Remote {
            host: host.to_string(),
            path: path.to_string(),
        })
    }

    /// The name the recent list, the saved places and the lock know this file by: `alias:/path`
    /// as a path. No file on this computer has such a path (they are all kept whole, from `/` or
    /// a drive), so the two never meet.
    pub fn as_path(&self) -> PathBuf {
        PathBuf::from(self.to_string())
    }

    /// What the picker's box holds to list this file's folder: `alias:/folder/`.
    pub fn typed_folder(&self) -> String {
        let folder = self.path.rfind('/').map_or("/", |at| &self.path[..=at]);
        format!("{}:{folder}", self.host)
    }
}

impl fmt::Display for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.path)
    }
}

/// Whether `host` can be handed to `ssh` as a host: letters, digits and `.`, `-`, `_`, `@`, not
/// starting with a `-` (which ssh would read as an option). On Windows one letter is a drive
/// (`C:/notes.txt`), not a host.
pub fn is_host(host: &str) -> bool {
    let plain = |c: char| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@');
    let drive = cfg!(windows) && host.len() == 1;
    !host.is_empty() && !host.starts_with('-') && host.chars().all(plain) && !drive
}

/// What the Ctrl+O picker's box names when it isn't a folder on this computer.
#[derive(Debug, Clone, PartialEq)]
pub enum Typed {
    /// `ssh:` and the start of a host's name: the hosts to choose from.
    Hosts(String),
    /// `alias:/folder/name`: a folder on a host and the name typed in it. `alias:` alone is the
    /// host's home folder, with an empty `folder`.
    Folder {
        host: String,
        folder: String,
        name: String,
    },
}

/// The picker's row and prefix that list the hosts.
pub const HOSTS: &str = "ssh:";

/// Reads the picker's box as a host's folder or the host list; `None` when it names a folder on
/// this computer.
pub fn typed(text: &str) -> Option<Typed> {
    if let Some(name) = text.strip_prefix(HOSTS) {
        return (!name.contains('/')).then(|| Typed::Hosts(name.to_string()));
    }
    let (host, path) = text.split_once(':')?;
    if !is_host(host) || !(path.is_empty() || path.starts_with('/')) {
        return None;
    }
    let cut = path.rfind('/').map_or(0, |at| at + 1);
    Some(Typed::Folder {
        host: host.to_string(),
        folder: path[..cut].to_string(),
        name: path[cut..].to_string(),
    })
}

/// How this computer reaches a host: the program to run and what it is always given first.
#[derive(Debug, Clone)]
pub struct Ssh {
    pub program: OsString,
    pub first: Vec<OsString>,
    /// How long a run may go without a byte moving either way before it is given up on. `ssh`
    /// itself drops a dead connection sooner (`OPTIONS`); this is for whatever that misses.
    pub quiet: Duration,
}

/// What every run of `ssh` is told: never ask anything (`BatchMode`), give up on a connect after
/// 10 seconds and on a silent connection after 15, and leave out what a host's config may add
/// for a terminal (forwarded ports, a remote command, a terminal), as scp does.
const OPTIONS: [&str; 11] = [
    "-T",
    "-x",
    "-a",
    "-oBatchMode=yes",
    "-oConnectTimeout=10",
    "-oServerAliveInterval=5",
    "-oServerAliveCountMax=3",
    "-oClearAllForwardings=yes",
    "-oPermitLocalCommand=no",
    "-oRemoteCommand=none",
    "-oRequestTTY=no",
];

/// Starts each line a script answers with, so anything a host's login files print first is
/// told apart from the answer.
const MARK: &str = "@re@";

/// What a run on a host gave back.
pub struct Ran {
    out: Vec<u8>,
    errors: String,
}

impl Ssh {
    /// The system's `ssh`, reading its own config. With the folders moved for a test run
    /// (`Paths::under`), it reads the config there instead, so a test never meets a real host.
    pub fn of(paths: &Paths) -> Ssh {
        let config = paths.ssh_config_override.iter();
        Ssh {
            program: "ssh".into(),
            first: config.flat_map(|file| ["-F".into(), file.into()]).collect(),
            quiet: Duration::from_secs(45),
        }
    }

    /// Runs `script` under `sh` on `host`, with `input` as its input. An error is why the run
    /// couldn't start or was given up on; a run that ended is returned whatever it printed.
    pub fn run(&self, host: &str, script: &str, input: Vec<u8>) -> Result<Ran, String> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.first)
            .args(OPTIONS)
            .arg("--")
            .arg(host)
            // One argument in single quotes, which the scripts never use, so the host's login
            // shell (bash, zsh, fish, csh) hands it to `sh` as it is.
            .arg(format!("sh -c '{}'", one_line(script)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Windows opens a console window for a console program started from one that has
        // none, unless told not to (CREATE_NO_WINDOW). Linux and macOS have no such window.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        let mut child = command.spawn().map_err(|error| match error.kind() {
            ErrorKind::NotFound => "ssh isn't installed on this computer".to_string(),
            _ => format!("couldn't start ssh: {error}"),
        })?;
        let moved = Arc::new(Mutex::new(Instant::now()));
        let (out, errors) = (Pipe::default(), Pipe::default());
        if let Some(stdout) = child.stdout.take() {
            out.fill_from(stdout, moved.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            errors.fill_from(stderr, moved.clone());
        }
        if let Some(mut stdin) = child.stdin.take() {
            let moved = moved.clone();
            thread::spawn(move || {
                for chunk in input.chunks(64 * 1024) {
                    if stdin.write_all(chunk).is_err() {
                        break;
                    }
                    touch(&moved);
                }
                // Dropping `stdin` here closes it, which ends the script's input.
            });
        }
        let since = |moved: &Mutex<Instant>| lock(moved).elapsed();
        while let Ok(None) = child.try_wait() {
            if since(&moved) > self.quiet {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{host} stopped answering"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        // What ssh printed last may still be in its pipes. They end when ssh does, unless a
        // helper it started (a jump host's, a shared connection's) holds them open: then
        // whatever has come once nothing more does.
        while !(out.ended() && errors.ended()) && since(&moved) < Duration::from_millis(300) {
            thread::sleep(Duration::from_millis(5));
        }
        Ok(Ran {
            out: out.take(),
            errors: String::from_utf8_lossy(&errors.take()).into_owned(),
        })
    }
}

/// What one of ssh's pipes has given so far, and whether it has ended.
#[derive(Default, Clone)]
struct Pipe(Arc<Mutex<(Vec<u8>, bool)>>);

impl Pipe {
    /// Reads `from` to its end on a thread of its own, noting in `moved` when bytes last came.
    fn fill_from(&self, mut from: impl Read + Send + 'static, moved: Arc<Mutex<Instant>>) {
        let pipe = self.clone();
        thread::spawn(move || {
            let mut chunk = vec![0; 64 * 1024];
            while let Ok(read @ 1..) = from.read(&mut chunk) {
                lock(&pipe.0).0.extend_from_slice(&chunk[..read]);
                touch(&moved);
            }
            lock(&pipe.0).1 = true;
        });
    }

    fn ended(&self) -> bool {
        lock(&self.0).1
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut lock(&self.0).0)
    }
}

/// `mutex`'s value, also after a thread died holding it: what it guards here is always whole.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn touch(moved: &Mutex<Instant>) {
    *lock(moved) = Instant::now();
}

/// `script` on one line: csh, as a host's login shell, takes no line break inside quotes. The
/// scripts are written so that a space does for every line break.
fn one_line(script: &str) -> String {
    let lines: Vec<&str> = script.lines().map(str::trim).collect();
    lines.join(" ").trim().to_string()
}

impl Ran {
    /// The script's answer on `host`: the words on its `MARK` line and the bytes after that line.
    /// Without one the run failed, and the error says why in plain words.
    pub fn answer(&self, host: &str) -> Result<(Vec<&str>, &[u8]), String> {
        let mut from = 0;
        while let Some(at) = find(&self.out[from..], MARK.as_bytes()) {
            let start = from + at;
            // A line the connection cut short has no end, and is no answer.
            let Some(end) = find(&self.out[start..], b"\n").map(|end| start + end) else {
                break;
            };
            let line = std::str::from_utf8(&self.out[start + MARK.len()..end]);
            if let Ok(line) = line
                && (start == 0 || self.out[start - 1] == b'\n')
            {
                return Ok((line.split_whitespace().collect(), &self.out[end + 1..]));
            }
            from = start + MARK.len();
        }
        Err(why(host, &self.errors))
    }
}

/// Where `needle` first is in `bytes`.
fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|part| part == needle)
}

/// Why a run on `host` failed, in plain words, from what `ssh` or the script's tools wrote as
/// errors (OpenSSH's own wording, the same on Linux, macOS and Windows).
pub fn why(host: &str, errors: &str) -> String {
    let lower = errors.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if says(&["could not resolve hostname"]) {
        format!("can't find the host {host}")
    } else if says(&["host identification has changed"]) {
        format!("{host}'s host key has changed: check it in a terminal")
    } else if says(&["host key verification failed"]) {
        format!("{host} isn't a known host yet: connect to it once in a terminal")
    } else if says(&["permission denied (", "too many authentication failures"]) {
        format!("{host} wants a password, or a key the agent doesn't hold")
    } else if says(&["connect to host", "connection timed out during banner"]) {
        format!("can't reach {host}")
    } else if says(&[
        "connection closed",
        "connection reset",
        "broken pipe",
        "not responding",
        "connection timed out",
    ]) {
        format!("lost the connection to {host}")
    } else if says(&["no space left"]) {
        "the host's disk is full".to_string()
    } else if says(&["quota"]) {
        "over the disk quota on the host".to_string()
    } else if says(&["read-only file system"]) {
        "the host's disk is read-only".to_string()
    } else if says(&["permission denied", "operation not permitted"]) {
        "no permission on the host".to_string()
    } else if says(&["no such file or directory", "nonexistent"]) {
        "the folder is missing on the host".to_string()
    } else {
        let last = errors.lines().rev().find(|line| !line.trim().is_empty());
        last.map_or_else(
            || format!("{host} ended the connection without saying why"),
            |line| line.trim().to_string(),
        )
    }
}

/// POSIX `cksum`'s answer for `bytes`, as the host's own `cksum` prints it: the CRC and the
/// length. It tells a file's contents apart from any other's, which its date and size can't, and
/// every Unix has the tool.
pub fn cksum(bytes: &[u8]) -> String {
    let step = |crc: u32, byte: u8| (crc << 8) ^ CRC_TABLE[usize::from((crc >> 24) as u8 ^ byte)];
    let mut crc = bytes.iter().fold(0, |crc, &byte| step(crc, byte));
    // The length goes in after the bytes, lowest byte first, for as many bytes as it needs.
    let mut length = bytes.len();
    while length != 0 {
        crc = step(crc, length as u8);
        length >>= 8;
    }
    format!("{} {}", !crc, bytes.len())
}

/// The CRC of each byte value, for `cksum`: polynomial 0x04C11DB7, highest bit first.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = (byte as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = match crc & 0x8000_0000 {
                0 => crc << 1,
                _ => (crc << 1) ^ 0x04C1_1DB7,
            };
            bit += 1;
        }
        table[byte] = crc;
        byte += 1;
    }
    table
};

/// Moves the symlink `$f` on to the file it points at, link by link, so a save replaces that file
/// and the link stays a link.
pub const FOLLOW_LINKS: &str = r#"
    n=0; while [ -L "$f" ] && [ "$n" -lt 40 ]; do
        l=$(readlink -- "$f") || exit 1;
        case "$l" in /*) f=$l;; *) f=$(dirname -- "$f")/$l;; esac;
        n=$((n+1));
    done;
"#;

/// Lists the folder named on the input's first line (the home folder for an empty line): its
/// real path, then each entry, a `/` after a folder's, with a NUL after each so any name works.
const LIST: &str = r#"
    IFS= read -r d; if [ -z "$d" ]; then d=$HOME; fi;
    cd -- "$d" 2>/dev/null || {
        if [ -d "$d" ]; then echo "@re@ denied"; else echo "@re@ missing"; fi; exit 0;
    };
    if [ -r . ]; then :; else echo "@re@ denied"; exit 0; fi;
    printf "@re@ ok\n%s\0" "$(pwd)";
    for f in .* *; do
        case "$f" in .|..) continue;; esac;
        if [ -d "$f" ]; then printf "%s/\0" "$f";
        elif [ -e "$f" ] || [ -L "$f" ]; then printf "%s\0" "$f"; fi;
    done
"#;

/// A folder on a host, as the picker shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub host: String,
    /// The folder as it was asked for (empty for the home folder), and its whole path there.
    pub asked: String,
    pub folder: String,
    /// Folders first, then by name ignoring case, as `folder::entries` orders them.
    pub entries: Vec<Entry>,
}

/// What's in `folder` on `host` (its home folder when `folder` is empty), or why it can't be
/// listed. Names that aren't UTF-8 are left out: they couldn't be asked for again.
pub fn list(ssh: &Ssh, host: &str, folder: &str) -> Result<Listing, String> {
    let ran = ssh.run(host, LIST, format!("{folder}\n").into_bytes())?;
    let (words, listed) = ran.answer(host)?;
    match words.first() {
        Some(&"ok") => {}
        Some(&"denied") => return Err(format!("no permission to look in {folder}")),
        _ => return Err(format!("{host} has no folder {folder}")),
    }
    let mut names = listed
        .split(|&byte| byte == 0)
        .filter_map(|name| std::str::from_utf8(name).ok())
        .filter(|name| !name.is_empty());
    // `pwd` may write the top folder as `//`.
    let found = names.next().unwrap_or("/").trim_start_matches('/');
    let found = format!("/{found}");
    let mut entries: Vec<Entry> = names
        .map(|name| {
            let plain = name.trim_end_matches('/');
            Entry {
                path: PathBuf::from(format!("{}/{plain}", found.trim_end_matches('/'))),
                is_dir: name.ends_with('/'),
            }
        })
        .collect();
    entries.sort_by_key(|entry| (!entry.is_dir, entry.name().to_lowercase()));
    Ok(Listing {
        host: host.to_string(),
        asked: folder.to_string(),
        folder: found,
        entries,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A stand-in for `ssh` that runs the script on this computer, as `sh` would on a host, so
    /// the scripts and what is made of their answers are tested without a server. A file named
    /// `mode` in `dir` makes it fail the way a connection does: `down` (no connection), `cut N`
    /// (the connection drops after N bytes of input), `short N` (it drops after N bytes of the
    /// answer), `deaf` (the script runs, but its answer never arrives), `silent` (connected, and
    /// then nothing). `noisy` is a host whose login files print something first, and `shell X` one
    /// whose login shell is the program X. It needs `sh`, which Windows doesn't have: there the
    /// scripts are only ever sent to a host, and the tests that run them are left out.
    #[cfg(unix)]
    pub fn fake_ssh(dir: &std::path::Path) -> Ssh {
        let script = dir.join("fake-ssh.sh");
        let mode = dir.join("mode");
        std::fs::write(
            &script,
            format!(
                r#"for command; do :; done
                read -r mode count < "{mode}" 2>/dev/null
                case "$mode" in
                    down) echo "ssh: connect to host web1 port 22: Connection refused" >&2; exit 255;;
                    cut) head -c "$count" | sh -c "$command" >/dev/null; echo "Connection reset by peer" >&2; exit 255;;
                    deaf) sh -c "$command" >/dev/null; echo "client_loop: send disconnect: Broken pipe" >&2; exit 255;;
                    short) sh -c "$command" | head -c "$count"; echo "Connection reset by peer" >&2; exit 255;;
                    shell) exec "$count" -c "$command";;
                    silent) sleep 30;;
                    noisy) echo "Welcome to web1, {MARK} of the fleet"; exec sh -c "$command";;
                    *) exec sh -c "$command";;
                esac"#,
                mode = mode.display()
            ),
        )
        .unwrap();
        Ssh {
            program: "sh".into(),
            first: vec![script.into()],
            quiet: Duration::from_millis(400),
        }
    }

    #[test]
    fn a_remote_file_is_named_like_scp_names_one() {
        let remote = Remote::parse("web1:/etc/nginx/nginx.conf").unwrap();
        assert_eq!(remote.host, "web1");
        assert_eq!(remote.path, "/etc/nginx/nginx.conf");
        assert_eq!(remote.to_string(), "web1:/etc/nginx/nginx.conf");
        assert_eq!(remote.typed_folder(), "web1:/etc/nginx/");
        assert_eq!(Remote::parse("me@10.0.0.2:/x").unwrap().host, "me@10.0.0.2");
        // Paths on this computer, folders, and anything ssh would read as an option.
        for local in [
            "/home/me/a:/b",
            "~/notes.txt",
            "notes.txt",
            "web1:/etc/",
            "web1:notes",
            "-oProxyCommand=x:/y",
            "a b:/c",
            ":/c",
        ] {
            assert_eq!(Remote::parse(local), None, "{local}");
        }
    }

    #[test]
    fn the_picker_box_names_hosts_and_their_folders() {
        let folder = |host: &str, folder: &str, name: &str| {
            Some(Typed::Folder {
                host: host.to_string(),
                folder: folder.to_string(),
                name: name.to_string(),
            })
        };
        assert_eq!(typed("ssh:"), Some(Typed::Hosts(String::new())));
        assert_eq!(typed("ssh:we"), Some(Typed::Hosts("we".to_string())));
        assert_eq!(typed("web1:"), folder("web1", "", ""));
        assert_eq!(typed("web1:/"), folder("web1", "/", ""));
        assert_eq!(typed("web1:/etc/ng"), folder("web1", "/etc/", "ng"));
        for local in ["~/", "/etc/", "notes", "web1:notes", "a/b:/c"] {
            assert_eq!(typed(local), None, "{local}");
        }
    }

    #[test]
    fn cksum_matches_the_posix_tool() {
        // From `printf ... | cksum`.
        assert_eq!(cksum(b""), "4294967295 0");
        assert_eq!(cksum(b"123456789"), "930766865 9");
        assert_eq!(cksum(b"hello\n"), "3015617425 6");
        let long: Vec<u8> = (0..70_000u32).map(|n| (n % 251) as u8).collect();
        assert_eq!(cksum(&long), "2458292535 70000");
    }

    #[test]
    fn failures_are_said_in_plain_words() {
        let said = |errors: &str| why("web1", errors);
        assert_eq!(
            said("ssh: connect to host 10.0.0.9 port 22: Connection timed out"),
            "can't reach web1"
        );
        assert_eq!(
            said("ssh: Could not resolve hostname web1: Name or service not known"),
            "can't find the host web1"
        );
        assert_eq!(
            said("me@web1: Permission denied (publickey,password)."),
            "web1 wants a password, or a key the agent doesn't hold"
        );
        assert_eq!(
            said("Host key verification failed."),
            "web1 isn't a known host yet: connect to it once in a terminal"
        );
        assert_eq!(
            said(
                "@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\nHost key verification failed."
            ),
            "web1's host key has changed: check it in a terminal"
        );
        assert_eq!(
            said("Timeout, server web1 not responding."),
            "lost the connection to web1"
        );
        assert_eq!(
            said("cat: write error: No space left on device"),
            "the host's disk is full"
        );
        assert_eq!(
            said("sh: 1: cannot create /etc/.x.tmp: Permission denied"),
            "no permission on the host"
        );
        assert_eq!(said("something odd\n\n"), "something odd");
        assert_eq!(said(""), "web1 ended the connection without saying why");
    }

    #[test]
    fn scripts_survive_any_login_shell() {
        for script in [LIST, FOLLOW_LINKS] {
            let line = one_line(script);
            // The whole script travels inside single quotes, on one line.
            assert!(!line.contains('\'') && !line.contains('\n'), "{line}");
        }
    }

    #[cfg(unix)] // The stand-in for ssh needs `sh` (see `fake_ssh`).
    #[test]
    fn a_folder_on_a_host_is_listed_like_one_here() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = fake_ssh(dir.path());
        let root = dir.path().join("root");
        for folder in ["src", "Docs", "a b"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for file in ["zeta.txt", "Alpha.md", ".env", "it's \"odd\" $HOME.txt"] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        std::os::unix::fs::symlink(root.join("src"), root.join("link")).unwrap();
        std::os::unix::fs::symlink("nowhere", root.join("dangling")).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let asked = format!("{}/src/..", root.display());
        let listing = list(&ssh, "web1", &asked).unwrap();
        assert_eq!(listing.folder, root.to_str().unwrap());
        assert_eq!(listing.asked, asked);
        let names: Vec<String> = listing
            .entries
            .iter()
            .map(|entry| match entry.is_dir {
                true => format!("{}/", entry.name()),
                false => entry.name(),
            })
            .collect();
        assert_eq!(
            names,
            [
                "a b/",
                "Docs/",
                "link/",
                "src/",
                ".env",
                "Alpha.md",
                "dangling",
                "it's \"odd\" $HOME.txt",
                "zeta.txt"
            ]
        );
        assert_eq!(listing.entries[0].path, root.join("a b"));
        // An empty folder, the home folder, and a folder that isn't there.
        let empty = list(&ssh, "web1", &format!("{}/Docs", root.display())).unwrap();
        assert!(empty.entries.is_empty());
        assert!(list(&ssh, "web1", "").unwrap().folder.starts_with('/'));
        assert_eq!(list(&ssh, "web1", "/").unwrap().folder, "/");
        assert_eq!(
            list(&ssh, "web1", "/no/such").unwrap_err(),
            "web1 has no folder /no/such"
        );
    }

    #[cfg(unix)] // The stand-in for ssh needs `sh` (see `fake_ssh`).
    #[test]
    fn a_dead_connection_fails_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = fake_ssh(dir.path());
        std::fs::write(dir.path().join("mode"), "down").unwrap();
        assert_eq!(list(&ssh, "web1", "/").unwrap_err(), "can't reach web1");
        // Connected, then nothing: given up on, not waited for without end.
        std::fs::write(dir.path().join("mode"), "silent").unwrap();
        let started = Instant::now();
        assert_eq!(
            list(&ssh, "web1", "/").unwrap_err(),
            "web1 stopped answering"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        // What a host's login files print first isn't taken for the answer.
        std::fs::write(dir.path().join("mode"), "noisy").unwrap();
        assert_eq!(list(&ssh, "web1", "/").unwrap().folder, "/");
        let missing = Ssh {
            program: "no-such-ssh-program".into(),
            ..ssh
        };
        assert_eq!(
            list(&missing, "web1", "/").unwrap_err(),
            "ssh isn't installed on this computer"
        );
    }
}
