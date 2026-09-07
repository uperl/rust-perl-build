# perl-build

Build new Perls from source — a Rust port of the
[`Perl::Build`](https://metacpan.org/pod/Perl::Build) CPAN distribution.

`perl-build` downloads a Perl source tarball (from CPAN, an arbitrary URL, or
the local filesystem), unpacks it, runs `Configure` / `make` / `make install`,
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
modern toolchain. There is no Rust reimplementation of that logic, so this crate
shells out to the `patchperl` program. By default (`PatchPerl::Auto`) it is used
when found on `PATH` and skipped with a warning otherwise. Install it with
`cpanm App::patchperl`, or point at a specific one with `PerlBuild::patchperl`.
`PERL5_PATCHPERL_PLUGIN` is inherited by the `patchperl` child process.

## Environment variables

Honoured for parity with `Perl::Build`:

| variable | effect |
| --- | --- |
| `PERL_BUILD_COMPILE_OPTIONS` | extra arguments appended to `make` |
| `PERL_BUILD_INSTALL_OPTIONS` | extra arguments appended to `make install` |
| `PERL5_PATCHPERL_PLUGIN` | read by `patchperl` (Devel::PatchPerl plugin) |
| `PERL_BUILD_TAR` | `tar` program to use for unpacking (default `tar`, `gtar` on illumos/Solaris) |

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
