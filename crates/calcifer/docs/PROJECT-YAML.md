# A project as YAML (format 4)

A project's state is its `project.db` (`PROJECT-DB.md`); the page and the CLI change it through Calcifer. YAML is its view: `GET /v1/projects/{id}/yaml` exports it for reading, a diff or git, and `PUT /v1/projects/{id}/yaml` reads one back, each box and field that differs written as one edit. It is never the source of truth, and a comment in it isn't kept. A folder of the files from before `project.db` (format 2, 3 or 4) is imported on its first open. The HTTP API is in `CALCIFER-API.md`, and the reasons in `tracker/design/project-format-ui.md` and `tracker/design/tools.md`.

## The idea

A project is a folder of files plus a graph that says how each file was made.

- **Asset:** a stable name holding one main file: a name, what it is, how it's made, the file you approved, and words for whatever mentions it.
- **Edge:** you don't write edges: `<name>` in any input reads that asset. The only rule is no cycles. `<name.2>` reads that one version, and is no edge; `<name.word>` an extra output of its version, and `<name:48-120>` frames 48 up to 120 of a video.
- **File:** bytes by hash, in `files/<sha256>.<ext>`.
- **Version**, or **take** if it has a video: one result of an asset's tool, recorded in `project.db`, with the words that made it where they changed from the version before.

Calcifer owns all of this. It knows no inputs and no packages by name.

## The folder

| Path | Holds |
|---|---|
| `project.db` | Everything the project says and has made: its boxes and fields, where each sits, versions and takes, what was measured of each file, what a render owes, its history and request keys. Calcifer alone writes it, one transaction per action |
| `files/` | Every file, by hash |
| `<edit_name>.mp4`, `.otio` | Each edit |
| `.runs/` | Where a run's outputs land on their way into `files/` |

An export holds what the project says, so it travels, and `files/` through the browser's upload and download controls. An unknown key warns when it's imported.

## The file

