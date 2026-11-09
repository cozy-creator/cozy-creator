//! A package's own UI (`docs/PACKAGE-UI.md`): the ES modules a function's `ui` names, as real modules in the page. Installing a package
//! is the trust decision, so its component loads once a machine you hold has it, with no prompt. What a pod has never decides the bytes:
//! a release's come from the Hub, held to its wheel's digest, and local code's from the folder the project names, and no code of it runs.
//! `import()` can carry no credential, so the modules sit below the session, at a URL only `find` says: named by the hash of what they
//! came from, which reaches those bytes and nothing else.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, Query, State, rejection::QueryRejection},
    http::header,
    response::{IntoResponse, Response},
};
use ::api::hub::{cards, types::PackageInstallFile};
use api::{
    error::{Error, Kind},
    interface::Interface,
    names::shown,
    calcifer::tool::ToolUi,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tensorfs::format::newest_by;

use crate::{
    api::{Api, Refusal},
    files::IMMUTABLE,
    local_code,
    page::POLICY,
    runs,
    toolbox::canonical,
};

/// How many components stay held, the oldest going first. One gone is read again by the next `find`.
const HELD: usize = 32;
/// What one component's modules may come to.
const MOST_BYTES: u64 = 16 << 20;
/// The largest wheel read for its component.
const MOST_WHEEL: u64 = 1 << 30;

type Files = BTreeMap<String, Bytes>;

/// What `find` has read for the page, in memory: nothing here outlives Calcifer, and a browser keeps its own copy.
#[derive(Default)]
pub(crate) struct Held(Mutex<VecDeque<Arc<Component>>>);

struct Component {
    id: String,
    /// The Hub's digest of its wheel, or the hash of local code's modules.
    hash: String,
    /// The release it's from: none for local code, which can change.
    release: String,
    /// The module to import, as its `ui` names it.
    entry: String,
    files: Files,
}

impl Held {
    fn all(&self) -> std::sync::MutexGuard<'_, VecDeque<Arc<Component>>> {
        self.0.lock().expect("held components")
    }

    fn hold(&self, component: Component) -> Arc<Component> {
        let component = Arc::new(component);
        let mut all = self.all();
        all.retain(|held| held.id != component.id || held.hash != component.hash);
        all.push_back(component.clone());
        if all.len() > HELD {
            all.pop_front();
        }
        component
    }

    fn released(&self, id: &str, release: &str) -> Option<Arc<Component>> {
        self.all().iter().find(|held| held.id == id && held.release == release).cloned()
    }

    fn file(&self, id: &str, hash: &str, file: &str) -> Option<Bytes> {
        self.all().iter().find(|held| held.id == id && held.hash == hash)?.files.get(file).cloned()
    }
}

#[derive(Deserialize)]
pub(crate) struct Asked {
    /// A project: local code it names can answer, and no other can.
    project: Option<String>,
}

/// `GET /v1/tools/{id}/ui`: where to import `id`'s component from, once its bytes are in hand and held to their hash.
pub(crate) async fn find(State(api): State<Arc<Api>>, UrlPath(id): UrlPath<String>, query: Result<Query<Asked>, QueryRejection>) -> Result<Json<ToolUi>, Refusal> {
    let Query(asked) = query?;
    let Some((package, function)) = id.rsplit_once('/') else {
        return Err(Refusal(none(&id)));
    };
    let named = match &asked.project {
        Some(project) => crate::project::local_code_named(&api, project)?,
        None => vec![],
    };
    let component = match named.into_iter().find(|(written, _)| written == package) {
        Some((_, path)) => local(&api, &id, &path, function).await,
        None => released(&api, &id, package, function).await,
    };
    let component = component.map_err(Refusal)?;
    Ok(Json(ToolUi { module: module_url(&component) }))
}

/// `GET /v1/tools/{id}/ui/{hash}/{*file}`: one module of a component, as `find` read it.
pub(crate) async fn file(State(api): State<Arc<Api>>, UrlPath((id, path)): UrlPath<(String, String)>) -> Result<Response, Refusal> {
    let bytes = path.split_once('/').and_then(|(hash, file)| api.tool_uis.file(&id, hash, file));
    let Some(bytes) = bytes else {
        return Err(Refusal(Error::new(Kind::NotFound, "tool_ui_file_not_found", format!("{id} has no such module held: {path}")).remedy("reload the page")));
    };
    let headers = [
        (header::CONTENT_TYPE, "application/javascript"),
        (header::CONTENT_SECURITY_POLICY, POLICY),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, IMMUTABLE),
    ];
    Ok((headers, bytes).into_response())
}

