use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::style::ResetColor;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::config::Config as AppConfig;
use crate::index;
use crate::metadata;
use crate::model::Reference;
use crate::semantic::{self, SearchHit, SearchRanking};
use crate::storage;
use crate::theme::{self, Theme};
use crate::validate;

#[derive(Clone, Copy, PartialEq)]
enum SortMode {
    Name,
    Author,
    Year,
    Title,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            SortMode::Name => SortMode::Author,
            SortMode::Author => SortMode::Year,
            SortMode::Year => SortMode::Title,
            SortMode::Title => SortMode::Name,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SortMode::Name => "name",
            SortMode::Author => "author",
            SortMode::Year => "year",
            SortMode::Title => "title",
        }
    }

    /// The direction a field sorts in when first selected. Year reads best
    /// newest-first; the text fields read best A→Z.
    fn defaults_descending(self) -> bool {
        matches!(self, SortMode::Year)
    }
}

struct Entry {
    dir: PathBuf,
    dir_name: String,
    reference: Reference,
    display: String,
}

pub struct App {
    entries: Vec<Entry>,
    filtered_indices: Vec<usize>,
    filter: String,
    list_state: ListState,
    config: AppConfig,
    theme: Theme,
    mode: Mode,
    input_mode: InputMode,
    should_quit: bool,
    pending_output: Option<String>,
    tag_filter: Option<String>,
    all_tags: Vec<String>,
    tag_popup: Option<TagPopup>,
    theme_popup: Option<ThemePopup>,
    layout: LayoutMode,
    // Space toggles a full-screen abstract preview over any layout (Quick Look).
    preview_overlay: bool,
    flash: Option<(String, std::time::Instant)>,
    preview_scroll: u16,
    show_help: bool,
    list_height: usize,
    add_input: Option<String>,
    enrich_preview: Option<EnrichPreview>,
    enrich_rx: Option<mpsc::Receiver<Vec<EnrichItem>>>,
    sort_mode: SortMode,
    /// Flip the current sort field away from its default direction (`S` key).
    sort_reverse: bool,
    validate_popup: Option<ValidatePopup>,
    semantic_input: Option<String>,
    semantic_view: Option<SemanticView>,
    semantic_rx: Option<mpsc::Receiver<std::result::Result<SemanticLoad, String>>>,
}

struct SemanticView {
    query: String,
    results: Vec<SearchHit>,
    ranking: Option<SearchRanking>,
    total: usize,
    list_state: ListState,
}

struct SemanticLoad {
    ranking: SearchRanking,
    results: Vec<SearchHit>,
    total: usize,
}

struct ValidatePopup {
    summary: String,
    issues: Vec<String>,
    scroll: u16,
}

type FieldDiff = (String, String, String); // (field, old, new)
type EnrichItem = (PathBuf, Reference, Vec<FieldDiff>); // (entry dir, updated ref, diffs)

struct EnrichPreview {
    dir: PathBuf,
    updated: Reference,
    diffs: Vec<FieldDiff>,
    scroll: u16,
    batch_queue: Vec<EnrichItem>,
    applied: usize,
    skipped: usize,
}

struct TagPopup {
    filter: String,
    filtered_tags: Vec<String>,
    counts: std::collections::BTreeMap<String, usize>,
    total: usize,
    selected: usize,
    scroll: usize,
    prev_tag_filter: Option<String>,
}

impl TagPopup {
    fn new(all_tags: &[String], entries: &[Entry], current_tag_filter: &Option<String>) -> Self {
        let mut counts = std::collections::BTreeMap::new();
        for e in entries {
            for tag in &e.reference.tags {
                *counts.entry(tag.clone()).or_insert(0) += 1;
            }
        }
        let total = entries.len();
        let mut tags = vec!["(all)".to_string()];
        tags.extend(all_tags.iter().cloned());
        Self {
            filter: String::new(),
            filtered_tags: tags,
            counts,
            total,
            selected: 0,
            scroll: 0,
            prev_tag_filter: current_tag_filter.clone(),
        }
    }

    fn rebuild(&mut self, all_tags: &[String]) {
        let mut tags = vec!["(all)".to_string()];
        tags.extend(all_tags.iter().cloned());
        if self.filter.is_empty() {
            self.filtered_tags = tags;
        } else {
            let f = self.filter.to_lowercase();
            self.filtered_tags = tags
                .into_iter()
                .filter(|t| t.to_lowercase().contains(&f))
                .collect();
        }
        if self.selected >= self.filtered_tags.len() {
            self.selected = self.filtered_tags.len().saturating_sub(1);
        }
    }

    fn count_for(&self, tag: &str) -> usize {
        if tag == "(all)" {
            self.total
        } else {
            self.counts.get(tag).copied().unwrap_or(0)
        }
    }

    fn selected_as_filter(&self) -> Option<String> {
        match self.selected_tag() {
            Some("(all)") | None => None,
            Some(t) => Some(t.to_string()),
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if !self.filtered_tags.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered_tags.len() - 1);
        }
    }

    fn page_down(&mut self) {
        if !self.filtered_tags.is_empty() {
            self.selected = (self.selected + 20).min(self.filtered_tags.len() - 1);
        }
    }

    fn page_up(&mut self) {
        self.selected = self.selected.saturating_sub(20);
    }

    fn clamp_scroll(&mut self, visible: usize) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected - visible + 1;
        }
    }

    fn selected_tag(&self) -> Option<&str> {
        self.filtered_tags.get(self.selected).map(|s| s.as_str())
    }
}

struct ThemePopup {
    names: Vec<String>,
    paths: Vec<String>,
    selected: usize,
}