```yaml
version: 4

assets:
  style:                     # a text asset: its words go wherever it's mentioned
    text: |
      High-quality Unreal Engine graphics, realistic and detailed.

  kyogre_art:                # a dropped file: a name, a hidden hash, and words for what mentions it
    approved: 65b436d4f9a3
    description: The original art of Kyogre, with its background already removed.

  kyogre:                    # made, and approved: it holds this file, and can still show out of date
    tool: qwen-image-2/image  # Turbo unless `turbo: false`
    instructions: <style> <kyogre_art> as a realistic creature, front and back on one sheet, with no text, on a transparent background.
    aspect_ratio: "3:2"
    approved: 2aee8c40753d
    description: Kyogre, the colossal blue whale-like legendary Pokémon with huge fins and glowing red lines.

  coast: {tool: qwen-image-2/image, instructions: "A volcanic coast, no people.", turbo: false}

  clash:                     # a shot, on Turbo unless `turbo: false`
    tool: minimax-h3/generate
    lora: [{ref: hf://someone/wushu, strength: 0.8}]
    summary: <style> <kyogre> and <groudon> fight on <coast> in one continuous shot with no cuts.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - duration: 14
        description: A wide shot of <coast> shows <kyogre> in the surf and <groudon> on the rocks.
      - duration: 14
        description: <groudon> slams the ground as in <stomp:48-120>.
    first_frame: <coast.2>     # a slot takes only what's written in it

  clash_2k: {tool: minimax-h3/upscale, video: <clash>, to: 1440p}

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
| `models`, `lora` | Go on its function's first call |

Every other key is a **field** of its tool: the binder gives it to the function by its type, and it counts in the key. A tool's struct is its box. An unknown key warns at render, and an unknown section warns at reading. A pickup is never written: Pick up from here fills it for one render.

**Text assets.** `text:` with no `tool` is an asset of words alone. `<name>` of one, in any input, is replaced by its words where it stands, before a package sees it, so a function has no `style` input and nothing is prepended: a style goes only where it's mentioned. Its text may mention others, and what it mentions is read by whatever mentions it. A cycle stays plain text, with a warning. Editing it puts what reaches it out of date, and what's approved stays approved. One deleted is no change to what was made with it. It can't go by a tag of a package the project uses.

**Names** hold letters, digits, `_` and `-`, never `.`, which selects inside a box, and match in any case, with `_` and `-` alike: `<Style>` and `<sTYLE>` are one asset, and it keeps the spelling it was written with. Two names in the file that are one are refused, with both lines. A `.` is a character like any other: `<Amy.original>` is one name, with nothing above it, and a model reads it as `<Amy original>`. Anything else is refused when the file is read, in one line with the fix. `all` is what `render all` means, and warns as a name.

## Defaults

`defaults` is still read, though `upgrade` writes none. It holds `image`, `video` and `audio`, and fills what an asset with a tool doesn't say, by what its tool's interface says it makes, never by what a version is: inputs, `models` and `lora`, and `null` in the asset takes one back. Not its tool, and not what Calcifer makes itself, an edit or a frame. A style edit puts every asset it reaches out of date, a background removal too.

## First-party inputs

| Tool | Inputs |
|---|---|
| A shot, `minimax-h3/generate` | `summary`, `beats` (each `description`, `duration`), `turbo`, `soundscape`, `music`, `aspect_ratio`, `first_frame`, `last_frame`, `plate` (`video`, `use_audio`), `preset`, `resolution`; on `generate`, `turbo`, on unless it says `false`, and `steps`, only with it off |
| An upscale, `minimax-h3/upscale` | `video`, `resolution` |
| A picture, `qwen-image-2/generate_turbo`, `generate` | `instructions`, `aspect_ratio`, `megapixels`. They're one box, and its Turbo flips `tool` between them |
| An edit or a frame, `cozy/edit`, `cozy/frame` | See below |

**A shot with no `soundscape` or `music`** says `N/A` for each, which H3 reads as silence, with no warning: it's the default. Dialogue goes in a beat's text. **One beat and no `summary`** goes to the model as written, in MiniMax's own form if you like, at most 15 s here: `cozy run` takes one of any length. A package chooses its own internals, such as how many context frames H3 keeps.

## Mentions

`<name>` in any input makes that asset an input.

**By version.** `<name>` is what it stands on, approved else newest, and moves: what reads it can go out of date. `<name.2>` is version 2, whatever else happens to it, and is never out of date. `<self.2>` is a version of the asset whose text it is, which is how a box edits its own picture. A version number is never reused, so one stays what it says when others go. Neither is a dependency: nothing waits for it and it closes no cycle. `<self>` alone is refused: it would change its own input with every render. A version deleted or never made fails a render with `input_missing`, naming it, and nothing is charged; `reclaim` keeps every file a `<name.N>` names. Names match in any case, and a name can't end in `.` and digits or be `self`: one already in a file goes on working, and `<foo.2>` reads the asset called that.

- **In a shot's `summary`:** reaches every segment. **In a beat:** only the segment holding it. H3 gets confused by extra references, so mention things only where they're needed.
- **A package's own tags:** its tool declares them, `@app.tool(reserved=["d", "scenetrans", "cutoff"])`. In the text of an asset that function makes, each stays text in any case, and reaches it unchanged. A project naming something the same is warned at the name, and `<d>` is the tag.
- **What can't be a name** is text, with no warning, as `<Picture 1>` and `</d>` are.
- **A name that names nothing** fails that asset before anything runs, at no cost: `<Image-1> is no longer in the project`, with `did you mean <kyogre>?` when it's close. The failure stays until a version lands or its inputs change, what reads it waits, and everything else goes on. `status` says it ahead of a render. Until a function is read, at its first render, its assets' are held back, because they may be its tags. An approved asset runs nothing, so it doesn't fail unless you name it. An edit leaves out a clip it can't read.
- **A cycle:** a mention that closes one stays plain text, with a warning, and both still render.

**What a reader gets of what it mentions:** in a prompt, its file and `description`, under the key the mention spells (`Amy`, `Amy.2`, `page.cleaned`, `walk:48-120`), in the prompt's own `references`; in a slot, its file, with its `description` and its version's extras (`latents`) when the slot takes them and the same package made it. Whatever tool made it, a reader sees only that. What made it, its `instructions` or a shot's `summary`, is for its maker alone, and never crosses a mention.

## How rendering decides

**An asset's state** is the first of these that applies:
1. `approved`: that file, and with it the whole file set of the version it is. Naming one with a tool makes a version to look at, its state unmoved.
2. Its newest version whose key matches now.
3. A version whose key differs only by a read that no longer exists: that is no change, as nothing could make it again.
4. Nothing: it's missing or out of date.

An asset that is only a `description`, with no tool and no file, stands as words: what mentions it reads them.

**A box with no tool.** A box is its words, an optional tool, and its versions. With versions and no `tool:` it's a box of pictures: one primary (`approved`, else its newest) and a description, nothing puts it out of date, and dropping another file on the box in the browser adds the next version. Taking a tool off takes its fields with it (Undo puts them back); its versions, numbers, `approved` and `description` stay.

Putting a tool on a text asset moves its `text` to `instructions`. On a picture, the tool starts from the version shown, approved else newest: `<self.N>` in its first required picture slot, else at the start of its first prompt. A tool approves nothing.

**The key** is blind to names: eight hex digits of its tool (no release, no root namespace), `models`, `lora` and its fields after defaults, each box they mention replaced by its number in order of first mention (`<Amy.2>` is `<#0.2>`), then twelve for each of those boxes: its `description` and the files of each version read, or a text box's words. So a rename, on the page or in an imported YAML, puts nothing out of date, and a box renamed in an import keeps its versions and approval when exactly one vanished row matches it. Editing a box's description puts its readers out of date, `<Abr.2>`'s too. Inputs plus the seed never run twice: a seed in its fields, or a function with no seed, has one result per key. Putting old text back returns the old take as its state, with no run.

