#![allow(clippy::too_many_arguments)]

use super::*;
use crate::compositor::layer_shell::CapturedLayerSurfaceCommitState;
use crate::compositor::state_data::SurfaceContentMapping;

mod admission;
mod apply;
mod tree;
