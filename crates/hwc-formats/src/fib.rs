//! `.fib` front-end screen layouts ("FEMan" output).
//!
//! A FIB file is a memory image of the front-end manager's screen list with every pointer stored
//! as a file offset (0 for null). Each screen has named links to other screens and a list of
//! atoms: rectangles that are buttons, text labels, decorative bitmaps, list windows and so on.
//! Callback binding is by name: the engine looks an atom's name up in its table of registered
//! functions (or, for links, its target screen). All values are little-endian.
//!
//! Header (12 bytes): identifier `"Bananna\0"`, `u16` version `0x212`, `u16` screen count.
//! Screens follow immediately.
//!
//! Screen (20 bytes): name offset, `u32` flags, `u16` link count, `u16` atom count, links offset,
//! atoms offset.
//!
//! Link (12 bytes): name offset, `u32` [`link flags`](LINK_ENABLED), target screen name offset.
//!
//! Atom (76 bytes):
//!
//! | offset | type     | field                                                          |
//! |--------|----------|----------------------------------------------------------------|
//! | 0      | u32      | name offset (callback name, link name or bitmap path)          |
//! | 4      | u32      | [flags](ATOM_LINK)                                             |
//! | 8      | u32      | runtime status (junk in files)                                 |
//! | 12     | u8       | [`AtomKind`]                                                   |
//! | 13     | u8       | border width                                                   |
//! | 14     | u16      | tab-stop order                                                 |
//! | 16     | [u8; 4]  | border colour RGBA (also the drop-shadow colour for text)      |
//! | 20     | [u8; 4]  | content colour RGBA (text colour for labels)                   |
//! | 24     | i32 × 4  | left, top, right, bottom on a 640×480 canvas                   |
//! | 40     | u32      | data: text offset for labels, a plain value for radio buttons  |
//! | 44     | u32      | attributes: font name offset for labels, bitmap name for bitmap buttons |
//! | 48     | u8       | hot-key modifier mask                                          |
//! | 49     | [u8; 5]  | hot key per language (English, French, German, Spanish, Italian) |
//! | 54     | [u8; 2]  | padding                                                        |
//! | 56     | u32 × 2  | draw styles                                                    |
//! | 64     | u32 × 3  | runtime region pointer and padding (junk)                      |
//!
//! Text labels are stored with kind 0 and non-null data; the engine turns them into static text
//! when loading. Uninitialised bytes are filled with `0xCD`.

use thiserror::Error;

pub const IDENTIFIER: &[u8; 8] = b"Bananna\0";
pub const VERSION: u16 = 0x212;
pub const HEADER_LEN: usize = 12;
pub const SCREEN_LEN: usize = 20;
pub const LINK_LEN: usize = 12;
pub const ATOM_LEN: usize = 76;
/// Number of languages with their own hot key.
pub const LANGUAGES: usize = 5;

/// Link flag: the link is live (its name and target are present).
pub const LINK_ENABLED: u32 = 0x1;
pub const LINK_DEFAULT_PREV: u32 = 0x2;
pub const LINK_DEFAULT_NEXT: u32 = 0x4;
pub const LINK_RETAIN_PREVIOUS: u32 = 0x8;

