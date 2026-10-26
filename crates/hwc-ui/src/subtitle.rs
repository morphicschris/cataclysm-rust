//! Subtitles (`Subtitle.c`): the text that goes with speech, simulated subtitles and location
//! cards, laid out in the regions and themes of `Subtitle.script`.
//!
//! A speech fragment arrives with its actor (from the speech event's actor flags, see
//! [`actor_for_speech`]); the actor picks the default theme (colour, font, fades and the speaker
//! portrait from `SpeechIcons\R1|R2`), the text goes to the letterbox region at the top of the
//! screen unless control codes at its start say otherwise (`#r<n>` region, `#t<n>` theme, `#c`
//! close captions, `#+` the event continues). Fragments of one actor and event that arrive
//! together are joined with a space. The text is word-wrapped to the region and split into
//! cards: in English whole pages that follow each other over the speech's length, in other
//! languages lines that scroll up after a dwell. Each card fades in and out (its colour is
//! scaled towards black; the text stays opaque).
//!
//! While anything is shown in the letterbox region outside a letterbox, a translucent band is
//! drawn across the top of the screen behind it, with the speaker portrait at the region's icon
//! offset. When the last letterbox card expires (not skipped) the "intel event ended" flag the
//! mission script waits on is raised.

use std::rc::Rc;

use hwc_formats::font::Font;
use hwc_formats::lif::Lif;

use crate::canvas::{BlitMode, Canvas, Image, Rect};
use crate::text::{self, Shadow};
use crate::Resources;

pub const NUMBER_THEMES: usize = 16;
pub const NUMBER_REGIONS: usize = 4;

/// Region indices (`STR_*`).
pub mod region {
    /// Text of a playing NIS (timed by the NIS clock).
    pub const NIS: usize = 0;
    /// Over the letterbox bar at the top of the screen (speech).
    pub const LETTERBOX: usize = 1;
    /// Close captions near the bottom.
    pub const CLOSE_CAPTIONED: usize = 2;
    /// Location cards.
    pub const LOCATION_CARD: usize = 3;
}

/// Theme of close captions (`#c`).
pub const THEME_CLOSE_CAPTIONED: usize = 14;
/// Theme of location cards.
pub const THEME_LOCATION_CARD: usize = 15;
/// Actor of location cards (no theme of its own: the card's control codes pick one).
pub const ACTOR_LOCATION_CARD: i32 = 0x70;
/// Actor whose theme has no picture.
pub const ACTOR_NO_PICTURE: i32 = 9;
/// Speaker portraits are this size.
pub const PICTURE_SIZE: i32 = 64;
/// Extra pixels between lines.
const INTERLINE: i32 = 1;
/// More fragments than this in one frame are dropped.
const MAX_NEW: usize = 7;
/// Grey level and alpha of the band behind the letterbox region (a float constant of 40).
pub const BAND_LEVEL: u8 = 40;
/// How fast a skipped fleet intel event fades (`smSkipFadeoutTime`).
pub const SKIP_FADE: f32 = 0.5;

/// Speech event actor flags.
pub mod actor_flag {
    pub const FLEET_COMMAND: u32 = 0x1000;
    pub const TACTICAL_OFFICER: u32 = 0x2000;
    pub const BEAST: u32 = 0x4000;
    pub const MISSION_VOICE: u32 = 0x8000;
    pub const CAALSHTO: u32 = 0x1_0000;
    pub const BENTUSI: u32 = 0x2_0000;
    pub const PIRATES: u32 = 0x4_0000;
    pub const CLEESAN: u32 = 0x8_0000;
    pub const GENERIC: u32 = 0x20_0000;
    /// Forces the actor without a picture.
    pub const NO_PICTURE: u32 = 0x4000_0000;
}

/// The actor (and so the default theme) of a speech fragment (`ssSubtitleRead`). `stream_actor`
/// is the actor the stream was queued with (0 for single-player voices, negative for none);
/// a non-zero one is used as is. For the per-mission voice flag the speaker depends on the
/// mission: Bushan-Re (2), Kiith Nabaal (1), Kiith Manaan (16), Faal-Corum (11, 13, 14),
/// otherwise the generic voice; outside single player always the generic voice.
pub fn actor_for_speech(
    stream_actor: i32,
    event: u32,
    single_player: bool,
    mission: u32,
) -> Option<i32> {
    use actor_flag::*;
    if stream_actor < 0 {
        return None;
    }
    if stream_actor != 0 {
        return Some(stream_actor);
    }
    let mut actor = if event & FLEET_COMMAND != 0 {
        0
    } else if event & TACTICAL_OFFICER != 0 {
        1
    } else if event & BEAST != 0 {
        11
    } else if event & BENTUSI != 0 {
        10
    } else if event & CAALSHTO != 0 {
        8
    } else if event & PIRATES != 0 {
        5
    } else if event & CLEESAN != 0 {
        6
    } else if event & GENERIC != 0 {
        12
    } else if event & MISSION_VOICE != 0 {
        if !single_player {
            12
        } else {
            match mission {
                2 => 2,
                1 => 4,
                16 => 7,
                11 | 13 | 14 => 3,
                _ => 12,
            }
        }
    } else {
        9
    };
    if event & NO_PICTURE != 0 {
        actor = 9;
    }
    Some(actor)
}

