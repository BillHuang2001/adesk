//! Input-script parser for the headless `adesk-viewer` binary.
//!
//! The binary's `--input <FILE>` / `--input-stdin` modes feed a small
//! line-oriented script to [`parse_script`]; every accepted line becomes one
//! [`ScriptCommand`] the CLI turns into VAP input messages (`docs/viewer.md` §4).
//!
//! Grammar — one command per line, leading/trailing whitespace trimmed, tokens
//! whitespace-separated; blank lines and lines whose first non-blank character
//! is `#` are ignored:
//!
//! ```text
//! move X Y          # normalized output position (0.0..=1.0)
//! click [BUTTON]    # left button when BUTTON is omitted
//! down BUTTON
//! up BUTTON
//! scroll DX DY
//! key KEYS...       # one token is a key, two or more are a chord
//! type TEXT         # the rest of the line, taken verbatim
//! control ai|human
//! wait MS
//! capture FILE      # FILE is a single token (paths may not contain spaces)
//! ```
//!
//! `BUTTON` is an AGP button wire name (`left|right|middle|side|extra`,
//! `adesk_core::Button`); `control` accepts the lowercase owners `ai` and
//! `human`. Parsing never panics: every malformed line yields a [`ScriptError`]
//! tagged with the 1-based line number it occurred on.

use std::path::PathBuf;

use adesk_core::Button;
use adesk_proto::KeySpec;
use adesk_viewer_proto::{ControlOwner, KeyAction};

/// One command of an input script.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptCommand {
    /// Move the pointer to the normalized output position `(x, y)`.
    Move {
        /// Normalized horizontal position (`0.0..=1.0`).
        x: f64,
        /// Normalized vertical position (`0.0..=1.0`).
        y: f64,
    },
    /// Press and release `button` at the current pointer position.
    Click {
        /// Button to click; [`Button::Left`] when the script omitted it.
        button: Button,
    },
    /// Press `button` (no release).
    Down {
        /// Button to press.
        button: Button,
    },
    /// Release `button`.
    Up {
        /// Button to release.
        button: Button,
    },
    /// Scroll by `(dx, dy)` at the current pointer position.
    Scroll {
        /// Horizontal scroll delta.
        dx: f64,
        /// Vertical scroll delta.
        dy: f64,
    },
    /// Tap a key or chord (press then release).
    Key {
        /// The key (one token) or chord (several tokens), in press order.
        keys: KeySpec,
        /// Always [`KeyAction::Tap`]; the script has no separate press/release
        /// syntax — use `key` followed by the key names.
        action: KeyAction,
    },
    /// Type `text` verbatim (including internal spaces).
    Text {
        /// The characters to type.
        text: String,
    },
    /// Announce who owns viewer input.
    Control {
        /// The announced owner.
        owner: ControlOwner,
    },
    /// Wait `ms` milliseconds before the next command.
    Wait {
        /// Delay in milliseconds.
        ms: u64,
    },
    /// Capture the next frame to `path`.
    Capture {
        /// Destination file path (a single token).
        path: PathBuf,
    },
}

/// A parse failure, tagged with the 1-based line it occurred on.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    /// The line starts with a token that is not a known command.
    #[error("line {line}: unknown command `{command}`")]
    UnknownCommand {
        /// The unrecognized first token.
        command: String,
        /// 1-based line number of the offending line.
        line: usize,
    },
    /// A command was given fewer arguments than it needs.
    #[error("line {line}: `{command}` is missing arguments")]
    MissingArgs {
        /// The command keyword.
        command: &'static str,
        /// 1-based line number of the offending line.
        line: usize,
    },
    /// A fixed-arity command was given more arguments than it accepts.
    #[error("line {line}: `{command}` has unexpected extra arguments")]
    UnexpectedArgs {
        /// The command keyword.
        command: &'static str,
        /// 1-based line number of the offending line.
        line: usize,
    },
    /// A numeric argument (`X`, `Y`, `DX`, `DY` or `MS`) could not be parsed.
    #[error("line {line}: `{value}` is not a valid number")]
    BadNumber {
        /// The offending token.
        value: String,
        /// 1-based line number of the offending line.
        line: usize,
    },
    /// A button name is not an AGP button wire name.
    #[error("line {line}: `{value}` is not a button (left|right|middle|side|extra)")]
    BadButton {
        /// The offending token.
        value: String,
        /// 1-based line number of the offending line.
        line: usize,
    },
    /// A control owner is neither `ai` nor `human`.
    #[error("line {line}: `{value}` is not a control owner (ai|human)")]
    BadOwner {
        /// The offending token.
        value: String,
        /// 1-based line number of the offending line.
        line: usize,
    },
}

