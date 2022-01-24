#![feature(io_error_more)] // for ErrorKind::IsADirectory and friends

//! This is a virtual filesystem layer for games. It's part of the Psilo
//! engine, but it doesn't depend on any other part of the engine. It provides
//! asynchronous IO (via Tokio) on a UNIX style virtual filesystem hierarchy.
//! The backing stores for this filesystem could come from a real filesystem,
//! files embedded in the executable, files inside an archive, or any other
//! source which can implement the relevant traits.
//!
//! Psilo-VFS tries really hard to be portable and platform-agnostic, but it
//! does require `std`.
//!
//! # Overview
//!
//! Psilo-VFS provides a UNIX-ish filesystem, anchored at `/`. "Sources" are
//! mounted at various points on the hierarchy.
//!
//! TODO: examples
//!
//! ## Paths
//!
//! Psilo-VFS has a relatively restrictive definition of a path. When working
//! with paths in Psilo-VFS, use the [`Path`](struct.Path.html) and
//! [`PathBuf`](struct.PathBuf.html) structs from `Psilo-VFS` instead of the
//! OS-dependent `Path` and `PathBuf` structs in the standard library.
//!
//! See the [`Path`](struct.Path.html) documentation for more information on
//! the path restrictions. The short version is that, with a few obscure
//! exceptions, any filename that is valid on Windows will be allowed by
//! Psilo-VFS.
//!
//! ## Mounts
//!
//! All mounts are "union mounts". This means that if you have more than one
//! source mounted at the same location, any file or directory present in
//! either source will appear in the final hierarchy. This is *unlike* typical
//! UNIX mounts, in which later mounts entirely replace parts of the hierarchy.
//!
//! For example, assume you have the following in tree A:
//!
//! - `/bar/`
//!     - `/bar/baz`
//! - `/foo`
//!
//! And the following in tree B:
//!
//! - `/bar/`
//!     - `/bar/bang`
//! - `/foo`
//!
//! If you mount both A and B (in that order), you will see this tree:
//!
//! - `/bar/`
//!     - `/bar/bang` (sourced from B)
//!     - `/bar/baz` (sourced from A)
//! - `/foo` (sourced from B)
//!
//! Each mount can be arbitrarily anchored within the overall logical tree. The
//! mountpoints don't need to "exist" in any sense beforehand; their existence
//! is implied by the mount. If you mounted A at `/` and B at
//! `/plugins/fnord"`, you would get:
//!
//! - `/bar/`
//!     - `/bar/baz` (sourced from A)
//! - `/foo` (sourced from A)
//! - `/plugins/`
//!     - `/plugins/fnord/`
//!         - `/plugins/fnord/bar/`
//!             - `/plugins/fnord/bar/bang` (sourced from B)
//!         - `/plugins/fnord/foo` (sourced from B)
//!
//! A directory in any mount will shadow any files that might be in another
//! mount with the same name. Given tree C:
//!
//! - `/foo/`
//!     - `/foo/barf`
//!
//! If you mount A, B, and C anchored at `/`, you get:
//!
//! - `/bar/`
//!     - `/bar/bang` (sourced from B)
//!     - `/bar/baz` (sourced from A)
//! - `/foo/`
//!     - `/foo/barf` (sourced from C)
//!
//! Later mounts override earlier mounts, as seen above.

/// Specifies a constant, literal path. Give it a string literal and it will
/// validate it and give you a [`&'static Path`](struct.Path.html), with no
/// runtime overhead.
///
/// ```rust
/// # use psilo_vfs::{Path, p};
/// const STARTUP_SCREEN_PATH: &Path = p!("/splash/StartupScreen.png");
/// ```
///
/// Anywhere you're hardcoding a path (or part of a path) in your code, you
/// should use `p!()` to denote it. If you've made some mistake in your path
/// that makes it invalid, this catches it at compile time instead of runtime.
/// And, if Unicode normalization or other canonicalization is needed, this
/// peforms it ahead of time. Nice!
///
/// The sole argument must be a single string literal. It can be a raw string
/// if you like, but it can't be a byte string, and it *definitely* can't be
/// some other expression. If you want to build a path at runtime, just use
/// [`Path`](struct.Path.html) or [`PathBuf`](struct.PathBuf.html) methods as
/// appropriate.
pub use psilo_vfs_pathmacro::p;

mod path;
pub use path::{Path, PathBuf};

mod vfs;
pub use vfs::*;

#[cfg(feature = "fs")]
mod fs;
#[cfg(feature = "fs")]
pub use fs::Source as FsSource;

#[cfg(feature = "rom")]
mod rom;
#[cfg(feature = "rom")]
pub use rom::Source as RomSource;