/// A component's entry, relative imports of it landing among its files. `id` is one segment, whatever it holds.
fn module_url(component: &Component) -> String {
    let mut url = url::Url::parse("http://calcifer/").expect("a constant URL");
    let mut segments = url.path_segments_mut().expect("an http URL has segments");
    segments.pop_if_empty().extend(["v1", "tools", &component.id, "ui", &component.hash]).extend(component.entry.split('/'));
    drop(segments);
    url.path().to_owned()
}

fn none(id: &str) -> Error {
    Error::new(Kind::NotFound, "tool_ui_none", format!("{id} ships no component"))
}

fn missing(id: &str, ui: &str) -> Error {
    Error::new(Kind::NotFound, "tool_ui_missing", format!("{id} names {ui}, and its package doesn't hold it"))
}

fn too_big(id: &str, what: &str) -> Error {
    Error::new(Kind::Validation, "tool_ui_too_big", format!("{id}: {what} is too big to read for a component"))
}

fn unreadable(id: &str, why: impl std::fmt::Display) -> Error {
    Error::new(Kind::Unavailable, "tool_ui_unreadable", format!("{id}: {why}"))
}

/// What `function` of `interface` names and the module that names it. Its path can't climb out of its package.
fn declared(interface: &Interface, function: &str, id: &str) -> Result<(String, String), Error> {
    let callable = interface.callables.iter().find(|callable| callable.name == function);
    let plain = |ui: &&String| !ui.contains('\\') && ui.split('/').all(|part| !matches!(part, "" | "." | ".."));
    let ui = callable.and_then(|callable| callable.tool.as_ref()?.ui.as_ref()).filter(plain).ok_or_else(|| none(id))?;
    Ok((ui.clone(), callable.map(|callable| callable.invocable.module.clone()).unwrap_or_default()))
}

/// Where a `ui` beside `module` can be, from its package's root: under the module, if it's a package, else beside it.
fn folders(module: &str) -> [String; 2] {
    let path = module.replace('.', "/");
    let beside = path.rsplit_once('/').map_or("", |(parent, _)| parent).to_owned();
    [path, beside]
}

fn joined(folder: &str, file: &str) -> String {
    if folder.is_empty() { file.to_owned() } else { format!("{folder}/{file}") }
}

