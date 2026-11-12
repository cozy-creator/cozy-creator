//! Project real video segment publications onto their planned frame ranges. H3's
//! plan log is authoritative; neither beat indexes nor elapsed fractions are used.
use std::collections::BTreeMap;

use api::calcifer::{Event, Run, Status, VideoProgress, VideoSegmentProgress, VideoSegmentStatus};

const FPS: u32 = 24;

pub(crate) fn of(run: &Run, events: &[Event]) -> Option<VideoProgress> {
    let plan = plan_of(events)?;
    let label = run.output.iter().filter(|item| item.name == "video" && item.index == 0)
        .max_by_key(|item| item.rev).map(|item| item.label.as_str());
    Some(plan.project(label, run.progress.as_ref().map(|progress| progress.stage.as_str()), run.status == Status::InProgress))
}

fn plan_of(events: &[Event]) -> Option<Plan> {
    let text = events.iter().rev().find_map(|event| match event {
        Event::Log { text, .. } if text.starts_with("h3 shot segments ") => Some(text),
        _ => None,
    })?;
    Plan::read(text)
}

struct Plan {
    boundaries: Vec<u64>,
}

impl Plan {
    fn read(text: &str) -> Option<Self> {
        let fields = fields(text.strip_prefix("h3 shot segments ")?)?;
        let count: usize = fields.get("segments")?.parse().ok()?;
        let resumed: u64 = fields.get("resumed_at")?.parse().ok()?;
        let total: u64 = fields.get("delivered_frames")?.parse().ok()?;
        let mut boundaries = vec![resumed];
        let starts = fields.get("boundaries_s")?;
        if !starts.is_empty() {
            for start in starts.split(',') {
                let seconds: f64 = start.parse().ok()?;
                let frame = (seconds * f64::from(FPS)).round();
                if !frame.is_finite() || frame < 0.0 || frame >= u64::MAX as f64 { return None }
                // H3 logs seconds to .01; at 24 fps rounding uniquely recovers
                // the integer frame (the maximum logging error is .12 frames).
                boundaries.push(frame as u64);
            }
        }
        boundaries.push(total);
        if count == 0 || boundaries.len() - 1 != count || boundaries.windows(2).any(|span| span[0] >= span[1]) { return None }
        Some(Self { boundaries })
    }

    fn project(&self, label: Option<&str>, stage: Option<&str>, live: bool) -> VideoProgress {
        let count = self.boundaries.len() - 1;
        let completed = match label {
            Some("Video") => count,
            Some(label) if label.starts_with("Video at ") => count,
            Some(label) => segment(label, count).unwrap_or(0),
            None => 0,
        };
        let active = live.then(|| stage.and_then(|stage| segment(stage.split(" / ").next()?, count))).flatten();
        let segments = self.boundaries.windows(2).enumerate().map(|(index, span)| VideoSegmentProgress {
            from_frame: span[0],
            to_frame: span[1],
            status: if index < completed { VideoSegmentStatus::Completed }
                else if active == Some(index + 1) { VideoSegmentStatus::InProgress }
                else { VideoSegmentStatus::Pending },
        }).collect();
        VideoProgress { fps: FPS, resumed_at: self.boundaries[0], completed_frames: self.boundaries[completed], segments }
    }
}

fn segment(label: &str, count: usize) -> Option<usize> {
    let (number, total) = label.strip_prefix("Segment ")?.split_once(" of ")?;
    let number: usize = number.parse().ok()?;
    let total: usize = total.parse().ok()?;
    (total == count && number > 0 && number <= count).then_some(number)
}

/// `Frame::log` prints a scalar as JSON when quoted. Respect quoted fields so
/// a reference name containing `segments=...` cannot masquerade as plan data.
fn fields(mut text: &str) -> Option<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    while !text.trim_start().is_empty() {
        text = text.trim_start();
        let (key, value) = text.split_once('=')?;
        if key.is_empty() || key.contains(char::is_whitespace) { return None }
        let (value, rest) = if value.starts_with('"') {
            let mut decoded = serde_json::Deserializer::from_str(value).into_iter::<String>();
            let text = decoded.next()?.ok()?;
            (text, &value[decoded.byte_offset()..])
        } else {
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            (value[..end].to_owned(), &value[end..])
        };
        if fields.insert(key.to_owned(), value).is_some() || rest.chars().next().is_some_and(|character| !character.is_whitespace()) { return None }
        text = rest;
    }
    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "h3 shot segments segments=3 seconds=12,14,9 resumed_at=0 boundaries_s=12.25,26.42 delivered_frames=855 clip_windows=\"a name segments=99\"";

    #[test]
    fn completed_segments_are_real_frame_ranges_not_beat_indexes() {
        let plan = Plan::read(PLAN).unwrap();
        let progress = plan.project(Some("Segment 1 of 3"), Some("Segment 2 of 3 / denoise"), true);
        assert_eq!(progress.completed_frames, 294);
        assert_eq!(progress.segments[0].status, VideoSegmentStatus::Completed);
        assert_eq!(progress.segments[1], VideoSegmentProgress { from_frame: 294, to_frame: 634, status: VideoSegmentStatus::InProgress });
        assert_eq!(progress.segments[2].status, VideoSegmentStatus::Pending);
        // Three authored four-second beats fit in the first generated segment.
        assert!([96, 192, 288].iter().all(|end| *end <= progress.completed_frames));
    }

    #[test]
    fn kept_prefix_and_finished_base_video_do_not_wait_for_upscale() {
        let plan = Plan::read("h3 shot segments segments=1 resumed_at=362 boundaries_s=\"\" delivered_frames=702").unwrap();
        let initial = plan.project(None, Some("Segment 1 of 1 / condition"), true);
        assert_eq!((initial.resumed_at, initial.completed_frames), (362, 362));
        let base = plan.project(Some("Video"), Some("upscale / refine"), true);
        assert_eq!(base.completed_frames, 702);
        assert!(base.segments.iter().all(|segment| segment.status == VideoSegmentStatus::Completed));
    }

    #[test]
    fn malformed_or_missing_evidence_never_invents_completion() {
        assert!(Plan::read("h3 shot segments segments=2 resumed_at=0 boundaries_s=\"\" delivered_frames=702").is_none());
        assert!(Plan::read("h3 shot segments segments=2 resumed_at=0 boundaries_s=NaN delivered_frames=702").is_none());
        let plan = Plan::read(PLAN).unwrap();
        assert_eq!(plan.project(Some("Segment 2 of 9"), None, true).completed_frames, 0);
        assert!(plan.project(None, Some("Segment 1 of 3 / denoise"), false).segments.iter().all(|segment| segment.status == VideoSegmentStatus::Pending));
        let events: Vec<_> = [PLAN, "h3 shot segments segments=2"].into_iter().enumerate().map(|(index, text)| Event::Log {
            sequence_number: index as u64 + 1, at_ms: 0, level: "info".into(), text: text.into(),
        }).collect();
        assert!(plan_of(&events).is_none(), "a malformed new plan cannot reuse an older plan");
    }
}
