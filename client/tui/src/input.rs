//! Crossterm input → `DispatchEvent` translator + a dedicated reader
//! thread that posts `UiInput` onto an mpsc.
//!
//! The translator is the *only* TUI module that knows about
//! `crossterm::KeyCode`. Everything below this layer (dispatch
//! handlers, queries, view memos) is plain Rust state mutation.
//! `runtime/src/dispatch.rs` stays crossterm-free.
//!
//! All persist + auto-connect + lost-modal + saved-view-restore
//! logic lives in `runtime/src/lifecycle/` (memo pairs + dispatch
//! trampolines); this translator only converts key events into the
//! corresponding `DispatchEvent`.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mkpclient_runtime::{Notifier, Runtime, SemanticEvent, TuiCursorEvent};
use mkpclient_state_link::LinkPhase;
use mkpclient_state_pairing::PairingPhase;
use mkpclient_state_ui_keybindings::{Action, KeyChord, KeyContext, Keybindings};
use mkpclient_state_ui_screen::{ActionKind, Screen};

use crate::app::AppState;

#[derive(Debug, Clone)]
pub enum UiInput {
    Key(KeyCode, KeyModifiers, KeyEventKind),
    Resize,
}

pub struct InputHandle {
    rx: Receiver<UiInput>,
}

impl InputHandle {
    pub fn try_next(&self) -> Option<UiInput> {
        match self.rx.try_recv() {
            Ok(ev) => Some(ev),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

/// Spawn a dedicated thread that blocks on `crossterm::event::read`
/// and forwards every key / resize as a `UiInput`. After each send
/// we `notify` the runtime's wake channel so `wait_for_wake`
/// unblocks immediately — otherwise keystrokes queue up for up to
/// the wait timeout and feel unresponsive.
pub fn spawn_input_thread(notify: Notifier) -> InputHandle {
    let (tx, rx) = mpsc::channel::<UiInput>();
    thread::Builder::new()
        .name("mkptui-input".into())
        .spawn(move || loop {
            match event::read() {
                Ok(Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                })) => {
                    if tx.send(UiInput::Key(code, modifiers, kind)).is_err() {
                        return;
                    }
                    notify.notify();
                }
                Ok(Event::Resize(_, _)) => {
                    if tx.send(UiInput::Resize).is_err() {
                        return;
                    }
                    notify.notify();
                }
                Ok(_) => {}
                Err(_) => return,
            }
        })
        .expect("spawning input thread");
    InputHandle { rx }
}

// ─── translator ─────────────────────────────────────────────────────

/// Translate one `UiInput` into `DispatchEvent`(s) and dispatch
/// them. Returns `true` when the user asked to quit.
pub fn translate(ev: UiInput, rt: &mut Runtime, app: &mut AppState) -> bool {
    let UiInput::Key(code, mods, kind) = ev else {
        return false;
    };

    // Release events are never commands. For next/previous, also
    // suppress OS key-repeat events so a held transport key cannot
    // flood the server; separate Press events remain lossless.
    if should_ignore_key(code, mods, kind, &rt.sources.keybindings) {
        return false;
    }

    // Always-on: Ctrl-C quits.
    if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
        return true;
    }

    // Legacy app-level suspend remains always available, even if the
    // configurable action is rebound.
    let suspend = is_suspend_key(code, mods, &rt.sources.screen, &rt.sources.keybindings);
    if suspend {
        app.suspend_requested = true;
        return false;
    }

    if matches!(rt.sources.screen, Screen::NowPlaying)
        && matches!(
            action(rt, KeyContext::Global, code, mods),
            Some(Action::Quit)
        )
    {
        return true;
    }

