//! A project window: the committed version on the left (read-only), the draft on the right, each
//! under a small header, and the Commit dialog that turns the draft into the next version.

use std::time::{Duration, Instant};

use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, mouse_area, responsive, row, space, text, text_input,
};
use iced::{Background, Border, Color, Element, Fill, Padding, Theme};

use crate::core::counts::{Counts, percent_change};
use crate::core::save::SaveState;
use crate::project::Project;
use crate::ui::app::{App, Message};
use crate::ui::editor::{self, Pane};
use crate::ui::theme::{GLYPH_LOCK, Roles};

pub const COMMIT_NOTE_ID: &str = "commit-note";
/// How long the text takes to slide left after a Rewrite.
const SLIDE: Duration = Duration::from_millis(300);
const PANE_PADDING: Padding = Padding {
    top: 28.0,
    right: 24.0,
    bottom: 40.0,
    left: 24.0,
};

pub struct ProjectView {
    pub project: Project,
    /// The left pane: a committed version. Only scrolling, selecting and copying reach it.
    pub reference: Pane,
    pub reference_version: u32,
    pub reference_counts: Counts,
    /// The newest version's counts, for the Commit dialog (the left pane may show an older one).
    pub latest_counts: Counts,
    /// When the Rewrite slide began; `None` once it has finished.
    pub slide_started: Option<Instant>,
}

impl ProjectView {
    /// A project whose left pane shows `reference_text`, the newest version.
    pub fn new(project: Project, reference: Pane) -> ProjectView {
        let counts = Counts::of(reference.model.text());
        ProjectView {
            reference_version: project.latest_version(),
            project,
            reference,
            reference_counts: counts,
            latest_counts: counts,
            slide_started: None,
        }
    }

    /// Shows version `number` (with its `text`) in the left pane.
    pub fn show_version(&mut self, number: u32, text: &str) {
        self.reference = Pane::new(text);
        self.reference_version = number;
        self.reference_counts = Counts::of(text);
        if number == self.project.latest_version() {
            self.latest_counts = self.reference_counts;
        }
    }

    /// The draft against the left pane, as "−45%"; `None` before there is anything to compare.
    pub fn change_label(&self, draft: Counts) -> Option<(String, bool)> {
        let percent = percent_change(self.reference_counts, draft)?;
        let label = match percent {
            p if p < 0 => format!("\u{2212}{}%", -p),
            p if p > 0 => format!("+{p}%"),
            _ => "0%".to_string(),
        };
        Some((label, percent < 0))
    }

    /// How far the Rewrite slide has got, from 0 to 1, eased to slow down at the end.
    fn slide_progress(&self, now: Instant) -> f32 {
        let Some(start) = self.slide_started else {
            return 1.0;
        };
        let t = (now.saturating_duration_since(start).as_secs_f32() / SLIDE.as_secs_f32()).min(1.0);
        1.0 - (1.0 - t).powi(3)
    }

    pub fn slide_done(&self, now: Instant) -> bool {
        self.slide_progress(now) >= 1.0
    }
}

/// The two panes side by side with a 1px border between. Right after a Rewrite the draft grows
/// in from the right edge while the old text slides into the left half.
pub fn view<'a>(app: &'a App, project: &'a ProjectView) -> Element<'a, Message> {
    responsive(move |size| panes(app, project, size.width)).into()
}

