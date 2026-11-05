# How `project.yaml` works (format 3)

The page is the way in, and this file is what it writes. A file of format 2 is refused, naming `cozy project upgrade`; format 1 is left as it is. The HTTP API is in `CALCIFER-API.md`, and the reasons in `tracker/design/project-format-ui.md`.

## The idea

A project is a folder of files plus a graph that says how each file was made.

- **Asset:** a stable name holding one main file: a name, what it is, how it's made, the file you approved, and words for whatever mentions it.
- **Edge:** you don't write edges: `<name>` in any input reads that asset. The only rule is no cycles.
- **File:** bytes by hash, in `files/<sha256>.<ext>`.
- **Version**, or **take** if it has a video: one result of an asset's tool, recorded in `project.lock`.

Calcifer owns all of this. It knows no inputs and no packages by name.

## The folder

| Path | Written by | Holds |
|---|---|---|
| `project.yaml` | The page and the CLI, line by line; or a hand or an LLM | What should exist |
| `project.lock` | Cozy only | YAML: each asset's versions, and a row for a file only when its file can't say it: where it stays, its parts. Replaced whole, and read again only when it changed. One that can't be read is kept as `project.lock.broken` and said, never written over: what's approved stands |
| `project.canvas.yaml` | Cozy, for the page | Where each box sits. Nothing else reads it, and nothing in it is an input |
| `files/` | Cozy only | Every file, by hash |
| `<edit_name>.mp4`, `.otio` | Cozy | Each edit |
| `.runs/` | Cozy only | Where a run's outputs land on their way into `files/`, and while a render goes what its walk owes (`walk.json`, `asked/`), of this folder and this Calcifer only: a copy goes on with none of it, and `push` and `pull` don't carry it |
| `.measured.json` | Cozy only | What was measured of each file. Disposable: it's measured again |

**A machine write changes only its own lines.** Comments, `|` blocks and everything else in the file stay as a hand left them. An unknown key warns when the file is read.

## The file

```yaml
version: 3

assets:
  style:                     # a text asset: its words go wherever it's mentioned
    text: |
      High-quality Unreal Engine graphics, realistic and detailed.

  kyogre_art:                # a dropped file: a name, a hidden hash, and words for what mentions it
    approved: 65b436d4f9a3
    description: The original art of Kyogre, with its background already removed.

  kyogre_voice: {approved: 7c1e04a9b2d5}

  kyogre:                    # made, and approved: it holds this file, and can still show out of date
    tool: qwen-image-2/generate_turbo
    instructions: <style> <kyogre_art> as a realistic creature, front and back on one sheet, with no text, on a transparent background.
    aspect_ratio: "3:2"
    approved: 2aee8c40753d
    description: Kyogre, the colossal blue whale-like legendary Pokémon with huge fins and glowing red lines.
    asset_type: character
    voice: <kyogre_voice>

  coast: {tool: qwen-image-2/generate, instructions: "A volcanic coast, no people.", asset_type: environment}

  clash:                     # a shot, on Turbo unless `turbo: false`
    tool: minimax-h3/generate
    lora: [{ref: hf://someone/wushu, strength: 0.8}]
    summary: <style> <kyogre> and <groudon> fight on <coast> in one continuous shot with no cuts.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - duration: 14
        description: A wide shot of <coast> shows <kyogre> in the surf and <groudon> on the rocks.
      - duration: 14
        description: The fight continues. <groudon> slams his foot into the ground.

  clash_2k: {tool: minimax-h3/upscale, video: <clash>, resolution: 1440p}

  edit:
    tool: cozy/edit          # free, no machine
    fps: 24
    tracks:
      - clips:
          - <awaken>                             # all of it
          - {video: <erupt>, in: 12, out: 132}   # frames 12 to 132 of erupt
          - {image: <title_card>, frames: 48}    # a picture held for 2 s
```

## What the platform reads

