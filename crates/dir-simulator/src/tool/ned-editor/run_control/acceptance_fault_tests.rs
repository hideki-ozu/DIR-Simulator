//! Actual subprocess, kernel-failure and persistence-cut acceptance evidence.
//! Run explicitly and serially, as in release-audit CI. Fork/exec briefly
//! inherits other libtests' CLOEXEC flock descriptors; ordinary fixture
//! operations must not run concurrently with this process-cut matrix.
use super::*;
use crate::acceptance_hooks as hooks;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicU64;

const CHILD_MODE: &str = "DIR_ACCEPTANCE_MODE";
const NAMESPACE_MODE: &str = "DIR_ACCEPTANCE_NAMESPACE_MODE";
static NEXT: AtomicU64 = AtomicU64::new(0);

fn sudo_mount_namespace() -> bool {
    match std::env::var(NAMESPACE_MODE) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(mode) if mode == "sudo-mount" => true,
        mode => panic!("Unsupported {NAMESPACE_MODE}: {mode:?}"),
    }
}

fn drop_privileges(command: &mut Command, uid: u32, gid: u32) {
    command.args([
        "setpriv",
        &format!("--reuid={uid}"),
        &format!("--regid={gid}"),
        "--clear-groups",
        "--bounding-set=-all",
        "--inh-caps=-all",
        "--ambient-caps=-all",
        "--no-new-privs",
        "--",
    ]);
}

