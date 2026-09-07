//! Locating Perl source archives on CPAN through the MetaCPAN API.

use metacpan_api_modern::{Client, Release, SearchResponse};
use serde_json::json;

use crate::error::{Error, Result};

/// One Perl release, distilled from a MetaCPAN `release` document.
///
/// This is the Rust equivalent of what `CPAN::Perl::Releases` /
/// `CPAN::Perl::Releases::MetaCPAN` give `Perl::Build`: enough to fetch the
/// archive and know what it is.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PerlRelease {
    /// Release name including version, e.g. `perl-5.38.2`.
    pub name: String,
    /// Dotted version, e.g. `5.38.2` (derived from [`name`](Self::name)).
    pub version: String,
    /// PAUSE id of the release manager, e.g. `PEVANS`.
    pub author: String,
    /// Direct download URL on the CPAN CDN.
    pub download_url: String,
    /// Archive file name, e.g. `perl-5.38.2.tar.gz`.
    pub archive: Option<String>,
    /// SHA-256 checksum of the archive, when MetaCPAN has one.
    pub checksum_sha256: Option<String>,
    /// Upload timestamp (ISO 8601, no timezone).
    pub date: Option<String>,
    /// Maturity: `released` for a stable Perl, `developer` for a `5.x.y` odd
    /// development release.
    pub maturity: Option<String>,
    /// Index status: `latest`, `cpan`, or `backpan`.
    pub status: Option<String>,
}

impl PerlRelease {
    fn from_release(r: Release) -> Self {
        let name = r.name.clone().unwrap_or_default();
        let version = name
            .strip_prefix("perl-")
            .map(str::to_owned)
            .or_else(|| r.version.clone())
            .unwrap_or_default();
        Self {
            version,
            author: r.author.unwrap_or_default(),
            download_url: r.download_url.unwrap_or_default(),
            archive: r.archive,
            checksum_sha256: r.checksum_sha256,
            date: r.date,
            maturity: r.maturity,
            status: r.status,
            name,
        }
    }

    /// Whether this is an odd-numbered development release rather than a stable
    /// production Perl.
    pub fn is_developer(&self) -> bool {
        self.maturity.as_deref() == Some("developer")
    }
}

/// A lookup for Perl source archives, backed by MetaCPAN.
///
/// ```no_run
/// # async fn run() -> Result<(), perl_build::Error> {
/// use perl_build::PerlReleases;
///
/// let releases = PerlReleases::new();
/// let r = releases.find("5.38.2").await?;
/// println!("{} -> {}", r.name, r.download_url);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PerlReleases {
    client: Client,
}

impl Default for PerlReleases {
    fn default() -> Self {
        Self::new()
    }
}

impl PerlReleases {
    /// Use a fresh client pointed at the public MetaCPAN API.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Use a caller-supplied [`Client`] (custom cache directory, `User-Agent`,
    /// proxy, alternate host, ...).
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// The underlying MetaCPAN client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Resolve a Perl version to its release.
    ///
    /// `version` is the dotted form as it appears in the release name, with or
    /// without the `perl-` prefix: `"5.38.2"`, `"5.8.9"`, `"perl-5.40.0"`. The
    /// MetaCPAN numified form (`"5.038002"`) is also accepted as a fallback.
    pub async fn find(&self, version: &str) -> Result<PerlRelease> {
        let wanted_name = normalize_release_name(version);
        let query = json!({
            "query": { "bool": {
                "must": [ { "term": { "distribution": "perl" } } ],
                "should": [
                    { "term": { "name": wanted_name } },
                    { "term": { "version": version.trim() } },
                ],
                "minimum_should_match": 1,
            }},
            "size": 5,
            "sort": [ { "date": "desc" } ],
        });

        let resp: SearchResponse<Release> = self.client.search("release", &query).await?;
        let mut hits: Vec<PerlRelease> = resp
            .into_sources()
            .into_iter()
            .map(PerlRelease::from_release)
            .collect();

        if hits.is_empty() {
            return Err(Error::ReleaseNotFound(version.to_owned()));
        }
        // Prefer an exact release-name match; otherwise take the newest hit.
        if let Some(pos) = hits.iter().position(|r| r.name == wanted_name) {
            return Ok(hits.swap_remove(pos));
        }
        Ok(hits.swap_remove(0))
    }

    /// The most recent stable Perl release.
    pub async fn latest(&self) -> Result<PerlRelease> {
        let release = self.client.release("perl").await?;
        Ok(PerlRelease::from_release(release))
    }

    /// Every stable Perl release, newest first.
    pub async fn list(&self) -> Result<Vec<PerlRelease>> {
        self.list_inner(false).await
    }

    /// Every Perl release including odd-numbered development releases, newest
    /// first.
    pub async fn list_with_dev(&self) -> Result<Vec<PerlRelease>> {
        self.list_inner(true).await
    }

    async fn list_inner(&self, include_dev: bool) -> Result<Vec<PerlRelease>> {
        let query = json!({
            "query": { "term": { "distribution": "perl" } },
            "size": 1000,
            "sort": [ { "date": "desc" } ],
        });
        let resp: SearchResponse<Release> = self.client.search("release", &query).await?;
        Ok(resp
            .into_sources()
            .into_iter()
            .map(PerlRelease::from_release)
            .filter(|r| include_dev || !r.is_developer())
            .collect())
    }
}

fn normalize_release_name(version: &str) -> String {
    let v = version.trim();
    if v.starts_with("perl-") {
        v.to_owned()
    } else {
        format!("perl-{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_release_names() {
        assert_eq!(normalize_release_name("5.38.2"), "perl-5.38.2");
        assert_eq!(normalize_release_name("  5.8.9 "), "perl-5.8.9");
        assert_eq!(normalize_release_name("perl-5.40.0"), "perl-5.40.0");
    }
}
