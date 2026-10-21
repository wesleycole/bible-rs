//! Sermon transcripts on this device. The transcript is made in the cloud from the uploaded audio
//! (Convex's transcription workflow, docs/plans/m1b-sermons-on-convex.md) and arrives through the
//! change feed as the sermon item's `text` file, a [`records::Transcript`]; the sync engine keeps a
//! copy beside the recording as `transcript.json`, which is what is read here. Files written by
//! earlier builds, which transcribed on the device's request, are read too.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::books::{BookId, ChapterIdx};
use crate::recording::{self, Details, Manifest, Source};
use crate::records;

pub use crate::records::Segment;

/// A recording's transcript: the library's [`records::Transcript`], with which recording it is and
/// when it was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub recording_id: String,
    /// Unix seconds, when the sermon was recorded.
    pub recorded_at: i64,
    pub text: records::Transcript,
    /// What the recording is, from its manifest rather than the transcript file.
    pub about: About,
}

/// What a transcribed sermon is: where it came from, what the user said about it, and the passage
/// it dwells on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct About {
    pub source: Option<Source>,
    pub details: Details,
    /// The chapter the sermon names most, at least twice: "Romans 10".
    pub passage: Option<String>,
}

impl Transcript {
    /// The user's title, else the passage it dwells on and the day: "Romans 10 · Oct 19, 2026",
    /// "Sermon · Oct 19, 2026".
    pub fn title(&self) -> String {
        match &self.about.details.title {
            Some(title) => title.clone(),
            None => format!("{} · {}", self.about.passage.as_deref().unwrap_or("Sermon"), self.day()),
        }
    }

    /// "Oct 19, 2026", the day it was recorded.
    pub fn day(&self) -> String {
        day(self.recorded_at)
    }

    /// Fills in [`about`](Self::about) from the recording's manifest.
    pub fn with_manifest(mut self, m: &Manifest) -> Self {
        let passage = main_passage(&self.text.segments);
        self.about = About { source: Some(m.source), details: m.details.clone(), passage };
        self
    }
}

/// "Oct 19, 2026", from Unix seconds.
pub fn day(unix: i64) -> String {
    let (y, m, d) = crate::time::civil_from_days(unix.div_euclid(86_400));
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}, {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
}

/// The chapter a sermon names most often, if it names one at least twice: references as the
/// transcriber writes them ("Romans 10:9", "First John 4"), with the book capitalized so "my job 3
/// times" isn't Job.
pub fn main_passage(segments: &[Segment]) -> Option<String> {
    let words: Vec<&str> = segments.iter().flat_map(|s| s.text.split_whitespace()).collect();
    let bare = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    // Each book's name as words, with "First John" beside "1 John" and "Psalm" beside "Psalms".
    let mut names: Vec<(Vec<String>, BookId)> = Vec::new();
    for book in BookId::all() {
        let name: Vec<String> = book.name().split(' ').map(str::to_lowercase).collect();
        if let Some(n) = name[0].parse::<usize>().ok().filter(|n| (1..=3).contains(n)) {
            let mut spoken = name.clone();
            spoken[0] = ["first", "second", "third"][n - 1].to_string();
            names.push((spoken, book));
        }
        if book == BookId::PSALMS {
            names.push((vec!["psalm".into()], book));
        }
        names.push((name, book));
    }
    // Longest names first, and a match skips its words: "First John 4" is 1 John, not John.
    names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut counts: Vec<(ChapterIdx, u32, usize)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // A book's name starts with a capital or a digit.
        let named = words[i].starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit());
        let found = names.iter().filter(|_| named).find_map(|(name, book)| {
            let n = name.len();
            if i + n >= words.len() || !(0..n).all(|k| bare(words[i + k]).to_lowercase() == name[k]) {
                return None;
            }
            let digits: String = words[i + n].chars().take_while(char::is_ascii_digit).collect();
            Some((n, digits.parse().ok().and_then(|c| ChapterIdx::new(*book, c))))
        });
        let Some((n, chapter)) = found else {
            i += 1;
            continue;
        };
        if let Some(chapter) = chapter {
            match counts.iter_mut().find(|(c, ..)| *c == chapter) {
                Some((_, count, _)) => *count += 1,
                None => counts.push((chapter, 1, i)),
            }
        }
        i += n + 1;
    }
    // The most named; the first named among equals.
    counts
        .into_iter()
        .filter(|(_, n, _)| *n >= 2)
        .max_by_key(|&(_, n, first)| (n, std::cmp::Reverse(first)))
        .map(|(c, ..)| c.display())
}

/// `<data>/recordings/<id>/transcript.json`.
pub fn path(data_dir: &Path, recording_id: &str) -> PathBuf {
    recording::recordings_dir(data_dir).join(recording_id).join("transcript.json")
}

/// A sermon's transcript, with [`About`] from its recording.
pub fn load(data_dir: &Path, recording_id: &str) -> Option<Transcript> {
    let m = recording::load(data_dir, recording_id)?;
    read(data_dir, &m)
}

fn read(data_dir: &Path, m: &Manifest) -> Option<Transcript> {
    let text = text_in(&recording::recordings_dir(data_dir).join(&m.id))?;
    let t =
        Transcript { recording_id: m.id.clone(), recorded_at: m.started_at, text, about: About::default() };
    Some(t.with_manifest(m))
}

/// The transcript kept beside the recording in `dir`, in the library's shape or an earlier build's.
pub(crate) fn text_in(dir: &Path) -> Option<records::Transcript> {
    let bytes = std::fs::read(dir.join("transcript.json")).ok()?;
    match serde_json::from_slice::<records::Transcript>(&bytes) {
        Ok(text) => Some(text),
        Err(_) => Some(serde_json::from_slice::<Earlier>(&bytes).ok()?.into()),
    }
}

