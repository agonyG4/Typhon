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
        source_surface_id: Option<u32>,
        source_fourcc: Option<u32>,
    ) -> DmabufFeedbackDoctorState {
        let hinted_surface = self
            .dmabuf_scanout_candidate_surface
            .filter(|surface_id| self.dmabuf_surface_scanout_hints.contains(surface_id))
            .or_else(|| self.dmabuf_surface_scanout_hints.iter().copied().min());
        let (live_resources, snapshots) = source_surface_id
            .map(|source_surface_id| {
                let bindings = self
                    .dmabuf_feedback_resources
                    .values()
                    .filter_map(|resource| {
                        let feedback = resource.resource.upgrade().ok()?;
                        let data = feedback.data::<DmabufFeedbackResourceData>()?;
                        let binding = data.binding.lock().ok()?;
                        Some((binding.scope(), binding.last_snapshot().cloned()))
                    });
                collect_surface_feedback_snapshots(source_surface_id, bindings)
            })
            .unwrap_or_default();
        let (
            snapshot_variants,
            snapshots_consistent,
            advertised_scanout_pair_count,
            advertised_source_modifiers,
            advertised_fallback_pair_count,
            advertised_fallback_source_modifiers,
        ) = source_fourcc
            .map(|source_fourcc| summarize_surface_feedback_snapshots(&snapshots, source_fourcc))
            .unwrap_or((0, true, None, None, None, None));
        let scanout_capability_pair_count = self
            .dmabuf_scanout_capabilities
            .as_ref()
            .map_or(0, |capabilities| capabilities.formats.len());
        let scanout_capability_source_modifiers = source_fourcc
            .and_then(|source_fourcc| {
                self.dmabuf_scanout_capabilities
                    .as_ref()
                    .map(|capabilities| {
                        let mut modifiers = capabilities
                            .formats
                            .iter()
                            .filter(|capability| capability.format == source_fourcc)
                            .map(|capability| capability.modifier)
                            .collect::<Vec<_>>();
                        modifiers.sort_unstable();
                        modifiers.dedup();
                        modifiers
                    })
            })
            .unwrap_or_default();

        DmabufFeedbackDoctorState {
            hint_active: hinted_surface.is_some(),
            hint_surface: hinted_surface,
            updates: self.dmabuf_feedback_updates,
            duplicate_suppressed: self.dmabuf_feedback_duplicate_suppressed,
            live_resources,
            snapshot_variants,
            snapshots_consistent,
            scanout_capability_pair_count,
            scanout_capability_source_modifiers,
            advertised_scanout_pair_count,
            advertised_source_modifiers,
            advertised_fallback_pair_count,
            advertised_fallback_source_modifiers,
            dmabuf_kms_preferred_requested: self.dmabuf_kms_preferred_state.requested,
            dmabuf_kms_preferred_effective: self.dmabuf_kms_preferred_state.effective,
            dmabuf_renderer_pairs_raw: self.dmabuf_kms_preferred_state.renderer_pairs_raw,
            dmabuf_kms_presentable_pairs: self.dmabuf_kms_preferred_state.kms_presentable_pairs,
            dmabuf_renderer_pairs_advertised: self
                .dmabuf_kms_preferred_state
                .renderer_pairs_advertised,
            dmabuf_renderer_pairs_removed: self.dmabuf_kms_preferred_state.renderer_pairs_removed,
        }
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
        self.set_dmabuf_feedback_with_scanout_capabilities_and_target_and_kms_preferred(
            feedback,
            main_device,
            main_device_path,
            scanout_capabilities,
            scanout_target_device_override,
            DmabufKmsPreferredState::default(),
        )
    }

    pub(in crate::compositor) fn set_dmabuf_feedback_with_scanout_capabilities_and_target_and_kms_preferred(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
        scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
        dmabuf_kms_preferred_state: DmabufKmsPreferredState,
    ) -> bool {
        let main_device = main_device.filter(|device| *device != 0).unwrap_or(0);
        let main_device_path = main_device_path.filter(|path| !path.is_empty());
        let changed = self.dmabuf_feedback != feedback
            || self.dmabuf_main_device != main_device
            || self.dmabuf_main_device_path != main_device_path
            || self.dmabuf_scanout_capabilities != scanout_capabilities
            || self.dmabuf_scanout_target_device_override != scanout_target_device_override
            || self.dmabuf_kms_preferred_state != dmabuf_kms_preferred_state;
        self.dmabuf_feedback = feedback;
        self.dmabuf_main_device = main_device;
        self.dmabuf_main_device_path = main_device_path;
        self.dmabuf_scanout_capabilities = scanout_capabilities;
        self.dmabuf_scanout_target_device_override = scanout_target_device_override;
        self.dmabuf_kms_preferred_state = dmabuf_kms_preferred_state;
        if changed {
            self.reconcile_all_dmabuf_feedback();
        }
        changed
    }
}

