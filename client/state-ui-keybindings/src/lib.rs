//! User-decision source for configurable TUI keybindings.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyChord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f: Option<u8>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl KeyChord {
    pub fn char(c: char) -> Self {
        Self::char_mod(c, false, false, false)
    }

    pub fn char_mod(c: char, ctrl: bool, alt: bool, shift: bool) -> Self {
        Self {
            char: Some(c.to_string()),
            key: None,
            f: None,
            ctrl,
            alt,
            shift,
        }
    }

    pub fn named(name: &str) -> Self {
        Self::named_mod(name, false, false, false)
    }

    pub fn named_mod(name: &str, ctrl: bool, alt: bool, shift: bool) -> Self {
        Self {
            char: None,
            key: Some(name.to_string()),
            f: None,
            ctrl,
            alt,
            shift,
        }
    }

    pub fn func(n: u8, ctrl: bool, alt: bool, shift: bool) -> Self {
        Self {
            char: None,
            key: None,
            f: Some(n),
            ctrl,
            alt,
            shift,
        }
    }

    pub fn is_text_key(&self) -> bool {
        self.char.is_some() && !self.ctrl && !self.alt
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            match self.char.as_deref().and_then(|c| c.chars().next()) {
                Some(c) if c.is_ascii_lowercase() => {}
                _ => parts.push("Shift".to_string()),
            }
        }
        let key = if let Some(c) = self.char.as_deref().and_then(|c| c.chars().next()) {
            if self.shift && c.is_ascii_lowercase() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        } else if let Some(name) = self.key.as_deref() {
            match name {
                "enter" => "Enter",
                "esc" => "Esc",
                "tab" => "Tab",
                "backtab" => "BackTab",
                "space" => "Space",
                "backspace" => "Backspace",
                "delete" => "Delete",
                "up" => "↑",
                "down" => "↓",
                "left" => "←",
                "right" => "→",
                "home" => "Home",
                "end" => "End",
                "pageup" => "PgUp",
                "pagedown" => "PgDn",
                "insert" => "Insert",
                other => other,
            }
            .to_string()
        } else if let Some(n) = self.f {
            format!("F{n}")
        } else {
            "?".into()
        };
        parts.push(key);
        write!(f, "{}", parts.join(" + "))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    Suspend,
    PlayPause,
    NextTrack,
    PreviousTrack,
    SeekForward10s,
    SeekBackward10s,
    SeekForward1s,
    SeekBackward1s,
    CycleRepeat,
    OpenSearch,
    ToggleHelp,
    ToggleFilter,
    HistoryBack,
    HistoryForward,
    EnterSelectionMode,
    MoveUp,
    MoveDown,
    MoveToTop,
    MoveToBottom,
    PageUp,
    PageDown,
    FocusLeft,
    FocusRight,
    Activate,
    ShuffleActivate,
    OpenActionMenu,
    Back,
    ActionGoToArtist,
    ActionGoToAlbum,
    ActionPlayNext,
    ActionPlayLast,
    ActionAddToPlaylist,
    ActionCopyLink,
    ActionRemove,
    CloseActionModal,
    PlaylistActionRename,
    PlaylistActionDelete,
    ClosePlaylistActionModal,
    SelectAndMoveDown,
    DeselectCurrent,
    ToggleRangeAnchor,
    PlaySelection,
    OpenSelectionActionMenu,
    CancelSelection,
    SelectionPlayNext,
    SelectionPlayLast,
    SelectionAddToPlaylist,
    SelectionDelete,
    CloseSelectionActionModal,
    CloseError,
    CopyError,
    ScrollHelpUp,
    ScrollHelpDown,
    ScrollHelpPageUp,
    ScrollHelpPageDown,
    ScrollHelpTop,
    CloseHelp,
    OpenKeybindingsEditor,
    CycleSearchType,
    CycleSearchTypePrev,
    EditHistoryItem,
    ExecuteSearch,
    CloseSearch,
    ServerPickerSelect,
    CloseServerPicker,
    DiscoveringSelect,
    DiscoveringQuit,
    ConfirmInput,
    CancelInput,
    ServerLostConfirm,
}

