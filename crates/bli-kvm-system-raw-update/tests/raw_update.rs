//! The RAW update flow over regular files standing in for partitions.

use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use bli_kvm_system_image_verifier::{
    SYSTEM_ARCHITECTURE, SYSTEM_FORMAT_V1, SYSTEM_PRODUCT, SYSTEM_TARGET, SystemComponent,
    SystemComponentRole, SystemContentType, SystemManifest, SystemTrustRoot,
    SystemVerificationPolicy,
};
use bli_kvm_system_raw_update::{
    BACKUP_FILE, BootAction, Phase, StageRequest, Tools, UpdateError, boot, cancel, confirm,
    mark_booted, merge_fstab, merge_shadow, minimum_sequence, patch_armbian_env, read_record,
    request_rollback, stage,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};

const LAYOUT: &str = "layout-h616-card-v1";
const PARTITION: usize = 16384;
const IMAGE: usize = 8192;

/// "Compression" is the identity, so the payload is the image itself.
#[derive(Default)]
struct FakeTools {
    corrupt_write: bool,
    fail_finish: bool,
    fail_archive: bool,
    calls: Vec<String>,
    restored: Vec<PathBuf>,
}

impl Tools for FakeTools {
    fn decompress(
        &mut self,
        payload: &Path,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut bytes = fs::read(payload)?;
        if self.corrupt_write && !payload.ends_with(BACKUP_FILE) {
            bytes[100] ^= 0xff;
        }
        for chunk in bytes.chunks(3000) {
            sink(chunk)?;
        }
        Ok(())
    }

    fn compress(&mut self, source: &Path, size: u64, target: &Path) -> io::Result<()> {
        let mut bytes = Vec::new();
        fs::File::open(source)?.take(size).read_to_end(&mut bytes)?;
        fs::write(target, bytes)
    }

    fn finish_filesystem(&mut self, _device: &Path, uuid: &str) -> io::Result<()> {
        self.calls.push(format!("finish {uuid}"));
        if self.fail_finish {
            return Err(io::Error::other("resize failed"));
        }
        Ok(())
    }

    fn prepare_root(&mut self, device: &Path, uuid: &str, staging: &Path) -> io::Result<()> {
        self.calls.push(format!(
            "patch {uuid} {} {}",
            fs::read_to_string(staging.join("shadow"))?.trim(),
            fs::read_to_string(staging.join("fstab"))?.trim()
        ));
        self.restore_state(device, &staging.join("state.tar"))
    }

    fn archive_state(&mut self, root: &Path, paths: &[&str], target: &Path) -> io::Result<()> {
        if self.fail_archive {
            // As the real tool fails: its unfinished archive is left.
            fs::write(target.with_extension("tar.new"), b"half")?;
            return Err(io::Error::other("the device state could not be archived"));
        }
        let present: Vec<String> = paths
            .iter()
            .filter(|path| root.join(path).exists())
            .map(|path| path.to_string())
            .collect();
        fs::write(target, present.join("\n"))
    }

