use super::*;

mod cycle;
mod frame;

pub(crate) use cycle::*;
pub(crate) use frame::*;

pub(crate) struct NativeRuntimeConfig {
    pub(crate) server: OwnCompositorServer,
    pub(crate) app: Vec<String>,
    pub(crate) app_gpu_preference: CompositorAppGpuPreference,
}

pub(crate) struct NativeRuntime {
    config: NativeRuntimeConfig,
}

impl NativeRuntime {
    pub(crate) fn bootstrap(config: NativeRuntimeConfig) -> NativeResult<Self> {
        Ok(Self { config })
    }

    pub(crate) fn run(self) -> NativeResult<()> {
        let NativeRuntimeConfig {
            server,
            app,
            app_gpu_preference,
        } = self.config;
        run_legacy_native_runtime(server, app, app_gpu_preference)
    }
}
