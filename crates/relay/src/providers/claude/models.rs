//! Claude Code 2.1.310's model facts that `relay.yml` can contradict.
//!
//! Claude Code compares a model with its tables only after canonicalizing
//! its name (`claude-opus-4` and `claude-opus-4-20250514` both become
//! `claude-opus-4-0`), so [`canonical`] reproduces that step: a port of the
//! binary's `NU` and the helpers it calls (`Ac`, `u`, `_2o`, `LU`, `Mv`).
//! The names are 2.1.306's, where the port was made; 2.1.310 renames them
//! and keeps the same code.
//! The rules, the excerpts and the cases checked against the binary's own
//! code are in docs/providers.md.

/// Canonical names whose effort check is false: the whole `claude-3-`
/// family and these. The request builder then drops `effort` without a
/// warning.
const NO_EFFORT: [&str; 5] = [
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
];

/// Canonical names whose baked capabilities include
/// `rejects_disabled_thinking`: the CLI marks thinking as always on and
/// sends adaptive thinking even when `MAX_THINKING_TOKENS=0` asks for none.
const THINKING_ALWAYS_ON: [&str; 6] = [
    "claude-haiku-5-5",
    "claude-sonnet-5-5",
    "claude-opus-5-5",
    "claude-fable-5",
    "claude-fable-5-1",
    "claude-mythos-5-1",
];

/// The baked catalog's model ids, in its order, then the three `qv()`
/// adds; a parsed name takes the first one with its family and version.
const CATALOG_IDS: [&str; 25] = [
    "claude-3-5-haiku",
    "claude-haiku-4-5",
    "claude-haiku-5-5",
    "claude-3-5-sonnet",
    "claude-3-7-sonnet",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "claude-sonnet-5-5",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-opus-4-5",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-opus-5-5",
    "claude-fable-5",
    "claude-fable-5-1",
    "claude-mythos-5",
    "claude-mythos-5-1",
    "claude-3-opus",
    "claude-3-sonnet",
    "claude-3-haiku",
];

/// Bedrock region prefixes (`<region>.anthropic.claude-…`).
const REGIONS: [&str; 7] = ["us", "eu", "apac", "jp", "au", "us-gov", "global"];

