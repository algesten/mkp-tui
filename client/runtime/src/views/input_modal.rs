//! View model for the generic input-modal (Create / Rename Playlist).
//!
//! `Screen::CreatePlaylist` and `Screen::RenamePlaylist` share the
//! same renderer with different titles + verbs; this memo carries
//! the title and active input string.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InputModalModel {
    pub title: &'static str,
    pub hint_verb: &'static str,
    pub input: std::sync::Arc<str>,
}

/// Which renderer flavour to draw. Encoded as a small enum so the
/// memo input stays `ToStatic` and the renderer doesn't need to
/// know `Screen` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum InputModalKind {
    CreatePlaylist,
    RenamePlaylist,
}

#[derive(drv::Input)]
pub struct InputModalInput<'a> {
    pub kind_create: bool,
    pub kind_rename: bool,
    pub input: &'a std::sync::Arc<str>,
}

impl<'a> InputModalInput<'a> {
    pub fn new(kind: InputModalKind, input: &'a std::sync::Arc<str>) -> Self {
        Self {
            kind_create: matches!(kind, InputModalKind::CreatePlaylist),
            kind_rename: matches!(kind, InputModalKind::RenamePlaylist),
            input,
        }
    }
}

#[drv::memo(single)]
pub fn input_modal_model<'a>(input: InputModalInput<'a>) -> InputModalModel {
    let (title, hint_verb) = if input.kind_create {
        ("New Playlist", "create")
    } else if input.kind_rename {
        ("Rename Playlist", "rename")
    } else {
        ("Input", "submit")
    };
    InputModalModel {
        title,
        hint_verb,
        input: input.input.clone(),
    }
}
