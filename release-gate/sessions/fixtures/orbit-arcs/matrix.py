"""se-152's arcs matrix (arcs.json): each render a question, its caption written out. `python3 matrix.py > arcs.json`.

Pictures (pictures.json, minimax-h3-image): m-panel (a manga swordswoman on a street, 3:4) and its edits m-right (90 degrees
clockwise round her), m-back (180), m-top (70 degrees above), m-face (a close-up); w-girl (anime, on white, 9:16); s-classroom
(an empty room, 16:9). w-grey is w-girl's white made ORB360's grey here."""

from __future__ import annotations

import json

FPS = 24
ORB = "matlowai/orb360@0.1.1"
T = "orbit/probe"
FRAMES = {5: 124, 8: 192, 15: 362}
TAIL = "\n\noverall_soundscape:\nN/A\n\nnon_diegetic_music:\nN/A"
FROZEN = ("The entire subject is a single frozen three-dimensional scene throughout the shot. Every visible eyelid keeps its "
          "reference position: no blinking. Each eye keeps a fixed gaze relative to the head and never tracks the moving camera. "
          "Facial expression, mouth, head, hands, feet and body pose remain unchanged. Hair strands, clothing, ribbons, accessories "
          "and loose parts are rigidly fixed in the same positions. There is no breathing, sway, settling or secondary motion. Only "
          "the camera changes position. Camera radius, focal length and look-at point stay fixed. Lighting stays fixed in world "
          "space; normal viewpoint-dependent shading and reflections remain physically consistent.")  # fmt: skip
NO = "No articulation, deformation, secondary motion, wind, zoom, dolly, camera roll, cuts, lighting changes, or scene changes."
KEPT = ("fully_preserved - her geometry, proportions, materials, colors, markings and surface details are retained, and she stays "
        "completely frozen.")


def at(frame: int) -> str:
    return f"At {frame / FPS:.6f} seconds (zero-based frame {frame})"


def turns(seconds: int, points: list[tuple[float, str]], what: str = "azimuth") -> str:
    """Milestones: (fraction of the clip, the words after the time), as ORB360's captions give them."""
    last = FRAMES[seconds] - 1
    lines = [f"{at(round(last * f))}, {words}" for f, words in points]
    head = (f"All {len(points)} milestones below belong to the same uninterrupted [Shot 1], not separate shots. The video runs at 24 "
            f"fps and has {last + 1} frames. ")  # fmt: skip
    if what == "azimuth":
        head += ("Angles describe camera motion clockwise as viewed from above, relative to the starting camera azimuth; the subject "
                 "itself does not rotate. The camera moves continuously at constant angular speed between these milestones, without "
                 "pausing or reversing.")  # fmt: skip
    else:
        head += ("Elevations are the camera's angle above (+) or below (-) the subject's centre, along a single vertical arc facing the "
                 "same side of the subject; the subject itself does not rotate. The camera moves continuously at constant angular speed "
                 "between these milestones, without pausing or reversing.")  # fmt: skip
    return head + "\n" + "\n".join(lines)


def deg(angle: int, note: str = "") -> str:
    return f"the camera has advanced {angle} degrees clockwise from its starting azimuth{note}."


def doc(defs: list[str], summary: str, keep: list[str], shot: str, marks: str) -> str:
    return "\n".join(["subject_definitions:", *defs, "", "summary:", summary, "", "retention_analysis:", *keep, "",
                      "detailed_description:", shot, "", marks, "", FROZEN]) + TAIL  # fmt: skip


SWORD = "<swordswoman> is the exact same rigid character shown in <Picture 1>. Her geometry, proportions, materials, colors, markings and surface details remain unchanged."
STREET = ("The surrounding street stays in place in the world and is revealed consistently as the camera moves, continuing the "
          "same location seen in <Picture 1>, drawn in the same black-and-white ink and screentone manga style.")


