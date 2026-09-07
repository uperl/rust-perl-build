//! [`PerlBuild`]: configure a build and run it.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use metacpan_api_modern::Client;

use crate::built::{Built, symlink_devel_executables};
use crate::error::{Error, Result};
use crate::releases::PerlReleases;

/// How to run [`Devel::PatchPerl`](https://metacpan.org/pod/Devel::PatchPerl)
/// source fix-ups before `Configure`.
///
/// There is no Rust port of `Devel::PatchPerl`, so this crate shells out to the
/// `patchperl` program. See the [crate docs](crate#patchperl).
#[derive(Debug, Clone, Default)]
pub enum PatchPerl {
    /// Run `patchperl` from `PATH` if it is there; otherwise skip patching and
    /// log a warning. This is the default.
    #[default]
    Auto,
    /// Always run this specific program (a name resolved via `PATH`, or a full
    /// path).
    Command(OsString),
    /// Never patch the source tree.
    Disabled,
}

/// A configurable Perl build.
///
/// Create one with [`PerlBuild::new`], set options with the builder methods,
/// then finish with an `install_*` method. Modelled on the `Perl::Build` class
/// methods of the same names.
#[derive(Debug, Clone)]
pub struct PerlBuild {
    dst_path: PathBuf,
    configure_options: Vec<String>,
    jobs: Option<usize>,
    test: bool,
    build_dir: Option<PathBuf>,
    tarball_dir: Option<PathBuf>,
    patchperl: PatchPerl,
    client: Option<Client>,
}

impl PerlBuild {
    /// Start a build that will install to `dst_path` (the `-Dprefix`).
    ///
    /// A relative path is resolved against the current directory when the build
    /// runs. The default configure options are `["-de"]`, matching
    /// `Perl::Build`.
    pub fn new(dst_path: impl Into<PathBuf>) -> Self {
        Self {
            dst_path: dst_path.into(),
            configure_options: vec!["-de".to_owned()],
            jobs: None,
            test: false,
            build_dir: None,
            tarball_dir: None,
            patchperl: PatchPerl::Auto,
            client: None,
        }
    }

    /// Replace the options passed to `./Configure`.
    ///
    /// `-Dprefix=<dst_path>` is prepended for you, and
    /// `-Aeval:scriptdir=<dst_path>/bin` appended, exactly as `Perl::Build`
    /// does — do not include those here. A typical non-default value is
    /// something like `["-de", "-Dusethreads"]`.
    pub fn configure_options<I, S>(mut self, opts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.configure_options = opts.into_iter().map(Into::into).collect();
        self
    }

    /// Append a single `./Configure` option.
    pub fn configure_option(mut self, opt: impl Into<String>) -> Self {
        self.configure_options.push(opt.into());
        self
    }

    /// Number of parallel jobs for `make` and `make test` (`make -j N`, and
    /// `TEST_JOBS=N` while testing). Default: serial.
    pub fn jobs(mut self, jobs: usize) -> Self {
        self.jobs = Some(jobs);
        self
    }

    /// Run the test suite (`make test_harness`, or `make test` for Perls older
    /// than 5.8) after `make` and before `make install`. Default: `false`.
    pub fn test(mut self, test: bool) -> Self {
        self.test = test;
        self
    }

