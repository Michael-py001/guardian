"""Generate the Guardian icon (transparent shield + checkmark).

Run from anywhere: python assets/make_icon.py
Requires Pillow (only needed to regenerate the asset, not to build the app).

Writes two files next to this script:
  - guardian.ico  multi-resolution Windows icon (exe / Explorer / taskbar / tray)
  - guardian.png  256px PNG embedded in the app for the titlebar logo
"""
from pathlib import Path

from PIL import Image, ImageDraw

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)


def P(x, y):
    return (int(x * S), int(y * S))


# Shield, same silhouette as ui.rs Icon::Shield. No dark plate or border —
# only the shield is kept, so it reads cleanly on any background.
shield = [P(0.5, 0.14), P(0.78, 0.27), P(0.78, 0.56),
          P(0.5, 0.84), P(0.22, 0.56), P(0.22, 0.27)]
d.polygon(shield, fill=(47, 111, 192, 255))
d.polygon([P(0.5, 0.14), P(0.78, 0.27), P(0.78, 0.42),
           P(0.22, 0.42), P(0.22, 0.27)], fill=(74, 143, 224, 255))
d.line(shield + [shield[0]], fill=(150, 195, 240, 255),
       width=int(S * 0.012), joint="curve")

# Checkmark.
d.line([P(0.37, 0.53), P(0.47, 0.64), P(0.66, 0.40)],
       fill=(255, 255, 255, 255), width=int(S * 0.055), joint="curve")

# Crop to the shield and scale it up to fill the icon. Windows draws the tray
# copy at 16 px (SM_CXSMICON), so every pixel of empty margin makes the logo
# read smaller and blurrier there; only a hair of padding is kept.
SHIELD_MARGIN = 0.03
body = img.crop(img.getchannel("A").getbbox())
fit = round(S * (1 - 2 * SHIELD_MARGIN))
scale = fit / max(body.width, body.height)
body = body.resize((round(body.width * scale), round(body.height * scale)),
                   Image.LANCZOS)
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
img.alpha_composite(body, ((S - body.width) // 2, (S - body.height) // 2))

base = img.resize((256, 256), Image.LANCZOS)
here = Path(__file__).parent
ico = here / "guardian.ico"
base.save(ico, format="ICO", sizes=[(16, 16), (24, 24), (32, 32),
                                     (48, 48), (64, 64), (128, 128), (256, 256)])
png = here / "guardian.png"
base.save(png, format="PNG")
print(ico)
print(png)
