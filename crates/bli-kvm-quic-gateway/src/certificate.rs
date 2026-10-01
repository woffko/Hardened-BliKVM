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

#[cfg(test)]
mod tests {
    use super::*;

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
