//! Deterministic transformation of one OpenAI-shaped chat request
//! (p11-formatter/D2).
//!
//! [`apply`] inserts the formatter's `system` text as one `system` message
//! directly before the first message that is not a leading `system` or
//! `developer` message, and changes nothing else. It works on the raw
//! request text: every value the client sent — numbers of any size or
//! precision, key order, string escapes, unknown members — is re-emitted
//! byte for byte from the input; top-level key names are re-serialized in
//! normalized form (escape sequences are canonicalized), and only
//! whitespace between top-level members and between `messages` elements
//! may differ. If the request carries
//! non-empty `system`/`developer` instructions *anywhere* in the
//! conversation — not just in the leading run — the default policy refuses
//! with [`ErrorCode::FormatterInstructionConflict`]; [`ConflictPolicy::Append`]
//! instead keeps the existing instructions and inserts the formatter's
//! message right after the leading ones. The dictated text itself is never
//! modified.

use std::borrow::Cow;
use std::collections::HashSet;

use serde::Deserialize;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::definition::FormatterDefinition;
use crate::error::Error;

/// What to do when the request already has non-empty `system`/`developer`
/// instructions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ConflictPolicy {
    /// Refuse with `formatter_instruction_conflict` (the default).
    #[default]
    Reject,
    /// Keep the existing instructions and insert the formatter's message
    /// right after the leading `system`/`developer` messages.
    Append,
}

impl std::fmt::Display for ConflictPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConflictPolicy::Reject => "reject",
            ConflictPolicy::Append => "append",
        })
    }
}

/// Applies `formatter` to one chat request, returning the transformed
/// request JSON.
///
/// The input must be a JSON object with a `messages` array whose entries are
/// objects with a string `role`; nothing else is required or rewritten. The
/// result differs from the input only by the inserted message, by
/// whitespace between top-level members and between `messages` elements,
/// and by the normalization of top-level key names: every value is
/// re-emitted byte for byte ([`RawValue`]), so numbers of any size or
/// precision, key order and string escapes survive untouched. Error
/// messages never quote request text (dictated content).
pub fn apply(
    request: &str,
    formatter: &FormatterDefinition,
    policy: ConflictPolicy,
) -> Result<String, Error> {
    // Reject non-objects before serde_json sees them: its type errors echo
    // the value (dictated text) into the message, which must never leak.
    let trimmed = request.trim_start_matches([' ', '\t', '\n', '\r']);
    if !trimmed.is_empty() && !trimmed.starts_with('{') {
        return Err(Error::invalid_input("the request must be a JSON object"));
    }
    // Only the position is reported, never serde's message: error text must
    // not carry client-controlled strings.
    let TopLevel(entries) = serde_json::from_str::<TopLevel>(request).map_err(|err| {
        Error::invalid_input(format!(
            "the request is not valid JSON (line {}, column {})",
            err.line(),
            err.column()
        ))
    })?;

    // The relay rejects duplicate struct fields (serde's "duplicate field"
    // error becomes its HTTP 400), so the formatter refuses repeated
    // top-level members too instead of silently keeping one of them. The
    // key itself is client-controlled, so the error does not name it.
    let mut seen = HashSet::with_capacity(entries.len());
    for (key, _) in &entries {
        if !seen.insert(key.as_str()) {
            return Err(Error::invalid_input(
                "the request repeats a top-level member; duplicate keys are rejected",
            ));
        }
    }

    let Some(messages_raw) = entries
        .iter()
        .find(|(key, _)| key == "messages")
        .map(|(_, value)| value)
    else {
        return Err(Error::invalid_input(
            "the request has no \"messages\" array",
        ));
    };
    // `messages_raw` already parsed as JSON, so a failure here can only mean
    // the value is not an array.
    let messages: Vec<Box<RawValue>> = serde_json::from_str(messages_raw.get())
        .map_err(|_| Error::invalid_input("\"messages\" must be an array"))?;

    let mut probes = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        // A derived struct would also accept positional arrays
        // (`["user","hi"]`); each message must be a JSON object.
        let malformed = || {
            Error::invalid_input(format!(
                "message {index} must be an object with a string \"role\""
            ))
        };
        if !starts_with_object(message.get()) {
            return Err(malformed());
        }
        let probe = serde_json::from_str::<MessageProbe>(message.get()).map_err(|_| malformed())?;
        probes.push(probe);
    }

    let leading = probes
        .iter()
        .take_while(|probe| probe.is_instruction())
        .count();
    // Every instruction message in the conversation conflicts, not just the
    // leading run: inserting the formatter before a `system` message that
    // sits after the user text would still reorder the client's own
    // instructions.
    let has_instructions = probes
        .iter()
        .any(|probe| probe.is_instruction() && probe.content_present());

    if has_instructions && policy == ConflictPolicy::Reject {
        return Err(Error::formatter_instruction_conflict());
    }

    let content = serde_json::to_string(&formatter.system).expect("a string always serializes");
    let inserted = format!("{{\"role\":\"system\",\"content\":{content}}}");

    let mut out = String::with_capacity(request.len() + inserted.len() + 2);
    out.push('{');
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).expect("a string always serializes"));
        out.push(':');
        if key == "messages" {
            write_messages(&mut out, &messages, &inserted, leading);
        } else {
            out.push_str(value.get());
        }
    }
    out.push('}');
    Ok(out)
}

