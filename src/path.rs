use std::{
    borrow::{Borrow, Cow},
    error::Error,
    fmt::{Debug, Display, Formatter},
    ops::Deref,
    str,
};
use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::{
    IsNormalized,
    char::decompose_canonical,
    is_nfd_quick,
};

#[derive(Debug,PartialEq,Eq)]
pub enum PathFromStrError {
    /// There were two or more consecutive slashes in the path.
    DoubleSlash,
    /// You used one of the characters that is forbidden at the beginning of a
    /// name.
    InvalidStartChar,
    /// You used one of the characters that is forbidden at the end of a name.
    InvalidEndChar,
    /// You used one of the forbidden characters in a name.
    InvalidChar,
    /// You used one of the Windows-reserved names.
    ReservedName,
    /// Your path tried to escape the root directory with ".."
    EscapedRoot,
    /// A path ended with ".." (as opposed to "../")
    DotDotFile,
}

impl Display for PathFromStrError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	match *self {
	    PathFromStrError::DoubleSlash
		=> write!(fmt, "double slash in path"),
	    PathFromStrError::InvalidStartChar
		=> write!(fmt, "invalid start char in some component of path"),
	    PathFromStrError::InvalidEndChar
		=> write!(fmt, "invalid end char in some component of path"),
	    PathFromStrError::InvalidChar
		=> write!(fmt, "invalid char in path"),
	    PathFromStrError::ReservedName
		=> write!(fmt, "reserved name in path"),
	    PathFromStrError::EscapedRoot
		=> write!(fmt, "path tried to denote root's parent (too many \
				\"..\")"),
	    PathFromStrError::DotDotFile
		=> write!(fmt, "path ended with \"..\" (instead of \"../\")"),
	}
    }
}

impl Error for PathFromStrError {}

#[derive(Debug,PartialEq,Eq)]
pub enum PathJoinError {
    /// You called `join` on a path that wasn't a directory.
    BasePathNotDir,
    /// You called `join` (not `join_or_replace`) and provided a path that was
    /// absolute.
    PathNotRelative,
    /// Your path tried to escape the root directory with ".."
    EscapedRoot,
}

impl Display for PathJoinError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	match *self {
	    PathJoinError::BasePathNotDir
		=> write!(fmt, "called join on a path that was not a dir"),
	    PathJoinError::PathNotRelative
		=> write!(fmt, "called join and provided a path that was not \
				relative"),
	    PathJoinError::EscapedRoot
		=> write!(fmt, "called join and provided a path that would \
				have escaped root (too many \"..\")"),
	}
    }
}

impl Error for PathJoinError {}

static INVALID_PATH_PREFIX_CHAR_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\."#)
	.unwrap()
});
static INVALID_PATH_SUFFIX_CHAR_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"[. ~^!]$"#)
	.unwrap()
});
static INVALID_PATH_CHAR_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"[\x00-\x1F\u{0080}-\u{009F}"*/:?\\<>|]"#)
	.unwrap()
});
static INVALID_PATH_NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^(?i:AUX|CO(?:M[1-9]|N)|LPT[1-9]|NUL|PRN)(?:\.|$)"#)
	.unwrap()
});

