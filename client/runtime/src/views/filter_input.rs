//! View model for the live-filter input modal.
//!
//! Per spec §4 every view is a `#[drv::memo]`. The filter input
//! reads the screen's `FilterInput(FilterState)` payload — `target`
//! says which pane the filter applies to, `input` is the active
//! query string. Shape is trivial so the model is just a thin
//! `Clone + PartialEq` mirror, but the memo cache means the
//! renderer's diff path can short-circuit on equality across frames.

use std::sync::Arc;

use mkpclient_state_ui_filter::FilterTarget;
use mkpclient_state_ui_screen::FilterState;

/// Payload the renderer needs to draw the filter modal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FilterInputModel {
    pub target: FilterTarget,
    pub input: Arc<str>,
}

#[derive(drv::Input)]
pub struct FilterStateInput<'a> {
    /// Encoded as a u8 because `FilterTarget` isn't `drv::ToStatic`
    /// — flatten in the projection so the memo input stays simple.
    pub target_idx: u8,
    pub input: &'a Arc<str>,
}

impl<'a> FilterStateInput<'a> {
    pub fn new(state: &'a FilterState) -> Self {
        Self {
            target_idx: encode_target(state.target),
            input: &state.input,
        }
    }
}

#[drv::memo(single)]
pub fn filter_input_model<'a>(state: FilterStateInput<'a>) -> FilterInputModel {
    FilterInputModel {
        target: decode_target(state.target_idx),
        input: state.input.clone(),
    }
}

fn encode_target(t: FilterTarget) -> u8 {
    match t {
        FilterTarget::Middle => 0,
        FilterTarget::Queue => 1,
    }
}

fn decode_target(i: u8) -> FilterTarget {
    match i {
        1 => FilterTarget::Queue,
        _ => FilterTarget::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_mirrors_state() {
        let s = FilterState {
            target: FilterTarget::Middle,
            input: Arc::from("love"),
        };
        let m = filter_input_model(FilterStateInput::new(&s));
        assert_eq!(m.target, FilterTarget::Middle);
        assert_eq!(&*m.input, "love");
    }

    #[test]
    fn empty_input_round_trips() {
        let s = FilterState {
            target: FilterTarget::Queue,
            input: Arc::from(""),
        };
        let m = filter_input_model(FilterStateInput::new(&s));
        assert_eq!(m.target, FilterTarget::Queue);
        assert!(m.input.is_empty());
    }
}
