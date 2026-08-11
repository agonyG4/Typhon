use super::*;
use std::collections::HashMap;

const MAX_SOURCE_MIME_TYPES: usize = 128;
const MAX_MIME_TYPE_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SelectionMutationEpoch(pub u64);

impl SelectionMutationEpoch {
    pub const ZERO: Self = Self(0);

    fn advance(self) -> Self {
        Self(self.0.saturating_add(1).max(1))
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

#[derive(Debug, Clone)]
pub enum SelectionSourceBackend {
    WaylandClipboard {
        source: wl_data_source::WlDataSource,
        client_id: ClientId,
    },
    WaylandPrimary {
        source: zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        client_id: ClientId,
    },
    DataControl {
        source: ext_data_control_source_v1::ExtDataControlSourceV1,
        client_id: ClientId,
    },
    HostClipboardBridge {
        offer_id: HostClipboardOfferId,
    },
}

#[derive(Debug, Clone)]
pub struct SelectionSourceRecord {
    pub key: SelectionSourceKey,
    pub kind: SelectionSourceKind,
    pub owner: Option<u64>,
    pub mime_types: Vec<String>,
    pub used: bool,
    backend: Option<SelectionSourceBackend>,
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
    pub mutation_epoch: SelectionMutationEpoch,
    pub replaced_source: Option<SelectionSourceKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionClear {
    pub generation: u64,
    pub mutation_epoch: SelectionMutationEpoch,
    pub cleared_source: Option<SelectionSourceKey>,
}

#[derive(Debug, Default, Clone)]
struct SelectionChannel {
    generation: u64,
    mutation_watermark: SelectionMutationEpoch,
    active: Option<ActiveSelection>,
    offers: HashMap<u64, DataOfferBinding>,
}

impl SelectionChannel {
    fn advance_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }

    fn accepts_epoch(&self, epoch: SelectionMutationEpoch) -> bool {
        epoch >= self.mutation_watermark
    }

    fn record_epoch(&mut self, epoch: SelectionMutationEpoch) {
        self.mutation_watermark = epoch;
    }
}

#[derive(Debug, Clone)]
pub struct SelectionState {
    sources: HashMap<SelectionSourceKey, SelectionSourceRecord>,
    channels: [SelectionChannel; 2],
    next_offer_id: u64,
    next_mutation_epoch: SelectionMutationEpoch,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            sources: HashMap::new(),
            channels: [SelectionChannel::default(), SelectionChannel::default()],
            next_offer_id: 0,
            next_mutation_epoch: SelectionMutationEpoch::ZERO,
        }
    }
}