    // Pairing confirmation modal wins while it's up.
    if rt.sources.pairing.phase == PairingPhase::AwaitingConfirmation {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => rt.dispatch(SemanticEvent::ConfirmPair),
            KeyCode::Char('n') => rt.dispatch(SemanticEvent::RejectPair),
            _ => {}
        }
        return false;
    }

    // The reconnect modal owns its keys even though the link is down —
    // it is shown *instead of* the pre-connect picker, and its legend
    // ("Enter=pick another · Esc=keep waiting") means nothing if the
    // picker handler answers instead. Esc there is `DiscoveringQuit`,
    // which would exit the application.
    if matches!(rt.sources.screen, Screen::ServerLostModal { .. }) {
        translate_server_lost_modal(code, mods, rt);
        return false;
    }

    // Pre-connect: server picker.
    if rt.sources.link.phase != LinkPhase::Connected {
        return translate_picker(code, mods, rt);
    }

    // Selection mode takes priority over the regular pane handlers
    // — its keymap is mostly orthogonal and swallows most input.
    if rt.sources.selection.is_active() && matches!(rt.sources.screen, Screen::NowPlaying) {
        translate_selection_mode(code, mods, rt);
        return false;
    }

    // Snapshot history-stack lengths so the offset-stack reconciler
    // can match the post-dispatch transition (drill / back / forward
    // / cleared) without each handler having to manipulate
    // AppState's offset stacks itself.
    let history_before = crate::history_offsets::HistoryLens::from_runtime(rt);

    match &rt.sources.screen {
        Screen::SearchInput(_) => translate_search_input(code, mods, rt),
        Screen::ActionModal(_) => translate_action_modal(code, mods, rt),
        Screen::FilterInput(_) => translate_filter_input(code, mods, rt),
        Screen::HelpOverlay { .. } => translate_help_overlay(code, mods, rt),
        Screen::KeybindingsEditor(_) => translate_keybindings_editor(code, mods, rt),
        Screen::CreatePlaylist { .. } => translate_create_playlist(code, mods, rt),
        Screen::RenamePlaylist { .. } => translate_rename_playlist(code, mods, rt),
        Screen::PlaylistAction { .. } => translate_playlist_action(code, mods, rt),
        Screen::ConfirmDeletePlaylist { .. } => translate_confirm_delete_playlist(code, mods, rt),
        Screen::PlaylistPicker { .. } => translate_playlist_picker(code, mods, rt),
        Screen::ConfirmRemoveFromPlaylist { .. } => translate_confirm_remove(code, mods, rt),
        Screen::SelectionActionModal { .. } => translate_selection_action_modal(code, mods, rt),
        Screen::ErrorModal { .. } => translate_error_modal(code, mods, rt),
        Screen::ServerLostModal { .. } => translate_server_lost_modal(code, mods, rt),
        Screen::ServerPicker { .. } => translate_server_picker_modal(code, mods, rt),
        Screen::NowPlaying => translate_now_playing(code, mods, rt),
    }

    crate::history_offsets::reconcile(history_before, rt, app);

    false
}

fn key_chord(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    match code {
        KeyCode::Char(' ') => KeyChord::named_mod("space", ctrl, alt, shift),
        KeyCode::Char(c) if c.is_ascii_uppercase() => {
            KeyChord::char_mod(c.to_ascii_lowercase(), ctrl, alt, true)
        }
        KeyCode::Char(c) => KeyChord::char_mod(c, ctrl, alt, c.is_ascii_lowercase() && shift),
        KeyCode::F(n) => KeyChord::func(n, ctrl, alt, shift),
        other => KeyChord::named_mod(
            match other {
                KeyCode::Enter => "enter",
                KeyCode::Esc => "esc",
                KeyCode::Tab => "tab",
                KeyCode::BackTab => "backtab",
                KeyCode::Backspace => "backspace",
                KeyCode::Delete => "delete",
                KeyCode::Up => "up",
                KeyCode::Down => "down",
                KeyCode::Left => "left",
                KeyCode::Right => "right",
                KeyCode::Home => "home",
                KeyCode::End => "end",
                KeyCode::PageUp => "pageup",
                KeyCode::PageDown => "pagedown",
                KeyCode::Insert => "insert",
                _ => "unknown",
            },
            ctrl,
            alt,
            shift,
        ),
    }
}

fn action(rt: &Runtime, ctx: KeyContext, code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    rt.sources
        .keybindings
        .action_for(ctx, &key_chord(code, mods))
}

