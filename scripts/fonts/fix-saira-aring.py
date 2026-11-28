#!/usr/bin/env -S uv run --no-project --with fonttools python3
"""Repair Å in the static Saira instances used by the Shield Guide.

Saira's variable source keeps Å (`Aring`) at its default-instance advance and
ring position for every width and weight, so in narrower instances Å is too wide
and its ring sits right of centre. This sets Å's advance and left side bearing to
those of its `A` component and centres the ring over `A`, as the default design
does. Running it again leaves already repaired fonts unchanged.

Usage: scripts/fonts/fix-saira-aring.py crates/sparrow-guide-ui/assets/fonts/*.ttf
"""

import sys

from fontTools.ttLib import TTFont


def centre(glyf, name):
    glyph = glyf[name]
    glyph.recalcBounds(glyf)
    return (glyph.xMin + glyph.xMax) / 2


def repair(path):
    font = TTFont(path)
    glyf, hmtx = font["glyf"], font["hmtx"]
    ring = next(c for c in glyf["Aring"].components if c.glyphName == "uni030A")
    shift = round(centre(glyf, "A") - (centre(glyf, "uni030A") + ring.x))
    if hmtx["Aring"] == hmtx["A"] and shift == 0:
        return False
    ring.x += shift
    hmtx["Aring"] = hmtx["A"]
    font.save(path)
    return True


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for path in sys.argv[1:]:
        print(("repaired " if repair(path) else "unchanged ") + path)
