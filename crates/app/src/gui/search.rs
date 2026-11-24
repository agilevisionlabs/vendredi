use super::*;
use egui::{Color32, TextFormat, text::LayoutJob};
use std::ops::Range;
use vendredi_core::engine::TrafficSearch;

pub(super) const HIGHLIGHT: Color32 = Color32::from_rgb(255, 220, 115);

impl Desktop {
    pub(super) fn update_search(&mut self) {
        self.matches.clear();
        self.highlight = None;
        self.reveal_match = true;
        if !self.search.is_empty() {
            match TrafficSearch::new(&self.search) {
                Ok(pattern) => {
                    self.highlight = Some(pattern);
                    self.command(Command::Traffic(self.search.clone()));
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }

    pub(super) fn clear_search(&mut self) {
        self.search.clear();
        self.update_search();
    }

    pub(super) fn hits(&self, text: &str) -> Vec<Range<usize>> {
        self.highlight.iter().flat_map(|q| q.ranges(text)).collect()
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
