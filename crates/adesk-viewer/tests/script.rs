//! Integration tests for the `adesk-viewer` input-script parser.
//!
//! The grammar is documented in `crates/adesk-viewer/src/script.rs` and re-exported
//! at the crate root (`adesk_viewer::{parse_script, ScriptCommand, ScriptError}`);
//! these tests pin that public contract from the outside: every command keyword,
//! the exact `ScriptCommand` shapes, the verbatim `type` rule and the 1-based line
//! numbers carried by each `ScriptError`.
//!
//! Plain `#[test]`s only — parsing is synchronous, does no I/O and needs no
//! display, GPU or network.

use std::path::PathBuf;

use adesk_core::Button;
use adesk_proto::KeySpec;
use adesk_viewer::{parse_script, ScriptCommand, ScriptError};
use adesk_viewer_proto::{ControlOwner, KeyAction};

/// A script using all ten command keywords parses to the exact command vector.
#[test]
fn parses_a_full_script() {
    let source = "\
move 0.25 0.5
click
down middle
up side
scroll -1 2.5
key ctrl c
type hello world
control ai
wait 250
capture /tmp/frame-1.png
";
    assert_eq!(
        parse_script(source).unwrap(),
        vec![
            ScriptCommand::Move { x: 0.25, y: 0.5 },
            ScriptCommand::Click {
                button: Button::Left
            },
            ScriptCommand::Down {
                button: Button::Middle
            },
            ScriptCommand::Up {
                button: Button::Side
            },
            ScriptCommand::Scroll { dx: -1.0, dy: 2.5 },
            ScriptCommand::Key {
                keys: KeySpec::Chord(vec!["ctrl".to_owned(), "c".to_owned()]),
                action: KeyAction::Tap,
            },
            ScriptCommand::Text {
                text: "hello world".to_owned()
            },
            ScriptCommand::Control {
                owner: ControlOwner::Ai
            },
            ScriptCommand::Wait { ms: 250 },
            ScriptCommand::Capture {
                path: PathBuf::from("/tmp/frame-1.png")
            },
        ]
    );
}

/// Comment lines (including indented ones) and blank lines produce no command.
#[test]
fn skips_comments_and_blank_lines() {
    let source = "\n   \n# a whole-line comment\n   # an indented comment\n\t\nmove 0 1\n# trailing comment\n";
    assert_eq!(
        parse_script(source).unwrap(),
        vec![ScriptCommand::Move { x: 0.0, y: 1.0 }]
    );
}

/// An empty or whitespace-only source parses to an empty command list.
#[test]
fn empty_and_whitespace_only_sources_parse_to_nothing() {
    assert!(parse_script("").unwrap().is_empty());
    assert!(parse_script("   \n\t\n  \n").unwrap().is_empty());
    assert!(parse_script("\n\n\n").unwrap().is_empty());
}

/// `click` without a button defaults to the left button.
#[test]
fn click_defaults_to_the_left_button() {
    assert_eq!(
        parse_script("click").unwrap(),
        vec![ScriptCommand::Click {
            button: Button::Left
        }]
    );
    assert_eq!(
        parse_script("click right").unwrap(),
        vec![ScriptCommand::Click {
            button: Button::Right
        }]
    );
}

/// Every AGP button wire name parses for `click`, `down` and `up`.
#[test]
fn every_button_name_parses() {
    for (name, button) in [
        ("left", Button::Left),
        ("right", Button::Right),
        ("middle", Button::Middle),
        ("side", Button::Side),
        ("extra", Button::Extra),
    ] {
        assert_eq!(
            parse_script(&format!("click {name}")).unwrap(),
            vec![ScriptCommand::Click { button }]
        );
        assert_eq!(
            parse_script(&format!("down {name}")).unwrap(),
            vec![ScriptCommand::Down { button }]
        );
        assert_eq!(
            parse_script(&format!("up {name}")).unwrap(),
            vec![ScriptCommand::Up { button }]
        );
    }
}

/// One `key` token is a single key; two or more form a chord in press order.
#[test]
fn key_single_and_chords() {
    assert_eq!(
        parse_script("key a").unwrap(),
        vec![ScriptCommand::Key {
            keys: KeySpec::Single("a".to_owned()),
            action: KeyAction::Tap,
        }]
    );
    assert_eq!(
        parse_script("key ctrl c").unwrap(),
        vec![ScriptCommand::Key {
            keys: KeySpec::Chord(vec!["ctrl".to_owned(), "c".to_owned()]),
            action: KeyAction::Tap,
        }]
    );
    assert_eq!(
        parse_script("key ctrl alt delete").unwrap(),
        vec![ScriptCommand::Key {
            keys: KeySpec::Chord(vec![
                "ctrl".to_owned(),
                "alt".to_owned(),
                "delete".to_owned(),
            ]),
            action: KeyAction::Tap,
        }]
    );
}