/// Analogous to the `Path` struct in the standard library, this is a
/// non-owned slice over a Psilo-VFS path.
///
/// # Restrictions
///
/// - A path is zero or more components separated by `/`.
/// - An absolute path begins with `/`. This is the root of the hierarchy. If
///   a path does not begin with `/`, it is a relative path, and cannot be used
///   directly.
/// - A path that denotes a directory MUST end with `/`. A path that denotes a
///   file MUST NOT end with `/`.
///
/// Most of the time, those are the only restrictions you need to care about.
/// Psilo-VFS cares a lot whether your paths properly begin and end with `/` in
/// keeping with how they're used. Other than that, *almost* any filename that
/// is legal on Microsoft Windows is permitted by Psilo-VFS, so you won't need
/// to read the restrictions below. Nevertheless, here they are, in case you
/// care.
///
/// - A path component MUST contain at least one character.
/// - A path component MUST NOT begin with `.` (U+0046)
/// - A path component MUST NOT end with a space (U+0020) or `.` (U+0046)
/// - A path component MUST NOT contain any of the following characters:
///     - U+0000 NULL
///     - Any C0 control character (U+0001 through U+001F) or C1 control
///       character (U+0080 through U+009F)
///     - `"` (U+0022 QUOTATION MARK)
///     - `*` (U+002A ASTERISK)
///     - `/` (U+002F SOLIDUS; this follows as `/` is used as a path separator)
///     - `:` (U+003A COLON)
///     - `?` (U+003F QUESTION MARK)
///     - `\` (U+005C REVERSE SOLIDUS)
///     - `<` (U+0060 LESS-THAN SIGN)
///     - `>` (U+0062 GREATER-THAN SIGN)
///     - `|` (U+007C VERTICAL LINE)
/// - A path component MUST NOT be any of the following, or start with any of
///   the following followed by a `.`. They are given in uppercase, but
///   mixed- and lowercase versions are forbidden as well.
///     - "AUX"
///     - "COM1" through "COM9"
///     - "CON"
///     - "LPT1" through "LPT9"
///     - "NUL"
///     - "PRN"
/// - A path component additionally MUST NOT *end* with any of the following:
///     - `!` (U+0021 EXCLAMATION MARK; reserved for future use)
///     - `^` (U+005E CIRCUMFLEX ACCENT; reserved for intermediate files)
///     - `~` (U+007E TILDE; reserved for backup files)
///
/// In most filing systems, a path component "." denotes the current directory
/// and a path component ".." denotes the parent directory. This is also the
/// case in Psilo-VFS. In some filing systems, ".." in the root directory is
/// equivalent to ".". This is *not* the case in Psilo-VFS! ".." that spill
/// above the root directory in an absolute path are an error. Psilo-VFS
/// accepts these special components anywhere, except where it would attempt to
/// reach the parent of the root. Psilo-VFS always removes "." components and
/// minimizes ".." components, which means that ".." will only appear in the
/// beginning of a relative path and all other instances of ".." will be
/// resolved (or rejected).
///
/// Psilo-VFS imposes no filename length restrictions, but as a rule you should
/// restrict your paths and filenames to 255 UTF-8 bytes or less. You may run
/// into problems well below even this limit with certain poorly-written (or
/// very old) operating systems and programs.
///
/// Arbitrary non-ASCII Unicode is allowed by Psilo-VFS, but you should
/// exercise reason and restraint when using this. Just because you *can* have
/// bidi control characters and Fraktur in your data file names doesn't mean
/// you *should*. Also, just a reminder, no matter what your Redmond-originated
/// OS might think, unpaired surrogates *are not legal in Unicode text*, and
/// Psilo-VFS will not accept filenames that contain them.
///
/// Psilo-VFS converts all path components to Unicode normal form D before use.
/// This means that "resumé" and "resumé" are always the same file, even if one
/// was represented with six code points and one was represented with seven.
/// After normalization, path components are compared and processed codepoint
/// by codepoint, with no accounting for case. This means that "Resumé" and
/// "resumé" *are* different files. Try to avoid being in a situation where
/// either of these things matter, since some underlying filesystems and
/// archive formats will be unable to handle this if pushed. In particular,
/// don't poke the sleeping dragon by using filenames that differ only
/// in case.
#[repr(transparent)]
#[derive(PartialEq,Eq,PartialOrd,Ord)]
pub struct Path {
    inner: str
}

