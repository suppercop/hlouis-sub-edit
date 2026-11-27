//! `frame-check <video> <ass> [--max N]`: prints every event whose libass visibility disagrees with
//! our frame timing. Exit code 0 = clean, 1 = mismatches, 2 = could not run.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut max = 50usize;
    if let Some(pos) = args.iter().position(|a| a == "--max") {
        if let Some(v) = args.get(pos + 1).and_then(|v| v.parse().ok()) {
            max = v;
            args.drain(pos..=pos + 1);
        } else {
            eprintln!("error: --max needs a number");
            return ExitCode::from(2);
        }
    }
    let mut track: Option<u64> = None;
    if let Some(pos) = args.iter().position(|a| a == "--track") {
        if let Some(v) = args.get(pos + 1).and_then(|v| v.parse().ok()) {
            track = Some(v);
            args.drain(pos..=pos + 1);
        } else {
            eprintln!("error: --track needs a track number");
            return ExitCode::from(2);
        }
    }
    if args.first().map(String::as_str) == Some("--frame-check") {
        args.remove(0);
    }
    let [video, ass] = args.as_slice() else {
        eprintln!("usage: frame-check <video> <script.ass | file.mkv> [--max N] [--track N]");
        return ExitCode::from(2);
    };
    let ass_path = PathBuf::from(ass);
    let is_mkv = ass_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mkv") || e.eq_ignore_ascii_case("mks"));
    let outcome = if is_mkv {
        frame_check::check_embedded_track(&PathBuf::from(video), &ass_path, track)
    } else {
        frame_check::check_files(&PathBuf::from(video), &ass_path)
    };
    match outcome {
        Ok(report) => {
            print!("{}", report.summary(max));
            if report.is_clean() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
