//! Parse-checking an asset by its file extension with the readers of this crate. Used by the
//! importer's verification and by tools that validate mods.

use crate::{audio, btg, fib, font, geo, hsf, lif, mad, mex, nis, script};

/// Extensions of the keyword/value text files read with [`script::ScriptFile`].
pub const TEXT_EXTS: [&str; 14] =
    ["script", "shp", "dist", "missphere", "level", "mission", "ebg", "plug", "mif", "lod", "cred", "lst", "list", "txt"];

/// Lowercase extension of the last path component (`/` or `\` separated), or `""`.
pub fn extension(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// `name` without its extension.
pub fn stem_path(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Whether `ext` (lowercase) is handled by [`check_asset`].
pub fn is_checked(ext: &str) -> bool {
    matches!(ext, "lif" | "geo" | "peo" | "mex" | "mad" | "btg" | "hff" | "fib" | "hsf" | "nis" | "bnk" | "lut" | "wxh")
        || TEXT_EXTS.contains(&ext)
}

/// Parses one asset by extension (lowercase). `nis_script` says the file is the event list of a
/// `.nis` of the same name, which is also parsed as a NIS event script. Extensions without a
/// parser are accepted.
pub fn check_asset(ext: &str, bytes: &[u8], nis_script: bool) -> Result<(), String> {
    fn s<E: std::fmt::Display>(r: Result<impl Sized, E>) -> Result<(), String> {
        r.map(|_| ()).map_err(|e| e.to_string())
    }
    match ext {
        "lif" => s(lif::Lif::parse(bytes)),
        "geo" | "peo" => s(geo::Geo::parse(bytes)),
        "mex" => s(mex::Mex::parse(bytes)),
        "mad" => s(mad::Mad::parse(bytes)),
        "btg" => s(btg::Btg::parse(bytes)),
        "hff" => s(font::Font::parse(bytes)),
        "fib" => s(fib::Fib::parse(bytes)),
        "hsf" => s(hsf::Hsf::parse(bytes)),
        "nis" => s(nis::Nis::parse(bytes)),
        "bnk" => s(audio::Bank::parse(bytes)),
        "lut" => s(audio::Lut::parse(bytes)),
        "wxh" => s(audio::MusicIndex::parse(bytes)),
        "script" if nis_script => {
            s(script::ScriptFile::parse_bytes(bytes))?;
            s(nis::NisScript::parse_bytes(bytes))
        }
        e if TEXT_EXTS.contains(&e) => s(script::ScriptFile::parse_bytes(bytes)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(extension("race_sect\\smothership\\rl0\\lod0\\smothership.GEO"), "geo");
        assert_eq!(extension("dir.d\\noext"), "");
        assert_eq!(stem_path("nis\\n01.script"), "nis\\n01");
        assert!(is_checked("peo") && is_checked("shp") && !is_checked("jpg"));
    }

    #[test]
    fn garbage_fails_binary_parsers() {
        for ext in ["lif", "geo", "mex", "mad", "btg", "hff", "fib", "hsf", "nis", "bnk", "lut", "wxh"] {
            assert!(check_asset(ext, b"garbage!garbage!", false).is_err(), "{ext} accepted garbage");
        }
        assert!(check_asset("shp", b"\x01\x02binary", false).is_err());
        assert!(check_asset("shp", b"Key 1\n", false).is_ok());
        assert!(check_asset("jpg", b"anything", false).is_ok());
    }
}
