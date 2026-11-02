//! The bounds of a recursive listing, defined once for every driver's
//! `ls --recursive`.
//!
//! The walk stays in each driver, because what identifies a directory depends
//! on the format. What is shared is what a person sees: the arguments, their
//! help, their defaults and the error when a bound is reached.

use super::CliError;
use clap::{value_parser, Arg, ArgAction, ArgMatches};

/// How deep `--recursive` goes by default; a directory's entries are depth 1.
pub const MAX_DEPTH: u64 = 64;
/// How many entries `--recursive` lists by default.
pub const LIMIT: u64 = 100_000;

/// `--recursive` (`-R`), `--max-depth` and `--limit`, the last two requiring
/// the first.
pub fn recursive_args() -> [Arg; 3] {
    [
        Arg::new("recursive")
            .long("recursive")
            .short('R')
            .action(ArgAction::SetTrue)
            .help("Walk depth first, names sorted as bytes; never follow a symlink"),
        Arg::new("max-depth")
            .long("max-depth")
            .value_name("N")
            .value_parser(value_parser!(u64))
            .requires("recursive")
            .help(format!(
                "How deep --recursive may go (default: {MAX_DEPTH}); a directory's entries are depth 1"
            )),
        Arg::new("limit")
            .long("limit")
            .value_name("N")
            .value_parser(value_parser!(u64))
            .requires("recursive")
            .help(format!(
                "How many entries --recursive may list (default: {LIMIT})"
            )),
    ]
}

/// The bounds a walk was given, or their defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListBounds {
    /// The deepest entry to list.
    pub max_depth: u64,
    /// The most entries to list.
    pub limit: u64,
}

impl ListBounds {
    /// Read the bounds from matches of [`recursive_args`].
    pub fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            max_depth: matches
                .get_one::<u64>("max-depth")
                .copied()
                .unwrap_or(MAX_DEPTH),
            limit: matches.get_one::<u64>("limit").copied().unwrap_or(LIMIT),
        }
    }
}

/// The error for a walk stopped by a bound, naming where it stopped.
pub fn bound_reached(bound: &str, value: u64, at: &str) -> CliError {
    CliError::failed(format!("{bound} {value} reached at {at}"))
}