/// One step of the fallback chain, tried in order on the whole name.
enum Rule {
    /// The name contains this text, which is also the canonical name.
    Contains(&'static str),
    /// The name contains this family and major version, not followed by
    /// `-<one digit>` (`/claude-opus-4(?!-\d(?!\d))/`).
    Major(&'static str, &'static str),
}

const FALLBACK: [Rule; 25] = [
    Rule::Contains("claude-fable-5-1"),
    Rule::Contains("claude-fable-5"),
    Rule::Contains("claude-mythos-5-1"),
    Rule::Contains("claude-mythos-5"),
    Rule::Contains("claude-opus-5-5"),
    Rule::Contains("claude-opus-5"),
    Rule::Contains("claude-opus-4-8"),
    Rule::Contains("claude-opus-4-7"),
    Rule::Contains("claude-opus-4-6"),
    Rule::Contains("claude-opus-4-5"),
    Rule::Contains("claude-opus-4-1"),
    Rule::Major("claude-opus-4", "claude-opus-4-0"),
    Rule::Contains("claude-sonnet-5-5"),
    Rule::Contains("claude-sonnet-5"),
    Rule::Contains("claude-sonnet-4-6"),
    Rule::Contains("claude-sonnet-4-5"),
    Rule::Major("claude-sonnet-4", "claude-sonnet-4-0"),
    Rule::Contains("claude-haiku-5-5"),
    Rule::Contains("claude-haiku-4-5"),
    Rule::Contains("claude-3-7-sonnet"),
    Rule::Contains("claude-3-5-sonnet"),
    Rule::Contains("claude-3-5-haiku"),
    Rule::Contains("claude-3-opus"),
    Rule::Contains("claude-3-sonnet"),
    Rule::Contains("claude-3-haiku"),
];

/// Spellings `Dt` (the `--model` resolution) replaces with the default
/// Opus on first-party, gateway and Anthropic cloud setups, unless
/// `CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP` is set. Like an alias, which
/// model they reach depends on the account and the environment, so the
/// effort check does not refuse them; the thinking check needs no exception,
/// since they canonicalize to `claude-opus-4-0` and `claude-opus-4-1`, which
/// can turn thinking off.
const REMAPPED: [&str; 4] = [
    "claude-opus-4-20250514",
    "claude-opus-4-1-20250805",
    "claude-opus-4-0",
    "claude-opus-4-1",
];

/// Whether Claude Code 2.1.310 sends no effort for `model`.
pub(super) fn has_no_effort(model: &str) -> bool {
    if remapped(model) {
        return false;
    }
    let name = canonical(model);
    name.contains("claude-3-") || NO_EFFORT.contains(&name.as_str())
}

/// Whether Claude Code 2.1.310 keeps thinking on for `model` whatever
/// `MAX_THINKING_TOKENS` says.
pub(super) fn keeps_thinking_on(model: &str) -> bool {
    let name = canonical(model).replace("[1m]", "");
    THINKING_ALWAYS_ON.contains(&name.as_str())
}

/// `Dt`'s test: the trimmed, lowercased name, without a `[1m]` suffix.
fn remapped(model: &str) -> bool {
    let name = model.trim().to_lowercase();
    let name = name.strip_suffix("[1m]").map_or(name.as_str(), str::trim);
    REMAPPED.contains(&name)
}

/// The name Claude Code compares with its tables (`NU` on the lowercased
/// name). Its first step, a lookup of the catalog's exact provider ids,
/// is left out: every one of them canonicalizes the same way without it.
fn canonical(model: &str) -> String {
    let name = model.to_lowercase();
    if let Some(parsed) = parse(&name)
        && !parsed.trailer
    {
        return catalog_match(&parsed)
            .map(str::to_owned)
            .unwrap_or(parsed.base);
    }
    for rule in &FALLBACK {
        match *rule {
            Rule::Contains(text) if name.contains(text) => return text.to_owned(),
            Rule::Major(text, canonical) if contains_major(&name, text) => {
                return canonical.to_owned();
            }
            _ => {}
        }
    }
    strip_date(&name).to_owned()
}

/// A name `Ac` parses: `claude-<family>-<major>[-<minor>]` (or the legacy
/// `claude-<major>[-<minor>]-<family>`), after an optional path and
/// `[<region>.]anthropic.` prefix.
struct Parsed {
    family: String,
    major: u32,
    minor: Option<u32>,
    legacy: bool,
    base: String,
    /// Something follows the version that is not a date, version or
    /// `-fast`/`-latest` suffix.
    trailer: bool,
}

/// `Ac`: `None` where the binary returns `null`.
fn parse(name: &str) -> Option<Parsed> {
    let name = name.trim().to_lowercase();
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    let name = name
        .strip_suffix("[1m]")
        .or_else(|| name.strip_suffix("[2m]"))
        .unwrap_or(&name);
    let name = name.rsplit('/').next().unwrap_or(name);
    let name = strip_anthropic_prefix(name)?;
    let base = parse_base(name)?;
    let rest = &name[base.len..];
    if !rest.is_empty() && !rest.starts_with(['-', '@']) {
        return None;
    }
    Some(Parsed {
        family: base.family.to_owned(),
        major: base.major,
        minor: base.minor,
        legacy: base.legacy,
        base: name[..base.len].to_owned(),
        trailer: !known_suffix(rest),
    })
}

/// `/^(?:([a-z-]+)\.)?anthropic\.(claude-.*)$/`: the name after the prefix,
/// the name itself without one, or `None` for a region the binary does not
/// know.
fn strip_anthropic_prefix(name: &str) -> Option<&str> {
    if let Some((region, rest)) = name.split_once('.')
        && !region.is_empty()
        && region.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        && let Some(model) = rest.strip_prefix("anthropic.")
        && model.starts_with("claude-")
    {
        return REGIONS.contains(&region).then_some(model);
    }
    Some(match name.strip_prefix("anthropic.") {
        Some(model) if model.starts_with("claude-") => model,
        _ => name,
    })
}

struct Base<'a> {
    family: &'a str,
    major: u32,
    minor: Option<u32>,
    legacy: bool,
    /// Bytes of the name the version took.
    len: usize,
}

