//! Keyboard browsing of one catalog page, with an explicit download destination.
use api::{
    calcifer::{HeldModel, MachineList, MachineModels, MachineState, On},
    calcifer::hub::{ModelDetail, ModelDownload, ModelSearch},
    error::Error,
};
use crossterm::event::KeyCode;

use super::catalog::{self, Searching};
use crate::{Context, output};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DownloadChoice {
    pub(crate) model: String,
    pub(crate) machine: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Download(DownloadChoice),
    Remove(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Page(u32),
    Preview(String),
    Destinations(String),
    Download(DownloadChoice),
    ConfirmRemove(String),
    Unclassified,
}

pub(crate) struct Catalog {
    query: Searching,
    page: ModelSearch,
    local: Option<Vec<HeldModel>>,
    local_only: bool,
    notice: Option<String>,
}

impl Catalog {
    pub(super) fn new(query: Searching, page: ModelSearch, local: Option<Vec<HeldModel>>) -> Self {
        Self { query, page, local, local_only: false, notice: None }
    }

    pub(super) fn from_local(query: Searching, local: Vec<HeldModel>) -> Self {
        let mut named = std::collections::BTreeMap::new();
        for held in &local {
            let matches_role = if query.role == "unknown" { !matches!(held.role.as_str(), "model" | "lora") } else { held.role == query.role };
            if !matches_role || !output::names(&held.model, &query.q.split_whitespace().map(str::to_owned).collect::<Vec<_>>()) { continue; }
            named.entry(held.model.clone()).or_insert_with(|| api::calcifer::hub::FoundModel {
                model: held.model.clone(), role: Some(held.role.clone()), release: Some(held.release.clone()), lanes: vec![held.lane.clone()], ..Default::default()
            });
        }
        let total = named.len() as i64;
        let models: Vec<_> = named.into_values().skip(query.offset as usize).take(query.limit as usize).collect();
        let older = (total - query.offset as i64 - models.len() as i64).max(0);
        Self { query, page: ModelSearch { models, total, older }, local: Some(local), local_only: true, notice: None }
    }

    pub(crate) async fn page(&mut self, ctx: &Context, offset: u32) -> Result<(), Error> {
        let mut query = self.query.clone();
        query.offset = offset;
        if self.local_only {
            let inventory: MachineModels = ctx.calcifer().await?.get("/v1/machines/local/models").await?;
            if inventory.silent {
                return Err(Error::unavailable("local_inventory_unavailable", "The local machine has not reported its model inventory yet").next("cozy machine ls"));
            }
            *self = Self::from_local(query, inventory.models);
        } else {
            let page = catalog::read(ctx, &query).await?;
            self.query = query;
            self.page = page;
            self.local = local_inventory(ctx).await;
        }
        Ok(())
    }

    pub(crate) async fn refresh(&mut self, ctx: &Context, notice: String) -> Result<(), Error> {
        self.page(ctx, self.query.offset).await?;
        if self.page.models.is_empty() && self.query.offset > 0 {
            let offset = self.query.offset.saturating_sub(self.query.limit);
            self.page(ctx, offset).await?;
        }
        self.notice = Some(notice);
        Ok(())
    }

    pub(crate) fn title(&self) -> String {
        if self.page.models.is_empty() {
            let unknown = self.local.as_ref().map_or(0, |rows| rows.iter().filter(|row| !matches!(row.role.as_str(), "model" | "lora")).count());
            let page = if self.local_only && unknown > 0 { format!("{} — no matches; {unknown} unclassified local entries", self.label()) } else { format!("{} — no matches", self.label()) };
            return self.notice.as_ref().map_or_else(|| page.clone(), |notice| format!("{page} · {notice}"));
        }
        let page = format!("{} {}–{} of {}", self.label(), self.query.offset + 1, self.query.offset as usize + self.page.models.len(), self.page.total);
        self.notice.as_ref().map_or_else(|| page.clone(), |notice| format!("{page} · {notice}"))
    }

    fn label(&self) -> &str { if self.local_only { if self.query.role == "unknown" { "unclassified local copies" } else if self.query.role == "lora" { "local LoRAs" } else { "local models" } } else if self.query.role == "lora" { "LoRAs" } else { "models" } }

    pub(crate) fn hint(&self) -> &str {
        if self.local_only && self.query.role != "unknown" && self.local.as_ref().is_some_and(|rows| rows.iter().any(|row| !matches!(row.role.as_str(), "model" | "lora"))) {
            "↑/↓ select · Enter preview · p/n pages · u unclassified copies · q back"
        } else {
            "↑/↓ select · Enter preview · PgUp/PgDn or p/n pages · q back"
        }
    }

    pub(crate) fn unclassified(&self) -> Self {
        let mut query = self.query.clone();
        query.role = "unknown".into();
        query.offset = 0;
        Self::from_local(query, self.local.clone().unwrap_or_default())
    }

    pub(crate) fn rows(&self) -> Vec<Vec<String>> {
        self.page.models.iter().map(|row| {
            let held = self.local.as_ref().map(|models| models.iter().filter(|held| same_model(&held.model, &row.model)).map(|held| held.bytes).max());
            let (local, size) = match held {
                Some(Some(bytes)) => ("yes", output::bytes(bytes)),
                Some(None) => ("no", "—".into()),
                None => ("unknown", "—".into()),
            };
            vec![row.model.clone(), row.family.clone().unwrap_or_else(|| "—".into()), size, local.into()]
        }).collect()
    }

    pub(crate) fn action(&self, key: KeyCode, selected: usize) -> Option<Action> {
        match key {
            KeyCode::Enter => self.page.models.get(selected).map(|row| Action::Preview(row.model.clone())),
            KeyCode::PageDown | KeyCode::Char('n') if self.page.older > 0 => Some(Action::Page(self.query.offset.saturating_add(self.query.limit))),
            KeyCode::PageUp | KeyCode::Char('p') if self.query.offset > 0 => Some(Action::Page(self.query.offset.saturating_sub(self.query.limit))),
            KeyCode::Char('u') if self.local_only && self.query.role != "unknown" => Some(Action::Unclassified),
            _ => None,
        }
    }

    pub(crate) fn local_bytes(&self, model: &str) -> Option<u64> {
        self.local.as_ref()?.iter().filter(|held| same_model(&held.model, model)).map(|held| held.bytes).max()
    }

    pub(crate) fn model_at(&self, index: usize) -> Option<&str> {
        self.page.models.get(index).map(|model| model.model.as_str())
    }

    pub(crate) fn selection_after(&self, model: Option<&str>, previous: usize) -> usize {
        model.and_then(|name| self.page.models.iter().position(|row| same_model(&row.model, name)))
            .unwrap_or_else(|| previous.min(self.page.models.len().saturating_sub(1)))
    }
}

pub(crate) struct Preview {
    model: ModelDetail,
    local_bytes: Option<u64>,
    metadata_missing: bool,
}

impl Preview {
    pub(crate) async fn open(ctx: &Context, model: &str, local_bytes: Option<u64>) -> Result<Self, Error> {
        match catalog::detail(ctx, model).await {
            Ok(model) => Ok(Self { model, local_bytes, metadata_missing: false }),
            Err(_) if local_bytes.is_some() => Ok(Self { model: ModelDetail { model: model.into(), ..Default::default() }, local_bytes, metadata_missing: true }),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn title(&self) -> &str { &self.model.model }

    pub(crate) fn action(&self, key: KeyCode) -> Option<Action> {
        match key {
            KeyCode::Char('d') if !self.metadata_missing => Some(Action::Destinations(self.model.model.clone())),
            KeyCode::Char('r') if self.local_bytes.is_some() => Some(Action::ConfirmRemove(self.model.model.clone())),
            _ => None,
        }
    }

    pub(crate) fn hint(&self) -> &str {
        if self.metadata_missing { "↑/↓ scroll · r remove local copy · Esc/q back" }
        else if self.local_bytes.is_some() { "↑/↓ scroll · d download · r remove local copy · Esc/q back" }
        else { "↑/↓ scroll · d download · Esc/q back" }
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        let model = &self.model;
        let mut lines = Vec::new();
        if self.metadata_missing { lines.push("Catalog details unavailable; showing the local copy.".into()); }
        if let Some(bytes) = self.local_bytes { lines.push(format!("Local size: {}", output::bytes(bytes))); }
        if let Some(family) = &model.family { lines.push(format!("Family: {family}")); }
        if model.role.as_deref() == Some("lora") {
            lines.push("Kind: LoRA".into());
        } else if let Some(kind) = model.kind {
            lines.push(format!("Kind: {}", match kind {
                api::calcifer::hub::ModelKind::Lora => "LoRA",
                api::calcifer::hub::ModelKind::Weights => "model",
                api::calcifer::hub::ModelKind::Component => "component",
            }));
        }
        if !model.tags.is_empty() { lines.push(format!("Tags: {}", model.tags.join(", "))); }
        if let Some(author) = &model.author { lines.push(format!("Publisher: {}", author.name)); }
        if model.role.as_deref() == Some("lora") || model.kind == Some(api::calcifer::hub::ModelKind::Lora) {
            if let Some(trigger) = &model.trigger { lines.push(format!("Trigger: {trigger}")); }
            if let Some(strength) = model.strength { lines.push(format!("Suggested strength: {strength}")); }
        }
        if let Some(release) = model.releases.first() {
            lines.push(format!("Release: {}", release.version));
            for lane in &release.lanes { lines.push(format!("{}: {}", lane.lane, output::bytes(lane.bytes.max(0) as u64))); }
        } else if !self.metadata_missing {
            lines.push("Private model — no published release".into());
        }
        lines
    }
}

pub(crate) struct Destinations {
    model: String,
    machines: Vec<(String, String)>,
}

impl Destinations {
    pub(crate) async fn open(ctx: &Context, model: String) -> Result<Self, Error> {
        let list: MachineList = ctx.calcifer().await?.get("/v1/machines").await?;
        let machines = list.machines.into_iter()
            .filter(|machine| matches!(machine.state, MachineState::Ready | MachineState::Stopped))
            .map(|machine| (machine.name, if machine.state == MachineState::Stopped { "starts on download".into() } else { "ready".into() }))
            .collect();
        Ok(Self { model, machines })
    }

    pub(crate) fn title(&self) -> String {
        if self.machines.is_empty() { return "No available machines — use cozy machine --help".into(); }
        format!("Download {} to", self.model)
    }

    pub(crate) fn rows(&self) -> Vec<Vec<String>> {
        self.machines.iter().map(|(name, state)| vec![name.clone(), state.clone()]).collect()
    }

    pub(crate) fn action(&self, key: KeyCode, selected: usize) -> Option<Action> {
        if key != KeyCode::Enter { return None; }
        self.machines.get(selected).map(|(machine, _)| Action::Download(DownloadChoice { model: self.model.clone(), machine: machine.clone() }))
    }
}

pub(crate) async fn remove(ctx: &Context, model: &str) -> Result<api::calcifer::RemovedModel, Error> {
    super::machine::remove_local(ctx, model).await
}

pub(crate) async fn download(ctx: &Context, choice: DownloadChoice) -> Result<std::process::ExitCode, Error> {
    let ask = ModelDownload { model: choice.model, name: None, alias: None, lane: None, source_format: None };
    super::machine::download_to(ctx, ask, Some(On::Machine(choice.machine))).await
}

fn same_model(left: &str, right: &str) -> bool {
    api::model_ref::Repo::parse(left).ok().zip(api::model_ref::Repo::parse(right).ok()).is_some_and(|(left, right)| left == right)
}

pub(super) async fn local_inventory(ctx: &Context) -> Option<Vec<HeldModel>> {
    let inventory: MachineModels = ctx.calcifer().await.ok()?.get("/v1/machines/local/models").await.ok()?;
    (!inventory.silent).then_some(inventory.models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::calcifer::hub::{FoundModel, ModelAuthor, ModelKind};

    fn catalog() -> Catalog {
        Catalog::new(Searching { hub: "https://example.test".into(), q: "fox".into(), family: Some("sdxl".into()), tag: "art".into(), no_tag: "nsfw".into(), role: "model".into(), limit: 2, offset: 2 },
            ModelSearch { models: vec![FoundModel { model: "author/one".into(), ..Default::default() }, FoundModel { model: "author/two".into(), ..Default::default() }], total: 5, older: 1 },
            Some(vec![HeldModel { model: "author/two".into(), bytes: 1024, ..Default::default() }] ))
    }

    #[test]
    fn browsing_and_paging_never_request_a_download() {
        let catalog = catalog();
        assert_eq!(catalog.action(KeyCode::Enter, 1), Some(Action::Preview("author/two".into())));
        assert_eq!(catalog.action(KeyCode::PageDown, 0), Some(Action::Page(4)));
        assert_eq!(catalog.action(KeyCode::PageUp, 0), Some(Action::Page(0)));
        assert_eq!(catalog.action(KeyCode::Down, 0), None);
        assert_eq!(catalog.rows()[0][3], "no");
        assert_eq!(catalog.rows()[1][3], "yes");
        assert_eq!(catalog.rows()[1][2], output::bytes(1024));
    }

    #[test]
    fn preview_is_concise_and_download_requires_destination_choice() {
        let preview = Preview { model: ModelDetail { model: "author/one".into(), family: Some("sdxl".into()), kind: Some(ModelKind::Lora), tags: vec!["art".into()], author: Some(ModelAuthor { name: "Artist".into(), ..Default::default() }), ..Default::default() }, local_bytes: Some(1024), metadata_missing: false };
        assert_eq!(preview.action(KeyCode::Enter), None);
        assert_eq!(preview.action(KeyCode::Char('d')), Some(Action::Destinations("author/one".into())));
        assert_eq!(preview.action(KeyCode::Char('r')), Some(Action::ConfirmRemove("author/one".into())));
        assert!(preview.lines().contains(&"Publisher: Artist".into()));
        let destinations = Destinations { model: "author/one".into(), machines: vec![("local".into(), "ready".into())] };
        assert_eq!(destinations.action(KeyCode::Down, 0), None);
        assert_eq!(destinations.action(KeyCode::Enter, 0), Some(Action::Download(DownloadChoice { model: "author/one".into(), machine: "local".into() })));
    }

    #[tokio::test]
    async fn terminal_navigation_and_explicit_download_use_fixtures_only() {
        use crossterm::event::{Event, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let home = tempfile::tempdir().unwrap();
        let ctx = Context::new(false, false, false, api::config::Loaded { config: Default::default(), home: platform::paths::Home::under(home.path()), user_home: home.path().into(), warnings: vec![] }, output::Term::detect());
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
        let keys = futures_util::stream::iter([KeyCode::Down, KeyCode::Char('q')].map(|code| Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))));
        assert_eq!(crate::tui::live(&ctx, &mut terminal, crate::tui::Screen::Models(catalog()), keys).await.unwrap(), None);
        let text = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(text.contains("author/two"));
        let screen = crate::tui::Screen::ModelDestinations(Destinations { model: "author/one".into(), machines: vec![("local".into(), "ready".into())] });
        let keys = futures_util::stream::iter([Ok(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))]);
        assert_eq!(crate::tui::live(&ctx, &mut terminal, screen, keys).await.unwrap(), Some(Operation::Download(DownloadChoice { model: "author/one".into(), machine: "local".into() })));
        let screen = crate::tui::Screen::RemoveLocalModel("author/one".into());
        let keys = futures_util::stream::iter([Ok(Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)))]);
        assert_eq!(crate::tui::live(&ctx, &mut terminal, screen, keys).await.unwrap(), None);
        let screen = crate::tui::Screen::RemoveLocalModel("author/one".into());
        let keys = futures_util::stream::iter([Ok(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))]);
        assert_eq!(crate::tui::live(&ctx, &mut terminal, screen, keys).await.unwrap(), Some(Operation::Remove("author/one".into())));
    }

    #[test]
    fn local_catalog_uses_authoritative_roles_and_lists_local_aliases() {
        let mut query = catalog().query;
        query.offset = 0;
        query.q.clear();
        let local = vec![
            HeldModel { model: "local/weights".into(), role: "model".into(), bytes: 100, ..Default::default() },
            HeldModel { model: "local/adapter".into(), role: "lora".into(), bytes: 20, ..Default::default() },
            HeldModel { model: "local/unknown".into(), ..Default::default() },
        ];
        let models = Catalog::from_local(query.clone(), local.clone());
        assert_eq!(models.rows().len(), 1);
        assert_eq!(models.rows()[0][0], "local/weights");
        query.role = "lora".into();
        let loras = Catalog::from_local(query, local);
        assert_eq!(loras.rows()[0][0], "local/adapter");
        assert_eq!(loras.local_bytes("local/adapter"), Some(20));
    }

    #[test]
    fn refresh_preserves_selected_model_when_rows_are_inserted() {
        let mut catalog = catalog();
        let selected = catalog.model_at(1).unwrap().to_owned();
        catalog.page.models.insert(0, FoundModel { model: "author/new".into(), ..Default::default() });
        assert_eq!(catalog.selection_after(Some(&selected), 1), 2);
        catalog.page.models.pop();
        assert_eq!(catalog.selection_after(Some(&selected), 2), 1);
    }
}
