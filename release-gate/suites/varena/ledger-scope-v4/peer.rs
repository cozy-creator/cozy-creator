//! Actual ledger module client; no CUDA/NVML dependency or fabricated namespace identity.
#![deny(unsafe_code)]
#[allow(dead_code)]
mod ledger;
use ledger::{Device, Ledger, Physical, Scope, Sharing};
use serde_json::{Value, json};
use std::{fs, io::{self, BufRead, Write}, path::Path};

fn emit(value: Value) { println!("{value}"); io::stdout().flush().unwrap(); }
fn scope(value: Scope) -> Value {
    json!({"boot":value.boot,"uid":value.uid,"user_namespace":value.user_namespace,
           "pid_namespace":value.pid_namespace,"mount_namespace":value.mount_namespace})
}
fn snapshot(table: &Ledger) -> ledger::Result<Value> {
    let s=table.snapshot()?;
    let data=fs::read(table.path())?;
    let header:Vec<u64>=data[..128].chunks_exact(8).map(|b|u64::from_ne_bytes(b.try_into().unwrap())).collect();
    Ok(json!({"protocol_version":s.protocol_version,"path":table.path(),"generation":s.generation,
      "backing_device":s.backing_device,"backing_inode":s.backing_inode,"scope":scope(s.scope),"header":header,
      "rows":s.rows.iter().map(|r|json!({"pid":r.pid,"start_ticks":r.start_ticks,"slot":r.id.slot,
        "incarnation":r.id.incarnation,"last_use":r.last_use,"physical":r.physical.total().unwrap()})).collect::<Vec<_>>(),
      "sample_present":s.sample.is_some(),"round_present":s.round.is_some()}))
}
fn run() -> Result<(),Box<dyn std::error::Error>> {
    let args:Vec<_>=std::env::args().collect();
    if args.len()!=4 { return Err("usage: peer hold|inspect|nonblocking DIRECTORY DEVICE_METADATA_KEY".into()); }
    ledger::initialize()?;
    let local=Scope::current()?;
    let libraries:Vec<_>=fs::read_to_string("/proc/self/maps")?.lines()
       .filter(|l|l.contains("libcuda.so")||l.contains("libnvidia-ml"))
       .map(str::to_owned).collect();
    assert!(libraries.is_empty(),"metadata client loaded GPU libraries");
    let table=if args[1]=="nonblocking" { Ledger::open_nonblocking(Path::new(&args[2]),Device::parse(&args[3])?)? }
              else { Ledger::open(Path::new(&args[2]),Device::parse(&args[3])?)? };
    let mut participant=if args[1]=="hold" { Some(table.register(Sharing::Shared,0)?) } else {None};
    emit(json!({"ok":true,"operation":"open","pid":std::process::id(),"scope":scope(local),
       "device_metadata_key":args[3],"gpu_identity_discovered":false,"gpu_libraries":libraries,"snapshot":snapshot(&table)?}));
    if participant.is_none() { return Ok(()); }
    for line in io::stdin().lock().lines() {
        let line=line?;
        let result:ledger::Result<Value>=(|| {
            match line.as_str() {
                "snapshot"=>Ok(json!({"ok":true,"operation":"snapshot","snapshot":snapshot(&table)?})),
                "publish"=>{
                    participant.as_ref().ok_or(ledger::Error::Invalid("participant closed"))?
                       .publish(Physical::default(),0,ledger::monotonic_ns()?)?;
                    Ok(json!({"ok":true,"operation":"publish","snapshot":snapshot(&table)?}))
                },
                "reap"=>{
                    let reaped=table.reap_dead()?;
                    Ok(json!({"ok":true,"operation":"reap","reaped":reaped.len(),"snapshot":snapshot(&table)?}))
                },
                "migrate_mount"=>{
                    let before=Scope::current()?;
                    match rustix::thread::unshare(rustix::thread::UnshareFlags::NEWNS) {
                        Err(error)=>Ok(json!({"ok":true,"operation":"migrate_mount","qualified":false,
                            "unavailable_errno":error.raw_os_error(),"reason":error.to_string()})),
                        Ok(())=>{
                            let after=Scope::current()?;
                            let answer=table.snapshot();
                            Ok(json!({"ok":true,"operation":"migrate_mount","qualified":true,
                                "before":scope(before),"after":scope(after),"refused":answer.is_err(),
                                "error":answer.err().map(|e|e.to_string())}))
                        }
                    }
                },
                "close"|"exit"=>{
                    participant.take();
                    Ok(json!({"ok":true,"operation":line,"snapshot":snapshot(&table)?}))
                },
                _=>Err(ledger::Error::Invalid("unknown metadata client command")),
            }
        })();
        match result { Ok(value)=>emit(value),Err(error)=>emit(json!({"ok":false,"operation":line,"error":error.to_string()})) }
        if line=="exit" { break; }
    }
    Ok(())
}
fn main() {
    if let Err(error)=run() { emit(json!({"ok":false,"operation":"open","error":error.to_string()})); std::process::exit(2); }
}
