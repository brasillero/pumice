//! CLI tests for the `pumice-format` binary: every command, the `--json`
//! envelopes, exit codes and the relay round-trip. Everything runs against a
//! hermetic config home; no provider or network is ever touched.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// Every config-reading test runs against its own XDG/APPDATA directory so
/// the developer's real per-user config never takes part.
struct HermeticHome {
    _keep: TempDir,
    xdg: PathBuf,
}

impl HermeticHome {
    fn new() -> Self {
        let keep = TempDir::new().expect("temp config home");
        let xdg = keep.path().join("xdg");
        fs::create_dir_all(&xdg).expect("create config home");
        HermeticHome { _keep: keep, xdg }
    }

    fn pumice_dir(&self) -> PathBuf {
        self.xdg.join("pumice")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pumice-format"));
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("APPDATA", &self.xdg)
            .env("HOME", self._keep.path());
        command
    }

    /// Writes a complete formatter.yml that registers nothing and selects
    /// no default: the values that were built-in defaults before every
    /// setting had to be written.
    fn write_empty_config(&self) {
        let dir = self.pumice_dir();
        fs::create_dir_all(&dir).expect("create per-user config dir");
        fs::write(
            dir.join("formatter.yml"),
            "schema_version: 1\nformatters: []\ndefault: null\n",
        )
        .expect("write formatter.yml");
    }

    /// Writes a formatter.yml plus one definition file into the config dir.
    fn write_config(&self, formatter_id: &str, default: Option<&str>) {
        let dir = self.pumice_dir();
        fs::create_dir_all(&dir).expect("create per-user config dir");
        fs::write(
            dir.join(format!("{formatter_id}.yml")),
            format!(
                "schema_version: 1\nid: {formatter_id}\ndescription: Test formatter\nsystem: TEST RULES\n"
            ),
        )
        .expect("write definition");
        let default_line = format!("default: {}\n", default.unwrap_or("null"));
        fs::write(
            dir.join("formatter.yml"),
            format!("schema_version: 1\nformatters:\n  - path: {formatter_id}.yml\n{default_line}"),
        )
        .expect("write formatter.yml");
    }
}

fn run(command: &mut Command) -> Output {
    command
        .stdin(Stdio::null())
        .output()
        .expect("pumice-format binary runs")
}

/// Runs the command with `input` on stdin.
fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pumice-format binary runs");
    // A command that fails before reading stdin (an argument or config
    // error) may exit first; the closed pipe is then expected, not a failure.
    if let Err(error) = child.stdin.take().expect("stdin is piped").write_all(input) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "write stdin");
    }
    child.wait_with_output().expect("reap pumice-format")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn envelope(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout is a JSON envelope")
}

fn user_request() -> Value {
    serde_json::json!({
        "model": "claude",
        "messages": [{"role": "user", "content": "buy apples and milk"}],
        "stream": false,
    })
}

#[test]
fn list_shows_the_builtin_without_a_config_file() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["list"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("builtin:clean"), "{stdout}");
    assert!(stdout.contains("built-in"), "{stdout}");
}

