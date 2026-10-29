//! Descriptor-relative, no-follow access for optional local artifacts.
use rustix::fd::OwnedFd;
use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};
use std::path::{Component, Path};

/// Open an absolute resolved artifact from a held root descriptor. This also
/// checks ancestors of the canonical data root without following replacements.
/// Callers must supply only the startup-resolved in-root path; this API does not enforce a data root.
pub fn open_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    let relative = path.strip_prefix("/").map_err(|_| invalid())?;
    let root = open_root(Path::new("/"))?;
    open_regular(&root, relative)
}

/// Open an absolute root without following any component. Keeping this descriptor
/// pins the directory even if a publisher later renames its path.
pub fn open_root(path: &Path) -> std::io::Result<OwnedFd> {
    if !path.is_absolute() || path.components().count() > 128 {
        return Err(invalid());
    }
    let mut directory = open("/", directory_flags(), Mode::empty())?;
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, directory_flags(), Mode::empty())?
            }
            _ => return Err(invalid()),
        }
    }
    Ok(directory)
}

/// Only normal relative components are accepted. All ancestors are opened as
/// directories without following links; FIFO/device targets never enter a read.
pub fn open_regular(root: &OwnedFd, relative: &Path) -> std::io::Result<std::fs::File> {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() || components.len() > 128 {
        return Err(invalid());
    }
    let mut directory = rustix::io::dup(root)?;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(invalid());
        };
        if index + 1 == components.len() {
            let file = openat(
                &directory,
                *name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            if FileType::from_raw_mode(fstat(&file)?.st_mode) != FileType::RegularFile {
                return Err(invalid());
            }
            return Ok(file.into());
        }
        directory = openat(&directory, *name, directory_flags(), Mode::empty())?;
    }
    Err(invalid())
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}

fn invalid() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::InvalidInput)
}
