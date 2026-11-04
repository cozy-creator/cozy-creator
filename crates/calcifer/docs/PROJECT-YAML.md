# How `project.yaml` works (format 2)

A file in format 1 (`entities:`, `shots:`), or a lock from one, is refused and left as it is. The HTTP API is in `CALCIFER-API.md`.

## The idea

A project is a folder of files plus a graph that says how each file was made.

- **Entity:** a thing in the film, like `kyogre`: its identity, and links to the assets that show it. No work, no versions.
- **Asset:** a stable name holding one file of one type, plus what the run that made it left, like `kyogre_sheet` or `edit`.
- **Operation:** the work that makes an asset: a package, plus what the YAML says about the asset. The graph's edges. You don't write edges; `<name>` mentions in text create them. Assets read assets and entities, and entities link to assets. The only rule is no cycles.
- **File:** bytes by hash, in `files/<sha256>.<ext>`.
- **Version**, or **take** if it has a video: one result of an operation, recorded in `project.lock`.

Calcifer owns all of this. It knows no kinds and no packages by name. A package makes an asset through an adapter.

## The folder

| Path | Written by | Holds |
|---|---|---|
| `project.yaml` | You, and cozy for pins | What should exist |
| `project.lock` | Cozy only | Every version: its key, seed, files, what it read |
| `files/` | Cozy only | Every file, by hash |
| `<edit_name>.mp4`, `.otio` | Cozy | Each cut |
| `.runs/` | Cozy only | Where a run's outputs land on their way into `files/`, and any folder a run made |

## The file

```yaml
version: 2

defaults:                      # one per file type: a default `render` block
  image:
    with: qwen-image-2         # which package makes it
    style: High-quality Unreal Engine graphics, realistic and detailed
    aspect_ratio: "16:9"       # any other key is the adapter's own
  video:
    with: minimax-h3
    style: High-quality Unreal Engine graphics, realistic and detailed
    aspect_ratio: "16:9"
    turbo: true

entities:                      # what things are; each links to assets by name
  kyogre:
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon.
    image: <kyogre_sheet>
    voice: <kyogre_voice>
    primordial:                # a sub-entity: what it doesn't have comes from kyogre
      description: Kyogre in its ancient form, with glowing blue veins.
      image: <kyogre_primordial_sheet>
  coast:
    kind: environment
    description: |
      A volcanic coast with a dark blue sea on the left and a volcano on the right.
    image: <coast_plate>

assets:
  kyogre_art:                  # a constant: a file hash, and nothing to make it from
    image: 65b436d4f9a3

  kyogre_voice:
    kind: audio
    audio: 7c1e04a9b2d5

  kyogre_sheet:                # approved: `image:` pins it
    image: 2aee8c40753d
    render:                    # at the root what it is; here how to make it
      prompt: |
        Kyogre, the colossal blue whale-like legendary Pokémon, from <kyogre_art>, front and back on one sheet, no text.
      aspect_ratio: "3:2"
      megapixels: 2

  kyogre_primordial_sheet:
    render: Kyogre in its ancient form, with glowing blue veins, from <kyogre_sheet>, in the same pose.

  coast_plate:
    render: A volcanic coast with a dark blue sea on the left and a volcano on the right.

  clash_board:                 # a storyboard: a picture drawn ahead of the video
    render: <kyogre_sheet> in the surf on the left, on <coast_plate>, as a single frame with no text.

  edit:
    kind: video
    render: {with: cozy}       # a cut, which Calcifer makes itself
    fps: 24
    pieces:
      - <awaken>
      - { video: <clash>, start: 12, end: 132 }   # a trim, in frames at this fps

  clash:                       # a shot, with no hash: it renders
    kind: video
    summary: |
      <kyogre> and <groudon> fight on the coast in one continuous shot.
    soundscape: Crashing surf and deep impacts.
    beats:
      - duration: 14
        description: |
          Following <clash_board>, a wide shot of <coast> shows <kyogre> and <groudon>.
        writes:                # screenshots saved as image assets
          - image: coast.1
            at: 14
      - duration: 14
        description: |
          The fight continues on <coast.1>.
```

