//! The JavaScript editor: highlighting, line numbers and a syntax check
//! that runs as you pause typing and marks the line it points at.
use super::*;
use egui::{TextFormat, Ui, text::LayoutJob};

const KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];
/// Seconds of quiet typing before the syntax check runs.
const PAUSE: f64 = 0.4;

/// The last syntax check and what it found.
#[derive(Default)]
pub(super) struct Check {
    checked: Option<(String, String)>,
    changed_at: f64,
    /// The message and the 1-based line and column it points at.
    pub(super) problem: Option<(String, Option<(usize, usize)>)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Keyword,
    Text,
    Number,
    Comment,
}

/// Splits JavaScript into coloured runs. It only needs to be right enough to
/// read: an unterminated string or comment runs to the end.
fn tokens(source: &str) -> Vec<(std::ops::Range<usize>, Kind)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let push =
        |out: &mut Vec<(std::ops::Range<usize>, Kind)>, r: std::ops::Range<usize>, k| match out
            .last_mut()
        {
            Some((last, kind)) if *kind == k && last.end == r.start => last.end = r.end,
            _ => out.push((r, k)),
        };
    while i < b.len() {
        let start = i;
        let kind = match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = source[i..].find('\n').map_or(b.len(), |n| i + n);
                Kind::Comment
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = source[i + 2..].find("*/").map_or(b.len(), |n| i + n + 4);
                Kind::Comment
            }
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                Kind::Text
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                    i += 1;
                }
                Kind::Number
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$')
                {
                    i += 1;
                }
                if KEYWORDS.contains(&&source[start..i]) {
                    Kind::Keyword
                } else {
                    Kind::Plain
                }
            }
            _ => {
                // Step a whole character, so multi-byte text stays intact.
                i += source[i..].chars().next().map_or(1, char::len_utf8);
                Kind::Plain
            }
        };
        push(&mut out, start..i, kind);
    }
    out
}

fn layout(source: &str, error_line: Option<usize>) -> LayoutJob {
    let mut job = LayoutJob::default();
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let marked = error_line.and_then(|l| {
        let start = *line_starts.get(l - 1)?;
        let end = line_starts.get(l).copied().unwrap_or(source.len());
        Some(start..end)
    });
    for (range, kind) in tokens(source) {
        let color = match kind {
            Kind::Plain => col::text(),
            Kind::Keyword => col::blue(),
            Kind::Text => col::green(),
            Kind::Number => col::amber(),
            Kind::Comment => col::muted(),
        };
        // Split runs at the marked line, so only it gets the background.
        let mut cuts = vec![range.start, range.end];
        if let Some(m) = &marked {
            cuts.extend([m.start, m.end].into_iter().filter(|c| range.contains(c)));
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let mut format = TextFormat::simple(ty::code(), color);
            if marked.as_ref().is_some_and(|m| m.contains(&w[0])) {
                format.background = col::amber_bg();
            }
            job.append(&source[w[0]..w[1]], 0., format);
        }
    }
    job
}

/// The editor, with line numbers, and the result of the last check below.
pub(super) fn js_editor(ui: &mut Ui, source: &mut String, function: &str, check: &mut Check) {
    let now = ui.input(|i| i.time);
    let current = (source.clone(), function.to_owned());
    if check.checked.as_ref() != Some(&current) {
        if check.changed_at == 0. {
            check.changed_at = now;
        }
        if now - check.changed_at >= PAUSE {
            check.problem = vendredi_core::script::validate(source, function)
                .err()
                .map(|e| {
                    let text = e.to_string();
                    let at = vendredi_core::script::error_position(&text);
                    (text, at)
                });
            check.checked = Some(current);
            check.changed_at = 0.;
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(PAUSE));
        }
    }
    let error_line = check
        .problem
        .as_ref()
        .and_then(|(_, at)| at.map(|(l, _)| l));
    let lines = source.lines().count().max(1) + usize::from(source.ends_with('\n'));
    ui.horizontal_top(|ui| {
        let numbers: String = (1..=lines.max(10)).map(|n| format!("{n}\n")).collect();
        ui.add_space(GAP_XS);
        ui.add(
            egui::Label::new(
                egui::RichText::new(numbers.trim_end())
                    .font(ty::code())
                    .color(col::muted()),
            )
            .selectable(false),
        );
        let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| {
            let mut job = layout(text.as_str(), error_line);
            job.wrap.max_width = width;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        ui.add(
            egui::TextEdit::multiline(source)
                .code_editor()
                .margin(egui::Margin::symmetric(10, 0))
                .desired_rows(10)
                .desired_width(f32::INFINITY)
                .layouter(&mut layouter),
        );
    });
    match &check.problem {
        Some((text, Some((line, column)))) => {
            ui.colored_label(
                col::amber(),
                format!("Line {line}, column {column}: {text}"),
            );
        }
        Some((text, None)) => {
            ui.colored_label(col::amber(), text);
        }
        None if check.checked.is_some() => {
            ui.label(muted(format!(
                "No syntax problems. {function}() is defined."
            )));
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_waits_for_a_pause_and_points_at_the_line() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut source = "function reply(i, api) {\n  let x = ;\n}".to_owned();
        let mut check = Check::default();
        for time in [0., 0.1, 0.6] {
            let _ = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        js_editor(ui, &mut source, "reply", &mut check);
                    });
                },
            );
            if time < 0.5 {
                assert!(check.problem.is_none(), "checked before the pause");
            }
        }
        let (text, at) = check.problem.expect("a syntax problem");
        assert!(text.contains("SyntaxError"), "{text}");
        assert_eq!(at, Some((2, 11)));
    }

    #[test]
    fn javascript_is_split_into_readable_runs() {
        let source = "// note\nconst n = 42; api.log(\"hé\" + n);";
        let kinds: Vec<_> = tokens(source)
            .into_iter()
            .map(|(r, k)| (&source[r], k))
            .filter(|(_, k)| *k != Kind::Plain)
            .collect();
        assert!(kinds.contains(&("// note", Kind::Comment)));
        assert!(kinds.contains(&("const", Kind::Keyword)));
        assert!(kinds.contains(&("42", Kind::Number)));
        assert!(kinds.contains(&("\"hé\"", Kind::Text)));
        // Every byte is covered once, in order.
        let mut at = 0;
        for (r, _) in tokens(source) {
            assert_eq!(r.start, at);
            at = r.end;
        }
        assert_eq!(at, source.len());
        assert!(tokens("'unterminated").last().is_some());
    }
}
