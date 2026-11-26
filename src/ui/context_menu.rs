//! The right-click menu in the editor: the same actions as the shortcuts, for the text under the
//! pointer. A row that can't apply right now (Cut with nothing selected, Undo with nothing to
//! undo) is greyed out; one that makes no sense for this file (comment lines in plain text, Format
//! JSON outside a .json file) is left out.

use iced::widget::{column, container, mouse_area, responsive, space};
use iced::{Element, Fill, Padding, Point};

use crate::core::markdown::{BOLD, ITALIC};
use crate::core::syntax::Syntax;
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::{LINE, SIZE};
use crate::ui::find_bar::FindAction;
use crate::ui::palette::tools;
use crate::ui::status_bar::menu_item;
use crate::ui::text_editor::Action;
use crate::ui::theme::{self, Roles, raised_box};

const WIDTH: f32 = 260.0;
/// The panel's padding above and below its rows, and its border.
const FRAME: f32 = 8.0 * 2.0 + 2.0;
/// The room a divider takes between two groups of rows.
const DIVIDER: f32 = 9.0;
/// How near the window's edge the menu may come.
const MARGIN: f32 = 4.0;

/// The menu with its top left corner at `at`, moved in as far as it takes to fit in the window.
/// Clicking anywhere else closes it.
pub fn view(app: &App, at: Point) -> Element<'_, Message> {
    responsive(move |window| {
        let groups = groups(app);
        let rows: usize = groups.iter().map(Vec::len).sum();
        let height = rows as f32 * SIZE * LINE + groups.len().saturating_sub(1) as f32 * DIVIDER;
        let mut panel = column![];
        for (index, group) in groups.into_iter().enumerate() {
            if index > 0 {
                panel = panel.push(divider());
            }
            for (label, shortcut, message) in group {
                let message = message.map(|message| Message::Context(Box::new(message)));
                panel = panel.push(menu_item(label, shortcut, message));
            }
        }
        let panel = container(panel.width(WIDTH))
            .padding([8, 0])
            .style(raised_box);
        let left = at.x.min(window.width - WIDTH - MARGIN).max(MARGIN);
        let top =
            at.y.min(window.height - height - FRAME - MARGIN)
                .max(MARGIN);
        iced::widget::stack![
            mouse_area(space().width(Fill).height(Fill))
                .on_press(Message::CloseMenu)
                .on_right_press(Message::CloseMenu),
            container(panel).width(Fill).height(Fill).padding(Padding {
                left,
                top,
                ..Padding::ZERO
            }),
        ]
        .into()
    })
    .into()
}

/// A thin line between groups of rows.
fn divider<'a>() -> Element<'a, Message> {
    let line = container(space().width(Fill).height(1))
        .style(|theme| theme::fill(Roles::of(theme).border));
    container(line).padding([4, 12]).into()
}

