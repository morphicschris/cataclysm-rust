//! Where front-end files come from, and caches for the bitmaps and fonts loaded from them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use hwc_formats::big::BigArchive;
use hwc_formats::font::Font;
use hwc_formats::lif::Lif;

use crate::canvas::Image;

/// Read access to the game's file namespace (archive names such as `feman\front_end.fib`).
/// Lookups ignore ASCII case and accept `/` or `\`, like the original's file layer.
pub trait AssetSource {
    fn read(&self, name: &str) -> Option<Vec<u8>>;
}

impl AssetSource for BigArchive {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.read_file(name).ok().flatten()
    }
}

impl AssetSource for hwc_formats::GameData {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        hwc_formats::GameData::read(self, name).ok().flatten()
    }
}

/// An in-memory source, for tests and tools.
#[derive(Debug, Default, Clone)]
pub struct MemorySource {
    files: HashMap<String, Vec<u8>>,
}

impl MemorySource {
    pub fn insert(&mut self, name: &str, data: Vec<u8>) {
        self.files.insert(normalize(name), data);
    }
}

impl AssetSource for MemorySource {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.files.get(&normalize(name)).cloned()
    }
}

/// Lower-case, backslash-separated key.
pub fn normalize(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Front-end language: selects the font directory and the hot-key slot of each atom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    French,
    German,
    Spanish,
    Italian,
}

impl Language {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: u32) -> Language {
        match i {
            1 => Language::French,
            2 => Language::German,
            3 => Language::Spanish,
            4 => Language::Italian,
            _ => Language::English,
        }
    }

    fn font_dir(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::French => "French",
            Language::German => "German",
            Language::Spanish => "Spanish",
            Language::Italian => "Italian",
        }
    }
}

/// Loads a `.lif` into an RGBA image.
pub fn decode_lif(data: &[u8]) -> Option<Image> {
    let lif = Lif::parse(data).ok()?;
    Some(Image::from_rgba(
        lif.width as i32,
        lif.height as i32,
        &lif.to_rgba(),
    ))
}

/// Shared caches. Missing files are remembered (and reported once) so they are not retried.
pub struct Resources {
    source: Rc<dyn AssetSource>,
    images: RefCell<HashMap<String, Option<Rc<Image>>>>,
    fonts: RefCell<HashMap<String, Option<Rc<Font>>>>,
    language: Cell<Language>,
}

impl Resources {
    pub fn new(source: Rc<dyn AssetSource>) -> Self {
        Self {
            source,
            images: RefCell::new(HashMap::new()),
            fonts: RefCell::new(HashMap::new()),
            language: Cell::new(Language::English),
        }
    }

    pub fn language(&self) -> Language {
        self.language.get()
    }

    /// Switches the font directory (and the hot-key slot the front end reads).
    pub fn set_language(&mut self, language: Language) {
        self.language.set(language);
    }

    /// Like [`Resources::set_language`] through a shared handle.
    pub fn switch_language(&self, language: Language) {
        self.language.set(language);
    }

    pub fn source(&self) -> &Rc<dyn AssetSource> {
        &self.source
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.source.read(name)
    }

    /// A `.lif` by archive path (cached; `None` if missing or unreadable).
    pub fn image(&self, path: &str) -> Option<Rc<Image>> {
        let key = normalize(path);
        if let Some(hit) = self.images.borrow().get(&key) {
            return hit.clone();
        }
        let img = self
            .source
            .read(path)
            .and_then(|d| decode_lif(&d))
            .map(Rc::new);
        if img.is_none() {
            eprintln!("hwc-ui: bitmap {path} unavailable");
        }
        self.images.borrow_mut().insert(key, img.clone());
        img
    }

    /// Caches a derived image (rotated frame pieces) under a private key.
    pub fn derived_image(
        &self,
        key: &str,
        make: impl FnOnce() -> Option<Image>,
    ) -> Option<Rc<Image>> {
        let key = format!("\u{0}{key}");
        if let Some(hit) = self.images.borrow().get(&key) {
            return hit.clone();
        }
        let img = make().map(Rc::new);
        self.images.borrow_mut().insert(key, img.clone());
        img
    }

    /// A font by file name, looked up in `Fonts\<language>\` like `frFontRegister`.
    pub fn font(&self, name: &str) -> Option<Rc<Font>> {
        let path = format!("Fonts\\{}\\{}", self.language.get().font_dir(), name);
        let key = normalize(&path);
        if let Some(hit) = self.fonts.borrow().get(&key) {
            return hit.clone();
        }
        let font = self
            .source
            .read(&path)
            .and_then(|d| Font::parse(&d).ok())
            .map(Rc::new);
        if font.is_none() {
            eprintln!("hwc-ui: font {path} unavailable");
        }
        self.fonts.borrow_mut().insert(key, font.clone());
        font
    }
}