    fn restore_state(&mut self, root: &Path, archive: &Path) -> io::Result<()> {
        self.restored.push(root.to_path_buf());
        self.calls
            .push(format!("restore {}", fs::read_to_string(archive)?));
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn image(fill: u8) -> Vec<u8> {
    let mut bytes = vec![fill; IMAGE];
    bytes[..4].copy_from_slice(&[0x28, 0xb5, 0x2f, 0xfd]);
    bytes
}

fn append(builder: &mut Builder<Vec<u8>>, path: &str, directory: bool, bytes: &[u8]) {
    let mut header = Header::new_ustar();
    header.set_entry_type(if directory {
        EntryType::Directory
    } else {
        EntryType::Regular
    });
    header.set_mode(if directory { 0o755 } else { 0o644 });
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(bytes.to_vec()))
        .unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    bundle: PathBuf,
    staging: PathBuf,
    device: PathBuf,
    root: PathBuf,
    fstab: PathBuf,
    shadow: PathBuf,
    passwd: PathBuf,
    group: PathBuf,
    trust: SystemTrustRoot,
    policy: SystemVerificationPolicy,
    image: Vec<u8>,
}

fn fixture(sequence: u64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let trust = SystemTrustRoot::new("sys-test-1", signing.verifying_key().to_bytes()).unwrap();
    let policy = SystemVerificationPolicy::new(LAYOUT, 1, 1, PARTITION as u64, None).unwrap();
    let payload = image(0x5a);
    let manifest = SystemManifest {
        format: SYSTEM_FORMAT_V1.into(),
        product: SYSTEM_PRODUCT.into(),
        target: SYSTEM_TARGET.into(),
        architecture: SYSTEM_ARCHITECTURE.into(),
        key_id: "sys-test-1".into(),
        version: "1.2.0".into(),
        sequence,
        minimum_recovery_sequence: 1,
        minimum_boot_control_sequence: 1,
        layout_id: LAYOUT.into(),
        source_commit: "1234567890abcdef1234567890abcdef12345678".into(),
        components: vec![SystemComponent {
            role: SystemComponentRole::Rootfs,
            path: "rootfs.img.zst".into(),
            content_type: SystemContentType::Ext4Zstd,
            size: payload.len() as u64,
            sha256: sha256(&payload),
            image_size: Some(payload.len() as u64),
            image_sha256: Some(sha256(&payload)),
        }],
    };
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = signing.sign(&manifest_bytes).to_bytes();
    let mut builder = Builder::new(Vec::new());
    append(&mut builder, "manifest.json", false, &manifest_bytes);
    append(&mut builder, "manifest.sig", false, &signature);
    append(&mut builder, "payload/", true, &[]);
    append(&mut builder, "payload/rootfs.img.zst", false, &payload);
    let bundle = dir.path().join("system.tar");
    fs::write(&bundle, builder.into_inner().unwrap()).unwrap();
    let staging = dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    let device = dir.path().join("mmcblk0p1");
    fs::write(&device, vec![0x11; PARTITION]).unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("etc/hardened-blikvm")).unwrap();
    fs::write(root.join("etc/machine-id"), "abc\n").unwrap();
    let fstab = dir.path().join("fstab");
    fs::write(&fstab, "UUID=data /mnt ext4 defaults 0 2\n").unwrap();
    let shadow = dir.path().join("shadow");
    fs::write(&shadow, "root:$6$device:20000:0:99999:7:::\n").unwrap();
    let passwd = dir.path().join("passwd");
    fs::write(&passwd, "root:x:0:0::/root:/bin/sh\n").unwrap();
    let group = dir.path().join("group");
    fs::write(&group, "root:x:0:\n").unwrap();
    Fixture {
        _dir: dir,
        bundle,
        staging,
        device,
        root,
        fstab,
        shadow,
        passwd,
        group,
        trust,
        policy,
        image: payload,
    }
}

fn stage_it(
    fixture: &Fixture,
    tools: &mut FakeTools,
) -> Result<bli_kvm_system_raw_update::Record, UpdateError> {
    stage(
        &StageRequest {
            bundle: &fixture.bundle,
            staging: &fixture.staging,
            trust: &fixture.trust,
            policy: &fixture.policy,
            minimum_sequence: 2,
            root: &fixture.root,
            root_uuid: "a990fd36-0857-4856-b0b8-5c84a7df516f",
            fstab: &fixture.fstab,
            shadow: &fixture.shadow,
            passwd: &fixture.passwd,
            group: &fixture.group,
        },
        tools,
    )
}

#[test]
fn a_staged_update_is_written_booted_restored_and_confirmed() {
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    let record = stage_it(&fixture, &mut tools).unwrap();
    assert_eq!(record.phase, Phase::Staged);
    // Only the carried paths that exist were archived.
    assert_eq!(
        fs::read_to_string(fixture.staging.join("state.tar")).unwrap(),
        "etc/hardened-blikvm\netc/machine-id"
    );
    // A second stage while one waits is still allowed (replaces it).
    assert!(stage_it(&fixture, &mut tools).is_ok());

    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    let device = fs::read(&fixture.device).unwrap();
    assert_eq!(&device[..IMAGE], fixture.image.as_slice());
    let record = read_record(&fixture.staging).unwrap().unwrap();
    assert_eq!(record.phase, Phase::Written);
    assert_eq!(record.backup_size, PARTITION as u64);
    assert!(
        tools
            .calls
            .iter()
            .any(|call| call == "finish a990fd36-0857-4856-b0b8-5c84a7df516f")
    );
    assert!(tools.calls.iter().any(|call| {
        call.starts_with("patch a990fd36-0857-4856-b0b8-5c84a7df516f root:$6$device:")
            && call.ends_with("UUID=data /mnt ext4 defaults 0 2")
    }));
    // No new stage while the new root is on probation.
    assert!(matches!(
        stage_it(&fixture, &mut tools),
        Err(UpdateError::Busy)
    ));

    // The carried state went onto the new root before it booted.
    assert_eq!(tools.restored, std::slice::from_ref(&fixture.device));
    // The new root's first boot: one attempt counted.
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Continue
    );
    assert_eq!(mark_booted(&fixture.staging).unwrap().phase, Phase::Booted);
    // Later boots leave a booted system alone.
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Continue
    );
    assert_eq!(confirm(&fixture.staging).unwrap().phase, Phase::Confirmed);
    assert!(!fixture.staging.join(BACKUP_FILE).exists());
    assert!(!fixture.staging.join("payload.img.zst").exists());
    assert!(confirm(&fixture.staging).is_err());
}

