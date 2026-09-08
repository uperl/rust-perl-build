//! Error and result types.

use std::path::PathBuf;
use std::process::ExitStatus;

/// Convenience alias for results returned by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Anything that can go wrong while resolving, fetching, or building a Perl.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The MetaCPAN API could not be queried, or returned an error.
    #[error("metacpan api error: {0}")]
    Metacpan(#[from] metacpan_api_modern::Error),

    /// A tarball download failed at the transport level (DNS, TLS, timeout, ...).
    #[error("http request failed: {0}")]
    Http(#[from] metacpan_api_modern::reqwest::Error),

    /// No Perl release matching the requested version was found on CPAN.
    #[error("no Perl release found on CPAN for version {0:?}")]
    ReleaseNotFound(String),

    /// A tarball download returned a non-success HTTP status.
    #[error("download of {url} failed: HTTP {status}")]
    Download {
        /// The URL that was requested.
        url: String,
        /// The HTTP status code that came back.
        status: u16,
    },

    /// An external program (`sh`, `make`, `tar`, `patchperl`, ...) could not be
    /// started at all.
    #[error("could not spawn `{0}`: {1}")]
    Spawn(String, #[source] std::io::Error),

    /// An external program ran but exited non-zero.
    #[error("command `{command}` failed ({status})")]
    Command {
        /// A human-readable rendering of the command line.
        command: String,
        /// The exit status it reported.
        status: ExitStatus,
    },

    /// A tarball did not unpack to exactly one top-level directory, so the
    /// source tree could not be located.
    #[error("tarball {0} does not unpack to exactly one top-level directory")]
    TarballLayout(PathBuf),

    /// The in-process [`patch-perl`](patch_perl) port failed to apply the
    /// Devel::PatchPerl fix-ups.
    #[error("Devel::PatchPerl fix-ups failed: {0}")]
    PatchPerl(#[from] patch_perl::Error),

    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Anything else, with a message.
    #[error("{0}")]
    Other(String),
}
