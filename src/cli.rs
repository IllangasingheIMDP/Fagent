use clap::{Parser, Subcommand};

use crate::config::ProviderKind;

#[derive(Debug, Parser)]
#[command(
    name = "fagent",
    version,
    about = "Plan and execute natural-language file operations with an approval gate.",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(value_name = "INSTRUCTION")]
    pub instruction: Option<String>,

    #[arg(long, global = true, value_enum)]
    pub provider: Option<ProviderKind>,

    #[arg(long, global = true)]
    pub model: Option<String>,

    #[arg(long, global = true, default_value_t = 1)]
    pub scan_depth: usize,

    #[arg(long, global = true, default_value_t = false)]
    pub allow_global: bool,

    #[arg(long, global = true, default_value_t = false)]
    pub permanent_delete: bool,

    #[arg(long, short, global = true, default_value_t = false)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Setup,
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn default_scan_depth_is_one() {
        let cli = Cli::parse_from(["fagent", "organize reports"]);
        assert_eq!(cli.scan_depth, 1);
    }

    #[test]
    fn custom_scan_depth_is_respected() {
        let cli = Cli::parse_from(["fagent", "--scan-depth", "4", "organize reports"]);
        assert_eq!(cli.scan_depth, 4);
    }
}