#[test]
fn a_new_root_that_never_reports_itself_is_replaced_by_the_backup() {
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    for _ in 0..2 {
        assert_eq!(
            boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
            BootAction::Continue
        );
    }
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
    let record = read_record(&fixture.staging).unwrap().unwrap();
    assert_eq!(record.phase, Phase::RolledBack);
    assert!(record.message.contains("did not come up"));
    // Nothing staged is left once the backup is the root again.
    for file in [
        BACKUP_FILE,
        "payload.img.zst",
        "state.tar",
        "fstab",
        "shadow",
    ] {
        assert!(!fixture.staging.join(file).exists(), "{file}");
    }
}

#[test]
fn a_failed_write_or_finish_restores_the_old_root() {
    for (corrupt_write, fail_finish) in [(true, false), (false, true)] {
        let fixture = fixture(3);
        let mut tools = FakeTools {
            corrupt_write,
            fail_finish,
            ..FakeTools::default()
        };
        stage_it(&fixture, &mut tools).unwrap();
        assert_eq!(
            boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
            BootAction::Reboot
        );
        assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
        let record = read_record(&fixture.staging).unwrap().unwrap();
        assert_eq!(record.phase, Phase::RolledBack);
        assert!(
            record.message.contains("backup was restored"),
            "{}",
            record.message
        );
    }
}

#[test]
fn a_changed_payload_or_an_interrupted_write_is_handled() {
    // The staged payload changed: nothing is written.
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
    let mut payload = fs::read(fixture.staging.join("payload.img.zst")).unwrap();
    payload[10] ^= 1;
    fs::write(fixture.staging.join("payload.img.zst"), payload).unwrap();
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Continue
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
    assert_eq!(
        read_record(&fixture.staging).unwrap().unwrap().phase,
        Phase::Failed
    );

    // Power lost during the write: the next boot restores the backup.
    let fixture = self::fixture(3);
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
    boot(&fixture.staging, &fixture.device, &mut tools).unwrap();
    let mut record = read_record(&fixture.staging).unwrap().unwrap();
    record.phase = Phase::Writing;
    bli_kvm_system_raw_update::write_record(&fixture.staging, &record).unwrap();
    fs::write(&fixture.device, vec![0x77; PARTITION]).unwrap();
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
}

#[test]
fn old_sequences_bad_bundles_and_cancellation() {
    let fixture = fixture(1);
    let mut tools = FakeTools::default();
    assert!(matches!(
        stage_it(&fixture, &mut tools),
        Err(UpdateError::Bundle(message)) if message.contains("below")
    ));
    let fixture = self::fixture(3);
    let mut bytes = fs::read(&fixture.bundle).unwrap();
    let last = bytes.len() - 1100;
    bytes[last] ^= 0xff;
    fs::write(&fixture.bundle, bytes).unwrap();
    assert!(matches!(
        stage_it(&fixture, &mut tools),
        Err(UpdateError::Bundle(_))
    ));
    let fixture = self::fixture(3);
    stage_it(&fixture, &mut tools).unwrap();
    cancel(&fixture.staging).unwrap();
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Continue
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
}

#[test]
fn a_failed_stage_leaves_nothing_behind() {
    // What a failed stage left (the payload, the unfinished archive) was
    // counted by the next upload's space check and removed by nothing
    // (security review of 2026-10-18, N4).
    let fixture = fixture(3);
    let mut tools = FakeTools {
        fail_archive: true,
        ..FakeTools::default()
    };
    assert!(stage_it(&fixture, &mut tools).is_err());
    let left: Vec<String> = fs::read_dir(&fixture.staging)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "apply.json")
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
    // The next stage goes through.
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
}

