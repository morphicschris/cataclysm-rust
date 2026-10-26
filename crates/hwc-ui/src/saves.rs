//! The saved-game lists of the front end's Load Game screen and the in-game save and load
//! screens (`gpGameWindowInit`, `gpTitleListLoad`, `gpDeleteGame`, `gpSaveGame`,
//! `gpOverwriteYes/No`), over a [`SaveStore`] the application provides (where the files are and
//! what is in them is the game's business, not the screens').

use std::rc::Rc;

use crate::fe::FrontEnd;
use crate::region::AtomRef;

/// Which list a screen shows (`SavedGames\SinglePlayer` or `SavedGames\Training`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFolder {
    SinglePlayer,
    Training,
}

/// A small RGB picture of a saved game.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// One saved game in a list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SaveEntry {
    /// What the game loads, overwrites and deletes it by (its file).
    pub key: String,
    /// The name the player gave it (what the original's list shows, `"%s"`).
    pub name: String,
    /// The mission and when it was saved (shown beside the name by the modern menus).
    pub detail: String,
    pub thumbnail: Option<Rc<Thumbnail>>,
}

/// The game's saved games.
pub trait SaveStore {
    /// The saved games of a folder, in the order the list shows them.
    fn list(&self, folder: SaveFolder) -> Vec<SaveEntry>;
    /// Deletes a saved game; false when it could not be.
    fn delete(&self, key: &str) -> bool;
    /// The key a game saved under `name` gets (whether or not it exists yet).
    fn key_for(&self, folder: SaveFolder, name: &str) -> String;
    /// Whether a saved game has this key.
    fn exists(&self, key: &str) -> bool;
}

/// `compareGamesCB`: names ignoring case, with an entry named "Quick Save" first.
pub fn sort_entries(v: &mut [SaveEntry]) {
    v.sort_by(|a, b| {
        let qa = !a.name.eq_ignore_ascii_case("Quick Save");
        let qb = !b.name.eq_ignore_ascii_case("Quick Save");
        qa.cmp(&qb).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
}

/// The shortest name a game may be saved under (`strAtLeast2Chars`).
pub const MIN_NAME: usize = 2;
/// The text box's length (`gpTextEntryWindowInit` resizes it to 30 characters).
pub const MAX_NAME: usize = 30;

/// Characters a saved game's name may not contain (the original's file-name filter).
pub fn name_char_allowed(c: char) -> bool {
    !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()
}

/// Fills a list atom with `entries`, the first selected (`gpTitleListLoad`).
pub fn fill_list<G>(fe: &mut FrontEnd<G>, atom: AtomRef, entries: &[SaveEntry]) {
    fe.list_init(atom, "Arial_12.hff", 18);
    if let Some(l) = fe.list_mut(atom) {
        l.items.clear();
        l.selected = None;
        l.upper = 0;
        for (i, g) in entries.iter().enumerate() {
            l.add(g.name.clone(), i as u32, i == 0);
        }
    }
    if let Some(id) = fe.atom(atom).region {
        fe.list_scroll_adjust(id);
    }
}

/// The index (into the entries the list was filled from) of the selected row of the list atom
/// `name`.
pub fn selected_index<G>(fe: &FrontEnd<G>, name: &str) -> Option<usize> {
    let a = fe.find_atom(name)?;
    let l = fe.list(a)?;
    l.selected
        .and_then(|i| l.items.get(i))
        .map(|item| item.data as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str) -> SaveEntry {
        SaveEntry {
            key: name.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn quick_save_sorts_first_then_names_ignoring_case() {
        let mut v = vec![e("beta"), e("Alpha"), e("Quick Save"), e("02 - Outskirts")];
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Quick Save", "02 - Outskirts", "Alpha", "beta"]);
    }

    #[test]
    fn file_name_filter() {
        assert!(name_char_allowed('a'));
        assert!(name_char_allowed(' '));
        assert!(name_char_allowed('~'));
        assert!(!name_char_allowed('/'));
        assert!(!name_char_allowed('?'));
    }
}
