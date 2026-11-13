//! The Ctrl+O picker: a box holding a path, starting at the open file's folder, and what's in
//! that folder below it. Typing filters the folder by name; Tab, Enter or a click goes into the
//! highlighted folder (`..` is the one above), and Enter or a click opens the highlighted file in
//! this window. → goes into the highlighted folder and ← up to the one above, so the arrows alone
//! get around.
//!
//! A host and a colon at the start of the box (`alias:`, as rclone writes a remote) is a folder
//! on that SSH host: its home folder, or the path after the colon. From there a host's folders
//! work like the ones here. Each is asked for over ssh, off the UI thread, so its rows come a
//! moment later. Without the colon, the box names a folder on this computer.

use std::path::{Path, PathBuf};

use iced::widget::operation::{focus, move_cursor_to_end};
use iced::widget::{column, text, text_input};
use iced::{Element, Task, Theme};

use crate::core::folder::{Entry, complete, picker_rows, rows_of, split_typed, typed_folder};
use crate::core::opened::Doc;
use crate::core::paths::display_path;
use crate::core::remote::{self, Listing, Remote, Ssh, Typed};
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{LINE, SIZE};
use crate::ui::editor;
use crate::ui::remote_actions::off_thread;
use crate::ui::status_bar::keys;
use crate::ui::switcher::{comment, list_row, panel, prompt, warning};
use crate::ui::theme::Roles;

pub const PICKER_ID: &str = "picker";
/// The most rows shown at once: the list scrolls with the highlight through a bigger folder.
const MAX_ROWS: usize = 14;

