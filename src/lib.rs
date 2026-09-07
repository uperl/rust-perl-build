//! Build new Perls from source.
//!
//! This crate is a Rust port of the [`Perl::Build`] CPAN distribution. It
//! downloads a Perl source tarball (from CPAN, an arbitrary URL, or the local
//! filesystem), unpacks it, runs `Configure` / `make` / `make install`, and
//! leaves you a ready-to-use Perl under a prefix you choose.
//!
//! Perl releases are located through [`metacpan-api-modern`][mcm], so the same
//! resolution CPAN clients use is available here: give a version like `5.38.2`
//! and the matching archive on the CPAN CDN is found for you.
//!
//! [`Perl::Build`]: https://metacpan.org/pod/Perl::Build
//! [mcm]: metacpan_api_modern
//!
//! # Interface
//!
//! [`PerlBuild`] is the entry point, configured with builder methods and then
//! finished with one of:
//!
//! | method | analogue in `Perl::Build` | needs network / async |
//! |---|---|---|
//! | [`PerlBuild::install_from_cpan`] | `install_from_cpan` | yes |
//! | [`PerlBuild::install_from_url`] | `install_from_url` | yes |
//! | [`PerlBuild::install_from_tarball`] | `install_from_tarball` | no |
//! | [`PerlBuild::install_from_source`] | `install` | no |
//!
//! Each returns a [`Built`], the equivalent of `Perl::Build::Built`, which knows
//! the installation prefix and the environment tweaks needed to run the new
//! Perl.
//!
//! [`PerlReleases`] wraps just the "which archive provides this Perl version"
//! lookup if that is all you need.
//!
//! # Examples
//!
//! Build Perl 5.38.2 straight from CPAN:
//!
//! ```no_run
//! # async fn run() -> Result<(), perl_build::Error> {
//! use perl_build::PerlBuild;
//!
//! let built = PerlBuild::new("/opt/perl-5.38.2")
//!     .jobs(4)
//!     .test(false)
//!     .install_from_cpan("5.38.2")
//!     .await?;
//!
//! println!("installed {}", built.perl().display());
//! # Ok(())
//! # }
//! ```
//!
//! Build from a tarball already on disk — no async runtime required, exactly as
//! `Perl::Build->install_from_tarball` works offline:
//!
//! ```no_run
//! # fn run() -> Result<(), perl_build::Error> {
//! use perl_build::PerlBuild;
//!
//! let built = PerlBuild::new("/opt/perl-5.38.2")
//!     .install_from_tarball("/downloads/perl-5.38.2.tar.gz")?;
//! # let _ = built;
//! # Ok(())
//! # }
//! ```
//!
//! # `patchperl`
//!
//! `Perl::Build` applies [`Devel::PatchPerl`](https://metacpan.org/pod/Devel::PatchPerl)
//! fix-ups to the source tree before `Configure`; without them, older Perls
//! frequently fail to build on a modern toolchain. There is no Rust
//! reimplementation of that logic, so this crate shells out to the `patchperl`
//! program instead. By default ([`PatchPerl::Auto`]) it is used when found on
//! `PATH` and skipped with a warning otherwise. Install it from CPAN with
//! `cpanm App::patchperl`, or point at a specific one with
//! [`PerlBuild::patchperl`]. The `PERL5_PATCHPERL_PLUGIN` environment variable
//! is inherited by the `patchperl` child process, so patch plugins work as they
//! do for `Perl::Build`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod build;
mod built;
mod error;
mod releases;

pub use build::{PatchPerl, PerlBuild, extract_tarball};
pub use built::{Built, symlink_devel_executables};
pub use error::{Error, Result};
pub use releases::{PerlRelease, PerlReleases};

/// Re-export of the MetaCPAN client library, so callers can build and pass a
/// customised [`metacpan_api_modern::Client`] (proxy, cache directory, ...)
/// into [`PerlBuild::metacpan_client`] / [`PerlReleases::with_client`] without
/// taking their own dependency on it.
pub use metacpan_api_modern;