| Key | Is |
|---|---|
| `tool` | A function, in full, in `cozy run`'s grammar: `org/package/function[@release]`, `package/function`, or local code by its path, `./dir/function`, keyed by its directory's name, so moving to the published package re-renders nothing. No `@release` uses what its machine has installed. With none, it's a dropped file, and what it is comes from the file |
| `approved` | The file it holds, by a prefix of its hash. A choice, not a claim: out of date comes from the lock, so an upstream change never edits the file. A number-shaped hash is its text |
| `description` | What it is. It reaches whatever mentions it, and never its own tool. A `<name>` in it stays text, with a warning |
| `asset_type` | `character`, `prop` or `environment`, for what a reader does with it. A package reads it: H3 keeps a character or prop's look at full size and takes an environment as the background, and a plain picture as a loose layout at 512 px |
| `voice` | `<name>`, another asset. Readers get its main file as `files.voice`. It isn't part of making this asset |
| `models`, `lora` | Go on its function's first call |

Every other key is an **input**: it passes through to the function and counts in the key. A function's adapter types them, and that type is its controls. An unknown key warns at render, and an unknown section warns at reading. A `kind` line warns and is ignored: what an asset is comes from its tool, its file or its version, and `GET /v1/projects/{id}` says it.

**Text assets.** `text:` with no `tool` is an asset of words alone. `<name>` of one, in any input, is replaced by its words where it stands, before a package sees it, so a function has no `style` input and nothing is prepended: a style goes only where it's mentioned. Its text may mention others, and what it mentions is read by whatever mentions it. A cycle stays plain text, with a warning. Editing it puts what reaches it out of date, and what's approved stays approved. One deleted is no change to what was made with it. It can't go by a tag of a package the project uses.

**Names** hold letters, digits, `_`, `-` and `.`, and match in any case, with `_` and `-` alike: `<Style>` and `<sTYLE>` are one asset, and it keeps the spelling it was written with. Two names in the file that are one are refused, with both lines. A `.` is a character like any other: `<Amy.original>` is one name, with nothing above it, and a model reads it as `<Amy original>`. Anything else is refused when the file is read, in one line with the fix. `all` is what `render all` means, and warns as a name.

## Defaults

`defaults` is still read, though `upgrade` writes none. It holds `image`, `video` and `audio`, and fills what an asset with a tool doesn't say, by what its tool's interface says it makes, never by what a version is: inputs, `models` and `lora`, and `null` in the asset takes one back. Not its tool, and not what Calcifer makes itself, an edit or a frame. A style edit puts every asset it reaches out of date, a background removal too.

## First-party inputs

