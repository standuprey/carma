//! `carma` command-line front end. Parses `record -o <file>`, drives the
//! engine facade and prints the result. It never reaches into clock, audio-io
//! or recorder directly.

use std::path::PathBuf;
use std::process::ExitCode;

use engine::CarmaEngine;
use interfaces::Engine;

const USAGE: &str = "\
carma — modular audio recorder

USAGE:
    carma record [-o <out.wav>]   record from the built-in fake source until Ctrl-C

OPTIONS:
    -o, --output <out.wav>   output file (default: out.wav)
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    };

    match cmd.as_str() {
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

fn record(mut args: impl Iterator<Item = String>) -> ExitCode {
    let mut out = PathBuf::from("out.wav");
    while let Some(arg) = args.next() {
        match arg.as_str() {
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

    print!(
        "recording from FakeAudioSource to {} — press Ctrl-C to stop\n",
        out.display()
    );

    let mut engine = CarmaEngine::new();
    if let Err(e) = engine.record_until_signal(&out) {
        eprintln!("recording failed: {e}");
        return ExitCode::FAILURE;
    }

    let status = engine.status();
    println!(
        "wrote {} frames ({}s stereo 48kHz 24-bit) to {}",
        status.written_frames,
        status.written_frames as f64 / 48_000.0,
        out.display()
    );
    ExitCode::SUCCESS
}