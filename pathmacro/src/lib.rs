use std::{
    error::Error,
    fmt::{Display, Formatter},
};
use proc_macro::TokenStream;
use syn::{parse_macro_input, LitStr};
use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::char::decompose_canonical;
use quote::quote;

fn normalized(s: &str) -> String {
    // `s.len()` will usually be exactly enough
    let mut ret = String::with_capacity(s.len());
    for c in s.chars() {
	decompose_canonical(c, |c| ret.push(c));
    }
    ret
}

// Let's duplicate most of the logic of `Path::from_str` and include
// `PathFromStrError` verbatim!
#[derive(Debug,PartialEq,Eq)]
enum PathFromStrError {
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

fn validated(s: &str) -> Result<String, PathFromStrError> {
    if s == "" || s == "/" {
	return Ok(s.to_string())
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
    for component in subset.split('/') {
	if component == "." || component == ".." {}
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
    }
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
    Ok(ret)
}

#[proc_macro]
pub fn p(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as LitStr);
    let value = input.value();
    let value = normalized(&value);
    let value = match validated(&value) {
	Ok(x) => x,
	Err(x) => {
	    return proc_macro::TokenStream::from(syn::parse::Error::new_spanned(input, x.to_string()).to_compile_error())
	},
    };
    (quote!{
	::psilo_vfs::Path::from_str_preverified(#value)
    }).into()
}