impl ThemePopup {
    fn new(catalog_path: Option<&str>) -> Self {
        let (names, paths) = theme::catalog_entries(catalog_path).into_iter().unzip();
        Self {
            names,
            paths,
            selected: 0,
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if !self.names.is_empty() {
            self.selected = (self.selected + 1).min(self.names.len() - 1);
        }
    }

    fn selected_path(&self) -> Option<&str> {
        self.paths.get(self.selected).map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LayoutMode {
    Wide,
    Tall,
    Auto,
    FullList,
}

impl LayoutMode {
    fn from_config(s: Option<&str>) -> Self {
        match s {
            Some("wide") => Self::Wide,
            Some("tall") => Self::Tall,
            Some("auto") => Self::Auto,
            Some("full" | "list") | None => Self::FullList,
            Some(_) => Self::FullList,
        }
    }

    /// Cycle order for the runtime layout toggle (the `L` key).
    fn next(self, width: u16, height: u16) -> Self {
        let current = match self {
            Self::Auto if width as u32 >= height as u32 * 2 => Self::Wide,
            Self::Auto => Self::Tall,
            concrete => concrete,
        };

        match current {
            Self::Wide => Self::Tall,
            Self::Tall => Self::FullList,
            Self::FullList => Self::Wide,
            Self::Auto => unreachable!(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Tall => "tall",
            Self::Auto => "auto",
            Self::FullList => "full",
        }
    }

    fn resolve(self, width: u16, height: u16) -> ResolvedLayout {
        match self {
            Self::Wide => ResolvedLayout::Wide,
            Self::Tall => ResolvedLayout::Tall,
            Self::FullList => ResolvedLayout::FullList,
            Self::Auto => {
                // Character cells are ~2x taller than wide, so scale height
                // to approximate pixel aspect ratio.
                if width as u32 >= height as u32 * 2 {
                    ResolvedLayout::Wide
                } else {
                    ResolvedLayout::Tall
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ResolvedLayout {
    Wide,
    Tall,
    FullList,
    FullPreview,
}

enum Mode {
    Browse,
    Cite { format: String },
}

#[derive(Clone, Copy, PartialEq)]
enum InputMode {
    Browse,
    Search,
}

pub fn browse(config: &AppConfig, library: &Path, initial_query: Option<&str>) -> Result<()> {
    let app = App::new(config, library, Mode::Browse, initial_query)?;
    if app.entries.is_empty() {
        println!("Library is empty. Use `grimoire add <file.pdf>` to import a paper.");
        return Ok(());
    }
    run_app(app)
}

pub fn cite(config: &AppConfig, library: &Path, format: &str) -> Result<()> {
    let app = App::new(
        config,
        library,
        Mode::Cite {
            format: format.to_string(),
        },
        None,
    )?;
    if app.entries.is_empty() {
        anyhow::bail!("Library is empty");
    }
    run_app(app)
}

fn run_app(mut app: App) -> Result<()> {
    let tty = File::options().read(true).write(true).open("/dev/tty")?;
    let mut tty_ctl = tty.try_clone()?;

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        if let Ok(mut f) = File::options().write(true).open("/dev/tty") {
            let _ = f.execute(LeaveAlternateScreen);
            let _ = f.execute(ResetColor);
            let _ = f.execute(Show);
        }
        prev_hook(info);
    }));

    tty_ctl.execute(EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let backend = CrosstermBackend::new(BufWriter::new(tty.try_clone()?));
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_event_loop(&mut terminal, &mut app, &mut tty_ctl);

    terminal::disable_raw_mode()?;
    tty_ctl.execute(LeaveAlternateScreen)?;
    tty_ctl.execute(ResetColor)?;
    tty_ctl.execute(Show)?;

    if let Some(output) = app.pending_output.take() {
        print!("{}", output);
    }
    result
}

type Term = Terminal<CrosstermBackend<BufWriter<File>>>;

fn run_event_loop(terminal: &mut Term, app: &mut App, tty_ctl: &mut File) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        if app.should_quit {
            return Ok(());
        }

        // Check for background enrich results
        if let Some(ref rx) = app.enrich_rx {
            match rx.try_recv() {
                Ok(items) => {
                    app.enrich_rx = None;
                    if items.is_empty() {
                        app.flash =
                            Some(("Nothing to enrich".to_string(), std::time::Instant::now()));
                    } else {
                        let mut queue = items;
                        let (dir, updated, diffs) = queue.remove(0);
                        app.jump_to_entry(&dir);
                        app.enrich_preview = Some(EnrichPreview {
                            dir,
                            updated,
                            diffs,
                            scroll: 0,
                            batch_queue: queue,
                            applied: 0,
                            skipped: 0,
                        });
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.enrich_rx = None;
                    app.flash = Some(("Enrich failed".to_string(), std::time::Instant::now()));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        if let Some(ref rx) = app.semantic_rx {
            match rx.try_recv() {
                Ok(Ok(load)) => {
                    app.semantic_rx = None;
                    if let Some(view) = app.semantic_view.as_mut() {
                        view.results = load.results;
                        view.ranking = Some(load.ranking);
                        view.total = load.total;
                        view.list_state
                            .select((!view.results.is_empty()).then_some(0));
                    }
                    app.preview_scroll = 0;
                    continue;
                }
                Ok(Err(error)) => {
                    app.semantic_rx = None;
                    app.flash = Some((error, std::time::Instant::now()));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.semantic_rx = None;
                    app.flash = Some((
                        "Semantic search failed".to_string(),
                        std::time::Instant::now(),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        let timeout = if app.flash.is_some() || app.enrich_rx.is_some() || app.semantic_rx.is_some()
        {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_secs(60)
        };
        if !event::poll(timeout)? {
            if app.enrich_rx.is_some() {
                // Keep the flash alive while fetching
                app.flash = Some(("Fetching...".to_string(), std::time::Instant::now()));
            } else if app.flash_message().is_none() {
                app.flash = None;
            }
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if app.show_help {
                app.show_help = false;
                continue;
            }

            if let Some(ref mut vp) = app.validate_popup {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                        app.validate_popup = None;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        vp.scroll = vp.scroll.saturating_add(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        vp.scroll = vp.scroll.saturating_sub(1);
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        vp.scroll = vp.scroll.saturating_add(10);
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        vp.scroll = vp.scroll.saturating_sub(10);
                    }
                    _ => {}
                }
                continue;
            }

            if app.semantic_input.is_some() {
                match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) => app.semantic_input = None,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => app.should_quit = true,
                    (KeyCode::Enter, _) => app.submit_semantic(),
                    (KeyCode::Backspace, _) => {
                        if let Some(input) = app.semantic_input.as_mut() {
                            input.pop();
                        }
                    }
                    (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        if let Some(input) = app.semantic_input.as_mut() {
                            input.push(c);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if app.add_input.is_some() {
                match key.code {
                    KeyCode::Esc => {
                        app.add_input = None;
                    }
                    KeyCode::Enter => {
                        app.submit_add();
                    }
                    KeyCode::Tab => {
                        if let Some(text) = app.add_input.take() {
                            app.filter = text;
                            app.rebuild_filter();
                        }
                        app.input_mode = InputMode::Search;
                    }
                    KeyCode::Char('a')
                        if key.modifiers.contains(KeyModifiers::ALT)
                            || key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let Some(text) = app.add_input.take() {
                            app.filter = text;
                            app.rebuild_filter();
                        }
                        app.input_mode = InputMode::Search;
                    }
                    KeyCode::Backspace => {
                        if let Some(ref mut s) = app.add_input {
                            s.pop();
                        }
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        if let Some(ref mut s) = app.add_input {
                            s.push(c);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if app.enrich_preview.is_some() {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('y') => {
                        app.apply_enrich();
                    }
                    KeyCode::Char('n') | KeyCode::Char('s') => {
                        app.skip_enrich();
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        app.finish_enrich();
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if let Some(ref mut ep) = app.enrich_preview {
                            ep.scroll = ep.scroll.saturating_add(1);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if let Some(ref mut ep) = app.enrich_preview {
                            ep.scroll = ep.scroll.saturating_sub(1);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if app.tag_popup.is_some() {
                match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) => {
                        let prev = app.tag_popup.as_ref().unwrap().prev_tag_filter.clone();
                        app.tag_popup = None;
                        app.tag_filter = prev;
                        app.rebuild_filter();
                    }
                    (KeyCode::Up, _) => {
                        app.tag_popup.as_mut().unwrap().move_up();
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Down, _) => {
                        app.tag_popup.as_mut().unwrap().move_down();
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                        app.tag_popup.as_mut().unwrap().page_down();
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                        app.tag_popup.as_mut().unwrap().page_up();
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Backspace, _) => {
                        let popup = app.tag_popup.as_mut().unwrap();
                        popup.filter.pop();
                        popup.rebuild(&app.all_tags);
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Char(c), _) => {
                        let popup = app.tag_popup.as_mut().unwrap();
                        popup.filter.push(c);
                        popup.rebuild(&app.all_tags);
                        app.tag_filter = app.tag_popup.as_ref().unwrap().selected_as_filter();
                        app.rebuild_filter();
                    }
                    (KeyCode::Enter, _) => {
                        app.tag_popup = None;
                    }
                    _ => {}
                }
                continue;
            }

            if app.theme_popup.is_some() {
                match key.code {
                    KeyCode::Esc => {
                        app.theme_popup = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.theme_popup.as_mut().unwrap().move_up();
                        if let Some(path) = app.theme_popup.as_ref().unwrap().selected_path() {
                            app.theme = theme::load_theme(Some(path));
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.theme_popup.as_mut().unwrap().move_down();
                        if let Some(path) = app.theme_popup.as_ref().unwrap().selected_path() {
                            app.theme = theme::load_theme(Some(path));
                        }
                    }
                    KeyCode::Enter => {
                        app.theme_popup = None;
                    }
                    _ => {}
                }
                continue;
            }

            match app.input_mode {
                InputMode::Search => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) => {
                        app.input_mode = InputMode::Browse;
                    }
                    (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                        app.semantic_input = Some(app.filter.clone());
                        app.filter.clear();
                        app.rebuild_filter();
                        app.input_mode = InputMode::Browse;
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => app.should_quit = true,

                    (KeyCode::Char('a'), KeyModifiers::ALT)
                    | (KeyCode::Char('a'), KeyModifiers::CONTROL)
                    | (KeyCode::Tab, _) => {
                        app.add_input = Some(app.filter.clone());
                        app.filter.clear();
                        app.rebuild_filter();
                        app.input_mode = InputMode::Browse;
                    }

                    (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        app.filter.push(c);
                        app.rebuild_filter();
                    }
                    (KeyCode::Backspace, _) => {
                        app.filter.pop();
                        app.rebuild_filter();
                    }

                    (KeyCode::Up, _)
                    | (KeyCode::Char('p'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('k'), KeyModifiers::CONTROL) => app.move_up(),
                    (KeyCode::Down, _)
                    | (KeyCode::Char('n'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('j'), KeyModifiers::CONTROL) => app.move_down(),
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.half_page_down(),
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.half_page_up(),
                    (KeyCode::Char('f'), KeyModifiers::CONTROL) => app.page_down(),
                    (KeyCode::Char('b'), KeyModifiers::CONTROL) => app.page_up(),

                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        app.tag_popup =
                            Some(TagPopup::new(&app.all_tags, &app.entries, &app.tag_filter));
                    }

                    (KeyCode::Enter, _) => {
                        app.input_mode = InputMode::Browse;
                    }

                    _ => {}
                },
                InputMode::Browse => match (key.code, key.modifiers) {
                    // Esc backs out of the full-screen preview before quitting.
                    (KeyCode::Esc, _) if app.preview_overlay => {
                        app.preview_overlay = false;
                    }
                    (KeyCode::Esc, _) if app.semantic_view.is_some() => {
                        app.clear_semantic();
                    }
                    (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                        app.should_quit = true;
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => app.should_quit = true,

                    (KeyCode::Char('v'), KeyModifiers::NONE)
                    | (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                        app.semantic_input = Some(
                            app.semantic_view
                                .as_ref()
                                .map(|view| view.query.clone())
                                .unwrap_or_default(),
                        );
                    }

                    (KeyCode::Char('/'), KeyModifiers::NONE)
                    | (KeyCode::Char('i'), KeyModifiers::NONE) => {
                        app.clear_semantic();
                        app.input_mode = InputMode::Search;
                    }

                    (KeyCode::Char('j'), KeyModifiers::NONE)
                    | (KeyCode::Down, _)
                    | (KeyCode::Char('n'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('j'), KeyModifiers::CONTROL) => app.move_down(),
                    (KeyCode::Char('k'), KeyModifiers::NONE)
                    | (KeyCode::Up, _)
                    | (KeyCode::Char('p'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('k'), KeyModifiers::CONTROL) => app.move_up(),
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.half_page_down(),
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.half_page_up(),
                    (KeyCode::Char('f'), KeyModifiers::CONTROL) => app.page_down(),
                    (KeyCode::Char('b'), KeyModifiers::CONTROL) => app.page_up(),
                    (KeyCode::Char('g'), KeyModifiers::NONE) => app.move_to_top(),
                    (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.move_to_bottom();
                    }

                    (KeyCode::Char('t'), KeyModifiers::NONE) => {
                        app.tag_popup =
                            Some(TagPopup::new(&app.all_tags, &app.entries, &app.tag_filter));
                    }
                    (KeyCode::Char('T'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.theme_popup =
                            Some(ThemePopup::new(app.config.theme_catalog.as_deref()));
                    }
                    (KeyCode::Char('L'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.preview_overlay = false;
                        let size = terminal.size()?;
                        app.layout = app.layout.next(size.width, size.height);
                        app.flash = Some((
                            format!("Layout: {}", app.layout.label()),
                            std::time::Instant::now(),
                        ));
                    }
                    (KeyCode::Char('c'), KeyModifiers::NONE) => {
                        app.clear_semantic();
                        app.filter.clear();
                        app.tag_filter = None;
                        app.rebuild_filter();
                    }

                    (KeyCode::Enter, _) => {
                        app.action_select()?;
                    }
                    // Space toggles the full-screen abstract in any layout
                    // (Quick Look); esc or space again closes it.
                    (KeyCode::Char(' '), _) => {
                        app.preview_overlay = !app.preview_overlay;
                        app.preview_scroll = 0;
                    }
                    (KeyCode::Char('e'), KeyModifiers::NONE) => {
                        app.action_edit(terminal, tty_ctl)?;
                    }
                    (KeyCode::Char('y'), KeyModifiers::NONE) => {
                        app.action_copy_bib()?;
                    }
                    (KeyCode::Char('o'), KeyModifiers::NONE) => {
                        app.action_open_url();
                    }
                    (KeyCode::Char('a'), KeyModifiers::NONE) => {
                        app.add_input = Some(String::new());
                    }
                    (KeyCode::Char('r'), KeyModifiers::NONE) => {
                        app.action_enrich_selected();
                    }
                    (KeyCode::Char('R'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.action_enrich_all();
                    }
                    (KeyCode::Char('d'), KeyModifiers::NONE) => {
                        run_dedup(terminal, app)?;
                        // Dedup moves directories on disk; refresh the in-memory
                        // list so removed entries stop showing in the browser.
                        app.reload_entries();
                    }
                    (KeyCode::Char('I'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.action_reindex();
                    }
                    (KeyCode::Char('V'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.action_validate();
                    }
                    (KeyCode::Char('s'), KeyModifiers::NONE)
                        if app.filter.is_empty() && app.semantic_view.is_none() =>
                    {
                        app.sort_mode = app.sort_mode.next();
                        app.sort_reverse = false;
                        app.rebuild_filter();
                    }
                    (KeyCode::Char('S'), KeyModifiers::SHIFT | KeyModifiers::NONE)
                        if app.filter.is_empty() && app.semantic_view.is_none() =>
                    {
                        app.sort_reverse = !app.sort_reverse;
                        app.rebuild_filter();
                    }

                    (KeyCode::Char('J'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.preview_scroll = app.preview_scroll.saturating_add(3);
                    }
                    (KeyCode::Char('K'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.preview_scroll = app.preview_scroll.saturating_sub(3);
                    }
                    (KeyCode::Char('?'), _) => {
                        app.show_help = true;
                    }

                    _ => {}
                },
            }
        }
    }
}

impl App {
    fn new(
        config: &AppConfig,
        library: &Path,
        mode: Mode,
        initial_query: Option<&str>,
    ) -> Result<Self> {
        let dirs = storage::list_ref_dirs(library)?;
        let entries: Vec<Entry> = dirs
            .into_iter()
            .filter_map(|dir| {
                let dir_name = dir.file_name()?.to_string_lossy().to_string();
                let reference = metadata::read_info(&dir).ok()?;
                let authors = if reference.authors.is_empty() {
                    String::new()
                } else if reference.authors.len() == 1 {
                    reference.authors[0].clone()
                } else {
                    format!("{} et al.", reference.authors[0])
                };
                let year = reference
                    .year
                    .map(|y| format!("({})", y))
                    .unwrap_or_default();
                let display = [authors, year, reference.title.clone()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
                Some(Entry {
                    dir,
                    dir_name,
                    reference,
                    display,
                })
            })
            .collect();

        let filter = initial_query.unwrap_or("").to_string();
        let filtered_indices: Vec<usize> = (0..entries.len()).collect();

        let theme = theme::load_theme(config.theme.as_deref());

        let mut tag_set = std::collections::BTreeSet::new();
        for e in &entries {
            for tag in &e.reference.tags {
                tag_set.insert(tag.clone());
            }
        }
        let all_tags: Vec<String> = tag_set.into_iter().collect();

        let mut app = App {
            entries,
            filtered_indices,
            filter,
            list_state: ListState::default(),
            config: config.clone(),
            theme,
            mode,
            input_mode: InputMode::Browse,
            should_quit: false,
            pending_output: None,
            tag_filter: None,
            all_tags,
            tag_popup: None,
            theme_popup: None,
            layout: LayoutMode::from_config(config.layout.as_deref()),
            preview_overlay: false,
            flash: None,
            preview_scroll: 0,
            show_help: false,
            list_height: 20,
            add_input: None,
            enrich_preview: None,
            enrich_rx: None,
            sort_mode: SortMode::Name,
            sort_reverse: false,
            validate_popup: None,
            semantic_input: None,
            semantic_view: None,
            semantic_rx: None,
        };

        if !app.filter.is_empty() {
            app.rebuild_filter();
        }
        if !app.filtered_indices.is_empty() {
            app.list_state.select(Some(0));
        }

        Ok(app)
    }

    fn rebuild_filter(&mut self) {
        let tag_filtered: Vec<usize> = if let Some(ref tag) = self.tag_filter {
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.reference.tags.iter().any(|t| t == tag))
                .map(|(i, _)| i)
                .collect()
        } else {
            (0..self.entries.len()).collect()
        };

        if self.filter.is_empty() {
            self.filtered_indices = tag_filtered;
            self.apply_sort();
        } else {
            let pattern = Pattern::parse(&self.filter, CaseMatching::Ignore, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();

            let mut scored: Vec<(usize, u32)> = tag_filtered
                .into_iter()
                .filter_map(|i| {
                    let haystack = Utf32Str::new(&self.entries[i].display, &mut buf);
                    pattern.score(haystack, &mut matcher).map(|s| (i, s))
                })
                .collect();
            scored.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
            self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        }

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.preview_scroll = 0;
    }

    fn submit_semantic(&mut self) {
        let Some(input) = self.semantic_input.take() else {
            return;
        };
        let query = input.trim().to_string();
        if query.is_empty() {
            self.flash = Some((
                "Semantic query cannot be empty".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }

        let library = self.config.library_dir();
        let limit = self.config.semantic_results();
        let embedding = self.config.embedding.clone();
        let worker_query = query.clone();
        let (tx, rx) = mpsc::channel();
        self.semantic_rx = Some(rx);
        self.semantic_view = Some(SemanticView {
            query,
            results: Vec::new(),
            ranking: None,
            total: 0,
            list_state: ListState::default(),
        });
        self.preview_overlay = false;
        self.preview_scroll = 0;

        std::thread::spawn(move || {
            let result = (|| {
                let ranking = semantic::rank_silent(&library, &worker_query, &embedding)?;
                let total = limit.map_or_else(|| ranking.total(), |cap| cap.min(ranking.total()));
                let page_size = semantic::DEFAULT_PAGE_SIZE.min(total).max(1);
                let page = ranking.page(&library, 0, page_size)?;
                Ok(SemanticLoad {
                    ranking,
                    results: page.hits,
                    total,
                })
            })()
            .map_err(|error: anyhow::Error| semantic_error_message(&error));
            let _ = tx.send(result);
        });
    }

    fn maybe_load_more_semantic(&mut self) {
        let library = self.config.library_dir();
        let result = {
            let Some(view) = self.semantic_view.as_mut() else {
                return;
            };
            let selected = view.list_state.selected().unwrap_or(0);
            if !should_load_more_semantic(selected, view.results.len(), view.total) {
                return;
            }
            let Some(ranking) = view.ranking.as_ref() else {
                return;
            };
            let offset = view.results.len();
            let page_size = semantic::DEFAULT_PAGE_SIZE.min(view.total - offset);
            ranking
                .page(&library, offset, page_size)
                .map(|page| view.results.extend(page.hits))
        };
        if let Err(error) = result {
            self.flash = Some((
                format!("Could not load more semantic results: {error}"),
                std::time::Instant::now(),
            ));
        }
    }

    fn clear_semantic(&mut self) {
        self.semantic_input = None;
        self.semantic_view = None;
        self.semantic_rx = None;
        self.preview_overlay = false;
        self.preview_scroll = 0;
    }

    fn apply_sort(&mut self) {
        let entries = &self.entries;
        let reverse = self.sort_reverse;
        self.filtered_indices.sort_by(|&a, &b| {
            let ea = &entries[a];
            let eb = &entries[b];
            let ordering = match self.sort_mode {
                SortMode::Name => ea.dir_name.cmp(&eb.dir_name),
                SortMode::Author => {
                    let last_name = |s: &str| -> String {
                        if s.contains(',') {
                            s.split(',').next().unwrap_or(s).trim().to_lowercase()
                        } else {
                            s.rsplit_once(' ')
                                .map(|(_, l)| l)
                                .unwrap_or(s)
                                .to_lowercase()
                        }
                    };
                    let aa = ea
                        .reference
                        .authors
                        .first()
                        .map(|s| last_name(s))
                        .unwrap_or_default();
                    let ba = eb
                        .reference
                        .authors
                        .first()
                        .map(|s| last_name(s))
                        .unwrap_or_default();
                    aa.cmp(&ba)
                }
                SortMode::Year => {
                    let ya = ea.reference.year.unwrap_or(0);
                    let yb = eb.reference.year.unwrap_or(0);
                    yb.cmp(&ya)
                }
                SortMode::Title => ea
                    .reference
                    .title
                    .to_lowercase()
                    .cmp(&eb.reference.title.to_lowercase()),
            };
            if reverse {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    fn selected_entry(&self) -> Option<&Entry> {
        if let Some(hit) = self.selected_semantic_hit() {
            return self
                .entries
                .iter()
                .find(|entry| entry.dir_name == hit.dir_name);
        }
        let selected = self.list_state.selected()?;
        let &idx = self.filtered_indices.get(selected)?;
        self.entries.get(idx)
    }

    fn selected_semantic_hit(&self) -> Option<&SearchHit> {
        let view = self.semantic_view.as_ref()?;
        view.results.get(view.list_state.selected()?)
    }

    fn move_up(&mut self) {
        if let Some(view) = self.semantic_view.as_mut() {
            let selected = view.list_state.selected().unwrap_or(0);
            view.list_state.select(Some(selected.saturating_sub(1)));
            self.preview_scroll = 0;
            return;
        }
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(0) | None => 0,
            Some(i) => i - 1,
        };
        self.list_state.select(Some(i));
        self.preview_scroll = 0;
    }

    fn move_down(&mut self) {
        if let Some(view) = self.semantic_view.as_mut() {
            if !view.results.is_empty() {
                let selected = view.list_state.selected().unwrap_or(0);
                view.list_state
                    .select(Some((selected + 1).min(view.results.len() - 1)));
            }
            self.preview_scroll = 0;
            self.maybe_load_more_semantic();
            return;
        }
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) if i >= self.filtered_indices.len() - 1 => self.filtered_indices.len() - 1,
            Some(i) => i + 1,
            None => 0,
        };
        self.list_state.select(Some(i));
        self.preview_scroll = 0;
    }

    fn scroll_up(&mut self, lines: usize) {
        if let Some(view) = self.semantic_view.as_mut() {
            let selected = view.list_state.selected().unwrap_or(0);
            view.list_state.select(Some(selected.saturating_sub(lines)));
            self.preview_scroll = 0;
            return;
        }
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(lines)));
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self, lines: usize) {
        if let Some(view) = self.semantic_view.as_mut() {
            if !view.results.is_empty() {
                let selected = view.list_state.selected().unwrap_or(0);
                view.list_state
                    .select(Some((selected + lines).min(view.results.len() - 1)));
            }
            self.preview_scroll = 0;
            self.maybe_load_more_semantic();
            return;
        }
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let max = self.filtered_indices.len() - 1;
        self.list_state.select(Some((i + lines).min(max)));
        self.preview_scroll = 0;
    }

    fn half_page_up(&mut self) {
        self.scroll_up(self.list_height / 2);
    }

    fn half_page_down(&mut self) {
        self.scroll_down(self.list_height / 2);
    }

    fn page_up(&mut self) {
        self.scroll_up(self.list_height);
    }

    fn page_down(&mut self) {
        self.scroll_down(self.list_height);
    }

    fn move_to_top(&mut self) {
        if let Some(view) = self.semantic_view.as_mut() {
            view.list_state
                .select((!view.results.is_empty()).then_some(0));
            self.preview_scroll = 0;
            return;
        }
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
            self.preview_scroll = 0;
        }
    }

    fn move_to_bottom(&mut self) {
        if let Some(view) = self.semantic_view.as_mut() {
            view.list_state
                .select((!view.results.is_empty()).then_some(view.results.len() - 1));
            self.preview_scroll = 0;
            self.maybe_load_more_semantic();
            return;
        }
        if !self.filtered_indices.is_empty() {
            self.list_state
                .select(Some(self.filtered_indices.len() - 1));
            self.preview_scroll = 0;
        }
    }

    fn action_select(&mut self) -> Result<()> {
        let semantic_page = self
            .selected_semantic_hit()
            .and_then(|hit| hit.pages.first().copied());
        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return Ok(()),
        };

        match &self.mode {
            Mode::Browse => {
                let pdf = if let Some(f) = entry.reference.files.first() {
                    let p = entry.dir.join(f);
                    if p.exists() { Some(p) } else { None }
                } else {
                    std::fs::read_dir(&entry.dir)
                        .ok()
                        .and_then(|rd| {
                            rd.flatten().find(|e| {
                                e.path()
                                    .extension()
                                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                            })
                        })
                        .map(|e| e.path())
                };
                match pdf {
                    Some(p) => match launch_reader(self.config.reader_command(), &p, semantic_page)
                    {
                        Ok(_) => {}
                        Err(e) => {
                            self.flash = Some((
                                format!("Failed to open PDF: {}", e),
                                std::time::Instant::now(),
                            ));
                        }
                    },
                    None => {
                        self.flash =
                            Some(("No PDF available".to_string(), std::time::Instant::now()));
                    }
                }
            }
            Mode::Cite { format } => {
                let key = entry.dir_name.clone();
                let output = match format.as_str() {
                    "latex" => format!("\\cite{{{}}}", key),
                    "typst" => format!("@{}", key),
                    _ => key,
                };
                self.pending_output = Some(output);
                self.should_quit = true;
            }
        }
        Ok(())
    }

    fn action_edit(&mut self, terminal: &mut Term, tty_ctl: &mut File) -> Result<()> {
        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return Ok(()),
        };
        let info_path = entry.dir.join("info.toml");
        let mut editor = self.config.editor_command()?;

        terminal::disable_raw_mode()?;
        tty_ctl.execute(LeaveAlternateScreen)?;

        let status = editor.arg(&info_path).status();

        tty_ctl.execute(EnterAlternateScreen)?;
        terminal::enable_raw_mode()?;
        terminal.clear()?;

        // A failure to launch the editor (e.g. a mis-configured $EDITOR) must
        // not tear down the whole TUI — report it and stay in the session.
        if let Err(e) = status {
            self.flash = Some((format!("Editor failed: {e}"), std::time::Instant::now()));
            return Ok(());
        }

        let idx = self.filtered_indices[self.list_state.selected().unwrap_or(0)];
        if let Ok(r) = metadata::read_info(&self.entries[idx].dir) {
            let authors = if r.authors.is_empty() {
                String::new()
            } else if r.authors.len() == 1 {
                r.authors[0].clone()
            } else {
                format!("{} et al.", r.authors[0])
            };
            let year = r.year.map(|y| format!("({})", y)).unwrap_or_default();
            let display = [authors, year, r.title.clone()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("  ");
            self.entries[idx].reference = r;
            self.entries[idx].display = display;
        }
        Ok(())
    }

    fn action_copy_bib(&mut self) -> Result<()> {
        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return Ok(()),
        };
        let r = &entry.reference;
        let bib = crate::export::to_bibtex(&entry.dir_name, r);

        match copy_to_clipboard(&bib) {
            Ok(()) => {
                self.flash = Some(("Copied BibTeX".to_string(), std::time::Instant::now()));
            }
            Err(e) => {
                self.flash = Some((format!("Copy failed: {}", e), std::time::Instant::now()));
            }
        }
        Ok(())
    }

    fn flash_message(&self) -> Option<&str> {
        self.flash.as_ref().and_then(|(msg, t)| {
            if t.elapsed().as_secs() < 2 {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    fn submit_add(&mut self) {
        let input = match self.add_input.take() {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                self.add_input = None;
                return;
            }
        };

        self.flash = Some(("Adding...".to_string(), std::time::Instant::now()));

        let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("grimoire"));
        let output = std::process::Command::new(bin)
            .arg("add")
            .arg(&input)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                // A successful run with no "Added:" line and a duplicate notice
                // means the import was skipped as a duplicate.
                let msg = stdout
                    .lines()
                    .find(|l| l.starts_with("Added:"))
                    .or_else(|| stderr.lines().find(|l| l.contains("duplicate of")))
                    .unwrap_or("Added successfully")
                    .to_string();
                self.flash = Some((msg, std::time::Instant::now()));
                self.reload_entries();
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.lines().last().unwrap_or("Add failed").to_string();
                self.flash = Some((msg, std::time::Instant::now()));
            }
            Err(e) => {
                self.flash = Some((format!("Error: {}", e), std::time::Instant::now()));
            }
        }
    }

    fn action_reindex(&mut self) {
        let library = self.config.library_dir();
        match index::Index::open(&library).and_then(|idx| idx.reindex(&library)) {
            Ok(count) => {
                self.reload_entries();
                self.flash = Some((
                    format!("Reindexed {} references", count),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.flash = Some((format!("Reindex error: {}", e), std::time::Instant::now()));
            }
        }
    }

    fn action_validate(&mut self) {
        let library = self.config.library_dir();
        match validate::validate(&library, true) {
            Ok(result) => {
                self.reload_entries();
                self.validate_popup = Some(ValidatePopup {
                    summary: result.summary(),
                    issues: result.issues,
                    scroll: 0,
                });
            }
            Err(e) => {
                self.flash = Some((format!("Validate error: {}", e), std::time::Instant::now()));
            }
        }
    }

    fn reload_entries(&mut self) {
        let library = self.config.library_dir();
        let dirs = match storage::list_ref_dirs(&library) {
            Ok(d) => d,
            Err(_) => return,
        };
        self.entries = dirs
            .into_iter()
            .filter_map(|dir| {
                let dir_name = dir.file_name()?.to_string_lossy().to_string();
                let reference = metadata::read_info(&dir).ok()?;
                let authors = if reference.authors.is_empty() {
                    String::new()
                } else if reference.authors.len() == 1 {
                    reference.authors[0].clone()
                } else {
                    format!("{} et al.", reference.authors[0])
                };
                let year = reference
                    .year
                    .map(|y| format!("({})", y))
                    .unwrap_or_default();
                let display = [authors, year, reference.title.clone()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
                Some(Entry {
                    dir,
                    dir_name,
                    reference,
                    display,
                })
            })
            .collect();

        let mut tag_set = std::collections::BTreeSet::new();
        for e in &self.entries {
            for tag in &e.reference.tags {
                tag_set.insert(tag.clone());
            }
        }
        self.all_tags = tag_set.into_iter().collect();
        self.rebuild_filter();
    }

    fn action_open_url(&mut self) {
        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return,
        };
        let r = &entry.reference;
        let url = if let Some(ref doi) = r.doi {
            format!("https://doi.org/{}", doi)
        } else if let Some(ref arxiv) = r.arxiv {
            format!("https://arxiv.org/abs/{}", arxiv)
        } else {
            self.flash = Some(("No DOI or arXiv ID".to_string(), std::time::Instant::now()));
            return;
        };
        match launch_detached(self.config.browser_command(), &url) {
            Ok(_) => {
                self.flash = Some(("Opened in browser".to_string(), std::time::Instant::now()));
            }
            Err(e) => {
                self.flash = Some((
                    format!("Failed to open browser: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn action_enrich_selected(&mut self) {
        if self.enrich_rx.is_some() {
            return;
        }
        let selected = match self.list_state.selected() {
            Some(s) => s,
            None => return,
        };
        let idx = self.filtered_indices[selected];
        let dir = self.entries[idx].dir.clone();
        let reference = self.entries[idx].reference.clone();

        self.flash = Some(("Fetching...".to_string(), std::time::Instant::now()));
        let (tx, rx) = mpsc::channel();
        self.enrich_rx = Some(rx);

        std::thread::spawn(move || {
            let result = match crate::enrich::enrich_entry(&dir, &reference) {
                Ok(Some(updated)) => {
                    let diffs = compute_diffs(&reference, &updated);
                    if diffs.is_empty() {
                        vec![]
                    } else {
                        vec![(dir, updated, diffs)]
                    }
                }
                _ => vec![],
            };
            let _ = tx.send(result);
        });
    }

    fn action_enrich_all(&mut self) {
        if self.enrich_rx.is_some() {
            return;
        }

        let work: Vec<(PathBuf, Reference)> = self
            .entries
            .iter()
            .filter(|e| needs_enrich(&e.reference))
            .map(|e| (e.dir.clone(), e.reference.clone()))
            .collect();

        if work.is_empty() {
            self.flash = Some(("Nothing to enrich".to_string(), std::time::Instant::now()));
            return;
        }

        self.flash = Some((
            format!("Fetching {} entries...", work.len()),
            std::time::Instant::now(),
        ));
        let (tx, rx) = mpsc::channel();
        self.enrich_rx = Some(rx);

        std::thread::spawn(move || {
            let mut items: Vec<EnrichItem> = Vec::new();
            for (dir, reference) in work {
                if let Ok(Some(updated)) = crate::enrich::enrich_entry(&dir, &reference) {
                    let diffs = compute_diffs(&reference, &updated);
                    if !diffs.is_empty() {
                        items.push((dir, updated, diffs));
                    }
                }
            }
            let _ = tx.send(items);
        });
    }

    fn apply_enrich(&mut self) {
        let ep = match self.enrich_preview.take() {
            Some(ep) => ep,
            None => return,
        };
        let library = self.config.library_dir();
        // Write to disk by directory path (stable), independent of the entry's
        // current position in the list — which may have shifted while the
        // background fetch was in flight.
        if let Err(error) = metadata::write_info(&ep.dir, &ep.updated)
            .and_then(|()| crate::index_reference(&library, &ep.dir, &ep.updated))
        {
            self.flash = Some((format!("Enrich failed: {error}"), std::time::Instant::now()));
            return;
        }
        if let Some(idx) = self.entries.iter().position(|e| e.dir == ep.dir) {
            self.update_entry_display(idx, ep.updated);
        }
        let applied = ep.applied + 1;
        self.advance_enrich_queue(ep.batch_queue, applied, ep.skipped);
    }

    fn skip_enrich(&mut self) {
        let ep = match self.enrich_preview.take() {
            Some(ep) => ep,
            None => return,
        };
        let skipped = ep.skipped + 1;
        self.advance_enrich_queue(ep.batch_queue, ep.applied, skipped);
    }

    fn finish_enrich(&mut self) {
        let ep = match self.enrich_preview.take() {
            Some(ep) => ep,
            None => return,
        };
        if ep.applied > 0 || ep.skipped > 0 {
            self.flash = Some((
                format!(
                    "Enriched {}, skipped {}",
                    ep.applied,
                    ep.skipped + ep.batch_queue.len() + 1
                ),
                std::time::Instant::now(),
            ));
        }
    }

    fn advance_enrich_queue(&mut self, mut queue: Vec<EnrichItem>, applied: usize, skipped: usize) {
        if queue.is_empty() {
            let msg = format!("Enriched {}, skipped {}", applied, skipped);
            self.flash = Some((msg, std::time::Instant::now()));
            return;
        }
        let (dir, updated, diffs) = queue.remove(0);
        self.jump_to_entry(&dir);
        self.enrich_preview = Some(EnrichPreview {
            dir,
            updated,
            diffs,
            scroll: 0,
            batch_queue: queue,
            applied,
            skipped,
        });
    }

    fn jump_to_entry(&mut self, dir: &Path) {
        let Some(entry_idx) = self.entries.iter().position(|e| e.dir.as_path() == dir) else {
            return;
        };
        if let Some(pos) = self.filtered_indices.iter().position(|&i| i == entry_idx) {
            self.list_state.select(Some(pos));
            self.preview_scroll = 0;
        }
    }

    fn update_entry_display(&mut self, idx: usize, r: Reference) {
        let authors = if r.authors.is_empty() {
            String::new()
        } else if r.authors.len() == 1 {
            r.authors[0].clone()
        } else {
            format!("{} et al.", r.authors[0])
        };
        let year = r.year.map(|y| format!("({})", y)).unwrap_or_default();
        let display = [authors, year, r.title.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ");
        self.entries[idx].reference = r;
        self.entries[idx].display = display;
    }
}

fn semantic_error_message(error: &anyhow::Error) -> String {
    let detail = format!("{error:#}");
    if detail.contains("Semantic index uses model") {
        "Semantic index was built with an older model; run `grimoire semantic-index`".to_string()
    } else {
        format!("Semantic search: {detail}")
    }
}

fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

fn launch_detached<T: AsRef<std::ffi::OsStr>>(
    command: Result<std::process::Command>,
    target: T,
) -> Result<std::process::Child> {
    let mut command = command?;
    command.arg(target);
    Ok(spawn_detached(&mut command)?)
}

fn launch_reader(
    command: Result<std::process::Command>,
    target: &Path,
    page: Option<u32>,
) -> Result<std::process::Child> {
    let mut command = prepare_reader_command(command?, target, page);
    Ok(spawn_detached(&mut command)?)
}

fn prepare_reader_command(
    command: std::process::Command,
    target: &Path,
    page: Option<u32>,
) -> std::process::Command {
    let mut prepared = std::process::Command::new(command.get_program());
    let mut has_path = false;
    for argument in command.get_args() {
        if argument == "{path}" {
            prepared.arg(target);
            has_path = true;
        } else if argument == "{page}" {
            prepared.arg(page.unwrap_or(1).to_string());
        } else {
            prepared.arg(argument);
        }
    }
    if !has_path {
        prepared.arg(target);
    }
    prepared
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;

    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    let mut last_error = None;
    for (bin, args) in candidates {
        match std::process::Command::new(bin)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take()
                    && let Err(error) = stdin.write_all(text.as_bytes())
                {
                    last_error = Some(error.into());
                    let _ = child.wait();
                    continue;
                }
                match child.wait() {
                    Ok(status) if status.success() => return Ok(()),
                    Ok(status) => {
                        last_error = Some(anyhow::anyhow!("{bin} exited with {status}"));
                    }
                    Err(error) => last_error = Some(error.into()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => last_error = Some(e.into()),
        }
    }

    if let Some(error) = last_error {
        return Err(error);
    }
    anyhow::bail!("no clipboard utility found (install wl-clipboard, xclip, or xsel)")
}

fn compute_diffs(old: &Reference, new: &Reference) -> Vec<(String, String, String)> {
    let mut diffs = Vec::new();

    if old.year != new.year {
        let o = old.year.map(|y| y.to_string()).unwrap_or_default();
        let n = new.year.map(|y| y.to_string()).unwrap_or_default();
        diffs.push(("year".into(), o, n));
    }
    if old.authors != new.authors {
        let o = if old.authors.is_empty() {
            String::new()
        } else {
            old.authors.join(", ")
        };
        let n = new.authors.join(", ");
        diffs.push(("authors".into(), o, n));
    }
    if old.doi != new.doi {
        diffs.push((
            "doi".into(),
            old.doi.clone().unwrap_or_default(),
            new.doi.clone().unwrap_or_default(),
        ));
    }
    if old.arxiv != new.arxiv {
        diffs.push((
            "arxiv".into(),
            old.arxiv.clone().unwrap_or_default(),
            new.arxiv.clone().unwrap_or_default(),
        ));
    }
    if old.journal != new.journal {
        diffs.push((
            "journal".into(),
            old.journal.clone().unwrap_or_default(),
            new.journal.clone().unwrap_or_default(),
        ));
    }
    if old.r#abstract != new.r#abstract && old.r#abstract.is_none() {
        diffs.push(("abstract".into(), String::new(), "(fetched)".into()));
    }

    diffs
}

fn needs_enrich(r: &Reference) -> bool {
    r.year.is_none()
        || r.year == Some(0)
        || r.authors.is_empty()
        || r.r#abstract.is_none()
        || r.doi.is_none()
}

fn is_importable(input: &str) -> bool {
    use crate::fetch;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    if fetch::detect_arxiv_id(trimmed).is_some() {
        return true;
    }
    if fetch::detect_doi(trimmed).is_some() {
        return true;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return true;
    }
    let path = std::path::PathBuf::from(trimmed);
    if path.exists() {
        return true;
    }
    false
}

fn picker_rect(area: Rect) -> Rect {
    let width = if area.width > 4 {
        (area.width * 3 / 4).max(50).min(area.width - 4)
    } else {
        area.width.max(1)
    };
    let height = if area.height > 4 {
        (area.height * 3 / 4).max(6).min(area.height - 2)
    } else {
        area.height.max(1)
    };
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn draw(f: &mut Frame, app: &mut App) {
    let t = &app.theme;
    let s_text = Style::default().fg(t.text);
    let s_dim = Style::default().fg(t.text_dim);
    let s_muted = Style::default().fg(t.text_muted);
    let s_author = Style::default().fg(t.author);
    let s_hl = Style::default().fg(t.highlight);
    let s_link = Style::default().fg(t.link);
    let s_date = Style::default().fg(t.date);

    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(t.background)),
        area,
    );
    // The full-screen preview overlay (drilled into from FullList) overrides
    // whatever layout is otherwise in effect.
    let resolved = if app.preview_overlay {
        ResolvedLayout::FullPreview
    } else {
        app.layout.resolve(area.width, area.height)
    };

    let border_style = Style::default().fg(t.border);

    // `show_list` is false only in FullPreview, where the list pane is dropped
    // but the search bar is kept so filtering still works.
    let (left_col, preview_area, show_list) = match resolved {
        ResolvedLayout::Wide => {
            let chunks =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(area);
            (chunks[0], Some(chunks[1]), true)
        }
        ResolvedLayout::Tall => {
            let chunks = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (chunks[0], Some(chunks[1]), true)
        }
        ResolvedLayout::FullList => (area, None, true),
        // The overlay spans the whole area; the search bar and preview are
        // carved out by the shared split below so the search bar keeps the
        // exact same 3-row height as every list layout.
        ResolvedLayout::FullPreview => (area, None, false),
    };

    // Split left column: search bar (3 rows) + list
    let left_parts = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(left_col);
    let search_area = left_parts[0];
    let list_area = left_parts[1];

    // In the full-screen preview overlay there is no list, so the region below
    // the search bar becomes the preview pane. Deriving it here (rather than in
    // the match) keeps the search bar's height identical to the list layouts.
    let preview_area = if resolved == ResolvedLayout::FullPreview {
        Some(list_area)
    } else {
        preview_area
    };

    // Search / add bar
    let search_content = if let Some(ref semantic_text) = app.semantic_input {
        Line::from(Span::styled(semantic_text.as_str(), s_text))
    } else if let Some(ref add_text) = app.add_input {
        Line::from(Span::styled(add_text.as_str(), s_text))
    } else if let Some(ref view) = app.semantic_view {
        Line::from(Span::styled(view.query.as_str(), s_text))
    } else {
        let mut spans = Vec::new();
        if let Some(ref tag) = app.tag_filter {
            spans.push(Span::styled(format!("[{}] ", tag), s_hl));
        }
        if !app.filter.is_empty() {
            spans.push(Span::styled(&app.filter, s_text));
        }
        Line::from(spans)
    };

    let search_title = if app.semantic_input.is_some() || app.semantic_view.is_some() {
        Line::from(Span::styled(" Semantic Search ", s_hl))
    } else if app.add_input.is_some() {
        Line::from(Span::styled(" Add ", s_hl))
    } else {
        Line::from(Span::styled(" Search ", s_hl))
    };

    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(search_title);
    let search_inner = search_block.inner(search_area);
    f.render_widget(search_block, search_area);
    f.render_widget(Paragraph::new(search_content), search_inner);

    // Cursor position for input
    if let Some(semantic_text) = &app.semantic_input {
        let cursor_x = search_inner.x + semantic_text.len() as u16;
        f.set_cursor_position((cursor_x, search_inner.y));
    } else if let Some(add_text) = &app.add_input {
        let cursor_x = search_inner.x + add_text.len() as u16;
        f.set_cursor_position((cursor_x, search_inner.y));
    } else if app.input_mode == InputMode::Search {
        let tag_label_len = app.tag_filter.as_ref().map(|t| t.len() + 3).unwrap_or(0);
        let cursor_x = search_inner.x + tag_label_len as u16 + app.filter.len() as u16;
        f.set_cursor_position((cursor_x, search_inner.y));
    }

    // Status bar as bottom title of list
    let mode_indicator = if app.semantic_input.is_some() || app.semantic_view.is_some() {
        Span::styled(
            if app.semantic_rx.is_some() {
                " SEARCHING "
            } else {
                " SEMANTIC "
            },
            Style::default()
                .fg(t.status_fg)
                .bg(t.insert_bg)
                .add_modifier(Modifier::BOLD),
        )
    } else if app.add_input.is_some() {
        // Add mode is orthogonal to Browse/Search (it's a text-entry overlay),
        // so it gets its own indicator with a distinct accent.
        Span::styled(
            " ADD ",
            Style::default()
                .fg(t.status_fg)
                .bg(t.highlight)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match app.input_mode {
            InputMode::Browse => Span::styled(
                " BROWSE ",
                Style::default()
                    .fg(t.status_fg)
                    .bg(t.normal_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            InputMode::Search => Span::styled(
                " SEARCH ",
                Style::default()
                    .fg(t.status_fg)
                    .bg(t.insert_bg)
                    .add_modifier(Modifier::BOLD),
            ),
        }
    };
    let mut bottom_spans = vec![mode_indicator];
    if let Some(view) = &app.semantic_view {
        bottom_spans.extend([
            Span::styled(
                format!(" {}", view.results.len()),
                s_date.add_modifier(Modifier::BOLD),
            ),
            Span::styled(" loaded · ", s_text),
            Span::styled(view.total.to_string(), s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" total ", s_text),
        ]);
    } else {
        bottom_spans.extend([
            Span::styled(
                format!(" {}", app.filtered_indices.len()),
                s_date.add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", s_text),
            Span::styled(
                app.entries.len().to_string(),
                s_hl.add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", s_text),
        ]);
    }
    if let Some(flash) = app.flash_message() {
        bottom_spans.push(Span::styled(format!(" {} ", flash), s_hl));
    } else if app.semantic_view.is_some() && app.semantic_input.is_none() {
        bottom_spans.extend([
            Span::styled(" enter", s_link.add_modifier(Modifier::BOLD)),
            Span::styled(" open  ", s_text),
            Span::styled("v", s_author.add_modifier(Modifier::BOLD)),
            Span::styled(" edit  ", s_text),
            Span::styled("esc", s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" papers ", s_text),
        ]);
    } else if app.semantic_input.is_some() {
        bottom_spans.extend([
            Span::styled(" enter", s_link.add_modifier(Modifier::BOLD)),
            Span::styled(" search  ", s_text),
            Span::styled("esc", s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" cancel ", s_text),
        ]);
    } else if app.add_input.is_some() {
        bottom_spans.extend([
            Span::styled(" enter", s_link.add_modifier(Modifier::BOLD)),
            Span::styled(" add  ", s_text),
            Span::styled("esc", s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" cancel ", s_text),
        ]);
    } else if app.input_mode == InputMode::Search {
        bottom_spans.extend([
            Span::styled(" esc", s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" browse ", s_text),
        ]);
    } else {
        bottom_spans.extend([
            Span::styled(" /", s_link.add_modifier(Modifier::BOLD)),
            Span::styled(" search  ", s_text),
            Span::styled("c", s_author.add_modifier(Modifier::BOLD)),
            Span::styled(" clear  ", s_text),
            Span::styled("q", s_hl.add_modifier(Modifier::BOLD)),
            Span::styled(" quit ", s_text),
        ]);
    }
    let bottom_left = Line::from(bottom_spans);

    // Show the sort indicator whenever the ordering isn't the clean default
    // (name, ascending). The arrow reflects the effective direction: a field's
    // default direction, flipped by `sort_reverse`.
    let sort_right = if app.semantic_view.is_none()
        && app.filter.is_empty()
        && (app.sort_mode != SortMode::Name || app.sort_reverse)
    {
        let descending = app.sort_mode.defaults_descending() ^ app.sort_reverse;
        let arrow = if descending { "↓" } else { "↑" };
        Line::from(Span::styled(
            format!(" sort: {} {} ", app.sort_mode.label(), arrow),
            s_hl,
        ))
    } else {
        Line::default()
    };

    if show_list {
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(Span::styled(
                if app.semantic_view.is_some() {
                    " Passages "
                } else {
                    " Papers "
                },
                s_hl,
            )))
            .title_bottom(bottom_left)
            .title_bottom(sort_right.alignment(ratatui::layout::Alignment::Right));

        let list_inner = list_block.inner(list_area);
        f.render_widget(list_block, list_area);

        // Paper list — year + author + title
        app.list_height = list_inner.height as usize;
        let list_width = list_inner.width as usize;
        let prefix_width = 3 + 6 + 14; // highlight_symbol + year + author
        let title_max = list_width.saturating_sub(prefix_width);

        if let Some(view) = app.semantic_view.as_mut() {
            if app.semantic_rx.is_some() {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Embedding query and ranking passages…",
                        s_dim,
                    )))
                    .alignment(ratatui::layout::Alignment::Center),
                    list_inner,
                );
            } else if view.results.is_empty() {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No indexed passages match this query.",
                        s_dim,
                    )))
                    .alignment(ratatui::layout::Alignment::Center),
                    list_inner,
                );
            } else {
                let excerpt_width = list_width.saturating_sub(6).max(8);
                let items: Vec<ListItem> = view
                    .results
                    .iter()
                    .enumerate()
                    .map(|(rank, hit)| {
                        let page = hit
                            .pages
                            .first()
                            .map(|page| format!(" · p. {page}"))
                            .unwrap_or_default();
                        let heading = if hit.headings.is_empty() {
                            String::new()
                        } else {
                            format!("  {}", semantic_heading_text(&hit.headings))
                        };
                        let compact = hit.text.split_whitespace().collect::<Vec<_>>().join(" ");
                        vec![
                            Line::from(vec![
                                Span::styled(format!(" {:>2}. ", rank + 1), s_date),
                                Span::styled(hit.paper_title.as_str(), s_text),
                                Span::styled(page, s_hl),
                            ]),
                            Line::from(Span::styled(heading, s_author)),
                            Line::from(Span::styled(
                                format!("  {}", truncate_ellipsis(&compact, excerpt_width)),
                                s_dim,
                            )),
                        ]
                    })
                    .map(ListItem::new)
                    .collect();
                let list = List::new(items)
                    .highlight_style(
                        Style::default()
                            .bg(t.selection)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(" > ");
                f.render_stateful_widget(list, list_inner, &mut view.list_state);
            }
        } else if app.filtered_indices.is_empty() {
            let is_query_importable = is_importable(&app.filter);
            let msg = if is_query_importable {
                vec![
                    Line::from(Span::styled("No papers match this query.", s_dim)),
                    Line::from(""),
                    Line::from(Span::styled(
                        "This query looks like an importable source!",
                        s_hl.add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Press ", s_dim),
                        Span::styled("Tab", s_hl.add_modifier(Modifier::BOLD)),
                        Span::styled(" or ", s_dim),
                        Span::styled("Ctrl-A", s_hl.add_modifier(Modifier::BOLD)),
                        Span::styled(" to load it into the Add bar.", s_dim),
                    ]),
                ]
            } else {
                vec![Line::from(Span::styled(
                    "No papers match this query.",
                    s_dim,
                ))]
            };

            let num_lines = msg.len();
            let paragraph = Paragraph::new(msg)
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: true });

            // Center vertically inside list_inner
            let vertical_margin = (list_inner.height as usize).saturating_sub(num_lines) / 2;
            let hint_area = Layout::vertical([
                Constraint::Length(vertical_margin as u16),
                Constraint::Min(num_lines as u16),
            ])
            .split(list_inner)[1];

            f.render_widget(paragraph, hint_area);
        } else {
            let items: Vec<ListItem> = app
                .filtered_indices
                .iter()
                .map(|&idx| {
                    let r = &app.entries[idx].reference;

                    let year_str = r
                        .year
                        .map(|y| format!(" {} ", y))
                        .unwrap_or_else(|| "      ".to_string());

                    let author_str = r
                        .authors
                        .first()
                        .map(|a| {
                            let last = if let Some((last, _)) = a.rsplit_once(',') {
                                last.trim()
                            } else {
                                a.split_whitespace().last().unwrap_or(a)
                            };
                            format!("{:>12}  ", truncate_str(last, 12))
                        })
                        .unwrap_or_else(|| "              ".to_string());

                    let title_lines = wrap_text(&r.title, title_max);
                    let indent = " ".repeat(prefix_width.saturating_sub(3)); // minus highlight_symbol

                    let mut lines: Vec<Line> = Vec::with_capacity(title_lines.len());
                    lines.push(Line::from(vec![
                        Span::styled(year_str, s_date),
                        Span::styled(author_str, s_author),
                        Span::styled(title_lines[0].clone(), s_text),
                    ]));
                    for cont in &title_lines[1..] {
                        lines.push(Line::from(vec![
                            Span::raw(indent.clone()),
                            Span::styled(cont.clone(), s_text),
                        ]));
                    }

                    ListItem::new(lines)
                })
                .collect();

            let list = List::new(items)
                .highlight_style(
                    Style::default()
                        .bg(t.selection)
                        .add_modifier(Modifier::BOLD),
                )
                // Selection is shown by the row's background highlight; keep a
                // blank 3-col gutter (matching prefix_width) so titles stay
                // aligned and wrapped continuation lines indent correctly.
                .highlight_symbol("   ");

            f.render_stateful_widget(list, list_inner, &mut app.list_state);
        }
    }

    // Preview pane
    if let Some(pane_area) = preview_area {
        let preview_title = app
            .selected_entry()
            .map(|e| Line::from(Span::styled(format!(" {} ", e.dir_name), s_hl)))
            .unwrap_or_default();

        let preview_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(preview_title)
            .padding(ratatui::widgets::Padding::horizontal(1));

        let content_area = preview_block.inner(pane_area);
        f.render_widget(preview_block, pane_area);

        let styles = Styles {
            text: s_text,
            dim: s_dim,
            muted: s_muted,
            author: s_author,
            highlight: s_hl,
            link: s_link,
            date: s_date,
        };
        if let Some(hit) = app.selected_semantic_hit() {
            draw_semantic_preview(f, hit, content_area, app.preview_scroll, &styles);
        } else {
            draw_preview(f, app, content_area, &styles);
        }
    }

    // Tag picker popup
    if let Some(ref mut popup) = app.tag_popup {
        let area = f.area();
        let max_visible = 20.min(popup.filtered_tags.len());
        let height = max_visible as u16 + 4;
        let width = 36.min(area.width.saturating_sub(4));
        let x = area.width.saturating_sub(width) / 2;
        let y = area.height.saturating_sub(height) / 2;
        let popup_area = ratatui::layout::Rect::new(x, y, width, height);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(t.popup_bg))
            .border_style(Style::default().fg(t.popup_border))
            .title(" Tags ")
            .title_style(s_author.add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let popup_chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

        let filter_line = if popup.filter.is_empty() {
            Line::from(Span::styled(" type to filter...", s_muted))
        } else {
            Line::from(vec![
                Span::styled(" > ", s_author),
                Span::styled(&popup.filter, s_text),
            ])
        };
        f.render_widget(Paragraph::new(filter_line), popup_chunks[0]);

        popup.clamp_scroll(max_visible);

        let inner_width = popup_chunks[1].width as usize;
        let lines: Vec<Line> = popup
            .filtered_tags
            .iter()
            .enumerate()
            .skip(popup.scroll)
            .take(max_visible)
            .map(|(i, tag)| {
                let is_selected = i == popup.selected;
                let prefix = if is_selected { " > " } else { "   " };
                let style = if is_selected {
                    Style::default()
                        .fg(t.text)
                        .bg(t.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    s_dim
                };
                let count = popup.count_for(tag);
                let count_str = format!("{} ", count);
                let label = format!("{}{}", prefix, tag);
                let pad = inner_width.saturating_sub(label.len() + count_str.len());
                let count_style = if is_selected {
                    Style::default().fg(t.text_dim).bg(t.selection)
                } else {
                    s_muted
                };
                Line::from(vec![
                    Span::styled(label, style),
                    Span::styled(" ".repeat(pad), style),
                    Span::styled(count_str, count_style),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), popup_chunks[1]);

        let hint = Line::from(Span::styled(" enter select  esc cancel", s_muted));
        f.render_widget(Paragraph::new(hint), popup_chunks[2]);
    }

    // Theme picker popup
    if let Some(ref popup) = app.theme_popup {
        let area = f.area();
        let popup_area = picker_rect(area);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(t.popup_bg))
            .border_style(Style::default().fg(t.popup_border))
            .title(" Theme ")
            .title_style(s_author.add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let popup_chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

        let max_visible = usize::from(popup_chunks[0].height).max(1);
        let scroll = popup.selected.saturating_sub(max_visible.saturating_sub(1));

        let lines: Vec<Line> = popup
            .names
            .iter()
            .enumerate()
            .skip(scroll)
            .take(max_visible)
            .map(|(i, name)| {
                let is_selected = i == popup.selected;
                let prefix = if is_selected { " > " } else { "   " };
                let style = if is_selected {
                    Style::default()
                        .fg(t.text)
                        .bg(t.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    s_dim
                };
                Line::from(Span::styled(format!("{}{}", prefix, name), style))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), popup_chunks[0]);

        let hint = Line::from(Span::styled(
            " j/k preview  enter select  esc cancel",
            s_muted,
        ));
        f.render_widget(Paragraph::new(hint), popup_chunks[1]);
    }

    // Enrich preview popup
    if let Some(ref ep) = app.enrich_preview {
        let title_text = truncate_ellipsis(&ep.updated.title, 40);
        let batch_info = if !ep.batch_queue.is_empty() || ep.applied > 0 || ep.skipped > 0 {
            let remaining = ep.batch_queue.len() + 1;
            let total = ep.applied + ep.skipped + remaining;
            format!(" [{}/{}] ", ep.applied + ep.skipped + 1, total)
        } else {
            String::new()
        };
        let header = format!(" Enrich{}: {} ", batch_info, title_text);

        let mut lines: Vec<Line> = Vec::new();
        for (field, old_val, new_val) in &ep.diffs {
            lines.push(Line::from(Span::styled(
                format!(" {}:", field),
                s_author.add_modifier(Modifier::BOLD),
            )));
            if !old_val.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  - ", Style::default().fg(t.highlight)),
                    Span::styled(old_val.as_str(), s_dim),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("  + ", Style::default().fg(t.insert_bg)),
                Span::styled(new_val.as_str(), s_text),
            ]));
        }

        let content_height = lines.len() as u16;
        let height = (content_height + 5).min(area.height.saturating_sub(4));
        let width = 70.min(area.width.saturating_sub(4));
        let x = area.width.saturating_sub(width) / 2;
        let y = area.height.saturating_sub(height) / 2;
        let popup_area = ratatui::layout::Rect::new(x, y, width, height);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(t.popup_bg))
            .border_style(Style::default().fg(t.popup_border))
            .title(header)
            .title_style(s_author.add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let popup_chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((ep.scroll, 0)),
            popup_chunks[0],
        );

        let hint = Line::from(vec![
            Span::styled(" enter/y", s_author),
            Span::styled("=apply  ", s_dim),
            Span::styled("n/s", s_author),
            Span::styled("=skip  ", s_dim),
            Span::styled("esc", s_author),
            Span::styled("=cancel", s_dim),
        ]);
        f.render_widget(Paragraph::new(hint), popup_chunks[1]);
    }

    // Validate popup
    if let Some(ref vp) = app.validate_popup {
        let line_count = vp.issues.len() as u16 + 3;
        let height = (line_count + 4).min(area.height.saturating_sub(4));
        let width = 60.min(area.width.saturating_sub(4));
        let x = area.width.saturating_sub(width) / 2;
        let y = area.height.saturating_sub(height) / 2;
        let popup_area = ratatui::layout::Rect::new(x, y, width, height);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(t.popup_bg))
            .border_style(Style::default().fg(t.popup_border))
            .title(" Validate ")
            .title_style(s_author.add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(format!(" {}", vp.summary), s_text)));
        lines.push(Line::from(""));

        for issue in &vp.issues {
            lines.push(Line::from(Span::styled(format!(" {}", issue), s_dim)));
        }

        f.render_widget(Paragraph::new(lines).scroll((vp.scroll, 0)), inner);
    }

    // Help popup
    if app.show_help {
        let help_lines = vec![
            ("", "Browse mode"),
            ("j / k", "Move down / up"),
            ("g / G", "Jump to top / bottom"),
            ("^d / ^u", "Half-page down / up"),
            ("^f / ^b", "Page down / up"),
            ("J / K", "Scroll preview down / up"),
            ("/ or i", "Enter search mode"),
            ("v", "Semantic passage search"),
            ("enter", "Open PDF"),
            ("space", "Toggle full-screen abstract"),
            ("e", "Edit info.toml"),
            ("y", "Copy BibTeX"),
            ("o", "Open DOI / arXiv in browser"),
            ("a", "Add paper (path, DOI, arXiv, URL)"),
            ("r", "Enrich selected (fetch metadata)"),
            ("R", "Enrich all with missing fields"),
            ("s", "Cycle sort (name/author/year/title)"),
            ("S", "Reverse sort direction"),
            ("d", "Deduplicate library"),
            ("I", "Reindex library"),
            ("V", "Validate library (auto-fix)"),
            ("c", "Clear search and tag filter"),
            ("t", "Browse tags"),
            ("T", "Switch theme"),
            ("L", "Cycle layout (full/wide/tall)"),
            ("q / esc", "Quit"),
            ("", ""),
            ("", "Search mode"),
            ("esc", "Return to browse mode"),
            ("^p / ^n", "Move up / down"),
            ("^d / ^u", "Half-page down / up"),
            ("^f / ^b", "Page down / up"),
            ("enter", "Open PDF"),
            ("tab", "Browse tags"),
            ("", ""),
            ("", "Semantic results"),
            ("j / k", "Move through ranked passages"),
            ("enter", "Open PDF at result page"),
            ("space", "Toggle full passage preview"),
            ("v", "Edit semantic query"),
            ("esc", "Return to papers"),
        ];

        let height = help_lines.len() as u16 + 4;
        let width = 56.min(area.width.saturating_sub(4));
        let x = area.width.saturating_sub(width) / 2;
        let y = area.height.saturating_sub(height) / 2;
        let popup_area = ratatui::layout::Rect::new(x, y, width, height);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(t.popup_bg))
            .border_style(Style::default().fg(t.popup_border))
            .title(" Help ")
            .title_style(s_author.add_modifier(Modifier::BOLD));
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let popup_chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

        let key_width = 10;
        let lines: Vec<Line> = help_lines
            .iter()
            .map(|(key, desc)| {
                if key.is_empty() {
                    Line::from(Span::styled(
                        format!(" {}", desc),
                        s_author.add_modifier(Modifier::BOLD),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled(format!(" {:>width$}  ", key, width = key_width), s_author),
                        Span::styled(*desc, s_dim),
                    ])
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), popup_chunks[0]);

        let hint = Line::from(Span::styled(" press any key to close", s_muted));
        f.render_widget(Paragraph::new(hint), popup_chunks[1]);
    }
}

fn should_load_more_semantic(selected: usize, loaded: usize, total: usize) -> bool {
    const LOAD_THRESHOLD: usize = 10;
    loaded < total && selected.saturating_add(LOAD_THRESHOLD) >= loaded
}

struct Styles {
    text: Style,
    dim: Style,
    muted: Style,
    author: Style,
    highlight: Style,
    link: Style,
    date: Style,
}

fn draw_semantic_preview(f: &mut Frame, hit: &SearchHit, area: Rect, scroll: u16, s: &Styles) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            hit.paper_title.as_str(),
            s.text.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let pages = match hit.pages.as_slice() {
        [] => String::new(),
        [page] => format!("page {page}"),
        pages => format!(
            "pages {}",
            pages
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("similarity {:.3}", hit.similarity), s.date),
        Span::styled(
            if pages.is_empty() {
                String::new()
            } else {
                format!(" · {pages}")
            },
            s.highlight,
        ),
    ]));
    lines.push(Line::from(""));
    if !hit.headings.is_empty() {
        lines.push(Line::from(Span::styled(
            semantic_heading_text(&hit.headings),
            s.author,
        )));
    }
    if !hit.headings.is_empty() {
        lines.push(Line::from(""));
    }
    lines.extend(
        semantic_passage_lines(&hit.text, &hit.headings)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, s.dim))),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{} · chunk {}", hit.source_path, hit.chunk_index),
        s.muted,
    )));

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn semantic_heading_text(headings: &[String]) -> String {
    format!("{}  ", headings.join(" › "))
}

fn semantic_passage_lines<'a>(text: &'a str, headings: &[String]) -> Vec<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0;
    while lines.get(start).is_some_and(|line| {
        headings
            .iter()
            .any(|heading| line.trim().eq_ignore_ascii_case(heading.trim()))
    }) {
        start += 1;
    }
    while lines.get(start).is_some_and(|line| line.trim().is_empty()) {
        start += 1;
    }
    lines[start..].to_vec()
}

fn draw_preview(f: &mut Frame, app: &App, area: ratatui::layout::Rect, s: &Styles) {
    let s_text = s.text;
    let s_dim = s.dim;
    let s_muted = s.muted;
    let s_author = s.author;
    let s_hl = s.highlight;
    let s_link = s.link;
    let s_date = s.date;
    if let Some(entry) = app.selected_entry() {
        let r = &entry.reference;
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled(
            &r.title,
            s_text.add_modifier(Modifier::BOLD),
        )));

        if r.year.is_some() || r.journal.is_some() {
            let mut parts = Vec::new();
            if let Some(year) = r.year {
                parts.push(Span::styled(year.to_string(), s_date));
            }
            if let Some(ref journal) = r.journal {
                if r.year.is_some() {
                    parts.push(Span::styled(" · ", s_muted));
                }
                parts.push(Span::styled(journal.as_str(), s_dim));
            }
            lines.push(Line::from(parts));
        }

        lines.push(Line::from(""));

        if !r.authors.is_empty() {
            let author_text = r.authors.join(" · ");
            lines.push(Line::from(Span::styled(author_text, s_author)));
            lines.push(Line::from(""));
        }

        if let Some(ref doi) = r.doi {
            lines.push(Line::from(vec![
                Span::styled("doi   ", s_muted),
                Span::styled(doi.as_str(), s_link),
            ]));
        }
        if let Some(ref arxiv) = r.arxiv {
            lines.push(Line::from(vec![
                Span::styled("arxiv ", s_muted),
                Span::styled(arxiv.as_str(), s_link),
            ]));
        }
        if !r.tags.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("tags  ", s_muted),
                Span::styled(r.tags.join(", "), s_hl),
            ]));
        }

        if let Some(ref abs) = r.r#abstract {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(abs.as_str(), s_dim)));
        }

        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((app.preview_scroll, 0)),
            area,
        );
    } else {
        f.render_widget(Paragraph::new(Span::styled("No selection", s_muted)), area);
    }
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}

fn truncate_ellipsis(s: &str, max: usize) -> String {
    if max < 2 || s.len() <= max {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max - 1);
        format!("{}…", &s[..end])
    }
}

/// Word-wrap `s` to lines no wider than `width` columns. Splits on whitespace;
/// a single word longer than `width` is hard-broken at char boundaries. Returns
/// at least one line (empty input yields one empty line).
fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in s.split_whitespace() {
        let mut word = word;
        // Hard-break any word that can't fit on its own line.
        while word.chars().count() > width {
            if current_width > 0 {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            let end = word.floor_char_boundary(width);
            lines.push(word[..end].to_string());
            word = &word[end..];
        }
        let word_width = word.chars().count();
        if current_width == 0 {
            current.push_str(word);
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        }
    }
    lines.push(current);
    lines
}

// --- Dedup TUI ---

fn run_dedup(terminal: &mut Term, app: &mut App) -> Result<()> {
    let library = app.config.library_dir();
    let groups = crate::dedup::find_duplicate_paths(&library)?;
    if groups.is_empty() {
        app.flash = Some(("No duplicates found".to_string(), std::time::Instant::now()));
        return Ok(());
    }

    let trash_dir = library.join(".trash");
    let mut removed = 0usize;
    let total_groups = groups.len();

    for (group_idx, group) in groups.iter().enumerate() {
        let mut selected: usize = 0;
        let entries: Vec<DedupEntry> = group.iter().map(|p| DedupEntry::from_path(p)).collect();

        if let Some((best, _)) = entries.iter().enumerate().max_by_key(|(_, e)| e.score) {
            selected = best;
        }

        loop {
            let theme = &app.theme;
            terminal.draw(|f| {
                draw_dedup(f, theme, &entries, selected, group_idx, total_groups);
            })?;

            if let Event::Key(key) = event::read()? {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _)
                    | (KeyCode::Esc, _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        let msg = if removed > 0 {
                            format!("Dedup: removed {}", removed)
                        } else {
                            "Dedup cancelled".to_string()
                        };
                        app.flash = Some((msg, std::time::Instant::now()));
                        terminal.clear()?;
                        return Ok(());
                    }
                    (KeyCode::Char('s'), _) => break,
                    (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                        selected = selected.saturating_sub(1);
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                        selected = (selected + 1).min(entries.len() - 1);
                    }
                    (KeyCode::Enter, _) => {
                        std::fs::create_dir_all(&trash_dir)?;
                        for (i, entry) in entries.iter().enumerate() {
                            if i != selected {
                                // Uniquify: a same-named dir may already sit in
                                // .trash from a previous dedup, and renaming onto
                                // an existing directory fails with ENOTEMPTY.
                                let dest =
                                    crate::dedup::unique_trash_path(&trash_dir, &entry.dir_name);
                                std::fs::rename(&entry.path, &dest)?;
                                removed += 1;
                            }
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let msg = if removed > 0 {
        format!("Dedup: removed {} (run reindex)", removed)
    } else {
        "Dedup: no changes".to_string()
    };
    app.flash = Some((msg, std::time::Instant::now()));
    terminal.clear()?;
    Ok(())
}

struct DedupEntry {
    path: PathBuf,
    dir_name: String,
    reference: Reference,
    score: u32,
    has_pdf: bool,
}

impl DedupEntry {
    fn from_path(path: &Path) -> Self {
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let reference = metadata::read_info(path).unwrap_or_else(|_| Reference {
            title: "Unknown".to_string(),
            authors: vec![],
            year: None,
            doi: None,
            arxiv: None,
            journal: None,
            tags: vec![],
            files: vec![],
            r#abstract: None,
        });
        let score = crate::dedup::metadata_score(&reference);
        let has_pdf = path
            .read_dir()
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                })
            })
            .unwrap_or(false);
        Self {
            path: path.to_path_buf(),
            dir_name,
            reference,
            score,
            has_pdf,
        }
    }
}

fn draw_dedup(
    f: &mut Frame,
    theme: &Theme,
    entries: &[DedupEntry],
    selected: usize,
    group_idx: usize,
    total_groups: usize,
) {
    let t = theme;
    let s_text = Style::default().fg(t.text);
    let s_dim = Style::default().fg(t.text_dim);
    let s_muted = Style::default().fg(t.text_muted);
    let s_author = Style::default().fg(t.author);
    let s_hl = Style::default().fg(t.highlight);

    let area = f.area();

    let chunks =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let left = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(chunks[0]);

    // Header
    let title = &entries[0].reference.title;
    let header = Line::from(vec![
        Span::styled(format!(" [{}/{}] ", group_idx + 1, total_groups), s_dim),
        Span::styled(
            truncate_ellipsis(title, left[0].width.saturating_sub(12) as usize),
            s_text,
        ),
    ]);
    f.render_widget(Paragraph::new(header), left[0]);

    // List of entries
    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let marker = if i == selected { "> " } else { "  " };
            let pdf_indicator = if e.has_pdf { " [PDF]" } else { "" };
            let label = format!("{}{}{} ({}/9)", marker, e.dir_name, pdf_indicator, e.score);
            let style = if i == selected {
                s_author.add_modifier(Modifier::BOLD)
            } else {
                s_dim
            };
            ListItem::new(Span::styled(label, style))
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::TOP).border_style(s_muted)),
        left[1],
    );

    // Footer
    let footer = Line::from(vec![
        Span::styled(" enter", s_author),
        Span::styled("=keep  ", s_dim),
        Span::styled("s", s_author),
        Span::styled("=skip  ", s_dim),
        Span::styled("q", s_author),
        Span::styled("=quit", s_dim),
    ]);
    f.render_widget(Paragraph::new(footer), left[2]);

    // Preview of selected entry
    let entry = &entries[selected];
    let r = &entry.reference;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        &r.title,
        s_text.add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if !r.authors.is_empty() {
        lines.push(Line::from(Span::styled(r.authors.join(" · "), s_author)));
    }
    if let Some(year) = r.year {
        lines.push(Line::from(Span::styled(format!("{}", year), s_dim)));
    }
    if let Some(ref journal) = r.journal {
        lines.push(Line::from(Span::styled(journal.as_str(), s_dim)));
    }
    lines.push(Line::default());

    if let Some(ref doi) = r.doi {
        lines.push(Line::from(vec![
            Span::styled("DOI: ", s_muted),
            Span::styled(doi.as_str(), s_dim),
        ]));
    }
    if let Some(ref arxiv) = r.arxiv {
        lines.push(Line::from(vec![
            Span::styled("arXiv: ", s_muted),
            Span::styled(arxiv.as_str(), s_dim),
        ]));
    }
    if !r.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Tags: ", s_muted),
            Span::styled(r.tags.join(", "), s_hl),
        ]));
    }
    if !r.files.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Files: ", s_muted),
            Span::styled(r.files.join(", "), s_dim),
        ]));
    }

    if let Some(ref abs) = r.r#abstract {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(abs.as_str(), s_dim)));
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(s_muted),
            )
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::{
        LayoutMode, picker_rect, prepare_reader_command, semantic_error_message,
        semantic_heading_text, semantic_passage_lines, should_load_more_semantic,
    };
    use ratatui::layout::Rect;

