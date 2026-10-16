use clap::Parser;
use wm_cli::cli::{run_cli, LuminaCtlCli};

fn main() {
    let cli = LuminaCtlCli::parse();
    match run_cli(&cli) {
        Ok(output) => {
            println!("{output}");
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
