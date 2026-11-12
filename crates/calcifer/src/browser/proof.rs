//! An inert-by-default hosted qualification channel on an inherited native Unix socket.
//! No HTTP route, public debugger, caller-selected JS, or durable proof payload is provided.
#[cfg(target_os = "linux")]
mod linux {
    use std::{os::fd::OwnedFd, sync::Arc, time::Duration};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    use crate::api::Api;
    use super::super::{Live, unavailable};

    const DOMAIN: &str = "cozy.project-containment.proof.v1";
    const SCRIPT: &str = include_str!("../../../../e2e/cli/containment/browser-fixed.js");
    const MAX_REQUEST: usize = 2 << 20;

    #[derive(Deserialize)]
    struct Hello { domain: String, parent: platform::process::Birth, calcifer: platform::process::Birth }
    #[derive(Deserialize)]
    #[serde(tag = "request", rename_all = "snake_case")]
    enum Request {
        Snapshot,
        ExerciseFixed { project: String, action: Action },
        CloseOwned,
    }
    #[derive(Deserialize)]
    #[serde(tag = "action", rename_all = "snake_case")]
    enum Action {
        Ready,
        Diagnose,
        Edit { asset: String, field: String, value: String },
        Observe { needle: String },
        Storage,
        HmrStart,
        HmrObserve,
        Media { path: String, #[serde(default)] range: bool, #[serde(default)] conditional: bool },
    }

