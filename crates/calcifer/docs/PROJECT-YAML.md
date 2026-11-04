# How `project.yaml` works (format 2)

A file in format 1 (`entities:`, `shots:`), or a lock from one, is refused and left as it is. The HTTP API is in `CALCIFER-API.md`.

## The idea

A project is a folder of files plus a graph that says how each file was made.

- **Asset:** a stable name holding a set of files, like `kyogre` or `clash`. The graph's nodes.
- **Operation:** the work that makes an asset: a package, plus what the YAML says about the asset. The graph's edges. You don't write edges; `<name>` mentions in text create them.
- **File:** bytes by hash, in `files/<sha256>.<ext>`.
- **Version**, or **take** if it has a video: one result of an operation, recorded in `project.lock`.

Calcifer owns all of this. It knows no kinds and no packages by name. A package makes an asset through an adapter.

## The folder

| Path | Written by | Holds |
|---|---|---|
| `project.yaml` | You, and cozy for pins | What should exist |
| `project.lock` | Cozy only | Every version: its key, seed, files, what it read |
| `files/` | Cozy only | Every file, by hash |
| `<edit name>.mp4`, `.otio` | Cozy | Each cut |
| `.runs/` | Cozy only | Where a run's outputs land on their way into `files/`, and any folder a run made |

## The file

```yaml
version: 2

defaults:                      # the same shape as an asset: it fills what an asset doesn't write
  all:
    style: High-quality Unreal Engine graphics, realistic and detailed
    aspect_ratio: "16:9"
  character:
    render:
      with: qwen-image-2       # which package makes this kind
  environment:
    render:
      with: qwen-image-2
  storyboard:
    render:
      with: qwen-image-2
  shot:
    render:
      with: minimax-h3
      turbo: true              # any other key is the adapter's own
  edit:
    render:
      with: cozy

assets:
  kyogre_art:                  # a constant: a file hash, and nothing to make it from
    kind: image
    image: 65b436d4f9a3

  kyogre:                      # approved: `image:` pins it, and `voice:` goes with it
    image: 2aee8c40753d
    voice: 7c1e04a9b2d5
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon.
    render:                    # at the root what it is; here how to make it
      prompt: |
        <kyogre_art> as a realistic creature, front and back on one sheet, no text.
      aspect_ratio: "3:2"
      megapixels: 2

  kyogre.primordial:           # another form: a plain asset that mentions what it is made from
    kind: character
    description: Kyogre in its ancient form, with glowing blue veins.
    render: <kyogre>, the same creature in the same pose.

  coast:
    kind: environment
    description: |
      A volcanic coast with a dark blue sea on the left and a volcano on the right.

  clash board:                 # a storyboard, drawn ahead of the video
    kind: storyboard
    description: |
      <kyogre> in the surf on the left, <groudon> on the rocks on the right.
    render: A wide shot of <coast>, as a single frame with no text.

  clash:                       # no hash: it renders
    kind: shot
    summary: |
      <kyogre> and <groudon> fight on the coast in one continuous shot.
    soundscape: Crashing surf and deep impacts.
    beats:
      - duration: 14
        description: |
          Following <clash board>, a wide shot of <coast> shows <kyogre> and <groudon>.
        writes:                # screenshots saved as world_state assets
          - world_state: coast.1
            at: 14
      - duration: 14
        description: |
          The fight continues on <coast.1>.

  edit:
    kind: edit
    fps: 24
    pieces:
      - <awaken>
      - { video: <clash>, start: 12, end: 132 }   # a trim, in frames at this fps
```

## What the platform reads

Names, `kind`, `render` (`with`, `prompt`, `models`, `lora`), a hash in `image`, `video`, `audio` or `voice`, a beat's `duration` and `writes`, and `<name>` anywhere in any text. At the top, `version`, `defaults` and `assets`.

Every other field at the root, such as `summary`, `beats`, `description`, `camera` or `fps`, passes through to the adapter untouched and counts as an input.

## `render`

At an asset's root is what it is. In `render` is how to make it: plain text as the short form, or a map.

- **`with`:** the package. **`prompt`:** the text, which reaches the adapter as `fields.instructions`. **`models`, `lora`:** added to its first call.
- **Every other key** is the adapter's own, such as turbo or megapixels, and reaches it as `op.options`. It isn't part of this standard.

