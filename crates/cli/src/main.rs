//! `carma` command-line front end. Parses `devices` and `record -d <device>`,
//! drives the engine facade and prints the result. It never reaches into
//! clock, audio-io or recorder directly.

use std::path::PathBuf;
use std::process::ExitCode;

use engine::CarmaEngine;
use interfaces::{Engine, SAMPLE_RATE};

const USAGE: &str = "\
carma — modular audio recorder

USAGE:
    carma devices                          list available capture devices
    carma record -d <device> [-o <out.wav>]  record from a capture device until Ctrl-C

OPTIONS:
    -d, --device <device>   capture device to record from (required)
    -o, --output <out.wav>  output file (default: out.wav)
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    };

    match cmd.as_str() {
        "devices" => devices(),
        "record" => record(args),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Lists available capture devices and prints their friendly descriptions.
fn devices() -> ExitCode {
    match CarmaEngine::list_devices() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("(no capture devices found)");
                return ExitCode::SUCCESS;
            }
            for device in &devices {
                println!("{}", device.description);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to list devices: {e}");
            ExitCode::FAILURE
        }
    }
}

fn record(mut args: impl Iterator<Item = String>) -> ExitCode {
    let mut out = PathBuf::from("out.wav");
    let mut device: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-d" | "--device" => match args.next() {
                Some(v) => device = Some(v),
                None => {
                    eprintln!("missing value for {arg}\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "-o" | "--output" | "--out" => match args.next() {
                Some(v) => out = PathBuf::from(v),
                None => {
                    eprintln!("missing value for {arg}\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown option: {other}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(selector) = device else {
        eprintln!("missing required device: -d <device>\n\n{USAGE}");
        return ExitCode::from(2);
    };

    let device = match CarmaEngine::find_capture_device(&selector) {
        Ok(Some(d)) => d,
        Ok(None) => {
            eprintln!("no capture device matches: {selector}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("failed to look up device: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut engine = match CarmaEngine::for_device(device) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("failed to open device: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "recording from {} to {} — press Ctrl-C to stop\n",
        selector,
        out.display()
    );

    if let Err(e) = engine.record_until_signal(&out) {
        eprintln!("recording failed: {e}");
        return ExitCode::FAILURE;
    }

    let status = engine.status();
    println!(
        "wrote {} frames ({}s stereo {}Hz 24-bit) to {}",
        status.written_frames,
        status.written_frames as f64 / SAMPLE_RATE as f64,
        SAMPLE_RATE,
        out.display()
    );
    ExitCode::SUCCESS
}
