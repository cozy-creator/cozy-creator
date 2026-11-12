//! Copy, verify and receipt a legacy closure before retiring its exact source rows.
//! Unattributed rows stay in the fenced source. No global recovery copy is made.
use super::*;
use rusqlite::types::Value;
use sha2::{Digest as _, Sha256};

struct Rows {
    table: &'static str,
    columns: Vec<String>,
    values: Vec<Vec<Value>>,
}

impl Rows {
    fn load(conn: &Connection, table: &'static str, predicate: &str, key: &str) -> Result<Self> {
        let mut stmt = conn.prepare(&format!("SELECT * FROM legacy_project_{table} WHERE {predicate} ORDER BY 1,2"))?;
        let columns = stmt.column_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
        let count = columns.len();
        let values = stmt.query_map([key], |row| (0..count).map(|index| row.get(index)).collect::<rusqlite::Result<Vec<Value>>>())?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self { table, columns, values })
    }
    fn column(&self, name: &str) -> Result<usize> {
        self.columns.iter().position(|column| column == name).ok_or_else(|| corrupt("legacy journal column"))
    }
    fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.table.as_bytes());
        for column in &self.columns { hash.update((column.len() as u64).to_le_bytes()); hash.update(column.as_bytes()); }
        for row in &self.values {
            for value in row {
                match value {
                    Value::Null => hash.update([0]),
                    Value::Integer(value) => { hash.update([1]); hash.update(value.to_le_bytes()); }
                    Value::Real(value) => { hash.update([2]); hash.update(value.to_bits().to_le_bytes()); }
                    Value::Text(value) => { hash.update([3]); hash.update((value.len() as u64).to_le_bytes()); hash.update(value.as_bytes()); }
                    Value::Blob(value) => { hash.update([4]); hash.update((value.len() as u64).to_le_bytes()); hash.update(value); }
                }
            }
        }
        hex::encode(hash.finalize())
    }
    fn insert_verified(&self, tx: &Transaction<'_>, destination: &str) -> Result<()> {
        let names = self.columns.join(",");
        let marks = (1..=self.columns.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(",");
        let mut insert = tx.prepare(&format!("INSERT OR IGNORE INTO {destination}({names}) VALUES({marks})"))?;
        let matches = self.columns.iter().enumerate().map(|(index,column)| format!("{column} IS ?{}",index+1)).collect::<Vec<_>>().join(" AND ");
        let mut verify = tx.prepare(&format!("SELECT EXISTS(SELECT 1 FROM {destination} WHERE {matches})"))?;
        for row in &self.values {
            insert.execute(rusqlite::params_from_iter(row))?;
            if !verify.query_row(rusqlite::params_from_iter(row), |row| row.get::<_,bool>(0))? {
                return Err(StoreError::Protected("project_migration_conflict".into()));
            }
        }
        Ok(())
    }
    fn open(&mut self, index: usize, field: &str, context: impl Fn(&[Value]) -> Result<String>) -> Result<()> {
        let column = self.column(field)?;
        let row = &mut self.values[index];
        let context = context(row)?;
        row[column] = match &row[column] {
            Value::Text(text) if vault::is_payload(text.as_bytes()) => Value::Text(open_text(&context,text)?),
            Value::Blob(bytes) if vault::is_payload(bytes) => Value::Blob(open_bytes(&context,bytes)?),
            value => value.clone(),
        };
        Ok(())
    }
}

fn text(value: &Value) -> Result<&str> { if let Value::Text(value)=value { Ok(value) } else { Err(corrupt("legacy journal text")) } }
fn integer(value: &Value) -> Result<i64> { if let Value::Integer(value)=value { Ok(*value) } else { Err(corrupt("legacy journal integer")) } }
fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)", [name], |row| row.get(0))?)
}

