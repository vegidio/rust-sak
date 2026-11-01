//! What an install reports, and one dependency's fraction done.

use std::borrow::Cow;

/// What [`install`](super::install) reports while it works.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// How far `dependency` has got, from 0 to 1. The download fills 0 to 0.8 and the extraction 0.8 to 1. It never
    /// decreases, and is exactly 1 only once the dependency's marker is written.
    Progress {
        /// The dependency's name, as the caller gave it.
        dependency: Cow<'static, str>,
        /// The step `bytes` and `total` count.
        phase: Phase,
        /// Bytes done in this phase: on disk while downloading (a resumed partial included), written while
        /// extracting.
        bytes: u64,
        /// The phase's total: the archive's size while downloading, the archive's declared contents while
        /// extracting (0 when it declares none).
        total: u64,
        /// From 0 to 1.
        fraction: f64,
    },
    /// A complete download of `dependency`, found in staging, is being hashed.
    Verifying {
        /// The dependency's name, as the caller gave it.
        dependency: Cow<'static, str>,
    },
    /// `dependency` is installed and intact, so nothing is done for it. Reported once for each such dependency,
    /// including one another process finished installing while this one waited for it.
    Current {
        /// The dependency's name, as the caller gave it.
        dependency: Cow<'static, str>,
    },
}

/// The step of an install a [`Event::Progress`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The archive is downloading.
    Download,
    /// The archive is extracting.
    Extract,
}

/// The share of an archive's fraction its download fills, the extraction filling the rest.
const DOWNLOAD_SHARE: f64 = 0.8;

/// The most a fraction reads before [`Fraction::finish`]: just below 1, so 1 means installed.
const UNFINISHED: f64 = 1.0_f64.next_down();

/// One dependency's fraction, which never decreases, so a resumed transfer that corrects its start, or a staged archive
/// downloaded again, holds it where it was. It is 1 only at [`finish`](Self::finish). Each method returns the fraction
/// to report.
#[derive(Debug, Default)]
pub(super) struct Fraction {
    high: f64,
}

impl Fraction {
    /// `done` of the download's `total` bytes are on disk.
    pub fn download(&mut self, done: u64, total: u64) -> f64 {
        self.report(DOWNLOAD_SHARE * ratio(done, total))
    }

    /// `done` of `total` bytes are on disk, for an install with no extraction, whose download fills it all.
    pub fn whole(&mut self, done: u64, total: u64) -> f64 {
        self.report(ratio(done, total))
    }

    /// `done` of the extraction's `total` bytes are written.
    pub fn extract(&mut self, done: u64, total: u64) -> f64 {
        self.report(DOWNLOAD_SHARE + (1.0 - DOWNLOAD_SHARE) * ratio(done, total))
    }

    /// The dependency is installed.
    pub fn finish(&mut self) -> f64 {
        self.high = 1.0;
        self.high
    }

    fn report(&mut self, fraction: f64) -> f64 {
        self.high = self.high.max(fraction.min(UNFINISHED));
        self.high
    }
}

#[allow(clippy::cast_precision_loss, reason = "a byte count far below 2^52")]
fn ratio(done: u64, total: u64) -> f64 {
    if total == 0 {
        1.0
    } else {
        (done as f64 / total as f64).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        assert!(
            actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < 1e-9),
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn the_download_fills_the_first_80_percent_and_the_extraction_the_rest() {
        let mut fraction = Fraction::default();
        let reported = [
            fraction.download(0, 100),
            fraction.download(50, 100),
            fraction.download(100, 100),
            fraction.extract(50, 200),
        ];
        assert_near(&reported, &[0.0, 0.4, 0.8, 0.85]);
        let almost = fraction.extract(200, 200);
        assert!(almost < 1.0 && almost > 0.999, "1 only at the finish: {almost}");
        assert_eq!(fraction.finish(), 1.0);
    }

    #[test]
    fn the_fraction_never_decreases_after_a_restart() {
        let mut fraction = Fraction::default();
        let reported = [
            fraction.download(60, 100),
            fraction.download(0, 100),
            fraction.download(30, 100),
            fraction.download(80, 100),
        ];
        assert_near(&reported, &[0.48, 0.48, 0.48, 0.64]);
    }
}