    pub(super) fn start(api: Arc<Api>) {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") { return; }
        let Some(file) = platform::process::take_handed(5) else { return };
        // Inspect native metadata before reading anything from a possibly unrelated inherited fd.
        if rustix::net::sockopt::socket_domain(&file).ok() != Some(rustix::net::AddressFamily::UNIX)
            || rustix::net::sockopt::socket_type(&file).ok() != Some(rustix::net::SocketType::STREAM)
            || rustix::net::getpeername(&file).ok().flatten().is_none() { return; }
        let Ok(peer) = rustix::net::sockopt::socket_peercred(&file) else { return };
        if peer.uid != rustix::process::geteuid() || platform::process::parent_of(std::process::id()).ok() != Some(peer.pid.as_raw_nonzero().get() as u32) { return; }
        let Ok(parent) = platform::process::Birth::of(peer.pid.as_raw_nonzero().get() as u32) else { return };
        let Ok(own) = platform::process::Birth::own() else { return };
        // A queued domain prelude is peeked without consuming an unrelated inherited stream.
        let expected = format!("{DOMAIN}\n");
        let mut prelude = [0u8; 128];
        let Ok((_, count)) = rustix::net::recv(&file, &mut prelude[..expected.len()], rustix::net::RecvFlags::PEEK | rustix::net::RecvFlags::DONTWAIT) else { return };
        if count != expected.len() || prelude[..count] != *expected.as_bytes() { return; }
        let fd: OwnedFd = file.into();
        let socket = std::os::unix::net::UnixStream::from(fd);
        if socket.set_nonblocking(true).is_err() { return; }
        let Ok(socket) = tokio::net::UnixStream::from_std(socket) else { return };
        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut reader = BufReader::new(reader);
            let Ok(Some(prelude)) = tokio::time::timeout(Duration::from_secs(10), read(&mut reader)).await else { return };
            if prelude != format!("{DOMAIN}\n").as_bytes() { return; }
            let Ok(Some(hello)) = tokio::time::timeout(Duration::from_secs(10), read(&mut reader)).await else { return };
            let Ok(hello) = serde_json::from_slice::<Hello>(&hello) else { return };
            if hello.domain != DOMAIN || hello.parent != parent || hello.calcifer != own { return; }
            if write(&mut writer, json!({ "domain": DOMAIN, "ready": true, "calcifer": own })).await.is_err() { return; }
            while let Some(bytes) = read(&mut reader).await {
                if parent.ended().unwrap_or(true) { return; }
                let reply = match serde_json::from_slice::<Request>(&bytes) {
                    Ok(request) => match exercise(&api, request).await {
                        Ok(value) => json!({ "ok": true, "observed": value }),
                        Err(_) => json!({ "ok": false, "code": "proof_action_unavailable" }),
                    },
                    Err(_) => json!({ "ok": false, "code": "proof_request_invalid" }),
                };
                if write(&mut writer, reply).await.is_err() { return; }
            }
        });
    }

    async fn read(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        let length = reader.read_until(b'\n', &mut bytes).await.ok()?;
        (length > 0 && length <= MAX_REQUEST).then_some(bytes)
    }
    async fn write(writer: &mut tokio::net::unix::OwnedWriteHalf, value: Value) -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec(&value).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        writer.write_all(&bytes).await
    }
    async fn fixed(live: &mut Live, session: &str, arguments: Value) -> Result<Value, crate::api::Refusal> {
        let expression = format!("({SCRIPT})({arguments})");
        let mut result = live.pipe.call("Runtime.evaluate", json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }), Some(session)).await?;
        if result.get("exceptionDetails").is_some() { return Err(unavailable()); }
        Ok(result["result"]["value"].take())
    }
    async fn exercise(api: &Arc<Api>, request: Request) -> Result<Value, crate::api::Refusal> {
        let mut held = api.browser.live.lock().await;
        let Some(live) = held.as_mut() else {
            return match request { Request::Snapshot => Ok(json!({ "present": false, "retirement_incomplete": api.browser.retirement_incomplete.load(std::sync::atomic::Ordering::Acquire) })), _ => Err(unavailable()) };
        };
        match request {
            Request::Snapshot => {
                let targets = live.pipe.call("Target.getTargets", json!({}), None).await?;
                live.profile.observe(&live.birth)?;
                let pages = targets["targetInfos"].as_array().ok_or_else(unavailable)?.iter().filter(|target| target["type"] == "page" && target["browserContextId"] == live.context).count();
                let processes = super::super::OwnedBrowser::processes(&live.birth).into_iter().map(|process| process.birth().clone()).collect::<Vec<_>>();
                Ok(json!({ "present": true, "birth": live.birth, "profile": live.profile.path(), "pages": pages, "processes": processes,
                    "retirement_incomplete": api.browser.retirement_incomplete.load(std::sync::atomic::Ordering::Acquire) }))
            }
            Request::CloseOwned => {
                let targets = live.pipe.call("Target.getTargets", json!({}), None).await?;
                let targets = targets["targetInfos"].as_array().ok_or_else(unavailable)?;
                let ids = targets.iter().filter(|target| target["type"] == "page" && target["browserContextId"] == live.context).filter_map(|target| target["targetId"].as_str().map(str::to_owned)).collect::<Vec<_>>();
                for target in &ids { live.pipe.call("Target.closeTarget", json!({ "targetId": target }), None).await?; }
                Ok(json!({ "closed": ids.len() }))
            }
            Request::ExerciseFixed { project, action } => {
                let target = live.targets.get(&project).ok_or_else(unavailable)?.clone();
                let attached = live.pipe.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None).await?;
                let session = attached["sessionId"].as_str().ok_or_else(unavailable)?;
                let arguments = match action {
                    Action::Ready => json!({ "kind": "ready" }),
                    Action::Diagnose => json!({ "kind": "diagnose" }),
                    Action::Observe { needle } => json!({ "kind": "observe", "needle": needle }),
                    Action::Storage => json!({ "kind": "storage" }),
                    Action::HmrStart => {
                        if api.development_web.get().is_none() { return Err(unavailable()); }
                        json!({ "kind": "hmr_start" })
                    }
                    Action::HmrObserve => {
                        if api.development_web.get().is_none() { return Err(unavailable()); }
                        json!({ "kind": "hmr_observe" })
                    }
                    Action::Media { path, range, conditional } => json!({ "kind": "media", "project": project, "path": path, "range": range, "conditional": conditional }),
                    Action::Edit { asset, field, value } => {
                        let at = fixed(live, session, json!({ "kind": "locate", "asset": asset })).await?;
                        if at["found"] != true { return Ok(json!({ "typed": false, "located": false, "focused": false })); }
                        // A single click only selects a box; a person opens it for writing with a double click.
                        for count in [1, 2] {
                            for kind in ["mousePressed", "mouseReleased"] { live.pipe.call("Input.dispatchMouseEvent", json!({ "type": kind, "button": "left", "clickCount": count, "x": at["x"], "y": at["y"] }), Some(session)).await?; }
                        }
                        let focused = fixed(live, session, json!({ "kind": "focus", "asset": asset, "field": field })).await?;
                        if focused["found"] != true { return Ok(json!({ "typed": false, "located": true, "focused": false })); }
                        for kind in ["keyDown", "keyUp"] { live.pipe.call("Input.dispatchKeyEvent", json!({ "type": kind, "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "modifiers": 2 }), Some(session)).await?; }
                        live.pipe.call("Input.insertText", json!({ "text": value }), Some(session)).await?;
                        for kind in ["keyDown", "keyUp"] { live.pipe.call("Input.dispatchKeyEvent", json!({ "type": kind, "key": "Tab", "code": "Tab", "windowsVirtualKeyCode": 9 }), Some(session)).await?; }
                        return Ok(json!({ "typed": true }));
                    }
                };
                fixed(live, session, arguments).await
            }
        }
    }
}

pub(super) fn start(api: std::sync::Arc<crate::api::Api>) {
    #[cfg(target_os = "linux")]
    linux::start(api);
    #[cfg(not(target_os = "linux"))]
    let _ = api;
}
