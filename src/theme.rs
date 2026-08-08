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

        let r = |field: Option<&Option<String>>, fallback: Color| -> Color {
            field
                .and_then(|opt| opt.as_ref())
                .and_then(|name| resolve_color(name, p))
                .unwrap_or(fallback)
        };

        Theme {
            text: r(ui.map(|u| &u.text), base.text),
            text_dim: r(ui.map(|u| &u.text_dim), base.text_dim),
            text_muted: r(ui.map(|u| &u.text_muted), base.text_muted),
            author: r(ui.map(|u| &u.author), base.author),
            highlight: r(ui.map(|u| &u.highlight), base.highlight),
            link: r(ui.map(|u| &u.link), base.link),
            date: r(ui.map(|u| &u.date), base.date),
            border: r(ui.map(|u| &u.border), base.border),
            selection: r(ui.map(|u| &u.selection), base.selection),
            popup_bg: r(ui.map(|u| &u.popup_bg), base.popup_bg),
            popup_border: r(ui.map(|u| &u.popup_border), base.popup_border),
            normal_bg: r(ui.map(|u| &u.normal_bg), base.normal_bg),
            insert_bg: r(ui.map(|u| &u.insert_bg), base.insert_bg),
            status_fg: r(ui.map(|u| &u.status_fg), base.status_fg),
        }
    }
}

pub fn load_theme(config_theme: Option<&str>) -> Theme {
    let theme_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("grimoire")
        .join("themes");

    load_theme_from_dir(&theme_dir, config_theme)
}

fn load_theme_from_dir(theme_dir: &Path, config_theme: Option<&str>) -> Theme {
    let base = Theme::default();
    let Some(name) = config_theme else {
        return base;
    };

    let theme_file = theme_dir.join(format!("{}.toml", name));
    if let Ok(contents) = std::fs::read_to_string(&theme_file)
        && let Ok(cfg) = toml::from_str::<ThemeConfig>(&contents)
    {
        return cfg.resolve(&base);
    }

    base
}

#[cfg(test)]
mod tests {
    use super::{Theme, load_theme_from_dir};
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
}