/// `u`: `/^claude-([a-z]+)-(\d{1,2})(?!\d)(?:-(\d{1,2})(?!\d))?/`, else
/// the legacy `/^claude-(\d{1,2})(?!\d)(?:-(\d{1,2})(?!\d))?-([a-z]+)/`.
fn parse_base(name: &str) -> Option<Base<'_>> {
    const PREFIX: usize = "claude-".len();
    let rest = name.strip_prefix("claude-")?;

    let family_len = letters(rest);
    if family_len > 0
        && let Some(after) = rest[family_len..].strip_prefix('-')
        && let Some((major, major_len)) = version_number(after)
    {
        let (minor, minor_len) = after[major_len..]
            .strip_prefix('-')
            .and_then(version_number)
            .map_or((None, 0), |(minor, len)| (Some(minor), len + 1));
        return Some(Base {
            family: &rest[..family_len],
            major,
            minor,
            legacy: false,
            len: PREFIX + family_len + 1 + major_len + minor_len,
        });
    }

    let (major, major_len) = version_number(rest)?;
    let mut len = major_len;
    let mut minor = None;
    if let Some((number, number_len)) = rest[len..].strip_prefix('-').and_then(version_number) {
        minor = Some(number);
        len += 1 + number_len;
    }
    let family = rest[len..].strip_prefix('-')?;
    let family_len = letters(family);
    if family_len == 0 {
        return None;
    }
    Some(Base {
        family: &family[..family_len],
        major,
        minor,
        legacy: true,
        len: PREFIX + len + 1 + family_len,
    })
}

/// `(\d{1,2})(?!\d)`: a leading run of one or two digits, with its length.
fn version_number(text: &str) -> Option<(u32, usize)> {
    let len = digits(text);
    if !(1..=2).contains(&len) {
        return None;
    }
    Some((text[..len].parse().ok()?, len))
}

/// `_2o`: `/^(?:-fast|-latest)?(?:-v\d{1,3}@\d{8}|[-@]\d{8})?(?:-v\d{1,3}(?::\d{1,3})?)?$/`.
fn known_suffix(rest: &str) -> bool {
    ["", "-fast", "-latest"].iter().any(|speed| {
        rest.strip_prefix(speed).is_some_and(|rest| {
            [Some(rest), versioned_date(rest), plain_date(rest)]
                .into_iter()
                .flatten()
                .any(version_tail)
        })
    })
}

/// `-v\d{1,3}@\d{8}`: what follows it.
fn versioned_date(text: &str) -> Option<&str> {
    let text = text.strip_prefix("-v")?;
    let len = digits(text);
    if !(1..=3).contains(&len) {
        return None;
    }
    eight_digits(text[len..].strip_prefix('@')?)
}

/// `[-@]\d{8}`: what follows it.
fn plain_date(text: &str) -> Option<&str> {
    eight_digits(text.strip_prefix(['-', '@'])?)
}

fn eight_digits(text: &str) -> Option<&str> {
    (text.len() >= 8 && text.as_bytes()[..8].iter().all(u8::is_ascii_digit)).then(|| &text[8..])
}

/// `(?:-v\d{1,3}(?::\d{1,3})?)?$`
fn version_tail(text: &str) -> bool {
    let Some(text) = text.strip_prefix("-v") else {
        return text.is_empty();
    };
    let len = digits(text);
    if !(1..=3).contains(&len) {
        return false;
    }
    match text[len..].strip_prefix(':') {
        None => text.len() == len,
        Some(revision) => (1..=3).contains(&digits(revision)) && digits(revision) == revision.len(),
    }
}

/// `LU`: the first catalog id with the same family, layout and version
/// (a missing minor counts as `0`).
fn catalog_match(parsed: &Parsed) -> Option<&'static str> {
    CATALOG_IDS.into_iter().find(|id| {
        parse(id).is_some_and(|entry| {
            !entry.trailer
                && entry.family == parsed.family
                && entry.legacy == parsed.legacy
                && entry.major == parsed.major
                && entry.minor.unwrap_or(0) == parsed.minor.unwrap_or(0)
        })
    })
}

/// Whether `text` occurs in `name` not followed by `-` and exactly one
/// digit.
fn contains_major(name: &str, text: &str) -> bool {
    name.match_indices(text).any(|(at, _)| {
        let after = &name[at + text.len()..];
        !after
            .strip_prefix('-')
            .is_some_and(|version| digits(version) == 1)
    })
}

