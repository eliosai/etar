//! Red-team corpus for SafePath, adapted from directory-traversal payloads

use etar::{EntryPath as SafePath, Error};
use std::path::Component;

#[test]
fn test_traversal_payloads_are_rejected() {
    let attacks = [
        "../etc/passwd",
        "../../../etc/passwd",
        "../../../../../../etc/passwd",
        "/etc/passwd",
        "/etc/shadow",
        "/root/.ssh/id_rsa",
        "a/../../b",
        "a/b/../../../c",
        "./../../etc/passwd",
        "foo/./../../bar",
        "..",
        "../",
        "subdir/..",
    ];
    for attack in attacks {
        let actual = SafePath::new(attack);
        assert!(
            matches!(actual, Err(Error::AbsolutePath | Error::ParentEscape)),
            "payload should be rejected: {attack:?} got {actual:?}"
        );
    }
}

#[test]
fn test_nul_and_empty_are_rejected() {
    assert!(matches!(
        SafePath::new("a\0/etc/passwd"),
        Err(Error::NulPath)
    ));
    assert!(matches!(SafePath::new(""), Err(Error::EmptyPath)));
}

#[test]
fn test_strip_bypass_name_is_literal_not_traversal() {
    let safe = SafePath::new("....//etc/passwd").expect("dotted name is in-tree");
    assert!(all_in_tree(&safe));
}

#[test]
fn test_backslash_is_a_literal_byte_on_linux() {
    let safe = SafePath::new("..\\..\\windows").expect("backslash is an ordinary byte");
    assert_eq!(safe.as_path().components().count(), 1);
}

#[test]
fn test_dot_only_path_names_the_root_and_is_accepted() {
    let safe = SafePath::new(".").expect("'.' names the in-tree root");
    assert!(all_in_tree(&safe));
}

#[test]
fn test_overlong_single_component_does_not_panic() {
    let long = "a".repeat(70_000);
    let safe = SafePath::new(&long).expect("a long literal name is in-tree");
    assert_eq!(safe.as_path().components().count(), 1);
}

#[test]
fn test_non_utf8_bytes_stay_in_tree() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let raw = OsStr::from_bytes(b"bin/\xff\xfe/sh");
    let safe = SafePath::new(raw).expect("non-utf8 bytes are ordinary name bytes");
    assert!(all_in_tree(&safe));
}

fn all_in_tree(path: &SafePath) -> bool {
    path.as_path()
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