    /// Directory to unpack the source tarball into. A temporary directory is
    /// created (and left in place) when unset.
    pub fn build_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.build_dir = Some(dir.into());
        self
    }

    /// Directory to download source tarballs into. A temporary directory is
    /// created (and left in place) when unset. Only used by
    /// [`install_from_cpan`](Self::install_from_cpan) and
    /// [`install_from_url`](Self::install_from_url).
    pub fn tarball_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.tarball_dir = Some(dir.into());
        self
    }

    /// How to apply `patchperl` fix-ups. Default: [`PatchPerl::Auto`].
    pub fn patchperl(mut self, patchperl: PatchPerl) -> Self {
        self.patchperl = patchperl;
        self
    }

    /// Supply a configured [`metacpan_api_modern::Client`] for CPAN version
    /// resolution and downloads. A default client is used otherwise.
    pub fn metacpan_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    // -- terminal operations --------------------------------------------------

    /// Resolve `version` to a CPAN archive, download it, and build.
    ///
    /// `version` is the dotted release version, with or without the `perl-`
    /// prefix: `"5.38.2"`, `"perl-5.40.0"`. Equivalent to
    /// `Perl::Build->install_from_cpan`.
    pub async fn install_from_cpan(&self, version: &str) -> Result<Built> {
        let releases = PerlReleases::with_client(self.client());
        let release = releases.find(version).await?;
        log::info!(
            "resolved perl {version} to {} ({})",
            release.name,
            release.download_url
        );
        self.install_from_url(&release.download_url).await
    }

    /// Download a source tarball from an arbitrary URL and build it. Equivalent
    /// to `Perl::Build->install_from_url`.
    pub async fn install_from_url(&self, url: &str) -> Result<Built> {
        let tarball_dir = match &self.tarball_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                dir.clone()
            }
            None => temp_dir("perl-build-tarball")?,
        };
        let dest = tarball_dir.join(url_filename(url));
        download(&self.client(), url, &dest).await?;
        self.install_from_tarball(&dest)
    }

    /// Unpack a local source tarball and build it. Handles `.tar.gz`,
    /// `.tar.bz2`, `.tar.xz`, and uncompressed `.tar`. Equivalent to
    /// `Perl::Build->install_from_tarball`; needs no network or async runtime.
    pub fn install_from_tarball(&self, tarball: impl AsRef<Path>) -> Result<Built> {
        let tarball = tarball.as_ref();
        let build_dir = match &self.build_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                dir.clone()
            }
            None => temp_dir("perl-build-src")?,
        };
        let src = extract_tarball(tarball, &build_dir)?;
        self.install_from_source(&src)
    }

    /// Build from an already-unpacked source tree. Runs `Configure`, `make`,
    /// optionally `make test`, then `make install`. Equivalent to
    /// `Perl::Build->install`; needs no network or async runtime.
    pub fn install_from_source(&self, src_path: impl AsRef<Path>) -> Result<Built> {
        let src = src_path.as_ref();
        let dst = absolute(&self.dst_path)?;
        log::info!(
            "building perl from {} into {}",
            src.display(),
            dst.display()
        );

        let configure_options = self.resolved_configure_options(&dst);

        // A stale config from an earlier, aborted build confuses Configure.
        for stale in ["config.sh", "Policy.sh"] {
            let path = src.join(stale);
            if path.exists() {
                log::debug!("removing stale {}", path.display());
                let _ = std::fs::remove_file(&path);
            }
        }

        self.run_patchperl(src)?;

        // Configure
        let mut configure = self.build_command("sh", src);
        configure.arg("Configure").args(&configure_options);
        run(
            &mut configure,
            &format!("sh Configure {}", configure_options.join(" ")),
        )?;

        // make
        let mut make = self.build_command("make", src);
        if let Some(jobs) = self.jobs {
            make.arg("-j").arg(jobs.to_string());
        }
        append_split(&mut make, env_opt("PERL_BUILD_COMPILE_OPTIONS"));
        run(&mut make, "make")?;

        // make test
        if self.test {
            let target = if uses_test_harness(src) {
                "test_harness"
            } else {
                "test"
            };
            let mut test = self.build_command("make", src);
            test.arg(target);
            if let Some(jobs) = self.jobs {
                test.env("TEST_JOBS", jobs.to_string());
            }
            run(&mut test, &format!("make {target}"))?;
        }

        // make install
        let mut install = self.build_command("make", src);
        install.arg("install");
        append_split(&mut install, env_opt("PERL_BUILD_INSTALL_OPTIONS"));
        run(&mut install, "make install")?;

        let built = Built::new(dst);

        if configure_options.iter().any(|o| o.contains("usedevel")) {
            log::info!("-Dusedevel build: linking versioned executables");
            symlink_devel_executables(&built.bin_dir())?;
        }

        log::info!("installed perl in {}", built.prefix().display());
        Ok(built)
    }

    // -- internals ----------------------------------------------------------

    fn client(&self) -> Client {
        self.client.clone().unwrap_or_default()
    }

    fn resolved_configure_options(&self, dst: &Path) -> Vec<String> {
        let mut opts = Vec::with_capacity(self.configure_options.len() + 2);
        if !self
            .configure_options
            .iter()
            .any(|o| o.starts_with("-Dprefix"))
        {
            opts.push(format!("-Dprefix={}", dst.display()));
        }
        opts.extend(self.configure_options.iter().cloned());
        if !opts.iter().any(|o| o.contains("scriptdir")) {
            opts.push(format!("-Aeval:scriptdir={}/bin", dst.display()));
        }
        opts
    }

    /// A `Command` that runs in `cwd` with the outer `PERL5LIB` / `PERL5OPT`
    /// removed, so an active perlbrew / local::lib does not perturb the build.
    fn build_command<S: AsRef<OsStr>>(&self, program: S, cwd: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(cwd);
        cmd.env_remove("PERL5LIB");
        cmd.env_remove("PERL5OPT");
        cmd
    }

    fn run_patchperl(&self, src: &Path) -> Result<()> {
        let program: OsString = match &self.patchperl {
            PatchPerl::Disabled => {
                log::info!("patchperl: disabled");
                return Ok(());
            }
            PatchPerl::Command(program) => program.clone(),
            PatchPerl::Auto => match which("patchperl") {
                Some(_) => OsString::from("patchperl"),
                None => {
                    log::warn!(
                        "`patchperl` not found on PATH; building without Devel::PatchPerl \
                         fix-ups (older Perls may fail to build). Install it with \
                         `cpanm App::patchperl`, or pass PatchPerl::Disabled to silence this."
                    );
                    return Ok(());
                }
            },
        };

        let mut cmd = self.build_command(&program, src);
        run(
            &mut cmd,
            &format!("{} (patchperl)", program.to_string_lossy()),
        )
    }
}