    #[test]
    fn picker_uses_pdfterm_three_quarter_layout() {
        let area = Rect::new(0, 0, 100, 40);

        assert_eq!(picker_rect(area), Rect::new(12, 5, 75, 30));
    }

    #[test]
    fn layout_defaults_to_full() {
        assert_eq!(LayoutMode::from_config(None), LayoutMode::FullList);
        assert_eq!(
            LayoutMode::from_config(Some("unknown")),
            LayoutMode::FullList
        );
    }

    #[test]
    fn layout_config_accepts_full_and_legacy_list() {
        assert_eq!(LayoutMode::from_config(Some("full")), LayoutMode::FullList);
        assert_eq!(LayoutMode::from_config(Some("list")), LayoutMode::FullList);
        assert_eq!(LayoutMode::from_config(Some("wide")), LayoutMode::Wide);
        assert_eq!(LayoutMode::from_config(Some("tall")), LayoutMode::Tall);
        assert_eq!(LayoutMode::from_config(Some("auto")), LayoutMode::Auto);
    }

    #[test]
    fn layout_cycle_wraps_after_three_visible_modes() {
        let full = LayoutMode::FullList;
        let wide = full.next(120, 40);
        let tall = wide.next(120, 40);

        assert_eq!(wide, LayoutMode::Wide);
        assert_eq!(tall, LayoutMode::Tall);
        assert_eq!(tall.next(120, 40), full);
    }

