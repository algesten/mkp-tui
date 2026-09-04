use mkpclient_state_ui_keybindings::KeyContext;
use mkpclient_state_ui_screen::KeybindingsEditorState;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeybindingsEditorAction {
    pub name: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeybindingsEditorModel {
    pub contexts: Vec<&'static str>,
    pub actions: Vec<KeybindingsEditorAction>,
    pub selected_context: usize,
    pub selected_binding: usize,
    pub listening: bool,
    pub adding: bool,
    pub focus_right: bool,
}

#[derive(drv::Input)]
pub struct KeybindingsEditorInput {
    pub actions: Vec<(String, Vec<String>)>,
    pub selected_context: usize,
    pub selected_binding: usize,
    pub listening: bool,
    pub adding: bool,
    pub focus_right: bool,
}

impl KeybindingsEditorInput {
    pub fn new(state: &KeybindingsEditorState) -> Self {
        let ctx = KeyContext::ALL[state.selected_context];
        Self {
            actions: state
                .draft
                .sorted_actions(ctx)
                .into_iter()
                .map(|action| {
                    (
                        action.display_name().to_string(),
                        state
                            .draft
                            .keys_for(ctx, action)
                            .into_iter()
                            .map(|key| key.to_string())
                            .collect(),
                    )
                })
                .collect(),
            selected_context: state.selected_context,
            selected_binding: state.selected_binding,
            listening: state.listening,
            adding: state.adding,
            focus_right: state.focus_right,
        }
    }
}

#[drv::memo(single)]
pub fn keybindings_editor_model(input: KeybindingsEditorInput) -> KeybindingsEditorModel {
    let actions = input
        .actions
        .into_iter()
        .map(|(name, keys)| KeybindingsEditorAction { name, keys })
        .collect();
    KeybindingsEditorModel {
        contexts: KeyContext::ALL
            .iter()
            .map(|ctx| ctx.display_name())
            .collect(),
        actions,
        selected_context: input.selected_context,
        selected_binding: input.selected_binding,
        listening: input.listening,
        adding: input.adding,
        focus_right: input.focus_right,
    }
}
