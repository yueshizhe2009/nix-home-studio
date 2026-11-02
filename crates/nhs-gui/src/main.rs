//! Entry point: `--check` for the headless path, otherwise the window.

fn main() {
    if std::env::args().any(|argument| argument == "--flow-check") {
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

    if std::env::args().any(|argument| argument == "--check") {
        match nhs_gui::check::run() {
            Ok(state) => println!("{state}"),
            Err(error) => {
                eprintln!("{}", describe(&error));
                std::process::exit(1);
            }
        }
        return;
    }

    if let Err(error) = nhs_gui::runtime::run() {
        eprintln!("Nix Home Studio could not start: {error}");
        std::process::exit(1);
    }
}

fn describe(error: &nhs_gui::bridge::BridgeError) -> String {
    match serde_json::to_string_pretty(error) {
        Ok(text) => text,
        Err(_) => format!("{} ({})", error.message, error.code),
    }
}