| Tool | Inputs |
|---|---|
| A shot, `minimax-h3/generate`, `generate_turbo` | `summary`, `beats` (each `description`, `duration`), `soundscape`, `music`, `aspect_ratio`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`), `preset`, `resolution`; on `generate`, `turbo`, on unless it says `false`, and `steps`, only with it off |
| An upscale, `minimax-h3/upscale` | `video`, `resolution` |
| A picture, `qwen-image-2/generate_turbo`, `generate` | `instructions`, `aspect_ratio`, `megapixels`, `preset`; `steps` on `generate` |
| An edit or a frame, `cozy/edit`, `cozy/frame` | See below |

**A shot with no `soundscape`** goes to H3 as `N/A`, which it reads as silence. A package chooses its own internals, such as how many context frames H3 keeps.

## Mentions

`<name>` in any input makes that asset an input.

- **In a shot's `summary`:** reaches every segment. **In a beat:** only the segment holding it. H3 gets confused by extra references, so mention things only where they're needed.
- **A package's own tags:** an adapter declares them, `@app.project_adapter("generate", reserved=["d", "scenetrans", "cutoff"])`, written out, as cozy reads it from the source. In the text of an asset that function makes, each stays text in any case, and reaches the adapter unchanged. A project naming something the same is warned at the name, and `<d>` is the tag.
- **What can't be a name** is text, with no warning, as `<Picture 1>` and `</d>` are.
- **A name that names nothing** fails that asset before anything runs, at no cost: `<Image-1> is no longer in the project`, with `did you mean <kyogre>?` when it's close. The failure stays until a version lands or its inputs change, what reads it waits, and everything else goes on. `status` says it ahead of a render. Until a function is read, at its first render, its assets' are held back, because they may be its tags. An approved asset runs nothing, so it doesn't fail unless you name it. An edit leaves out a clip it can't read.
- **A cycle:** a mention that closes one stays plain text, with a warning, and both still render.

**What a reader gets of what it mentions:** `name`, `kind`, `asset_type`, `description`, `files` by role (`image`, `video` or `audio`, `latents`, `voice`), `made_by`, and `inputs` holding only `use_audio` and `follow`. What made it, its `instructions` or a shot's `summary`, is for its maker alone, and never crosses a mention.

## How rendering decides

**An asset's state** is the first of these that applies:
1. `approved`: that file, and with it the whole file set of the version it is. Naming one with a tool makes a version to look at, its state unmoved.
2. Its newest version whose key matches now.
3. A version whose key differs only by a read that no longer exists: that is no change, as nothing could make it again.
4. Nothing: it's missing or out of date.

An asset that is only a `description`, with no tool and no file, stands as words: what mentions it reads them.

**The key** is eight hex digits of its tool (no release, no root namespace), `models`, `lora` and its inputs after defaults, then twelve for each read: four of its name, and eight of its `asset_type`, `description` and `files`, and twelve for each text asset it takes in, so one gone hides no change in the others. Its inputs are as written, `<style>` and all. So editing an approved asset's `instructions` leaves what reads it as it is. Inputs plus the seed never run twice: a seed in its inputs, or a function with no seed, has one result per key. Putting old text back returns the old take as its state, with no run.

**Not in the key:** its own `description`, `asset_type` and `voice`; package, adapter and cozy updates; the machine or card. An update never re-renders anything. A hit reuses the stored file, whatever another card would make.

**Seed:** Calcifer draws it when the function takes one, and records it.

## Commands

| Command | Does |
|---|---|
| `render all [--on <machine>]` | A new version, with a new seed, of every asset with a tool, each reading the new ones before it. Approved assets are skipped, and said. One of each: `-n` is for an asset you name, and with `all` it warns and makes one |
| `render <name>`, `-n 4` | A new version of it, or four, and of what it reads only if that has no state. Naming an approved asset makes a version to look at, and what it holds stays |
| `render <names\|all> --missing` | Only what has no state: continues a render that half failed without paying twice |
| `render all --outdated` | As `render all`, and each approved asset out of date when its turn comes. What it approved goes when the new version lands, and the version it held stays, to approve again. If it can't be made, it stands |
| `render <names\|all> --queue` | Asks and returns at once, printing what it queued and what's ahead of each |
| `cancel <names\|all>` | Drops their queued work and cancels their runs |
| `render clash --take 2 --in 15` | Keeps take 2 up to 15 s and regenerates the rest |
| `approve <name> <version>` | Writes that version's hash as its `approved` |
| `approve <name> <hash>` | The same with any file in `files/`, whatever made it |
| `unapprove <name>` | Takes `approved` away. A dropped file keeps it: it's all it is |
| `status` | Each asset's state and why, orphans, and how many files `cozy run` left here |
| `import <file\|hash> [name] [--description <words>]` | Keeps a file, URL or a hash already in `files/`, adds `{approved}` under its name, and prints the name. No machine needed. Removing a background is a tool's: drop the file, drop the tool, approve |
| `rename <old> <new>` | Renames an asset in `project.yaml` and in every `<mention>`, and keeps its versions. If you renamed it there already, this only moves its versions |
| `reclaim [--yes]` | Lists orphans and the bytes they'd free, or with `--yes` deletes their versions and any file a version or run has on record and nothing names now. A file with no record is listed and stays: it may be all that's left of a take. With `project.lock` gone, unreadable or listing nothing, `--yes` is refused |
| `push`, `pull` | Sends `files/` to your Tensorhub account, or fetches what's missing here. Commit `project.yaml` and `project.lock`, not `files/` |
| `add <tool\|text> [name] [field=value…]` | An asset of that tool, as dropping it on the canvas, or with `text` a text asset, and prints its name. A tool is a function: `cozy package ls`, `cozy run <package>`, `cozy run <tool> --describe`. With no `at=` the page gives it a free place on screen |
| `set <name> field=value… [--input <file>]` | Its fields, as its box's controls. A number, `true` or `false` is that, `:=` is JSON, `field=` clears it, `tool=` is what makes it, and `at=x,y[,w]` is where it sits. An Edit's tracks and clips are its `timeline`, in a file |
| `status <name>` | One asset: its fields and each take or version |
| `delete <name>…` | As the X on its box: its work stops and it goes, and this prints the `restore` line that brings it back |
| `restore <name> <kept>` | That line: back as it was, where it was, with what it made |
| `upgrade [--status <file>]` | A format 2 folder to format 3, here, with no Calcifer. See below |

Everything the page does has a command, each the same route with the same fields, and what either does shows on the other within a second: a hand's save of `project.yaml` too. Caption is `cozy run` of a function that reads a picture, then `set <name> description=…`.

## Queueing

A project has one walker, and each render adds to it. Without `--queue` a render follows until it ends; Ctrl-C only stops following.

- **A render uses the file as it was when you asked, so keep editing.** Each ask keeps that text in `.runs/` until it is done, so a restart resumes it, and an edit, `approve` or `unpin` touches nothing of it. Each plans on its own text, and independent work goes at once across all of them. What you edit shows out of date in `status` until a render makes it, and when you delete an asset, what a render still owes of it stops: queued, it isn't made and its row says so; running, it's canceled so billing stops, and lands nothing. Putting it back brings back the asset, not a canceled render.
- **A save that can't be read** refuses a new `render`, and does nothing else. `status` says what's wrong, and `cancel` works.
- **`--on`:** each render's work goes where it said. With several machines, a job goes to the one where the plan has it ending soonest, once that has its GPUs free.
- **Everything at once, up to 32 jobs per render going:** what doesn't read each other goes side by side, each sent, installed and made on its own. What fails stops what reads it, there and then, and nothing else. How wide a job goes, how many GPUs of its machine, comes from how long things took there: it never changes once a job has started.
- **Ahead of its turn,** a machine fetches the code and weights of what's queued for it, behind whatever a run is waiting on: its `tool`'s function, and for one that loads nothing itself, what its package's internal functions load, what they share first. It's a guess, so a wrong one costs bytes, and it stops when its last job's run begins. It holds no GPU, doesn't show in `cozy run ls`, and a machine with no room for it stays as it is, saying so in one line. `status` and the render's view show it; `cancel all` stops it.
- **`--missing`** doesn't make again what another render is making with the same inputs. A plain render makes its own version.

## An asset's files

An asset has one main file; any other files are what the same run left, such as a video's latents, or `image-2` from a function that returned several. **Untyped outputs, such as latents, stay on the machine that made them** when the run made anything else: `project.lock` records the hash and that machine. `--take`, `--in` and an upscale read them there by hash, or fetch them while it lives; once it's gone `--take` and `--in` refuse, and an upscale encodes the video.

`approved` freezes files, not text: editing an approved asset's inputs shows it out of date, and `description` changes what reads it.

## Import

`cozy project import ./kyogre.png [name]` is one act: the file in `files/`, its name in `project.yaml`, its hash never handled. With no name it takes the file's, lower-cased: `Kyogre Art (1).png` is `kyogre_art_1`. A name cozy picks gets `_2` if the project has it. One you typed that the project has is refused, naming what has it. The same file twice is two assets and one file.

**A take lifted out** is the normal way to keep one: `cozy project import 2aee8c40753d amy.smiling --description "Amy, smiling."` is a plain asset, a file and its words, with no `tool` and no link to what made it.

## Orphans

An asset no longer named in the file keeps its versions and files until `reclaim`. Deleted from the page or the CLI, or gone from the file once a render has seen that, its name is free: a later asset of that name starts with no versions, and `cozy project rename` or Undo still finds the old ones. What it made stands.

## Storyboards

A storyboard is a picture drawn before the video exists:

```
cozy project render clash_board -n 4
cozy project approve clash_board 3
cozy project render clash
```

## Adapters

**A function no adapter speaks for** is called directly with its inputs, each `<mention>` in a file input the file it names (`CALCIFER-API.md`). **An adapter** translates:

```python
class TurboOperation(Operation, kw_only=True):
    inputs: TurboInputs                          # its controls

