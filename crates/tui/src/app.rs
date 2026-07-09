use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct App {
    pub input: String,
    pub messages: Vec<(String, String)>,
    pub scroll: usize,
    pub mode: Mode,
    pub status: String,
    pub running: bool,
}

pub enum Mode {
    Normal,
    Help,
    Command,
}

impl App {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            messages: Vec::new(),
            scroll: 0,
            mode: Mode::Normal,
            status: String::from("Ready"),
            running: true,
        }
    }

    pub fn add_message(&mut self, role: &str, text: &str) {
        self.messages.push((role.to_string(), text.to_string()));
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.running = false;
                }
                KeyCode::Char('q') => {
                    self.running = false;
                }
                KeyCode::Char(':') => {
                    self.mode = Mode::Command;
                    self.input = String::from(":");
                }
                KeyCode::Char('?') => {
                    self.mode = Mode::Help;
                }
                KeyCode::Enter => {
                    if self.input.trim().is_empty() {
                        return;
                    }
                    let msg = self.input.clone();
                    self.add_message("user", &msg);
                    self.input.clear();
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => {
                    self.handle_input(c);
                }
                KeyCode::Esc => {
                    self.input.clear();
                }
                KeyCode::Up => {
                    if self.scroll < self.messages.len() {
                        self.scroll += 1;
                    }
                }
                KeyCode::Down => {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                _ => {}
            },
            Mode::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                    self.mode = Mode::Normal;
                }
                _ => {}
            },
            Mode::Command => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.input.clear();
                }
                KeyCode::Enter => {
                    let cmd = self.input.clone();
                    self.add_message("cmd", &cmd);
                    self.input.clear();
                    self.mode = Mode::Normal;
                    // Handle commands
                    if cmd == ":q" || cmd == ":quit" {
                        self.running = false;
                    } else if cmd == ":h" || cmd == ":help" {
                        self.mode = Mode::Help;
                    }
                }
                KeyCode::Backspace => {
                    // Keep the ':' prefix
                    if self.input.len() > 1 {
                        self.input.pop();
                    }
                }
                KeyCode::Char(c) => {
                    self.handle_input(c);
                }
                _ => {}
            },
        }
    }

    pub fn handle_input(&mut self, c: char) {
        self.input.push(c);
    }
}