fn text_action(rt: &Runtime, ctx: KeyContext, code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    rt.sources
        .keybindings
        .action_for_text_input(ctx, &key_chord(code, mods))
}

fn is_suspend_key(code: KeyCode, mods: KeyModifiers, screen: &Screen, keys: &Keybindings) -> bool {
    (code == KeyCode::Char('z') && mods.contains(KeyModifiers::CONTROL))
        || (matches!(screen, Screen::NowPlaying)
            && matches!(
                keys.action_for(KeyContext::Global, &key_chord(code, mods)),
                Some(Action::Suspend)
            ))
}

fn should_ignore_key(
    code: KeyCode,
    mods: KeyModifiers,
    kind: KeyEventKind,
    keys: &Keybindings,
) -> bool {
    kind == KeyEventKind::Release
        || (kind == KeyEventKind::Repeat
            && matches!(
                keys.action_for(KeyContext::Global, &key_chord(code, mods)),
                Some(Action::NextTrack | Action::PreviousTrack)
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_skip_presses_are_kept_but_repeats_are_ignored() {
        let keys = Keybindings::defaults();
        for code in [KeyCode::Char(']'), KeyCode::Char('[')] {
            assert!(!should_ignore_key(
                code,
                KeyModifiers::NONE,
                KeyEventKind::Press,
                &keys
            ));
            assert!(should_ignore_key(
                code,
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
                &keys
            ));
        }
        assert!(!should_ignore_key(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
            &keys
        ));
        assert!(should_ignore_key(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
            &keys
        ));
    }

    #[test]
    fn suspend_keeps_ctrl_z_and_honours_a_rebinding_on_now_playing() {
        let mut keys = Keybindings::defaults();
        keys.replace(KeyContext::Global, Action::Suspend, KeyChord::char('v'));
        assert!(is_suspend_key(
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
            &Screen::HelpOverlay { scroll: 0 },
            &keys
        ));
        assert!(is_suspend_key(
            KeyCode::Char('v'),
            KeyModifiers::NONE,
            &Screen::NowPlaying,
            &keys
        ));
        assert!(!is_suspend_key(
            KeyCode::Char('v'),
            KeyModifiers::NONE,
            &Screen::SearchInput(mkpclient_state_ui_screen::SearchState::default()),
            &keys
        ));
    }
}

fn translate_picker(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) -> bool {
    if matches!(
        action(rt, KeyContext::Discovering, code, mods),
        Some(Action::DiscoveringQuit)
    ) {
        return true;
    }
    match action(rt, KeyContext::Discovering, code, mods) {
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::PickerCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::PickerCursorUp),
        Some(Action::DiscoveringSelect) => rt.dispatch(TuiCursorEvent::PickerConnect),
        _ => {}
    }
    false
}

fn translate_action_modal(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::ActionModal, code, mods) {
        Some(Action::CloseActionModal) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::ActionModalCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::ActionModalCursorUp),
        Some(Action::Activate) => {
            // Translate Enter to the highlighted row's char so the
            // dispatch handler can stay key-driven.
            let chosen = match &rt.sources.screen {
                Screen::ActionModal(s) => s.menu().get(s.selected).map(|(c, _)| *c),
                _ => None,
            };
            if let Some(c) = chosen {
                handle_action_choice(rt, c);
            }
        }
        Some(action) => {
            if let Some(c) = action_choice(action) {
                handle_action_choice(rt, c);
            }
        }
        _ => {}
    }
}

fn action_choice(action: Action) -> Option<char> {
    Some(match action {
        Action::ActionPlayNext => 'n',
        Action::ActionPlayLast => 'e',
        Action::ActionAddToPlaylist => 'a',
        Action::ActionRemove => 'd',
        Action::ActionGoToArtist => 'q',
        Action::ActionGoToAlbum => 'w',
        Action::ActionCopyLink => 'c',
        _ => return None,
    })
}

