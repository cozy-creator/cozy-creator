//! `POST /v1/responses` (xs-209): OpenAI's Responses API over Calcifer's runs, for any program that speaks it (the official SDKs).
//! A request is one ordinary run of the function `model` names, in the project `metadata.project` names: recorded, placed and followed
//! as any run is, so `cozy run list` shows it. Its words come as its machine makes them (`Event::OutputTextDelta`); `stream: true` says
//! each as a server-sent event as it comes, else the Response is answered once the run ends. A client that leaves a streamed response
//! cancels its run. It is one more route under Calcifer's one access rule: a program brings the CLI's bearer as its API key.
//!
//! `model` is the function as `cozy run` names it, `[org/]pkg/fn[@release]`, and `:<checkpoint>` after it sets its model slot
//! (`qwen3/write`, `qwen3/caption:huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0`). `input` is a string or one user message of
//! `input_text`, `input_image` and `input_video` parts (Cozy's, shaped as `input_image`, with `video_url`); a picture or video comes as a
//! `data:` URL or as `file_id`, a file Calcifer holds by its `sha256:<hex>`. No URL is fetched. A function with a prompt (`write`) reads
//! them as its words with each file mentioned where it stands (`<image_1>`); one with `prompt`, `images` and `videos` (`ask`) reads the
//! words, then the files. `instructions` is its `system`, `max_output_tokens` its `max_tokens`; `temperature` and `top_p` as named;
//! `reasoning.effort` `none` turns `thinking` off, any other on at that `reasoning_effort`.
use std::{collections::VecDeque, convert::Infallible, sync::Arc};

use axum::{
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    response::{IntoResponse as _, Response, sse::{self, KeepAlive, Sse}},
    Json,
};
use ::api::{
    calcifer::{Event, Run, RunRequest, Status},
    error::{Error, Kind},
    ids::RunId,
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, watch};

use crate::api::{Api, Refusal};

/// `GET /v1/models` (xs-209): the tools that write words, as OpenAI's model list says them, each by the name `cozy run` and
/// `POST /v1/responses` take it (`qwen3/write`), so a program that lists models first finds them.
pub(crate) async fn models(State(api): State<Arc<Api>>) -> Result<Response, Refusal> {
    let tools = crate::toolbox::listed(&api).await?;
    let data: Vec<Value> = (tools.iter().filter(|tool| tool.makes == "text"))
        .map(|tool| {
            let owner = tool.package.split_once('/').map_or("cozy", |(org, _)| org);
            json!({"id": tool.id, "object": "model", "created": 0, "owned_by": owner})
        })
        .collect();
    Ok(Json(json!({"object": "list", "data": data})).into_response())
}

/// The most a request's body holds: its pictures and videos ride in it as `data:` URLs.
pub(crate) const MAX_BODY: usize = 256 << 20;

#[derive(Deserialize)]
struct Asked {
    model: String,
    input: Input,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    reasoning: Option<Reasoning>,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default)]
    stream: bool,
    /// What it asks that no function here serves: refused where it would change the answer, else let be.
    #[serde(default)]
    tools: Vec<Value>,
    #[serde(default)]
    previous_response_id: Option<String>,
    #[serde(default)]
    conversation: Option<Value>,
    #[serde(default)]
    background: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Input {
    Text(String),
    Items(Vec<Item>),
}

#[derive(Deserialize)]
struct Item {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Option<Content>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Parts(Vec<Part>),
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    /// The Responses API's string; Chat Completions' `{url}` is read too.
    #[serde(default)]
    image_url: Option<Value>,
    #[serde(default)]
    video_url: Option<Value>,
    #[serde(default)]
    file_id: Option<String>,
}

#[derive(Deserialize)]
struct Reasoning {
    #[serde(default)]
    effort: Option<String>,
}

