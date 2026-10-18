//! A project window: the committed version on the left (read-only), the draft on the right, each
//! under a small header.

use std::time::{Duration, Instant};

use iced::widget::{column, container, mouse_area, responsive, row, space, text};
use iced::{Background, Border, Element, Fill, Padding, Rectangle, Theme};

use crate::core::counts::{Counts, percent_change};
use crate::core::save::SaveState;
use crate::core::text_model::TextModel;
use crate::project::Project;
use crate::project::manifest::Manifest;
use crate::project::paint::IdeaId;
use crate::project::weights::{CommitCheck, Status, Weights, commit_check, weights};
use crate::ui::app::{App, Message};
use crate::ui::assistant_view::PANE_WIDTH;
use crate::ui::editor::{self, Pane};
use crate::ui::idea_panel::IdeaPanel;
use crate::ui::ideas_view;
use crate::ui::text_editor::{PaintLook, PaintRun};
use crate::ui::theme::{GLYPH_LOCK, Roles, bold, fill, muted};
use crate::ui::weights_view::Measure;

/// How long the text takes to slide left after a Rewrite.
const SLIDE: Duration = Duration::from_millis(300);
/// With the assistant pane open, a window at most this wide hides the left pane first, as in the
/// mockup.
const HIDE_REFERENCE_BELOW: f32 = 1100.0;
const PANE_PADDING: Padding = Padding {
    top: 28.0,
    right: 24.0,
    bottom: 40.0,
    // The pane's 24px margin, then the gutter stripes (10px) and their 12px gap, as in the
    // mockup, so the stripes never touch the window edge or the divider.
    left: 46.0,
};

