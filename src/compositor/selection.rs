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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectionSourceKey(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionSourceKind {
    WaylandClipboard,
    WaylandPrimary,
    DataControl,
    HostClipboardBridge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSourceRecord {
    pub key: SelectionSourceKey,
    pub kind: SelectionSourceKind,
    pub owner: Option<u64>,
    pub mime_types: Vec<String>,
    pub used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSelection {
    pub generation: u64,
    pub kind: SelectionKind,
    pub source_key: SelectionSourceKey,
    pub source_kind: SelectionSourceKind,
    pub source_id: u32,
    pub mime_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataOfferBinding {
    pub offer_id: u64,
    pub target_id: u32,
    pub kind: SelectionKind,
    pub source_generation: u64,
    pub source_key: SelectionSourceKey,
    pub mime_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionCommit {
    pub generation: u64,
    pub replaced_source: Option<SelectionSourceKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionClear {
    pub generation: u64,
    pub cleared_source: Option<SelectionSourceKey>,
}

#[derive(Debug, Default, Clone)]
struct SelectionChannel {
    generation: u64,
    active: Option<ActiveSelection>,
    offers: HashMap<u64, DataOfferBinding>,
}

impl SelectionChannel {
    fn advance_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }
}

#[derive(Debug, Clone)]
pub struct SelectionState {
    max_history: usize,
    clipboard_history: Vec<SelectionOfferRecord>,
    primary_selection: Option<SelectionOfferRecord>,
    data_control_enabled: bool,
    sources: HashMap<SelectionSourceKey, SelectionSourceRecord>,
    legacy_source_keys: HashMap<u32, SelectionSourceKey>,
    channels: [SelectionChannel; 2],
    next_offer_id: u64,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            max_history: 16,
            clipboard_history: Vec::new(),
            primary_selection: None,
            data_control_enabled: true,
            sources: HashMap::new(),
            legacy_source_keys: HashMap::new(),
            channels: [SelectionChannel::default(), SelectionChannel::default()],
            next_offer_id: 0,
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

    pub fn register_source(
        &mut self,
        key: SelectionSourceKey,
        kind: SelectionSourceKind,
        owner: Option<u64>,
    ) {
        self.sources.insert(
            key,
            SelectionSourceRecord {
                key,
                kind,
                owner,
                mime_types: Vec::new(),
                used: false,
            },
        );
    }

    pub fn mark_source_used(&mut self, key: SelectionSourceKey) -> bool {
        let Some(source) = self.sources.get_mut(&key) else {
            return false;
        };
        if source.used {
            return false;
        }
        source.used = true;
        true
    }

    pub fn source(&self, key: SelectionSourceKey) -> Option<&SelectionSourceRecord> {
        self.sources.get(&key)
    }

    pub fn offer_source_mime_type_for_key(
        &mut self,
        key: SelectionSourceKey,
        mime_type: impl Into<String>,
    ) {
        let mime_type = mime_type.into();
        if mime_type.is_empty() || mime_type.len() > MAX_MIME_TYPE_LEN {
            return;
        }
        let Some(source) = self.sources.get_mut(&key) else {
            return;
        };
        if source.mime_types.len() >= MAX_SOURCE_MIME_TYPES
            || source
                .mime_types
                .iter()
                .any(|existing| existing == &mime_type)
        {
            return;
        }
        source.mime_types.push(mime_type);
    }

    pub fn source_mime_types_for_key(&self, key: SelectionSourceKey) -> Option<&[String]> {
        self.sources
            .get(&key)
            .map(|source| source.mime_types.as_slice())
    }

    pub fn commit_selection(
        &mut self,
        kind: SelectionKind,
        key: SelectionSourceKey,
    ) -> Option<SelectionCommit> {
        let source = self.sources.get(&key)?.clone();
        if source.mime_types.is_empty() {
            return None;
        }
        let channel = self.channel_mut(kind);
        let replaced_source = channel
            .active
            .as_ref()
            .map(|selection| selection.source_key)
            .filter(|active_key| *active_key != key);
        let generation = channel.advance_generation();
        channel.active = Some(ActiveSelection {
            generation,
            kind,
            source_key: key,
            source_kind: source.kind,
            source_id: key.0 as u32,
            mime_types: source.mime_types,
        });
        channel.offers.clear();
        Some(SelectionCommit {
            generation,
            replaced_source,
        })
    }

    pub fn clear_selection(&mut self, kind: SelectionKind) -> SelectionClear {
        let channel = self.channel_mut(kind);
        let cleared_source = channel.active.take().map(|selection| selection.source_key);
        let generation = channel.advance_generation();
        channel.offers.clear();
        if kind == SelectionKind::Primary {
            self.primary_selection = None;
        }
        SelectionClear {
            generation,
            cleared_source,
        }
    }

    pub fn active_selection(&self, kind: SelectionKind) -> Option<&ActiveSelection> {
        self.channel(kind).active.as_ref()
    }

    pub fn current_generation(&self, kind: SelectionKind) -> u64 {
        self.channel(kind).generation
    }

    pub fn register_offer(
        &mut self,
        kind: SelectionKind,
        target_id: u32,
        source_generation: u64,
    ) -> Option<u64> {
        let selection = self.active_selection(kind)?.clone();
        if selection.generation != source_generation {
            return None;
        }
        self.next_offer_id = self.next_offer_id.wrapping_add(1).max(1);
        let offer_id = self.next_offer_id;
        self.channel_mut(kind).offers.insert(
            offer_id,
            DataOfferBinding {
                offer_id,
                target_id,
                kind,
                source_generation,
                source_key: selection.source_key,
                mime_types: selection.mime_types,
            },
        );
        Some(offer_id)
    }

    pub fn offer_is_current(
        &self,
        offer_id: u64,
        kind: SelectionKind,
        generation: u64,
        target_id: u32,
        mime_type: &str,
    ) -> bool {
        let Some(offer) = self.channel(kind).offers.get(&offer_id) else {
            return false;
        };
        let Some(selection) = self.active_selection(kind) else {
            return false;
        };
        offer.kind == kind
            && offer.target_id == target_id
            && offer.source_generation == generation
            && offer.source_generation == selection.generation
            && offer.source_key == selection.source_key
            && offer.mime_types.iter().any(|mime| mime == mime_type)
    }

    pub fn remove_source_key(&mut self, key: SelectionSourceKey) -> Vec<SelectionKind> {
        self.sources.remove(&key);
        self.legacy_source_keys.retain(|_, value| *value != key);
        let mut cleared = Vec::new();
        for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
            if self
                .active_selection(kind)
                .is_some_and(|selection| selection.source_key == key)
            {
                self.clear_selection(kind);
                cleared.push(kind);
            }
        }
        cleared
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
        let key = SelectionSourceKey(u64::from(source_id));
        self.legacy_source_keys.insert(source_id, key);
        self.register_source(key, SelectionSourceKind::WaylandClipboard, None);
    }

    pub fn offer_source_mime_type(&mut self, source_id: u32, mime_type: impl Into<String>) {
        let Some(key) = self.legacy_source_keys.get(&source_id).copied() else {
            return;
        };
        self.offer_source_mime_type_for_key(key, mime_type);
    }

    pub fn source_mime_types(&self, source_id: u32) -> Option<&[String]> {
        let key = self.legacy_source_keys.get(&source_id).copied()?;
        self.source_mime_types_for_key(key)
    }

    pub fn set_clipboard_selection_from_source(&mut self, source_id: u32) -> Option<u64> {
        let key = self.legacy_source_keys.get(&source_id).copied()?;
        self.commit_selection(SelectionKind::Clipboard, key)
            .map(|commit| commit.generation)
    }

    pub fn clear_clipboard_selection(&mut self) {
        self.clear_selection(SelectionKind::Clipboard);
    }

    pub fn active_clipboard_selection(&self) -> Option<&ActiveSelection> {
        self.active_selection(SelectionKind::Clipboard)
    }

    pub fn register_clipboard_offer(
        &mut self,
        target_id: u32,
        source_generation: u64,
    ) -> Option<u64> {
        self.register_offer(SelectionKind::Clipboard, target_id, source_generation)
    }

    pub fn offer_matches_active_selection(&self, offer_id: u64, mime_type: &str) -> bool {
        let Some(offer) = self.channel(SelectionKind::Clipboard).offers.get(&offer_id) else {
            return false;
        };
        self.offer_is_current(
            offer_id,
            SelectionKind::Clipboard,
            offer.source_generation,
            offer.target_id,
            mime_type,
        )
    }

    pub fn remove_source(&mut self, source_id: u32) {
        if let Some(key) = self.legacy_source_keys.get(&source_id).copied() {
            self.remove_source_key(key);
        }
    }

    pub fn commit_source_to_primary_selection(&mut self, source_id: u32, byte_len: usize) -> bool {
        let Some(key) = self.legacy_source_keys.get(&source_id).copied() else {
            return false;
        };
        let Some(mime_types) = self.source_mime_types_for_key(key).map(ToOwned::to_owned) else {
            return false;
        };
        self.set_primary_selection(mime_types, byte_len);
        true
    }

    pub fn clear_primary_selection(&mut self) {
        self.clear_selection(SelectionKind::Primary);
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

    fn channel(&self, kind: SelectionKind) -> &SelectionChannel {
        &self.channels[match kind {
            SelectionKind::Clipboard => 0,
            SelectionKind::Primary => 1,
        }]
    }

    fn channel_mut(&mut self, kind: SelectionKind) -> &mut SelectionChannel {
        &mut self.channels[match kind {
            SelectionKind::Clipboard => 0,
            SelectionKind::Primary => 1,
        }]
    }
}

fn normalize_mime_types(mime_types: impl SelectionMimeTypes) -> Vec<String> {
    let mut normalized = Vec::new();
    for mime_type in mime_types.into_mime_types() {
        if mime_type.is_empty()
            || mime_type.len() > MAX_MIME_TYPE_LEN
            || normalized.iter().any(|existing| existing == &mime_type)
        {
            continue;
        }
        normalized.push(mime_type);
        if normalized.len() == MAX_SOURCE_MIME_TYPES {
            break;
        }
    }
    if normalized.is_empty() {
        vec!["application/octet-stream".to_string()]
    } else {
        normalized
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

    #[test]
    fn broker_keeps_clipboard_and_primary_generations_independent() {
        let mut broker = SelectionState::default();
        let clipboard_source = SelectionSourceKey(100);
        let primary_source = SelectionSourceKey(200);
        let replacement_source = SelectionSourceKey(201);
        broker.register_source(
            clipboard_source,
            SelectionSourceKind::WaylandClipboard,
            Some(1),
        );
        broker.register_source(primary_source, SelectionSourceKind::WaylandPrimary, Some(2));
        broker.register_source(
            replacement_source,
            SelectionSourceKind::DataControl,
            Some(3),
        );
        broker.offer_source_mime_type_for_key(clipboard_source, "text/plain");
        broker.offer_source_mime_type_for_key(primary_source, "text/plain");
        broker.offer_source_mime_type_for_key(replacement_source, "text/plain");

        let clipboard_commit = broker
            .commit_selection(SelectionKind::Clipboard, clipboard_source)
            .unwrap();
        let primary_commit = broker
            .commit_selection(SelectionKind::Primary, primary_source)
            .unwrap();
        let clipboard_offer = broker
            .register_offer(SelectionKind::Clipboard, 7, clipboard_commit.generation)
            .unwrap();
        let primary_offer = broker
            .register_offer(SelectionKind::Primary, 8, primary_commit.generation)
            .unwrap();

        broker
            .commit_selection(SelectionKind::Primary, replacement_source)
            .unwrap();

        assert!(broker.offer_is_current(
            clipboard_offer,
            SelectionKind::Clipboard,
            clipboard_commit.generation,
            7,
            "text/plain"
        ));
        assert!(!broker.offer_is_current(
            primary_offer,
            SelectionKind::Primary,
            primary_commit.generation,
            8,
            "text/plain"
        ));
        assert_eq!(
            broker
                .active_selection(SelectionKind::Primary)
                .unwrap()
                .source_key,
            replacement_source
        );
    }

    #[test]
    fn stale_source_removal_cannot_clear_newer_selection() {
        let mut broker = SelectionState::default();
        let old_source = SelectionSourceKey(1);
        let new_source = SelectionSourceKey(2);
        for key in [old_source, new_source] {
            broker.register_source(key, SelectionSourceKind::DataControl, None);
            broker.offer_source_mime_type_for_key(key, "text/plain");
        }
        broker
            .commit_selection(SelectionKind::Primary, old_source)
            .unwrap();
        broker
            .commit_selection(SelectionKind::Primary, new_source)
            .unwrap();
        broker.remove_source_key(old_source);

        assert_eq!(
            broker
                .active_selection(SelectionKind::Primary)
                .unwrap()
                .source_key,
            new_source
        );
    }
}
