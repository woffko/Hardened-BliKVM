//! Bounded systemd adapter for the closed application composition.

use std::{
    collections::BTreeMap,
    io::{self, Read},
    os::fd::AsRawFd,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation};

const PROPERTIES: &str = "LoadState,ActiveState,SubState,MainPID,ControlPID,Job,Result,ExecMainCode,ExecMainStatus,KillMode,SendSIGKILL,ControlGroup";
const MAX_OUTPUT: usize = 4096;
const RUNNING_PROPERTIES: &str = "Id,LoadState,ActiveState,SubState,MainPID,ControlPID,Job,Result";
const START_PROPERTIES: &str = "Id,LoadState,Type,ExecCondition,ExecStartPre,ExecStartPost,After,Requires,Wants,Requisite,BindsTo";
const JOB_PROPERTIES: &str = "Id,Job";
const START_DEPENDENCIES: [&str; 5] = ["After", "Requires", "Wants", "Requisite", "BindsTo"];
const LEGACY_UPDATE_UNITS: [&str; 4] = [
    "hardened-blikvm-update-recover.service",
    "hardened-blikvm-update-confirm.socket",
    "hardened-blikvm-update-apply-candidate.service",
    "hardened-blikvm-update-apply-confirmed.service",
];

pub struct SystemdController {
    executable: PathBuf,
    deadline: Instant,
    #[cfg(test)]
    shell_fixture: bool,
}

impl SystemdController {
    /// A closed native start request; the caller still verifies identity and
    /// readiness. A killed/timed-out client does not cancel the systemd job.
    pub fn start_native_unit(
        &self,
        unit: crate::native_identity::NativeStartUnit,
    ) -> Result<(), CompositionError> {
        self.validate_native_start(unit)?;
        self.run(&["start", "--", unit.unit().name()])?;
        Ok(())
    }

    /// Read-only preflight, also repeated immediately before dispatch.
    pub fn validate_native_start(
        &self,
        unit: crate::native_identity::NativeStartUnit,
    ) -> Result<(), CompositionError> {
        // Legacy candidate ExecStartPost runs a generic health reporter. Native
        // confirmation belongs to the coordinator after full composition health,
        // not a unit hook waiting on the manager locks held by that coordinator.
        let output = self.run(&[
            "show",
            "--all",
            "--property",
            START_PROPERTIES,
            "--",
            unit.unit().name(),
        ])?;
        validate_start_contract(&output, unit.unit())
    }

    /// Bind a closed unit's live main process to an already verified open slot
    /// executable. This is identity evidence only, not IPC/application readiness.
    pub fn observe_unit_identity(
        &self,
        unit: ApplicationUnit,
        expected: &std::fs::File,
    ) -> Result<crate::process_identity::ProcessIdentity, CompositionError> {
        let observe_pid = || {
            let output = self.run(&[
                "show",
                "--all",
                "--property",
                RUNNING_PROPERTIES,
                "--",
                unit.name(),
            ])?;
            running_pid(&output, unit)
        };
        let pid = observe_pid()?;
        let before = crate::process_identity::observe_process_identity(pid, expected)
            .map_err(|_| CompositionError::Controller)?;
        if observe_pid()? != pid {
            return Err(CompositionError::Controller);
        }
        let after = crate::process_identity::observe_process_identity(pid, expected)
            .map_err(|_| CompositionError::Controller)?;
        if before != after {
            return Err(CompositionError::Controller);
        }
        if Instant::now() >= self.deadline {
            return Err(CompositionError::Deadline);
        }
        Ok(after)
    }

    /// Cancel a queued job of a closed unit and re-observe until none remains.
    /// A disconnected coordinator can leave accepted jobs behind; the next
    /// session reconciles them here instead of refusing forever. The cancel
    /// client's own result is not trusted: only the re-observation counts.
    pub fn cancel_queued_job(&self, unit: ApplicationUnit) -> Result<(), CompositionError> {
        loop {
            let output = self.run(&[
                "show",
                "--all",
                "--property",
                JOB_PROPERTIES,
                "--",
                unit.name(),
            ])?;
            let Some(job) = queued_job(&output, unit)? else {
                return Ok(());
            };
            let _ = self.run(&["cancel", "--", &job.to_string()]);
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or(CompositionError::Deadline)?;
            thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    }

    /// One deadline covers both stop phases and all observations. Callers must
    /// reserve additional time for candidate startup within the health budget.
    pub fn new(deadline: Instant) -> Result<Self, CompositionError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(CompositionError::Deadline)?;
        if remaining.is_zero() || remaining > Duration::from_secs(45) {
            return Err(CompositionError::Deadline);
        }
        Ok(Self {
            executable: PathBuf::from("/usr/bin/systemctl"),
            deadline,
            #[cfg(test)]
            shell_fixture: false,
        })
    }