/// Re-emits the `messages` array: the raw bytes of every client message,
/// with the formatter's freshly serialized message inserted at `position`.
fn write_messages(out: &mut String, messages: &[Box<RawValue>], inserted: &str, position: usize) {
    out.push('[');
    let mut first = true;
    for index in 0..=messages.len() {
        if index == position {
            if !first {
                out.push(',');
            }
            out.push_str(inserted);
            first = false;
        }
        if let Some(message) = messages.get(index) {
            if !first {
                out.push(',');
            }
            out.push_str(message.get());
            first = false;
        }
    }
    out.push(']');
}

/// The request's top-level members in order, each value kept as raw JSON.
struct TopLevel(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for TopLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TopLevelVisitor;
        impl<'de> Visitor<'de> for TopLevelVisitor {
            type Value = TopLevel;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    entries.push(entry);
                }
                Ok(TopLevel(entries))
            }
        }
        deserializer.deserialize_map(TopLevelVisitor)
    }
}

/// True when `raw` starts with `{` after any whitespace: a JSON object.
fn starts_with_object(raw: &str) -> bool {
    raw.trim_start_matches([' ', '\t', '\n', '\r'])
        .starts_with('{')
}

/// The only fields `apply` needs from a message: its `role` and its raw
/// `content`. Everything else stays in the raw bytes.
#[derive(Deserialize)]
struct MessageProbe<'a> {
    #[serde(borrow)]
    role: Cow<'a, str>,
    #[serde(default)]
    content: Option<Box<RawValue>>,
}

impl MessageProbe<'_> {
    /// True for the instruction roles the relay already joins into the
    /// system prompt (`crates/relay/src/request.rs`).
    fn is_instruction(&self) -> bool {
        matches!(&*self.role, "system" | "developer")
    }

    /// True when the message carries content worth protecting: a
    /// non-whitespace string, a non-empty array or any other value. Absent,
    /// null, empty-string and empty-array content does not conflict.
    fn content_present(&self) -> bool {
        self.content
            .as_deref()
            .is_some_and(|raw| content_present(raw.get()))
    }
}

