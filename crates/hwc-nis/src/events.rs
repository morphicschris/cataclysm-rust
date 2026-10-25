//! The `.script` event list of an NIS (`nisScriptTable` 0x87eb3c and its `*Set` parsers).
//!
//! The script is read line by line like `scriptSet`: the first word picks a parser by exact,
//! case-sensitive name (unknown words such as `motionCueScale`, `minimumLODSet`, `stretch` or
//! the lower-case `widescreenOut` and `closeslipgate` some scripts use are ignored, as in the
//! original); `at` sets the time of the events that follow and `with` the object they apply to.
//! Each parser appends an event stamped with that time, object and a running counter, and the
//! list is sorted by time and then by the counter.

use hwc_formats::nis::{NisScript, ScriptTime};

use crate::data::{NisData, FRAME_TO_SECONDS};

/// What an event does (the original's `nisopcode` in comments).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `deathDamage` (0), `deathProjectile` (1), `deathBeam` (2): the object dies.
    Death { kind: i32 },
    /// `attack` (4): attack another NIS object.
    Attack { target: usize },
    /// `repair` (5, Cataclysm): repair another NIS object.
    Repair { target: usize },
    /// `haltAttack` (6).
    HaltAttack,
    /// `fire` (7): the ship's special-fire function.
    Fire,
    /// `show` (8).
    Show,
    /// `hide` (9).
    Hide,
    /// `invincible` (10), `vincible` (11).
    Invincible,
    Vincible,
    /// `soundEvent` (12).
    SoundEvent { event: i32 },
    /// `speechEvent` (13): `actor` is -1 without an `ACTOR n` clause.
    SpeechEvent { event: i32, variable: i32, actor: i32 },
    /// `remainAtEnd` (19): the object stays in the universe after the NIS, optionally joining
    /// a KAS team.
    RemainAtEnd { team: Option<String> },
    /// `fleetSpeechEvent` (20).
    FleetSpeechEvent { event: i32, variable: i32 },
    /// `cameraFOV` (21) in degrees.
    CameraFov(f32),
    /// `cameraClipNear` / `cameraClipFar` (Cataclysm).
    CameraClipNear(f32),
    CameraClipFar(f32),
    /// `wideScreenOut` (29): the letterbox goes out over this many seconds.
    WideScreenOut(f32),
    /// `startMusic` (32), `stopMusic` (33, fade seconds).
    StartMusic(i32),
    StopMusic(f32),
    /// `fadeSet` (34): fade level 0..1 at once; `white` fades to white instead of black.
    FadeSet { level: f32, white: bool },
    /// `fadeTo` (35): towards `level` over `seconds`.
    FadeTo { level: f32, seconds: f32 },
    /// `colorScheme` (36).
    ColorScheme(i32),
    /// `meshAnimStart` (37): a `.mad` animation by name.
    MeshAnimStart(String),
    /// `musicVolume` (42), `speechVolume` (43), `SFXVolume` (44).
    MusicVolume(f32),
    SpeechVolume(f32),
    SfxVolume(f32),
    /// `hideUniverseToggle` (46): hide or unhide everything that is not part of the NIS.
    HideUniverseToggle,
    /// `pauseUniverseToggle` (45).
    PauseUniverseToggle,
    /// `keepMovingAtEnd`.
    KeepMovingAtEnd,
    /// `health` (64): health as a fraction of the maximum.
    DamageLevel(f32),
    /// `lightAmbient` / `lightDiffuse` (66, 67, Cataclysm): colour 0..255 of light 0 or 1
    /// (chosen by which light-source object the block is for).
    LightAmbient { light: i32, rgb: [i32; 3] },
    LightDiffuse { light: i32, rgb: [i32; 3] },
    /// `globalAmbient` (68), `numberLights` (69).
    GlobalAmbient([i32; 3]),
    NumberLights(i32),
    /// `subtitle` (74, Cataclysm): a location card from the `strNIS..` strings for `ms`.
    Subtitle { index: i32, ms: i32 },
    /// Cataclysm effects the presentation layer handles: `fog`, `ebolts`, `beastEffect`,
    /// `mimic`, `openSlipgate`, `closeSlipgate`; the argument text as written.
    Fog(String),
    EBolts(String),
    BeastEffect(String),
    Mimic(String),
    OpenSlipgate,
    CloseSlipgate,
}