/// A text theme.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Theme {
    pub color: [u8; 3],
    /// Portrait file name (without folder or extension) and the colour scheme it takes.
    pub picture: Option<(String, usize)>,
    pub drop_shadow: bool,
    pub centred: bool,
    pub margin: i32,
    pub font: Option<String>,
    pub fade_in: f32,
    pub fade_out: f32,
}

/// A region as the script defines it, in 640x480 coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RegionDef {
    pub bounds: Rect,
    pub scale_x: bool,
    pub scale_y: bool,
    pub max_lines: usize,
    pub icon_offset: (i32, i32),
}

impl RegionDef {
    pub fn defined(&self) -> bool {
        self.bounds.x1 != 0
    }

    /// `subRegionsRescale`: scaled with the screen or kept centred.
    pub fn rect_for(&self, width: i32, height: i32) -> Rect {
        let b = self.bounds;
        let (dx, dy) = ((width - 640) / 2, (height - 480) / 2);
        let (x0, x1) = if self.scale_x {
            (b.x0 * width / 640, b.x1 * width / 640)
        } else {
            (b.x0 + dx, b.x1 + dx)
        };
        let (y0, y1) = if self.scale_y {
            (b.y0 * height / 480, b.y1 * height / 480)
        } else {
            (b.y0 + dy, b.y1 + dy)
        };
        Rect::new(x0, y0, x1, y1)
    }
}

/// `Subtitle.script`.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub themes: Vec<Theme>,
    pub regions: [RegionDef; NUMBER_REGIONS],
    pub scroll_dwell_start: f32,
    pub scroll_dwell_end: f32,
    pub scroll_shortest: f32,
    pub title_shortest: f32,
}

impl Default for Script {
    fn default() -> Self {
        Script {
            themes: vec![Theme::default(); NUMBER_THEMES],
            regions: [RegionDef::default(); NUMBER_REGIONS],
            scroll_dwell_start: 0.7,
            scroll_dwell_end: 0.5,
            scroll_shortest: 0.25,
            title_shortest: 1.2,
        }
    }
}

/// `scriptStringToBool`: only TRUE and YES (any case) are true.
fn script_bool(s: &str) -> bool {
    s.eq_ignore_ascii_case("TRUE") || s.eq_ignore_ascii_case("YES")
}

