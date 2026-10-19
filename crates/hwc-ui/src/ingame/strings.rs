//! The language string table (`<Language>.script`, read through `scriptSet` into `strings[]`):
//! `name  text` lines, `;` comments, `\n` for line breaks.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Strings {
    map: HashMap<String, String>,
}

impl Strings {
    pub fn parse(text: &str) -> Strings {
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim_start();
            if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
                continue;
            }
            let Some(split) = line.find(char::is_whitespace) else {
                continue;
            };
            let (name, rest) = line.split_at(split);
            let value = rest.trim().replace("\\n", "\n");
            map.entry(name.to_string()).or_insert(value);
        }
        Strings { map }
    }

    /// Adds the entries of another table that this one lacks (English fallback).
    pub fn fill_from(&mut self, other: &Strings) {
        for (k, v) in &other.map {
            self.map.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(String::as_str)
    }

    /// The string, or its name when missing.
    pub fn text(&self, name: &str) -> String {
        self.get(name)
            .map_or_else(|| name.to_string(), str::to_string)
    }

    /// Display description of a key action: the `..._Description` entry next to the
    /// `..._KeyBindingName` entry whose text is the action's configuration name.
    pub fn key_description(&self, config_name: &str) -> Option<&str> {
        let key = self
            .map
            .iter()
            .filter(|(k, v)| k.ends_with("_KeyBindingName") && v.as_str() == config_name)
            .map(|(k, _)| k)
            .min()?;
        let base = key.strip_suffix("_KeyBindingName")?;
        self.get(&format!("{base}_Description"))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_descriptions() {
        let s = Strings::parse(
            "; comment\nstr_UI_MoveShips_KeyBindingName\t\tMove\n\
             str_UI_MoveShips_Description\tMove Ships\nstrX  A\\nB\n",
        );
        assert_eq!(s.get("strX"), Some("A\nB"));
        assert_eq!(s.key_description("Move"), Some("Move Ships"));
        assert_eq!(s.key_description("Dock"), None);
    }
}
