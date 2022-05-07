use crate::*;

use std::{
    fs::{File, OpenOptions, rename, read_dir, remove_file},
    io::{self, Write},
    path,
};
use log::debug;

pub struct Source {
    base: path::PathBuf,
    read_only: bool,
}

impl DataFile for File {}

impl Source {
    pub fn new(base: path::PathBuf, read_only: bool) -> io::Result<Source> {
        debug!("Mounting {:?} read-{}", base,
               if read_only { "only" } else { "write" });
        Ok(Source { base, read_only })
    }
}

impl VFSSource for Source {
    // TODO: Do we need to make a mapping from normalized paths to physical
    // paths? Apple filesystems handle this correctly for us, maybe Microsoft
    // ones do too. Not sure about Linux ones?
    fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>> {
        debug_assert!(path.is_absolute() && !path.is_directory());
        let os_path = self.base.join(&path.as_str()[1..]);
        match File::open(&os_path) {
            Err(x) if x.kind() == io::ErrorKind::NotFound => {
                let mut backup_path = os_path;
                backup_path.set_file_name(backup_path.file_name().unwrap()
                                          .to_str().unwrap()
                                          .to_string() + "~");
                File::open(&backup_path)
            },
            x => x,
        }.map(|x| -> Box<dyn DataFile> { Box::new(x) })
    }
    fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        debug_assert!(path.is_absolute() && path.is_directory());
        let mut paths = Vec::<PathBuf>::new();
        let os_path = self.base.join(&path.as_str()[1..]);
        let mut dir = read_dir(os_path)?;
        while let Some(entry) = dir.next() {
            let entry = entry?;
            let path = entry.path();
            let mut filename = match path.file_name().and_then(|x| x.to_str())
                .map(|x| x.to_string()) {
                    Some(x) => x,
                    _ => continue,
                };
            if filename.ends_with("^") || filename.ends_with("!")
                || filename.ends_with("~~") { continue }
            else if filename.ends_with("~") {
                filename.pop(); // :)
            }
            if entry.path().is_dir() { filename.push('/'); }
            match PathBuf::try_from_str(&filename) {
                Ok(path) => paths.push(path),
                _ => continue,
            }
        }
        Ok(paths)
    }
    fn update(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        debug_assert!(path.is_absolute() && !path.is_directory());
        if self.read_only { return Err(io::Error::from(io::ErrorKind
                                                       ::ReadOnlyFilesystem)) }
        let os_path = self.base.join(&path.as_str()[1..]);
        let mut backup_path = os_path.clone();
        backup_path.set_file_name(os_path.file_name().unwrap()
                                  .to_str().unwrap().to_string() + "~");
        let mut updated_path = os_path.clone();
        updated_path.set_file_name(os_path.file_name().unwrap()
                                   .to_str().unwrap().to_string() + "^");
        // Try to write the new data to "FILENAME^"
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&updated_path)?;
        file.write_all(data)?;
        drop(file);
        // Delete "FILENAME~", ignoring errors
        let _ = remove_file(&backup_path);
        // Move "FILENAME" to "FILENAME~"
        rename(&os_path, &backup_path)?;
        // Move "FILENAME^" to "FILENAME"
        rename(&updated_path, &os_path)
    }
}
