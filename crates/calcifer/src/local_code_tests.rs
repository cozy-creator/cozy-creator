use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use super::{describe_with, locked_runtime, run_said};

fn capture(project: &Path, runtime: Option<&str>) -> Vec<String> {
    let uv = project.join("uv");
    fs::write(
        &uv,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s' '{\"format\":\"cozy.package.interface/1\",\"entrypoints\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o700)).unwrap();
    describe_with(&uv, project, "3.12", runtime).unwrap();
    fs::read_to_string(project.join("uv.args"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn from(args: &[String]) -> &str {
    let index = args.iter().position(|arg| arg == "--from").unwrap();
    &args[index + 1]
}

#[test]
fn source_describer_combines_normalized_runtime_bounds() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("pyproject.toml"),
        "[project]\ndependencies = ['cozy_runtime[media]>=0.43.1', 'Cozy...Runtime (<0.44)', 'cozy-runtime-tools==999']\n",
    )
    .unwrap();
    let args = capture(project.path(), None);
    assert_eq!(from(&args), "cozy-runtime>=0.43.1, <0.44");
    assert!(args.iter().any(|arg| arg == "--isolated"));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--upgrade-package", "cozy-runtime"])
    );
    assert_eq!(
        &args[args.len() - 5..],
        [
            "cozy-runtime",
            "--json",
            "--dir",
            project.path().to_str().unwrap(),
            "describe"
        ]
    );
}

#[test]
fn source_describer_keeps_the_exact_locked_runtime_authoritative() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("pyproject.toml"),
        "[project]\ndependencies = ['cozy-runtime>=99']\n",
    )
    .unwrap();
    fs::write(
        project.path().join("uv.lock"),
        "[[package]]\nname = 'cozy-runtime'\nversion = '0.43.1'\n",
    )
    .unwrap();
    let runtime = locked_runtime(project.path()).unwrap();
    let args = capture(project.path(), Some(&runtime));
    assert_eq!(from(&args), "cozy-runtime==0.43.1");
    assert!(!args.iter().any(|arg| arg == "--isolated"));
    assert!(!args.iter().any(|arg| arg == "--upgrade-package"));
}

#[test]
fn source_describer_fallback_does_not_forward_sources_or_markers() {
    for requirement in [
        "cozy-runtime",
        "cozy-runtime[media]",
        "cozy-runtime>=0.43; python_version < '3.13'",
        "cozy-runtime @ https://example.invalid/untrusted.whl",
        "cozy-runtime @ git+https://example.invalid/untrusted.git",
        "cozy-runtime @ file:///untrusted.whl",
        "cozy-runtime-tools>=0.43.1,<0.44",
    ] {
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("pyproject.toml"),
            format!(
                "[project]\ndependencies = [{}]\n",
                serde_json::to_string(requirement).unwrap()
            ),
        )
        .unwrap();
        let args = capture(project.path(), None);
        assert_eq!(from(&args), "cozy-runtime@latest", "{requirement}");
        assert!(!args.iter().any(|arg| arg.contains("example.invalid")
            || arg.starts_with("file:")
            || arg.contains("python_version")));
        assert!(!args.iter().any(|arg| arg == "--isolated"));
        assert!(!args.iter().any(|arg| arg == "--upgrade-package"));
    }
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("pyproject.toml"),
        "[project]\ndependencies = []\n",
    )
    .unwrap();
    assert_eq!(from(&capture(project.path(), None)), "cozy-runtime@latest");
}

#[test]
fn source_describer_refuses_invalid_bounds_before_invoking_uv() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("pyproject.toml"),
        "[project]\ndependencies = ['cozy-runtime>=0.43 --index https://example.invalid']\n",
    )
    .unwrap();
    let error = describe_with(
        &project.path().join("absent-uv"),
        project.path(),
        "3.12",
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, "runtime_requirement_invalid");
}

fn failed_tool(stderr: &str) -> (api::error::Error, String) {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "printf '%s' \"$TOOL_DIAGNOSTIC\" >&2; exit 3"]).env("TOOL_DIAGNOSTIC", stderr);
    run_said(&mut command, "describe_failed", "cozy-runtime describe").unwrap_err()
}

#[test]
fn source_describer_keeps_structured_recovery_without_expanding_detail_lines() {
    let record = serde_json::json!({
        "error": {"name":"validation", "code":3,
            "message":"package_contract: app.py:2: internal_import (and 1 more)\n  app.py:3: internal_import",
            "remedy":"Use the public author APIs"},
        "next":["cozy run --help"]
    });
    let stderr = format!("warning: preparing Runtime\n{}\n", serde_json::to_string_pretty(&record).unwrap());
    let (error, kept) = failed_tool(&stderr);
    assert_eq!(kept, stderr);
    assert_eq!(error.kind, api::error::Kind::Validation);
    assert_eq!(error.code, "describe_failed");
    assert_eq!(error.message, "cozy-runtime describe exited with code 3: package_contract: app.py:2: internal_import (and 1 more)");
    assert_eq!(error.remedy.as_deref(), Some("Use the public author APIs"));
    assert_eq!(error.next, ["cozy run --help"]);
    let envelope = serde_json::to_value(api::error::Envelope { error }).unwrap();
    assert!(!envelope["error"]["message"].as_str().unwrap().contains('\\'));
}

#[test]
fn source_describer_preserves_plain_errors_and_ignores_malformed_records() {
    for stderr in [
        "error: dependency versions do not resolve\nhint: pin a compatible version\n",
        "{\"error\": invalid}\nerror: dependency versions do not resolve\n",
    ] {
        let (error, kept) = failed_tool(stderr);
        assert_eq!(kept, stderr);
        assert!(error.message.ends_with("error: dependency versions do not resolve"));
        assert!(error.remedy.is_none());
        assert_eq!(error.next, ["cozy machine logs local calcifer"]);
    }
}

#[test]
#[ignore = "requires uv and the published Runtime; hosted cli-describe-errors workflow runs it explicitly"]
fn source_describer_reports_the_published_runtime_diagnostic_and_remedy() {
    let project = tempfile::tempdir().unwrap();
    fs::create_dir(project.path().join("invalid_app")).unwrap();
    fs::write(project.path().join("pyproject.toml"), r#"
[project]
name = "invalid-app"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime==0.45.0"]
[project.entry-points."cozy.application"]
default = "invalid_app:app"
[tool.uv.build-backend]
module-name = "invalid_app"
module-root = ""
"#).unwrap();
    fs::write(project.path().join("invalid_app/__init__.py"), r#"from cozy_runtime.author import App
import tensorfs
from tensorfs import _ext

app = App()

@app.entrypoint(accelerator=False)
def echo(value: int) -> int:
    return value
"#).unwrap();
    let error = describe_with(Path::new("uv"), project.path(), "3.12", Some("0.45.0")).unwrap_err();
    println!("{}", serde_json::to_string(&api::error::Envelope { error: error.clone() }).unwrap());
    assert_eq!(error.kind, api::error::Kind::Validation);
    assert_eq!(error.code, "describe_failed");
    assert!(error.message.contains("internal_import: import tensorfs"), "{}", error.message);
    assert_eq!(error.message.lines().count(), 1);
    assert!(error.remedy.as_deref().is_some_and(|remedy| remedy.contains("surface must be valid")));
    let encoded = serde_json::to_value(api::error::Envelope { error: error.clone() }).unwrap();
    assert_eq!(encoded["error"]["message"], error.message);
    assert!(!encoded["error"]["message"].as_str().unwrap().contains("\"error\""));
}