fn handle_action_choice(rt: &mut Runtime, choice: char) {
    // 'c' is Copy Link — read the URL out of the action modal,
    // dispatch a ClipboardCopy that the clipboard driver will fulfil
    // off-thread, and close the modal. The toast on success is fired
    // by the clipboard-toast lifecycle once the worker reports back.
    if choice == 'c' {
        let url = match &rt.sources.screen {
            Screen::ActionModal(s) => s.item.url.clone(),
            _ => None,
        };
        if let Some(url) = url {
            rt.dispatch(SemanticEvent::ClipboardCopy {
                text: url.to_string(),
                success_toast: "Link copied to clipboard".into(),
            });
        }
        rt.dispatch(TuiCursorEvent::CloseModal);
        return;
    }
    rt.dispatch(TuiCursorEvent::ApplyActionChoice(choice));
}

fn translate_help_overlay(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::HelpOverlay, code, mods) {
        Some(Action::CloseHelp) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::ScrollHelpDown) => rt.dispatch(TuiCursorEvent::HelpScroll(1)),
        Some(Action::ScrollHelpUp) => rt.dispatch(TuiCursorEvent::HelpScroll(-1)),
        Some(Action::ScrollHelpPageDown) => rt.dispatch(TuiCursorEvent::HelpScroll(10)),
        Some(Action::ScrollHelpPageUp) => rt.dispatch(TuiCursorEvent::HelpScroll(-10)),
        Some(Action::ScrollHelpTop) => rt.dispatch(TuiCursorEvent::HelpScrollHome),
        Some(Action::OpenKeybindingsEditor) => rt.dispatch(TuiCursorEvent::OpenKeybindingsEditor),
        _ => {}
    }
}

fn translate_keybindings_editor(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    let capturing =
        matches!(&rt.sources.screen, Screen::KeybindingsEditor(s) if s.listening || s.adding);
    if capturing {
        if code == KeyCode::Esc {
            if let Screen::KeybindingsEditor(state) = &mut rt.sources.screen {
                state.listening = false;
                state.adding = false;
            }
        } else {
            rt.dispatch(TuiCursorEvent::KeybindingsEditorBind(key_chord(code, mods)));
        }
        return;
    }
    match code {
        KeyCode::Esc => rt.dispatch(TuiCursorEvent::CloseKeybindingsEditor),
        KeyCode::Up | KeyCode::Char('k') => rt.dispatch(TuiCursorEvent::KeybindingsEditorType('k')),
        KeyCode::Down | KeyCode::Char('j') => {
            rt.dispatch(TuiCursorEvent::KeybindingsEditorType('j'))
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            rt.dispatch(TuiCursorEvent::KeybindingsEditorType('l'))
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
            rt.dispatch(TuiCursorEvent::KeybindingsEditorType('h'))
        }
        KeyCode::Char(c @ ('a' | 'd')) => rt.dispatch(TuiCursorEvent::KeybindingsEditorType(c)),
        KeyCode::Char('s') => rt.dispatch(TuiCursorEvent::KeybindingsEditorSave),
        KeyCode::Home => rt.dispatch(TuiCursorEvent::KeybindingsEditorType('g')),
        KeyCode::End => rt.dispatch(TuiCursorEvent::KeybindingsEditorType('G')),
        KeyCode::Enter => {
            let selected = match &rt.sources.screen {
                Screen::KeybindingsEditor(state) if state.focus_right => {
                    let ctx = KeyContext::ALL[state.selected_context];
                    state
                        .draft
                        .sorted_actions(ctx)
                        .get(state.selected_binding)
                        .copied()
                }
                _ => None,
            };
            if let Some(action) = selected {
                rt.dispatch(TuiCursorEvent::KeybindingsEditorSelect(action));
            }
        }
        _ => {}
    }
}

fn translate_filter_input(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match text_action(rt, KeyContext::TextInput, code, mods) {
        Some(Action::CancelInput) => rt.dispatch(TuiCursorEvent::FilterInputCancel),
        Some(Action::ConfirmInput) => rt.dispatch(TuiCursorEvent::FilterInputSubmit),
        _ => match code {
            KeyCode::Backspace => rt.dispatch(TuiCursorEvent::FilterInputBackspace),
            KeyCode::Char(c) => rt.dispatch(TuiCursorEvent::FilterInputType(c)),
            _ => {}
        },
    }
}

