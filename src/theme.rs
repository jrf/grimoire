use ratatui::style::Color;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub text: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub author: Color,
    pub highlight: Color,
    pub link: Color,
    pub date: Color,
    pub border: Color,
    pub selection: Color,
    pub popup_bg: Color,
    pub popup_border: Color,
    pub normal_bg: Color,
    pub insert_bg: Color,
    pub status_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Color::Reset,
            text_dim: Color::Reset,
            text_muted: Color::Reset,
            author: Color::Reset,
            highlight: Color::Reset,
            link: Color::Reset,
            date: Color::Reset,
            border: Color::Reset,
            selection: Color::Reset,
            popup_bg: Color::Reset,
            popup_border: Color::Reset,
            normal_bg: Color::Reset,
            insert_bg: Color::Reset,
            status_fg: Color::Reset,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ThemeConfig {
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub ui: Option<UiConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UiConfig {
    pub text: Option<String>,
    pub text_dim: Option<String>,
    pub text_muted: Option<String>,
    pub author: Option<String>,
    pub highlight: Option<String>,
    pub link: Option<String>,
    pub date: Option<String>,
    pub border: Option<String>,
    pub selection: Option<String>,
    pub cursor_bg: Option<String>,
    pub popup_bg: Option<String>,
    pub popup_border: Option<String>,
    pub normal_bg: Option<String>,
    pub insert_bg: Option<String>,
    pub status_fg: Option<String>,
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn resolve_color(name: &str, palette: &BTreeMap<String, String>) -> Option<Color> {
    palette.get(name).and_then(|hex| parse_hex(hex))
}

impl ThemeConfig {
    pub fn resolve(&self, base: &Theme) -> Theme {
        let p = &self.colors;
        let ui = self.ui.as_ref();

        let r = |field: Option<&Option<String>>, names: &[&str], fallback: Color| -> Color {
            field
                .and_then(|opt| opt.as_ref())
                .and_then(|name| resolve_color(name, p))
                .or_else(|| names.iter().find_map(|name| resolve_color(name, p)))
                .unwrap_or(fallback)
        };
        let selection_field = ui.map(|u| {
            if u.cursor_bg.is_some() {
                &u.cursor_bg
            } else {
                &u.selection
            }
        });

        Theme {
            text: r(ui.map(|u| &u.text), &["fg", "text"], base.text),
            text_dim: r(
                ui.map(|u| &u.text_dim),
                &["comment", "fg_dim", "subtext0"],
                base.text_dim,
            ),
            text_muted: r(
                ui.map(|u| &u.text_muted),
                &["fg_muted", "overlay0"],
                base.text_muted,
            ),
            author: r(
                ui.map(|u| &u.author),
                &["magenta", "mauve", "pink"],
                base.author,
            ),
            highlight: r(
                ui.map(|u| &u.highlight),
                &["yellow", "peach"],
                base.highlight,
            ),
            link: r(ui.map(|u| &u.link), &["blue", "sapphire"], base.link),
            date: r(ui.map(|u| &u.date), &["green", "teal"], base.date),
            border: r(
                ui.map(|u| &u.border),
                &["fg_gutter", "fg_muted", "surface1"],
                base.border,
            ),
            selection: r(
                selection_field,
                &["bg_highlight", "surface0", "surface1", "blue", "fg_bright"],
                base.selection,
            ),
            popup_bg: r(
                ui.map(|u| &u.popup_bg),
                &["bg_dark", "mantle", "bg"],
                base.popup_bg,
            ),
            popup_border: r(
                ui.map(|u| &u.popup_border),
                &["blue7", "lavender", "blue"],
                base.popup_border,
            ),
            normal_bg: r(
                ui.map(|u| &u.normal_bg),
                &["blue", "sapphire"],
                base.normal_bg,
            ),
            insert_bg: r(ui.map(|u| &u.insert_bg), &["green", "teal"], base.insert_bg),
            status_fg: r(
                ui.map(|u| &u.status_fg),
                &["bg_dark1", "crust", "bg"],
                base.status_fg,
            ),
        }
    }
}

pub fn load_theme(config_theme: Option<&str>) -> Theme {
    let home = dirs::home_dir().unwrap_or_default();
    let path = config_theme.map(|path| expand_home(&home, path));
    load_theme_path(path.as_deref())
}

#[cfg(test)]
fn load_theme_from_dir(theme_dir: &Path, config_theme: Option<&str>) -> Theme {
    let path = config_theme.map(|name| theme_dir.join(format!("{name}.toml")));
    load_theme_path(path.as_deref())
}

fn load_theme_path(path: Option<&Path>) -> Theme {
    let base = Theme::default();
    let Some(path) = path else {
        return base;
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return base;
    };
    let Ok(config) = toml::from_str::<ThemeConfig>(&contents) else {
        return base;
    };
    config.resolve(&base)
}

pub fn catalog_entries(configured_catalog: Option<&str>) -> Vec<(String, String)> {
    let Some(configured_catalog) = configured_catalog else {
        return Vec::new();
    };
    let home = dirs::home_dir().unwrap_or_default();
    let catalog_path = expand_home(&home, configured_catalog);
    let Ok(contents) = std::fs::read_to_string(catalog_path) else {
        return Vec::new();
    };
    let Ok(catalog) = contents.parse::<toml::Value>() else {
        return Vec::new();
    };
    catalog
        .get("themes")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(|path| {
            let expanded = expand_home(&home, path);
            (theme_name(&expanded), path.to_string())
        })
        .collect()
}

fn expand_home(home: &Path, configured_path: &str) -> std::path::PathBuf {
    configured_path
        .strip_prefix("~/")
        .map(|rest| home.join(rest))
        .unwrap_or_else(|| std::path::PathBuf::from(configured_path))
}

fn theme_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("theme")
        .replace('-', " ")
}

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeConfig, catalog_entries, load_theme_from_dir};
    use ratatui::style::Color;

    #[test]
    fn no_configured_theme_uses_terminal_colors() {
        let theme_dir = tempfile::tempdir().unwrap();

        assert_eq!(
            load_theme_from_dir(theme_dir.path(), None),
            Theme::default()
        );
    }

    #[test]
    fn selected_theme_is_loaded_from_the_config_directory() {
        let theme_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            theme_dir.path().join("synthetic.toml"),
            r##"
[colors]
foreground = "#123456"
accent = "#abcdef"

[ui]
text = "foreground"
highlight = "accent"
"##,
        )
        .unwrap();

        let theme = load_theme_from_dir(theme_dir.path(), Some("synthetic"));
        assert_eq!(theme.text, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(theme.highlight, Color::Rgb(0xab, 0xcd, 0xef));
        assert_eq!(theme.author, Color::Reset);
    }

    #[test]
    fn missing_or_invalid_theme_uses_terminal_colors() {
        let theme_dir = tempfile::tempdir().unwrap();
        std::fs::write(theme_dir.path().join("invalid.toml"), "not toml").unwrap();

        assert_eq!(
            load_theme_from_dir(theme_dir.path(), Some("missing")),
            Theme::default()
        );
        assert_eq!(
            load_theme_from_dir(theme_dir.path(), Some("invalid")),
            Theme::default()
        );
    }

    #[test]
    fn catalog_contains_only_explicit_theme_paths() {
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().join("catalog.toml");
        let synthetic = root.path().join("synthetic-theme.toml");
        std::fs::write(
            &catalog,
            format!("themes = [\"{}\"]\n", synthetic.display()),
        )
        .unwrap();

        assert_eq!(
            catalog_entries(catalog.to_str()),
            vec![(
                "synthetic theme".to_string(),
                synthetic.display().to_string()
            )]
        );
    }

    #[test]
    fn shared_palette_names_fill_grimoire_roles() {
        let config: ThemeConfig = toml::from_str(
            r##"
[colors]
fg = "#cdd6f4"
comment = "#6c7086"
mauve = "#cba6f7"
yellow = "#f9e2af"
blue = "#89b4fa"
green = "#a6e3a1"
surface1 = "#45475a"
mantle = "#181825"
crust = "#11111b"
"##,
        )
        .unwrap();
        let theme = config.resolve(&Theme::default());

        assert_eq!(theme.text, Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(theme.author, Color::Rgb(0xcb, 0xa6, 0xf7));
        assert_eq!(theme.selection, Color::Rgb(0x45, 0x47, 0x5a));
        assert_eq!(theme.insert_bg, Color::Rgb(0xa6, 0xe3, 0xa1));
    }

    #[test]
    fn cursor_background_takes_precedence_for_selection() {
        let config: ThemeConfig = toml::from_str(
            r##"
[colors]
blue = "#82aaff"
bg_highlight = "#2f334d"

[ui]
selection = "blue"
cursor_bg = "bg_highlight"
"##,
        )
        .unwrap();

        let theme = config.resolve(&Theme::default());
        assert_eq!(theme.selection, Color::Rgb(0x2f, 0x33, 0x4d));
    }
}
