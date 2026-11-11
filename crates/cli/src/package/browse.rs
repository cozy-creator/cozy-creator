//! Catalog browsing: local installation state is read once per page, never per package.
use std::process::ExitCode;
use api::{calcifer::{HeldPackage, MachineList, MachinePackages, MachineState, On, hub::{FoundPackage, PackageSearch}}, error::Error};
use crossterm::event::KeyCode;
use serde::Serialize;
use crate::{Context, output};

pub(crate) const HEADERS: [&str; 4] = ["PACKAGE", "LATEST", "INSTALLED", "UPDATED"];

#[derive(Clone, Serialize)]
pub(super) struct Searching {
    pub hub: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub q: String,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Install { package: String, machine: String },
    Uninstall(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Page(u32),
    Preview(Preview),
    Destinations(String),
    Install { package: String, machine: String },
    ConfirmUninstall(String),
}

pub(crate) struct Catalog {
    query: Searching,
    page: PackageSearch,
    local: Option<Vec<HeldPackage>>,
    notice: Option<String>,
}

impl Catalog {
    pub(super) fn new(query: Searching, page: PackageSearch, local: Option<Vec<HeldPackage>>) -> Self {
        Self { query, page, local, notice: None }
    }

    pub(crate) async fn page(&mut self, ctx: &Context, offset: u32) -> Result<(), Error> {
        let query = Searching { offset, ..self.query.clone() };
        let page = super::catalog::read(ctx, &query).await?;
        self.query = query;
        self.page = page;
        self.local = local_inventory(ctx).await;
        self.notice = None;
        Ok(())
    }

    pub(crate) async fn refresh(&mut self, ctx: &Context, notice: String) -> Result<(), Error> {
        self.local = local_inventory(ctx).await;
        self.notice = Some(notice);
        Ok(())
    }

    pub(crate) fn title(&self) -> String {
        let title = if self.page.packages.is_empty() { "packages — no matches".into() }
            else { format!("packages {}–{} of {} · installed on local", self.query.offset + 1, self.query.offset as usize + self.page.packages.len(), self.page.total) };
        self.notice.as_ref().map_or(title.clone(), |notice| format!("{title} · {notice}"))
    }

    fn installed(&self, name: &str) -> Option<bool> {
        Some(self.local.as_ref()?.iter().any(|held| same(&held.package, name)))
    }

    pub(crate) fn rows(&self) -> Vec<Vec<String>> {
        self.page.packages.iter().map(|row| vec![row.package.clone(), row.latest_release.clone().unwrap_or_else(|| "—".into()),
            match self.installed(&row.package) { Some(true) => "yes", Some(false) => "no", None => "unknown" }.into(), date(row.updated_at.as_deref())]).collect()
    }

    pub(crate) fn package_at(&self, at: usize) -> Option<&str> { self.page.packages.get(at).map(|row| row.package.as_str()) }

    pub(crate) fn selection_after(&self, package: Option<&str>, before: usize) -> usize {
        package.and_then(|name| self.page.packages.iter().position(|row| same(&row.package, name)))
            .unwrap_or(before.min(self.page.packages.len().saturating_sub(1)))
    }

