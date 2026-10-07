//! `pumice-test-cli`: a portable fake AI CLI for Pumice's automated tests.
//!
//! Tests never call a real AI CLI. They copy (or hard-link) this executable
//! into a disposable directory and write a scenario file next to the copy:
//!
//! ```text
//! <dir>/pumice-test-cli[.exe]
//! <dir>/pumice-test-cli[.exe].scenario.json
//! ```
//!
//! The fake locates its scenario through `std::env::current_exe()`, so no
//! global environment variables are needed and tests can run in parallel.
//!
//! Order of operations:
//!
//! 1. If argv is exactly `--pumice-test-grandchild <ms>`, sleep that long and
//!    exit 0 (no scenario is read). This is the grandchild mode.
//! 2. Load the scenario (exit 97 if missing, 98 if invalid).
//! 3. Read stdin to EOF unless `read_stdin` is false.
//! 4. Spawn the grandchild if `spawn_grandchild_sleep_ms` is set. It inherits
//!    stdout and stderr, so it keeps the caller's pipes open after this
//!    process exits.
//! 5. Write the report if `report_path` is set.
//! 6. Sleep `sleep_ms`.
//! 7. Write `stdout`, then `stdout_repeat_bytes` filler bytes, then `stderr`.
//! 8. Exit with `exit_code` without waiting for the grandchild.
//!
//! Only std and serde/serde_json are used, so it builds and behaves the same
//! on Windows, Linux and macOS.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Argv flag that switches the fake into grandchild mode.
const GRANDCHILD_FLAG: &str = "--pumice-test-grandchild";
/// Suffix appended to the executable path to find the scenario file.
const SCENARIO_SUFFIX: &str = ".scenario.json";
/// Exit code when the scenario file does not exist.
const EXIT_NO_SCENARIO: i32 = 97;
/// Exit code when the scenario file cannot be read or parsed.
const EXIT_BAD_SCENARIO: i32 = 98;
/// Exit code when the fake itself fails (report write, grandchild spawn).
const EXIT_INTERNAL: i32 = 99;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// Where to write the JSON report. Should be outside the working directory
    /// so the workspace stays empty.
    #[serde(default)]
    report_path: Option<PathBuf>,
    /// Environment variables to include in the report (value or null).
    #[serde(default)]
    report_env: Vec<String>,
    /// Flags whose following argument is a file path to capture in the report
    /// (for example `--system-prompt-file`). Captured before the caller can
    /// delete the file.
    #[serde(default)]
    report_arg_files: Vec<String>,
    /// `-c` keys whose `key="<path>"` argument value is a TOML-quoted file
    /// path to capture in the report (for example `model_instructions_file`).
    #[serde(default)]
    report_config_files: Vec<String>,
    /// Arbitrary files to read and report, resolved relative to the working
    /// directory (for example `../control/agents/pumice.json`). A directory
    /// reports every regular file inside it, keyed by its relative path.
    #[serde(default)]
    report_files: Vec<String>,
    /// Paths (relative to the working directory) whose Unix permission mode
    /// should be reported. Always reports null on non-Unix targets.
    #[serde(default)]
    report_modes: Vec<String>,
    /// Read stdin to EOF before doing anything else. When false, stdin is never
    /// read (a CLI that ignores its input).
    #[serde(default = "default_true")]
    read_stdin: bool,
    /// Written to stdout as-is.
    #[serde(default)]
    stdout: String,
    /// Number of filler bytes written to stdout after `stdout`.
    #[serde(default)]
    stdout_repeat_bytes: Option<u64>,
    /// Written to stderr as-is.
    #[serde(default)]
    stderr: String,
    /// Process exit code.
    #[serde(default)]
    exit_code: i32,
    /// Delay before writing any output.
    #[serde(default)]
    sleep_ms: Option<u64>,
    /// Spawn a grandchild that inherits stdout/stderr and sleeps this long.
    #[serde(default)]
    spawn_grandchild_sleep_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct Report {
    /// Arguments after argv[0].
    argv: Vec<String>,
    /// Full stdin (lossy UTF-8), or null when `read_stdin` is false.
    stdin: Option<String>,
    cwd: String,
    /// Sorted names of the entries in the working directory.
    cwd_entries: Vec<String>,
    /// Requested environment variables.
    env: BTreeMap<String, Option<String>>,
    /// Contents of files named after the requested flags, keyed by flag.
    arg_files: BTreeMap<String, Option<ArgFile>>,
    /// Contents of files named by `-c key="<path>"` arguments, keyed by key.
    config_files: BTreeMap<String, Option<ArgFile>>,
    /// Contents of arbitrary files requested by `report_files`.
    report_files: BTreeMap<String, Option<ArgFile>>,
    /// Unix permission modes requested by `report_modes`.
    modes: BTreeMap<String, Option<u32>>,
    pid: u32,
    grandchild_pid: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ArgFile {
    path: String,
    /// File contents, or null when the file could not be read.
    contents: Option<String>,
}

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    if argv.first().map(String::as_str) == Some(GRANDCHILD_FLAG) {
        grandchild(&argv);
    }

    let exe = std::env::current_exe()
        .unwrap_or_else(|e| fail(EXIT_INTERNAL, &format!("cannot locate own executable: {e}")));
    let scenario = load_scenario(&exe);

    let stdin = if scenario.read_stdin {
        let mut buf = Vec::new();
        if let Err(e) = io::stdin().read_to_end(&mut buf) {
            fail(EXIT_INTERNAL, &format!("cannot read stdin: {e}"));
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    } else {
        None
    };

    let grandchild_pid = scenario.spawn_grandchild_sleep_ms.map(|ms| {
        Command::new(&exe)
            .arg(GRANDCHILD_FLAG)
            .arg(ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| fail(EXIT_INTERNAL, &format!("cannot spawn grandchild: {e}")))
            .id()
    });

    if let Some(path) = &scenario.report_path {
        let report = build_report(&scenario, argv, stdin, grandchild_pid);
        write_report(path, &report);
    }

    if let Some(ms) = scenario.sleep_ms {
        thread::sleep(Duration::from_millis(ms));
    }

    // Write errors (for example a reader that closed the pipe) are ignored:
    // the fake still exits with the scenario's code.
    let mut out = io::stdout().lock();
    let _ = out.write_all(scenario.stdout.as_bytes());
    if let Some(n) = scenario.stdout_repeat_bytes {
        let _ = write_filler(&mut out, n);
    }
    let _ = out.flush();
    drop(out);

    let mut err = io::stderr().lock();
    let _ = err.write_all(scenario.stderr.as_bytes());
    let _ = err.flush();
    drop(err);

    process::exit(scenario.exit_code);
}

/// Grandchild mode: sleep, then exit. Holds inherited pipes open meanwhile.
fn grandchild(argv: &[String]) -> ! {
    let ms = match argv {
        [_, ms] => ms.parse::<u64>().ok(),
        _ => None,
    };
    let Some(ms) = ms else {
        fail(
            EXIT_INTERNAL,
            &format!("usage: {GRANDCHILD_FLAG} <milliseconds>"),
        );
    };
    thread::sleep(Duration::from_millis(ms));
    process::exit(0);
}

fn scenario_path(exe: &Path) -> PathBuf {
    let mut path = exe.as_os_str().to_owned();
    path.push(SCENARIO_SUFFIX);
    PathBuf::from(path)
}

fn load_scenario(exe: &Path) -> Scenario {
    let path = scenario_path(exe);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => fail(
            EXIT_NO_SCENARIO,
            &format!(
                "scenario file not found: {} (tests must write it next to the fake executable)",
                path.display()
            ),
        ),
        Err(e) => fail(
            EXIT_BAD_SCENARIO,
            &format!("cannot read scenario {}: {e}", path.display()),
        ),
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        fail(
            EXIT_BAD_SCENARIO,
            &format!("invalid scenario {}: {e}", path.display()),
        )
    })
}

