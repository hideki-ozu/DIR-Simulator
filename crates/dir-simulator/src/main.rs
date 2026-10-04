use dir_simulator::{Diagnostic, prepare, run_with_diagnostics, viewer};
use serde_json::json;
use std::path::Path;
use std::process::ExitCode;

const HELP: &str = "DIR Simulator — Classical CAN, CAN FD, Gateway and Ethernet L2/QoS/VLAN/media\n\nUsage:\n  dir-simulator validate --config PATH\n  dir-simulator run --config PATH --output DIR\n  dir-simulator view --input results.json --output viewer.html\n  dir-simulator ned-editor [--config PATH] [--export-root DIR] [--state-root DIR]\n  dir-simulator --help\n  dir-simulator --version\n\nInput: NED topology, INI settings and JSON workload.\nOutput: manifest.json, results.json, events.csv, summary.csv, diagnostics.jsonl\nViewer: standalone local HTML; preserves existing files and opens no browser.\n";

fn diagnostic(d: &Diagnostic) {
    eprintln!("{}", serde_json::to_string(d).expect("diagnostic JSON"));
}
fn execute(args: &[String]) -> Result<u8, Diagnostic> {
    if args.first().is_some_and(|a| a == "ned-editor") {
        if args.len() == 2 && args[1] == "--help" {
            println!(
                "dir-simulator ned-editor [--config PATH] [--export-root DIR] [--state-root DIR]"
            );
            return Ok(0);
        }
        let mut options = std::collections::BTreeMap::new();
        let mut i = 1;
        while i < args.len() {
            let key = &args[i];
            if !["--config", "--export-root", "--state-root"].contains(&key.as_str())
                || options.contains_key(key)
            {
                return Err(Diagnostic::prepare(format!(
                    "unknown or duplicate option {key}"
                )));
            }
            let value = args
                .get(i + 1)
                .filter(|v| !v.is_empty() && !v.starts_with("--"))
                .ok_or_else(|| Diagnostic::prepare(format!("missing value for {key}")))?;
            options.insert(key.clone(), std::path::PathBuf::from(value));
            i += 2;
        }
        let config = options.remove("--config");
        dir_simulator::tool::ned_editor::serve(dir_simulator::tool::ned_editor::EditorOptions {
            config,
            export_root: options.remove("--export-root"),
            state_root: options.remove("--state-root"),
        })?;
        return Ok(0);
    }
    if args == ["--version"] {
        println!("dir-simulator {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if args == ["--help"]
        || (args.len() == 2
            && ["run", "validate", "view"].contains(&args[0].as_str())
            && args[1] == "--help")
    {
        print!("{HELP}");
        return Ok(0);
    }
    let command = args.first().map(String::as_str).unwrap_or("");
    if !["run", "validate", "view"].contains(&command) {
        return Err(Diagnostic::prepare(
            "expected validate, run or view; see --help",
        ));
    }
    let (mut config, mut output, mut input) = (None, None, None);
    let mut index = 1;
    while index < args.len() {
        let slot = match args[index].as_str() {
            "--config" if command != "view" => &mut config,
            "--input" if command == "view" => &mut input,
            "--output" if command == "run" || command == "view" => &mut output,
            other => return Err(Diagnostic::prepare(format!("unknown option {other}"))),
        };
        if slot.is_some() {
            return Err(Diagnostic::prepare(format!(
                "duplicate option {}",
                args[index]
            )));
        }
        let value = args
            .get(index + 1)
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| Diagnostic::prepare(format!("missing value for {}", args[index])))?;
        *slot = Some(value.as_str());
        index += 2;
    }
    if command == "view" {
        let input = input.ok_or_else(|| Diagnostic::prepare("--input is required"))?;
        let output = output.ok_or_else(|| Diagnostic::prepare("--output is required"))?;
        let path = viewer::write(Path::new(input), Path::new(output))?;
        println!(
            "{}",
            json!({"schema_version":1,"status":"complete","viewer_path":path})
        );
        return Ok(0);
    }
    let config = config.ok_or_else(|| Diagnostic::prepare("--config is required"))?;
    if command == "run" && output.is_none() {
        return Err(Diagnostic::prepare("--output is required"));
    }
    let prepared = prepare(Path::new(config))?;
    if command == "validate" {
        println!(
            "{}",
            json!({"schema_version":1,"status":"valid","network":prepared.common.network,"node_count":prepared.ethernet.as_ref().map_or(prepared.canfd.as_ref().map_or(prepared.can.controllers.len()+prepared.can.buses.len(), |fd| fd.controllers.len()+1), |ethernet| ethernet.devices.len()).to_string(),"channel_count":prepared.common.channel_count.to_string()})
        );
        return Ok(0);
    }
    let report = match run_with_diagnostics(prepared, Path::new(output.unwrap())) {
        Ok(report) => report,
        Err(failure) => {
            for d in &failure.prior_diagnostics {
                diagnostic(d);
            }
            return Err(failure.diagnostic);
        }
    };
    for d in &report.diagnostics {
        diagnostic(d);
    }
    println!(
        "{}",
        serde_json::to_string(&report).expect("run report JSON")
    );
    Ok(report.exit_code)
}
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match execute(&args) {
        Ok(code) => ExitCode::from(code),
        Err(d) => {
            diagnostic(&d);
            let code = match d.code.as_str() {
                "E-0003" => 4,
                "E-0002" | "E-0004" => 3,
                _ => 2,
            };
            if args.first().is_some_and(|a| a == "run") {
                let termination = match code {
                    4 => "output_failed",
                    3 => "execution_failed",
                    _ => "prep_failed",
                };
                println!(
                    "{}",
                    json!({"schema_version":1,"termination":termination,"exit_code":code,"partial":true,"output_path":null,"manifest_path":null})
                );
            }
            ExitCode::from(code)
        }
    }
}
