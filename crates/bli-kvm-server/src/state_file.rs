//! The server's small state files under its state directory.

/// Replaces `name` in the state directory with `bytes`: written to a file of
/// its own (named from `prefix`) of mode 0600, synced, and renamed over the
/// old one, so that two changes at once cannot share a file and a reader
/// never sees half of one.
pub(crate) fn replace(
    state_dir: &std::path::Path,
    prefix: &str,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(state_dir)?;
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(state_dir)?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(state_dir.join(name))
        .map_err(|error| error.error)?;
    Ok(())
}
