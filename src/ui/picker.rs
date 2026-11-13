//! The Ctrl+O picker: a box holding a path, starting at the open file's folder, and what's in
//! that folder below it. Typing filters the folder by name; Tab, Enter or a click goes into the
//! highlighted folder (`..` is the one above), and Enter or a click opens the highlighted file in
//! this window. → goes into the highlighted folder and ← up to the one above, so the arrows alone
//! get around.
//!
//! With hosts in the SSH config, a folder's rows start with `ssh:`, which lists them. Choosing
//! one lists its home folder, and from there a host's folders work like the ones here, with
//! `alias:/path/` in the box. Each folder there is asked for over ssh, off the UI thread, so
//! its rows come a moment later.

use std::path::{Path, PathBuf};

use iced::widget::operation::{focus, move_cursor_to_end};
use iced::widget::{column, text, text_input};
use iced::{Element, Task, Theme};

use crate::core::folder::{Entry, complete, picker_rows, rows_of, split_typed, typed_folder};
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::recent::match_score;
use crate::core::remote::{self, HOSTS, Listing, Remote, Ssh, Typed};
use crate::core::ssh::host_aliases;
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{LINE, SIZE};
use crate::ui::editor;
use crate::ui::remote_actions::off_thread;
use crate::ui::status_bar::keys;
use crate::ui::switcher::{comment, list_row, panel, prompt};
use crate::ui::theme::Roles;

pub const PICKER_ID: &str = "picker";
/// The most rows shown; typing narrows a bigger folder.
const MAX_ROWS: usize = 14;

pub struct Picker {
    pub typed: String,
    /// What's in the folder `typed` lists (see `picker_rows`), or the hosts after `ssh:`.
    rows: Vec<Entry>,
    selected: usize,
    /// The hosts in the SSH config, read when the picker opens.
    hosts: Vec<String>,
    /// The folder on a host listed last, which typing a name filters without asking again.
    listing: Option<Listing>,
    /// The host and folder the box names that isn't in hand, for `App::ask_for_listing`.
    wanted: Option<(String, String)>,
    /// The host and folder asked for last, on its way or failed, so it isn't asked for again
    /// at every key.
    asked_for: Option<(String, String)>,
    /// Counts the folders asked for, so the answer for an earlier one is dropped.
    asked: u64,
    /// Said under the box in place of rows: a folder is on its way, or why it didn't come.
    status: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PickerAction {
    Typed(String),
    /// Tab: into the highlighted folder, or the highlighted file's name completed.
    Complete,
    /// Enter: into the highlighted folder, or opens the highlighted file.
    Choose,
    /// A row clicked: as Enter on it.
    Pick(usize),
    /// →: into the highlighted folder.
    Into,
    /// ←: up to the folder above the one listed.
    Up,
    /// A host's folder arrived, or why it couldn't: the answer to the request with this number.
    Listed(u64, Result<Listing, String>),
}

impl Picker {
    fn new(typed: String, hosts: Vec<String>) -> Picker {
        let mut picker = Picker {
            typed: String::new(),
            rows: Vec::new(),
            selected: 0,
            hosts,
            listing: None,
            wanted: None,
            asked_for: None,
            asked: 0,
            status: None,
        };
        picker.set_typed(typed);
        picker
    }

    fn set_typed(&mut self, typed: String) {
        let waiting = self.status.take();
        self.wanted = None;
        self.rows = match remote::typed(&typed) {
            None => {
                let (folder, name) = split_typed(&typed, &home());
                let mut rows = picker_rows(&folder, &name);
                if name.is_empty() && !self.hosts.is_empty() {
                    rows.insert(0, named_row(HOSTS));
                }
                rows
            }
            Some(Typed::Hosts(name)) => {
                let hosts = self.hosts.iter();
                let mut rows: Vec<Entry> = hosts
                    .filter(|host| match_score(&name, host).is_some())
                    .map(|host| named_row(&format!("{host}:")))
                    .collect();
                if name.is_empty() {
                    rows.insert(0, named_row(".."));
                }
                rows
            }
            Some(Typed::Folder { host, folder, name }) => match &self.listing {
                Some(listing) if listing.host == host && with_slash(&listing.folder) == folder => {
                    rows_of(listing.entries.clone(), &name, Some(Path::new(&folder)))
                }
                // Not the folder in hand: `App::ask_for_listing` asks its host for it, unless
                // it is already on its way or just failed (what is said then stays).
                _ => {
                    let asking = format!("# asking {host}…");
                    let folder = (host, folder);
                    self.status = match self.asked_for.as_ref() == Some(&folder) {
                        true => waiting.or(Some(asking)),
                        false => {
                            self.wanted = Some(folder);
                            Some(asking)
                        }
                    };
                    Vec::new()
                }
            },
        };
        self.typed = typed;
        // The first entry is highlighted, not `..` or `ssh:`, so Enter doesn't leave the folder
        // by surprise.
        let entry = |row: &Entry| !row.is_up() && row.path != Path::new(HOSTS);
        self.selected = self.rows.iter().position(entry).unwrap_or(0);
    }

