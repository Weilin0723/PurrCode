# PurrCode brand assets

Everything in this directory is derived from `purrcode-logo-source.png`, the
authoritative mascot and wordmark supplied with the v1.0 PRD (§5.2, §25). The
mascot is never redrawn. Regenerate the whole tree with:

```bash
python3 scripts/brand/build_brand_assets.py
```

The script is deterministic, so a re-run reproduces the tree byte for byte.

| File | What it is | How it is derived |
| --- | --- | --- |
| `purrcode-logo-source.png` | Supplied source. Do not edit. | — |
| `purrcode-logo-horizontal-dark.png` | Full lockup for dark surfaces | Mascot + wordmark cut from the source, white paper turned to transparency |
| `purrcode-logo-horizontal-light.png` | Full lockup for light surfaces | As above, with `Purr` repainted slate so it does not vanish on white |
| `purrcode-mascot-large.png` | 1024px mascot | Source mascot, trimmed and centred on a transparent square |
| `purrcode-wordmark-dark.svg` / `.png` | `Purr` white, `Code` blue | Traced from the source glyph masks |
| `purrcode-wordmark-light.svg` / `.png` | `Purr` slate, `Code` blue | Same trace, different fill |
| `purrcode-cat-head.svg` | Colour icon for the Activity Bar and avatars | Mascot posterised to six brand colours, each region traced |
| `purrcode-cat-head-monochrome.svg` | Single-colour icon (`currentColor`) | Mascot silhouette with the eyes knocked out |
| `icons/16…512.png` | App, marketplace and OS icons | Source mascot resampled; no wordmark, no canvas |

## Why the wordmark needed reconstructing

In the source, `Purr` is white type on white paper: only its contour carries
ink, and the letterforms themselves are the background. Shipping that crop
would give a wordmark that disappears on white and shows as a hairline outline
on black. The build classifies each enclosed region by how many contours
separate it from the page — one is the letter body, two is a counter that has
to stay open — then fills the bodies and repaints them per variant. `Code` is
kept pixel for pixel; nothing about it is regenerated.

## Review gate

PRD §21.2 forbids auto-tracing and shipping without review. The build refuses to
write a wordmark whose traced outline scores below 0.97 IoU against the glyph
mask it came from, and today both words trace at 1.0000 — the vector is the
drawing, not an approximation of it.

The cat-head SVGs are deliberately *not* held to that bar: they are the
simplified derivatives §21.2 asks for, because fur strands and the collar tag's
inner detail cannot survive 16px. They keep what identifies the cat — the
silhouette with its ears, the ragdoll mask, the blue eyes, the purple collar —
and were reviewed rendered at 16, 24, 32, 48 and 64px.

## Usage

- **Full logo**: README, onboarding, About, extension marketplace, docs, release art.
- **Cat head**: VS Code Activity Bar, session avatar, status bar, app icon.
- **Monochrome cat head**: 16–24px UI, high-contrast surfaces, terminal docs.
- **TUI**: the `PurrCode` wordmark as text. No raster art, no emoji dependency.
