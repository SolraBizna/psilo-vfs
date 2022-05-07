use std::{
    env,
    fs,
    path,
};
use path::Path as StdPath;
use path::PathBuf as StdPathBuf;

use super::*;

fn cranky_does_exist(path: &StdPath) -> bool {
    match fs::read_dir(path) {
        Ok(_) => true,
        Err(x) if x.kind() == ErrorKind::NotFound => false,
        Err(x) => {
            log::error!("{:?}: {:?}", path, x);
            false
        },
    }
}

fn try_data_dir(vfs: &mut VFS, us_dir: &StdPath) {
    // First, try Data (capital D)
    let mut pb: StdPathBuf = us_dir.join("Data");
    if !cranky_does_exist(&pb) {
        // If that didn't work, try data (lowercase D)
        pb.pop();
        pb.push("data");
        if !cranky_does_exist(&pb) {
            // Neither exists, quietly give up
            log::info!("No data directory found under {:?}", us_dir);
            return
        }
    }
    let source = match crate::fs::Source::new(pb.clone(), true) {
        Ok(x) => x,
        Err(x) => {
            log::error!("{:?}: {:?}", pb, x);
            return
        },
    };
    log::info!("Data directory found: {:?}", pb);
    vfs.mount(Path::from_str_preverified("/").to_owned(),
              Box::new(source)).unwrap();
}

fn try_direct_config_dir(vfs: &mut VFS, us_dir: &StdPath) {
    let source = match crate::fs::Source::new(us_dir.to_owned(), false) {
        Ok(x) => x,
        Err(x) => {
            log::error!("{:?}: {:?}", us_dir, x);
            return
        },
    };
    log::info!("Config directory found: {:?}", us_dir);
    vfs.mount(Path::from_str_preverified("/config/").to_owned(),
              Box::new(source)).unwrap();
}

fn try_config_dir(vfs: &mut VFS, us_dir: &StdPath) {
    // First, try Config (capital C)
    let mut pb: StdPathBuf = us_dir.join("Config");
    if !cranky_does_exist(&pb) {
        // If that didn't work, try config (lowercase C)
        pb.pop();
        pb.push("config");
        if !cranky_does_exist(&pb) {
            // Neither exists, quietly give up
            log::info!("No config directory found under {:?}", us_dir);
            return
        }
    }
    let source = match crate::fs::Source::new(pb.clone(), false) {
        Ok(x) => x,
        Err(x) => {
            log::error!("{:?}: {:?}", pb, x);
            return
        },
    };
    log::info!("Config directory found: {:?}", pb);
    vfs.mount(Path::from_str_preverified("/config/").to_owned(),
              Box::new(source)).unwrap();
}

fn get_us_dir() -> StdPathBuf {
    match std::env::current_exe() {
        Ok(mut x) => {
            if x.pop() {
                x
            }
            else {
                ".".into()
            }
        },
        Err(x) => {
            log::warn!("Couldn't get the path to our own executable! {:?}",
                       x);
            log::warn!("Assuming it's in the working directory.");
            ".".into()
        },
    }
}