/// Keeps the library's transcript of the recording in `dir` beside it, as `transcript.json`. The
/// sync engine calls it when a sermon's `text` file arrives; bytes that aren't a transcript are
/// refused.
#[cfg(feature = "sync")]
pub(crate) fn keep(dir: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    serde_json::from_slice::<records::Transcript>(bytes)?;
    let tmp = dir.join("transcript.json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join("transcript.json"))?;
    Ok(())
}

/// `transcript.json` as builds before M1b wrote it, when the device asked for the transcript.
#[derive(Deserialize)]
struct Earlier {
    duration_ms: u64,
    language: Option<String>,
    segments: Vec<EarlierSegment>,
}

#[derive(Deserialize)]
struct EarlierSegment {
    start_ms: u64,
    end_ms: u64,
    speaker: Option<String>,
    text: String,
}

impl From<Earlier> for records::Transcript {
    fn from(e: Earlier) -> Self {
        let segments = e
            .segments
            .into_iter()
            .map(|s| Segment { start_ms: s.start_ms, end_ms: s.end_ms, speaker: s.speaker, text: s.text })
            .collect();
        Self { duration_ms: e.duration_ms, language: e.language, segments }
    }
}

/// Every transcribed sermon, newest first.
pub fn all(data_dir: &Path) -> Vec<Transcript> {
    recording::list(data_dir).iter().filter_map(|m| read(data_dir, m)).collect()
}

/// "12:30", "1:02:09": a moment in a sermon.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Reads "12:30", "1:02:09" or "750" (seconds) back into milliseconds.
pub fn parse_clock(s: &str) -> Option<u64> {
    let mut total = 0u64;
    for part in s.trim().split(':') {
        total = total * 60 + part.trim().parse::<u64>().ok()?;
    }
    Some(total * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_carry_the_day() {
        let t = Transcript {
            recording_id: "x".into(),
            recorded_at: 1_792_368_000,
            text: records::Transcript { duration_ms: 0, language: None, segments: vec![] },
            about: About::default(),
        };
        assert_eq!(t.title(), "Sermon · Oct 19, 2026");
    }

    #[test]
    fn the_main_passage_is_the_chapter_named_most() {
        let seg = |text: &str| Segment { start_ms: 0, end_ms: 0, speaker: None, text: text.into() };
        let said = [
            seg("Turn with me to Romans 10. I lost my job 3 times, and John 3 matters too."),
            seg("Paul said in Romans 10, verse 9, confess. First John 4:2 agrees, as 1 John 4 says."),
        ];
        // Romans 10 and 1 John 4 twice each ("First John" is 1 John): the first named wins. "job 3"
        // isn't Job, and John 3 is named once.
        assert_eq!(main_passage(&said).as_deref(), Some("Romans 10"));
        assert_eq!(main_passage(&said[..1]), None, "named once isn't enough");
        let psalm = [seg("Psalm 23 says it. Psalms 23 again, and Psalm 151 doesn't exist, Psalm 151.")];
        assert_eq!(main_passage(&psalm).as_deref(), Some("Psalm 23"));
        let epistle = [seg("First John 4 and 1 John 4:8 say God is love.")];
        assert_eq!(main_passage(&epistle).as_deref(), Some("1 John 4"), "not the Gospel's chapter 4");
    }

    #[test]
    fn clocks_round_trip() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(750_400), "12:30");
        assert_eq!(clock(3_729_000), "1:02:09");
        assert_eq!(parse_clock("12:30"), Some(750_000));
        assert_eq!(parse_clock("1:02:09"), Some(3_729_000));
        assert_eq!(parse_clock("90"), Some(90_000));
        assert_eq!(parse_clock("ten"), None);
    }

    #[test]
    #[cfg(feature = "sync")]
    fn a_kept_transcript_and_an_earlier_one_both_read() {
        let data = std::env::temp_dir().join(format!("linger-transcripts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut w = recording::ChunkWriter::create(&data, Source::Microphone, None).unwrap();
        w.push(&[0; 16_000]).unwrap();
        let dir = w.dir().to_path_buf();
        let m = w.finish().unwrap();

        assert!(keep(&dir, b"{\"not\":\"a transcript\"}").is_err());
        assert_eq!(load(&data, &m.id), None);
        let library = br#"{"durationMs":5000,"language":"en","segments":[{"startMs":0,"endMs":2500,"speaker":"A","text":"Romans 10 and Romans 10."}]}"#;
        keep(&dir, library).unwrap();
        let t = load(&data, &m.id).unwrap();
        assert_eq!((t.recording_id.as_str(), t.recorded_at), (m.id.as_str(), m.started_at));
        assert_eq!(t.text.segments[0].speaker.as_deref(), Some("A"));
        assert_eq!(t.about.passage.as_deref(), Some("Romans 10"));

        let earlier = br#"{"recording_id":"x","recorded_at":1,"duration_ms":4000,"language":null,"segments":[{"start_ms":10,"end_ms":20,"speaker":null,"text":"Grace."}]}"#;
        std::fs::write(dir.join("transcript.json"), earlier).unwrap();
        let t = load(&data, &m.id).unwrap();
        assert_eq!(t.text.duration_ms, 4000);
        assert_eq!(
            t.text.segments,
            [Segment { start_ms: 10, end_ms: 20, speaker: None, text: "Grace.".into() }]
        );
        assert_eq!(all(&data).len(), 1);
    }
}