impl Path {
    /// Internal use only. Takes a `&str` and transmutes it into a `&Path`,
    /// without rechecking.
    ///
    /// Used by the `path`
    #[doc(hidden)]
    pub const fn from_str_preverified(s: &str) -> &Path {
	// This is `unsafe`, but sound. It's sound because `Path` is a
	// transparent wrapper around `str`.
	unsafe { std::mem::transmute(s) }
    }
    /// Creates a new `Path` or `PathBuf` from a `&str`. If the passed string
    /// is already in normal form D, no copying will take place. Panics if the
    /// passed path is invalid in any way. Convenient, but fragile.
    pub fn from_str(s: &str) -> Cow<'_, Path> {
	Path::try_from_str(s).expect("Invalid path")
    }
    /// Creates a new `Path` or `PathBuf` from a `&str`. If the passed string
    /// is already in normal form D, no copying will take place. Returns an
    /// error if the passed path is invalid in any way.
    pub fn try_from_str(s: &str) -> Result<Cow<'_, Path>, PathFromStrError> {
	if s == "" || s == "/" {
	    return Ok(Cow::Borrowed(Path::from_str_preverified(s)))
	}
	else if s == "//" {
	    return Err(PathFromStrError::DoubleSlash)
	}
	else if s.ends_with("/..") || s == ".." {
	    return Err(PathFromStrError::DotDotFile)
	}
	// First, check that all components are valid.
	let subset = s.strip_prefix("/").unwrap_or(s);
	let subset = subset.strip_suffix("/").unwrap_or(subset);
	let mut need_edit = false;
	let mut any_non_dotdot_components = false;
	for component in subset.split('/') {
	    if component == "." {
		need_edit = true;
	    }
	    else if component == ".." {
		if any_non_dotdot_components {
		    need_edit = true;
		}
	    }
	    else if INVALID_PATH_PREFIX_CHAR_PATTERN.is_match(component) {
		return Err(PathFromStrError::InvalidStartChar)
	    }
	    else if INVALID_PATH_SUFFIX_CHAR_PATTERN.is_match(component) {
		return Err(PathFromStrError::InvalidEndChar)
	    }
	    else if INVALID_PATH_CHAR_PATTERN.is_match(component) {
		return Err(PathFromStrError::InvalidChar)
	    }
	    else if INVALID_PATH_NAME_PATTERN.is_match(component) {
		return Err(PathFromStrError::ReservedName)
	    }
	    else {
		any_non_dotdot_components = true;
	    }
	}
	need_edit = need_edit
	    || is_nfd_quick(s.chars()) != IsNormalized::Yes;
	if !need_edit {
	    return Ok(Cow::Borrowed(Path::from_str_preverified(s)))
	}
	else {
	    // this string might grow slightly beyond this, hope that's OK
	    let mut ret = String::with_capacity(s.len()+1);
	    if s.starts_with("/") { ret.push('/') }
	    for component in subset.split('/') {
		if component == "." { continue }
		if component == ".." {
		    if ret == "" || ret.ends_with("../") {
			ret.push_str("../");
		    }
		    else if ret == "/" {
			return Err(PathFromStrError::EscapedRoot)
		    }
		    else {
			if ret.len() > 0 {
			    assert!(ret.ends_with("/"));
			    ret.pop();
			}
			while ret.len() > 0 && !ret.ends_with("/") {
			    ret.pop();
			}
			if ret.len() > 0 {
			    debug_assert!(ret.ends_with("/"))
			}
		    }
		}
		else {
		    for c in component.chars() {
			decompose_canonical(c, |c| ret.push(c));
		    }
		    ret.push('/');
		}
	    }
	    // remove the extra trailing `/` that appeared
	    if !s.ends_with("/") { ret.pop(); }
	    Ok(Cow::Owned(PathBuf { inner: ret }))
	}
    }
    /// Returns the path as a `&str`.
    pub fn as_str(&self) -> &str { &self.inner }
    /// Returns true if the path is absolute (begins with `/`), false if it's
    /// relative.
    pub fn is_absolute(&self) -> bool {
	self.inner.chars().next().map(|x| x == '/').unwrap_or(false)
    }
    /// Returns true if the path is relative (does not begin with `/`), false
    /// if it's absolute.
    pub fn is_relative(&self) -> bool {
	!self.is_absolute()
    }
    /// Returns true if the path refers to a directory (ends with `/` or is
    /// empty), false otherwise.
    pub fn is_directory(&self) -> bool {
	self.inner.chars().rev().next().map(|x| x == '/').unwrap_or(false)
	    || &self.inner == ""
    }
    /// Returns the components of this path.
    ///
    /// Note that, in accordance with our definition of a path, there is no
    /// empty component at the beginning of an absolute path, nor an empty
    /// component at the end of a path designating a directory. And an empty
    /// path has no components.
    pub fn components(&self) -> PathComponents<'_> {
	let slice = self.inner.strip_prefix('/').unwrap_or(&self.inner);
	let slice = slice.strip_suffix('/').unwrap_or(slice);
	if slice == "" {
	    let mut iter = slice.split('/');
	    iter.next();
	    PathComponents::new(iter)
	}
	else { PathComponents::new(slice.split('/')) }
    }
    /// Returns `Some(...)` giving the path to the parent directory of this
    /// path if there is one, `None` if the path is "" or "/".
    pub fn parent(&self) -> &Path {
	Path::from_str_preverified(self.inner.trim_end_matches('/')
				 .trim_end_matches(|x| x != '/'))
    }
    /// Returns `Some(...)` if the last component of this `Path` has a "dot
    /// extension", `None` if it does not. If multiple extensions are present,
    /// only the *last* is returned.
    pub fn extension(&self) -> Option<&str> {
	if let Some(final_component) = self.components().rev().next() {
	    final_component.inner.split('.').last()
	} else { None }
    }
    /// If the given path is a prefix of this path, returns an absolute path
    /// containing the parts of this path minus the prefix. For example:
    ///
    /// ```
    /// # use psilo_vfs::p;
    /// assert_eq!(p!("/foo/bar").with_prefix_absolute(p!("/foo/")),
    ///            Some(p!("/bar")));
    /// ```
    ///
    /// This does work with relative paths, but if `other` is not a path to a
    /// directory, this will never work!
    pub fn with_prefix_absolute(&self, other: &Path) -> Option<&Path> {
        if !other.is_directory() { return None }
        match self.inner.strip_prefix(&other.inner[..other.inner.len()-1]) {
            None => None,
            Some(x) if !x.starts_with('/') => None,
            Some(x) => Some(Path::from_str_preverified(x))
        }
    }
}

