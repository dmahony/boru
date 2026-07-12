#!/usr/bin/env python3
"""Generate representative test images for the chat image optimizer verification."""

import struct
import zlib
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

OUT = Path("/tmp/optimizer_test_images")
OUT.mkdir(parents=True, exist_ok=True)


def make_photo():
    """Simulate a photo: 4032x3024 (typical phone camera), smooth gradients + noise."""
    img = Image.new("RGB", (4032, 3024), (70, 120, 200))
    draw = ImageDraw.Draw(img)
    for y in range(0, 3024, 8):
        r = 70 + (y * 30 // 3024)
        g = 120 + (y * 20 // 3024)
        b = 200 - (y * 15 // 3024)
        draw.rectangle([(0, y), (4032, y + 8)], fill=(r, g, b))
    # Add some simulated content (a gradient circle)
    draw.ellipse([(1000, 500), (3000, 2500)], fill=(180, 90, 40), outline=(255, 255, 200), width=20)
    # Lens flare
    draw.ellipse([(500, 300), (800, 600)], fill=(255, 255, 200, 80))
    img.save(str(OUT / "photo_4032x3024.jpg"), quality=95)
    print(f"photo: {OUT / 'photo_4032x3024.jpg'} -> {img.fp.tell() if hasattr(img, 'fp') and img.fp else 'see below'}")
    img.save(str(OUT / "photo_4032x3024.png"))
    print(f"  also saved as PNG")


def make_screenshot():
    """Simulate a desktop screenshot: 1920x1080, flat colors, sharp text edges."""
    img = Image.new("RGB", (1920, 1080), (240, 240, 245))
    draw = ImageDraw.Draw(img)
    # Taskbar
    draw.rectangle([(0, 1000), (1920, 1080)], fill=(30, 30, 35))
    # Window
    draw.rectangle([(100, 50), (1500, 900)], fill=(255, 255, 255), outline=(0, 100, 200), width=3)
    # Title bar
    draw.rectangle([(100, 50), (1500, 100)], fill=(0, 100, 200))
    # Text lines
    for i, y in enumerate(range(130, 800, 60)):
        draw.rectangle([(130, y), (1300 + (i % 3) * 100, y + 30)], fill=(50, 50, 55))
    img.save(str(OUT / "screenshot_1920x1080.jpg"), quality=95)
    img.save(str(OUT / "screenshot_1920x1080.png"))


def make_transparent():
    """PNG with alpha transparency: circular gradient on transparent bg."""
    w, h = 1200, 1200
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    # Semi-transparent circle
    draw.ellipse([(100, 100), (1100, 1100)], fill=(255, 80, 50, 180))
    # Fully transparent center
    draw.ellipse([(400, 400), (800, 800)], fill=(255, 80, 50, 40))
    # Another partially transparent shape
    draw.ellipse([(600, 200), (1000, 600)], fill=(50, 200, 100, 120))
    img.save(str(OUT / "transparent_1200.png"))
    # Also save as JPEG (should be rejected by optimizer since input PNG but we test the flow)
    # Actually the optimizer accepts PNG and JPEG inputs, outputs JPEG


def make_rotated():
    """PNG with EXIF rotation tag to test auto-orientation."""
    # The Python PIL doesn't embed EXIF rotation easily in JPEGs without pillow >9,
    # but we can create a raw rotated JPEG with EXIF orientation tag.
    # Test 1: physically rotated pixel data
    img = Image.new("RGB", (800, 1200), (100, 150, 200))
    draw = ImageDraw.Draw(img)
    draw.rectangle([(100, 300), (700, 700)], fill=(200, 50, 50))
    draw.text((300, 600), "ROTATED", fill=(255, 255, 255))
    img.save(str(OUT / "rotated_800x1200.jpg"), quality=90)
    # Test 2: image with EXIF orientation flag set
    # Use raw EXIF with orientation tag
    exif_data = Image.Exif()
    exif_data[0x0112] = 6  # Rotate 90 CW
    img = Image.new("RGB", (1200, 800), (100, 150, 200))
    draw = ImageDraw.Draw(img)
    draw.text((500, 400), "EXIF rotated", fill=(255, 255, 255))
    img.save(str(OUT / "rotated_exif_1200x800.jpg"), quality=90, exif=exif_data.tobytes())
    print("  saved exif-rotated jpeg")


def make_already_compressed():
    """Small, already heavily compressed JPEG that's already under 2 MiB."""
    img = Image.new("RGB", (640, 480), (200, 220, 240))
    draw = ImageDraw.Draw(img)
    for i in range(20):
        x1, y1 = i * 30, i * 20
        x2, y2 = x1 + 50, y1 + 40
        draw.rectangle([(x1, y1), (x2, y2)], fill=(i * 10, i * 8, i * 5))
    img.save(str(OUT / "already_compressed_640.jpg"), quality=30)
    img.save(str(OUT / "already_compressed_640.png"))


def make_unsupported_formats():
    """GIF, BMP, WebP - formats the image crate may or may not handle."""
    # BMP should decode fine
    img = Image.new("RGB", (100, 100), (255, 0, 0))
    img.save(str(OUT / "unsupported_100x100.bmp"))

    # Create a minimal GIF (should be unsupported by the optimizer since it only accepts JPEG/PNG)
    frames = []
    for i in range(3):
        frame = Image.new("RGB", (50, 50), (i * 80, 50, 200 - i * 50))
        frames.append(frame)
    frames[0].save(str(OUT / "unsupported_animated.gif"), save_all=True, append_images=frames[1:], loop=0, duration=500)

    # Single-frame GIF
    Image.new("RGB", (100, 100), (0, 200, 0)).save(str(OUT / "unsupported_single.gif"))

    # Minimal ICO
    img = Image.new("RGBA", (32, 32), (100, 200, 255, 200))
    img.save(str(OUT / "unsupported_32x32.ico"))


def make_oversized():
    """Image larger than 10 MiB to test the input-size rejection."""
    # 8000x6000 at quality 95 should be well over 10 MiB
    img = Image.new("RGB", (8000, 6000), (100, 100, 100))
    draw = ImageDraw.Draw(img)
    for x in range(0, 8000, 100):
        for y in range(0, 6000, 100):
            draw.rectangle([(x, y), (x + 50, y + 50)], fill=(x % 256, y % 256, (x + y) % 256))
    path = str(OUT / "oversized_8000x6000.jpg")
    img.save(path, quality=95)
    size = Path(path).stat().st_size
    print(f"  oversized JPEG: {size / 1024 / 1024:.1f} MiB")


def make_tiny():
    """Tiny 1x1 JPEG and PNG - edge cases."""
    Image.new("RGB", (1, 1), (128, 128, 128)).save(str(OUT / "tiny_1x1.jpg"), quality=95)
    Image.new("RGB", (1, 1), (128, 128, 128)).save(str(OUT / "tiny_1x1.png"))
    # Fully transparent 1x1
    Image.new("RGBA", (1, 1), (0, 0, 0, 0)).save(str(OUT / "tiny_transparent_1x1.png"))


def make_photo_10mp():
    """High-res photo near the 10 MiB boundary."""
    # 5472x3648 is roughly 20MP - with quality 85 it should be near or over 10 MiB
    img = Image.new("RGB", (5472, 3648), (50, 100, 150))
    draw = ImageDraw.Draw(img)
    for y in range(0, 3648, 50):
        r = 50 + (y * 200 // 3648)
        g = 100 + (y * 100 // 3648)
        b = 150 - (y * 50 // 3648)
        draw.rectangle([(0, y), (5472, y + 25)], fill=(r, g, b))
    path = str(OUT / "photo_20mp_5472x3648.jpg")
    img.save(path, quality=85)
    size = Path(path).stat().st_size
    print(f"  20MP photo: {size / 1024 / 1024:.1f} MiB")


if __name__ == "__main__":
    make_photo()
    make_screenshot()
    make_transparent()
    make_rotated()
    make_already_compressed()
    make_unsupported_formats()
    make_oversized()
    make_tiny()
    make_photo_10mp()

    print("\n=== All test images generated ===")
    for f in sorted(OUT.iterdir()):
        size = f.stat().st_size
        print(f"  {f.name:45s} {size:>8,} bytes ({size/1024:.1f} KiB)")
