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

## The file

```yaml
version: 2

defaults:
  all:                         # only what every kind shares; fills assets that don't write it
    style: High-quality Unreal Engine graphics, realistic and detailed
    aspect_ratio: "16:9"
  character:
    with: qwen-image-2         # which package makes this kind
  environment:
    with: qwen-image-2
  scene:
    with: qwen-image-2
  shot:
    with: minimax-h3
    options:                   # a model's own settings: any field its adapter reads
      turbo: true
  edit:
    with: cozy

assets:
  kyogre_art:                  # a constant: a file hash, so nothing is made
    image: 65b436d4f9a3

  kyogre:                      # approved: `image:` pins it, so it's a constant too
    image: 2aee8c40753d
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon.
    instructions: |
      <kyogre_art> as a realistic creature, front and back on one sheet, no text.
    options:
      aspect_ratio: "3:2"
      megapixels: 2

  coast:
    kind: environment
    description: |
      A volcanic coast with a dark blue sea on the left and a volcano on the right.

  clash board:                 # a scene drawn ahead of the video
    kind: scene
    description: |
      <kyogre> in the surf on the left, <groudon> on the rocks on the right.
    instructions: |
      A wide shot of <coast>, as a single frame with no text.

  clash:                       # no hash: it renders
    kind: shot
    summary: |
      <kyogre> and <groudon> fight on the coast in one continuous shot.
    soundscape: Crashing surf and deep impacts.
    beats:
      - duration: 14
        description: |
          Following <clash board>, a wide shot of <coast> shows <kyogre> and <groudon>.
        writes:                # screenshots saved as scene assets
          - scene: coast.1
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

Names, `with`, `kind`, `models`, `lora`, `options`, a hash in `image`, `video` or `audio`, a beat's `duration` and `writes`, and `<name>` anywhere in any text. At the top, `version`, `defaults` and `assets`.

Every other field, such as `summary`, `beats`, `description`, `instructions`, `camera` or `fps`, passes through to the adapter untouched and counts as an input.

## Defaults

`defaults.all` is only for what every kind shares, such as `style`.

**`options`** holds a model's own settings, such as turbo or a default sound prompt, and isn't part of this standard: it's an unstructured map, any field to any value, on `defaults.<kind>` and on an asset, the asset's values over the defaults' key by key. It goes only to the adapter, as `op.options`, which turns it into that package's calls; another model has other fields. Cozy adds nothing from it to a call. It counts in the memo, and an asset naming its own `with:` takes none from defaults.

`defaults` holds settings, not what's in the picture or sound, beyond the project-wide `style`. `soundscape` and `music` go on each shot.

A package chooses its own internals, such as how many context frames H3 keeps. They aren't configurable and aren't fields, so they don't go in a project file.

## Kinds

Each first-party kind has a stable set of fields, defined as typed structs in cozy-runtime, so another package can read the same `beats`. Before launch they can change freely. After launch, fields are only added, and a rename gets an alias. A third party can define a new kind, like `mesh`.

| Kind | What it is | First-party package |
|---|---|---|
| `character`, `prop` | The subject alone, on a blank background | `qwen-image-2` |
| `environment` | Background only, no characters | `qwen-image-2` |
| `scene` | Characters in a place at a moment: drawn, or a screenshot from a shot | `qwen-image-2`, or a shot's `writes` |
| `image` | Any other picture; the default with no `kind` | `qwen-image-2` |
| `shot` | Video from a `summary` and `beats` | `minimax-h3` |
| `upscale` | A bigger take: `{kind: upscale, video: <clash>, resolution: 1440p}` | `minimax-h3` |
| `edit` | A cut of other assets. No seed. Add as many cuts as you like. | `cozy`, run by Calcifer itself |

The fields of each first-party kind, beyond what the platform reads:

| Kind | Fields |
|---|---|
| `shot` | `summary`, `beats` (each `description`, `duration`, `writes`), `style`, `soundscape`, `music`, `aspect_ratio`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`) |
| `character`, `prop`, `environment`, `scene`, `image` | `description`, `instructions`, `style`, `aspect_ratio` |
| `setup` | Those of a picture, plus `camera`, `from_frame` |
| `upscale` | `video`, `resolution`, `style` |

Nothing else is a field of these kinds. At render, a typed adapter warns of an unknown field an asset writes itself. It doesn't warn of one that comes from `defaults` or sits inside a beat: that one is ignored silently, so check it against this table.

**A shot with no `soundscape`** goes to H3 as `N/A`, which it reads as silence.

**Two text fields, on `character`, `environment` and `scene`:** `description` goes to the picture model and the video model. `instructions` goes only to the picture model, for how to draw it.

## Mentions

`<name>` in any text makes that asset an input. Names match in any case, with `_`, `-` and space alike.

- **In a shot's `summary`:** reaches every segment.
- **In a beat:** reaches only the segment holding that beat. H3 gets confused by extra references, so mention things only where they're needed.
- **Explicit:** `<coast>` always means the asset `coast`, and `<coast.2>` always means `coast.2`. Nothing resolves to "the newest".

## How rendering decides

**An asset's state** is the first of these that applies:
1. A hash in the YAML: a constant. Nothing is made for it, but `--new` on one with an operation makes takes to look at, its state unmoved.
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
| `unpin <name>` | Removes that line |
| `status` | Each asset's state and why, which version a pin is, whether its text changed since, and orphans |
| `import <file>` | Copies a file or URL into `files/` and prints the line to paste |
| `rename <old> <new>` | Renames an asset and keeps its versions |
| `reclaim [--yes]` | Lists orphans and the bytes they'd free, or with `--yes` deletes their versions and any file nothing else names |
| `push`, `pull` | Sends `files/` to your Tensorhub account, or fetches what's missing here. Commit `project.yaml` and `project.lock`, not `files/` |

