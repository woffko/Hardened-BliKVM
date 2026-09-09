#[test]
fn upload_wire_limit_matches_the_signed_archive_verifier() {
    assert_eq!(
        bli_kvm_update_protocol::upload::MAX_UPLOAD_ARCHIVE_BYTES,
        bli_kvm_update_verifier::MAX_ARCHIVE_BYTES,
    );
}