@app.project_adapter("generate_turbo", reserved=["d"])       # project-adapter-generate_turbo
def adapt(op: TurboOperation) -> list[Invocation]: ...
```

For each asset that needs a run, Calcifer:
1. Builds an `Operation`: `{version: 3, asset, kind, function, inputs, reads, keep}`.
2. Stops there if the memo already has a result.
3. Installs the package on the render machine and runs its adapter as a small CPU run. Adapters are pure, so they only take text, names and hashes.
4. Runs the invocations it returns, in order, adding the seed, and on the first its `models` and `lora`. An empty list means no run: the asset's files are what it reads. In `input`, a string that is exactly `$0.video` means that field of invocation 0's result. A failed one stops the list and makes no version.
5. Records all the resulting files as one new version, by output name, later over earlier.

## Edits and frames

The package `cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. An edit reads each asset's state, else its newest version, and says so. The same edit isn't rendered again: it says `(unchanged)`, and a deleted `.mp4` or `.otio` comes back at the next render. Each different edit is a version, holding its film, its timeline and the clips that made it.

- **`cozy/edit`:** `tracks: [{clips}]`, one track for now, and a clip's place in the list is its place in the film. A clip is `<name>`, all of it; `{video: <name>, in: 12, out: 132}`, frames at its `fps` (24 by default) counted from 0, `out` exclusive; or `{image: <name>, frames: 48}`, 3 s with no `frames`. On the wire these are `timeline: {tracks: [{clips: [{clip: "<name>", in, out, frames}]}]}`. Sound comes with each clip. It writes `<name>.mp4` and `<name>.otio` at the root. A clip that can't be had, has no version yet, or names nothing is left out with a warning; the rest is cut, and the next render cuts it whole. A clip here that ffprobe can't read fails the edit, and the film that stands stays.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every clip fills it, center-cropped, a still's transparency going to black. A take at another rate is retimed. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **`cozy/frame`:** `{tool: cozy/frame, video: <clash>, at: 3}` is one frame of a video.

