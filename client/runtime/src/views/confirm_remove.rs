//! View model for the confirm-remove-from-playlist modal.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfirmRemoveModel {
    pub song_title: std::sync::Arc<str>,
}

#[derive(drv::Input)]
pub struct ConfirmRemoveInput<'a> {
    pub song_title: &'a std::sync::Arc<str>,
}

#[drv::memo(single)]
pub fn confirm_remove_model<'a>(input: ConfirmRemoveInput<'a>) -> ConfirmRemoveModel {
    ConfirmRemoveModel {
        song_title: input.song_title.clone(),
    }
}