impl Action {
    /// Whether the event needs the object of its `with` block.
    pub fn needs_object(&self) -> bool {
        !matches!(
            self,
            Action::CameraFov(_)
                | Action::CameraClipNear(_)
                | Action::CameraClipFar(_)
                | Action::WideScreenOut(_)
                | Action::StartMusic(_)
                | Action::StopMusic(_)
                | Action::FadeSet { .. }
                | Action::FadeTo { .. }
                | Action::MusicVolume(_)
                | Action::SpeechVolume(_)
                | Action::SfxVolume(_)
                | Action::HideUniverseToggle
                | Action::PauseUniverseToggle
                | Action::GlobalAmbient(_)
                | Action::NumberLights(_)
                | Action::Subtitle { .. }
                | Action::FleetSpeechEvent { .. }
                | Action::Fog(_)
        )
    }
}

/// One event (`nisevent`).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Seconds from the start.
    pub time: f32,
    /// The object of the enclosing `with` block (`shipID`).
    pub object: Option<usize>,
    /// Running counter in script order (`initialID`), the tie-break of the sort.
    pub order: u32,
    pub action: Action,
}

/// `nisCurrentTimeSet`: `FRAME n` (scaled by the `float` 1/30) or seconds.
pub fn script_seconds(t: ScriptTime) -> f32 {
    match t {
        ScriptTime::Frame(f) => f * FRAME_TO_SECONDS,
        ScriptTime::Seconds(s) => s,
    }
}

/// `sscanf("%f")`: the leading number of `s`, 0 when there is none.
fn scan_f32(s: &str) -> Option<f32> {
    let s = s.trim_start();
    let mut end = 0;
    let b = s.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_e = false;
    while end < b.len() {
        let c = b[end];
        match c {
            b'+' | b'-' if end == 0 || matches!(b[end - 1], b'e' | b'E') => {}
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot && !seen_e => seen_dot = true,
            b'e' | b'E' if seen_digit && !seen_e => seen_e = true,
            _ => break,
        }
        end += 1;
    }
    let mut t = &s[..end];
    while !t.is_empty() && t.parse::<f32>().is_err() {
        t = &t[..t.len() - 1];
    }
    t.parse().ok()
}

/// `sscanf("%d")`.
fn scan_i32(s: &str) -> Option<i32> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
        end += 1;
    }
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    s[..end].parse().ok()
}

/// `sscanf("%f FRAME")` / `sscanf("%f")` of a duration: frames when the text has `FRAME`.
fn duration(s: &str) -> f32 {
    let v = scan_f32(s).unwrap_or(0.0);
    if s.to_ascii_uppercase().contains("FRAME") {
        v * FRAME_TO_SECONDS
    } else {
        v
    }
}

/// `evalEvaluate`: the integer/real expression the speech parsers allow, e.g.
/// `(343 + 2048 + 536870912)`.
pub fn evaluate(expr: &str) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'+') => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    Some(b'-') => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.atom()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'*') => {
                        self.i += 1;
                        v *= self.atom()?;
                    }
                    Some(b'/') => {
                        self.i += 1;
                        v /= self.atom()?;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn atom(&mut self) -> Option<f64> {
            self.ws();
            match self.s.get(self.i)? {
                b'(' => {
                    self.i += 1;
                    let v = self.expr()?;
                    self.ws();
                    if self.s.get(self.i) == Some(&b')') {
                        self.i += 1;
                    }
                    Some(v)
                }
                b'-' => {
                    self.i += 1;
                    Some(-self.atom()?)
                }
                _ => {
                    let start = self.i;
                    while self.i < self.s.len()
                        && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.')
                    {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
                }
            }
        }
    }
    let mut p = P {
        s: expr.as_bytes(),
        i: 0,
    };
    p.expr()
}

