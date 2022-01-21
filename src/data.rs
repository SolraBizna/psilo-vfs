use crate::*;

use std::{
    cmp::Ordering,
    io, io::{Cursor, ErrorKind},
    marker::Unpin,
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncSeek},
    sync::RwLock,
};
use async_trait::async_trait;

#[async_trait]
pub trait DataVFSSource {
    /// Opens a given file for reading.
    ///
    /// Takes: an absolute path to a file.
    async fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>>;
    /// List files under a given directory.
    ///
    /// Takes: an absolute path to a directory.
    ///
    /// Returns: one or more single-component relative paths.
    async fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}

struct DataVFSInner {
    mounts: Vec<(PathBuf, Box<dyn DataVFSSource>)>,
}

#[derive(Clone)]
pub struct DataVFS {
    inner: Arc<RwLock<DataVFSInner>>,
}

pub trait DataFile : AsyncRead + AsyncSeek + Unpin {}
impl<T: AsRef<[u8]> + Unpin> DataFile for Cursor<T> {}

impl DataVFS {
    pub fn new() -> DataVFS {
        DataVFS { inner: Arc::new(RwLock::new(DataVFSInner {
            mounts: vec![]
        }))}
    }
    pub async fn mount(&mut self, point:PathBuf, source:Box<dyn DataVFSSource>)
        -> io::Result<()> {
        if !point.is_absolute() {
            let err = format!("attempt to mount at a non-absolute path: {:?}",
                              point);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if !point.is_directory() {
            return Err(io::Error::from(ErrorKind::NotADirectory))
        }
        let mut this = self.inner.write().await;
        this.mounts.push((point, source));
        Ok(())
    }
    pub async fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>> {
        if !path.is_absolute() {
            let err = format!("attempt to open a non-absolute path: {:?}",
                              path);
            return Err(io::Error::new(ErrorKind::Other, err))
        }
        if path.is_directory() {
            return Err(io::Error::from(ErrorKind::IsADirectory))
        }
        let this = self.inner.read().await;
        for (prefix, source) in this.mounts.iter().rev() {
            match path.with_prefix_absolute(prefix) {
                None => (),
                Some(suffix) => {
                    match source.open(suffix).await {
                        Ok(x) => return Ok(x),
                        Err(x) if x.kind() == ErrorKind::NotFound => continue,
                        Err(x) => return Err(x)
                    }
                },
            }
        }
        Err(io::Error::from(ErrorKind::NotFound))
    }
    pub async fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
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
        let this = self.inner.read().await;
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
                    let mut res = match source.ls(suffix).await {
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
}
