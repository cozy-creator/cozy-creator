//! The only persistent daemon diagnostics: closed health events, never project detail.
use std::{fs::OpenOptions, sync::Mutex};
use platform::{durable, paths::Home};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};

pub(crate) enum HealthEvent { Started, Stopping, Stopped, StartRefused, Panicked }

pub(crate) fn install(home: &Home) -> Result<(), ::api::error::Error> {
    let failed = || ::api::error::Error::new(::api::error::Kind::Internal, "calcifer_health", "Calcifer cannot open its service health record");
    home.open().and_then(|()| durable::private_dir(&home.calcifer())).map_err(|_| failed())?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(home.calcifer_health()).map_err(|_| failed())?;
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(Mutex::new(file))
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| meta.target() == "cozy.service_health"));
    tracing_subscriber::registry().with(layer).try_init().map_err(|_| failed())?;
    std::panic::set_hook(Box::new(|_| service_health(HealthEvent::Panicked)));
    Ok(())
}

pub(crate) fn service_health(event: HealthEvent) {
    let event = match event {
        HealthEvent::Started => "started",
        HealthEvent::Stopping => "stopping",
        HealthEvent::Stopped => "stopped",
        HealthEvent::StartRefused => "start_refused",
        HealthEvent::Panicked => "panicked",
    };
    tracing::info!(target: "cozy.service_health", event, pid = std::process::id(), version = env!("CARGO_PKG_VERSION"));
}