A shot can hold just `render: {turbo: false}`. `with`, `prompt`, `models` and `lora` at an asset's root warn and are ignored.

## Defaults

A section has the shape of an asset, and fills what an asset doesn't write: `defaults.character.render.with: qwen-image-2`, `defaults.shot.render.turbo: true`, and `defaults.all.style` for a root field. Render keys merge key by key. An asset naming its own `render.with` takes no render keys from there.

`defaults` holds settings, not what's in the picture or sound, beyond the project-wide `style`. `soundscape` and `music` go on each shot.

A package chooses its own internals, such as how many context frames H3 keeps. They aren't configurable and aren't fields, so they don't go in a project file.

## Kinds

Each first-party kind has a stable set of fields, defined as typed structs in cozy-runtime, so another package can read the same `beats`. Before launch they can change freely. After launch, fields are only added, and a rename gets an alias. A third party can define a new kind, like `mesh`.

| Kind | What it is | First-party package |
|---|---|---|
| `character`, `prop` | The subject alone, on a blank background | `qwen-image-2` |
| `environment` | Background only, no characters | `qwen-image-2` |
| `storyboard` | Characters in a place at a moment, drawn | `qwen-image-2` |
| `world_state` | How things stand at a moment, by default a screenshot from a shot | a shot's `writes`, or `qwen-image-2` |
| `image` | Any other picture; the default with no `kind` | `qwen-image-2` |
| `shot` | Video from a `summary` and `beats` | `minimax-h3` |
| `upscale` | A bigger take: `{kind: upscale, video: <clash>, resolution: 1440p}` | `minimax-h3` |
| `edit` | A cut of other assets. No seed. Add as many cuts as you like. | `cozy`, run by Calcifer itself |

The fields of each first-party kind, beyond what the platform reads:

| Kind | Fields |
|---|---|
| `shot` | `summary`, `beats` (each `description`, `duration`, `writes`), `style`, `soundscape`, `music`, `aspect_ratio`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`) |
| `character`, `prop`, `environment`, `storyboard`, `world_state`, `image` | `description`, `render.prompt`, `style`, `aspect_ratio` |
| `setup` | Those of a picture, plus `camera`, `from_frame` |
| `upscale` | `video`, `resolution`, `style` |

Nothing else is a field of these kinds. At render, a typed adapter warns of an unknown field an asset writes itself. It doesn't warn of one that comes from `defaults` or sits inside a beat: that one is ignored silently, so check it against this table.

**A shot with no `soundscape`** goes to H3 as `N/A`, which it reads as silence.

**Every asset has a kind.** With none written, it comes from the file it holds: `image:` is `image`, `video:` is `video`, `audio:` is `audio`. With no file it is `image`. `status` shows it, and a reader's adapter gets it. A constant needs no adapter or operation, whatever its kind.

**Two texts, on `character`, `environment` and `storyboard`:** `description` goes to the picture model and the video model. `render.prompt` goes only to the picture model, for how to draw it.

## Mentions

`<name>` in any text makes that asset an input. Names match in any case, with `_`, `-` and space alike.

- **In a shot's `summary`:** reaches every segment.
- **In a beat:** reaches only the segment holding that beat. H3 gets confused by extra references, so mention things only where they're needed.
- **Explicit:** `<coast>` always means the asset `coast`, and `<coast.2>` always means `coast.2`. Nothing resolves to "the newest".

## How rendering decides

**An asset's state** is the first of these that applies:
1. A hash in `image`, `video` or `audio`: a constant. Nothing is made for it, but `--new` on one with an operation makes takes to look at, its state unmoved.
2. Its newest version whose inputs match now.
3. Nothing: it's missing or out of date.

**`cozy project render <names|all>`: an asset with no state runs its operation, along with what it reads. Nothing else runs.**

**The memo:** inputs are the package, models, LoRAs, options, kind, every field, and the name, fields and file hashes of everything it reads. Inputs plus the seed never run twice. So putting old text back returns the old take with no run.

**Not inputs:** package, adapter and cozy updates, and the machine or card. An update never re-renders anything. "Deterministic" is the memo's promise, not a fact about bytes: the same seed on another card can differ, which doesn't matter, because a hit reuses the stored file.

**Seed:** the one thing that varies. Calcifer draws it when the function takes one, and records it.

## Commands

| Command | Does |
|---|---|
| `render <names\|all> [--on <machine>]` | Makes what has no state |
| `render <name> --new`, `-n 4` | Draws new seeds and makes more versions |
| `render clash --take 2 --in 15` | Keeps take 2 up to 15 s and regenerates the rest |
| `approve <name> <version>` | Writes that version's hash into `project.yaml`, editing only that line |
| `approve <name> <hash>...` | The same with any files in `files/`, whatever made them; several write a list |
| `unpin <name>` | Removes that line |
| `status` | Each asset's state and why, which version a pin is, whether its text changed since, orphans, and how many files `cozy run` left here |
| `import <file>` | Copies a file or URL into `files/` and prints the line to paste |
| `rename <old> <new>` | Renames an asset and keeps its versions |
| `reclaim [--yes]` | Lists orphans and the bytes they'd free, or with `--yes` deletes their versions and any file nothing else names, a run's among them |
| `push`, `pull` | Sends `files/` to your Tensorhub account, or fetches what's missing here. Commit `project.yaml` and `project.lock`, not `files/` |

## Pins

Approve and pin are the same thing, so reading the file shows what's constant.

An asset is a set of files, each named by its role. A hash in `image`, `video`, `audio` or `voice` pins that file, and a `<mention>` in the same field is an input.

- **The line:** `approve` writes the role of the version's main file, `video: <hash>` for a take and `image:` for a picture, as that one line, keeping comments, order and formatting, and a save made just before it stands. A line holding an input keeps it, `video: <hash>  # was <clash>`, and `unpin` puts it back.
- **Several files:** `image: [65b436d4f9a3, f1a83554adab]`, in the order written, all pinned, all in the memo. Approving a version whose function returned several writes the list. A reader's `files` has them as `image`, `image-2`, `image-3`, as a function's list lands, so one that takes one takes the first.
- **`voice`** only attaches: it doesn't stop the image being made, and goes with whatever the asset's state is.
- **A shot's pin:** `video: <hash>` brings that take's whole set: latents and any world states it wrote. A hash that matches no version is just that file.
- **Other fields while pinned:** `render` and the text stay in the file and are ignored for that asset. Delete the line and they apply again.
- **A pin freezes files, not text:** editing an approved character's `description` doesn't re-render it, but shots that read it go out of date, because H3 receives those words.
- **Pinning a written world state:** it's read and no longer written, which can put the shot that wrote it out of date.

