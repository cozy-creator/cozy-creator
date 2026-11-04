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
| `project.yaml` | You, and cozy for pins, imports and renames | What should exist |
| `project.lock` | Cozy only | Every version: its key, seed, files, what it read |
| `files/` | Cozy only | Every file, by hash |
| `project.canvas.yaml` | Cozy, for the page | Where each element sits: one line per card, sorted by name, so git merges it. Nothing reads it but the page, and nothing in it is an input |
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
      - duration: 14
        description: |
          The fight continues on <coast>.
```

## What the platform reads

Names, `kind`, `render` (`with`, `prompt`, `models`, `lora`), a hash in the field of an asset's type, an entity's links, and `<name>` anywhere in any text. At the top, `version`, `defaults`, `entities` and `assets`, one namespace.

Every other field at an asset's root, such as `summary`, `beats`, `description`, `camera` or `fps`, passes through to the adapter untouched and counts as an input.

## `render`

At an asset's root is what it is. In `render` is how to make it: plain text as the short form, or a map.

- **`with`:** the function or package. **`prompt`:** the text, which reaches the adapter as `fields.instructions`. **`models`, `lora`:** added to its first call.
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
| A shot, `minimax-h3` | `summary`, `beats` (each `description`, `duration`), `soundscape`, `music`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`) |
| An upscale, a `video` asset of `minimax-h3` | `video`, `resolution` |
| An `image`, `qwen-image-2` | `description`, `render.prompt` |
| A cut or frame, `cozy` | See below |

At render, a typed adapter warns of an unknown field an asset writes itself. **A shot with no `soundscape`** goes to H3 as `N/A`, which it reads as silence.

## Mentions

`<name>` in any text makes that asset an input.

**Names** of assets, entities and sub-entities hold only letters, digits, `_`, `-` and `.`, such as `kyogre_sheet`. Anything else is refused when the file is read, in one line with the fix, and nothing renders: `` `kyogre sheet`: names can't hold spaces or commas; use `kyogre_sheet` ``. `rename` takes the same rule. Names match in any case, with `_` and `-` alike.

- **In a shot's `summary`:** reaches every segment.
- **In a beat:** reaches only the segment holding that beat. H3 gets confused by extra references, so mention things only where they're needed.
- **Explicit:** `<coast>` always means what is named `coast`. Nothing resolves to "the newest".
- **Of an entity:** see below.
- **A package's own tags:** an adapter can declare them, `@app.project_adapter("video", reserved=["d", "scenetrans", "cutoff"])`. In the text of an asset that package makes, each stays text in any case (`<d>`, `<D>`), exactly as written, with no warning, and reaches the adapter unchanged. A project naming something the same, in any case, is warned at the name, and `<d>` is the tag; a tag is never a `did you mean`.
- **What can't be a name** is text, with no warning, as `<Picture 1>` and `</d>` are.
- **A name that names nothing** fails that asset before anything runs, at no cost: `<Image-1> is no longer in the project`, with `did you mean <kyogre>?` when it is within two edits of a real name and at least four letters long. Bring it back, or edit the text. The failure stays on the asset until a version lands or its inputs change, what reads it waits, and everything else goes on. `status` says it ahead of a render. Until a package is read, at its first render, its assets' are held back, because they may be its tags. A pinned asset runs nothing, so it doesn't fail, whatever its text says, unless you name it to make another version. A cut leaves out a piece it can't read, as below.
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
- **`status`** lists entities apart with their links, and a link to nothing warns with the fix. What reads an entity whose `image`, `video`, `audio` or `voice` links to nothing fails as a name that names nothing does. **`render <entity>`** renders its linked assets and its sub-entities'.
- **A voice** is its own asset (`kind: audio`, `audio: <hash>`, or made by a package), linked as `voice:`. Several pictures are several assets.

## How rendering decides

