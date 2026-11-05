# h3-joins' two characters, drawn flat on white so nothing about them is a photo's: draw.py <dir> writes pip.png and tova.png.
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = (768, 1024)


def pip(draw: ImageDraw.ImageDraw) -> None:
    """A round orange robot with one big blue eye, an antenna and grey legs."""
    grey, dark = (130, 135, 145), (90, 45, 15)
    draw.line([(384, 340), (384, 210)], fill=grey, width=12)
    draw.ellipse([356, 170, 412, 226], fill=(220, 40, 40), outline=dark, width=4)
    for x in (300, 428):
        draw.rectangle([x, 760, x + 40, 900], fill=grey)
        draw.ellipse([x - 25, 885, x + 65, 935], fill=(70, 70, 80))
    draw.line([(170, 560), (90, 470)], fill=grey, width=18)
    draw.line([(598, 560), (678, 470)], fill=grey, width=18)
    draw.ellipse([154, 330, 614, 790], fill=(240, 140, 40), outline=dark, width=8)
    draw.ellipse([294, 410, 474, 590], fill="white", outline=dark, width=6)
    draw.ellipse([339, 455, 429, 545], fill=(40, 90, 220))
    draw.ellipse([366, 482, 402, 518], fill="black")
    draw.ellipse([392, 470, 408, 486], fill="white")
    draw.arc([324, 610, 444, 690], 20, 160, fill=dark, width=8)


def tova(draw: ImageDraw.ImageDraw) -> None:
    """A tall teal penguin-like bird with a white belly, a yellow beak and a red scarf."""
    teal, dark = (30, 150, 150), (15, 70, 70)
    for box in ([280, 880, 370, 940], [398, 880, 488, 940]):
        draw.ellipse(box, fill=(250, 150, 30))
    draw.ellipse([176, 470, 276, 790], fill=teal, outline=dark, width=6)
    draw.ellipse([492, 470, 592, 790], fill=teal, outline=dark, width=6)
    draw.ellipse([234, 230, 534, 910], fill=teal, outline=dark, width=8)
    draw.ellipse([294, 450, 474, 870], fill="white")
    for x in (334, 434):
        draw.ellipse([x - 20, 330, x + 20, 370], fill="black")
        draw.ellipse([x - 8, 336, x + 4, 348], fill="white")
    draw.polygon([(354, 390), (414, 390), (384, 440)], fill=(250, 200, 30), outline=dark)
    draw.rectangle([244, 460, 524, 505], fill=(210, 40, 50))
    draw.polygon([(450, 500), (500, 500), (520, 640), (470, 640)], fill=(210, 40, 50))


out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
for name, paint in (("pip", pip), ("tova", tova)):
    image = Image.new("RGB", SIZE, "white")
    paint(ImageDraw.Draw(image))
    image.save(out / f"{name}.png")
