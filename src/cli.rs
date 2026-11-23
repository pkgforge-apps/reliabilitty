//! Alacritty-style command-line parsing.

use std::path::PathBuf;

use crate::VERSION;

#[derive(Clone, Debug, Default)]
pub struct Cli {
    pub config_file: Option<PathBuf>,
    pub title: Option<String>,
    pub instance: Option<String>,
    pub class: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub hold: bool,
    pub smoke: bool,
    pub command: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    Run(Cli),
    Help,
    Version,
}

pub fn parse<I, S>(args: I) -> Result<Action, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut cli = Cli::default();
    let mut args = args.into_iter().map(Into::into);
    while let Some(arg) = args.next() {
        if arg == "--" {
            cli.command.extend(args);
            break;
        }
        if arg == "-e" || arg == "--command" {
            cli.command.extend(args);
            break;
        }
        if arg == "-h" || arg == "--help" {
            return Ok(Action::Help);
        }
        if arg == "-V" || arg == "--version" {
            return Ok(Action::Version);
        }
        if arg == "--hold" {
            cli.hold = true;
            continue;
        }
        if arg == "--smoke" {
            cli.smoke = true;
            continue;
        }
        if arg.starts_with('-')
            && let Some((flag, value)) = arg.split_once('=')
        {
            match flag {
                "--config-file" => cli.config_file = Some(PathBuf::from(value)),
                "--title" => cli.title = Some(value.to_string()),
                "--class" => cli.class = Some(value.to_string()),
                "--instance" => cli.instance = Some(value.to_string()),
                "--working-directory" => cli.working_directory = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown option {arg}")),
            }
            continue;
        }
        let option = match arg.as_str() {
            "--config-file" => Some(OptionKind::Config),
            "-t" | "--title" => Some(OptionKind::Title),
            "--class" => Some(OptionKind::Class),
            "--instance" => Some(OptionKind::Instance),
            "-d" | "--working-directory" => Some(OptionKind::Directory),
            _ => None,
        };
        if let Some(kind) = option {
            let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
            match kind {
                OptionKind::Config => cli.config_file = Some(PathBuf::from(value)),
                OptionKind::Title => cli.title = Some(value),
                OptionKind::Class => cli.class = Some(value),
                OptionKind::Instance => cli.instance = Some(value),
                OptionKind::Directory => cli.working_directory = Some(PathBuf::from(value)),
            }
            continue;
        }
        if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        }
        // A bare program starts the command.
        cli.command.push(arg);
        cli.command.extend(args);
        break;
    }
    Ok(Action::Run(cli))
}

#[derive(Clone, Copy)]
enum OptionKind {
    Config,
    Title,
    Class,
    Instance,
    Directory,
}

pub fn help() -> String {
    format!(
        "\
rtty {VERSION}
A small, CPU-rendered X11 terminal emulator.

Usage:
    rtty [OPTIONS] [--] [COMMAND [ARGS...]]
    rtty -e COMMAND [ARGS...]

Options:
    -e, --command COMMAND...   Program to run instead of the shell
    -t, --title TITLE          Window title
        --class CLASS          X11 WM_CLASS class
        --instance NAME        X11 WM_CLASS instance
    -d, --working-directory DIR
                               Directory the child starts in
        --config-file PATH     Configuration file
        --hold                 Stay open after the child exits
        --smoke                Draw one frame, then exit
    -h, --help                 Print this help
    -V, --version              Print the version

Configuration:
    $RTTY_CONFIG, or ~/.config/rtty/rtty.conf, or /etc/rtty/rtty.conf. See the example in the repository.
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_after_e() {
        let Action::Run(cli) = parse(["-e", "vim", "file.txt"]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.command, ["vim", "file.txt"]);
    }

    #[test]
    fn parses_options_around_command() {
        let Action::Run(cli) = parse(["--title", "Work", "-e", "htop"]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.title.as_deref(), Some("Work"));
        assert_eq!(cli.command, ["htop"]);
    }

    #[test]
    fn rejects_unknown_options() {
        assert!(parse(["--wat"]).is_err());
        assert!(parse(["--title"]).is_err());
    }

    #[test]
    fn parses_equals_form() {
        let Action::Run(cli) = parse(["--config-file=/tmp/x.conf"]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.config_file.unwrap(), PathBuf::from("/tmp/x.conf"));
    }
}