impl Script {
    /// Parses the script. Keys are matched exactly, as the engine does, so the shipped
    /// `subTitleShortest` line (the table says `subtitleShortest`) leaves the default.
    pub fn parse(text: &str) -> Script {
        let mut s = Script::default();
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            let line = line.replace(',', " ");
            let mut f = line.split_whitespace();
            let Some(key) = f.next() else { continue };
            let rest: Vec<&str> = f.collect();
            let int = |i: usize| rest.get(i).and_then(|v| v.parse::<i32>().ok());
            let real = |i: usize| rest.get(i).and_then(|v| v.parse::<f32>().ok());
            let theme = int(0)
                .filter(|&t| (0..NUMBER_THEMES as i32).contains(&t))
                .map(|t| t as usize);
            let reg = int(0)
                .filter(|&t| (0..NUMBER_REGIONS as i32).contains(&t))
                .map(|t| t as usize);
            match key {
                "themeColor" => {
                    if let (Some(t), Some(r), Some(g), Some(b)) = (theme, int(1), int(2), int(3)) {
                        s.themes[t].color = [r as u8, g as u8, b as u8];
                    }
                }
                "themeDropshadow" => {
                    if let (Some(t), Some(v)) = (theme, rest.get(1)) {
                        s.themes[t].drop_shadow = script_bool(v);
                    }
                }
                "themeCentred" => {
                    if let (Some(t), Some(v)) = (theme, rest.get(1)) {
                        s.themes[t].centred = script_bool(v);
                    }
                }
                "themeMargin" => {
                    if let (Some(t), Some(m)) = (theme, int(1)) {
                        s.themes[t].margin = m;
                    }
                }
                "themeFont" => {
                    if let (Some(t), Some(v)) = (theme, rest.get(1)) {
                        s.themes[t].font = Some(v.to_string());
                    }
                }
                "themeFadeIn" => {
                    if let (Some(t), Some(v)) = (theme, real(1)) {
                        s.themes[t].fade_in = v;
                    }
                }
                "themeFadeOut" => {
                    if let (Some(t), Some(v)) = (theme, real(1)) {
                        s.themes[t].fade_out = v;
                    }
                }
                "themePicture" => {
                    if let (Some(t), Some(c), Some(p)) = (theme, int(1), rest.get(2)) {
                        s.themes[t].picture = Some((p.to_string(), c.max(0) as usize));
                    }
                }
                "regionBounds" => {
                    if let (Some(r), Some(a), Some(b), Some(c), Some(d)) =
                        (reg, int(1), int(2), int(3), int(4))
                    {
                        s.regions[r].bounds = Rect::new(a, b, c, d);
                    }
                }
                "regionScaleRez" => {
                    if let (Some(r), Some(x), Some(y)) = (reg, rest.get(1), rest.get(2)) {
                        s.regions[r].scale_x = script_bool(x);
                        s.regions[r].scale_y = script_bool(y);
                    }
                }
                "regionMaxLines" => {
                    if let (Some(r), Some(n)) = (reg, int(1)) {
                        s.regions[r].max_lines = n.max(0) as usize;
                    }
                }
                "regionIconOffset" => {
                    if let (Some(r), Some(x), Some(y)) = (reg, int(1), int(2)) {
                        s.regions[r].icon_offset = (x, y);
                    }
                }
                "scrollDwellStart" => {
                    s.scroll_dwell_start = real(0).unwrap_or(s.scroll_dwell_start)
                }
                "scrollDwellEnd" => s.scroll_dwell_end = real(0).unwrap_or(s.scroll_dwell_end),
                "scrollShortest" => s.scroll_shortest = real(0).unwrap_or(s.scroll_shortest),
                "subtitleShortest" => s.title_shortest = real(0).unwrap_or(s.title_shortest),
                _ => {}
            }
        }
        s
    }
}

/// `subStringsChop`: breaks `text` into lines no wider than the region (a line is cut once it
/// reaches `width - 1` pixels, at the last blank before that point, or mid-word if there is
/// none). `#n`, `\n` (two characters), newline and carriage return end lines.
pub fn chop(text: &[u8], width: i32, measure: impl Fn(&[u8]) -> i32) -> Vec<Vec<u8>> {
    let n = text.len();
    let blank = |c: u8| c == b' ' || c == b'\t';
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = start;
        let (line_end, next) = loop {
            let c = text.get(end).copied().unwrap_or(0);
            if (c == b'#' || c == b'\\') && text.get(end + 1) == Some(&b'n') {
                let mut k = end + 2;
                while k < n && b" \t\n\r".contains(&text[k]) {
                    k += 1;
                }
                break (end, k);
            }
            if c == b'\n' || c == b'\r' {
                let mut k = end + 1;
                while k < n && b" \t\r".contains(&text[k]) {
                    k += 1;
                }
                break (end, k);
            }
            if end >= n {
                break (end, end);
            }
            if measure(&text[start..=end]) >= width - 1 {
                let mut in_blank = false;
                let mut after = end;
                let mut found = None;
                let mut p = end;
                while p > start {
                    if !in_blank {
                        if blank(text[p]) {
                            in_blank = true;
                            after = p + 1;
                        }
                    } else if !blank(text[p]) {
                        found = Some(p + 1);
                        break;
                    }
                    p -= 1;
                }
                break match found {
                    Some(e) => (e, after),
                    // No blank to break at: cut before the character that did not fit (at
                    // least one character, so a glyph wider than the region still advances).
                    None if end > start => (end, end),
                    None => (end + 1, end + 1),
                };
            }
            end += 1;
        };
        out.push(text[start..line_end].to_vec());
        start = next;
    }
    out
}

/// One line or page of text in a region.
#[derive(Debug, Clone)]
pub struct Card {
    /// When it appears (game or NIS time).
    pub created: f32,
    /// How long it stays, fades included.
    pub duration: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub color: [u8; 3],
    pub drop_shadow: bool,
    pub theme: usize,
    /// Offset in the region.
    pub x: i32,
    pub y: i32,
    pub text: Vec<u8>,
}

/// A region at the current screen size with its cards.
#[derive(Debug, Clone, Default)]
pub struct Region {
    pub def: RegionDef,
    pub rect: Rect,
    pub enabled: bool,
    pub scroll_start: f32,
    pub scroll_end: f32,
    pub scroll_distance: f32,
    /// Theme whose portrait is shown.
    pub picture: Option<usize>,
    /// Fading out because it was skipped.
    pub aborted: bool,
    /// The event goes on after this fragment.
    pub continuous: bool,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone)]
