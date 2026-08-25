use super::*;

pub(crate) fn sync_xwayland_reactor_sources(
    event_loop: &mut NativeEventLoop,
    service: &mut XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
) -> NativeResult<()> {
    let mut last_synced_generation = 0;
    let _ = sync_xwayland_reactor_sources_with_generation(
        event_loop,
        service,
        tokens,
        &mut last_synced_generation,
    )?;
    Ok(())
}

pub(crate) fn sync_xwayland_reactor_sources_with_generation(
    event_loop: &mut NativeEventLoop,
    service: &mut XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
    last_synced_generation: &mut u64,
) -> NativeResult<bool> {
    let current_generation = service.reactor_registration_generation();
    if current_generation == *last_synced_generation {
        service.record_reactor_sync(false);
        return Ok(false);
    }
    service.record_reactor_sync(true);
    let desired: Vec<_> = service.reactor_registrations().collect();
    let mut retained = Vec::new();
    for (token, registration) in tokens.drain(..) {
        if desired.contains(&registration) {
            retained.push((token, registration));
        } else {
            let removed = event_loop.unregister(token)?;
            if removed {
                service.note_reactor_registration_with_token(
                    registration,
                    false,
                    Some(token.raw()),
                );
            }
        }
    }
    *tokens = retained;
    for registration in desired {
        if tokens.iter().any(|(_, current)| *current == registration) {
            continue;
        }
        let source = match registration.purpose {
            XwaylandReactorPurpose::ListenFilesystem | XwaylandReactorPurpose::ListenAbstract => {
                NativeEventSource::XwaylandListen
            }
            XwaylandReactorPurpose::DisplayReady => NativeEventSource::XwaylandDisplayReady,
            XwaylandReactorPurpose::Xwm => NativeEventSource::XwaylandXwm,
            XwaylandReactorPurpose::Stderr => NativeEventSource::XwaylandStderr,
        };
        let events = (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32
            | if registration.writable {
                libc::EPOLLOUT as u32
            } else {
                0
            };
        let token = event_loop.register_with_events(registration.fd, source, events)?;
        service.note_reactor_registration_with_token(registration, true, Some(token.raw()));
        tokens.push((token, registration));
    }
    service.finish_reactor_teardown()?;
    *last_synced_generation = service.reactor_registration_generation();
    Ok(true)
}