impl Action {
    pub fn display_name(self) -> &'static str {
        use Action::*;
        match self {
            Quit => "Quit",
            Suspend => "Suspend",
            PlayPause => "Play / Pause",
            NextTrack => "Next Track",
            PreviousTrack => "Previous Track",
            SeekForward10s => "Seek +10s",
            SeekBackward10s => "Seek -10s",
            SeekForward1s => "Seek +1s",
            SeekBackward1s => "Seek -1s",
            CycleRepeat => "Cycle Repeat",
            OpenSearch => "Search",
            ToggleHelp => "Help",
            ToggleFilter => "Filter",
            HistoryBack => "History Back",
            HistoryForward => "History Forward",
            EnterSelectionMode => "Selection Mode",
            MoveUp => "Move Up",
            MoveDown => "Move Down",
            MoveToTop => "Move to Top",
            MoveToBottom => "Move to Bottom",
            PageUp => "Page Up",
            PageDown => "Page Down",
            FocusLeft => "Focus Left",
            FocusRight => "Focus Right",
            Activate => "Activate",
            ShuffleActivate => "Shuffle Play",
            OpenActionMenu => "Actions Menu",
            Back => "Back",
            ActionGoToArtist => "Go to Artist",
            ActionGoToAlbum => "Go to Album",
            ActionPlayNext | SelectionPlayNext => "Play Next",
            ActionPlayLast | SelectionPlayLast => "Play Last",
            ActionAddToPlaylist | SelectionAddToPlaylist => "Add to Playlist",
            ActionCopyLink => "Copy Link",
            ActionRemove | SelectionDelete => "Remove",
            CloseActionModal | ClosePlaylistActionModal | CloseSelectionActionModal => "Close",
            PlaylistActionRename => "Rename Playlist",
            PlaylistActionDelete => "Delete Playlist",
            SelectAndMoveDown => "Select & Move Down",
            DeselectCurrent => "Deselect",
            ToggleRangeAnchor => "Toggle Range",
            PlaySelection => "Play Selection",
            OpenSelectionActionMenu => "Selection Actions",
            CancelSelection => "Cancel Selection",
            CloseError => "Close",
            CopyError => "Copy Error",
            ScrollHelpUp => "Scroll Up",
            ScrollHelpDown => "Scroll Down",
            ScrollHelpPageUp => "Scroll Page Up",
            ScrollHelpPageDown => "Scroll Page Down",
            ScrollHelpTop => "Scroll to Top",
            CloseHelp => "Close Help",
            OpenKeybindingsEditor => "Configure Keys",
            CycleSearchType => "Next Type",
            CycleSearchTypePrev => "Previous Type",
            EditHistoryItem => "Edit History",
            ExecuteSearch => "Search",
            CloseSearch => "Close Search",
            ServerPickerSelect => "Select Server",
            CloseServerPicker => "Close",
            DiscoveringSelect => "Connect",
            DiscoveringQuit => "Quit",
            ConfirmInput => "Confirm",
            CancelInput => "Cancel",
            ServerLostConfirm => "Server Selection",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyContext {
    Move,
    Global,
    ListNavigation,
    ActionModal,
    PlaylistActionModal,
    SelectionMode,
    SelectionActionModal,
    ErrorModal,
    HelpOverlay,
    SearchInput,
    ServerPicker,
    Discovering,
    TextInput,
    ServerLost,
}

impl KeyContext {
    pub const ALL: &'static [Self] = &[
        Self::Move,
        Self::Global,
        Self::ListNavigation,
        Self::ActionModal,
        Self::PlaylistActionModal,
        Self::SelectionMode,
        Self::SelectionActionModal,
        Self::ErrorModal,
        Self::SearchInput,
        Self::ServerPicker,
        Self::Discovering,
        Self::TextInput,
        Self::ServerLost,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Global => "Global",
            Self::ListNavigation => "List Navigation",
            Self::ActionModal => "Action Modal",
            Self::PlaylistActionModal => "Playlist Action",
            Self::SelectionMode => "Selection Mode",
            Self::SelectionActionModal => "Selection Action",
            Self::ErrorModal => "Error Modal",
            Self::HelpOverlay => "Help Overlay",
            Self::SearchInput => "Search Input",
            Self::ServerPicker => "Server Picker",
            Self::Discovering => "Discovering",
            Self::TextInput => "Text Input",
            Self::ServerLost => "Server Lost",
        }
    }

