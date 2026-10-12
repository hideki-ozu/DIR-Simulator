//! Read-only Viewer capabilities on one independent loopback origin per service.
use super::*;
use std::io::Read;
use std::os::unix::fs::FileExt;
use tiny_http::{Header, Request, Response, Server, StatusCode};

struct ActiveRoute {
    file: File,
    // An independent shared flock prevents another process deleting this run.
    _lease: File,
}
struct Route {
    deadline: Instant,
    active: Option<ActiveRoute>,
    html_sha256: String,
    csp: String,
}
impl Route {
    fn expire(&mut self) {
        if Instant::now() >= self.deadline {
            self.active.take();
        }
    }
}
type Routes = Arc<Mutex<BTreeMap<String, Arc<Mutex<Route>>>>>;

pub(super) struct ViewerService {
    host: String,
    routes: Routes,
    stopped: Arc<AtomicBool>,
}
pub(super) struct ViewerLease {
    url: String,
    expires_ms: u64,
    route: Arc<Mutex<Route>>,
    html_sha256: String,
}
impl ViewerLease {
    pub(super) fn active(&self) -> bool {
        let mut route = self.route.lock().unwrap();
        route.expire();
        route.active.is_some()
    }
    pub(super) fn revoke(&self) {
        self.route.lock().unwrap().active.take();
    }
    pub(super) fn value(&self, id: &str) -> Value {
        json!({"run_id":id,"viewer_state":"ready","url":self.url,"expires_at_ms":self.expires_ms.to_string(),"target":"_blank","rel":"noopener noreferrer","readonly":true,"sha256":self.html_sha256})
    }
}
impl Drop for ViewerLease {
    fn drop(&mut self) {
        self.revoke();
    }
}
impl Drop for ViewerService {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        clear_routes(&self.routes);
    }
}
impl ViewerService {
    fn start(shared: &Arc<Shared>) -> Result<Arc<Self>> {
        let server = Server::http("127.0.0.1:0")
            .map_err(|e| EditorError::new("E-EDITOR-VIEWER", e.to_string(), 500))?;
        let host = server.server_addr().to_string();
        let routes: Routes = Arc::new(Mutex::new(BTreeMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let service = Arc::new(Self {
            host: host.clone(),
            routes: routes.clone(),
            stopped: stopped.clone(),
        });
        {
            let routes = routes.clone();
            let stopped = stopped.clone();
            let shutdown = Arc::downgrade(shared);
            std::thread::Builder::new()
                .name("ned-run-viewer-expiry".into())
                .spawn(move || {
                    expiry_loop(routes, || {
                        stopped.load(Ordering::Acquire)
                            || shutdown
                                .upgrade()
                                .is_none_or(|shared| shared.shutdown.load(Ordering::Acquire))
                    });
                })
                .map_err(|e| EditorError::new("E-EDITOR-VIEWER", e.to_string(), 500))?;
        }
        let shutdown = Arc::downgrade(shared);
        std::thread::Builder::new()
            .name("ned-run-viewer".into())
            .spawn(move || {
                listen(server, host, routes, || {
                    stopped.load(Ordering::Acquire)
                        || shutdown
                            .upgrade()
                            .is_none_or(|shared| shared.shutdown.load(Ordering::Acquire))
                });
            })
            .map_err(|e| EditorError::new("E-EDITOR-VIEWER", e.to_string(), 500))?;
        Ok(service)
    }

    fn publish(&self, generated: GeneratedViewer) -> Result<ViewerLease> {
        let mut routes = self.routes.lock().unwrap();
        let mut path = None;
        for _ in 0..32 {
            let candidate = format!("/{}", random_id("")?);
            if !routes.contains_key(&candidate) {
                path = Some(candidate);
                break;
            }
        }
        let path = path.ok_or_else(|| invalid("Cannot reserve Viewer capability"))?;
        let expires_ms = now_ms()
            .checked_add(3_600_000)
            .ok_or_else(|| invalid("Viewer expiry overflow"))?;
        let sha = generated.html_sha256;
        let route = Arc::new(Mutex::new(Route {
            deadline: Instant::now() + Duration::from_secs(3600),
            active: Some(ActiveRoute {
                file: generated.file,
                _lease: generated.lease,
            }),
            html_sha256: sha.clone(),
            csp: generated.csp,
        }));
        routes.insert(path.clone(), route.clone());
        Ok(ViewerLease {
            url: format!("http://{}{path}", self.host),
            expires_ms,
            route,
            html_sha256: sha,
        })
    }
}
fn listen(server: Server, host: String, routes: Routes, shutdown: impl Fn() -> bool) {
    while !shutdown() {
        let received = server.recv_timeout(Duration::from_millis(250));
        // Expire after the receive wait. Never authorize from a pre-wait snapshot.
        expire_routes(&routes);
        if shutdown() {
            break;
        }
        match received {
            Ok(Some(request)) => serve(request, &host, &routes),
            Ok(None) => {}
            Err(_) => break,
        }
    }
    clear_routes(&routes);
}
fn expiry_loop(routes: Routes, shutdown: impl Fn() -> bool) {
    while !shutdown() {
        expire_routes(&routes);
        // This fixed service worker keeps expiry independent of slow HTTP clients.
        std::thread::sleep(Duration::from_millis(250));
    }
    clear_routes(&routes);
}
fn clear_routes(routes: &Routes) {
    for route in routes.lock().unwrap().values() {
        route.lock().unwrap().active.take();
    }
}
fn expire_routes(routes: &Routes) {
    for route in routes.lock().unwrap().values() {
        route.lock().unwrap().expire();
    }
}
fn header(n: &str, v: &str) -> Header {
    Header::from_bytes(n, v).expect("Viewer header")
}
fn policy(hashes: &[String]) -> String {
    format!(
        "default-src 'none'; script-src {}; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        hashes.join(" ")
    )
}
fn gate(
    host: &str,
    path: &str,
    method: &str,
    hosts: &[&str],
    origins: &[&str],
    url: &str,
    expired: bool,
) -> u16 {
    if hosts != [host] {
        return 403;
    }
    let origin = format!("http://{host}");
    if !origins.is_empty() && origins != [origin.as_str()] {
        return 403;
    }
    if method != "GET" && method != "HEAD" {
        return 405;
    }
    if url != path {
        return 404;
    }
    if expired {
        return 410;
    }
    200
}

// File::try_clone shares an offset with the route descriptor. Use positional
// reads so HEAD, hashing and earlier responses cannot consume later responses.
struct ViewerBody {
    file: File,
    // Accepted responses remain a cross-process reference until transmission ends.
    _lease: File,
    offset: u64,
}
impl Read for ViewerBody {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.file.read_at(bytes, self.offset)?;
        self.offset = self
            .offset
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("Viewer offset overflow"))?;
        Ok(count)
    }
}
fn serve(request: Request, host: &str, routes: &Routes) {
    let values = |n: &'static str| {
        request
            .headers()
            .iter()
            .filter(|h| h.field.equiv(n))
            .map(|h| h.value.as_str())
            .collect::<Vec<_>>()
    };
    let route = routes.lock().unwrap().get(request.url()).cloned();
    let mut status = gate(
        host,
        if route.is_some() { request.url() } else { "" },
        request.method().as_str(),
        &values("Host"),
        &values("Origin"),
        request.url(),
        false,
    );
    let mut csp = "default-src 'none'; base-uri 'none'; frame-ancestors 'none'".to_owned();
    let mut body = None;
    let mut expected_sha = None;
    if let Some(route) = &route {
        let mut route = route.lock().unwrap();
        route.expire();
        csp = route.csp.clone();
        if status == 200 {
            if let Some(active) = &route.active {
                match active.file.try_clone().and_then(|file| {
                    let size = file.metadata()?.len();
                    let lease = active._lease.try_clone()?;
                    Ok((
                        ViewerBody {
                            file,
                            _lease: lease,
                            offset: 0,
                        },
                        size,
                    ))
                }) {
                    Ok(response) => {
                        body = Some(response);
                        expected_sha = Some(route.html_sha256.clone());
                    }
                    Err(_) => status = 500,
                }
            } else {
                status = 410;
            }
        }
    }
    if let Some((candidate, _)) = &body {
        // Checksums may be large: do not make revocation wait on this read.
        if !hash_file(&candidate.file).is_ok_and(|h| Some(h) == expected_sha) {
            status = 422;
            body = None;
        }
        let mut route = route.as_ref().unwrap().lock().unwrap();
        route.expire();
        if route.active.is_none() {
            status = 410;
            body = None;
        }
    }
    let headers = [
        header("Cache-Control", "no-store"),
        header("Referrer-Policy", "no-referrer"),
        header("X-Content-Type-Options", "nosniff"),
        header("Cross-Origin-Opener-Policy", "same-origin"),
        header("Content-Security-Policy", &csp),
    ];
    if let Some((body, size)) = body {
        let Ok(size) = usize::try_from(size) else {
            let _ = request.respond(Response::empty(StatusCode(500)));
            return;
        };
        let mut response = Response::new(StatusCode(200), Vec::new(), body, Some(size), None)
            .with_header(header("Content-Type", "text/html; charset=utf-8"));
        for h in headers {
            response.add_header(h);
        }
        let _ = request.respond(response);
    } else {
        let mut response = Response::empty(StatusCode(status));
        for h in headers {
            response.add_header(h);
        }
        let _ = request.respond(response);
    }
}