**Not in the key:** its own `description`; a pickup; package and cozy updates; the machine or card. An update never re-renders anything. A hit reuses the stored file, whatever another card would make.

**Seed:** Calcifer draws it when the function takes one, and records it.

## Working with a project

```sh
cozy new garden
cd garden
cozy open
cozy status
```

`cozy new [directory]` creates a project explicitly. `cozy open`, `cozy run` and `cozy status` use an existing project for the current directory.

Use the browser for asset import, creation, field edits, rendering, approval, history, undo, export and file transfers. The browser and other API clients use the project endpoints described in `CALCIFER-API.md`, and edits appear in connected views.

## Queueing

A project has one walker, and each render adds to it. The browser follows the work while it runs.

- **A render uses the project as it was when you asked, so keep editing.** Each ask keeps its export in `project.db` until it is done, so a restart resumes it, and an edit, `approve` or `unpin` touches nothing of it. Each plans on its own text, and independent work goes at once across all of them. What you edit shows out of date in `status` until a render makes it, and when you delete an asset, what a render still owes of it stops: queued, it isn't made and its row says so; running, it's canceled so billing stops, and lands nothing. Putting it back brings back the asset, not a canceled render.
- **Placement:** each render's work goes to its selected machines. With several machines, a job goes to the one where the plan has it ending soonest, once that has its GPUs free.
- **Everything at once, up to 32 jobs per render going:** what doesn't read each other goes side by side, each sent, installed and made on its own. What fails stops what reads it, there and then, and nothing else. How wide a job goes, how many GPUs of its machine, comes from how long things took there: it never changes once a job has started.
- **Ahead of its turn,** a machine fetches the code and weights of what's queued for it, behind whatever a run is waiting on: its `tool`'s function, and for one that loads nothing itself, only what all its package's internal functions load. It's a guess, so a wrong one costs bytes, and it stops when its last job's run begins. It holds no GPU, doesn't show in `cozy run ls`, and a machine with no room for it stays as it is, saying so in one line. `status` and the render's view show it; Cancel All stops it.
- **Rendering only missing versions** doesn't make again what another render is making with the same inputs. A plain render makes its own version.

## An asset's files

An asset has one main file; any other files are what the same run left, such as a video's latents, or `image-2` from a function that returned several. **What a result declares `Latents` stays on the machine that made it**: the project records the hash and that machine, and a pickup or an upscale reads it there by hash, or fetches it while it lives; once that machine is gone, the box warns before Render and the call goes without it. Any other file comes here.

`approved` freezes files, not text: editing an approved asset's inputs shows it out of date, and `description` changes what reads it.

## Import

Dropping a picture, video or sound onto the browser canvas is one act: the file in `files/`, a box under its name, its hash never handled. With no name it takes the file's, lower-cased: `Kyogre Art (1).png` is `kyogre_art_1`. A name cozy picks gets `_2` if the project has it. One you typed that the project has is refused, naming what has it. The same file twice is two assets and one file.

