//! A trace of the fleet-intel presentation: each mission speech with its subtitle length, the
//! letterbox subtitle region's state, whether the intel speech is playing, the end of each
//! intel event with its reason, and the mission's wide-screen calls, all at game time. Kept
//! in memory (bounded) for tests; echoed to stderr when `HWC_INTEL_TRACE` is set, to follow a
//! long letterbox hold in a real game.

use hwc_sim::mission::{IntelEndReason, WideScreenKind};

/// At most this many entries are kept (the oldest go first).
const KEEP: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntelTraceKind {
    /// A mission speech started with subtitles of `seconds` in all (`fragments` pieces).
    Speech {
        event: u32,
        seconds: f32,
        fragments: usize,
    },
    /// The mission's location card (`LocationCard`), shown for `seconds`.
    LocationCard { seconds: f32 },
    /// A mission speech that gave no subtitle (no audio, a placeholder, too quiet).
    Unsubtitled { event: u32 },
    /// The subtitle system reported the event over (`subMessageEnded`): a region's last
    /// card expired, or the player skipped.
    SubtitlesEnded { skipped: bool },
    /// The mission's `IntelEventEnded` answered yes.
    IntelEnded(IntelEndReason),
    /// A wide-screen call applied to the letterbox.
    WideScreen {
        kind: WideScreenKind,
        frames: i32,
        sensors_open: bool,
    },
    /// The letterbox subtitle region changed.
    Region {
        cards: usize,
        aborted: bool,
        continuous: bool,
    },
    /// The intel speech stream started or stopped sounding.
    SpeechPlaying(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntelTraceEntry {
    pub time: f32,
    pub kind: IntelTraceKind,
}

#[derive(Debug, Clone, Default)]
pub struct IntelTrace {
    pub entries: Vec<IntelTraceEntry>,
    echo: bool,
    region: Option<(usize, bool, bool)>,
    playing: Option<bool>,
    /// The subtitle system's end report was seen last frame (it repeats every frame while
    /// the region stays empty; only its start is traced).
    ended: bool,
}

impl IntelTrace {
    pub fn new() -> IntelTrace {
        IntelTrace {
            echo: std::env::var_os("HWC_INTEL_TRACE").is_some(),
            ..IntelTrace::default()
        }
    }

    pub fn push(&mut self, time: f32, kind: IntelTraceKind) {
        if self.echo {
            eprintln!("intel {time:8.2}s {kind:?}");
        }
        if self.entries.len() >= KEEP {
            self.entries.drain(..KEEP / 4);
        }
        self.entries.push(IntelTraceEntry { time, kind });
    }

    /// The letterbox region's state this frame; traced when it changes.
    pub fn region(&mut self, time: f32, cards: usize, aborted: bool, continuous: bool) {
        let now = (cards, aborted, continuous);
        if self.region != Some(now) {
            self.region = Some(now);
            self.push(
                time,
                IntelTraceKind::Region {
                    cards,
                    aborted,
                    continuous,
                },
            );
        }
    }

    /// Whether the intel speech sounds this frame; traced when it changes.
    pub fn playing(&mut self, time: f32, playing: bool) {
        if self.playing != Some(playing) {
            self.playing = Some(playing);
            self.push(time, IntelTraceKind::SpeechPlaying(playing));
        }
    }

    /// The subtitle system's report this frame (0 none, 1 ended, 2 skipped).
    pub fn message_ended(&mut self, time: f32, report: u8) {
        if report != 0 && !self.ended {
            self.push(
                time,
                IntelTraceKind::SubtitlesEnded {
                    skipped: report == 2,
                },
            );
        }
        self.ended = report != 0;
    }
}
