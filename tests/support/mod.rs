//! Shared helpers for integration tests.
//!
//! Each integration test file pulls this in with `mod support;`. Not every
//! file uses every helper.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

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
        install_executable(Path::new(FAKE_CLI_BIN), &exe);

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

/// Places the fake executable at `dest`.
///
/// A hard link is preferred: copying writes a new executable, and on Linux
/// another test thread forking at that moment can make the first spawn fail
/// with "text file busy" (ETXTBSY). The temp directory is under Cargo's target
/// directory, so the link stays on one file system. Copy is the fallback.
fn install_executable(src: &Path, dest: &Path) {
    if fs::hard_link(src, dest).is_err() {
        fs::copy(src, dest).expect("copy fake CLI executable");
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
