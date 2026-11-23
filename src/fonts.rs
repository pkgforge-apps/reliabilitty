//! Fast, lazy font discovery on top of `fontdb`.
//!
//! `fontdb::Database::load_system_fonts()` mmaps and parses every font file on
//! the host (~700+ files on a typical Linux desktop), which dominates startup
//! and short-run CPU time. `FontIndex` first loads only the files whose path
//! matches the requested family names (resolving generic aliases like
//! `monospace` from fontconfig XML), and defers the full system scan until a
//! fallback or unindexed family actually needs it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use fontdb::Database;

/// Cached font database that starts with a targeted subset of system fonts
/// and upgrades to a full `fontdb` scan only on demand.
pub struct FontIndex {
    database: Database,
    full_loaded: bool,
    dirs: Vec<PathBuf>,
    generic_aliases: HashMap<String, Vec<String>>,
    /// Normalized filename stem -> full path, populated cheaply via `read_dir`
    /// without opening or mmapping any font file.
    file_index: Option<Vec<(String, PathBuf)>>,
    loaded_files: HashSet<PathBuf>,
}

impl FontIndex {
    pub fn empty() -> Self {
        Self {
            database: Database::new(),
            full_loaded: false,
            dirs: Vec::new(),
            generic_aliases: HashMap::new(),
            file_index: None,
            loaded_files: HashSet::new(),
        }
    }

    /// Creates an index initialized with fontconfig directories and aliases.
    /// Actual font files are loaded lazily when families are requested.
    pub fn new() -> Self {
        let mut index = Self::empty();
        index.parse_fontconfig_hints();
        index.apply_aliases_to_database();
        index
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn database_mut(&mut self) -> &mut Database {
        &mut self.database
    }

    /// Loads any font files whose filename matches `family` (or its fontconfig
    /// generic aliases). Returns true if at least one new file was loaded.
    pub fn load_matching_family(&mut self, family: &str) -> bool {
        if self.full_loaded {
            return false;
        }
        let targets = self.expand_family_names(family);
        let normalized_targets: Vec<String> = targets
            .iter()
            .map(|name| normalize_token(name))
            .filter(|token| !token.is_empty())
            .collect();
        if normalized_targets.is_empty() {
            return false;
        }
        self.ensure_file_index();
        let mut loaded_any = false;
        let prefer_generic = is_generic_family(family);
        if prefer_generic {
            // Walk aliases in preference order and stop once a family has any
            // matching face already loaded or newly loaded, so repeated calls
            // for `monospace` do not keep pulling in fallback families.
            for (raw_name, target) in targets.iter().zip(&normalized_targets) {
                let mut any_match = false;
                let mut to_load = Vec::new();
                if let Some(entries) = self.file_index.as_ref() {
                    for (stem, path) in entries {
                        if stem.contains(target.as_str()) {
                            any_match = true;
                            if !self.loaded_files.contains(path) {
                                to_load.push(path.clone());
                            }
                        }
                    }
                }
                if !any_match {
                    continue;
                }
                for path in to_load {
                    if self.loaded_files.insert(path.clone())
                        && self.database.load_font_file(&path).is_ok()
                    {
                        loaded_any = true;
                    }
                }
                if self.has_family(raw_name) {
                    self.set_generic_family(family, raw_name);
                    break;
                }
            }
            return loaded_any;
        }
        let matches: Vec<PathBuf> = self
            .file_index
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(stem, path)| {
                        !self.loaded_files.contains(path)
                            && normalized_targets
                                .iter()
                                .any(|target| stem.contains(target.as_str()))
                    })
                    .map(|(_, path)| path.clone())
                    .collect()
            })
            .unwrap_or_default();

        for path in matches {
            if self.loaded_files.insert(path.clone()) && self.database.load_font_file(&path).is_ok()
            {
                loaded_any = true;
            }
        }
        loaded_any
    }

    fn has_family(&self, family: &str) -> bool {
        self.database.faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        })
    }

    fn set_generic_family(&mut self, generic: &str, name: &str) {
        match generic.trim().to_ascii_lowercase().as_str() {
            "monospace" => self.database.set_monospace_family(name),
            "sans-serif" | "sans serif" | "sans" => self.database.set_sans_serif_family(name),
            "serif" => self.database.set_serif_family(name),
            "cursive" => self.database.set_cursive_family(name),
            "fantasy" => self.database.set_fantasy_family(name),
            _ => {}
        }
    }

    /// Upgrades to a full `fontdb` scan of every system font. No-op once done.
    pub fn ensure_full(&mut self) {
        if self.full_loaded {
            return;
        }
        self.full_loaded = true;
        self.database.load_system_fonts();
    }

    fn expand_family_names(&self, family: &str) -> Vec<String> {
        let lower = family.trim().to_ascii_lowercase();
        let canonical = match lower.as_str() {
            "sans" | "sans serif" => "sans-serif",
            other => other,
        };
        if let Some(aliases) = self.generic_aliases.get(canonical)
            && !aliases.is_empty()
        {
            return aliases.clone();
        }
        if canonical == "monospace" {
            return vec![
                "Noto Sans Mono".into(),
                "DejaVu Sans Mono".into(),
                "Liberation Mono".into(),
                "Adwaita Mono".into(),
                "Droid Sans Mono".into(),
                "Nimbus Mono".into(),
                "FreeMono".into(),
                "Courier".into(),
                "mono".into(),
            ];
        }
        vec![family.to_string()]
    }

    fn apply_aliases_to_database(&mut self) {
        for (generic, list) in &self.generic_aliases {
            let Some(first) = list.first() else {
                continue;
            };
            match generic.as_str() {
                "monospace" => self.database.set_monospace_family(first),
                "sans-serif" => self.database.set_sans_serif_family(first),
                "serif" => self.database.set_serif_family(first),
                "cursive" => self.database.set_cursive_family(first),
                "fantasy" => self.database.set_fantasy_family(first),
                _ => {}
            }
        }
    }

    fn ensure_file_index(&mut self) {
        if self.file_index.is_some() {
            return;
        }
        let mut entries = Vec::new();
        let mut seen_dirs = HashSet::new();
        for dir in &self.dirs {
            walk_font_names(dir, &mut seen_dirs, &mut entries);
        }
        self.file_index = Some(entries);
    }

    fn parse_fontconfig_hints(&mut self) {
        let home = std::env::var("HOME").ok();
        let mut dirs = Vec::new();
        let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
        let mut visited = HashSet::new();

        if let Ok(config_file) = std::env::var("FONTCONFIG_FILE") {
            parse_fc_file(
                Path::new(&config_file),
                home.as_deref(),
                &mut dirs,
                &mut aliases,
                &mut visited,
            );
        } else {
            let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_deref().map(|h| Path::new(h).join(".config")));
            let mut loaded_user = false;
            if let Some(ref xdg) = xdg_config_home {
                let user_conf = xdg.join("fontconfig/fonts.conf");
                if user_conf.is_file() {
                    parse_fc_file(
                        &user_conf,
                        home.as_deref(),
                        &mut dirs,
                        &mut aliases,
                        &mut visited,
                    );
                    loaded_user = true;
                }
            }
            if !loaded_user {
                parse_fc_file(
                    Path::new("/etc/fonts/local.conf"),
                    home.as_deref(),
                    &mut dirs,
                    &mut aliases,
                    &mut visited,
                );
            }
            parse_fc_file(
                Path::new("/etc/fonts/fonts.conf"),
                home.as_deref(),
                &mut dirs,
                &mut aliases,
                &mut visited,
            );
        }

        if dirs.is_empty() {
            dirs.push(PathBuf::from("/usr/share/fonts"));
            dirs.push(PathBuf::from("/usr/local/share/fonts"));
            if let Some(ref home) = home {
                dirs.push(Path::new(home).join(".local/share/fonts"));
                dirs.push(Path::new(home).join(".fonts"));
            }
        }
        self.dirs = dirs;
        self.generic_aliases = aliases;
    }
}