## What the platform reads

Names, `kind`, `render` (`with`, `prompt`, `models`, `lora`), a hash in the field of an asset's type, an entity's links, a beat's `duration` and `writes`, and `<name>` anywhere in any text. At the top, `version`, `defaults`, `entities` and `assets`, one namespace.

Every other field at an asset's root, such as `summary`, `beats`, `description`, `camera` or `fps`, passes through to the adapter untouched and counts as an input.

## `render`

At an asset's root is what it is. In `render` is how to make it: plain text as the short form, or a map.

- **`with`:** the package. **`prompt`:** the text, which reaches the adapter as `fields.instructions`. **`models`, `lora`:** added to its first call.
- **Every other key** is the adapter's own, such as turbo or megapixels, and reaches it as `op.options`. It isn't part of this standard.

A shot can hold just `render: {turbo: false}`. `with`, `models`, `lora`, `options` and `instructions` at an asset's root warn and are ignored.

## Defaults

The only keys are `image`, `video` and `audio`; any other warns and is ignored. Each is a default `render` block, and merges key by key under the asset's own: `with`, `models`, `lora` and any other key, which reach the adapter as `op.options`, as `style` and `aspect_ratio` do. An asset naming its own `with` takes nothing from defaults, and a constant needs none.

A package chooses its own internals, such as how many context frames H3 keeps. They aren't configurable and aren't fields, so they don't go in a project file.

## Kinds

- **An asset's `kind` is its file's type:** `image` (the default), `video` or `audio`. With none written, it comes from the field holding its hash. Anything else warns and is ignored: it belongs on an entity.
- **A shot** is a `kind: video` asset with `summary` and `beats`, pinned with `video:`.
- **An entity's `kind` is free:** `character`, `prop`, `environment`. Calcifer knows no kinds.
- **On the wire:** `Operation.kind` is `image`, `video` or `audio`, what is being produced. `Read.kind` is the entity's kind for an entity mention, else the file type.

