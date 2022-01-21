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
//! Psilo-VFS presents two filesystem-like systems:
//! - **Data**: Data is read-only, intended for use with game data and mods.
//! - **Config**: Config is read-write, and is slightly more restrictive on
//!   filenames. It's intended for configuration, save data, screenshots, etc.
//!
//! Both systems are completely independent. Most games will have exactly one
//! of each, but you can have as many or as few as you want. You could even
//! intermix Psilo-VFS IO with regular IO or Tokio where appropriate.
//!
//! ## Paths
//!
//! Psilo-VFS has a relatively restrictive definition of a path. When working
//! with paths in Psilo-VFS, use the [`Path`](struct.Path.html) and
//! [`PathBuf`](struct.PathBuf.html) structs from `Psilo-VFS` instead of the
//! OS-dependent `Path` and `PathBuf` structs in the standard library.
//!
//! - A path is zero or more components separated by `/`.
//! - An absolute path begins with `/`. This is the root of the hierarchy. If
//!   a path does not begin with `/`, it is a relative path, and cannot be used
//!   directly.
//! - A path that denotes a directory MUST end with `/`. A path that denotes a
//!   file MUST NOT end with `/`.
//!
//! Most of the time, those are the only restrictions you need to care about.
//! Psilo-VFS cares a lot whether your paths properly begin and end with `/` in
//! keeping with how they're used. Other than that, *almost* any filename that
//! is legal on Microsoft Windows is permitted by Psilo-VFS, so you won't need
//! to read the restrictions below. Nevertheless, here they are, in case you
//! care.
//!
//! - A path component MUST contain at least one character.
//! - A path component MUST NOT begin with `.` (U+0046)
//! - A path component MUST NOT end with a space (U+0020) or `.` (U+0046)
//! - A path component MUST NOT contain any of the following characters:
//!     - U+0000 NULL
//!     - Any C0 control character (U+0001 through U+001F) or C1 control
//!       character (U+0080 through U+009F)
//!     - `"` (U+0022 QUOTATION MARK)
//!     - `*` (U+002A ASTERISK)
//!     - `/` (U+002F SOLIDUS; this follows as `/` is used as a path separator)
//!     - `:` (U+003A COLON)
//!     - `?` (U+003F QUESTION MARK)
//!     - `\` (U+005C REVERSE SOLIDUS)
//!     - `<` (U+0060 LESS-THAN SIGN)
//!     - `>` (U+0062 GREATER-THAN SIGN)
//!     - `|` (U+007C VERTICAL LINE)
//! - A path component MUST NOT be any of the following, or start with any of
//!   the following followed by a `.`. They are given in uppercase, but
//!   mixed- and lowercase versions are forbidden as well.
//!     - "AUX"
//!     - "COM1" through "COM9"
//!     - "CON"
//!     - "LPT1" through "LPT9"
//!     - "NUL"
//!     - "PRN"
//! - A path component additionally MUST NOT *end* with any of the following:
//!     - `!` (U+0021 EXCLAMATION MARK; reserved for future use)
//!     - `^` (U+005E CIRCUMFLEX ACCENT; reserved for intermediate files)
//!     - `~` (U+007E TILDE; reserved for backup files)
//!
//! In most filing systems, a path component "." denotes the current directory
//! and a path component ".." denotes the parent directory. This is also the
//! case in Psilo-VFS. In some filing systems, ".." in the root directory is
//! equivalent to ".". This is *not* the case in Psilo-VFS! ".." that spill
//! above the root directory in an absolute path are an error. Psilo-VFS
//! accepts these special components anywhere, except where it would attempt to
//! reach the parent of the root. Psilo-VFS always removes "." components and
//! minimizes ".." components, which means that ".." will only appear in the
//! beginning of a relative path and all other instances of ".." will be
//! resolved (or rejected).
//!
//! Psilo-VFS imposes no filename length restrictions, but as a rule you should
//! restrict your paths and filenames to 255 UTF-8 bytes or less. You may run
//! into problems well below even this limit with certain poorly-written (or
//! very old) operating systems and programs.
//!
//! Arbitrary non-ASCII Unicode is allowed by Psilo-VFS, but you should
//! exercise reason and restraint when using this. Just because you *can* have
//! bidi control characters and Fraktur in your data file names doesn't mean
//! you *should*. Also, just a reminder, no matter what your Redmond-originated
//! OS might think, unpaired surrogates *are not legal in Unicode text*, and
//! Psilo-VFS will not accept filenames that contain them.
//!
//! Psilo-VFS converts all path components to Unicode normal form D before use.
//! This means that "resumé" and "resumé" are always the same file, even if one
//! was represented with six code points and one was represented with seven.
//! After normalization, path components are compared and processed codepoint
//! by codepoint, with no accounting for case. This means that "Resumé" and
//! "resumé" *are* different files. Try to avoid being in a situation where
//! either of these things matter, since some underlying filesystems and
//! archive formats will be unable to handle this if pushed. In particular,
//! don't poke the sleeping dragon by using Config filenames that differ only
//! in case.
//!
//! ## Mounts
//!
//! Both Data and Config are "union mounts". This means that they can have
//! multiple disparate backing trees, and they present the game with one tree
//! that contains everything that's in any of them.
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

mod data;
pub use data::*;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(feature = "rom")]
pub mod rom;

