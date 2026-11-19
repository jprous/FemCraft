//! Solve an `.rf6` with the RFEM 6 server (`RFEM6Server.exe`), through RFEM's API I web
//! service (SOAP over HTTP on `127.0.0.1:<port>`, 8081 by default).
//!
//! [`solve`] copies the model into a working folder, starts the server if nothing answers on
//! the port, opens the copy, runs `calculate_all`, saves it (results land under `!solver/`)
//! and closes it again. A server this call started is stopped afterwards; one that was
//! already running (the RFEM window, or a server started by hand) is left alone.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 8081;
pub const DEFAULT_SERVER: &str = r"C:\Program Files\Dlubal\RFEM 6.16\bin\RFEM6Server.exe";

const NS: &str = "http://www.dlubal.com/rfem.xsd";

/// Where the RFEM server is and how to start it.
#[derive(Debug, Clone)]
pub struct RfemConfig {
    /// `RFEM6Server.exe`.
    pub server_exe: PathBuf,
    pub port: u16,
    /// Dlubal account the server logs in with (`--email`). The password is never stored: it
    /// is read from the `FEMCRAFT_RFEM_PASSWORD` environment variable.
    pub email: Option<String>,
    /// Licence to use (`--license`), optional.
    pub license: Option<String>,
}

/// Environment variable holding the Dlubal account password for the server (overrides the
/// saved one).
pub const PASSWORD_ENV: &str = "FEMCRAFT_RFEM_PASSWORD";

const CREDENTIAL_SERVICE: &str = "FemCraft RFEM server";

/// Saves the Dlubal password for `email` in the Windows Credential Manager (not in
/// FemCraft's settings files). An empty password removes it.
pub fn store_password(email: &str, password: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(CREDENTIAL_SERVICE, email).map_err(|e| e.to_string())?;
    if password.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    entry.set_password(password).map_err(|e| e.to_string())
}

/// True when a password is saved for `email`.
pub fn has_saved_password(email: &str) -> bool {
    saved_password(email).is_some()
}

fn saved_password(email: &str) -> Option<String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, email)
        .ok()?
        .get_password()
        .ok()
        .filter(|p| !p.is_empty())
}

/// The password to log in with: the environment variable, else the saved one.
fn password_for(email: &str) -> Option<String> {
    std::env::var(PASSWORD_ENV)
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| saved_password(email))
}

impl Default for RfemConfig {
    fn default() -> Self {
        Self {
            server_exe: PathBuf::from(DEFAULT_SERVER),
            port: DEFAULT_PORT,
            email: std::env::var("FEMCRAFT_RFEM_EMAIL").ok().filter(|e| !e.is_empty()),
            license: std::env::var("FEMCRAFT_RFEM_LICENSE").ok().filter(|e| !e.is_empty()),
        }
    }
}

/// One HTTP POST; returns the response body.
fn http_post(addr: SocketAddr, path: &str, action: &str, body: &str) -> Result<String, String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("no RFEM server on {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(900))).ok();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/xml; charset=utf-8\r\n\
         SOAPAction: \"{action}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    // The server may keep the connection open, so stop at the end of the message (its
    // Content-Length, or the last chunk), not at the end of the stream.
    let mut raw = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = s.read(&mut buf).map_err(|e| e.to_string())?;
        raw.extend_from_slice(&buf[..n]);
        if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
            let body = &raw[split + 4..];
            let done = if head.contains("transfer-encoding: chunked") {
                body.ends_with(b"0\r\n\r\n")
            } else if let Some(len) = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
            {
                body.len() >= len
            } else {
                false // no length: read to the end of the stream
            };
            if done || n == 0 {
                let data = if head.contains("transfer-encoding: chunked") {
                    dechunk(body)
                } else {
                    body.to_vec()
                };
                return Ok(String::from_utf8_lossy(&data).into_owned());
            }
        } else if n == 0 {
            return Err("bad HTTP response from the RFEM server".into());
        }
    }
}

fn dechunk(mut d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = d.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&d[..i]).trim(), 16).unwrap_or(0);
        if size == 0 || d.len() < i + 2 + size {
            break;
        }
        out.extend_from_slice(&d[i + 2..i + 2 + size]);
        d = &d[(i + 2 + size + 2).min(d.len())..];
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Text of the first element called `tag` (any namespace prefix).
fn element(xml: &str, tag: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = xml[from..].find(tag) {
        let at = from + i;
        let before = xml[..at].chars().next_back();
        let after = xml[at + tag.len()..].chars().next();
        if matches!(before, Some('<') | Some(':')) && after == Some('>') {
            let start = at + tag.len() + 1;
            let end = xml[start..].find("</")? + start;
            return Some(xml[start..end].trim().to_owned());
        }
        from = at + tag.len();
    }
    None
}

