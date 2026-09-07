//! The result of a successful build: [`Built`], mirroring `Perl::Build::Built`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::Result;

/// A Perl that has been built and installed under a prefix.
///
/// Returned by every `install_*` method on [`PerlBuild`](crate::PerlBuild). It
/// is the Rust equivalent of `Perl::Build::Built`: it knows where the Perl
/// lives and how to arrange an environment that runs it cleanly.
#[derive(Debug, Clone)]
pub struct Built {
    prefix: PathBuf,
}

impl Built {
    /// Wrap an existing installation prefix (the `-Dprefix` the Perl was built
    /// with).
    pub fn new(prefix: impl Into<PathBuf>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// The installation prefix.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// `<prefix>/bin`.
    pub fn bin_dir(&self) -> PathBuf {
        self.prefix.join("bin")
    }

    /// `<prefix>/bin/perl`.
    pub fn perl(&self) -> PathBuf {
        self.bin_dir().join("perl")
    }

    /// The environment changes needed to run this Perl: `PATH` with
    /// [`bin_dir`](Self::bin_dir) prepended, and the `PERL5LIB` / `PERL5OPT` /
    /// installer-option variables cleared so an outer environment (perlbrew,
    /// local::lib, ...) cannot leak in. A `None` value means "unset this".
    pub fn env_vars(&self) -> Vec<(&'static str, Option<OsString>)> {
        vec![
            ("PATH", Some(self.prepended_path())),
            ("PERL5LIB", None),
            ("PERL5OPT", None),
            ("PERL_MB_OPT", None),
            ("PERL_MM_OPT", None),
            ("PERL_LOCAL_LIB_ROOT", None),
        ]
    }

    /// Apply [`env_vars`](Self::env_vars) to a [`Command`] so it runs against
    /// this Perl.
    ///
    /// ```no_run
    /// # fn run() -> Result<(), perl_build::Error> {
    /// use std::process::Command;
    /// let built = perl_build::Built::new("/opt/perl-5.38.2");
    /// let mut cmd = Command::new("perl");
    /// cmd.arg("-v");
    /// built.apply_env(&mut cmd);
    /// cmd.status()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn apply_env(&self, cmd: &mut Command) {
        for (key, val) in self.env_vars() {
            match val {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
    }

    fn prepended_path(&self) -> OsString {
        let bin = self.bin_dir();
        match std::env::var_os("PATH") {
            Some(existing) => {
                let mut dirs = vec![bin.clone()];
                dirs.extend(std::env::split_paths(&existing));
                std::env::join_paths(dirs).unwrap_or_else(|_| bin.into_os_string())
            }
            None => bin.into_os_string(),
        }
    }
}

/// For every versioned executable in `bin_dir` (`perl5.38.2`, `h2ph5.38.2`,
/// ...), create a symlink under the un-versioned name (`perl`, `h2ph`) unless
/// one already exists.
///
/// Mirrors `Perl::Build->symlink_devel_executables`, which exists because a
/// `-Dusedevel` build installs its binaries with a version suffix.
/// [`PerlBuild`](crate::PerlBuild) runs this automatically after a `-Dusedevel`
/// install; call it yourself for other cases.
///
/// On non-Unix platforms the files are copied instead of symlinked.
pub fn symlink_devel_executables(bin_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(bin_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(base) = devel_base_name(&name) else {
            continue;
        };
        let link = bin_dir.join(base);
        if link.exists() {
            continue;
        }
        log::info!("symlink {base} -> {name}");
        symlink(&file_name, &link)?;
    }
    Ok(())
}

/// Mirror of the Perl regex `/^(.+?)(5\.\d.*)?$/`: the base name is the part
/// before the first `5.` that is followed by a digit, and there must be
/// something before it.
fn devel_base_name(name: &str) -> Option<&str> {
    let idx = name.match_indices("5.").find_map(|(i, _)| {
        (i > 0 && name[i + 2..].starts_with(|c: char| c.is_ascii_digit())).then_some(i)
    })?;
    Some(&name[..idx])
}

#[cfg(unix)]
fn symlink(target: &OsStr, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink(target: &OsStr, link: &Path) -> Result<()> {
    let dir = link.parent().unwrap_or_else(|| Path::new("."));
    std::fs::copy(dir.join(target), link)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::devel_base_name;

    #[test]
    fn extracts_devel_base_names() {
        assert_eq!(devel_base_name("perl5.38.2"), Some("perl"));
        assert_eq!(devel_base_name("h2ph5.38.2"), Some("h2ph"));
        assert_eq!(devel_base_name("perl5.40.0"), Some("perl"));
        assert_eq!(devel_base_name("perl"), None);
        assert_eq!(devel_base_name("corelist"), None);
        assert_eq!(devel_base_name("5.38.2"), None); // nothing before the version
        assert_eq!(devel_base_name("cpan5."), None); // "5." not followed by a digit
    }
}
