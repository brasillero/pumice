//! Shared helpers for integration tests.
//!
//! Each integration test file pulls this in with `mod support;`. Not every
//! file uses every helper.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

pub mod adapter_contract;
pub mod bounded;
pub mod fake_http;
pub mod install;
pub mod test_provider;

/// Every top-level `relay.yml` setting with the value `relay.example.yml`
/// writes, keyed by its name. [`relay_config`] appends the ones a test does
/// not write.
const RELAY_SETTINGS: [(&str, &str); 5] = [
    ("port", "port: 7567\n"),
    ("total_timeout_secs", "total_timeout_secs: 30\n"),
    ("max_parallel", "max_parallel: 4\n"),
    ("providers", "providers: []\n"),
    (
        "debug_log",
        "debug_log:\n  enabled: false\n  path: pumice-debug.jsonl\n",
    ),
];

/// A complete `relay.yml` for a test: `text`, then every top-level setting
/// it does not write itself (every setting is required). Appending keeps the
/// line numbers in `text` unchanged. Provider entries are never completed:
/// a test writes each entry's keys itself.
pub fn relay_config(text: &str) -> String {
    let mut yaml = text.to_owned();
    if !yaml.is_empty() && !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    for (key, line) in RELAY_SETTINGS {
        let prefix = format!("{key}:");
        if !text.lines().any(|written| written.starts_with(&prefix)) {
            yaml.push_str(line);
        }
    }
    yaml
}

/// Path of the fake CLI built by Cargo for this test run.
pub const FAKE_CLI_BIN: &str = env!("CARGO_BIN_EXE_pumice-test-cli");

/// File name of the report written by [`FakeCli`] unless the scenario sets
/// its own `report_path`.
const REPORT_FILE: &str = "report.json";

/// A disposable copy of the fake CLI with its own scenario.
///
/// The copy lives in a fresh temporary directory together with
/// `<exe>.scenario.json` and, by default, the report file. The directory is
/// deleted when this value is dropped. Point code under test at [`path`].
///
/// [`path`]: FakeCli::path
pub struct FakeCli {
    dir: TempDir,
    exe: PathBuf,
    report_path: PathBuf,
}

impl FakeCli {
    /// Creates a fake CLI that follows `scenario` (see `tests/support/fake_cli.rs`
    /// for the fields). When the scenario has no `report_path`, one inside the
    /// fake's own directory is added, so the report is always available
    /// through [`report`](FakeCli::report) and stays outside any working
    /// directory the test creates.
    pub fn new(mut scenario: Value) -> FakeCli {
        let dir = tempfile::Builder::new()
            .prefix("pumice-fake-cli-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("create fake CLI directory");

        let exe = dir
            .path()
            .join(format!("pumice-test-cli{}", std::env::consts::EXE_SUFFIX));
        install::install_executable(Path::new(FAKE_CLI_BIN), &exe);

        let object = scenario
            .as_object_mut()
            .expect("fake CLI scenario must be a JSON object");
        let report_path = match object.get("report_path") {
            Some(Value::String(path)) => PathBuf::from(path),
            Some(other) => panic!("report_path must be a string, got {other}"),
            None => {
                let path = dir.path().join(REPORT_FILE);
                object.insert(
                    "report_path".into(),
                    Value::String(path.to_str().expect("UTF-8 temp path").to_owned()),
                );
                path
            }
        };

        let mut scenario_path = exe.as_os_str().to_owned();
        scenario_path.push(".scenario.json");
        fs::write(
            scenario_path,
            serde_json::to_vec_pretty(&scenario).expect("serialize scenario"),
        )
        .expect("write fake CLI scenario");

        FakeCli {
            dir,
            exe,
            report_path,
        }
    }

    /// Path of the fake executable to spawn.
    pub fn path(&self) -> &Path {
        &self.exe
    }

    /// Directory holding the fake, its scenario and (by default) its report.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Path of the report the fake writes when it runs.
    pub fn report_path(&self) -> &Path {
        &self.report_path
    }

    /// Reads the report written by the last run of the fake.
    ///
    /// Panics if the fake has not run (or did not get far enough to write it).
    pub fn report(&self) -> Value {
        let bytes = fs::read(&self.report_path).unwrap_or_else(|e| {
            panic!(
                "fake CLI report {} not readable: {e}",
                self.report_path.display()
            )
        });
        serde_json::from_slice(&bytes).expect("fake CLI report is valid JSON")
    }
}

/// Absolute path of a file under `tests/fixtures/`.
pub fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

/// Contents of a file under `tests/fixtures/`, e.g. `"claude/success.json"`.
pub fn fixture(relative: &str) -> String {
    let path = fixture_path(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {} not readable: {e}", path.display()))
}
