//! Local NED editing: immutable input copies, common parsing, and explicit project export.
mod analysis;
mod builtin;
mod commands;
mod composition;
mod controller;
mod input;
mod layout;
mod model;
mod output;
mod server;
mod settings;
mod source_index;
mod template;
mod view;

use crate::types::Diagnostic;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub use server::{EditorOptions, serve};

pub(crate) type Result<T> = std::result::Result<T, EditorError>;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EditorError {
    pub code: String,
    pub message: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<Box<Diagnostic>>,
}
impl EditorError {
    pub fn new(code: &str, message: impl Into<String>, status: u16) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            status,
            diagnostic: None,
        }
    }
    pub fn common(code: &str, diagnostic: Diagnostic) -> Self {
        Self {
            code: code.into(),
            message: diagnostic.message.clone(),
            status: 422,
            diagnostic: Some(Box::new(diagnostic)),
        }
    }
    pub fn io(path: &std::path::Path, error: impl std::fmt::Display) -> Self {
        Self::new("E-EDITOR-SAVE", format!("{}: {error}", path.display()), 500)
    }
}
impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for EditorError {}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn random_id(prefix: &str) -> Result<String> {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    let mut offset = 0;
    while offset < bytes.len() {
        let n =
            rustix::rand::getrandom(&mut bytes[offset..], rustix::rand::GetRandomFlags::empty())
                .map_err(|e| EditorError::new("E-EDITOR-RANDOM", e.to_string(), 500))?;
        if n == 0 {
            return Err(EditorError::new(
                "E-EDITOR-RANDOM",
                "Random source returned no bytes",
                500,
            ));
        }
        offset += n;
    }
    let mut encoded = String::with_capacity(prefix.len() + bytes.len() * 2);
    encoded.push_str(prefix);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("String formatting");
    }
    Ok(encoded)
}
pub(crate) fn absolute(path: &std::path::Path, base: &std::path::Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for c in if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
    .components()
    {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            c => normalized.push(c.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests;
