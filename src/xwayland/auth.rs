use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::fs;

use super::fs_security;

const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";
const FAMILY_LOCAL: u16 = 256;

pub(crate) struct AuthFile {
    pub(crate) path: PathBuf,
}

pub(crate) fn create_auth_file_at(
    directory_fd: RawFd,
    directory: &Path,
    display_number: u32,
) -> io::Result<AuthFile> {
    let mut suffix_bytes = vec![0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut suffix_bytes)?;
    let suffix = suffix_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let path = directory.join(format!(".Xauthority-{display_number}-{suffix}"));
    let mut cookie = vec![0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut cookie)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "authority file has no name"))?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "authority name contains NUL"))?;
    let fd = unsafe {
        libc::openat(
            directory_fd,
            name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let identity = match fs_security::Identity::from_fd(file.as_raw_fd()) {
        Ok(identity) => identity,
        Err(error) => {
            drop(file);
            return Err(error);
        }
    };
    let result = (|| {
        file.write_all(&authority_record(display_number, &cookie)?)?;
        file.flush()?;
        file.sync_all()?;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok::<(), io::Error>(())
    })();
    if let Err(error) = result {
        drop(file);
        let _ = fs_security::unlink_owned_at(directory_fd, name, identity);
        return Err(error);
    }
    Ok(AuthFile { path })
}

fn authority_record(display_number: u32, cookie: &[u8]) -> io::Result<Vec<u8>> {
    let number = display_number.to_string();
    let mut record = Vec::new();
    append_field(&mut record, FAMILY_LOCAL.to_be_bytes().as_slice())?;
    append_field(&mut record, &[])?;
    append_field(&mut record, number.as_bytes())?;
    append_field(&mut record, COOKIE_NAME)?;
    append_field(&mut record, cookie)?;
    Ok(record)
}

fn append_field(record: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    let length = u16::try_from(value.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Xauthority field exceeds 16-bit length",
        )
    })?;
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
pub(crate) fn read_cookie_for_tests(path: &Path) -> io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    let mut cursor = 0usize;
    let family = read_field(&bytes, &mut cursor)?;
    if family.len() != 2 || u16::from_be_bytes([family[0], family[1]]) != FAMILY_LOCAL {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Xauthority family is not local",
        ));
    }
    let _address = read_field(&bytes, &mut cursor)?;
    let _number = read_field(&bytes, &mut cursor)?;
    let name = read_field(&bytes, &mut cursor)?;
    if name != COOKIE_NAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Xauthority cookie name is not MIT-MAGIC-COOKIE-1",
        ));
    }
    let cookie = read_field(&bytes, &mut cursor)?.to_vec();
    if cookie.len() < 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Xauthority cookie is too short",
        ));
    }
    Ok(cookie)
}

#[cfg(test)]
fn read_field<'a>(bytes: &'a [u8], cursor: &mut usize) -> io::Result<&'a [u8]> {
    let end = cursor
        .checked_add(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Xauthority length overflow"))?;
    let length = bytes
        .get(*cursor..end)
        .map(|field| u16::from_be_bytes([field[0], field[1]]) as usize)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated Xauthority field"))?;
    *cursor = end;
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Xauthority length overflow"))?;
    let field = bytes
        .get(*cursor..end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated Xauthority data"))?;
    *cursor = end;
    Ok(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_authority_field_is_rejected() {
        assert!(authority_record(0, &vec![0u8; usize::from(u16::MAX) + 1]).is_err());
    }
}
