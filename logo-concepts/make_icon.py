"""Build the Sonic Launcher app icon from the user's PS logo image.

Source: the user's uploaded image (white P+S monogram on blue gradient).
Writes Tauri icon set into src-tauri/icons/ (backing up the old ones first).
"""
from PIL import Image
import os
import shutil

SRC = "/home/hatch/workspace/user/files/18434_0_rcch.jpg"
ICONS = "/home/hatch/workspace/sonic-launcher/src-tauri/icons"
BACKUP = "/home/hatch/workspace/sonic-launcher/logo-concepts/old-icons-backup"

os.makedirs(BACKUP, exist_ok=True)
for f in os.listdir(ICONS):
    shutil.copy2(os.path.join(ICONS, f), os.path.join(BACKUP, f))
print("backed up old icons to", BACKUP)

im = Image.open(SRC).convert("RGBA")
w, h = im.size
print("source:", w, "x", h)
if w != h:
    s = min(w, h)
    im = im.crop(((w - s) // 2, (h - s) // 2, (w + s) // 2, (h + s) // 2))
    print("center-cropped to square:", im.size)


def save_png(size, name):
    out = os.path.join(ICONS, name)
    im.resize((size, size), Image.LANCZOS).save(out, "PNG")
    print("wrote", name, size, "x", size)


save_png(512, "icon.png")
save_png(256, "128x128@2x.png")
save_png(128, "128x128.png")
save_png(32, "32x32.png")
im.save(
    os.path.join(ICONS, "icon.ico"),
    "ICO",
    sizes=[(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (16, 16)],
)
print("wrote icon.ico (256/128/64/48/32/16)")
print("done")
