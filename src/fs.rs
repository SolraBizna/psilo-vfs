use crate::*;

use std::{
    io,
    path,
};
use tokio::{
    fs::{File, read_dir},
};
use async_trait::async_trait;

pub struct DataSource {
    base: path::PathBuf,
}

impl DataFile for File {}

#[async_trait]
impl DataVFSSource for DataSource {
    // TODO: Do we need to make a mapping from normalized paths to physical
    // paths? Apple filesystems handle this correctly for us, maybe Microsoft
    // ones do too. Not sure about Linux ones?
    async fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>> {
        debug_assert!(path.is_absolute() && !path.is_directory());
        let os_path = self.base.join(&path.as_str()[1..]);
        File::open(os_path).await.map(|x| -> Box<dyn DataFile> { Box::new(x) })
    }
    async fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        debug_assert!(path.is_absolute() && path.is_directory());
        let mut paths = Vec::<PathBuf>::new();
        let os_path = self.base.join(&path.as_str()[1..]);
        let mut dir = read_dir(os_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            let mut filename = match path.file_name().and_then(|x| x.to_str())
                .map(|x| x.to_string()) {
                    Some(x) => x,
                    _ => continue,
            };
            if entry.path().is_dir() { filename.push('/'); }
            match PathBuf::try_from_str(&filename) {
                Ok(path) => paths.push(path),
                _ => continue,
            }
        }
        Ok(paths)
    }
}
