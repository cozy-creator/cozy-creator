//! Resolve legacy custody from the actual registered local worker. A controller
//! ask digest, memo digest or reconstructed invocation is never a worker binding.
use std::{collections::{HashMap,HashSet},sync::Arc};
use api::{calcifer::MachineKind,error::{Error,Kind},machine::ProjectExecutionMigrationReceipt};
use crate::{api::Api,private_io::Scope,store::Endpoint};

fn pending()->Error {Error::new(Kind::Conflict,"legacy_worker_custody_pending","the original worker acceptance and complete private custody still need verification")}
fn registered(api:&Api)->Result<Endpoint,Error> {
    let machine=api.calcifer.maintenance_store().machine("local").map_err(|_|pending())?.ok_or_else(pending)?;
    if machine.kind!=MachineKind::Local {return Err(pending());}
    let endpoint=machine.endpoint.ok_or_else(pending)?;
    let address:std::net::SocketAddr=endpoint.address.parse().map_err(|_|pending())?;
    if !address.ip().is_loopback() {return Err(pending());}
    Ok(endpoint)
}


/// The selectors must first pass Store::verify_legacy_worker_requests against the
/// unchanged old SQL. No read lease or database handle crosses these worker waits.
pub(crate) async fn migrate(api:&Arc<Api>,scope:Scope,selectors:&[(String,String,Option<u64>)])->Result<ProjectExecutionMigrationReceipt,Error> {
    if selectors.is_empty() {return Ok(ProjectExecutionMigrationReceipt::default());}
    if !api.machines.local.running() || selectors.iter().any(|(_,machine,_)|machine!="local") {return Err(pending());}
    let endpoint=registered(api)?;
    let base=api.calcifer.link("local").map_err(|_|pending())?.ok_or_else(pending)?;
    let identity=base.status().await.map_err(|_|pending())?;
    if identity.worker_id!=endpoint.worker_id {return Err(pending());}
    let selected=base.with_private(scope);
    let mut verified=ProjectExecutionMigrationReceipt::default();
    for batch in selectors.chunks(256) {
        let expected:HashMap<_,_>=batch.iter().map(|(id,_,number)|(id.as_str(),*number)).collect();
        if expected.len()!=batch.len() {return Err(pending());}
        let ids=batch.iter().map(|(id,_,_)|id.clone()).collect::<Vec<_>>();
        let actual=selected.legacy_bindings(&ids).await.map_err(|_|pending())?;
        let mut seen=HashSet::new();
        for record in actual.bindings {
            let binding=record.binding;
            let Some(number)=expected.get(binding.request_id.as_str()) else {return Err(pending());};
            // A missing controller ACK is not filled from a guess: this lookup
            // returns only an actual durable accepted submission with its number.
            if record.source_number==0 || number.is_some_and(|number|number!=record.source_number)
                || !seen.insert(binding.request_id.clone()) || binding.source_workspace_id.is_empty() || binding.invocation_spec_digest.is_empty() {return Err(pending());}
            verified.bindings.push(binding);
        }
        if seen.len()!=expected.len() {return Err(pending());}
    }
    if registered(api)?!=endpoint {return Err(pending());}
    selected.migrate_private_bindings(&verified).await.map_err(|_|pending())?;
    let complete=selected.migration_receipt().await.map_err(|_|pending())?;
    for expected in &verified.bindings {
        if !complete.bindings.iter().any(|actual|actual.request_id==expected.request_id && actual.source_workspace_id==expected.source_workspace_id && actual.invocation_spec_digest==expected.invocation_spec_digest) {return Err(pending());}
    }
    if registered(api)?!=endpoint {return Err(pending());}
    Ok(complete)
}
