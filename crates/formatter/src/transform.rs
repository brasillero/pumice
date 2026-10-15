//! Deterministic transformation of one OpenAI-shaped chat request
//! (p11-formatter/D2).
//!
//! [`apply`] inserts the formatter's `system` text as one `system` message
//! directly before the first message that is not a leading `system` or
//! `developer` message, and changes nothing else: the model, `stream`,
//! unknown top-level members and every other message (including unknown
//! message fields) are preserved exactly. If the request already carries
//! non-empty `system`/`developer` instructions the default policy refuses
//! with [`ErrorCode::FormatterInstructionConflict`]; [`ConflictPolicy::Append`]
//! instead keeps the existing instructions and inserts the formatter's
//! message right after them. The dictated text itself is never modified.

use serde_json::Value;

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
/// request.
///
/// The input must be a JSON object with a `messages` array whose entries are
/// objects with a string `role`; nothing else is required or rewritten. The
/// result differs from the input only by the inserted message.
pub fn apply(
    mut request: Value,
    formatter: &FormatterDefinition,
    policy: ConflictPolicy,
) -> Result<Value, Error> {
    let Value::Object(ref mut map) = request else {
        return Err(Error::invalid_input("the request must be a JSON object"));
    };
    let Some(messages) = map.get_mut("messages") else {
        return Err(Error::invalid_input(
            "the request has no \"messages\" array",
        ));
    };
    let Value::Array(ref mut messages) = *messages else {
        return Err(Error::invalid_input("\"messages\" must be an array"));
    };

    for (index, message) in messages.iter().enumerate() {
        let Value::Object(_) = message else {
            return Err(Error::invalid_input(format!(
                "message {index} must be an object with a string \"role\""
            )));
        };
        if !matches!(message.get("role"), Some(Value::String(_))) {
            return Err(Error::invalid_input(format!(
                "message {index} must be an object with a string \"role\""
            )));
        }
    }

    let leading = messages
        .iter()
        .take_while(|message| is_instruction_message(message))
        .count();
    let has_instructions = messages[..leading]
        .iter()
        .any(|message| content_present(message));

    if has_instructions && policy == ConflictPolicy::Reject {
        return Err(Error::formatter_instruction_conflict());
    }

    messages.insert(
        leading,
        serde_json::json!({
            "role": "system",
            "content": formatter.system,
        }),
    );
    Ok(request)
}

/// True for the instruction roles the relay already joins into the system
/// prompt (`crates/relay/src/request.rs`).
fn is_instruction_message(message: &Value) -> bool {
    matches!(
        message.get("role").and_then(Value::as_str),
        Some("system" | "developer")
    )
}

