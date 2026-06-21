# Native Explicit-Sync Eventfd Integration

Date: 2026-06-21

## Delivered Path

Native GPU-buffer globals use a syncobj device duplicated from the active DRM
file description. Each unsignaled dmabuf acquire point receives a unique
compositor commit ID. The native watch registry creates an
`EFD_NONBLOCK | EFD_CLOEXEC` eventfd, asks
`DRM_IOCTL_SYNCOBJ_EVENTFD` to notify signal completion with flags zero, and
registers the eventfd dynamically with epoll.

Registry identity includes a generation-safe reactor token, DRM file
generation, imported timeline ownership, point, surface, buffer, and exact
commit ID. Numeric fds and syncobj handles are not durable identity. After an
eventfd wakeup, Typhon drains one full `u64` counter and performs a final
nonblocking timeline check before marking the exact commit ready.

## Races And Lifecycle

Registration checks readiness before creating a watch and again after ioctl and
epoll registration. A point signaling between those operations is observed by
the final check or remains level-triggered readable in epoll. Already-signaled
points use the same exact commit transition and retain no watch.

Supersession and surface, buffer, sync-surface, or timeline destruction remove
the compositor commit and emit exact cancellation. The registry executes
`EPOLL_CTL_DEL` before dropping its eventfd. Reactor slot generations make an
event already returned for a canceled source stale. Each backend run receives a
new nonzero DRM-file generation, preventing an old watch from matching a
recreated backend even if handle values repeat.

Acquire readiness remains separate from frame completion. While a pageflip is
outstanding, a ready future commit is not promoted into that older frame's
callback, presentation, buffer-release, or explicit-release batch. Acquire
wakeup and cancellation never signal release.

## Fallback

`ENOTTY`, `EOPNOTSUPP`, and `ENOSYS` classify syncobj eventfd as unsupported.
Other operational driver rejections enter `BrokenOrRejected(errno)`, while
`EINVAL` and `EBADF` remain hard errors because they indicate invalid local or
imported state. Eventfd or epoll resource failure preserves correctness through
the per-commit fallback.

Fallback entries are checked immediately and then from one absolute monotonic
deadline derived from output refresh. The next deadline advances from its prior
absolute boundary and skips missed boundaries, avoiding relative-sleep drift.
The timer is disarmed at zero entries. Eventfd-backed watches are never scanned,
and a deadline never fabricates readiness.

## Validation Boundary

Portable tests cover UAPI layout and flags, errno classification, active-file
duplication, reactor token/fd reuse, setup signaling, counters greater than one,
duplicate and stale wakeups, cancellation, capability fallback, absolute
deadlines, and at-most-once commit readiness. Notifier behavior is injected, so
these tests do not require physical DRM hardware.

Real NVIDIA/TTY, browser video, Sober, VT-switch, 165 Hz, idle CPU, and measured
latency validation remain hardware work and must not be inferred from the
portable suite.