/// Texts of every element called `tag`.
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(v) = element(rest, tag) {
        let Some(i) = rest.find(&format!("{tag}>")) else { break };
        out.push(v);
        rest = &rest[i + tag.len() + 1..];
    }
    out
}

/// An API I call on the endpoint `url_path` (`/` for the application, or a model's path).
fn call(
    addr: SocketAddr,
    url_path: &str,
    method: &str,
    args: &[(&str, String)],
) -> Result<String, String> {
    let params: String = args
        .iter()
        .map(|(k, v)| format!("<{k}>{}</{k}>", xml_escape(v)))
        .collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><soap:Envelope \
         xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body>\
         <{method} xmlns=\"{NS}\">{params}</{method}></soap:Body></soap:Envelope>"
    );
    let action = format!("http://localhost:{}{url_path}{method}", addr.port());
    let trace = std::env::var_os("FEMCRAFT_TRACE").is_some();
    if trace {
        eprintln!("-> {url_path} {method} {args:?}");
    }
    let reply = http_post(addr, url_path, &action, &xml)?;
    if trace {
        eprintln!("<- {}", &reply[..reply.len().min(400)]);
    }
    if reply.contains("Fault>") {
        let msg = element(&reply, "faultstring").unwrap_or_else(|| reply.clone());
        return Err(format!("RFEM: {method}: {msg}"));
    }
    Ok(reply)
}

/// Path part of a model address returned by RFEM (`http://localhost:8081/1234/`).
fn url_path(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[i..].to_owned(),
        None => "/".to_owned(),
    }
}

/// Address of a model service from its URL (`http://127.0.0.1:8083/`).
fn url_addr(url: &str) -> Option<SocketAddr> {
    let rest = url.split("://").nth(1)?;
    let host_port = rest.split('/').next()?;
    let port: u16 = host_port.rsplit(':').next()?.parse().ok()?;
    Some(([127, 0, 0, 1], port).into())
}

/// True when an RFEM web service answers on `addr`.
pub fn answers(addr: SocketAddr) -> bool {
    call(addr, "/", "get_information", &[]).is_ok()
}

/// What a solve produced.
#[derive(Debug, Clone)]
pub struct Solved {
    /// The calculated model (a copy of the input, with results).
    pub path: PathBuf,
    pub seconds: f64,
    pub started_server: bool,
}

/// Calculates `input` with the RFEM server into `out_dir/<name>.rf6`.
pub fn solve(cfg: &RfemConfig, input: &Path, out_dir: &Path) -> Result<Solved, String> {
    run_job(cfg, Job::Calculate, input, out_dir)
}

/// What the server does with the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// `calculate_all`: mesh and solve; the file gets results and the FE mesh.
    Calculate,
    /// `generate_mesh`: only the FE mesh (`!solver/M/`), no results.
    Mesh,
}

impl Job {
    fn method(self) -> &'static str {
        match self {
            Job::Calculate => "calculate_all",
            Job::Mesh => "generate_mesh",
        }
    }
}

/// Generates the FE mesh of `input` with the RFEM server into `out_dir/<name>.rf6`.
pub fn mesh(cfg: &RfemConfig, input: &Path, out_dir: &Path) -> Result<Solved, String> {
    run_job(cfg, Job::Mesh, input, out_dir)
}

/// Copies `input` to `out_dir`, runs `job` on the copy and returns it.
pub fn run_job(cfg: &RfemConfig, job: Job, input: &Path, out_dir: &Path) -> Result<Solved, String> {
    run_job_with(cfg, job, input, out_dir, &|_| {})
}

