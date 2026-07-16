use std::{io, num::NonZeroU64, os::fd::RawFd};

use super::{
    XwaylandAppEnvironment, XwaylandGeneration, XwaylandMode, config::XwaylandConfig,
    metrics::XwaylandMetrics, next_nonzero,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandStateKind {
    Disabled,
    Armed,
    Starting,
    RunningBase,
    Backoff,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandReactorPurpose {
    Listen,
    DisplayReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandReactorRegistration {
    pub fd: RawFd,
    pub generation: Option<XwaylandGeneration>,
    pub purpose: XwaylandReactorPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XwaylandState {
    Disabled,
    Armed,
    Starting(XwaylandGeneration),
    RunningBase(XwaylandGeneration),
    Backoff,
    Failed,
}

#[derive(Debug)]
pub struct XwaylandService {
    pub(crate) mode: XwaylandMode,
    pub(crate) state: XwaylandState,
    pub(crate) next_generation: NonZeroU64,
    pub(crate) config: XwaylandConfig,
    pub(crate) metrics: XwaylandMetrics,
}

impl XwaylandService {
    pub fn bootstrap() -> io::Result<Self> {
        Self::bootstrap_with_config(XwaylandConfig::from_environment())
    }

    pub fn bootstrap_with_config(config: XwaylandConfig) -> io::Result<Self> {
        let mode = config.mode;
        Ok(Self {
            mode,
            state: if mode.is_enabled() {
                XwaylandState::Armed
            } else {
                XwaylandState::Disabled
            },
            next_generation: NonZeroU64::new(1).expect("one is nonzero"),
            config,
            metrics: XwaylandMetrics::default(),
        })
    }

    pub fn state_kind(&self) -> XwaylandStateKind {
        match self.state {
            XwaylandState::Disabled => XwaylandStateKind::Disabled,
            XwaylandState::Armed => XwaylandStateKind::Armed,
            XwaylandState::Starting(_) => XwaylandStateKind::Starting,
            XwaylandState::RunningBase(_) => XwaylandStateKind::RunningBase,
            XwaylandState::Backoff => XwaylandStateKind::Backoff,
            XwaylandState::Failed => XwaylandStateKind::Failed,
        }
    }

    pub fn app_environment(&self) -> Option<XwaylandAppEnvironment> {
        None
    }

    pub fn reactor_registrations(&self) -> impl Iterator<Item = XwaylandReactorRegistration> {
        std::iter::empty()
    }

    pub(crate) fn allocate_generation(&mut self) -> XwaylandGeneration {
        self.metrics.generations_started = self.metrics.generations_started.saturating_add(1);
        next_nonzero(&mut self.next_generation)
    }

    pub(crate) fn mark_starting(&mut self, generation: XwaylandGeneration) {
        self.state = XwaylandState::Starting(generation);
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
    }

    pub(crate) fn generation(&self) -> Option<XwaylandGeneration> {
        match self.state {
            XwaylandState::Starting(generation) | XwaylandState::RunningBase(generation) => {
                Some(generation)
            }
            XwaylandState::Disabled
            | XwaylandState::Armed
            | XwaylandState::Backoff
            | XwaylandState::Failed => None,
        }
    }
}

#[allow(dead_code)]
fn _keep_public_purpose_in_scope(_: XwaylandReactorPurpose) {}