fn parse_fc_file(
    path: &Path,
    home: Option<&str>,
    dirs: &mut Vec<PathBuf>,
    aliases: &mut HashMap<String, Vec<String>>,
    visited: &mut HashSet<PathBuf>,
) {
    if !visited.insert(path.to_path_buf()) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("/etc/fonts"));
    let mut rest = text.as_str();
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => {
                    rest = &rest[end + 3..];
                    continue;
                }
                None => break,
            }
        }
        if let Some((attrs, body, after)) = take_tag(rest, "dir") {
            let trimmed = body.trim();
            if !trimmed.is_empty()
                && let Some(resolved) = resolve_fc_dir(attrs, trimmed, home)
            {
                dirs.push(resolved);
            }
            rest = after;
            continue;
        }
        if let Some((attrs, body, after)) = take_tag(rest, "include") {
            let trimmed = body.trim();
            if !trimmed.is_empty()
                && let Some(target) = resolve_fc_include(attrs, trimmed, parent, home)
            {
                if target.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&target) {
                        let mut paths: Vec<PathBuf> = entries
                            .flatten()
                            .map(|entry| entry.path())
                            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("conf"))
                            .collect();
                        paths.sort();
                        for child in paths {
                            parse_fc_file(&child, home, dirs, aliases, visited);
                        }
                    }
                } else if target.is_file() {
                    parse_fc_file(&target, home, dirs, aliases, visited);
                }
            }
            rest = after;
            continue;
        }
        if let Some((_, body, after)) = take_tag(rest, "alias") {
            parse_fc_alias(body, aliases);
            rest = after;
            continue;
        }
        rest = &rest[1..];
    }
}