/// [`run_job`], reporting what it is doing (for a progress dialog) through `progress`.
pub fn run_job_with(
    cfg: &RfemConfig,
    job: Job,
    input: &Path,
    out_dir: &Path,
    progress: &dyn Fn(&str),
) -> Result<Solved, String> {
    let t0 = Instant::now();
    progress("Preparing the model…");
    if !input.is_file() {
        return Err(format!(
            "{} does not exist; save the model first",
            input.display()
        ));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let name = input.file_name().ok_or("model has no file name")?;
    let target = out_dir.join(name);
    if std::path::absolute(input).ok() != std::path::absolute(&target).ok() {
        std::fs::copy(input, &target).map_err(|e| format!("copy to {}: {e}", target.display()))?;
    }
    let addr: SocketAddr = ([127, 0, 0, 1], cfg.port).into();
    let mut child = None;
    let started_server = !answers(addr);
    if started_server {
        progress("Starting the RFEM server (this can take half a minute)…");
        child = Some(start_server(cfg, addr)?);
    }
    let result = run(addr, &target, job, progress);
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    result?;
    Ok(Solved {
        path: target,
        seconds: t0.elapsed().as_secs_f64(),
        started_server,
    })
}

fn start_server(cfg: &RfemConfig, addr: SocketAddr) -> Result<Child, String> {
    if !cfg.server_exe.is_file() {
        return Err(format!(
            "RFEM server not found at {} (Settings > RFEM server)",
            cfg.server_exe.display()
        ));
    }
    // Without a login the server stops right after "Missing login credentials".
    let email = cfg.email.as_deref().filter(|e| !e.is_empty());
    let password = email.and_then(password_for);
    let (Some(email), Some(password)) = (email, password) else {
        return Err("The RFEM server needs a Dlubal login: enter the account e-mail and \
             password in Settings > RFEM server… (or start RFEM, which is then used instead)."
            .to_owned());
    };
    let mut cmd = Command::new(&cfg.server_exe);
    cmd.arg("--email").arg(email).arg("--password").arg(password);
    if let Some(l) = cfg.license.as_deref().filter(|l| !l.is_empty()) {
        cmd.arg("--license").arg(l);
    }
    // Without a port the server logs in, then "No valid port specified for WS server" and
    // exits with code 1.
    cmd.arg("--start-soap-server").arg(cfg.port.to_string());
    cmd.current_dir(cfg.server_exe.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", cfg.server_exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if answers(addr) {
            return Ok(child);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the RFEM server exited ({status}) before answering on port {}: wrong login, no \
                 free licence, or another RFEM holds the port (see the logs in \
                 %LOCALAPPDATA%\\Dlubal\\RFEM6_6.16\\logs)",
                addr.port()
            ));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("the RFEM server did not answer within 120 s".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Open, calculate, save, close; the file is complete when it stops changing.
fn run(addr: SocketAddr, file: &Path, job: Job, progress: &dyn Fn(&str)) -> Result<(), String> {
    let path = file.to_string_lossy().into_owned();
    let before = std::fs::metadata(file).and_then(|m| m.modified()).ok();
    let indexes = || {
        call(addr, "/", "get_model_list_with_indexes", &[])
            .map(|l| elements(&l, "index"))
            .unwrap_or_default()
    };
    let before_open = indexes();
    progress("Opening the model in RFEM…");
    let r = call(addr, "/", "open_model", &[("model_path", path.clone())])?;
    let url = element(&r, "value").ok_or("RFEM did not return the opened model")?;
    // Each open model is served on its own port (`http://127.0.0.1:8083/`).
    let maddr = url_addr(&url).unwrap_or(addr);
    let mp = url_path(&url);
    let outcome = (|| {
        progress(match job {
            Job::Calculate => "RFEM is calculating…",
            Job::Mesh => "RFEM is generating the FE mesh…",
        });
        let r = call(maddr, &mp, job.method(), &[("skip_warnings", "true".into())])?;
        if element(&r, "succeeded").as_deref() == Some("false") {
            let what = element(&r, "messages").unwrap_or_default();
            let object = element(&r, "object").unwrap_or_default();
            let field = element(&r, "input_field").unwrap_or_default();
            let msg = element(&r, "message").unwrap_or_default();
            return Err(format!("RFEM could not calculate: {what} {object} {field} {msg}"));
        }
        progress("Saving the model with RFEM…");
        call(maddr, &mp, "save", &[("file_path", path.clone())])?;
        wait_written(file, before)
    })();
    // Close the model we opened (never one that was open before), whatever happened.
    progress("Closing the model…");
    if let Some(index) = indexes().into_iter().find(|i| !before_open.contains(i)) {
        let _ = call(
            addr,
            "/",
            "close_model",
            &[("index", index), ("save_changes", "false".into())],
        );
    }
    outcome
}

/// RFEM's `save` returns before the file is written: wait until it changed and is stable.
fn wait_written(file: &Path, before: Option<std::time::SystemTime>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = (0u64, None);
    let mut stable = 0;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(m) = std::fs::metadata(file) else {
            continue;
        };
        let now = (m.len(), m.modified().ok());
        if now.1 != before && now == last {
            stable += 1;
            if stable >= 3 {
                return Ok(());
            }
        } else {
            stable = 0;
        }
        last = now;
    }
    Err("RFEM did not write the calculated model in time".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_helpers() {
        let x = "<s:Body><ns:open_modelResponse><ns:value>http://localhost:8081/7/</ns:value></ns:open_modelResponse></s:Body>";
        assert_eq!(element(x, "value").unwrap(), "http://localhost:8081/7/");
        assert_eq!(url_path("http://localhost:8081/7/"), "/7/");
        assert_eq!(url_addr("http://127.0.0.1:8083/").unwrap().port(), 8083);
        assert_eq!(dechunk(b"5\r\nhello\r\n0\r\n\r\n"), b"hello");
    }
}
