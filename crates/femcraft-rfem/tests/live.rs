//! Calculates a model with a real RFEM 6 server. Skipped unless `FEMCRAFT_TEST_SOLVE` names an
//! `.rf6`; uses an RFEM already answering on the port, else starts `RFEM6Server.exe`.

use std::path::Path;

#[test]
fn solve_with_rfem() {
    let Some(model) = std::env::var_os("FEMCRAFT_TEST_SOLVE") else {
        eprintln!("skipped: FEMCRAFT_TEST_SOLVE not set");
        return;
    };
    let out = tempfile_dir();
    let done = femcraft_rfem::solve(&femcraft_rfem::RfemConfig::default(), Path::new(&model), &out)
        .expect("solve");
    let zip = std::fs::read(&done.path).unwrap();
    let has_results = zip.windows(8).any(|w| w == b"!solver/");
    eprintln!("solved in {:.1} s, started server: {}", done.seconds, done.started_server);
    assert!(has_results, "no !solver/ entries in {}", done.path.display());
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("femcraft-solve-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