/// Unpack `tarball` into `dest`, returning the path of the single top-level
/// directory it contains.
///
/// Compression is chosen from the file extension (`.gz`/`.tgz`, `.bz2`/`.tbz`,
/// `.xz`/`.txz`, or none). The `tar` program is used (`gtar` on illumos /
/// Solaris); override it with the `PERL_BUILD_TAR` environment variable.
pub fn extract_tarball(tarball: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<PathBuf> {
    let tarball = tarball.as_ref();
    let dest = dest.as_ref();
    std::fs::create_dir_all(dest)?;

    let tar = tar_program();
    let compression = compression_flag(tarball);

    // List first, to find (and sanity-check) the top-level directory.
    let mut list = Command::new(&tar);
    if let Some(flag) = compression {
        list.arg(flag);
    }
    list.arg("-tf").arg(tarball);
    let label = format!("{} -tf {}", tar.to_string_lossy(), tarball.display());
    let output = list.output().map_err(|e| Error::Spawn(label.clone(), e))?;
    if !output.status.success() {
        return Err(Error::Command {
            command: label,
            status: output.status,
        });
    }

    let mut roots = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let entry = line.trim().trim_start_matches("./");
        if entry.is_empty() {
            continue;
        }
        if let Some(root) = entry.split('/').next().filter(|s| !s.is_empty()) {
            roots.insert(root.to_owned());
        }
    }
    let mut roots = roots.into_iter();
    let (Some(root), None) = (roots.next(), roots.next()) else {
        return Err(Error::TarballLayout(tarball.to_path_buf()));
    };

    let mut extract = Command::new(&tar);
    if let Some(flag) = compression {
        extract.arg(flag);
    }
    extract.arg("-xf").arg(tarball).arg("-C").arg(dest);
    run(
        &mut extract,
        &format!(
            "{} -xf {} -C {}",
            tar.to_string_lossy(),
            tarball.display(),
            dest.display()
        ),
    )?;

    Ok(dest.join(root))
}

// -- free helpers ---------------------------------------------------------

/// Run `cmd`, inheriting stdio; map failure onto [`Error`].
fn run(cmd: &mut Command, label: &str) -> Result<()> {
    log::info!("running: {label}");
    let status = cmd
        .status()
        .map_err(|e| Error::Spawn(label.to_owned(), e))?;
    if !status.success() {
        return Err(Error::Command {
            command: label.to_owned(),
            status,
        });
    }
    Ok(())
}

async fn download(client: &Client, url: &str, dest: &Path) -> Result<()> {
    log::info!("downloading {url} -> {}", dest.display());
    let response = client.http().get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Download {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }
    let bytes = response.bytes().await?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, &*bytes)?;
    log::debug!("wrote {} bytes to {}", bytes.len(), dest.display());
    Ok(())
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn append_split(cmd: &mut Command, value: Option<String>) {
    if let Some(value) = value {
        for arg in value.split_whitespace() {
            cmd.arg(arg);
        }
    }
}