    /// What the box holds once `entry` is chosen with Tab, Enter or →: its folder gone into,
    /// or its name completed.
    fn completed(&self, entry: &Entry) -> String {
        match remote::typed(&self.typed) {
            None if entry.path == Path::new(HOSTS) => HOSTS.to_string(),
            None => complete(&self.typed, entry),
            // Back from the hosts to the folders here; or into a host's home folder.
            Some(Typed::Hosts(_)) if entry.is_up() => typed_folder(display_path(&home())),
            Some(Typed::Hosts(_)) => entry.name(),
            Some(Typed::Folder { host, folder, .. }) => {
                let folder = folder.trim_end_matches('/');
                match (entry.is_up(), folder.rfind('/')) {
                    // Above a host's top folder are the hosts.
                    (true, None) => HOSTS.to_string(),
                    (true, Some(cut)) => format!("{host}:{}", &folder[..=cut]),
                    (false, _) if entry.is_dir => format!("{host}:{folder}/{}/", entry.name()),
                    (false, _) => format!("{host}:{folder}/{}", entry.name()),
                }
            }
        }
    }

    pub fn move_selection(&mut self, step: i32) {
        let last = self.rows.len().min(MAX_ROWS).saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(step as isize).min(last);
    }
}

impl App {
    /// Ctrl+O: opens the picker at the open file's folder (the home folder in scratch), or
    /// closes it.
    pub fn open_picker(&mut self) -> Task<Message> {
        let was_open = self.picker.is_some();
        self.close_panels();
        if was_open {
            return focus(editor::EDITOR_ID);
        }
        let start = match &self.doc {
            Doc::File { path, .. } => path.parent().map(display_path),
            Doc::Remote { remote, .. } => Some(remote.typed_folder()),
            Doc::Scratch => None,
        };
        let hosts = host_aliases(&self.paths.ssh_config());
        let typed = typed_folder(start.unwrap_or("~/".into()));
        self.picker = Some(Picker::new(typed, hosts));
        let asked = self.ask_for_listing();
        Task::batch([focus(PICKER_ID), move_cursor_to_end(PICKER_ID), asked])
    }

    /// Handles the picker's actions, then asks a host for the folder the box has come to name.
    pub fn update_picker(&mut self, action: PickerAction) -> Task<Message> {
        let updated = self.picker_action(action);
        Task::batch([updated, self.ask_for_listing()])
    }

    /// Asks the host for the folder the picker wants (`Picker::wanted`), off the UI thread;
    /// `Listed` brings it back.
    fn ask_for_listing(&mut self) -> Task<Message> {
        let ssh = Ssh::of(&self.paths);
        let Some(picker) = &mut self.picker else {
            return Task::none();
        };
        let Some((host, folder)) = picker.wanted.take() else {
            return Task::none();
        };
        picker.asked_for = Some((host.clone(), folder.clone()));
        picker.asked += 1;
        let asked = picker.asked;
        // A host's top folder is `/`; any other is asked for without the `/` after it.
        let folder = match folder.as_str() {
            "/" => folder,
            _ => folder.trim_end_matches('/').to_string(),
        };
        off_thread(move || remote::list(&ssh, &host, &folder))
            .map(move |listed| Message::Picker(PickerAction::Listed(asked, listed)))
    }