fn collect_surface_feedback_snapshots<I>(
    source_surface_id: u32,
    bindings: I,
) -> (usize, Vec<DmabufFeedbackSnapshot>)
where
    I: IntoIterator<Item = (DmabufFeedbackScope, Option<DmabufFeedbackSnapshot>)>,
{
    let mut live_resources = 0usize;
    let mut snapshots = Vec::new();
    for (scope, snapshot) in bindings {
        if scope != DmabufFeedbackScope::Surface(source_surface_id) {
            continue;
        }
        live_resources = live_resources.saturating_add(1);
        if let Some(snapshot) = snapshot {
            snapshots.push(snapshot);
        }
    }
    (live_resources, snapshots)
}

type DmabufFeedbackSnapshotSummary = (
    usize,
    bool,
    Option<usize>,
    Option<Vec<u64>>,
    Option<usize>,
    Option<Vec<u64>>,
);

fn summarize_surface_feedback_snapshots(
    snapshots: &[DmabufFeedbackSnapshot],
    source_fourcc: u32,
) -> DmabufFeedbackSnapshotSummary {
    let mut variants = Vec::new();
    for snapshot in snapshots {
        if !variants.contains(snapshot) {
            variants.push(snapshot.clone());
        }
    }
    let snapshots_consistent = variants.len() <= 1;
    let (
        advertised_scanout_pair_count,
        advertised_source_modifiers,
        advertised_fallback_pair_count,
        advertised_fallback_source_modifiers,
    ) = if let Some(snapshot) = variants.first().filter(|_| snapshots_consistent) {
        (
            Some(snapshot.scanout_pair_count()),
            Some(snapshot.scanout_modifiers_for_fourcc(source_fourcc)),
            Some(snapshot.fallback_pair_count()),
            Some(snapshot.fallback_modifiers_for_fourcc(source_fourcc)),
        )
    } else {
        (None, None, None, None)
    };
    (
        variants.len(),
        snapshots_consistent,
        advertised_scanout_pair_count,
        advertised_source_modifiers,
        advertised_fallback_pair_count,
        advertised_fallback_source_modifiers,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::gpu_protocol_capabilities::GpuFormat;
    use crate::render_backend::egl_gles::EglGlesDmabufFormat;

    #[test]
    fn advertised_snapshot_summary_deduplicates_consistent_live_resources() {
        let first = snapshot_with_scanout_modifier(7);
        let summary = summarize_surface_feedback_snapshots(
            &[first.clone(), first],
            DrmFormat::Xrgb8888.as_fourcc(),
        );

        assert_eq!(
            summary,
            (1, true, Some(1), Some(vec![7]), Some(1), Some(Vec::new()))
        );
    }

    #[test]
    fn advertised_snapshot_summary_withholds_effective_pairs_when_inconsistent() {
        let summary = summarize_surface_feedback_snapshots(
            &[
                snapshot_with_scanout_modifier(7),
                snapshot_with_scanout_modifier(9),
            ],
            DrmFormat::Xrgb8888.as_fourcc(),
        );

        assert_eq!(summary, (2, false, None, None, None, None));
    }

    #[test]
    fn advertised_snapshot_summary_reports_actual_fallback_pairs() {
        let snapshot = snapshot_with_scanout_and_fallback(7, [9, 10]);

        assert_eq!(
            summarize_surface_feedback_snapshots(
                std::slice::from_ref(&snapshot),
                DrmFormat::Xrgb8888.as_fourcc(),
            ),
            (1, true, Some(1), Some(vec![7]), Some(2), Some(vec![9, 10]))
        );
    }

    #[test]
    fn actual_snapshot_collection_ignores_other_and_inert_scopes() {
        let source_snapshot = snapshot_with_scanout_modifier(7);
        let (live_resources, snapshots) = collect_surface_feedback_snapshots(
            35,
            [
                (
                    DmabufFeedbackScope::Surface(35),
                    Some(source_snapshot.clone()),
                ),
                (
                    DmabufFeedbackScope::Surface(36),
                    Some(snapshot_with_scanout_modifier(9)),
                ),
                (DmabufFeedbackScope::InertSurface, Some(source_snapshot)),
                (DmabufFeedbackScope::Surface(35), None),
            ],
        );

        assert_eq!(live_resources, 2);
        assert_eq!(snapshots, vec![snapshot_with_scanout_modifier(7)]);
    }

    #[test]
    fn capability_and_advertised_modifier_views_remain_independent() {
        let capability_modifier = 0x10;
        let advertised_modifier = 0x20;
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier: capability_modifier,
            }],
        );
        let advertised = snapshot_with_scanout_modifier(advertised_modifier);

        assert_eq!(
            capabilities
                .formats
                .iter()
                .filter(|capability| capability.format == DrmFormat::Xrgb8888.as_fourcc())
                .map(|capability| capability.modifier)
                .collect::<Vec<_>>(),
            vec![capability_modifier]
        );
        assert_eq!(
            advertised.scanout_modifiers_for_fourcc(DrmFormat::Xrgb8888.as_fourcc()),
            vec![advertised_modifier]
        );
    }

    fn snapshot_with_scanout_modifier(modifier: u64) -> DmabufFeedbackSnapshot {
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier,
            }],
        );
        DmabufFeedbackData::build_snapshot(
            &EglGlesDmabufFeedback::with_scanout_tranche(
                [],
                [EglGlesDmabufFormat::new(
                    DrmFormat::Argb8888,
                    DrmModifier::LINEAR,
                )],
            ),
            0x100,
            &[
                GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), modifier),
                GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
            ],
            Some(&capabilities),
            None,
        )
        .unwrap()
    }

    fn snapshot_with_scanout_and_fallback(
        scanout_modifier: u64,
        fallback_modifiers: impl IntoIterator<Item = u64>,
    ) -> DmabufFeedbackSnapshot {
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier: scanout_modifier,
            }],
        );
        let fallback_formats = fallback_modifiers
            .into_iter()
            .map(|modifier| EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(modifier)))
            .collect::<Vec<_>>();
        let allowed_formats = std::iter::once(GpuFormat::new(
            DrmFormat::Xrgb8888.as_fourcc(),
            scanout_modifier,
        ))
        .chain(
            fallback_formats
                .iter()
                .map(|format| GpuFormat::new(format.format.as_fourcc(), format.modifier.0)),
        )
        .collect::<Vec<_>>();
        DmabufFeedbackData::build_snapshot(
            &EglGlesDmabufFeedback::with_scanout_tranche(
                [EglGlesDmabufFormat::new(
                    DrmFormat::Xrgb8888,
                    DrmModifier(scanout_modifier),
                )],
                fallback_formats,
            ),
            0x100,
            &allowed_formats,
            Some(&capabilities),
            None,
        )
        .unwrap()
    }
}
