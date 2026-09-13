//! ninjaedit's terminal user interface.
//!
//! The editor logic lives in `ninjaedit-core`; this crate draws its state
//! with ratatui and translates terminal input into model updates. Everything
//! on screen belongs to one [`App`], which owns the open tabs, the command
//! palette, and the status bar.

mod app;
mod build_view;
mod clicks;
mod clipboard;
mod command;
mod editor_view;
mod fields;
mod goto_line;
mod input;
mod palette;
mod project_search;
mod search_box;
mod settings_view;
mod tabs;
mod terminal_view;
mod theme;
mod tool;

use crate::app::{App, AppEvent};
use crate::theme::Theme;
use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{SetTitle, supports_keyboard_enhancement};
use ninjaedit_core::{Project, Storage};
use std::io::{self, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// How long the event loop waits when idle before checking for background
/// changes (such as the file index filling in). Input and program output
/// wake it at once through the event channel, so this only paces the
/// housekeeping [`App::tick`] does.
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

    // The project is the git repository the current directory belongs to.
    // If it isn't inside one there is no project: the directory itself is
    // opened, with just its own files indexed.
    let cwd = std::env::current_dir()?;
    let project = Project::discover(&cwd)?;

    // Settings and the rest of what outlives a run live in ~/.ninjaedit.
    // The app reads the settings itself, reporting a bad file in the
    // status bar rather than refusing to start over it.
    let storage = Storage::in_home().unwrap_or_else(|err| {
        eprintln!("ninjaedit: {err}");
        std::process::exit(1);
    });

    // One channel carries everything the loop reacts to: terminal input on
    // a reader thread, and the output of programs running in tool panes
    // from their pty threads. The app keeps the sender to start sessions.
    let (events, event_queue) = mpsc::channel();
    let mut app = App::new(project, storage, events.clone());
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
    execute!(stdout(), EnableMouseCapture, SetTitle(app.window_title()))?;
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

    // Read terminal input on its own thread and forward it to the event
    // channel, so the loop can wait on input and program output together.
    // The thread ends when reading stdin fails, as it does on shutdown.
    {
        let events = events.clone();
        std::thread::Builder::new()
            .name("input".to_owned())
            .spawn(move || {
                while let Ok(event) = event::read() {
                    if events.send(AppEvent::Terminal(event)).is_err() {
                        break;
                    }
                }
            })?;
    }

    let result = run(&mut terminal, &mut app, &event_queue);

    if enhanced.load(Ordering::Relaxed) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    events: &mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    let mut dirty = true;
    while !app.should_quit() {
        if dirty {
            terminal.draw(|frame| app.render(frame))?;
            dirty = false;
        }
        match events.recv_timeout(TICK) {
            Ok(event) => {
                // Handle every queued event before redrawing so a burst of
                // input or program output doesn't cost a frame each.
                dirty |= handle(app, event);
                while let Ok(event) = events.try_recv() {
                    dirty |= handle(app, event);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if app.tick() {
                    dirty = true;
                }
            }
            // Both the input thread and every pty thread are gone.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

/// Route one event to the app and say whether the screen needs redrawing.
/// Focus notifications alone don't, so a terminal that reports focus
/// doesn't cost a redraw each time the window is clicked away and back.
fn handle(app: &mut App, event: AppEvent) -> bool {
    let redraw = !matches!(
        event,
        AppEvent::Terminal(Event::FocusGained | Event::FocusLost)
    );
    app.handle_app_event(event);
    redraw
}
