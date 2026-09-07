//! Offline test of `extract_tarball` against tarballs built on the fly with the
//! system `tar`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use perl_build::extract_tarball;

fn scratch(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "perl-build-it-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn have_tar() -> bool {
    Command::new("tar").arg("--help").output().is_ok()
}

/// Build `<work>/<root>/...` and pack it into `<work>/<archive>`.
fn make_tarball(work: &Path, root: &str, archive: &str, compression: &str) {
    let tree = work.join(root);
    fs::create_dir_all(tree.join("lib")).unwrap();
    fs::write(tree.join("Configure"), b"#!/bin/sh\necho fake\n").unwrap();
    fs::write(tree.join("lib").join("Foo.pm"), b"1;\n").unwrap();

    let status = Command::new("tar")
        .arg(compression)
        .arg("-cf")
        .arg(work.join(archive))
        .arg("-C")
        .arg(work)
        .arg(root)
        .status()
        .unwrap();
    assert!(status.success(), "tar failed to create {archive}");
}

#[test]
fn extracts_gzip_tarball_and_returns_root() {
    if !have_tar() {
        eprintln!("skipping: no `tar` on PATH");
        return;
    }
    let work = scratch("gz");
    make_tarball(&work, "perl-5.38.2", "perl-5.38.2.tar.gz", "-z");

    let dest = work.join("out");
    let src = extract_tarball(work.join("perl-5.38.2.tar.gz"), &dest).unwrap();

    assert_eq!(src, dest.join("perl-5.38.2"));
    assert!(src.join("Configure").is_file());
    assert!(src.join("lib/Foo.pm").is_file());

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn extracts_uncompressed_tarball() {
    if !have_tar() {
        eprintln!("skipping: no `tar` on PATH");
        return;
    }
    let work = scratch("plain");
    make_tarball(&work, "perl-5.8.9", "perl-5.8.9.tar", "--posix");

    let dest = work.join("out");
    let src = extract_tarball(work.join("perl-5.8.9.tar"), &dest).unwrap();
    assert_eq!(src, dest.join("perl-5.8.9"));

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn rejects_tarball_without_single_root() {
    if !have_tar() {
        eprintln!("skipping: no `tar` on PATH");
        return;
    }
    let work = scratch("multi");
    fs::create_dir_all(work.join("a")).unwrap();
    fs::create_dir_all(work.join("b")).unwrap();
    fs::write(work.join("a/x"), b"x").unwrap();
    fs::write(work.join("b/y"), b"y").unwrap();
    let status = Command::new("tar")
        .arg("-czf")
        .arg(work.join("bad.tar.gz"))
        .arg("-C")
        .arg(&work)
        .arg("a")
        .arg("b")
        .status()
        .unwrap();
    assert!(status.success());

    let err = extract_tarball(work.join("bad.tar.gz"), work.join("out")).unwrap_err();
    assert!(
        matches!(err, perl_build::Error::TarballLayout(_)),
        "unexpected error: {err:?}"
    );

    let _ = fs::remove_dir_all(&work);
}
