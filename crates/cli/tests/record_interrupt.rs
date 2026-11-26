//! Command-boundary tests for `carma record`: SIGINT must stop the take and
//! finalize a valid WAV. Paced by the built-in fake source via the
//! `CARMA_CAPTURE=fake` test hook, so no PipeWire device or daemon is needed.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Unique scratch directory for one run.
fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("carma_cli_{}_{}", label, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sigint_stops_record_and_finalizes_valid_wav() {
    let dir = scratch("sigint");
    let started = Instant::now();

    let child = Command::new(env!("CARGO_BIN_EXE_carma"))
        .env("CARMA_CAPTURE", "fake")
        .args(["record", "-d", "test", "-o", "out.wav"])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("carma binary must start");

    std::thread::sleep(Duration::from_millis(600));
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill must run");
    assert!(kill.success(), "SIGINT must be deliverable");

    let out = child.wait_with_output().expect("carma must exit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean Ctrl-C stop must exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("wrote"),
        "CLI must report the written frames, got: {stdout}"
    );

    let h = recorder::read_wav_header(&dir.join("out.wav")).expect("output must be a valid WAV");
    assert!(h.num_frames > 0, "take must not be empty");
    let expected = started.elapsed().as_secs_f64() * 48_000.0;
    let delta = h.num_frames as f64 - expected;
    assert!(
        delta.abs() < 48_000.0,
        "recorded length must track elapsed time: {h:?} vs {expected:.0} frames"
    );

    std::fs::remove_dir_all(&dir).ok();
}
