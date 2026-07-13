#![allow(dead_code)] // The explicit Atomic runtime wires fence export in Task 12.

use std::{
    ffi::c_void,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use glow::HasContext;
use khronos_egl as egl;

use super::{EglInstance, RendererResult};

const EGL_SYNC_NATIVE_FENCE_ANDROID: egl::Enum = 0x3144;

type EglDupNativeFenceFdAndroid =
    unsafe extern "system" fn(egl::EGLDisplay, egl::EGLSync) -> egl::Int;

#[derive(Clone, Copy)]
pub(crate) struct NativeFenceFunctions {
    dup_native_fence_fd_android: EglDupNativeFenceFdAndroid,
}

impl NativeFenceFunctions {
    pub(crate) fn load(egl: &EglInstance, display: egl::Display) -> RendererResult<Self> {
        let extensions = egl
            .query_string(Some(display), egl::EXTENSIONS)?
            .to_string_lossy();
        if !extensions
            .split_ascii_whitespace()
            .any(|extension| extension == "EGL_ANDROID_native_fence_sync")
        {
            return Err(io::Error::other(
                "explicit Atomic EGL/GBM requires EGL_ANDROID_native_fence_sync",
            )
            .into());
        }
        Self::load_with(|name| {
            egl.get_proc_address(name)
                .map(|symbol| symbol as *const c_void)
        })
    }

    fn load_with(mut load: impl FnMut(&str) -> Option<*const c_void>) -> RendererResult<Self> {
        let symbol = load("eglDupNativeFenceFDANDROID")
            .filter(|symbol| !symbol.is_null())
            .ok_or_else(|| io::Error::other("eglDupNativeFenceFDANDROID is unavailable"))?;
        Ok(Self {
            dup_native_fence_fd_android: unsafe {
                std::mem::transmute::<*const c_void, EglDupNativeFenceFdAndroid>(symbol)
            },
        })
    }
}

#[derive(Debug)]
pub(crate) struct NativeRenderFence {
    submission_fd: Option<OwnedFd>,
    timing_fd: Option<OwnedFd>,
}

impl NativeRenderFence {
    pub(crate) fn create(
        egl: &EglInstance,
        display: egl::Display,
        gl: &glow::Context,
        functions: NativeFenceFunctions,
    ) -> RendererResult<Self> {
        let sync = unsafe {
            egl.create_sync(display, EGL_SYNC_NATIVE_FENCE_ANDROID, &[egl::ATTRIB_NONE])?
        };
        unsafe { gl.flush() };
        let raw_fd =
            unsafe { (functions.dup_native_fence_fd_android)(display.as_ptr(), sync.as_ptr()) };
        let destroy_result = unsafe { egl.destroy_sync(display, sync) };
        if raw_fd < 0 {
            return Err(io::Error::other("failed to export EGL native fence FD").into());
        }
        let submission_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        destroy_result?;
        Ok(Self::from_submission_fd(submission_fd))
    }

    pub(crate) fn from_submission_fd(submission_fd: OwnedFd) -> Self {
        Self::from_submission_fd_with(submission_fd, duplicate_cloexec)
    }

    fn from_submission_fd_with(
        submission_fd: OwnedFd,
        duplicate: impl FnOnce(i32) -> io::Result<OwnedFd>,
    ) -> Self {
        let timing_fd = duplicate(submission_fd.as_raw_fd()).ok();
        Self {
            submission_fd: Some(submission_fd),
            timing_fd,
        }
    }

    pub(crate) fn take_submission_fd(&mut self) -> io::Result<OwnedFd> {
        self.submission_fd
            .take()
            .ok_or_else(|| io::Error::other("native render submission fence already consumed"))
    }

    pub(crate) fn timing_fd(&self) -> Option<&OwnedFd> {
        self.timing_fd.as_ref()
    }

    pub(crate) fn take_timing_fd(&mut self) -> Option<OwnedFd> {
        self.timing_fd.take()
    }
}

fn duplicate_cloexec(fd: i32) -> io::Result<OwnedFd> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd, FromRawFd};

    use super::*;

    unsafe extern "system" fn fake_dup(_display: egl::EGLDisplay, _sync: egl::EGLSync) -> egl::Int {
        -1
    }

    fn pipe_read_end() -> OwnedFd {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { libc::close(pipe[1]) };
        unsafe { OwnedFd::from_raw_fd(pipe[0]) }
    }

    #[test]
    fn extension_loader_requires_native_fence_dup_entry_point() {
        assert!(NativeFenceFunctions::load_with(|_| None).is_err());
        let functions = NativeFenceFunctions::load_with(|name| {
            (name == "eglDupNativeFenceFDANDROID").then_some(fake_dup as *const c_void)
        });
        assert!(functions.is_ok());
    }

    #[test]
    fn submission_fence_moves_exactly_once_and_timing_fence_is_independent() {
        let submission = pipe_read_end();
        let raw_submission = submission.as_raw_fd();
        let mut fence = NativeRenderFence::from_submission_fd(submission);
        let raw_timing = fence.timing_fd().unwrap().as_raw_fd();
        assert_ne!(raw_submission, raw_timing);

        let moved = fence.take_submission_fd().unwrap();
        assert_eq!(moved.as_raw_fd(), raw_submission);
        assert!(fence.take_submission_fd().is_err());
        assert_eq!(fence.take_timing_fd().unwrap().as_raw_fd(), raw_timing);
        assert!(fence.take_timing_fd().is_none());
    }

    #[test]
    fn reactor_can_borrow_timing_fd_without_taking_ownership() {
        let mut fence = NativeRenderFence::from_submission_fd(pipe_read_end());
        let borrowed = fence.timing_fd().unwrap().as_raw_fd();
        assert_eq!(fence.timing_fd().unwrap().as_raw_fd(), borrowed);
        assert_eq!(fence.take_timing_fd().unwrap().as_raw_fd(), borrowed);
    }

    #[test]
    fn timing_fence_duplication_failure_keeps_valid_submission_fence() {
        let submission = pipe_read_end();
        let raw_submission = submission.as_raw_fd();
        let mut fence = NativeRenderFence::from_submission_fd_with(submission, |_| {
            Err(io::Error::from_raw_os_error(libc::EMFILE))
        });

        assert!(fence.timing_fd().is_none());
        assert_eq!(
            fence.take_submission_fd().unwrap().as_raw_fd(),
            raw_submission
        );
    }
}