/// One piece of the question, in order: words, or a file by its digest and kind.
enum Piece {
    Words(String),
    File { kind: &'static str, file: String },
}

fn invalid(code: &str, message: impl Into<String>) -> Refusal {
    Refusal(Error::new(Kind::Validation, code, message))
}

/// `POST /v1/responses`.
pub(crate) async fn create(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let body = body?;
    let asked: Asked = serde_json::from_slice(&body).map_err(|e| invalid("invalid_request", format!("Cozy couldn't read this Responses request: {e}")))?;
    let unserved = [
        ("tools", !asked.tools.is_empty()),
        ("previous_response_id", asked.previous_response_id.is_some()),
        ("conversation", asked.conversation.is_some()),
        ("background", asked.background),
    ];
    if let Some((name, _)) = unserved.iter().find(|(_, asked)| *asked) {
        return Err(Refusal(Error::new(Kind::Validation, "unsupported_parameter", format!("Cozy's models answer one question at a time: `{name}` isn't served."))));
    }
    let Some(project) = asked.metadata.get("project").and_then(Value::as_str).map(str::to_owned) else {
        return Err(invalid("project_required", "Name the project its run belongs to in `metadata.project`."));
    };
    let project_id = api.projects.store(&api, &project).await.map_err(Refusal)?.project_id().to_owned();
    let answerer = api.clone();
    crate::project::vault::project_reply(&api, Some(&project), async move {
        let api = answerer;
        let (system, pieces) = question(&api, &project_id, &asked)?;
        let ask = request(&api, &project_id, &asked, system, pieces).await?;
        // Subscribed first: whatever its run says after it's recorded is in the tail.
        let tail = api.calcifer.subscribe();
        let key = format!("responses-{}", hex::encode(random()));
        let submitted = crate::runs::request(&api, &key, ask).await.map_err(Refusal)?;
        let answering = Answering::new(&api, &asked, submitted.run, tail);
        if asked.stream {
            let stream = futures_util::stream::unfold(answering, Answering::next);
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response());
        }
        let response = answering.wait().await;
        Ok(Json(response).into_response())
    })
    .await
}

fn random() -> [u8; 12] {
    let mut bytes = [0; 12];
    getrandom::fill(&mut bytes).expect("the OS provides randomness");
    bytes
}

/// What the request asks, its system turn (`instructions` and any system or developer message) and its question's pieces, each picture
/// or video held for its project's runs.
fn question(api: &Arc<Api>, project: &str, asked: &Asked) -> Result<(String, Vec<Piece>), Refusal> {
    let mut system: Vec<String> = asked.instructions.iter().filter(|said| !said.is_empty()).cloned().collect();
    let mut pieces = vec![];
    let items = match &asked.input {
        Input::Text(text) => return Ok((system.join("\n\n"), vec![Piece::Words(text.clone())])),
        Input::Items(items) => items,
    };
    for item in items {
        if !matches!(item.kind.as_str(), "" | "message") {
            return Err(invalid("unsupported_input", format!("Cozy's models read messages, not `{}` items.", item.kind)));
        }
        let parts = match &item.content {
            Some(Content::Text(text)) => vec![Piece::Words(text.clone())],
            Some(Content::Parts(parts)) => parts.iter().map(|part| piece(api, project, part)).collect::<Result<_, _>>()?,
            None => vec![],
        };
        match item.role.as_str() {
            "system" | "developer" => system.extend(parts.into_iter().filter_map(|piece| match piece {
                Piece::Words(words) => Some(words),
                Piece::File { .. } => None,
            })),
            "" | "user" => pieces.extend(parts),
            role => return Err(invalid("conversation_unsupported", format!("Cozy's models answer one question: send one user message, not a `{role}` one."))),
        }
    }
    if !pieces.iter().any(|piece| matches!(piece, Piece::Words(words) if !words.trim().is_empty())) {
        return Err(invalid("input_empty", "The request asks nothing: its user message has no words."));
    }
    Ok((system.join("\n\n"), pieces))
}