    fn picker_action(&mut self, action: PickerAction) -> Task<Message> {
        let Some(picker) = &mut self.picker else {
            return Task::none();
        };
        match action {
            PickerAction::Typed(typed) => picker.set_typed(typed),
            // For a request since overtaken by another.
            PickerAction::Listed(asked, _) if asked != picker.asked => {}
            PickerAction::Listed(_, listed) => {
                match (listed, remote::typed(&picker.typed)) {
                    // Only if the box still names the folder that was asked for.
                    (Ok(listing), Some(Typed::Folder { host, folder, name }))
                        if picker.asked_for == Some((host.clone(), folder.clone())) =>
                    {
                        // The box takes the folder's whole path, which a home folder's is not.
                        let typed = format!("{host}:{}{name}", with_slash(&listing.folder));
                        picker.listing = Some(listing);
                        picker.asked_for = None;
                        picker.set_typed(typed);
                        return move_cursor_to_end(PICKER_ID);
                    }
                    (Err(reason), _) => {
                        picker.status = Some(format!("# {reason} · Enter asks again"))
                    }
                    _ => {}
                }
            }
            PickerAction::Complete => {
                if let Some(entry) = picker.rows.get(picker.selected).cloned() {
                    picker.set_typed(picker.completed(&entry));
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Into => {
                if picker
                    .rows
                    .get(picker.selected)
                    .is_some_and(|row| row.is_dir)
                {
                    return self.update_picker(PickerAction::Complete);
                }
            }
            // On a host, or among the hosts: up is what the `..` row does.
            PickerAction::Up if remote::typed(&picker.typed).is_some() => {
                picker.set_typed(picker.completed(&named_row("..")));
                return move_cursor_to_end(PICKER_ID);
            }
            PickerAction::Up => {
                let (folder, _) = split_typed(&picker.typed, &home());
                if let Some(above) = folder.parent() {
                    picker.set_typed(typed_folder(display_path(above)));
                    // The folder just left is highlighted, so → goes back into it.
                    if let Some(at) = picker.rows.iter().position(|row| row.path == folder) {
                        picker.selected = at;
                    }
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Pick(index) => {
                picker.selected = index;
                let chosen = self.update_picker(PickerAction::Choose);
                // A folder keeps the picker open, and the click took the box's focus. A file
                // closed it and focused the text, which focusing the gone box would undo.
                return match self.picker {
                    Some(_) => Task::batch([chosen, focus(PICKER_ID)]),
                    None => chosen,
                };
            }
            PickerAction::Choose => match picker.rows.get(picker.selected) {
                Some(row) if row.is_dir => return self.update_picker(PickerAction::Complete),
                Some(row) => {
                    // A file on a host is opened by its name there, `alias:/path`.
                    let path = match Remote::parse(&picker.completed(row)) {
                        Some(remote) => remote.as_path(),
                        None => row.path.clone(),
                    };
                    self.picker = None;
                    let open = self.update(Message::OpenPath(path));
                    return Task::batch([open, focus(editor::EDITOR_ID)]);
                }
                // No rows because a host's folder didn't come: Enter asks for it again.
                None => {
                    picker.asked_for = None;
                    picker.set_typed(picker.typed.clone());
                }
            },
        }
        Task::none()
    }
}

/// A row that isn't a folder's entry: `ssh:`, a host (`alias:`), or `..` above the hosts.
fn named_row(name: &str) -> Entry {
    Entry {
        path: PathBuf::from(name),
        is_dir: true,
    }
}

/// `folder` ending in a `/`, as the box writes a folder on a host.
fn with_slash(folder: &str) -> String {
    format!("{}/", folder.trim_end_matches('/'))
}

/// The home folder, which `~` and a relative path in the box start from.
fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().to_path_buf())
        .unwrap_or_default()
}

/// The picker panel near the top of the window: what it does, the `>` prompt holding the path,
/// then the folder's entries, folders in blue with a `/` after them.
pub fn view(picker: &Picker) -> Element<'_, Message> {
    let search = text_input("~/", &picker.typed)
        .id(PICKER_ID)
        .on_input(|typed| Message::Picker(PickerAction::Typed(typed)))
        .on_submit(Message::Picker(PickerAction::Choose));
    let mut rows = column![];
    for (index, entry) in picker.rows.iter().enumerate().take(MAX_ROWS) {
        let selected = index == picker.selected;
        let (label, folder) = match entry.is_dir {
            // `ssh:` and a host's `alias:` end as the box writes them.
            true if entry.name().ends_with(':') => (entry.name(), true),
            true => (format!("{}/", entry.name()), true),
            false => (entry.name(), false),
        };
        // The highlight's colour wins over a folder's blue.
        let name = text(label)
            .size(SIZE)
            .line_height(LINE)
            .style(move |theme: &Theme| text::Style {
                color: (folder && !selected).then_some(Roles::of(theme).accent_cool),
            });
        rows = rows.push(list_row(
            name,
            selected,
            Message::Picker(PickerAction::Pick(index)),
        ));
    }
    let foot = match (&picker.status, picker.rows.len()) {
        (Some(status), _) => status.clone(),
        (None, 0) => "# nothing here matches".to_string(),
        (None, shown) => {
            let more = match shown.saturating_sub(MAX_ROWS) {
                0 => String::new(),
                more => format!("{more} more · "),
            };
            format!("# {more}→ goes in · ← goes up · {}", keys("Ctrl+O closes"))
        }
    };
    panel(column![prompt(search), rows, comment(foot)])
}

// The stand-in for ssh is a shell script, so this runs where there is an `sh`: not on Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::core::paths::Paths;
    use crate::core::remote::tests::fake_ssh;
    use std::fs;

