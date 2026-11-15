//! Unseen box versions (cl-928), retained only in their owning project's existing journal.
use std::{collections::{BTreeMap, BTreeSet},sync::Arc};
use axum::{extract::{Path,State},http::StatusCode};
use api::error::{Error,Kind};
use rusqlite::params;
use super::{Folder,Project,graph::name_key,written::Boxed};
use crate::api::{Api,Refusal};

pub(super) type Unseen=BTreeMap<String,BTreeSet<u32>>;

pub(super) fn id_of(held:&Boxed)->String {
    if held.id.is_empty() {format!("name:{}",name_key(&held.name))} else {held.id.clone()}
}
fn unavailable()->Error {Error::new(Kind::Unavailable,"project_unseen","the project's unseen versions could not be read or kept")}

pub(super) fn read(folder:&Folder,project:&str)->Result<Unseen,Error> {
    folder.journal_read(|conn| {
        let mut statement=conn.prepare("SELECT box,number FROM project_unseen WHERE project=?1")?;
        let mut unseen=Unseen::new();
        for row in statement.query_map([project],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u32>(1)?)))? {
            let (held,number)=row?;unseen.entry(held).or_default().insert(number);
        }
        Ok(unseen)
    }).map_err(|_|unavailable())
}

pub(super) fn refresh(project:&Project)->Result<(),Error> {
    let mut unseen=project.unseen.lock().expect("unseen lock");
    *unseen=read(&project.folder,&project.id)?;
    Ok(())
}

pub(super) fn landed(project:&Project,name:&str,number:u32) {
    let Ok(_work)=super::vault::try_admit_work(project) else {return};
    let held=project.folder.reading_boxes(|written|written.get(name).filter(|held|held.get("approved").is_some_and(|value|!value.is_null())).map(id_of));
    let Ok(Some(held))=held else {return};
    match keep(project,&held,number,false) {
        Ok(true)=>crate::events::project_settled(&project.folder.dir),
        Ok(false)=>{},
        Err(_)=>tracing::warn!("project unseen versions could not be retained"),
    }
}

fn keep(project:&Project,held:&str,number:u32,seen:bool)->Result<bool,Error> {
    let mut unseen=project.unseen.lock().expect("unseen lock");
    let present=unseen.get(held).is_some_and(|numbers|numbers.contains(&number));
    if present!=seen {return Ok(false);}
    // Commit before changing the RAM projection, so a refused write is retryable.
    project.folder.journal_write(|tx| {
        if seen {tx.execute("DELETE FROM project_unseen WHERE project=?1 AND box=?2 AND number=?3",params![project.id,held,number])?;}
        else {tx.execute("INSERT OR IGNORE INTO project_unseen(project,box,number) VALUES(?1,?2,?3)",params![project.id,held,number])?;}
        Ok(())
    }).map_err(|_|unavailable())?;
    if seen {
        if let Some(numbers)=unseen.get_mut(held) {numbers.remove(&number);if numbers.is_empty() {unseen.remove(held);}}
    } else {unseen.entry(held.to_owned()).or_default().insert(number);}
    Ok(true)
}

pub(super) async fn seen(State(api):State<Arc<Api>>,Path((id,name,number)):Path<(String,String,String)>)->Result<StatusCode,Refusal> {
    let _view=api.projects.store(&api,&id).await.map_err(Refusal)?;
    let project=api.projects.get(&api,&id)?;
    project.require_view().map_err(Refusal)?;
    let number:u32=number.parse().map_err(|_|Refusal(Error::new(Kind::Validation,"version_unknown","the version number is invalid")))?;
    let held=project.folder.reading_boxes(|written|written.get(&name).map(id_of)).map_err(Refusal)?;
    let held=held.ok_or_else(||Refusal(Error::new(Kind::NotFound,"asset_unknown","the project has no such box")))?;
    if keep(&project,&held,number,true).map_err(Refusal)? {crate::events::project_settled(&project.folder.dir);}
    Ok(StatusCode::NO_CONTENT)
}
