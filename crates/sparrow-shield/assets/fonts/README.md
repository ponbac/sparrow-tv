# Saira instances for the Shield Guide

The static TrueType files come from `@fontsource-variable/saira` 5.3.0, which also supplies the Tauri Guide font. The source is `files/saira-latin-wdth-normal.woff2` with SHA-256 `d5f1ee1ce85a2f6611d76bcd98738132f4706b099dc167f02c2093a1ec5eb975`. The original license is included.

In `sparrow-guide-80-650.ttf`, 80 is the `wdth` axis and 650 is the `wght` axis. The same pattern applies to each file. Each width has a separate family name such as `Sparrow Guide 80` so GPUI can select the width and weight independently. The instances use Saira’s tabular digit glyphs by default. Platform fonts supply fallback glyphs for scripts outside the Latin source.

Saira's variable source keeps Å at its default-instance width and ring position for every width and weight. After creating instances, run `scripts/fonts/fix-saira-aring.py crates/sparrow-shield/assets/fonts/*.ttf`. It gives Å the width of its A and centres the ring.