## Orphans

An asset no longer named in the YAML keeps its versions and files. Editing the graph loses nothing as long as names stay the same. Removing or renaming a name in the file orphans its files, and only `reclaim` deletes them.

## Forms

An asset can have other forms: outfits, a primal form, night and day. A form is a plain asset that says what it's made from by mentioning it. The dot means nothing beyond the name.

```yaml
kyogre.primordial:
  kind: character
  description: Kyogre in its ancient form, with glowing blue veins.
  render: <kyogre>, the same creature in the same pose.
```

Its `render.with` and the rest come from `defaults`, like any asset.

## World states and writes

A world state is a continuity still: how things stand at a moment, so the next shot matches.

A write is a screenshot taken at `at` seconds into that beat, the beat's end if left out, saved as a `world_state` asset. A beat may hold several. It exists to carry the world from one run into another, so it only matters across segments and shots: within a segment the model remembers.

- **Reading a world state:** a later segment or another shot gets the picture as a loose guide. H3 reads it small and adds a retention line like "use this layout of characters and background as a starting point". That wording lives in the adapter, not the YAML.
- **Numbers:** you write them, `coast.1`, `coast.2`, so inserting one never moves another's versions or pins.
- **A write is `{world_state, at}`, and has no description.** A picture is the state. To add words, declare the world state as its own asset with a `description`, and the write fills in its picture. Any other field warns.
- **Past its beat,** `at` clamps with a warning. Two assets writing one name: the first stands, with a warning.
- **A write the shot doesn't read after its beat** is a free frame, cut from the take here, outside the shot's memo: moving it re-renders nothing. One the shot reads, in a later beat or its `summary`, goes to the function, in its one take. A package that takes no `writes` gets its frames cut from the take, with a warning.
- **Anything can read one:** `<coast.1>` also works in a Qwen `storyboard`, so a storyboard can start from what really happened.

