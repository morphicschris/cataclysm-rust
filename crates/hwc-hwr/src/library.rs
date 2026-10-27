//! An imported Remastered effects folder (see [`crate::import`]), loaded for playing.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fx::{Anim, Effect, FxDef};
use crate::import::INDEX_FORMAT;
use crate::ships::{engine_trails, DeathSequence, EngineTrail};

/// Index of an effect in [`Library::effects`].
pub type FxId = usize;
/// Index of a texture path in [`Library::textures`].
pub type TexId = u32;

/// A texture as drawn: either a still image or the frames of an animation (image and texture
/// rectangle each).
#[derive(Debug, Clone, PartialEq)]
pub enum TexRef {
    Still(TexId),
    Anim {
        frames: Vec<(TexId, [f32; 4])>,
        fps: f32,
        loops: bool,
    },
}

/// A decoded image, top-down RGBA8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The file had an alpha channel.
    pub alpha: bool,
}

/// Decodes a true-colour TGA (types 2 and 10, 24 or 32 bits).
pub fn decode_tga(data: &[u8]) -> Option<Image> {
    let h = data.get(..18)?;
    let (id_len, cmap, kind) = (h[0] as usize, h[1], h[2]);
    let width = u16::from_le_bytes([h[12], h[13]]) as usize;
    let height = u16::from_le_bytes([h[14], h[15]]) as usize;
    let bpp = h[16] as usize / 8;
    if cmap != 0 || !matches!(kind, 2 | 10) || !matches!(bpp, 3 | 4) || width == 0 || height == 0 {
        return None;
    }
    let mut src = data.get(18 + id_len..)?;
    let count = width * height;
    let mut px: Vec<[u8; 4]> = Vec::with_capacity(count);
    let take = |s: &mut &[u8]| -> Option<[u8; 4]> {
        let p = s.get(..bpp)?;
        let v = [p[2], p[1], p[0], if bpp == 4 { p[3] } else { 255 }];
        *s = &s[bpp..];
        Some(v)
    };
    while px.len() < count {
        if kind == 2 {
            px.push(take(&mut src)?);
        } else {
            let (&c, rest) = src.split_first()?;
            src = rest;
            let n = (c & 0x7f) as usize + 1;
            if c & 0x80 != 0 {
                let p = take(&mut src)?;
                px.extend(std::iter::repeat_n(p, n));
            } else {
                for _ in 0..n {
                    px.push(take(&mut src)?);
                }
            }
        }
    }
    px.truncate(count);
    let top_down = h[17] & 0x20 != 0;
    let mut rgba = Vec::with_capacity(count * 4);
    for y in 0..height {
        let row = if top_down { y } else { height - 1 - y };
        for p in &px[row * width..(row + 1) * width] {
            rgba.extend_from_slice(p);
        }
    }
    Some(Image {
        width: width as u32,
        height: height as u32,
        rgba,
        alpha: bpp == 4,
    })
}

/// Loaded effects, ship data and texture names.
#[derive(Debug, Default)]
pub struct Library {
    root: PathBuf,
    pub effects: Vec<Effect>,
    pub effect_names: Vec<String>,
    by_name: HashMap<String, FxId>,
    pub textures: Vec<String>,
    tex_by_path: HashMap<String, TexId>,
    anims: HashMap<String, Anim>,
    deaths: HashMap<String, DeathSequence>,
    trails: HashMap<String, Vec<EngineTrail>>,
}

impl Library {
    /// Whether `dir` holds an import.
    pub fn exists(dir: &Path) -> bool {
        fs::read_to_string(dir.join("index.txt"))
            .is_ok_and(|s| s.lines().next() == Some(INDEX_FORMAT))
    }