/// Whether one raw `content` value is worth protecting, classified from its
/// first byte so numbers are never converted (serde_json's `deserialize_any`
/// turns `1e400` into an f64 overflow error, which would surface as a
/// misleading "message must be an object" failure): strings conflict unless
/// blank, arrays unless empty, `null` never (the `Option` usually absorbs it
/// as absent first), every other value — objects, booleans, numbers of any
/// size — always.
fn content_present(raw: &str) -> bool {
    let text = raw.trim_start_matches([' ', '\t', '\n', '\r']);
    match text.as_bytes().first() {
        // The raw value already parsed as JSON, so these cannot fail.
        Some(b'"') => serde_json::from_str::<String>(text)
            .map(|string| !string.trim().is_empty())
            .unwrap_or(true),
        Some(b'[') => serde_json::from_str::<Vec<IgnoredAny>>(text)
            .map(|items| !items.is_empty())
            .unwrap_or(true),
        Some(b'n') => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::builtin_clean;
    use crate::error::ErrorCode;
    use serde_json::Value;

    fn formatter() -> FormatterDefinition {
        FormatterDefinition {
            id: "test".to_owned(),
            description: "test".to_owned(),
            system: "THE RULES".to_owned(),
        }
    }

    fn user_request() -> &'static str {
        r#"{"model":"claude","messages":[{"role":"user","content":"buy apples and milk"}],"stream":false}"#
    }

    fn value(json: &str) -> Value {
        serde_json::from_str(json).expect("the JSON parses")
    }

    fn roles(json: &str) -> Vec<String> {
        value(json)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["role"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The raw bytes of each message in a (possibly transformed) request.
    fn raw_messages(json: &str) -> Vec<Box<RawValue>> {
        #[derive(Deserialize)]
        struct RawMessages {
            messages: Vec<Box<RawValue>>,
        }
        serde_json::from_str::<RawMessages>(json).unwrap().messages
    }

    #[test]
    fn inserts_the_system_message_before_the_user_message() {
        let out = apply(user_request(), &formatter(), ConflictPolicy::Reject).unwrap();
        let messages = value(&out);
        let messages = messages["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "THE RULES");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "buy apples and milk");
    }

    #[test]
    fn keeps_every_other_member_and_message_field_unchanged() {
        let request = r#"{"model":"claude","messages":[{"role":"user","content":"hi","name":"dictation","extra":{"nested":[1,2]}}],"reasoning_effort":"none","top_p":0.5}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        for member in [
            r#""name":"dictation""#,
            r#""extra":{"nested":[1,2]}"#,
            r#""reasoning_effort":"none""#,
            r#""top_p":0.5"#,
        ] {
            assert!(out.contains(member), "member {member} preserved: {out}");
        }
        let out_messages = raw_messages(&out);
        let in_messages = raw_messages(request);
        assert_eq!(out_messages[1].get(), in_messages[0].get());
    }

    #[test]
    fn existing_nonempty_system_message_conflicts_on_reject() {
        let request = r#"{"messages":[{"role":"system","content":"app rules"},{"role":"user","content":"hi"}]}"#;
        let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::FormatterInstructionConflict);
        assert!(err.message.contains("--on-conflict append"));
    }

    #[test]
    fn append_inserts_after_the_existing_instructions() {
        let request = r#"{"messages":[{"role":"system","content":"app rules"},{"role":"developer","content":"more rules"},{"role":"user","content":"hi"}]}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Append).unwrap();
        assert_eq!(roles(&out), ["system", "developer", "system", "user"]);
        let messages = value(&out);
        assert_eq!(messages["messages"][2]["content"], "THE RULES");
        assert_eq!(messages["messages"][0]["content"], "app rules");
    }

    #[test]
    fn developer_role_alone_conflicts_on_reject() {
        let request = r#"{"messages":[{"role":"developer","content":"dev rules"},{"role":"user","content":"hi"}]}"#;
        let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::FormatterInstructionConflict);
    }

    #[test]
    fn whitespace_only_system_content_does_not_conflict() {
        let request =
            r#"{"messages":[{"role":"system","content":"  \n "},{"role":"user","content":"hi"}]}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert_eq!(roles(&out), ["system", "system", "user"]);
    }

    #[test]
    fn instructions_after_the_user_message_still_conflict() {
        // The conflict scan covers every system/developer message, not just
        // the leading run: a system message later in the conversation is the
        // client's own instruction and must not be bypassed.
        for instruction in [
            r#"{"role":"system","content":"late rules"}"#,
            r#"{"role":"developer","content":"late rules"}"#,
            r#"{"role":"system","content":[{"type":"text","text":""}]}"#,
        ] {
            let request =
                format!(r#"{{"messages":[{{"role":"user","content":"hi"}},{instruction}]}}"#);
            let err = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::FormatterInstructionConflict);
        }
    }

    #[test]
    fn append_keeps_late_instructions_where_they_are() {
        // With append, a late system message stays untouched and the
        // formatter's message goes before the first user message.
        for role in ["system", "developer"] {
            let request = format!(
                r#"{{"messages":[{{"role":"user","content":"hi"}},{{"role":"{role}","content":"late rules"}}]}}"#
            );
            let out = apply(&request, &formatter(), ConflictPolicy::Append).unwrap();
            assert_eq!(roles(&out), ["system", "user", role]);
            let messages = value(&out);
            assert_eq!(messages["messages"][0]["content"], "THE RULES");
            let in_messages = raw_messages(&request);
            let out_messages = raw_messages(&out);
            assert_eq!(out_messages[1].get(), in_messages[0].get());
            assert_eq!(out_messages[2].get(), in_messages[1].get());
        }
    }

    #[test]
    fn non_object_request_is_rejected() {
        for request in ["null", r#""hi""#, "[]", "5"] {
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
            assert!(err.message.contains("JSON object"), "{err}");
        }
    }

    #[test]
    fn empty_input_is_not_valid_json() {
        let err = apply("", &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        assert!(err.message.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn missing_or_non_array_messages_is_rejected() {
        for request in [r#"{"model":"claude"}"#, r#"{"messages":"hi"}"#] {
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        }
    }

    #[test]
    fn malformed_messages_are_rejected_with_the_index() {
        for messages in [
            r#"["hi"]"#,
            r#"[{"role": 5}]"#,
            r#"[{"content": "no role"}]"#,
        ] {
            let request = format!(r#"{{"messages": {messages}}}"#);
            let err = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
            assert!(err.message.contains("message 0"), "{err}");
        }
    }

    #[test]
    fn duplicate_top_level_members_are_rejected() {
        // Consistent with the relay, whose typed parse fails on duplicate
        // struct fields; silently keeping one value would rewrite the
        // client's request.
        for request in [
            r#"{"messages":[],"messages":[]}"#,
            r#"{"model":"a","messages":[],"model":"b"}"#,
        ] {
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
            assert!(err.message.contains("repeats a top-level member"), "{err}");
        }
    }

    #[test]
    fn duplicate_member_errors_never_name_the_key() {
        // Keys are client-controlled text: the error must not echo them.
        let request = r#"{"messages":[],"PRIVATE_DICTATION":1,"PRIVATE_DICTATION":2}"#;
        let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        assert!(!err.message.contains("PRIVATE_DICTATION"), "{err}");
    }

    #[test]
    fn many_distinct_top_level_members_stay_fast() {
        // Near-1 MiB of distinct keys: a quadratic duplicate scan needed
        // seconds here; the HashSet finishes inside the normal test budget.
        let mut request = String::from(r#"{"messages":[{"role":"user","content":"hi"}]"#);
        for index in 0..10_000 {
            request.push_str(&format!(",\"key_{index:05}\":\"{:0>80}\"", index));
        }
        request.push('}');
        assert!(request.len() > 900_000, "{}", request.len());
        let out = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert!(out.contains("\"key_09999\""), "{out:.200}");
    }

    #[test]
    fn messages_must_be_objects_not_arrays_scalars_or_null() {
        // A derived struct would accept positional arrays; every other shape
        // is rejected with the same message.
        for messages in [
            r#"[["user","hi"]]"#,
            r#"[["system","rules"],{"role":"user","content":"hi"}]"#,
            r#"["hi"]"#,
            r#"[5]"#,
            r#"[null]"#,
        ] {
            let request = format!(r#"{{"messages": {messages}}}"#);
            let err = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{messages}: {err}");
            assert!(err.message.contains("message 0"), "{messages}: {err}");
        }
    }

    #[test]
    fn content_of_any_number_size_is_classified_without_conversion() {
        // serde_json's deserialize_any fails on f64 overflow ("number out of
        // range"); content must be classified from its raw bytes instead.
        // A user message carries no instructions, so it always transforms.
        for policy in [ConflictPolicy::Reject, ConflictPolicy::Append] {
            let request = r#"{"messages":[{"role":"user","content":1e400}]}"#;
            let out = apply(request, &formatter(), policy).unwrap();
            assert!(out.contains(r#""content":1e400"#), "{policy}: {out}");
        }
    }

    #[test]
    fn huge_number_content_still_conflicts_on_instructions() {
        // Numbers (of any size), booleans and objects count as instructions
        // and conflict on reject; append keeps them in place. Byte positions
        // stand in for a Value parse here: serde_json::Value itself rejects
        // f64 overflows, which is exactly what this test exercises.
        for content in ["1e400", "true", r#"{"text":"rules"}"#] {
            let request = format!(
                r#"{{"messages":[{{"role":"system","content":{content}}},{{"role":"user","content":"hi"}}]}}"#
            );
            let err = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(
                err.code,
                ErrorCode::FormatterInstructionConflict,
                "{content}: {err}"
            );
            let out = apply(&request, &formatter(), ConflictPolicy::Append).unwrap();
            let existing = out.find(&format!("\"content\":{content}")).unwrap();
            let inserted = out.find("THE RULES").unwrap();
            let user = out.find(r#""content":"hi""#).unwrap();
            assert!(existing < inserted && inserted < user, "{content}: {out}");
        }
    }

    #[test]
    fn empty_and_null_content_still_does_not_conflict() {
        for content in ["null", r#"[]"#, r#" "" "#] {
            let request = format!(
                r#"{{"messages":[{{"role":"system","content":{content}}},{{"role":"user","content":"hi"}}]}}"#
            );
            let out = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap();
            assert_eq!(roles(&out), ["system", "system", "user"], "{content}");
        }
    }

    #[test]
    fn top_level_key_names_are_re_emitted_normalized() {
        // Values stay byte for byte, but key names go through JSON string
        // serialization, so escapes in a key are canonicalized.
        let request =
            "{\n \"mod\\u0065l\" : \"x\",\t\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}] }";
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert!(out.contains(r#"{"model":"x""#), "{out}");
        assert!(out.contains(r#""content":"hi""#), "{out}");
    }

    #[test]
    fn values_pass_through_byte_for_byte() {
        // Numbers of any size or precision, odd keys (including serde_json's
        // internal number marker) and escapes survive untouched.
        let request = r#"{"big":18446744073709551617,"negative":-18446744073709551617,"decimal":1.1000000000000000055511151231257827021181583404541015625,"huge":1e400,"zero":-0,"one":1.0,"other":1,"metadata":{"$serde_json::private::Number":"123"},"escaped":"\u00e9","messages":[{"role":"user","content":"hi","id":18446744073709551617}]}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        for fragment in [
            r#""big":18446744073709551617"#,
            r#""negative":-18446744073709551617"#,
            r#""decimal":1.1000000000000000055511151231257827021181583404541015625"#,
            r#""huge":1e400"#,
            r#""zero":-0"#,
            r#""one":1.0"#,
            r#""other":1"#,
            r#""metadata":{"$serde_json::private::Number":"123"}"#,
            "\"escaped\":\"\\u00e9\"",
            r#""id":18446744073709551617"#,
        ] {
            assert!(out.contains(fragment), "missing {fragment} in {out}");
        }
    }

    #[test]
    fn nested_key_order_is_preserved() {
        // OpenAI Structured Outputs follow schema key order.
        let request = r#"{"messages":[{"role":"user","content":"hi"}],"response_format":{"json_schema":{"schema":{"properties":{"zeta":{"type":"string"},"alpha":{"type":"integer"}}}}}}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert!(
            out.find("\"zeta\"").unwrap() < out.find("\"alpha\"").unwrap(),
            "zeta serializes before alpha: {out}"
        );
    }

    #[test]
    fn multimodal_content_arrays_pass_through_byte_for_byte() {
        let parts = r#"[{"type":"text","text":"describe this"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]"#;
        let request = format!(r#"{{"messages":[{{"role":"user","content":{parts}}}]}}"#);
        let out = apply(&request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert!(out.contains(parts), "{out}");
    }

    #[test]
    fn builtin_clean_transforms_a_plain_dictation_request() {
        let out = apply(user_request(), &builtin_clean(), ConflictPolicy::Reject).unwrap();
        let messages = value(&out);
        assert_eq!(messages["messages"][0]["role"], "system");
        assert_eq!(
            messages["messages"][0]["content"].as_str().unwrap(),
            builtin_clean().system
        );
        assert_eq!(messages["messages"][1]["content"], "buy apples and milk");
    }

    #[test]
    fn messages_without_any_non_instruction_message_still_work() {
        let request = r#"{"messages":[{"role":"system","content":""}]}"#;
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        assert_eq!(roles(&out), ["system", "system"]);
    }

    #[test]
    fn a_handy_request_loses_only_the_inserted_message() {
        // The real request Handy sends (captured in the relay's fixtures):
        // removing the inserted message must give back the original request
        // semantically, and every original message byte for byte.
        let original = include_str!("../../relay/tests/fixtures/handy-request.json");
        let out = apply(original, &builtin_clean(), ConflictPolicy::Reject).unwrap();

        let mut transformed = value(&out);
        let messages = transformed["messages"].as_array_mut().unwrap();
        assert_eq!(messages[0]["role"], "system");
        messages.remove(0);
        assert_eq!(transformed, value(original));

        let original_messages = raw_messages(original);
        let out_messages = raw_messages(&out);
        assert_eq!(out_messages.len(), original_messages.len() + 1);
        for (index, message) in original_messages.iter().enumerate() {
            assert_eq!(out_messages[index + 1].get(), message.get());
        }
    }
}
