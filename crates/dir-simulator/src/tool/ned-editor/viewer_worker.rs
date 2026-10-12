//! Resource-limited Viewer rendering. Only small policy metadata crosses the pipe.
use super::run_control::{
    storage::{hash_file, invalid},
    worker,
};
use super::{EditorError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    parent_pid: u32,
    source_fd: i32,
    output_fd: i32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub sha256: String,
    pub script_hashes: Vec<String>,
    pub style_hashes: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Complete { policy: Policy },
    Failed { message: String },
}

fn execute(request: Request) -> Result<Policy> {
    worker::enforce()?;
    if rustix::process::getppid().map(|p| p.as_raw_nonzero().get() as u32)
        != Some(request.parent_pid)
        || request.source_fd < 0
        || request.output_fd < 0
        || request.source_fd == request.output_fd
    {
        return Err(invalid(
            "Viewer worker parent or descriptor identity changed",
        ));
    }
    // Parent owns and holds both already-validated descriptors until this child is reaped.
    // Procfs duplicates them without re-resolving their original input/output paths.
    let source = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        request.parent_pid, request.source_fd
    ));
    let destination = format!("/proc/{}/fd/{}", request.parent_pid, request.output_fd);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .open(destination)
        .map_err(|e| invalid(e.to_string()))?;
    let rendered = crate::tool::viewer::render_input_with_policy(&source)
        .map_err(|d| EditorError::common("E-EDITOR-VIEWER", d))?;
    let sha256 = format!("{:x}", Sha256::digest(rendered.html.as_bytes()));
    output
        .write_all(rendered.html.as_bytes())
        .and_then(|()| output.sync_all())
        .map_err(|e| invalid(e.to_string()))?;
    Ok(Policy {
        sha256,
        script_hashes: rendered.script_hashes,
        style_hashes: rendered.style_hashes,
    })
}

pub fn hidden_viewer_worker() -> std::result::Result<(), crate::Diagnostic> {
    let result = worker::read_frame::<Request>(&mut std::io::stdin().lock()).and_then(execute);
    let reply = match result {
        Ok(policy) => Reply::Complete { policy },
        Err(e) => Reply::Failed {
            message: e.to_string(),
        },
    };
    worker::write_frame(&mut std::io::stdout().lock(), &reply)
        .map_err(|e| crate::Diagnostic::prepare(e.to_string()))
}

pub(crate) fn render(
    binary: &Path,
    expected_binary_hash: &str,
    source: &File,
    output: &File,
    shutdown: &AtomicBool,
) -> Result<Policy> {
    let executable = File::open(binary).map_err(|e| invalid(e.to_string()))?;
    if hash_file(&executable)? != expected_binary_hash {
        return Err(invalid("Viewer worker binary changed"));
    }
    let mut child = Command::new(binary)
        .arg("--ned-editor-viewer-worker")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| invalid(e.to_string()))?;
    let request = Request {
        parent_pid: std::process::id(),
        source_fd: source.as_raw_fd(),
        output_fd: output.as_raw_fd(),
    };
    if let Err(e) =
        worker::write_frame(child.stdin.as_mut().expect("Viewer worker stdin"), &request)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    child.stdin.take();
    let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("owned Viewer child");
    let pidfd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(invalid(e.to_string()));
        }
    };
    let mut stdout = child.stdout.take().expect("Viewer worker stdout");
    let response = std::thread::spawn(move || {
        let reply = worker::read_frame::<Reply>(&mut stdout);
        // Drain malformed or trailing output without allocating it or blocking the child.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        reply
    });
    let mut stderr = child.stderr.take().expect("Viewer worker stderr");
    let log = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut chunk = [0; 8192];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..n]);
            if tail.len() > 1024 * 1024 {
                tail.drain(..tail.len() - 1024 * 1024);
            }
        }
        tail
    });
    let started = Instant::now();
    let mut stopping = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(invalid(e.to_string()));
            }
            Ok(None) => {}
        }
        if stopping.is_none()
            && (shutdown.load(Ordering::Acquire) || started.elapsed() >= Duration::from_secs(600))
        {
            let _ = rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Term);
            stopping = Some(Instant::now());
        }
        if stopping.is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(2)) {
            let _ = rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Kill);
            break child.wait().map_err(|e| invalid(e.to_string()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let reply = response
        .join()
        .map_err(|_| invalid("Viewer response thread failed"))?;
    let logs = log.join().unwrap_or_default();
    let status = status?;
    if !status.success() || stopping.is_some() {
        return Err(EditorError::new(
            "E-EDITOR-VIEWER-WORKER",
            format!(
                "Viewer worker stopped ({status}): {}",
                String::from_utf8_lossy(&logs)
            ),
            422,
        ));
    }
    match reply? {
        Reply::Complete { policy } => Ok(policy),
        Reply::Failed { message } => Err(EditorError::new("E-EDITOR-VIEWER", message, 422)),
    }
}