    fn names(app: &App) -> Vec<String> {
        let picker = app.picker.as_ref().unwrap();
        picker.rows.iter().map(Entry::name).collect()
    }

    fn picker(app: &App) -> &Picker {
        app.picker.as_ref().unwrap()
    }

    #[test]
    fn hosts_come_after_ssh_and_their_folders_work_like_the_ones_here() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("host/docs")).unwrap();
        fs::write(root.join("host/a.txt"), "a\n").unwrap();
        fs::create_dir_all(root.join("ssh")).unwrap();
        fs::write(root.join("ssh/config"), "Host web1 *.lan\nHost db1\n").unwrap();
        let ssh = fake_ssh(&root);
        let (mut app, _) = App::with_paths(Doc::Scratch, Paths::under(&root));
        let typed = |app: &mut App, typed: String| {
            let _ = app.update_picker(PickerAction::Typed(typed));
        };

        let _ = app.open_picker();
        assert_eq!(names(&app)[0], "ssh:", "first in a folder here");
        typed(&mut app, "ssh:".to_string());
        assert_eq!(names(&app), ["..", "web1:", "db1:"]);
        typed(&mut app, "ssh:d".to_string());
        assert_eq!(names(&app), ["db1:"]);
        typed(&mut app, "ssh:".to_string());

        // Enter on a host asks for its home folder, whose whole path the box takes.
        let _ = app.update_picker(PickerAction::Choose);
        assert_eq!(picker(&app).typed, "web1:");
        assert_eq!(picker(&app).status.as_deref(), Some("# asking web1…"));
        let home = Listing {
            host: "web1".to_string(),
            asked: String::new(),
            folder: "/home/me".to_string(),
            entries: Vec::new(),
        };
        let _ = app.update_picker(PickerAction::Listed(1, Ok(home.clone())));
        assert_eq!(picker(&app).typed, "web1:/home/me/");
        assert_eq!(
            (names(&app), picker(&app).asked),
            (vec!["..".to_string()], 1)
        );

        // A folder typed in is asked for once, however much is typed after it; the answer for
        // one since left is dropped.
        let host = format!("web1:{}/host/", root.display());
        typed(&mut app, host.clone());
        typed(&mut app, format!("{host}a"));
        typed(&mut app, host.clone());
        assert_eq!(picker(&app).asked, 2);
        let _ = app.update_picker(PickerAction::Listed(1, Ok(home)));
        assert_eq!(picker(&app).typed, host);
        let listed = remote::list(&ssh, "web1", &format!("{}/host", root.display()));
        let _ = app.update_picker(PickerAction::Listed(2, listed));
        assert_eq!(names(&app), ["..", "docs", "a.txt"]);
        // Typing narrows it without asking again; → goes into a folder, which is asked for.
        typed(&mut app, format!("{host}a"));
        assert_eq!(
            (names(&app), picker(&app).asked),
            (vec!["a.txt".to_string()], 2)
        );
        typed(&mut app, host.clone());
        let _ = app.update_picker(PickerAction::Into);
        assert_eq!(picker(&app).typed, format!("{host}docs/"));
        assert_eq!(picker(&app).asked, 3);

        // A folder that doesn't come says why, and Enter asks again.
        let away = Err("can't reach web1".to_string());
        let _ = app.update_picker(PickerAction::Listed(3, away));
        let status = picker(&app).status.clone().unwrap();
        assert_eq!(status, "# can't reach web1 · Enter asks again");
        let _ = app.update_picker(PickerAction::Choose);
        assert_eq!(picker(&app).asked, 4);

        // ← goes up, and above a host's top folder are the hosts.
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, host);
        typed(&mut app, "web1:/".to_string());
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, "ssh:");

        // Enter on a file opens it from its host.
        typed(&mut app, host.clone());
        let listed = remote::list(&ssh, "web1", &format!("{}/host", root.display()));
        let asked = picker(&app).asked;
        let _ = app.update_picker(PickerAction::Listed(asked, listed));
        typed(&mut app, format!("{host}a"));
        let _ = app.update_picker(PickerAction::Choose);
        assert!(app.picker.is_none());
        let opening = format!("Opening web1:{}/host/a.txt…", root.display());
        assert_eq!(app.fetching, Some(opening));
    }
}
