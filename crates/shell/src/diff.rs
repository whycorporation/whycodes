//! `diff FROM TO` over the uutils diffutils library.
//!
//! The library exposes the option parser and the output formats but not its
//! `diff` entry point, so this is that entry point. Directory comparison
//! (`diff -r`) is not part of the library and is not supported.

use std::ffi::OsString;
use std::io::{self, Read, Write};

use diffutilslib::params::{Format, Params, parse_params};

/// Exit status: 0 identical, 1 different, 2 trouble.
pub(crate) fn main(args: Vec<OsString>) -> i32 {
    let params = match parse_params(args.into_iter().peekable()) {
        Ok(params) => params,
        Err(err) => {
            eprintln!("diff: {err}");
            return 2;
        }
    };
    match run(&params, &mut io::stdout().lock()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("diff: {err}");
            2
        }
    }
}

fn run(params: &Params, out: &mut impl Write) -> Result<i32, String> {
    let from = read(&params.from)?;
    let to = read(&params.to)?;
    let result = match params.format {
        Format::Normal => diffutilslib::normal_diff(&from, &to, params),
        Format::Unified => diffutilslib::unified_diff(&from, &to, params),
        Format::Context => diffutilslib::context_diff(&from, &to, params),
        Format::Ed => diffutilslib::ed_diff(&from, &to, params).map_err(|e| e.to_string())?,
        Format::SideBySide => {
            diffutilslib::side_by_side_diff(&from, &to, out, params);
            Vec::new()
        }
    };
    let differ = !result.is_empty() || (params.format == Format::SideBySide && from != to);
    let report = if params.brief && differ {
        format!(
            "Files {} and {} differ\n",
            params.from.to_string_lossy(),
            params.to.to_string_lossy()
        )
        .into_bytes()
    } else if params.report_identical_files && !differ {
        format!(
            "Files {} and {} are identical\n",
            params.from.to_string_lossy(),
            params.to.to_string_lossy()
        )
        .into_bytes()
    } else {
        result
    };
    out.write_all(&report).map_err(|e| e.to_string())?;
    Ok(i32::from(differ))
}

fn read(path: &OsString) -> Result<Vec<u8>, String> {
    let mut content = Vec::new();
    let read = if path == "-" {
        io::stdin().read_to_end(&mut content).map(|_| content)
    } else {
        std::fs::read(path)
    };
    read.map_err(|err| format!("{}: {err}", path.to_string_lossy()))
}
