//! A tripwire for agent names written into text without their machine.
//!
//! Agent names are bare labels and they collide — a Circle routinely holds two
//! `claude`s — so any agent named in prose must go through
//! `agent::label::Roster`, which renders `claude (on suzy/jessair)`. Before that
//! existed, bare names had crept into prompt history, thread ancestors, failure
//! notices, the ambient instruction and the inbox, each one a separate bug.
//!
//! This scans `format!` calls in the code that produces such text and fails on
//! any that interpolates an agent's name, unless it is one of the allowed
//! forms below. It is a tripwire, not a proof: it works from what things are
//! called, so a name smuggled in under another variable will get past it. What
//! it reliably catches is the pattern every one of those bugs followed.
use std::path::Path;

/// Text that looks like an agent name but is not prose about one, each with
/// its reason. Add to this only for identifiers — never to silence a real one.
const ALLOWED: &[(&str, &str)] = &[
    ("\"claim:{", "activity id for an explicit claim"),
    ("\"~ambient:{", "ambient dedup key"),
    ("\"agent:{", "activity id for a running turn"),
    ("\"@{agent}\"", "a handle or dedup key, not a description"),
    ("roster.", "already named through the roster"),
    ("Roster", "already named through the roster"),
    (
        "You are \\\"{agent}\\\"",
        "the agent being told its own name",
    ),
    (
        "Your own address is",
        "the agent's own handle, fully qualified",
    ),
    ("To address someone", "handle grammar, no particular agent"),
];

/// What an agent's name looks like inside a `format!`.
fn names_an_agent(span: &str) -> bool {
    span.contains("agent_id")
        || span.contains("{agent}")
        || span.contains(".agent,")
        || span.contains(".agent)")
        || span.contains(".agent ")
}

fn format_calls(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("format!(") {
        let start = from + at;
        let (mut i, mut depth) = (start + "format!(".len(), 1);
        let bytes = src.as_bytes();
        while depth > 0 && i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        out.push((
            src[..start].matches('\n').count() + 1,
            src[start..i].to_string(),
        ));
        from = i;
    }
    out
}

#[test]
fn no_agent_is_named_in_prose_without_its_machine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for dir in ["src/agent", "src/api", "src/commands"] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            // The one place allowed to build the qualified form from parts.
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("label.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
            for (line, span) in format_calls(code) {
                if names_an_agent(&span) && !ALLOWED.iter().any(|(form, _)| span.contains(form)) {
                    let shown: String = span.split_whitespace().collect::<Vec<_>>().join(" ");
                    offenders.push(format!(
                        "{}:{line}: {}",
                        path.strip_prefix(root).unwrap().display(),
                        shown.chars().take(120).collect::<String>()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "an agent is named in text without its machine — use \
         agent::label::Roster (or, for a pure identifier, add it to ALLOWED \
         with a reason):\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_tripwire_catches_the_shape_it_was_written_for() {
    // Every bug it guards against looked like one of these.
    for bare in [
        r#"format!("{agent} could not answer")"#,
        r#"format!("  {}: {}", m.agent_id, text)"#,
        r#"format!("{} is already working on it", claim.agent)"#,
    ] {
        let (_, span) = format_calls(bare).remove(0);
        assert!(
            names_an_agent(&span) && !ALLOWED.iter().any(|(f, _)| span.contains(f)),
            "missed {bare}"
        );
    }
    let (_, ok) = format_calls(r#"format!("{} is working", roster.agent(a, p))"#).remove(0);
    assert!(ALLOWED.iter().any(|(f, _)| ok.contains(f)));
}