def quarter(name: str, subject_line: str, scene: str, words: str, seconds: int = 5) -> str:
    """A 90-degree clockwise arc from <Picture 1>, its first frame."""
    return doc(
        [subject_line, "<Picture 1> is the first-frame viewpoint of [Shot 1]."],
        f"ORB360_CW. A frozen-moment 90-degree orbit arc around {name}, like a single instant stopped in time: the camera travels a "
        "quarter of the way around and stops.",
        [f"{name} (appears in [Shot 1]): {KEPT}",
         "<Picture 1> ([Shot 1] first frame): fully_preserved - the arc starts on this view, including its environment and light."],
        f"[Shot 1] The shot begins from <Picture 1>, with its exact environment, background and light. The camera performs exactly one "
        f"smooth clockwise 90-degree arc, a quarter of an orbit, around {words}, at constant radius, constant elevation, constant "
        f"focal length, constant speed and constant framing, and stops 90 degrees clockwise from where it began. {scene} {NO}",
        turns(seconds, [(0, deg(0, "; the viewpoint corresponds to <Picture 1>")), (0.5, deg(45)),
                        (1, deg(90, "; it stops there"))]),
    )  # fmt: skip


def full(name: str, subject_line: str, summary_where: str, first: str, scene: str, words: str, pinned: bool = True,
         extra_defs: list[str] = (), extra_keep: list[str] = (), views: dict[int, str] = {}, seconds: int = 15) -> str:  # noqa: B006
    """One clockwise 360-degree turn in 15 s; `views` puts a picture at a milestone."""
    defs = [subject_line, *extra_defs]
    if pinned:
        defs.append("<Picture 1> is the first-frame viewpoint of [Shot 1].")
        keep0 = "<Picture 1> ([Shot 1] first frame and last frame): fully_preserved - the orbit starts and ends on this view."
    else:
        keep0 = ("<Picture 1> (not a frame of [Shot 1]): fully_preserved - its character's appearance, outfit and pose define "
                 f"{name}; its plain white background is not part of the scene.")  # fmt: skip
    marks = [(0.0, deg(0, f"; {'the viewpoint corresponds to <Picture 1>' if pinned else f'it faces the same side of {name} as <Picture 1>'}"))]
    for k, angle in ((1, 90), (2, 180), (3, 270)) if views else ((1, 120), (2, 240)):
        marks.append(((angle / 360), deg(angle, f"; it faces {views[angle]}" if angle in views else "")))
    marks.append((1.0, deg(360, "; the viewpoint corresponds to <Picture 1>" if pinned else f"; it faces the same side of {name} as at the start")))
    return doc(defs, f"ORB360_CW. A frozen-moment 360-degree orbit of {name} {summary_where}, like a single instant stopped in time.",
               [f"{name} (appears in [Shot 1]): {KEPT}", keep0, *extra_keep],
               f"[Shot 1] {first} The camera performs exactly one smooth clockwise 360-degree orbit around {words}, at constant radius, "
               f"constant elevation, constant focal length, constant speed, and constant framing, ending at the starting viewpoint. "
               f"{scene} {NO}",
               turns(seconds, marks))  # fmt: skip


def plain(name: str, kind: str, words: str) -> str:
    """orbit's caption() for a plain backdrop (`white` or `plain`), as orbit 0.4.0rc1 writes it."""
    backdrop = "plain white" if kind == "white" else "plain"
    return full(name, f"{name} is the exact same rigid character shown in <Picture 1>. Her geometry, proportions, materials, colors, "
                "markings and surface details remain unchanged.",
                f"against the {backdrop} background shown in <Picture 1>",
                f"The shot begins from <Picture 1>, with its {backdrop} background and studio light.",
                f"The background stays the same {'plain, empty white' if kind == 'white' else 'plain and empty'} all the way around, with no "
                "ground, sky, landscape or weather.", words)  # fmt: skip