fn tar_program() -> OsString {
    if let Some(tar) = std::env::var_os("PERL_BUILD_TAR") {
        return tar;
    }
    if cfg!(any(target_os = "solaris", target_os = "illumos")) {
        OsString::from("gtar")
    } else {
        OsString::from("tar")
    }
}

fn compression_flag(tarball: &Path) -> Option<&'static str> {
    let name = tarball
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some("-z")
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz") || name.ends_with(".tbz2") {
        Some("-j")
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Some("-J")
    } else if name.ends_with(".tar") {
        None
    } else {
        // Unknown: assume gzip, which is what the CPAN Perl archives use.
        Some("-z")
    }
}

/// `true` for Perl >= 5.8 (which has `make test_harness`), read from
/// `patchlevel.h` in the source tree; defaults to `true` when it cannot be
/// determined.
fn uses_test_harness(src: &Path) -> bool {
    match perl_version_from_source(src) {
        Some((revision, version, _)) => (revision, version) >= (5, 8),
        None => true,
    }
}

fn perl_version_from_source(src: &Path) -> Option<(u32, u32, u32)> {
    let text = std::fs::read_to_string(src.join("patchlevel.h")).ok()?;
    Some((
        parse_define_u32(&text, "PERL_REVISION")?,
        parse_define_u32(&text, "PERL_VERSION")?,
        parse_define_u32(&text, "PERL_SUBVERSION")?,
    ))
}

fn parse_define_u32(text: &str, key: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("#define")?.trim_start();
        let rest = rest.strip_prefix(key)?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn url_filename(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').find(|s| !s.is_empty()) {
        Some(name) if name.contains('.') => name.to_owned(),
        _ => "perl-source.tar.gz".to_owned(),
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable(&candidate).then_some(candidate)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn temp_dir(prefix: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_flags_from_extension() {
        assert_eq!(
            compression_flag(Path::new("perl-5.38.2.tar.gz")),
            Some("-z")
        );
        assert_eq!(
            compression_flag(Path::new("perl-5.8.9.tar.bz2")),
            Some("-j")
        );
        assert_eq!(
            compression_flag(Path::new("perl-5.40.0.tar.xz")),
            Some("-J")
        );
        assert_eq!(compression_flag(Path::new("perl.tar")), None);
        assert_eq!(compression_flag(Path::new("weird-name")), Some("-z"));
    }

    #[test]
    fn resolved_options_add_prefix_and_scriptdir() {
        let pb = PerlBuild::new("/opt/perl");
        let opts = pb.resolved_configure_options(Path::new("/opt/perl"));
        assert_eq!(opts[0], "-Dprefix=/opt/perl");
        assert!(opts.iter().any(|o| o == "-de"));
        assert!(opts.iter().any(|o| o == "-Aeval:scriptdir=/opt/perl/bin"));
    }

    #[test]
    fn resolved_options_respect_caller_supplied_prefix_and_scriptdir() {
        let pb = PerlBuild::new("/opt/perl").configure_options([
            "-de",
            "-Dprefix=/elsewhere",
            "-Aeval:scriptdir=/custom",
        ]);
        let opts = pb.resolved_configure_options(Path::new("/opt/perl"));
        assert_eq!(opts.iter().filter(|o| o.starts_with("-Dprefix")).count(), 1);
        assert_eq!(opts[0], "-de");
        assert!(!opts.iter().any(|o| o == "-Aeval:scriptdir=/opt/perl/bin"));
    }

    #[test]
    fn parses_perl_version_defines() {
        let text =
            "#define PERL_REVISION\t5\n#define PERL_VERSION 38\n#define PERL_SUBVERSION  2\n";
        assert_eq!(parse_define_u32(text, "PERL_REVISION"), Some(5));
        assert_eq!(parse_define_u32(text, "PERL_VERSION"), Some(38));
        assert_eq!(parse_define_u32(text, "PERL_SUBVERSION"), Some(2));
        assert_eq!(parse_define_u32(text, "PERL_NOPE"), None);
    }

    #[test]
    fn url_filenames() {
        assert_eq!(
            url_filename("https://cpan.metacpan.org/authors/id/P/PE/PEVANS/perl-5.38.2.tar.gz"),
            "perl-5.38.2.tar.gz"
        );
        assert_eq!(
            url_filename("https://example.com/perl-5.40.0.tar.xz?mirror=1"),
            "perl-5.40.0.tar.xz"
        );
        assert_eq!(
            url_filename("https://example.com/pub/perl/"),
            "perl-source.tar.gz"
        );
    }
}