impl Store {
    /// Only an admitted, mounted project can receive its legacy history. The source's
    /// immediate transaction spans import and retirement, so its closure cannot change.
    pub(crate) fn migrate_project(&self, project: &crate::project::ProjectStore, inputs: &Path) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|error| error.into_inner());
        if !table_exists(&writer,"legacy_project_runs")? { return Ok(()); }
        let source = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = project.execution_scope().1;
        let landing = crate::project::landing(root);
        let candidates: Vec<(String,String,Vec<u8>)> = {
            let mut stmt=source.prepare("SELECT id,out_dir,spec FROM legacy_project_runs")?;
            stmt.query_map([],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?.collect::<rusqlite::Result<_>>()?
        };
        for (id,out,spec) in candidates {
            let envelope_root=vault::payload_root(&spec).map_err(private_error)?;
            if envelope_root.as_deref()!=Some(root) && Path::new(&out)!=landing { continue; }
            // An envelope must open through this admitted view before any source is retired.
            if envelope_root.is_some() { open_bytes(&run_context(&id,"spec"),&spec)?; }
            let mut runs=Rows::load(&source,"runs","id=?1",&id)?;
            let mut outputs=Rows::load(&source,"outputs","run=?1",&id)?;
            let mut logs=Rows::load(&source,"run_logs","run=?1",&id)?;
            let mut asks=Rows::load(&source,"asks","run=?1",&id)?;
            let key=text(&runs.values[0][runs.column("idempotency_key")?])?.to_owned();
            let after=if table_exists(&source,"legacy_project_after_steps")? { Some(Rows::load(&source,"after_steps","key_sha256=?1",&key_digest(&key))?) } else { None };
            let source_digest=key_digest(&[runs.fingerprint(),outputs.fingerprint(),logs.fingerprint(),asks.fingerprint(),after.as_ref().map_or(String::new(),Rows::fingerprint)].join(":"));
            for field in ["spec","outcome","target","warnings","inputs","reason_code","reason","job"] {
                runs.open(0,field,|_| Ok(run_context(&id,field)))?;
            }
            let out_column=runs.column("out_dir")?;
            runs.values[0][out_column]=Value::Text(landing.to_string_lossy().into_owned());
            for index in 0..outputs.values.len() {
                let (name,idx,rev,exported,failed)=(outputs.column("name")?,outputs.column("idx")?,outputs.column("rev")?,outputs.column("exported_rev")?,outputs.column("failed_rev")?);
                for (field,revision) in [("label",rev),("checkpoint",rev),("path",exported),("export_error",failed)] {
                    outputs.open(index,field,|row| {
                        let revision=if matches!(row[revision],Value::Null) { integer(&row[rev])? } else { integer(&row[revision])? };
                        Ok(output_context(&id,text(&row[name])?,integer(&row[idx])? as u32,revision as u64,field))
                    })?;
                }
                // Retained output bytes must already be within this project's custody.
                // Unknown external trees/exports remain pending instead of being discarded.
                let path=outputs.column("path")?;
                if let Value::Text(path)=&outputs.values[index][path] {
                    if !vault::logical(Path::new(path)).starts_with(root) { return Err(StoreError::Protected("project_output_migration_pending".into())); }
                }
            }
            let seq=logs.column("seq")?;
            for index in 0..logs.values.len() { logs.open(index,"text",|row| Ok(log_context(&id,integer(&row[seq])? as u64)))?; }
            let ask_key=asks.column("key")?;
            for index in 0..asks.values.len() {
                for field in ["body","reason"] { asks.open(index,field,|row| Ok(format!("ask:{}:{field}",text(&row[ask_key])?)))?; }
            }
            let input_column=runs.column("inputs")?;
            let objects: Vec<(Digest,u64)>=serde_json::from_str(text(&runs.values[0][input_column])?).map_err(|_| corrupt("legacy inputs"))?;
            let retained=project.input_root()?;
            let state=text(&runs.values[0][runs.column("state")?])?;
            for (digest,length) in objects {
                let name=hex::encode(digest.bytes());
                let target=retained.join(&name);
                if target.is_file() {
                    let mut file=std::fs::File::open(&target).map_err(|_| StoreError::Protected("project_input_migration_pending".into()))?;
                    let mut hash=Sha256::new();
                    let size=std::io::copy(&mut file,&mut hash).map_err(|_| StoreError::Protected("project_input_migration_pending".into()))?;
                    if size!=length || Digest::from_bytes(hash.finalize().into())!=digest { return Err(StoreError::Protected("project_input_migration_pending".into())); }
                    continue;
                }
                let source_file=inputs.join(&name);
                if !source_file.exists() && state=="settled" { continue; }
                crate::copy_checked(&retained,&api::calcifer::StagedInput {path:source_file,digest,length},&target)
                    .map_err(|_| StoreError::Protected("project_input_migration_pending".into()))?;
            }
            let receipt=format!("controller-v7:run:{id}");
            project.write(|tx| {
                let prior:Option<String>=tx.query_row("SELECT digest FROM journal_migrations WHERE source=?1",[&receipt],|row|row.get(0)).optional()?;
                if prior.as_ref().is_some_and(|prior| prior!=&source_digest) { return Err(StoreError::Protected("project_migration_source_changed".into())); }
                runs.insert_verified(tx,"runs")?; outputs.insert_verified(tx,"outputs")?; logs.insert_verified(tx,"run_logs")?; asks.insert_verified(tx,"asks")?;
                if let Some(after)=&after { after.insert_verified(tx,"after_steps")?; }
                tx.execute("INSERT OR IGNORE INTO journal_migrations(source,digest,record) VALUES(?1,?2,'complete_run_closure')",params![receipt,source_digest])?;
                Ok(())
            })?;
            // FULL-synchronous project commit has returned; verify its durable receipt before
            // the old transaction may delete even one of the imported records.
            let verified=project.read(|conn| Ok(conn.query_row("SELECT digest FROM journal_migrations WHERE source=?1",[&receipt],|row|row.get::<_,String>(0))?))?;
            if verified!=source_digest { return Err(StoreError::Protected("project_migration_unverified".into())); }
            for table in ["outputs","run_logs","asks"] { source.execute(&format!("DELETE FROM legacy_project_{table} WHERE run=?1"),[&id])?; }
            if after.is_some() { source.execute("DELETE FROM legacy_project_after_steps WHERE key_sha256=?1",[key_digest(&key)])?; }
            source.execute("DELETE FROM legacy_project_runs WHERE id=?1",[&id])?;
        }
        // API mutation retry responses have their own explicit owner, never inferred from a
        // generic body without an absolute admitted project path or protected envelope.
        if table_exists(&source,"legacy_project_answers")? {
            let mut answers=Rows::load(&source,"answers","?1=?1","")?;
            for index in 0..answers.values.len() {
                let (key,body)=(answers.column("key")?,answers.column("body")?);
                let Value::Blob(bytes)=&answers.values[index][body] else { return Err(corrupt("legacy answer")) };
                let owner=vault::payload_root(bytes).map_err(private_error)?.or_else(|| answer_path(bytes).map(|path|vault::logical(&path)));
                if owner.as_ref().is_none_or(|owner| !owner.starts_with(root)) { continue; }
                let named=text(&answers.values[index][key])?.to_owned();
                let one=Rows::load(&source,"answers","key=?1",&named)?;
                let digest=one.fingerprint();
                answers.open(index,"body",|_| Ok(format!("answer:{named}:body")))?;
                let row=&answers.values[index];
                project.write(|tx| {
                    tx.execute("INSERT OR IGNORE INTO request_keys(key,digest,status,content_type,body,at_ms) VALUES(?1,?2,?3,?4,?5,?6)",rusqlite::params_from_iter(row))?;
                    let matches=tx.query_row("SELECT EXISTS(SELECT 1 FROM request_keys WHERE key IS ?1 AND digest IS ?2 AND status IS ?3 AND content_type IS ?4 AND body IS ?5 AND at_ms IS ?6)",rusqlite::params_from_iter(row),|row|row.get::<_,bool>(0))?;
                    if !matches { return Err(StoreError::Protected("project_retry_migration_conflict".into())); }
                    tx.execute("INSERT OR REPLACE INTO journal_migrations(source,digest,record) VALUES(?1,?2,'request_key')",params![format!("controller-v7:answer:{named}"),digest])?;
                    Ok(())
                })?;
                source.execute("DELETE FROM legacy_project_answers WHERE key=?1",[named])?;
            }
        }
        if table_exists(&source,"legacy_project_projects")? {
            let index=Rows::load(&source,"projects","id=?1",project.project_id())?;
            if !index.values.is_empty() {
                let fingerprint=index.fingerprint();
                // Retain old project-only activity inside the project; the global index is
                // already reduced to the allowed public reference columns.
                let record=format!("{:?}",index.values);
                project.write(|tx| { tx.execute("INSERT OR REPLACE INTO journal_migrations(source,digest,record) VALUES('controller-v7:index',?1,?2)",params![fingerprint,record])?; Ok(()) })?;
                source.execute("DELETE FROM legacy_project_projects WHERE id=?1",[project.project_id()])?;
            }
        }
        source.commit()?;
        drop(writer);
        self.cleanup_private_journal()
    }
}
