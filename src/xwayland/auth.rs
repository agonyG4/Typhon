use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";
const FAMILY_LOCAL: u16 = 256;

pub(crate) struct AuthFile {
    pub(crate) path: PathBuf,
    pub(crate) cookie: Vec<u8>,
}

pub(crate) fn create_auth_file(directory: &Path, display_number: u32) -> io::Result<AuthFile> {
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;

    let path = directory.join(format!(".Xauthority-{display_number}"));
    let mut cookie = vec![0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut cookie)?;
    let mut file = OpenOptions::new();
    file.write(true)
        .create_new(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .mode(0o600);
    let mut file = file.open(&path)?;
    file.write_all(&authority_record(display_number, &cookie))?;
    file.flush()?;
    file.sync_all()?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(AuthFile { path, cookie })
}

fn authority_record(display_number: u32, cookie: &[u8]) -> Vec<u8> {
    let number = display_number.to_string();
    let mut record = Vec::new();
    append_field(&mut record, FAMILY_LOCAL.to_be_bytes().as_slice());
    append_field(&mut record, &[]);
    append_field(&mut record, number.as_bytes());
    append_field(&mut record, COOKIE_NAME);
    append_field(&mut record, cookie);
    record
}

fn append_field(record: &mut Vec<u8>, value: &[u8]) {
    record.extend_from_slice(&(u16::try_from(value.len()).unwrap_or(u16::MAX)).to_be_bytes());
    record.extend_from_slice(value);
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
