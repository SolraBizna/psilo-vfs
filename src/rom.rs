use crate::*;

use std::{
    fmt,
    io, io::{Cursor, ErrorKind},
};

use async_trait::async_trait;

#[derive(Clone)]
pub enum Node {
    File(&'static [u8]),
    Dir(Vec<(&'static Path, Node)>),
}

impl fmt::Debug for Node {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Node::File(data) => write!(fmt, "Node::File({} bytes)",
                                       data.len()),
            Node::Dir(subnodes) => write!(fmt, "Node::Dir({} entries)",
                                          subnodes.len()),
        }
    }
}

#[derive(Clone)]
pub struct Source {
    root: Node,
}

impl Source {
    pub fn new(listing: &[(&'static Path, &'static [u8])]) -> Source {
        let mut root = Node::Dir(vec![]);
        for (path, data) in listing {
            if !path.is_absolute() {
                panic!("BUG IN YOUR PROGRAM: \
                        RomSource listing contained a relative path! {:?}",
                       path)
            }
            if path.is_directory() && data.len() > 0 {
                panic!("BUG IN YOUR PROGRAM: \
                        RomSource listing contained a directory with \
                        data! {:?}", path)
            }
            if *path == "/" {
                panic!("BUG IN YOUR PROGRAM: \
                        RomSource listing contained an explicit root!")
            }
            let mut this_node = &mut root;
            let mut components = path.components();
            let mut cur_component = components.next().unwrap();
            while let Some(next_component) = components.next() {
                match this_node {
                    Node::File(..) =>
                        panic!("BUG IN YOUR PROGRAM: \
                                RomSource listing contained a file that \
                                was \"under\" another file! {:?}",
                               path),
                    Node::Dir(ref mut subnodes) => {
                        match subnodes.binary_search_by
                          (|(x,_)| (*x).cmp(cur_component)) {
                            Ok(i) => {
                                // This component already exists in the tree.
                                this_node = &mut subnodes[i].1;
                            },
                            Err(i) => {
                                // This component doesn't already exist in the
                                // tree. Insert it as a directory.
                                subnodes.insert(i, (cur_component,
                                                    Node::Dir(vec![])));
                                this_node = &mut subnodes[i].1;
                            },
                        }
                    },
                }
                cur_component = next_component;
            }
            match this_node {
                Node::File(..) =>
                    panic!("BUG IN YOUR PROGRAM: \
                            RomSource listing contained a file that was \
                            \"under\" another file! {:?}", path),
                Node::Dir(ref mut subnodes) => {
                    match subnodes.binary_search_by
                        (|(x,_)| (*x).cmp(cur_component)) {
                            Ok(_) =>
                                // This component already exists in the tree.
                                panic!("BUG IN YOUR PROGRAM: \
                                        RomSource listing contained a \
                                        duplicate! {:?}", path),
                            Err(i) => {
                                // This component doesn't already exist in the
                                // tree. Insert it as a new file or directory.
                                if path.is_directory() {
                                    subnodes.insert(i, (cur_component,
                                                        Node::Dir(vec![])));
                                }
                                else {
                                    subnodes.insert(i, (cur_component,
                                                        Node::File(data)));
                                }
                            },
                        }
                },
            }
        }
        Source { root }
    }
    fn resolve(&self, path: &Path) -> Option<&Node> {
        let mut this_node = &self.root;
        'outer: for component in path.components() {
            match this_node {
                Node::File(..) => return None,
                Node::Dir(subnodes) => {
                    for (name, subnode) in subnodes.iter() {
                        if *name != component { continue }
                        this_node = &subnode;
                        continue 'outer
                    }
                    return None
                },
            }
        }
        return Some(this_node)
    }
}

