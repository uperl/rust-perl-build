//! Build new Perls from source.
//!
//! This crate is a Rust port of the [`Perl::Build`] CPAN distribution. It
//! downloads a Perl source tarball (from CPAN, an arbitrary URL, or the local
//! filesystem), unpacks it, runs `Configure` / `make` / `make install` (or
//! `nmake` against `win32\Makefile` on Windows — see [below](#windows-and-visual-c)),
//! and leaves you a ready-to-use Perl under a prefix you choose.
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
//! frequently fail to build on a modern toolchain. This crate can apply them
//! two ways, selected with [`PerlBuild::patchperl`] / [`PatchPerl`]:
//!
//! * the **external `patchperl` program** (from `App::patchperl` on CPAN); the
//!   `PERL5_PATCHPERL_PLUGIN` environment variable is inherited by the child, so
//!   Perl-module patch plugins work as they do for `Perl::Build`;
//! * the **in-process [`patch-perl`](patch_perl) crate**, a Rust port of the
//!   `Devel::PatchPerl` library — no external program, no `sh` / `patch`. The
//!   `PERL5_PATCHPERL_PLUGIN` hook is not honoured on this path (the crate
//!   resolves it to a native shared library, not a Perl module), so it applies
//!   only the standard fix-ups.
//!
//! The default, [`PatchPerl::Auto`], uses the external program when it is on
//! `PATH` and the in-process port otherwise, so the fix-ups are always applied.
//! [`PatchPerl::External`] keeps the old behaviour (external only; warn and skip
//! when missing), [`PatchPerl::Internal`] always uses the crate, and
//! [`PatchPerl::Disabled`] applies nothing.
//!
//! # Windows and Visual C++
//!
//! On Windows the default toolchain ([`Toolchain::Auto`]) builds
//! `win32\Makefile` from the source tree with `nmake` and Microsoft Visual
//! C++, rather than `sh Configure` / `make`. `cl.exe` must be on `PATH`, so run
//! from a Visual Studio "x64 Native Tools" command prompt or a shell that has
//! sourced `vcvarsall.bat`.
//!
//! The installation prefix is handed to the Makefile as the `INST_TOP` /
//! `INST_DRV` macros, so the source tree is not edited. `win32\Makefile`
//! requires `CCTYPE` and has no detection of its own, so this crate runs `cl`
//! to work it out — unless `PERL_BUILD_COMPILE_OPTIONS` already contains a
//! `CCTYPE=...` entry. Force a particular toolchain (the Unix path on Windows,
//! or the MSVC path elsewhere) with [`PerlBuild::toolchain`]. The make program
//! can be overridden with `PERL_BUILD_NMAKE`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod build;
mod built;
mod error;
mod releases;

pub use build::{PatchPerl, PerlBuild, Toolchain, extract_tarball};
pub use built::{Built, symlink_devel_executables};
pub use error::{Error, Result};
pub use releases::{PerlRelease, PerlReleases};

/// Re-export of the MetaCPAN client library, so callers can build and pass a
/// customised [`metacpan_api_modern::Client`] (proxy, cache directory, ...)
/// into [`PerlBuild::metacpan_client`] / [`PerlReleases::with_client`] without
/// taking their own dependency on it.
pub use metacpan_api_modern;
