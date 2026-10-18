//! How the carried state is archived, against a stand-in `tar` that records
//! what it is asked and answers as told. A test binary of its own, as it
//! changes the process's PATH.

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use bli_kvm_system_raw_update::{SystemTools, Tools};

/// A `tar` that appends its arguments to `calls`, makes the `--file` it is
/// given, and exits with the next status in `answers` (0 once they run out).
fn stand_in(directory: &Path, answers: &[u8]) {
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "{calls}"
next=$(head -n 1 "{answers}" 2>/dev/null)
sed -i '1d' "{answers}" 2>/dev/null
while [ $# -gt 0 ]; do
  if [ "$1" = "--file" ]; then : > "$2"; fi
  shift
done
exit "${{next:-0}}"
"#,
        calls = directory.join("calls").display(),
        answers = directory.join("answers").display(),
    );
    fs::write(directory.join("tar"), script).unwrap();
    fs::set_permissions(directory.join("tar"), fs::Permissions::from_mode(0o755)).unwrap();
    let lines: String = answers.iter().map(|answer| format!("{answer}\n")).collect();
    fs::write(directory.join("answers"), lines).unwrap();
}

#[test]
fn the_state_is_archived_without_crossing_into_other_file_systems_and_tried_twice() {
    let bin = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("var/lib/hardened-blikvm-gadget")).unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: this test binary has this one test, so nothing reads the
    // environment meanwhile.
    unsafe { std::env::set_var("PATH", format!("{}:{path}", bin.path().display())) };

    // The virtual media partition is mounted inside a carried directory: the
    // archive must not copy the images (security review of 2026-10-18, N3).
    stand_in(bin.path(), &[0]);
    let target = root.path().join("state.tar");
    SystemTools
        .archive_state(root.path(), &["var/lib/hardened-blikvm-gadget"], &target)
        .unwrap();
    let calls = fs::read_to_string(bin.path().join("calls")).unwrap();
    assert!(calls.contains("--one-file-system"), "{calls}");

    // A file that changed while it was read (tar's status 1) is tried once
    // more; a second change is reported.
    fs::remove_file(bin.path().join("calls")).unwrap();
    stand_in(bin.path(), &[1, 0]);
    SystemTools
        .archive_state(root.path(), &["var/lib/hardened-blikvm-gadget"], &target)
        .unwrap();
    assert_eq!(
        fs::read_to_string(bin.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    stand_in(bin.path(), &[1, 1]);
    let error = SystemTools
        .archive_state(root.path(), &["var/lib/hardened-blikvm-gadget"], &target)
        .unwrap_err();
    assert!(error.to_string().contains("changed while"), "{error}");
}