#[test]
fn carried_owners_must_name_the_same_accounts_in_the_new_image() {
    use bli_kvm_system_raw_update::owners_agree;
    let board = "root:x:0:0::/root:/bin/sh\nhardened-blikvm-server:x:997:993::/:/usr/sbin/nologin\nhardened-blikvm-update:x:996:993::/:/usr/sbin/nologin\noperator-tool:x:1500:1500::/:/bin/sh\n";
    // The same allocation: the state's owners mean the same accounts.
    assert_eq!(owners_agree(board, board), Ok(()));
    // Another board-only account does not matter.
    let image = "root:x:0:0::/root:/bin/sh\nhardened-blikvm-server:x:997:993::/:/usr/sbin/nologin\nhardened-blikvm-update:x:996:993::/:/usr/sbin/nologin\n";
    assert_eq!(owners_agree(board, image), Ok(()));
    // The image gives the server's id to another account: refused.
    let swapped = "root:x:0:0::/root:/bin/sh\nhardened-blikvm-quic:x:997:997::/:/usr/sbin/nologin\nhardened-blikvm-server:x:995:993::/:/usr/sbin/nologin\nhardened-blikvm-update:x:996:993::/:/usr/sbin/nologin\n";
    let refused = owners_agree(board, swapped).unwrap_err();
    assert!(refused.contains("hardened-blikvm-server"), "{refused}");
}

#[test]
fn staging_keeps_the_boards_accounts_beside_its_shadow() {
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.staging.join("passwd")).unwrap(),
        fs::read_to_string(&fixture.passwd).unwrap()
    );
    assert_eq!(
        fs::read_to_string(fixture.staging.join("group")).unwrap(),
        fs::read_to_string(&fixture.group).unwrap()
    );
}

#[test]
fn the_root_uuid_must_be_a_uuid_at_stage_and_at_boot() {
    // tune2fs takes "random", "time", "clear" and "null" as well, and the
    // value goes into the boot configuration (security review of
    // 2026-10-18, MED-08).
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    for wrong in [
        "random",
        "time",
        "a990fd36-0857-4856-b0b8-5c84a7df516",
        "x990fd36-0857-4856-b0b8-5c84a7df516f",
    ] {
        let request = StageRequest {
            bundle: &fixture.bundle,
            staging: &fixture.staging,
            trust: &fixture.trust,
            policy: &fixture.policy,
            minimum_sequence: 2,
            root: &fixture.root,
            root_uuid: wrong,
            fstab: &fixture.fstab,
            shadow: &fixture.shadow,
            passwd: &fixture.passwd,
            group: &fixture.group,
        };
        assert!(stage(&request, &mut tools).is_err(), "{wrong}");
    }
    // A record changed after staging is not written.
    let mut record = stage_it(&fixture, &mut tools).unwrap();
    record.root_uuid = "a990fd36-0857-4856-b0b8-5c84a7df516f\nextraargs=init=/bin/sh".into();
    bli_kvm_system_raw_update::write_record(&fixture.staging, &record).unwrap();
    let before = fs::read(&fixture.device).unwrap();
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Continue
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), before);
    let after = bli_kvm_system_raw_update::read_record(&fixture.staging)
        .unwrap()
        .unwrap();
    assert_eq!(after.phase, bli_kvm_system_raw_update::Phase::Failed);
}

#[test]
fn boot_configuration_follows_the_device() {
    assert_eq!(
        patch_armbian_env("verbosity=1\nrootdev=UUID=new\nrootfstype=ext4\n", "old"),
        "verbosity=1\nrootdev=UUID=old\nrootfstype=ext4\n"
    );
    let image = "# comment\nUUID=img-root / ext4 defaults,noatime 0 1\nUUID=img-data /mnt ext4 defaults 0 2\nUUID=img-msd /var/lib/hardened-blikvm-gadget/images ext4 defaults 0 2\ntmpfs /tmp tmpfs defaults 0 0\n";
    let device = "UUID=old-root / ext4 defaults 0 1\nUUID=dev-data /mnt ext4 defaults,noatime 0 2\nUUID=dev-msd /var/lib/hardened-blikvm-gadget/images ext4 defaults 0 2\n";
    assert_eq!(
        merge_fstab(image, device, "old-root"),
        "# comment\nUUID=old-root / ext4 defaults,noatime 0 1\nUUID=dev-data /mnt ext4 defaults,noatime 0 2\nUUID=dev-msd /var/lib/hardened-blikvm-gadget/images ext4 defaults 0 2\ntmpfs /tmp tmpfs defaults 0 0\n"
    );
    // The image's default root password never replaces the operator's.
    let image = "root:$6$image:0:0:99999:7:::\nsshd:*:19000:0:99999:7:::\nnewsvc:!:19000::::::\n";
    let device =
        "root:$6$device:20000:0:99999:7:::\nsshd:*:18000:0:99999:7:::\ngone:$6$x:1::::::\n";
    assert_eq!(
        merge_shadow(image, device),
        "root:$6$device:20000:0:99999:7:::\nsshd:*:18000:0:99999:7:::\nnewsvc:!:19000::::::\n"
    );
}

