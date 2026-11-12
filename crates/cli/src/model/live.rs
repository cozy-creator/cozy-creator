//! Keyboard browsing of one catalog page, with an explicit download destination.
use api::{
    calcifer::{HeldModel, MachineList, MachineModels, MachineState, On},
    calcifer::hub::{FoundModel, ModelDetail, ModelDownload, ModelSearch},
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

struct CatalogRow {
    model: Option<String>,
    fields: Vec<String>,
    hub: bool,
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
        if !self.local_only {
            let known = self.local_models(&self.local_records(false).unwrap_or_default());
            let local = known.len();
            let unknown = self.local_models(&self.local_records(true).unwrap_or_default()).iter()
                .filter(|row| !known.iter().any(|held| same_model(&held.model, &row.model))).count();
            let local_count = if self.local.is_some() { local.to_string() } else { "unknown".into() };
            let first = if self.page.models.is_empty() { 0 } else { self.query.offset as usize + 1 };
            let last = if self.page.models.is_empty() { 0 } else { self.query.offset as usize + self.page.models.len() };
            let page = format!("{} · Local {local_count} (+{unknown} unclassified) · Hub {first}–{last} of {}", self.label(), self.page.total);
            return self.notice.as_ref().map_or_else(|| page.clone(), |notice| format!("{page} · {notice}"));
        }
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
        } else if !self.local_only && (self.query.family.is_some() || !self.query.tag.is_empty() || !self.query.no_tag.is_empty()) {
            "↑/↓ · Enter preview · p/n Hub pages · family/tags filter Hub only · q back"
        } else if !self.local_only {
            "↑/↓ select · Enter preview · p/n Hub pages · q back"
        } else {
            "↑/↓ select · Enter preview · p/n local pages · q back"
        }
    }

    pub(crate) fn unclassified(&self) -> Self {
        let mut query = self.query.clone();
        query.role = "unknown".into();
        query.offset = 0;
        Self::from_local(query, self.local.clone().unwrap_or_default())
    }

    fn local_records(&self, unknown: bool) -> Option<Vec<HeldModel>> {
        let words: Vec<String> = self.query.q.split_whitespace().map(str::to_owned).collect();
        self.local.as_ref().map(|rows| rows.iter().filter(|row| {
            let role = if unknown { !matches!(row.role.as_str(), "model" | "lora") } else { row.role == self.query.role };
            role && output::names(&row.model, &words)
        }).cloned().collect())
    }

    fn local_models(&self, records: &[HeldModel]) -> Vec<FoundModel> {
        let mut models = std::collections::BTreeMap::new();
        for held in records {
            let key = api::model_ref::ModelId::parse(&held.model).map_or_else(|_| held.model.clone(), |model_id| model_id.to_string());
            models.entry(key).or_insert_with(|| self.page.models.iter().find(|row| same_model(&row.model, &held.model)).cloned().unwrap_or_else(|| FoundModel {
                model: held.model.clone(), role: Some(held.role.clone()), release: Some(held.release.clone()), lanes: vec![held.lane.clone()], ..Default::default()
            }));
        }
        models.into_values().collect()
    }

    fn sections(&self) -> Vec<(String, Vec<FoundModel>, bool)> {
        if self.local_only { return vec![(String::new(), self.page.models.clone(), false)]; }
        let local = self.local_models(&self.local_records(false).unwrap_or_default());
        let unknown: Vec<_> = self.local_models(&self.local_records(true).unwrap_or_default()).into_iter()
            .filter(|row| !local.iter().any(|known| same_model(&known.model, &row.model))).collect();
        let hub: Vec<_> = self.page.models.iter().filter(|row| !local.iter().chain(&unknown).any(|held| same_model(&held.model, &row.model))).cloned().collect();
        let label = if self.local.is_some() { format!("LOCAL ({})", local.len()) } else { "LOCAL (unknown)".into() };
        let mut sections = vec![(label, local, false)];
        if !unknown.is_empty() { sections.push((format!("UNCLASSIFIED ({})", unknown.len()), unknown, false)); }
        sections.push((format!("HUB PAGE ({})", hub.len()), hub, true));
        sections
    }

    fn fields(&self, row: &FoundModel) -> Vec<String> {
        let (local, bytes) = local_state(self.local.as_deref(), &row.model);
        // An unreleased model reads only to its owner, its org and admins (xs-173).
        let model = if row.private { format!("{} (private)", row.model) } else { row.model.clone() };
        vec![model, row.family.clone().unwrap_or_else(|| "—".into()), bytes.map_or_else(|| "—".into(), output::bytes), local.into()]
    }

    fn entries(&self) -> Vec<CatalogRow> {
        let mut entries = Vec::new();
        for (title, models, hub) in self.sections() {
            if !title.is_empty() { entries.push(CatalogRow { model: None, fields: vec![title, String::new(), String::new(), String::new()], hub }); }
            entries.extend(models.iter().map(|row| CatalogRow { model: Some(row.model.clone()), fields: self.fields(row), hub }));
        }
        entries
    }

    pub(crate) fn rows(&self) -> Vec<Vec<String>> { self.entries().into_iter().map(|row| row.fields).collect() }
    pub(crate) fn headings(&self) -> Vec<usize> { self.entries().iter().enumerate().filter_map(|(index, row)| row.model.is_none().then_some(index)).collect() }
    pub(crate) fn first_selection(&self) -> usize { self.entries().iter().position(|row| row.model.is_some()).unwrap_or(0) }
    pub(crate) fn page_selection(&self) -> usize { self.entries().iter().position(|row| row.hub && row.model.is_some()).unwrap_or_else(|| self.first_selection()) }
    pub(crate) fn navigate(&self, at: usize, down: bool) -> usize {
        let selected: Vec<_> = self.entries().iter().enumerate().filter_map(|(index, row)| row.model.is_some().then_some(index)).collect();
        if down { selected.iter().copied().find(|index| *index > at).or_else(|| selected.last().copied()).unwrap_or(0) }
        else { selected.iter().rev().copied().find(|index| *index < at).or_else(|| selected.first().copied()).unwrap_or(0) }
    }

    pub(super) fn tables(&self) -> Vec<(String, Vec<Vec<String>>)> {
        self.sections().into_iter().map(|(title, rows, _)| (title, rows.iter().map(|row| self.fields(row)).collect())).collect()
    }

    pub(super) fn document(&self) -> serde_json::Value {
        let mut document = serde_json::to_value(&self.page).expect("a catalog page serializes");
        document["local_models"] = serde_json::to_value(self.local_records(false)).expect("local inventory serializes");
        document["unclassified_local"] = serde_json::to_value(self.local_records(true)).expect("local inventory serializes");
        document
    }

    pub(crate) fn action(&self, key: KeyCode, selected: usize) -> Option<Action> {
        match key {
            KeyCode::Enter => self.model_at(selected).map(Action::Preview),
            KeyCode::PageDown | KeyCode::Char('n') if self.page.older > 0 => Some(Action::Page(self.query.offset.saturating_add(self.query.limit))),
            KeyCode::PageUp | KeyCode::Char('p') if self.query.offset > 0 => Some(Action::Page(self.query.offset.saturating_sub(self.query.limit))),
            KeyCode::Char('u') if self.local_only && self.query.role != "unknown" => Some(Action::Unclassified),
            _ => None,
        }
    }

    pub(crate) fn local_bytes(&self, model: &str) -> Option<u64> {
        self.local.as_ref()?.iter().filter(|held| same_model(&held.model, model)).map(|held| held.bytes).max()
    }

    pub(crate) fn model_at(&self, index: usize) -> Option<String> {
        self.entries().into_iter().nth(index).and_then(|row| row.model)
    }

    pub(crate) fn selection_after(&self, model: Option<&str>, previous: usize) -> usize {
        let entries = self.entries();
        model.and_then(|name| entries.iter().position(|row| row.model.as_deref().is_some_and(|shown| same_model(shown, name))))
            .unwrap_or_else(|| entries.iter().enumerate().filter_map(|(index, row)| row.model.is_some().then_some(index)).min_by_key(|index| index.abs_diff(previous)).unwrap_or(0))
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
            KeyCode::Char('i') if !self.metadata_missing => Some(Action::Destinations(self.model.model.clone())),
            KeyCode::Char('u') if self.local_bytes.is_some() => Some(Action::ConfirmRemove(self.model.model.clone())),
            _ => None,
        }
    }

    pub(crate) fn hint(&self) -> &str {
        if self.metadata_missing { "↑/↓ scroll · u uninstall local copy · Esc/q back" }
        else if self.local_bytes.is_some() { "↑/↓ scroll · i install · u uninstall local copy · Esc/q back" }
        else { "↑/↓ scroll · i install · Esc/q back" }
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        let model = &self.model;
        let mut lines = Vec::new();
        if self.metadata_missing { lines.push("Catalog details unavailable; showing the local copy.".into()); }
        if let Some(bytes) = self.local_bytes { lines.push(format!("Local size: {}", output::bytes(bytes))); }
        if let Some(family) = &model.family { lines.push(format!("Family: {family}")); }
        if let Some(kind) = model.model_kind {
            lines.push(format!("Kind: {}", match kind {
                api::model_ref::ModelKind::Base => "base model",
                api::model_ref::ModelKind::Lora => "LoRA",
            }));
        } else if model.role.as_deref() == Some("lora") {
            lines.push("Kind: LoRA".into());
        }
        if !model.tags.is_empty() { lines.push(format!("Tags: {}", model.tags.join(", "))); }
        if let Some(author) = &model.author { lines.push(format!("Publisher: {}", author.name)); }
        if model.role.as_deref() == Some("lora") || model.content_kind == Some(api::calcifer::hub::ModelContentKind::Lora) {
            if let Some(trigger) = &model.trigger { lines.push(format!("Trigger: {trigger}")); }
            if let Some(strength) = model.strength { lines.push(format!("Suggested strength: {strength}")); }
        }
        if let Some(release) = model.releases.first() {
            lines.push(format!("Release: {}{}", release.version, if release.visibility == api::model_ref::ReleaseVisibility::Private { " (private)" } else { "" }));
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
            .map(|machine| (machine.name, if machine.state == MachineState::Stopped { "starts on install".into() } else { "ready".into() }))
            .collect();
        Ok(Self { model, machines })
    }

    pub(crate) fn title(&self) -> String {
        if self.machines.is_empty() { return "No available machines — use cozy machine --help".into(); }
        format!("Install {} on", self.model)
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

fn local_state(models: Option<&[HeldModel]>, model: &str) -> (&'static str, Option<u64>) {
    let Some(models) = models else { return ("unknown", None) };
    let mut bytes = None;
    let (mut complete, mut partial, mut unknown) = (false, false, false);
    for held in models.iter().filter(|held| same_model(&held.model, model)) {
        bytes = Some(bytes.map_or(held.bytes, |size: u64| size.max(held.bytes)));
        match held.complete {
            Some(true) => complete = true,
            Some(false) => partial = true,
            None => unknown = true,
        }
    }
    let state = if complete { "yes" } else if unknown { "cached" } else if partial { "partial" } else { "no" };
    (state, bytes)
}

fn same_model(left: &str, right: &str) -> bool {
    left == right || api::model_ref::ModelId::parse(left).ok().zip(api::model_ref::ModelId::parse(right).ok()).is_some_and(|(left, right)| left == right)
}

pub(super) async fn local_inventory(ctx: &Context) -> Option<Vec<HeldModel>> {
    let inventory: MachineModels = ctx.calcifer().await.ok()?.get("/v1/machines/local/models").await.ok()?;
    (!inventory.silent).then_some(inventory.models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::calcifer::hub::{FoundModel, ModelAuthor, ModelContentKind};

    fn catalog() -> Catalog {
        Catalog::new(Searching { hub: "https://example.test".into(), q: String::new(), family: Some("sdxl".into()), tag: "art".into(), no_tag: "nsfw".into(), role: "model".into(), limit: 2, offset: 2, fits: None },
            ModelSearch { models: vec![FoundModel { model: "author/one".into(), ..Default::default() }, FoundModel { model: "author/two".into(), ..Default::default() }], total: 5, older: 1 },
            Some(vec![HeldModel { model: "author/two".into(), role: "model".into(), complete: Some(true), bytes: 1024, ..Default::default() }] ))
    }

    #[test]
    fn browsing_and_paging_never_request_an_install() {
        let catalog = catalog();
        assert_eq!(catalog.action(KeyCode::Enter, 1), Some(Action::Preview("author/two".into())));
        assert_eq!(catalog.action(KeyCode::PageDown, 0), Some(Action::Page(4)));
        assert_eq!(catalog.action(KeyCode::PageUp, 0), Some(Action::Page(0)));
        assert_eq!(catalog.action(KeyCode::Down, 0), None);
        let remote = catalog.selection_after(Some("author/one"), 0);
        let local = catalog.selection_after(Some("author/two"), 0);
        assert_eq!(catalog.rows()[remote][3], "no");
        assert_eq!(catalog.rows()[local][3], "yes");
        assert_eq!(catalog.rows()[local][2], output::bytes(1024));
        assert_eq!(catalog.rows().iter().filter(|row| row[0] == "author/two").count(), 1);
        assert!(catalog.headings().iter().all(|index| catalog.action(KeyCode::Enter, *index).is_none()));
    }

    #[test]
    fn preview_is_concise_and_install_requires_destination_choice() {
        let preview = Preview { model: ModelDetail { model: "author/one".into(), family: Some("sdxl".into()), content_kind: Some(ModelContentKind::Lora), tags: vec!["art".into()], author: Some(ModelAuthor { name: "Artist".into(), ..Default::default() }), ..Default::default() }, local_bytes: Some(1024), metadata_missing: false };
        assert_eq!(preview.action(KeyCode::Enter), None);
        assert_eq!(preview.action(KeyCode::Char('i')), Some(Action::Destinations("author/one".into())));
        assert_eq!(preview.action(KeyCode::Char('u')), Some(Action::ConfirmRemove("author/one".into())));
        assert!(preview.lines().contains(&"Publisher: Artist".into()));
        let destinations = Destinations { model: "author/one".into(), machines: vec![("local".into(), "ready".into())] };
        assert_eq!(destinations.action(KeyCode::Down, 0), None);
        assert_eq!(destinations.action(KeyCode::Enter, 0), Some(Action::Download(DownloadChoice { model: "author/one".into(), machine: "local".into() })));
    }

    #[tokio::test]
    async fn terminal_navigation_and_explicit_install_use_fixtures_only() {
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
        catalog.local = Some(vec![]);
        let previous = catalog.selection_after(Some("author/two"), 0);
        let selected = catalog.model_at(previous).unwrap();
        catalog.page.models.insert(0, FoundModel { model: "author/new".into(), ..Default::default() });
        assert_eq!(catalog.selection_after(Some(&selected), previous), previous + 1);
        catalog.page.models.pop();
        assert_eq!(catalog.selection_after(Some(&selected), previous + 1), previous);
    }
    #[test]
    fn local_presence_requires_a_complete_checkpoint() {
        let row = |complete, bytes| HeldModel { model: "author/model".into(), complete, bytes, ..Default::default() };
        assert_eq!(local_state(None, "author/model"), ("unknown", None));
        assert_eq!(local_state(Some(&[]), "author/model"), ("no", None));
        assert_eq!(local_state(Some(&[row(Some(false), 4)]), "author/model"), ("partial", Some(4)));
        assert_eq!(local_state(Some(&[row(None, 6)]), "author/model"), ("cached", Some(6)));
        assert_eq!(local_state(Some(&[row(Some(false), 4), row(None, 6)]), "author/model"), ("cached", Some(6)));
        assert_eq!(local_state(Some(&[row(None, 6), row(Some(true), 10)]), "author/model"), ("yes", Some(10)));
    }

    #[test]
    fn default_catalog_includes_local_only_copies_without_changing_hub_page() {
        let mut view = catalog();
        view.local = Some(vec![
            HeldModel { model: "local/external".into(), role: "model".into(), complete: Some(true), bytes: 50, ..Default::default() },
            HeldModel { model: "author/two".into(), role: "model".into(), complete: Some(false), bytes: 10, ..Default::default() },
            HeldModel { model: "local/unknown".into(), bytes: 4, ..Default::default() },
            HeldModel { model: "local/adapter".into(), role: "lora".into(), bytes: 2, ..Default::default() },
        ]);
        let selected = view.selection_after(Some("local/external"), 0);
        assert_eq!(view.model_at(selected).as_deref(), Some("local/external"));
        assert_eq!(view.action(KeyCode::Enter, selected), Some(Action::Preview("local/external".into())));
        assert!(view.rows().iter().any(|row| row[0] == "UNCLASSIFIED (1)"));
        assert!(!view.rows().iter().any(|row| row[0] == "local/adapter"));
        assert_eq!(view.rows().iter().filter(|row| row[0] == "author/two").count(), 1);
        let document = view.document();
        assert_eq!(document["models"].as_array().unwrap().len(), 2);
        assert_eq!(document["total"], 5);
        assert_eq!(document["older"], 1);
        assert_eq!(document["local_models"].as_array().unwrap().len(), 2);
        assert_eq!(document["unclassified_local"].as_array().unwrap().len(), 1);
        let preview = Preview { model: ModelDetail { model: "local/external".into(), ..Default::default() }, local_bytes: Some(50), metadata_missing: true };
        assert_eq!(preview.action(KeyCode::Char('u')), Some(Action::ConfirmRemove("local/external".into())));
    }

    #[test]
    fn local_name_filter_and_unavailable_inventory_are_explicit() {
        let mut view = catalog();
        view.query.q = "external".into();
        view.local = Some(vec![
            HeldModel { model: "local/external".into(), role: "model".into(), ..Default::default() },
            HeldModel { model: "local/other".into(), role: "model".into(), ..Default::default() },
        ]);
        assert_eq!(view.document()["local_models"].as_array().unwrap().len(), 1);
        assert!(view.rows().iter().any(|row| row[0] == "local/external"));
        view.local = None;
        assert!(view.document()["local_models"].is_null());
        assert!(view.rows().iter().any(|row| row[0] == "LOCAL (unknown)"));
    }

}