impl Display for Path {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	// fortunately, our path definition forbids backslashes or double-
	// quotes or weird control characters in paths, so we can just print
	// the path out and have no ambiguity.
	Display::fmt(&self.inner, fmt)
    }
}

impl Debug for Path {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	write!(fmt, "\"{}\"", &self.inner)
    }
}

impl<'a> From<&'a Path> for &'a str {
    fn from(x: &Path) -> &str { &x.inner }
}

impl AsRef<str> for Path {
    fn as_ref(&self) -> &str { &self.inner }
}

impl ToOwned for Path {
    type Owned = PathBuf;
    fn to_owned(&self) -> PathBuf {
	PathBuf { inner: self.to_string() }
    }
}

impl Deref for Path {
    type Target = str;
    fn deref(&self) -> &str {
        &self.inner
    }
}

impl PartialEq<str> for Path {
    fn eq(&self, other: &str) -> bool {
        &self.inner == other
    }
}

/// An iterator over the components of a `Path`.
pub struct PathComponents<'a> {
    inner: str::Split<'a, char>,
}

impl<'a> PathComponents<'a> {
    fn new(inner: str::Split<'a, char>) -> PathComponents<'a> {
        PathComponents { inner }
    }
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = &'a Path;
    fn next(&mut self) -> Option<&'a Path> {
        match self.inner.next() {
            None => None,
            Some(x) => Some(Path::from_str_preverified(x)),
        }
    }
}

impl<'a> DoubleEndedIterator for PathComponents<'a> {
    fn next_back(&mut self) -> Option<&'a Path> {
        match self.inner.next_back() {
            None => None,
            Some(x) => Some(Path::from_str_preverified(x)),
        }
    }
}

/// Analogous to the `PathBuf` struct in the standard library, this is an
/// owned Psilo-VFS path on the heap.
///
/// See [`Path`](struct.Path.html) for more information on how Psilo-VFS paths
/// work, and what restrictions they have.
#[repr(transparent)]
#[derive(PartialEq,Eq,PartialOrd,Ord,Clone)]
pub struct PathBuf {
    inner: String
}