    #[test]
    fn auto_advances_from_its_resolved_layout() {
        assert_eq!(LayoutMode::Auto.next(120, 40), LayoutMode::Tall);
        assert_eq!(LayoutMode::Auto.next(80, 50), LayoutMode::FullList);
    }

    #[test]
    fn reader_template_places_page_and_path_for_pdfterm() {
        let mut command = Command::new("kitty");
        command.args(["@", "launch", "pdfterm", "--page", "{page}", "{path}"]);

        let prepared = prepare_reader_command(command, Path::new("synthetic.pdf"), Some(7));

        assert_eq!(prepared.get_program(), "kitty");
        assert_eq!(
            prepared.get_args().collect::<Vec<_>>(),
            ["@", "launch", "pdfterm", "--page", "7", "synthetic.pdf"]
        );
    }

    #[test]
    fn reader_without_template_still_appends_path() {
        let command = Command::new("open");
        let prepared = prepare_reader_command(command, Path::new("synthetic.pdf"), Some(7));

        assert_eq!(prepared.get_args().collect::<Vec<_>>(), ["synthetic.pdf"]);
    }

    #[test]
    fn stale_semantic_index_error_is_concise() {
        let error = anyhow::anyhow!(
            "Semantic index uses model old/model; expected current/model. Run `grimoire semantic-index` to rebuild it."
        );

        assert_eq!(
            semantic_error_message(&error),
            "Semantic index was built with an older model; run `grimoire semantic-index`"
        );
    }

    #[test]
    fn semantic_headings_leave_space_before_passage_text() {
        assert_eq!(
            semantic_heading_text(&["1 INTRODUCTION".to_string(), "Background".to_string()]),
            "1 INTRODUCTION › Background  "
        );
    }

    #[test]
    fn semantic_passage_uses_real_lines_without_repeating_heading() {
        let headings = vec!["1 Introduction".to_string()];
        assert_eq!(
            semantic_passage_lines(
                "1 Introduction\nAdditionally, DINOv3 improves features.\n\nSecond paragraph.",
                &headings,
            ),
            [
                "Additionally, DINOv3 improves features.",
                "",
                "Second paragraph."
            ]
        );
    }

    #[test]
    fn semantic_results_load_lazily_near_the_page_end() {
        assert!(!should_load_more_semantic(20, 100, 14_887));
        assert!(should_load_more_semantic(90, 100, 14_887));
        assert!(!should_load_more_semantic(99, 100, 100));
    }
}