    fn run(&self, arguments: &[&str]) -> Result<Vec<u8>, CompositionError> {
        if Instant::now() >= self.deadline {
            return Err(CompositionError::Deadline);
        }
        let deadline = self.deadline.min(Instant::now() + Duration::from_secs(2));
        let mut command = Command::new(&self.executable);
        #[cfg(test)]
        if self.shell_fixture {
            // Read generated test scripts as data instead of exec'ing a freshly
            // written inode, which can transiently fail with ETXTBSY. Production
            // always executes the closed systemctl path directly, without retry.
            command = Command::new("/bin/sh");
            command.arg(&self.executable);
        }
        let mut child = ReapedChild(
            command
                .env_clear()
                .env("LC_ALL", "C")
                .args(["--system", "--no-ask-password", "--no-pager"])
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|source| CompositionError::ClientIo {
                    operation: "spawn",
                    source,
                })?,
        );
        let mut stdout = child.0.stdout.take().ok_or(CompositionError::Controller)?;
        let fd = stdout.as_raw_fd();
        // SAFETY: stdout owns this live descriptor. F_GETFL/F_SETFL do not
        // dereference memory; the original flags are preserved.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(CompositionError::ClientIo {
                operation: "configure stdout",
                source: io::Error::last_os_error(),
            });
        }
        let mut output = Vec::new();
        let mut buffer = [0_u8; 1024];
        let mut eof = false;
        loop {
            if Instant::now() >= deadline {
                // Killing this client does not cancel a queued systemd job.
                // The caller retains transaction recovery intent on error.
                return Err(CompositionError::Deadline);
            }
            if !eof {
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(count) => {
                        if output.len() + count > MAX_OUTPUT {
                            return Err(CompositionError::ClientOutputLimit);
                        }
                        output.extend_from_slice(&buffer[..count]);
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(source) => {
                        return Err(CompositionError::ClientIo {
                            operation: "read stdout",
                            source,
                        });
                    }
                }
            }
            if let Some(status) =
                child
                    .0
                    .try_wait()
                    .map_err(|source| CompositionError::ClientIo {
                        operation: "wait",
                        source,
                    })?
            {
                if !status.success() {
                    return Err(CompositionError::ClientExit {
                        code: status.code(),
                        signal: status.signal(),
                    });
                }
                if eof {
                    return Ok(output);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl ServiceController for SystemdController {
    fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError> {
        self.run(&["--no-block", "stop", "--", unit.name()])?;
        Ok(())
    }

    fn observe_stop(&mut self, unit: ApplicationUnit) -> Result<StopObservation, CompositionError> {
        loop {
            let output =
                self.run(&["show", "--all", "--property", PROPERTIES, "--", unit.name()])?;
            let state = parse_stop_observation(&output);
            if state != StopObservation::RunningOrQueued {
                return Ok(state);
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or(CompositionError::Deadline)?;
            thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    }
}

struct ReapedChild(Child);
impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn validate_start_contract(output: &[u8], unit: ApplicationUnit) -> Result<(), CompositionError> {
    let invalid = || CompositionError::IncompatibleStartup(unit);
    if output.len() > MAX_OUTPUT {
        return Err(invalid());
    }
    let text = std::str::from_utf8(output).map_err(|_| invalid())?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or_else(invalid)?;
        if !START_PROPERTIES.split(',').any(|expected| expected == key)
            || fields.insert(key, value).is_some()
        {
            return Err(invalid());
        }
    }
    // systemd 249 omits empty exec hook lists even with --all; absent means
    // no hook. Every other property must be reported exactly once.
    for hook in ["ExecCondition", "ExecStartPre", "ExecStartPost"] {
        fields.entry(hook).or_insert("");
    }
    if fields.len() != START_PROPERTIES.split(',').count()
        || fields["Id"] != unit.name()
        || fields["LoadState"] != "loaded"
        || fields["Type"] != "simple"
        || !fields["ExecCondition"].is_empty()
        || !fields["ExecStartPre"].is_empty()
        || !fields["ExecStartPost"].is_empty()
    {
        return Err(invalid());
    }
    // Inspect effective dependencies, not just the base unit's hook text. The
    // legacy recover gate can wait on this coordinator, and legacy confirm/apply
    // units introduce competing transition authority. This is a direct-edge
    // rejection, not proof that the complete installed graph is cycle-free.
    for property in START_DEPENDENCIES {
        if fields[property]
            .split_ascii_whitespace()
            .any(|dependency| LEGACY_UPDATE_UNITS.contains(&dependency))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn queued_job(output: &[u8], unit: ApplicationUnit) -> Result<Option<u32>, CompositionError> {
    if output.len() > MAX_OUTPUT {
        return Err(CompositionError::Controller);
    }
    let text = std::str::from_utf8(output).map_err(|_| CompositionError::Controller)?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or(CompositionError::Controller)?;
        if !JOB_PROPERTIES.split(',').any(|expected| expected == key)
            || fields.insert(key, value).is_some()
        {
            return Err(CompositionError::Controller);
        }
    }
    if fields.len() != JOB_PROPERTIES.split(',').count() || fields["Id"] != unit.name() {
        return Err(CompositionError::Controller);
    }
    let job = fields["Job"];
    if job.is_empty() {
        return Ok(None);
    }
    let id = job
        .parse::<u32>()
        .map_err(|_| CompositionError::Controller)?;
    if id == 0 || id.to_string() != job {
        return Err(CompositionError::Controller);
    }
    Ok(Some(id))
}

fn running_pid(output: &[u8], unit: ApplicationUnit) -> Result<u32, CompositionError> {
    if output.len() > MAX_OUTPUT {
        return Err(CompositionError::Controller);
    }
    let text = std::str::from_utf8(output).map_err(|_| CompositionError::Controller)?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or(CompositionError::Controller)?;
        if !RUNNING_PROPERTIES
            .split(',')
            .any(|expected| expected == key)
            || fields.insert(key, value).is_some()
        {
            return Err(CompositionError::Controller);
        }
    }
    if fields.len() != RUNNING_PROPERTIES.split(',').count()
        || fields["Id"] != unit.name()
        || fields["LoadState"] != "loaded"
        || fields["ActiveState"] != "active"
        || fields["SubState"] != "running"
        || fields["ControlPID"] != "0"
        || !fields["Job"].is_empty()
        || fields["Result"] != "success"
    {
        return Err(CompositionError::Controller);
    }
    let pid = fields["MainPID"]
        .parse::<u32>()
        .map_err(|_| CompositionError::Controller)?;
    if pid == 0 || pid > i32::MAX as u32 || fields["MainPID"] != pid.to_string() {
        return Err(CompositionError::Controller);
    }
    Ok(pid)
}

fn parse_stop_observation(output: &[u8]) -> StopObservation {
    if output.len() > MAX_OUTPUT {
        return StopObservation::Unknown;
    }
    let Ok(text) = std::str::from_utf8(output) else {
        return StopObservation::Unknown;
    };
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return StopObservation::Unknown;
        };
        if !PROPERTIES.split(',').any(|expected| expected == key)
            || fields.insert(key, value).is_some()
        {
            return StopObservation::Unknown;
        }
    }
    if fields.len() != PROPERTIES.split(',').count() || fields["LoadState"] != "loaded" {
        return StopObservation::Unknown;
    }
    // MainPID=0 alone cannot exclude surviving child processes. Require the
    // reviewed whole-group kill policy and systemd's released group evidence.
    if fields["KillMode"] != "control-group" || fields["SendSIGKILL"] != "yes" {
        return StopObservation::Unknown;
    }
    for key in ["MainPID", "ControlPID", "ExecMainCode", "ExecMainStatus"] {
        let value = fields[key];
        if value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || value.parse::<u32>().is_err()
        {
            return StopObservation::Unknown;
        }
    }
    if fields["ActiveState"] == "failed" || fields["Result"] != "success" {
        return StopObservation::Failed;
    }
    if fields["MainPID"] != "0"
        || fields["ControlPID"] != "0"
        || !fields["Job"].is_empty()
        || !fields["ControlGroup"].is_empty()
        || matches!(
            fields["ActiveState"],
            "active" | "activating" | "deactivating" | "reloading"
        )
    {
        return StopObservation::RunningOrQueued;
    }
    if fields["ActiveState"] != "inactive" || fields["SubState"] != "dead" {
        return StopObservation::Unknown;
    }
    if !matches!(fields["ExecMainCode"], "0" | "1") || fields["ExecMainStatus"] != "0" {
        return StopObservation::Failed;
    }
    StopObservation::Clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn running(pid: u32) -> String {
        format!(
            "Id=hardened-blikvm-board-owner.service\nLoadState=loaded\nActiveState=active\nSubState=running\nMainPID={pid}\nControlPID=0\nJob=\nResult=success\n"
        )
    }

    #[test]
    fn running_unit_requires_exact_identity_state_and_canonical_pid() {
        let valid = running(42);
        assert_eq!(
            running_pid(valid.as_bytes(), ApplicationUnit::Owner).unwrap(),
            42
        );
        for (from, to) in [
            ("board-owner", "board-server"),
            ("LoadState=loaded", "LoadState=not-found"),
            ("ActiveState=active", "ActiveState=activating"),
            ("SubState=running", "SubState=dead"),
            ("ControlPID=0", "ControlPID=7"),
            ("Job=", "Job=9"),
            ("Result=success", "Result=exit-code"),
            ("MainPID=42", "MainPID=0"),
            ("MainPID=42", "MainPID=+42"),
            ("MainPID=42", "MainPID=042"),
            ("Job=\n", ""),
        ] {
            assert!(
                running_pid(valid.replace(from, to).as_bytes(), ApplicationUnit::Owner).is_err()
            );
        }
        assert!(
            running_pid(
                format!("{valid}MainPID=42\n").as_bytes(),
                ApplicationUnit::Owner
            )
            .is_err()
        );
    }

    #[test]
    fn native_start_dispatch_is_closed_and_reports_failure() {
        use crate::native_identity::{NativeStartUnit, SlotServer};
        for unit in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(SlotServer::Candidate),
            NativeStartUnit::Server(SlotServer::Current),
        ] {
            let (_directory, controller) = fixture(
                &format!(
                    "if test \"$4\" = show; then printf '%s' '{}'; else printf '%s\\n' \"$@\" > \"$0.args\"; fi",
                    startup(unit.unit())
                ),
                Duration::from_secs(2),
            );
            controller.start_native_unit(unit).unwrap();
            let args =
                fs::read_to_string(format!("{}.args", controller.executable.display())).unwrap();
            assert_eq!(
                args.lines().collect::<Vec<_>>(),
                [
                    "--system",
                    "--no-ask-password",
                    "--no-pager",
                    "start",
                    "--",
                    unit.unit().name()
                ]
            );
        }
        let (_directory, controller) = fixture(
            &format!(
                "if test \"$4\" = show; then printf '%s' '{}'; else exit 7; fi",
                startup(ApplicationUnit::Owner)
            ),
            Duration::from_secs(2),
        );
        assert!(matches!(
            controller.start_native_unit(NativeStartUnit::Owner),
            Err(CompositionError::ClientExit { code: Some(7), .. })
        ));
    }

    fn startup(unit: ApplicationUnit) -> String {
        format!(
            "Id={}\nLoadState=loaded\nType=simple\nExecCondition=\nExecStartPre=\nExecStartPost=\nAfter=\nRequires=\nWants=\nRequisite=\nBindsTo=\n",
            unit.name()
        )
    }

    #[test]
    fn native_start_rejects_hooks_and_incomplete_effective_contract_before_dispatch() {
        use crate::native_identity::{NativeStartUnit, SlotServer};
        let unit = NativeStartUnit::Server(SlotServer::Candidate);
        let clean = startup(unit.unit());
        let mut invalid = vec![
            clean.replace(unit.unit().name(), ApplicationUnit::CurrentServer.name()),
            clean.replace("LoadState=loaded", "LoadState=not-found"),
            clean.replace("Type=simple", "Type=oneshot"),
            format!("{clean}Type=simple\n"),
            format!("{clean}Unexpected=yes\n"),
        ];
        for key in ["ExecCondition", "ExecStartPre", "ExecStartPost"] {
            invalid.push(clean.replace(
                &format!("{key}=\n"),
                &format!("{key}={{ path=/legacy/health-reporter ; }}\n"),
            ));
            // systemd 249 omits empty exec hook lists entirely.
            validate_start_contract(
                clean.replace(&format!("{key}=\n"), "").as_bytes(),
                unit.unit(),
            )
            .unwrap();
        }
        for property in START_DEPENDENCIES {
            invalid.push(clean.replace(&format!("{property}=\n"), ""));
            for dependency in LEGACY_UPDATE_UNITS {
                invalid.push(clean.replace(
                    &format!("{property}=\n"),
                    &format!("{property}=network.target {dependency} basic.target\n"),
                ));
            }
        }
        let capture_dependency = clean.replace(
            "Requires=\n",
            "Requires=hardened-blikvm-ustreamer.service\n",
        );
        validate_start_contract(capture_dependency.as_bytes(), unit.unit()).unwrap();
        for output in invalid {
            let (_directory, controller) = fixture(
                &format!(
                    "if test \"$4\" = show; then printf '%s' '{output}'; else : > \"$0.started\"; fi"
                ),
                Duration::from_secs(2),
            );
            let result = controller.start_native_unit(unit);
            assert!(
                matches!(result, Err(CompositionError::IncompatibleStartup(_))),
                "unexpected startup rejection for {output:?}: {result:?}"
            );
            assert!(
                !PathBuf::from(format!("{}.started", controller.executable.display())).exists()
            );
        }
        assert!(validate_start_contract(&[255], unit.unit()).is_err());
    }

    #[test]
    fn systemd_249_start_contract_output_is_accepted_and_reverse_legacy_edges_rejected() {
        // Captured from the H616 test board (systemd 249) for the board daemon.
        let observed = "Type=simple\nId=hardened-blikvm-boardd.service\nRequires=sysinit.target -.mount system.slice\nRequisite=\nWants=tmp.mount\nBindsTo=\nAfter=systemd-tmpfiles-setup.service basic.target tmp.mount sysinit.target system.slice -.mount systemd-journald.socket local-fs.target\nLoadState=loaded\n";
        validate_start_contract(observed.as_bytes(), ApplicationUnit::BoardDaemon).unwrap();
        // An unmasked legacy recover unit adds itself to After= through its
        // own Before=; the native profile must mask it.
        let reverse = observed.replace(
            "After=systemd-tmpfiles-setup.service",
            "After=hardened-blikvm-update-recover.service systemd-tmpfiles-setup.service",
        );
        assert!(validate_start_contract(reverse.as_bytes(), ApplicationUnit::BoardDaemon).is_err());
    }

    #[test]
    fn unit_observation_checks_the_live_executable_in_both_snapshots() {
        let script = format!("printf '%s' '{}'", running(std::process::id()));
        let (_directory, controller) = fixture(&script, Duration::from_secs(2));
        let expected = fs::File::open(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(
            controller
                .observe_unit_identity(ApplicationUnit::Owner, &expected)
                .unwrap()
                .pid,
            std::process::id()
        );
        assert!(
            controller
                .observe_unit_identity(ApplicationUnit::BoardServer, &expected)
                .is_err()
        );
        assert!(
            controller
                .observe_unit_identity(ApplicationUnit::Owner, &tempfile::tempfile().unwrap())
                .is_err()
        );
    }

    #[test]
    fn changing_main_pid_invalidates_the_unit_identity_observation() {
        let pid = std::process::id();
        let script = format!(
            "if test -f \"$0.seen\"; then\n  printf '%s' '{}'\nelse\n  : > \"$0.seen\"\n  printf '%s' '{}'\nfi",
            running(pid + 1),
            running(pid),
        );
        let (_directory, controller) = fixture(&script, Duration::from_secs(2));
        let expected = fs::File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(
            controller
                .observe_unit_identity(ApplicationUnit::Owner, &expected)
                .is_err()
        );
    }
    const CLEAN: &str = "LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nControlPID=0\nJob=\nResult=success\nExecMainCode=1\nExecMainStatus=0\nKillMode=control-group\nSendSIGKILL=yes\nControlGroup=\n";

    #[test]
    fn only_complete_successful_process_free_state_is_clean() {
        assert_eq!(
            parse_stop_observation(CLEAN.as_bytes()),
            StopObservation::Clean
        );
        assert_eq!(
            parse_stop_observation(CLEAN.replace("ExecMainCode=1", "ExecMainCode=0").as_bytes()),
            StopObservation::Clean
        );
        for (from, to, expected) in [
            ("MainPID=0", "MainPID=123", StopObservation::RunningOrQueued),
            (
                "ControlPID=0",
                "ControlPID=123",
                StopObservation::RunningOrQueued,
            ),
            ("Job=", "Job=123", StopObservation::RunningOrQueued),
            (
                "ActiveState=inactive",
                "ActiveState=deactivating",
                StopObservation::RunningOrQueued,
            ),
            ("Result=success", "Result=timeout", StopObservation::Failed),
            (
                "ExecMainStatus=0",
                "ExecMainStatus=1",
                StopObservation::Failed,
            ),
            ("ExecMainCode=1", "ExecMainCode=2", StopObservation::Failed),
            (
                "LoadState=loaded",
                "LoadState=not-found",
                StopObservation::Unknown,
            ),
            ("MainPID=0", "MainPID=+0", StopObservation::Unknown),
            ("Job=\n", "", StopObservation::Unknown),
            (
                "KillMode=control-group",
                "KillMode=process",
                StopObservation::Unknown,
            ),
            (
                "KillMode=control-group",
                "KillMode=none",
                StopObservation::Unknown,
            ),
            (
                "SendSIGKILL=yes",
                "SendSIGKILL=no",
                StopObservation::Unknown,
            ),
            ("ControlGroup=\n", "", StopObservation::Unknown),
            (
                "ControlGroup=",
                "ControlGroup=/system.slice/remaining.service",
                StopObservation::RunningOrQueued,
            ),
        ] {
            assert_eq!(
                parse_stop_observation(CLEAN.replace(from, to).as_bytes()),
                expected,
                "{to}"
            );
        }
        assert_eq!(
            parse_stop_observation(format!("{CLEAN}MainPID=0\n").as_bytes()),
            StopObservation::Unknown
        );
        assert_eq!(parse_stop_observation(b""), StopObservation::Unknown);
        assert_eq!(parse_stop_observation(&[255]), StopObservation::Unknown);
    }

    fn fixture(script: &str, budget: Duration) -> (tempfile::TempDir, SystemdController) {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("systemctl-fixture");
        fs::write(&executable, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut controller = SystemdController::new(Instant::now() + budget).unwrap();
        controller.executable = executable;
        controller.shell_fixture = true;
        (directory, controller)
    }

    #[test]
    fn command_arguments_and_clean_observation_use_closed_units() {
        // Static shell fixture checks argv without invoking systemctl or services.
        let script = format!(
            r#"
test "$LC_ALL" = C
test "$1" = --system
test "$2" = --no-ask-password
test "$3" = --no-pager
shift 3
case "$1" in
  --no-block)
    test "$#" = 4
    test "$2" = stop
    test "$3" = --
    test "$4" = hardened-blikvm-board-owner.service
    ;;
  show)
    test "$#" = 6
    test "$2" = --all
    test "$3" = --property
    test "$4" = '{PROPERTIES}'
    test "$5" = --
    test "$6" = hardened-blikvm-board-owner.service
    printf '%s' '{CLEAN}'
    ;;
  *) exit 1 ;;
esac
"#
        );
        let (_directory, mut controller) = fixture(&script, Duration::from_secs(2));
        controller.stop(ApplicationUnit::Owner).unwrap();
        assert_eq!(
            controller.observe_stop(ApplicationUnit::Owner).unwrap(),
            StopObservation::Clean
        );
    }

    #[test]
    fn queued_jobs_are_cancelled_and_reobserved() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("cancelled");
        let script = format!(
            r#"
shift 3
case "$1" in
  show)
    test "$4" = '{JOB_PROPERTIES}'
    test "$6" = hardened-blikvm-board-owner.service
    if test -e '{marker}'; then
      printf 'Id=hardened-blikvm-board-owner.service\nJob=\n'
    else
      printf 'Id=hardened-blikvm-board-owner.service\nJob=12\n'
    fi
    ;;
  cancel)
    test "$#" = 3
    test "$3" = 12
    : > '{marker}'
    ;;
  *) exit 1 ;;