/// The panes in a row `width` pixels wide.
fn panes<'a>(app: &'a App, project: &'a ProjectView, width: f32) -> Element<'a, Message> {
    let roles = Roles::of(&app.theme);
    let version = project
        .project
        .manifest
        .versions
        .iter()
        .find(|version| version.number == project.reference_version);
    let mut committed = match version {
        Some(version) => format!("committed {}", version.committed_on()),
        None => String::new(),
    };
    if let Some(note) = version
        .map(|version| &version.note)
        .filter(|note| !note.is_empty())
    {
        committed.push_str(&format!(" · \"{note}\""));
    }
    let reference_header = header(vec![
        muted(GLYPH_LOCK).size(14).into(),
        text(format!("v{}", project.reference_version))
            .size(13)
            .font(bold())
            .into(),
        // A long note is cut off rather than pushing the counts out of view.
        container(muted(committed)).width(Fill).clip(true).into(),
        muted(counts_label(project.reference_counts)).into(),
    ]);

    let draft = app.counts;
    // A read-only window never saves, so it shows no save state (as the status bar hides its dot).
    let save_word = match app.autosaver.state() {
        _ if !app.editable() => "read-only".to_string(),
        SaveState::Saved => match app.saved_at {
            Some(at) if app.now.saturating_duration_since(at).as_secs() >= 60 => format!(
                "autosaved {} min ago",
                app.now.saturating_duration_since(at).as_secs() / 60
            ),
            Some(_) => "autosaved just now".to_string(),
            None => "autosaved".to_string(),
        },
        SaveState::Saving => "saving…".to_string(),
        SaveState::Failed(_) => "save failed".to_string(),
    };
    let mut draft_row = vec![
        text("Draft")
            .size(13)
            .font(bold())
            .color(roles.accent_text)
            .into(),
        // Cut off first when the pane is narrow, so the counts and badge stay in view.
        container(muted(save_word)).width(Fill).clip(true).into(),
        muted(counts_label(draft)).into(),
    ];
    if let Some((label, shorter)) = project.change_label(draft) {
        draft_row.push(badge(label, shorter));
    }
    let draft_header = header(draft_row);

    let reference_pane = column![
        reference_header,
        editor::view(
            &project.reference.content,
            &app.settings,
            false,
            editor::REFERENCE_ID,
            Message::ReferenceEdit,
            PANE_PADDING,
        )
    ];
    let draft_pane = column![
        draft_header,
        editor::view(
            &app.pane.content,
            &app.settings,
            app.editable(),
            editor::EDITOR_ID,
            Message::Edit,
            PANE_PADDING,
        )
    ];
    // The draft's share of the width grows from nothing to half as the slide plays. The left pane
    // gets a whole number of pixels, so the 1px border lands on one pixel column, not two.
    let reference_share = 1.0 / (1.0 + project.slide_progress(app.now));
    let reference_width = ((width - 1.0) * reference_share).floor();
    row![
        container(reference_pane).width(reference_width).clip(true),
        container(space().width(1).height(Fill)).style(|theme: &Theme| container::Style {
            background: Some(Background::Color(Roles::of(theme).border)),
            ..container::Style::default()
        }),
        container(draft_pane).width(Fill).clip(true),
    ]
    .height(Fill)
    .into()
}

/// A pane's header row, with the soft border under it.
fn header(items: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    column![
        container(row(items).spacing(10).height(18).align_y(iced::Center)).padding(Padding {
            top: 14.0,
            right: 24.0,
            bottom: 12.0,
            left: 24.0,
        }),
        container(space().width(Fill).height(1)).style(|theme: &Theme| container::Style {
            background: Some(Background::Color(Roles::of(theme).bg_raised)),
            ..container::Style::default()
        }),
    ]
    .into()
}

fn counts_label(counts: Counts) -> String {
    match counts.tokens {
        Some(tokens) => format!("{} words · {tokens} tokens", counts.words),
        None => format!("{} words · … tokens", counts.words),
    }
}

/// The draft's size change as a small pill: aqua when it got shorter.
fn badge<'a>(label: String, shorter: bool) -> Element<'a, Message> {
    container(text(label).size(11))
        .padding([1, 6])
        .style(move |theme: &Theme| {
            let roles = Roles::of(theme);
            let color = if shorter {
                roles.accent_aqua
            } else {
                roles.fg_muted
            };
            container::Style {
                background: Some(Background::Color(color.scale_alpha(0.16))),
                text_color: Some(color),
                border: Border::default().rounded(4),
                ..container::Style::default()
            }
        })
        .into()
}

fn muted<'a>(words: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(words)
        .size(12)
        .wrapping(Wrapping::None)
        .style(|theme: &Theme| text::Style {
            color: Some(Roles::of(theme).fg_muted),
        })
}

