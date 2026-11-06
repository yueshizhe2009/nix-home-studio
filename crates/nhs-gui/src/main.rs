//! Entry point: `--check` for the headless path, otherwise the window.

const USAGE: &str = "\
usage:
  nhs-gui                 open the window
  nhs-gui --check         print the discovery state as JSON and exit
  nhs-gui --flow-check    run the whole flow against a throw-away HOME and exit
  nhs-gui --help          print this help
  nhs-gui --version       print the version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Window,
    Help,
    Version,
    Check,
    FlowCheck,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mode(&args) {
        Ok(Mode::Help) => {
            println!("{USAGE}");
            return;
        }
        Ok(Mode::Version) => {
            println!("nhs-gui {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(Mode::Check) => {
            match nhs_gui::check::run() {
                Ok(state) => println!("{state}"),
                Err(error) => {
                    eprintln!("{}", describe(&error));
                    std::process::exit(1);
                }
            }
            return;
        }
        Ok(Mode::FlowCheck) => {
            match nhs_gui::flowcheck::run() {
                Ok(report) => println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
                ),
                Err(error) => {
                    eprintln!("{}", describe(&error));
                    std::process::exit(1);
                }
            }
            return;
        }
        Ok(Mode::Window) => {}
        Err(message) => {
            eprintln!("nhs-gui: {message}\n{USAGE}");
            std::process::exit(2);
        }
    }

    if let Err(error) = nhs_gui::runtime::run() {
        eprintln!("Nix Home Studio could not start: {error}");
        std::process::exit(1);
    }
}

/// Parse exactly one supported option; anything else is a usage error.
///
/// The window is the only mode that initialises GTK, so help, version and the
/// headless checks cannot fail just because no display is available.
fn mode(args: &[String]) -> Result<Mode, String> {
    match args {
        [] => Ok(Mode::Window),
        [argument] if argument == "--check" => Ok(Mode::Check),
        [argument] if argument == "--flow-check" => Ok(Mode::FlowCheck),
        [argument] if argument == "--help" || argument == "-h" => Ok(Mode::Help),
        [argument] if argument == "--version" || argument == "-V" => Ok(Mode::Version),
        [argument] => Err(format!("unknown argument `{argument}`")),
        _ => Err(format!(
            "expected one option, got {} arguments; pass --help for usage",
            args.len()
        )),
    }
}

fn describe(error: &nhs_gui::bridge::BridgeError) -> String {
    match serde_json::to_string_pretty(error) {
        Ok(text) => text,
        Err(_) => format!("{} ({})", error.message, error.code),
    }
}
