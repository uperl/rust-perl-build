//! [`PerlBuild`]: configure a build and run it.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use metacpan_api_modern::Client;

use crate::built::{Built, symlink_devel_executables};
use crate::error::{Error, Result};
use crate::releases::PerlReleases;

/// How to apply [`Devel::PatchPerl`](https://metacpan.org/pod/Devel::PatchPerl)
/// source fix-ups before `Configure`.
///
/// The fix-ups can be applied by the external `patchperl` program (from
/// `App::patchperl` on CPAN) or in-process by the [`patch-perl`](patch_perl)
/// crate — a Rust port of the `Devel::PatchPerl` library. See the
/// [crate docs](crate#patchperl).
#[derive(Debug, Clone, Default)]
pub enum PatchPerl {
    /// Use the external `patchperl` when it is on `PATH`; otherwise fall back to
    /// the in-process [`patch-perl`](patch_perl) port. This is the default, and
    /// always applies the fix-ups one way or another.
    #[default]
    Auto,
    /// Always use the external `patchperl` on `PATH`. If it cannot be run, warn
    /// and build without the fix-ups (older Perls may then fail to build).
    External,
    /// Always run this specific external program (a name resolved via `PATH`, or
    /// a full path).
    Command(OsString),
    /// Always apply the fix-ups in-process with the [`patch-perl`](patch_perl)
    /// crate, with no external program.
    Internal,
    /// Never patch the source tree.
    Disabled,
}

/// Which toolchain compiles the unpacked Perl source tree.
///
/// `Perl::Build` only ever runs the Unix `sh Configure` / `make` path. This
/// crate adds a Windows path that builds `win32\Makefile` with `nmake` and
/// Microsoft Visual C++, and makes it the default when running on Windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Toolchain {
    /// [`Msvc`](Self::Msvc) on Windows, [`GnuConfigure`](Self::GnuConfigure)
    /// everywhere else. The default.
    #[default]
    Auto,
    /// Unix-style: `sh Configure -de ...`, then `make`, `make test_harness`,
    /// `make install`.
    GnuConfigure,
    /// Windows: build `win32\Makefile` from the source tree with `nmake` and
    /// Microsoft Visual C++, then `nmake test` / `nmake install`.
    ///
    /// `cl.exe` must be on `PATH`, so run from a Visual Studio "x64 Native
    /// Tools" command prompt or a shell that has sourced `vcvarsall.bat`. The
    /// install prefix is passed as the `INST_TOP` / `INST_DRV` macros (the
    /// Makefile is not edited), and `CCTYPE` — which that Makefile requires and
    /// does not default — is detected by running `cl`, unless it is already
    /// given in `PERL_BUILD_COMPILE_OPTIONS`. Override the make program with the
    /// `PERL_BUILD_NMAKE` environment variable.
    Msvc,
}

impl Toolchain {
    /// Resolve [`Auto`](Self::Auto) to the concrete toolchain for this platform.
    fn resolved(self) -> Toolchain {
        match self {
            Toolchain::Auto if cfg!(windows) => Toolchain::Msvc,
            Toolchain::Auto => Toolchain::GnuConfigure,
            explicit => explicit,
        }
    }
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
    toolchain: Toolchain,
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
            toolchain: Toolchain::Auto,
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

    /// How to apply the [`Devel::PatchPerl`](patch_perl) fix-ups — the external
    /// `patchperl` program, the in-process [`patch-perl`](patch_perl) crate, or
    /// not at all. Default: [`PatchPerl::Auto`].
    pub fn patchperl(mut self, patchperl: PatchPerl) -> Self {
        self.patchperl = patchperl;
        self
    }

