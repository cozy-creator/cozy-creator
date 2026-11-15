//! `cozy config`: this computer's settings Calcifer holds (`GET` and `PUT /v1/settings`), in config.toml's `[calcifer]`.
use std::process::ExitCode;

use api::{
    calcifer::Settings,
    error::{Error, Kind},
};
use clap::ValueEnum;

use crate::{calcifer_client::path, context::Context, output};

/// Show or change this computer's settings.
#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// None: list them.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Every setting and its value.
    List,
    /// One setting's value.
    Get { key: Key },
    /// Change one setting: true or false.
    Set { key: Key, value: String },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Key {
    /// Send anonymous performance data: each run's function, package, versions, GPU, size and times, never its prompts,
    /// inputs, outputs or names, and nothing that says who ran it.
    #[value(name = "send_performance_data")]
    SendPerformanceData,
}

impl Key {
    fn name(self) -> &'static str {
        match self {
            Key::SendPerformanceData => "send_performance_data",
        }
    }

    fn of(self, settings: &Settings) -> bool {
        match self {
            Key::SendPerformanceData => settings.send_performance_data,
        }
    }
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let client = ctx.calcifer().await?;
    let at = path(&["v1", "settings"]);
    let mut settings: Settings = client.get(&at).await?;
    let shown = match args.command.unwrap_or(Command::List) {
        Command::List => Key::value_variants().to_vec(),
        Command::Get { key } => vec![key],
        Command::Set { key, value } => {
            let value = boolean(&value)?;
            match key {
                Key::SendPerformanceData => settings.send_performance_data = value,
            }
            settings = client.put(&at, &settings).await?;
            vec![key]
        }
    };
    if ctx.json {
        output::json(&settings);
        return Ok(ExitCode::SUCCESS);
    }
    for key in shown {
        println!("{} = {}", key.name(), key.of(&settings));
    }
    Ok(ExitCode::SUCCESS)
}

fn boolean(value: &str) -> Result<bool, Error> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" => Ok(true),
        "false" | "off" | "no" => Ok(false),
        _ => Err(Error::new(Kind::Usage, "setting_value", format!("{value:?} is not true or false"))
            .remedy("cozy config set send_performance_data false")),
    }
}