**An asset's state** is the first of these that applies:
1. A hash in the field of its type: a constant, or with versions of its own, what was approved of them. Nothing is made for a constant, but naming one with an operation makes a version to look at, its state unmoved.
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
| `render all --outdated` | As `render all`, and each pin out of date when its turn comes, reading the new ones before it, so a pin downstream of a new version counts. Its pin goes when the new version lands, and the version it held stays, to approve again. If it can't be made, the pin stands. A pin no version is behind, such as a dropped file, stays, and so does one with an input gone from the project: a deleted input alone doesn't put a pin out of date |
| `render <names\|all> --queue` | Asks and returns at once, printing what it queued and what's ahead of each |
| `cancel <names\|all>` | Drops their queued work and cancels their runs; `all` is everything the walk owes |
| `render clash --take 2 --in 15` | Keeps take 2 up to 15 s and regenerates the rest |
| `approve <name> <version>` | Writes that version's hash into `project.yaml`, editing only that line |
| `approve <name> <hash>` | The same with any file in `files/`, whatever made it |
| `unpin <name>` | Removes that line |
| `status` | Each asset's state and why, which version a pin is, whether its text changed since, orphans, and how many files `cozy run` left here |
| `import <file> [name]` | Keeps a file or URL in `files/`, adds it to `assets:` as one constant, `kyogre_art: {image: 65b436d4f9a3}`, and prints its name. No machine needed |
| `import <file> [name] --remove-background [--on <machine>]` | Runs `birefnet/remove_background` on it, and adds only the result |
| `rename <old> <new>` | Renames an asset in `project.yaml` and in every `<mention>`, and keeps its versions. If you renamed it there already, this only moves its versions. During a render the file changes at once, and its versions move as that render ends |
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

An asset has one main file, from its one `render` or its one pin; any other files are what the same run left, such as a video's latents, or `image-2` from a function that returned several. **Untyped outputs, such as latents, stay on the machine that made them** when the run made anything else: `project.lock` records the hash and that machine, and `files/` holds nothing until something needs it. `--take`, `--in` and an upscale read them there by hash, or fetch them for another machine while it lives; once it's gone `--take` and `--in` refuse, saying `render <name>` renders the whole shot, and an upscale encodes the video. A hash in the field of its type pins it, and a `<mention>` there is an input. A hash in another file field warns and is ignored, as does a list: make an asset for each.

- **A hash is its text.** One that looks like a number, `12345678e012` or `012345678901`, is that hash, quoted or not, in an asset's `image`, `video` or `audio` and anywhere in an entity. Cozy quotes the ones it writes, so other YAML readers see them the same way. Anywhere else a number is a number, and one too big to be one, `1e999`, is its text.
- **The line:** `approve` writes the field of its type, `video: <hash>` for a take and `image:` for a picture, as that one line, keeping comments, order and formatting, and a save made just before it stands. A line holding an input keeps it, `video: <hash>  # was <clash>`, and `unpin` puts it back.
- **A shot's pin:** `video: <hash>` brings that take's whole set, its latents too. A hash that matches no version is just that file.
- **Other fields while pinned:** `render` and the text stay in the file and are ignored for that asset. Delete the line and they apply again.
- **A pin freezes files, not text:** editing an approved character's `description` doesn't re-render it, but shots that read it go out of date, because H3 receives those words.

## Import and the background

`cozy project import ./kyogre.png [name]` is one act: the file in `files/`, its name in `project.yaml`, its hash never handled. With no name it takes the file's, lower-cased: `Kyogre Art (1).png` is `kyogre_art_1`. A name you write keeps its case. Whatever a name can't hold becomes `_`, `.` too. A name the project has gets `_2`, so none is refused, and it prints the one it got. The same file twice is two elements and one file.

`cozy project import ./kyogre.png --remove-background` runs `birefnet/remove_background` on it as a normal run of the project, keeps only the cleaned file in `files/`, adds that as the element, and prints its name. Until the run lands there is no element.

- **Where:** `--on <machine>` if you name one, else this computer if it can run models, else a machine you have up. With none, or a run that can't start, it says what to do, and a plain copy is the element, so nothing is lost.
- **Already transparent,** at 5% or more: kept as it is, with no run, and it says so.
- **The same file again** runs again: there is no memo, as with any `cozy run`.

## Orphans

An asset no longer named in the YAML keeps its versions and files. Editing the graph loses nothing as long as names stay the same. Removing or renaming a name in the file orphans its files, and only `reclaim` deletes them.

## Storyboards

A storyboard is an image asset drawn before the video exists. Make cheap pictures, choose one, then render the shot:

```
cozy project render clash_board -n 4
cozy project approve clash_board 3
cozy project render clash
```

## Choosing a package, and adapters

`render.with` names what makes it: a function, `org/package/function[@release]` or `package/function`, or a package alone, `org/package[@release]`, or local code by its path (`./dir`, `./dir/function`, `./script.py`, `~/…`), keyed by its directory's name, so moving to the published package re-renders nothing. The function is an input, so changing it can put a take out of date. `x/y` reads as `cozy run` reads it: root package x's function y if the Hub has one, else account x's package y. With no `@release`, a run uses what its machine has installed, or installs the newest if it has none; `cozy package update` moves it forward. The order is the asset's own, then `defaults.<type>.with`. An asset naming its own takes no render keys from `defaults`. With neither, or with nothing said of it (`name:` alone), it has nothing to make, and `status` and `render` say so as a warning.

