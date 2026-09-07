//! End-to-end build test: fetch a real Perl from CPAN, build it, run it.
//!
//! `#[ignore]`d by default (needs network, a C toolchain, and several minutes).
//!
//! ```text
//! cargo test --test build_e2e -- --ignored --nocapture
//! ```

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use perl_build::{PatchPerl, PerlBuild};

fn scratch() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("perl-build-e2e-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore = "network + toolchain + slow"]
async fn builds_perl_from_cpan_and_runs_it() {
    let version = option_env!("PERL_BUILD_TEST_VERSION").unwrap_or("5.40.2");
    let work = scratch();
    let prefix = work.join("perl");

    let built = PerlBuild::new(&prefix)
        .jobs(std::thread::available_parallelism().map_or(2, |n| n.get()))
        .test(false)
        .patchperl(PatchPerl::Auto)
        .build_dir(work.join("build"))
        .tarball_dir(work.join("tarball"))
        .install_from_cpan(version)
        .await
        .expect("build failed");

    assert_eq!(built.prefix(), prefix);
    assert!(
        built.perl().is_file(),
        "no perl at {}",
        built.perl().display()
    );

    let mut cmd = Command::new(built.perl());
    cmd.arg("-e").arg("print qq{$^V\\n}");
    built.apply_env(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(version), "expected {version} in {stdout:?}");

    let _ = fs::remove_dir_all(&work);
}