fn translate_search_input(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    let history_empty = match &rt.sources.screen {
        Screen::SearchInput(s) => s.history.is_empty(),
        _ => true,
    };
    let history_selected = match &rt.sources.screen {
        Screen::SearchInput(s) => s.history_selected,
        _ => None,
    };
    match text_action(rt, KeyContext::SearchInput, code, mods) {
        Some(Action::CloseSearch) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) if !history_empty => rt.dispatch(TuiCursorEvent::SearchHistoryDown),
        Some(Action::MoveUp) if !history_empty => rt.dispatch(TuiCursorEvent::SearchHistoryUp),
        Some(Action::EditHistoryItem) if history_selected.is_some() => {
            rt.dispatch(TuiCursorEvent::SearchEditFromHistory)
        }
        Some(Action::ExecuteSearch) => {
            rt.dispatch(TuiCursorEvent::SearchSubmit);
            // The view-persist lifecycle mirrors the resulting
            // SearchResults mode and the search-history-push
            // lifecycle appends the (query, type) tuple — both run
            // on the next tick keyed off the new search.task_id.
        }
        Some(Action::CycleSearchType) => {
            rt.dispatch(TuiCursorEvent::SearchCycleType { forward: true })
        }
        Some(Action::CycleSearchTypePrev) => {
            rt.dispatch(TuiCursorEvent::SearchCycleType { forward: false })
        }
        _ => match code {
            KeyCode::Backspace => rt.dispatch(TuiCursorEvent::SearchInputBackspace),
            KeyCode::Char(c) => rt.dispatch(TuiCursorEvent::SearchInputType(c)),
            _ => {}
        },
    }
}

fn translate_create_playlist(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match text_action(rt, KeyContext::TextInput, code, mods) {
        Some(Action::CancelInput) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::ConfirmInput) => rt.dispatch(TuiCursorEvent::CreatePlaylistSubmit),
        _ => match code {
            KeyCode::Backspace => rt.dispatch(TuiCursorEvent::CreatePlaylistBackspace),
            KeyCode::Char(c) => rt.dispatch(TuiCursorEvent::CreatePlaylistType(c)),
            _ => {}
        },
    }
}

fn translate_rename_playlist(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match text_action(rt, KeyContext::TextInput, code, mods) {
        Some(Action::CancelInput) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::ConfirmInput) => rt.dispatch(TuiCursorEvent::RenamePlaylistSubmit),
        _ => match code {
            KeyCode::Backspace => rt.dispatch(TuiCursorEvent::RenamePlaylistBackspace),
            KeyCode::Char(c) => rt.dispatch(TuiCursorEvent::RenamePlaylistType(c)),
            _ => {}
        },
    }
}

fn translate_confirm_delete_playlist(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match text_action(rt, KeyContext::TextInput, code, mods) {
        Some(Action::CancelInput) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::ConfirmInput) => rt.dispatch(TuiCursorEvent::ConfirmDeleteSubmit),
        _ => match code {
            KeyCode::Backspace => rt.dispatch(TuiCursorEvent::ConfirmDeleteBackspace),
            KeyCode::Char(c) => rt.dispatch(TuiCursorEvent::ConfirmDeleteType(c)),
            _ => {}
        },
    }
}

fn translate_playlist_action(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::PlaylistActionModal, code, mods) {
        Some(Action::ClosePlaylistActionModal) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::PlaylistActionCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::PlaylistActionCursorUp),
        Some(Action::Activate) => rt.dispatch(TuiCursorEvent::PlaylistActionSubmit),
        Some(Action::PlaylistActionRename) => rt.dispatch(TuiCursorEvent::PlaylistActionRename),
        Some(Action::PlaylistActionDelete) => rt.dispatch(TuiCursorEvent::PlaylistActionDelete),
        _ => {}
    }
}

fn translate_playlist_picker(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::ListNavigation, code, mods) {
        Some(Action::Back) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::PlaylistPickerCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::PlaylistPickerCursorUp),
        Some(Action::Activate) => {
            rt.dispatch(TuiCursorEvent::PlaylistPickerSubmit);
            // The last-add-persist lifecycle mirrors picker.last_add_playlist
            // to disk on the next tick, regardless of how it was set.
        }
        _ => {}
    }
}