    /// Loads an import folder.
    pub fn open(dir: &Path) -> io::Result<Self> {
        if !Self::exists(dir) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no imported Remastered effects in {}", dir.display()),
            ));
        }
        let mut lib = Self {
            root: dir.to_path_buf(),
            ..Self::default()
        };
        let mut defs = Vec::new();
        for e in fs::read_dir(dir.join("fx"))? {
            let path = e?.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(def) = FxDef::parse(stem, &fs::read(&path)?) {
                defs.push(def);
            }
        }
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        for d in &defs {
            lib.by_name.insert(d.name.clone(), lib.effects.len());
            lib.effect_names.push(d.name.clone());
            lib.effects.push(Effect::from_def(d));
        }
        if let Ok(rd) = fs::read_dir(dir.join("ships")) {
            for e in rd {
                let path = e?.path();
                let (Some(stem), Some(ext)) = (
                    path.file_stem().and_then(|s| s.to_str()),
                    path.extension().and_then(|s| s.to_str()),
                ) else {
                    continue;
                };
                let bytes = fs::read(&path)?;
                match ext {
                    "events" => {
                        lib.deaths
                            .insert(stem.to_ascii_lowercase(), DeathSequence::parse(&bytes));
                    }
                    "ship" => {
                        lib.trails
                            .insert(stem.to_ascii_lowercase(), engine_trails(&bytes));
                    }
                    _ => {}
                }
            }
        }
        Ok(lib)
    }

    /// Builds a library from effect definitions in memory (tests, tools).
    pub fn from_defs(defs: &[FxDef]) -> Self {
        let mut lib = Self::default();
        for d in defs {
            lib.by_name.insert(d.name.clone(), lib.effects.len());
            lib.effect_names.push(d.name.clone());
            lib.effects.push(Effect::from_def(d));
        }
        lib
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn effect(&self, name: &str) -> Option<FxId> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn death(&self, ship: &str) -> Option<&DeathSequence> {
        self.deaths.get(&ship.to_ascii_lowercase())
    }

    pub fn engine_trails(&self, ship: &str) -> &[EngineTrail] {
        self.trails
            .get(&ship.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    fn tex_id(&mut self, path: &str) -> TexId {
        if let Some(&id) = self.tex_by_path.get(path) {
            return id;
        }
        let id = self.textures.len() as TexId;
        self.textures.push(path.to_string());
        self.tex_by_path.insert(path.to_string(), id);
        id
    }

    fn file(&self, rel: &str) -> PathBuf {
        self.root
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    /// Resolves a texture or animation path (normalised) to what to draw.
    pub fn texture(&mut self, path: &str) -> Option<TexRef> {
        if path.ends_with(".anim") {
            if !self.anims.contains_key(path) {
                let anim = Anim::parse(&fs::read(self.file(path)).ok()?);
                self.anims.insert(path.to_string(), anim);
            }
            let anim = self.anims.get(path)?.clone();
            if anim.frames.is_empty() {
                return None;
            }
            let frames = anim
                .frames
                .iter()
                .map(|f| (self.tex_id(&f.path), f.uv))
                .collect();
            return Some(TexRef::Anim {
                frames,
                fps: anim.fps.max(1.0),
                loops: anim.loops,
            });
        }
        Some(TexRef::Still(self.tex_id(path)))
    }

    /// Reads and decodes a texture by id.
    pub fn load_image(&self, id: TexId) -> Option<Image> {
        let path = self.textures.get(id as usize)?;
        decode_tga(&fs::read(self.file(path)).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tga_bottom_up_24() {
        let mut t = vec![0u8; 18];
        t[2] = 2;
        t[12] = 1;
        t[14] = 2;
        t[16] = 24;
        t.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let img = decode_tga(&t).unwrap();
        assert_eq!((img.width, img.height, img.alpha), (1, 2, false));
        assert_eq!(img.rgba, vec![6, 5, 4, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn tga_rle_32() {
        let mut t = vec![0u8; 18];
        t[2] = 10;
        t[12] = 3;
        t[14] = 1;
        t[16] = 32;
        t[17] = 0x20;
        t.extend_from_slice(&[0x82, 1, 2, 3, 4]);
        let img = decode_tga(&t).unwrap();
        assert!(img.alpha);
        assert_eq!(img.rgba, [3, 2, 1, 4].repeat(3));
    }
}