fn take_tag<'a>(text: &'a str, tag: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let after_lt = text.strip_prefix('<')?;
    let after_name = after_lt.strip_prefix(tag)?;
    let first = after_name.as_bytes().first().copied()?;
    if !matches!(first, b'>' | b' ' | b'\t' | b'\r' | b'\n') {
        return None;
    }
    let open_end = after_name.find('>')?;
    let attrs = &after_name[..open_end];
    if attrs.ends_with('/') {
        return Some((attrs, "", &after_name[open_end + 1..]));
    }
    let body_start = &after_name[open_end + 1..];
    let close = format!("</{tag}>");
    let close_pos = body_start.find(&close)?;
    Some((
        attrs,
        &body_start[..close_pos],
        &body_start[close_pos + close.len()..],
    ))
}

fn parse_fc_alias(body: &str, aliases: &mut HashMap<String, Vec<String>>) {
    let Some(family_start) = body.find("<family") else {
        return;
    };
    let Some((_, alias_name, rest)) = take_tag(&body[family_start..], "family") else {
        return;
    };
    let generic = match alias_name.trim().to_ascii_lowercase().as_str() {
        "monospace" => "monospace",
        "sans-serif" | "sans serif" | "sans" => "sans-serif",
        "serif" => "serif",
        "cursive" => "cursive",
        "fantasy" => "fantasy",
        _ => return,
    };
    let mut names = Vec::new();
    for section in ["prefer", "accept", "default"] {
        let needle = format!("<{section}");
        if let Some(pos) = rest.find(&needle)
            && let Some((_, section_body, _)) = take_tag(&rest[pos..], section)
        {
            let mut scan = section_body;
            while let Some(fpos) = scan.find("<family") {
                if let Some((_, value, next)) = take_tag(&scan[fpos..], "family") {
                    let value = value.trim();
                    if !value.is_empty() {
                        names.push(value.to_string());
                    }
                    scan = next;
                } else {
                    break;
                }
            }
        }
    }
    if !names.is_empty() {
        let entry = aliases.entry(generic.to_string()).or_default();
        for name in names {
            if !entry
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&name))
            {
                entry.push(name);
            }
        }
    }
}

fn resolve_fc_dir(attrs: &str, value: &str, home: Option<&str>) -> Option<PathBuf> {
    if attrs.contains("prefix=\"xdg\"") {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|h| Path::new(h).join(".local/share")))?;
        return Some(data_home.join(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.map(|h| Path::new(h).join(rest));
    }
    Some(PathBuf::from(value))
}

fn resolve_fc_include(
    attrs: &str,
    value: &str,
    parent: &Path,
    home: Option<&str>,
) -> Option<PathBuf> {
    if attrs.contains("prefix=\"xdg\"") {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|h| Path::new(h).join(".config")))?;
        return Some(config_home.join(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.map(|h| Path::new(h).join(rest));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(parent.join(path))
    }
}

fn walk_font_names(dir: &Path, seen_dirs: &mut HashSet<PathBuf>, out: &mut Vec<(String, PathBuf)>) {
    let Ok(canon) = std::fs::canonicalize(dir) else {
        return;
    };
    if !seen_dirs.insert(canon) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_font_names(&path, seen_dirs, out);
        } else if file_type.is_symlink() {
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.is_dir() {
                    walk_font_names(&path, seen_dirs, out);
                } else if meta.is_file() && is_font_path(&path) {
                    push_font_entry(&path, out);
                }
            }
        } else if file_type.is_file() && is_font_path(&path) {
            push_font_entry(&path, out);
        }
    }
}

fn is_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("ttf")
                || ext.eq_ignore_ascii_case("otf")
                || ext.eq_ignore_ascii_case("ttc")
                || ext.eq_ignore_ascii_case("otc")
        })
}

fn push_font_entry(path: &Path, out: &mut Vec<(String, PathBuf)>) {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let norm = normalize_token(stem);
        if !norm.is_empty() {
            out.push((norm, path.to_path_buf()));
        }
    }
}

fn is_generic_family(family: &str) -> bool {
    matches!(
        family.trim().to_ascii_lowercase().as_str(),
        "monospace" | "sans-serif" | "sans serif" | "sans" | "serif" | "cursive" | "fantasy"
    )
}

fn normalize_token(text: &str) -> String {
    text.bytes()
        .filter(|b| b.is_ascii_alphanumeric())
        .map(|b| b.to_ascii_lowercase() as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fontconfig_alias_preference() {
        let mut aliases = HashMap::new();
        parse_fc_alias(
            "<family>monospace</family><prefer><family>Noto Sans Mono</family><family>DejaVu Sans Mono</family></prefer>",
            &mut aliases,
        );
        assert_eq!(
            aliases.get("monospace").map(Vec::as_slice),
            Some(["Noto Sans Mono".to_string(), "DejaVu Sans Mono".to_string()].as_slice())
        );
    }

    #[test]
    fn normalizes_family_tokens() {
        assert_eq!(normalize_token("Noto Sans Mono"), "notosansmono");
        assert_eq!(
            normalize_token("NotoSansMono-Regular"),
            "notosansmonoregular"
        );
        assert!(
            normalize_token("NotoSansMono-Regular").contains(&normalize_token("Noto Sans Mono"))
        );
    }
}