fn translate_confirm_remove(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match text_action(rt, KeyContext::TextInput, code, mods) {
        Some(Action::CancelInput) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::ConfirmInput) => rt.dispatch(TuiCursorEvent::ConfirmRemoveSubmit),
        _ => {}
    }
}

fn translate_selection_action_modal(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::SelectionActionModal, code, mods) {
        Some(Action::CloseSelectionActionModal) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::SelectionActionCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::SelectionActionCursorUp),
        Some(Action::Activate) => {
            // Resolve the currently-highlighted choice.
            let choice = match &rt.sources.screen {
                Screen::SelectionActionModal { selected } => {
                    let menu = mkpclient_runtime::queries::selection_action_menu(&rt.sources);
                    menu.get(*selected).map(|(c, _)| *c)
                }
                _ => None,
            };
            if let Some(c) = choice {
                rt.dispatch(TuiCursorEvent::SelectionActionApply(c));
            }
        }
        Some(action) => {
            if let Some(c) = selection_action_choice(action) {
                rt.dispatch(TuiCursorEvent::SelectionActionApply(c));
            }
        }
        _ => {}
    }
}

fn selection_action_choice(action: Action) -> Option<char> {
    Some(match action {
        Action::SelectionPlayNext => 'n',
        Action::SelectionPlayLast => 'e',
        Action::SelectionAddToPlaylist => 'a',
        Action::SelectionDelete => 'd',
        _ => return None,
    })
}

fn translate_error_modal(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    let message = match &rt.sources.screen {
        Screen::ErrorModal { message } => message.clone(),
        _ => return,
    };
    match action(rt, KeyContext::ErrorModal, code, mods) {
        Some(Action::CloseError) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::CopyError) => {
            rt.dispatch(SemanticEvent::ClipboardCopy {
                text: message.to_string(),
                success_toast: "Error copied to clipboard".into(),
            });
        }
        _ => {}
    }
}

fn translate_server_lost_modal(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    if let Some(Action::ServerLostConfirm) = action(rt, KeyContext::ServerLost, code, mods) {
        rt.dispatch(TuiCursorEvent::ServerLostGiveUp);
    }
}

fn translate_server_picker_modal(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::ServerPicker, code, mods) {
        Some(Action::CloseServerPicker) => rt.dispatch(TuiCursorEvent::CloseModal),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::ServerPickerModalCursorDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::ServerPickerModalCursorUp),
        Some(Action::ServerPickerSelect) => rt.dispatch(TuiCursorEvent::ServerPickerModalSelect),
        _ => {}
    }
}

fn translate_selection_mode(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    match action(rt, KeyContext::SelectionMode, code, mods) {
        Some(Action::CancelSelection) => rt.dispatch(TuiCursorEvent::SelectionClear),
        Some(Action::SelectAndMoveDown) => rt.dispatch(TuiCursorEvent::SelectionAdd),
        Some(Action::DeselectCurrent) => rt.dispatch(TuiCursorEvent::SelectionRemove),
        Some(Action::ToggleRangeAnchor) => rt.dispatch(TuiCursorEvent::SelectionToggleAnchor),
        Some(Action::MoveDown) => rt.dispatch(TuiCursorEvent::SelectionMoveDown),
        Some(Action::MoveUp) => rt.dispatch(TuiCursorEvent::SelectionMoveUp),
        Some(Action::PlaySelection) => rt.dispatch(TuiCursorEvent::SelectionPlayReset),
        Some(Action::OpenSelectionActionMenu) => rt.dispatch(TuiCursorEvent::SelectionOpenModal),
        _ => {}
    }
}

