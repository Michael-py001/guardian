"""Generate the multi-resolution Windows icon for Guardian.

Run from anywhere: python assets/make_icon.py
Requires Pillow (only needed to regenerate the asset, not to build the app).
"""
from pathlib import Path

from PIL import Image, ImageDraw

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)


def P(x, y):
    return (int(x * S), int(y * S))


# Dark rounded-square background with a subtle hairline border.
d.rounded_rectangle(
    [P(0.03, 0.03), P(0.97, 0.97)],
    radius=int(S * 0.22),
    fill=(16, 21, 29, 255),
    outline=(45, 55, 72, 255),
    width=int(S * 0.012),
)

# Shield, same silhouette as ui.rs Icon::Shield.
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

base = img.resize((256, 256), Image.LANCZOS)
out = Path(__file__).with_name("guardian.ico")
base.save(out, format="ICO", sizes=[(16, 16), (24, 24), (32, 32),
                                     (48, 48), (64, 64), (128, 128), (256, 256)])
print(out)
