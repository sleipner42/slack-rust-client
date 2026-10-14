#!/usr/bin/env python3
"""Pack Twemoji 72x72 PNGs into assets/emoji/twemoji-72.pack.

Usage: python3 scripts/build-emoji-pack.py <twemoji>/assets/72x72 assets/emoji/twemoji-72.pack

Format (little endian): b"TWEMOJI1", u32 count, then per entry sorted by name:
u8 name length, name (codepoints as in the file name), u32 offset, u32 length;
then the PNG data. Offsets are relative to the start of the PNG data.
"""

import struct
import sys
from pathlib import Path


def main(source: Path, target: Path) -> None:
    files = sorted(source.glob("*.png"))
    index = bytearray(b"TWEMOJI1" + struct.pack("<I", len(files)))
    data = bytearray()
    for path in files:
        name = path.stem.encode("ascii")
        png = path.read_bytes()
        index += struct.pack("<B", len(name)) + name + struct.pack("<II", len(data), len(png))
        data += png
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(bytes(index + data))
    print(f"{len(files)} emoji, {len(index) + len(data)} bytes -> {target}")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
