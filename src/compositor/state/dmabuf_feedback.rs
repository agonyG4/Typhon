use super::*;

impl CompositorState {
    pub(in crate::compositor) fn dmabuf_feedback_snapshot_for_scope(
        &self,
        scope: DmabufFeedbackScope,
    ) -> io::Result<Option<DmabufFeedbackSnapshot>> {
        let scanout_capabilities = match scope {
            DmabufFeedbackScope::Surface(surface_id)
                if self.dmabuf_surface_scanout_hints.contains(&surface_id) =>
            {
                self.dmabuf_scanout_capabilities.as_ref()
            }
            DmabufFeedbackScope::Default | DmabufFeedbackScope::Surface(_) => None,
            DmabufFeedbackScope::InertSurface => return Ok(None),
        };
        DmabufFeedbackData::build_snapshot(
            &self.dmabuf_feedback,
            self.dmabuf_main_device,
            self.gpu_protocol_capabilities.dmabuf_formats(),
            scanout_capabilities,
            self.dmabuf_scanout_target_device_override,
        )
        .map(Some)
    }

    pub(in crate::compositor) fn register_dmabuf_feedback_resource(
        &mut self,
        feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
    ) {
        self.dmabuf_feedback_resources.insert(
            feedback.id(),
            LiveDmabufFeedbackResource {
                resource: feedback.downgrade(),
            },
        );
    }

    pub(in crate::compositor) fn unregister_dmabuf_feedback_resource(
        &mut self,
        feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
    ) {
        self.dmabuf_feedback_resources.remove(&feedback.id());
    }

    fn update_dmabuf_feedback_resource(
        &mut self,
        resource_id: ObjectId,
        snapshot: DmabufFeedbackSnapshot,
    ) {
        let Some(resource) = self.dmabuf_feedback_resources.get(&resource_id) else {
            return;
        };
        let Some(resource) = resource.resource.upgrade().ok() else {
            self.dmabuf_feedback_resources.remove(&resource_id);
            return;
        };
        let Some(resource_data) = resource.data::<DmabufFeedbackResourceData>() else {
            self.dmabuf_feedback_resources.remove(&resource_id);
            return;
        };
        let Ok(mut binding) = resource_data.binding.lock() else {
            return;
        };
        if binding.scope() == DmabufFeedbackScope::InertSurface {
            return;
        }
        let changed = match binding.replace_snapshot(snapshot) {
            Ok(changed) => changed,
            Err(_) => return,
        };
        if changed {
            self.dmabuf_feedback_updates = self.dmabuf_feedback_updates.saturating_add(1);
            drop(binding);
            send_dmabuf_feedback_resource(&resource);
        } else {
            self.dmabuf_feedback_duplicate_suppressed =
                self.dmabuf_feedback_duplicate_suppressed.saturating_add(1);
        }
    }

    fn reconcile_dmabuf_feedback_resource(&mut self, resource_id: ObjectId) {
        let Some(resource) = self.dmabuf_feedback_resources.get(&resource_id) else {
            return;
        };
        let Some(resource) = resource.resource.upgrade().ok() else {
            self.dmabuf_feedback_resources.remove(&resource_id);
            return;
        };
        let Some(resource_data) = resource.data::<DmabufFeedbackResourceData>() else {
            self.dmabuf_feedback_resources.remove(&resource_id);
            return;
        };
        let Ok(binding) = resource_data.binding.lock() else {
            return;
        };
        let scope = binding.scope();
        drop(binding);
        let Ok(Some(snapshot)) = self.dmabuf_feedback_snapshot_for_scope(scope) else {
            return;
        };
        self.update_dmabuf_feedback_resource(resource_id, snapshot);
    }

    pub(in crate::compositor) fn reconcile_all_dmabuf_feedback(&mut self) {
        let resource_ids = self
            .dmabuf_feedback_resources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for resource_id in resource_ids {
            self.reconcile_dmabuf_feedback_resource(resource_id);
        }
    }

    pub(in crate::compositor) fn activate_surface_scanout_hint(&mut self, surface_id: u32) {
        if self.dmabuf_surface_scanout_hints.insert(surface_id) {
            self.reconcile_all_dmabuf_feedback_for_surface(surface_id);
        }
    }

    pub(in crate::compositor) fn clear_surface_scanout_hint(&mut self, surface_id: u32) {
        if self.dmabuf_surface_scanout_hints.remove(&surface_id) {
            self.reconcile_all_dmabuf_feedback_for_surface(surface_id);
        }
    }

    fn reconcile_all_dmabuf_feedback_for_surface(&mut self, surface_id: u32) {
        let resource_ids = self
            .dmabuf_feedback_resources
            .iter()
            .filter_map(|(resource_id, resource)| {
                let feedback = resource.resource.upgrade().ok()?;
                let data = feedback.data::<DmabufFeedbackResourceData>()?;
                let binding = data.binding.lock().ok()?;
                (binding.scope() == DmabufFeedbackScope::Surface(surface_id))
                    .then_some(resource_id.clone())
            })
            .collect::<Vec<_>>();
        for resource_id in resource_ids {
            self.reconcile_dmabuf_feedback_resource(resource_id);
        }
    }

    pub(in crate::compositor) fn reconcile_surface_scanout_candidate(
        &mut self,
        candidate_surface_id: Option<u32>,
    ) {
        if self.dmabuf_scanout_candidate_surface == candidate_surface_id {
            return;
        }
        if let Some(previous_surface_id) = self.dmabuf_scanout_candidate_surface
            && Some(previous_surface_id) != candidate_surface_id
        {
            self.clear_surface_scanout_hint(previous_surface_id);
        }
        self.dmabuf_scanout_candidate_surface = candidate_surface_id;
    }

    pub(in crate::compositor) fn detach_dmabuf_surface(&mut self, surface_id: u32) {
        self.dmabuf_surface_scanout_hints.remove(&surface_id);
        if self.dmabuf_scanout_candidate_surface == Some(surface_id) {
            self.dmabuf_scanout_candidate_surface = None;
        }
        let resource_ids = self
            .dmabuf_feedback_resources
            .iter()
            .filter_map(|(resource_id, resource)| {
                let feedback = resource.resource.upgrade().ok()?;
                let data = feedback.data::<DmabufFeedbackResourceData>()?;
                let binding = data.binding.lock().ok()?;
                (binding.scope() == DmabufFeedbackScope::Surface(surface_id))
                    .then_some(resource_id.clone())
            })
            .collect::<Vec<_>>();
        for resource_id in resource_ids {
            let Some(resource) = self.dmabuf_feedback_resources.get(&resource_id) else {
                continue;
            };
            let Some(feedback) = resource.resource.upgrade().ok() else {
                self.dmabuf_feedback_resources.remove(&resource_id);
                continue;
            };
            if let Some(data) = feedback.data::<DmabufFeedbackResourceData>()
                && let Ok(mut binding) = data.binding.lock()
            {
                binding.make_inert();
            }
        }
    }

    pub(in crate::compositor) fn dmabuf_feedback_doctor_state(
        &self,
    ) -> (bool, Option<u32>, u64, u64) {
        let hinted_surface = self
            .dmabuf_scanout_candidate_surface
            .filter(|surface_id| self.dmabuf_surface_scanout_hints.contains(surface_id))
            .or_else(|| self.dmabuf_surface_scanout_hints.iter().copied().min());
        (
            hinted_surface.is_some(),
            hinted_surface,
            self.dmabuf_feedback_updates,
            self.dmabuf_feedback_duplicate_suppressed,
        )
    }

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
        if changed {
            self.reconcile_all_dmabuf_feedback();
        }
        changed
    }
}
