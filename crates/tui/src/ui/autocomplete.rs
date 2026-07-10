// ── autocomplete.rs: File/directory autocomplete for the prompt input ──
// Ctrl+Space shows suggestions, type to filter, Tab/Enter to select, Esc to dismiss.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;

/// Autocomplete state.
pub struct Autocomplete {
    /// Whether the autocomplete popup is active.
    pub active: bool,
    /// Current filter string (what the user has typed so far in the prompt).
    pub filter: String,
    /// All matching file/directory entries.
    pub matches: Vec<PathEntry>,
    /// Selected index in the popup.
    pub selected: usize,
    /// Base directory for listing.
    pub work_dir: PathBuf,
}

/// A filesystem entry for display.
#[derive(Debug, Clone)]
pub struct PathEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_hidden: bool,
}

impl Autocomplete {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            active: false,
            filter: String::new(),
            matches: Vec::new(),
            selected: 0,
            work_dir,
        }
    }

    /// Activate autocomplete and populate initial matches.
    pub fn activate(&mut self, current_input: &str) {
        self.active = true;
        // Extract the last word/partial path from the input
        self.filter = current_input
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string();
        self.refresh();
    }

    /// Dismiss the autocomplete popup.
    pub fn dismiss(&mut self) {
        self.active = false;
        self.matches.clear();
        self.selected = 0;
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&PathEntry> {
        self.matches.get(self.selected)
    }

    /// Move selection up.
    pub fn prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else if !self.matches.is_empty() {
            self.selected = self.matches.len() - 1;
        }
    }

    /// Move selection down.
    pub fn next(&mut self) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1) % self.matches.len();
        }
    }

    /// Update the filter and refresh matches.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.refresh();
    }

    /// Refresh the match list from the filesystem.
    fn refresh(&mut self) {
        self.matches.clear();
        self.selected = 0;

        // Determine the search directory from the filter
        let (search_dir, prefix) = if self.filter.contains('/') {
            // Has a path component
            let path = PathBuf::from(&self.filter);
            if let Some(parent) = path.parent() {
                let full = if parent.is_relative() {
                    self.work_dir.join(parent)
                } else {
                    parent.to_path_buf()
                };
                (full, path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
            } else {
                (self.work_dir.clone(), self.filter.clone())
            }
        } else {
            (self.work_dir.clone(), self.filter.clone())
        };

        let prefix_lower = prefix.to_lowercase();

        // Read directory entries
        let entries = match std::fs::read_dir(&search_dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut results: Vec<PathEntry> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_hidden = name.starts_with('.');

            // Filter: skip hidden unless explicitly typed
            if is_hidden && !prefix.starts_with('.') {
                continue;
            }

            // Match by prefix or contains
            if prefix.is_empty() || name.to_lowercase().contains(&prefix_lower) {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                results.push(PathEntry {
                    name,
                    path: entry.path(),
                    is_dir,
                    is_hidden,
                });
            }
        }

        // Sort: directories first, then alphabetically
        results.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        self.matches = results;
    }

    /// Complete the current text with the selected entry.
    /// Returns the new text to replace the last word.
    pub fn complete_selection(&self, current_input: &str) -> Option<String> {
        let entry = self.selected_entry()?;

        // Find where the last word starts
        let last_space = current_input.rfind(' ');
        let prefix_len = match last_space {
            Some(pos) => current_input[..=pos].len(),
            None => 0,
        };

        let display_path = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };

        Some(format!(
            "{}{}",
            &current_input[..prefix_len],
            display_path
        ))
    }
}

impl Default for Autocomplete {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

/// Render the autocomplete popup below the input area.
pub fn render(frame: &mut Frame, area: Rect, autocomplete: &Autocomplete) {
    if !autocomplete.active || autocomplete.matches.is_empty() {
        return;
    }

    let num_rows = autocomplete.matches.len().min(10);
    let popup_height = (num_rows + 2) as u16;
    let popup_width = 50u16;

    let popup_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(popup_height),
        ])
        .split(area)[1];

    let popup_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(popup_width),
            Constraint::Min(0),
        ])
        .split(popup_area)[1];

    frame.render_widget(Clear, popup_area);

    let max_visible = popup_height.saturating_sub(2) as usize;
    let start_idx = autocomplete
        .selected
        .saturating_sub(max_visible.saturating_sub(1))
        .min(autocomplete.matches.len().saturating_sub(max_visible));

    let visible: Vec<&PathEntry> = autocomplete
        .matches
        .iter()
        .skip(start_idx)
        .take(max_visible)
        .collect();

    let mut lines: Vec<Line> = Vec::new();

    for (i, entry) in visible.iter().enumerate() {
        let global_idx = start_idx + i;
        let is_selected = global_idx == autocomplete.selected;

        let icon = if entry.is_dir { "📁 " } else { "📄 " };
        let suffix = if entry.is_dir { "/" } else { "" };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .bg(Color::Rgb(50, 50, 70))
                .add_modifier(Modifier::BOLD)
        } else if entry.is_hidden {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Rgb(200, 200, 210))
        };

        lines.push(Line::from(Span::styled(
            format!(" {}{}{}", icon, entry.name, suffix),
            style,
        )));
    }

    // Show count if more items exist
    let total = autocomplete.matches.len();
    if total > max_visible {
        let remaining = if start_idx > 0 {
            format!(
                " {} more above, {} more below",
                start_idx,
                total - start_idx - max_visible
            )
        } else {
            format!(" {} more below", total - max_visible)
        };
        lines.push(Line::from(Span::styled(
            remaining,
            Style::default().fg(Color::DarkGray),
        )));
    }

    let block = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(" Files ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(100, 149, 237)))
                .style(Style::default().bg(Color::Rgb(22, 22, 30))),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(block, popup_area);
}
