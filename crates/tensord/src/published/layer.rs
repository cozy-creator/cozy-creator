//! The machine's PyTorch layer (cl-715): torch, torchvision, torchaudio and triton with what they
//! require, as installed once in the machine's base interpreter: an image's `/opt/cozy/python`,
//! else the machine's own. A package environment takes it rather than installing its own Torch
//! (`environment`): each distribution linked at the highest directory it alone owns in the base
//! (`torch/` whole, the shared `nvidia/cu13/lib/` file by file), its dist-info a real copy whose
//! RECORD names only those links. Whatever uv later removes or replaces is a link, never the base.
//! A distribution the environment already has is its own: the package's lock wins.
use crate::catalog::normalized;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
};

/// What the layer is built from: what these require, transitively, as the base holds it.
pub const ROOTS: [&str; 4] = ["torch", "torchvision", "torchaudio", "triton"];

#[derive(Debug)]
pub struct Layer {
    site: PathBuf,
    /// Each distribution of the layer: its version, dist-info and links.
    dists: BTreeMap<String, Dist>,
}

#[derive(Debug)]
struct Dist {
    version: String,
    info: PathBuf,
    /// Paths relative to site-packages, each linked whole.
    links: Vec<String>,
}

impl Layer {
    /// The layer the base environment at `base` holds; none without torch.
    pub fn read(base: &Path) -> io::Result<Option<Layer>> {
        let Some(site) = platform::paths::venv_site_packages(base) else {
            return Ok(None);
        };
        let mut found = BTreeMap::new();
        let mut owner: HashMap<String, String> = HashMap::new();
        for entry in fs::read_dir(&site)?.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some((name, version)) = file.strip_suffix(".dist-info").and_then(|stem| stem.rsplit_once('-')) else {
                continue;
            };
            let (name, info) = (normalized(name), entry.path());
            let record = fs::read_to_string(info.join("RECORD")).unwrap_or_default();
            let files: Vec<String> = record.lines().filter_map(|line| line.split(',').next())
                .filter(|path| !path.starts_with("..") && !path.starts_with(&format!("{file}/")) && !path.is_empty())
                .map(str::to_owned).collect();
            for path in &files {
                owner.insert(path.clone(), name.clone());
            }
            let metadata = fs::read_to_string(info.join("METADATA")).unwrap_or_default();
            found.insert(name, (version.to_owned(), info, files, requires(&metadata)));
        }
        // Every directory, with who owns a file under it.
        let mut under: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        for (path, name) in &owner {
            for (at, _) in path.match_indices('/') {
                under.entry(&path[..at]).or_default().insert(name);
            }
        }
        let (mut todo, mut layer): (Vec<String>, BTreeSet<String>) = (ROOTS.map(String::from).to_vec(), BTreeSet::new());
        while let Some(name) = todo.pop() {
            if let Some((.., requires)) = found.get(&name).filter(|_| !layer.contains(&name)) {
                todo.extend(requires.iter().cloned());
                layer.insert(name);
            }
        }
        if !layer.contains("torch") {
            return Ok(None);
        }
        let dists = layer.into_iter().map(|name| {
            let (version, info, files, _) = found.remove(&name).expect("found");
            let alone = |dir: &str| under.get(dir).is_some_and(|owners| owners.len() == 1 && owners.contains(name.as_str()));
            let mut links = BTreeSet::new();
            for path in &files {
                let top = path.match_indices('/').map(|(at, _)| &path[..at]).find(|dir| alone(dir)).unwrap_or(path);
                links.insert(top.to_owned());
            }
            (name, Dist { version, info, links: links.into_iter().collect() })
        }).collect();
        Ok(Some(Layer { site, dists }))
    }

    /// `name==version` for each distribution: what an environment built on it is built from.
    pub fn pins(&self) -> Vec<String> {
        self.dists.iter().map(|(name, dist)| format!("{name}=={}", dist.version)).collect()
    }

    pub fn version(&self, name: &str) -> Option<&str> {
        self.dists.get(name).map(|dist| dist.version.as_str())
    }

    /// Whether `name` (normalized) is one of the layer's distributions.
    pub fn holds(&self, name: &str) -> bool {
        self.dists.contains_key(name)
    }

    /// What a package never installs while the machine provides it: the roots and the CUDA wheels
    /// they bring. Torch's general dependencies (sympy, jinja2, …) stay the lock's.
    pub fn provided(&self) -> impl Iterator<Item = &str> {
        let torch_only = |name: &str| ROOTS.contains(&name) || name.starts_with("nvidia-") || name.starts_with("cuda-");
        self.dists.keys().map(String::as_str).filter(move |name| torch_only(name))
    }

    /// Links the layer into the environment at `env`, every distribution it does not hold itself.
    pub fn link(&self, env: &Path) -> io::Result<()> {
        let site = platform::paths::venv_site_packages(env).ok_or_else(|| io::Error::other("the environment has no site-packages"))?;
        let held: BTreeSet<String> = fs::read_dir(&site)?.flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".dist-info")?.rsplit_once('-').map(|(n, _)| normalized(n)))
            .collect();
        for (name, dist) in self.dists.iter().filter(|(name, _)| !held.contains(*name)) {
            let mut record = String::new();
            for link in &dist.links {
                let at = site.join(link);
                if at.symlink_metadata().is_ok() {
                    continue; // another distribution's file of the same name: the environment's
                }
                fs::create_dir_all(at.parent().expect("under site-packages"))?;
                platform::durable::symlink(&self.site.join(link), &at)?;
                record.push_str(&format!("{link},,\n"));
            }
            let info = dist.info.file_name().expect("a dist-info").to_string_lossy();
            let copy = site.join(&*info);
            fs::create_dir(&copy)?;
            for entry in fs::read_dir(&dist.info)?.flatten().filter(|e| e.file_name() != "RECORD") {
                if entry.file_type()?.is_file() {
                    fs::copy(entry.path(), copy.join(entry.file_name()))?;
                    record.push_str(&format!("{info}/{},,\n", entry.file_name().to_string_lossy()));
                }
            }
            fs::write(copy.join("RECORD"), format!("{record}{info}/RECORD,,\n"))?;
            tracing::debug!("layer {name} {} linked", dist.version);
        }
        Ok(())
    }
}

/// The distributions METADATA requires outside any extra, by name.
fn requires(metadata: &str) -> Vec<String> {
    let name = |spec: &str| normalized(spec.split(|c: char| " ;<>=!~[(".contains(c)).next().unwrap_or_default());
    metadata.lines().filter_map(|line| line.strip_prefix("Requires-Dist:"))
        .filter(|spec| !spec.contains("extra =="))
        .map(|spec| name(spec.trim()))
        .collect()
}
