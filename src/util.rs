//! Small replacements for Go standard-library helpers the port relies on.

use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

/// Go url.PathEscape: escapes everything except unreserved characters and
/// `$&+:=@`, so `/`, `;`, `,` and `?` are escaped inside one path segment.
pub fn path_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~$&+:=@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Go url.PathUnescape: decodes %XX and keeps `+`. The error text matches Go.
pub fn path_unescape(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit));
            let Some(hex) = hex else {
                let end = (index + 3).min(bytes.len());
                return Err(format!(
                    "invalid URL escape {:?}",
                    String::from_utf8_lossy(&bytes[index..end])
                ));
            };
            let text = std::str::from_utf8(hex).map_err(|err| err.to_string())?;
            out.push(u8::from_str_radix(text, 16).map_err(|err| err.to_string())?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Go filepath.Abs: joins the working directory and cleans `.` and `..`
/// lexically, without resolving symlinks. The working directory follows Go's
/// os.Getwd, including the `$PWD` rule (see `working_dir`).
pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Go os.Getwd: prefers `$PWD` when it names the current directory, so paths
/// keep the logical (symlinked) spelling the shell used.
fn working_dir() -> io::Result<PathBuf> {
    logical_dir(std::env::var_os("PWD"))
}

#[cfg(unix)]
fn logical_dir(pwd: Option<OsString>) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    if let Some(pwd) = pwd.map(PathBuf::from).filter(|pwd| pwd.is_absolute()) {
        if let (Ok(logical), Ok(physical)) = (std::fs::metadata(&pwd), std::fs::metadata(".")) {
            if logical.dev() == physical.dev() && logical.ino() == physical.ino() {
                return Ok(pwd);
            }
        }
    }
    std::env::current_dir()
}

#[cfg(not(unix))]
fn logical_dir(_pwd: Option<OsString>) -> io::Result<PathBuf> {
    std::env::current_dir()
}

/// Writes a temporary file beside `path`, syncs it, and renames it into place,
/// so readers never see a partial file. `mode` applies on Unix only.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32, prefix: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".tmp")
        .tempfile_in(directory)?;
    set_mode(file.path(), mode)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|err| err.error)?;
    set_mode(path, mode)
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_escape_matches_go_path_segment_rules() {
        assert_eq!(
            path_escape("a b/c;d,e?f$&+:=@~-_.%#"),
            "a%20b%2Fc%3Bd%2Ce%3Ff$&+:=@~-_.%25%23"
        );
        assert_eq!(path_escape("é"), "%C3%A9");
    }

    #[test]
    fn path_unescape_decodes_and_rejects_bad_escapes() {
        assert_eq!(path_unescape("spec%20v2.pdf").unwrap(), "spec v2.pdf");
        assert_eq!(path_unescape("a+b").unwrap(), "a+b");
        assert_eq!(
            path_unescape("bad%zz").unwrap_err(),
            "invalid URL escape \"%zz\""
        );
        assert_eq!(
            path_unescape("end%4").unwrap_err(),
            "invalid URL escape \"%4\""
        );
    }

    #[test]
    fn absolute_cleans_dot_segments() {
        let cwd = working_dir().unwrap();
        assert_eq!(absolute(Path::new("a/./b/../c")).unwrap(), cwd.join("a/c"));
        assert_eq!(
            absolute(Path::new("/x/y/../z/")).unwrap(),
            PathBuf::from("/x/z")
        );
    }

    #[cfg(unix)]
    #[test]
    fn logical_dir_keeps_pwd_symlink_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(std::env::current_dir().unwrap(), &link).unwrap();
        assert_eq!(logical_dir(Some(link.clone().into())).unwrap(), link);
    }

    #[test]
    fn logical_dir_falls_back_to_physical_cwd() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(logical_dir(None).unwrap(), cwd);
        assert_eq!(
            logical_dir(Some("/nonexistent-dir-xyz".into())).unwrap(),
            cwd
        );
        assert_eq!(logical_dir(Some("relative".into())).unwrap(), cwd);
    }

    #[test]
    fn write_atomic_replaces_file_with_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");
        write_atomic(&path, b"one", 0o644, ".test-").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
        write_atomic(&path, b"two", 0o600, ".test-").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "temporary file left behind"
        );
    }
}