#[test]
fn list_marks_the_default_and_shows_configured_formatters() {
    let home = HermeticHome::new();
    home.write_config("mine", Some("mine"));
    let output = run(&mut home.command(&["list"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("mine"), "{stdout}");
    assert!(stdout.contains("yes"), "a default is marked: {stdout}");
}

#[test]
fn list_json_envelope_lists_builtins_and_configured() {
    let home = HermeticHome::new();
    home.write_config("mine", Some("builtin:clean"));
    let output = run(&mut home.command(&["list", "--json"]));
    assert_eq!(output.status.code(), Some(0));
    let envelope = envelope(&output);
    assert_eq!(envelope["ok"], true);
    let formatters = envelope["result"]["formatters"].as_array().unwrap();
    let builtin = formatters
        .iter()
        .find(|entry| entry["id"] == "builtin:clean")
        .expect("builtin listed");
    assert_eq!(builtin["builtin"], true);
    assert_eq!(builtin["default"], true, "config default marks the builtin");
    let mine = formatters
        .iter()
        .find(|entry| entry["id"] == "mine")
        .expect("configured formatter listed");
    assert_eq!(mine["builtin"], false);
    assert_eq!(mine["default"], false);
}

#[test]
fn show_builtin_prints_an_editable_definition_with_a_new_id() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["show", "builtin:clean"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("id: my-clean"), "placeholder id: {stdout}");
    assert!(stdout.contains("Handy"), "Handy attribution: {stdout}");
    assert!(stdout.contains("system: |"), "{stdout}");

    // The output is a valid definition file under its placeholder id.
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("my.yaml");
    fs::write(&path, &stdout).unwrap();
    let loaded = pumice_formatter::load_definition_file(&path).expect("valid definition file");
    assert_eq!(loaded.id, "my-clean");
    assert_eq!(loaded.system, pumice_formatter::builtin_clean().system);
}

#[test]
fn show_output_reloads_to_the_exact_same_system_text() {
    // The CLI path, not just the renderer: `show` stdout must be a
    // definition file that reloads to the exact same text — no added or
    // dropped trailing newlines, whatever the block style.
    for (id, system) in [
        ("trail-blank", "Rule\n\n"),
        ("trail-one", "Rule\n"),
        ("no-trailing", "Rule"),
        ("indented", "  first\nsecond"),
        ("crlf", "Rule\r\nSecond\rline"),
    ] {
        let home = HermeticHome::new();
        let dir = home.pumice_dir();
        fs::create_dir_all(&dir).expect("create per-user config dir");
        fs::write(
            dir.join(format!("{id}.yml")),
            format!(
                "schema_version: 1\nid: {id}\ndescription: Test\nsystem: {}\n",
                yaml_double_quoted(system)
            ),
        )
        .expect("write definition");
        fs::write(
            dir.join("formatter.yml"),
            format!("schema_version: 1\nformatters:\n  - path: {id}.yml\ndefault: null\n"),
        )
        .expect("write formatter.yml");

        let output = run(&mut home.command(&["show", id]));
        assert_eq!(output.status.code(), Some(0), "{id}: {}", stderr(&output));
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join("show.yml");
        fs::write(&path, &output.stdout).expect("write show output");
        let loaded =
            pumice_formatter::load_definition_file(&path).expect("show output is a definition");
        assert_eq!(loaded.id, id, "{id}");
        assert_eq!(loaded.system, system, "{id}: stdout must reload exactly");
    }
}

/// Renders text as a YAML double-quoted scalar, so a definition file can
/// carry any exact `system` string (newlines, CR, leading whitespace).
fn yaml_double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[test]
fn show_json_describes_the_builtin() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["show", "builtin:clean", "--json"]));
    assert_eq!(output.status.code(), Some(0));
    let envelope = envelope(&output);
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["result"]["id"], "builtin:clean");
    assert_eq!(envelope["result"]["builtin"], true);
    assert!(
        envelope["result"]["system"]
            .as_str()
            .unwrap()
            .contains("Clean the text by")
    );
}

#[test]
fn show_unknown_id_fails_with_2_and_a_safe_json_error() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let output = run(&mut home.command(&["show", "nope", "--json"]));
    assert_eq!(output.status.code(), Some(2));
    let envelope = envelope(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "formatter_not_found");
}

#[test]
fn apply_transforms_a_request_from_stdin() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let request = serde_json::to_vec(&user_request()).unwrap();
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        &request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let messages = transformed["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1], user_request()["messages"][0]);
    assert_eq!(transformed["model"], "claude");
}

#[test]
fn apply_uses_the_configured_default() {
    let home = HermeticHome::new();
    home.write_config("mine", Some("mine"));
    let request = serde_json::to_vec(&user_request()).unwrap();
    let output = run_with_stdin(&mut home.command(&["apply"]), &request);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(transformed["messages"][0]["content"], "TEST RULES");
}

#[test]
fn apply_without_a_formatter_and_default_fails_with_2() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let request = serde_json::to_vec(&user_request()).unwrap();
    let output = run_with_stdin(&mut home.command(&["apply"]), &request);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("no formatter selected"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn apply_json_wraps_the_transformed_request() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let request = serde_json::to_vec(&user_request()).unwrap();
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean", "--json"]),
        &request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let envelope = envelope(&output);
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["result"]["messages"][0]["role"], "system");
}

