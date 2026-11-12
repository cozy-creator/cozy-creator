//! The page's types: the records of Calcifer's API the page reads and sends, and each type they name, once, as
//! ts-rs declares them. `scripts/web-types.sh` writes them to web/app/src/api/calcifer.ts and checks that the file
//! holds them. A JSON value is `unknown`: the page reads it as it finds it.
#![deny(unreachable_pub)]
use std::{
    any::TypeId,
    collections::{BTreeMap, HashSet},
};

use api::{
    calcifer::{self, hub, project, report, tool},
    error, interface,
};
use ts_rs::{Config, TS, TypeVisitor};

struct Declared {
    cfg: Config,
    seen: HashSet<TypeId>,
    decls: BTreeMap<String, Vec<(&'static str, String)>>,
}

impl TypeVisitor for Declared {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let name = T::ident(&self.cfg);
        let json = TypeId::of::<T>() == TypeId::of::<serde_json::Value>();
        let decl = match json {
            true => format!("export type {name} = unknown"),
            false => format!("{}export {}", T::docs().unwrap_or_default(), T::decl(&self.cfg)),
        };
        self.decls.entry(name).or_default().push((std::any::type_name::<T>(), decl));
        if !json {
            T::visit_dependencies(self);
        }
    }
}

fn main() {
    let mut v = Declared { cfg: Config::new().with_large_int("number"), seen: HashSet::new(), decls: BTreeMap::new() };
    v.visit::<error::Envelope>();
    v.visit::<interface::Snap>();
    v.visit::<calcifer::BrowserSession>();
    v.visit::<calcifer::CalciferStatus>();
    v.visit::<calcifer::RemoveModel>();
    v.visit::<calcifer::RemovedModel>();
    v.visit::<calcifer::ProjectShowReply>();
    v.visit::<calcifer::StreamEvent>();
    v.visit::<calcifer::Event>();
    v.visit::<calcifer::RunList>();
    v.visit::<calcifer::File>();
    v.visit::<calcifer::RunRequest>();
    v.visit::<calcifer::Submitted>();
    v.visit::<calcifer::Target>();
    v.visit::<calcifer::Play>();
    v.visit::<calcifer::MachineList>();
    v.visit::<calcifer::MachineLog>();
    v.visit::<calcifer::MachinePackages>();
    v.visit::<calcifer::MachineModels>();
    v.visit::<calcifer::MachineUpdate>();
    v.visit::<calcifer::Updating>();
    v.visit::<calcifer::NewMachine>();
    v.visit::<calcifer::Rented>();
    v.visit::<calcifer::Ended>();
    v.visit::<calcifer::Keepalive>();
    v.visit::<calcifer::RunQuote>();
    v.visit::<calcifer::RentalSkus>();
    v.visit::<project::Projects>();
    v.visit::<project::Open>();
    v.visit::<project::Opened>();
    v.visit::<project::Address>();
    v.visit::<project::VaultStatus>();
    v.visit::<project::VaultAsk>();
    v.visit::<project::VaultChanged>();
    v.visit::<project::VaultEvent>();
    v.visit::<project::State>();
    v.visit::<project::Asked>();
    v.visit::<project::Runs>();
    v.visit::<project::FitAsk>();
    v.visit::<project::Fit>();
    v.visit::<project::AssetVersions>();
    v.visit::<project::NewAsset>();
    v.visit::<project::AssetMade>();
    v.visit::<project::Copies>();
    v.visit::<project::Copied>();
    v.visit::<project::AssetPatch>();
    v.visit::<project::AssetPatched>();
    v.visit::<project::AssetDeleted>();
    v.visit::<project::VersionMoved>();
    v.visit::<project::VersionPatch>();
    v.visit::<project::VersionPatched>();
    v.visit::<project::Approve>();
    v.visit::<project::Approved>();
    v.visit::<project::Unpin>();
    v.visit::<project::Unpinned>();
    v.visit::<project::Machines>();
    v.visit::<project::Rename>();
    v.visit::<project::Renamed>();
    v.visit::<project::Clip>();
    v.visit::<project::Import>();
    v.visit::<project::Imported>();
    v.visit::<project::Canvas>();
    v.visit::<project::CanvasPatch>();
    v.visit::<project::CanvasPatched>();
    v.visit::<project::History>();
    v.visit::<project::HistoryStep>();
    v.visit::<project::HistoryStepped>();
    v.visit::<hub::Account>();
    v.visit::<hub::CodeRequest>();
    v.visit::<hub::Code>();
    v.visit::<hub::CodeAnswer>();
    v.visit::<hub::Proven>();
    v.visit::<hub::SignedOut>();
    v.visit::<hub::OrgClaim>();
    v.visit::<hub::Families>();
    v.visit::<hub::ModelSearch>();
    v.visit::<hub::ModelDetail>();
    v.visit::<hub::ModelEdit>();
    v.visit::<hub::ModelEdited>();
    v.visit::<hub::ModelReleaseAsk>();
    v.visit::<hub::ModelReleased>();
    v.visit::<hub::ModelUnpublished>();
    v.visit::<hub::ModelDownload>();
    v.visit::<hub::ModelUpload>();
    v.visit::<hub::ModelQuantize>();
    v.visit::<hub::RecordedCheckpoint>();
    v.visit::<hub::PackageSearch>();
    v.visit::<hub::PackageDetail>();
    v.visit::<hub::PackageInstall>();
    v.visit::<hub::PackageRemove>();
    v.visit::<hub::PackagesRemoved>();
    v.visit::<report::Report>();
    v.visit::<tool::Tools>();
    v.visit::<tool::ToolList>();
    v.visit::<tool::ToolListChange>();
    v.visit::<tool::Browsed>();
    v.visit::<tool::ToolUi>();
    let twice: Vec<_> = v.decls.iter().filter(|(_, rust)| rust.len() > 1).collect();
    if !twice.is_empty() {
        for (name, rust) in twice {
            let rust: Vec<_> = rust.iter().map(|(rust, _)| *rust).collect();
            eprintln!("ts: {} are all `{name}` in TypeScript: #[ts(rename)] all but one", rust.join(", "));
        }
        std::process::exit(1);
    }
    println!("// Calcifer's API, as crates/api declares it: generated by scripts/web-types.sh --write. Do not edit.\n");
    for (index, decl) in v.decls.values().flatten().map(|(_, decl)| decl).enumerate() {
        if index > 0 { println!(); }
        for line in decl.lines() { println!("{}", line.trim_end()); }
    }
}