impl SelectionState {
    pub fn allocate_mutation_epoch(&mut self) -> SelectionMutationEpoch {
        self.next_mutation_epoch = self.next_mutation_epoch.advance();
        self.next_mutation_epoch
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
                backend: None,
            },
        );
    }

    pub fn set_source_backend(&mut self, key: SelectionSourceKey, backend: SelectionSourceBackend) {
        if let Some(source) = self.sources.get_mut(&key) {
            source.backend = Some(backend);
        }
    }

    pub fn source_backend(&self, key: SelectionSourceKey) -> Option<&SelectionSourceBackend> {
        self.sources.get(&key)?.backend.as_ref()
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
        mutation_epoch: SelectionMutationEpoch,
    ) -> Option<SelectionCommit> {
        let source = self.sources.get(&key)?.clone();
        if source.mime_types.is_empty() {
            return None;
        }
        let channel = self.channel_mut(kind);
        if !channel.accepts_epoch(mutation_epoch) {
            return None;
        }
        let replaced_source = channel
            .active
            .as_ref()
            .map(|selection| selection.source_key)
            .filter(|active_key| *active_key != key);
        let generation = channel.advance_generation();
        channel.record_epoch(mutation_epoch);
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
            mutation_epoch,
            replaced_source,
        })
    }

    pub fn clear_selection(
        &mut self,
        kind: SelectionKind,
        mutation_epoch: SelectionMutationEpoch,
    ) -> Option<SelectionClear> {
        let channel = self.channel_mut(kind);
        if !channel.accepts_epoch(mutation_epoch) {
            return None;
        }
        let cleared_source = channel.active.take().map(|selection| selection.source_key);
        let generation = channel.advance_generation();
        channel.record_epoch(mutation_epoch);
        channel.offers.clear();
        Some(SelectionClear {
            generation,
            mutation_epoch,
            cleared_source,
        })
    }

    pub fn active_selection(&self, kind: SelectionKind) -> Option<&ActiveSelection> {
        self.channel(kind).active.as_ref()
    }

    pub fn current_generation(&self, kind: SelectionKind) -> u64 {
        self.channel(kind).generation
    }

    pub fn current_mutation_epoch(&self, kind: SelectionKind) -> SelectionMutationEpoch {
        self.channel(kind).mutation_watermark
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
        source_key: SelectionSourceKey,
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
            && offer.source_key == source_key
            && offer.source_key == selection.source_key
            && offer.mime_types.iter().any(|mime| mime == mime_type)
    }

    pub fn remove_source_key(
        &mut self,
        key: SelectionSourceKey,
        mutation_epoch: SelectionMutationEpoch,
    ) -> Vec<SelectionKind> {
        self.sources.remove(&key);
        let mut cleared = Vec::new();
        for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
            if self
                .active_selection(kind)
                .is_some_and(|selection| selection.source_key == key)
                && self.clear_selection(kind, mutation_epoch).is_some()
            {
                cleared.push(kind);
            }
        }
        cleared
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

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(state: &mut SelectionState) -> SelectionMutationEpoch {
        state.allocate_mutation_epoch()
    }

    fn register_source(
        state: &mut SelectionState,
        key: SelectionSourceKey,
        kind: SelectionSourceKind,
    ) {
        state.register_source(key, kind, None);
        state.offer_source_mime_type_for_key(key, "text/plain");
    }

    #[test]
    fn data_source_mime_offers_are_deduplicated_bounded_and_ordered() {
        let mut state = SelectionState::default();
        let key = SelectionSourceKey(7);
        state.register_source(key, SelectionSourceKind::WaylandClipboard, None);
        state.offer_source_mime_type_for_key(key, "");
        state.offer_source_mime_type_for_key(key, "text/plain");
        state.offer_source_mime_type_for_key(key, "text/html");
        state.offer_source_mime_type_for_key(key, "text/plain");
        state.offer_source_mime_type_for_key(key, "x".repeat(4097));
        for index in 0..140 {
            state.offer_source_mime_type_for_key(key, format!("application/x-{index}"));
        }

        let mime_types = state.source_mime_types_for_key(key).unwrap();
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
    fn selection_offer_validation_is_generation_and_source_scoped() {
        let mut state = SelectionState::default();
        let old_key = SelectionSourceKey(7);
        let new_key = SelectionSourceKey(8);
        register_source(&mut state, old_key, SelectionSourceKind::WaylandClipboard);
        register_source(&mut state, new_key, SelectionSourceKind::DataControl);

        let first_epoch = epoch(&mut state);
        let first = state
            .commit_selection(SelectionKind::Clipboard, old_key, first_epoch)
            .unwrap();
        let offer = state
            .register_offer(SelectionKind::Clipboard, 42, first.generation)
            .unwrap();
        assert!(state.offer_is_current(
            offer,
            SelectionKind::Clipboard,
            first.generation,
            42,
            old_key,
            "text/plain"
        ));

        let second_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Clipboard, new_key, second_epoch)
            .unwrap();
        assert!(!state.offer_is_current(
            offer,
            SelectionKind::Clipboard,
            first.generation,
            42,
            old_key,
            "text/plain"
        ));
    }

    #[test]
    fn clipboard_and_primary_mutation_watermarks_are_independent() {
        let mut state = SelectionState::default();
        let clipboard = SelectionSourceKey(100);
        let primary = SelectionSourceKey(200);
        register_source(&mut state, clipboard, SelectionSourceKind::WaylandClipboard);
        register_source(&mut state, primary, SelectionSourceKind::WaylandPrimary);

        let clipboard_epoch = epoch(&mut state);
        let primary_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Clipboard, clipboard, clipboard_epoch)
            .unwrap();
        state
            .commit_selection(SelectionKind::Primary, primary, primary_epoch)
            .unwrap();
        let clipboard_watermark = state.current_mutation_epoch(SelectionKind::Clipboard);
        let primary_watermark = state.current_mutation_epoch(SelectionKind::Primary);

        let clear_epoch = epoch(&mut state);
        state
            .clear_selection(SelectionKind::Primary, clear_epoch)
            .unwrap();

        assert_eq!(
            state.current_mutation_epoch(SelectionKind::Clipboard),
            clipboard_watermark
        );
        assert_ne!(
            state.current_mutation_epoch(SelectionKind::Primary),
            primary_watermark
        );
    }

    #[test]
    fn older_mutation_epoch_cannot_replace_newer_channel_state() {
        let mut state = SelectionState::default();
        let old_key = SelectionSourceKey(1);
        let new_key = SelectionSourceKey(2);
        register_source(&mut state, old_key, SelectionSourceKind::WaylandClipboard);
        register_source(&mut state, new_key, SelectionSourceKind::DataControl);

        let old_epoch = epoch(&mut state);
        let new_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Clipboard, new_key, new_epoch)
            .unwrap();
        assert!(
            state
                .commit_selection(SelectionKind::Clipboard, old_key, old_epoch)
                .is_none()
        );
        assert_eq!(
            state
                .active_selection(SelectionKind::Clipboard)
                .unwrap()
                .source_key,
            new_key
        );
    }

    #[test]
    fn late_wayland_primary_mutation_cannot_replace_newer_data_control_state() {
        let mut state = SelectionState::default();
        let old_key = SelectionSourceKey(11);
        let new_key = SelectionSourceKey(12);
        register_source(&mut state, old_key, SelectionSourceKind::WaylandPrimary);
        register_source(&mut state, new_key, SelectionSourceKind::DataControl);

        let old_epoch = epoch(&mut state);
        let new_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Primary, new_key, new_epoch)
            .unwrap();
        assert!(
            state
                .commit_selection(SelectionKind::Primary, old_key, old_epoch)
                .is_none()
        );
        assert_eq!(
            state
                .active_selection(SelectionKind::Primary)
                .unwrap()
                .source_key,
            new_key
        );
    }

    #[test]
    fn stale_source_removal_cannot_clear_newer_selection() {
        let mut state = SelectionState::default();
        let old_key = SelectionSourceKey(1);
        let new_key = SelectionSourceKey(2);
        register_source(&mut state, old_key, SelectionSourceKind::DataControl);
        register_source(&mut state, new_key, SelectionSourceKind::DataControl);
        let old_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Primary, old_key, old_epoch)
            .unwrap();
        let new_epoch = epoch(&mut state);
        state
            .commit_selection(SelectionKind::Primary, new_key, new_epoch)
            .unwrap();
        let remove_epoch = epoch(&mut state);
        state.remove_source_key(old_key, remove_epoch);

        assert_eq!(
            state
                .active_selection(SelectionKind::Primary)
                .unwrap()
                .source_key,
            new_key
        );
    }
}
