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
cozy run sdxl/generate --input request.yaml
cozy run sdxl/generate --input request.yaml seed=8      # an inline input overrides the file
cozy run birefnet/remove_background --file image=./character.png
cozy run sdxl/generate --describe                       # the fields a function takes
```

Inline `key=value` inputs are the quick form: `cozy run sdxl/generate prompt='a fox in fresh snow'`.
Files go in with `--file` (or by path inside the YAML, relative to it). A run is followed to its end;
Ctrl-C stops following, never the run, and `cozy run show <N>` picks it up again.

A video path can name a segment of it, `#t=<start>,<end>` (whole seconds, `m:ss` or `h:mm:ss`; either
end may be left out: `#t=5`, `#t=,0:12`), in the YAML or in `--file name=path#t=…`. cozy cuts it once,
frame-accurately, keeps it in `~/.cozy/cache/clips`, and sends the cut, not the whole file; a machine that
already holds it is not sent it again:

```yaml
references:
  - {name: Video 1, kind: video, video: "videos/match.m4v#t=3:19,3:32"}
```

```sh
cozy machine rent            # what is for rent; then cozy machine rent <sku>
cozy model ls sdxl
```

First-party packages have no owner (`sdxl/generate`); everyone else's are `owner/name`
(`you/my-pack/generate`). Models live in their authors' orgs (`stabilityai/sdxl`).

`cozy login` signs you up on first use, with a code it emails. On Linux, runs go to this computer's
machine, which starts when used and stops after 15 minutes idle; without an NVIDIA GPU here, add
`--on new` to run on a rented one. macOS and Windows don't run models locally yet: add `--on new` to
rent a Linux GPU, or `--on <machine>` for one you have. Outputs land in `~/.cozy/outputs`.

`cozy --help` and `cozy <verb> --help` are the reference.

`cozy doctor` diagnoses this computer's cozy (Calcifer, its service, the machine, tools, GPUs) and
restarts a Calcifer that is stuck or runs an older cozy.

## Projects

A video project is a folder: `project.yaml` (yours; cozy never writes it after `new`), `project.lock`
(cozy's: each node's versions, what made each, and which is approved), `files/` (every version, by
content) and `edit.mp4`, cut on this computer as shots land. `cozy project new --project garden` writes a
commented `project.yaml`; fill it in:

```yaml
type: video
style: soft watercolor
resolution: 1440p
defaults: {shot: {with: minimax-h3/shot, options: {turbo: true}}}
entities:
  - {name: theo, kind: character, description: "A tall man in a green raincoat."}
  - {name: juno, kind: character, description: "<theo>'s sister, in a red coat.", render: {with: flux-2/generate}}
  - {name: dock, kind: environment, description: "a foggy harbour at dawn",
     setups: [{name: a, camera: "wide: the red hut near on the left, the crates far on the right"}]}
world_states:
  - {name: stage, init: "<juno> and <theo> face off on <dock>.", blocking: "juno by the crates."}
shots:
  - {name: fight, setup: dock.a, prompt: "<theo> and <juno> spar.", duration_s: 20, writes: [stage],
     beats: [{at: 0, prompt: "They circle."}, {at: 10, prompt: "<juno> sweeps his legs."}]}
  - {name: reaction, prompt: "Close on <theo>, wincing, after <stage>.", duration_s: 4}
edit: [{shot: fight, end: 8}, {shot: reaction}, {shot: fight, start: 10}]
```

```sh
cozy render all --project garden --on <machine>   # everything missing or out of date, then the edit
cozy render theo --project garden --on <machine>  # another take of theo
cozy project approve theo 2 --project garden      # approve it: what is made from theo goes out of date
cozy project status --project garden              # what is missing, out of date and why, and the edit
```

Each version is sent as soon as what it reads is made, so independent ones render at once and the
machine's scheduler places them; `--on a,b` spreads them over machines (the least busy, one that holds
the models first), and a `cozy render` asked while another goes joins it.

`<name>` in a text names another node, which it is then made from; a setup's camera is written in
screen terms (what is near or far, left or right). A shot reads a world state it
names and changes the ones it `writes`. Character and prop pictures are cleaned on this computer before
use. Only a changed upstream file puts something out of date, never a text edit or a render block:
`cozy render <name>` when you want a new take. A node renders by its `render: {with, models, lora,
options}`: `with` a function, `models: {base_model: {ref: org/checkpoint}}` and `lora: [{ref: org/lora,
strength: 0.8}]` as `cozy run --input` takes them, and `options`, the function's own fields. A block that
names no function adjusts its kind's `defaults:` (entity, setup, world_state, shot, upscale, clean).
`cozy project status` shows what made each approved version. A file you attach (`image:`, or a shot's
`video:`) is used until you approve one cozy made.

`resolution: 1440p` and `aspect_ratio: 16:9` (the default; `9:16` and `2.39:1` work alike) fix the
project's canvas: pictures of places are made at its aspect, and every shot fills it, center-cropped,
never letterboxed or stretched. A shot whose approved version is below the resolution is upscaled
(`defaults: {upscale: …}`, or a shot's own `upscale: {render: …}`, which can change the function, models,
LoRAs and options but never the canvas). `render all` makes the upscales and cuts `edit.mp4` at the
canvas; `preview.mp4`, the native cut, is redone as shots land, and `render all --preview` stops there.

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
`tensorfs`, `varena`, `tensorfs-py`, `hub-client`, `protocol` and `platform`.
`crates/tensorfs` is the Rust library and `tfs` CLI; `crates/tensorfs-py` contains its PyO3 bindings,
Python package and wheel configuration together. The browser player is `web/player`.