def vertical(top: int, seconds: int = 5) -> str:
    rise = top > 0
    where = (f"rising from eye level up over her until it looks steeply down at her from {top} degrees above" if rise
             else f"sinking from eye level down below her until it looks up at her from {-top} degrees below")
    return doc(
        [SWORD, "<Picture 1> is the first-frame viewpoint of [Shot 1]."],
        f"A frozen-moment vertical camera arc over <swordswoman>, like a single instant stopped in time: the camera {'rises' if rise else 'sinks'} "
        "along a single vertical arc while always looking at her.",
        [f"<swordswoman> (appears in [Shot 1]): {KEPT}",
         "<Picture 1> ([Shot 1] first frame): fully_preserved - the arc starts on this eye-level view."],
        f"[Shot 1] The shot begins from <Picture 1>, at eye level. The camera moves vertically along a single meridian around the "
        f"completely stationary <swordswoman>, at constant radius and constant focal length, always looking at her, {where}, and stops "
        f"there. It stays on the same side of her: no turn around her. {STREET} {NO}",
        turns(seconds, [(0, "elevation 0 degrees (eye level); the viewpoint corresponds to <Picture 1>."),
                        (0.5, f"elevation {top // 2:+d} degrees ({'above, looking down' if rise else 'below, looking up'})."),
                        (1, f"elevation {top:+d} degrees ({'high above, looking steeply down at the top of her head' if rise else 'low below, looking up at her'}); it stops there.")],
              what="elevation"),
    )  # fmt: skip


def fl2va(words: str) -> str:
    return f"[Shot 1] {words}{TAIL}"


def two_keys() -> str:
    """b2, b3: Ref2VA with m-panel first and m-right last."""
    return doc(
        [SWORD.replace("<Picture 1>.", "<Picture 1> and <Picture 2>."),
         "<Picture 1> is the first-frame viewpoint of [Shot 1].",
         "<right> is the last-frame viewpoint of [Shot 1], shown in <Picture 2>: the same <swordswoman> seen 90 degrees clockwise round her, from her right side."],
        "ORB360_CW. A frozen-moment 90-degree orbit arc around <swordswoman>, like a single instant stopped in time: the camera travels "
        "a quarter of the way around her, from the view in <Picture 1> to the view in <Picture 2>, and stops.",
        [f"<swordswoman> (appears in [Shot 1]): {KEPT}",
         "<Picture 1> ([Shot 1] first frame): fully_preserved - the arc starts on this view.",
         "<right> ([Shot 1] last frame): fully_preserved - the arc ends on this view."],
        "[Shot 1] The shot begins from <Picture 1>. The camera performs exactly one smooth clockwise 90-degree arc, a quarter of an orbit, "
        "around the completely stationary <swordswoman>, at constant radius, constant elevation, constant focal length and constant "
        f"speed, and stops on the view in <Picture 2>. {STREET} {NO}",
        turns(5, [(0, deg(0, "; the viewpoint corresponds to <Picture 1>")), (0.5, deg(45)),
                  (1, deg(90, "; the viewpoint corresponds to <Picture 2>"))]),
    )  # fmt: skip


SCENE_ROOM = ("<classroom> is the exact same room shown in <Picture 1>: its desks, chairs, windows, blackboard, walls and light remain "
              "unchanged.")
ROOM_WORDS = ("The room stays in place in the world and is revealed consistently as the camera moves: the desks in the middle stay "
              "where they are while the walls behind them change, the windows, the blackboard, the door and the back wall in turn. "
              "Nobody is in the room. Everything is frozen in time: dust and sunbeams hang still.")
GIRL = "<girl> is the exact same rigid character shown in <Picture 1>. Her geometry, proportions, materials, colors, markings and surface details remain unchanged."

B1_TEXT = fl2va(
         "The camera performs a slow, smooth clockwise 90-degree orbit arc around the completely stationary swordswoman, moving from the "
         "first frame's viewpoint (her front) to the last frame's viewpoint (her right side, in profile), at constant angular speed, "
         "constant radius, constant elevation and constant focal length, without pausing or reversing. The swordswoman and the whole street "
         "stay frozen in time and perfectly rigid; the street behind her is revealed consistently as the camera moves. Black-and-white "
         "manga ink and screentone style throughout. No zoom, dolly, camera roll, cuts, lighting changes or scene changes.")