**A take lifted out** is a plain asset, a file and its words, with no `tool` and no link to what made it. The project import API accepts an existing file hash with its new name and description.

## Orphans

An asset no longer named in the project keeps its versions and files until `reclaim`. Deleted from the page or through the API, or gone from an imported YAML once a render has seen that, its name is free: a later asset of that name starts with no versions, and the browser's Rename or Undo still finds the old ones. What it made stands.

## Discarded takes

Discarding a take takes it off its asset at once. Nothing runs and no bytes go: it stays on record, discarded, until `reclaim`, and undo brings it back (Undo and history in the browser). Its number is never given again, so a list can have gaps.

- **The approved take:** its `approved:` goes with it, and the asset stands on its newest take left. Undo brings both back, unless another take has been approved since.
- **The last take:** the asset stays, with no take, and a render makes it again like anything with no state.
- **A running take** isn't a take yet: Cancel in the browser stops it.
- **What read it** is out of date only by the usual rule, that what it reads has changed. A take that picked up from this one stands, and still says which take it picked up from.
- **`reclaim`** ends Undo and deletes the files, except a file another version names, or that a clip of an Edit's version cut.

## Storyboards

A storyboard is a picture drawn before the video exists:

In the browser, render four versions of `clash_board`, approve version 3, then render `clash`.

## Functions

A box is a call of its tool, by one binder, the same for the browser's Render and `cozy run`. A tool is a function its package declares with `@app.tool`, taking one struct: its fields are the box (`tracker/design/tools.md`). Each written field goes by its type:

- **Words** (`str`): as written, each text box in place as its words; a box with a file it mentions goes as its name, with a warning.
- **A prompt** (`Prompt`, or `Prompt[Image]`): its text as written, each mention spelled as its box spells itself, with the boxes it mentions as its own references, in the order first mentioned. What it doesn't take (a video in a `Prompt[Image]`) goes as its name, with a warning.
- **A slot** (`ImageFile`, `VideoFile`, `AudioFile`, or `Image`, `Video`, `Audio` with the description): only what's written in it, `<coast.2>`, or a hash of a project file. One of the wrong kind is left empty for that render, with a warning.
- **A range** of a video, `<take3:48-336>`, is cut here, frame-accurate, with cozy's ffmpeg, and sent as that clip.
- **A group** (a struct of these, or a list of them with `Count`): its members the same way, as deep as it nests.
- **A number, choice, shape or list** past what it takes is clamped to it, with a warning. A required field left empty, or one of a type this cozy can't fill, fails the box before anything is sent, free, naming the fix.
- **A pickup** (`Pickup`) is filled only by Pick up from here: `{take, out_frame, …}`, on the frames its `Snap` allows.

`@app.tool(label=, named=, reserved=, ui=)` says the rest: its name in the tool list, what a box of it is called (`shot_1`), its model's own tags, and a component that draws it. Markers on a field (`Range`, `Count`, `Total`, `Ratios`, `Snap`, `Spans`, `Label`, `Placeholder`, `Advanced`) tell the box how to draw it.

## Edits and frames

The package `cozy` is reserved: Calcifer runs it itself, free, with no machine, once what it reads settles and on `approve`. An edit reads each asset's state, else its newest version, and says so. The same edit isn't rendered again: it says `(unchanged)`, and a deleted `.mp4` or `.otio` comes back at the next render. Each different edit is a version, holding its film, its timeline and the clips that made it.

- **`cozy/edit`:** `tracks: [{clips}]`, one track for now, and a clip's place in the list is its place in the film. A clip is `<name>`, all of it; `{video: <name>, in: 12, out: 132}`, frames at its `fps` (24 by default) counted from 0, `out` exclusive; or `{image: <name>, frames: 48}`, 3 s with no `frames`. On the wire these are `timeline: {tracks: [{clips: [{clip: "<name>", in, out, frames}]}]}`. Sound comes with each clip. It writes `<name>.mp4` and `<name>.otio` at the root. A clip that can't be had, has no version yet, or names nothing is left out with a warning; the rest is cut, and the next render cuts it whole. A clip here that ffprobe can't read fails the edit, and the film that stands stays.
- **The canvas:** `aspect_ratio` and `resolution: 1440p` on it. Every clip fills it, center-cropped, a still's transparency going to black. A take at another rate is retimed. With no `resolution`, it's the first take's own size when that is the aspect but for a model's rounding, so such takes join with no encode.
- **`cozy/frame`:** `{tool: cozy/frame, video: <clash>, at: 3}` is one frame of a video.

