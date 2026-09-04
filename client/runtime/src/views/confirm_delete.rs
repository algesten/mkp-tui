//! View model for the confirm-delete-playlist modal (type-to-confirm).

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfirmDeletePlaylistModel {
    pub name: std::sync::Arc<str>,
    pub input: std::sync::Arc<str>,
    pub matches: bool,
}

#[derive(drv::Input)]
pub struct ConfirmDeletePlaylistInput<'a> {
    pub name: &'a std::sync::Arc<str>,
    pub input: &'a std::sync::Arc<str>,
}

#[drv::memo(single)]
pub fn confirm_delete_playlist_model<'a>(
    input: ConfirmDeletePlaylistInput<'a>,
) -> ConfirmDeletePlaylistModel {
    ConfirmDeletePlaylistModel {
        name: input.name.clone(),
        input: input.input.clone(),
        matches: nfc_eq(input.input.trim(), input.name.trim()),
    }
}

// MusicKit can return names in NFD (e.g. `å` as `a` + U+030A) while
// keyboard input arrives precomposed, so byte-level equality would
// reject a visually-identical match.
fn nfc_eq(a: &str, b: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    a.nfc().eq(b.nfc())
}