impl PathBuf {
    /// Creates a new, empty `PathBuf`.
    pub fn new() -> PathBuf {
	PathBuf { inner: String::new() }
    }
    /// Creates a new `PathBuf` with a given initial capacity in its underlying
    /// `String`.
    pub fn with_capacity(capacity: usize) -> PathBuf {
	PathBuf { inner: String::with_capacity(capacity) }
    }
    /// Creates a new `PathBuf` from a `&str`. Panics if the passed path is
    /// invalid in any way. Convenient, but fragile.
    pub fn from_str(s: &str) -> PathBuf {
	PathBuf::try_from_str(s).expect("Invalid path")
    }
    /// Creates a new `PathBuf` from a `&str`. Returns an error if the passed
    /// path is invalid in any way.
    pub fn try_from_str(s: &str) -> Result<PathBuf, PathFromStrError> {
	Path::try_from_str(s).map(Cow::into_owned)
    }
    /// Borrows this `PathBuf`'s contents as a `&Path`.
    pub fn as_path(&self) -> &Path {
	self.as_ref()
    }
    /// Attempts to extend `self` by applying a relative path to it. The path
    /// *must* be relative. Panics on failure. Convenient but fragile.
    /// `some_path.join(foo)` is basically equivalent to
    /// `some_path.try_join(foo).unwrap()`.
    pub fn join(&mut self, moar: &Path) -> &mut Self {
	self.try_join(moar).expect("Error attempting to join two paths")
    }
    /// Attempts to extend `self` by applying a relative path to it. The path
    /// *must* be relative. Returns an error on failure.
    pub fn try_join(&mut self, moar: &Path)
		    -> Result<&mut Self, PathJoinError> {
	if moar.is_absolute() {
	    Err(PathJoinError::PathNotRelative)
	}
	else if self.is_directory() || moar.inner.starts_with("../") {
	    let mut addendum = &moar.inner;
	    while let Some(nu) = addendum.strip_prefix("../") {
		if self.inner == "/" {
		    return Err(PathJoinError::EscapedRoot);
		}
		else if self.inner == "" {
		    break
		}
		addendum = nu;
		self.inner.pop(); // OK even if we didn't end with "/"
		while !self.inner.ends_with("/")
		    && self.inner.len() > 0 { self.inner.pop(); }
	    }
	    self.inner.push_str(addendum);
	    Ok(self)
	}
	else {
	    Err(PathJoinError::BasePathNotDir)
	}
    }
    /// If the given path is relative, attempts to extend `self` by applying
    /// this path. If the given path is absolute, replaces `self` with the new
    /// path. Panics on failure. Convenient but fragile.
    /// `some_path.join_or_replace(foo)` is basically equivalent to
    /// `some_path.try_join_or_replace(foo).unwrap()`.
    pub fn join_or_replace(&mut self, moar: &Path) -> &mut Self {
	self.try_join_or_replace(moar).expect("Error attempting to join two \
					       paths.")
    }
    /// If the given path is relative, attempts to extend `self` by applying
    /// this path. If the given path is absolute, replaces `self` with the new
    /// path. Returns an error on failure.
    pub fn try_join_or_replace(&mut self, moar: &Path)
			       -> Result<&mut Self, PathJoinError> {
	if moar.is_absolute() {
	    self.inner.clear();
	    self.inner.push_str(&moar.inner);
	    Ok(self)
	}
	else {
	    self.try_join(moar)
	}
    }
    /// Removes the innermost component of the path. Returns true if there was
    /// a component to remove, false otherwise. (Like calling
    /// [parent](struct.Path.html#method.parent) and making a new `PathBuf`
    /// from the result, but more efficient, and won't raise an error if called
    /// on acomponent-less path (returning false instead).
    pub fn up_one_level(&mut self) -> bool {
	if self.inner == "" || self.inner == "/" { return false }
	self.inner.pop();
	while !self.inner.ends_with("/")
	    && self.inner.len() > 0 { self.inner.pop(); }
	true
    }
    /// Invokes `reserve` on the internal `String`.
    pub fn reserve(&mut self, s: usize) { self.inner.reserve(s) }
    /// Invokes `reserve_exact` on the internal `String`.
    pub fn reserve_exact(&mut self, s: usize) { self.inner.reserve_exact(s) }
    /// Invokes `shrink_to_fit` on the internal `String`.
    pub fn shrink_to_fit(&mut self) { self.inner.shrink_to_fit() }
    /// Given a path to a file (e.g. `foo`), converts it into a path to a
    /// directory with the same name (e.g. `foo/`).
    pub fn make_file_into_dir(&mut self) {
        assert!(!self.is_directory());
        self.inner.push('/');
    }
}

impl Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
	Path::from_str_preverified(self.inner.as_str())
    }
}

impl AsRef<Path> for PathBuf {
    fn as_ref(&self) -> &Path {
	self.borrow()
    }
}

impl AsRef<str> for PathBuf {
    fn as_ref(&self) -> &str {
	self.inner.as_str()
    }
}

impl Deref for PathBuf {
    type Target = Path;
    fn deref(&self) -> &Path {
	self.as_ref()
    }
}

impl Display for PathBuf {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	Display::fmt(self.as_path(), fmt)
    }
}