pub struct ProjectView {
    pub project: Project,
    /// Each idea's share of the left pane's text and the draft's, refreshed on every change.
    pub weights: Weights,
    /// The paint as the two editors draw it, rebuilt with `weights` (the draft's suggestions are
    /// added in `view`).
    pub reference_runs: Vec<PaintRun>,
    pub draft_runs: Vec<PaintRun>,
    /// The idea panel, while it is open.
    pub idea_panel: Option<IdeaPanel>,
    pub weights_open: bool,
    pub measure: Measure,
    /// The Commit dialog's idea rows, while it is open.
    pub commit_check: Option<CommitCheck>,
    /// A mouse selection is being made in the left pane; letting go paints it with the brush.
    pub mouse_selecting: bool,
    /// Where the draft's caret is drawn, and its text area, for the feeling-note card.
    pub caret: Option<Rectangle>,
    pub text_area: Rectangle,
    /// The left pane: a committed version. Only scrolling, selecting and copying reach it.
    pub reference: Pane,
    pub reference_version: u32,
    pub reference_counts: Counts,
    /// The newest version's text, for the Commit check (the left pane may show an older one);
    /// `None` until it has been read from git.
    pub latest_text: Option<String>,
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
            weights: Weights::default(),
            reference_runs: Vec::new(),
            draft_runs: Vec::new(),
            idea_panel: None,
            weights_open: false,
            measure: Measure::Percent,
            commit_check: None,
            mouse_selecting: false,
            caret: None,
            text_area: Rectangle::default(),
            reference,
            reference_counts: counts,
            latest_text: None,
            slide_started: None,
        }
    }

    /// Shows version `number` (with its `text`) in the left pane.
    pub fn show_version(&mut self, number: u32, text: &str) {
        self.reference = Pane::new(text);
        self.reference.model.paint = self.project.version_paint(number, text);
        self.reference_version = number;
        self.reference_counts = Counts::of(text);
        if number == self.project.latest_version() {
            self.latest_text = Some(text.to_string());
        }
    }

    /// The Commit check: the draft against the newest version. `None` until that version's text
    /// has loaded.
    pub fn check_commit(&self, draft: &TextModel) -> Option<CommitCheck> {
        let text = self.latest_text.as_deref()?;
        let paint = self
            .project
            .version_paint(self.project.latest_version(), text);
        let manifest = &self.project.manifest;
        Some(commit_check(
            manifest,
            text,
            &paint,
            draft.text(),
            &draft.paint,
        ))
    }

    /// Weighs the ideas again and rebuilds the paint both editors draw, after any change to the
    /// text, the paint, the ideas, the suggestions or the theme. In the left pane a missing idea
    /// is outlined and one cut on purpose struck through.
    pub fn refresh_ideas(&mut self, draft_pane: &Pane, roles: &Roles) {
        let manifest = &self.project.manifest;
        let old = &self.reference.model;
        let draft = &draft_pane.model;
        let version = self.reference_version;
        self.weights = weights(
            manifest,
            version,
            old.text(),
            &old.paint,
            draft.text(),
            &draft.paint,
        );
        let statuses = &self.weights.ideas;
        self.reference_runs = paint_runs(manifest, &self.reference, roles, |idea| {
            match statuses
                .iter()
                .find(|row| row.idea == idea)
                .map(|row| row.status)
            {
                Some(Status::Missing) => PaintLook::Missing(roles.accent_warm),
                Some(Status::Cut) => PaintLook::Cut(roles.fg_subtle),
                _ => PaintLook::Fill,
            }
        });
        self.draft_runs = paint_runs(manifest, draft_pane, roles, |_| PaintLook::Fill);
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

/// `pane`'s paint as the editor draws it, each span in the colour of the idea it now belongs to,
/// looking as `look` says.
fn paint_runs(
    manifest: &Manifest,
    pane: &Pane,
    roles: &Roles,
    look: impl Fn(IdeaId) -> PaintLook,
) -> Vec<PaintRun> {
    pane.model
        .paint
        .iter()
        .filter_map(|span| {
            let idea = live_idea(manifest, span.idea)?;
            Some(PaintRun {
                start: pane.position(span.start, true),
                end: pane.position(span.end, false),
                color: roles.idea(manifest.idea(idea)?.colour),
                look: look(idea),
            })
        })
        .collect()
}

/// The idea that `idea`'s paint now belongs to: itself, or the one it was merged into.
fn live_idea(manifest: &Manifest, idea: IdeaId) -> Option<IdeaId> {
    let live = manifest
        .live_ideas()
        .find(|live| manifest.lineage(live.id).contains(&idea))?;
    Some(live.id)
}

/// The two panes side by side with a 1px border between. Right after a Rewrite the draft grows
/// in from the right edge while the old text slides into the left half.
pub fn view<'a>(app: &'a App, project: &'a ProjectView) -> Element<'a, Message> {
    column![
        responsive(move |size| panes(app, project, size.width)),
        ideas_view::strip(app, project),
    ]
    .into()
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
    let waiting = app.pane.suggestions().len();
    let save_word = match app.autosaver.state() {
        _ if !app.editable() => "read-only".to_string(),
        SaveState::Failed(_) => "save failed".to_string(),
        _ if waiting == 1 => "1 suggestion waiting".to_string(),
        _ if waiting > 1 => format!("{waiting} suggestions waiting"),
        SaveState::Saved => match app.saved_at {
            Some(at) if app.now.saturating_duration_since(at).as_secs() >= 60 => format!(
                "autosaved {} min ago",
                app.now.saturating_duration_since(at).as_secs() / 60
            ),
            Some(_) => "autosaved just now".to_string(),
            None => "autosaved".to_string(),
        },
        SaveState::Saving => "saving…".to_string(),
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
            &project.reference,
            &app.settings,
            false,
            editor::REFERENCE_ID,
            Message::ReferenceEdit,
            PANE_PADDING,
            project.reference_runs.clone(),
        )
    ];
    let manifest = &project.project.manifest;
    let idea_color = |idea| Some(roles.idea(manifest.idea(live_idea(manifest, idea)?)?.colour));
    let mut draft_runs = project.draft_runs.clone();
    draft_runs.extend(app.pane.suggestion_runs(roles, idea_color, true));
    let draft_pane = column![
        draft_header,
        editor::view(
            &app.pane,
            &app.settings,
            app.editable(),
            editor::EDITOR_ID,
            Message::Edit,
            PANE_PADDING,
            draft_runs,
        )
    ];
    let draft_pane = container(draft_pane).width(Fill).clip(true);
    if app.assistant.open && width + PANE_WIDTH <= HIDE_REFERENCE_BELOW {
        return draft_pane.height(Fill).into();
    }
    // The draft's share of the width grows from nothing to half as the slide plays. The left pane
    // gets a whole number of pixels, so the 1px border lands on one pixel column, not two.
    let reference_share = 1.0 / (1.0 + project.slide_progress(app.now));
    let reference_width = ((width - 1.0) * reference_share).floor();
    row![
        container(reference_pane).width(reference_width).clip(true),
        container(space().width(1).height(Fill))
            .style(|theme: &Theme| fill(Roles::of(theme).border)),
        draft_pane,
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
        container(space().width(Fill).height(1))
            .style(|theme: &Theme| fill(Roles::of(theme).bg_raised)),
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
