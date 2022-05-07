use crate::*;

use std::{
    cmp::Ordering,
    io, io::{Cursor, ErrorKind, Seek, Read},
    marker::Unpin,
    sync::{Arc, RwLock},
};

pub trait VFSSource {
    /// Opens a given file for reading.
    ///
    /// Takes: an absolute path to a file.
    fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>>;
    /// List files under a given directory.
    ///
    /// Takes: an absolute path to a directory.
    ///
    /// Returns: one or more single-component relative paths.
    fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    /// Atomically replace the contents of a given file.
    ///
    /// Takes: an absolute path to a file.
    fn update(&self, path: &Path, data: &[u8]) -> io::Result<()>;
}

struct VFSInner {
    mounts: Vec<(PathBuf, Box<dyn VFSSource>)>,
}

#[derive(Clone)]
pub struct VFS {
    inner: Arc<RwLock<VFSInner>>,
}

pub trait DataFile : Read + Seek {}
impl<T: AsRef<[u8]> + Unpin> DataFile for Cursor<T> {}

#[cfg(feature = "stdpaths")]
mod stdpaths;

impl VFS {
    pub fn new() -> VFS {
        VFS { inner: Arc::new(RwLock::new(VFSInner {
            mounts: vec![]
        }))}
    }
    #[cfg(feature = "stdpaths")]
    pub fn with_standard_paths(unixy_name: &str, humanish_name: &str)
        -> VFS {
        let mut ret = VFS::new();
        stdpaths::do_standard_mounts(&mut ret, unixy_name, humanish_name);
        ret
    }
    pub fn mount(&mut self, point:PathBuf, source:Box<dyn VFSSource>)
        -> io::Result<()> {
        if !point.is_absolute() {
            let err = format!("attempt to mount at a non-absolute path: {:?}",
                              point);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if !point.is_directory() {
            return Err(io::Error::from(ErrorKind::NotADirectory))
        }
        let mut this = self.inner.write().unwrap();
        this.mounts.push((point, source));
        Ok(())
    }
    pub fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>> {
        if !path.is_absolute() {
            let err = format!("attempt to open a non-absolute path: {:?}",
                              path);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if path.is_directory() {
            return Err(io::Error::from(ErrorKind::IsADirectory))
        }
        let this = self.inner.read().unwrap();
        for (prefix, source) in this.mounts.iter().rev() {
            match path.with_prefix_absolute(prefix) {
                None => (),
                Some(suffix) => {
                    match source.open(suffix) {
                        Ok(x) => return Ok(x),
                        Err(x) if x.kind() == ErrorKind::NotFound => continue,
                        Err(x) => return Err(x)
                    }
                },
            }
        }
        Err(io::Error::from(ErrorKind::NotFound))
    }
    pub fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        if !path.is_absolute() {
            let err = format!("attempt to list a non-absolute path: {:?}",
                              path);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if !path.is_directory() {
            let err = format!("attempt to list a file: {:?}",
                              path);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        let this = self.inner.read().unwrap();
        let mut result = vec![];
        let mut any_succeeded = false;
        let mut failed_with_not_dir = false;
        // Iterate through each mount...
        for (prefix, source) in this.mounts.iter() {
            // If this mount's prefix is relevant to this path...
            match path.with_prefix_absolute(prefix) {
                None => (),
                Some(suffix) => {
                    // ...then take the output of ls according to this mount...
                    let mut res = match source.ls(suffix) {
                        Ok(x) => x,
                        Err(x) if x.kind() == ErrorKind::NotFound => continue,
                        Err(x) if x.kind() == ErrorKind::NotADirectory => {
                            failed_with_not_dir = true;
                            continue;
                        },
                        Err(x) => return Err(x)
                    };
                    // ...and merge it into result.
                    result.append(&mut res);
                    any_succeeded = true;
                }
            }
            // Otherwise, if this path is above this mount's prefix...
            match prefix.with_prefix_absolute(path) {
                None => (),
                Some(suffix) => {
                    match suffix.components().next() {
                        None => (),
                        Some(x) => {
                            // ...make sure that the mounted-on directory
                            // appears in listings, even if there is no source
                            // that explicitly contains it.
                            let mut buf = x.to_owned();
                            buf.make_file_into_dir();
                            result.push(buf);
                            any_succeeded = true;
                        },
                    }
                },
            }
        }
        if !any_succeeded {
            debug_assert!(result.len() == 0);
            if failed_with_not_dir {
                return Err(io::Error::from(ErrorKind::NotADirectory))
            }
            else {
                return Err(io::Error::from(ErrorKind::NotFound))
            }
        }
        // Sort and deduplicate. (In cases where "foo" and "foo/" both exist,
        // remove "foo".)
        result.sort_by(|a, b| {
            if a.is_directory() && b.as_str() == &a.as_str()[..a.len()-1] {
                Ordering::Less
            }
            else if b.is_directory() && a.as_str() == &b.as_str()[..b.len()-1]{
                Ordering::Greater
            }
            else {
                a.cmp(b)
            }
        });
        result.dedup_by(|next, first| {
            if first == next { return true }
            if first.is_directory() && !next.is_directory() {
                if &first.as_str()[..first.len()-1] == next.as_str() {
                    return true
                }
            }
            return false
        });
        Ok(result)
    }
    /// Attempts to atomically update the file with the given path.
    ///
    /// NOTE: Only the *latest mount that contains the given path* will attempt
    /// to update the file. If that source fails to update the file, the update
    /// will fail!
    pub fn update(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        if !path.is_absolute() {
            let err = format!("attempt to open a non-absolute path: {:?}",
                              path);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if path.is_directory() {
            return Err(io::Error::from(ErrorKind::IsADirectory))
        }
        let this = self.inner.read().unwrap();
        for (prefix, source) in this.mounts.iter().rev() {
            match path.with_prefix_absolute(prefix) {
                None => (),
                Some(suffix) => match source.update(suffix, data) {
                    Err(x) if x.kind() == ErrorKind::ReadOnlyFilesystem
                        => continue,
                    x => return x,
                },
            }
        }
        Err(io::Error::from(ErrorKind::ReadOnlyFilesystem))
    }
}
