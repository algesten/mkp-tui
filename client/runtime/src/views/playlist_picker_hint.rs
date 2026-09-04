//! View model for the playlist-picker hint (`Screen::PlaylistPicker`).
//!
//! While the playlist-picker overlay is active the *left column*
//! becomes the picker (its cursor tracks `picker.selected`); this
//! tiny hint just nudges the user to look there. The action item's
//! label feeds the "Add `<X>` to…" header so the hint reflects what
//! the user is about to add.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistPickerHintModel {
    /// Label of the row the user invoked the picker on (song title,
    /// album name, etc.). Renderer prefixes it with "Add ".
    pub item_label: std::sync::Arc<str>,
}

#[derive(drv::Input)]
pub struct PlaylistPickerHintInput<'a> {
    pub item_label: &'a std::sync::Arc<str>,
}

#[drv::memo(single)]
pub fn playlist_picker_hint_model<'a>(
    input: PlaylistPickerHintInput<'a>,
) -> PlaylistPickerHintModel {
    PlaylistPickerHintModel {
        item_label: input.item_label.clone(),
    }
}
