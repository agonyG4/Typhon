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

#[allow(unused_imports)]
pub(in crate::compositor) use frames::*;
#[allow(unused_imports)]
pub(in crate::compositor) use helpers::*;
#[allow(unused_imports)]
pub(in crate::compositor) use hit_testing::*;
#[allow(unused_imports)]
pub(in crate::compositor) use input_dispatch::*;
#[allow(unused_imports)]
pub(in crate::compositor) use input_resources::*;
#[allow(unused_imports)]
pub(in crate::compositor) use pointer_constraints::*;
#[allow(unused_imports)]
pub(in crate::compositor) use resize::*;
#[allow(unused_imports)]
pub(in crate::compositor) use shutdown::*;
#[allow(unused_imports)]
pub(in crate::compositor) use subsurfaces::*;
#[allow(unused_imports)]
pub(in crate::compositor) use support_types::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_commits::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surfaces::*;
#[allow(unused_imports)]
pub(in crate::compositor) use windows::*;

#[cfg(test)]
mod task_05_8_tests;
