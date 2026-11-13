//! What took a second or more, one line in Calcifer's health log (`health`; its service discards stdout and stderr): a request by
//! its method and route, a background task by its name, and the milliseconds. Never a path, an id or a query
//! (PROJECT-DATA-OWNERSHIP.md: global diagnostics).
use std::time::{Duration, Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};

pub(crate) const TARGET: &str = "cozy.took";
const SLOW: Duration = Duration::from_secs(1);

/// One background task's iteration: said when it is dropped, if it took long.
pub(crate) struct Took {
    task: &'static str,
    began: Instant,
}

impl Took {
    pub(crate) fn new(task: &'static str) -> Self {
        Self { task, began: Instant::now() }
    }
}

impl Drop for Took {
    fn drop(&mut self) {
        let took = self.began.elapsed();
        if took >= SLOW {
            tracing::info!(target: TARGET, task = self.task, ms = took.as_millis() as u64, "slow task");
        }
    }
}

/// One request, until its answer is made (a stream's body is not counted): `status` is none when its caller left first.
struct Served {
    method: axum::http::Method,
    route: Option<MatchedPath>,
    began: Instant,
    status: Option<u16>,
}

impl Drop for Served {
    fn drop(&mut self) {
        let took = self.began.elapsed();
        if took < SLOW {
            return;
        }
        let route = self.route.as_ref().map_or("unrouted", MatchedPath::as_str);
        let ms = took.as_millis() as u64;
        match self.status {
            Some(status) => tracing::info!(target: TARGET, method = %self.method, route, status, ms, "slow request"),
            None => tracing::info!(target: TARGET, method = %self.method, route, ms, "slow request, its caller gone"),
        }
    }
}

pub(crate) async fn request(request: Request, next: Next) -> Response {
    let mut served = Served {
        method: request.method().clone(),
        route: request.extensions().get::<MatchedPath>().cloned(),
        began: Instant::now(),
        status: None,
    };
    let response = next.run(request).await;
    served.status = Some(response.status().as_u16());
    response
}
