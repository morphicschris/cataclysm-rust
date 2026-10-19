//! The game files seen by the renderer (`hwc_render::AssetSource`) and the simulation
//! (`hwc_sim::statics::DataSource`): one shared [`GameData`].

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_sim::statics::DataSource;

#[derive(Clone)]
pub struct Assets(pub Rc<GameData>);

impl Assets {
    pub fn data(&self) -> &GameData {
        &self.0
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path).ok().flatten()
    }
}

impl hwc_render::AssetSource for Assets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_file(path)
    }

    fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .0
            .archive()
            .entries()
            .iter()
            .map(|e| hwc_formats::big::normalize_name(&e.name))
            .collect();
        names.extend(self.0.overrides().keys().cloned());
        names.sort();
        names.dedup();
        names
    }
}

impl DataSource for Assets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_file(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.0.contains(path)
    }
}

/// `name` as an archive path in `dir` with extension `ext` (level `Background`/`Lighting`
/// values are bare names).
pub fn normalize_asset(name: &str, dir: &str, ext: &str) -> String {
    let mut p = name.to_ascii_lowercase().replace('/', "\\");
    if !p.contains('\\') {
        p = format!("{dir}\\{p}");
    }
    if !p.ends_with(&format!(".{ext}")) {
        p.push('.');
        p.push_str(ext);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert_eq!(
            normalize_asset("mission13", "btg", "btg"),
            "btg\\mission13.btg"
        );
        assert_eq!(normalize_asset("BTG/M.btg", "btg", "btg"), "btg\\m.btg");
    }
}
