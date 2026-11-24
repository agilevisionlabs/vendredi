use super::*;
use egui::{Color32, TextFormat, text::LayoutJob};
use std::ops::Range;
use vendredi_core::engine::TrafficSearch;

pub(super) const HIGHLIGHT: Color32 = Color32::from_rgb(255, 220, 115);

impl Desktop {
    pub(super) fn edit_search(&mut self, ctx: &egui::Context) {
        if self.search.is_empty() {
            self.clear_search();
        } else {
            let delay = std::time::Duration::from_millis(100);
            self.search_due = Some(std::time::Instant::now() + delay);
            ctx.request_repaint_after(delay);
        }
    }

    pub(super) fn dispatch_search(&mut self, ctx: &egui::Context) {
        if let Some(due) = self.search_due {
            if let Some(remaining) = due.checked_duration_since(std::time::Instant::now()) {
                ctx.request_repaint_after(remaining);
            } else {
                self.search_due = None;
                self.command(Command::Traffic(self.search.clone()));
            }
        }
    }

    pub(super) fn apply_search(&mut self, query: &str, rows: Vec<TrafficRow>) {
        self.matches = rows.into_iter().map(|r| r.id).collect();
        if self
            .highlight
            .as_ref()
            .is_some_and(|(text, _)| text == query)
        {
            return;
        }
        match TrafficSearch::new(query) {
            Ok(pattern) => {
                self.highlight = Some((query.to_owned(), pattern));
                self.reveal_match = true;
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(super) fn clear_search(&mut self) {
        self.search.clear();
        self.search_due = None;
        self.matches.clear();
        self.highlight = None;
    }

    pub(super) fn hits(&self, text: &str) -> Vec<Range<usize>> {
        self.highlight
            .iter()
            .flat_map(|(_, q)| q.ranges(text))
            .collect()
    }
}

// Offsets refer to the whole displayed field or raw message. A single occurrence
// may cross a syntax-color boundary or continue onto the next EDIFACT segment.
pub(super) fn append(
    job: &mut LayoutJob,
    text: &str,
    start: usize,
    hits: &[Range<usize>],
    format: TextFormat,
) {
    let mut pos = 0;
    let first = hits.partition_point(|r| r.end <= start);
    for hit in hits[first..]
        .iter()
        .take_while(|r| r.start < start + text.len())
    {
        let left = hit.start.saturating_sub(start);
        let right = (hit.end - start).min(text.len());
        if pos < left {
            job.append(&text[pos..left], 0., format.clone());
        }
        job.append(
            &text[left..right],
            0.,
            TextFormat {
                color: TEXT,
                background: HIGHLIGHT,
                ..format.clone()
            },
        );
        pos = right;
    }
    if pos < text.len() {
        job.append(&text[pos..], 0., format);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_spans_syntax_and_segment_boundaries() -> Result<()> {
        let message = "É+'BGM+é+'BGM+";
        let hits = TrafficSearch::new("é+'bgm")?
            .ranges(message)
            .collect::<Vec<_>>();
        let mut job = LayoutJob::default();
        for (start, end) in [(0, 4), (4, 7), (7, 12), (12, message.len())] {
            append(
                &mut job,
                &message[start..end],
                start,
                &hits,
                TextFormat::default(),
            );
        }
        assert_eq!(job.text, message);
        let highlighted: String = job
            .sections
            .iter()
            .filter(|s| s.format.background != Color32::TRANSPARENT)
            .map(|s| &job.text[s.byte_range.clone()])
            .collect();
        assert_eq!(highlighted, "É+'BGMé+'BGM");
        Ok(())
    }
}