fn bold() -> iced::Font {
    iced::Font {
        weight: iced::font::Weight::Bold,
        ..crate::ui::theme::MONO
    }
}

/// The Commit dialog: the next version's number, how the counts change, a note, and the two
/// ways out (Esc goes back, Enter commits). Idea rows arrive in phase 4.
pub fn commit_dialog<'a>(
    app: &'a App,
    project: &'a ProjectView,
    note: &'a str,
) -> Element<'a, Message> {
    let roles = Roles::of(&app.theme);
    let old = project.latest_counts;
    let new = app.counts;
    let tokens = match (old.tokens, new.tokens) {
        (Some(old), Some(new)) => format!("{old} → {new} tokens · "),
        _ => String::new(),
    };
    let summary = format!("{tokens}{} → {} words", old.words, new.words);
    let next = project.project.latest_version() + 1;

    let note_field = text_input("Note for this version (optional)", note)
        .id(COMMIT_NOTE_ID)
        .on_input(Message::CommitNote)
        .on_submit(Message::Commit)
        .size(13)
        .padding([5, 8])
        .style(|theme: &Theme, status| {
            let roles = Roles::of(theme);
            let border = match status {
                text_input::Status::Focused { .. } => roles.accent_text,
                _ => roles.border,
            };
            text_input::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default().rounded(4).width(1).color(border),
                icon: roles.fg_muted,
                placeholder: roles.fg_subtle,
                value: roles.fg,
                selection: roles.accent_cool.scale_alpha(0.35),
            }
        });

    let buttons = row![
        space::horizontal(),
        dialog_button("go back", "esc", false, Some(Message::CloseMenu)),
        dialog_button("commit", "⏎", true, (!app.busy).then_some(Message::Commit)),
    ]
    .spacing(6);
    let card = column![
        row![
            text(format!("Commit v{next}")).size(13),
            space::horizontal(),
            muted(summary)
        ]
        .align_y(iced::Center),
        note_field,
        column![
            container(space().width(Fill).height(1)).style(move |_: &Theme| container::Style {
                background: Some(Background::Color(roles.border)),
                ..container::Style::default()
            }),
            buttons
        ]
        .spacing(12),
    ]
    .spacing(16);
    let card = container(card)
        .width(520)
        .padding(Padding {
            top: 20.0,
            right: 22.0,
            bottom: 16.0,
            left: 22.0,
        })
        .style(|theme: &Theme| {
            let roles = Roles::of(theme);
            container::Style {
                background: Some(Background::Color(roles.bg_surface)),
                text_color: Some(roles.fg),
                border: Border::default().rounded(6).width(1).color(roles.border),
                ..container::Style::default()
            }
        });
    modal(card.into())
}

/// `card` centred over a dimmed window; clicking outside it goes back.
pub fn modal(card: Element<'_, Message>) -> Element<'_, Message> {
    iced::widget::stack![
        mouse_area(
            container(space().width(Fill).height(Fill)).style(|theme: &Theme| container::Style {
                background: Some(Background::Color(Roles::of(theme).bg_page.scale_alpha(0.7))),
                ..container::Style::default()
            })
        )
        .on_press(Message::CloseMenu),
        container(iced::widget::opaque(card)).center(Fill),
    ]
    .into()
}

/// A word button with its key hint, as in the Commit mockup; `primary` is the yellow default.
fn dialog_button<'a>(
    label: &'a str,
    key: &'a str,
    primary: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    button(row![text(label).size(12), muted(key)].spacing(6))
        .padding([4, 10])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: None,
                text_color: match (primary, hovered) {
                    (true, _) => roles.accent_text,
                    (false, true) => roles.fg,
                    (false, false) => roles.fg_muted,
                },
                border: Border::default()
                    .rounded(4)
                    .width(1)
                    .color(if primary || hovered {
                        roles.border
                    } else {
                        Color::TRANSPARENT
                    }),
                ..button::Style::default()
            }
        })
        .on_press_maybe(on_press)
        .into()
}
