use super::controller::{Controller, Shared, strict_json};
use super::output::TargetRegistry;
use super::{EditorError, Result, absolute, random_id};
use crate::types::Diagnostic;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const BODY_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct EditorOptions {
    pub config: Option<PathBuf>,
    pub export_root: Option<PathBuf>,
    pub state_root: Option<PathBuf>,
}
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("static HTTP header")
}
fn respond(request: Request, status: u16, body: Vec<u8>, content: &str) {
    let response=Response::from_data(body).with_status_code(StatusCode(status)).with_header(header("Content-Type",content)).with_header(header("Cache-Control","no-store")).with_header(header("Referrer-Policy","no-referrer")).with_header(header("X-Content-Type-Options","nosniff")).with_header(header("Content-Security-Policy","default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    let _ = request.respond(response);
}
fn json_response(request: Request, status: u16, value: Value) {
    respond(
        request,
        status,
        serde_json::to_vec(&value).expect("JSON value"),
        "application/json; charset=utf-8",
    );
}
fn failure(request: Request, error: EditorError) {
    json_response(
        request,
        error.status,
        json!({"code":error.code,"message":error.message}),
    );
}
fn decode(s: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut i = 0;
    let raw = s.as_bytes();
    while i < raw.len() {
        if raw[i] == b'%' {
            let part = raw.get(i + 1..i + 3).ok_or_else(|| {
                EditorError::new("E-EDITOR-HTTP", "Malformed percent encoding", 400)
            })?;
            let digits = std::str::from_utf8(part).map_err(|_| {
                EditorError::new("E-EDITOR-HTTP", "Malformed percent encoding", 400)
            })?;
            bytes.push(u8::from_str_radix(digits, 16).map_err(|_| {
                EditorError::new("E-EDITOR-HTTP", "Malformed percent encoding", 400)
            })?);
            i += 3;
        } else {
            bytes.push(if raw[i] == b'+' { b' ' } else { raw[i] });
            i += 1;
        }
    }
    String::from_utf8(bytes)
        .map_err(|_| EditorError::new("E-EDITOR-HTTP", "Query is not UTF-8", 400))
}
fn route(mut request: Request, shared: &Shared, host: &str, secret: &str) {
    let origin = format!("http://{host}");
    let headers = request.headers();
    let values = |name: &'static str| -> Vec<String> {
        headers
            .iter()
            .filter(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_owned())
            .collect()
    };
    if values("Host") != [host] {
        return failure(
            request,
            EditorError::new("E-EDITOR-HOST", "Unexpected Host header", 403),
        );
    }
    let url = request.url().to_owned();
    let (path, raw_query) = url.split_once('?').unwrap_or((&url, ""));
    let method = request.method().as_str().to_owned();
    if !path.starts_with("/api/") {
        if request.method() != &Method::Get && request.method() != &Method::Head {
            return failure(
                request,
                EditorError::new("E-EDITOR-HTTP", "Method not allowed", 405),
            );
        }
        let asset = match path {
            "/" => Some((
                include_bytes!("assets/index.html").as_slice(),
                "text/html; charset=utf-8",
            )),
            "/assets/style.css" => Some((
                include_bytes!("assets/style.css").as_slice(),
                "text/css; charset=utf-8",
            )),
            "/assets/input.js" => Some((
                include_bytes!("assets/input.js").as_slice(),
                "text/javascript; charset=utf-8",
            )),
            "/assets/view.js" => Some((
                include_bytes!("assets/view.js").as_slice(),
                "text/javascript; charset=utf-8",
            )),
            _ => None,
        };
        if let Some((bytes, content)) = asset {
            return respond(request, 200, bytes.to_vec(), content);
        }
        return failure(
            request,
            EditorError::new("E-EDITOR-HTTP", "Unknown route", 404),
        );
    }
    if values("X-Editor-Session") != [secret] {
        return failure(
            request,
            EditorError::new("E-EDITOR-AUTH", "Session authentication required", 403),
        );
    }
    let supplied_origin = values("Origin");
    if !supplied_origin.is_empty() && supplied_origin != [origin.clone()] {
        return failure(
            request,
            EditorError::new("E-EDITOR-ORIGIN", "Unexpected Origin header", 403),
        );
    }
    if method != "GET" && method != "POST" {
        return failure(
            request,
            EditorError::new("E-EDITOR-HTTP", "Method not allowed", 405),
        );
    }
    let mut query = BTreeMap::new();
    for pair in raw_query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = decode(k).and_then(|k| decode(v).map(|v| (k, v)));
        match decoded {
            Ok((k, v)) => {
                if query.insert(k, v).is_some() {
                    return failure(
                        request,
                        EditorError::new("E-EDITOR-HTTP", "Duplicate query key", 400),
                    );
                }
            }
            Err(e) => return failure(request, e),
        }
    }
    let body = if method == "POST" {
        if values("Origin") != [origin] {
            return failure(
                request,
                EditorError::new("E-EDITOR-ORIGIN", "Unexpected Origin header", 403),
            );
        }
        let content = values("Content-Type");
        if content.len() != 1
            || content[0].split(';').next().map(str::trim) != Some("application/json")
        {
            return failure(
                request,
                EditorError::new("E-EDITOR-HTTP", "JSON Content-Type required", 415),
            );
        }
        if request.body_length().is_some_and(|n| n > BODY_LIMIT) {
            return failure(
                request,
                EditorError::new("E-EDITOR-BODY-LIMIT", "Request exceeds 64 MiB", 413),
            );
        }
        let mut bytes = Vec::new();
        let result =
            std::io::Read::take(request.as_reader(), BODY_LIMIT as u64 + 1).read_to_end(&mut bytes);
        if let Err(e) = result {
            return failure(
                request,
                EditorError::new("E-EDITOR-HTTP", e.to_string(), 400),
            );
        }
        if bytes.len() > BODY_LIMIT {
            return failure(
                request,
                EditorError::new("E-EDITOR-BODY-LIMIT", "Request exceeds 64 MiB", 413),
            );
        }
        match strict_json(&bytes) {
            Ok(value) => Some(value),
            Err(e) => return failure(request, e),
        }
    } else {
        None
    };
    let decoded_path = match decode(path) {
        Ok(path) => path,
        Err(e) => return failure(request, e),
    };
    let (status, value) = Controller::handle(shared, &method, &decoded_path, &query, body);
    json_response(request, status, value);
}
fn start(options: EditorOptions) -> Result<()> {
    let cwd = std::env::current_dir()
        .map_err(|e| EditorError::new("E-EDITOR-START", e.to_string(), 500))?;
    let export = absolute(options.export_root.as_deref().unwrap_or(&cwd), &cwd);
    let state = options.state_root.unwrap_or_else(|| {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| cwd.clone())
                    .join(".local/state")
            })
            .join("dir-simulator/ned-editor")
    });
    let registry = TargetRegistry::open(&export, &absolute(&state, &cwd))?;
    let shared = Arc::new(Mutex::new(Controller::with_config(
        options.config.map(|path| absolute(&path, &cwd)),
        cwd,
        registry,
    )?));
    let server = Server::http("127.0.0.1:0")
        .map_err(|e| EditorError::new("E-EDITOR-START", e.to_string(), 500))?;
    let host = server.server_addr().to_string();
    let secret = random_id("")?;
    println!(
        "{}",
        json!({"schema_version":1,"status":"listening","url":format!("http://{host}/#session={secret}")})
    );
    let active = Arc::new(AtomicUsize::new(0));
    for request in server.incoming_requests() {
        if active.fetch_add(1, Ordering::AcqRel) >= 256 {
            active.fetch_sub(1, Ordering::AcqRel);
            failure(
                request,
                EditorError::new("E-EDITOR-BUSY", "Request queue is full", 503),
            );
            continue;
        }
        let active = active.clone();
        let shared = shared.clone();
        let host = host.clone();
        let secret = secret.clone();
        std::thread::spawn(move || {
            route(request, &shared, &host, &secret);
            active.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}
pub fn serve(options: EditorOptions) -> std::result::Result<(), Diagnostic> {
    start(options).map_err(|e| Diagnostic::prepare(e.to_string()))
}
