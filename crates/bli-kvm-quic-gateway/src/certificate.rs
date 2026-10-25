//! The endpoint's certificate: made in memory, short-lived, known to the page
//! by its hash (`serverCertificateHashes`). No private key reaches a disk.

use wtransport::{
    Identity,
    tls::{
        Sha256DigestFmt,
        self_signed::time::{Duration, OffsetDateTime},
    },
};

/// What the web server tells a page about the certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// SHA-256 of the certificate, lower-case hexadecimal.
    pub sha256: String,
    /// Validity, seconds since 1970.
    pub not_before: u64,
    pub not_after: u64,
}

/// A browser accepts a certificate by its hash only if it is valid for less
/// than two weeks.
pub const MAX_DAYS: u32 = 13;

/// A new ECDSA P-256 certificate, valid from one hour ago (a page's clock
/// may be a little behind the board's) for `days` days.
pub fn make(days: u32) -> Result<(Identity, CertificateInfo), String> {
    if days == 0 || days > MAX_DAYS {
        return Err(format!(
            "a certificate may be valid for 1 to {MAX_DAYS} days"
        ));
    }
    let not_before = OffsetDateTime::now_utc() - Duration::HOUR;
    let not_after = not_before + Duration::days(i64::from(days));
    let identity = Identity::self_signed_builder()
        .subject_alt_names(["localhost"])
        .not_before(not_before)
        .not_after(not_after)
        .build()
        .map_err(|error| format!("certificate: {error}"))?;
    let certificate = identity
        .certificate_chain()
        .as_slice()
        .first()
        .ok_or("certificate: empty chain")?;
    let info = CertificateInfo {
        sha256: certificate
            .hash()
            .fmt(Sha256DigestFmt::DottedHex)
            .replace(':', "")
            .to_ascii_lowercase(),
        not_before: not_before.unix_timestamp().max(0) as u64,
        not_after: not_after.unix_timestamp().max(0) as u64,
    };
    Ok((identity, info))
}

/// Whether `now` (seconds since 1970) lies outside a certificate's validity.
/// The board starts with its clock wherever the card left it and takes the
/// network time later, so a certificate made at the start may be months old
/// or not yet valid by the time a page asks for it: such a one is replaced
/// as soon as the clock says so (security review of 2026-10-18, N11).
pub fn outside(info: &CertificateInfo, now: u64) -> bool {
    now < info.not_before || now >= info.not_after
}

/// The clock now, in seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_made_by_a_clock_months_behind_is_outside_once_the_clock_is_right() {
        // The board of N11 started at 29 July with the network time of
        // 18 October still to come.
        const STARTED: u64 = 1_785_283_200;
        const RIGHT: u64 = 1_792_281_600;
        let made = CertificateInfo {
            sha256: String::new(),
            not_before: STARTED - 3600,
            not_after: STARTED - 3600 + 13 * 24 * 3600,
        };
        assert!(!outside(&made, STARTED));
        assert!(outside(&made, RIGHT));
        // A clock that went back is outside too.
        assert!(outside(&made, STARTED - 7200));
        let (_, fresh) = make(13).unwrap();
        assert!(!outside(&fresh, now()));
    }

    #[test]
    fn a_certificate_is_short_lived_and_backdated() {
        let (_, info) = make(13).unwrap();
        assert_eq!(info.sha256.len(), 64);
        assert!(
            info.sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
        assert!(info.not_before <= now - 3500 && info.not_before >= now - 3700);
        assert_eq!(info.not_after - info.not_before, 13 * 24 * 3600);
        // Two certificates differ.
        assert_ne!(make(13).unwrap().1.sha256, info.sha256);
        assert!(make(14).is_err());
        assert!(make(0).is_err());
    }
}
