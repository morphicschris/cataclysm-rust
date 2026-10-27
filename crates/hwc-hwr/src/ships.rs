//! What the Remastered ships say about their deaths and engine trails.
//!
//! * `ship/<name>/<name>.events`: `effects.events` holds named events, each a list of pairs
//!   such as `{ "anim", "Death" }`, `{ "animtime", "0.65" }`, `{ "marker", "Root" }`,
//!   `{ "fx", "a,b" }`, `{ "fx_scale", "2.5" | "entity" }`. Events of the `Death` animation
//!   fire at `animtime × length` seconds into it.
//! * `ship/<name>/<name>.ship`: `setEngineTrail(ship, index, width, texture, a, b, c, length)`.

use crate::lua::{self, Value};

/// One effect start of a death sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct DeathEvent {
    /// Seconds after the ship died.
    pub time: f32,
    /// Effects started at once (lowercase names).
    pub fx: Vec<String>,
    /// Marker names (`Root`, `top_00`, ...): where on the hull the effects start.
    pub markers: Vec<String>,
    /// `fx_scale`: a number, or `None` for `entity` (scaled by the ship's size).
    pub scale: Option<f32>,
}

/// A ship's death sequence (the events of its `Death` animation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeathSequence {
    pub length: f32,
    pub events: Vec<DeathEvent>,
}

fn pair_list(v: &Value) -> Vec<(String, String)> {
    v.as_table().map_or_else(Vec::new, |t| {
        t.items
            .iter()
            .filter_map(|p| {
                let p = p.as_table()?;
                let k = p.items.first()?.as_str()?.to_ascii_lowercase();
                let v = match p.items.get(1)? {
                    Value::Str(s) => s.clone(),
                    Value::Num(n) => format!("{n}"),
                    _ => return None,
                };
                Some((k, v))
            })
            .collect()
    })
}

fn split(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl DeathSequence {
    /// Reads the `Death` animation's events from an `.events` file.
    pub fn parse(src: &[u8]) -> Self {
        let c = lua::parse_bytes(src);
        let Some(effects) = c.global("effects") else {
            return Self::default();
        };
        let mut length = 0.0;
        if let Some(anims) = effects.get("animations").and_then(Value::as_table) {
            for (_, a) in &anims.fields {
                if a.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.eq_ignore_ascii_case("death"))
                {
                    length = a.get("length").and_then(Value::as_num).unwrap_or(0.0) as f32;
                }
            }
        }
        let mut events = Vec::new();
        if let Some(evs) = effects.get("events").and_then(Value::as_table) {
            for (_, e) in &evs.fields {
                let pairs = pair_list(e);
                let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                if !get("anim").is_some_and(|a| a.eq_ignore_ascii_case("death")) {
                    continue;
                }
                let fx: Vec<String> = get("fx")
                    .map(split)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
                if fx.is_empty() {
                    continue;
                }
                let at: f32 = get("animtime").and_then(|t| t.parse().ok()).unwrap_or(0.0);
                events.push(DeathEvent {
                    time: at.max(0.0) * length,
                    fx,
                    markers: get("marker").map(split).unwrap_or_default(),
                    scale: get("fx_scale").and_then(|s| s.parse().ok()),
                });
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        Self { length, events }
    }

    /// Every effect the sequence starts.
    pub fn effects(&self) -> impl Iterator<Item = &str> {
        self.events
            .iter()
            .flat_map(|e| e.fx.iter().map(String::as_str))
    }
}

/// `setEngineTrail` of one engine.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineTrail {
    pub index: u32,
    pub width: f32,
    pub texture: String,
    /// The three tuning values between texture and length, kept as read.
    pub params: [f32; 3],
    pub length: f32,
}

/// Reads the engine trails of a `.ship` file.
pub fn engine_trails(src: &[u8]) -> Vec<EngineTrail> {
    let c = lua::parse_bytes(src);
    c.calls_named("setEngineTrail")
        .filter_map(|call| {
            let n = |i: usize| call.args.get(i).and_then(Value::as_num).map(|v| v as f32);
            Some(EngineTrail {
                index: n(1)? as u32,
                width: n(2)?,
                texture: call.args.get(3)?.as_str()?.to_ascii_lowercase(),
                params: [
                    n(4).unwrap_or(0.0),
                    n(5).unwrap_or(0.0),
                    n(6).unwrap_or(0.0),
                ],
                length: n(7).unwrap_or(1.0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_events() {
        let src = br#"effects =
{
    animations = {
        animation1 = { name = "Death", length = 2, loop = 0, parent = "", markers = {""} },
        animation7 = { name = "Damage", length = 1, loop = 1, parent = "", markers = {""} },
    },
    events = {
        death2 = {
            { "anim", "Death" }, { "animtime", "0.65" }, { "marker", "Root" },
            { "fx", "B_Fx,c_fx" }, { "fx_scale", "1" },
        },
        --comment
        death1 = { { "anim", "Death" }, { "animtime", "0" }, { "marker", "top_00,left_00" }, { "fx", "a" }, { "fx_scale", "entity" }, },
        dmg1 = { { "anim", "dmg01" }, { "fx", "smoke" }, },
    },
}"#;
        let d = DeathSequence::parse(src);
        assert_eq!(d.length, 2.0);
        assert_eq!(d.events.len(), 2);
        assert_eq!(d.events[0].fx, vec!["a"]);
        assert_eq!(d.events[0].scale, None);
        assert_eq!(d.events[0].markers, vec!["top_00", "left_00"]);
        assert!((d.events[1].time - 1.3).abs() < 1e-6);
        assert_eq!(d.events[1].fx, vec!["b_fx", "c_fx"]);
        assert_eq!(d.events[1].scale, Some(1.0));
    }

    #[test]
    fn trails() {
        let t = engine_trails(
            b"setEngineTrail(NewShipType,1,3.86,\"Trail_Ribbon.tga\",0.001,0.5,0.025,2);",
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].index, 1);
        assert_eq!(t[0].width, 3.86);
        assert_eq!(t[0].texture, "trail_ribbon.tga");
        assert_eq!(t[0].length, 2.0);
    }
}