pub struct Picker {
    pub typed: String,
    /// What's in the folder `typed` lists (see `picker_rows`).
    rows: Vec<Entry>,
    selected: usize,
    /// The first row shown (see `select`).
    top: usize,
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
    /// Whether `status` says why a folder didn't come, which is drawn as a warning.
    failed: bool,
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
    fn new(typed: String) -> Picker {
        let mut picker = Picker {
            typed: String::new(),
            rows: Vec::new(),
            selected: 0,
            top: 0,
            listing: None,
            wanted: None,
            asked_for: None,
            asked: 0,
            status: None,
            failed: false,
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
                picker_rows(&folder, &name)
            }
            Some(Typed { host, folder, name }) => match &self.listing {
                Some(listing) if listing.host == host && with_slash(&listing.folder) == folder => {
                    // `/` has no folder above it, there as here.
                    let above = Path::new(&folder).parent().map(|_| Path::new(&folder));
                    rows_of(listing.entries.clone(), &name, above)
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
                            self.failed = false;
                            Some(asking)
                        }
                    };
                    Vec::new()
                }
            },
        };
        self.typed = typed;
        // The first entry is highlighted, not `..`, so Enter doesn't leave the folder by
        // surprise.
        let entry = |row: &Entry| !row.is_up();
        self.top = 0;
        self.select(self.rows.iter().position(entry).unwrap_or(0));
    }

    /// Highlights row `index`, scrolling the list just far enough to show it.
    fn select(&mut self, index: usize) {
        self.selected = index;
        let lowest_top = (index + 1).saturating_sub(MAX_ROWS);
        self.top = self.top.clamp(lowest_top, index);
    }

    /// What the box holds once `entry` is chosen with Tab, Enter or →: its folder gone into,
    /// or its name completed.
    fn completed(&self, entry: &Entry) -> String {
        match remote::typed(&self.typed) {
            // A name here that reads as a host and a colon gets `./` first, as in rclone.
            None => match complete(&self.typed, entry) {
                local if remote::typed(&local).is_some() => format!("./{local}"),
                local => local,
            },
            Some(on_host) if entry.is_up() => above_on_host(&on_host),
            Some(Typed { host, folder, .. }) if entry.is_dir => {
                format!("{host}:{folder}{}/", entry.name())
            }
            Some(Typed { host, folder, .. }) => format!("{host}:{folder}{}", entry.name()),
        }
    }

    pub fn move_selection(&mut self, step: i32) {
        let last = self.rows.len().saturating_sub(1);
        self.select(self.selected.saturating_add_signed(step as isize).min(last));
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
        let typed = typed_folder(start.unwrap_or("~/".into()));
        self.picker = Some(Picker::new(typed));
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
        // A host's top folder is `/`; any other is asked for without the `/` after it, and its
        // home folder by an empty path.
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
                    (Ok(listing), Some(Typed { host, folder, name }))
                        if picker.asked_for == Some((host.clone(), folder.clone())) =>
                    {
                        // The box takes the folder's whole path, which one named from the
                        // home folder is not.
                        let typed = format!("{host}:{}{name}", with_slash(&listing.folder));
                        picker.listing = Some(listing);
                        picker.asked_for = None;
                        picker.set_typed(typed);
                        return move_cursor_to_end(PICKER_ID);
                    }
                    (Err(reason), _) => {
                        picker.status = Some(format!("# {reason} · Enter asks again"));
                        picker.failed = true;
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
            PickerAction::Up => {
                // On a host, up is what the `..` row does.
                if let Some(on_host) = remote::typed(&picker.typed) {
                    picker.set_typed(above_on_host(&on_host));
                    return move_cursor_to_end(PICKER_ID);
                }
                let (folder, _) = split_typed(&picker.typed, &home());
                if let Some(above) = folder.parent() {
                    picker.set_typed(typed_folder(display_path(above)));
                    // The folder just left is highlighted, so → goes back into it.
                    if let Some(at) = picker.rows.iter().position(|row| row.path == folder) {
                        picker.select(at);
                    }
                    return move_cursor_to_end(PICKER_ID);
                }
            }
            PickerAction::Pick(index) => {
                picker.select(index);
                let chosen = self.update_picker(PickerAction::Choose);
                // A folder keeps the picker open, and the click took the box's focus. A file
                // closed it and focused the text, which focusing the gone box would undo.
                return match self.picker {
                    Some(_) => Task::batch([chosen, focus(PICKER_ID)]),
                    None => chosen,
                };
            }
            PickerAction::Choose => {
                let path = match picker.rows.get(picker.selected) {
                    Some(row) if row.is_dir => return self.update_picker(PickerAction::Complete),
                    // A file on a host is opened by its name there, `alias:/path`.
                    Some(row) if remote::typed(&picker.typed).is_some() => {
                        PathBuf::from(picker.completed(row))
                    }
                    Some(row) => row.path.clone(),
                    // No rows, as a host's folder hasn't come or couldn't be listed: a file
                    // named in it is opened without them; with no name, Enter asks again.
                    None => {
                        match Remote::parse(&picker.typed).filter(|_| picker.status.is_some()) {
                            Some(remote) => remote.as_path(),
                            None => match new_file(&picker.typed) {
                                // A name no file here has yet starts one, which its first
                                // save creates (`open_doc`).
                                Some(path) => path,
                                None => {
                                    picker.asked_for = None;
                                    picker.set_typed(picker.typed.clone());
                                    return Task::none();
                                }
                            },
                        }
                    }
                };
                self.picker = None;
                let open = self.update(Message::OpenPath(path));
                return Task::batch([open, focus(editor::EDITOR_ID)]);
            }
        }
        Task::none()
    }
}

/// What the box holds for the folder above the one it names on a host. Above the top of a
/// path from the home folder is the home folder (`alias:`); `/` has nothing above it.
fn above_on_host(typed: &Typed) -> String {
    let Typed { host, folder, .. } = typed;
    match folder.trim_end_matches('/').rfind('/') {
        Some(cut) => format!("{host}:{}", &folder[..=cut]),
        None if folder.starts_with('/') => format!("{host}:/"),
        None => format!("{host}:"),
    }
}