QUARTER_SWORD = quarter("<swordswoman>", SWORD, STREET, "the completely stationary <swordswoman>")
renders = [
    # (a) partial arcs: ORB360 with its milestones placed for 90 degrees; the same caption without it; and the mirror (counter-clockwise).
    {"name": "a1-orb-quarter", "tool": T, "files": {"picture": "fixture:m-panel.png"},
     "fields": {"text": QUARTER_SWORD, "refs": ["swordswoman"], "first": 1, "lora": ORB, "duration": 5, "seed": 152101}},
    {"name": "a2-base-quarter", "tool": T, "files": {"picture": "fixture:m-panel.png"},
     "fields": {"text": QUARTER_SWORD, "refs": ["swordswoman"], "first": 1, "duration": 5, "seed": 152101}},
    # (b) two keyframes, m-panel to m-right: FL2VA without the LoRA; Ref2VA with and without ORB360.
    {"name": "b1-fl2va-two-keys", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-right2.png"},
     "fields": {"text": B1_TEXT, "first": 1, "last": 2, "duration": 5, "seed": 152102}},
    {"name": "b2-orb-two-keys", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-right2.png"},
     "fields": {"text": two_keys(), "refs": ["swordswoman", "right"], "first": 1, "last": 2, "lora": ORB, "duration": 5, "seed": 152103}},
    {"name": "b3-base-two-keys", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-right2.png"},
     "fields": {"text": two_keys(), "refs": ["swordswoman", "right"], "first": 1, "last": 2, "duration": 5, "seed": 152103}},
    {"name": "b4-fl2va-turned-subject", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-right.png"},
     "fields": {"text": B1_TEXT, "first": 1, "last": 2, "duration": 5, "seed": 152102}},
    # (c) vertical: H3 alone up over her and down below her; FL2VA up to m-top.
    {"name": "c1-base-up", "tool": T, "files": {"picture": "fixture:m-panel.png"},
     "fields": {"text": vertical(70), "refs": ["swordswoman"], "first": 1, "duration": 5, "seed": 152104}},
    {"name": "c2-base-down", "tool": T, "files": {"picture": "fixture:m-panel.png"},
     "fields": {"text": vertical(-40), "refs": ["swordswoman"], "first": 1, "duration": 5, "seed": 152105}},
    {"name": "c3-fl2va-up-two-keys", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-top.png"},
     "fields": {"text": fl2va(
         "The camera rises smoothly along a single vertical arc around the completely stationary swordswoman, from the first frame's "
         "eye-level viewpoint up to the last frame's high viewpoint looking steeply down at her, about 70 degrees of arc over the whole "
         "shot, at constant angular speed and constant radius, always looking at her and staying on the same side of her. The swordswoman "
         "and the whole street stay frozen in time and perfectly rigid. Black-and-white manga ink and screentone style throughout. No zoom, "
         "camera roll, cuts, lighting changes or scene changes."), "first": 1, "last": 2, "duration": 5, "seed": 152106}},
    # (d) a scene with no subject: H3 alone, a quarter round the room; ORB360, a whole turn.
    {"name": "d1-base-room-quarter", "tool": T, "files": {"picture": "fixture:s-classroom.png"},
     "fields": {"text": quarter("<classroom>", SCENE_ROOM, ROOM_WORDS, "the centre of <classroom>, always looking at the desks in the middle of the room"),
                "refs": ["classroom"], "first": 1, "duration": 5, "seed": 152108}},
    {"name": "a3-orb-quarter-ccw", "tool": T, "files": {"picture": "fixture:m-panel.png"}, "flip": True,
     "fields": {"text": QUARTER_SWORD, "refs": ["swordswoman"], "first": 1, "lora": ORB, "duration": 5, "seed": 152101}},
    # (e) character references on a whole ORB360 turn of the panel: none (orbit as it is), views at 90 and 180, her face.
    {"name": "e1-orb-turn", "tool": T, "files": {"picture": "fixture:m-panel.png"},
     "fields": {"text": full("<swordswoman>", SWORD, "in the environment shown in <Picture 1>",
                             "The shot begins from <Picture 1>, with its exact environment, ground, background and light.", STREET,
                             "the completely stationary <swordswoman>"),
                "refs": ["swordswoman"], "first": 1, "last": 1, "lora": ORB, "duration": 15, "seed": 152109}},
    # (f) backgrounds on the anime girl on white: orbit as it is, keyed back to white after.
    {"name": "f1-orb-white", "tool": T, "files": {"picture": "fixture:w-girl.png"}, "key": "white",
     "fields": {"text": plain("<girl>", "white", "the completely stationary <girl>"), "refs": ["girl"], "first": 1, "last": 1, "lora": ORB,
                "duration": 15, "seed": 152110}},
    {"name": "d2-orb-room-turn", "tool": T, "files": {"picture": "fixture:s-classroom.png"},
     "fields": {"text": full("<classroom>", SCENE_ROOM, "around the centre of the room shown in <Picture 1>",
                             "The shot begins from <Picture 1>, with its exact room and light.", ROOM_WORDS,
                             "the centre of <classroom>, always looking at the desks in the middle of the room").replace(KEPT, (
                                 "fully_preserved - its desks, chairs, windows, blackboard and walls are retained, and it stays completely frozen.")),
                "refs": ["classroom"], "first": 1, "last": 1, "lora": ORB, "duration": 15, "seed": 152107}},
    {"name": "e2-orb-turn-views", "tool": T,
     "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-right2.png", "picture3": "fixture:m-back.png"},
     "fields": {"text": full("<swordswoman>", SWORD.replace("<Picture 1>.", "<Picture 1>, <Picture 2> and <Picture 3>."),
                             "in the environment shown in <Picture 1>",
                             "The shot begins from <Picture 1>, with its exact environment, ground, background and light.", STREET,
                             "the completely stationary <swordswoman>",
                             extra_defs=["<right> shows <swordswoman> from her right side, 90 degrees clockwise round her, in <Picture 2>; it is not a frame of the video.",
                                         "<back> shows <swordswoman> from directly behind, 180 degrees round her, in <Picture 3>; it is not a frame of the video, and its street is not the scene."],
                             extra_keep=["<right> (not a frame of [Shot 1]; seen from 90 degrees clockwise of the starting direction): fully_preserved - its geometry, clothing and details define her right side; the orbit passes the same side.",
                                         "<back> (not a frame of [Shot 1]; seen from 180 degrees clockwise of the starting direction): fully_preserved - its geometry, clothing and details define her back; the orbit passes the same side."],
                             views={90: "her right side, as in <Picture 2>", 180: "her back, as in <Picture 3>"}),
                "refs": ["swordswoman", "right", "back"], "first": 1, "last": 1, "lora": ORB, "duration": 15, "seed": 152109}},
    {"name": "e3-orb-turn-face", "tool": T, "files": {"picture": "fixture:m-panel.png", "picture2": "fixture:m-face.png"},
     "fields": {"text": full("<swordswoman>", SWORD, "in the environment shown in <Picture 1>",
                             "The shot begins from <Picture 1>, with its exact environment, ground, background and light.", STREET,
                             "the completely stationary <swordswoman>",
                             extra_defs=["<face> is a close-up of <swordswoman>'s face, shown in <Picture 2>; it is not a frame of the video. "
                                         "Her face keeps exactly these features from every angle the camera sees it."],
                             extra_keep=["<face> (not a frame of [Shot 1]): fully_preserved - her facial features, hair and expression are "
                                         "retained from every angle."]),
                "refs": ["swordswoman", "face"], "first": 1, "last": 1, "lora": ORB, "duration": 15, "seed": 152109}},
    {"name": "f2-orb-new-scene", "tool": T, "files": {"picture": "fixture:w-girl.png"},
     "fields": {"text": full("<girl>", GIRL, "standing on a city rooftop at sunset",
                             "The shot begins facing <girl> from the same direction as <Picture 1>, but she stands on a city rooftop at "
                             "sunset: a concrete floor, a low railing, a water tank, the city skyline and an orange sky around her.",
                             "The rooftop and skyline stay in place in the world and are revealed consistently as the camera circles. "
                             "Everything is frozen in time.", "the completely stationary <girl>", pinned=False),
                "refs": ["girl"], "lora": ORB, "duration": 15, "aspect_ratio": "9:16", "seed": 152111}},
    {"name": "f3-orb-pictured-scene", "tool": T, "files": {"picture": "fixture:w-girl.png", "picture2": "fixture:s-classroom.png"},
     "fields": {"text": full("<girl>", GIRL, "standing in the middle of <classroom>",
                             "The shot begins facing <girl> from the same direction as <Picture 1>, but she stands in the middle of "
                             "<classroom> between the rows of desks, in its warm late-afternoon light.",
                             "The room stays in place in the world and is revealed consistently as the camera circles: the windows, the "
                             "blackboard, the door and the back wall pass behind her in turn. Nobody else is in the room. Everything is "
                             "frozen in time.", "the completely stationary <girl>", pinned=False,
                             extra_defs=["<classroom> is the room shown in <Picture 2>, where <girl> stands; its desks, windows, blackboard, "
                                         "walls and light remain unchanged."],
                             extra_keep=["<classroom> (not a frame of [Shot 1]): fully_preserved - the room's layout, furniture and light are "
                                         "retained around her."]),
                "refs": ["girl", "classroom"], "lora": ORB, "duration": 15, "aspect_ratio": "9:16", "seed": 152112}},
    {"name": "f4-orb-grey-keyed", "tool": T, "files": {"picture": "derive:w-grey"}, "key": "white",
     "fields": {"text": plain("<girl>", "plain", "the completely stationary <girl>"), "refs": ["girl"], "first": 1, "last": 1, "lora": ORB,
                "duration": 15, "seed": 152110}},
]
import sys