## Pins

Approve and pin are the same thing, so reading the file shows what's constant.

- **The line:** `approve` writes `video: <hash>` (`image:` for a picture) as that one line, keeping comments, order and formatting, and a save made just before it stands. A line holding an input keeps it, `video: <hash>  # was <clash>`, and `unpin` puts it back.
- **A shot's pin:** `video: <hash>` brings that take's whole set: latents and any scenes it wrote. A hash that matches no version is just that file.
- **Other fields while pinned:** `with` and the text stay in the file and are ignored for that asset. Delete the line and they apply again.
- **A pin freezes files, not text:** editing an approved character's `description` doesn't re-render it, but shots that read it go out of date, because H3 receives those words.
- **Pinning a written scene:** it's read and no longer written, which can put the shot that wrote it out of date.

## Orphans

An asset no longer named in the YAML keeps its versions and files. Editing the graph loses nothing as long as names stay the same. Removing or renaming a name in the file orphans its files, and only `reclaim` deletes them.

## States

An asset can hold other forms of itself: outfits, a primal form, night and day. Each state gets its own picture, drawn from the asset's.

```yaml
kyogre:
  image: 2aee8c40753d
  kind: character
  description: Kyogre, the colossal blue whale-like legendary Pokémon.
  states:
    primal:
      description: Kyogre in its primal form, with glowing blue veins.
      instructions: The same creature as the picture, in the same pose.
```

`kyogre.primal` is an ordinary asset named that way, and `<kyogre.primal>` reads it. So its versions, memo, `-n`, `render kyogre.primal` and `reclaim` work as for any asset.

- **What it takes from its parent:** `kind` and `with`, unless it writes its own, and `options`, its own over the parent's. Nothing else.
- **It always reads its parent,** mentioned or not, and the adapter gets that picture as a reference. Re-drawing the parent puts the state out of date. Adding a state changes nothing for the parent.
- **Pins:** `approve kyogre.primal 2` writes `image: <hash>` on the state's own line inside the block, and `unpin` removes it. Writing one by hand works too.
- **Removing one** from the file orphans its files, as with any name.
- **States don't nest.** A `states:` inside a state warns and is ignored.

## Scenes and writes

A write is a screenshot taken at `at` seconds into that beat, the beat's end if left out, saved as a scene asset. A beat may hold several. It exists to carry the world from one run into another, so it only matters across segments and shots: within a segment the model remembers.

- **Reading a scene:** a later segment or another shot gets the picture as a loose guide. H3 reads it small and adds a retention line like "use this layout of characters and background as a starting point". That wording lives in the adapter, not the YAML.
- **Numbers:** you write them, `coast.1`, `coast.2`, so inserting one never moves another's versions or pins.
- **A write is `{scene, at}`, and has no description.** A picture is the state. To add words, declare the scene as its own asset with a `description`, and the write fills in its picture. Any other field warns.
- **Past its beat,** `at` clamps with a warning. Two assets writing one name: the first stands, with a warning.
- **A write the shot doesn't read after its beat** is a free frame, cut from the take here, outside the shot's memo: moving it re-renders nothing. One the shot reads, in a later beat or its `summary`, goes to the function, in its one take. A package that takes no `writes` gets its frames cut from the take, with a warning.
- **Anything can read one:** `<coast.1>` also works in a Qwen `scene`, so a storyboard can start from what really happened.

## Storyboards

A storyboard is a `scene` drawn before the video exists. Make cheap pictures, choose one, then render the shot:

```
cozy project render "clash board" -n 4
cozy project approve "clash board" 3
cozy project render clash
```

## Choosing a package, and adapters

`with:` names a package, `org/package[@release]`, never a function, or local code by its path (`./dir`, `./script.py`, `~/…`), keyed by its directory's name, so moving to the published package re-renders nothing. The order is the asset's own `with:`, then `defaults.<kind>.with`. An asset naming its own `with:` takes nothing from `defaults`. With neither, it has nothing to make, and `status` and `render` say so as a warning.

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

The adapter holds all model-specific words and picks the function, such as `generate_turbo` from `options: {turbo: true}`. So a long shot is one operation: H3 splits it into segments itself.

## Building something other than film

Nothing is film-specific except the kinds you choose. Art, then pictures, then 3D models, then a layout, with made-up packages:

```yaml
version: 2
assets:
  automaton art:
    with: sketchlab
    kind: art
    description: A bronze clockwork automaton, waist up.
  automaton:
    with: meshforge
    kind: mesh
    sheet: <automaton art>
    polygons: 40000
  layout:
    with: stagehand
    kind: arrange
    brief: A workshop at dusk, with the <automaton> at the bench.
```

There are no loops, so at hundreds of assets a tool or an LLM writes the file.

## Cuts and frames

`with: cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. A cut reads each asset's state, else its newest version, and says so. Same takes, same key, not cut again; a deleted `.mp4` or `.otio` comes back at the next render. It keeps one version, and its pinned one, being free to make again.

- **`kind: edit`:** `pieces` holds `<name>`, the whole take, or `{video: <name>, start: 12, end: 132}`, frames at its `fps` (24 by default), every in and out point exact. It writes `<name>.mp4` and `<name>.otio` at the root, the same edit as an OpenTimelineIO timeline.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every piece fills it, center-cropped, never letterboxed or stretched. A take at another rate is retimed, and `status` names it. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **`kind: frame`:** `{with: cozy, kind: frame, video: <clash>, at: 3}` is one frame of a video.