impl Debug for PathBuf {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
	Debug::fmt(self.as_path(), fmt)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    fn is_borrowed(wat: &Cow<Path>) -> bool {
	match wat {
	    &Cow::Borrowed(_) => true,
	    _ => false,
	}
    }
    #[test] fn components() {
	assert_eq!(Path::from_str_preverified("foo/bar/baz").components()
		   .collect::<Vec<_>>(),
		   &["foo", "bar", "baz"]);
	assert_eq!(Path::from_str_preverified("/sora/donald/goofy").components()
		   .collect::<Vec<_>>(),
		   &["sora", "donald", "goofy"]);
	assert_eq!(Path::from_str_preverified("x/zero/").components()
		   .collect::<Vec<_>>(),
		   &["x", "zero"]);
	// this is an invalid path but this is what should happen with it
	assert_eq!(Path::from_str_preverified("sword/go//").components()
		   .collect::<Vec<_>>(),
		   &["sword", "go", ""]);
    }
    #[test] fn normalize_good() {
	const PAIRS_TO_CHECK: &[(&str, &str)] = &[
	    ("foo/./bar", "foo/bar"),
	    ("foo/../bar", "bar"),
	    ("/foo/./bar", "/foo/bar"),
	    ("/foo/../bar", "/bar"),
	    ("foo/../../bar", "../bar"),
	    ("tesuto/COM0", "tesuto/COM0"),
	];
	for (big, small) in PAIRS_TO_CHECK.iter() {
	    assert_eq!(&Path::from_str(big).inner, *small);
	}
    }
    #[test] fn normalize_bad() {
	const PAIRS_TO_CHECK: &[(&str, PathFromStrError)] = &[
	    ("/foo/../../bar", PathFromStrError::EscapedRoot),
	    ("asdf/NUL", PathFromStrError::ReservedName),
	    ("asdf/COM4", PathFromStrError::ReservedName),
	    ("asdf/COM5.test", PathFromStrError::ReservedName),
	    ("asdf/jkl/Lpt6.printer", PathFromStrError::ReservedName),
	];
	for (big, small) in PAIRS_TO_CHECK.iter() {
	    match Path::try_from_str(big) {
		Ok(_) => panic!("try_from_str on {:?} should fail", big),
		Err(x) => {
		    if x != *small {
			panic!("try_from_str on {:?} should fail with {:?}, \
				got {:?} instead", big, small, x);
		    }
		},
	    }
	}
    }
    #[test] fn joins_good() {
	const JOINS_TO_CHECK: &[(&str, &str, &str)] = &[
	    ("foo/", "bar", "foo/bar"),
	    ("/george/michael/", "../maharris", "/george/maharris"),
	    ("", "foo/bar", "foo/bar"),
	    ("", "../../tesla", "../../tesla"),
	    ("/seven/good/beers", "../../dwarves", "/seven/dwarves"),
	    ("lorem/ipsum/dolor/sit", "../../../../../rock", "../rock"),
	    ("mune/ni/kokoro", "../rocks", "mune/ni/rocks"),
	];
	for (a, b, r) in JOINS_TO_CHECK.iter() {
	    let mut a = PathBuf::from_str(a);
	    let b = Path::from_str(b);
	    a.join(&b);
	    assert_eq!(a.inner, *r);
	}
    }
    #[test] fn joins_bad() {
	const JOINS_TO_CHECK: &[(&str, &str, PathJoinError)] = &[
	    ("/test/toast", "natto", PathJoinError::BasePathNotDir),
	    ("/supreme", "../../ilpallazzo", PathJoinError::EscapedRoot),
	    ("/dinner", "/breakfast", PathJoinError::PathNotRelative),
	];
	for (a, b, e) in JOINS_TO_CHECK.iter() {
	    let mut buf = PathBuf::from_str(a);
	    let moar = Path::from_str(b);
	    match buf.try_join(&moar) {
		Ok(_) => panic!("Joining {:?} and {:?} is supposed to fail, ends up with {:?} instead", a, b, buf),
		Err(x) => {
		    if x != *e {
			panic!("Joining {:?} and {:?} is supposed to fail with {:?}, gets {:?} instead", a, b, e, x);
		    }
		}
	    }
	}
    }
    #[test] fn copies_vs_keeps() {
	const PATHS_TO_CHECK: &[(&str, bool)] = &[
	    ("/asdf", true),
	    ("/asdf/truth", true),
	    ("/asdf/../foxes", false),
	    ("resume\u{0301}", true),
	    ("resum\u{00e9}", false),
	    ("../foxes", true),
	];
	for (src, kept) in PATHS_TO_CHECK.iter() {
	    let result = Path::from_str(src);
	    if is_borrowed(&result) != *kept {
		if *kept {
		    panic!("{:?} is supposed to be borrowed but is copied.",
			   src);
		}
		else {
		    panic!("{:?} is supposed to be copied but is borrowed.",
			   src);
		}
	    }
	}
    }
    #[test] fn accent() {
	assert_eq!(Path::from_str("resume\u{0301}"),
		   Path::from_str("resum\u{00e9}"));
    }
}