/// A row: its label, its shortcut, and what it does (`None` when greyed out).
type Row = (String, &'static str, Option<Message>);

/// The rows in their groups: the clipboard, undo, then what suits this file and selection.
fn groups(app: &App) -> Vec<Vec<Row>> {
    let editable = app.editable();
    let selected = app.pane.selection().is_some();
    let syntax = app.syntax();
    // A row that does something only when `when` holds.
    let row = |label: &str, shortcut, when: bool, message: Message| -> Row {
        (label.to_string(), shortcut, when.then_some(message))
    };

    let clipboard = vec![
        row(
            "cut",
            "Ctrl+X",
            editable && selected,
            Message::Clip { cut: true },
        ),
        row("copy", "Ctrl+C", selected, Message::Clip { cut: false }),
        row("paste", "Ctrl+V", editable, Message::Paste),
        row(
            "select all",
            "Ctrl+A",
            true,
            Message::Edit(Action::SelectAll),
        ),
    ];
    let history = vec![
        row(
            "undo",
            "Ctrl+Z",
            editable && app.pane.model.can_undo(),
            Message::Undo,
        ),
        row(
            "redo",
            "Ctrl+Shift+Z",
            editable && app.pane.model.can_redo(),
            Message::Redo,
        ),
    ];

    let mut tools_for_file = vec![row("find", "Ctrl+F", true, Message::Find(FindAction::Open))];
    if editable {
        tools_for_file.push(row(
            "duplicate lines",
            "Ctrl+Shift+D",
            true,
            Message::DuplicateLines,
        ));
        tools_for_file.push(row(
            "delete lines",
            "Ctrl+Shift+K",
            true,
            Message::DeleteLines,
        ));
        if syntax.comment_marker().is_some() {
            tools_for_file.push(row("comment lines", "Ctrl+/", true, Message::ToggleComment));
        }
        if syntax == Syntax::Markdown {
            tools_for_file.push(row("bold", "Ctrl+B", true, Message::ToggleMarker(BOLD)));
            tools_for_file.push(row("italic", "Ctrl+I", true, Message::ToggleMarker(ITALIC)));
        }
        // The palette's tools (Format JSON, New Host block), written like the other rows.
        for (name, message) in tools(syntax) {
            tools_for_file.push((name.to_lowercase(), "", Some(message)));
        }
        if matches!(app.doc, crate::core::opened::Doc::Scratch) {
            tools_for_file.push(row(
                "save selection as",
                "Ctrl+Shift+S",
                selected,
                Message::SaveAs,
            ));
        }
    }
    vec![clipboard, history, tools_for_file]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch window holding `text`, in a data folder of its own.
    fn scratch_window(text: &str) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::core::paths::Paths::under(dir.path());
        let (mut app, _) = App::with_paths(crate::core::opened::Doc::Scratch, paths);
        for letter in text.chars() {
            let typed = crate::ui::text_editor::Edit::Insert(letter);
            let _ = app.update(Message::Edit(Action::Edit(typed)));
        }
        (dir, app)
    }

    fn labels(app: &App) -> Vec<String> {
        let groups = groups(app);
        groups.iter().flatten().map(|row| row.0.clone()).collect()
    }

    fn enabled(app: &App, label: &str) -> bool {
        let groups = groups(app);
        let found = groups.iter().flatten().find(|row| row.0 == label);
        found.is_some_and(|row| row.2.is_some())
    }

    #[test]
    fn with_nothing_selected_cut_and_copy_are_greyed_out() {
        let (_dir, app) = scratch_window("one two");
        assert!(!enabled(&app, "cut"));
        assert!(!enabled(&app, "copy"));
        assert!(!enabled(&app, "save selection as"));
        assert!(enabled(&app, "paste"));
        assert!(enabled(&app, "select all"));
    }

    #[test]
    fn a_selection_turns_cut_copy_and_save_selection_on() {
        let (_dir, mut app) = scratch_window("one two");
        let _ = app.update(Message::Edit(Action::SelectAll));
        assert!(enabled(&app, "cut"));
        assert!(enabled(&app, "copy"));
        assert!(enabled(&app, "save selection as"));
    }

    #[test]
    fn undo_waits_for_something_to_undo() {
        let (_dir, mut app) = scratch_window("");
        assert!(!enabled(&app, "undo"));
        let _ = app.update(Message::Edit(Action::Edit(
            crate::ui::text_editor::Edit::Insert('x'),
        )));
        assert!(enabled(&app, "undo"));
        assert!(!enabled(&app, "redo"));
    }

    #[test]
    fn rows_for_other_kinds_of_file_are_left_out() {
        let (_dir, mut app) = scratch_window("one two");
        app.settings.scratch_syntax = crate::core::settings::ScratchSyntax::Plain;
        let shown = labels(&app);
        assert!(!shown.contains(&"comment lines".to_string()));
        assert!(!shown.contains(&"bold".to_string()));
        assert!(shown.contains(&"save selection as".to_string()));
    }

    #[test]
    fn a_right_click_opens_the_menu_and_choosing_a_row_closes_it() {
        let (_dir, mut app) = scratch_window("one two");
        let _ = app.update(Message::ContextMenu(Point::new(10.0, 10.0)));
        assert!(app.context_menu.is_some());
        let _ = app.update(Message::Context(Box::new(Message::Edit(Action::SelectAll))));
        assert!(app.context_menu.is_none());
        assert!(app.pane.selection().is_some());
    }
}
