# perl-build

Build new Perls from source — a Rust port of the
[`Perl::Build`](https://metacpan.org/pod/Perl::Build) CPAN distribution.

`perl-build` downloads a Perl source tarball (from CPAN, an arbitrary URL, or
the local filesystem), unpacks it, runs `Configure` / `make` / `make install`
(or `nmake` against `win32\Makefile` on Windows — see [Windows](#windows) below),
and leaves you a ready-to-use Perl under a prefix you choose. Perl releases are
located through
[`metacpan-api-modern`](https://github.com/uperl/rust-metacpan-api-modern), so a
version like `5.38.2` resolves to the right archive on the CPAN CDN for you.

## Interface

[`PerlBuild`] is configured with builder methods and finished with one of:

| method | `Perl::Build` analogue | needs network / async |
| --- | --- | --- |
| `PerlBuild::install_from_cpan` | `install_from_cpan` | yes |
| `PerlBuild::install_from_url` | `install_from_url` | yes |
| `PerlBuild::install_from_tarball` | `install_from_tarball` | no |
| `PerlBuild::install_from_source` | `install` | no |

Each returns a `Built` (the equivalent of `Perl::Build::Built`), which knows the
installation prefix and the environment changes needed to run the new Perl.

`PerlReleases` exposes just the "which archive provides this Perl version"
lookup.

## Examples

Build Perl 5.38.2 straight from CPAN:

```rust,no_run
use perl_build::PerlBuild;

# async fn run() -> Result<(), perl_build::Error> {
let built = PerlBuild::new("/opt/perl-5.38.2")
    .jobs(4)
    .test(false)
    .install_from_cpan("5.38.2")
    .await?;

println!("installed {}", built.perl().display());
# Ok(())
# }
```

Build from a tarball already on disk — no async runtime required:

```rust,no_run
use perl_build::PerlBuild;

# fn run() -> Result<(), perl_build::Error> {
let built = PerlBuild::new("/opt/perl-5.38.2")
    .install_from_tarball("/downloads/perl-5.38.2.tar.gz")?;
# let _ = built;
# Ok(())
# }
```

Run something against the built Perl:

```rust,no_run
use std::process::Command;
# fn run(built: perl_build::Built) -> Result<(), perl_build::Error> {
let mut cmd = Command::new(built.perl());
cmd.arg("-V");
built.apply_env(&mut cmd); // prepends <prefix>/bin to PATH, clears PERL5LIB/PERL5OPT/...
cmd.status()?;
# Ok(())
# }
```

## `patchperl`

`Perl::Build` applies
[`Devel::PatchPerl`](https://metacpan.org/pod/Devel::PatchPerl) source fix-ups
before `Configure`; without them, older Perls frequently fail to build on a
modern toolchain. This crate can apply them two ways, chosen with
`PerlBuild::patchperl` / the `PatchPerl` enum:

* the **external `patchperl` program** (`cpanm App::patchperl`);
  `PERL5_PATCHPERL_PLUGIN` is inherited by the child, so Perl-module patch
  plugins work as they do for `Perl::Build`;
* the **in-process
  [`patch-perl`](https://github.com/uperl/patch-perl) crate**, a Rust port of
  the `Devel::PatchPerl` library — no external program. `PERL5_PATCHPERL_PLUGIN`
  is ignored on this path (the crate resolves it to a native shared library, not
  a Perl module); use the external `patchperl` if you need patch plugins.

| `PatchPerl` | behaviour |
|---|---|
| `Auto` (default) | external `patchperl` if on `PATH`, otherwise the in-process port — the fix-ups are always applied |
| `External` | external `patchperl` only; warn and build without the fix-ups if it is missing |
| `Command(path)` | a specific external program |
| `Internal` | the in-process `patch-perl` crate |
| `Disabled` | no fix-ups |

## Windows

On Windows the default toolchain builds `win32\Makefile` from the source tree
with `nmake` and Microsoft Visual C++ (`nmake` / `nmake test` / `nmake
install`), following
[`README.win32`](https://github.com/Perl/perl5/blob/blead/README.win32).
`cl.exe` must be on `PATH`, so run from a Visual Studio "x64 Native Tools"
command prompt, or from a shell that has sourced `vcvarsall.bat`.

* The install prefix is passed to the Makefile as the `INST_TOP` / `INST_DRV`
  macros — the source tree is not edited.
* `win32\Makefile` requires `CCTYPE` and does not detect the compiler itself, so
  `perl-build` runs `cl` and maps its version to `MSVC140`…`MSVC145`. Set
  `CCTYPE=...` in `PERL_BUILD_COMPILE_OPTIONS` to override.
* `PerlBuild::toolchain(Toolchain::GnuConfigure)` forces the Unix `sh Configure`
  path instead (and `Toolchain::Msvc` forces the MSVC path off Windows).

## Environment variables

Honoured for parity with `Perl::Build`:

| variable | effect |
| --- | --- |
| `PERL_BUILD_COMPILE_OPTIONS` | extra arguments appended to `make` (or `nmake`; a `CCTYPE=...` here disables auto-detection) |
| `PERL_BUILD_INSTALL_OPTIONS` | extra arguments appended to `make install` |
| `PERL5_PATCHPERL_PLUGIN` | Devel::PatchPerl plugin — a Perl module for the external `patchperl`, a native shared library for the in-process port |
| `PERL_BUILD_TAR` | `tar` program to use for unpacking (default `tar`, `gtar` on illumos/Solaris) |
| `PERL_BUILD_NMAKE` | make program for the Windows / Visual C++ build (default `nmake`) |

`PERL5LIB` and `PERL5OPT` are removed from the environment of every build
command, so an active perlbrew / local::lib does not perturb the build.

## Logging

Progress is reported through the [`log`](https://docs.rs/log) facade at `info`
level (the command being run) and `debug` (details). Install any logger to see
it; child process output (`Configure`, `make`, …) goes to inherited stdio
regardless.

## Tests

`cargo test` runs offline. Network and build tests are `#[ignore]`d:

```text
cargo test --test live      -- --ignored   # MetaCPAN resolution
cargo test --test build_e2e -- --ignored   # full build (needs a C toolchain, slow)
```

## License

MIT
