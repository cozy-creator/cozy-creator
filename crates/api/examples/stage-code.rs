//! Stage a local gate fixture with the same source selection as the CLI.
use std::path::Path;

use api::{code, error::{Error, Kind}};
use serde_json::{Value, json};

fn stage(target: &Path) -> Result<Value, Error> {
    let (package, function) = code::split(target).ok_or_else(|| {
        Error::new(Kind::NotFound, "target_not_found", format!("{} is not a package or script", target.display()))
    })?;
    let staged = code::stage(&package, None)?;
    Ok(json!({
        "target": function.unwrap_or_default(),
        "code": staged.code(),
        "document": String::from_utf8(staged.document).expect("tree documents are JSON"),
        "files": staged.files,
        "left": staged.left,
    }))
}

fn main() {
    let Some(target) = std::env::args_os().nth(1) else {
        eprintln!("usage: stage-code <local-package-or-script[/function]>");
        std::process::exit(2);
    };
    match stage(Path::new(&target)) {
        Ok(staged) => println!("{staged}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_package_and_path_dependency_with_cli_source_filter() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("app");
        let dependency = root.path().join("shared");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(package.join("pyproject.toml"), "[project]\nname = 'app'\n[tool.uv.sources]\nshared = { path = '../shared' }\n").unwrap();
        std::fs::write(package.join("app.py"), "def generate(): pass\n").unwrap();
        std::fs::write(package.join(".env"), "PRIVATE=not-source\n").unwrap();
        std::fs::write(dependency.join("shared.py"), "VALUE = 1\n").unwrap();

        let staged = stage(&package.join("generate")).unwrap();
        assert_eq!(staged["target"], "generate");
        assert_eq!(staged["code"]["at"], "app");
        let document = staged["document"].as_str().unwrap();
        assert_eq!(staged["code"]["tree"], api::ids::Digest::of(document.as_bytes()).to_string());
        let files = api::tree::parse(document.as_bytes()).unwrap();
        assert!(files.contains_key("app/app.py"));
        assert!(files.contains_key("shared/shared.py"));
        assert!(!files.contains_key("app/.env"));
        assert_eq!(staged["files"].as_array().unwrap().len(), files.len());
    }
}