esac
"#,
            marker = marker.display()
        );
        let (_fixture, controller) = fixture(&script, Duration::from_secs(2));
        controller
            .cancel_queued_job(ApplicationUnit::Owner)
            .unwrap();
        assert!(marker.exists());

        // A job that never leaves is reported at the deadline, not ignored.
        let stuck = "shift 3\ncase \"$1\" in show) printf 'Id=hardened-blikvm-board-owner.service\\nJob=12\\n' ;; *) exit 5 ;; esac";
        let (_fixture, controller) = fixture(stuck, Duration::from_millis(300));
        assert!(matches!(
            controller.cancel_queued_job(ApplicationUnit::Owner),
            Err(CompositionError::Deadline)
        ));

        for output in [
            "Id=hardened-blikvm-board-owner.service\nJob=012\n",
            "Id=hardened-blikvm-board-owner.service\nJob=0\n",
            "Id=ssh.service\nJob=\n",
            "Id=hardened-blikvm-board-owner.service\n",
            "Id=hardened-blikvm-board-owner.service\nJob=\nJob=\n",
        ] {
            assert!(
                queued_job(output.as_bytes(), ApplicationUnit::Owner).is_err(),
                "{output}"
            );
        }
    }

    #[test]
    fn failed_and_oversized_clients_are_rejected() {
        for (script, expected_code, expected_signal) in [
            ("exit 7", Some(7), None),
            ("kill -TERM \"$$\"", None, Some(libc::SIGTERM)),
        ] {
            let (_directory, controller) = fixture(script, Duration::from_secs(2));
            match controller.run(&[]) {
                Err(CompositionError::ClientExit { code, signal }) => {
                    assert_eq!(code, expected_code);
                    assert_eq!(signal, expected_signal);
                }
                result => panic!("unexpected client result: {result:?}"),
            }
        }
        let (_directory, controller) = fixture(
            "while :; do printf '0123456789'; done",
            Duration::from_secs(2),
        );
        assert!(matches!(
            controller.run(&[]),
            Err(CompositionError::ClientOutputLimit)
        ));
    }

    #[test]
    fn spawn_failure_preserves_os_cause_without_dispatch_retry() {
        let (_directory, mut controller) = fixture("exit 0", Duration::from_secs(2));
        controller.executable.set_file_name("missing-client");
        controller.shell_fixture = false;
        match controller.stop(ApplicationUnit::Owner) {
            Err(CompositionError::ClientIo { operation, source }) => {
                assert_eq!(operation, "spawn");
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            result => panic!("unexpected spawn result: {result:?}"),
        }
    }

    #[test]
    fn hung_client_is_killed_and_reaped_on_deadline() {
        let (directory, controller) = fixture(
            "printf '%s' \"$$\" > \"$0.pid\"\nexec /bin/sleep 5",
            Duration::from_millis(150),
        );
        let start = Instant::now();
        assert!(matches!(
            controller.run(&[]),
            Err(CompositionError::Deadline)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        let pid: libc::pid_t = fs::read_to_string(directory.path().join("systemctl-fixture.pid"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: only queries the fixture child PID; the status pointer is
        // explicitly permitted to be null. ECHILD proves our guard reaped it.
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn repeated_operations_share_the_original_deadline() {
        let (_directory, controller) = fixture("exec /bin/sleep 0.08", Duration::from_millis(350));
        let start = Instant::now();
        controller.run(&[]).unwrap();
        let mut expired = false;
        for _ in 0..8 {
            match controller.run(&[]) {
                Err(CompositionError::Deadline) => {
                    expired = true;
                    break;
                }
                result => {
                    result.unwrap();
                }
            }
        }
        assert!(expired);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn running_observations_are_bounded_and_invalid_deadlines_rejected() {
        assert!(SystemdController::new(Instant::now()).is_err());
        assert!(SystemdController::new(Instant::now() + Duration::from_secs(46)).is_err());
        let script = format!("printf '%s' '{}'", CLEAN.replace("MainPID=0", "MainPID=42"));
        let (_directory, mut controller) = fixture(&script, Duration::from_millis(150));
        let result = controller.observe_stop(ApplicationUnit::Owner);
        assert!(
            matches!(result, Err(CompositionError::Deadline)),
            "unexpected stop observation: {result:?}"
        );
    }
}