fn piece(api: &Arc<Api>, project: &str, part: &Part) -> Result<Piece, Refusal> {
    let kind = match part.kind.as_str() {
        "input_text" | "text" => return Ok(Piece::Words(part.text.clone())),
        "input_image" => "image",
        "input_video" => "video",
        other => return Err(invalid("unsupported_input", format!("Cozy's models read words, pictures and videos, not `{other}`."))),
    };
    if let Some(file) = part.file_id.as_deref().filter(|file| !file.is_empty()) {
        return Ok(Piece::File { kind, file: file.to_owned() });
    }
    let url = match (kind, &part.image_url, &part.video_url) {
        ("image", Some(url), _) | ("video", _, Some(url)) => url.as_str().or_else(|| url.get("url").and_then(Value::as_str)).unwrap_or_default(),
        _ => "",
    };
    let Some((_, data)) = url.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,")) else {
        return Err(invalid("url_input_unsupported", format!("Send the {kind} as a `data:` URL, or as `file_id`: a file Calcifer holds, by its sha256.")));
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| invalid("invalid_data_url", format!("The {kind}'s `data:` URL isn't base64.")))?;
    let digest = crate::runs::files::hold_in(api, project, &bytes).map_err(Refusal)?;
    Ok(Piece::File { kind, file: digest.to_string() })
}

/// The run that answers it: `model`'s function, its fields filled from the request as its interface names them.
async fn request(api: &Arc<Api>, project: &str, asked: &Asked, system: String, pieces: Vec<Piece>) -> Result<RunRequest, Refusal> {
    let (target, checkpoint) = match asked.model.split_once(':') {
        Some((target, checkpoint)) => (target.trim(), Some(checkpoint.trim())),
        None => (asked.model.trim(), None),
    };
    if target.is_empty() {
        return Err(invalid("model_required", "Name the function that answers in `model`, as `cozy run` names it: `qwen3/write`."));
    }
    let hub = crate::runs::hub(api, "");
    let on = crate::project::machines_of(api, project).map_err(Refusal)?.into_iter().next().unwrap_or_else(|| crate::runs::default_on(api));
    let root = crate::project::named(api, project).map_err(Refusal)?;
    let scope = crate::private_io::of_output(&crate::project::landing(&root)).map_err(Refusal)?;
    let described = crate::private_io::context(scope, crate::runs::target::describe(api, &hub, target, None, None, &on)).await.map_err(Refusal)?;
    let fields: Vec<(String, Value)> = (described.request.as_ref().and_then(|request| request["fields"].as_array()).into_iter().flatten())
        .filter_map(|field| Some((field["name"].as_str()?.to_owned(), field["type"].clone())))
        .collect();
    let has = |name: &str| fields.iter().any(|(field, _)| field == name);
    let mut input = Map::new();
    if let Some((prompt, _)) = fields.iter().find(|(_, ty)| ty.get("prompt").is_some()) {
        // Its words, each file mentioned where it stands, and the files as the run's own references, which its mentions send.
        let (text, references) = mentioned(&pieces);
        input.insert(prompt.clone(), Value::from(text));
        if !references.is_empty() {
            input.insert("references".into(), Value::Array(references));
        }
    } else if has("prompt") {
        let words: Vec<&str> = pieces.iter().filter_map(|piece| match piece { Piece::Words(words) => Some(words.as_str()), Piece::File { .. } => None }).collect();
        input.insert("prompt".into(), Value::from(words.join("\n")));
        for (field, kind) in [("images", "image"), ("videos", "video")] {
            let files: Vec<Value> = pieces.iter().filter_map(|piece| match piece { Piece::File { kind: of, file } if *of == kind => Some(Value::from(file.clone())), _ => None }).collect();
            if !files.is_empty() {
                input.insert(field.into(), Value::Array(files));
            }
        }
    } else {
        return Err(invalid("model_reads_no_text", format!("{target} takes no prompt: name a function that answers words, as `qwen3/write`.")));
    }
    let mut set = |field: &str, value: Option<Value>| {
        if let Some(value) = value.filter(|_| has(field)) {
            input.insert(field.into(), value);
        }
    };
    set("system", (!system.is_empty()).then(|| Value::from(system)));
    set("max_tokens", asked.max_output_tokens.map(Value::from));
    set("temperature", asked.temperature.map(Value::from));
    set("top_p", asked.top_p.map(Value::from));
    match asked.reasoning.as_ref().and_then(|reasoning| reasoning.effort.as_deref()) {
        Some("none") => set("thinking", Some(Value::Bool(false))),
        Some(effort) => {
            set("thinking", Some(Value::Bool(true)));
            set("reasoning_effort", Some(Value::from(effort)));
        }
        None => {}
    }
    let slot = |entry: &Value| -> Vec<String> {
        ::api::interface::models(entry).iter().filter_map(|slot| ::api::interface::slot_path(slot)?.split_once(".models.").map(|(_, param)| param.to_owned())).collect()
    };
    let terms = match checkpoint {
        Some(checkpoint) => {
            let slots = described.function.as_deref().and_then(|function| ::api::interface::entrypoint(&described.interface, function)).map(slot).unwrap_or_default();
            let slot = slots.iter().find(|slot| *slot == "model").or_else(|| slots.first()).cloned().unwrap_or_else(|| "model".into());
            vec![format!("model.{slot}={checkpoint}")]
        }
        None => vec![],
    };
    Ok(RunRequest { target: target.to_owned(), input: Some(input), terms, project: Some(project.to_owned()), hub, ..Default::default() })
}

