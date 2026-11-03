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

A video path can name a segment of it, `#t=<start>,<end>` (seconds, `m:ss` or `h:mm:ss`; either
end may be left out: `#t=5`, `#t=,0:12`), in the YAML or in `--file name=path#t=…`. cozy cuts it once,
frame-accurately, keeps it in `~/.cozy/cache/clips`, and sends the cut, not the whole file; a machine that
already holds it is not sent it again:

```yaml
references:
  - name: Video 1
    kind: video
    video: "videos/match.m4v#t=3:19,3:32"
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

A video project is a folder:
- `project.yaml`: yours. cozy writes it only at `new`.
- `project.lock`: cozy's record of each node's takes or versions, what made each, which is approved, and the
  files imported.
- `assets/`: every file the project uses, a real file kept by content as `assets/<sha256>.<ext>`: what cozy makes,
  and what you import.
- `edit.mp4`: the film, cut on this computer as shots land, and beside it `edit.otio`, the same edit as an
  OpenTimelineIO timeline for an editing app, each clip referencing its take in `assets/`.

Files enter a project one way, and `project.yaml` names them only by hash:

```sh
cozy project import ./kyogre.png --character
cozy project import https://example.com/groudon.png --character
cozy project import ./coast.jpg
cozy project import ./sword.png --prop
```

```
imported ab12cd34ef56 (background removed)
image: ab12cd34ef56
```

Paste that line under the node. It works for pictures, video and sound in every file field: an entity's `image` and
`voice`, a setup's or world state's `image`, `references:`, and a shot's `plate`, `first_frame` and `last_frame`. Any
prefix of the hash will do; one that fits two assets is said, and asks for a longer one. A URL is downloaded here, up to
64 MiB, from a public address. A made version is an asset too: `cozy project status` prints each one's 12 characters.
Importing a file that is already here copies nothing and says so, and so does a clean it reuses:
`already imported ab12cd34ef56 (background already removed)`, with the same line to paste.
`--character` and `--prop` have its background removed: at once if this computer runs models, else by the first machine
that renders something reading it. That clean is an asset like any other, and it is what the hash means from then on.
A path or URL in a file field is never read: it warns with the import command, and that node renders without it.
An asset never changes, so only a different hash, or something upstream, puts what reads it out of date.

On another computer: commit `project.yaml` and `project.lock`, keep `assets/` out of git, and `cozy project push`
sends the files to your Tensorhub account, private to you. `cozy project pull` next to a clone fetches them as real
files, and a picture not cleaned yet is cleaned there when something reads it. Both send only
what is missing and go on after an interruption. What you pushed stays until you `cozy model delete
<account>/project-assets`, the private model that holds it. A file over 5 GiB is skipped with a warning: everything
else goes, and the command exits 3. Pulling such a project does the same for what is not there.

Rename a node in `project.yaml` (and its `<mentions>`), then `cozy project rename <old> <new>` keeps its
versions; `status` suggests it.

A shot's alternatives (and its upscale's) are takes; every other node's are versions. Each one is a set
of assets, numbered from 1 and never renumbered: a shot's take is its video and latents. `cozy project new garden`
makes the folder with a `project.yaml` holding a short film that renders as it stands; change it, or write your own:

```yaml
version: 1
style: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.
aspect_ratio: "16:9"
fps: 24

entities:
  - name: kyogre
    kind: character
    description: Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines.

  - name: groudon
    kind: character
    description: Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws.

  - name: coast
    kind: environment
    description: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.
    setups:
      - name: wide
        camera: The camera stands high and far back, with the sea on the left, the cliffs on the right, and the crater city small at the center.

shots:
  - name: clash
    summary: |
      <kyogre> and <groudon> fight on <coast.wide> in one continuous shot with no cuts.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - duration: 14
        description: |
          A wide shot shows <kyogre> in the surf on the left and <groudon> on the rocks on the right. <kyogre> fires a thick column of water across the frame.
      - duration: 14
        description: |
          <groudon> leans out of the steam and breathes a wide torrent of orange fire across the surf. <kyogre> dives beneath the flames.
      - duration: 10
        description: |
          <kyogre> erupts from the water at the shoreline, and the two crash together chest to chest in the breaking waves.
```

What else a project can set:

```yaml
resolution: 1440p
soundscape: gulls, surf, wind

defaults:
  shot:
    with: minimax-h3/generate
    options:
      turbo: true

references:
  - name: kata
    video: 7c9e2d41a0b3

world_states:
  - name: shore
    init: <coast> after the fight.
    blocking: <groudon> on the rocks.

shots:
  - name: clash
    writes:
      - state: shore
        at: 30

edit:
  - shot: clash
    start: 24
    end: 192
```

```sh
cd garden
cozy project render all --on <machine>     # everything missing or out of date, then the edit
cozy project render clash -n 3 --on <machine>
                                           # three more takes of clash
cozy project approve clash 2               # approve take 2: what is made from it goes out of date
cozy project render kyogre --approve --on <machine>
                                           # a new version of kyogre, approved when it lands
