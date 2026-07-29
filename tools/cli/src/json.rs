//! Minimal JSON emission for the `--json` output mode.
//!
//! The output surface is one flat document per invocation; hand-rolled
//! emission is a few dozen lines and keeps the binary dependency-free.

/// A JSON value. Object fields keep insertion order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Number(i64),
    Bool(bool),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Render the value as compact JSON.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::String(text) => {
                out.push('"');
                escape_into(text, out);
                out.push('"');
            }
            Self::Number(number) => out.push_str(&number.to_string()),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    escape_into(key, out);
                    out.push_str("\":");
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn escape_into(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                use core::fmt::Write as _;

                let code = control as u32;
                let _ = write!(out, "\\u{code:04x}");
            }
            other => out.push(other),
        }
    }
}

/// Build the standard result document every subcommand emits in `--json`
/// mode. `schema_version` is always the first field.
#[must_use]
pub fn result_envelope(
    command: &str,
    status: &str,
    exit_code: i64,
    duration_ms: i64,
    stdout: &str,
    stderr: &str,
) -> Value {
    Value::Object(vec![
        ("schema_version".to_string(), Value::Number(1)),
        ("command".to_string(), Value::String(command.to_string())),
        ("status".to_string(), Value::String(status.to_string())),
        ("exit_code".to_string(), Value::Number(exit_code)),
        ("duration_ms".to_string(), Value::Number(duration_ms)),
        ("stdout".to_string(), Value::String(stdout.to_string())),
        ("stderr".to_string(), Value::String(stderr.to_string())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_quotes_backslashes_and_control_characters() {
        let value = Value::String("a\"b\\c\nd\re\tf\u{1}g".to_string());
        assert_eq!(value.render(), "\"a\\\"b\\\\c\\nd\\re\\tf\\u0001g\"");
    }

    #[test]
    fn non_ascii_text_passes_through_unescaped() {
        let value = Value::String("çağdaş ✔".to_string());
        assert_eq!(value.render(), "\"çağdaş ✔\"");
    }

    #[test]
    fn objects_preserve_field_order() {
        let value = Value::Object(vec![
            ("b".to_string(), Value::Number(2)),
            ("a".to_string(), Value::Number(1)),
        ]);
        assert_eq!(value.render(), "{\"b\":2,\"a\":1}");
    }

    #[test]
    fn arrays_and_booleans_render_compactly() {
        let value = Value::Array(vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Number(-3),
        ]);
        assert_eq!(value.render(), "[true,false,-3]");
    }

    #[test]
    fn envelope_leads_with_schema_version_one() {
        let document = result_envelope("test", "ok", 0, 42, "out", "err").render();
        assert!(
            document.starts_with("{\"schema_version\":1,\"command\":\"test\""),
            "unexpected prefix: {document}"
        );
        assert!(document.contains("\"status\":\"ok\""));
        assert!(document.contains("\"exit_code\":0"));
        assert!(document.contains("\"duration_ms\":42"));
    }
}
