use clap::Parser;
use std::path::PathBuf;
#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Command {
    /// The config file to use
    #[arg(short, long, default_value = "configs/config.yml")]
    pub config: PathBuf,
}
pub fn parse_args() -> Command {
    Command::parse()
}