## Runs

`cozy run` belongs to a project, as a commit does to a repository: the folder you're in or the nearest above it with a `project.yaml`, or `-p <path>`. Elsewhere it refuses.

- **Outputs** land in `files/` by hash, listed in `project.lock` under `runs`, and the run prints one line per file: `image: 65b436d4f9a3`. There's no memo.
- **Use one:** `--file image=65b436d4f9a3` names a file by its hash, and `cozy project approve kyogre 65b436d4f9a3` makes it what `kyogre` holds.
- **`reclaim`** lists a run's files once nothing names them.

## Upgrading from format 2

```
cozy project status --json > status.json      # with the cozy that reads format 2
cozy project upgrade --status status.json     # with this one
```

It runs here with no Calcifer, and the old files stay beside the new as `project.yaml.v2` and `project.lock.v2`. It refuses while runs are on their way. With no `--status`, each asset's approved version, else its newest, is taken as current.

- Each asset's `render` goes flat, `prompt` as `instructions`, and `with` as `tool`, always a function: H3 is `generate`, with `turbo: false` only where a file said so or where it set `steps`; any other package's `turbo: true` is `generate_turbo`, `turbo: false` `generate`, and with neither what made its newest version. A video with only `video` and `resolution` is `upscale`. `with: cozy` is `cozy/edit`, or `cozy/frame` for a picture. `pieces` become `tracks`, `start` `in` and `end` `out`. A hash in `image`, `video` or `audio` is `approved`, and an input under `# was` is back. `kind` lines go. Comments stay.
- Each entity folds into its sheet, which takes its name, `description` and `asset_type`. Each form becomes an asset, `<entity>.<form>`, and mentions are rewritten.
- A description that was in an asset's prompt, and now goes only to what mentions it, is said by name.
- `defaults` go. A style becomes the text asset `style`, and a second, `video_style`, where the video's differs: `<style>` and the rest of its words, if it extends the first. Each prompt mentions the one it took: in Qwen's `instructions` as `Style: <style>.`, at the front of H3's `summary`, which it starts if there's none, and as `style: <style>` in any other package. A shared `soundscape` becomes a text asset too, mentioned in each shot's. `music: N/A` goes, as an empty field says the same. `aspect_ratio` and the rest go into each asset that took them, and Qwen's `transparent: true` into the words it stood for.
- Names that fold to one are said, by name, and nothing is written: rename one, and upgrade again.
- A still a shot wrote, and that something mentions, becomes a file asset. A mention no version wrote and nothing makes is left as it is, and said.
- **The new key is stamped on each asset's current version**, so nothing goes out of date and nothing runs. Other versions keep their old keys: putting old text back won't find them, and what was out of date stays so.