| First-party | Fields, beyond what the platform reads |
|---|---|
| A shot, `minimax-h3` | `summary`, `beats` (each `description`, `duration`, `writes`), `soundscape`, `music`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`) |
| An upscale, a `video` asset of `minimax-h3` | `video`, `resolution` |
| An `image`, `qwen-image-2` | `description`, `render.prompt` |
| A cut or frame, `cozy` | See below |

At render, a typed adapter warns of an unknown field an asset writes itself. **A shot with no `soundscape`** goes to H3 as `N/A`, which it reads as silence.

## Mentions

`<name>` in any text makes that asset an input.

**Names** of assets, entities and sub-entities hold only letters, digits, `_`, `-` and `.`, such as `kyogre_sheet` or `coast.1`. Anything else is refused when the file is read, in one line with the fix, and nothing renders: `` `kyogre sheet`: names can't hold spaces or commas; use `kyogre_sheet` ``. `rename` takes the same rule. Names match in any case, with `_` and `-` alike.

- **In a shot's `summary`:** reaches every segment.
- **In a beat:** reaches only the segment holding that beat. H3 gets confused by extra references, so mention things only where they're needed.
- **Explicit:** `<coast>` always means what is named `coast`, and `<coast.2>` always means `coast.2`. Nothing resolves to "the newest".
- **Of an entity:** see below.
- **A package's own tags:** an adapter can declare them, `@app.project_adapter("video", reserved=["d", "scenetrans", "cutoff"])`. In the text of an asset that package makes, each stays text, exactly as written, with no warning, and reaches the adapter unchanged. A project naming something the same is warned at the name, and `<d>` is the tag.
- **What can't be a name** is text, with no warning, as `<Picture 1>` and `</d>` are.
- **A name that names nothing** warns, and stays text: `<kyogee>: did you mean <kyogre>?` when it is within two edits of a real name and at least four letters long, else that it is nothing in the project. Until a package is read, at its first render, its assets' are held back, because they may be its tags.
- **A cycle:** a mention that closes one, A mentioning B and B mentioning A, stays plain text, with a warning, and both still render. An entity's links count: `kyogre_sheet` mentioning `<kyogre>`, which links to it, is a cycle, and so is a sub-entity's asset mentioning its parent.

## Entities

An entity holds `kind`, `description`, and links: any other key whose value is `<asset_name>`, or a file's hash as shorthand. `kyogre: {kind: character, image: 2aee8c40753d}` makes an ordinary constant asset named `kyogre.image`, so the graph, reads and memo don't change and it shows in `status`; `approve kyogre.image <hash>` rewrites that line, and a sub-entity's is `kyogre.primordial.image`. Anything else there warns and is ignored. It has no `render`, versions or pins; a `render` warns and is ignored. It isn't a node of work: nothing renders it.

| Mention | What the adapter gets |
|---|---|
| `<kyogre>` | A read of the entity: its `kind`, `description`, and files from its `image`, `video`, `audio` and `voice` links, each that asset's main file. Other links aren't sent. |
| `<kyogre.primordial>`, a sub-entity | Its own `description` and files, and what it doesn't have from its parent, such as `kind` and `voice`. |
| `<kyogre.pose>`, a link | That asset's files, as a read named `kyogre.pose` with the entity's kind, and the asset's own `description` if it has one, else the entity's. |
| `<kyogre_sheet>` | The asset itself. |

- **Sub-entities:** a key whose value is a map, one level deep. A map inside one warns and is ignored.
- **Memo:** a read's description and file hashes count, so editing an entity's description puts what reads it out of date.
- **Names** are shared across both sections. A clash warns, and the asset stands.
- **`status`** lists entities apart with their links, and a link to nothing warns with the fix. **`render <entity>`** renders its linked assets and its sub-entities'.
- **A voice** is its own asset (`kind: audio`, `audio: <hash>`, or made by a package), linked as `voice:`. Several pictures are several assets.

## How rendering decides

**An asset's state** is the first of these that applies:
1. A hash in the field of its type: a constant. Nothing is made for it, but naming one with an operation makes a version to look at, its state unmoved.
2. Its newest version whose inputs match now.
3. Nothing: it's missing or out of date.

**`cozy project render` makes new versions, except of what is pinned.** A pinned asset is only made when it is named, and that makes a version to look at: only `approve`, `--approve` or `unpin` moves a pin. `--missing` makes only what has no state.

**The memo:** inputs are the package, models, LoRAs, options, kind, every field, and the name, fields and file hashes of everything it reads. Inputs plus the seed never run twice: a seed fixed in `render`, or a function with no seed, has one result per inputs, and says so with no run. Putting old text back returns the old take as its state, with no run.

**Not inputs:** package, adapter and cozy updates, and the machine or card. An update never re-renders anything. "Deterministic" is the memo's promise, not a fact about bytes: the same seed on another card can differ, which doesn't matter, because a hit reuses the stored file.

**Seed:** the one thing that varies. Calcifer draws it when the function takes one, and records it.

## Commands

| Command | Does |
|---|---|
| `render all [--on <machine>]` | A new version, with a new seed, of every asset with an operation, each reading the new ones before it. Pinned assets are skipped, and said |
| `render <name>`, `-n 4` | A new version of it, or four, and of what it reads only if that has no state. Naming a pinned asset makes a version to look at, and the pin stays |
| `render <entity>` | Its linked assets, skipping pinned ones, as an entity's name doesn't name them |
| `render <names\|all> --missing` | Only what has no state: continues a render that half failed without paying twice |
| `render <names\|all> --queue` | Asks and returns at once, printing what it queued and what's ahead of each |
| `cancel <names\|all>` | Drops their queued work and cancels their runs; `all` is everything the walk owes |
| `render clash --take 2 --in 15` | Keeps take 2 up to 15 s and regenerates the rest |
| `approve <name> <version>` | Writes that version's hash into `project.yaml`, editing only that line |
| `approve <name> <hash>` | The same with any file in `files/`, whatever made it |
| `unpin <name>` | Removes that line |
| `status` | Each asset's state and why, which version a pin is, whether its text changed since, orphans, and how many files `cozy run` left here |
| `import <file>` | Copies a file or URL into `files/` and prints the line to paste. No machine needed |
| `import <file> --remove-background [--on <machine>]` | Runs `birefnet/remove_background` on it, keeps only the result, and prints its line |
| `rename <old> <new>` | Renames an asset and keeps its versions |
| `reclaim [--yes]` | Lists orphans and the bytes they'd free, or with `--yes` deletes their versions and any file nothing else names, a run's among them |
| `push`, `pull` | Sends `files/` to your Tensorhub account, or fetches what's missing here. A file over 5 GiB goes as parts, which `project.lock` lists. Commit `project.yaml` and `project.lock`, not `files/` |

## Queueing

A project has one walker, and each render adds to it: a second `render all` is a second pass of new versions, joining the first, and `--queue` adds without following. Without `--queue` a render follows until it ends, and its exit code says how it went; Ctrl-C only stops following, and the work stays queued. `status` shows what is running and what is queued, and queued work outlives a Calcifer restart until `cancel` drops it.

- **A render uses the file as it was when you asked, so keep editing.** Each ask keeps that text in `project.lock` until it is done, so a restart resumes it, and an edit, `approve` or `unpin` touches nothing of it. Each plans on its own text, and independent work goes at once across all of them. What you edit shows out of date in `status` until a render makes it, and an asset you delete from the file is still made by a render that had it.
- **A save that can't be read** refuses a new `render`, and does nothing else. `status` says what's wrong, and `cancel` works.
- **`--on`:** each render's work goes where it said.
- **`--missing`** doesn't make again what another render is making with the same inputs. A plain render makes its own version.

## Pins

Approve and pin are the same thing, so reading the file shows what's constant.

An asset has one main file, from its one `render` or its one pin; any other files are what the same run left, such as a video's latents, or `image-2` from a function that returned several. **Untyped outputs, such as latents, stay on the machine that made them** when the run made anything else: `project.lock` records the hash and that machine, and `files/` holds nothing until something needs it. `--take`, `--in` and an upscale read them there by hash, or fetch them for another machine. A machine keeps them while it lives, for 7 days from their last use or less on a low disk; once they're gone `--take` and `--in` refuse, saying `render <name>` renders the whole shot, and an upscale encodes the video. A hash in the field of its type pins it, and a `<mention>` there is an input. A hash in another file field warns and is ignored, as does a list: make an asset for each.

- **The line:** `approve` writes the field of its type, `video: <hash>` for a take and `image:` for a picture, as that one line, keeping comments, order and formatting, and a save made just before it stands. A line holding an input keeps it, `video: <hash>  # was <clash>`, and `unpin` puts it back.
- **A shot's pin:** `video: <hash>` brings that take's whole set: latents and any world states it wrote. A hash that matches no version is just that file.
- **Other fields while pinned:** `render` and the text stay in the file and are ignored for that asset. Delete the line and they apply again.
- **A pin freezes files, not text:** editing an approved character's `description` doesn't re-render it, but shots that read it go out of date, because H3 receives those words.
- **Pinning a written world state:** it's read and no longer written, which can put the shot that wrote it out of date.