#[test]
fn apply_rejects_existing_instructions_and_append_places_after_them() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let mut request = user_request();
    request["messages"] = serde_json::json!([
        {"role": "system", "content": "app rules"},
        {"role": "user", "content": "hi"},
    ]);
    let bytes = serde_json::to_vec(&request).unwrap();

    let conflict = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        &bytes,
    );
    assert_eq!(conflict.status.code(), Some(2));
    assert!(conflict.stdout.is_empty(), "no partial stdout");
    assert!(
        stderr(&conflict).contains("--on-conflict append"),
        "{}",
        stderr(&conflict)
    );

    let conflict_json = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean", "--json"]),
        &bytes,
    );
    assert_eq!(conflict_json.status.code(), Some(2));
    let envelope = envelope(&conflict_json);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "formatter_instruction_conflict");

    let appended = run_with_stdin(
        &mut home.command(&[
            "apply",
            "--formatter",
            "builtin:clean",
            "--on-conflict",
            "append",
        ]),
        &bytes,
    );
    assert_eq!(
        appended.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&appended)
    );
    let transformed: Value = serde_json::from_slice(&appended.stdout).unwrap();
    let roles: Vec<&str> = transformed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "system", "user"]);
}

#[test]
fn apply_rejects_invalid_json_and_shape_with_2() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let bad = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        b"{not json",
    );
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("not valid JSON"), "{}", stderr(&bad));

    let no_messages = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        br#"{"model": "claude"}"#,
    );
    assert_eq!(no_messages.status.code(), Some(2));
    assert!(
        stderr(&no_messages).contains("\"messages\""),
        "{}",
        stderr(&no_messages)
    );

    let bad_message = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        br#"{"messages": [{"role": "user"}, {"content": "no role"}]}"#,
    );
    assert_eq!(bad_message.status.code(), Some(2));
    assert!(
        stderr(&bad_message).contains("message 1"),
        "{}",
        stderr(&bad_message)
    );
}

#[test]
fn apply_bounds_the_request_to_1_mib() {
    let home = HermeticHome::new();
    home.write_empty_config();
    // A valid prefix, then padding past the limit; it must never parse or
    // transform.
    let mut input = br#"{"messages": [{"role": "user", "content": ""#.to_vec();
    input.resize(1024 * 1024 + 16, b'x');
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        &input,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("too large"), "{}", stderr(&output));
}