    pub(crate) fn action(&self, key: KeyCode, selected: usize) -> Option<Action> {
        match key {
            KeyCode::PageDown | KeyCode::Char('n') if self.page.older > 0 => return Some(Action::Page(self.query.offset.saturating_add(self.query.limit))),
            KeyCode::PageUp | KeyCode::Char('p') if self.query.offset > 0 => return Some(Action::Page(self.query.offset.saturating_sub(self.query.limit))),
            _ => {},
        }
        let row = self.page.packages.get(selected)?;
        match key {
            KeyCode::Enter => Some(Action::Preview(Preview { row: row.clone(), installed: self.installed(&row.package) })),
            KeyCode::Char('i') if row.latest_release.is_some() => Some(Action::Destinations(row.package.clone())),
            KeyCode::Char('u') if self.installed(&row.package) == Some(true) => Some(Action::ConfirmUninstall(row.package.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Preview {
    row: FoundPackage,
    installed: Option<bool>,
}

impl Preview {
    pub(crate) fn title(&self) -> &str { &self.row.package }
    pub(crate) fn lines(&self) -> Vec<String> {
        vec![format!("Latest: {}", self.row.latest_release.as_deref().unwrap_or("no published release")),
            format!("Installed on local: {}", match self.installed { Some(true) => "yes", Some(false) => "no", None => "unknown" }),
            format!("Updated: {}", date(self.row.updated_at.as_deref())),
            match (self.row.latest_release.is_some(), self.installed == Some(true)) {
                (true, true) => "i install · u uninstall from local · Esc/q back",
                (true, false) => "i install · Esc/q back",
                (false, true) => "u uninstall from local · Esc/q back",
                (false, false) => "Esc/q back",
            }.into()]
    }
    pub(crate) fn action(&self, key: KeyCode) -> Option<Action> {
        match key {
            KeyCode::Char('i') if self.row.latest_release.is_some() => Some(Action::Destinations(self.row.package.clone())),
            KeyCode::Char('u') if self.installed == Some(true) => Some(Action::ConfirmUninstall(self.row.package.clone())),
            _ => None,
        }
    }
}

pub(crate) struct Destinations {
    package: String,
    machines: Vec<(String, String)>,
}

impl Destinations {
    pub(crate) async fn open(ctx: &Context, package: String) -> Result<Self, Error> {
        let listed: MachineList = ctx.calcifer().await?.get("/v1/machines").await?;
        let machines = listed.machines.into_iter().filter(|machine| matches!(machine.state, MachineState::Ready | MachineState::Stopped))
            .map(|machine| (machine.name, if machine.state == MachineState::Stopped { "starts on install".into() } else { "ready".into() })).collect();
        Ok(Self { package, machines })
    }
    pub(crate) fn title(&self) -> String {
        if self.machines.is_empty() { "No available machines — use cozy machine --help".into() }
        else { format!("Install {} on", self.package) }
    }
    pub(crate) fn rows(&self) -> Vec<Vec<String>> { self.machines.iter().map(|(name, state)| vec![name.clone(), state.clone()]).collect() }
    pub(crate) fn action(&self, key: KeyCode, selected: usize) -> Option<Action> {
        if key != KeyCode::Enter { return None; }
        self.machines.get(selected).map(|(machine, _)| Action::Install { package: self.package.clone(), machine: machine.clone() })
    }
}

pub(crate) async fn perform(ctx: &Context, operation: Operation) -> Result<(ExitCode, String), Error> {
    match operation {
        Operation::Install { package, machine } => {
            let named = super::PackageRef::parse(&package)?;
            let status = super::machine::install(ctx, named.package(), None, None, Some(On::Machine(machine.clone()))).await?;
            let notice = if status == ExitCode::SUCCESS { format!("Installed {package} on {machine}") } else { "Installation stopped or failed".into() };
            Ok((status, notice))
        }
        Operation::Uninstall(package) => {
            let named = super::PackageRef::parse(&package)?;
            let status = super::machine::uninstall(ctx, &[named.package()], Some(On::Machine("local".into()))).await?;
            Ok((status, format!("Uninstalled {package} from local")))
        }
    }
}

pub(super) async fn local_inventory(ctx: &Context) -> Option<Vec<HeldPackage>> {
    let listed: MachinePackages = ctx.calcifer().await.ok()?.get("/v1/machines/local/packages?fresh=true").await.ok()?;
    Some(listed.packages)
}

fn same(left: &str, right: &str) -> bool {
    super::PackageRef::parse(left).ok().zip(super::PackageRef::parse(right).ok()).is_some_and(|(left, right)| left.package() == right.package())
}

fn date(value: Option<&str>) -> String {
    value.and_then(|value| value.parse::<jiff::Timestamp>().ok()).map_or_else(|| "—".into(), |timestamp| timestamp.strftime("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::new(Searching { hub: "https://example.invalid".into(), q: "image".into(), limit: 2, offset: 2 },
            PackageSearch { packages: vec![
                FoundPackage { package: "first".into(), latest_release: Some("1.0".into()), created_at: "2025-01-01T00:00:00Z".into(), updated_at: None },
                FoundPackage { package: "author/second".into(), latest_release: Some("2.0".into()), created_at: "2025-01-01T00:00:00Z".into(), updated_at: Some("2026-11-10T12:34:56.123Z".into()) },
            ], total: 5, older: 1 },
            Some(vec![HeldPackage { package: "author/second".into(), ..Default::default() }]))
    }

    #[test]
    fn catalog_navigation_and_dates_are_truthful() {
        let catalog = catalog();
        assert_eq!(catalog.action(KeyCode::Down, 0), None);
        assert!(matches!(catalog.action(KeyCode::Enter, 0), Some(Action::Preview(_))));
        assert_eq!(catalog.action(KeyCode::PageDown, 0), Some(Action::Page(4)));
        assert_eq!(catalog.action(KeyCode::PageUp, 0), Some(Action::Page(0)));
        assert_eq!(catalog.action(KeyCode::Char('u'), 0), None);
        assert_eq!(catalog.action(KeyCode::Char('u'), 1), Some(Action::ConfirmUninstall("author/second".into())));
        let rows = catalog.rows();
        assert_eq!(&rows[0][2..], ["no", "—"]);
        assert_eq!(&rows[1][2..], ["yes", "2026-11-10"]);
        let query = serde_json::to_value(&catalog.query).unwrap();
        assert_eq!(query["q"], "image");
        assert_eq!(query["offset"], 2);
        assert_eq!(catalog.selection_after(Some("author/second"), 0), 1);
    }

    #[tokio::test]
    async fn terminal_navigation_never_installs_or_uninstalls() {
        use crossterm::event::{Event, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let home = tempfile::tempdir().unwrap();
        let ctx = Context::new(false, false, false, api::config::Loaded { config: Default::default(), home: platform::paths::Home::under(home.path()), user_home: home.path().into(), warnings: vec![] }, output::Term::detect());
        let mut terminal = Terminal::new(TestBackend::new(110, 12)).unwrap();
        let keys = |codes: Vec<KeyCode>| futures_util::stream::iter(codes.into_iter().map(|code| Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))));
        assert_eq!(crate::tui::package_live(&ctx, &mut terminal, crate::tui::Screen::PackageCatalog(catalog()),
            keys(vec![KeyCode::Down, KeyCode::Enter, KeyCode::Esc, KeyCode::Char('q')])).await.unwrap(), None);
        assert_eq!(crate::tui::package_live(&ctx, &mut terminal, crate::tui::Screen::PackageCatalog(catalog()),
            keys(vec![KeyCode::Down, KeyCode::Char('u'), KeyCode::Esc, KeyCode::Char('q')])).await.unwrap(), None);
        assert_eq!(crate::tui::package_live(&ctx, &mut terminal, crate::tui::Screen::UninstallPackage("author/second".into()),
            keys(vec![KeyCode::Enter])).await.unwrap(), Some(Operation::Uninstall("author/second".into())));
        let destinations = Destinations { package: "author/second".into(), machines: vec![("local".into(), "ready".into())] };
        assert_eq!(crate::tui::package_live(&ctx, &mut terminal, crate::tui::Screen::PackageDestinations(destinations),
            keys(vec![KeyCode::Enter])).await.unwrap(), Some(Operation::Install { package: "author/second".into(), machine: "local".into() }));
    }
}