/// Parses a whole input script into its commands.
///
/// Lines are numbered from 1 and counted physically, so a skipped comment or
/// blank line still advances the number reported in an error. Blank lines and
/// comment lines produce no command. Parsing stops at the first error.
///
/// # Errors
///
/// Returns the [`ScriptError`] for the first malformed line, with its 1-based
/// line number.
pub fn parse_script(source: &str) -> Result<Vec<ScriptCommand>, ScriptError> {
    let mut commands = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if let Some(command) = parse_line(line, index + 1)? {
            commands.push(command);
        }
    }
    Ok(commands)
}

/// Parses one line; `Ok(None)` for a blank or comment line.
fn parse_line(line: &str, line_no: usize) -> Result<Option<ScriptCommand>, ScriptError> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }

    let mut tokens = trimmed.split_whitespace();
    let Some(command) = tokens.next() else {
        return Ok(None);
    };
    let args: Vec<&str> = tokens.collect();

    let command = match command {
        "move" => {
            check_arity(&args, "move", line_no, 2)?;
            ScriptCommand::Move {
                x: parse_number(args[0], line_no)?,
                y: parse_number(args[1], line_no)?,
            }
        }
        "click" => ScriptCommand::Click {
            button: match args.len() {
                0 => Button::Left,
                1 => parse_button(args[0], line_no)?,
                _ => {
                    return Err(ScriptError::UnexpectedArgs {
                        command: "click",
                        line: line_no,
                    })
                }
            },
        },
        "down" => {
            check_arity(&args, "down", line_no, 1)?;
            ScriptCommand::Down {
                button: parse_button(args[0], line_no)?,
            }
        }
        "up" => {
            check_arity(&args, "up", line_no, 1)?;
            ScriptCommand::Up {
                button: parse_button(args[0], line_no)?,
            }
        }
        "scroll" => {
            check_arity(&args, "scroll", line_no, 2)?;
            ScriptCommand::Scroll {
                dx: parse_number(args[0], line_no)?,
                dy: parse_number(args[1], line_no)?,
            }
        }
        "key" => {
            if args.is_empty() {
                return Err(ScriptError::MissingArgs {
                    command: "key",
                    line: line_no,
                });
            }
            let keys = if args.len() == 1 {
                KeySpec::Single(args[0].to_owned())
            } else {
                KeySpec::Chord(args.iter().map(|key| (*key).to_owned()).collect())
            };
            ScriptCommand::Key {
                keys,
                action: KeyAction::Tap,
            }
        }
        // `type` takes the rest of the line verbatim: everything after the
        // whitespace run separating it from the `type` token.
        "type" => ScriptCommand::Text {
            text: trimmed
                .strip_prefix("type")
                .unwrap_or_default()
                .trim_start()
                .to_owned(),
        },
        "control" => {
            check_arity(&args, "control", line_no, 1)?;
            ScriptCommand::Control {
                owner: match args[0] {
                    "ai" => ControlOwner::Ai,
                    "human" => ControlOwner::Human,
                    other => {
                        return Err(ScriptError::BadOwner {
                            value: other.to_owned(),
                            line: line_no,
                        })
                    }
                },
            }
        }
        "wait" => {
            check_arity(&args, "wait", line_no, 1)?;
            ScriptCommand::Wait {
                ms: args[0].parse::<u64>().map_err(|_| ScriptError::BadNumber {
                    value: args[0].to_owned(),
                    line: line_no,
                })?,
            }
        }
        "capture" => {
            check_arity(&args, "capture", line_no, 1)?;
            ScriptCommand::Capture {
                path: PathBuf::from(args[0]),
            }
        }
        other => {
            return Err(ScriptError::UnknownCommand {
                command: other.to_owned(),
                line: line_no,
            })
        }
    };

    Ok(Some(command))
}

