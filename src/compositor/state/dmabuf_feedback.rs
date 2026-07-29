use super::*;

impl CompositorState {
    pub(in crate::compositor) fn set_dmabuf_feedback(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
    ) -> bool {
        self.set_dmabuf_feedback_with_scanout_capabilities(
            feedback,
            main_device,
            main_device_path,
            None,
        )
    }

    pub(in crate::compositor) fn set_dmabuf_feedback_with_scanout_capabilities(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
        scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
    ) -> bool {
        self.set_dmabuf_feedback_with_scanout_capabilities_and_target(
            feedback,
            main_device,
            main_device_path,
            scanout_capabilities,
            None,
        )
    }

    pub(in crate::compositor) fn set_dmabuf_feedback_with_scanout_capabilities_and_target(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
        scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
    ) -> bool {
        let main_device = main_device.filter(|device| *device != 0).unwrap_or(0);
        let main_device_path = main_device_path.filter(|path| !path.is_empty());
        let changed = self.dmabuf_feedback != feedback
            || self.dmabuf_main_device != main_device
            || self.dmabuf_main_device_path != main_device_path
            || self.dmabuf_scanout_capabilities != scanout_capabilities
            || self.dmabuf_scanout_target_device_override != scanout_target_device_override;
        self.dmabuf_feedback = feedback;
        self.dmabuf_main_device = main_device;
        self.dmabuf_main_device_path = main_device_path;
        self.dmabuf_scanout_capabilities = scanout_capabilities;
        self.dmabuf_scanout_target_device_override = scanout_target_device_override;
        changed
    }
}