## Import and the background

`cozy project import ./kyogre.png --remove-background` runs `birefnet/remove_background` on it as a normal run of the project, keeps only the cleaned file in `files/`, and prints one line: `image: <the cleaned hash>`.

- **Where:** `--on <machine>` if you name one, else this computer if it can run models, else a machine you have up. With none, or a run that can't start, it says what to do, and a plain copy is kept and its line printed, so nothing is lost.
- **Already transparent,** at 5% or more: kept as it is, with no run, and it says so.
- **The same file again** runs again: there is no memo, as with any `cozy run`.

## Orphans

An asset no longer named in the YAML keeps its versions and files. Editing the graph loses nothing as long as names stay the same. Removing or renaming a name in the file orphans its files, and only `reclaim` deletes them.

## Writes

A write is a screenshot taken at `at` seconds into that beat, the beat's end if left out, saved as an image asset: a continuity still, how things stand at a moment, so the next shot matches. A beat may hold several. It only matters across segments and shots: within a segment the model remembers.

- **Reading one:** a later segment or another shot gets the picture as a loose guide; assets read it too. That wording lives in the adapter, not the YAML.
- **Numbers:** you write them, `coast.1`, `coast.2`, so inserting one never moves another's versions or pins.
- **A write is `{image, at}`, and has no description.** A picture is the state. To add words, declare it as its own asset with a `description`, and the write fills in its picture. Any other field warns.
- **On a `video` asset's beats only.** Past its beat, `at` clamps with a warning. Two assets writing one name: the first stands, with a warning.
- **A write the shot doesn't read after its beat** is a free frame, cut from the take here, outside the shot's memo: moving it re-renders nothing. One the shot reads, in a later beat or its `summary`, goes to the function, in its one take. A package that takes no `writes` gets its frames cut from the take, with a warning.