    fn toml_key(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Global => "global",
            Self::ListNavigation => "list_navigation",
            Self::ActionModal => "action_modal",
            Self::PlaylistActionModal => "playlist_action_modal",
            Self::SelectionMode => "selection_mode",
            Self::SelectionActionModal => "selection_action_modal",
            Self::ErrorModal => "error_modal",
            Self::HelpOverlay => "help_overlay",
            Self::SearchInput => "search_input",
            Self::ServerPicker => "server_picker",
            Self::Discovering => "discovering",
            Self::TextInput => "text_input",
            Self::ServerLost => "server_lost",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(KeyChord),
    Many(Vec<KeyChord>),
}

#[derive(Serialize, Deserialize)]
struct ActionValue {
    action: Action,
}

fn action_from_name(name: &str) -> Option<Action> {
    let mut table = toml::map::Map::new();
    table.insert("action".into(), toml::Value::String(name.into()));
    toml::Value::Table(table)
        .try_into::<ActionValue>()
        .ok()
        .map(|v| v.action)
}

fn action_name(action: Action) -> String {
    toml::Value::try_from(ActionValue { action })
        .ok()
        .and_then(|v| {
            v.get("action")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        })
        .expect("Action always serializes as a string")
}

fn chord_toml(key: &KeyChord) -> String {
    let mut fields = Vec::new();
    if let Some(value) = &key.char {
        fields.push(format!("char = {}", toml::Value::String(value.clone())));
    }
    if let Some(value) = &key.key {
        fields.push(format!("key = {}", toml::Value::String(value.clone())));
    }
    if let Some(value) = key.f {
        fields.push(format!("f = {value}"));
    }
    if key.ctrl {
        fields.push("ctrl = true".into());
    }
    if key.alt {
        fields.push("alt = true".into());
    }
    if key.shift {
        fields.push("shift = true".into());
    }
    format!("{{ {} }}", fields.join(", "))
}

fn bindings_toml(keys: &[KeyChord]) -> String {
    if keys.len() == 1 {
        chord_toml(&keys[0])
    } else {
        format!(
            "[{}]",
            keys.iter().map(chord_toml).collect::<Vec<_>>().join(", ")
        )
    }
}

impl OneOrMany {
    fn into_vec(self) -> Vec<KeyChord> {
        match self {
            Self::One(k) => vec![k],
            Self::Many(v) => v,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybindings {
    pub map: HashMap<KeyContext, HashMap<Action, Vec<KeyChord>>>,
    reverse: HashMap<KeyContext, HashMap<KeyChord, Action>>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Keybindings {
    fn from_map(map: HashMap<KeyContext, HashMap<Action, Vec<KeyChord>>>) -> Self {
        let mut this = Self {
            map,
            reverse: HashMap::new(),
        };
        this.rebuild_reverse();
        this
    }

    pub fn rebuild_reverse(&mut self) {
        self.reverse = self
            .map
            .iter()
            .map(|(&ctx, actions)| {
                let reverse = actions
                    .iter()
                    .flat_map(|(&action, keys)| keys.iter().cloned().map(move |key| (key, action)))
                    .collect();
                (ctx, reverse)
            })
            .collect();
    }

    pub fn defaults() -> Self {
        use Action::*;
        let mut map = HashMap::new();
        let rows: Vec<(KeyContext, Action, Vec<KeyChord>)> = vec![
            (
                KeyContext::Move,
                MoveUp,
                vec![KeyChord::named("up"), KeyChord::char('k')],
            ),
            (
                KeyContext::Move,
                MoveDown,
                vec![KeyChord::named("down"), KeyChord::char('j')],
            ),
            (
                KeyContext::Move,
                FocusLeft,
                vec![KeyChord::named("left"), KeyChord::char('h')],
            ),
            (
                KeyContext::Move,
                FocusRight,
                vec![KeyChord::named("right"), KeyChord::char('l')],
            ),
            (
                KeyContext::Move,
                MoveToTop,
                vec![KeyChord::named("home"), KeyChord::char('g')],
            ),
            (
                KeyContext::Move,
                MoveToBottom,
                vec![
                    KeyChord::named("end"),
                    KeyChord::char_mod('g', false, false, true),
                ],
            ),
            (KeyContext::Move, PageUp, vec![KeyChord::named("pageup")]),
            (
                KeyContext::Move,
                PageDown,
                vec![KeyChord::named("pagedown")],
            ),
            (
                KeyContext::Global,
                Quit,
                vec![KeyChord::char_mod('c', true, false, false)],
            ),
            (
                KeyContext::Global,
                Suspend,
                vec![KeyChord::char_mod('z', true, false, false)],
            ),
            (
                KeyContext::Global,
                PlayPause,
                vec![KeyChord::named("space")],
            ),
            (KeyContext::Global, NextTrack, vec![KeyChord::char(']')]),
            (KeyContext::Global, PreviousTrack, vec![KeyChord::char('[')]),
            (
                KeyContext::Global,
                SeekForward10s,
                vec![KeyChord::char('}')],
            ),
            (
                KeyContext::Global,
                SeekBackward10s,
                vec![KeyChord::char('{')],
            ),
            (
                KeyContext::Global,
                SeekForward1s,
                vec![KeyChord::char_mod('}', false, true, false)],
            ),
            (
                KeyContext::Global,
                SeekBackward1s,
                vec![KeyChord::char_mod('{', false, true, false)],
            ),
            (KeyContext::Global, CycleRepeat, vec![KeyChord::char('.')]),
            (
                KeyContext::Global,
                OpenSearch,
                vec![KeyChord::char_mod('s', false, false, true)],
            ),
            (KeyContext::Global, ToggleHelp, vec![KeyChord::char('?')]),
            (
                KeyContext::Global,
                ToggleFilter,
                vec![KeyChord::char_mod('f', false, false, true)],
            ),
            (
                KeyContext::Global,
                HistoryBack,
                vec![KeyChord::named_mod("left", false, false, true)],
            ),
            (
                KeyContext::Global,
                HistoryForward,
                vec![KeyChord::named_mod("right", false, false, true)],
            ),
            (
                KeyContext::Global,
                EnterSelectionMode,
                vec![KeyChord::char_mod('m', false, false, true)],
            ),
        ];
        for (ctx, action, keys) in rows {
            map.entry(ctx)
                .or_insert_with(HashMap::new)
                .insert(action, keys);
        }
        macro_rules! context { ($ctx:ident: $($action:ident => [$($key:expr),+]),+ $(,)?) => {{ let m = map.entry(KeyContext::$ctx).or_insert_with(HashMap::new); $(m.insert($action, vec![$($key),+]);)+ }} }
        context!(ListNavigation: Activate => [KeyChord::named("enter")], ShuffleActivate => [KeyChord::named_mod("enter", false, true, false)], OpenActionMenu => [KeyChord::named("tab")], Back => [KeyChord::named("esc")]);
        context!(ActionModal: Activate => [KeyChord::named("enter")], ActionGoToArtist => [KeyChord::char('q')], ActionGoToAlbum => [KeyChord::char('w')], ActionPlayNext => [KeyChord::char('n')], ActionPlayLast => [KeyChord::char('e')], ActionAddToPlaylist => [KeyChord::char('a')], ActionCopyLink => [KeyChord::char('c')], ActionRemove => [KeyChord::char('d')], CloseActionModal => [KeyChord::named("esc"), KeyChord::named("tab")]);
        context!(PlaylistActionModal: Activate => [KeyChord::named("enter")], PlaylistActionRename => [KeyChord::char('r')], PlaylistActionDelete => [KeyChord::char('d')], ClosePlaylistActionModal => [KeyChord::named("esc"), KeyChord::named("tab")]);
        context!(SelectionMode: SelectAndMoveDown => [KeyChord::named("right"), KeyChord::char('l')], DeselectCurrent => [KeyChord::named("left"), KeyChord::char('h')], ToggleRangeAnchor => [KeyChord::named("space")], PlaySelection => [KeyChord::named("enter")], OpenSelectionActionMenu => [KeyChord::named("tab")], CancelSelection => [KeyChord::named("esc")]);
        context!(SelectionActionModal: Activate => [KeyChord::named("enter")], SelectionPlayNext => [KeyChord::char('n')], SelectionPlayLast => [KeyChord::char('e')], SelectionAddToPlaylist => [KeyChord::char('a')], SelectionDelete => [KeyChord::char('d')], CloseSelectionActionModal => [KeyChord::named("esc"), KeyChord::named("tab")]);
        context!(ErrorModal: CloseError => [KeyChord::named("esc")], CopyError => [KeyChord::char('c')]);
        context!(HelpOverlay: CloseHelp => [KeyChord::named("esc"), KeyChord::char('?')], ScrollHelpUp => [KeyChord::named("up"), KeyChord::char('k')], ScrollHelpDown => [KeyChord::named("down"), KeyChord::char('j')], ScrollHelpPageUp => [KeyChord::named("pageup")], ScrollHelpPageDown => [KeyChord::named("pagedown")], ScrollHelpTop => [KeyChord::named("home")], OpenKeybindingsEditor => [KeyChord::char('c')]);
        context!(SearchInput: CloseSearch => [KeyChord::named("esc")], CycleSearchType => [KeyChord::named("tab")], CycleSearchTypePrev => [KeyChord::named("backtab")], EditHistoryItem => [KeyChord::char('e')], ExecuteSearch => [KeyChord::named("enter")]);
        context!(ServerPicker: ServerPickerSelect => [KeyChord::named("enter")], CloseServerPicker => [KeyChord::named("esc")]);
        context!(Discovering: DiscoveringSelect => [KeyChord::named("enter")], DiscoveringQuit => [KeyChord::named("esc")]);
        context!(TextInput: ConfirmInput => [KeyChord::named("enter")], CancelInput => [KeyChord::named("esc")]);
        context!(ServerLost: ServerLostConfirm => [KeyChord::named("enter")]);
        Self::from_map(map)
    }

    pub fn action_for(&self, ctx: KeyContext, key: &KeyChord) -> Option<Action> {
        self.reverse
            .get(&ctx)
            .and_then(|m| m.get(key))
            .copied()
            .or_else(|| {
                (ctx != KeyContext::Move)
                    .then(|| {
                        self.reverse
                            .get(&KeyContext::Move)
                            .and_then(|m| m.get(key))
                            .copied()
                    })
                    .flatten()
            })
            .or_else(|| {
                (!matches!(ctx, KeyContext::Move | KeyContext::Global))
                    .then(|| {
                        self.reverse
                            .get(&KeyContext::Global)
                            .and_then(|m| m.get(key))
                            .copied()
                    })
                    .flatten()
            })
    }

    pub fn action_for_text_input(&self, ctx: KeyContext, key: &KeyChord) -> Option<Action> {
        self.reverse
            .get(&ctx)
            .and_then(|m| m.get(key))
            .copied()
            .or_else(|| {
                (!key.is_text_key())
                    .then(|| self.action_for(ctx, key))
                    .flatten()
            })
    }

    pub fn keys_for(&self, ctx: KeyContext, action: Action) -> Vec<KeyChord> {
        self.map
            .get(&ctx)
            .and_then(|m| m.get(&action))
            .cloned()
            .unwrap_or_default()
    }

    pub fn hint_for(&self, ctx: KeyContext, action: Action) -> String {
        self.keys_for(ctx, action)
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "?".into())
    }

    pub fn merge_toml(content: &str) -> Result<Self, toml::de::Error> {
        let file: HashMap<String, HashMap<String, OneOrMany>> = toml::from_str(content)?;
        let mut result = Self::defaults();
        for ctx in KeyContext::ALL {
            if let Some(actions) = file.get(ctx.toml_key()) {
                for (name, keys) in actions {
                    if let Some(action) = action_from_name(name) {
                        result
                            .map
                            .entry(*ctx)
                            .or_default()
                            .insert(action, keys.clone().into_vec());
                    }
                }
            }
        }
        result.rebuild_reverse();
        Ok(result)
    }

    pub fn to_toml(&self) -> String {
        let mut output = String::from("# MkPlay Keybindings\n# Key names for 'key': enter, esc, tab, backtab, space, backspace, delete,\n#   up, down, left, right, home, end, pageup, pagedown\n# Character keys use 'char' instead: char = \"q\"\n# Modifiers: ctrl = true, alt = true, shift = true\n# Multiple keys: action = [{ char = \"k\" }, { key = \"up\" }]\n\n");
        for ctx in KeyContext::ALL {
            if let Some(actions) = self.map.get(ctx) {
                output.push_str(&format!("[{}]\n", ctx.toml_key()));
                let mut actions: Vec<_> = actions.iter().collect();
                actions.sort_by_key(|(action, _)| format!("{action:?}"));
                for (action, keys) in actions {
                    let name = action_name(*action);
                    let value = bindings_toml(keys);
                    output.push_str(&format!("{name} = {value}\n"));
                }
                output.push('\n');
            }
        }
        output
    }

    pub fn sorted_actions(&self, ctx: KeyContext) -> Vec<Action> {
        let mut actions: Vec<_> = self
            .map
            .get(&ctx)
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default();
        actions.sort_by_key(|a| a.display_name());
        actions
    }

    pub fn replace(&mut self, ctx: KeyContext, action: Action, key: KeyChord) {
        self.map.entry(ctx).or_default().insert(action, vec![key]);
        self.rebuild_reverse();
    }

    pub fn add(&mut self, ctx: KeyContext, action: Action, key: KeyChord) {
        self.map
            .entry(ctx)
            .or_default()
            .entry(action)
            .or_default()
            .push(key);
        self.rebuild_reverse();
    }

    pub fn reset(&mut self, ctx: KeyContext, action: Action) {
        if let Some(keys) = Self::defaults()
            .map
            .get(&ctx)
            .and_then(|m| m.get(&action))
            .cloned()
        {
            self.map.entry(ctx).or_default().insert(action, keys);
            self.rebuild_reverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_and_merge_keep_defaults() {
        let mut keys = Keybindings::defaults();
        keys.replace(KeyContext::Global, Action::PlayPause, KeyChord::char('p'));
        let loaded = Keybindings::merge_toml(&keys.to_toml()).unwrap();
        assert_eq!(
            loaded.keys_for(KeyContext::Global, Action::PlayPause),
            vec![KeyChord::char('p')]
        );
        assert!(!loaded.keys_for(KeyContext::Move, Action::MoveUp).is_empty());
    }
}
