//! Integration tests that hit the real `fastapi.metacpan.org` service.
//!
//! They are `#[ignore]`d by default so `cargo test` stays offline. Run them
//! with:
//!
//! ```text
//! cargo test --test live -- --ignored --nocapture
//! ```

use perl_build::PerlReleases;

#[tokio::test]
#[ignore = "network"]
async fn finds_a_known_release() {
    let releases = PerlReleases::new();
    let r = releases.find("5.38.2").await.unwrap();

    assert_eq!(r.name, "perl-5.38.2");
    assert_eq!(r.version, "5.38.2");
    assert_eq!(r.author, "PEVANS");
    assert_eq!(
        r.download_url,
        "https://cpan.metacpan.org/authors/id/P/PE/PEVANS/perl-5.38.2.tar.gz"
    );
    assert!(!r.is_developer());
}

#[tokio::test]
#[ignore = "network"]
async fn accepts_the_perl_prefix() {
    let r = PerlReleases::new().find("perl-5.36.0").await.unwrap();
    assert_eq!(r.name, "perl-5.36.0");
    assert!(r.download_url.ends_with("perl-5.36.0.tar.gz"));
}

#[tokio::test]
#[ignore = "network"]
async fn unknown_version_is_release_not_found() {
    let err = PerlReleases::new().find("5.999.999").await.unwrap_err();
    assert!(
        matches!(&err, perl_build::Error::ReleaseNotFound(v) if v == "5.999.999"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
#[ignore = "network"]
async fn latest_is_a_stable_release() {
    let r = PerlReleases::new().latest().await.unwrap();
    assert!(r.name.starts_with("perl-"), "got {}", r.name);
    assert!(!r.is_developer());
    assert!(r.download_url.starts_with("https://"));
}

#[tokio::test]
#[ignore = "network"]
async fn list_has_many_stable_releases_and_no_dev() {
    let stable = PerlReleases::new().list().await.unwrap();
    assert!(stable.len() > 50, "only got {}", stable.len());
    assert!(stable.iter().all(|r| !r.is_developer()));
    assert!(stable.iter().any(|r| r.name == "perl-5.38.2"));

    let with_dev = PerlReleases::new().list_with_dev().await.unwrap();
    assert!(with_dev.len() >= stable.len());
}
