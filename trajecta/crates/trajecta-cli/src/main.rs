//! Trajecta command-line binary entry point.

fn main() {
    std::process::exit(trajecta_cli::main_entry(std::env::args_os()));
}
