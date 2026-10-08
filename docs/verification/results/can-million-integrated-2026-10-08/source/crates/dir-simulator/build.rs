//! Capture only explicit, non-secret build identity at compilation time.
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path, process::Command};

fn command(program: &str, args: &[&str], root: &Path) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn source_hash(root: &Path) -> String {
    fn collect(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, files);
                } else if path.is_file() {
                    files.push(path);
                }
            }
        }
    }
    let mut files = vec![
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("rust-toolchain.toml"),
        root.join("crates/dir-simulator/Cargo.toml"),
        root.join("crates/dir-simulator/build.rs"),
    ];
    collect(&root.join("crates/dir-simulator/src"), &mut files);
    collect(&root.join("docs/specs"), &mut files);
    let ledger = root.join("docs/third-party/採用物台帳.md");
    if ledger.is_file() {
        files.push(ledger);
    }
    files.sort();
    let mut digest = Sha256::new();
    for path in files {
        let Ok(bytes) = fs::read(&path) else {
            return "unknown".into();
        };
        let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}
fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest).join("../..");
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let git = command("git", &["rev-parse", "HEAD"], &root).unwrap_or_else(|| "unknown".into());
    let dirty = command(
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
        &root,
    )
    .map(|s| !s.is_empty());
    let lock = fs::read(root.join("Cargo.lock"))
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .unwrap_or_else(|_| "unknown".into());
    let values = [
        ("git_commit", git.clone()),
        (
            "git_dirty",
            dirty
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".into()),
        ),
        (
            "compiler",
            command(&rustc, &["-V"], &root).unwrap_or_else(|| "unknown".into()),
        ),
        (
            "toolchain",
            command(&rustc, &["-Vv"], &root).unwrap_or_else(|| "unknown".into()),
        ),
        (
            "target_triple",
            env::var("TARGET").unwrap_or_else(|_| "unknown".into()),
        ),
        ("cargo_lock_sha256", lock),
        ("build_source_sha256", source_hash(&root)),
        (
            "adoption_ledger_sha256",
            fs::read(root.join("docs/third-party/採用物台帳.md"))
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .unwrap_or_else(|_| "not-present".into()),
        ),
        // A full repository revision identifies the adopted model/specification set.
        ("adoption_ledger_version", git),
    ];
    let mut generated = String::new();
    for (key, value) in values {
        generated.push_str(&format!("({key:?}, {value:?}),\n"));
    }
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("build_provenance.rs"),
        format!("&[{generated}]\n"),
    )
    .unwrap();
    for path in [
        "Cargo.lock",
        "Cargo.toml",
        "crates/dir-simulator/Cargo.toml",
        "rust-toolchain.toml",
        "docs/third-party/採用物台帳.md",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = command("git", &["rev-parse", "--git-path", name], &root) {
            let path = Path::new(&path);
            println!(
                "cargo:rerun-if-changed={}",
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                }
                .display()
            );
        }
    }
    // Rebuild after source edits, including an uncommitted dirty build.
    println!("cargo:rerun-if-changed=src");
    println!(
        "cargo:rerun-if-changed={}",
        root.join("docs/specs").display()
    );
    println!("cargo:rerun-if-changed=build.rs");
    if let Some(head) = command("git", &["symbolic-ref", "-q", "HEAD"], &root) {
        if let Some(path) = command("git", &["rev-parse", "--git-path", &head], &root) {
            let path = Path::new(&path);
            println!(
                "cargo:rerun-if-changed={}",
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                }
                .display()
            );
        }
    }
    for name in ["RUSTC", "TARGET"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
}
