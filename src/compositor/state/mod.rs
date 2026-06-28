#![allow(private_interfaces, unused_imports)]

use super::*;

mod frames;
mod helpers;
mod hit_testing;
mod input_dispatch;
mod input_resources;
mod pointer_constraints;
mod resize;
mod shutdown;
mod subsurfaces;
mod support_types;
mod surface_commits;
mod surfaces;
mod windows;

pub(crate) use frames::*;
pub(crate) use helpers::*;
pub(crate) use hit_testing::*;
pub(crate) use input_dispatch::*;
pub(crate) use input_resources::*;
pub(crate) use pointer_constraints::*;
pub(crate) use resize::*;
pub(crate) use shutdown::*;
pub(crate) use subsurfaces::*;
pub(crate) use support_types::*;
pub(crate) use surface_commits::*;
pub(crate) use surfaces::*;
pub(crate) use windows::*;

#[cfg(test)]
mod task_05_8_tests;
