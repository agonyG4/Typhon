use super::*;

pub(crate) fn sync_xwayland_reactor_sources(
    event_loop: &mut NativeEventLoop,
    service: &mut XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
) -> NativeResult<()> {
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
    Ok(())
}
