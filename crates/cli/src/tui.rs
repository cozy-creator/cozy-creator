//! Live full-screen views over Calcifer's event streams. A view is drawn again when its data
//! changes or a key is pressed, never on a timer: ↑/↓ or k/j select, Enter opens the selected
//! row's own view (a run as `cozy run watch` follows it, a machine as `cozy machine show` draws
//! it), Esc or q goes back to the list as it was left, and q there quits. The terminal is restored
//! on every way out: q, Ctrl-C, a signal, an error, and (ratatui's hook) a panic.
use std::{io, process::ExitCode};

use api::error::{Error, Kind};
use crossterm::{
    cursor::{MoveTo, Show},
    event::{Event as Input, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{Clear, ClearType},
};
use futures_util::{Stream, StreamExt as _};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::Constraint,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph, Row, Table, TableState, Wrap},
};
#[cfg(unix)]
use rustix::termios::{OptionalActions, OutputModes, tcgetattr, tcsetattr};

use crate::{Context, machine, model, output, package, run};

/// One live view.
pub(crate) enum Screen {
    Runs(run::live::Runs),
    Machines(machine::live::Machines),
    Machine(Box<machine::live::Shown>),
    Packages(package::live::Packages),
    Models(model::live::Catalog),
    Model(model::live::Preview),
    ModelDestinations(model::live::Destinations),
    RemoveLocalModel(String),
    PackageCatalog(package::browse::Catalog),
    Package(package::browse::Preview),
    PackageDestinations(package::browse::Destinations),
    UninstallPackage(String),
}

enum Operation {
    Model(model::live::Operation),
    Package(package::browse::Operation),
}

impl Screen {
    /// Waits until its data changed. Cancel-safe: a key pressed meanwhile loses nothing.
    async fn changed(&mut self, ctx: &Context) -> Result<(), Error> {
        match self {
            Screen::Runs(view) => view.changed(ctx).await,
            Screen::Machines(view) => view.changed(ctx).await,
            Screen::Machine(view) => view.changed().await,
            Screen::Packages(view) => view.changed(ctx).await,
            Screen::Models(_) | Screen::Model(_) | Screen::ModelDestinations(_) | Screen::RemoveLocalModel(_)
                | Screen::PackageCatalog(_) | Screen::Package(_) | Screen::PackageDestinations(_) | Screen::UninstallPackage(_) => std::future::pending().await,
        }
    }

    /// The view opened by Enter on row `selected`, if its rows open one.
    async fn open(&self, ctx: &Context, selected: usize) -> Result<Option<Screen>, Error> {
        match self {
            Screen::Machines(view) => match view.id(selected) {
                Some(id) => Ok(Some(Screen::Machine(Box::new(
                    machine::live::Shown::open(ctx, &id).await?,
                )))),
                None => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn draw(&self, ctx: &Context, frame: &mut Frame, selected: &mut TableState) {
        match self {
            Screen::Runs(view) => {
                table(frame, &view.title(), &view.headers(), view.rows(), selected)
            }
            Screen::Machines(view) => table(
                frame,
                &view.title(),
                &view.headers(),
                view.rows(ctx),
                selected,
            ),
            Screen::Packages(view) => table(
                frame,
                &view.title(),
                &view.headers(),
                view.rows(ctx),
                selected,
            ),
            Screen::Machine(view) => detail(frame, &view.title(), view.lines(ctx)),
            Screen::Models(view) => {
                if view.model_at(selected.selected().unwrap_or(0)).is_none() { selected.select(Some(view.first_selection())); }
                table_sections(frame, &view.title(), &["model", "family", "size", "local"], view.rows(), selected, view.hint(), &view.headings());
            }
            Screen::Model(view) => {
                let lines = view.lines().into_iter().map(Line::from).collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false })
                    .scroll((selected.selected().unwrap_or(0).min(u16::MAX as usize) as u16, 0))
                    .block(Block::new().title(view.title()).title_bottom(view.hint())), frame.area());
            }
            Screen::ModelDestinations(view) => table_hint(frame, &view.title(), &["machine", "status"], view.rows(), selected, "↑/↓ select · Enter install · Esc/q back"),
            Screen::RemoveLocalModel(model) => detail(frame, "Uninstall local copy?", vec![model.clone(), "All cached releases and lanes on this machine will be removed.".into(), "The Hub is unchanged. Active and shared data stays protected.".into(), "Enter confirms · Esc/q cancels".into()]),
            Screen::PackageCatalog(view) => table_hint(frame, &view.title(), &package::browse::HEADERS, view.rows(), selected, "↑/↓ select · Enter preview · i install · u uninstall local · p/n pages · q quit"),
            Screen::Package(view) => detail(frame, view.title(), view.lines()),
            Screen::PackageDestinations(view) => table_hint(frame, &view.title(), &["MACHINE", "STATUS"], view.rows(), selected, "↑/↓ select · Enter install · Esc/q back"),
            Screen::UninstallPackage(package) => detail(frame, "Uninstall from local?", vec![package.clone(), "All installed releases of this package will be uninstalled from local.".into(), "Files in use stay until their runs finish.".into(), "Enter confirms · Esc/q cancels".into()]),
        }
    }

