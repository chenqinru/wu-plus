use std::{fmt, str::FromStr};

use semver::Version;

/// The upstream Wu version and the Wu Plus revision used by GitHub Releases.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WuPlusVersion {
    upstream: [u64; 3],
    revision: u64,
}

impl WuPlusVersion {
    /// Creates a release version. Revision zero represents a legacy three-part release.
    pub const fn new(major: u64, minor: u64, patch: u64, revision: u64) -> Self {
        Self {
            upstream: [major, minor, patch],
            revision,
        }
    }
}

impl From<&Version> for WuPlusVersion {
    fn from(version: &Version) -> Self {
        let mut fields = version.build.as_str().split('.');
        let revision = match (fields.next(), fields.next(), fields.next()) {
            (Some("stable" | "dev"), Some("wu-plus"), Some(revision)) => {
                revision.parse().unwrap_or_default()
            }
            _ => 0,
        };
        Self::new(version.major, version.minor, version.patch, revision)
    }
}

impl fmt::Display for WuPlusVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.upstream;
        write!(formatter, "{major}.{minor}.{patch}")?;
        if self.revision != 0 {
            write!(formatter, ".{}", self.revision)?;
        }
        Ok(())
    }
}

/// An invalid Wu Plus release version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidWuPlusVersion;

impl fmt::Display for InvalidWuPlusVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected an upstream version and optional positive Wu Plus revision")
    }
}

impl std::error::Error for InvalidWuPlusVersion {}

impl FromStr for WuPlusVersion {
    type Err = InvalidWuPlusVersion;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (upstream, revision) = match value.rsplit_once('.') {
            Some((upstream, revision)) if upstream.matches('.').count() == 2 => {
                if revision.starts_with('0') || !revision.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(InvalidWuPlusVersion);
                }
                let revision = revision.parse().map_err(|_| InvalidWuPlusVersion)?;
                (upstream, revision)
            }
            _ => (value, 0),
        };
        let version = Version::parse(upstream).map_err(|_| InvalidWuPlusVersion)?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(InvalidWuPlusVersion);
        }
        Ok(Self::new(
            version.major,
            version.minor,
            version.patch,
            revision,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_revisions_numerically_and_legacy_releases_as_revision_zero() {
        let versions = ["1.5.0", "1.5.0.1", "1.5.0.9", "1.5.0.10", "1.5.1.1"]
            .map(|version| version.parse::<WuPlusVersion>().unwrap());
        assert!(versions.is_sorted());
        assert_eq!(versions[3].to_string(), "1.5.0.10");
        assert_eq!(versions[0].to_string(), "1.5.0");
    }

    #[test]
    fn rejects_invalid_releases() {
        for version in [
            "1.5",
            "1.5.0.0",
            "1.5.0.01",
            "1.5.0.+1",
            "1.5.0.1.2",
            "1.5.0+old",
        ] {
            assert!(version.parse::<WuPlusVersion>().is_err(), "{version}");
        }
    }

    #[test]
    fn extracts_release_revision_without_using_build_number_or_commit() {
        let bundled = Version::parse("1.5.0+stable.wu-plus.10.452.abcdef").unwrap();
        let legacy = Version::parse("1.5.0+stable.452.abcdef").unwrap();
        assert_eq!(WuPlusVersion::from(&bundled).to_string(), "1.5.0.10");
        assert_eq!(WuPlusVersion::from(&legacy).to_string(), "1.5.0");
    }
}
