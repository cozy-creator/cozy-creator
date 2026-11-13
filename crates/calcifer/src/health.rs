//! The only persistent daemon diagnostics: closed health events, never project detail. A test's
//! config can add its warnings (`calcifer.diagnostics`), which may name projects.
use std::{fs::OpenOptions, sync::Mutex};
use platform::{durable, paths::Home};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};

pub(crate) enum HealthEvent { Started, Stopping, Stopped, StartRefused, Panicked }

pub(crate) fn install(home: &Home, diagnostics: bool) -> Result<(), ::api::error::Error> {
    let failed = || ::api::error::Error::new(::api::error::Kind::Internal, "calcifer_health", "Calcifer cannot open its service health record");
    home.open().and_then(|()| durable::private_dir(&home.calcifer())).map_err(|_| failed())?;
    let open = |path: std::path::PathBuf| -> Result<std::fs::File, ::api::error::Error> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600).custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let file = options.open(path).map_err(|_| failed())?;
        #[cfg(unix)] {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            let metadata = file.metadata().map_err(|_| failed())?;
            if !metadata.is_file() || metadata.uid() != rustix::process::geteuid().as_raw() { return Err(failed()); }
            file.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|_| failed())?;
        }
        Ok(file)
    };
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(Mutex::new(open(home.calcifer_health())?))
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| meta.target() == "cozy.service_health"));
    let warnings = diagnostics.then(|| open(home.calcifer().join("diagnostics.log"))).transpose()?.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            // Where each project job goes, and why, beside them.
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                *meta.level() <= tracing::Level::WARN || (*meta.level() <= tracing::Level::INFO && meta.target() == "calcifer::project")
            }))
    });
    tracing_subscriber::registry().with(layer).with(warnings).try_init().map_err(|_| failed())?;
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