/// `Mv`: drops a trailing `-YYYYMMDD`.
fn strip_date(name: &str) -> &str {
    match name.len().checked_sub(9) {
        Some(at)
            if name.as_bytes()[at] == b'-'
                && name.as_bytes()[at + 1..].iter().all(u8::is_ascii_digit) =>
        {
            &name[..at]
        }
        _ => name,
    }
}

fn letters(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_lowercase).count()
}

fn digits(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each expected name is what the binary's own functions return, run
    /// under node on the extracted code; the port also matched them on
    /// 94,263 generated spellings (docs/providers.md).
    const CASES: &[(&str, &str)] = &[
        ("claude-sonnet-4", "claude-sonnet-4-0"),
        ("claude-sonnet-4-20250514", "claude-sonnet-4-0"),
        ("claude-sonnet-4@20250514", "claude-sonnet-4-0"),
        ("claude-sonnet-4-latest", "claude-sonnet-4-0"),
        ("us.anthropic.claude-sonnet-4-20250514-v1:0", "claude-sonnet-4-0"),
        ("eu.anthropic.claude-sonnet-4-20250514-v1:0", "claude-sonnet-4-0"),
        ("claude-opus-4", "claude-opus-4-0"),
        ("claude-opus-4-20250514", "claude-opus-4-0"),
        ("claude-opus-4@20250514", "claude-opus-4-0"),
        ("claude-opus-4-1-20250805", "claude-opus-4-1"),
        ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
        ("claude-haiku-4-5@20251001", "claude-haiku-4-5"),
        ("claude-haiku-4-5[1m]", "claude-haiku-4-5"),
        ("anthropic.claude-haiku-4-5", "claude-haiku-4-5"),
        ("claude-3-5-haiku-20241022", "claude-3-5-haiku"),
        ("claude-3-5-sonnet-v2@20241022", "claude-3-5-sonnet"),
        ("claude-3-opus-20240229", "claude-3-opus"),
        ("claude-haiku-5-5", "claude-haiku-5-5"),
        ("Claude-Haiku-5-5", "claude-haiku-5-5"),
        ("Claude-Haiku-5-5[1M]", "claude-haiku-5-5"),
        ("claude-haiku-5-5-20250514", "claude-haiku-5-5"),
        ("claude-sonnet-5-5", "claude-sonnet-5-5"),
        ("claude-fable-5-0", "claude-fable-5"),
        ("claude-sonnet-5-0", "claude-sonnet-5"),
        ("claude-opus-4-10", "claude-opus-4-10"),
        ("claude-haiku-4-50", "claude-haiku-4-50"),
        ("claude-opus-41", "claude-opus-41"),
        ("claude-opus-4-10-foo", "claude-opus-4-1"),
        ("claude-opus-4x", "claude-opus-4-0"),
        ("claude-opus-4-1-foo", "claude-opus-4-1"),
        ("my-claude-opus-4", "claude-opus-4-0"),
        ("xx.anthropic.claude-opus-4", "claude-opus-4-0"),
        ("anthropic.anthropic.claude-opus-4", "claude-opus-4-0"),
        ("claude-haiku-4", "claude-haiku-4"),
        ("claude-haiku-4.5", "claude-haiku-4.5"),
        ("claude-sonnet-4-1", "claude-sonnet-4-1"),
        ("haiku", "haiku"),
        ("sonnet", "sonnet"),
        ("opus", "opus"),
        ("opusplan", "opusplan"),
        ("haiku[1m]", "haiku[1m]"),
        ("glm-5", "glm-5"),
        (" claude-opus-4 ", "claude-opus-4-0"),
    ];

    #[test]
    fn canonicalizes_like_claude_code() {
        for (model, expected) in CASES {
            assert_eq!(canonical(model), *expected, "{model}");
        }
    }

    #[test]
    fn remapped_opus_spellings_are_left_to_run_time() {
        for model in [
            "claude-opus-4-0",
            "Claude-Opus-4-1",
            "claude-opus-4-20250514",
            " claude-opus-4-1-20250805[1m] ",
        ] {
            assert!(!has_no_effort(model), "{model}");
            assert!(!keeps_thinking_on(model), "{model}");
        }
        for model in ["claude-opus-4", "claude-opus-4-0@20250514", "claude-opus-4-1-fast"] {
            assert!(has_no_effort(model), "{model}");
        }
    }
}