/// Atom flag: pressing the atom follows the link of the same name.
pub const ATOM_LINK: u32 = 0x0000_0001;
/// Atom flag: pressing the atom calls the callback of the same name.
pub const ATOM_FUNCTION: u32 = 0x0000_0002;
pub const ATOM_BITMAP: u32 = 0x0000_0004;
pub const ATOM_MODAL: u32 = 0x0000_0008;
pub const ATOM_POPUP: u32 = 0x0000_0010;
pub const ATOM_CALL_ON_CREATE: u32 = 0x0000_0020;
pub const ATOM_CONTENTS_VISIBLE: u32 = 0x0000_0040;
pub const ATOM_DEFAULT_OK: u32 = 0x0000_0080;
pub const ATOM_DEFAULT_BACK: u32 = 0x0000_0100;
pub const ATOM_ALWAYS_ON_TOP: u32 = 0x0000_0200;
pub const ATOM_DRAGGABLE: u32 = 0x0000_0400;
pub const ATOM_BORDER_VISIBLE: u32 = 0x0000_0800;
pub const ATOM_DISABLED: u32 = 0x0000_1000;
pub const ATOM_DONT_CUTOUT_BASE: u32 = 0x0000_2000;
pub const ATOM_CALL_ON_DELETE: u32 = 0x0000_4000;
pub const ATOM_HIDDEN: u32 = 0x0000_8000;
pub const ATOM_JUSTIFY_MASK: u32 = 0x0003_0000;
pub const ATOM_BACKGROUND: u32 = 0x0004_0000;
pub const ATOM_DROP_SHADOW_MASK: u32 = 0x0ff0_0000;
const DROP_SHADOW_SHIFT: u32 = 20;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a FIB header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u16),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("unterminated string at offset {0}")]
    UnterminatedString(usize),
    #[error("screen {screen} has no name")]
    UnnamedScreen { screen: usize },
    #[error("screen {screen:?} atom {atom}: empty rectangle {rect:?}")]
    EmptyRect {
        screen: String,
        atom: usize,
        rect: Rect,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Control types. Numbering follows the front-end manager; 0 is a plain region (a text label when
/// it carries text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomKind {
    Plain,
    UserRegion,
    StaticText,
    Button,
    CheckBox,
    ToggleButton,
    ScrollBar,
    StatusBar,
    TextEntry,
    ListViewExpandButton,
    TitleBar,
    MenuItem,
    RadioButton,
    CutoutRegion,
    DecorativeRegion,
    Divider,
    ListWindow,
    BitmapButton,
    HorizontalSlider,
    VerticalSlider,
    DragButton,
    OpaqueDecorativeRegion,
    Other(u8),
}

impl AtomKind {
    pub fn from_raw(v: u8) -> AtomKind {
        use AtomKind::*;
        const KINDS: [AtomKind; 22] = [
            Plain,
            UserRegion,
            StaticText,
            Button,
            CheckBox,
            ToggleButton,
            ScrollBar,
            StatusBar,
            TextEntry,
            ListViewExpandButton,
            TitleBar,
            MenuItem,
            RadioButton,
            CutoutRegion,
            DecorativeRegion,
            Divider,
            ListWindow,
            BitmapButton,
            HorizontalSlider,
            VerticalSlider,
            DragButton,
            OpaqueDecorativeRegion,
        ];
        KINDS.get(v as usize).copied().unwrap_or(Other(v))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justification {
    Default,
    Left,
    Right,
    Centre,
}

/// Edges in 640×480 front-end coordinates; right and bottom are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomData {
    None,
    /// A text label: the string (with `&` marking the hot-key letter) and its `.hff` font name.
    Text {
        text: String,
        font: Option<String>,
    },
    /// A radio button's value within its group (the group is the atom name).
    RadioValue(u32),
    /// Any other non-null data pointer (not seen in shipped files).
    Raw(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom {
    /// Callback or link name; for decorative regions, the bitmap path.
    pub name: Option<String>,
    pub flags: u32,
    pub raw_kind: u8,
    pub kind: AtomKind,
    pub border_width: u8,
    pub tab_stop: u16,
    pub border_color: [u8; 4],
    pub content_color: [u8; 4],
    pub rect: Rect,
    pub data: AtomData,
    /// Bitmap name for bitmap buttons.
    pub bitmap: Option<String>,
    pub hotkey_modifiers: u8,
    /// One key per language; 0 for none.
    pub hotkeys: [u8; LANGUAGES],
    pub draw_style: [u32; 2],
}

impl Atom {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    pub fn justification(&self) -> Justification {
        match (self.flags & ATOM_JUSTIFY_MASK) >> 16 {
            1 => Justification::Left,
            2 => Justification::Right,
            3 => Justification::Centre,
            _ => Justification::Default,
        }
    }

    /// Drop-shadow direction bits for text (N, S, E, W, NE, NW, SE, SW from bit 0).
    pub fn drop_shadow(&self) -> u8 {
        ((self.flags & ATOM_DROP_SHADOW_MASK) >> DROP_SHADOW_SHIFT) as u8
    }

    pub fn text(&self) -> Option<&str> {
        match &self.data {
            AtomData::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn font(&self) -> Option<&str> {
        match &self.data {
            AtomData::Text { font, .. } => font.as_deref(),
            _ => None,
        }
    }

    /// The image a decorative region or bitmap button draws.
    pub fn bitmap_path(&self) -> Option<&str> {
        match self.kind {
            AtomKind::DecorativeRegion | AtomKind::OpaqueDecorativeRegion => self.name.as_deref(),
            _ => self.bitmap.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: Option<String>,
    pub flags: u32,
    /// Name of the screen the link opens.
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub flags: u32,
    pub links: Vec<Link>,
    pub atoms: Vec<Atom>,
}

impl Screen {
    pub fn link(&self, name: &str) -> Option<&Link> {
        self.links.iter().find(|l| l.name.as_deref() == Some(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fib {
    pub version: u16,
    pub screens: Vec<Screen>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&self, what: &'static str, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.data.get(offset..end))
            .ok_or(Error::OutOfBounds {
                what,
                offset,
                len,
                file_len: self.data.len(),
            })
    }

    fn table(
        &self,
        what: &'static str,
        offset: u32,
        count: usize,
        stride: usize,
    ) -> Result<std::slice::ChunksExact<'a, u8>> {
        Ok(self
            .bytes(what, offset as usize, count * stride)?
            .chunks_exact(stride))
    }

    fn opt_string(&self, offset: u32) -> Result<Option<String>> {
        if offset == 0 {
            return Ok(None);
        }
        self.bytes("string", offset as usize, 1)?;
        let rest = &self.data[offset as usize..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::UnterminatedString(offset as usize))?;
        // Latin-1: every byte is the code point of the same value.
        Ok(Some(rest[..end].iter().map(|&b| b as char).collect()))
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn i32_at(b: &[u8], at: usize) -> i32 {
    u32_at(b, at) as i32
}

fn rgba_at(b: &[u8], at: usize) -> [u8; 4] {
    b[at..at + 4].try_into().unwrap()
}

impl Fib {
    pub fn parse(data: &[u8]) -> Result<Fib> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[..8].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = u16_at(data, 8);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let count = u16_at(data, 10) as usize;
        let r = Reader { data };

        let mut screens = Vec::with_capacity(count);
        for (index, s) in r
            .table("screen", HEADER_LEN as u32, count, SCREEN_LEN)?
            .enumerate()
        {
            let name = r
                .opt_string(u32_at(s, 0))?
                .ok_or(Error::UnnamedScreen { screen: index })?;
            let links = r
                .table("links", u32_at(s, 12), u16_at(s, 8) as usize, LINK_LEN)?
                .map(|l| {
                    Ok(Link {
                        name: r.opt_string(u32_at(l, 0))?,
                        flags: u32_at(l, 4),
                        target: r.opt_string(u32_at(l, 8))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut atoms = Vec::new();
            for (i, a) in r
                .table("atoms", u32_at(s, 16), u16_at(s, 10) as usize, ATOM_LEN)?
                .enumerate()
            {
                let atom = parse_atom(&r, a)?;
                if atom.rect.width() <= 0 || atom.rect.height() <= 0 {
                    return Err(Error::EmptyRect {
                        screen: name,
                        atom: i,
                        rect: atom.rect,
                    });
                }
                atoms.push(atom);
            }
            screens.push(Screen {
                name,
                flags: u32_at(s, 4),
                links,
                atoms,
            });
        }
        Ok(Fib { version, screens })
    }

    pub fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.name == name)
    }
}

fn parse_atom(r: &Reader, a: &[u8]) -> Result<Atom> {
    let raw_kind = a[12];
    let kind = AtomKind::from_raw(raw_kind);
    let data_word = u32_at(a, 40);
    let attribs = u32_at(a, 44);
    let data = if data_word == 0 {
        AtomData::None
    } else if kind == AtomKind::RadioButton {
        AtomData::RadioValue(data_word)
    } else if matches!(kind, AtomKind::Plain | AtomKind::StaticText) {
        AtomData::Text {
            text: r.opt_string(data_word)?.unwrap_or_default(),
            font: r.opt_string(attribs)?,
        }
    } else {
        AtomData::Raw(data_word)
    };
    let bitmap = if kind == AtomKind::BitmapButton {
        r.opt_string(attribs)?
    } else {
        None
    };
    Ok(Atom {
        name: r.opt_string(u32_at(a, 0))?,
        flags: u32_at(a, 4),
        raw_kind,
        kind,
        border_width: a[13],
        tab_stop: u16_at(a, 14),
        border_color: rgba_at(a, 16),
        content_color: rgba_at(a, 20),
        rect: Rect {
            left: i32_at(a, 24),
            top: i32_at(a, 28),
            right: i32_at(a, 32),
            bottom: i32_at(a, 36),
        },
        data,
        bitmap,
        hotkey_modifiers: a[48],
        hotkeys: a[49..49 + LANGUAGES].try_into().unwrap(),
        draw_style: [u32_at(a, 56), u32_at(a, 60)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_screen() {
        let screen = HEADER_LEN;
        let link = screen + SCREEN_LEN;
        let atom = link + LINK_LEN;
        let strings = atom + 2 * ATOM_LEN;
        let mut d = vec![0u8; strings];
        let add = |d: &mut Vec<u8>, s: &str| {
            let at = d.len() as u32;
            d.extend_from_slice(s.as_bytes());
            d.push(0);
            at
        };
        let s_main = add(&mut d, "Main");
        let s_link = add(&mut d, "Main-TO-Quit");
        let s_quit = add(&mut d, "Quit");
        let s_text = add(&mut d, "&QUIT");
        let s_font = add(&mut d, "Arial_12.hff");
        d[..8].copy_from_slice(IDENTIFIER);
        d[8..10].copy_from_slice(&VERSION.to_le_bytes());
        d[10..12].copy_from_slice(&1u16.to_le_bytes());
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, screen, s_main);
        d[screen + 8..screen + 12].copy_from_slice(&[1, 0, 2, 0]);
        put(&mut d, screen + 12, link as u32);
        put(&mut d, screen + 16, atom as u32);
        put(&mut d, link, s_link);
        put(&mut d, link + 4, LINK_ENABLED);
        put(&mut d, link + 8, s_quit);
        for (i, (kind, name, flags, text)) in [(3u8, s_link, ATOM_LINK, 0), (0, 0, 0x30040, s_text)]
            .into_iter()
            .enumerate()
        {
            let a = atom + i * ATOM_LEN;
            put(&mut d, a, name);
            put(&mut d, a + 4, flags);
            d[a + 12] = kind;
            for (j, v) in [10u32, 20, 110, 40].into_iter().enumerate() {
                put(&mut d, a + 24 + j * 4, v);
            }
            put(&mut d, a + 40, text);
            put(&mut d, a + 44, if text != 0 { s_font } else { 0 });
            d[a + 49] = b'Q';
        }
        let fib = Fib::parse(&d).unwrap();
        let s = fib.screen("Main").unwrap();
        assert_eq!(
            s.link("Main-TO-Quit").unwrap().target.as_deref(),
            Some("Quit")
        );
        assert_eq!(s.atoms[0].kind, AtomKind::Button);
        assert!(s.atoms[0].has(ATOM_LINK));
        assert_eq!(s.atoms[1].text(), Some("&QUIT"));
        assert_eq!(s.atoms[1].font(), Some("Arial_12.hff"));
        assert_eq!(s.atoms[1].justification(), Justification::Centre);
        assert_eq!(s.atoms[1].rect.width(), 100);
        assert_eq!(s.atoms[1].hotkeys[0], b'Q');

        put(&mut d, atom + 32, 5);
        assert!(matches!(
            Fib::parse(&d),
            Err(Error::EmptyRect { atom: 0, .. })
        ));
    }
}