**A function** outputs a picture, a video or a sound, and that's the asset's `kind`: write it, or a line says what to write. **An adapter** translates: it's told the function and turns the asset into that function's input, with the words around a mention or a shot as segments. A function no adapter speaks for is called directly with the asset's fields, each `<mention>` the file it names (`CALCIFER-API.md`).

```python
@app.project_adapter("video", functions=["upscale"])     # project-adapter-video-upscale: told `op.function`; its typed `fields` are the tool's box
def adapt_upscale(op: UpscaleOperation) -> list[Invocation]: ...

@app.project_adapter("video")                            # project-adapter-video: only a package named, so it picks
def adapt_video(op: VideoOperation) -> list[Invocation]: ...
```

For each asset that needs a run, Calcifer:
1. Finds the package and function and builds an `Operation`: `{version, asset, kind, function, fields, options, reads, keep}`.
2. Stops there if the memo already has a result.
3. Installs the package on the render machine and runs its adapter as a small CPU run, or goes straight to the function if none speaks for it. Adapters are pure, so they only take text, names and hashes.
4. Runs the invocations it returns, in order, adding the seed, and on the first its `models` and `lora`. An empty list means no run: the asset's files are what it reads. In `input`, a string that is exactly `$0.video` means that field of invocation 0's result. A failed one stops the list and makes no version, and the next render tries again with the same seed.
5. Records all the resulting files as one new version, by output name, later over earlier.

**What crosses a mention** is a read's `description`, never what made it: `render.prompt` goes only to the function making that asset. A shot is one operation: H3 splits it into segments itself.

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

The package `cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. A cut reads each asset's state, else its newest version, and says so. The same edit isn't rendered again, named or not: same takes, same trims, same key, and it says `(unchanged)`; a deleted `.mp4` or `.otio` comes back at the next render. Each different edit is a version, as anything else is, holding its film, its timeline and the clips that made it: status says what each was cut from, and `cozy project approve <name> <version>` picks one. The film and its timeline at the root follow the state; a file put there that is no version of it is kept beside it. A frame keeps one version, and its pinned one, being free to make again.

- **A cut, `kind: video`:** `pieces` holds `<name>`, the whole take, `{video: <name>, start: 12, end: 132}`, frames at its `fps` (24 by default) with `end` exclusive, every in and out point exact, or a still, `{image: <name>, frames: 48}`, 3 s with no `frames`. A page and the API say these as clips, `{clip: "<name>", in, out}` and `{clip: "<name>", frames}`, in `timeline: {tracks: [{clips}]}`. Sound comes with each clip, and silence with one that has none. It writes `<name>.mp4` and `<name>.otio` at the root, the same edit as an OpenTimelineIO timeline. A piece not on this computer is fetched, from the machine that keeps it or the Hub. One that can't be had, that has no version yet, or that names nothing, is left out with a warning naming it: the rest is cut, as a new version, and the closing line and `status` say `(without <piece>: <why>)`. The next render cuts it whole once that piece is there. A piece here that ffprobe can't read fails the cut, with ffprobe's answer, and the film that stands stays. A film whose frames don't match what its pieces add up to isn't its state, and cuts again at the next render.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every piece fills it, center-cropped, never letterboxed or stretched, a still's transparency going to black. A take at another rate is retimed, and `status` names it. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **A frame, `kind: image`:** `{video: <clash>, at: 3, render: {with: cozy}}` is one frame of a video. A shot makes no pictures: `writes:`, on a beat or an asset, warns and is ignored, and a mention of what it named names nothing.

## Runs

`cozy run` belongs to a project, as a commit does to a repository. The project is the folder you're in or the nearest above it with a `project.yaml`, or `-p <path>`. Elsewhere it refuses: `not in a project: cd into one, or cozy project new <name>`.

- **Outputs** land in the project's `files/` by hash, listed in `project.lock` under `runs`, and the run prints one line per file to paste: `image: 65b436d4f9a3`. There's no memo.
- **Use one:** `--file image=65b436d4f9a3` names a file by its hash, and `cozy project approve kyogre 65b436d4f9a3` makes it what `kyogre` is. So any model can make the picture of a character: link that asset from the entity.
- **`cozy run list`** in a project lists its runs, and elsewhere or with `--all` every run, each with its project.
- **`reclaim`** lists a run's files once nothing names them.

`cozy machine`, `cozy package`, `cozy model`, `cozy update`, `cozy project new` and `cozy project ls` work anywhere. Updates, warm installs and quantizing are no one's run and stay outside a project.
