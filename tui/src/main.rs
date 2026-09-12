//! ninjaedit's terminal user interface.
//!
//! The editor logic lives in `ninjaedit-core`; this crate draws its state
//! with ratatui and translates terminal input into model updates. Everything
//! on screen belongs to one [`App`], which owns the open tabs, the command
//! palette, and the status bar.

mod app;
mod clicks;
mod clipboard;
mod editor_view;
mod goto_line;
mod input;
mod palette;
mod project_search;
mod search_box;
mod tabs;
mod theme;

use crate::app::App;
use crate::theme::Theme;
use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;
use ninjaedit_core::Project;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the event loop waits for input before checking for background
/// changes (such as the file index filling in).
const TICK: Duration = Duration::from_millis(100);

/// A terminal IDE for fast navigation of large projects.
#[derive(Parser, Debug)]
#[command(name = "ninjaedit", version, about)]
struct Args {
    /// Files to open for editing.
    files: Vec<PathBuf>,

    /// A theme file to use instead of the built-in default.
    #[arg(long, value_name = "FILE")]
    theme: Option<PathBuf>,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Read the theme before touching the terminal so a bad file is
    // reported plainly rather than flashed through the alternate screen.
    let theme = match &args.theme {
        Some(path) => Theme::load(path).unwrap_or_else(|err| {
            eprintln!("ninjaedit: {err}");
            std::process::exit(1);
        }),
        None => Theme::default(),
    };

    // The project is the git repository the current directory belongs to,
    // or the current directory itself if it isn't inside one.
    let cwd = std::env::current_dir()?;
    let project = Project::discover(&cwd)?;
    let mut app = App::new(project);
    app.set_theme(theme);
    for file in args.files {
        app.open_file(std::path::absolute(&file)?);
    }

    // ratatui's init enables raw mode, enters the alternate screen, and
    // installs a panic hook that undoes both. Our own hook, installed first
    // so that ratatui's wraps it, turns mouse capture and the keyboard
    // protocol back off as well.
    let enhanced = Arc::new(AtomicBool::new(false));
    let hook = std::panic::take_hook();
    let hook_enhanced = Arc::clone(&enhanced);
    std::panic::set_hook(Box::new(move |info| {
        if hook_enhanced.load(Ordering::Relaxed) {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    // Where the terminal supports the kitty keyboard protocol, ask for
    // unambiguous key codes: without them Ctrl+Shift+F arrives as the same
    // byte as Ctrl+F.
    if supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        enhanced.store(true, Ordering::Relaxed);
    }

    let result = run(&mut terminal, &mut app);

    if enhanced.load(Ordering::Relaxed) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut dirty = true;
    while !app.should_quit() {
        if dirty {
            terminal.draw(|frame| app.render(frame))?;
            dirty = false;
        }
        if event::poll(TICK)? {
            // Handle every pending event before redrawing so a burst of
            // input (a paste, a fast scroll) doesn't cost a frame each.
            loop {
                let event = event::read()?;
                if !matches!(event, Event::FocusGained | Event::FocusLost) {
                    dirty = true;
                }
                app.handle_event(event);
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        } else if app.tick() {
            dirty = true;
        }
    }
    Ok(())
}
