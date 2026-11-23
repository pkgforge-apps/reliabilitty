//! Reliabilitty, a CPU-rendered X11 terminal emulator.

mod app;
mod cli;
mod config;
mod fonts;
mod input;
mod pty;
mod render;
mod scale;
mod scrollbar;
mod signals;
mod theme;
mod vt;
mod x11;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let action = match cli::parse(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("rtty: {error}");
            eprintln!("Try `rtty --help`.");
            std::process::exit(2);
        }
    };
    match action {
        cli::Action::Help => print!("{}", cli::help()),
        cli::Action::Version => println!("rtty {VERSION}"),
        cli::Action::Run(cli) => {
            if let Err(error) = app::run(cli) {
                eprintln!("rtty: {error}");
                std::process::exit(1);
            }
        }
    }
}