/// Rejects an argument list whose length is not exactly `expected`.
fn check_arity(
    args: &[&str],
    command: &'static str,
    line: usize,
    expected: usize,
) -> Result<(), ScriptError> {
    match args.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(ScriptError::MissingArgs { command, line }),
        std::cmp::Ordering::Greater => Err(ScriptError::UnexpectedArgs { command, line }),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

/// Parses a free-form numeric argument.
fn parse_number(value: &str, line: usize) -> Result<f64, ScriptError> {
    value.parse::<f64>().map_err(|_| ScriptError::BadNumber {
        value: value.to_owned(),
        line,
    })
}

/// Parses an AGP button wire name.
fn parse_button(value: &str, line: usize) -> Result<Button, ScriptError> {
    match value {
        "left" => Ok(Button::Left),
        "right" => Ok(Button::Right),
        "middle" => Ok(Button::Middle),
        "side" => Ok(Button::Side),
        "extra" => Ok(Button::Extra),
        other => Err(ScriptError::BadButton {
            value: other.to_owned(),
            line,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let source = "\
move 0.25 0.5
click
click right
down middle
up side
scroll -1 2.5
key a
key CTRL L
type hello world
control human
wait 250
capture /tmp/out.png
";
        assert_eq!(
            parse_script(source).unwrap(),
            vec![
                ScriptCommand::Move { x: 0.25, y: 0.5 },
                ScriptCommand::Click {
                    button: Button::Left
                },
                ScriptCommand::Click {
                    button: Button::Right
                },
                ScriptCommand::Down {
                    button: Button::Middle
                },
                ScriptCommand::Up {
                    button: Button::Side
                },
                ScriptCommand::Scroll { dx: -1.0, dy: 2.5 },
                ScriptCommand::Key {
                    keys: KeySpec::Single("a".to_owned()),
                    action: KeyAction::Tap,
                },
                ScriptCommand::Key {
                    keys: KeySpec::Chord(vec!["CTRL".to_owned(), "L".to_owned()]),
                    action: KeyAction::Tap,
                },
                ScriptCommand::Text {
                    text: "hello world".to_owned()
                },
                ScriptCommand::Control {
                    owner: ControlOwner::Human
                },
                ScriptCommand::Wait { ms: 250 },
                ScriptCommand::Capture {
                    path: PathBuf::from("/tmp/out.png")
                },
            ]
        );
    }

    #[test]
    fn skips_blanks_and_comments() {
        let source = "\n   \n# a comment\n   # indented comment\n\t\nmove 0 1\n# trailing\n";
        assert_eq!(
            parse_script(source).unwrap(),
            vec![ScriptCommand::Move { x: 0.0, y: 1.0 }]
        );
        assert!(parse_script("").unwrap().is_empty());
    }

    #[test]
    fn type_takes_the_rest_verbatim() {
        assert_eq!(
            parse_script("type  two   spaces  ").unwrap(),
            vec![ScriptCommand::Text {
                text: "two   spaces".to_owned()
            }]
        );
        assert_eq!(
            parse_script("type").unwrap(),
            vec![ScriptCommand::Text {
                text: String::new()
            }]
        );
        // `type` keeps `#` and later command keywords as literal text.
        assert_eq!(
            parse_script("type # not a comment").unwrap(),
            vec![ScriptCommand::Text {
                text: "# not a comment".to_owned()
            }]
        );
    }

    #[test]
    fn button_and_owner_names() {
        assert_eq!(
            parse_script("control ai").unwrap(),
            vec![ScriptCommand::Control {
                owner: ControlOwner::Ai
            }]
        );
        for (name, button) in [
            ("left", Button::Left),
            ("right", Button::Right),
            ("middle", Button::Middle),
            ("side", Button::Side),
            ("extra", Button::Extra),
        ] {
            assert_eq!(
                parse_script(&format!("down {name}")).unwrap(),
                vec![ScriptCommand::Down { button }]
            );
        }
    }

    #[test]
    fn reports_unknown_command_with_line() {
        assert_eq!(
            parse_script("move 0 0\n# comment\nfrobnicate 1\n"),
            Err(ScriptError::UnknownCommand {
                command: "frobnicate".to_owned(),
                line: 3,
            })
        );
    }

    #[test]
    fn reports_bad_numbers_and_buttons() {
        assert_eq!(
            parse_script("scroll 1 nope"),
            Err(ScriptError::BadNumber {
                value: "nope".to_owned(),
                line: 1,
            })
        );
        assert_eq!(
            parse_script("wait -5"),
            Err(ScriptError::BadNumber {
                value: "-5".to_owned(),
                line: 1,
            })
        );
        assert_eq!(
            parse_script("click upside"),
            Err(ScriptError::BadButton {
                value: "upside".to_owned(),
                line: 1,
            })
        );
        assert_eq!(
            parse_script("control Human"),
            Err(ScriptError::BadOwner {
                value: "Human".to_owned(),
                line: 1,
            })
        );
    }

    #[test]
    fn reports_missing_and_unexpected_args() {
        assert_eq!(
            parse_script("move 0"),
            Err(ScriptError::MissingArgs {
                command: "move",
                line: 1,
            })
        );
        assert_eq!(
            parse_script("key"),
            Err(ScriptError::MissingArgs {
                command: "key",
                line: 1,
            })
        );
        assert_eq!(
            parse_script("click left right"),
            Err(ScriptError::UnexpectedArgs {
                command: "click",
                line: 1,
            })
        );
        assert_eq!(
            parse_script("capture a.png b.png"),
            Err(ScriptError::UnexpectedArgs {
                command: "capture",
                line: 1,
            })
        );
    }
}