## Runs

`cozy run` uses the existing project for the current directory. Create a project explicitly with `cozy new [directory]` before running it. `cozy open` opens that project for asset editing in the browser.

- **Outputs** land in `files/` by hash, listed as the run's, and the run prints one line per file: `image: 65b436d4f9a3`. There's no memo.
- **Use one:** `--file image=65b436d4f9a3` names a file by its hash, and approving that file for `kyogre` in the browser makes it what `kyogre` holds.
- **`reclaim`** lists a run's files once nothing names them.

## Upgrading from format 2

A folder of format 2 upgrades itself the first time Calcifer opens it, and says so once, before its files are imported into `project.db`. The old files stay beside as `project.yaml.v2` and `project.lock.v2`. Each asset's state is worked out from the old files as the cozy that reads format 2 did, so what was current stays current and what was out of date stays so. What can't convert is refused, says why and stays as it is: runs on their way, or two names that fold to one. A run that lands in it keeps its output in `files/`, unlisted, and its warning says why. Format 1 isn't moved.

- Each asset's `render` goes flat, `prompt` as `instructions`, and `with` as `tool`, always a function: H3 is `generate`, with `turbo: false` only where a file said so or where it set `steps`; any other package's `turbo: true` is `generate_turbo`, `turbo: false` `generate`, and with neither what made its newest version. A video with only `video` and `resolution` is `upscale`. `with: cozy` is `cozy/edit`, or `cozy/frame` for a picture. `pieces` become `tracks`, `start` `in` and `end` `out`. A hash in `image`, `video` or `audio` is `approved`, and an input under `# was` is back. `kind` lines go. Comments stay.
- Each entity folds into its sheet, which takes its name, `description` and `asset_type`. Each form becomes an asset, `<entity>.<form>`, and mentions are rewritten.
- A description that was in an asset's prompt, and now goes only to what mentions it, is said by name.
- `defaults` go. A style becomes the text asset `style`, and a second, `video_style`, where the video's differs: `<style>` and the rest of its words, if it extends the first. Each prompt mentions the one it took: in Qwen's `instructions` as `Style: <style>.`, at the front of H3's `summary`, which it starts if there's none, and as `style: <style>` in any other package. A shared `soundscape` becomes a text asset too, mentioned in each shot's. `music: N/A` goes, as an empty field says the same. `aspect_ratio` and the rest go into each asset that took them, and Qwen's `transparent: true` into the words it stood for.
- Names that fold to one are said, by name, and nothing is written: rename one, and upgrade again.
- A still a shot wrote, and that something mentions, becomes a file asset. A mention no version wrote and nothing makes is left as it is, and said.
- A mention whose meaning changes is said, with its asset and lines. A beat writes nothing in format 3, so a mention of what one wrote now names nothing, or a version: `<coast.1>` reads as version 1 of `coast`.
- **The new key is stamped on each asset's current version**, so nothing goes out of date and nothing runs. Other versions keep their old keys: putting old text back won't find them, and what was out of date stays so.

**Format 3 to 4** (`tracker/design/tools.md`) goes on from there, or alone on a format 3 folder, keeping each box's state as format 3 read it: the new name-blind key is stamped on exactly what stood, so nothing goes out of date and nothing out of date comes back. `qwen-image-2/generate` and `generate_turbo` become `qwen-image-2/image` with `turbo`; a shot with a `plate` becomes `minimax-h3/edit`, its `lora` its `edit` choice and its typed trigger word off its beats; a written `continue_from` goes; a shot's `resolution` and `upscale` go, an upscale box's `resolution` is `to`, and `upscale.model: seedvr2` a `seedvr2/upscale` box; `kind`, `asset_type` and `voice` lines go, with one warning per project naming the boxes; a dotted name becomes `_`, its mentions, lock row and canvas row with it. The old files stay as `project.yaml.v3` and `project.lock.v3`.