fn build_report(
    scenario: &Scenario,
    argv: Vec<String>,
    stdin: Option<String>,
    grandchild_pid: Option<u32>,
) -> Report {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        fail(
            EXIT_INTERNAL,
            &format!("cannot read working directory: {e}"),
        )
    });
    let cwd = normalize_canonical_path(&cwd.canonicalize().unwrap_or(cwd));
    let cwd = PathBuf::from(cwd);
    let mut cwd_entries: Vec<String> = std::fs::read_dir(&cwd)
        .unwrap_or_else(|e| {
            fail(
                EXIT_INTERNAL,
                &format!("cannot list working directory: {e}"),
            )
        })
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    cwd_entries.sort();

    let env = scenario
        .report_env
        .iter()
        .map(|name| {
            let value = std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
            (name.clone(), value)
        })
        .collect();

    let arg_files = scenario
        .report_arg_files
        .iter()
        .map(|flag| {
            let file = argv
                .iter()
                .position(|a| a == flag)
                .and_then(|i| argv.get(i + 1))
                .map(|path| ArgFile {
                    path: path.clone(),
                    contents: std::fs::read_to_string(path).ok(),
                });
            (flag.clone(), file)
        })
        .collect();

    let config_files = scenario
        .report_config_files
        .iter()
        .map(|key| {
            let file = argv
                .windows(2)
                .filter(|pair| pair[0].as_str() == "-c")
                .find_map(|pair| config_path_argument(&pair[1], key))
                .map(|path| ArgFile {
                    path: path.clone(),
                    contents: std::fs::read_to_string(&path).ok(),
                });
            (key.clone(), file)
        })
        .collect();

    let mut report_files: BTreeMap<String, Option<ArgFile>> = BTreeMap::new();
    for path in &scenario.report_files {
        let resolved = cwd.join(path);
        if resolved.is_dir() {
            collect_report_files(&resolved, path, &cwd, &mut report_files);
        } else {
            let file = if resolved.exists() {
                let canonical = resolved.canonicalize().unwrap_or(resolved);
                Some(ArgFile {
                    path: normalize_canonical_path(&canonical),
                    contents: std::fs::read_to_string(&canonical).ok(),
                })
            } else {
                None
            };
            report_files.insert(path.clone(), file);
        }
    }

    let modes = scenario
        .report_modes
        .iter()
        .map(|path| {
            let resolved = cwd.join(path);
            (path.clone(), unix_mode(&resolved))
        })
        .collect();

    Report {
        argv,
        stdin,
        cwd: cwd.to_string_lossy().into_owned(),
        cwd_entries,
        env,
        arg_files,
        config_files,
        report_files,
        modes,
        pid: process::id(),
        grandchild_pid,
    }
}