#[test]
fn apply_reads_the_request_from_a_file() {
    let home = HermeticHome::new();
    home.write_empty_config();
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("request.json");
    fs::write(&path, serde_json::to_vec(&user_request()).unwrap()).unwrap();
    let output = run(&mut home.command(&[
        "apply",
        "--formatter",
        "builtin:clean",
        "--request",
        path.to_str().unwrap(),
    ]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(transformed["messages"][0]["role"], "system");
}

#[test]
fn check_config_validates_the_per_user_file() {
    let home = HermeticHome::new();
    home.write_config("mine", Some("mine"));
    let output = run(&mut home.command(&["check-config"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(
            &home
                .pumice_dir()
                .join("formatter.yml")
                .display()
                .to_string()
        ),
        "{stdout}"
    );

    let json = run(&mut home.command(&["check-config", "--json"]));
    assert_eq!(json.status.code(), Some(0));
    let envelope = envelope(&json);
    assert_eq!(envelope["ok"], true);
    assert!(envelope["result"]["file"].is_string());
}

#[test]
fn check_config_reports_config_errors_with_location() {
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    fs::write(
        dir.join("formatter.yml"),
        "schema_version: 1\nformatters: []\ndefault: gone\n",
    )
    .unwrap();
    let output = run(&mut home.command(&["check-config"]));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(stderr.contains("does not name a formatter"), "{stderr}");
    assert!(stderr.contains(":3:"), "line reported: {stderr}");
}

#[test]
fn operational_commands_fail_without_a_config_file() {
    let home = HermeticHome::new();
    let expected = home.pumice_dir().join("formatter.yml");
    let request = serde_json::to_vec(&user_request()).unwrap();
    for args in [
        &["check-config"][..],
        &["apply", "--formatter", "builtin:clean"],
        &["show", "mine"],
    ] {
        let output = run_with_stdin(&mut home.command(args), &request);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = stderr(&output);
        assert!(
            stderr.contains(&expected.display().to_string())
                && stderr.contains("the formatter configuration file does not exist"),
            "{args:?}: {stderr}"
        );
    }
    // `list` and `show builtin:clean` only report built-ins.
    let output = run(&mut home.command(&["show", "builtin:clean"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn an_incomplete_config_file_fails_even_for_list() {
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    fs::write(
        dir.join("formatter.yml"),
        "schema_version: 1\nformatters: []\n",
    )
    .unwrap();
    let output = run(&mut home.command(&["list"]));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(":1:1: \"default\" is missing; every setting must be written"),
        "{stderr}"
    );
}

#[test]
fn clap_errors_exit_2_and_emit_an_envelope_with_json() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["apply", "--formatter", "--json"]));
    assert_eq!(output.status.code(), Some(2));
    let envelope = envelope(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "invalid_arguments");
}

#[test]
fn help_names_the_program_and_lists_the_commands() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["--help"]));
    assert_eq!(output.status.code(), Some(0));
    let stdout = stdout(&output);
    assert!(stdout.contains("pumice-format"), "{stdout}");
    for command in ["list", "show", "apply", "check-config"] {
        assert!(stdout.contains(command), "help lists {command}: {stdout}");
    }
}

#[test]
fn apply_preserves_big_integers_and_long_decimals_in_unknown_fields() {
    let home = HermeticHome::new();
    home.write_empty_config();
    // A u64-overflowing integer and a decimal longer than f64 precision:
    // both must pass through byte-exact (raw pass-through, no Value
    // round-trip).
    let request = br#"{"model":"claude","messages":[{"role":"user","content":"hi"}],"big":18446744073709551617,"negative":-18446744073709551617,"price":1.1000000000000000055511151231257827021181583404541015625}"#;
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("\"big\":18446744073709551617"), "{stdout}");
    assert!(
        stdout.contains("\"negative\":-18446744073709551617"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"price\":1.1000000000000000055511151231257827021181583404541015625"),
        "{stdout}"
    );
}

#[test]
fn apply_preserves_nested_key_order_in_unknown_fields() {
    let home = HermeticHome::new();
    home.write_empty_config();
    // OpenAI Structured Outputs follow schema key order, so the formatter
    // must not reorder nested object keys (serde_json preserve_order).
    let request = br#"{"model":"claude","messages":[{"role":"user","content":"hi"}],"response_format":{"type":"json_schema","json_schema":{"schema":{"properties":{"z":{"type":"string"},"a":{"type":"integer"}}}}}}"#;
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys: Vec<&str> = transformed["response_format"]["json_schema"]["schema"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["z", "a"], "schema property order preserved");
    let stdout = stdout(&output);
    assert!(
        stdout.find("\"z\"").unwrap() < stdout.find("\"a\"").unwrap(),
        "z serializes before a: {stdout}"
    );
}

#[test]
fn apply_preserves_odd_keys_byte_for_byte() {
    let home = HermeticHome::new();
    home.write_empty_config();
    // serde_json's internal number marker is a legitimate client key: it
    // must come out untouched, not decoded into a number.
    let request = br#"{"messages":[{"role":"user","content":"hi"}],"metadata":{"$serde_json::private::Number":"123"}}"#;
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(r#""metadata":{"$serde_json::private::Number":"123"}"#),
        "{stdout}"
    );
}