struct GeneratedViewer {
    file: File,
    lease: File,
    html_sha256: String,
    csp: String,
}
struct RenderReference(Arc<Shared>);
impl Drop for RenderReference {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.viewer_renders -= 1;
        self.0.wake.notify_all();
    }
}
fn next_generation(generations: &mut BTreeMap<String, u64>, id: &str) -> Result<u64> {
    let next = generations
        .get(id)
        .copied()
        .unwrap_or_default()
        .checked_add(1)
        .ok_or_else(|| invalid("Viewer generation overflow"))?;
    generations.insert(id.into(), next);
    Ok(next)
}
fn generation_current(generations: &BTreeMap<String, u64>, id: &str, generation: u64) -> bool {
    generations.get(id) == Some(&generation)
}
pub(super) fn build(manager: &RunManager, id: &str) -> Result<Value> {
    manager.mutation()?;
    manager.record(id)?;
    let (record, generation, pin) = {
        let mut state = manager.shared.state.lock().unwrap();
        if manager.shared.shutdown.load(Ordering::Acquire) {
            return Err(busy("Run service is shutting down"));
        }
        if let Some(v) = state.viewers.get(id) {
            if v.active() {
                return Ok(v.value(id));
            }
        }
        let r = state.records.get(id).ok_or_else(missing)?;
        if r.viewer_state == "building" {
            return Err(busy("Viewer generation is in progress"));
        }
        if r.artifact_state != "published" {
            return Err(EditorError::new(
                "E-EDITOR-RUN-UNPUBLISHED",
                "Viewer requires a verified result",
                409,
            ));
        }
        let pin = reference_lock(&manager.shared, id, false)?;
        let generation = next_generation(&mut state.viewer_generations, id)?;
        let r = state.records.get_mut(id).ok_or_else(missing)?;
        r.viewer_state = "building".into();
        persist(&manager.shared, r)?;
        let record = r.clone();
        *state.downloads.entry(id.into()).or_default() += 1;
        state.viewer_renders += 1;
        (record, generation, pin)
    };
    let _render_reference = RenderReference(manager.shared.clone());
    let _reference = DownloadReference {
        shared: manager.shared.clone(),
        run_id: id.into(),
        _lease: pin,
    };
    let generated = (|| {
        let mut bundle = validated(&manager.shared, &record)?;
        let source = bundle
            .artifacts
            .remove("results.json")
            .ok_or_else(|| invalid("Missing Viewer input"))?;
        let root = verify_owner(&manager.shared, &record)?;
        let viewer = root.child("viewer")?;
        let name = random_id("viewer-")? + ".html";
        cleanup::reserve_viewer(&manager.shared, &record, &name)?;
        let output = viewer.create_output(&name)?;
        cleanup::finish_viewer(&manager.shared, &record, &name, &output)?;
        let artifact =
            render_with_limits(&manager.shared, &record, &source.file, &output, generation)?;
        viewer.sync()?;
        let file = viewer.open_file(&name)?;
        let before = rustix::fs::fstat(&output).map_err(|e| invalid(e.to_string()))?;
        let after = rustix::fs::fstat(&file).map_err(|e| invalid(e.to_string()))?;
        if before.st_dev != after.st_dev || before.st_ino != after.st_ino {
            return Err(invalid("Generated Viewer identity changed"));
        }
        if hash_file(&file)? != artifact.sha256 {
            return Err(invalid("Generated Viewer checksum mismatch"));
        }
        Ok(GeneratedViewer {
            file,
            lease: reference_lock(&manager.shared, id, false)?,
            html_sha256: artifact.sha256,
            csp: policy(&artifact.script_hashes),
        })
    })();
    let mut state = manager.shared.state.lock().unwrap();
    if manager.shared.shutdown.load(Ordering::Acquire) {
        return Err(busy("Run service is shutting down"));
    }
    if !generation_current(&state.viewer_generations, id, generation)
        || !state.records.contains_key(id)
    {
        // Revocation/deletion wins over every pending render outcome, including failures.
        return Err(EditorError::new(
            "E-EDITOR-VIEWER-REVOKED",
            "Viewer generation was revoked",
            410,
        ));
    }
    let installed = generated.and_then(|generated| {
        let service = if let Some(service) = &state.viewer_service {
            service.clone()
        } else {
            let service = ViewerService::start(&manager.shared)?;
            state.viewer_service = Some(service.clone());
            service
        };
        let lease = service.publish(generated)?;
        let record = state.records.get_mut(id).ok_or_else(missing)?;
        record.viewer_state = "ready".into();
        persist(&manager.shared, record)?;
        let value = lease.value(id);
        state.viewers.insert(id.into(), lease);
        Ok(value)
    });
    if let Err(e) = &installed {
        let record = state.records.get_mut(id).ok_or_else(missing)?;
        record.viewer_state = "failed".into();
        record
            .diagnostics
            .push(json!({"run_id":id,"source_stamp":record.stamp,"phase":"viewer","error":e}));
        persist(&manager.shared, record)?;
    }
    installed
}
pub(super) fn render_with_limits(
    shared: &Arc<Shared>,
    record: &RunRecord,
    source: &File,
    output: &File,
    generation: u64,
) -> Result<super::super::viewer_worker::Policy> {
    if verify_owner(shared, record)?.bytes()? > RUN_LIMIT
        || Directory::open(&shared.root)?.bytes()? > RUNS_LIMIT
    {
        return Err(busy("Viewer storage capacity is full"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let monitor_shared = shared.clone();
    let monitor_stop = stop.clone();
    let monitor_done = done.clone();
    let id = record.run_id.clone();
    let monitor=std::thread::Builder::new().name("ned-viewer-resource-monitor".into()).spawn(move||{
        loop {
            if *monitor_done.0.lock().unwrap(){break;}
            if monitor_shared.shutdown.load(Ordering::Acquire)||!generation_current(&monitor_shared.state.lock().unwrap().viewer_generations,&id,generation){monitor_stop.store(true,Ordering::Release);break;}
            let usage=record_dir(&monitor_shared,&id).and_then(|d|d.bytes()).and_then(|run|Directory::open(&monitor_shared.root)?.bytes().map(|total|(run,total)));
            let exceeded=match usage{Ok((run,total)) if run>RUN_LIMIT||total>RUNS_LIMIT=>Some(json!({"run_bytes":run.to_string(),"runs_bytes":total.to_string(),"soft_limit_overshoot":true})),Err(error)=>Some(json!({"error":error})),_=>None};
            if let Some(observation)=exceeded {
                let mut state=monitor_shared.state.lock().unwrap();if let Some(r)=state.records.get_mut(&id){r.diagnostics.push(json!({"run_id":id,"source_stamp":r.stamp,"phase":"viewer","resource_observation":observation}));let _=persist(&monitor_shared,r);}
                monitor_stop.store(true,Ordering::Release);break;
            }
            let guard=monitor_done.0.lock().unwrap();let _=monitor_done.1.wait_timeout_while(guard,Duration::from_secs(1),|done|!*done);
        }
    }).map_err(|e|EditorError::new("E-EDITOR-VIEWER",e.to_string(),500))?;
    let result = super::super::viewer_worker::render(
        &shared.binary,
        &shared.binary_hash,
        source,
        output,
        &stop,
    );
    *done.0.lock().unwrap() = true;
    done.1.notify_all();
    let _ = monitor.join();
    result
}
pub(super) fn revoke(manager: &RunManager, id: &str) -> Result<Value> {
    manager.mutation()?;
    manager.record(id)?;
    let mut state = manager.shared.state.lock().unwrap();
    if !state.records.contains_key(id) {
        return Err(missing());
    }
    next_generation(&mut state.viewer_generations, id)?;
    if let Some(lease) = state.viewers.remove(id) {
        lease.revoke();
    }
    let record = state.records.get_mut(id).ok_or_else(missing)?;
    record.viewer_state = "revoked".into();
    persist(&manager.shared, record)?;
    Ok(json!({"run_id":id,"viewer_state":"revoked"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct HttpFixture {
        directory: PathBuf,
        host: String,
        path: String,
        lease: ViewerLease,
        service: Arc<ViewerService>,
        shutdown: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        expiry_thread: Option<std::thread::JoinHandle<()>>,
    }
    impl HttpFixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(random_id("ned-viewer-http-").unwrap());
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(
                directory.join("viewer.html"),
                b"<html>fixed Viewer body</html>",
            )
            .unwrap();
            std::fs::write(directory.join("reference.lock"), b"").unwrap();
            let file = File::open(directory.join("viewer.html")).unwrap();
            let lock = File::open(directory.join("reference.lock")).unwrap();
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockShared).unwrap();
            let server = Server::http("127.0.0.1:0").unwrap();
            let host = server.server_addr().to_string();
            let routes: Routes = Arc::new(Mutex::new(BTreeMap::new()));
            let shutdown = Arc::new(AtomicBool::new(false));
            let service = Arc::new(ViewerService {
                host: host.clone(),
                routes: routes.clone(),
                stopped: shutdown.clone(),
            });
            let lease = service
                .publish(GeneratedViewer {
                    html_sha256: hash_file(&file).unwrap(),
                    file,
                    lease: lock,
                    csp: policy(&["'sha256-abc='".into()]),
                })
                .unwrap();
            let path = lease
                .url
                .strip_prefix(&format!("http://{host}"))
                .unwrap()
                .to_owned();
            let expiry_thread = {
                let routes = routes.clone();
                let shutdown = shutdown.clone();
                std::thread::spawn(move || expiry_loop(routes, || shutdown.load(Ordering::Acquire)))
            };
            let thread = {
                let host = host.clone();
                let shutdown = shutdown.clone();
                std::thread::spawn(move || {
                    listen(server, host, routes, || shutdown.load(Ordering::Acquire))
                })
            };
            Self {
                directory,
                host,
                path,
                lease,
                service,
                shutdown,
                thread: Some(thread),
                expiry_thread: Some(expiry_thread),
            }
        }
        fn request(&self, method: &str, path: &str, extra: &str) -> (u16, String, String) {
            let mut stream = std::net::TcpStream::connect(&self.host).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(
                stream,
                "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{extra}\r\n",
                self.host
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, headers.into(), body.into())
        }
        fn reference_released(&self) -> bool {
            let lock = File::open(self.directory.join("reference.lock")).unwrap();
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok()
        }
    }
    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Release);
            self.thread.take().unwrap().join().unwrap();
            self.expiry_thread.take().unwrap().join().unwrap();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn strict_request_gates() {
        let h = "127.0.0.1:1234";
        let p = "/opaque";
        assert_eq!(gate(h, p, "GET", &[h], &[], p, false), 200);
        assert_eq!(
            gate(h, p, "HEAD", &[h], &["http://127.0.0.1:1234"], p, false),
            200
        );
        for hosts in [vec![], vec!["localhost:1234"], vec![h, h]] {
            assert_eq!(gate(h, p, "GET", &hosts, &[], p, false), 403);
        }
        for origins in [
            vec!["http://127.0.0.1:5678"],
            vec!["null"],
            vec!["http://127.0.0.1:1234", "http://127.0.0.1:1234"],
        ] {
            assert_eq!(gate(h, p, "GET", &[h], &origins, p, false), 403);
        }
        assert_eq!(gate(h, p, "POST", &[h], &[], p, false), 405);
        for path in ["/", "/opaque?x=1", "/%6fpaque", "/opaque/", "/api/runs"] {
            assert_eq!(gate(h, p, "GET", &[h], &[], path, false), 404);
        }
        assert_eq!(gate(h, p, "GET", &[h], &[], p, true), 410);
    }
    #[test]
    fn csp_has_only_hash_script_sources() {
        let p = policy(&["'sha256-abc='".into()]);
        assert!(p.contains("script-src 'sha256-abc=';"));
        assert!(!p.contains("unsafe-eval"));
        assert!(!p.contains("script-src 'unsafe-inline'"));
        assert!(p.contains("connect-src 'none'"));
    }
    #[test]
    fn expiry_releases_route_files_but_pins_an_accepted_response() {
        let directory = std::env::temp_dir().join(random_id("ned-viewer-pin-").unwrap());
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("html"), b"body").unwrap();
        std::fs::write(directory.join("lock"), b"").unwrap();
        let pin = File::open(directory.join("lock")).unwrap();
        rustix::fs::flock(&pin, rustix::fs::FlockOperation::LockShared).unwrap();
        let route = Arc::new(Mutex::new(Route {
            deadline: Instant::now() + Duration::from_secs(3600),
            active: Some(ActiveRoute {
                file: File::open(directory.join("html")).unwrap(),
                _lease: pin,
            }),
            html_sha256: String::new(),
            csp: String::new(),
        }));
        let mut accepted = {
            let route = route.lock().unwrap();
            let active = route.active.as_ref().unwrap();
            ViewerBody {
                file: active.file.try_clone().unwrap(),
                _lease: active._lease.try_clone().unwrap(),
                offset: 0,
            }
        };
        let routes = Arc::new(Mutex::new(BTreeMap::from([(
            "/opaque".into(),
            route.clone(),
        )])));
        route.lock().unwrap().deadline = Instant::now() - Duration::from_secs(1);
        let iterations = std::sync::atomic::AtomicUsize::new(0);
        expiry_loop(routes.clone(), || {
            iterations.fetch_add(1, Ordering::AcqRel) > 0
        });
        assert!(route.lock().unwrap().active.is_none());
        assert_eq!(routes.lock().unwrap().len(), 1);
        let exclusive = File::open(directory.join("lock")).unwrap();
        assert_eq!(
            rustix::fs::flock(
                &exclusive,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            ),
            Err(rustix::io::Errno::WOULDBLOCK)
        );
        let mut body = String::new();
        accepted.read_to_string(&mut body).unwrap();
        assert_eq!(body, "body");
        drop(accepted);
        rustix::fs::flock(
            &exclusive,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[ignore = "Requires loopback sockets; run explicitly in permitted integration environment"]
    fn http_repeated_get_head_revoke_and_headers() {
        let fixture = HttpFixture::new();
        assert!(!fixture.reference_released());
        for _ in 0..2 {
            let (status, headers, body) = fixture.request("GET", &fixture.path, "");
            assert_eq!(status, 200);
            assert_eq!(body, "<html>fixed Viewer body</html>");
            let headers = headers.to_ascii_lowercase();
            for required in [
                "content-security-policy: default-src 'none'; script-src 'sha256-abc=';",
                "cache-control: no-store",
                "referrer-policy: no-referrer",
                "x-content-type-options: nosniff",
                "cross-origin-opener-policy: same-origin",
            ] {
                assert!(headers.contains(required), "Missing {required}");
            }
            assert!(!headers.contains("access-control-allow-origin"));
        }
        let (status, _, body) = fixture.request("HEAD", &fixture.path, "");
        assert_eq!(status, 200);
        assert!(body.is_empty());
        assert_eq!(fixture.request("POST", &fixture.path, "").0, 405);
        assert_eq!(fixture.request("GET", "/unknown", "").0, 404);
        assert_eq!(
            fixture.request("GET", &fixture.path, "Origin: null\r\n").0,
            403
        );
        assert_eq!(
            fixture
                .request("GET", &fixture.path, "Host: 127.0.0.1:1234\r\n")
                .0,
            403
        );
        fixture.lease.revoke();
        assert!(!fixture.lease.active());
        assert!(fixture.reference_released());
        assert_eq!(fixture.request("GET", &fixture.path, "").0, 410);
        assert_eq!(fixture.service.routes.lock().unwrap().len(), 1);
        let pin = File::open(fixture.directory.join("reference.lock")).unwrap();
        rustix::fs::flock(&pin, rustix::fs::FlockOperation::LockShared).unwrap();
        let file = File::open(fixture.directory.join("viewer.html")).unwrap();
        let second = fixture
            .service
            .publish(GeneratedViewer {
                html_sha256: hash_file(&file).unwrap(),
                file,
                lease: pin,
                csp: policy(&["'sha256-abc='".into()]),
            })
            .unwrap();
        let second_path = second
            .url
            .strip_prefix(&format!("http://{}", fixture.host))
            .unwrap();
        assert_ne!(second_path, fixture.path);
        assert_eq!(fixture.request("GET", second_path, "").0, 200);
        assert_eq!(fixture.request("GET", &fixture.path, "").0, 410);
        second.revoke();
        assert!(fixture.reference_released());
        assert_eq!(fixture.service.routes.lock().unwrap().len(), 2);
    }
    #[test]
    #[ignore = "Requires loopback sockets; run explicitly in permitted integration environment"]
    fn http_deadline_change_during_receive_releases_pin_and_keeps_tombstone() {
        let fixture = HttpFixture::new();
        fixture.lease.route.lock().unwrap().deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(fixture.request("GET", &fixture.path, "").0, 410);
        assert!(!fixture.lease.active());
        assert!(fixture.reference_released());
        assert!(fixture.lease.route.lock().unwrap().active.is_none());
        assert_eq!(fixture.request("HEAD", &fixture.path, "").0, 410);
        assert_eq!(fixture.service.routes.lock().unwrap().len(), 1);
    }
    #[test]
    fn revocation_invalidates_a_render_already_in_progress() {
        let generations = Arc::new(Mutex::new(BTreeMap::new()));
        let generation = next_generation(&mut generations.lock().unwrap(), "run").unwrap();
        let rendering = Arc::new(std::sync::Barrier::new(2));
        let install = Arc::new(std::sync::Barrier::new(2));
        let worker = {
            let generations = generations.clone();
            let rendering = rendering.clone();
            let install = install.clone();
            std::thread::spawn(move || {
                rendering.wait();
                install.wait();
                generation_current(&generations.lock().unwrap(), "run", generation)
            })
        };
        rendering.wait();
        next_generation(&mut generations.lock().unwrap(), "run").unwrap();
        install.wait();
        assert!(!worker.join().unwrap());
        let next = next_generation(&mut generations.lock().unwrap(), "run").unwrap();
        assert!(generation_current(
            &generations.lock().unwrap(),
            "run",
            next
        ));
        assert!(!generation_current(
            &generations.lock().unwrap(),
            "run",
            generation
        ));
    }
}