pub(crate) fn do_standard_mounts(vfs: &mut VFS, unixy_name: &str,
                                       _humanish_name: &str) {
    if cfg!(target_family="windows") {
        // First in the list, data next to the executable.
        let us_dir = get_us_dir();
        try_data_dir(vfs, &us_dir);
        try_config_dir(vfs, &us_dir);
        // TODO: USERPROFILE and stuff...
    }
    else if cfg!(target_family="wasm") {
        // ENTIRE LIST: Data in root.
        let us_dir: StdPathBuf = "/".into();
        try_data_dir(vfs, &us_dir);
        try_config_dir(vfs, &us_dir);
    }
    else if cfg!(target_family="unix") {
        // First in the list, executable-specific data.
        let mut us_dir = get_us_dir();
        if us_dir.file_name().map(|x| x == "bin").unwrap_or(false) {
            // We're in a `bin` directory.
            // This is a UNIX-style systemwide installation, or a simulacrum of
            // one.
            // For: .../bin/our_exe
            // Use: .../share/unixy_name/data
            //      .../share/unixy_name/config
            us_dir.pop();
            us_dir.push("share");
            us_dir.push(unixy_name);
            try_data_dir(vfs, &us_dir);
            try_config_dir(vfs, &us_dir);
        }
        else if us_dir.parent().and_then(StdPath::file_name)
            .map(|x| x == "target").unwrap_or(false) {
                // We're in a `target` directory.
                // This is a Cargo project, being executed in the place where
                // it was built.
                // For .../target/.../our_exe
                // Use: .../data
                //      .../config
                us_dir.pop();
                us_dir.pop();
                try_data_dir(vfs, &us_dir);
                try_config_dir(vfs, &us_dir);
            }
        else {
            // Assume that we're like other OSes, and we're just plopped in the
            // same directory as our data/config dirs.
            try_data_dir(vfs, &us_dir);
            try_config_dir(vfs, &us_dir);
        }
        // TODO: /etc... ugh
        // Now, to follow the XDG Base Directory Specification to the letter.
        // If the HOME variable isn't set, there isn't really a reasonable
        // default. However, XDG-compliant shell scripts would them act as
        // though it were empty, and end up putting everyting into `/`...
        let home: StdPathBuf = env::var_os("HOME")
            .filter(|x| !x.is_empty()).map(|x| StdPathBuf::from(x))
            .unwrap_or_else(|| "/".into());
        let home = &home;
        // Mount XDG_DATA_DIRS, in reverse.
        // Now, normally, if XDG_DATA_DIRS is not set or is empty, you're
        // supposed to look in `/usr/local/share` and `/usr/share`. However, we
        // already have better logic for handling that. So, in that case, just
        // don't even bother.
        match env::var("XDG_DATA_DIRS").ok().filter(|x| !x.is_empty()) {
            Some(list) => {
                let paths: Vec<StdPathBuf> = list.split(":")
                    .filter(|x| !x.is_empty()).map(|x| StdPathBuf::from(x)).collect();
                // Do them in reverse order, because later mounts take priority.
                for path in paths.into_iter().rev() {
                    try_data_dir(vfs, &path);
                }
            },
            None => (),
        }
        // Now mount XDG_DATA_HOME.
        let mut xdg_data_home: StdPathBuf = env::var_os("XDG_DATA_HOME")
            .filter(|x| !x.is_empty()).map(|x| StdPathBuf::from(x))
            .unwrap_or_else(|| {
                let mut ret = home.to_owned();
                ret.push(".local");
                ret.push("share");
                ret
            });
        xdg_data_home.push(unixy_name);
        try_data_dir(vfs, &xdg_data_home);
        // Alright, now do all that again but for config.
        match env::var("XDG_CONFIG_DIRS").ok().filter(|x| !x.is_empty()) {
            Some(list) => {
                let paths: Vec<StdPathBuf> = list.split(":")
                    .filter(|x| !x.is_empty()).map(|x| x.into()).collect();
                for path in paths.into_iter().rev() {
                    try_direct_config_dir(vfs, &path);
                }
            },
            None => (),
        }
        let mut xdg_config_home: StdPathBuf = env::var_os("XDG_CONFIG_HOME")
            .filter(|x| !x.is_empty()).map(|x| StdPathBuf::from(x))
            .unwrap_or_else(|| {
                let mut ret = home.to_owned();
                ret.push(".config");
                ret
            });
        xdg_config_home.push(unixy_name);
        // Before anything else, try recursively making this directory
        if let Err(x) = fs::create_dir_all(&xdg_config_home) {
            log::warn!("{:?}: {:?}", xdg_config_home, x);
        }
        try_direct_config_dir(vfs, &xdg_config_home);
    }
    else {
        panic!("Unknown platform, no idea how to do standard mounts!\n\
                Please figure out what standard mounts should be on your \
                platform, and then make a pull request.\n\
                (Or, if appropriate, just disable the `stdpaths` feature.)");
    }
}