fn namespace_command(pid: &str, sudo_mount: bool) -> Command {
    let mut command = if sudo_mount {
        let mut command = Command::new("sudo");
        command.args(["-n", "--", "nsenter"]);
        command
    } else {
        Command::new("nsenter")
    };
    command.args(["--target", pid]);
    if !sudo_mount {
        command.args(["--user", "--preserve-credentials"]);
    }
    command.args(["--mount", "--"]);
    command
}
struct Fixture {
    root: PathBuf,
    project: ProjectSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ned-fault-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(root.join("source/models/demo")).unwrap();
        fs::create_dir(root.join("exports")).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (source, target) in [
            ("examples/can/baseline.ini", "project.ini"),
            ("examples/can/baseline.json", "baseline.json"),
            ("examples/can/models/demo/Main.ned", "models/demo/Main.ned"),
        ] {
            fs::copy(repo.join(source), root.join("source").join(target)).unwrap();
        }
        let project = load(&root);
        Self { root, project }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            evidence(json!({"failed_fixture_retained":self.root}));
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn load(root: &Path) -> ProjectSnapshot {
    super::super::input::load_project(&root.join("source/project.ini"), &root.join("source"))
        .unwrap()
}
fn stamp(p: &ProjectSnapshot, command: &str) -> RunStamp {
    RunStamp {
        session_id: "acceptance-session".into(),
        command_id: command.into(),
        project_instance_id: "acceptance-project".into(),
        revision: "1".into(),
        input_revision: "1".into(),
        snapshot_id: p.id.clone(),
        input_digest: p.input_digest(),
        origin: json!({"kind":"disk"}),
        dirty_at_start: false,
    }
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, (String, u64, u64)> {
    fn walk(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (String, u64, u64)>) {
        if !path.exists() {
            return;
        }
        let st = fs::symlink_metadata(path).unwrap();
        if st.is_dir() {
            for e in fs::read_dir(path).unwrap() {
                walk(base, &e.unwrap().path(), out);
            }
        } else {
            let bytes = if st.file_type().is_symlink() {
                fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec()
            } else {
                fs::read(path).unwrap()
            };
            out.insert(
                path.strip_prefix(base).unwrap().into(),
                (hash(&bytes), st.dev(), st.ino()),
            );
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn content(root: &Path) -> BTreeMap<PathBuf, String> {
    inventory(root)
        .into_iter()
        .map(|(p, (h, _, _))| (p, h))
        .collect()
}
fn evidence(value: Value) {
    println!("FAULT_EVIDENCE {value}");
    if let Some(path) = std::env::var_os("DIR_ACCEPTANCE_EVIDENCE") {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{value}").unwrap();
    }
}
struct Supervisor {
    child: Child,
    listener: UnixListener,
    socket: PathBuf,
    sudo_namespace: bool,
}
impl Supervisor {
    fn spawn(root: &Path, mode: &str, site: &str, occurrence: &str, namespace: bool) -> Self {
        let socket = root.join("barrier.sock");
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let binary = std::env::current_exe().unwrap();
        let sudo_namespace = namespace && sudo_mount_namespace();
        let variables = [
            (CHILD_MODE, std::ffi::OsString::from(mode)),
            ("DIR_ACCEPTANCE_ROOT", root.as_os_str().to_owned()),
            ("DIR_ACCEPTANCE_SITE", site.into()),
            ("DIR_ACCEPTANCE_OCCURRENCE", occurrence.into()),
            ("DIR_ACCEPTANCE_SOCKET", socket.as_os_str().to_owned()),
            (hooks::WORKERS, "1".into()),
        ];
        let mut cmd = if sudo_namespace {
            // CI runners may prohibit uid_map while permitting a private mount
            // namespace through their existing passwordless sudo policy.
            let identity = fs::metadata("/proc/self").unwrap();
            assert_ne!(identity.uid(), 0, "Run the test harness as the runner user");
            let mut c = Command::new("sudo");
            c.args([
                "-n",
                "--",
                "unshare",
                "--mount",
                "--propagation",
                "private",
                "--",
            ]);
            drop_privileges(&mut c, identity.uid(), identity.gid());
            // Pass only these test controls after dropping privilege. sudo -E
            // would carry unrelated runner environment into privileged tools.
            c.arg("env");
            for (key, value) in &variables {
                let mut assignment = std::ffi::OsString::from(key);
                assignment.push("=");
                assignment.push(value);
                c.arg(assignment);
            }
            c.arg(&binary);
            c.process_group(0);
            c
        } else if namespace {
            let mut c = Command::new("unshare");
            c.args(["-Ur", "-m", "--propagation", "private", "--"]);
            c.arg(&binary);
            c
        } else {
            Command::new(&binary)
        };
        cmd.args(["--exact", hooks::ENTRY, "--nocapture"])
            .envs(variables)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        Self {
            child: cmd.spawn().unwrap(),
            listener,
            socket,
            sudo_namespace,
        }
    }
    fn next(&mut self) -> Option<(Value, UnixStream)> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut line = String::new();
                    BufReader::new(stream.try_clone().unwrap())
                        .read_line(&mut line)
                        .unwrap();
                    return Some((serde_json::from_str(&line).unwrap(), stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("barrier accept: {e}"),
            }
            if self.child.try_wait().unwrap().is_some() {
                return None;
            }
            assert!(
                Instant::now() < deadline,
                "child never reached acknowledged barrier"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn kill(&mut self) -> std::process::ExitStatus {
        let pid = rustix::process::Pid::from_raw(self.child.id() as i32).unwrap();
        let fd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
        rustix::process::pidfd_send_signal(&fd, rustix::process::Signal::Kill).unwrap();
        let exit = self.child.wait().unwrap();
        assert_eq!(exit.signal(), Some(9));
        exit
    }
    fn finish(&mut self) {
        assert!(
            self.child.wait().unwrap().success(),
            "child scenario failed"
        );
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        if self.sudo_namespace && self.child.try_wait().ok().flatten().is_none() {
            // The wrapper has its own group; kill only that group on assertion
            // failure. The real worker also has its normal parent-death signal.
            let _ = Command::new("sudo")
                .args([
                    "-n",
                    "--",
                    "kill",
                    "-KILL",
                    "--",
                    &format!("-{}", self.child.id()),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket);
    }
}
fn release(stream: &mut UnixStream) {
    stream.write_all(b"R").unwrap();
}
fn close(m: RunManager) {
    m.shutdown();
    let root = m.shared.root.clone();
    drop(m);
    let fd = Directory::open(&root)
        .unwrap()
        .open_file(".coordinator.lock")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rustix::fs::flock(&fd, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => break,
            Err(e) if e == rustix::io::Errno::WOULDBLOCK => {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            Err(e) => panic!("{e}"),
        }
    }
}

// Only the test subprocess uses this launch transport. Real worker execution
// and frame codecs are unchanged; libtest stdout never enters worker framing.
pub(super) fn spawn_worker(request: &worker::Request) -> Result<(Child, Instant)> {
    let root = PathBuf::from(std::env::var_os("DIR_ACCEPTANCE_ROOT").unwrap());
    let socket = root.join(format!(
        "protocol-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = UnixListener::bind(&socket).map_err(|e| invalid(e.to_string()))?;
    listener.set_nonblocking(true).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", hooks::ENTRY, "--nocapture"])
        .env_clear()
        .env(CHILD_MODE, "worker")
        .env("DIR_ACCEPTANCE_PROTOCOL", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hooks::propagate(&mut cmd);
    if std::env::var(CHILD_MODE).as_deref() == Ok("logs") {
        cmd.env("DIR_ACCEPTANCE_LOG_PATTERN", "1");
    }
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| invalid(e.to_string()))?;
    if !request.probe {
        let mut launches = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("execution-launches.jsonl"))
            .map_err(|e| invalid(e.to_string()))?;
        writeln!(
            launches,
            "{}",
            json!({"run_id":request.run_id,"pid":child.id(),"launched_ms":now_ms()})
        )
        .map_err(|e| invalid(e.to_string()))?;
    }
    let stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait().unwrap().is_some() || started.elapsed() > Duration::from_secs(5)
                {
                    return Err(invalid("test worker transport did not connect"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(invalid(e.to_string())),
        }
    };
    fs::remove_file(&socket).unwrap();
    child.stdin = Some(std::process::ChildStdin::from(OwnedFd::from(
        stream.try_clone().unwrap(),
    )));
    child.stdout = Some(std::process::ChildStdout::from(OwnedFd::from(stream)));
    Ok((child, started))
}

fn manager(root: &Path, project: &ProjectSnapshot) -> RunManager {
    let binary = std::env::current_exe().unwrap();
    let m = RunManager::open_impl(&root.join("state"), &project.input_regions(), Some(&binary))
        .unwrap();
    assert!(m.shared.state.lock().unwrap().worker_error.is_none());
    m
}
fn save(root: &Path, restore: bool) {
    let project = load(root);
    let mut registry =
        super::super::output::TargetRegistry::open(&root.join("exports"), &root.join("save-state"))
            .unwrap();
    registry.register_source(&project).unwrap();
    let mut model = super::super::model::EditorSession::new(project);
    let candidate=super::super::file_creation::plan_create_file(&model,&json!({"role":"ned","root_id":"ned-root-0","relative_path":"extra/nested/Added.ned","template_id":"ned.module_empty","declaration":{"kind":"module","local_name":"Added"},"format":{"newline":"LF","bom":false}})).unwrap();
    model
        .commit_with_files(candidate.text, vec![], candidate.added)
        .unwrap();
    let ids: Vec<_> = model
        .project
        .files
        .values()
        .map(|f| json!({"id":f.id,"path":f.path,"hash":f.hash}))
        .collect();
    fs::write(
        root.join("input-identities.json"),
        serde_json::to_vec(&ids).unwrap(),
    )
    .unwrap();
    let target = registry.register_destination("saved").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan = super::super::output::build_plan(
        &model.project,
        model.revision,
        model.input_revision,
        &target,
        &registry,
    )
    .unwrap();
    if restore {
        super::super::output::inject_failure("after_publish");
    }
    let mut report = super::super::output::save_plan(plan, &mut registry, None, false);
    if restore {
        report = super::super::output::recover(
            &mut registry,
            report.recovery_id.as_ref().unwrap(),
            "restore",
        );
    }
    assert_eq!(
        report.state,
        if restore {
            super::super::output::SaveOutcome::Restored
        } else {
            super::super::output::SaveOutcome::Complete
        },
        "{:?}",
        report.error
    );
    let after: Vec<_> = model
        .project
        .files
        .values()
        .map(|f| json!({"id":f.id,"path":f.path,"hash":f.hash}))
        .collect();
    assert_eq!(ids, after);
    fs::write(
        root.join("save-report.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn fault_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    if mode == "worker" {
        let mut stream =
            UnixStream::connect(std::env::var_os("DIR_ACCEPTANCE_PROTOCOL").unwrap()).unwrap();
        let request: worker::Request = worker::read_frame(&mut stream).unwrap();
        let logs = !request.probe && std::env::var_os("DIR_ACCEPTANCE_LOG_PATTERN").is_some();
        if logs {
            std::io::stderr().write_all(&log_pattern()).unwrap();
        }
        worker::acceptance_execute(request, &mut stream).unwrap();
        if logs {
            stream.write_all(&log_pattern()).unwrap();
        }
        return;
    }
    let root = PathBuf::from(std::env::var_os("DIR_ACCEPTANCE_ROOT").unwrap());
    if mode == "nonowner" {
        let project = load(&root);
        let other = RunManager::open(&root.join("state"), &project.input_regions()).unwrap();
        assert!(!other.shared.owner);
        let id = fs::read_to_string(root.join("run-id")).unwrap();
        let read = other.get(&id).unwrap();
        let error = other.cancel(&id).unwrap_err();
        let mut editor = super::super::model::EditorSession::new(project);
        let file = editor.project.ned_files().next().unwrap().clone();
        editor
            .commit(
                vec![(
                    file.id.clone(),
                    format!("{}\n// non-owner service edit\n", file.text),
                )],
                vec![],
            )
            .unwrap();
        assert!(editor.dirty());
        fs::write(root.join("nonowner.json"),serde_json::to_vec(&json!({"pid":std::process::id(),"read":read,"error":error,"normal_edit_dirty":editor.dirty()})).unwrap()).unwrap();
        return;
    }
    if mode == "save" || mode == "restore" {
        save(&root, mode == "restore");
        return;
    }
    if mode == "reopen" || mode == "reopen-delete" {
        let p = load(&root);
        let m = manager(&root, &p);
        let records = m.list(None).unwrap();
        let mut deleted = Vec::new();
        if mode == "reopen-delete" {
            for run in records["runs"].as_array().unwrap() {
                let id = run["run_id"].as_str().unwrap();
                m.delete(id).unwrap();
                deleted.push(id.to_owned());
            }
            assert!(m.list(None).unwrap()["runs"].as_array().unwrap().is_empty());
        }
        fs::write(
            root.join("reopened.json"),
            serde_json::to_vec(
                &json!({"worker_launching_enabled":m.shared.launch_worker,"records":records,"deleted_run_ids":deleted}),
            )
            .unwrap(),
        )
        .unwrap();
        close(m);
        return;
    }
    if mode.starts_with("rlimit-") {
        resource_probe(&root, &mode);
        return;
    }
    let mut p = load(&root);
    if ["partial", "output-failure"].contains(&mode.as_str()) {
        let config = p
            .files
            .values_mut()
            .find(|f| f.roles.contains(&super::super::input::FileRole::Config))
            .unwrap();
        config.text = format!("{}\nmax-events = 1\n", config.text).into();
        config.hash = hash(config.text.as_bytes());
        p.refresh_inputs();
    }
    let m = manager(&root, &p);
    let _ = fs::remove_file(root.join("controls.sock"));
    let controls = UnixListener::bind(root.join("controls.sock")).unwrap();
    controls.set_nonblocking(true).unwrap();
    let id = m
        .create_run(
            &p,
            stamp(&p, if mode == "run-b" { "second" } else { "first" }),
            if mode == "timeout" { 1000 } else { 600000 },
        )
        .unwrap();
    fs::write(root.join("run-id"), &id).unwrap();
    hooks::hit("run.after_create_ack");
    if mode == "cancel-persist" {
        m.cancel(&id).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    let r = loop {
        match controls.accept() {
            Ok((mut stream, _)) => {
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let command: Value = serde_json::from_str(&line).unwrap();
                let target = command["id"].as_str().unwrap_or(&id);
                let reply = match command["action"].as_str().unwrap() {
                    "cancel" => m.cancel(target).unwrap(),
                    "get" => m.get(target).unwrap(),
                    "create" => match m.create_run(
                        &p,
                        stamp(&p, command["command"].as_str().unwrap()),
                        600000,
                    ) {
                        Ok(run) => json!({"id":run}),
                        Err(e) => json!({"error":e}),
                    },
                    "nonowner" => {
                        let exit = Command::new(std::env::current_exe().unwrap())
                            .args(["--exact", hooks::ENTRY, "--nocapture"])
                            .env_clear()
                            .env(CHILD_MODE, "nonowner")
                            .env("DIR_ACCEPTANCE_ROOT", &root)
                            .stdout(Stdio::null())
                            .status()
                            .unwrap();
                        assert!(exit.success());
                        serde_json::from_slice(&fs::read(root.join("nonowner.json")).unwrap())
                            .unwrap()
                    }
                    "edit-save" => {
                        let mut editor = super::super::model::EditorSession::new(p.clone());
                        let file = editor.project.ned_files().next().unwrap().clone();
                        editor
                            .commit(
                                vec![(
                                    file.id.clone(),
                                    format!("{}\n// edited while run active\n", file.text),
                                )],
                                vec![],
                            )
                            .unwrap();
                        assert!(editor.dirty());
                        save(&root, false);
                        json!({"dirty":editor.dirty(),"run_source":m.source(&id,&file.id).unwrap()})
                    }
                    other => panic!("unknown control {other}"),
                };
                writeln!(stream, "{reply}").unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("{e}"),
        }
        let current = m.record(&id).unwrap();
        if !current.active() {
            break current;
        }
        assert!(Instant::now() < deadline, "controlled worker did not end");
        std::thread::sleep(Duration::from_millis(5));
    };
    fs::write(
        root.join("terminal.json"),
        serde_json::to_vec(&r.value()).unwrap(),
    )
    .unwrap();
    close(m);
}

fn log_pattern() -> Vec<u8> {
    (0..LOG_LIMIT * 2 + 123).map(|n| (n % 251) as u8).collect()
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc34_actual_worker_stdout_stderr_tails_keep_exact_one_mib_suffix() {
    let f = Fixture::new();
    let mut child = Supervisor::spawn(&f.root, "logs", "unused", "0", false);
    assert!(child.next().is_none());
    child.finish();
    let run = fs::read_to_string(f.root.join("run-id")).unwrap();
    let control = f.root.join("state/runs").join(run).join("control");
    let pattern = log_pattern();
    let suffix = &pattern[pattern.len() - LOG_LIMIT..];
    for file in ["stdout.log", "stderr.log"] {
        let actual = fs::read(control.join(file)).unwrap();
        assert_eq!(actual, suffix, "{file}");
        evidence(
            json!({"case":"XC34","real_worker_log":file,"input_bytes":pattern.len(),"retained_bytes":actual.len(),"sha256":hash(&actual)}),
        );
    }
}

fn control(root: &Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(root.join("controls.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc31_real_enospc_and_eacces_after_execution_preserve_original_diagnostics() {
    for failure in ["eacces", "enospc"] {
        let f = Fixture::new();
        let input = inventory(&f.root.join("source"));
        let mut child = Supervisor::spawn(
            &f.root,
            "output-failure",
            "output.export_entry",
            "0",
            failure == "enospc",
        );
        let (event, mut stream) = child
            .next()
            .expect("execution must reach actual export entry");
        let output = PathBuf::from(event["path"].as_str().unwrap());
        let kernel = if failure == "eacces" {
            fs::set_permissions(&output, fs::Permissions::from_mode(0o500)).unwrap();
            let error = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output.join("permission-probe"))
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(13));
            json!({"errno":13})
        } else {
            let pid = event["pid"].as_u64().unwrap().to_string();
            assert!(output.starts_with(f.root.join("state/runs")));
            assert_eq!(output.file_name().unwrap(), "result");
            let namespace = fs::read_link(format!("/proc/{pid}/ns/mnt")).unwrap();
            assert_ne!(namespace, fs::read_link("/proc/self/ns/mnt").unwrap());
            let mountinfo = fs::read_to_string(format!("/proc/{pid}/mountinfo")).unwrap();
            assert!(
                !mountinfo.lines().any(|line| line.contains(" shared:")),
                "Test mount namespace must have private propagation"
            );
            let sudo_mount = sudo_mount_namespace();
            let identity = fs::metadata("/proc/self").unwrap();
            if sudo_mount {
                // Verify the acknowledged actual worker, not sudo's wrapper PID.
                let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
                for (name, expected) in [("Uid:", identity.uid()), ("Gid:", identity.gid())] {
                    let values = status
                        .lines()
                        .find(|line| line.starts_with(name))
                        .unwrap()
                        .split_whitespace()
                        .skip(1)
                        .map(|value| value.parse::<u32>().unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(values, vec![expected; 4]);
                }
                for name in ["CapInh:", "CapPrm:", "CapEff:", "CapBnd:", "CapAmb:"] {
                    let value = status
                        .lines()
                        .find(|line| line.starts_with(name))
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap();
                    assert_eq!(u64::from_str_radix(value, 16).unwrap(), 0, "{name}");
                }
                for (name, expected) in [("NoNewPrivs:", Some("1")), ("Groups:", None)] {
                    let value = status
                        .lines()
                        .find(|line| line.starts_with(name))
                        .unwrap()
                        .split_whitespace()
                        .nth(1);
                    assert_eq!(value, expected, "{name}");
                }
            }
            let options = if sudo_mount {
                format!(
                    "size=4096,mode=0700,uid={},gid={}",
                    identity.uid(),
                    identity.gid()
                )
            } else {
                "size=4096,mode=0700".into()
            };
            let mount = namespace_command(&pid, sudo_mount)
                .args(["mount", "-t", "tmpfs", "-o", &options, "tmpfs"])
                .arg(&output)
                .output()
                .unwrap();
            assert!(
                mount.status.success(),
                "private tmpfs mount: {}",
                String::from_utf8_lossy(&mount.stderr)
            );
            let mut fill = namespace_command(&pid, sudo_mount);
            if sudo_mount {
                drop_privileges(&mut fill, identity.uid(), identity.gid());
            }
            let fill=fill.args(["python3","-c","import os,sys,json\nf=open(sys.argv[1]+'/filler','wb',buffering=0)\ntry:\n while True:f.write(b'x'*4096)\nexcept OSError as e:\n assert e.errno==28;print(json.dumps({'errno':e.errno}))"]).arg(&output).output().unwrap();
            assert!(
                fill.status.success(),
                "{}",
                String::from_utf8_lossy(&fill.stderr)
            );
            let mut kernel: Value = serde_json::from_slice(&fill.stdout).unwrap();
            kernel["namespace_mode"] = json!(if sudo_mount {
                "sudo-mount"
            } else {
                "user-mount"
            });
            kernel["private_mount_namespace"] = json!(true);
            if sudo_mount {
                kernel["worker_uid"] = json!(identity.uid());
                kernel["worker_gid"] = json!(identity.gid());
                kernel["worker_capabilities_cleared"] = json!(true);
            }
            kernel
        };
        release(&mut stream);
        assert!(child.next().is_none());
        child.finish();
        if failure == "eacces" {
            fs::set_permissions(&output, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let terminal: Value =
            serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
        assert_eq!(terminal["phase"], "output_failed", "{terminal}");
        assert_eq!(terminal["artifact_state"], "unpublished");
        assert_eq!(terminal["viewer_state"], "absent");
        assert_eq!(terminal["original_termination"], "execution_failed");
        let diagnostics = terminal["diagnostics"].as_array().unwrap();
        assert!(diagnostics.len() >= 2, "{terminal}");
        assert_eq!(diagnostics[0]["diagnostic"]["reason"], "event_limit");
        assert_eq!(diagnostics.last().unwrap()["diagnostic"]["code"], "E-0003");
        assert!(
            diagnostics.last().unwrap()["diagnostic"]["message"]
                .as_str()
                .unwrap()
                .contains(&format!("os error {}", kernel["errno"]))
        );
        assert!(!output.join("manifest.json").exists());
        assert_eq!(inventory(&f.root.join("source")), input);
        evidence(
            json!({"case":"XC31","failure":failure,"barrier":event,"kernel":kernel,"terminal":terminal}),
        );
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc33_real_cancellation_at_prepare_execute_export_and_completion_barriers() {
    for site in [
        "worker.before_prepare",
        "worker.before_execute",
        "output.export_entry",
        "output.after_manifest",
        "worker.after_response",
        "run.before_completion_lock",
        "run.verifying_unlocked",
    ] {
        let f = Fixture::new();
        let mut child = Supervisor::spawn(&f.root, "controlled", site, "0", false);
        let (event, mut stream) = child.next().expect("selected cancellation phase");
        let ack = control(&f.root, json!({"action":"cancel"}));
        if site == "run.verifying_unlocked" {
            assert_eq!(ack["phase"], "verifying");
            assert!(ack["stop_reason"].is_null());
        } else {
            assert_eq!(ack["stop_reason"], "cancel");
        }
        // The worker may be killed by accepted cancellation before release.
        if ["run.before_completion_lock", "run.verifying_unlocked"].contains(&site) {
            let _ = stream.write_all(b"R");
        }
        assert!(child.next().is_none());
        child.finish();
        let terminal: Value =
            serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
        assert_eq!(terminal["worker_exit"]["reaped"], true);
        if [
            "output.after_manifest",
            "worker.after_response",
            "run.before_completion_lock",
            "run.verifying_unlocked",
        ]
        .contains(&site)
        {
            assert_eq!(terminal["phase"], "completed", "{site}: {terminal}");
            assert_eq!(terminal["artifact_state"], "published");
            if !terminal["report"].is_null() {
                assert_eq!(terminal["report"]["partial"], false);
            }
        } else {
            assert_eq!(terminal["phase"], "cancelled", "{site}: {terminal}");
            assert_eq!(terminal["artifact_state"], "unpublished");
        }
        evidence(json!({"case":"XC33","barrier":event,"cancel_ack":ack,"terminal":terminal}));
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc34_real_timeout_soft_storage_monitor_and_exact_log_tail() {
    for kind in ["timeout", "run-soft-limit", "runs-soft-limit"] {
        let f = Fixture::new();
        let mut child = Supervisor::spawn(
            &f.root,
            if kind == "timeout" {
                "timeout"
            } else {
                "controlled"
            },
            "worker.before_execute",
            "0",
            false,
        );
        let (event, _stream) = child
            .next()
            .expect("worker must acknowledge paused execution");
        let run = fs::read_to_string(f.root.join("run-id")).unwrap();
        if kind != "timeout" {
            let path = if kind == "run-soft-limit" {
                f.root
                    .join("state/runs")
                    .join(&run)
                    .join("result/overshoot")
            } else {
                f.root.join("state/runs/overshoot")
            };
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            file.set_len(if kind == "run-soft-limit" {
                RUN_LIMIT + 1
            } else {
                RUNS_LIMIT + 1
            })
            .unwrap();
            assert!(file.metadata().unwrap().blocks() < 1024);
        }
        // Keep the acknowledged worker paused: actual coordinator timeout /
        // one-second metadata monitor sends TERM and reaps it.
        assert!(child.next().is_none());
        child.finish();
        let terminal: Value =
            serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
        assert_eq!(
            terminal["phase"],
            if kind == "timeout" {
                "timed_out"
            } else {
                "resource_limited"
            },
            "{terminal}"
        );
        assert_eq!(terminal["worker_exit"]["reaped"], true);
        assert_eq!(
            terminal["stop_reason"],
            if kind == "timeout" {
                "timeout"
            } else {
                "resource_limit"
            }
        );
        if kind != "timeout" {
            assert_eq!(
                terminal["resource_observation"]["soft_limit_overshoot"],
                true
            );
        }
        evidence(json!({"case":"XC34","kind":kind,"barrier":event,"terminal":terminal}));
    }
    let mut data = vec![b'A'; LOG_LIMIT + 123];
    data.extend((0..LOG_LIMIT).map(|n| (n % 251) as u8));
    let retained = stderr_ring(std::io::Cursor::new(&data));
    assert_eq!(retained.len(), LOG_LIMIT);
    assert_eq!(retained, &data[data.len() - LOG_LIMIT..]);
    evidence(
        json!({"case":"XC34","log_input_bytes":data.len(),"retained_bytes":retained.len(),"retained_sha256":hash(&retained)}),
    );
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc37_one_actual_worker_two_waiters_nonowner_and_concurrent_edit_save() {
    let f = Fixture::new();
    let input = inventory(&f.root.join("source"));
    let mut child = Supervisor::spawn(&f.root, "controlled", "worker.before_execute", "0", false);
    let (event, mut stream) = child.next().unwrap();
    let second = control(&f.root, json!({"action":"create","command":"second"}));
    let third = control(&f.root, json!({"action":"create","command":"third"}));
    assert!(second["id"].is_string());
    assert!(third["id"].is_string());
    assert_eq!(
        control(&f.root, json!({"action":"get","id":second["id"]}))["phase"],
        "queued"
    );
    assert_eq!(
        control(&f.root, json!({"action":"get","id":third["id"]}))["phase"],
        "queued"
    );
    let full = control(&f.root, json!({"action":"create","command":"fourth"}));
    assert_eq!(full["error"]["status"], 503);
    assert_eq!(
        fs::read_dir(f.root.join("state/runs"))
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        3,
        "rejection must precede another freeze directory"
    );
    let other = control(&f.root, json!({"action":"nonowner"}));
    assert_eq!(other["error"]["status"], 503);
    assert!(other["read"]["phase"].is_string());
    assert_eq!(other["normal_edit_dirty"], true);
    assert_ne!(other["pid"], json!(child.child.id()));
    let saved = control(&f.root, json!({"action":"edit-save"}));
    assert_eq!(saved["dirty"], true);
    assert!(f.root.join("exports/saved/project.ini").is_file());
    let cancelled = control(&f.root, json!({"action":"cancel","id":second["id"]}));
    assert_eq!(cancelled["phase"], "cancelled");
    let replacement = control(&f.root, json!({"action":"create","command":"replacement"}));
    assert!(replacement["id"].is_string());
    for id in [&third["id"], &replacement["id"]] {
        assert_eq!(
            control(&f.root, json!({"action":"cancel","id":id}))["phase"],
            "cancelled"
        );
    }
    release(&mut stream);
    assert!(child.next().is_none());
    child.finish();
    let terminal: Value =
        serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
    assert_eq!(terminal["phase"], "completed");
    assert_eq!(terminal["worker_exit"]["reaped"], true);
    let launches = fs::read_to_string(f.root.join("execution-launches.jsonl")).unwrap();
    assert_eq!(
        launches.lines().count(),
        1,
        "only the running worker may launch; both waiters were cancelled"
    );
    let launch: Value = serde_json::from_str(launches.lines().next().unwrap()).unwrap();
    assert_eq!(launch["pid"], event["pid"]);
    assert_eq!(inventory(&f.root.join("source")), input);
    evidence(
        json!({"case":"XC37","execution_worker":event,"launch":launch,"second":second,"third":third,"fourth":full,"nonowner":other,"edit_save":saved,"replacement":replacement,"terminal":terminal}),
    );
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc23_actual_sigkill_at_each_observed_saveas_checkpoint_recovers_exact_bytes() {
    for mode in ["save", "restore"] {
        let oracle = Fixture::new();
        let before = inventory(&oracle.root.join("source"));
        let mut trace = Supervisor::spawn(&oracle.root, mode, "*", "*", false);
        let mut checkpoints = Vec::new();
        while let Some((event, mut stream)) = trace.next() {
            if event["site"].as_str().unwrap().starts_with("save.") {
                checkpoints.push((
                    event["site"].as_str().unwrap().to_owned(),
                    event["occurrence"].as_u64().unwrap(),
                ));
            }
            release(&mut stream);
        }
        trace.finish();
        assert_eq!(inventory(&oracle.root.join("source")), before);
        let expected = content(&oracle.root.join("exports/saved"));
        assert!(!checkpoints.is_empty());
        for (site, occurrence) in checkpoints {
            // Restore-specific cuts already observed during save are covered by
            // the save matrix; its after_restore/GC cuts are independently cut.
            if mode == "restore" && !site.contains("restore") && !site.contains("gc_") {
                continue;
            }
            let f = Fixture::new();
            let input = inventory(&f.root.join("source"));
            let mut child = Supervisor::spawn(&f.root, mode, &site, &occurrence.to_string(), false);
            let (event, _stream) = child
                .next()
                .expect("selected B4 checkpoint must be reached");
            let at_cut = inventory(&f.root.join("exports/saved"));
            child.kill();
            let mut registry = super::super::output::TargetRegistry::open(
                &f.root.join("exports"),
                &f.root.join("save-state"),
            )
            .unwrap();
            let mut outcomes = Vec::new();
            for pending in registry.pending_recoveries().unwrap() {
                let report = super::super::output::recover(
                    &mut registry,
                    pending["id"].as_str().unwrap(),
                    if mode == "restore" {
                        "restore"
                    } else {
                        "complete"
                    },
                );
                outcomes.push(serde_json::to_value(&report).unwrap());
                assert!(
                    matches!(
                        report.state,
                        super::super::output::SaveOutcome::Complete
                            | super::super::output::SaveOutcome::Restored
                    ),
                    "{site}/{occurrence}: {:?}",
                    report.error
                );
            }
            let mut observed = inventory(&f.root.join("exports/saved"));
            let mut preserved = BTreeMap::new();
            for (path, entry) in observed.clone() {
                if !expected.contains_key(&path) {
                    assert_eq!(
                        site, "save.after_stage_file_sync",
                        "unexpected residual {path:?}"
                    );
                    assert!(
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".ned-editor-save-")
                    );
                    assert_eq!(
                        at_cut.get(&path),
                        Some(&entry),
                        "unclaimed staging identity/hash changed"
                    );
                    observed.remove(&path);
                    preserved.insert(path, entry);
                }
            }
            assert_eq!(
                observed
                    .into_iter()
                    .map(|(p, (h, _, _))| (p, h))
                    .collect::<BTreeMap<_, _>>(),
                expected,
                "{mode}:{site}:{occurrence}"
            );
            assert_eq!(inventory(&f.root.join("source")), input);
            assert!(registry.pending_recoveries().unwrap().is_empty());
            evidence(
                json!({"case":"XC23","mode":mode,"barrier":event,"signal":9,"recovered":true,"outcomes":outcomes,"managed_output_hashes":expected,"preserved_unclaimed_staging":preserved}),
            );
        }
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc35_actual_coordinator_sigkill_covers_snapshot_publication_manifest_and_response_cuts() {
    for site in [
        "snapshot.before_commit",
        "snapshot.after_commit",
        "snapshot.after_sync",
        "run.before_publish",
        "run.after_publish",
        "run.after_create_ack",
        "worker.before_prepare",
        "worker.before_execute",
        "output.before_manifest",
        "output.after_manifest",
        "worker.after_response",
        "worker.before_frame",
        "run.before_completion_lock",
        "run.verifying_unlocked",
        "run.after_cancel_persist",
        "run.before_cancel_persist",
    ] {
        let f = Fixture::new();
        let source = inventory(&f.root.join("source"));
        let mut child = Supervisor::spawn(
            &f.root,
            if site == "run.after_cancel_persist" || site == "run.before_cancel_persist" {
                "cancel-persist"
            } else {
                "run"
            },
            site,
            "0",
            false,
        );
        let (event, _stream) = child.next().expect("selected run barrier must be reached");
        let expected_ids: Vec<_> = fs::read_dir(f.root.join("state/runs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("run-"))
            .collect();
        let unpublished = [
            "snapshot.before_commit",
            "snapshot.after_commit",
            "snapshot.after_sync",
            "run.before_publish",
        ]
        .contains(&site);
        assert_eq!(
            expected_ids.len(),
            usize::from(!unpublished),
            "{site}: {expected_ids:?}"
        );
        let worker_pid = if event["pid"] != json!(child.child.id()) {
            Some(event["pid"].as_u64().unwrap() as i32)
        } else {
            None
        };
        child.kill();
        if let Some(pid) = worker_pid {
            // Parent-death protection must end the worker; zombie is already
            // terminated and is reaped by its adopted parent, not this supervisor.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let alive = fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .is_some_and(|s| !s.split_once(") ").unwrap().1.starts_with('Z'));
                if !alive {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker survived coordinator death"
                );
                std::thread::yield_now();
            }
        }
        let launches_before = fs::read(f.root.join("execution-launches.jsonl")).unwrap_or_default();
        drop(child);
        let mut reopened = Supervisor::spawn(&f.root, "reopen-delete", "unused", "0", false);
        assert!(reopened.next().is_none());
        reopened.finish();
        let observation: Value =
            serde_json::from_slice(&fs::read(f.root.join("reopened.json")).unwrap()).unwrap();
        assert_eq!(observation["worker_launching_enabled"], true);
        assert_eq!(
            fs::read(f.root.join("execution-launches.jsonl")).unwrap_or_default(),
            launches_before,
            "restart must not launch another execution worker"
        );
        let runs = observation["records"].clone();
        assert_eq!(
            runs["runs"].as_array().unwrap().len(),
            expected_ids.len(),
            "{site}: {runs}"
        );
        assert_eq!(observation["deleted_run_ids"], json!(expected_ids));
        for run in runs["runs"].as_array().unwrap() {
            assert!(
                expected_ids.iter().any(|id| run["run_id"] == *id),
                "{site}: {run}"
            );
            assert!(
                !["queued", "preparing", "running", "verifying", "stopping"]
                    .contains(&run["phase"].as_str().unwrap())
            );
            if [
                "output.after_manifest",
                "worker.after_response",
                "worker.before_frame",
                "run.before_completion_lock",
                "run.verifying_unlocked",
            ]
            .contains(&site)
            {
                assert_eq!(run["phase"], "completed", "{site}: {run}");
                assert_eq!(run["artifact_state"], "published");
            } else {
                assert!(
                    ["interrupted", "cancelled"].contains(&run["phase"].as_str().unwrap()),
                    "{site}: {run}"
                );
            }
        }
        assert_eq!(inventory(&f.root.join("source")), source);
        evidence(
            json!({"case":"XC35","barrier":event,"coordinator_signal":9,"worker_parent_death_checked":worker_pid.is_some(),"reopened_worker_launching_enabled":true,"execution_launches_unchanged_sha256":hash(&launches_before),"expected_run_ids":expected_ids,"deleted_run_ids":observation["deleted_run_ids"],"reopened_runs":runs}),
        );
    }
}

fn resource_probe(root: &Path, mode: &str) {
    worker::enforce().unwrap();
    if mode == "rlimit-as" {
        // Python mmap uses a real anonymous mmap and does not touch 4GiB pages.
        // The child inherits the actual worker enforce() ceiling.
        let out=Command::new("python3").arg("-c").arg("import mmap,resource,json; a=resource.getrlimit(resource.RLIMIT_AS); f=resource.getrlimit(resource.RLIMIT_FSIZE);\ntry:\n m=mmap.mmap(-1,4*1024**3+1); raise RuntimeError('mapping unexpectedly accepted')\nexcept OSError as e:\n assert e.errno==12; print(json.dumps({'as':a,'fsize':f,'errno':e.errno}))").output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        fs::write(root.join("resource.json"), out.stdout).unwrap();
    } else {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("sparse"))
            .unwrap();
        f.seek(SeekFrom::Start(1024 * 1024 * 1024 - 1)).unwrap();
        f.write_all(&[1]).unwrap();
        fs::write(root.join("at-limit"), b"1GiB sparse write succeeded").unwrap();
        f.write_all(&[2]).unwrap();
        panic!("write beyond RLIMIT_FSIZE survived");
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc34_actual_worker_address_space_and_sparse_file_hard_limits() {
    for mode in ["rlimit-as", "rlimit-fsize"] {
        let f = Fixture::new();
        let mut child = Supervisor::spawn(&f.root, mode, "unused", "0", false);
        assert!(child.next().is_none());
        let exit = child.child.wait().unwrap();
        if mode == "rlimit-as" {
            assert!(exit.success());
            let value: Value =
                serde_json::from_slice(&fs::read(f.root.join("resource.json")).unwrap()).unwrap();
            assert_eq!(value["errno"], 12);
            assert_eq!(
                value["as"],
                json!([4u64 * 1024 * 1024 * 1024, 4u64 * 1024 * 1024 * 1024])
            );
            evidence(json!({"case":"XC34","mode":mode,"kernel":value}));
        } else {
            assert_eq!(exit.signal(), Some(25));
            assert!(f.root.join("at-limit").exists());
            assert_eq!(
                fs::metadata(f.root.join("sparse")).unwrap().len(),
                1024 * 1024 * 1024
            );
            assert!(fs::metadata(f.root.join("sparse")).unwrap().blocks() < 1024);
            evidence(json!({"case":"XC34","mode":mode,"signal":25,"bytes":1024u64*1024*1024}));
        }
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc30_actual_partial_report_manifest_and_records_are_consistent() {
    let f = Fixture::new();
    let mut child = Supervisor::spawn(&f.root, "partial", "unused", "0", false);
    assert!(child.next().is_none());
    child.finish();
    let terminal: Value =
        serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
    assert_eq!(terminal["phase"], "partial");
    assert_eq!(terminal["report"]["partial"], true);
    assert_eq!(terminal["report"]["exit_code"], 3);
    assert_eq!(terminal["worker_exit"]["reaped"], true);
    let m = RunManager::open(&f.root.join("state"), &f.project.input_regions()).unwrap();
    let id = fs::read_to_string(f.root.join("run-id")).unwrap();
    let r = m.record(&id).unwrap();
    let b = validated(&m.shared, &r).unwrap();
    assert_eq!(b.exit_code, 3);
    assert!(b.partial);
    assert_eq!(b.committed_events, "1");
    assert_eq!(
        b.termination,
        terminal["report"]["termination"].as_str().unwrap()
    );
    evidence(
        json!({"case":"XC30","terminal":terminal,"validator":{"partial":b.partial,"exit_code":b.exit_code,"termination":b.termination,"committed_events":b.committed_events,"finish_ps":b.finish_ps,"last_event_time_ps":b.last_event_time_ps}}),
    );
    close(m);
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc36_actual_publication_conflicts_preserve_external_files_and_other_roots() {
    for kind in [
        "same-id-stage",
        "nonempty",
        "symlink",
        "hardlink",
        "unknown-entry",
    ] {
        let f = Fixture::new();
        let source = inventory(&f.root.join("source"));
        fs::create_dir(f.root.join("save-recovery-sentinel")).unwrap();
        fs::write(
            f.root.join("save-recovery-sentinel/journal"),
            b"owned by unrelated save",
        )
        .unwrap();
        fs::write(f.root.join("external"), b"external sentinel").unwrap();
        let unrelated = inventory(&f.root.join("save-recovery-sentinel"));
        let external = inventory(&f.root.join("external"));
        let mut child = Supervisor::spawn(&f.root, "controlled", "output.export_entry", "0", false);
        let (event, mut stream) = child.next().unwrap();
        let output = PathBuf::from(event["path"].as_str().unwrap());
        let run = fs::read_to_string(f.root.join("run-id")).unwrap();
        let foreign = match kind {
            "same-id-stage" => {
                let path = output.join(format!(".tmp-{run}"));
                fs::create_dir(&path).unwrap();
                fs::write(path.join("sentinel"), b"foreign staging owner").unwrap();
                path
            }
            "nonempty" => {
                let path = output.join("events.csv");
                fs::write(&path, b"existing output").unwrap();
                path
            }
            "symlink" => {
                let path = output.join("results.json");
                std::os::unix::fs::symlink(f.root.join("external"), &path).unwrap();
                path
            }
            "hardlink" => {
                let path = output.join("results.json");
                fs::hard_link(f.root.join("external"), &path).unwrap();
                path
            }
            _ => {
                let path = output.join("unknown");
                fs::write(&path, b"unknown owner").unwrap();
                path
            }
        };
        let foreign_before = inventory(&foreign);
        release(&mut stream);
        assert!(child.next().is_none());
        child.finish();
        let terminal: Value =
            serde_json::from_slice(&fs::read(f.root.join("terminal.json")).unwrap()).unwrap();
        if kind == "symlink" {
            // The independent quota observer may detect the unsafe entry
            // before export reports its collision. Both are terminal denials.
            assert!(
                ["resource_limited", "integrity_failed", "output_failed"]
                    .contains(&terminal["phase"].as_str().unwrap()),
                "{terminal}"
            );
        } else {
            // The publisher now checks the directory before committing the
            // manifest, so an unknown entry is an output failure as well.
            assert_eq!(terminal["phase"], "output_failed", "{kind}: {terminal}");
            assert_eq!(terminal["artifact_state"], "unpublished");
            assert!(!output.join("manifest.json").exists());
        }
        assert_eq!(inventory(&foreign), foreign_before);
        assert_eq!(inventory(&f.root.join("external")), external);
        assert_eq!(inventory(&f.root.join("source")), source);
        assert_eq!(inventory(&f.root.join("save-recovery-sentinel")), unrelated);
        evidence(
            json!({"case":"XC36","kind":kind,"barrier":event,"foreign_preserved":true,"terminal":terminal}),
        );
    }
}

#[test]
#[ignore = "Requires Linux fault isolation; run explicitly with --ignored --test-threads=1"]
fn xc38_independently_valid_cross_run_bundle_substitution_and_unsafe_paths_are_denied() {
    let f = Fixture::new();
    let source = inventory(&f.root.join("source"));
    let mut a = Supervisor::spawn(&f.root, "run", "unused", "0", false);
    assert!(a.next().is_none());
    a.finish();
    let aid = fs::read_to_string(f.root.join("run-id")).unwrap();
    drop(a);
    let mut b = Supervisor::spawn(&f.root, "run-b", "unused", "0", false);
    assert!(b.next().is_none());
    b.finish();
    let bid = fs::read_to_string(f.root.join("run-id")).unwrap();
    drop(b);
    assert_ne!(aid, bid);
    let m = RunManager::open(&f.root.join("state"), &f.project.input_regions()).unwrap();
    let ar = m.record(&aid).unwrap();
    let br = m.record(&bid).unwrap();
    validated(&m.shared, &ar).unwrap();
    validated(&m.shared, &br).unwrap();
    let original_expectation = expectation(&m.shared, &ar).unwrap();
    let aroot = m.shared.root.join(&aid);
    let broot = m.shared.root.join(&bid);
    let other = inventory(&broot.join("result"));
    let original: BTreeMap<_, _> = fs::read_dir(aroot.join("result"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_owned(),
                fs::read(path).unwrap(),
            )
        })
        .collect();
    for entry in fs::read_dir(broot.join("result")).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, aroot.join("result").join(path.file_name().unwrap())).unwrap();
    }
    fs::copy(
        broot.join("control/worker-response.json"),
        aroot.join("control/worker-response.json"),
    )
    .unwrap();
    let live = m.artifacts(&aid).unwrap_err();
    assert_eq!(live.code, "E-EDITOR-RUN-INTEGRITY");
    assert!(m.download(&aid, "results.json").is_err());
    assert!(m.build_viewer(&aid).is_err());
    assert_eq!(inventory(&broot.join("result")), other);
    close(m);
    let reopened = RunManager::open(&f.root.join("state"), &f.project.input_regions()).unwrap();
    assert_eq!(reopened.get(&aid).unwrap()["phase"], "integrity_failed");
    assert_eq!(reopened.get(&bid).unwrap()["phase"], "completed");
    close(reopened);
    // Preserve A's authoritative ledger; restore artifact bytes for independent
    // live checks. Symlink and manifest path mutations must not expose a sentinel.
    fs::write(f.root.join("external-sentinel"), b"never disclose").unwrap();
    let sentinel = inventory(&f.root.join("external-sentinel"));
    for kind in [
        "absolute",
        "traversal",
        "symlink",
        "artifact-bytes",
        "manifest-bytes",
        "response-bytes",
    ] {
        for (name, bytes) in &original {
            let path = aroot.join("result").join(name);
            let _ = fs::remove_file(&path);
            fs::write(path, bytes).unwrap();
        }
        let mut expected = original_expectation.clone();
        if kind == "absolute" || kind == "traversal" {
            let mut manifest: Value =
                serde_json::from_slice(&original[std::ffi::OsStr::new("manifest.json")]).unwrap();
            manifest["files"][0]["name"] = json!(if kind == "absolute" {
                f.root.join("external-sentinel").display().to_string()
            } else {
                "../external-sentinel".into()
            });
            fs::write(
                aroot.join("result/manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            expected.manifest_sha256 = None;
        } else if kind == "symlink" {
            fs::remove_file(aroot.join("result/results.json")).unwrap();
            std::os::unix::fs::symlink(
                f.root.join("external-sentinel"),
                aroot.join("result/results.json"),
            )
            .unwrap();
        } else if kind == "artifact-bytes" {
            fs::write(aroot.join("result/events.csv"), b"corrupt").unwrap();
        } else if kind == "manifest-bytes" {
            fs::write(aroot.join("result/manifest.json"), b"{").unwrap();
        } else {
            let response: worker::Message = serde_json::from_slice(
                &fs::read(broot.join("control/worker-response.json")).unwrap(),
            )
            .unwrap();
            let mut record = ar.clone();
            record.response = Some(response);
            assert!(validate_worker_identity(&record).is_err());
        }
        if kind != "response-bytes" {
            assert!(
                validate_result_bundle(&aroot.join("result"), &expected).is_err(),
                "{kind}"
            );
        }
        assert_eq!(inventory(&f.root.join("external-sentinel")), sentinel);
        assert_eq!(inventory(&broot.join("result")), other);
        evidence(json!({"case":"XC38","kind":kind,"foreign_bytes_exposed":false}));
    }
    assert_eq!(inventory(&f.root.join("source")), source);
    evidence(
        json!({"case":"XC38","valid_runs_before_substitution":[aid,bid],"live_error":live,"recovery_integrity_failed":true}),
    );
}
