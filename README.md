# Cozy

Cozy runs generative-media packages on your own machines and on rented GPUs. One `cozy` binary
carries the command line, Calcifer (the local controller), TensorD (the machine) and TensorFS
(the weight store).

## Install

Linux and macOS (Apple Silicon; Intel Macs aren't supported):

```sh
curl -LsSf https://tensorhub.com/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://tensorhub.com/install.ps1 | iex
```

Both put `cozy` in `~/.cozy/bin` and on your PATH for new terminals: open a new one, or run the line
the installer prints. `cozy update` moves to the newest release (`cozy update 0.12.0` to a given one): it checks the
release's signature, restarts Calcifer on it, and the local machine follows once idle. Homebrew's cozy updates with
`brew upgrade cozy`.

## Use

Write the request in a YAML file, `request.yaml`:

```yaml
prompt: a lighthouse on a basalt cliff at dusk, oil painting
aspect_ratio: "16:9"
steps: 30
seed: 7
```

```sh
cozy login you@example.com
cozy new                                  # initialize the current directory
cozy run sdxl/generate --input request.yaml
cozy run sdxl/generate --input request.yaml seed=8      # an inline input overrides the file
cozy run birefnet/remove_background --file image=./character.png
cozy run sdxl/generate --describe                       # the fields a function takes
cozy run sdxl/generate --input request.yaml --estimate  # when each machine would finish it; sends nothing
```

Inline `key=value` inputs are the quick form: `cozy run sdxl/generate prompt='a fox in fresh snow'`.
Files go in with `--file` (or by path inside the YAML, relative to it), and local code as `./dir/function`
or `./script.py/function`: each is sent by content, so Calcifer, here or on another computer, reads no folder
of yours. A run is followed to its end; Ctrl-C stops following, never the run, and `cozy run show <N>` picks
it up again. `-o <dir>` downloads its outputs, on `cozy run` and `cozy run show`. A video's frames are a box
of the project mentioned with them, `clip='<match:48-144>'`, cut before it's sent.
With no `--on`, a run goes to the machine that quotes the soonest finish, and that machine chooses how many
GPUs it uses; `--estimate [N]` prints each machine's quote for N of it sent at once (GPUs, start, finish,
preparation, queue position, and whether it stands on measured runs).

```sh
cozy machine rent            # what is for rent; then cozy machine rent <sku>
cozy model ls sdxl
```

First-party packages have no owner (`sdxl/generate`); everyone else's are `owner/name`
(`you/my-pack/generate`). Models live in their authors' orgs (`stabilityai/sdxl`).

`cozy login` signs you up on first use, with a code it emails. On Linux, runs go to this computer's
machine, which starts when used and stops after 15 minutes idle; without an NVIDIA GPU here, add
`--on new` to run on a rented one. macOS and Windows don't run models locally yet: add `--on new` to
rent a Linux GPU, or `--on <machine>` for one you have. Run project commands from the project directory. `cozy new [directory]` creates a project explicitly.
`cozy open`, `cozy run` and `cozy status` use an existing project. Outputs land in its `files/` directory.

`cozy` lists the main commands. `cozy <verb> --help` shows a command's options.
The browser opened by `cozy open` provides asset import, editing, rendering, export and history.
Use `cozy push` and `cozy pull` to synchronize the current project's files with your account.

`cozy doctor` diagnoses this computer's cozy (Calcifer, its service, the machine, tools, GPUs) and
restarts a Calcifer that is stuck or runs an older cozy.

## Projects

A project has a name and an id, and lives in a folder on Calcifer's computer: `project.db`, what it says and has made, `files/`
and the cuts. Everything in the film is an asset, a package's function makes it, and a run happens only when an asset has no
state. Putting old text back returns the old take with no run, and approving pins a take.

```sh
cozy new garden
cd garden
cozy open                                  # edit assets in the browser
cozy status                                # this project's runs and machines
```

The format and every command: [PROJECT-YAML.md](crates/calcifer/docs/PROJECT-YAML.md).

## Remove cozy

End your rentals first (`cozy machine ls`, `cozy machine end <name>`). Then:

Linux:

```sh
systemctl --user disable --now cozy-calcifer
pkill -f ~/.cozy/bin/cozy      # what they started (a machine, its executors)
rm -rf ~/.cozy ~/.config/cozy ~/.local/share/bash-completion/completions/cozy ~/.config/fish/completions/cozy.fish
```

macOS:

```sh
launchctl bootout gui/$(id -u)/com.cozy-creator.calcifer
pkill -f ~/.cozy/bin/cozy
rm -rf ~/.cozy ~/.config/cozy ~/Library/LaunchAgents/com.cozy-creator.*.plist
```

Windows (PowerShell):

```powershell
Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Run cozy-calcifer -ErrorAction SilentlyContinue
Get-Process cozy -ErrorAction SilentlyContinue | Stop-Process
Remove-Item -Recurse -Force ~\.cozy, $env:LOCALAPPDATA\cozy
$path = ([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') -ne "$HOME\.cozy\bin"
[Environment]::SetEnvironmentVariable('Path', $path -join ';', 'User')
```

On Linux and macOS, delete the line that sources `~/.config/cozy/env.sh` (or `env.fish`) from `.profile`,
`.bashrc`, `.zshrc` or fish's `conf.d`. `~/.cozy` holds the machine's model store, which can be large;
a store set elsewhere in `~/.cozy/config.toml` stays until you delete it. Homebrew: `brew uninstall
cozy`, then the same steps.

## Build

```sh
cargo build --release -p cozy
```

The workspace's crates live in `crates/`: `cozy` (the binary), `cli`, `calcifer`, `tensord`,
`tensorfs`, `varena`, `tensorfs-py`, `api` and `platform`.
`crates/tensorfs` is the Rust library and `tfs` CLI; `crates/tensorfs-py` contains its PyO3 bindings,
Python package and wheel configuration together. The browser player is `web/player`.

### Local development

On Linux, `scripts/dev-local.sh` watches the checkout with Watchexec, builds incrementally, installs
successful builds into `~/.cozy/bin/cozy`, and restarts `cozy-calcifer.service`. It needs Watchexec,
pnpm, binutils, and the pinned Rust toolchain. `scripts/dev-local.sh --once` runs one update.

For browser hot reload, run `pnpm --dir web/app dev --host 127.0.0.1`. Vite uses the local Calcifer
through the existing development proxy. The watcher also rebuilds the page embedded in the binary.