struct NewSubtitle {
    actor: i32,
    event: u32,
    text: Vec<u8>,
    seconds: f32,
}

/// Whether the band and the letterbox interact this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrawState {
    /// `nisScissorFade` is non-zero (the letterbox is moving).
    pub letterbox_fading: bool,
    /// `nisFullyScissored`.
    pub letterboxed: bool,
    /// A movie is playing (`binkPlay` turns the game's rendering off while it runs, and the
    /// band is drawn only while rendering is on): the text and portraits go over the movie
    /// without the band.
    pub movie: bool,
}

/// What the subtitle pass tells the main overlay (`moSetTop` from `subTitlesDraw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HudTop {
    /// Some region was drawn this frame: the overlay's slide back up waits.
    pub drawn: bool,
    /// The bottom of the speaker portrait drawn (the last one, if several regions show one):
    /// the top-left and top-right HUD text moves below it.
    pub portrait_bottom: Option<i32>,
}

/// The subtitle system.
pub struct Subtitles {
    pub script: Script,
    pub regions: Vec<Region>,
    fonts: Vec<Option<Rc<Font>>>,
    pictures: Vec<Option<Rc<Image>>>,
    pending: Vec<NewSubtitle>,
    /// Fragments dropped because too many arrived at once.
    pub missed: usize,
    /// `strCurLanguage`: 0 English (paged), others scroll.
    pub language: usize,
    /// `subMessageEnded`: 1 when the last letterbox card expired, 2 when skipped.
    message_ended: u8,
    /// Counter for events of simulated subtitles and location cards.
    dummy_event: u32,
    size: (i32, i32),
    /// What the last [`Subtitles::draw`] reported for the overlay.
    hud_top: HudTop,
}

impl Subtitles {
    /// `subStartup` with the script text. Fonts and portraits come from `res` (portraits in
    /// `SpeechIcons\R1` or `R2` by the local player's race), coloured with the team colour
    /// scheme each theme names (`schemes[i]` = (base, stripe)).
    pub fn new(
        script: Script,
        res: &Resources,
        race_one: bool,
        schemes: &[([u8; 3], [u8; 3])],
    ) -> Subtitles {
        let fonts = script
            .themes
            .iter()
            .map(|t| t.font.as_deref().and_then(|f| res.font(f)))
            .collect();
        let folder = if race_one { "R1" } else { "R2" };
        let pictures = script
            .themes
            .iter()
            .map(|t| {
                let (name, scheme) = t.picture.as_ref()?;
                let data = res.read(&format!("SpeechIcons\\{folder}\\{name}.lif"))?;
                let lif = Lif::parse(&data).ok()?;
                let (base, stripe) = schemes
                    .get(*scheme)
                    .copied()
                    .unwrap_or(([255; 3], [255; 3]));
                Some(Rc::new(Image::from_rgba(
                    lif.width as i32,
                    lif.height as i32,
                    &lif.to_rgba_team(base, stripe),
                )))
            })
            .collect();
        let regions = script
            .regions
            .iter()
            .map(|d| Region {
                def: *d,
                rect: d.rect_for(640, 480),
                enabled: d.defined(),
                picture: None,
                ..Region::default()
            })
            .collect();
        Subtitles {
            script,
            regions,
            fonts,
            pictures,
            pending: Vec::new(),
            missed: 0,
            language: 0,
            message_ended: 0,
            dummy_event: 0,
            size: (640, 480),
            hud_top: HudTop::default(),
        }
    }

    /// Loads `Subtitle.script` through `res`.
    pub fn load(
        res: &Resources,
        race_one: bool,
        schemes: &[([u8; 3], [u8; 3])],
    ) -> Option<Subtitles> {
        let data = res.read("Subtitle.script")?;
        let text: String = data.iter().map(|&b| b as char).collect();
        Some(Subtitles::new(Script::parse(&text), res, race_one, schemes))
    }

    /// The font of a theme.
    pub fn font(&self, theme: usize) -> Option<&Rc<Font>> {
        self.fonts.get(theme)?.as_ref()
    }

    /// The portrait of a theme.
    pub fn picture(&self, theme: usize) -> Option<&Rc<Image>> {
        self.pictures.get(theme)?.as_ref()
    }