/// True when a message carries content worth protecting: a non-whitespace
/// string or a non-empty array. Absent, null and empty-string content does
/// not conflict.
fn content_present(message: &Value) -> bool {
    match message.get("content") {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(parts)) => !parts.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::builtin_clean;
    use crate::error::ErrorCode;

    fn formatter() -> FormatterDefinition {
        FormatterDefinition {
            id: "test".to_owned(),
            description: "test".to_owned(),
            system: "THE RULES".to_owned(),
        }
    }

    fn user_request() -> Value {
        serde_json::json!({
            "model": "claude",
            "messages": [{"role": "user", "content": "buy apples and milk"}],
            "stream": false,
        })
    }

    #[test]
    fn inserts_the_system_message_before_the_user_message() {
        let out = apply(user_request(), &formatter(), ConflictPolicy::Reject).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "THE RULES");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "buy apples and milk");
    }

    #[test]
    fn keeps_every_other_member_and_message_field_unchanged() {
        let mut request = user_request();
        request["messages"][0]["name"] = Value::String("dictation".to_owned());
        request["messages"][0]["extra"] = serde_json::json!({"nested": [1, 2]});
        request["reasoning_effort"] = Value::String("none".to_owned());
        request["top_p"] = serde_json::json!(0.5);

        let out = apply(request.clone(), &formatter(), ConflictPolicy::Reject).unwrap();
        for key in ["model", "stream", "reasoning_effort", "top_p"] {
            assert_eq!(out[key], request[key], "member {key} preserved");
        }
        assert_eq!(
            out["messages"][1], request["messages"][0],
            "user message preserved exactly"
        );
    }

    #[test]
    fn existing_nonempty_system_message_conflicts_on_reject() {
        let mut request = user_request();
        request["messages"] = serde_json::json!([
            {"role": "system", "content": "app rules"},
            {"role": "user", "content": "hi"},
        ]);
        let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::FormatterInstructionConflict);
        assert!(err.message.contains("--on-conflict append"));
    }

    #[test]
    fn append_inserts_after_the_existing_instructions() {
        let mut request = user_request();
        request["messages"] = serde_json::json!([
            {"role": "system", "content": "app rules"},
            {"role": "developer", "content": "more rules"},
            {"role": "user", "content": "hi"},
        ]);
        let out = apply(request, &formatter(), ConflictPolicy::Append).unwrap();
        let roles: Vec<&str> = out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "developer", "system", "user"]);
        assert_eq!(out["messages"][2]["content"], "THE RULES");
        assert_eq!(out["messages"][0]["content"], "app rules");
    }

    #[test]
    fn developer_role_alone_conflicts_on_reject() {
        let mut request = user_request();
        request["messages"] = serde_json::json!([
            {"role": "developer", "content": "dev rules"},
            {"role": "user", "content": "hi"},
        ]);
        let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
        assert_eq!(err.code, ErrorCode::FormatterInstructionConflict);
    }

    #[test]
    fn whitespace_only_system_content_does_not_conflict() {
        let mut request = user_request();
        request["messages"] = serde_json::json!([
            {"role": "system", "content": "  \n "},
            {"role": "user", "content": "hi"},
        ]);
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        let roles: Vec<&str> = out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "system", "user"]);
    }

    #[test]
    fn instructions_after_the_user_message_do_not_conflict() {
        // Only the *leading* instruction prefix is protected; a system
        // message later in the conversation is left exactly where it is.
        let mut request = user_request();
        request["messages"] = serde_json::json!([
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "late rules"},
        ]);
        let out = apply(request.clone(), &formatter(), ConflictPolicy::Reject).unwrap();
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][0]["content"], "THE RULES");
        assert_eq!(out["messages"][2], request["messages"][1]);
    }

    #[test]
    fn non_object_request_is_rejected() {
        for request in [
            Value::Null,
            Value::String("hi".to_owned()),
            serde_json::json!([]),
        ] {
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
            assert!(err.message.contains("JSON object"), "{err}");
        }
    }

    #[test]
    fn missing_or_non_array_messages_is_rejected() {
        for request in [
            serde_json::json!({"model": "claude"}),
            serde_json::json!({"messages": "hi"}),
        ] {
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        }
    }

    #[test]
    fn malformed_messages_are_rejected_with_the_index() {
        for messages in [
            serde_json::json!(["hi"]),
            serde_json::json!([{"role": 5}]),
            serde_json::json!([{"content": "no role"}]),
        ] {
            let request = serde_json::json!({"messages": messages});
            let err = apply(request, &formatter(), ConflictPolicy::Reject).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
            assert!(err.message.contains("message 0"), "{err}");
        }
    }

    #[test]
    fn builtin_clean_transforms_a_plain_dictation_request() {
        let out = apply(user_request(), &builtin_clean(), ConflictPolicy::Reject).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], builtin_clean().system);
        assert_eq!(messages[1]["content"], "buy apples and milk");
    }

    #[test]
    fn messages_without_any_non_instruction_message_still_work() {
        let request = serde_json::json!({"messages": [{"role": "system", "content": ""}]});
        let out = apply(request, &formatter(), ConflictPolicy::Reject).unwrap();
        let roles: Vec<&str> = out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "system"]);
    }
}