    fn len(&self, ctx: &Context) -> usize {
        match self {
            Screen::Runs(view) => view.rows().len(),
            Screen::Machines(view) => view.rows(ctx).len(),
            Screen::Packages(view) => view.rows(ctx).len(),
            Screen::Machine(_) => 0,
            Screen::Models(view) => view.rows().len(),
            Screen::Model(view) => view.lines().len(),
            Screen::ModelDestinations(view) => view.rows().len(),
            Screen::RemoveLocalModel(_) => 0,
            Screen::PackageCatalog(view) => view.rows().len(),
            Screen::Package(view) => view.lines().len(),
            Screen::PackageDestinations(view) => view.rows().len(),
            Screen::UninstallPackage(_) => 0,
        }
    }
}

/// Shows `first` until the person quits. Only for a terminal: a caller off one prints once.
pub(crate) async fn show(ctx: &Context, first: Screen) -> Result<ExitCode, Error> {
    let mut stack = vec![(first, TableState::default().with_selected(Some(0)))];
    let mut keys = EventStream::new();
    let mut signals = Signals::new()?;
    let mut status = ExitCode::SUCCESS;
    loop {
        let mut terminal = ratatui::try_init().map_err(drawing)?;
        let shown = drive(ctx, &mut terminal, &mut stack, &mut keys, &mut signals).await;
        ratatui::restore();
        let (notice, package_action) = match shown? {
            Some(Operation::Model(model::live::Operation::Download(choice))) => {
                status = model::live::download(ctx, choice).await?;
                (if status == ExitCode::SUCCESS { "Install complete".into() } else { "Install stopped or failed".into() }, false)
            }
            Some(Operation::Model(model::live::Operation::Remove(model))) => {
                let removed = model::live::remove(ctx, &model).await?;
                status = ExitCode::SUCCESS;
                if removed.removed {
                    (format!("Cache {} removed · {} retained", output::bytes(removed.cache_bytes_removed), output::bytes(removed.retained_bytes)), false)
                } else {
                    ("Local copy already absent".into(), false)
                }
            }
            Some(Operation::Package(operation)) => {
                let (code, notice) = package::browse::perform(ctx, operation).await?;
                status = code;
                (notice, true)
            }
            None => return Ok(status),
        };
        if package_action {
            let Some(at) = stack.iter().rposition(|(screen, _)| matches!(screen, Screen::PackageCatalog(_))) else { return Ok(status) };
            stack.truncate(at + 1);
            if let Some((Screen::PackageCatalog(view), selected)) = stack.last_mut() {
                let previous = selected.selected().unwrap_or(0);
                let package = view.package_at(previous).map(str::to_owned);
                view.refresh(ctx, notice).await?;
                selected.select(Some(view.selection_after(package.as_deref(), previous)));
            }
            continue;
        }
        let Some(at) = stack.iter().rposition(|(screen, _)| matches!(screen, Screen::Models(_))) else { return Ok(status) };
        stack.truncate(at + 1);
        if let Some((Screen::Models(view), selected)) = stack.last_mut() {
            let previous = selected.selected().unwrap_or(0);
            let model = view.model_at(previous);
            view.refresh(ctx, notice).await?;
            selected.select(Some(view.selection_after(model.as_deref(), previous)));
        }
    }
}

