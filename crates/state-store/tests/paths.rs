use state_store::VirtualPath;

#[test]
fn virtual_paths_retain_normalization_and_reject_traversal() {
    assert_eq!(VirtualPath::parse("/a//./b/").unwrap().as_str(), "/a/b");
    assert_eq!(VirtualPath::parse("//./").unwrap().as_str(), "/");
    for path in ["relative", "/a/../b", "/a\\b", "/a\0b"] {
        assert!(VirtualPath::parse(path).is_err(), "{path:?}");
    }
    assert!(VirtualPath::parse(&format!("/{}", "a".repeat(4096))).is_err());
}

#[test]
fn containment_respects_components_and_root() {
    let prefix = VirtualPath::parse("/notes").unwrap();
    for (path, contains) in [
        ("/notes", true),
        ("/notes/file", true),
        ("/notes-other/file", false),
        ("/", false),
    ] {
        let path = VirtualPath::parse(path).unwrap();
        assert_eq!(prefix.contains(&path), contains);
        assert!(VirtualPath::parse("/").unwrap().contains(&path));
    }
}
