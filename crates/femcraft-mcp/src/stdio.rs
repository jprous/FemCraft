//! stdio transport: one JSON-RPC message per line on stdin / stdout.

use std::io::{BufRead, Write};

use crate::Workspace;

pub fn run(ws: &mut dyn Workspace) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = crate::handle_text(ws, &line) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}
