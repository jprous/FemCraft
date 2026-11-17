//! HTTP transport for the running app: MCP "streamable HTTP" with plain JSON responses (no
//! server-sent events), on 127.0.0.1 only.
//!
//! The listener runs on its own threads; each request is passed to the UI thread as a
//! [`Request`] (the models live there) and answered with [`Request::answer`].

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use crate::Workspace;

/// How long a request waits for the UI thread.
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_BODY: usize = 16 << 20;

/// One MCP message waiting for the UI thread.
pub struct Request {
    body: String,
    reply: mpsc::Sender<Option<String>>,
}

impl Request {
    pub fn answer(self, ws: &mut dyn Workspace) {
        let _ = self.reply.send(crate::handle_text(ws, &self.body));
    }
}

/// Listens on `127.0.0.1:port`; `wake` is called after each request is queued (to repaint the
/// window so the UI thread picks it up).
pub fn serve(
    port: u16,
    wake: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<mpsc::Receiver<Request>> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let (tx, rx) = mpsc::channel();
    let wake = std::sync::Arc::new(wake);
    std::thread::Builder::new()
        .name("femcraft-mcp".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let (tx, wake) = (tx.clone(), wake.clone());
                std::thread::spawn(move || {
                    let _ = connection(stream, &tx, &*wake);
                });
            }
        })?;
    Ok(rx)
}

fn respond(s: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let ctype = if body.is_empty() {
        "text/plain"
    } else {
        "application/json"
    };
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    s.flush()
}

fn connection(
    mut stream: TcpStream,
    tx: &mpsc::Sender<Request>,
    wake: &dyn Fn(),
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (mut len, mut origin) = (0usize, None);
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let v = v.trim();
            if k.eq_ignore_ascii_case("content-length") {
                len = v.parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("origin") {
                origin = Some(v.to_owned());
            }
        }
    }
    // Browsers send an Origin: refuse pages from other sites (DNS rebinding).
    if let Some(o) = origin {
        let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"];
        if !local
            .iter()
            .any(|l| o == *l || o.starts_with(&format!("{l}:")))
        {
            return respond(&mut stream, "403 Forbidden", "");
        }
    }
    if path.split('?').next() != Some("/mcp") {
        return respond(&mut stream, "404 Not Found", "");
    }
    if method != "POST" {
        return respond(&mut stream, "405 Method Not Allowed", "");
    }
    if len > MAX_BODY {
        return respond(&mut stream, "413 Payload Too Large", "");
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let (reply_tx, reply_rx) = mpsc::channel();
    let req = Request {
        body: String::from_utf8_lossy(&body).into_owned(),
        reply: reply_tx,
    };
    if tx.send(req).is_err() {
        return respond(&mut stream, "503 Service Unavailable", "");
    }
    wake();
    match reply_rx.recv_timeout(TIMEOUT) {
        Ok(Some(json)) => respond(&mut stream, "200 OK", &json),
        Ok(None) => respond(&mut stream, "202 Accepted", ""),
        Err(_) => respond(&mut stream, "503 Service Unavailable", ""),
    }
}
