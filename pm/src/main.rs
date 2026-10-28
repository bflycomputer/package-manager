use std::error::Error;
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use libpm::{App as Package, InstallOutcome, PackageManager};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{prelude::*, widgets::*, DefaultTerminal};

const RESULT_LIMIT: usize = 5;

struct Model {
    manager: Option<PackageManager>,
    manager_rx: Receiver<Result<PackageManager, String>>,
    query: String,
    hits: Vec<Package>,
    selected: usize,
    error: Option<String>,
    confirmation: Option<Package>,
    chosen: Option<Package>,
}

impl Model {
    fn new(query: String) -> Self {
        let (manager_tx, manager_rx) = mpsc::channel();
        thread::spawn(move || {
            let manager = PackageManager::init().map_err(|error| format!("{error:#}"));
            let _ = manager_tx.send(manager);
        });

        Self {
            manager: None,
            manager_rx,
            query,
            hits: Vec::new(),
            selected: 0,
            error: None,
            confirmation: None,
            chosen: None,
        }
    }

    fn search(&mut self) {
        let Some(manager) = self.manager.as_ref() else {
            return;
        };

        match manager.search(&self.query, RESULT_LIMIT) {
            Ok(hits) => {
                self.hits = hits;
                self.selected = 0;
                self.error = None;
            }
            Err(error) => {
                self.hits.clear();
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            if let Ok(manager) = self.manager_rx.try_recv() {
                self.manager = Some(manager.map_err(io::Error::other)?);
                self.search();
            }

            terminal.draw(|frame| self.render(frame))?;

            if self.manager.is_none() && !event::poll(Duration::from_millis(16))? {
                continue;
            }

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release && self.handle_key(key) {
                    return Ok(());
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }

        if self.confirmation.is_some() {
            return match key.code {
                KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                    self.chosen = self.confirmation.take();
                    true
                }
                KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                    self.confirmation = None;
                    false
                }
                _ => false,
            };
        }

        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
            }
            KeyCode::Enter => self.confirmation = self.hits.get(self.selected).cloned(),
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    self.search();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                // currently doing synchronously so it blocks keyboard input (buffered anyways)
                // generally sub-millisecond tho so not worth async unless search time can get high
                self.search();
            }
            _ => {}
        }

        false
    }

    fn render(&self, frame: &mut Frame) {
        let [search_area, results_area, status_area] = frame.area().layout(&Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ]));

        frame.render_widget(
            Paragraph::new(format!("{}▌", self.query)).block(Block::bordered().title("Search")),
            search_area,
        );

        let items = self.hits.iter().enumerate().map(|(index, package)| {
            let selected = index == self.selected;
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(if selected { "› " } else { "  " }, Color::Cyan),
                    Span::styled(
                        package.name.clone(),
                        if selected {
                            Style::new().cyan().bold()
                        } else {
                            Style::new().bold()
                        },
                    ),
                ]),
                Line::styled(format!("  {}", package.description), Color::Gray),
            ])
        });
        frame.render_widget(
            List::new(items).block(Block::bordered().title("Packages")),
            results_area,
        );

        let status = if let Some(error) = &self.error {
            Line::styled(error.clone(), Color::Red)
        } else if self.manager.is_none() {
            Line::styled("Loading package catalog…", Color::DarkGray)
        } else if self.hits.is_empty() {
            Line::styled("No packages found", Color::DarkGray)
        } else {
            Line::styled("↑/↓ select  •  Enter install  •  Esc quit", Color::DarkGray)
        };
        frame.render_widget(status, status_area);

        if let Some(package) = &self.confirmation {
            let area = frame
                .area()
                .centered(Constraint::Percentage(70), Constraint::Length(5));
            let dialog = Paragraph::new(vec![
                Line::from(format!("Install {}?", package.name)),
                Line::from(""),
                Line::from("Enter/y confirm  •  Esc/n cancel"),
            ])
            .alignment(Alignment::Center)
            .block(Block::bordered().title("Confirm"));
            frame.render_widget(Clear, area);
            frame.render_widget(dialog, area);
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["-u"] {
        if libpm::update()? {
            std::process::exit(2);
        }
        return Ok(());
    }
    if args == ["-a"] {
        println!("{}", libpm::updates_available()?);
        return Ok(());
    }

    let query = args.join(" ");
    let mut model = Model::new(query);
    ratatui::run(|terminal| model.run(terminal))?;

    if let (Some(package), Some(manager)) = (model.chosen, model.manager) {
        println!("Installing {}…", package.name);

        match manager.install(package.id)? {
            InstallOutcome::Installed => println!("Installed {}.", package.name),
            InstallOutcome::AlreadyInstalled => {
                println!("{} is already installed.", package.name);
            }
        }
    }

    Ok(())
}