if sys.argv[1:] == ["resume"]:
    # The renders 44188990755 didn't reach (its pod ended at a render boundary on the 3-rental rule), and se-156's audit against
    # ORB360's recipe, each against e1 at its seed: no keyframe pin (the trained `[reference generation]` tag alone), `<Subject 1>`
    # as the trained captions name it, and the card's 124 frames.
    by = {r["name"]: r for r in renders}
    e1 = by["e1-orb-turn"]
    sword = full("<swordswoman>", SWORD, "in the environment shown in <Picture 1>",
                 "The shot begins from <Picture 1>, with its exact environment, ground, background and light.", STREET,
                 "the completely stationary <swordswoman>", seconds=5)
    # The coordinator's order: g1 first (it may fix the dark drift), then f2-f4, e2/e3, g2/g3, so a run cut short answers the main questions.
    renders = [
        {"name": "g1-orb-turn-unpinned", "tool": T, "files": e1["files"],
         "fields": {**{k: v for k, v in e1["fields"].items() if k not in ("first", "last")}, "aspect_ratio": "3:4"}},
    ] + [by[n] for n in ("f2-orb-new-scene", "f3-orb-pictured-scene", "f4-orb-grey-keyed", "e2-orb-turn-views", "e3-orb-turn-face")] + [
        {"name": "g2-orb-turn-subject-1", "tool": T, "files": e1["files"],
         "fields": {**e1["fields"], "text": e1["fields"]["text"].replace("<swordswoman>", "<Subject 1>"), "refs": ["Subject 1"]}},
        {"name": "g3-orb-turn-124", "tool": T, "files": e1["files"], "fields": {**e1["fields"], "text": sword, "duration": 5}},
    ]
print(json.dumps({"tree": "pk", "derive": [{"name": "w-grey", "op": "grey", "from": "fixture:w-girl.png"}], "renders": renders,
                  "seconds": 9000}, indent=1))
