//! View model for the help-overlay modal (`Screen::HelpOverlay`).
//!
//! Per spec §4 every view is a `#[drv::memo]`. The help text itself
//! is static — the only varying input is the scroll offset. Wrapping
//! it in a memo keeps the model-cache + diff path uniform with
//! every other view (queue, left, middle_header, …).
//!
//! Legacy `mkp2/mkp/src/nav/help_overlay.rs` lays the help out as
//! two columns: Global on the left, Navigation + Lists on the
//! right. Each column is a flat list of sections (heading + key /
//! description rows). The renderer paints each column as its own
//! `Paragraph`, so the model carries the two flows independently
//! rather than aligning row-by-row across the gutter.

use mkpclient_state_ui_keybindings::{Action, KeyContext, Keybindings};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HelpEntry {
    /// Key chord shown in the left column of each section
    /// (e.g. `Space`, `]`, `↑ / ↓`). Pre-formatted so the painter
    /// is allocation-free on cache hits.
    pub key: String,
    /// Human description shown next to the key.
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HelpSection {
    pub heading: String,
    pub entries: Vec<HelpEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HelpOverlayModel {
    pub scroll: u16,
    /// Left column flow (legacy: "Global"). `Arc` so the memo's
    /// cache-hit clone is a refcount bump, not a Vec realloc.
    pub left: Arc<Vec<HelpSection>>,
    /// Right column flow (legacy: "Navigation" + "Lists").
    pub right: Arc<Vec<HelpSection>>,
    pub configure_key: String,
    pub close_key: String,
}

#[derive(drv::Input)]
pub struct HelpOverlayInput {
    pub scroll: u16,
    pub hints: Vec<String>,
}

impl HelpOverlayInput {
    pub fn new(scroll: u16, keys: &Keybindings) -> Self {
        let g = KeyContext::Global;
        let m = KeyContext::Move;
        let l = KeyContext::ListNavigation;
        let h = KeyContext::HelpOverlay;
        let actions = [
            (g, Action::PlayPause),
            (g, Action::NextTrack),
            (g, Action::PreviousTrack),
            (g, Action::SeekBackward10s),
            (g, Action::SeekForward10s),
            (g, Action::SeekBackward1s),
            (g, Action::SeekForward1s),
            (g, Action::CycleRepeat),
            (g, Action::OpenSearch),
            (g, Action::ToggleHelp),
            (g, Action::Suspend),
            (g, Action::Quit),
            (m, Action::MoveUp),
            (m, Action::MoveDown),
            (m, Action::FocusLeft),
            (m, Action::FocusRight),
            (g, Action::HistoryBack),
            (g, Action::HistoryForward),
            (m, Action::MoveToTop),
            (m, Action::MoveToBottom),
            (m, Action::PageUp),
            (m, Action::PageDown),
            (l, Action::Activate),
            (l, Action::ShuffleActivate),
            (l, Action::OpenActionMenu),
            (g, Action::ToggleFilter),
            (l, Action::Back),
            (h, Action::OpenKeybindingsEditor),
            (h, Action::CloseHelp),
        ];
        Self {
            scroll,
            hints: actions
                .into_iter()
                .map(|(ctx, action)| keys.hint_for(ctx, action))
                .collect(),
        }
    }
}

#[drv::memo(single)]
pub fn help_overlay_model(input: HelpOverlayInput) -> HelpOverlayModel {
    let h = &input.hints;
    HelpOverlayModel {
        scroll: input.scroll,
        left: Arc::new(canonical_left_sections(h)),
        right: Arc::new(canonical_right_sections(h)),
        configure_key: h[27].clone(),
        close_key: h[28].clone(),
    }
}

fn entry(key: &str, description: &str) -> HelpEntry {
    HelpEntry {
        key: key.to_string(),
        description: description.to_string(),
    }
}

/// Left column. Mirrors `help_sections_left` in
/// `mkp2/mkp/src/nav/help_overlay.rs`.
fn canonical_left_sections(h: &[String]) -> Vec<HelpSection> {
    vec![HelpSection {
        heading: "Global".into(),
        entries: vec![
            entry(&h[0], "Play / Pause"),
            entry(&h[1], "Next track"),
            entry(&h[2], "Previous track"),
            entry(&format!("{} / {}", h[3], h[4]), "Seek \u{00b1}10s"),
            entry(&format!("{} / {}", h[5], h[6]), "Seek \u{00b1}1s"),
            entry(&h[7], "Cycle repeat"),
            entry(&h[8], "Search"),
            entry(&h[9], "Help"),
            entry(&h[10], "Suspend"),
            entry(&h[11], "Quit"),
        ],
    }]
}

/// Right column. Mirrors `help_sections_right` in
/// `mkp2/mkp/src/nav/help_overlay.rs`.
fn canonical_right_sections(h: &[String]) -> Vec<HelpSection> {
    vec![
        HelpSection {
            heading: "Navigation".into(),
            entries: vec![
                entry(&format!("{} / {}", h[12], h[13]), "Move cursor"),
                entry(&format!("{} / {}", h[14], h[15]), "Switch column"),
                entry(&format!("{} / {}", h[16], h[17]), "History back/fwd"),
                entry(&format!("{} / {}", h[18], h[19]), "Jump top / bottom"),
                entry(&format!("{} / {}", h[20], h[21]), "Scroll page"),
            ],
        },
        HelpSection {
            heading: "Lists".into(),
            entries: vec![
                entry(&h[22], "Play / select"),
                entry(&h[23], "Shuffle play"),
                entry(&h[24], "Actions menu"),
                entry(&h[25], "Filter"),
                entry(&h[26], "Back"),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_is_propagated() {
        let m = help_overlay_model(HelpOverlayInput::new(7, &Keybindings::defaults()));
        assert_eq!(m.scroll, 7);
    }

    #[test]
    fn cached_call_reuses_arcs() {
        let a = help_overlay_model(HelpOverlayInput::new(0, &Keybindings::defaults()));
        let b = help_overlay_model(HelpOverlayInput::new(0, &Keybindings::defaults()));
        assert!(Arc::ptr_eq(&a.left, &b.left));
        assert!(Arc::ptr_eq(&a.right, &b.right));
    }

    #[test]
    fn columns_carry_legacy_sections() {
        let m = help_overlay_model(HelpOverlayInput::new(0, &Keybindings::defaults()));
        assert_eq!(m.left.len(), 1);
        assert_eq!(m.left[0].heading, "Global");
        assert_eq!(m.right.len(), 2);
        assert_eq!(m.right[0].heading, "Navigation");
        assert_eq!(m.right[1].heading, "Lists");
    }

    #[test]
    fn reflects_active_bindings() {
        let mut keys = Keybindings::defaults();
        keys.replace(
            KeyContext::Global,
            Action::PlayPause,
            mkpclient_state_ui_keybindings::KeyChord::char('p'),
        );
        let m = help_overlay_model(HelpOverlayInput::new(0, &keys));
        assert_eq!(m.left[0].entries[0].key, "p");
    }
}
