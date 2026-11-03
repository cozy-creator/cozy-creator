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

A project is a folder:
- `project.yaml`: yours. cozy writes it at `new`, and one line of it at `approve` and `unpin`, keeping everything else as you wrote it.
- `project.lock`: cozy's record of every file and each asset's takes or versions, and what made each.
- `files/`: every file the project uses, a real file kept by content as `files/<sha256>.<ext>`.
- `edit.mp4`: a film, cut on this computer, and beside it `edit.otio`, the same edit as an OpenTimelineIO timeline, each
  clip referencing its take in `files/`.

Everything in the film is an **asset**: a stable name holding a set of files. An **operation** makes it: a package, by `with`,
and a `kind`, which that package's adapter turns into calls. Each result is a **version**, or a **take** if it holds a video,
numbered from 1 and never renumbered. `cozy project new garden` makes a folder holding a short film that renders as it stands.

```yaml
version: 2
defaults:
  all:
    style: High-quality Unreal Engine graphics.
    aspect_ratio: "16:9"
  character:
    with: qwen-image-2
  shot:
    with: minimax-h3
  edit:
    with: cozy

assets:
  kyogre art:
    image: 47bc2b8d463a          # a hash: a constant, nothing is made
  kyogre:
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon.
    instructions: |
      <kyogre art> as a realistic creature, front and back on one sheet.
  clash:
    kind: shot
    summary: |
      <kyogre> fights on the coast in one continuous shot.
    beats:
      - duration: 14
        description: |
          <kyogre> surges forward and a storm breaks.
        writes:
          - scene: coast.1
            at: 12
      - duration: 10
        description: |
          The fight goes on under <coast.1>.
  edit:
    kind: edit
    fps: 24
    pieces:
      - <clash>
      - {video: <clash>, start: 12, end: 132}
```

`<name>` in any text reads that asset. Cozy reads `with`, `kind`, `models`, `lora`, `options`, a hash in `image`, `video` or
`audio`, and a beat's `duration` and `writes`. Every other field goes to the adapter and is yours and the package's. `with` is
`[org/]package[@release]`, or local code (`./dir`, `./script.py`); with none, of its own or in `defaults.<kind>`, an asset has nothing
to make, and a package with no adapter for its kind can't make it: `status` and `render` say so, and the rest goes on. An asset with an
operation and no `kind` is an `image`. An asset naming its own `with` takes nothing from `defaults`.

**State.** An asset's state is the hash in `project.yaml`, else its newest version for its current inputs, else none. Its inputs are what its
adapter is given: everything the file says of it, and the files and fields of what it mentions. So a comment, a reformat or a package
update changes nothing, a text, option or `defaults` edit puts it out of date, and reverting the text returns the old output with no
run. A new release of a package doesn't re-render anything; `--new` shows what it makes.

```sh
cozy project render all --on <machine>     # everything with no state; cuts follow, free
cozy project render clash --on <machine>   # clash and what it reads, if it has no state
cozy project render clash --new -n 3 --on <machine>   # three more takes
cozy project approve clash 2               # pins take 2
cozy project unpin clash
cozy project render clash --in 15 --on <machine>      # a new take: the take in use up to about 15 s, the rest new
cozy project render clash --take 2 --in 15 --on <machine>
cozy project status                        # what has no state and why, and each cut
cozy project ls                            # every project this computer has opened
```

**Approve** writes `video: b333794f3640` (`image:` for a picture) into that asset in `project.yaml`, and `unpin` removes the line, so
the file shows at a glance what is a constant. Its `with` and text stay, ignored while pinned, and `status` says which version a pin is and
what has changed since. When the hash is a version's, its latents and written world states come with it. A pinned asset is never made
again; `--new` makes takes to look at, its state unmoved. The first `render` of a project with nothing owed sends nothing.

**Files** enter a project one way, and `project.yaml` names them only by hash:

```sh
cozy project import ./coast.jpg
cozy project import https://example.com/groudon.png --character
```

```
imported ab12cd34ef56
image: ab12cd34ef56
```

Any prefix of the hash will do; one that fits two is said. `--character` and `--prop` print two assets to paste: the picture, and its clean.
A URL is downloaded here, up to 64 MiB, from a public address. A path or URL in a file field is never read: it warns with the import
command. A made version is a file too: `status` prints each one's 12 characters.

**Scenes.** A `scene` asset holds a place as it stands, and a beat can write one: `writes: [{scene: coast.1, at: 12}]` is a screenshot `at` seconds
into that beat (its end if left out, clamped with a warning past it), its picture re-used as a reference in a later generation. A scene may be
declared on its own with a `description`; the write only fills in its picture. A mention names exactly that asset: `<coast.1>` is `coast.1`, and
`<coast>` is `coast`. A write the shot reads after its beat goes to its function, inside its key, as one take; any other is a free frame, cut here,
and moving it re-renders nothing.

**Cuts.** `with: cozy`, `kind: edit`, as many as you like, each writing `<name>.mp4` and `<name>.otio` at the root. `pieces` holds `<name>`,
the whole take, or `{video: <name>, start: 12, end: 132}`, in frames at its `fps` (24 by default), every in and out point exact. `aspect_ratio` and
`resolution` set its canvas: every piece fills it, center-cropped, never letterboxed or stretched; a take at another rate is retimed, and
`status` names it. With no `resolution`, the cut is its first take's own size when that is the aspect but for a model's rounding, so takes of that
size join with no encode. A cut runs here, free, once what it reads settles and on `approve`; same takes, not cut again. It reads each
asset's state, else its newest version, and says so. `kind: frame`, `{with: cozy, kind: frame, video: <clash>, at: 3}`, is one frame the same way.

**Moving a project.** Commit `project.yaml` and `project.lock`, keep `files/` out of git, and `cozy project push` sends the files to your
Tensorhub account, private to you. `cozy project pull` next to a clone fetches them as real files. Both send only what is missing and go on
after an interruption. What you pushed stays until you `cozy model delete <account>/project-assets`. A file over 5 GiB is skipped with a
warning: everything else goes, and the command exits 3.

**Renames and space.** Rename an asset in `project.yaml` (and its `<mentions>`), then `cozy project rename <old> <new>` keeps its versions;
`status` suggests it. What reads it goes out of date unless pinned. An asset the file no longer names keeps its versions and files: `status` lists
each with its bytes, `cozy project reclaim` lists what it would free, and `--yes` deletes their versions and any file nothing else names. Never
automatic. `render` runs go on in Calcifer, and Ctrl-C stops following, never the runs.

The project is the folder holding `project.yaml`, found from where you are or above it; `-p <path>` names another.

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
