//! Actual metadata-only ledger process; never opens a CUDA participant or substitutes a GPU.
use ledger_gate::ledger::{Bytes, Device, Ledger, Physical, Sharing};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;

fn emit(value: Value) { println!("{value}"); io::stdout().flush().unwrap(); }
fn show(table: &Ledger) -> ledger_gate::ledger::Result<Value> {
    let value = table.snapshot()?;
    Ok(json!({"inode":value.backing_inode,"generation":value.generation,"capacity_epoch":value.capacity_epoch,
        "rows":value.rows.iter().map(|row| {
            let mut value = json!({"pid":row.pid,"slot":row.id.slot,"incarnation":row.id.incarnation,
                "priority":row.priority,"want":row.want,"reclaimable":row.reclaimable});
            #[cfg(feature="v3")]
            { value["activity"] = format!("{:?}",row.activity).into(); value["requestable"] = row.requestable.into(); }
            value
        }).collect::<Vec<_>>(),
        "sample":value.sample.map(|sample|json!({"reserve":sample.reserve,"used":sample.driver_used,"attributable":sample.attributable})),
        "round":value.round.map(|round| {
            let mut value = json!({"generation":round.generation,"requested":round.requested,"finished":round.finished(),
                "completed":round.completed().unwrap(),"donors":round.donations.iter().map(|donor|donor.owner.slot).collect::<Vec<_>>()});
            #[cfg(feature="v3")]
            { value["kind"] = format!("{:?}",round.kind).into();
              value["assignments"] = json!(round.donations.iter().map(|donor|json!({"slot":donor.owner.slot,
                "requested":donor.requested,"held":donor.held_at_ask,"offered":donor.offered_at_ask,
                "requestable":donor.requestable,"action":donor.action,"completed":donor.completed,
                "pending":donor.pending,"answer":format!("{:?}",donor.answer)})).collect::<Vec<_>>()); }
            value
        })}))
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = &args[1];
    #[cfg(feature="v3")]
    if mode == "migrate-held" {
        let value = ledger_gate::ledger::gate_migrate(Path::new(&args[2]));
        emit(match value { Ok(()) => json!({"committed":true}), Err(error) => json!({"error":error.to_string()}) });
        return;
    }
    let device = Device::parse("GPU-13800000-0000-0000-0000-000000000003").unwrap();
    let open = if mode == "try" { Ledger::open_nonblocking(Path::new(&args[2]),device) }
        else { Ledger::open(Path::new(&args[2]),device) };
    let table = match open { Ok(table) => table, Err(error) => {emit(json!({"error":error.to_string()}));return} };
    let priority = args.get(3).map_or(0, |value|value.parse().unwrap());
    let mut peer = if mode == "hold" { Some(table.register(Sharing::Shared, priority).unwrap()) } else { None };
    emit(json!({"ready":true,"pid":std::process::id(),"path":table.path(),"table":show(&table).unwrap()}));
    if mode == "try" { return; }
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts[0] == "quit" { break; }
        let value = (|| -> ledger_gate::ledger::Result<Value> {
            match parts[0] {
                "snapshot" => show(&table),
                "reap" => Ok(json!({"reaped":table.reap_dead()?.len()})),
                "register" => {peer = Some(table.register(Sharing::Shared,priority)?);Ok(json!({"registered":true}))},
                "close" => {drop(peer.take());Ok(json!({"closed":true}))},
                "sample" => {
                    let reserve = parts.get(1).filter(|value|**value!="unknown").map(|value|value.parse().unwrap());
                    let before = table.snapshot()?;
                    peer.as_ref().unwrap().sample(before.generation,1024,512,1,reserve,reserve.is_some())?;
                    show(&table)
                },
                "ask" => {
                    let sample = table.snapshot()?.sample.unwrap();
                    peer.as_ref().unwrap().ask(sample.generation,32)?;
                    show(&table)
                },
                "withdraw" => {
                    let round = table.snapshot()?.round.unwrap();
                    Ok(json!({"changed":peer.as_ref().unwrap().withdraw(round.generation)?}))
                },
                #[cfg(feature="v3")]
                "policy" => {
                    use ledger_gate::ledger::Activity;
                    let activity = match parts[1] {"idle"=>Activity::Idle,"active"=>Activity::Active,_=>Activity::Unknown};
                    let physical = Physical {working:Bytes{idle:64,..Bytes::default()},..Physical::default()};
                    peer.as_ref().unwrap().publish_policy(physical,64,0,Some((activity,parts[2]=="yes")))?;
                    show(&table)
                },
                #[cfg(feature="v3")]
                "requestable" => {
                    peer.as_ref().unwrap().publish_policy(Physical::default(),0,0,
                        Some((ledger_gate::ledger::Activity::Idle,true)))?;
                    show(&table)
                },
                #[cfg(feature="v3")]
                "publish" => {
                    let activity = match parts[5] { "idle"=>ledger_gate::ledger::Activity::Idle,
                        "active"=>ledger_gate::ledger::Activity::Active,_=>ledger_gate::ledger::Activity::Unknown };
                    let physical = Physical {working:Bytes{live:parts[1].parse().unwrap(),idle:parts[2].parse().unwrap(),pending:0},..Physical::default()};
                    peer.as_ref().unwrap().publish_policy(physical,parts[3].parse().unwrap(),0,Some((activity,parts[4]=="yes")))?;
                    show(&table)
                },
                #[cfg(feature="v3")]
                "ack" => {
                    let round = table.snapshot()?.round.unwrap();
                    let answer = match parts[4] {"complete"=>ledger_gate::ledger::Answer::Complete,
                        "pending"=>ledger_gate::ledger::Answer::Pending,_=>ledger_gate::ledger::Answer::Refused};
                    peer.as_ref().unwrap().acknowledge_stage(round.generation,parts[1]=="action",
                        parts[2].parse().unwrap(),parts[3].parse().unwrap(),answer)?;
                    show(&table)
                },
                #[cfg(feature="v3")]
                "ask-idle" => {
                    let sample = table.snapshot()?.sample.unwrap();
                    peer.as_ref().unwrap().ask_idle(sample.generation)?;
                    show(&table)
                },
                _ => Err(ledger_gate::ledger::Error::Invalid("gate command")),
            }
        })();
        emit(match value { Ok(value) => value, Err(error) => json!({"error":error.to_string()}) });
    }
}