/// The views on `terminal`, read from `keys`, until q, Ctrl-C or a signal.
#[cfg(test)]
pub(crate) async fn live<B: Backend>(
    ctx: &Context,
    terminal: &mut Terminal<B>,
    first: Screen,
    mut keys: impl Stream<Item = io::Result<Input>> + Unpin,
) -> Result<Option<model::live::Operation>, Error> {
    let mut stack = vec![(first, TableState::default().with_selected(Some(0)))];
    let mut signals = Signals::new()?;
    Ok(drive(ctx, terminal, &mut stack, &mut keys, &mut signals).await?.map(|operation| match operation {
        Operation::Model(operation) => operation,
        Operation::Package(_) => panic!("package operation in a model fixture"),
    }))
}

#[cfg(test)]
pub(crate) async fn package_live<B: Backend>(ctx: &Context, terminal: &mut Terminal<B>, first: Screen,
    mut keys: impl Stream<Item = io::Result<Input>> + Unpin,
) -> Result<Option<package::browse::Operation>, Error> {
    let mut stack = vec![(first, TableState::default().with_selected(Some(0)))];
    let mut signals = Signals::new()?;
    Ok(drive(ctx, terminal, &mut stack, &mut keys, &mut signals).await?.map(|operation| match operation {
        Operation::Package(operation) => operation,
        Operation::Model(_) => panic!("model operation in a package fixture"),
    }))
}

async fn drive<B: Backend>(
    ctx: &Context,
    terminal: &mut Terminal<B>,
    stack: &mut Vec<(Screen, TableState)>,
    keys: &mut (impl Stream<Item = io::Result<Input>> + Unpin),
    signals: &mut Signals,
) -> Result<Option<Operation>, Error> {
    let drew = |e: B::Error| drawing(io::Error::other(e.to_string()));
    loop {
        let (screen, selected) = stack.last_mut().expect("a view");
        terminal
            .draw(|frame| screen.draw(ctx, frame, selected))
            .map_err(drew)?;
        let rows = screen.len(ctx);
        let next = tokio::select! {
            changed = screen.changed(ctx) => Next::Changed(changed),
            input = keys.next() => Next::Input(input),
            () = signals.next() => Next::Quit,
            // Redrawn each second regardless: a running run's time ticks.
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => Next::Changed(Ok(())),
        };
        let key = match next {
            Next::Changed(changed) => {
                changed?;
                continue;
            }
            Next::Quit => return Ok(None),
            Next::Input(None) => return Ok(None),
            Next::Input(Some(input)) => match input.map_err(drawing)? {
                Input::Key(key) if key.kind == KeyEventKind::Press => key,
                _ => continue,
            },
        };
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        let depth = stack.len();
        let (screen, selected) = stack.last_mut().expect("a view");
        let at = selected.selected().unwrap_or(0);
        if let Screen::RemoveLocalModel(model) = &*screen {
            if key.code == KeyCode::Enter { return Ok(Some(Operation::Model(model::live::Operation::Remove(model.clone())))); }
        }
        if let Screen::UninstallPackage(package) = &*screen {
            if key.code == KeyCode::Enter { return Ok(Some(Operation::Package(package::browse::Operation::Uninstall(package.clone())))); }
        }
        let package_action = match &*screen {
            Screen::PackageCatalog(view) => view.action(key.code, at),
            Screen::Package(view) => view.action(key.code),
            Screen::PackageDestinations(view) => view.action(key.code, at),
            _ => None,
        };
        if let Some(action) = package_action {
            match action {
                package::browse::Action::Page(offset) => {
                    if let Screen::PackageCatalog(view) = screen { view.page(ctx, offset).await?; }
                    selected.select(Some(0));
                }
                package::browse::Action::Preview(view) => stack.push((Screen::Package(view), TableState::default())),
                package::browse::Action::Destinations(package) => stack.push((Screen::PackageDestinations(package::browse::Destinations::open(ctx, package).await?), TableState::default().with_selected(Some(0)))),
                package::browse::Action::Install { package, machine } => return Ok(Some(Operation::Package(package::browse::Operation::Install { package, machine }))),
                package::browse::Action::ConfirmUninstall(package) => stack.push((Screen::UninstallPackage(package), TableState::default())),
            }
            continue;
        }
        let local_bytes = match &*screen { Screen::Models(view) => view.rows().get(at).and_then(|row| view.local_bytes(&row[0])), _ => None };
        let action = match &*screen {
            Screen::Models(view) => view.action(key.code, at),
            Screen::Model(view) => view.action(key.code),
            Screen::ModelDestinations(view) => view.action(key.code, at),
            _ => None,
        };
        if let Some(action) = action {
            match action {
                model::live::Action::Page(offset) => {
                    if let Screen::Models(view) = screen { view.page(ctx, offset).await?; }
                    if let Screen::Models(view) = screen { selected.select(Some(view.page_selection())); }
                }
                model::live::Action::Preview(model) => stack.push((Screen::Model(model::live::Preview::open(ctx, &model, local_bytes).await?), TableState::default())),
                model::live::Action::Destinations(model) => stack.push((Screen::ModelDestinations(model::live::Destinations::open(ctx, model).await?), TableState::default().with_selected(Some(0)))),
                model::live::Action::Download(choice) => return Ok(Some(Operation::Model(model::live::Operation::Download(choice)))),
                model::live::Action::ConfirmRemove(model) => stack.push((Screen::RemoveLocalModel(model), TableState::default())),
                model::live::Action::Unclassified => {
                    let opened = match &*screen { Screen::Models(view) => Some(view.unclassified()), _ => None };
                    if let Some(opened) = opened { stack.push((Screen::Models(opened), TableState::default().with_selected(Some(0)))); }
                }
            }
            continue;
        }
        match key.code {
            _ if ctrl_c => return Ok(None),
            KeyCode::Esc | KeyCode::Char('q') if depth > 1 => drop(stack.pop()),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Up | KeyCode::Char('k') => selected.select(Some(match &*screen { Screen::Models(view) => view.navigate(at, false), _ => at.saturating_sub(1) })),
            KeyCode::Down | KeyCode::Char('j') if rows > 0 => {
                selected.select(Some(match &*screen { Screen::Models(view) => view.navigate(at, true), _ => (at + 1).min(rows - 1) }))
            }
            KeyCode::Enter if let Screen::Runs(view) = &*screen => {
                if let Some(id) = view.id(at) {
                    if watch(ctx, &id, keys, signals).await? {
                        return Ok(None);
                    }
                    // Drawn whole again: a resize forgets the screen without asking the
                    // terminal where its cursor is, as `clear` would.
                    let area = terminal.size().map_err(drew)?;
                    terminal.resize(area.into()).map_err(drew)?;
                }
            }
            KeyCode::Enter => {
                if let Some(opened) = screen.open(ctx, at).await? {
                    stack.push((opened, TableState::default()));
                }
            }
            _ => {}
        }
    }
}