/// Recursively collects regular files under `dir` into `out`, keyed by their
/// path relative to `cwd`.
fn collect_report_files(
    dir: &Path,
    prefix: &str,
    cwd: &Path,
    out: &mut BTreeMap<String, Option<ArgFile>>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let relative = strip_prefix_or_key(&path, cwd, prefix);
        if path.is_dir() {
            collect_report_files(&path, prefix, cwd, out);
        } else if path.is_file() {
            let canonical = path.canonicalize().unwrap_or(path.clone());
            out.insert(
                relative,
                Some(ArgFile {
                    path: normalize_canonical_path(&canonical),
                    contents: std::fs::read_to_string(&canonical).ok(),
                }),
            );
        }
    }
}

/// Returns `path` relative to `cwd`, falling back to the original `prefix` if
/// stripping fails.
fn strip_prefix_or_key(path: &Path, cwd: &Path, prefix: &str) -> String {
    path.strip_prefix(cwd)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| prefix.to_owned())
}

/// Returns the decoded path from a `-c` argument of the form
/// `key="<TOML-quoted path>"` when `key` matches. An undecodable value falls
/// back to the raw text so the report still shows what was passed.
fn config_path_argument(argument: &str, wanted: &str) -> Option<String> {
    let (key, value) = argument.split_once('=')?;
    if key != wanted {
        return None;
    }
    Some(decode_toml_basic_string(value).unwrap_or_else(|| value.to_owned()))
}

/// Decodes the TOML basic string produced by the adapter's encoder.
fn decode_toml_basic_string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'b' => out.push('\u{8}'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'f' => out.push('\u{c}'),
            'r' => out.push('\r'),
            'u' => out.push(decode_hex(&mut chars, 4)?),
            'U' => out.push(decode_hex(&mut chars, 8)?),
            _ => return None,
        }
    }
    Some(out)
}

fn decode_hex(chars: &mut impl Iterator<Item = char>, n: usize) -> Option<char> {
    let hex: String = chars.by_ref().take(n).collect();
    if hex.len() != n {
        return None;
    }
    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
}

/// Writes the report through a temporary file and a rename so a reader never
/// sees a partial report.
fn write_report(path: &Path, report: &Report) {
    let json = serde_json::to_vec_pretty(report)
        .unwrap_or_else(|e| fail(EXIT_INTERNAL, &format!("cannot serialize report: {e}")));
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, json)
        .and_then(|()| std::fs::rename(&tmp, path))
        .unwrap_or_else(|e| {
            fail(
                EXIT_INTERNAL,
                &format!("cannot write report {}: {e}", path.display()),
            )
        });
}

/// Returns the Unix permission bits of `path`, or `None` on non-Unix targets
/// or if the metadata cannot be read.
fn unix_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Converts a canonicalized path to a comparable form. On Windows
/// `std::fs::canonicalize` returns a verbatim UNC path (`\\?\C:\...`)
/// that does not prefix-match ordinary absolute paths, so strip that
/// prefix when present.
fn normalize_canonical_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(String::from)
        .unwrap_or_else(|| s.into_owned())
}

fn write_filler(out: &mut impl Write, mut remaining: u64) -> io::Result<()> {
    let chunk = [b'x'; 64 * 1024];
    while remaining > 0 {
        let n = remaining.min(chunk.len() as u64) as usize;
        out.write_all(&chunk[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("pumice-test-cli: {message}");
    process::exit(code);
}