/// A prompt of `pieces`: their words, each file mentioned where it stands (`<image_1>`), and the run's own references that name
/// those files (`[{name, image|video: <file>}]`, as the binder takes them).
fn mentioned(pieces: &[Piece]) -> (String, Vec<Value>) {
    let (mut text, mut references) = (String::new(), vec![]);
    let (mut images, mut videos) = (0, 0);
    for piece in pieces {
        let words = match piece {
            Piece::Words(words) => words.clone(),
            Piece::File { kind, file } => {
                let n = if *kind == "image" { &mut images } else { &mut videos };
                *n += 1;
                let key = format!("{kind}_{n}");
                let mut reference = Map::new();
                reference.insert("name".into(), Value::from(key.as_str()));
                reference.insert((*kind).into(), Value::from(file.as_str()));
                references.push(Value::Object(reference));
                format!("<{key}>")
            }
        };
        if !text.is_empty() && !text.ends_with(char::is_whitespace) && !words.starts_with(char::is_whitespace) {
            text.push(' ');
        }
        text.push_str(&words);
    }
    (text, references)
}

/// One output item as it is answered: its key (`text`, `texts/2`), its message id and the words said of it.
struct Said {
    key: String,
    id: String,
    words: String,
}

/// A response as its run goes: what is to be said next, and what was.
struct Answering {
    api: Arc<Api>,
    id: RunId,
    echo: Value,
    created_at: i64,
    tail: broadcast::Receiver<(RunId, Event)>,
    stop: watch::Receiver<bool>,
    sequence: u64,
    pending: VecDeque<Value>,
    items: Vec<Said>,
    /// Its run ended: its last events are queued, and leaving cancels nothing.
    ended: bool,
}