## Storyboards

A storyboard is an image asset drawn before the video exists. Make cheap pictures, choose one, then render the shot:

```
cozy project render clash_board -n 4
cozy project approve clash_board 3
cozy project render clash
```

## Choosing a package, and adapters

`render.with` names a package, `org/package[@release]`, never a function, or local code by its path (`./dir`, `./script.py`, `~/…`), keyed by its directory's name, so moving to the published package re-renders nothing. With no `@release`, a run uses what its machine has installed, or installs the newest if it has none; `cozy package update` moves it forward. The order is the asset's own, then `defaults.<type>.with`. An asset naming its own takes no render keys from `defaults`. With neither, it has nothing to make, and `status` and `render` say so as a warning.

A package makes a file type by providing one Python adapter for it, as `render` is about the file being produced. With no adapter for that type, it can't make that asset: `status` and `render` say so in one line and carry on. There's no fallback.

```python
@app.project_adapter("video")
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

Nothing is film-specific. Art, then pictures, then a turntable, with made-up packages:

```yaml
version: 2
assets:
  automaton_art:
    description: A bronze clockwork automaton, waist up.
    render: {with: sketchlab}
  automaton:
    sheet: <automaton_art>
    render: {with: portraitlab, polygons: 40000}
  turntable:
    kind: video
    brief: A workshop at dusk, with the <automaton> at the bench.
    render: {with: stagehand}
```

There are no loops, so at hundreds of assets a tool or an LLM writes the file.

## Cuts and frames

The package `cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. A cut reads each asset's state, else its newest version, and says so. Same takes, same key, not cut again; a deleted `.mp4` or `.otio` comes back at the next render. It keeps one version, and its pinned one, being free to make again.

- **A cut, `kind: video`:** `pieces` holds `<name>`, the whole take, or `{video: <name>, start: 12, end: 132}`, frames at its `fps` (24 by default), every in and out point exact. It writes `<name>.mp4` and `<name>.otio` at the root, the same edit as an OpenTimelineIO timeline.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every piece fills it, center-cropped, never letterboxed or stretched. A take at another rate is retimed, and `status` names it. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **A frame, `kind: image`:** `{video: <clash>, at: 3, render: {with: cozy}}` is one frame of a video.

## Runs

`cozy run` belongs to a project, as a commit does to a repository. The project is the folder you're in or the nearest above it with a `project.yaml`, or `-p <path>`. Elsewhere it refuses: `not in a project: cd into one, or cozy project new <name>`.

- **Outputs** land in the project's `files/` by hash, listed in `project.lock` under `runs`, and the run prints one line per file to paste: `image: 65b436d4f9a3`. There's no memo.
- **Use one:** `--file image=65b436d4f9a3` names a file by its hash, and `cozy project approve kyogre 65b436d4f9a3` makes it what `kyogre` is. So any model can make the picture of a character: link that asset from the entity.
- **`cozy run list`** in a project lists its runs, and elsewhere or with `--all` every run, each with its project.
- **`reclaim`** lists a run's files once nothing names them.

`cozy machine`, `cozy package`, `cozy model`, `cozy update`, `cozy project new` and `cozy project ls` work anywhere. Updates, warm installs and quantizing are no one's run and stay outside a project.