/// The speech parsers' split: the expression (up to the bracket that closes it, or a
/// separator outside brackets), then `%d` after one more character.
fn speech_args(args: &str) -> (i32, i32) {
    let mut depth = 0;
    let mut end = args.len();
    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            ',' | ' ' | '\t' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let value = evaluate(&args[..end]).map_or(0, |v| v.trunc() as i32);
    let rest = &args[end..];
    let variable = rest
        .get(1..)
        .and_then(scan_i32)
        .unwrap_or(0);
    (value, variable)
}

fn rgb(args: &str) -> [i32; 3] {
    let mut it = args.split([',', ' ', '\t']).filter(|s| !s.is_empty());
    std::array::from_fn(|_| it.next().and_then(scan_i32).unwrap_or(0))
}

/// Builds the event list of `data` from `script` (`nisLoad`'s `scriptSet` pass).
pub fn compile(data: &mut NisData, script: &NisScript) {
    let mut out: Vec<Event> = Vec::new();
    let mut counter = 0u32;
    for e in &script.events {
        let time = script_seconds(e.time);
        let object = e.object.as_deref().and_then(|o| data.find_object(o));
        let args = e.args.as_str();
        let light_of = |object: Option<usize>| {
            object
                .and_then(|i| data.objects.get(i))
                .map_or(0, |o| match o.type_name.to_ascii_lowercase().as_str() {
                    "nislightsource2" => 1,
                    _ => 0,
                })
        };
        let action = match e.keyword.as_str() {
            "deathDamage" => Some(Action::Death { kind: 0 }),
            "deathProjectile" => Some(Action::Death { kind: 1 }),
            "deathBeam" => Some(Action::Death { kind: 2 }),
            "attack" => match data.find_object(args) {
                Some(target) => Some(Action::Attack { target }),
                None => {
                    data.ignored.push((e.line, format!("attack: no object {args:?}")));
                    None
                }
            },
            "repair" => data.find_object(args).map(|target| Action::Repair { target }),
            "haltAttack" => Some(Action::HaltAttack),
            "fire" => Some(Action::Fire),
            "show" => Some(Action::Show),
            "hide" => Some(Action::Hide),
            "invincible" => Some(Action::Invincible),
            "vincible" => Some(Action::Vincible),
            "soundEvent" => Some(Action::SoundEvent {
                event: speech_args(args).0,
            }),
            "speechEvent" => {
                let (event, variable) = speech_args(args);
                let up = args.to_ascii_uppercase();
                let actor = match up.find("ACTOR") {
                    Some(i) => up.get(i + 6..).and_then(scan_i32).unwrap_or(-1),
                    None => -1,
                };
                Some(Action::SpeechEvent {
                    event,
                    variable,
                    actor,
                })
            }
            "fleetSpeechEvent" => {
                let (event, variable) = speech_args(args);
                Some(Action::FleetSpeechEvent { event, variable })
            }
            "remainAtEnd" => {
                let team = args.split_whitespace().next().map(str::to_string);
                Some(Action::RemainAtEnd { team })
            }
            "cameraFOV" => Some(Action::CameraFov(scan_f32(args).unwrap_or(0.0))),
            "cameraClipNear" => Some(Action::CameraClipNear(scan_f32(args).unwrap_or(0.0))),
            "cameraClipFar" => Some(Action::CameraClipFar(scan_f32(args).unwrap_or(0.0))),
            "wideScreenIn" => {
                data.scissor_fade_in = Some(duration(args));
                None
            }
            "wideScreenOut" => Some(Action::WideScreenOut(duration(args))),
            "startMusic" => Some(Action::StartMusic(scan_i32(args).unwrap_or(0))),
            "stopMusic" => Some(Action::StopMusic(scan_f32(args).unwrap_or(0.0))),
            "fadeSet" => {
                let mut level = scan_f32(args).unwrap_or(0.0);
                if args.contains('%') {
                    level *= 0.01;
                }
                let white = args
                    .find(',')
                    .and_then(|i| scan_i32(&args[i + 1..]))
                    .unwrap_or(0);
                Some(Action::FadeSet {
                    level,
                    white: white as u8 != 0,
                })
            }
            "fadeTo" => {
                let (level_text, time_text) = args.split_once(',').unwrap_or((args, ""));
                let mut level = scan_f32(level_text).unwrap_or(0.0);
                if level_text.contains('%') {
                    level *= 0.01;
                }
                Some(Action::FadeTo {
                    level,
                    seconds: duration(time_text),
                })
            }
            "colorScheme" => Some(Action::ColorScheme(scan_i32(args).unwrap_or(0))),
            "meshAnimStart" => Some(Action::MeshAnimStart(args.trim().to_string())),
            "musicVolume" => Some(Action::MusicVolume(scan_f32(args).unwrap_or(0.0))),
            "speechVolume" => Some(Action::SpeechVolume(scan_f32(args).unwrap_or(0.0))),
            "SFXVolume" => Some(Action::SfxVolume(scan_f32(args).unwrap_or(0.0))),
            "hideUniverseToggle" => Some(Action::HideUniverseToggle),
            "pauseUniverseToggle" => Some(Action::PauseUniverseToggle),
            "keepMovingAtEnd" => Some(Action::KeepMovingAtEnd),
            "health" => {
                let v = scan_f32(args).unwrap_or(0.0);
                // The parser looks for the character 0x2525, which never matches: a `%` is
                // not honoured.
                Some(Action::DamageLevel(v))
            }
            "lightAmbient" => Some(Action::LightAmbient {
                light: light_of(object),
                rgb: rgb(args),
            }),
            "lightDiffuse" => Some(Action::LightDiffuse {
                light: light_of(object),
                rgb: rgb(args),
            }),
            "globalAmbient" => Some(Action::GlobalAmbient(rgb(args))),
            "numberLights" => Some(Action::NumberLights(scan_i32(args).unwrap_or(0))),
            "subtitle" => {
                let (index, ms) = speech_args(args);
                Some(Action::Subtitle { index, ms })
            }
            "midPointLookAt" => {
                data.looky_object = object;
                None
            }
            "fog" => Some(Action::Fog(args.to_string())),
            "ebolts" => Some(Action::EBolts(args.to_string())),
            "beastEffect" => Some(Action::BeastEffect(args.to_string())),
            "mimic" => Some(Action::Mimic(args.to_string())),
            "openSlipgate" => Some(Action::OpenSlipgate),
            "closeSlipgate" => Some(Action::CloseSlipgate),
            other => {
                data.ignored.push((e.line, other.to_string()));
                None
            }
        };
        let Some(action) = action else { continue };
        counter += 1;
        if action.needs_object() && object.is_none() {
            data.ignored
                .push((e.line, format!("{}: no object {:?}", e.keyword, e.object)));
            continue;
        }
        out.push(Event {
            time,
            object,
            order: counter,
            action,
        });
    }
    out.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.order.cmp(&b.order))
    });
    data.events = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_expressions() {
        assert_eq!(speech_args("(343 + 2048 + 536870912)"), (536873303, 0));
        assert_eq!(speech_args("( 702 + 4096 + 536870912 )"), (536875710, 0));
        assert_eq!(speech_args("0 4000"), (0, 4000));
        assert_eq!(speech_args("2 4000"), (2, 4000));
    }

    #[test]
    fn numbers() {
        assert_eq!(scan_f32("100%, 0"), Some(100.0));
        assert_eq!(scan_f32(" 0.5"), Some(0.5));
        assert_eq!(duration(" 60 FRAMES"), 60.0 * FRAME_TO_SECONDS);
        assert_eq!(rgb("40,40,40"), [40, 40, 40]);
        assert_eq!(rgb("255,255,255"), [255, 255, 255]);
    }
}