#[test]
fn apply_json_keeps_the_transformed_request_byte_exact() {
    let home = HermeticHome::new();
    home.write_empty_config();
    // The --json envelope embeds the transformed request raw, so precision
    // and key order survive there too.
    let request = br#"{"messages":[{"role":"user","content":"hi"}],"big":18446744073709551617}"#;
    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean", "--json"]),
        request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains(r#""ok":true"#), "{stdout}");
    assert!(stdout.contains(r#""big":18446744073709551617"#), "{stdout}");
}

#[test]
fn errors_never_echo_request_text_in_plain_or_json_mode() {
    // Dictated text is private: no matter where it sits in a rejected
    // request, neither stdout nor stderr may contain it (AGENTS.md rule 5).
    const SENTINEL: &str = "PRIVATE_DICTATION_9f3b7c";
    let home = HermeticHome::new();
    home.write_empty_config();
    let cases = [
        // duplicate top-level member named with the sentinel
        format!(r#"{{"messages":[],"{SENTINEL}":1,"{SENTINEL}":2}}"#),
        // invalid JSON where the content should be
        format!(r#"{{"messages":[{{"role":"user","content":{SENTINEL}}}]}}"#),
        // a message that is a bare string
        format!(r#"{{"messages":["{SENTINEL}"]}}"#),
        // a positional-array message
        format!(r#"{{"messages":[["{SENTINEL}","hi"]]}}"#),
        // a role of the wrong type, sentinel in content
        format!(r#"{{"messages":[{{"role":5,"content":"{SENTINEL}"}}]}}"#),
        // an instruction conflict carrying the sentinel
        format!(
            r#"{{"messages":[{{"role":"system","content":"{SENTINEL}"}},{{"role":"user","content":"hi"}}]}}"#
        ),
        // trailing garbage after a valid request
        format!(r#"{{"messages":[]}} {SENTINEL}"#),
    ];
    for case in &cases {
        for json in [false, true] {
            let mut args = vec!["apply", "--formatter", "builtin:clean"];
            if json {
                args.push("--json");
            }
            let output = run_with_stdin(&mut home.command(&args), case.as_bytes());
            assert_eq!(output.status.code(), Some(2), "{case}");
            let combined = format!("{}{}", stdout(&output), stderr(&output));
            assert!(
                !combined.contains(SENTINEL),
                "sentinel leaked (json={json}) for {case}: {combined}"
            );
        }
    }
}

#[test]
fn a_transformed_handy_like_request_is_accepted_by_the_relay() {
    // The Handy replacement shape: the user message is the plain dictation
    // (Handy itself would wrap it and append its own prompt; the formatter
    // is that prompt's new home). The transformed request must still parse
    // as the relay's ChatCompletionRequest and extract cleanly.
    let home = HermeticHome::new();
    home.write_empty_config();
    let mut request = user_request();
    request["model"] = Value::String("pumice-echo".to_owned());
    request["reasoning_effort"] = Value::String("none".to_owned());
    request["messages"][0]["content"] =
        Value::String("Reunião com a equipe às nove horas.".to_owned());

    let output = run_with_stdin(
        &mut home.command(&["apply", "--formatter", "builtin:clean"]),
        &serde_json::to_vec(&request).unwrap(),
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: Value = serde_json::from_slice(&output.stdout).unwrap();

    let parsed: pumice_relay::request::ChatCompletionRequest =
        serde_json::from_value(transformed.clone()).expect("the relay accepts the request");
    let extracted = pumice_relay::request::extract_request(parsed)
        .expect("the relay extracts the transformed request");

    let builtin = pumice_formatter::builtin_clean();
    assert_eq!(extracted.system_texts, vec![builtin.system]);
    assert_eq!(extracted.raw_text, "Reunião com a equipe às nove horas.");
    assert!(!extracted.is_empty());

    // Everything the formatter did not own is untouched.
    assert_eq!(transformed["model"], "pumice-echo");
    assert_eq!(transformed["reasoning_effort"], "none");
    assert_eq!(transformed["stream"], false);
}

#[test]
fn the_example_config_writes_every_setting() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../formatter.example.yml");
    let loaded = pumice_formatter::load(Some(&example)).expect("the example loads");
    let ids: Vec<&str> = loaded
        .config
        .formatters
        .iter()
        .map(|entry| entry.definition.id.as_str())
        .collect();
    assert_eq!(ids, ["clean-example"]);
    assert_eq!(loaded.config.default.as_deref(), Some("clean-example"));
}