/// Whether `path`, from the folder its `ui` is named from, is a module of that component: under the folder `ui` is in, or `ui` alone at the
/// top, so a package's own top level isn't read.
fn of_component(path: &str, ui: &str) -> bool {
    let module = path.ends_with(".js") || path.ends_with(".mjs");
    module && ui.rsplit_once('/').map_or(path == ui, |(folder, _)| path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')))
}

// A release

/// The component of `function` of `shown_package`, from the Hub, for the newest release a machine you hold has.
async fn released(api: &Arc<Api>, id: &str, shown_package: &str, function: &str) -> Result<Arc<Component>, Error> {
    let hub = runs::hub(api, "");
    let (package, release) = installed(api, &hub, shown_package).await.ok_or_else(|| {
        Error::new(Kind::NotFound, "tool_ui_not_installed", format!("{shown_package} is installed on no machine you hold, so its component doesn't load"))
            .remedy(format!("cozy package install {shown_package}"))
    })?;
    if let Some(held) = api.tool_uis.released(id, &release) {
        return Ok(held);
    }
    let client = || api.machines.hubs.reader(&hub);
    let card = cards::card(client, api.machines.hubs.home().root(), &hub, &package, &release).await?;
    let (ui, module) = declared(&card.interface, function, id)?;
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let plan = client()?.get_package_download_plan(org, name, &release, None).await.map_err(cards::refusal)?;
    let wheel = plan.downloads.iter().find(|file| wheel_of(&file.path, name)).ok_or_else(|| missing(id, &ui))?;
    let held = fetched(id, wheel).await?;
    let (id_owned, entry) = (id.to_owned(), ui.clone());
    let files = tokio::task::spawn_blocking(move || from_wheel(&id_owned, held, &module, &ui)).await.map_err(|why| unreadable(id, why))??;
    let hash = wheel.digest.strip_prefix("sha256:").unwrap_or(&wheel.digest).to_owned();
    Ok(api.tool_uis.hold(Component { id: id.to_owned(), hash, release, entry, files }))
}

/// `shown_package` as a machine you hold has it from `hub`: its whole name and its newest release on any of them.
async fn installed(api: &Arc<Api>, hub: &str, shown_package: &str) -> Option<(String, String)> {
    let here = canonical(hub);
    let machines = api.machines.list(false).await.ok()?.machines;
    let environments: Vec<_> = machines.iter().filter_map(|machine| api.machines.frame(&machine.name)).flat_map(|frame| frame.environments).collect();
    let held = environments.iter().filter(|env| !env.release.is_empty() && canonical(&env.hub) == here && shown(&env.package) == shown_package);
    newest_by(held, |env| env.release.as_str()).map(|env| (env.package.clone(), env.release.clone()))
}

/// Whether a wheel's file name is `name`'s own, not a dependency it carries.
fn wheel_of(file: &str, name: &str) -> bool {
    let distribution = file.split('-').next().unwrap_or_default();
    file.ends_with(".whl") && distribution.replace('.', "_").eq_ignore_ascii_case(&name.replace(['-', '.'], "_"))
}

/// `wheel`, held to the Hub's digest, in a file the OS takes back. Bytes that aren't what the Hub says go no further.
async fn fetched(id: &str, wheel: &PackageInstallFile) -> Result<File, Error> {
    let length = u64::try_from(wheel.length).unwrap_or(0);
    if length > MOST_WHEEL {
        return Err(too_big(id, &wheel.path));
    }
    let mut kept = tempfile::tempfile().map_err(|why| unreadable(id, why))?;
    let (mut hash, mut at) = (Sha256::new(), 0u64);
    // A server that starts the body over starts the file and its hash over.
    let mut sink = |start: u64, frame: &[u8]| -> tensorfs::err::Result<()> {
        let failed = |why: std::io::Error| tensorfs::err::Refusal { code: tensorfs::err::Code::IO_FAILED, detail: why.to_string() };
        if start != at {
            kept.set_len(0).and_then(|()| kept.rewind()).map_err(failed)?;
            (hash, at) = (Sha256::new(), 0);
        }
        at += frame.len() as u64;
        if start != 0 && start + frame.len() as u64 != at || at > MOST_WHEEL {
            return tensorfs::err::refuse(tensorfs::err::Code::ORIGIN_REFUSED, "not one wheel's bytes");
        }
        hash.update(frame);
        kept.write_all(frame).map_err(failed)
    };
    let got = tensorfs::fetch::get_pinned(&wheel.url, 0, &mut sink).await;
    if at > MOST_WHEEL {
        return Err(too_big(id, &wheel.path));
    }
    got.map_err(|why| unreadable(id, why))?;
    let got = format!("sha256:{}", hex::encode(hash.finalize()));
    if !got.eq_ignore_ascii_case(&wheel.digest) {
        return Err(Error::new(Kind::Unavailable, "tool_ui_hash_mismatch", format!("{id}: {} is {got}, and the Hub says {}, so it doesn't load", wheel.path, wheel.digest)));
    }
    kept.rewind().map_err(|why| unreadable(id, why))?;
    Ok(kept)
}

fn named(archive: &zip::ZipArchive<File>) -> BTreeSet<String> {
    archive.file_names().filter_map(Result::ok).map(|name| name.into_owned()).collect()
}

/// The modules a wheel holds of the component `ui` names.
fn from_wheel(id: &str, wheel: File, module: &str, ui: &str) -> Result<Files, Error> {
    let mut archive = zip::ZipArchive::new(wheel).map_err(|why| unreadable(id, why))?;
    let names = named(&archive);
    let folder = folders(module).into_iter().find(|folder| names.contains(&joined(folder, ui))).ok_or_else(|| missing(id, ui))?;
    let prefix = joined(&folder, "");
    let mut files = Files::new();
    let mut left = MOST_BYTES;
    for name in &names {
        let Some(path) = name.strip_prefix(&prefix).filter(|path| of_component(path, ui)) else { continue };
        let entry = archive.by_name(name).map_err(|why| unreadable(id, why))?;
        let mut bytes = Vec::new();
        entry.take(left + 1).read_to_end(&mut bytes).map_err(|why| unreadable(id, why))?;
        left = left.checked_sub(bytes.len() as u64).ok_or_else(|| too_big(id, ui))?;
        files.insert(path.to_owned(), bytes.into());
    }
    Ok(files)
}

/// What a wheel lacks of the components `interface` declares: what `package publish` warns of, as it would only show as the declared box.
pub(crate) fn not_in_wheel(wheel: &Path, interface: &Interface) -> Vec<String> {
    let names = File::open(wheel).ok().and_then(|file| zip::ZipArchive::new(file).ok()).map_or_else(BTreeSet::new, |archive| named(&archive));
    let gone = |callable: &&api::interface::Callable| -> Option<String> {
        let ui = callable.tool.as_ref()?.ui.as_ref()?;
        (!folders(&callable.invocable.module).iter().any(|folder| names.contains(&joined(folder, ui)))).then(|| ui.clone())
    };
    interface.callables.iter().filter_map(|callable| gone(&callable)).collect()
}

// Local code

/// The component of `function` of the project's local code at `path`, as its files are now. An edit to them makes another hash, so
/// another URL.
async fn local(api: &Arc<Api>, id: &str, path: &Path, function: &str) -> Result<Arc<Component>, Error> {
    let hub = runs::hub(api, "");
    let code = local_code::own(api.machines.hubs.home(), path)?;
    let (_, document) = local_code::describe(api, &code, &runs::default_on(api), &hub).await?;
    let interface = Interface::parse(&document).map_err(|why| unreadable(id, why))?;
    let (ui, module) = declared(&interface, function, id)?;
    let roots: Vec<PathBuf> = match path.is_file() {
        true => path.parent().map(Path::to_path_buf).into_iter().collect(),
        false => vec![path.to_path_buf(), path.join("src")],
    };
    let (id_owned, entry) = (id.to_owned(), ui.clone());
    let files = tokio::task::spawn_blocking(move || from_folder(&id_owned, &roots, &module, &ui)).await.map_err(|why| unreadable(id, why))??;
    let mut hash = Sha256::new();
    for (path, bytes) in &files {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(api.tool_uis.hold(Component { id: id.to_owned(), hash: hex::encode(hash.finalize()), release: String::new(), entry, files }))
}

/// The modules of the component `ui` names, from the folder of local code that holds it.
fn from_folder(id: &str, roots: &[PathBuf], module: &str, ui: &str) -> Result<Files, Error> {
    let candidates = roots.iter().flat_map(|root| folders(module).map(|folder| root.join(folder)));
    let base = candidates.into_iter().find(|base| base.join(ui).is_file()).ok_or_else(|| missing(id, ui))?;
    let mut files = Files::new();
    let mut left = MOST_BYTES;
    let mut read = |relative: String, at: &Path| -> Result<(), Error> {
        let bytes = fs::read(at).map_err(|why| unreadable(id, why))?;
        left = left.checked_sub(bytes.len() as u64).ok_or_else(|| too_big(id, ui))?;
        files.insert(relative, bytes.into());
        Ok(())
    };
    let Some((folder, _)) = ui.rsplit_once('/') else {
        read(ui.to_owned(), &base.join(ui))?;
        return Ok(files);
    };
    let mut waiting = vec![(folder.to_owned(), base.join(folder))];
    while let Some((relative, dir)) = waiting.pop() {
        for entry in fs::read_dir(&dir).map_err(|why| unreadable(id, why))? {
            let entry = entry.map_err(|why| unreadable(id, why))?;
            let kind = entry.file_type().map_err(|why| unreadable(id, why))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let inner = joined(&relative, &name);
            if kind.is_dir() {
                waiting.push((inner, entry.path()));
            } else if kind.is_file() && of_component(&inner, ui) {
                read(inner, &entry.path())?;
            }
        }
    }
    Ok(files)
}