/// The file the box names on this computer when a name is typed after its folder, for a name
/// that matched nothing there.
fn new_file(typed: &str) -> Option<PathBuf> {
    let (folder, name) = split_typed(typed, &home());
    (remote::typed(typed).is_none() && !name.is_empty()).then(|| folder.join(name))
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
    let shown = picker.rows.iter().enumerate().skip(picker.top);
    for (index, entry) in shown.take(MAX_ROWS) {
        let selected = index == picker.selected;
        let (label, folder) = match entry.is_dir {
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
        (None, 0) if new_file(&picker.typed).is_some() => {
            "# nothing here matches · Enter starts a new file by that name".to_string()
        }
        (None, 0) => "# nothing here matches".to_string(),
        (None, rows) => {
            let more = match rows.saturating_sub(picker.top + MAX_ROWS) {
                0 => String::new(),
                more => format!("{more} more · "),
            };
            format!("# {more}→ goes in · ← goes up · {}", keys("Ctrl+O closes"))
        }
    };
    let foot = match picker.failed && picker.status.is_some() {
        true => warning(foot),
        false => comment(foot),
    };
    panel(column![prompt(search), rows, foot])
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
    fn the_list_scrolls_to_keep_the_highlight_in_view() {
        let dir = tempfile::tempdir().unwrap();
        for n in 1..=30 {
            fs::write(dir.path().join(format!("file-{n}.txt")), "x").unwrap();
        }
        // 31 rows: `..`, then the files, the first of them highlighted.
        let mut picker = Picker::new(typed_folder(dir.path().display().to_string()));
        assert_eq!((picker.selected, picker.top), (1, 0));
        picker.move_selection(13);
        assert_eq!((picker.selected, picker.top), (14, 1));
        picker.move_selection(100);
        assert_eq!((picker.selected, picker.top), (30, 30 + 1 - MAX_ROWS));
        // Back up, the list stays until the highlight reaches its first row.
        picker.move_selection(-10);
        assert_eq!((picker.selected, picker.top), (20, 30 + 1 - MAX_ROWS));
        picker.move_selection(-100);
        assert_eq!((picker.selected, picker.top), (0, 0));
    }

    #[test]
    fn enter_on_a_name_that_matches_nothing_starts_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.md"), "x").unwrap();
        let (mut app, _) = App::with_paths(Doc::Scratch, Paths::under(dir.path()));
        let _ = app.open_picker();
        let folder = typed_folder(dir.path().display().to_string());
        // With nothing typed after the folder there is no name to start a file by.
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert_eq!(new_file(&typed_folder(empty.display().to_string())), None);
        let _ = app.update_picker(PickerAction::Typed(format!("{folder}plan.txt")));
        assert!(names(&app).is_empty());
        let _ = app.update_picker(PickerAction::Choose);
        let plan = dir.path().join("plan.txt");
        assert!(app.picker.is_none());
        assert!(matches!(&app.doc, Doc::File { path, .. } if *path == plan));
        assert!(!plan.exists(), "made by its first save");
    }

    #[test]
    fn a_name_here_with_a_colon_in_it_is_not_taken_for_a_host() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("web1:old")).unwrap();
        fs::write(dir.path().join("web1:old/a.txt"), "a\n").unwrap();
        let (mut app, _) = App::with_paths(Doc::Scratch, Paths::under(dir.path()));
        let _ = app.open_picker();
        // In a folder's path the colon comes after a `/`, so it names no host.
        let folder = typed_folder(dir.path().display().to_string());
        let _ = app.update_picker(PickerAction::Typed(format!("{folder}web")));
        assert_eq!(names(&app), ["web1:old"]);
        let _ = app.update_picker(PickerAction::Complete);
        assert_eq!(names(&app), ["..", "a.txt"]);
        assert_eq!(picker(&app).asked, 0, "no host was asked");
        // With no folder before it, the name gets `./`.
        let entry = Entry {
            path: PathBuf::from("web1:old"),
            is_dir: true,
        };
        let completed = Picker::new("web".to_string()).completed(&entry);
        assert_eq!(completed, "./web1:old/");
        assert_eq!(remote::typed(&completed), None);
    }

    #[test]
    fn a_host_and_a_colon_names_a_folder_there_which_works_like_one_here() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("home/docs")).unwrap();
        fs::write(root.join("home/a.txt"), "a\n").unwrap();
        fs::create_dir_all(root.join("ssh")).unwrap();
        fs::write(root.join("ssh/config"), "Host web1 *.lan\nHost db1\n").unwrap();
        let ssh = fake_ssh(&root);
        let (mut app, _) = App::with_paths(Doc::Scratch, Paths::under(&root));
        let typed = |app: &mut App, typed: &str| {
            let _ = app.update_picker(PickerAction::Typed(typed.to_string()));
        };
        let listed = |app: &mut App, folder: &str| {
            let asked = picker(app).asked;
            let listed = remote::list(&ssh, "web1", folder);
            let _ = app.update_picker(PickerAction::Listed(asked, listed));
        };
        let home = format!("web1:{}/home/", root.display());

        // The hosts in the SSH config aren't offered: the rows are this computer's folders,
        // and without a colon so is what's typed.
        let _ = app.open_picker();
        assert!(names(&app).iter().all(|name| !name.contains(':')));
        typed(&mut app, "web1");
        assert_eq!((picker(&app).asked, &picker(&app).status), (0, &None));

        // A host and a colon asks for its home folder, whose whole path the box takes.
        typed(&mut app, "web1:");
        assert_eq!(picker(&app).status.as_deref(), Some("# asking web1…"));
        listed(&mut app, "");
        assert_eq!(picker(&app).typed, home);
        assert_eq!(
            (names(&app), picker(&app).asked),
            (
                vec!["..", "docs", "a.txt"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                1
            )
        );

        // A path after the colon is from the home folder, and a name after it narrows the
        // folder once it comes. `~/` may start it.
        typed(&mut app, "web1:docs/");
        typed(&mut app, "web1:docs/x");
        assert_eq!(picker(&app).asked, 2);
        listed(&mut app, "docs");
        assert_eq!(picker(&app).typed, format!("{home}docs/x"));
        assert!(names(&app).is_empty());
        typed(&mut app, "web1:~/docs/");
        listed(&mut app, "docs");
        assert_eq!(picker(&app).typed, format!("{home}docs/"));
        assert_eq!(names(&app), [".."]);

        // A folder typed in is asked for once, however much is typed after it; the answer for
        // one since left is dropped.
        typed(&mut app, &home);
        typed(&mut app, &format!("{home}a"));
        typed(&mut app, &home);
        let asked = picker(&app).asked;
        let late = remote::list(&ssh, "web1", "docs");
        let _ = app.update_picker(PickerAction::Listed(asked - 1, late));
        assert_eq!(
            (picker(&app).typed.as_str(), names(&app).len()),
            (home.as_str(), 0)
        );
        listed(&mut app, &format!("{}/home", root.display()));
        assert_eq!(names(&app), ["..", "docs", "a.txt"]);
        // Typing narrows it without asking again; → goes into a folder, which is asked for.
        typed(&mut app, &format!("{home}a"));
        assert_eq!(
            (names(&app), picker(&app).asked),
            (vec!["a.txt".to_string()], asked)
        );
        typed(&mut app, &home);
        let _ = app.update_picker(PickerAction::Into);
        assert_eq!(picker(&app).typed, format!("{home}docs/"));
        assert_eq!(picker(&app).asked, asked + 1);

        // A folder that doesn't come says why, and Enter asks again.
        let away = Err("can't reach web1".to_string());
        let _ = app.update_picker(PickerAction::Listed(asked + 1, away));
        let status = picker(&app).status.clone().unwrap();
        assert_eq!(status, "# can't reach web1 · Enter asks again");
        assert!(picker(&app).failed, "drawn as a warning");
        let _ = app.update_picker(PickerAction::Choose);
        assert_eq!(picker(&app).asked, asked + 2);
        assert!(!picker(&app).failed, "asking again is no warning");

        // ← goes up: to the home folder from a path named from it, and no further than `/`,
        // which has no `..` row.
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, home);
        typed(&mut app, "web1:docs/sub/");
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, "web1:docs/");
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, "web1:");
        typed(&mut app, "web1:/");
        listed(&mut app, "/");
        assert!(!names(&app).contains(&"..".to_string()));
        let _ = app.update_picker(PickerAction::Up);
        assert_eq!(picker(&app).typed, "web1:/");

        // Enter on a file opens it from its host.
        typed(&mut app, &home);
        listed(&mut app, &format!("{}/home", root.display()));
        typed(&mut app, &format!("{home}a"));
        let _ = app.update_picker(PickerAction::Choose);
        assert!(app.picker.is_none());
        let opening = format!("Opening web1:{}/home/a.txt…", root.display());
        assert_eq!(app.fetching, Some(opening));

        // Enter on a file's name typed out doesn't wait for its folder's rows. A name that
        // matches nothing in a folder that did come opens nothing, as here.
        let _ = app.open_picker();
        typed(&mut app, "web1:a.txt");
        let _ = app.update_picker(PickerAction::Choose);
        assert!(app.picker.is_none());
        assert_eq!(app.fetching.as_deref(), Some("Opening web1:a.txt…"));
        let _ = app.open_picker();
        typed(&mut app, &home);
        listed(&mut app, &format!("{}/home", root.display()));
        typed(&mut app, &format!("{home}nothing"));
        let _ = app.update_picker(PickerAction::Choose);
        assert!(app.picker.is_some());
    }
}