/// What woke the view.
enum Next {
    Changed(Result<(), Error>),
    Input(Option<io::Result<Input>>),
    Quit,
}

/// Enter on a run: `cozy run watch` itself, on the terminal the list lends it, until Esc or q goes
/// back; true when Ctrl-C quit. Leaving stops watching, never the run; a run that ended stays on
/// screen, its record and all, until then.
async fn watch(
    ctx: &Context,
    id: &crate::calcifer_client::RunAddress,
    keys: &mut (impl Stream<Item = io::Result<Input>> + Unpin),
    signals: &mut Signals,
) -> Result<bool, Error> {
    lines(true).map_err(drawing)?;
    crossterm::execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0), Show).map_err(drawing)?;
    let mut left = None;
    let watched = async {
        let run = ctx.calcifer().await?.run(id).await?;
        output::say(ctx, &format!("cozy run watch {}  (Esc or q back)", run.number));
        let stop = async { left = Some(leave(&mut *keys, &mut *signals).await) };
        run::follow_until(ctx, run, stop).await
    };
    if let Err(error) = watched.await {
        output::refusal(ctx, &error);
    }
    let quit = match left {
        Some(quit) => quit,
        None => leave(keys, signals).await,
    };
    lines(false).map_err(drawing)?;
    Ok(quit)
}

/// The next key that leaves a run's watch: Esc or q goes back; Ctrl-C or a signal quits (true).
async fn leave(
    keys: &mut (impl Stream<Item = io::Result<Input>> + Unpin),
    signals: &mut Signals,
) -> bool {
    loop {
        let key = tokio::select! {
            input = keys.next() => match input {
                Some(Ok(Input::Key(key))) if key.kind == KeyEventKind::Press => key,
                Some(Ok(_)) => continue,
                None | Some(Err(_)) => return true,
            },
            () = signals.next() => return true,
        };
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc | KeyCode::Char('q') => return false,
            _ => {}
        }
    }
}