    /// Choose the build toolchain. Default: [`Toolchain::Auto`] —
    /// [`Toolchain::Msvc`] on Windows, [`Toolchain::GnuConfigure`] elsewhere.
    pub fn toolchain(mut self, toolchain: Toolchain) -> Self {
        self.toolchain = toolchain;
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

    /// Build from an already-unpacked source tree. Equivalent to
    /// `Perl::Build->install`; needs no network or async runtime.
    ///
    /// The steps depend on [`toolchain`](Self::toolchain): `sh Configure` /
    /// `make` / `make install` for [`Toolchain::GnuConfigure`], or `nmake` from
    /// the source tree's `win32\` directory for [`Toolchain::Msvc`] (the
    /// default on Windows).
    pub fn install_from_source(&self, src_path: impl AsRef<Path>) -> Result<Built> {
        let src = src_path.as_ref();
        let dst = absolute(&self.dst_path)?;
        let toolchain = self.toolchain.resolved();
        log::info!(
            "building perl from {} into {} with {toolchain:?}",
            src.display(),
            dst.display()
        );

        self.run_patchperl(src)?;

        match toolchain {
            Toolchain::Msvc => self.build_msvc(src, &dst)?,
            _ => self.build_gnu_configure(src, &dst)?,
        }

        let built = Built::new(dst);

        if toolchain != Toolchain::Msvc
            && self
                .configure_options
                .iter()
                .any(|o| o.contains("usedevel"))
        {
            log::info!("-Dusedevel build: linking versioned executables");
            symlink_devel_executables(&built.bin_dir())?;
        }

        log::info!("installed perl in {}", built.prefix().display());
        Ok(built)
    }

    /// The Unix build: `sh Configure` then `make` / `make test` / `make
    /// install`.
    fn build_gnu_configure(&self, src: &Path, dst: &Path) -> Result<()> {
        let configure_options = self.resolved_configure_options(dst);

        // A stale config from an earlier, aborted build confuses Configure.
        for stale in ["config.sh", "Policy.sh"] {
            let path = src.join(stale);
            if path.exists() {
                log::debug!("removing stale {}", path.display());
                let _ = std::fs::remove_file(&path);
            }
        }

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

        Ok(())
    }

    /// The Windows build: `win32\Makefile` driven by `nmake` and Visual C++.
    ///
    /// `INST_TOP` / `INST_DRV` are passed as command-line macros so the source
    /// tree's `win32\Makefile` does not have to be edited. `CCTYPE` has no
    /// default in that Makefile and it refuses to build without one, so it is
    /// detected by running `cl` unless the caller has already set it through
    /// `PERL_BUILD_COMPILE_OPTIONS`.
    fn build_msvc(&self, src: &Path, dst: &Path) -> Result<()> {
        let win32 = src.join("win32");
        if !win32.join("Makefile").is_file() {
            return Err(Error::Other(format!(
                "no win32\\Makefile under {} — an MSVC build needs a Windows \
                 Perl source tree",
                src.display()
            )));
        }

        let nmake = nmake_program();
        let nmake_label = nmake.to_string_lossy().into_owned();

        let mut macros = msvc_install_macros(dst);
        let compile_options = env_opt("PERL_BUILD_COMPILE_OPTIONS");
        let caller_set_cctype = compile_options
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .any(|w| w.starts_with("CCTYPE="));
        if !caller_set_cctype {
            match detect_msvc_cctype() {
                Some(cctype) => {
                    log::info!("detected Visual C++: CCTYPE={cctype}");
                    macros.push(format!("CCTYPE={cctype}"));
                }
                None => log::warn!(
                    "could not detect the Visual C++ version by running `cl`; \
                     win32\\Makefile requires CCTYPE. Run this from a Visual \
                     Studio \"x64 Native Tools\" command prompt, or set it via \
                     `PERL_BUILD_COMPILE_OPTIONS=CCTYPE=MSVC143` (or similar)."
                ),
            }
        }

        // nmake
        let mut make = self.build_command(&nmake, &win32);
        make.args(&macros);
        append_split(&mut make, compile_options);
        run(&mut make, &format!("{nmake_label} {}", macros.join(" ")))?;

        // nmake test
        if self.test {
            let mut test = self.build_command(&nmake, &win32);
            test.args(&macros).arg("test");
            if let Some(jobs) = self.jobs {
                test.env("TEST_JOBS", jobs.to_string());
                test.env("HARNESS_OPTIONS", format!("j{jobs}"));
            }
            run(&mut test, &format!("{nmake_label} test"))?;
        }

        // nmake install
        let mut install = self.build_command(&nmake, &win32);
        install.args(&macros).arg("install");
        append_split(&mut install, env_opt("PERL_BUILD_INSTALL_OPTIONS"));
        run(&mut install, &format!("{nmake_label} install"))?;

        Ok(())
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
        // Perl's win32\Makefile invokes `miniperl` / `perl` bare from the source
        // tree, relying on the legacy behaviour where the current directory is
        // searched for executables. When `NoDefaultCurrentDirectoryInExePath` is
        // set in the environment that lookup is disabled and the build dies with
        // "'miniperl' is not recognized". Restore it for the build subprocess
        // tree.
        #[cfg(windows)]
        cmd.env_remove("NoDefaultCurrentDirectoryInExePath");
        cmd
    }

    fn run_patchperl(&self, src: &Path) -> Result<()> {
        let program: OsString = match &self.patchperl {
            PatchPerl::Disabled => {
                log::info!("patchperl: disabled");
                return Ok(());
            }
            PatchPerl::Internal => return run_patchperl_internal(src),
            PatchPerl::Command(program) => program.clone(),
            PatchPerl::External => match which("patchperl") {
                Some(_) => OsString::from("patchperl"),
                None => {
                    log::warn!(
                        "`patchperl` not found on PATH; building without Devel::PatchPerl \
                         fix-ups (older Perls may fail to build). Install it with \
                         `cpanm App::patchperl`, use PatchPerl::Internal for the in-process \
                         port, or pass PatchPerl::Disabled to silence this."
                    );
                    return Ok(());
                }
            },
            PatchPerl::Auto => match which("patchperl") {
                Some(_) => OsString::from("patchperl"),
                None => {
                    log::info!(
                        "`patchperl` not found on PATH; applying Devel::PatchPerl fix-ups \
                         in-process with the patch-perl crate"
                    );
                    return run_patchperl_internal(src);
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

/// Apply the standard Devel::PatchPerl fix-ups to `src` in-process with the
/// [`patch-perl`](patch_perl) crate. The Perl version is read from the tree's
/// `patchlevel.h`.
///
/// The `PERL5_PATCHPERL_PLUGIN` hook is **not** honoured here: `patch-perl`
/// resolves it to a native shared library rather than the Perl module the
/// external `patchperl` expects, so silently acting on it would break a build
/// that has a Perl-module plugin configured. Use the external `patchperl`
/// ([`PatchPerl::External`] / [`PatchPerl::Command`]) if you need plugins.
fn run_patchperl_internal(src: &Path) -> Result<()> {
    log::info!(
        "applying Devel::PatchPerl fix-ups in-process (patch-perl crate) in {}",
        src.display()
    );
    patch_perl::PatchPerl::new()
        .source(src)
        .run_plugins(false)
        .run()?;
    Ok(())
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

/// The make program for the Windows / Visual C++ build. `nmake` (which ships
/// with Visual C++) unless overridden with `PERL_BUILD_NMAKE`.
fn nmake_program() -> OsString {
    std::env::var_os("PERL_BUILD_NMAKE").unwrap_or_else(|| OsString::from("nmake"))
}

/// `INST_TOP=` (always) and `INST_DRV=` (when a drive letter can be recovered)
/// macros that repoint `win32\Makefile` from its built-in `C:\perl` to `dst`.
fn msvc_install_macros(dst: &Path) -> Vec<String> {
    let inst_top = dst.to_string_lossy().replace('/', "\\");
    let mut macros = vec![format!("INST_TOP={inst_top}")];
    if let Some(drive) = windows_drive(dst) {
        macros.push(format!("INST_DRV={drive}"));
    }
    macros
}

/// The upper-cased `C:`-style drive prefix of `path`, on platforms that parse
/// one (i.e. Windows); `None` otherwise.
fn windows_drive(path: &Path) -> Option<String> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                Some(format!("{}:", (d as char).to_ascii_uppercase()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Best-effort `CCTYPE` for `win32\Makefile`, which has no compiler
/// auto-detection of its own. Runs `cl` and maps its version banner to the
/// `MSVCnnn` token Perl expects; `None` if `cl` will not run or its banner
/// cannot be parsed.
fn detect_msvc_cctype() -> Option<String> {
    // `cl` with no arguments writes its banner to stderr and exits non-zero.
    let out = Command::new("cl").output().ok()?;
    let banner = String::from_utf8_lossy(&out.stderr);
    cctype_for_msc_ver(parse_cl_msc_ver(&banner)?)
}

/// Pull the `_MSC_VER`-style number out of a `cl` banner: the `19.44` in
/// `... Optimizing Compiler Version 19.44.35207.1 for x64` becomes `1944`.
/// Scans for the first `MAJOR.MINOR.BUILD` token with a plausible major version
/// rather than matching surrounding words, so it survives localized banners.
fn parse_cl_msc_ver(banner: &str) -> Option<u32> {
    banner.split_whitespace().find_map(|token| {
        let mut parts = token.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let minor: u32 = parts.next()?.parse().ok()?;
        (parts.next().is_some() && (19..=40).contains(&major)).then_some(major * 100 + minor)
    })
}

/// Map an `_MSC_VER` number to Perl's `CCTYPE`. Visual C++ 2022's whole
/// 14.3x–14.4x line is `MSVC143`; 14.5+ (VS 2026) is `MSVC145`.
fn cctype_for_msc_ver(msc_ver: u32) -> Option<String> {
    let name = match msc_ver {
        1900..=1909 => "MSVC140",
        1910..=1919 => "MSVC141",
        1920..=1929 => "MSVC142",
        1930..=1949 => "MSVC143",
        v if v >= 1950 => "MSVC145",
        _ => return None,
    };
    Some(name.to_owned())
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
    fn toolchain_auto_picks_platform_default() {
        assert_eq!(Toolchain::Msvc.resolved(), Toolchain::Msvc);
        assert_eq!(Toolchain::GnuConfigure.resolved(), Toolchain::GnuConfigure);
        let expected = if cfg!(windows) {
            Toolchain::Msvc
        } else {
            Toolchain::GnuConfigure
        };
        assert_eq!(Toolchain::Auto.resolved(), expected);
    }

    #[test]
    fn msvc_macros_point_at_prefix() {
        let macros = msvc_install_macros(Path::new(r"C:\opt\perl-5.40"));
        assert_eq!(macros[0], r"INST_TOP=C:\opt\perl-5.40");
        // The drive is only recoverable where the platform parses a prefix.
        if cfg!(windows) {
            assert!(macros.iter().any(|m| m == "INST_DRV=C:"));
        }
    }

    #[test]
    fn msvc_macros_normalise_forward_slashes() {
        let macros = msvc_install_macros(Path::new("Z:/perls/dev"));
        assert_eq!(macros[0], r"INST_TOP=Z:\perls\dev");
    }

    #[test]
    fn parses_cl_version_banner() {
        assert_eq!(
            parse_cl_msc_ver(
                "Microsoft (R) C/C++ Optimizing Compiler Version 19.44.35207.1 for x64"
            ),
            Some(1944)
        );
        assert_eq!(
            parse_cl_msc_ver("... Compiler Version 19.00.24245.1 for x86"),
            Some(1900)
        );
        assert_eq!(parse_cl_msc_ver("no version here"), None);
    }

    #[test]
    fn maps_msc_ver_to_cctype() {
        assert_eq!(cctype_for_msc_ver(1900).as_deref(), Some("MSVC140"));
        assert_eq!(cctype_for_msc_ver(1916).as_deref(), Some("MSVC141"));
        assert_eq!(cctype_for_msc_ver(1929).as_deref(), Some("MSVC142"));
        assert_eq!(cctype_for_msc_ver(1944).as_deref(), Some("MSVC143"));
        assert_eq!(cctype_for_msc_ver(1952).as_deref(), Some("MSVC145"));
        assert_eq!(cctype_for_msc_ver(1800), None);
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

    #[test]
    fn patchperl_default_is_auto() {
        assert!(matches!(PatchPerl::default(), PatchPerl::Auto));
    }

    /// A stand-in Perl source tree: just a `patchlevel.h` declaring `5.V.S`.
    fn fake_source_tree(tag: &str, version: u32, subversion: u32) -> PathBuf {
        let dir = temp_dir(&format!("perl-build-patchperl-{tag}")).unwrap();
        std::fs::write(
            dir.join("patchlevel.h"),
            format!(
                "#define PERL_REVISION 5\n#define PERL_VERSION {version}\n\
                 #define PERL_SUBVERSION {subversion}\n"
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn internal_patchperl_applies_cleanly_to_a_modern_tree() {
        // 5.40 is past Devel::PatchPerl's CERTIFIED cutoff, so the in-process
        // port has no fix-ups to apply and must still succeed.
        let src = fake_source_tree("modern", 40, 0);
        run_patchperl_internal(&src).expect("in-process patchperl on a modern tree");
        assert!(src.join("patchlevel.h").is_file());
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn run_patchperl_routes_disabled_and_internal() {
        // Disabled: a no-op, no source tree needed.
        PerlBuild::new("/unused")
            .patchperl(PatchPerl::Disabled)
            .run_patchperl(Path::new("/definitely/not/a/tree"))
            .expect("Disabled is a no-op");

        // Internal: routes through the patch-perl crate.
        let src = fake_source_tree("route", 40, 0);
        PerlBuild::new("/unused")
            .patchperl(PatchPerl::Internal)
            .run_patchperl(&src)
            .expect("Internal routes to the patch-perl crate");
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn internal_patchperl_reports_a_non_source_tree_as_patchperl_error() {
        let dir = temp_dir("perl-build-patchperl-empty").unwrap();
        match run_patchperl_internal(&dir) {
            Err(Error::PatchPerl(_)) => {}
            other => panic!("expected Error::PatchPerl, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