#[async_trait]
impl VFSSource for Source {
    async fn open(&self, path: &Path) -> io::Result<Box<dyn DataFile>> {
        debug_assert!(path.is_absolute() && !path.is_directory());
        match self.resolve(path) {
            Some(Node::File(data))
                => Ok(Box::new(Cursor::new(data as &'static[u8]))),
            Some(Node::Dir(..))
                => Err(io::Error::from(ErrorKind::IsADirectory)),
            None => Err(io::Error::from(ErrorKind::NotFound)),
        }
    }
    async fn ls(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        debug_assert!(path.is_absolute() && path.is_directory());
        match self.resolve(path) {
            Some(Node::Dir(nodes)) =>
                Ok(nodes.iter().map(|(name, node)| {
                    let mut ret = (*name).to_owned();
                    if let Node::Dir(..) = node {
                        ret.make_file_into_dir();
                    }
                    ret
                }).collect()),
            Some(Node::File(..))
                => Err(io::Error::from(ErrorKind::NotADirectory)),
            None => Err(io::Error::from(ErrorKind::NotFound)),
        }
    }
    async fn update(&self, _: &Path, _: &[u8]) -> io::Result<()> {
        Err(io::Error::from(ErrorKind::ReadOnlyFilesystem))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::io::AsyncReadExt;
    const fn fsp(i: &str) -> &Path { Path::from_str_preverified(i) }
    #[test] #[should_panic]
    fn no_relative_paths() {
        Source::new(&[(fsp("relative/path"), b"some_data")]);
    }
    #[test] #[should_panic]
    fn no_root_path() {
        Source::new(&[(fsp("/"), b"some_data")]);
    }
    #[test] #[should_panic]
    fn no_dir_data() {
        Source::new(&[(fsp("/dir/"), b"some_data")]);
    }
    #[test] #[should_panic]
    fn no_file_under_file() {
        Source::new(&[(fsp("/some/file"), b"some_data"),
                          (fsp("/some/file/beneath"), b"some_data")]);
    }
    #[test] #[should_panic]
    fn no_file_deep_under_file() {
        Source::new(&[(fsp("/some/file"), b"some_data"),
                          (fsp("/some/file/deep/beneath"), b"some_data")]);
    }
    #[tokio::test]
    async fn some_stuff() {
        const LISTING: &[(&Path, &[u8])] = &[
            (fsp("/Data/"), b""),
            (fsp("/Data/Subdir/Pi"), b"3.1415 etc."),
            (fsp("/Data/Subdir/lipsum"), b"Lorem ipsum dolor sit amet?"),
            (fsp("/Data/freq"), b"456"),
        ];
        let source = Source::new(LISTING);
        for (path, data) in LISTING {
            if path.is_directory() { continue }
            let mut file = source.open(path).await.unwrap();
            let mut buf = Vec::with_capacity(data.len());
            file.read_to_end(&mut buf).await.unwrap();
            assert_eq!(*data, buf);
        }
    }
    #[tokio::test]
    /// Tests the specific union mounts that are given in the documentation.
    /// This actually tests the `data` module, it's just that the `rom` module
    /// is required in order for the test to work.
    async fn documented_unions() {
        const A: &[(&Path, &[u8])] = &[
            (fsp("/bar/"), b""),
            (fsp("/bar/baz"), b"baz from A"),
            (fsp("/foo"), b"foo from A"),
        ];
        const B: &[(&Path, &[u8])] = &[
            (fsp("/bar/"), b""),
            (fsp("/bar/bang"), b"bang from B"),
            (fsp("/foo"), b"foo from B"),
        ];
        const C: &[(&Path, &[u8])] = &[
            (fsp("/foo/"), b""),
            (fsp("/foo/barf"), b"barf from C"),
        ];
        struct Expectation {
            name: &'static str,
            sources: &'static [(&'static Path,
                                &'static [(&'static Path, &'static [u8])])],
            files: &'static [(&'static Path, &'static [u8])],
            listings: &'static [(&'static Path, &'static [&'static str])],
        }
        const EXPECTATIONS: &[Expectation] = &[
            // sanity check, each mount came through intact
            Expectation {
                name: "/A",
                sources: &[(fsp("/"), A)],
                files: &[
                    (fsp("/bar/baz"), b"baz from A"),
                    (fsp("/foo"), b"foo from A"),
                ],
                listings: &[
                    (fsp("/"), &["bar/", "foo"]),
                    (fsp("/bar/"), &["baz"]),
                ],
            },
            Expectation {
                name: "/B",
                sources: &[(fsp("/"), B)],
                files: &[
                    (fsp("/bar/bang"), b"bang from B"),
                    (fsp("/foo"), b"foo from B"),
                ],
                listings: &[
                    (fsp("/"), &["bar/", "foo"]),
                    (fsp("/bar/"), &["bang"]),
                ],
            },
            Expectation {
                name: "/C",
                sources: &[(fsp("/"), C)],
                files: &[
                    (fsp("/foo/barf"), b"barf from C"),
                ],
                listings: &[
                    (fsp("/"), &["foo/"]),
                    (fsp("/foo/"), &["barf"]),
                ],
            },
            // the documented unions
            Expectation {
                name: "/A + /B",
                sources: &[(fsp("/"), A), (fsp("/"), B)],
                files: &[
                    (fsp("/bar/baz"), b"baz from A"),
                    (fsp("/bar/bang"), b"bang from B"),
                    (fsp("/foo"), b"foo from B"),
                ],
                listings: &[
                    (fsp("/"), &["bar/", "foo"]),
                    (fsp("/bar/"), &["bang", "baz"]),
                ],
            },
            Expectation {
                name: "/A + /plugins/fnord/B",
                sources: &[(fsp("/"), A), (fsp("/plugins/fnord/"), B)],
                files: &[
                    (fsp("/bar/baz"), b"baz from A"),
                    (fsp("/foo"), b"foo from A"),
                    (fsp("/plugins/fnord/bar/bang"), b"bang from B"),
                    (fsp("/plugins/fnord/foo"), b"foo from B"),
                ],
                listings: &[
                    (fsp("/"), &["bar/", "foo", "plugins/"]),
                    (fsp("/bar/"), &["baz"]),
                    (fsp("/plugins/"), &["fnord/"]),
                    (fsp("/plugins/fnord/"), &["bar/", "foo"]),
                    (fsp("/plugins/fnord/bar/"), &["bang"]),
                ],
            },
            Expectation {
                name: "/A + /B + /C",
                sources: &[(fsp("/"), A), (fsp("/"), B), (fsp("/"), C)],
                files: &[
                    (fsp("/bar/baz"), b"baz from A"),
                    (fsp("/bar/bang"), b"bang from B"),
                    (fsp("/foo/barf"), b"barf from C"),
                ],
                listings: &[
                    (fsp("/"), &["bar/", "foo/"]),
                    (fsp("/bar/"), &["bang", "baz"]),
                    (fsp("/foo/"), &["barf"]),
                ],
            },
        ];
        let mut all_failures: Vec<(&'static str, Vec<String>)> = vec![];
        for expectation in EXPECTATIONS {
            let mut failures = Vec::new();
            let mut vfs = VFS::new();
            for &(point, source) in expectation.sources {
                let source = Box::new(Source::new(source));
                vfs.mount(point.to_owned(), source).await.unwrap();
            }
            let vfs = vfs;
            for &(path, content) in expectation.files {
                assert!(!path.is_directory());
                let mut file = match vfs.open(path).await {
                    Ok(x) => x,
                    Err(x) => {
                        failures.push(format!("{:?}: open: {}", path, x));
                        continue
                    },
                };
                let mut buf = Vec::with_capacity(content.len());
                file.read_to_end(&mut buf).await.unwrap(); // should never fail
                if content != buf {
                    failures.push(format!("{:?}: bad content, \
                                           wanted {:?}, got {:?}", path,
                                          String::from_utf8_lossy(content),
                                          String::from_utf8_lossy(&buf)));
                }
            }
            for &(path, results) in expectation.listings {
                let ls: Vec<String> = match vfs.ls(path).await {
                    Ok(x) => x,
                    Err(x) => {
                        failures.push(format!("{:?}: ls: {}", path, x));
                        continue
                    },
                }.into_iter().map(|x| x.as_str().to_owned()).collect();
                let res: Vec<String> = results.iter().map(|&x| x.to_owned()).collect();
                if ls != res {
                    failures.push(format!("{:?}: bad listing, \
                                           wanted {:?}, got {:?}", path,
                                          res, ls));
                }
            }
            if failures.len() > 0 {
                all_failures.push((expectation.name, failures))
            }
        }
        if all_failures.len() > 0 {
            for (wo, was) in all_failures.into_iter() {
                eprintln!("\nWithin {}:\n", wo);
                for failure in was.into_iter() {
                    eprintln!("{}", failure);
                }
            }
            eprintln!("");
            panic!("See above");
        }
    }
}