/// A bare `key` needs at least one key name.
#[test]
fn key_without_arguments_is_missing_args() {
    assert!(matches!(
        parse_script("key"),
        Err(ScriptError::MissingArgs {
            command: "key",
            line: 1
        })
    ));
}

/// `type` takes everything after the keyword verbatim, preserving inner spaces.
#[test]
fn type_takes_the_rest_of_the_line_verbatim() {
    assert_eq!(
        parse_script("type hello world").unwrap(),
        vec![ScriptCommand::Text {
            text: "hello world".to_owned()
        }]
    );
    // Internal whitespace runs survive as written.
    assert_eq!(
        parse_script("type  two   spaces").unwrap(),
        vec![ScriptCommand::Text {
            text: "two   spaces".to_owned()
        }]
    );
    // A bare `type` with no text types the empty string.
    assert_eq!(
        parse_script("type").unwrap(),
        vec![ScriptCommand::Text {
            text: String::new()
        }]
    );
    // `#` inside the payload is literal text, never a comment.
    assert_eq!(
        parse_script("type a # b").unwrap(),
        vec![ScriptCommand::Text {
            text: "a # b".to_owned()
        }]
    );
    assert_eq!(
        parse_script("type #leading").unwrap(),
        vec![ScriptCommand::Text {
            text: "#leading".to_owned()
        }]
    );
}

/// `control` accepts exactly the lowercase owners `ai` and `human`.
#[test]
fn control_owners() {
    assert_eq!(
        parse_script("control ai").unwrap(),
        vec![ScriptCommand::Control {
            owner: ControlOwner::Ai
        }]
    );
    assert_eq!(
        parse_script("control human").unwrap(),
        vec![ScriptCommand::Control {
            owner: ControlOwner::Human
        }]
    );
}

/// An unknown owner is a `BadOwner` carrying the token and its line.
#[test]
fn control_with_a_bad_owner_errors() {
    assert!(matches!(
        parse_script("control bogus"),
        Err(ScriptError::BadOwner { value, line: 1 }) if value == "bogus"
    ));
}

/// An unknown keyword errors with the offending token and its 1-based line.
#[test]
fn unknown_command_reports_the_line() {
    let source = "# leading comment\nmove 0 0\n\nfrobnicate 1\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::UnknownCommand { command, line }) if command == "frobnicate" && line == 4
    ));
}

/// Too few arguments error with the command keyword and its 1-based line.
#[test]
fn missing_args_reports_the_line() {
    let source = "# leading comment\n\nmove 1\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::MissingArgs {
            command: "move",
            line: 3
        })
    ));
    assert!(matches!(
        parse_script("down"),
        Err(ScriptError::MissingArgs {
            command: "down",
            line: 1
        })
    ));
}

/// Extra arguments error with the command keyword and its 1-based line.
#[test]
fn unexpected_args_reports_the_line() {
    let source = "\nmove 0 0\nmove 1 2 3\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::UnexpectedArgs {
            command: "move",
            line: 3
        })
    ));
    assert!(matches!(
        parse_script("click left right"),
        Err(ScriptError::UnexpectedArgs {
            command: "click",
            line: 1
        })
    ));
}

/// An unparsable number errors with the token and its 1-based line.
#[test]
fn bad_number_reports_the_value_and_line() {
    let source = "# leading comment\nmove x 1\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::BadNumber { value, line }) if value == "x" && line == 2
    ));

    let source = "# leading comment\n\nwait abc\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::BadNumber { value, line }) if value == "abc" && line == 3
    ));
}

/// An unparsable button error carries the offending token and its 1-based line.
#[test]
fn bad_button_reports_the_value_and_line() {
    assert!(matches!(
        parse_script("click middle-left"),
        Err(ScriptError::BadButton { value, line }) if value == "middle-left" && line == 1
    ));

    let source = "move 0 0\n# comment\ndown sideways\n";
    assert!(matches!(
        parse_script(source),
        Err(ScriptError::BadButton { value, line }) if value == "sideways" && line == 3
    ));
}