A world state and a storyboard behave the same today. A `storyboard` is drawn; a `world_state` is, by default, captured by a shot.

## Storyboards

A storyboard is drawn before the video exists. Make cheap pictures, choose one, then render the shot:

```
cozy project render "clash board" -n 4
cozy project approve "clash board" 3
cozy project render clash
```

## Choosing a package, and adapters

`render.with` names a package, `org/package[@release]`, never a function, or local code by its path (`./dir`, `./script.py`, `~/…`), keyed by its directory's name, so moving to the published package re-renders nothing. With no `@release`, a run uses what its machine has installed, or installs the newest if it has none; `cozy package update` moves it forward. The order is the asset's own, then `defaults.<kind>.render.with`. An asset naming its own takes no render keys from `defaults`. With neither, it has nothing to make, and `status` and `render` say so as a warning.

A package makes a kind by providing one Python adapter for it. With no adapter for that kind, it can't make that asset: `status` and `render` say so in one line and carry on. There's no fallback.

```python
@app.project_adapter("shot")
def adapt(op: Operation) -> list[Invocation]: ...
```

For each asset that needs a run, Calcifer:
1. Finds the package and builds an `Operation`: `{version, asset, kind, fields, options, reads, writes, keep}`.
2. Stops there if the memo already has a result.
3. Installs the package on the render machine and runs its adapter as a small CPU run. Adapters are pure, so they only take text, names and hashes.
4. Runs the invocations it returns, in order, adding the seed, and on the first its `models` and `lora`. An empty list means no run: the asset's files are what it reads. In `input`, a string that is exactly `$0.video` means that field of invocation 0's result. A failed one stops the list and makes no version, and the next render tries again with the same seed.
5. Records all the resulting files as one new version, by output name, later over earlier.

The adapter holds all model-specific words and picks the function, such as `generate_turbo` from `render: {turbo: true}`. So a long shot is one operation: H3 splits it into segments itself.

## Building something other than film

Nothing is film-specific except the kinds you choose. Art, then pictures, then 3D models, then a layout, with made-up packages:

```yaml
version: 2
assets:
  automaton art:
    kind: art
    description: A bronze clockwork automaton, waist up.
    render: {with: sketchlab}
  automaton:
    kind: mesh
    sheet: <automaton art>
    render: {with: meshforge, polygons: 40000}
  layout:
    kind: arrange
    brief: A workshop at dusk, with the <automaton> at the bench.
    render: {with: stagehand}
```

There are no loops, so at hundreds of assets a tool or an LLM writes the file.

## Cuts and frames

The package `cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. A cut reads each asset's state, else its newest version, and says so. Same takes, same key, not cut again; a deleted `.mp4` or `.otio` comes back at the next render. It keeps one version, and its pinned one, being free to make again.

- **`kind: edit`:** `pieces` holds `<name>`, the whole take, or `{video: <name>, start: 12, end: 132}`, frames at its `fps` (24 by default), every in and out point exact. It writes `<name>.mp4` and `<name>.otio` at the root, the same edit as an OpenTimelineIO timeline.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every piece fills it, center-cropped, never letterboxed or stretched. A take at another rate is retimed, and `status` names it. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **`kind: frame`:** `{kind: frame, video: <clash>, at: 3, render: {with: cozy}}` is one frame of a video.

## Runs

`cozy run` belongs to a project, as a commit does to a repository. The project is the folder you're in or the nearest above it with a `project.yaml`, or `-p <path>`. Elsewhere it refuses: `not in a project: cd into one, or cozy project new <name>`.

- **Outputs** land in the project's `files/` by hash, listed in `project.lock` under `runs`, and the run prints one line per file to paste: `image: 65b436d4f9a3`. There's no memo.
- **Use one:** `--file image=65b436d4f9a3` names a file by its hash, and `cozy project approve kyogre 65b436d4f9a3` makes it what `kyogre` is. So any model can make the picture of a character.
- **`cozy run list`** in a project lists its runs, and elsewhere or with `--all` every run, each with its project.
- **`reclaim`** lists a run's files once nothing names them.

`cozy machine`, `cozy package`, `cozy model`, `cozy update`, `cozy project new` and `cozy project ls` work anywhere. Updates, warm installs and quantizing are no one's run and stay outside a project.
