//! stdio transport: one JSON-RPC message per line on stdin / stdout.
//!
//! Each message goes to the running FemCraft window when one listens on the HTTP port (so MCP
//! clients that only start commands, such as Claude Desktop, drive the open window); with no
//! window it is handled here, windowless, by `ws`. Decided per message: a window opened later
//! is used from then on.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::Workspace;

pub fn run(ws: &mut dyn Workspace) -> std::io::Result<()> {
    let port = std::env::var("FEMCRAFT_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(crate::DEFAULT_PORT);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match forward(port, &line) {
            Some(r) => r,
            None => crate::handle_text(ws, &line),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Sends one message to the window on `port`. `None` when no window listens (handle it
/// here); `Some(None)` when it answered without a body (a notification).
fn forward(port: u16, body: &str) -> Option<Option<String>> {
    if port == 0 {
        return None;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    // From here on the window has the message: failures are reported, not retried here.
    Some(exchange(&mut s, port, body).unwrap_or_else(|e| error_reply(body, &e)))
}

fn exchange(s: &mut TcpStream, port: u16, body: &str) -> Result<Option<String>, String> {
    // Solving can take minutes; the window gives up on its own after its timeout.
    s.set_read_timeout(Some(Duration::from_secs(30 * 60)))
        .map_err(|e| e.to_string())?;
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| format!("FemCraft window: {e}"))?;
    let mut r = BufReader::new(s);
    let mut status = String::new();
    r.read_line(&mut status).map_err(|e| e.to_string())?;
    let code = status.split_whitespace().nth(1).unwrap_or("");
    let mut len = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut buf = Vec::new();
    match len {
        Some(n) => {
            buf.resize(n, 0);
            r.read_exact(&mut buf).map_err(|e| e.to_string())?;
        }
        None => {
            r.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        }
    }
    match code {
        "200" => Ok(Some(String::from_utf8_lossy(&buf).into_owned())),
        "202" => Ok(None),
        "503" => {
            Err("the FemCraft window did not answer in time (busy or a dialog is open)".into())
        }
        _ => Err(format!("the FemCraft window answered '{}'", status.trim())),
    }
}

/// A JSON-RPC error for the message's id (nothing for a notification).
fn error_reply(body: &str, msg: &str) -> Option<String> {
    let id = serde_json::from_str::<Json>(body).ok()?.get("id")?.clone();
    Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": msg}}).to_string())
}
