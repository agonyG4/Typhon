//! Small test-only X11 connection helper shared by native compatibility tests.

use std::{io, os::unix::net::UnixStream, path::Path};

pub fn connect_filesystem_socket(path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(path)
}
