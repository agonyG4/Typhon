use std::collections::HashMap;

const MAX_SOURCE_MIME_TYPES: usize = 128;
const MAX_MIME_TYPE_LEN: usize = 4096;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSelection {
    pub generation: u64,
    pub kind: SelectionKind,
    pub source_id: u32,
    pub mime_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataOfferBinding {
    pub offer_id: u64,
    pub target_id: u32,
    pub source_generation: u64,
    pub mime_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionState {
    max_history: usize,
    clipboard_history: Vec<SelectionOfferRecord>,
    primary_selection: Option<SelectionOfferRecord>,
    data_control_enabled: bool,
    sources: HashMap<u32, Vec<String>>,
    clipboard_selection: Option<ActiveSelection>,
    offers: HashMap<u64, DataOfferBinding>,
    next_offer_id: u64,
    next_selection_generation: u64,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            max_history: 16,
            clipboard_history: Vec::new(),
            primary_selection: None,
            data_control_enabled: true,
            sources: HashMap::new(),
            clipboard_selection: None,
            offers: HashMap::new(),
            next_offer_id: 0,
            next_selection_generation: 0,
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
        if mime_type.is_empty() || mime_type.len() > MAX_MIME_TYPE_LEN {
            return;
        }
        let mime_types = self.sources.entry(source_id).or_default();
        if mime_types.len() >= MAX_SOURCE_MIME_TYPES
            || mime_types.iter().any(|existing| existing == &mime_type)
        {
            return;
        }
        mime_types.push(mime_type);
    }

    pub fn source_mime_types(&self, source_id: u32) -> Option<&[String]> {
        self.sources.get(&source_id).map(Vec::as_slice)
    }

    pub fn set_clipboard_selection_from_source(&mut self, source_id: u32) -> Option<u64> {
        let mime_types = self.sources.get(&source_id)?.clone();
        if mime_types.is_empty() {
            return None;
        }
        self.next_selection_generation = self.next_selection_generation.saturating_add(1);
        let generation = self.next_selection_generation;
        self.clipboard_selection = Some(ActiveSelection {
            generation,
            kind: SelectionKind::Clipboard,
            source_id,
            mime_types,
        });
        self.offers.clear();
        Some(generation)
    }

    pub fn clear_clipboard_selection(&mut self) {
        self.next_selection_generation = self.next_selection_generation.saturating_add(1);
        self.clipboard_selection = None;
        self.offers.clear();
    }

    pub fn active_clipboard_selection(&self) -> Option<&ActiveSelection> {
        self.clipboard_selection.as_ref()
    }

    pub fn register_clipboard_offer(
        &mut self,
        target_id: u32,
        source_generation: u64,
    ) -> Option<u64> {
        let selection = self.clipboard_selection.as_ref()?;
        if selection.generation != source_generation {
            return None;
        }
        self.next_offer_id = self.next_offer_id.saturating_add(1).max(1);
        let offer_id = self.next_offer_id;
        self.offers.insert(
            offer_id,
            DataOfferBinding {
                offer_id,
                target_id,
                source_generation,
                mime_types: selection.mime_types.clone(),
            },
        );
        Some(offer_id)
    }

    pub fn offer_matches_active_selection(&self, offer_id: u64, mime_type: &str) -> bool {
        let Some(offer) = self.offers.get(&offer_id) else {
            return false;
        };
        let Some(selection) = self.clipboard_selection.as_ref() else {
            return false;
        };
        offer.source_generation == selection.generation
            && offer.mime_types.iter().any(|mime| mime == mime_type)
    }

    pub fn remove_source(&mut self, source_id: u32) {
        self.sources.remove(&source_id);
        if self
            .clipboard_selection
            .as_ref()
            .is_some_and(|selection| selection.source_id == source_id)
        {
            self.clear_clipboard_selection();
        }
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

    #[test]
    fn data_source_mime_offers_are_deduplicated_bounded_and_ordered() {
        let mut state = SelectionState::default();

        state.begin_source(7);
        state.offer_source_mime_type(7, "");
        state.offer_source_mime_type(7, "text/plain");
        state.offer_source_mime_type(7, "text/html");
        state.offer_source_mime_type(7, "text/plain");
        state.offer_source_mime_type(7, "x".repeat(4097));
        for index in 0..140 {
            state.offer_source_mime_type(7, format!("application/x-{index}"));
        }

        let mime_types = state.source_mime_types(7).unwrap();
        assert_eq!(mime_types[0], "text/plain");
        assert_eq!(mime_types[1], "text/html");
        assert_eq!(mime_types.len(), 128);
        assert_eq!(
            mime_types
                .iter()
                .filter(|mime| *mime == "text/plain")
                .count(),
            1
        );
        assert!(!mime_types.iter().any(|mime| mime.len() > 4096));
    }

    #[test]
    fn clipboard_selection_uses_generation_and_invalidates_stale_offers() {
        let mut state = SelectionState::default();

        state.begin_source(7);
        state.offer_source_mime_type(7, "text/plain");
        let first_generation = state
            .set_clipboard_selection_from_source(7)
            .expect("source should become clipboard selection");
        let offer = state
            .register_clipboard_offer(42, first_generation)
            .expect("offer should be valid for active generation");

        assert!(state.offer_matches_active_selection(offer, "text/plain"));

        state.begin_source(8);
        state.offer_source_mime_type(8, "text/html");
        let second_generation = state
            .set_clipboard_selection_from_source(8)
            .expect("replacement source should become clipboard selection");

        assert_ne!(first_generation, second_generation);
        assert!(!state.offer_matches_active_selection(offer, "text/plain"));
        assert!(!state.offer_matches_active_selection(offer, "text/html"));
    }

    #[test]
    fn destroying_active_source_clears_clipboard_selection() {
        let mut state = SelectionState::default();

        state.begin_source(7);
        state.offer_source_mime_type(7, "text/plain");
        state
            .set_clipboard_selection_from_source(7)
            .expect("source should become clipboard selection");

        state.remove_source(7);

        assert!(state.active_clipboard_selection().is_none());
    }
}