cozy project status                        # what is missing, out of date and why, and the cuts
cozy project ls                            # every project this computer has opened
```

The project is the folder holding `project.yaml`, found from where you are or above it; `-p <path>` names
another, as `git -C` does. Each take or version is sent as soon as what it reads is made, so independent
ones render at once and the machine's scheduler places them. `--on a,b` spreads them over machines (the
least busy, one that holds the models first), and a render asked while another goes joins it.

A shot is a `summary`, the scene, sent with every part of it, and `beats`, each with its own `duration:` in seconds. A beat
starts where the one before it ended, and a shot is as long as its beats. A beat with no `duration:` lasts 5 s, with a warning;
a shot with no beats is as long as its attached `video:`, else 15 s. A cut is a new shot, and a one-action shot is one beat. Say the scene in the summary, not the sequence: a part can act on what it reads, and a camera move there runs again in every part, so camera moves go in beats. `soundscape:`
and `music:` go with every part, and a sound of one moment goes in its beat's `description`. A beat's timing is approximate: H3 lands
an action about 2 s late even when a part opens on it.

Each package turns a node into its own call with a project adapter (`@app.project_adapter("shot")`), so `defaults:` can name
another model and cozy needs no change. It runs on the render machine and only reads text, names and digests. Its call is
recorded on the take in `project.lock`. A function with no adapter gets one flat `prompt`, the summary then each beat's
description, with a warning. Adapter and package updates never put anything out of date.

`version: 1` names the format of `project.yaml`. A file with none reads as 1, with a warning. Nothing you write is refused: a
shot with no text gets a warning and renders anyway.

A take can be shorter than its shot: `minimax-h3` renders 15 s of a longer shot with one beat. The edit cuts
the take's real length, a write or `frame:` past its end takes its last frame, and `status` shows what the function said.

`<name>` in a text names another node, which it is then made from; a setup's camera is written in
screen terms (what is near or far, left or right). A shot reads only what its text names: a mention in its `summary` reaches
every segment, and one in a beat only the segment holding that beat. `<coast.wide>` names a setup; `<coast>` alone is the
setup of it the shot names, else its first. A shot reads a world state it
names and changes the ones it `writes` (each a `state`, and `at`, seconds into the shot, by default its end). A shot that mentions a state it
writes partway renders as consecutive calls split there (`continue_from`), the later one reading its own frame. A
renderer that cannot continue, or a call under its shortest piece, renders on through: the shot reads the state as it
stood before, and `status` says so. A clip or sound mentioned by anything but a shot, or a mention that would make
something from itself, stays plain text, also said.

A file that is no entity goes under `references:` with a name and one `video:`, `audio:` or `image:` (sent as a
prop). A shot that mentions it gets it, exactly as with an entity: any other field, such as `use_audio:
true`, goes to its function, and importing another file for it puts only those shots out of date. Names are shared with entities,
in any case, and each is used once. `plate:` (the video a shot edits), `first_frame:` and `last_frame:` stay on the
shot. What a function does not take is left out and said.

`qwen-image-2` draws a character as a 16:9 sheet (front, back, three close-ups) and a prop as a 3:2 sheet (front, back), both
transparent. Any character or prop picture that comes back without a real share of transparent pixels is cleaned before
use. A picture you import is used as it is unless you marked it. Only a changed upstream file puts something out of date, never a text edit or a render block:
`cozy project render <name>` when you want a new take. A node renders by its `render:` block: `with` a function as `cozy run` takes it (`pkg/fn@0.3.0`, or local code, `./dir/fn` from the
project's folder, which Calcifer makes ready on each `--on` machine), `models` and `lora` as `cozy run --input` takes them, and
`options`, the function's own fields. A block that
names no function adjusts its kind's `defaults:` (entity, setup, world_state, shot, upscale, clean).
`cozy project status` shows what made each approved version. A file you attach (`image:`, or a shot's
`video:`) is used until you approve one cozy made.

`resolution: 1440p` and `aspect_ratio: 16:9` (the default; `9:16` and `2.39:1` work alike) fix the
project's canvas: pictures of places are made at its aspect, and every shot fills it, center-cropped,
never letterboxed or stretched. A shot whose approved version is below the resolution is upscaled
(`upscale` under `defaults:`, or a `render:` under a shot's own `upscale:`, which can change the function, models,
LoRAs and options but never the canvas). `render all` makes the upscales and cuts `edit.mp4` at the
canvas; `preview.mp4`, the native cut, is redone as shots land, and `render all --preview` stops there.
With no `resolution:`, the edit is its first take's own size when that is the aspect but for a model's rounding
(H3's 1344x768 at 16:9), so takes of that size join with no encode.

`fps:` (24 by default) is the edit's frame rate, fixed per project. `edit:` lists shots, each with an optional `start` and `end` in
frames at it (`end` exclusive; by default the shot's first frame and its length, or its take's end if sooner), so every cut lands on an
exact frame; a beat's `duration` and a shot's `writes` stay in seconds. A take at another rate is retimed to
the project's (each frame its nearest in time), and `status` names it. Each cut writes `edit.otio` (and
`preview.otio`) with it: the project's rate, a video and an audio track, one clip per `edit:` entry with its
frames, referencing that take's asset.

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