fn translate_now_playing(code: KeyCode, mods: KeyModifiers, rt: &mut Runtime) {
    use mkpclient_state_ui_cursor::ColumnFocus;
    match action(rt, KeyContext::ListNavigation, code, mods) {
        Some(Action::FocusLeft) => rt.dispatch(TuiCursorEvent::CycleFocusBackward),
        Some(Action::FocusRight) => rt.dispatch(TuiCursorEvent::CycleFocusForward),
        Some(Action::HistoryBack) => rt.dispatch(TuiCursorEvent::HistoryBack),
        Some(Action::HistoryForward) => rt.dispatch(TuiCursorEvent::HistoryForward),
        Some(Action::PlayPause) => rt.dispatch(SemanticEvent::TogglePlayPause),
        Some(Action::NextTrack) => rt.dispatch(SemanticEvent::SkipNext),
        Some(Action::PreviousTrack) => rt.dispatch(SemanticEvent::SkipPrevious),
        Some(Action::SeekForward10s) => rt.dispatch(SemanticEvent::SeekForward { fine: false }),
        Some(Action::SeekBackward10s) => rt.dispatch(SemanticEvent::SeekBackward { fine: false }),
        Some(Action::SeekForward1s) => rt.dispatch(SemanticEvent::SeekForward { fine: true }),
        Some(Action::SeekBackward1s) => rt.dispatch(SemanticEvent::SeekBackward { fine: true }),
        Some(Action::CycleRepeat) => rt.dispatch(SemanticEvent::CycleRepeatMode),
        Some(Action::OpenSearch) => rt.dispatch(TuiCursorEvent::OpenSearchInput),
        Some(Action::ToggleHelp) => rt.dispatch(TuiCursorEvent::OpenHelpOverlay),
        Some(Action::ToggleFilter) => rt.dispatch(TuiCursorEvent::OpenFilterInputForFocused),
        Some(Action::EnterSelectionMode) => rt.dispatch(TuiCursorEvent::ToggleSelectionForFocused),
        Some(Action::MoveToTop) => rt.dispatch(TuiCursorEvent::JumpTopFocused),
        Some(Action::MoveToBottom) => rt.dispatch(TuiCursorEvent::JumpBottomFocused),
        Some(Action::PageUp) => rt.dispatch(TuiCursorEvent::PageUpFocused),
        Some(Action::PageDown) => rt.dispatch(TuiCursorEvent::PageDownFocused),
        Some(Action::ShuffleActivate) => rt.dispatch(TuiCursorEvent::ShuffleActivateFocused),
        Some(Action::OpenActionMenu) => rt.dispatch(TuiCursorEvent::OpenActionMenuOrCycle),
        Some(Action::Back) => rt.dispatch(TuiCursorEvent::ClearFocusedFilter),
        Some(action) => match rt.sources.cursor.focus {
            ColumnFocus::Left => translate_left_pane(action, rt),
            ColumnFocus::Middle => translate_middle_pane(action, rt),
            ColumnFocus::Queue => translate_queue_pane(action, rt),
        },
        None => {}
    }
}

fn translate_left_pane(action: Action, rt: &mut Runtime) {
    match action {
        Action::MoveDown => rt.dispatch(TuiCursorEvent::LeftCursorDown),
        Action::MoveUp => rt.dispatch(TuiCursorEvent::LeftCursorUp),
        Action::Activate => rt.dispatch(TuiCursorEvent::LeftActivate),
        _ => {}
    }
}

fn translate_middle_pane(action: Action, rt: &mut Runtime) {
    match action {
        Action::MoveDown => rt.dispatch(TuiCursorEvent::MiddleCursorDown),
        Action::MoveUp => rt.dispatch(TuiCursorEvent::MiddleCursorUp),
        Action::Activate => rt.dispatch(TuiCursorEvent::MiddleActivate),
        _ => {}
    }
}

fn translate_queue_pane(action: Action, rt: &mut Runtime) {
    match action {
        Action::MoveDown => rt.dispatch(TuiCursorEvent::QueueCursorDown),
        Action::MoveUp => rt.dispatch(TuiCursorEvent::QueueCursorUp),
        Action::Activate => rt.dispatch(TuiCursorEvent::QueueActivate),
        _ => {}
    }
}

// `ActionKind` is referenced by the action menu plumbing.
const _: () = {
    fn _enforce(_: ActionKind) {}
};