impl Answering {
    /// The response to `run` as it was recorded: made and going, or (a memoized answer) ended at once.
    fn new(api: &Arc<Api>, asked: &Asked, run: Run, tail: broadcast::Receiver<(RunId, Event)>) -> Self {
        let echo = json!({
            "model": asked.model,
            "instructions": asked.instructions,
            "max_output_tokens": asked.max_output_tokens,
            "temperature": asked.temperature,
            "top_p": asked.top_p,
            "metadata": asked.metadata,
            "parallel_tool_calls": false,
            "tool_choice": "auto",
            "tools": [],
            "text": {"format": {"type": "text"}},
        });
        let mut answering = Self {
            api: api.clone(),
            id: run.id.clone(),
            echo,
            created_at: run.created_at_ms / 1000,
            tail,
            stop: api.stop_signal(),
            sequence: 0,
            pending: VecDeque::new(),
            items: vec![],
            ended: false,
        };
        answering.say(json!({"type": "response.created", "response": answering.response("in_progress", Value::Null)}));
        answering.say(json!({"type": "response.in_progress", "response": answering.response("in_progress", Value::Null)}));
        if matches!(run.status, Status::Completed | Status::Failed | Status::Canceled) {
            answering.finish(&run);
        }
        answering
    }

    /// The Response as it stands, at `status`.
    fn response(&self, status: &str, error: Value) -> Value {
        let mut response = json!({
            "id": format!("resp_{}", self.id),
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "error": error,
            "incomplete_details": null,
            "output": self.items.iter().map(|item| message(item, status)).collect::<Vec<_>>(),
            "usage": null,
        });
        if let (Some(response), Some(echo)) = (response.as_object_mut(), self.echo.as_object()) {
            response.extend(echo.clone());
        }
        response
    }

    fn say(&mut self, mut event: Value) {
        event["sequence_number"] = self.sequence.into();
        self.sequence += 1;
        self.pending.push_back(event);
    }

    /// Item `key`'s place, said as added the first time.
    fn item(&mut self, key: &str) -> usize {
        if let Some(at) = self.items.iter().position(|item| item.key == key) {
            return at;
        }
        let at = self.items.len();
        let id = format!("msg_{}_{}", self.id, key.replace('/', "_"));
        self.items.push(Said { key: key.to_owned(), id: id.clone(), words: String::new() });
        self.say(json!({"type": "response.output_item.added", "output_index": at,
            "item": {"type": "message", "id": id, "status": "in_progress", "role": "assistant", "content": []}}));
        self.say(json!({"type": "response.content_part.added", "item_id": id, "output_index": at, "content_index": 0,
            "part": {"type": "output_text", "text": "", "annotations": []}}));
        at
    }

    fn words(&mut self, key: &str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let at = self.item(key);
        self.items[at].words.push_str(delta);
        let id = self.items[at].id.clone();
        self.say(json!({"type": "response.output_text.delta", "item_id": id, "output_index": at, "content_index": 0, "delta": delta, "logprobs": []}));
    }

    /// The words its record holds now, of each output, past what was said: after a stream that fell behind.
    fn caught_up(&mut self) {
        let Some(run) = self.record() else { return };
        for text in &run.text {
            let key = if text.index == 0 { text.output.clone() } else { format!("{}/{}", text.output, text.index) };
            let said = self.items.iter().find(|item| item.key == key).map_or(0, |item| item.words.len());
            if text.text.len() > said && text.text.is_char_boundary(said) {
                self.words(&key, &text.text[said..]);
            }
        }
        if matches!(run.status, Status::Completed | Status::Failed | Status::Canceled) {
            self.finish(&run);
        }
    }

    fn record(&self) -> Option<Run> {
        let calcifer = &self.api.calcifer;
        let row = calcifer.run_store(self.id.as_str()).and_then(|store| store.run(self.id.as_str())).ok()??;
        calcifer.record(&row).ok()
    }

