use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionOfferRecord {
    pub mime_type: String,
    mime_types: Vec<String>,
    pub byte_len: usize,
}

impl SelectionOfferRecord {
    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }
}

pub trait SelectionMimeTypes {
    fn into_mime_types(self) -> Vec<String>;
}

impl SelectionMimeTypes for &str {
    fn into_mime_types(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl SelectionMimeTypes for String {
    fn into_mime_types(self) -> Vec<String> {
        vec![self]
    }
}

impl<const N: usize> SelectionMimeTypes for [&str; N] {
    fn into_mime_types(self) -> Vec<String> {
        self.into_iter().map(str::to_string).collect()
    }
}

impl SelectionMimeTypes for Vec<String> {
    fn into_mime_types(self) -> Vec<String> {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionState {
    max_history: usize,
    clipboard_history: Vec<SelectionOfferRecord>,
    primary_selection: Option<SelectionOfferRecord>,
    data_control_enabled: bool,
    sources: HashMap<u32, Vec<String>>,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            max_history: 16,
            clipboard_history: Vec::new(),
            primary_selection: None,
            data_control_enabled: true,
            sources: HashMap::new(),
        }
    }
}

impl SelectionState {
    pub fn with_max_history(max_history: usize) -> Self {
        Self {
            max_history: max_history.max(1),
            ..Self::default()
        }
    }

    pub fn record_clipboard_offer(&mut self, mime_types: impl SelectionMimeTypes, byte_len: usize) {
        let mime_types = normalize_mime_types(mime_types);
        self.clipboard_history.push(SelectionOfferRecord {
            mime_type: mime_types[0].clone(),
            mime_types,
            byte_len,
        });
        let excess = self
            .clipboard_history
            .len()
            .saturating_sub(self.max_history);
        if excess > 0 {
            self.clipboard_history.drain(0..excess);
        }
    }

    pub fn set_primary_selection(&mut self, mime_types: impl SelectionMimeTypes, byte_len: usize) {
        let mime_types = normalize_mime_types(mime_types);
        self.primary_selection = Some(SelectionOfferRecord {
            mime_type: mime_types[0].clone(),
            mime_types,
            byte_len,
        });
    }

    pub fn begin_source(&mut self, source_id: u32) {
        self.sources.entry(source_id).or_default();
    }

    pub fn offer_source_mime_type(&mut self, source_id: u32, mime_type: impl Into<String>) {
        let mime_type = mime_type.into();
        if mime_type.is_empty() {
            return;
        }
        self.sources.entry(source_id).or_default().push(mime_type);
    }

    pub fn commit_source_to_primary_selection(&mut self, source_id: u32, byte_len: usize) -> bool {
        let Some(mime_types) = self.sources.get(&source_id).cloned() else {
            return false;
        };
        self.set_primary_selection(mime_types, byte_len);
        true
    }

    pub fn clear_primary_selection(&mut self) {
        self.primary_selection = None;
    }

    pub fn set_data_control_enabled(&mut self, enabled: bool) {
        self.data_control_enabled = enabled;
    }

    pub const fn data_control_enabled(&self) -> bool {
        self.data_control_enabled
    }

    pub fn clipboard_history(&self) -> &[SelectionOfferRecord] {
        &self.clipboard_history
    }

    pub fn primary_selection(&self) -> Option<&SelectionOfferRecord> {
        self.primary_selection.as_ref()
    }
}

fn normalize_mime_types(mime_types: impl SelectionMimeTypes) -> Vec<String> {
    let mime_types: Vec<_> = mime_types
        .into_mime_types()
        .into_iter()
        .filter(|mime_type| !mime_type.is_empty())
        .collect();
    if mime_types.is_empty() {
        vec!["application/octet-stream".to_string()]
    } else {
        mime_types
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_state_keeps_bounded_clipboard_history() {
        let mut state = SelectionState::with_max_history(2);

        state.record_clipboard_offer("text/plain", 4);
        state.record_clipboard_offer("text/html", 10);
        state.record_clipboard_offer("image/png", 32);

        assert_eq!(state.clipboard_history().len(), 2);
        assert_eq!(state.clipboard_history()[0].mime_type, "text/html");
        assert_eq!(state.clipboard_history()[1].mime_type, "image/png");
    }

    #[test]
    fn selection_offer_records_all_announced_mime_types() {
        let mut state = SelectionState::default();

        state.record_clipboard_offer(["text/plain", "text/plain;charset=utf-8"], 8);

        let offer = state.clipboard_history().last().unwrap();
        assert_eq!(offer.mime_type, "text/plain");
        assert_eq!(
            offer.mime_types(),
            ["text/plain", "text/plain;charset=utf-8"]
        );
    }

    #[test]
    fn selection_state_tracks_primary_selection_separately() {
        let mut state = SelectionState::default();

        state.record_clipboard_offer("text/plain", 4);
        state.set_primary_selection("text/plain;charset=utf-8", 8);

        assert_eq!(state.clipboard_history().len(), 1);
        assert_eq!(
            state
                .primary_selection()
                .map(|offer| offer.mime_type.as_str()),
            Some("text/plain;charset=utf-8")
        );
        state.clear_primary_selection();
        assert!(state.primary_selection().is_none());
    }

    #[test]
    fn selection_state_commits_announced_source_to_primary_selection() {
        let mut state = SelectionState::default();

        state.begin_source(7);
        state.offer_source_mime_type(7, "text/plain");
        state.offer_source_mime_type(7, "text/plain;charset=utf-8");

        assert!(state.commit_source_to_primary_selection(7, 12));
        let offer = state.primary_selection().unwrap();
        assert_eq!(offer.mime_type, "text/plain");
        assert_eq!(
            offer.mime_types(),
            ["text/plain", "text/plain;charset=utf-8"]
        );
    }

    #[test]
    fn data_control_can_be_disabled_by_policy() {
        let mut state = SelectionState::default();

        state.set_data_control_enabled(false);

        assert!(!state.data_control_enabled());
    }
}
