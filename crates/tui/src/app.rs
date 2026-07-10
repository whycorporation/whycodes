use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct App {
    pub input: String,
    pub messages: Vec<(String, String)>,
    pub scroll: usize,
    pub mode: Mode,
    pub status: String,
    pub running: bool,
    // Provider management fields
    pub provider_form: ProviderForm,
}

pub enum Mode {
    Normal,
    Help,
    Command,
    ProviderSetup,
}

/// Form for adding a custom provider via TUI
pub struct ProviderForm {
    pub active: bool,
    pub fields: Vec<(String, String)>,    // (label, value)
    pub active_field: usize,
    pub saved: bool,
    pub error: Option<String>,
}

impl ProviderForm {
    pub fn new() -> Self {
        Self {
            active: false,
            fields: vec![
                ("Name".to_string(), String::new()),
                ("API Key".to_string(), String::new()),
                ("Base URL".to_string(), String::new()),
                ("Headers".to_string(), String::new()),
            ],
            active_field: 0,
            saved: false,
            error: None,
        }
    }

    pub fn reset(&mut self) {
        for (_, v) in &mut self.fields {
            v.clear();
        }
        self.active_field = 0;
        self.saved = false;
        self.error = None;
    }

    pub fn name(&self) -> &str { &self.fields[0].1 }
    pub fn api_key(&self) -> &str { &self.fields[1].1 }
    pub fn base_url(&self) -> &str { &self.fields[2].1 }
    pub fn headers(&self) -> &str { &self.fields[3].1 }

    /// Save to config.toml
    pub fn save_to_config(&self) -> Result<(), String> {
        let name = self.name().trim().to_string();
        if name.is_empty() {
            return Err("Provider name is required".to_string());
        }

        let _config_path = whycode_core::config::Config::default_path()
            .map_err(|e| format!("Config error: {e}"))?;

        let mut config = whycode_core::config::Config::load()
            .unwrap_or_default();

        let api_key = self.api_key().trim().to_string();
        let base_url = self.base_url().trim().to_string();
        let headers_str = self.headers().trim().to_string();

        // Parse headers: key1=val1,key2=val2
        let headers: Option<std::collections::HashMap<String, String>> =
            if headers_str.is_empty() {
                None
            } else {
                let mut map = std::collections::HashMap::new();
                for pair in headers_str.split(',') {
                    let parts: Vec<&str> = pair.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
                    }
                }
                if map.is_empty() { None } else { Some(map) }
            };

        let pc = whycode_core::types::ProviderConfig {
            name: name.clone(),
            api_key: if api_key.is_empty() { None } else { Some(api_key) },
            api_base: None,
            base_url: if base_url.is_empty() { None } else { Some(base_url) },
            headers,
            models: vec![],
            extra: std::collections::HashMap::new(),
        };

        config.providers.insert(name.clone(), pc);
        config.save().map_err(|e| format!("Save error: {e}"))?;

        Ok(())
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            messages: Vec::new(),
            scroll: 0,
            mode: Mode::Normal,
            status: String::from("Ready — press ? for help, : to set provider"),
            running: true,
            provider_form: ProviderForm::new(),
        }
    }

    pub fn add_message(&mut self, role: &str, text: &str) {
        self.messages.push((role.to_string(), text.to_string()));
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Help => self.handle_help(key),
            Mode::Command => self.handle_command(key),
            Mode::ProviderSetup => self.handle_provider_setup(key),
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key.code {
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
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+P → open provider setup
                self.mode = Mode::ProviderSetup;
                self.provider_form.active = true;
                self.provider_form.reset();
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
        }
    }

    fn handle_help(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    fn handle_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
            }
            KeyCode::Enter => {
                let cmd = self.input.clone();
                self.input.clear();
                self.mode = Mode::Normal;

                if cmd == ":q" || cmd == ":quit" {
                    self.running = false;
                } else if cmd == ":h" || cmd == ":help" {
                    self.mode = Mode::Help;
                } else if cmd.starts_with(":provider") || cmd == ":prov" {
                    self.mode = Mode::ProviderSetup;
                    self.provider_form.active = true;
                    self.provider_form.reset();
                    self.status = "Provider setup — fill fields, Ctrl+S to save".to_string();
                } else {
                    self.add_message("cmd", &format!("Unknown: {}", cmd));
                }
            }
            KeyCode::Backspace => {
                if self.input.len() > 1 {
                    self.input.pop();
                }
            }
            KeyCode::Char(c) => {
                self.handle_input(c);
            }
            _ => {}
        }
    }

    fn handle_provider_setup(&mut self, key: KeyEvent) {
        let pf = &mut self.provider_form;

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    // Save
                    match pf.save_to_config() {
                        Ok(()) => {
                            pf.saved = true;
                            pf.error = None;
                            self.status = format!("Provider '{}' saved to config!", pf.name());
                            self.mode = Mode::Normal;
                        }
                        Err(e) => {
                            pf.error = Some(e);
                        }
                    }
                    return;
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    // Cancel
                    pf.active = false;
                    self.mode = Mode::Normal;
                    self.status = "Provider setup cancelled".to_string();
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => {
                pf.active = false;
                self.mode = Mode::Normal;
                self.status = "Provider setup cancelled".to_string();
            }
            KeyCode::Tab => {
                pf.active_field = (pf.active_field + 1) % pf.fields.len();
            }
            KeyCode::BackTab => {
                pf.active_field = if pf.active_field == 0 {
                    pf.fields.len() - 1
                } else {
                    pf.active_field - 1
                };
            }
            KeyCode::Up => {
                pf.active_field = if pf.active_field == 0 {
                    pf.fields.len() - 1
                } else {
                    pf.active_field - 1
                };
            }
            KeyCode::Down => {
                pf.active_field = (pf.active_field + 1) % pf.fields.len();
            }
            KeyCode::Enter => {
                pf.active_field = (pf.active_field + 1) % pf.fields.len();
            }
            KeyCode::Backspace => {
                let val = &mut pf.fields[pf.active_field].1;
                val.pop();
            }
            KeyCode::Char(c) => {
                pf.fields[pf.active_field].1.push(c);
            }
            _ => {}
        }
    }

    pub fn handle_input(&mut self, c: char) {
        self.input.push(c);
    }
}
