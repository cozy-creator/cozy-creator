#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use calcifer::local_code::{describe_with, locked_runtime};

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
        "[project]\ndependencies = ['cozy_runtime[media]>=0.43', 'Cozy...Runtime (<0.43.1)', 'cozy-runtime-tools==999']\n",
    )
    .unwrap();
    let args = capture(project.path(), None);
    assert_eq!(from(&args), "cozy-runtime>=0.43, <0.43.1");
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
        "[[package]]\nname = 'cozy-runtime'\nversion = '0.43.0'\n",
    )
    .unwrap();
    let runtime = locked_runtime(project.path()).unwrap();
    let args = capture(project.path(), Some(&runtime));
    assert_eq!(from(&args), "cozy-runtime==0.43.0");
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
        "cozy-runtime-tools>=0.43,<0.43.1",
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