    /// `subRegionsRescale` for a new screen size.
    pub fn resize(&mut self, width: i32, height: i32) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        for r in &mut self.regions {
            if r.def.defined() {
                r.rect = r.def.rect_for(width, height);
            }
        }
    }

    /// `subReset`: clears every region.
    pub fn reset(&mut self) {
        for r in &mut self.regions {
            r.cards.clear();
        }
    }

    /// Whether any region shows something (`subAnyCardsOnScreen`).
    pub fn any_cards(&self) -> bool {
        self.regions
            .iter()
            .any(|r| r.enabled && !r.cards.is_empty())
    }

    /// `subTitleAdd`: a fragment to show; joined with others of the same actor and event that
    /// arrive before the next [`Subtitles::update`].
    pub fn add(&mut self, actor: i32, event: u32, text: &[u8], seconds: f32) {
        if self.pending.len() < MAX_NEW {
            self.pending.push(NewSubtitle {
                actor,
                event,
                text: text.to_vec(),
                seconds,
            });
        } else {
            self.missed += 1;
        }
    }

    /// `kasfSubtitleSimulate`.
    pub fn simulate(&mut self, actor: i32, milliseconds: i32, text: &[u8]) {
        let e = self.dummy_event;
        self.dummy_event += 1;
        self.add(actor, e, text, milliseconds as f32 * 0.001);
        self.message_ended = 0;
    }

    /// `kasfLocationCard`: centred in the location-card region.
    pub fn location_card(&mut self, milliseconds: i32, text: &[u8]) {
        let mut s =
            format!("#r{:x}#t{:x}", region::LOCATION_CARD, THEME_LOCATION_CARD).into_bytes();
        s.extend_from_slice(text);
        s.truncate(2039);
        let e = self.dummy_event;
        self.dummy_event += 1;
        self.add(ACTOR_LOCATION_CARD, e, &s, milliseconds as f32 * 0.001);
        self.message_ended = 0;
    }

    /// `subControlsScan`: control codes at the start of a fragment.
    fn controls<'a>(
        mut s: &'a [u8],
        region: &mut usize,
        theme: &mut Option<usize>,
        continuous: &mut bool,
    ) -> &'a [u8] {
        while let Some(&c) = s.first() {
            if c != b'#' && c != b'\\' {
                break;
            }
            let code = s.get(1).copied().unwrap_or(0);
            match code {
                b'+' => {
                    *continuous = true;
                    s = &s[2.min(s.len())..];
                }
                b'r' => {
                    let d = s.get(2).copied().unwrap_or(b'0');
                    let i = i32::from(d) - i32::from(b'0');
                    if (0..NUMBER_REGIONS as i32).contains(&i) {
                        *region = i as usize;
                    }
                    s = &s[3.min(s.len())..];
                }
                b't' => {
                    let d = s.get(2).copied().unwrap_or(b'0');
                    let i = if d.is_ascii_digit() {
                        i32::from(d) - i32::from(b'0')
                    } else {
                        i32::from(d) - 0x57
                    };
                    if (0..NUMBER_THEMES as i32).contains(&i) {
                        *theme = Some(i as usize);
                    }
                    s = &s[3.min(s.len())..];
                }
                b'c' => {
                    *region = region::CLOSE_CAPTIONED;
                    *theme = Some(THEME_CLOSE_CAPTIONED);
                    s = &s[2.min(s.len())..];
                }
                _ => s = &s[2.min(s.len())..],
            }
        }
        s
    }

    /// `subTitlesUpdate`: lays out what arrived since the last frame. `now` is game time.
    pub fn update(&mut self, now: f32) {
        if self.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        let mut continuous = false;
        let mut result = None;
        let mut i = 0;
        // Each run of fragments of one actor and event is joined; only the last run is shown.
        while i < pending.len() {
            let first = &pending[i];
            let mut region = region::LETTERBOX;
            let mut theme = usize::try_from(first.actor)
                .ok()
                .filter(|&t| t < NUMBER_THEMES);
            let mut text =
                Self::controls(&first.text, &mut region, &mut theme, &mut continuous).to_vec();
            let mut seconds = first.seconds;
            while let Some(next) = pending.get(i + 1) {
                if next.actor != first.actor || next.event != first.event {
                    break;
                }
                i += 1;
                text.push(b' ');
                let mut ignore = false;
                text.extend_from_slice(Self::controls(
                    &next.text,
                    &mut region,
                    &mut theme,
                    &mut ignore,
                ));
                seconds += next.seconds;
            }
            result = Some((region, theme, text, seconds));
            i += 1;
        }
        let Some((region, Some(theme), text, seconds)) = result else {
            return;
        };
        let Some(font) = self.font(theme).cloned() else {
            return;
        };
        let rect = self.regions[region].rect;
        let lines = chop(&text, rect.x1 - rect.x0, |s| text::width(&font, s));
        if lines.is_empty() {
            return;
        }
        if self.language >= 1 {
            self.scrolly(region, theme, &font, lines, seconds, now);
        } else {
            self.paged(region, theme, &font, lines, seconds, now);
        }
        self.regions[region].continuous = continuous;
    }

    fn card(
        &self,
        theme: usize,
        x: i32,
        y: i32,
        created: f32,
        duration: f32,
        text: Vec<u8>,
    ) -> Card {
        let t = &self.script.themes[theme];
        Card {
            created,
            duration,
            fade_in: t.fade_in,
            fade_out: t.fade_out,
            color: t.color,
            drop_shadow: t.drop_shadow,
            theme,
            x,
            y,
            text,
        }
    }

    fn centre_x(&self, region: usize, theme: usize, font: &Font, line: &[u8]) -> i32 {
        if self.script.themes[theme].centred {
            let r = self.regions[region].rect;
            ((r.x1 - r.x0) - text::width(font, line)) / 2
        } else {
            0
        }
    }

    fn picture_of(&self, theme: usize) -> Option<usize> {
        self.script.themes[theme].picture.as_ref().map(|_| theme)
    }

    /// `subCreateNonScrollyLines`: pages of as many lines as fit, shown one after another.
    fn paged(
        &mut self,
        region: usize,
        theme: usize,
        font: &Font,
        lines: Vec<Vec<u8>>,
        seconds: f32,
        now: f32,
    ) {
        let seconds = seconds.max(self.script.title_shortest);
        let lh = text::height(font) + INTERLINE;
        let rect = self.regions[region].rect;
        let per_page = ((rect.y1 - rect.y0) / lh).max(1) as usize;
        let pages = lines.len().div_ceil(per_page);
        let per = seconds / pages as f32;
        let mut cards = Vec::with_capacity(lines.len());
        for (i, line) in lines.into_iter().enumerate() {
            let delay = if i < per_page {
                0.0
            } else {
                (i / per_page) as f32 * per
            };
            let x = self.centre_x(region, theme, font, &line);
            let y = lh * (i % per_page) as i32;
            cards.push(self.card(theme, x, y, now + delay, per, line));
        }
        let picture = self.picture_of(theme);
        let r = &mut self.regions[region];
        r.scroll_distance = 0.0;
        r.cards = cards;
        r.picture = picture;
        r.aborted = false;
    }

    /// `subCreateScrollyLines`: lines that scroll up through the region over the speech.
    fn scrolly(
        &mut self,
        region: usize,
        theme: usize,
        font: &Font,
        lines: Vec<Vec<u8>>,
        seconds: f32,
        now: f32,
    ) {
        let (dwell_start, dwell_end) =
            (self.script.scroll_dwell_start, self.script.scroll_dwell_end);
        let lh = text::height(font) + INTERLINE;
        let rect = self.regions[region].rect;
        let n = lines.len() as i32;
        let in_region = (rect.y1 - rect.y0) / lh;
        let to_scroll = n - in_region;
        let distance = ((lh * to_scroll) as f32).max(0.0);
        let scroll_time = seconds - dwell_start - dwell_end;
        let speed;
        {
            let r = &mut self.regions[region];
            if distance <= 0.0 {
                speed = 0.0;
                r.scroll_distance = 0.0;
            } else {
                r.scroll_start = now + dwell_start;
                let len = if scroll_time < 0.0 {
                    self.script.scroll_shortest
                } else {
                    scroll_time
                };
                r.scroll_end = r.scroll_start + len;
                r.scroll_distance = -distance;
                speed = distance / (r.scroll_end - r.scroll_start);
            }
        }
        let mut cards = Vec::with_capacity(lines.len());
        for (i, line) in lines.into_iter().enumerate() {
            let i = i as i32;
            let y = i * lh;
            let (delay, duration) = if i < in_region {
                let d = if i < to_scroll {
                    y as f32 / speed + dwell_start
                } else {
                    dwell_end + dwell_start + scroll_time
                };
                (0.0, d)
            } else {
                let delay = (lh * (i - in_region)) as f32 / speed + dwell_start;
                let d = if i < to_scroll {
                    (lh * in_region) as f32 / speed
                } else {
                    ((lh * n) as f32 - y as f32) / speed + dwell_end
                };
                (delay, d)
            };
            let x = self.centre_x(region, theme, font, &line);
            cards.push(self.card(theme, x, y, now + delay, duration, line));
        }
        let picture = self.picture_of(theme);
        let r = &mut self.regions[region];
        r.cards = cards;
        r.picture = picture;
    }

    /// `subTitlesFadeOut`: what is left in a region fades out over `seconds`.
    pub fn fade_out(&mut self, region: usize, seconds: f32, now: f32) {
        let r = &mut self.regions[region];
        for c in &mut r.cards {
            if c.created > now {
                c.created = now;
                c.duration = -1.0;
            } else if now - c.created + seconds < c.duration {
                c.fade_out = seconds;
                c.duration = now - c.created + seconds;
            }
        }
        r.aborted = true;
    }

    /// `smSensorsSkip`: the player skips the fleet intel event. A location card fades if one
    /// is up, otherwise the letterbox speech (the caller stops the speech itself).
    /// Returns whether the speech should be stopped.
    pub fn skip(&mut self, now: f32) -> bool {
        self.message_ended = 2;
        let card = &self.regions[region::LOCATION_CARD];
        if card.enabled && !card.cards.is_empty() {
            self.fade_out(region::LOCATION_CARD, SKIP_FADE, now);
            return false;
        }
        self.fade_out(region::LETTERBOX, SKIP_FADE, now);
        true
    }

    /// `subMessageEnded` taken by the mission script: 0 none, 1 ended, 2 skipped.
    pub fn take_message_ended(&mut self) -> u8 {
        std::mem::take(&mut self.message_ended)
    }

    /// Draws every region with cards (`rndRenderTask`'s subtitle pass). `now` is game time,
    /// `nis_now` the playing NIS's clock for the NIS region.
    pub fn draw(&mut self, canvas: &mut Canvas, now: f32, nis_now: Option<f32>, state: DrawState) {
        self.hud_top = HudTop::default();
        for i in 0..self.regions.len() {
            if !self.regions[i].enabled || self.regions[i].cards.is_empty() {
                continue;
            }
            let t = if i == region::NIS {
                nis_now.unwrap_or(now)
            } else {
                now
            };
            self.draw_region(canvas, i, t, state);
        }
    }

    /// What the last [`Subtitles::draw`] means for the main overlay's top (`moSetTop`).
    pub fn hud_top(&self) -> HudTop {
        self.hud_top
    }

    /// The band behind the letterbox region: the full screen width, 64 pixels from the
    /// portrait's top.
    pub fn band_rect(&self, width: i32) -> Rect {
        let r = &self.regions[region::LETTERBOX];
        let y0 = r.rect.y0 + r.def.icon_offset.1;
        Rect::new(0, y0, width, y0 + PICTURE_SIZE)
    }

    /// `subTitlesDraw`.
    fn draw_region(&mut self, canvas: &mut Canvas, index: usize, now: f32, state: DrawState) {
        self.hud_top.drawn = true;
        if index == region::LETTERBOX
            && !state.movie
            && !state.letterbox_fading
            && !state.letterboxed
        {
            let band = self.band_rect(canvas.width);
            canvas.blend_rect(band, [BAND_LEVEL, BAND_LEVEL, BAND_LEVEL, BAND_LEVEL]);
        }
        let r = &self.regions[index];
        let mut scroll = r.rect.y0;
        if r.scroll_distance != 0.0 && now >= r.scroll_start {
            let f = if now <= r.scroll_end {
                r.scroll_distance * (now - r.scroll_start) / (r.scroll_end - r.scroll_start)
            } else {
                r.scroll_distance
            };
            scroll = f as i32 + r.rect.y0;
        }
        let x0 = r.rect.x0;
        let mut i = 0;
        while i < self.regions[index].cards.len() {
            let card = &mut self.regions[index].cards[i];
            if now < card.created {
                i += 1;
                continue;
            }
            if now - card.created > card.duration {
                // Expired: the last card takes its place and is looked at next.
                self.regions[index].cards.swap_remove(i);
                continue;
            }
            let scale = |c: [u8; 3], v: f32| {
                let m = (v * 256.0).round() as u32;
                c.map(|x| ((u32::from(x) * m) >> 8) as u8)
            };
            let mut c = card.color;
            if card.fade_in != 0.0 {
                let mut v = (now - card.created) / card.fade_in;
                if v >= 1.0 {
                    card.fade_in = 0.0;
                    v = 1.0;
                }
                c = scale(c, v);
            }
            if card.fade_out != 0.0 {
                let from = card.duration + card.created - card.fade_out;
                if from <= now {
                    let v = (1.0 - (now - from) / card.fade_out).max(0.0);
                    c = scale(c, v);
                }
            }
            let (x, y) = (card.x + x0, card.y + scroll);
            let shadow = if card.drop_shadow {
                Shadow::SE
            } else {
                Shadow::None
            };
            let theme = card.theme;
            let text = card.text.clone();
            if let Some(font) = self.font(theme).cloned() {
                text::print(
                    canvas,
                    &font,
                    x,
                    y,
                    [c[0], c[1], c[2], 255],
                    &text,
                    (shadow, [0, 0, 0, 255]),
                );
            }
            i += 1;
        }
        let r = &self.regions[index];
        if let Some(img) = r.picture.and_then(|t| self.picture(t)) {
            let px = r.rect.x0 + r.def.icon_offset.0;
            let py = r.rect.y0 + r.def.icon_offset.1;
            canvas.blit(img, px, py, BlitMode::Blend);
            self.hud_top.portrait_bottom = Some(py + PICTURE_SIZE);
        }
        if index == region::LETTERBOX && r.cards.is_empty() && !r.aborted && !r.continuous {
            self.message_ended = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "themeColor 0, 255, 200, 100\nthemeDropshadow 0, YES\nthemeFont 0, Arial_13.hff\nthemeFadeIn 0, 0.2\nthemeFadeOut 0, 0.2\nthemePicture\t\t0, 0, FleetCommand\nthemeCentred 15, YES\nregionBounds 1, 80, 10, 620, 57 // top\nregionScaleRez 1, TRUE, TRUE\nregionMaxLines 1, 64\nregionIconOffset 1, -67, -10\nregionBounds 2, 20, 360, 620, 400\nregionScaleRez 2, TRUE, FALSE\nscrollDwellStart 2.7\nsubTitleShortest 2.0\n";

    #[test]
    fn parses_script() {
        let s = Script::parse(SCRIPT);
        let t = &s.themes[0];
        assert_eq!(t.color, [255, 200, 100]);
        assert!(t.drop_shadow);
        assert_eq!(t.font.as_deref(), Some("Arial_13.hff"));
        assert_eq!(t.picture, Some(("FleetCommand".to_string(), 0)));
        assert!(s.themes[15].centred);
        assert_eq!(s.regions[1].bounds, Rect::new(80, 10, 620, 57));
        assert_eq!(s.regions[1].icon_offset, (-67, -10));
        assert_eq!(s.scroll_dwell_start, 2.7);
        // The mis-cased key is ignored like the engine's exact match.
        assert_eq!(s.title_shortest, 1.2);
        assert_eq!(
            s.regions[1].rect_for(1280, 960),
            Rect::new(160, 20, 1240, 114)
        );
        assert_eq!(
            s.regions[2].rect_for(1280, 960),
            Rect::new(40, 600, 1240, 640)
        );
    }

    #[test]
    fn actors() {
        assert_eq!(actor_for_speech(0, 0x1000_10d6, true, 1), Some(0));
        assert_eq!(actor_for_speech(0, 0x1000_20dc, true, 1), Some(1));
        assert_eq!(actor_for_speech(0, 0x1000_80d9, true, 1), Some(4));
        assert_eq!(actor_for_speech(0, 0x1000_80d9, true, 2), Some(2));
        assert_eq!(actor_for_speech(0, 0x1000_80d9, true, 13), Some(3));
        assert_eq!(actor_for_speech(0, 0x1000_80d9, false, 1), Some(12));
        assert_eq!(actor_for_speech(0, 0x1000_0001, true, 1), Some(9));
        assert_eq!(actor_for_speech(0, 0x4000_1000, true, 1), Some(9));
        assert_eq!(actor_for_speech(-1, 0x1000, true, 1), None);
        assert_eq!(actor_for_speech(5, 0x1000, true, 1), Some(5));
    }

    fn mono(s: &[u8]) -> i32 {
        s.len() as i32 * 10
    }

    #[test]
    fn chops_at_blanks_and_newlines() {
        let lines = chop(b"aaa bbb ccc", 75, mono);
        assert_eq!(lines, vec![b"aaa bbb".to_vec(), b"ccc".to_vec()]);
        let lines = chop(b"one#n  two\\nthree\nfour", 1000, mono);
        assert_eq!(
            lines,
            vec![
                b"one".to_vec(),
                b"two".to_vec(),
                b"three".to_vec(),
                b"four".to_vec()
            ]
        );
        // No blank: cut mid-word before the character that reached the limit.
        let lines = chop(b"abcdefgh", 41, mono);
        assert_eq!(
            lines,
            vec![b"abc".to_vec(), b"def".to_vec(), b"gh".to_vec()]
        );
        // Several blanks: the line ends at the first, the next starts after the last.
        let lines = chop(b"ab   cdef", 75, mono);
        assert_eq!(lines, vec![b"ab".to_vec(), b"cdef".to_vec()]);
        assert!(chop(b"", 100, mono).is_empty());
        assert_eq!(chop(b"x", 0, mono), vec![b"x".to_vec()]);
    }

    #[test]
    fn controls() {
        let (mut r, mut t, mut c) = (1, Some(0), false);
        let rest = Subtitles::controls(b"#r3#tfHello", &mut r, &mut t, &mut c);
        assert_eq!((rest, r, t, c), (&b"Hello"[..], 3, Some(15), false));
        let rest = Subtitles::controls(b"\\+#cX", &mut r, &mut t, &mut c);
        assert_eq!((rest, r, t, c), (&b"X"[..], 2, Some(14), true));
    }
}
