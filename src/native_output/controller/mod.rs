mod device;
mod frame;
mod manager;
mod policy;
pub(crate) mod semantic;

pub(crate) use device::ControllerDeviceId;
// Preserve the physical vocabulary for future internal mapping configuration.
#[allow(unused_imports)]
pub(crate) use frame::ControllerButton;
pub(crate) use manager::{ControllerManager, MAX_CONTROLLER_DEVICES};
pub(crate) use policy::ControllerPolicy;

#[cfg(test)]
mod tests;