/// Raw mode keeps keys coming; on Unix it also stops a newline returning the carriage, which the
/// lines `cozy run watch` writes need (`on`) and ratatui's frames do not.
#[cfg(unix)]
fn lines(on: bool) -> io::Result<()> {
    let mut mode = tcgetattr(io::stdout())?;
    mode.output_modes.set(OutputModes::OPOST, on);
    Ok(tcsetattr(io::stdout(), OptionalActions::Now, &mode)?)
}

/// Windows' raw mode leaves output as it was.
#[cfg(not(unix))]
fn lines(_on: bool) -> io::Result<()> {
    Ok(())
}

/// Rows under `headers`, the selected one highlighted.
fn table(
    frame: &mut Frame,
    title: &str,
    headers: &[&str],
    rows: Vec<Vec<String>>,
    selected: &mut TableState,
) {
    table_hint(frame, title, headers, rows, selected, "↑/↓ select · Enter opens · q quits");
}

fn table_hint(frame: &mut Frame, title: &str, headers: &[&str], rows: Vec<Vec<String>>, selected: &mut TableState, hint: &str) {
    table_sections(frame, title, headers, rows, selected, hint, &[]);
}

fn table_sections(frame: &mut Frame, title: &str, headers: &[&str], rows: Vec<Vec<String>>, selected: &mut TableState, hint: &str, headings: &[usize]) {
    let widths: Vec<Constraint> = (0..headers.len())
        .map(|i| {
            let widest = rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|c| c.chars().count());
            let width = widest.chain([headers[i].len()]).max().unwrap_or(0);
            match i + 1 == headers.len() {
                true => Constraint::Fill(1),
                false => Constraint::Length(width as u16),
            }
        })
        .collect();
    let table = Table::new(rows.into_iter().enumerate().map(|(index, row)| {
        let row = Row::new(row);
        if headings.contains(&index) { row.style(Style::new().add_modifier(Modifier::BOLD)) } else { row }
    }), widths)
        .header(Row::new(headers.to_vec()).style(Style::new().add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .column_spacing(2)
        .block(Block::new().title(title).title_bottom(hint));
    frame.render_stateful_widget(table, frame.area(), selected);
}

/// A machine's record as `cozy machine show` prints it, drawn again as it changes.
fn detail(frame: &mut Frame, title: &str, lines: Vec<String>) {
    let block = Block::new().title(format!("{title}  (Esc or q back)"));
    let lines: Vec<Line> = lines.into_iter().map(Line::from).collect();
    frame.render_widget(Paragraph::new(lines).block(block), frame.area());
}

/// SIGINT and SIGTERM sent from outside: raw mode turns the terminal's own Ctrl-C into a key.
struct Signals {
    #[cfg(unix)]
    term: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> Result<Self, Error> {
        Ok(Self {
            #[cfg(unix)]
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(drawing)?,
        })
    }

    async fn next(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = self.term.recv() => {}
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn drawing(e: io::Error) -> Error {
    Error::new(Kind::Internal, "terminal", format!("the terminal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::backend::TestBackend;

    #[tokio::test]
    async fn confirmed_action_consumes_enter_before_browser_resumes() {
        let home = tempfile::tempdir().unwrap();
        let ctx = Context::new(false, false, false, api::config::Loaded {
            config: Default::default(), home: platform::paths::Home::under(home.path()), user_home: home.path().into(), warnings: vec![],
        }, output::Term::detect());
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
        let mut stack = vec![(Screen::RemoveLocalModel("local/example".into()), TableState::default())];
        let mut keys = futures_util::stream::iter([KeyCode::Enter, KeyCode::Char('q')].map(|code| Ok(Input::Key(KeyEvent::new(code, KeyModifiers::NONE)))));
        let mut signals = Signals::new().unwrap();
        assert!(matches!(drive(&ctx, &mut terminal, &mut stack, &mut keys, &mut signals).await.unwrap(), Some(Operation::Model(model::live::Operation::Remove(name))) if name == "local/example"));
        assert!(drive(&ctx, &mut terminal, &mut stack, &mut keys, &mut signals).await.unwrap().is_none());
    }
}