    /// Its run ended: each item's words whole as its result holds them, then the response as it ended.
    fn finish(&mut self, run: &Run) {
        if self.ended {
            return;
        }
        self.ended = true;
        if run.status != Status::Completed {
            let code = run.reason.as_ref().map_or("failed", |reason| reason.code.as_str()).to_owned();
            let message = run.reason.as_ref().map_or_else(|| "The run failed.".to_owned(), |reason| reason.message.clone());
            let status = if run.status == Status::Canceled { "cancelled" } else { "failed" };
            let response = self.response(status, json!({"code": code, "message": message}));
            self.say(json!({"type": "response.failed", "response": response}));
            return;
        }
        for (key, text) in answers(run.result.as_ref()) {
            let at = self.item(&key);
            let said = self.items[at].words.clone();
            if let Some(rest) = text.strip_prefix(said.as_str()) {
                self.words(&key, rest);
            }
            self.items[at].words = text.clone();
            let id = self.items[at].id.clone();
            self.say(json!({"type": "response.output_text.done", "item_id": id, "output_index": at, "content_index": 0, "text": text, "logprobs": []}));
            self.say(json!({"type": "response.content_part.done", "item_id": id, "output_index": at, "content_index": 0,
                "part": {"type": "output_text", "text": text, "annotations": []}}));
            let item = message(&self.items[at], "completed");
            self.say(json!({"type": "response.output_item.done", "output_index": at, "item": item}));
        }
        let response = self.response("completed", Value::Null);
        self.say(json!({"type": "response.completed", "response": response}));
    }

    fn heard(&mut self, event: Event) {
        match event {
            Event::OutputTextDelta { item_id, delta, .. } => self.words(&item_id, &delta),
            Event::Completed { run, .. } | Event::Failed { run, .. } | Event::Canceled { run, .. } => self.finish(&run),
            _ => {}
        }
    }

    /// Waits for what its run says next; false once there is no more to hear.
    async fn listen(&mut self) -> bool {
        let received = tokio::select! {
            received = self.tail.recv() => received,
            () = stopping(&mut self.stop) => return false,
        };
        match received {
            Ok((id, event)) if id == self.id => self.heard(event),
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => self.caught_up(),
            Err(broadcast::error::RecvError::Closed) => return false,
        }
        true
    }

    async fn next(mut self) -> Option<(Result<sse::Event, Infallible>, Self)> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                let name = event["type"].as_str().unwrap_or("message").to_owned();
                let sent = sse::Event::default().event(name).data(event.to_string());
                return Some((Ok(sent), self));
            }
            if self.ended || !self.listen().await {
                // Calcifer stopping ends the stream, not the run.
                self.ended = true;
                return None;
            }
        }
    }

    /// The Response once its run ends.
    async fn wait(mut self) -> Value {
        while !self.ended && self.listen().await {}
        if !self.ended {
            self.ended = true;
            return self.response("in_progress", Value::Null);
        }
        self.pending.iter().rev().find_map(|event| event.get("response").cloned()).unwrap_or(Value::Null)
    }
}

/// A streamed response its client left before its run ended cancels the run: no one waits for its words.
impl Drop for Answering {
    fn drop(&mut self) {
        if !self.ended {
            if let Err(e) = self.api.calcifer.cancel(&self.id) {
                tracing::warn!(run = %self.id, "canceling a response its client left: {e}");
            }
        }
    }
}

async fn stopping(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// An output item as a message of the Responses API.
fn message(item: &Said, status: &str) -> Value {
    let status = if status == "completed" { "completed" } else { "in_progress" };
    json!({"type": "message", "id": item.id, "status": status, "role": "assistant",
        "content": [{"type": "output_text", "text": item.words, "annotations": []}]})
}

/// The words a result holds, by item: each field of words (`text`), and each of a list of them (`texts/1`, …).
fn answers(result: Option<&Value>) -> Vec<(String, String)> {
    let mut said = vec![];
    for (name, value) in result.and_then(Value::as_object).into_iter().flatten() {
        match value {
            Value::String(text) => said.push((name.clone(), text.clone())),
            Value::Array(texts) if texts.iter().all(Value::is_string) => {
                for (n, text) in texts.iter().enumerate() {
                    said.push((format!("{name}/{}", n + 1), text.as_str().unwrap_or_default().to_owned()));
                }
            }
            _ => {}
        }
    }
    said
}