#[test]
fn the_operator_can_roll_back_and_confirmed_sequences_are_the_floor() {
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    assert_eq!(minimum_sequence(&fixture.staging), 1);
    // Rollback is offered only once the new root reported itself.
    stage_it(&fixture, &mut tools).unwrap();
    assert!(request_rollback(&fixture.staging).is_err());
    boot(&fixture.staging, &fixture.device, &mut tools).unwrap();
    boot(&fixture.staging, &fixture.device, &mut tools).unwrap();
    mark_booted(&fixture.staging).unwrap();
    assert_eq!(
        request_rollback(&fixture.staging).unwrap().phase,
        Phase::RollbackRequested
    );
    assert!(matches!(
        stage_it(&fixture, &mut tools),
        Err(UpdateError::Busy)
    ));
    assert_eq!(
        boot(&fixture.staging, &fixture.device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    assert_eq!(fs::read(&fixture.device).unwrap(), vec![0x11; PARTITION]);
    let record = read_record(&fixture.staging).unwrap().unwrap();
    assert_eq!(record.phase, Phase::RolledBack);
    assert_eq!(minimum_sequence(&fixture.staging), 1);

    // Confirming a bundle raises the floor above its sequence.
    stage_it(&fixture, &mut tools).unwrap();
    boot(&fixture.staging, &fixture.device, &mut tools).unwrap();
    boot(&fixture.staging, &fixture.device, &mut tools).unwrap();
    mark_booted(&fixture.staging).unwrap();
    confirm(&fixture.staging).unwrap();
    assert_eq!(minimum_sequence(&fixture.staging), 4);
    assert!(!fixture.staging.join("shadow").exists());
}

#[test]
fn the_floor_counts_the_running_systems_own_sequence() {
    use bli_kvm_system_raw_update::{SYSTEM_SEQUENCE_FILE, accepted_minimum};
    // A fresh card has no confirmed sequence; its root says which system it
    // is, so an older signed bundle is refused (security review of
    // 2026-10-18, MED-07).
    let staging = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    assert_eq!(accepted_minimum(staging.path(), root.path()).unwrap(), 1);
    let file = root.path().join(SYSTEM_SEQUENCE_FILE);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "6\n").unwrap();
    assert_eq!(accepted_minimum(staging.path(), root.path()).unwrap(), 6);
    fs::write(staging.path().join("confirmed-sequence"), "7\n").unwrap();
    assert_eq!(accepted_minimum(staging.path(), root.path()).unwrap(), 8);
    // A floor or a sequence that cannot be read is no floor of 1.
    fs::write(staging.path().join("confirmed-sequence"), "seven\n").unwrap();
    assert!(accepted_minimum(staging.path(), root.path()).is_err());
    fs::remove_file(staging.path().join("confirmed-sequence")).unwrap();
    fs::write(&file, "x\n").unwrap();
    assert!(accepted_minimum(staging.path(), root.path()).is_err());
}

/// The real zstd, e2fsprogs and tar on image files; only mounting (which
/// needs root) is replaced.
struct RealExceptMount(bli_kvm_system_raw_update::SystemTools);

impl Tools for RealExceptMount {
    fn decompress(
        &mut self,
        payload: &Path,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        self.0.decompress(payload, sink)
    }
    fn compress(&mut self, source: &Path, size: u64, target: &Path) -> io::Result<()> {
        self.0.compress(source, size, target)
    }
    fn finish_filesystem(&mut self, device: &Path, uuid: &str) -> io::Result<()> {
        self.0.finish_filesystem(device, uuid)
    }
    fn prepare_root(&mut self, _device: &Path, _uuid: &str, _staging: &Path) -> io::Result<()> {
        Ok(())
    }
    fn archive_state(&mut self, root: &Path, paths: &[&str], target: &Path) -> io::Result<()> {
        self.0.archive_state(root, paths, target)
    }
    fn restore_state(&mut self, root: &Path, archive: &Path) -> io::Result<()> {
        self.0.restore_state(root, archive)
    }
}

fn tool(name: &str) -> bool {
    ["/sbin", "/usr/sbin", "/usr/bin", "/bin"]
        .iter()
        .any(|dir| Path::new(dir).join(name).exists())
}

#[test]
fn real_tools_write_grow_and_relabel_an_ext4_image() {
    if ![
        "zstd",
        "mke2fs",
        "e2fsck",
        "resize2fs",
        "tune2fs",
        "dumpe2fs",
        "tar",
    ]
    .iter()
    .all(|name| tool(name))
    {
        eprintln!("skipped: e2fsprogs, zstd or tar missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let run = |program: &str, args: &[&str]| {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{program}: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    // The new root, shrunk to 4 MiB, compressed; the partition is 16 MiB.
    let image = dir.path().join("root.ext4");
    let content = dir.path().join("content");
    fs::create_dir_all(content.join("etc")).unwrap();
    fs::write(content.join("etc/marker"), "new root\n").unwrap();
    run(
        "mke2fs",
        &[
            "-q",
            "-t",
            "ext4",
            "-U",
            "11111111-2222-3333-4444-555555555555",
            "-d",
            content.to_str().unwrap(),
            image.to_str().unwrap(),
            "4M",
        ],
    );
    let image_bytes = fs::read(&image).unwrap();
    let payload = dir.path().join("root.ext4.zst");
    run(
        "zstd",
        &[
            "-q",
            "-19",
            image.to_str().unwrap(),
            "-o",
            payload.to_str().unwrap(),
        ],
    );
    let device = dir.path().join("partition");
    run(
        "mke2fs",
        &[
            "-q",
            "-t",
            "ext4",
            "-U",
            "a990fd36-0857-4856-b0b8-5c84a7df516f",
            device.to_str().unwrap(),
            "16M",
        ],
    );
    let old_device = fs::read(&device).unwrap();

    // Stage by hand (the bundle path is covered by the other tests).
    let staging = dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    fs::copy(&payload, staging.join("payload.img.zst")).unwrap();
    fs::write(staging.join("fstab"), "").unwrap();
    let (payload_size, payload_sha256) = bli_kvm_system_raw_update::digest(&payload, None).unwrap();
    let record = bli_kvm_system_raw_update::Record {
        schema: 1,
        generation: 1,
        phase: Phase::Staged,
        version: "1.0.0".into(),
        sequence: 1,
        manifest_sha256: "0".repeat(64),
        payload_size,
        payload_sha256,
        image_size: image_bytes.len() as u64,
        image_sha256: sha256(&image_bytes),
        root_uuid: "a990fd36-0857-4856-b0b8-5c84a7df516f".into(),
        backup_size: 0,
        backup_sha256: String::new(),
        boot_attempts: 0,
        message: String::new(),
    };
    bli_kvm_system_raw_update::write_record(&staging, &record).unwrap();
    let mut tools = RealExceptMount(bli_kvm_system_raw_update::SystemTools);
    assert_eq!(
        boot(&staging, &device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    let record = read_record(&staging).unwrap().unwrap();
    assert_eq!(record.phase, Phase::Written, "{}", record.message);
    // Grown to the partition, relabelled with the old identity, new content.
    let info = run("dumpe2fs", &["-h", device.to_str().unwrap()]);
    assert!(
        info.contains("a990fd36-0857-4856-b0b8-5c84a7df516f"),
        "{info}"
    );
    let blocks: u64 = info
        .lines()
        .find_map(|line| line.strip_prefix("Block count:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let block_size: u64 = info
        .lines()
        .find_map(|line| line.strip_prefix("Block size:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(blocks * block_size, 16 * 1024 * 1024);
    let marker = run(
        "debugfs",
        &["-R", "cat /etc/marker", device.to_str().unwrap()],
    );
    assert!(marker.contains("new root"), "{marker}");

    // The backup puts the old partition back byte for byte.
    let mut record = record;
    record.phase = Phase::Written;
    record.boot_attempts = 2;
    bli_kvm_system_raw_update::write_record(&staging, &record).unwrap();
    assert_eq!(
        boot(&staging, &device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    assert_eq!(fs::read(&device).unwrap(), old_device);

    // The carried state survives an archive and restore round trip.
    let old_root = dir.path().join("old");
    fs::create_dir_all(old_root.join("etc/hardened-blikvm")).unwrap();
    fs::write(old_root.join("etc/hardened-blikvm/tls.key"), "secret-ish").unwrap();
    fs::set_permissions(
        old_root.join("etc/hardened-blikvm/tls.key"),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let archive = dir.path().join("state.tar");
    tools
        .archive_state(
            &old_root,
            bli_kvm_system_raw_update::CARRIED_STATE,
            &archive,
        )
        .unwrap();
    let new_root = dir.path().join("new");
    fs::create_dir(&new_root).unwrap();
    tools.restore_state(&new_root, &archive).unwrap();
    let restored = new_root.join("etc/hardened-blikvm/tls.key");
    assert_eq!(fs::read_to_string(&restored).unwrap(), "secret-ish");
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&restored).unwrap().permissions())
            & 0o777,
        0o600
    );
}

#[test]
fn the_bundle_builder_output_stages_and_writes() {
    if ![
        "python3",
        "openssl",
        "zstd",
        "mke2fs",
        "e2fsck",
        "resize2fs",
        "tar",
    ]
    .iter()
    .all(|name| tool(name))
    {
        eprintln!("skipped: python3, openssl, zstd, e2fsprogs or tar missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let run = |program: &str, args: &[&str]| {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{program}: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    // An ephemeral system key and trust file.
    let key = dir.path().join("key.pem");
    run(
        "openssl",
        &[
            "genpkey",
            "-algorithm",
            "ed25519",
            "-out",
            key.to_str().unwrap(),
        ],
    );
    let der = std::process::Command::new("openssl")
        .args([
            "pkey",
            "-in",
            key.to_str().unwrap(),
            "-pubout",
            "-outform",
            "DER",
        ])
        .output()
        .unwrap()
        .stdout;
    let public: [u8; 32] = der[der.len() - 32..].try_into().unwrap();
    let public_hex: String = public.iter().map(|byte| format!("{byte:02x}")).collect();
    let trust_path = dir.path().join("trust.json");
    fs::write(
        &trust_path,
        format!(
            "{{\"keyId\":\"sys-test-builder\",\"publicKey\":\"{public_hex}\",\"layoutId\":\"{LAYOUT}\"}}"
        ),
    )
    .unwrap();
    // A 32 MiB root image with a little content.
    let content = dir.path().join("content");
    fs::create_dir_all(content.join("etc")).unwrap();
    fs::write(content.join("etc/marker"), "built root\n").unwrap();
    let rootfs = dir.path().join("root.ext4");
    run(
        "mke2fs",
        &[
            "-q",
            "-t",
            "ext4",
            "-b",
            "4096",
            "-d",
            content.to_str().unwrap(),
            rootfs.to_str().unwrap(),
            "32M",
        ],
    );
    let builder =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/build_system_raw_bundle.py");
    let out = dir.path().join("out");
    run(
        "python3",
        &[
            builder.to_str().unwrap(),
            "--rootfs",
            rootfs.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--trust",
            trust_path.to_str().unwrap(),
            "--version",
            "1.4.0",
            "--sequence",
            "5",
            "--source-commit",
            &"ab".repeat(20),
            "--output-dir",
            out.to_str().unwrap(),
        ],
    );
    let bundle = out.join("hardened-blikvm-system-1.4.0-seq5.tar");

    let (trust, layout) = bli_kvm_system_raw_update::load_trust(&trust_path).unwrap();
    let device = dir.path().join("partition");
    run(
        "mke2fs",
        &[
            "-q",
            "-t",
            "ext4",
            "-U",
            "a990fd36-0857-4856-b0b8-5c84a7df516f",
            device.to_str().unwrap(),
            "48M",
        ],
    );
    let policy =
        SystemVerificationPolicy::new(layout, 1, 1, fs::metadata(&device).unwrap().len(), None)
            .unwrap();
    let staging = dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/machine-id"), "abc\n").unwrap();
    let fstab = dir.path().join("fstab");
    fs::write(&fstab, "").unwrap();
    let shadow = dir.path().join("shadow");
    fs::write(&shadow, "root:!:1::::::\n").unwrap();
    let passwd = dir.path().join("passwd");
    fs::write(&passwd, "root:x:0:0::/root:/bin/sh\n").unwrap();
    let group = dir.path().join("group");
    fs::write(&group, "root:x:0:\n").unwrap();
    let mut tools = RealExceptMount(bli_kvm_system_raw_update::SystemTools);
    let request = StageRequest {
        bundle: &bundle,
        staging: &staging,
        trust: &trust,
        policy: &policy,
        minimum_sequence: 5,
        root: &root,
        root_uuid: "a990fd36-0857-4856-b0b8-5c84a7df516f",
        fstab: &fstab,
        shadow: &shadow,
        passwd: &passwd,
        group: &group,
    };
    let record = stage(&request, &mut tools).unwrap();
    assert_eq!((record.version.as_str(), record.sequence), ("1.4.0", 5));
    // Shrunk below the 32 MiB it was built with.
    assert!(record.image_size < 32 * 1024 * 1024 && record.image_size % 4096 == 0);
    assert_eq!(
        boot(&staging, &device, &mut tools).unwrap(),
        BootAction::Reboot
    );
    assert_eq!(
        read_record(&staging).unwrap().unwrap().phase,
        Phase::Written
    );
    let marker = run(
        "debugfs",
        &["-R", "cat /etc/marker", device.to_str().unwrap()],
    );
    assert!(marker.contains("built root"), "{marker}");

    // A tampered payload byte is refused before anything is staged.
    let mut bytes = fs::read(&bundle).unwrap();
    let payload = bytes
        .windows(4)
        .position(|window| window == [0x28, 0xb5, 0x2f, 0xfd])
        .unwrap();
    bytes[payload + 100] ^= 0xff;
    let tampered = dir.path().join("tampered.tar");
    fs::write(&tampered, bytes).unwrap();
    let staging2 = dir.path().join("staging2");
    fs::create_dir(&staging2).unwrap();
    let request = StageRequest {
        bundle: &tampered,
        staging: &staging2,
        ..request
    };
    assert!(matches!(
        stage(&request, &mut tools),
        Err(UpdateError::Bundle(_))
    ));
    assert!(read_record(&staging2).unwrap().is_none());
}

#[test]
fn every_ssh_host_key_type_is_carried() {
    // The image's first-boot provisioning creates any missing host key, so a
    // type left out would be replaced silently by the update.
    for kind in ["dsa", "ecdsa", "ed25519", "rsa"] {
        for suffix in ["", ".pub"] {
            let path = format!("etc/ssh/ssh_host_{kind}_key{suffix}");
            assert!(
                bli_kvm_system_raw_update::CARRIED_STATE.contains(&path.as_str()),
                "{path}"
            );
        }
    }
}

#[test]
fn the_application_update_state_is_carried() {
    // The journal (floor and confirmed slot), the slot pointer and the
    // protected copies' selection: without them the board starts the image's
    // own application after the update, with the image's floor (security
    // review of 2026-10-18, HIGH-04).
    for path in [
        "var/lib/hardened-blikvm-update",
        "var/lib/hardened-blikvm-update-v2",
        "var/lib/hardened-blikvm-slots",
        "var/lib/hardened-blikvm-slots-v2",
        "var/lib/hardened-blikvm-exec",
        "var/lib/hardened-blikvm-exec-ledger",
    ] {
        assert!(
            bli_kvm_system_raw_update::CARRIED_STATE.contains(&path),
            "{path}"
        );
    }
}

#[test]
fn the_network_identity_is_carried() {
    for path in [
        "etc/NetworkManager/system-connections",
        "var/lib/NetworkManager/secret_key",
        "etc/machine-id",
        "etc/hostname",
    ] {
        assert!(
            bli_kvm_system_raw_update::CARRIED_STATE.contains(&path),
            "{path}"
        );
    }
}

#[test]
fn leftovers_of_a_finished_update_can_be_discarded() {
    let fixture = fixture(3);
    let mut tools = FakeTools::default();
    stage_it(&fixture, &mut tools).unwrap();
    // Files an older boot step left behind after a rollback.
    let mut record = read_record(&fixture.staging).unwrap().unwrap();
    record.phase = Phase::RolledBack;
    bli_kvm_system_raw_update::write_record(&fixture.staging, &record).unwrap();
    fs::write(fixture.staging.join(BACKUP_FILE), b"old root").unwrap();
    cancel(&fixture.staging).unwrap();
    for file in [
        BACKUP_FILE,
        "payload.img.zst",
        "state.tar",
        "fstab",
        "shadow",
    ] {
        assert!(!fixture.staging.join(file).exists(), "{file}");
    }
    let record = read_record(&fixture.staging).unwrap().unwrap();
    assert_eq!(record.phase, Phase::RolledBack);
}
